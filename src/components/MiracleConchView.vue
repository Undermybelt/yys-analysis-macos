<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { desktopApi } from "../api/client";
import { normalizeCommandError } from "../api/errors";
import { filterShikigami, isMaterialShikigamiRarity } from "../shikigamiCatalog";
import SoulSetPicker from "./SoulSetPicker.vue";
import CharacterArchiveSwitcher from "./CharacterArchiveSwitcher.vue";
import { onActiveCharacterChanged } from "../state/activeCharacter";
import {
  getMiracleBasePanelItems,
  getMiracleTargetInputFromPanel,
  parseOptionalMetricMaximum,
} from "../miracleConchPresentation";
import type {
  CatalogStatus,
  MiracleConchResult,
  MiracleMetric,
  MiracleMetricConstraint,
  MiracleMetricResult,
  MiracleMainAttributeConstraint,
  MiraclePanel,
  MiracleRecipeLine,
  MiracleSolution,
  MiracleSoulView,
  MiracleTarget,
  Shikigami,
  ShikigamiRarity,
  MySoulScore,
  ScoreColorThresholds,
  SoulSet,
} from "../api/contracts";

/** 神奇海螺只使用后端当前活动目录；目录更新后无需重新编译页面即可参与配装计算。 */
const EXPECTED_SET_COUNT = 70;
const SCATTERED_SET_ID = "__scattered__";
/** 分类已选但具体御魂留空时的稳定占位值；后端会按 category 匹配该分类下任意套装。 */
const UNRESTRICTED_SET_ID = "__unrestricted__";
/** 神奇海螺页面配置只保存在当前 WebView 的本地存储，不写入御魂库存或分析数据库。 */
const MIRACLE_CONFIG_STORAGE_KEY = "yys-analysis.miracle-conch.config.v1";

type TargetField = keyof MiracleTarget;
type TargetInput = Record<TargetField, string>;
type TargetMode = "panel" | "metric";
/** 数字输入框编辑后由 Vue 产出 number，初始化或配置恢复时仍可能是 string。 */
type NumericInput = string | number;
type MetricConstraintInput = Omit<MiracleMetricConstraint, "minimum" | "maximum"> & {
  minimum: NumericInput;
  maximum: NumericInput;
};
type MetricDraft = Omit<MiracleMetric, "threshold" | "constraints"> & {
  threshold: NumericInput;
  constraints: MetricConstraintInput[];
};
type EnhancementCandidate = MiracleConchResult["enhancementCandidates"][number];
type RecipeDraft = MiracleRecipeLine & {
  /** 仅用于界面两级筛选，不写入后端配方契约。 */
  category: string;
};
type MainAttributeDraft = {
  slot: 2 | 4 | 6;
  mainAttrTypes: string[];
};
type MiracleConchSavedConfiguration = {
  version: 1;
  /** 用于在 Rust 文件与 WebView 本地存储同时存在时选择最新的一份配置。 */
  updatedAt: number;
  selectedShikigamiId: string;
  selectedShikigamiRarity: ShikigamiRarity | "ALL";
  targetMode: TargetMode;
  target: TargetInput;
  onlyMaxLevel: boolean;
  metrics: MetricDraft[];
  mainAttributes: MainAttributeDraft[];
  recipe: RecipeDraft[];
};

const targetDefinitions: readonly {
  key: TargetField;
  label: string;
  unit: string;
  percent: boolean;
}[] = [
  { key: "attack", label: "攻击", unit: "点", percent: false },
  { key: "hp", label: "生命", unit: "点", percent: false },
  { key: "defense", label: "防御", unit: "点", percent: false },
  { key: "speed", label: "速度", unit: "点", percent: false },
  { key: "critRate", label: "暴击", unit: "%", percent: true },
  { key: "critDamage", label: "暴伤", unit: "%", percent: true },
  { key: "effectHit", label: "效果命中", unit: "%", percent: true },
  { key: "effectResist", label: "效果抵抗", unit: "%", percent: true },
];

const shikigami = ref<Shikigami[]>([]);
const selectedShikigamiId = ref("389");
/** 式神选择先按稀有度缩小范围；默认跟随神奇海螺原来的须佐之男（SSR）。 */
const selectedShikigamiRarity = ref<ShikigamiRarity | "ALL">("SSR");
/** 稀有度顺序与式神图鉴保持一致，避免两个页面的筛选认知不一致。 */
const shikigamiRarityOptions: readonly ShikigamiRarity[] = [
  "UR",
  "SP",
  "SSR",
  "SR",
  "R",
  "N",
];
/** 控制带头像的式神候选面板；切换式神后立即收起，避免遮挡目标指标。 */
const isShikigamiPickerOpen = ref(false);
/** 式神面板内的名字搜索词；只在面板打开时参与过滤，关闭面板后自动清空。 */
const shikigamiQuery = ref("");
/** 用于处理点击选择器外部关闭面板，同时不影响页面其他控件。 */
const shikigamiPickerRef = ref<HTMLElement | null>(null);
/** 打开面板后自动聚焦名字搜索框，让“先选式神”直接进入输入状态。 */
const shikigamiSearchRef = ref<HTMLInputElement | null>(null);
const sets = ref<SoulSet[]>([]);
const catalogStatus = ref<CatalogStatus | null>(null);
/** 创建空目标，避免组合指标模式残留式神面板目标并进入后端搜索。 */
const createEmptyTarget = (): TargetInput => ({
  attack: "",
  hp: "",
  defense: "",
  speed: "",
  critRate: "",
  critDamage: "",
  effectHit: "",
  effectResist: "",
});
const target = ref<TargetInput>(createEmptyTarget());
/** 默认采用组合指标，更符合目标驱动配装；用户仍可切换到面板最低值。 */
const targetMode = ref<TargetMode>("metric");
/** 默认只计算六星 +15 御魂；关闭后才把六星未满级御魂纳入候选。 */
const onlyMaxLevel = ref(true);
const metricPresets = [
  {
    id: "damage_output",
    label: "伤害输出",
    metricType: "product" as const,
    operands: ["attack", "crit_damage"],
    percentage: false,
    threshold: "30000",
    constraints: [{ attribute: "crit_rate", minimum: "100" }],
  },
  {
    id: "effect_hit",
    label: "效果命中",
    metricType: "minimum" as const,
    operands: ["effect_hit"],
    percentage: true,
    threshold: "100",
    constraints: [],
  },
  {
    id: "effect_resist",
    label: "效果抵抗",
    metricType: "minimum" as const,
    operands: ["effect_resist"],
    percentage: true,
    threshold: "100",
    constraints: [],
  },
  {
    id: "hp",
    label: "生命",
    metricType: "minimum" as const,
    operands: ["hp"],
    percentage: false,
    threshold: "30000",
    constraints: [],
  },
  {
    id: "crit_damage",
    label: "暴击伤害",
    metricType: "minimum" as const,
    operands: ["crit_damage"],
    percentage: true,
    threshold: "250",
    constraints: [{ attribute: "crit_rate", minimum: "100" }],
  },
  {
    id: "healing",
    label: "治疗量",
    metricType: "product" as const,
    operands: ["hp", "crit_damage"],
    percentage: false,
    threshold: "50000",
    constraints: [],
  },
  {
    id: "dual_control",
    label: "命抗双修",
    metricType: "product" as const,
    operands: ["effect_hit", "effect_resist"],
    percentage: false,
    threshold: "0.64",
    constraints: [],
  },
  {
    id: "defense_output",
    label: "防御输出",
    metricType: "product" as const,
    operands: ["defense", "crit_damage"],
    percentage: false,
    threshold: "12000",
    constraints: [],
  },
  {
    id: "attack",
    label: "攻击",
    metricType: "minimum" as const,
    operands: ["attack"],
    percentage: false,
    threshold: "20000",
    constraints: [],
  },
  {
    id: "defense",
    label: "防御",
    metricType: "minimum" as const,
    operands: ["defense"],
    percentage: false,
    threshold: "3000",
    constraints: [],
  },
  {
    id: "speed",
    label: "速度",
    metricType: "minimum" as const,
    operands: ["speed"],
    percentage: false,
    threshold: "250",
    constraints: [],
  },
  {
    id: "crit_rate",
    label: "暴击",
    metricType: "minimum" as const,
    operands: ["crit_rate"],
    percentage: true,
    threshold: "100",
    constraints: [],
  },
] as const;
const mainAttributeOptions: Record<number, { value: string; label: string }[]> = {
  2: [
    { value: "attack_rate", label: "攻击加成" },
    { value: "hp_rate", label: "生命加成" },
    { value: "defense_rate", label: "防御加成" },
    { value: "speed", label: "速度" },
  ],
  4: [
    { value: "attack_rate", label: "攻击加成" },
    { value: "hp_rate", label: "生命加成" },
    { value: "defense_rate", label: "防御加成" },
    { value: "effect_hit", label: "效果命中" },
    { value: "effect_resist", label: "效果抵抗" },
  ],
  6: [
    { value: "attack_rate", label: "攻击加成" },
    { value: "hp_rate", label: "生命加成" },
    { value: "defense_rate", label: "防御加成" },
    { value: "crit_rate", label: "暴击" },
    { value: "crit_damage", label: "暴伤" },
  ],
};
const metricOperandsLabel: Record<string, string> = {
  attack: "攻击",
  hp: "生命",
  defense: "防御",
  speed: "速度",
  crit_rate: "暴击",
  crit_damage: "暴伤",
  effect_hit: "效果命中",
  effect_resist: "效果抵抗",
};
const createMetricDraft = (presetId = "damage_output"): MetricDraft => {
  const preset = metricPresets.find((item) => item.id === presetId) ?? metricPresets[0];
  return {
    presetId: preset.id,
    metricType: preset.metricType,
    label: preset.label,
    operands: [...preset.operands],
    percentage: preset.percentage,
    threshold: preset.threshold,
    constraints: preset.constraints.map((constraint) => ({
      ...constraint,
      maximum: "",
    })),
  };
};
const metrics = ref<MetricDraft[]>([createMetricDraft()]);
/** 号位主属性的界面草稿允许多选；发送请求时再展开成多个“号位 + 主属性”约束。 */
const defaultMainAttributes = (): MainAttributeDraft[] => [
  { slot: 2, mainAttrTypes: ["speed"] },
  { slot: 4, mainAttrTypes: ["attack_rate"] },
  { slot: 6, mainAttrTypes: ["crit_damage"] },
];
const mainAttributes = ref<MainAttributeDraft[]>(defaultMainAttributes());

/** 重置号位主属性到目标驱动配装的通用默认：2号位速度，4/6号位输出向主属性。 */
function resetMainAttributes(): void {
  mainAttributes.value = defaultMainAttributes();
}
/** 配方行在界面中保留“分类 + 具体套装”，请求发送时携带套装、件数和分类约束。 */
const recipe = ref<RecipeDraft[]>([
  { setId: SCATTERED_SET_ID, count: 6, category: SCATTERED_SET_ID },
]);
let stopActiveCharacterChanged: (() => void) | null = null;
const result = ref<MiracleConchResult | null>(null);
/** 复用“御魂分析”已保存的标准评分；神奇海螺不在前端重复实现评分规则。 */
const scoreBySoulKey = ref(new Map<string, MySoulScore>());
/** 评分读取请求序号；新的配装结果产生后，旧结果不能回写当前页面。 */
let solutionScoreLoadGeneration = 0;
/** 与“御魂分析”共用当前启用标准的颜色阈值；读取失败时使用兼容旧标准的默认值。 */
const scoreColorThresholds = ref<ScoreColorThresholds>({
  jadeScore: 60,
  goldScore: 70,
  rainbowScore: 75,
});
/** 正式页面展示后端排序后的前十枚胚子，方便比较期望收益和理论上限。 */
const displayedEnhancementCandidates = computed(
  () => result.value?.enhancementCandidates.slice(0, 10) ?? [],
);
const loading = ref(true);
const calculating = ref(false);
/** 计算遮罩的估算进度；后端命令是一次性调用，因此进度只用于反馈当前计算阶段。 */
const calculationProgress = ref(0);
/** 计算遮罩当前阶段文案，帮助玩家理解等待期间正在处理什么。 */
const calculationStage = ref("准备读取当前库存");
const errorMessage = ref("");
const validationMessage = ref("");

/** 神奇海螺搜索阶段的保守上限；未收到后端结果前不会伪装成已经完成。 */
const calculationStages = [
  { label: "正在读取当前库存", ceiling: 24 },
  { label: "正在筛选六星御魂", ceiling: 46 },
  { label: "正在组合六个位置", ceiling: 70 },
  { label: "正在比较最接近方案", ceiling: 86 },
  // 后端命令返回前没有可推送的真实百分比；94% 只表示正在等待最终结果。
  { label: "等待后端计算返回", ceiling: 94 },
] as const;
let calculationStageIndex = 0;
/** 仅用于推进遮罩的视觉反馈；组件销毁时必须清理，避免后台定时器继续运行。 */
let calculationTimer: ReturnType<typeof setInterval> | null = null;
/** 目录加载并完成配置恢复前不写入默认值，避免初始化过程覆盖用户上次保存的配置。 */
let configurationHydrated = false;
/** 保存请求串行执行并只保留最新快照，避免用户连续输入时旧请求晚到而覆盖新值。 */
let configurationSaveInFlight = false;
let queuedConfigurationPayload: string | null = null;

const selectedShikigami = computed(
  () =>
    shikigami.value.find((entry) => entry.shikigamiId === selectedShikigamiId.value) ??
    null,
);
/** 只展示当前稀有度且名字模糊命中的式神，保证用户完成“先选稀有度、再选式神”的两级选择。 */
const selectableShikigami = computed(() =>
  filterShikigami(
    // 神奇海螺需要可计算的基础面板，素材条目只用于图鉴与式神录，不能作为计算目标。
    shikigami.value.filter((entry) => !isMaterialShikigamiRarity(entry.rarity)),
    shikigamiQuery.value,
    "ALL",
    selectedShikigamiRarity.value,
  ),
);
const selectedBasePanelItems = computed(() =>
  selectedShikigami.value
    ? getMiracleBasePanelItems(selectedShikigami.value.panel)
    : [],
);
const setNameById = computed(
  () => new Map(sets.value.map((set) => [set.setId, set.name])),
);
/** 用户明确指定的件数；不足六件的剩余位置会在请求中自动补为不限制套装。 */
const recipeSpecifiedTotal = computed(() =>
  recipe.value.reduce((total, line) => total + Number(line.count || 0), 0),
);
/** 界面显示已指定件数；指定不满六件时，旁边会标出由任意御魂补足的剩余位置。 */
const recipeTotal = computed(() => Math.max(0, recipeSpecifiedTotal.value));
/** 供界面解释“4件套 + 2件不限”的隐式剩余配方。 */
const recipeUnrestrictedCount = computed(() =>
  Math.max(0, 6 - recipeSpecifiedTotal.value),
);
const recipeHasDuplicate = computed(() => {
  const ids = recipe.value
    .filter((line) => line.setId !== SCATTERED_SET_ID)
    .map((line) =>
      line.setId === UNRESTRICTED_SET_ID
        ? `${UNRESTRICTED_SET_ID}:${line.category}`
        : line.setId,
    );
  return new Set(ids).size !== ids.length;
});
const hasTarget = computed(() =>
  Object.values(target.value).some((value) => value.trim() !== ""),
);
/** 未达标时只展示排序第一的最接近方案，避免把玩家注意力分散到多个未完成方案。 */
const displayedSolutions = computed(() => {
  if (!result.value) return [];
  return result.value.status === "closest"
    ? result.value.solutions.slice(0, 1)
    : result.value.solutions;
});
const metricsValid = computed(
  () =>
    metrics.value.length > 0 &&
    metrics.value.every(
      (metric) =>
        metric.label.trim() !== "" &&
        metric.operands.length >= 1 &&
        new Set(metric.operands).size === metric.operands.length &&
        Number(metric.threshold) > 0 &&
        metric.constraints.every((constraint) => {
          const minimum = Number(constraint.minimum);
          // 同时接受数字输入框产生的 number 与旧配置恢复的 string，空值才表示不限上限。
          const maximum = parseOptionalMetricMaximum(constraint.maximum);
          return (
            Boolean(constraint.attribute) &&
            Number.isFinite(minimum) &&
            minimum >= 0 &&
            (maximum === null || (Number.isFinite(maximum) && maximum >= minimum))
          );
        }),
    ),
);

/** 组合指标的展示公式；百分比字段仍由后端按小数计算。 */
const metricExpression = (metric: MetricDraft): string =>
  metric.operands
    .map((operand) => metricOperandsLabel[operand] ?? operand)
    .join(metric.metricType === "product" ? " × " : " 取最低 ");

/** 当前配方中首领/星痕和普通套装的候选目录，避免让玩家在一个下拉中翻完整目录。 */
const specialSets = computed(() =>
  sets.value.filter(
    (set) => set.specialCategory === "首领御魂" || set.specialCategory === "星痕御魂",
  ),
);
const ordinarySets = computed(() => sets.value.filter((set) => !set.specialCategory));

/** 把目录的套装属性分类统一成两级选择器使用的分类键。 */
function setCategory(set: SoulSet): string {
  return set.specialCategory ?? set.category ?? "其他";
}

/** 套装分类按目录动态生成，散件和常用属性方向固定排在前面。 */
const recipeCategories = computed(() => {
  const categories = new Set<string>([SCATTERED_SET_ID]);
  for (const set of sets.value) categories.add(setCategory(set));
  const priority = [
    SCATTERED_SET_ID,
    "暴击",
    "攻击加成",
    "生命加成",
    "防御加成",
    "速度",
    "效果命中",
    "效果抵抗",
    "首领御魂",
    "星痕御魂",
  ];
  return [...categories].sort((left, right) => {
    const leftIndex = priority.indexOf(left);
    const rightIndex = priority.indexOf(right);
    return (
      (leftIndex < 0 ? priority.length : leftIndex) -
        (rightIndex < 0 ? priority.length : rightIndex) ||
      left.localeCompare(right, "zh-CN")
    );
  });
});

/** 根据一级分类返回具体套装，散件分类不对应目录套装。 */
function setsForCategory(category: string): SoulSet[] {
  if (category === SCATTERED_SET_ID) return [];
  return sets.value.filter((set) => setCategory(set) === category);
}

/** 创建配方界面行，并根据具体套装自动带出一级分类；空具体套装默认表示该分类全部可用。 */
function createRecipeDraft(setId: string, count: number): RecipeDraft {
  const selected = sets.value.find((set) => set.setId === setId);
  return {
    setId,
    count,
    category: selected
      ? setCategory(selected)
      : setId === SCATTERED_SET_ID
        ? SCATTERED_SET_ID
        : "",
  };
}

/** 分类切换后默认不指定具体御魂；用户不再需要从长列表中任选一套才能开始搜索。 */
function handleRecipeCategoryChange(index: number): void {
  const line = recipe.value[index];
  if (!line) return;
  const available = setsForCategory(line.category);
  line.setId =
    line.category === SCATTERED_SET_ID
      ? SCATTERED_SET_ID
      : available.length > 0
        ? UNRESTRICTED_SET_ID
        : SCATTERED_SET_ID;
}

/** 具体套装切换后回写一级分类；保留分类通配占位值，不把“未选择”误判成散件。 */
function handleRecipeSetChange(index: number): void {
  const line = recipe.value[index];
  if (!line || line.setId === SCATTERED_SET_ID || line.setId === UNRESTRICTED_SET_ID)
    return;
  const selected = sets.value.find((set) => set.setId === line.setId);
  if (selected) line.category = setCategory(selected);
}

/** 套装分类键转成玩家看到的标题。 */
function recipeCategoryLabel(category: string): string {
  const labels: Record<string, string> = {
    [SCATTERED_SET_ID]: "散件",
    暴击: "暴击套",
    攻击加成: "攻击套",
    生命加成: "生命套",
    防御加成: "防御套",
    效果命中: "命中套",
    效果抵抗: "抵抗套",
  };
  return labels[category] ?? category;
}

/** 把目录默认的特殊套装和普通套装转换成 2+4 的初始配方。 */
function initializeDefaultRecipe(): void {
  const special =
    specialSets.value.find((set) => set.setId === "鬼灵歌伎") ??
    specialSets.value.find((set) => set.specialCategory === "首领御魂") ??
    specialSets.value[0];
  const ordinary =
    ordinarySets.value.find((set) => set.setId === "片叶之苇") ?? ordinarySets.value[0];
  recipe.value = [
    createRecipeDraft(ordinary?.setId ?? SCATTERED_SET_ID, 4),
    createRecipeDraft(special?.setId ?? SCATTERED_SET_ID, 2),
  ];
}

/** 判断本地存储解析结果是否为普通对象，避免损坏配置触发页面初始化异常。 */
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** 判断持久化字段是否为可恢复的数字输入；恢复后统一转回字符串供表单继续编辑。 */
function isNumericInput(value: unknown): value is NumericInput {
  return typeof value === "string" || (typeof value === "number" && Number.isFinite(value));
}

/** 从本地存储恢复指标草稿；旧配置没有最高值时按“不限上限”兼容恢复。 */
function restoreMetrics(value: unknown): MetricDraft[] | null {
  if (!Array.isArray(value)) return null;
  const restored = value.flatMap((item): MetricDraft[] => {
    if (!isRecord(item) || typeof item.presetId !== "string") return [];
    const preset = metricPresets.find((candidate) => candidate.id === item.presetId);
    if (!preset || typeof item.label !== "string" || !isNumericInput(item.threshold))
      return [];
    const operands = Array.isArray(item.operands)
      ? item.operands.filter(
          (operand): operand is string => typeof operand === "string",
        )
      : [];
    const constraints = Array.isArray(item.constraints)
      ? item.constraints.flatMap((constraint): MetricConstraintInput[] => {
          if (
            !isRecord(constraint) ||
            typeof constraint.attribute !== "string" ||
            !isNumericInput(constraint.minimum)
          )
            return [];
          return [
            {
              attribute: constraint.attribute,
              minimum: String(constraint.minimum),
              maximum: isNumericInput(constraint.maximum)
                ? String(constraint.maximum)
                : "",
            },
          ];
        })
      : [];
    if (operands.length === 0) return [];
    return [
      {
        presetId: item.presetId,
        metricType: item.metricType === "product" ? "product" : "minimum",
        label: item.label,
        operands,
        percentage: item.percentage === true,
        threshold: String(item.threshold),
        constraints,
      },
    ];
  });
  return restored.length > 0 ? restored : null;
}

/** 从本地存储恢复号位主属性；目录更新后自动丢弃已经不存在的主属性。 */
function restoreMainAttributes(value: unknown): MainAttributeDraft[] | null {
  if (!Array.isArray(value)) return null;
  const restored = value.flatMap((item): MainAttributeDraft[] => {
    if (
      !isRecord(item) ||
      ![2, 4, 6].includes(Number(item.slot)) ||
      !Array.isArray(item.mainAttrTypes)
    )
      return [];
    const slot = Number(item.slot) as 2 | 4 | 6;
    const allowed = new Set(
      (mainAttributeOptions[slot] ?? []).map((option) => option.value),
    );
    const mainAttrTypes = item.mainAttrTypes.filter(
      (mainAttrType): mainAttrType is string =>
        typeof mainAttrType === "string" && allowed.has(mainAttrType),
    );
    return [{ slot, mainAttrTypes }];
  });
  return restored.length > 0 ? restored : null;
}

/** 从本地存储恢复套装配方；具体套装不存在时降级为同分类通配，避免目录升级后表单失效。 */
function restoreRecipe(value: unknown): RecipeDraft[] | null {
  if (!Array.isArray(value)) return null;
  const restored = value.flatMap((item): RecipeDraft[] => {
    if (
      !isRecord(item) ||
      typeof item.setId !== "string" ||
      !Number.isInteger(Number(item.count))
    )
      return [];
    const count = Number(item.count);
    if (count < 1 || count > 6) return [];
    const category = typeof item.category === "string" ? item.category : "";
    if (item.setId === SCATTERED_SET_ID)
      return [{ setId: SCATTERED_SET_ID, count, category: SCATTERED_SET_ID }];
    if (item.setId === UNRESTRICTED_SET_ID) {
      return category && recipeCategories.value.includes(category)
        ? [{ setId: UNRESTRICTED_SET_ID, count, category }]
        : [];
    }
    const selected = sets.value.find((set) => set.setId === item.setId);
    if (selected)
      return [{ setId: selected.setId, count, category: setCategory(selected) }];
    return category && recipeCategories.value.includes(category)
      ? [{ setId: UNRESTRICTED_SET_ID, count, category }]
      : [];
  });
  return restored.length > 0 ? restored : null;
}

/** 恢复上次离开页面时的配置；Rust 文件和 WebView 存储取更新时间较新者，组合指标模式强制清空隐藏面板目标。 */
async function restoreSavedConfiguration(): Promise<boolean> {
  try {
    const configurationCandidates: string[] = [];
    try {
      const rustConfiguration = await desktopApi.getMiracleConchConfiguration();
      if (rustConfiguration) configurationCandidates.push(rustConfiguration);
    } catch {
      // 旧版本或非 Tauri 预览环境没有配置命令时，继续使用浏览器本地存储。
    }
    const browserConfiguration = window.localStorage.getItem(
      MIRACLE_CONFIG_STORAGE_KEY,
    );
    if (browserConfiguration) configurationCandidates.push(browserConfiguration);
    if (configurationCandidates.length === 0) return false;

    // 新配置按时间戳恢复；旧配置没有时间戳时让浏览器快照优先，兼容旧版“先写本地存储、后异步写 Rust”的保存顺序。
    const saved = configurationCandidates
      .map((raw, index) => {
        try {
          return {
            value: JSON.parse(raw) as Partial<MiracleConchSavedConfiguration>,
            index,
          };
        } catch {
          return null;
        }
      })
      .filter(
        (
          candidate,
        ): candidate is {
          value: Partial<MiracleConchSavedConfiguration>;
          index: number;
        } => candidate !== null,
      )
      .sort(
        (left, right) =>
          (Number(right.value.updatedAt) || 0) - (Number(left.value.updatedAt) || 0) ||
          right.index - left.index,
      )[0]?.value;
    if (!saved) return false;
    if (saved.version !== 1) return false;

    if (
      typeof saved.selectedShikigamiId === "string" &&
      shikigami.value.some((entry) => entry.shikigamiId === saved.selectedShikigamiId)
    ) {
      selectedShikigamiId.value = saved.selectedShikigamiId;
    }
    // 恢复已保存的稀有度筛选；ALL 表示不限制，其余值必须来自目录支持的稀有度。
    const savedRarity = saved.selectedShikigamiRarity;
    if (
      savedRarity === "ALL" ||
      (typeof savedRarity === "string" && shikigamiRarityOptions.includes(savedRarity))
    ) {
      selectedShikigamiRarity.value = savedRarity;
    }
    if (saved.targetMode === "metric" || saved.targetMode === "panel")
      targetMode.value = saved.targetMode;
    if (typeof saved.onlyMaxLevel === "boolean")
      onlyMaxLevel.value = saved.onlyMaxLevel;
    if (isRecord(saved.target)) {
      const restoredTarget = createEmptyTarget();
      for (const key of Object.keys(restoredTarget) as TargetField[]) {
        const value = saved.target[key];
        if (typeof value === "string") restoredTarget[key] = value;
      }
      target.value = restoredTarget;
    }
    const restoredMetrics = restoreMetrics(saved.metrics);
    if (restoredMetrics) metrics.value = restoredMetrics;
    const restoredMainAttributes = restoreMainAttributes(saved.mainAttributes);
    if (restoredMainAttributes) mainAttributes.value = restoredMainAttributes;
    const restoredRecipe = restoreRecipe(saved.recipe);
    if (restoredRecipe) recipe.value = restoredRecipe;

    if (targetMode.value === "metric") target.value = createEmptyTarget();
    return true;
  } catch {
    // 配置只影响当前页面；损坏或旧版本配置直接回退默认值，不阻塞库存读取。
    return false;
  }
}

/** 将当前表单配置立即保存到 Rust 配置文件并保留浏览器兜底；结果和错误提示不会持久化。 */
function saveConfiguration(): void {
  if (!configurationHydrated) return;
  const configuration: MiracleConchSavedConfiguration = {
    version: 1,
    updatedAt: Date.now(),
    selectedShikigamiId: selectedShikigamiId.value,
    selectedShikigamiRarity: selectedShikigamiRarity.value,
    targetMode: targetMode.value,
    target: { ...target.value },
    onlyMaxLevel: onlyMaxLevel.value,
    metrics: metrics.value.map((metric) => ({
      ...metric,
      operands: [...metric.operands],
      constraints: metric.constraints.map((constraint) => ({ ...constraint })),
    })),
    mainAttributes: mainAttributes.value.map((constraint) => ({
      slot: constraint.slot,
      mainAttrTypes: [...constraint.mainAttrTypes],
    })),
    recipe: recipe.value.map((line) => ({ ...line })),
  };
  const payload = JSON.stringify(configuration);
  try {
    window.localStorage.setItem(MIRACLE_CONFIG_STORAGE_KEY, payload);
  } catch {
    // 本地存储不可用时仍允许本次计算，不能把浏览器存储异常升级成业务错误。
  }
  queuedConfigurationPayload = payload;
  if (configurationSaveInFlight) return;
  configurationSaveInFlight = true;
  void (async () => {
    // 采用“最新快照优先”的串行队列，既不延迟首次保存，也不让旧的异步 IPC 写入反超新值。
    while (queuedConfigurationPayload) {
      const nextPayload = queuedConfigurationPayload;
      queuedConfigurationPayload = null;
      try {
        await desktopApi.setMiracleConchConfiguration(nextPayload);
      } catch {
        // 配置写入失败不影响当前页面计算，浏览器本地存储仍可作为下一次恢复的兜底。
      }
    }
    configurationSaveInFlight = false;
  })();
}

/** 从目录中解析套装名称；散件是前端专用伪套装，不请求目录。 */
function setLabel(setId: string): string {
  return setId === SCATTERED_SET_ID
    ? "散件"
    : (setNameById.value.get(setId) ?? "未知套装");
}

/** 把后端小数面板转成玩家习惯的百分比文本。 */
function panelValue(panel: MiraclePanel, key: TargetField): string {
  const value = panel[key] ?? 0;
  return targetDefinitions.find((definition) => definition.key === key)?.percent
    ? `${numberText(value * 100)}%`
    : numberText(value);
}

/** 统一数字显示，避免页面出现无意义的尾随零。 */
function numberText(value: number): string {
  return Number.isInteger(value)
    ? String(value)
    : value.toFixed(2).replace(/0+$/, "").replace(/\.$/, "");
}

/** 按指标单位展示结果；百分比指标以玩家习惯的百分数输出。 */
function metricNumberText(metric: { percentage: boolean }, value: number): string {
  return numberText(metric.percentage ? value * 100 : value);
}

/** 判断面板属性是否以百分比展示，避免“效果命中指标”的速度限制被误标为百分号。 */
function isPercentAttribute(attribute: string): boolean {
  return ["crit_rate", "crit_damage", "effect_hit", "effect_resist"].includes(
    attribute,
  );
}

/** 按属性自身单位展示组合指标的属性限制。 */
function metricConstraintText(attribute: string, value: number): string {
  return `${numberText(isPercentAttribute(attribute) ? value * 100 : value)}${isPercentAttribute(attribute) ? "%" : ""}`;
}

/** 展示区间约束的具体缺口；下限不足显示“还差”，上限超出显示“超出”。 */
function metricConstraintStatusText(
  constraint: MiracleMetricResult["constraints"][number],
): string {
  if (constraint.actual + 1e-9 < constraint.minimum) {
    return `${attributeLabel(constraint.attribute)}还差 ${metricConstraintText(constraint.attribute, constraint.gap)}`;
  }
  if (constraint.maximum != null && constraint.actual > constraint.maximum + 1e-9) {
    return `${attributeLabel(constraint.attribute)}超出 ${metricConstraintText(constraint.attribute, constraint.gap)}`;
  }
  return `${attributeLabel(constraint.attribute)}已在区间内`;
}

/** 御魂属性沿用“我的御魂”的格式：比例属性转百分数，固定属性保留最多两位小数。 */
function formatSoulAttributeValue(attributeType: string, value: number): string {
  const percentageAttributes = new Set([
    "attack_rate",
    "hp_rate",
    "defense_rate",
    "crit_rate",
    "crit_damage",
    "effect_hit",
    "effect_resist",
  ]);
  if (percentageAttributes.has(attributeType)) {
    const percentage = value * 100;
    return `${Number.isInteger(percentage) ? percentage : percentage.toFixed(2)}%`;
  }
  return Number.isInteger(value) ? String(value) : value.toFixed(2);
}

/** 固有属性在主属性区域单独展示，强化副属性区域只保留可强化属性。 */
function fixedSoulAttributes(soul: MiracleSolution["souls"][number]) {
  return soul.attributes.filter((attribute) => attribute.fixedAttribute);
}

/** 按“我的御魂”规则展示可强化副属性；未知强化次数继续显示问号。 */
function subSoulAttributes(soul: MiracleSolution["souls"][number]) {
  return soul.attributes.filter((attribute) => !attribute.fixedAttribute);
}

/** 按“我的御魂”顺序汇总一枚御魂的副属性，原始御魂与推荐胚子共用同一展示口径。 */
function soulSubstatSummary(soul: MiracleSoulView): string {
  const attributes = subSoulAttributes(soul);
  if (!attributes.length) return "暂无副属性";
  return attributes
    .map((attribute) => {
      const rollCount =
        attribute.enhancementCount === null ? "?" : `+${attribute.enhancementCount}`;
      return `${attributeLabel(attribute.attributeType)} ${formatSoulAttributeValue(attribute.attributeType, attribute.value)} ${rollCount}`;
    })
    .join(" · ");
}

/** 汇总固定属性，避免对比卡片把固有属性误混进可强化副属性。 */
function soulFixedAttributeSummary(soul: MiracleSoulView): string {
  const attributes = fixedSoulAttributes(soul);
  if (!attributes.length) return "无";
  return attributes
    .map(
      (attribute) =>
        `${attributeLabel(attribute.attributeType)} ${formatSoulAttributeValue(attribute.attributeType, attribute.value)}`,
    )
    .join(" · ");
}

/** 用玩家熟悉的中文号位名称，正式页面不再使用井号作为位置标记。 */
function positionText(slot: number): string {
  const labels: Record<number, string> = {
    1: "一号位",
    2: "二号位",
    3: "三号位",
    4: "四号位",
    5: "五号位",
    6: "六号位",
  };
  return labels[slot] ?? `${slot}号位`;
}

/** 展示替换后的式神面板，并同时给出相对于候选御魂当前组合的变化量。 */
function panelDeltaText(
  candidate: EnhancementCandidate,
  key: TargetField,
  panel: MiraclePanel,
): string {
  const delta = (panel[key] ?? 0) - (candidate.currentPanel[key] ?? 0);
  return Math.abs(delta) < 1e-9 ? "不变" : signedPanelDelta(key, delta);
}

/** 判断替换后的式神面板是否真的发生变化，用于把理论强化带来的属性直接标出来。 */
function panelStatChanged(
  candidate: EnhancementCandidate,
  key: TargetField,
  panel: MiraclePanel,
): boolean {
  return Math.abs((panel[key] ?? 0) - (candidate.currentPanel[key] ?? 0)) > 1e-9;
}

/** 将指标变化整理成底部解释行，理论与一般收益保持同一文案口径。 */
function metricExplanationText(candidate: EnhancementCandidate): string[] {
  const expected = enhancementExpectedMetricChanges(candidate);
  return [
    ...enhancementMetricChanges(candidate),
    ...expected.map((item) => `一般收益：${item}`),
  ];
}

/** 展示有实际变化的面板属性，明确给出替换后当前值到理论强化上限的变化。 */
function enhancementPanelChanges(candidate: EnhancementCandidate): string[] {
  return targetDefinitions
    .map((definition) => {
      const from = candidate.currentPanel[definition.key] ?? 0;
      const to = candidate.theoreticalPanel[definition.key] ?? 0;
      if (Math.abs(to - from) < 1e-9) return "";
      return `${definition.label} ${panelValue(candidate.currentPanel, definition.key)} → ${panelValue(candidate.theoreticalPanel, definition.key)}（${signedPanelDelta(definition.key, to - from)}）`;
    })
    .filter(Boolean);
}

/** 提炼该胚子最值得投入的目标属性，作为“为什么选它”的可读补充。 */
function enhancementPriorityText(candidate: EnhancementCandidate): string {
  const priorities = targetDefinitions
    .map((definition) => {
      const current = candidate.currentGap[definition.key] ?? 0;
      const theoretical = candidate.theoreticalGap[definition.key] ?? 0;
      const improvement = current - theoretical;
      return { label: definition.label, improvement };
    })
    .filter((item) => item.improvement > 1e-9)
    .sort((left, right) => right.improvement - left.improvement)
    .slice(0, 3)
    .map((item) => item.label);
  return priorities.length
    ? `主要改善：${priorities.join("、")}`
    : "主要改善：组合指标缺口";
}

/** 展示强化前后仍存在或发生变化的目标缺口，帮助玩家判断强化是否值得。 */
function enhancementGapChanges(candidate: EnhancementCandidate): string[] {
  return targetDefinitions
    .map((definition) => {
      const from = candidate.currentGap[definition.key] ?? 0;
      const to = candidate.theoreticalGap[definition.key] ?? 0;
      if (from <= 1e-9 && to <= 1e-9) return "";
      const formatGap = (value: number) =>
        definition.percent ? `${numberText(value * 100)}%` : numberText(value);
      return `${definition.label} 缺 ${formatGap(from)} → ${formatGap(to)}`;
    })
    .filter(Boolean);
}

/** 展示按新增属性概率加权后的平均面板收益，避免把三腿胚子的某个幸运分支当成必然结果。 */
function enhancementExpectedChanges(candidate: EnhancementCandidate): string[] {
  return targetDefinitions
    .map((definition) => {
      const from = candidate.currentPanel[definition.key] ?? 0;
      const to = candidate.expectedPanel[definition.key] ?? 0;
      if (Math.abs(to - from) < 1e-9) return "";
      return `${definition.label} ${panelValue(candidate.currentPanel, definition.key)} → ${panelValue(candidate.expectedPanel, definition.key)}（${signedPanelDelta(definition.key, to - from)}）`;
    })
    .filter(Boolean);
}

/** 面板增益使用玩家习惯的单位，百分比属性的内部小数在这里转回百分数。 */
function signedPanelDelta(key: TargetField, value: number): string {
  const definition = targetDefinitions.find((item) => item.key === key);
  const displayValue = definition?.percent
    ? `${numberText(value * 100)}%`
    : numberText(value);
  return value >= 0 ? `+${displayValue}` : displayValue;
}

/** 逐项展示组合指标的强化前后结果；公式达标但附加限制未达标时也会保留解释。 */
function enhancementMetricChanges(candidate: EnhancementCandidate): string[] {
  return candidate.currentMetrics.map((current, index) => {
    const theoretical = candidate.theoreticalMetrics[index];
    if (!theoretical)
      return `${current.label}：${metricNumberText(current, current.actual)}`;
    return `${current.label} ${metricNumberText(current, current.actual)} → ${metricNumberText(theoretical, theoretical.actual)}；${metricResultStatus(current)} → ${metricResultStatus(theoretical)}`;
  });
}

/** 展示组合指标的平均收益；理论上限和平均收益必须并列，方便玩家判断是否值得赌。 */
function enhancementExpectedMetricChanges(candidate: EnhancementCandidate): string[] {
  return candidate.currentMetrics.map((current, index) => {
    const expected = candidate.expectedMetrics[index];
    if (!expected)
      return `${current.label}：${metricNumberText(current, current.actual)}`;
    return `${current.label} ${metricNumberText(current, current.actual)} → ${metricNumberText(expected, expected.actual)}；${metricResultStatus(current)} → ${metricResultStatus(expected)}`;
  });
}

/** 格式化典型强化轨迹的整数手数，避免出现 1.25 手之类不符合游戏展示的值。 */
function expectedEnhancementCountText(count: number): string {
  return `+${count}`;
}

/** 判断期望后的副属性是否发生变化，用于高亮真正会被强化影响的属性。 */
function expectedAttributeChanged(
  attribute: EnhancementCandidate["expectedAttributes"][number],
): boolean {
  return attribute.isNewAttribute || Math.abs(attribute.expectedAddedValue) > 1e-9;
}

/** 展示期望副属性的概率说明；已有属性没有额外概率文案。 */
function expectedAttributeProbabilityText(
  attribute: EnhancementCandidate["expectedAttributes"][number],
): string {
  return attribute.probability === null
    ? ""
    : ` · ${numberText(attribute.probability * 100)}%分支`;
}

/** 判断理论路径中承担最大强化增量的副属性；并列时允许并列高亮，避免用排序顺序掩盖同值结果。 */
function theoreticalAttributeIsBest(
  attribute: EnhancementCandidate["theoreticalAttributes"][number],
  attributes: EnhancementCandidate["theoreticalAttributes"],
): boolean {
  const bestAddedValue = Math.max(
    0,
    ...attributes.map((item) => item.expectedAddedValue),
  );
  return bestAddedValue > 1e-9 && attribute.expectedAddedValue >= bestAddedValue - 1e-9;
}

/** 理论副属性行沿用右侧的强化手数展示，并给最有利路径增加明确标签。 */
function theoreticalAttributeStatusText(
  attribute: EnhancementCandidate["theoreticalAttributes"][number],
  attributes: EnhancementCandidate["theoreticalAttributes"],
): string {
  const rollCount = expectedEnhancementCountText(attribute.expectedEnhancementCount);
  if (theoreticalAttributeIsBest(attribute, attributes))
    return `最有利强化 · ${rollCount}`;
  if (attribute.isNewAttribute) return `新增分支 · ${rollCount}`;
  return rollCount;
}

/** 初始腿数缺少原始事实时，仅对未强化御魂按当前可强化属性数量推断，并明确这是推断值。 */
function initialSubstatText(soul: EnhancementCandidate["soul"]): string {
  if (soul.initialSubstatCount !== null) return `${soul.initialSubstatCount} 条`;
  if (soul.level === 0) {
    const count = subSoulAttributes(soul).length;
    return `推断 ${count} 条${count < 4 ? `（缺 ${4 - count} 条腿）` : ""}`;
  }
  return "未知（导入数据未提供）";
}

/** 属性稳定键转成前端中文标签；后端返回未知键时保留原值便于诊断。 */
function attributeLabel(attribute: string): string {
  const labels: Record<string, string> = {
    attack: "攻击",
    hp: "生命",
    defense: "防御",
    speed: "速度",
    critRate: "暴击",
    critDamage: "暴伤",
    effectHit: "效果命中",
    effectResist: "效果抵抗",
    attack_flat: "攻击",
    attack_rate: "攻击%",
    hp_flat: "生命",
    hp_rate: "生命%",
    defense_flat: "防御",
    defense_rate: "防御%",
    speed_flat: "速度",
    speed_rate: "速度%",
    crit_rate: "暴击",
    crit_damage: "暴伤",
    effect_hit: "效果命中",
    effect_resist: "效果抵抗",
  };
  return labels[attribute] ?? attribute;
}

/** 把面板字段映射为后端组合指标约束使用的稳定键。 */
function targetFieldStableKey(key: TargetField): string {
  const stableKeys: Record<TargetField, string> = {
    attack: "attack",
    hp: "hp",
    defense: "defense",
    speed: "speed",
    critRate: "crit_rate",
    critDamage: "crit_damage",
    effectHit: "effect_hit",
    effectResist: "effect_resist",
  };
  return stableKeys[key];
}

/** 展示面板目标、组合指标约束和无目标三种状态，避免空白目标被误报为已达标。 */
function panelStatusText(solution: MiracleSolution, key: TargetField): string {
  const label =
    targetDefinitions.find((definition) => definition.key === key)?.label ?? key;
  const stableKey = targetFieldStableKey(key);
  const constraints = solution.metrics.flatMap((metric) =>
    metric.constraints.filter((constraint) => constraint.attribute === stableKey),
  );
  const unmetConstraint = constraints.find((constraint) => !constraint.met);
  if (unmetConstraint) {
    return `未达标 · ${metricConstraintStatusText(unmetConstraint)}`;
  }
  if (solution.unmetAttributes.includes(label)) {
    // 组合指标的同名标签（例如“攻击”指标）不能反过来当成面板目标状态。
    if (solution.metrics.length > 0) return "未限制";
    const definition = targetDefinitions.find((item) => item.key === key);
    const gap = solution.gap[key] ?? 0;
    return definition?.percent
      ? `未达标 · 缺 ${numberText(gap * 100)}%`
      : `未达标 · 缺 ${numberText(gap)}`;
  }
  // 组合指标名称也会进入后端达标列表；只有普通面板模式才用该列表判断面板属性。
  if (solution.metrics.length === 0 && solution.metAttributes.includes(label))
    return "已达标";
  if (constraints.length > 0) return "已达标";
  return "未限制";
}

/** 判断面板状态是否存在缺口；仅为未达标文本增加视觉强调，不改变原有状态计算。 */
function panelStatusIsUnmet(solution: MiracleSolution, key: TargetField): boolean {
  return panelStatusText(solution, key).startsWith("未达标");
}

/** 组合指标优先展示公式缺口，其次明确列出暴击、速度等附加限制的缺口。 */
function metricResultStatus(metric: MiracleMetricResult): string {
  if (metric.met) return `已达到 ${metricNumberText(metric, metric.threshold)}`;
  const parts: string[] = [];
  if (metric.gap > 0) {
    parts.push(`公式还差 ${metricNumberText(metric, metric.gap)}`);
  }
  const unmetConstraints = metric.constraints
    .filter((constraint) => !constraint.met)
    .map((constraint) => metricConstraintStatusText(constraint));
  if (unmetConstraints.length) parts.push(`属性限制：${unmetConstraints.join("、")}`);
  return parts.join("；") || "未达标";
}

/** 将标准评分映射为与“御魂分析”一致的颜色层级；分数为空时保持未计算状态。 */
function scoreTone(score: number | null): string {
  if (score === null) return "unknown";
  if (score >= scoreColorThresholds.value.rainbowScore) return "legendary";
  if (score >= scoreColorThresholds.value.goldScore) return "epic";
  if (score >= scoreColorThresholds.value.jadeScore) return "high";
  if (score >= 50) return "medium";
  return "low";
}

/** 标准评分与“御魂分析”保持一位小数；尚未生成评分时不伪造数值。 */
function formatStandardScore(score: number | null): string {
  return score === null ? "—" : score.toFixed(1);
}

/** 生成标准评分的简短悬浮说明，优先展示命中的规则卡片和已有公式。 */
function standardScoreTooltip(soul: MiracleSoulView): string {
  const summary = scoreBySoulKey.value.get(soul.soulKey);
  if (!summary) return "标准评分尚未计算";
  return [
    `标准评分：${formatStandardScore(summary.standardScore)}`,
    summary.standardScoreRule
      ? `规则卡片：${summary.standardScoreRule}`
      : "规则卡片：尚未命中",
    summary.standardScoreFormula ?? "计算公式：尚未生成",
  ].join("\n");
}

/** 读取“御魂分析”当前启用标准的颜色阈值；标准读取失败不阻断神奇海螺主体结果。 */
async function loadScoreStandard(): Promise<void> {
  try {
    const standards = await desktopApi.listScoreStandards();
    const enabled = standards.find((standard) => standard.enabled);
    if (enabled) scoreColorThresholds.value = enabled.scoreColorThresholds;
  } catch {
    // 标准评分摘要仍可正常展示；颜色阈值失败时沿用默认分级，避免阻断配装结果。
  }
}

/** 按当前结果中的御魂键批量读取已保存的标准评分，避免逐枚调用评分接口。 */
async function loadSolutionScores(
  nextResult: MiracleConchResult,
  generation: number,
): Promise<void> {
  const soulKeys = [
    ...new Set(
      nextResult.solutions.flatMap((solution) =>
        solution.souls.map((soul) => soul.soulKey),
      ),
    ),
  ];
  scoreBySoulKey.value = new Map();
  if (!soulKeys.length) return;
  try {
    const summaries = await desktopApi.listMySoulScores(soulKeys);
    // 评分查询可能晚于下一次配装请求返回，旧查询不能覆盖新结果的评分状态。
    if (generation !== solutionScoreLoadGeneration) return;
    scoreBySoulKey.value = new Map(
      summaries.map((summary) => [summary.soulKey, summary]),
    );
  } catch {
    // 评分摘要缺失或读取失败时保留“—”，不影响用户查看配装组合本身。
  }
}

/** 目录未安装或校验不满足页面门槛时，使用应用内置目录重新安装。 */
async function loadPage(): Promise<void> {
  loading.value = true;
  errorMessage.value = "";
  try {
    let catalog = await desktopApi.getCatalogStatus();
    if (
      !catalog ||
      catalog.setCount < EXPECTED_SET_COUNT ||
      !["verified", "partial"].includes(catalog.validationStatus)
    ) {
      catalog = await desktopApi.installBuiltinCatalog();
    }
    catalogStatus.value = catalog;
    [sets.value, shikigami.value] = await Promise.all([
      desktopApi.listSoulSets(catalog.version),
      desktopApi.listShikigami(catalog.version),
    ]);
    await loadScoreStandard();
    initializeDefaultRecipe();
    const defaultShikigami =
      shikigami.value.find(
        (entry) => entry.shikigamiId === selectedShikigamiId.value,
      ) ??
      shikigami.value[0] ??
      null;
    selectedShikigamiId.value = defaultShikigami?.shikigamiId ?? "389";
    selectedShikigamiRarity.value = defaultShikigami?.rarity ?? "ALL";
    const restored = await restoreSavedConfiguration();
    if (!restored && targetMode.value === "panel") fillTargetFromSelectedShikigami();
    configurationHydrated = true;
    saveConfiguration();
  } catch (caught) {
    const error = normalizeCommandError(caught);
    errorMessage.value = `${error.code} · ${error.message}`;
  } finally {
    loading.value = false;
  }
}

/** 新增一行配方，默认给出散件，用户再调整为 4+2 或首领组合。 */
function addRecipeLine(): void {
  recipe.value.push(
    createRecipeDraft(ordinarySets.value[0]?.setId ?? SCATTERED_SET_ID, 0),
  );
}

/** 删除配方行；至少保留一行，避免表单无法恢复到合法状态。 */
function removeRecipeLine(index: number): void {
  if (recipe.value.length <= 1) return;
  recipe.value.splice(index, 1);
}

/** 稀有度变化后自动选中该稀有度的第一位式神，避免下拉框保留已不可见的旧值。 */
function handleShikigamiRarityChange(): void {
  const currentSelectionStillVisible = selectableShikigami.value.some(
    (entry) => entry.shikigamiId === selectedShikigamiId.value,
  );
  if (!currentSelectionStillVisible) {
    selectedShikigamiId.value = selectableShikigami.value[0]?.shikigamiId ?? "";
  }
  if (targetMode.value === "panel" && selectedShikigami.value) {
    fillTargetFromSelectedShikigami();
  } else if (targetMode.value === "panel") {
    // 稀有度没有目录条目时清空旧目标，避免面板模式继续沿用上一位式神的数据。
    target.value = createEmptyTarget();
  } else {
    // 组合指标模式不携带式神基础面板目标，避免把搜索从两维扩大成八维。
    target.value = createEmptyTarget();
  }
  result.value = null;
  validationMessage.value = "";
}

/** 选择具体式神后同步基础面板目标，并清理旧结果，防止结果看起来仍属于上一位式神。 */
function handleShikigamiChange(): void {
  if (targetMode.value === "panel") fillTargetFromSelectedShikigami();
  else target.value = createEmptyTarget();
  result.value = null;
  validationMessage.value = "";
}

/** 打开或关闭式神头像面板；空稀有度不会打开空列表。 */
function toggleShikigamiPicker(): void {
  if (selectableShikigami.value.length === 0) return;
  isShikigamiPickerOpen.value = !isShikigamiPickerOpen.value;
  if (isShikigamiPickerOpen.value) {
    void nextTick(() => shikigamiSearchRef.value?.focus());
  }
}

/** 让 Enter、空格和方向键都可以打开头像选择面板，Escape 可快速收起。 */
function handleShikigamiPickerKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    isShikigamiPickerOpen.value = false;
    return;
  }
  if (["Enter", " ", "ArrowDown"].includes(event.key)) {
    event.preventDefault();
    isShikigamiPickerOpen.value = true;
    void nextTick(() => shikigamiSearchRef.value?.focus());
  }
}

/** 搜索框按键不冒泡到面板快捷键，避免输入空格或回车被拦截；Escape 在框内也能收起。 */
function handleShikigamiSearchKeydown(event: KeyboardEvent): void {
  event.stopPropagation();
  if (event.key === "Escape") isShikigamiPickerOpen.value = false;
}

/** 选择头像卡片对应的式神，复用原有目标同步逻辑并收起候选面板。 */
function selectShikigami(shikigamiId: string): void {
  selectedShikigamiId.value = shikigamiId;
  isShikigamiPickerOpen.value = false;
  handleShikigamiChange();
}

/** 返回随前端打包的离线头像路径，目录加载失败时不依赖远程图片。 */
function shikigamiAvatarPath(entry: Shikigami): string {
  return `${import.meta.env.BASE_URL}shikigami-avatars/${entry.shikigamiId}.webp`;
}

/** 头像缺失时隐藏图片但保留头像容器，避免候选列表发生布局跳动。 */
function hideBrokenShikigamiAvatar(event: Event): void {
  const image = event.currentTarget;
  if (image instanceof HTMLImageElement) image.hidden = true;
}

/** 点击选择器外部时关闭头像候选面板。 */
function closeShikigamiPickerOnOutsideClick(event: PointerEvent): void {
  if (
    isShikigamiPickerOpen.value &&
    event.target instanceof Node &&
    !shikigamiPickerRef.value?.contains(event.target)
  ) {
    isShikigamiPickerOpen.value = false;
  }
}

/** 在单项面板和组合指标之间切换；切换到组合指标时清空单项最低值，避免两种目标意外叠加。 */
function setTargetMode(mode: TargetMode): void {
  targetMode.value = mode;
  if (mode === "metric") {
    target.value = createEmptyTarget();
  } else {
    fillTargetFromSelectedShikigami();
  }
}

// 重置整个神奇海螺配置：恢复默认式神、目标模式、指标、号位主属性、套装配方和满级开关。
// 同时清理旧结果和提示，并立即保存默认配置，保证下次进入页面仍然是一个可用的初始状态。
function resetConfiguration(): void {
  errorMessage.value = "";
  validationMessage.value = "";
  result.value = null;
  const defaultShikigami =
    shikigami.value.find((entry) => entry.shikigamiId === "389") ?? shikigami.value[0];
  selectedShikigamiId.value = defaultShikigami?.shikigamiId ?? "389";
  selectedShikigamiRarity.value = defaultShikigami?.rarity ?? "SSR";
  targetMode.value = "metric";
  onlyMaxLevel.value = true;
  target.value = createEmptyTarget();
  metrics.value = [createMetricDraft()];
  resetMainAttributes();
  initializeDefaultRecipe();
  saveConfiguration();
}

/** 选择指标预设后同步公式、单位和默认属性限制，用户仍可继续改写。 */
function applyMetricPreset(metric: MetricDraft): void {
  const preset =
    metricPresets.find((item) => item.id === metric.presetId) ?? metricPresets[0];
  metric.metricType = preset.metricType;
  metric.label = preset.label;
  metric.operands = [...preset.operands];
  metric.percentage = preset.percentage;
  metric.threshold = preset.threshold;
  metric.constraints = preset.constraints.map((constraint) => ({
    ...constraint,
    maximum: "",
  }));
}

/** 新增一项组合指标；每一项都会独立参与达标判断。 */
function addMetric(): void {
  metrics.value.push(createMetricDraft("healing"));
}

/** 删除组合指标；至少保留一项，避免组合指标模式失去目标。 */
function removeMetric(index: number): void {
  if (metrics.value.length <= 1) return;
  metrics.value.splice(index, 1);
}

/** 为某一项指标增加属性限制，例如暴击 100%～120% 或速度 152～180。 */
function addMetricConstraint(metric: MetricDraft): void {
  const existing = new Set(
    metric.constraints.map((constraint) => constraint.attribute),
  );
  const next = Object.keys(metricOperandsLabel).find(
    (attribute): attribute is string => !existing.has(attribute),
  );
  if (next) metric.constraints.push({ attribute: next, minimum: "100", maximum: "" });
}

/** 删除当前指标的一项属性限制。 */
function removeMetricConstraint(metric: MetricDraft, index: number): void {
  metric.constraints.splice(index, 1);
}

/** 把指标草稿转换成后端所需的小数单位；百分比上下限统一转换为小数。 */
function buildMetrics(): MiracleMetric[] {
  return metrics.value.map((metric) => ({
    presetId: metric.presetId,
    metricType: metric.metricType,
    label: metric.label.trim(),
    operands: [...metric.operands],
    threshold: Number(metric.threshold) * (metric.percentage ? 0.01 : 1),
    percentage: metric.percentage,
    constraints: metric.constraints.map((constraint) => {
      const scale = isPercentAttribute(constraint.attribute) ? 0.01 : 1;
      const maximum = parseOptionalMetricMaximum(constraint.maximum);
      // 数字输入和旧字符串配置统一转换；只有真正空白的上限才发送 null。
      return {
        attribute: constraint.attribute,
        minimum: Number(constraint.minimum) * scale,
        maximum: maximum === null ? null : maximum * scale,
      };
    }),
  }));
}

/** 把号位主属性多选草稿展开为后端约束；同一号位的多个属性表示满足任一项即可。 */
function buildMainAttributes(): MiracleMainAttributeConstraint[] {
  return mainAttributes.value.flatMap((constraint) =>
    constraint.mainAttrTypes
      .filter((mainAttrType) => mainAttrType)
      .map((mainAttrType) => ({ slot: constraint.slot, mainAttrType })),
  );
}

/** 将界面百分比和普通数值统一转换为后端使用的目标面板。 */
function buildTarget(): MiracleTarget {
  const built: MiracleTarget = {};
  for (const definition of targetDefinitions) {
    const raw = target.value[definition.key].trim();
    if (!raw) continue;
    const parsed = Number(raw);
    if (!Number.isFinite(parsed)) continue;
    built[definition.key] = definition.percent ? parsed / 100 : parsed;
  }
  return built;
}

/** 将界面中的部分套装配方补成六件请求；未指定的剩余位置统一按任意御魂处理。 */
function buildRecipe(): MiracleRecipeLine[] {
  const lines = recipe.value.map((line) => ({
    setId: line.setId,
    count: Number(line.count),
    ...(line.setId === UNRESTRICTED_SET_ID && line.category
      ? { category: line.category }
      : {}),
  }));
  const remaining = 6 - recipeSpecifiedTotal.value;
  if (remaining > 0) {
    const unrestrictedLine = lines.find((line) => line.setId === SCATTERED_SET_ID);
    if (unrestrictedLine) unrestrictedLine.count += remaining;
    else lines.push({ setId: SCATTERED_SET_ID, count: remaining });
  }
  return lines;
}

/** 前端先完成轻量表单校验，后端继续执行相同规则作为最终边界。 */
function validateForm(): boolean {
  validationMessage.value = "";
  if (!selectedShikigamiId.value) {
    validationMessage.value = "请选择式神。";
  } else if (targetMode.value === "panel" && !hasTarget.value) {
    validationMessage.value = "至少填写一项目标属性，空白属性表示不限制。";
  } else if (targetMode.value === "metric" && !metricsValid.value) {
    const hasInvalidRange = metrics.value.some((metric) =>
      metric.constraints.some((constraint) => {
        const minimum = Number(constraint.minimum);
        const maximum = parseOptionalMetricMaximum(constraint.maximum);
        return (
          Number.isFinite(minimum) &&
          maximum !== null &&
          Number.isFinite(maximum) &&
          maximum < minimum
        );
      }),
    );
    // 明确指出上限小于下限，避免用户误以为点击后没有触发计算。
    validationMessage.value = hasInvalidRange
      ? "属性限制的最高值不能小于最低值。"
      : "请至少保留一项指标，并填写有效目标值和属性限制。";
  } else if (recipeSpecifiedTotal.value > 6) {
    validationMessage.value = `套装配方已指定 ${recipeSpecifiedTotal.value} 件，不能超过 6 件；不足部分会自动不限制套装。`;
  } else if (
    recipe.value.some((line) => !line.setId || line.count <= 0 || line.count > 6)
  ) {
    validationMessage.value = "每行套装件数必须是 1 到 6 之间的整数。";
  } else if (
    recipe.value.some((line) => line.setId === UNRESTRICTED_SET_ID && !line.category)
  ) {
    validationMessage.value = "请选择套装方向，或将该行改为散件。";
  } else if (recipeHasDuplicate.value) {
    validationMessage.value =
      "同一套装或同一套装方向请合并到一行，剩余位置会自动不限制。";
  }
  return !validationMessage.value;
}

/** 启动计算遮罩：阶段进度只缓慢推进到保守上限，真正完成时才跳到 100%。 */
function startCalculationProgress(): void {
  calculationStageIndex = 0;
  calculationProgress.value = 8;
  calculationStage.value = calculationStages[0].label;
  if (calculationTimer) clearInterval(calculationTimer);
  calculationTimer = setInterval(() => {
    const stage =
      calculationStages[calculationStageIndex] ??
      calculationStages[calculationStages.length - 1];
    const nextStage = calculationStages[calculationStageIndex];
    if (!stage) return;
    if (calculationProgress.value < stage.ceiling) {
      calculationProgress.value = Math.min(
        stage.ceiling,
        calculationProgress.value + 2,
      );
      return;
    }
    if (calculationStageIndex < calculationStages.length - 1) {
      calculationStageIndex += 1;
      const followingStage = calculationStages[calculationStageIndex] ?? nextStage;
      if (!followingStage) return;
      calculationStage.value = followingStage.label;
      calculationProgress.value = Math.min(
        calculationProgress.value + 1,
        followingStage.ceiling,
      );
    }
  }, 180);
}

/** 结束计算遮罩并清理计时器；下一帧再关闭，保证用户能看到“已完成”的收束状态。 */
async function finishCalculationProgress(): Promise<void> {
  if (calculationTimer) {
    clearInterval(calculationTimer);
    calculationTimer = null;
  }
  calculationProgress.value = 100;
  calculationStage.value = "方案已整理完成";
  await nextTick();
}

/** 调用后端一次性读取库存并返回最佳组合、缺口和后续方向。 */
async function calculate(): Promise<void> {
  result.value = null;
  scoreBySoulKey.value = new Map();
  const scoreLoadGeneration = ++solutionScoreLoadGeneration;
  errorMessage.value = "";
  if (!validateForm()) return;
  calculating.value = true;
  startCalculationProgress();
  try {
    // 请求组装也放进 try，异常配置不得绕过 finally 留下无法关闭的计算遮罩。
    const request = {
      shikigamiId: selectedShikigamiId.value,
      // 组合指标模式只发送指标目标；隐藏的式神基础面板不能混入搜索维度。
      target: targetMode.value === "panel" ? buildTarget() : {},
      onlyMaxLevel: onlyMaxLevel.value,
      metrics: targetMode.value === "metric" ? buildMetrics() : [],
      mainAttributes: buildMainAttributes(),
      recipe: buildRecipe(),
    };
    const nextResult = await desktopApi.calculateMiracleConch(request);
    result.value = nextResult;
    // 评分是结果卡片的附加信息，不能阻塞主方案从 94% 收束到完成状态。
    void loadSolutionScores(nextResult, scoreLoadGeneration);
  } catch (caught) {
    const error = normalizeCommandError(caught);
    errorMessage.value = `${error.code} · ${error.message}`;
  } finally {
    await finishCalculationProgress();
    calculating.value = false;
  }
}

/** 套装计数只展示给用户看，散件伪套装不显示为激活效果。 */
function setCountText(solution: MiracleSolution): string {
  return Object.entries(solution.setCounts)
    .filter(([setId]) => setId !== SCATTERED_SET_ID)
    .map(([setId, count]) => `${setLabel(setId)} ${count}件`)
    .concat(
      solution.setCounts[SCATTERED_SET_ID]
        ? [`散件 ${solution.setCounts[SCATTERED_SET_ID]}件`]
        : [],
    )
    .join(" · ");
}

/** 把选中的式神基础面板带入目标输入，用户仍可手动改写或清空。 */
function fillTargetFromSelectedShikigami(): void {
  if (!selectedShikigami.value) return;
  Object.assign(
    target.value,
    getMiracleTargetInputFromPanel(selectedShikigami.value.panel),
  );
}

/** 关闭式神面板时清空名字搜索，避免下次打开时列表仍然停留在上次的过滤结果。 */
watch(isShikigamiPickerOpen, (open) => {
  if (!open) shikigamiQuery.value = "";
});

/** 监听所有用户可编辑配置；目录、结果和错误提示不进入持久化快照。 */
watch(
  [
    selectedShikigamiId,
    selectedShikigamiRarity,
    targetMode,
    target,
    onlyMaxLevel,
    metrics,
    mainAttributes,
    recipe,
  ],
  saveConfiguration,
  { deep: true },
);

/** 页面只在挂载时加载目录和当前库存入口；配置恢复后自动保留用户上次选择。 */
onMounted(async () => {
  window.addEventListener("pointerdown", closeShikigamiPickerOnOutsideClick);
  stopActiveCharacterChanged = onActiveCharacterChanged(() => {
    result.value = null;
    void loadPage();
  });
  await loadPage();
});

/** 页面离开时停止计算进度定时器，避免组件卸载后仍有异步状态更新。 */
onBeforeUnmount(() => {
  window.removeEventListener("pointerdown", closeShikigamiPickerOnOutsideClick);
  stopActiveCharacterChanged?.();
  stopActiveCharacterChanged = null;
  if (calculationTimer) clearInterval(calculationTimer);
});
</script>

<template>
  <main class="workspace miracle-page" aria-labelledby="miracle-title">
    <div
      v-if="calculating"
      class="miracle-calculation-overlay"
      role="status"
      aria-live="polite"
      aria-label="神奇海螺正在计算"
    >
      <div class="miracle-calculation-dialog">
        <div class="miracle-calculation-orbit" aria-hidden="true">
          <span></span>
          <i></i>
        </div>
        <p class="section-kicker">MIRACLE CONCH / SEARCHING</p>
        <h2>{{ calculationStage }}</h2>
        <p class="miracle-calculation-copy">
          正在从当前库存中寻找满足目标的六件组合，请稍候。
        </p>
        <div class="miracle-calculation-progress" aria-hidden="true">
          <span :style="{ width: `${calculationProgress}%` }"></span>
        </div>
        <div class="miracle-calculation-meta">
          <span>搜索进度</span>
          <strong>{{ calculationProgress }}%</strong>
        </div>
        <small>计算期间暂不修改库存，也不会创建行动批次。</small>
      </div>
    </div>
    <header class="page-header">
      <div>
        <p class="eyebrow">TARGET BUILD / MIRACLE CONCH</p>
        <h1 id="miracle-title">神奇海螺</h1>
        <p class="lede">告诉我你想要的面板，我从当前御魂库存里找一套最接近的答案。</p>
      </div>
      <div class="miracle-header-note">
        <CharacterArchiveSwitcher />
        <span class="status-chip">只读计算</span>
        <span v-if="catalogStatus" class="status-chip status-chip--quiet">{{
          catalogStatus.version
        }}</span>
      </div>
    </header>

    <div v-if="loading" class="empty-state miracle-loading" role="status">
      正在读取本地目录和当前库存入口……
    </div>
    <div v-else-if="errorMessage && !result" class="error-panel" role="alert">
      <strong>神奇海螺暂时不可用</strong>
      <p>{{ errorMessage }}</p>
      <button class="button button--quiet" type="button" @click="loadPage">
        重新读取
      </button>
    </div>

    <template v-else>
      <section class="miracle-controls" aria-label="神奇海螺计算条件">
        <div class="surface-card miracle-card">
          <div class="miracle-card__heading">
            <div>
              <p class="section-kicker">01 · 目标对象</p>
              <h2>先选式神</h2>
            </div>
            <span class="miracle-default-note">默认须佐之男 · 使用当前库存</span>
          </div>
          <div class="miracle-form-grid miracle-form-grid--two">
            <label class="compact-field">
              <span>稀有度</span>
              <select
                v-model="selectedShikigamiRarity"
                @change="handleShikigamiRarityChange"
              >
                <option value="ALL">全部稀有度</option>
                <option
                  v-for="rarity in shikigamiRarityOptions"
                  :key="rarity"
                  :value="rarity"
                >
                  {{ rarity }}
                </option>
              </select>
            </label>
            <div
              ref="shikigamiPickerRef"
              class="compact-field miracle-shikigami-picker"
            >
              <span>式神</span>
              <button
                class="miracle-shikigami-trigger"
                type="button"
                :disabled="selectableShikigami.length === 0"
                aria-haspopup="listbox"
                :aria-expanded="isShikigamiPickerOpen"
                aria-controls="miracle-shikigami-options"
                @click="toggleShikigamiPicker"
                @keydown="handleShikigamiPickerKeydown"
              >
                <span v-if="selectedShikigami" class="miracle-shikigami-option-main">
                  <span
                    class="miracle-shikigami-avatar"
                    :class="`miracle-shikigami-avatar--${selectedShikigami.rarity.toLowerCase()}`"
                    aria-hidden="true"
                  >
                    <img
                      :src="shikigamiAvatarPath(selectedShikigami)"
                      alt=""
                      @error="hideBrokenShikigamiAvatar"
                    />
                  </span>
                  <span class="miracle-shikigami-option-copy">
                    <strong>{{ selectedShikigami.name }}</strong>
                    <small
                      >{{ selectedShikigami.rarity }} ·
                      {{ selectedShikigami.shikigamiId }}</small
                    >
                  </span>
                </span>
                <span v-else class="miracle-shikigami-empty">当前稀有度暂无式神</span>
                <span class="miracle-shikigami-chevron" aria-hidden="true">⌄</span>
              </button>
              <div
                v-if="isShikigamiPickerOpen"
                id="miracle-shikigami-options"
                class="miracle-shikigami-menu"
                role="listbox"
                aria-label="选择式神"
                @keydown="handleShikigamiPickerKeydown"
              >
                <label class="miracle-shikigami-search">
                  <input
                    ref="shikigamiSearchRef"
                    v-model="shikigamiQuery"
                    type="search"
                    aria-label="搜索式神名字"
                    placeholder="输入名字模糊查找"
                    autocomplete="off"
                    @keydown.stop="handleShikigamiSearchKeydown"
                  />
                </label>
                <button
                  v-for="entry in selectableShikigami"
                  :key="entry.shikigamiId"
                  class="miracle-shikigami-option"
                  :class="{
                    'miracle-shikigami-option--selected':
                      selectedShikigamiId === entry.shikigamiId,
                  }"
                  type="button"
                  role="option"
                  :aria-selected="selectedShikigamiId === entry.shikigamiId"
                  @click="selectShikigami(entry.shikigamiId)"
                >
                  <span
                    class="miracle-shikigami-avatar"
                    :class="`miracle-shikigami-avatar--${entry.rarity.toLowerCase()}`"
                    aria-hidden="true"
                  >
                    <img
                      :src="shikigamiAvatarPath(entry)"
                      alt=""
                      @error="hideBrokenShikigamiAvatar"
                    />
                  </span>
                  <span class="miracle-shikigami-option-copy">
                    <strong>{{ entry.name }}</strong>
                    <small>{{ entry.rarity }} · {{ entry.shikigamiId }}</small>
                  </span>
                  <span
                    v-if="selectedShikigamiId === entry.shikigamiId"
                    class="miracle-shikigami-selected-mark"
                    aria-hidden="true"
                    >✓</span
                  >
                </button>
                <p
                  v-if="selectableShikigami.length === 0"
                  class="miracle-shikigami-empty"
                >
                  {{
                    shikigamiQuery.trim() ? "没有找到匹配的式神" : "当前稀有度暂无式神"
                  }}
                </p>
              </div>
            </div>
          </div>
          <p v-if="selectedShikigami" class="miracle-base-panel">
            基础面板：<template
              v-for="(item, index) in selectedBasePanelItems"
              :key="item.key"
              ><span v-if="index > 0"> · </span>{{ item.label }}
              {{
                item.percent
                  ? `${numberText(item.value * 100)}%`
                  : numberText(item.value)
              }}</template
            >
          </p>
        </div>

        <div class="surface-card miracle-card">
          <div class="miracle-card__heading">
            <div>
              <p class="section-kicker">02 · 目标指标</p>
              <h2>面板目标，或者一个组合公式</h2>
            </div>
            <div class="miracle-heading-actions">
              <span class="miracle-default-note">默认使用组合指标</span>
              <button
                class="button button--quiet button--small"
                type="button"
                @click="resetConfiguration"
              >
                重置配置
              </button>
            </div>
          </div>
          <div class="miracle-mode-switch" role="tablist" aria-label="目标指标模式">
            <button
              type="button"
              :class="{ 'miracle-mode-switch__item--active': targetMode === 'metric' }"
              @click="setTargetMode('metric')"
            >
              组合指标
            </button>
            <button
              type="button"
              :class="{ 'miracle-mode-switch__item--active': targetMode === 'panel' }"
              @click="setTargetMode('panel')"
            >
              面板最低值
            </button>
          </div>
          <div v-if="targetMode === 'panel'" class="miracle-target-grid">
            <label
              v-for="definition in targetDefinitions"
              :key="definition.key"
              class="miracle-target-field"
            >
              <span>{{ definition.label }}</span>
              <div class="miracle-input-wrap">
                <input
                  v-model="target[definition.key]"
                  type="number"
                  min="0"
                  step="definition.percent ? 0.1 : 1"
                  :placeholder="definition.percent ? '例如 100' : '不限'"
                />
                <em>{{ definition.unit }}</em>
              </div>
            </label>
          </div>
          <div v-else class="miracle-metric-editor">
            <div
              v-for="(metric, metricIndex) in metrics"
              :key="metricIndex"
              class="miracle-metric-block"
            >
              <div class="miracle-metric-topline">
                <label class="compact-field"
                  ><span>指标名称</span
                  ><select
                    v-model="metric.presetId"
                    @change="applyMetricPreset(metric)"
                  >
                    <option
                      v-for="preset in metricPresets"
                      :key="preset.id"
                      :value="preset.id"
                    >
                      {{ preset.label }}
                    </option>
                  </select></label
                >
                <label class="compact-field"
                  ><span>目标值</span>
                  <div class="miracle-input-wrap">
                    <input
                      v-model="metric.threshold"
                      type="number"
                      min="0"
                      step="0.1"
                      :placeholder="metric.percentage ? '例如 100' : '例如 30000'"
                    /><em>{{ metric.percentage ? "%" : "点" }}</em>
                  </div></label
                >
                <button
                  v-if="metrics.length > 1"
                  class="button button--quiet button--small"
                  type="button"
                  @click="removeMetric(metricIndex)"
                >
                  移除指标
                </button>
              </div>
              <div class="miracle-formula-line">
                <span class="miracle-formula-label">公式</span>
                <strong class="miracle-formula-preview">{{
                  metricExpression(metric)
                }}</strong>
                <span class="miracle-formula-operator">≥</span>
                <strong
                  >{{ metric.threshold }}{{ metric.percentage ? "%" : "" }}</strong
                >
              </div>
              <div class="miracle-constraint-editor">
                <span class="miracle-formula-label">属性限制（区间）</span>
                <div
                  v-for="(constraint, constraintIndex) in metric.constraints"
                  :key="constraintIndex"
                  class="miracle-constraint-row"
                >
                  <div class="miracle-input-wrap">
                    <input
                      v-model="constraint.minimum"
                      type="number"
                      min="0"
                      step="0.1"
                      placeholder="最低值"
                    /><em>{{
                      isPercentAttribute(constraint.attribute) ? "%" : "点"
                    }}</em>
                  </div>
                  <span class="miracle-range-operator">≤</span>
                  <select v-model="constraint.attribute" class="miracle-formula-select">
                    <option
                      v-for="(label, attribute) in metricOperandsLabel"
                      :key="attribute"
                      :value="attribute"
                    >
                      {{ label }}
                    </option>
                  </select>
                  <span class="miracle-range-operator">≤</span>
                  <div class="miracle-input-wrap">
                    <input
                      v-model="constraint.maximum"
                      type="number"
                      min="0"
                      step="0.1"
                      placeholder="不限"
                    /><em>{{
                      isPercentAttribute(constraint.attribute) ? "%" : "点"
                    }}</em>
                  </div>
                  <button
                    class="button button--quiet button--small"
                    type="button"
                    @click="removeMetricConstraint(metric, constraintIndex)"
                  >
                    移除
                  </button>
                </div>
                <button
                  class="button button--quiet button--small"
                  type="button"
                  @click="addMetricConstraint(metric)"
                >
                  + 添加属性限制
                </button>
              </div>
            </div>
            <div class="miracle-metric-actions">
              <button
                class="button button--quiet button--small"
                type="button"
                @click="addMetric"
              >
                + 添加指标</button
              ><span
                >每项指标都必须达标；属性限制填写最低值和可选最高值，最高值留空表示不限。治疗量示例为生命
                × 暴伤，百分比属性按面板百分比展示。</span
              >
            </div>
          </div>
        </div>

        <div class="surface-card miracle-card">
          <div class="miracle-card__heading">
            <div>
              <p class="section-kicker">03 · 套装配方</p>
              <h2>六个位置怎么分配</h2>
            </div>
            <span
              class="miracle-total"
              :class="{ 'miracle-total--invalid': recipeSpecifiedTotal > 6 }"
            >
              {{ recipeTotal }} / 6 件
              <small v-if="recipeUnrestrictedCount > 0"
                >剩余 {{ recipeUnrestrictedCount }} 件不限套装</small
              >
            </span>
          </div>
          <div class="miracle-recipe-list">
            <div
              v-for="(line, index) in recipe"
              :key="index"
              class="miracle-recipe-row"
            >
              <label class="compact-field">
                <span>套装方向 {{ index + 1 }}</span>
                <select
                  v-model="line.category"
                  @change="handleRecipeCategoryChange(index)"
                >
                  <option
                    v-for="category in recipeCategories"
                    :key="category"
                    :value="category"
                  >
                    {{ recipeCategoryLabel(category) }}
                  </option>
                </select>
              </label>
              <label class="compact-field">
                <span>御魂</span>
                <SoulSetPicker
                  :model-value="line.setId"
                  :options="setsForCategory(line.category)"
                  :empty-value="
                    line.category === SCATTERED_SET_ID
                      ? SCATTERED_SET_ID
                      : UNRESTRICTED_SET_ID
                  "
                  :empty-label="
                    line.category === SCATTERED_SET_ID
                      ? '散件（不限制套装）'
                      : '不限定具体御魂（该方向全部可用）'
                  "
                  :show-categories="false"
                  @update:model-value="
                    line.setId = $event;
                    handleRecipeSetChange(index);
                  "
                />
              </label>
              <label class="compact-field miracle-count-field">
                <span>件数</span>
                <input
                  v-model.number="line.count"
                  type="number"
                  min="1"
                  max="6"
                  step="1"
                />
              </label>
              <button
                class="button button--quiet button--small"
                type="button"
                :disabled="recipe.length <= 1"
                @click="removeRecipeLine(index)"
              >
                移除
              </button>
            </div>
          </div>
          <div class="miracle-main-attributes">
            <div class="miracle-subheading">
              <strong>2 / 4 / 6 号位主属性</strong
              ><span>默认：2号位速度；每个号位支持多选</span>
            </div>
            <div class="miracle-main-attribute-grid">
              <div
                v-for="constraint in mainAttributes"
                :key="constraint.slot"
                class="miracle-main-attribute-picker"
              >
                <strong>{{ constraint.slot }}号位主属性</strong>
                <span class="miracle-main-attribute-hint">满足任一所选主属性</span>
                <div class="miracle-main-attribute-options">
                  <label
                    v-for="option in mainAttributeOptions[constraint.slot]"
                    :key="option.value"
                    class="miracle-main-attribute-option"
                  >
                    <input
                      v-model="constraint.mainAttrTypes"
                      type="checkbox"
                      :value="option.value"
                    />
                    <span>{{ option.label }}</span>
                  </label>
                </div>
                <small>{{
                  constraint.mainAttrTypes.length
                    ? `已选：${constraint.mainAttrTypes.map((value) => mainAttributeOptions[constraint.slot]?.find((option) => option.value === value)?.label ?? value).join("、")}`
                    : "未限制主属性"
                }}</small>
              </div>
            </div>
          </div>
          <div class="miracle-recipe-actions">
            <button
              class="button button--quiet button--small"
              type="button"
              @click="addRecipeLine"
            >
              + 添加一行
            </button>
            <span
              >具体御魂可不选，表示该方向全部可用；只指定 2 件或 4
              件时，剩余位置自动不限制套装。</span
            >
          </div>
          <div class="miracle-max-level-row">
            <label class="miracle-max-level-toggle">
              <input v-model="onlyMaxLevel" type="checkbox" />
              <span>仅满级</span>
            </label>
            <small>只计算六星 +15 御魂；关闭后会纳入六星未满级御魂。</small>
          </div>
        </div>

        <div class="miracle-submit-row">
          <p v-if="validationMessage" class="inline-error" role="alert">
            {{ validationMessage }}
          </p>
          <p v-else-if="errorMessage" class="inline-error" role="alert">
            {{ errorMessage }}
          </p>
          <span v-else class="miracle-submit-note"
            >已装备御魂默认允许使用；{{
              onlyMaxLevel
                ? "只评估六星 +15 且当前存在的御魂。"
                : "评估当前存在的全部六星御魂。"
            }}</span
          >
          <button
            class="button button--primary"
            type="button"
            :disabled="calculating"
            @click="calculate"
          >
            {{ calculating ? "正在寻找……" : "开始寻找配装" }}
          </button>
        </div>
      </section>

      <section v-if="result" class="miracle-results" aria-live="polite">
        <div
          class="miracle-result-banner"
          :class="`miracle-result-banner--${result.status}`"
        >
          <div>
            <p class="section-kicker">SEARCH RESULT</p>
            <h2>
              {{
                result.status === "matched"
                  ? "找到达标方案"
                  : result.status === "closest"
                    ? "先给你最接近的方案"
                    : "当前库存没有可拼出的六件组合"
              }}
            </h2>
            <p>
              {{
                result.status === "matched"
                  ? "以下方案满足所有已填写的最低面板。"
                  : result.status === "closest"
                    ? "方案按未达标属性数量和加权缺口排序，可从强化或购买方向继续补齐。"
                    : "可以先调整套装配方，或按下方购买建议补齐关键号位。"
              }}
            </p>
          </div>
        </div>

        <div v-if="result.qualityNotes.length" class="miracle-notice-list">
          <p v-for="note in result.qualityNotes" :key="note">{{ note }}</p>
        </div>

        <section
          v-for="(solution, solutionIndex) in displayedSolutions"
          :key="solutionIndex"
          class="surface-card miracle-solution-card"
        >
          <div class="miracle-solution-heading">
            <div>
              <p class="section-kicker">
                方案 {{ solutionIndex + 1
                }}<span v-if="solution.approximate"> · 近似</span>
              </p>
              <h2>{{ setCountText(solution) || "配方组合" }}</h2>
            </div>
            <div class="miracle-solution-state">
              <span
                v-if="solution.unmetAttributes.length"
                class="status-chip status-chip--warning"
                >未达标 {{ solution.unmetAttributes.length }} 项</span
              >
              <span v-else class="status-chip status-chip--success">全部达标</span>
            </div>
          </div>
          <div class="miracle-panel-strip">
            <div
              v-for="definition in targetDefinitions"
              :key="definition.key"
              class="miracle-panel-item"
            >
              <span>{{ definition.label }}</span>
              <strong>{{ panelValue(solution.panel, definition.key) }}</strong>
              <small
                :class="{
                  'miracle-panel-status--unmet': panelStatusIsUnmet(
                    solution,
                    definition.key,
                  ),
                }"
                >{{ panelStatusText(solution, definition.key) }}</small
              >
            </div>
          </div>
          <div v-if="solution.metrics.length" class="miracle-metric-result-list">
            <div
              v-for="metric in solution.metrics"
              :key="metric.presetId + metric.label"
              class="miracle-metric-result"
            >
              <span>{{ metric.label }}</span>
              <strong
                >{{ metric.expression }} =
                {{ metricNumberText(metric, metric.actual) }}</strong
              >
              <small :class="{ 'miracle-metric-status--unmet': !metric.met }">{{
                metricResultStatus(metric)
              }}</small>
              <div v-if="metric.constraints.length" class="miracle-metric-constraints">
                <span
                  v-for="constraint in metric.constraints"
                  :key="constraint.attribute"
                  :class="
                    constraint.met
                      ? 'miracle-metric-constraint--met'
                      : 'miracle-metric-constraint--unmet'
                  "
                >
                  {{ attributeLabel(constraint.attribute) }}
                  {{ metricConstraintText(constraint.attribute, constraint.actual) }} /
                  目标 ≥
                  {{ metricConstraintText(constraint.attribute, constraint.minimum) }}
                  <template v-if="constraint.maximum != null">
                    · ≤
                    {{ metricConstraintText(constraint.attribute, constraint.maximum) }}
                  </template>
                </span>
              </div>
            </div>
          </div>
          <div class="miracle-soul-table-wrap">
            <table class="miracle-soul-table">
              <thead>
                <tr>
                  <th>位置</th>
                  <th>套装</th>
                  <th>主属性</th>
                  <th>等级</th>
                  <th>副属性</th>
                  <th>状态</th>
                  <th>标准评分</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="soul in solution.souls" :key="soul.soulKey">
                  <td>
                    <strong>{{ positionText(soul.slot) }}</strong>
                  </td>
                  <td>{{ setLabel(soul.setId) }}</td>
                  <td>
                    <span class="miracle-main-attribute-value"
                      >{{ attributeLabel(soul.mainAttrType) }}
                      {{
                        formatSoulAttributeValue(soul.mainAttrType, soul.mainAttrValue)
                      }}</span
                    >
                    <span
                      v-for="attribute in fixedSoulAttributes(soul)"
                      :key="`${soul.soulKey}-fixed-${attribute.attributeIndex}`"
                      class="miracle-fixed-attribute"
                      >{{ attributeLabel(attribute.attributeType) }}
                      {{
                        formatSoulAttributeValue(
                          attribute.attributeType,
                          attribute.value,
                        )
                      }}
                      <em>固有</em></span
                    >
                  </td>
                  <td>+{{ soul.level }}</td>
                  <td>
                    <span
                      v-for="attribute in subSoulAttributes(soul)"
                      :key="`${soul.soulKey}-${attribute.attributeIndex}`"
                      class="miracle-substat"
                      ><span
                        >{{ attributeLabel(attribute.attributeType) }}
                        {{
                          formatSoulAttributeValue(
                            attribute.attributeType,
                            attribute.value,
                          )
                        }}</span
                      ><b
                        v-if="attribute.enhancementCount !== null"
                        class="miracle-substat-roll"
                        >+{{ attribute.enhancementCount }}</b
                      ><b
                        v-else
                        class="miracle-substat-roll miracle-substat-roll--unknown"
                        >?</b
                      ></span
                    ><span v-if="!subSoulAttributes(soul).length" class="muted-text"
                      >暂无副属性</span
                    >
                  </td>
                  <td>{{ soul.equippedState || "未装备" }}</td>
                  <td>
                    <span
                      class="miracle-soul-score"
                      :class="
                        `miracle-soul-score--${scoreTone(
                          scoreBySoulKey.get(soul.soulKey)?.standardScore ?? null,
                        )}`
                      "
                      :title="standardScoreTooltip(soul)"
                    >
                      <strong>{{
                        formatStandardScore(
                          scoreBySoulKey.get(soul.soulKey)?.standardScore ?? null,
                        )
                      }}</strong>
                      <small>{{
                        scoreBySoulKey.get(soul.soulKey)?.standardScoreRule ??
                          "尚未计算"
                      }}</small>
                    </span>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
          <div v-if="solution.uncountedSetEffects.length" class="miracle-effects">
            <p class="section-kicker">套装效果提示</p>
            <span
              v-for="effect in solution.uncountedSetEffects"
              :key="`${effect.setId}-${effect.kind}`"
              >{{ effect.setName }} {{ effect.pieceCount }}件：{{
                effect.effect || "已激活但未计入静态面板"
              }}</span
            >
          </div>
        </section>

        <div v-if="result.status !== 'matched'" class="miracle-advice-grid">
          <section class="surface-card miracle-advice-card">
            <p class="section-kicker">SLOT GAPS</p>
            <h2>最该换哪个位置</h2>
            <div v-if="result.slotGaps.length" class="miracle-advice-list">
              <div
                v-for="gap in result.slotGaps"
                :key="gap.slot"
                class="miracle-advice-row"
              >
                <strong>{{ positionText(gap.slot) }}</strong
                ><span
                  >{{ gap.reason
                  }}<small>{{
                    gap.missingAttributes.map(attributeLabel).join("、") || "已接近"
                  }}</small></span
                >
              </div>
            </div>
            <p v-else class="empty-state">当前没有可解释的号位缺口。</p>
          </section>
          <section class="surface-card miracle-advice-card">
            <p class="section-kicker">PURCHASE DIRECTION</p>
            <h2>可以购买什么方向</h2>
            <div v-if="result.purchaseSuggestions.length" class="miracle-advice-list">
              <div
                v-for="suggestion in result.purchaseSuggestions"
                :key="`${suggestion.slot}-${suggestion.missingAttribute}`"
                class="miracle-advice-row"
              >
                <strong>{{ positionText(suggestion.slot) }}</strong
                ><span
                  >{{ suggestion.setDirection }} · {{ suggestion.mainAttribute
                  }}<small
                    >优先：{{ suggestion.prioritySubAttributes.join("、") }} ·
                    {{ suggestion.reason }}</small
                  ></span
                >
              </div>
            </div>
            <p v-else class="empty-state">当前库存暂时不需要额外购买建议。</p>
          </section>
        </div>
        <section
          v-if="result.status !== 'matched'"
          class="surface-card miracle-enhancement-card"
        >
          <p class="section-kicker">ENHANCEMENT</p>
          <h2>可以先强化的胚子</h2>
          <div
            v-if="displayedEnhancementCandidates.length"
            class="miracle-enhancement-list"
          >
            <article
              v-for="(candidate, candidateIndex) in displayedEnhancementCandidates"
              :key="candidate.soul.soulKey"
              class="miracle-enhancement-candidate"
            >
              <div class="miracle-enhancement-heading">
                <div>
                  <strong
                    >推荐{{ candidateIndex + 1 }} · 替换{{
                      positionText(candidate.slot)
                    }}
                    · {{ setLabel(candidate.soul.setId) }}</strong
                  >
                  <span
                    >六星 · 等级 {{ candidate.soul.level }} ·
                    {{ candidate.soul.equippedState || "未装备" }} · 初始条数
                    {{ initialSubstatText(candidate.soul) }}</span
                  >
                </div>
                <span class="miracle-enhancement-gain"
                  >理论 {{ numberText(candidate.possibleImprovement) }} · 一般
                  {{ numberText(candidate.expectedImprovement) }}</span
                >
              </div>

              <div class="miracle-soul-compare">
                <div
                  class="miracle-soul-compare-panel miracle-soul-compare-panel--original"
                >
                  <div class="miracle-soul-compare-heading">
                    <b>原始御魂 · 当前方案</b>
                    <span
                      >{{ positionText(candidate.originalSoul.slot) }} ·
                      {{ setLabel(candidate.originalSoul.setId) }} · 等级
                      {{ candidate.originalSoul.level }} ·
                      {{ candidate.originalSoul.equippedState || "未装备" }}</span
                    >
                  </div>
                  <span
                    ><b>主属性</b
                    >{{ attributeLabel(candidate.originalSoul.mainAttrType) }}
                    {{
                      formatSoulAttributeValue(
                        candidate.originalSoul.mainAttrType,
                        candidate.originalSoul.mainAttrValue,
                      )
                    }}</span
                  >
                  <span
                    ><b>副属性</b>{{ soulSubstatSummary(candidate.originalSoul) }}</span
                  >
                  <span
                    ><b>固有属性</b
                    >{{ soulFixedAttributeSummary(candidate.originalSoul) }}</span
                  >
                </div>
                <div
                  class="miracle-soul-compare-panel miracle-soul-compare-panel--candidate"
                >
                  <div class="miracle-soul-compare-heading">
                    <b>推荐替换胚子 · 目标方案</b>
                    <span
                      >{{ positionText(candidate.soul.slot) }} ·
                      {{ setLabel(candidate.soul.setId) }} · 等级
                      {{ candidate.soul.level }} ·
                      {{ candidate.soul.equippedState || "未装备" }}</span
                    >
                  </div>
                  <span
                    ><b>主属性</b>{{ attributeLabel(candidate.soul.mainAttrType) }}
                    {{
                      formatSoulAttributeValue(
                        candidate.soul.mainAttrType,
                        candidate.soul.mainAttrValue,
                      )
                    }}</span
                  >
                  <span><b>副属性</b>{{ soulSubstatSummary(candidate.soul) }}</span>
                  <span
                    ><b>固有属性</b
                    >{{ soulFixedAttributeSummary(candidate.soul) }}</span
                  >
                </div>
              </div>

              <p class="miracle-subheading miracle-enhancement-section-label">
                副属性成长对比
              </p>
              <div class="miracle-enhancement-attribute-grid">
                <div class="miracle-attribute-panel miracle-attribute-panel--best">
                  <div class="miracle-attribute-heading">
                    <h3>最佳属性</h3>
                    <span>理论上限，仅用于判断潜力</span>
                  </div>
                  <div class="miracle-roll-list">
                    <span
                      v-for="attribute in candidate.theoreticalAttributes"
                      :key="`${candidate.soul.soulKey}-best-${attribute.attributeIndex}-${attribute.attributeType}`"
                      class="miracle-roll"
                      :class="{
                        'miracle-roll--theoretical-best': theoreticalAttributeIsBest(
                          attribute,
                          candidate.theoreticalAttributes,
                        ),
                        'miracle-roll--new': attribute.isNewAttribute,
                      }"
                    >
                      <strong
                        >{{ attributeLabel(attribute.attributeType) }}
                        {{
                          formatSoulAttributeValue(
                            attribute.attributeType,
                            attribute.expectedValue,
                          )
                        }}</strong
                      >
                      <em>{{
                        theoreticalAttributeStatusText(
                          attribute,
                          candidate.theoreticalAttributes,
                        )
                      }}</em>
                    </span>
                    <span
                      v-if="!candidate.theoreticalAttributes.length"
                      class="muted-text"
                      >没有可计算的理论强化属性</span
                    >
                  </div>
                </div>
                <div class="miracle-attribute-panel miracle-attribute-panel--normal">
                  <div class="miracle-attribute-heading">
                    <h3>一般属性</h3>
                    <span>六星成长区间中位值</span>
                  </div>
                  <div class="miracle-roll-list">
                    <span
                      v-for="attribute in candidate.expectedAttributes"
                      :key="`${candidate.soul.soulKey}-normal-${attribute.attributeIndex}-${attribute.attributeType}`"
                      class="miracle-roll"
                      :class="{
                        'miracle-roll--changed': expectedAttributeChanged(attribute),
                        'miracle-roll--new': attribute.isNewAttribute,
                      }"
                    >
                      <strong
                        >{{ attributeLabel(attribute.attributeType) }}
                        {{
                          formatSoulAttributeValue(
                            attribute.attributeType,
                            attribute.expectedValue,
                          )
                        }}</strong
                      >
                      <em
                        >{{
                          expectedEnhancementCountText(
                            attribute.expectedEnhancementCount,
                          )
                        }}{{
                          attribute.isNewAttribute
                            ? ` · 新增${expectedAttributeProbabilityText(attribute)}`
                            : ""
                        }}</em
                      >
                    </span>
                    <span v-if="!candidate.expectedAttributes.length" class="muted-text"
                      >没有可计算的一般强化属性</span
                    >
                  </div>
                </div>
              </div>

              <p class="miracle-subheading miracle-enhancement-section-label">
                替换后的式神面板
              </p>
              <div class="miracle-panel-grid">
                <div class="miracle-enhancement-panel-card">
                  <div class="miracle-attribute-heading">
                    <h3>最佳属性替换后的式神属性</h3>
                    <span>六件御魂重新计算 · 变化项高亮</span>
                  </div>
                  <div class="miracle-enhancement-stat-list">
                    <div
                      v-for="definition in targetDefinitions"
                      :key="`${candidate.soul.soulKey}-best-panel-${definition.key}`"
                      class="miracle-enhancement-stat"
                      :class="{
                        'miracle-enhancement-stat--changed': panelStatChanged(
                          candidate,
                          definition.key,
                          candidate.theoreticalPanel,
                        ),
                      }"
                    >
                      <span>{{ definition.label }}</span>
                      <strong>{{
                        panelValue(candidate.theoreticalPanel, definition.key)
                      }}</strong>
                      <em>{{
                        panelDeltaText(
                          candidate,
                          definition.key,
                          candidate.theoreticalPanel,
                        )
                      }}</em>
                    </div>
                  </div>
                </div>
                <div
                  class="miracle-enhancement-panel-card miracle-enhancement-panel-card--normal"
                >
                  <div class="miracle-attribute-heading">
                    <h3>一般属性替换后的式神属性</h3>
                    <span>六件御魂重新计算</span>
                  </div>
                  <div class="miracle-enhancement-stat-list">
                    <div
                      v-for="definition in targetDefinitions"
                      :key="`${candidate.soul.soulKey}-expected-panel-${definition.key}`"
                      class="miracle-enhancement-stat"
                    >
                      <span>{{ definition.label }}</span
                      ><strong>{{
                        panelValue(candidate.expectedPanel, definition.key)
                      }}</strong
                      ><em>{{
                        panelDeltaText(
                          candidate,
                          definition.key,
                          candidate.expectedPanel,
                        )
                      }}</em>
                    </div>
                  </div>
                </div>
              </div>

              <div class="miracle-enhancement-explanation">
                <div class="miracle-explanation-row">
                  <b>为什么排在第一</b><span>{{ candidate.rankExplanation }}</span>
                </div>
                <div class="miracle-explanation-row">
                  <b>为什么选择它</b
                  ><span
                    >{{ candidate.reason }}
                    {{ enhancementPriorityText(candidate) }}</span
                  >
                </div>
                <div class="miracle-explanation-row">
                  <b>强化前到理论上限</b
                  ><span
                    >{{
                      enhancementPanelChanges(candidate).join("；") ||
                      "没有可计算的理论面板增益"
                    }}；理论值不表示成功概率。</span
                  >
                </div>
                <div class="miracle-explanation-row">
                  <b>强化前到一般收益</b
                  ><span
                    >{{
                      enhancementExpectedChanges(candidate).join("；") ||
                      "新增属性命中有效方向的平均收益不足"
                    }}；使用六星成长区间中位值估算。</span
                  >
                </div>
                <div class="miracle-explanation-row">
                  <b>指标收益</b
                  ><span>{{
                    metricExplanationText(candidate).join("；") ||
                    enhancementGapChanges(candidate).join("；") ||
                    "当前没有组合指标收益变化"
                  }}</span>
                </div>
                <small class="miracle-enhancement-footnote"
                  >加权缺口：{{ numberText(candidate.currentGapScore) }} → 理论
                  {{ numberText(candidate.theoreticalGapScore) }} · 一般
                  {{
                    numberText(
                      candidate.currentGapScore - candidate.expectedImprovement,
                    )
                  }}；理论上限是假设剩余节点命中最有利属性，一般收益按新增属性类别概率和后续四腿均匀命中计算。</small
                >
              </div>
            </article>
          </div>
          <p v-else class="empty-state">当前组合没有满足条件的可强化六星胚子。</p>
        </section>
      </section>
    </template>
  </main>
</template>
