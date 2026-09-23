<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { desktopApi } from "../api/client";
import type {
  CatalogStatus,
  MySoul,
  MySoulFilterOptions,
  MySoulScore,
  ScoreColorThresholds,
  StandardScoreContribution,
  SoulSet,
  TaskProgress,
} from "../api/contracts";
import { normalizeCommandError } from "../api/errors";
import MySoulRadarView from "./MySoulRadarView.vue";
import SoulSetPicker from "./SoulSetPicker.vue";
import SpeedSoulView from "./SpeedSoulView.vue";
import PvePanelView from "./PvePanelView.vue";
import HeadTailAnalysisPrototype from "./HeadTailAnalysisPrototype.vue";
import Plus15GrowthQualityView from "./Plus15GrowthQualityView.vue";
import FourLegEmbryoDecisionView from "./FourLegEmbryoDecisionView.vue";
import CharacterArchiveSwitcher from "./CharacterArchiveSwitcher.vue";
import { onActiveCharacterChanged } from "../state/activeCharacter";

type AttributeOperator = "gt" | "lt";

type SoulFilters = {
  setId: string;
  slot: string;
  quality: string;
  level: string;
  mainAttrType: string;
  subAttrType: string;
  attributeType: string;
  attributeOperator: AttributeOperator;
  attributeValue: string;
  standardScoreMin: string;
  standardScoreMax: string;
  auspiciousOnly: boolean;
};

// 图标与御魂档案共用同一份离线资源，套装关联只依赖目录稳定编号，不依赖显示名称。
const iconModules = import.meta.glob("../assets/soul-icons/*.png", {
  eager: true,
  import: "default",
  query: "?url",
});

// 与御魂档案保持同一目录门槛，保证“我的御魂”显示的套装名称和图标来自同一版本。
const EXPECTED_CATALOG_SET_COUNT = 70;
// 首屏和翻页都只请求 10 枚，避免把整份御魂属性正文一次搬进 WebView。
const PAGE_SIZE = 10;

// 主属性按游戏号位规则固定展示：固定位置使用攻击/防御/生命，二四六号位使用对应的百分比或特殊属性。
// 即使当前采集批次暂时没有某类属性，也保留筛选项，避免“没有采到”被误认为“游戏不存在”。
const MAIN_ATTRIBUTE_ORDER = [
  "attack_flat",
  "defense_flat",
  "hp_flat",
  "attack_rate",
  "defense_rate",
  "hp_rate",
  "speed",
  "effect_hit",
  "effect_resist",
  "crit_rate",
  "crit_damage",
] as const;

// 比例属性在数据库中按 0~1 保存，但筛选输入沿用列表展示习惯按百分数填写。
const PERCENTAGE_ATTRIBUTE_TYPES = new Set([
  "attack_rate",
  "hp_rate",
  "defense_rate",
  "crit_rate",
  "crit_damage",
  "effect_hit",
  "effect_resist",
]);

// 这些属性是输出御魂最关注的强化目标；达到满 5 手时才显示喜庆的“大吉”标识。
const AUSPICIOUS_ATTRIBUTE_TYPES = new Set([
  "crit_rate",
  "crit_damage",
  "effect_hit",
  "speed",
  "effect_resist",
]);

const catalogStatus = ref<CatalogStatus | null>(null);
const sets = ref<SoulSet[]>([]);
const souls = ref<MySoul[]>([]);
const inventoryTotal = ref(0);
const soulTotal = ref(0);
// 副属性数值筛选按批次追加结果；只有后端确认还有下一批时才显示“加载更多”。
const soulPageHasMore = ref(false);
// 当前库存的来源只在标题旁展示一次；来源按库存整体汇总，不随分页变化。
const sourceKinds = ref<string[]>([]);
const filterOptions = ref<MySoulFilterOptions>({
  setIds: [],
  slots: [],
  qualities: [],
  levels: [],
  mainAttrTypes: [],
  subAttrTypes: [],
});
const scoreSummaries = ref<MySoulScore[]>([]);
const scoreCount = ref(0);
// 当前全局启用的评分标准名称；评分尚未开始时也要明确告诉用户将使用哪套标准。
const activeScoreStandardTitle = ref<string | null>(null);
// 当前启用标准的规范正文哈希；同名标准保存后据此识别页面中的历史旧评分。
const activeScoreStandardHash = ref<string | null>(null);
// 当前启用评分标准的颜色阈值；读取失败时沿用兼容旧标准的默认值。
const scoreColorThresholds = ref<ScoreColorThresholds>({
  jadeScore: 60,
  goldScore: 70,
  rainbowScore: 75,
});
// 评分任务必须复用库存分页接口返回的实际档案 ID，不能从档案列表顺序推测。
const selectedProfileId = ref<string | null>(null);
// 筛选条件全部采用稳定字段；多个下拉条件同时存在时按 AND 关系收窄结果。
const filters = ref<SoulFilters>({
  setId: "",
  slot: "",
  quality: "",
  level: "",
  mainAttrType: "",
  subAttrType: "",
  attributeType: "",
  attributeOperator: "gt",
  attributeValue: "",
  standardScoreMin: "",
  standardScoreMax: "",
  auspiciousOnly: false,
});
// “我的御魂”可见页签顺序固定为库存、一速御魂、PVE 面板、头尾分析和御魂雷达。
const activeSubPage = ref<
  | "inventory"
  | "growth-quality"
  | "embryo-decision"
  | "radar"
  | "speed"
  | "pve"
  | "head-tail"
>("inventory");
const page = ref(1);
const loading = ref(true);
const error = ref("");
const scoreBusy = ref(false);
const scoreTaskId = ref<string | null>(null);
const scoreProgress = ref<TaskProgress | null>(null);
const scoreError = ref("");
// 评分事务已提交但当前页还在重新读取时保持独立状态，避免用户误以为结果没有保存。
const scoreRefreshing = ref(false);
const soulPageLoading = ref(false);
// 问号说明只控制评分表头的轻量浮层，不影响评分任务和分页请求。
const standardScoreHelpOpen = ref(false);
// 只记录一枚御魂的展开状态，避免同时展开多行导致表格高度失控和信息难以扫描。
const expandedSoulKey = ref<string | null>(null);
// 悬浮评分详情使用 fixed 定位，避免被分页表格的横向滚动容器裁剪。
const hoveredScoreSoulKey = ref<string | null>(null);
const scoreTooltipPosition = ref({ top: 16, left: 16 });
let stopTaskProgress: (() => void) | null = null;
let stopActiveCharacterChanged: (() => void) | null = null;
let soulPageRequestId = 0;
let soulFilterReloadTimer: number | null = null;

const setById = computed(() => new Map(sets.value.map((set) => [set.setId, set])));
const matchedSoulCount = computed(
  () => souls.value.filter((soul) => setById.value.has(soul.setId)).length,
);
const unmatchedSoulCount = computed(() => souls.value.length - matchedSoulCount.value);

const setOptions = computed(() => {
  // 二级御魂选择器展示的是完整目录，不依赖当前库存；没有库存时仍可选择套装并看到 0 条结果。
  return sets.value
    .map((set) => ({
      setId: set.setId,
      name: set.name,
      iconAssetId: set.iconAssetId,
      category: set.category,
      specialCategory: set.specialCategory,
    }))
    .sort((left, right) => left.name.localeCompare(right.name, "zh-CN"));
});
// 一级分类只负责缩小第二级御魂候选范围；具体库存查询仍由 setId 作为唯一套装条件提交。
const selectedSetCategory = ref("all");
const setCategories = computed(() => {
  const categories = new Set<string>();
  // 一级分类来自完整御魂目录，而不是当前库存；即使当前库存为空或筛选选项尚未返回，分类也必须可见。
  for (const set of sets.value) {
    const category = set.specialCategory ?? set.category;
    if (category) categories.add(category);
  }
  const priority = [
    "攻击加成",
    "生命加成",
    "防御加成",
    "暴击",
    "暴击伤害",
    "效果命中",
    "效果抵抗",
    "首领御魂",
    "星痕御魂",
  ];
  return [
    { key: "all", label: "全部效果" },
    ...Array.from(categories)
      .sort(
        (left, right) =>
          (priority.indexOf(left) < 0 ? priority.length : priority.indexOf(left)) -
            (priority.indexOf(right) < 0 ? priority.length : priority.indexOf(right)) ||
          left.localeCompare(right, "zh-CN"),
      )
      .map((category) => ({ key: category, label: category })),
  ];
});
const filteredSetOptions = computed(() => {
  if (selectedSetCategory.value === "all") return setOptions.value;
  return setOptions.value.filter(
    (option) =>
      (option.specialCategory ?? option.category) === selectedSetCategory.value,
  );
});

/** 切换一级分类时清理已经不属于该分类的二级套装，避免查询条件与界面显示不一致。 */
function handleSetCategoryChange(): void {
  if (
    filters.value.setId &&
    !filteredSetOptions.value.some((option) => option.setId === filters.value.setId)
  ) {
    filters.value.setId = "";
  }
}

/** 选择具体御魂时回写一级套装效果，保证两个控件始终描述同一套装。 */
function handleSetSelectionChange(setId: string): void {
  filters.value.setId = setId;
  if (!setId) return;
  const selected = setById.value.get(setId);
  const category = selected?.specialCategory ?? selected?.category;
  if (category) selectedSetCategory.value = category;
}
const slotOptions = computed(() =>
  [...filterOptions.value.slots].sort((left, right) => left - right),
);
const qualityOptions = computed(() =>
  [...filterOptions.value.qualities].sort((left, right) => right - left),
);
const levelOptions = computed(() =>
  [...filterOptions.value.levels].sort((left, right) => right - left),
);
const mainAttributeOptions = computed(() => {
  // 将固定规则和实际数据合并；未知类型排在规则类型之后，避免丢失旧数据的可见性。
  return [
    ...new Set([...MAIN_ATTRIBUTE_ORDER, ...filterOptions.value.mainAttrTypes]),
  ].sort((left, right) => {
    const leftIndex = MAIN_ATTRIBUTE_ORDER.indexOf(
      left as (typeof MAIN_ATTRIBUTE_ORDER)[number],
    );
    const rightIndex = MAIN_ATTRIBUTE_ORDER.indexOf(
      right as (typeof MAIN_ATTRIBUTE_ORDER)[number],
    );
    if (leftIndex !== -1 || rightIndex !== -1) {
      return (
        (leftIndex === -1 ? MAIN_ATTRIBUTE_ORDER.length : leftIndex) -
        (rightIndex === -1 ? MAIN_ATTRIBUTE_ORDER.length : rightIndex)
      );
    }
    return attributeLabel(left).localeCompare(attributeLabel(right), "zh-CN");
  });
});
const subAttributeOptions = computed(() =>
  [...filterOptions.value.subAttrTypes].sort((left, right) =>
    attributeLabel(left).localeCompare(attributeLabel(right), "zh-CN"),
  ),
);
const attributeValueOptions = computed(() => {
  // 选项保留完整的可出现属性类型，但查询只检查 fixedAttribute=false 的副属性事实。
  return [...new Set([...MAIN_ATTRIBUTE_ORDER, ...filterOptions.value.subAttrTypes])].sort(
    (left, right) => attributeLabel(left).localeCompare(attributeLabel(right), "zh-CN"),
  );
});
const attributeValueUnit = computed(() =>
  PERCENTAGE_ATTRIBUTE_TYPES.has(filters.value.attributeType) ? "%" : "点",
);
const attributeValuePlaceholder = computed(() => "例如 10");
const hasAttributeValueFilter = computed(
  () =>
    Boolean(filters.value.attributeType && filters.value.attributeValue !== "") &&
    attributeFilterStoredValue(
      filters.value.attributeType,
      filters.value.attributeValue,
    ) !== null,
);
const hasFilters = computed(() =>
  Boolean(
    filters.value.setId ||
      filters.value.slot ||
      filters.value.quality ||
      filters.value.level ||
      filters.value.mainAttrType ||
      filters.value.subAttrType ||
      filters.value.attributeType ||
      filters.value.attributeValue ||
      filters.value.standardScoreMin ||
      filters.value.standardScoreMax ||
      filters.value.auspiciousOnly ||
      selectedSetCategory.value !== "all",
  ),
);

/** 把界面输入的百分数转换为数据库和属性事实使用的原始数值。 */
function attributeFilterStoredValue(
  attributeType: string,
  input: string,
): number | null {
  const value = Number(input);
  if (!Number.isFinite(value)) return null;
  return PERCENTAGE_ATTRIBUTE_TYPES.has(attributeType) ? value / 100 : value;
}

/** 按筛选条件比较单个属性值；比较运算符只允许界面提供的大于/小于。 */
function matchesAttributeValue(
  value: number,
  threshold: number,
  operator: string,
): boolean {
  return operator === "lt" ? value < threshold : value > threshold;
}

const filteredSouls = computed(() => {
  const activeFilters = filters.value;

  return souls.value.filter((soul) => {
    if (activeFilters.setId && soul.setId !== activeFilters.setId) return false;
    if (activeFilters.slot && soul.slot !== Number(activeFilters.slot)) return false;
    if (activeFilters.quality && soul.quality !== Number(activeFilters.quality))
      return false;
    // 等级 0 是有效筛选值，不能用 truthy 判断，否则 "0" 会被当成未选择。
    if (activeFilters.level !== "" && soul.level !== Number(activeFilters.level)) {
      return false;
    }
    if (
      activeFilters.mainAttrType &&
      soul.mainAttrType !== activeFilters.mainAttrType
    ) {
      return false;
    }
    if (
      activeFilters.subAttrType &&
      !subAttributes(soul).some(
        (attribute) => attribute.attributeType === activeFilters.subAttrType,
      )
    ) {
      return false;
    }
    if (activeFilters.attributeType && activeFilters.attributeValue !== "") {
      const threshold = attributeFilterStoredValue(
        activeFilters.attributeType,
        activeFilters.attributeValue,
      );
      if (
        threshold === null ||
        !subAttributes(soul).some(
          (attribute) =>
            attribute.attributeType === activeFilters.attributeType &&
            matchesAttributeValue(
              attribute.value,
              threshold,
              activeFilters.attributeOperator,
            ),
        )
      ) {
        return false;
      }
    }
    // 分数和大吉已经由数据库分页筛选；这里仅保留当前页的防御性检查，避免旧响应短暂混入。
    const summary = scoreBySoulKey.value.get(soul.soulKey);
    if (
      activeFilters.standardScoreMin !== "" &&
      (summary?.standardScore === null ||
        summary?.standardScore === undefined ||
        summary.standardScore <= Number(activeFilters.standardScoreMin))
    ) {
      return false;
    }
    if (
      activeFilters.standardScoreMax !== "" &&
      (summary?.standardScore === null ||
        summary?.standardScore === undefined ||
        summary.standardScore >= Number(activeFilters.standardScoreMax))
    ) {
      return false;
    }
    if (activeFilters.auspiciousOnly && !isAuspiciousSoul(soul)) return false;
    return true;
  });
});

const pageCount = computed(() => Math.max(1, Math.ceil(soulTotal.value / PAGE_SIZE)));
const visibleSouls = computed(() => {
  // 后端已经完成全量筛选，这里只对当前 10 条做一次防御性校验，避免旧数据混入页面。
  return filteredSouls.value;
});
const rangeLabel = computed(() => {
  if (!soulTotal.value) return "0 / 0";
  if (hasAttributeValueFilter.value) return `已加载 ${souls.value.length} 条`;
  const start = (Math.min(page.value, pageCount.value) - 1) * PAGE_SIZE + 1;
  const end = Math.min(start + souls.value.length - 1, soulTotal.value);
  return String(start) + "–" + String(end) + " / " + String(soulTotal.value);
});

const catalogHealthLabel = computed(() => {
  if (!inventoryTotal.value) return "暂无导入数据";
  return String(inventoryTotal.value) + " 枚已导入";
});

const scoreBySoulKey = computed(
  () => new Map(scoreSummaries.value.map((summary) => [summary.soulKey, summary])),
);
const hoveredScoreSummary = computed(() =>
  hoveredScoreSoulKey.value
    ? (scoreBySoulKey.value.get(hoveredScoreSoulKey.value) ?? null)
    : null,
);
const scoreTooltipStyle = computed(() => ({
  top: `${scoreTooltipPosition.value.top}px`,
  left: `${scoreTooltipPosition.value.left}px`,
}));
const scoredSoulCount = computed(() => scoreCount.value);
const scoreCoveragePercent = computed(() =>
  inventoryTotal.value
    ? Math.round((scoredSoulCount.value / inventoryTotal.value) * 100)
    : 0,
);
// 当前页任一评分哈希与启用标准不同，就说明保存标准后尚未完成新一轮计算。
const hasStaleScoreStandard = computed(() => {
  if (!activeScoreStandardHash.value || scoreCount.value === 0) return false;
  return scoreSummaries.value.some(
    (summary) =>
      Boolean(summary.standardHash) &&
      summary.standardHash !== activeScoreStandardHash.value,
  );
});
// 库存尚未评分或仍保存旧标准结果时邀请用户计算；进入页面本身始终保持只读。
const shouldPromptScoreCalculation = computed(
  () =>
    inventoryTotal.value > 0 &&
    (scoreCount.value === 0 || hasStaleScoreStandard.value) &&
    !scoreBusy.value &&
    !scoreRefreshing.value,
);
const scorePercent = computed(() => {
  const progress = scoreProgress.value;
  if (!progress) return 0;
  if (progress.total <= 0) return 100;
  return Math.min(100, Math.round((progress.completed / progress.total) * 100));
});
const activeScoreStandardLabel = computed(() => {
  const standard = scoreSummaries.value.find((summary) => summary.standardTitle);
  return activeScoreStandardTitle.value ?? standard?.standardTitle ?? "当前评分标准";
});
const scoreStatusLabel = computed(() => {
  if (scoreBusy.value) return "正在按当前评分标准计算";
  if (scoreRefreshing.value) return "评分已保存，正在刷新当前页";
  return {
    running: "正在按当前评分标准计算",
    completed: "评分结果已保存",
    cancelled: "评分已停止，未提交本批结果",
    failed: "评分计算失败",
  }[scoreProgress.value?.status ?? "completed"];
});

/** 将稳定属性编号转换成玩家能直接辨认的名称；未知属性保留原值，避免信息丢失。 */
function attributeLabel(attributeType: string): string {
  const labels: Record<string, string> = {
    attack_rate: "攻击加成",
    attack_flat: "攻击",
    hp_rate: "生命加成",
    hp_flat: "生命",
    defense_rate: "防御加成",
    defense_flat: "防御",
    speed: "速度",
    crit_rate: "暴击",
    crit_damage: "暴伤",
    effect_hit: "效果命中",
    effect_resist: "效果抵抗",
  };
  return labels[attributeType] ?? attributeType;
}

/** 将后端来源枚举映射为库存列表中的短标签；未知旧数据保留来源值，避免丢失可追溯性。 */
function sourceKindLabel(sourceKind: string): string {
  if (sourceKind === "mumu") return "MuMu";
  if (sourceKind === "desktop") return "桌面版";
  if (sourceKind === "cbg") return "藏宝阁";
  if (sourceKind === "native") return "内置";
  if (sourceKind === "importer") return "旧导入";
  return sourceKind || "未知来源";
}

/** 将库存来源转换成标题旁的徽标文本；未知来源保留为可追溯标签。 */
function sourceKindLabels(): string[] {
  return sourceKinds.value.map(sourceKindLabel);
}

/** 规范化数据中的比例属性以百分比显示，平坦属性按整数或小数保留原始精度。 */
function formatAttributeValue(attributeType: string, value: number): string {
  if (PERCENTAGE_ATTRIBUTE_TYPES.has(attributeType)) {
    const percentage = value * 100;
    return (
      String(Number.isInteger(percentage) ? percentage : percentage.toFixed(2)) + "%"
    );
  }
  return Number.isInteger(value) ? String(value) : value.toFixed(2);
}

/** 根据目录套装返回离线图标；没有覆盖的套装用文字印章代替。 */
function soulIcon(set: SoulSet | undefined): string | null {
  if (!set?.iconAssetId) return null;
  return iconModules["../assets/soul-icons/" + set.iconAssetId + ".png"] ?? null;
}

/** 将评分摘要的状态映射为列表中稳定的颜色层级，分数仍只在同一标准内比较。 */
function scoreTone(score: number | null): string {
  if (score === null) return "unknown";
  if (score >= scoreColorThresholds.value.rainbowScore) return "legendary";
  if (score >= scoreColorThresholds.value.goldScore) return "epic";
  if (score >= scoreColorThresholds.value.jadeScore) return "high";
  if (score >= 50) return "medium";
  return "low";
}

/** 方案评分统一保留一位小数，避免整数分数掩盖规则归一化产生的小数差异。 */
function formatScore(score: number | null): string {
  return score === null ? "—" : score.toFixed(1);
}

/** 评分详情中的数字统一保持紧凑格式，避免悬浮卡片出现无意义的长小数。 */
function formatScoreDetailNumber(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(2);
}

/** 将结构化属性贡献还原为用户熟悉的加法公式。 */
function formatScoreContribution(contribution: StandardScoreContribution): string {
  return `${contribution.attributeName}：${formatScoreDetailNumber(contribution.baseWeight)} + ${contribution.enhancementCount}×${formatScoreDetailNumber(contribution.enhancementWeight)} = ${formatScoreDetailNumber(contribution.contribution)}`;
}

/** 从兼容旧数据的公式文本中提取权重说明；新版数据优先使用结构化属性贡献。 */
function standardScoreRuleNote(summary: MySoulScore): string | null {
  if (!summary.standardScoreFormula) return null;
  const contributionCount = summary.standardScoreContributions.length;
  const parts = summary.standardScoreFormula.split("；");
  return parts.slice(contributionCount).join("；") || null;
}

/** 打开固定定位的评分详情卡片，让低分到高分的色阶和核心属性标识都可直接核对。 */
function showScoreTooltip(soulKey: string, event: MouseEvent | FocusEvent): void {
  const target = event.currentTarget;
  if (!(target instanceof HTMLElement)) return;
  const rect = target.getBoundingClientRect();
  const tooltipWidth = 330;
  const tooltipHeight = 230;
  scoreTooltipPosition.value = {
    top: Math.max(
      12,
      Math.min(window.innerHeight - tooltipHeight - 12, rect.bottom + 8),
    ),
    left: Math.max(
      12,
      Math.min(window.innerWidth - tooltipWidth - 12, rect.left - tooltipWidth / 2),
    ),
  };
  hoveredScoreSoulKey.value = soulKey;
}

/** 离开评分单元格或失去键盘焦点时关闭悬浮详情，避免遮挡后续列表操作。 */
function hideScoreTooltip(): void {
  hoveredScoreSoulKey.value = null;
}

/** 组合标准评分的悬浮说明，保留规则卡片和逐属性公式，方便核对最终得分来源。 */
function standardScoreTooltip(soul: MySoul): string {
  const summary = scoreBySoulKey.value.get(soul.soulKey);
  if (!summary) return "标准评分尚未计算";

  return [
    `标准评分：${formatScore(summary.standardScore)}`,
    summary.standardScoreRule
      ? `规则卡片：${summary.standardScoreRule}`
      : "规则卡片：尚未命中",
    summary.standardScoreFormula ?? "计算公式：尚未生成",
  ].join("\n");
}

/** 切换单行评分详情；详情只展开当前页数据，不额外读取整份库存。 */
function toggleSoulDetails(soulKey: string): void {
  expandedSoulKey.value = expandedSoulKey.value === soulKey ? null : soulKey;
}

/** 将固有属性与可强化副属性拆开，表格两列分别对应参考页面的属性分组。 */
function fixedAttributes(soul: MySoul): MySoul["attributes"] {
  return soul.attributes.filter((attribute) => attribute.fixedAttribute);
}

/** 只返回可强化副属性；没有解析到的副属性仍在详情中保留来源提示。 */
function subAttributes(soul: MySoul): MySoul["attributes"] {
  return soul.attributes.filter((attribute) => !attribute.fixedAttribute);
}

/** 判断属性是否属于满 5 手可标记“大吉”的重点属性，使用稳定类型而不是中文名称。 */
function isAuspiciousAttribute(attributeType: string): boolean {
  return AUSPICIOUS_ATTRIBUTE_TYPES.has(attributeType);
}

/** 一枚御魂只要重点属性有一项强化满 5 手，就在身份列展示“大吉”整体标识。 */
function isAuspiciousSoul(soul: MySoul): boolean {
  return subAttributes(soul).some(
    (attribute) =>
      attribute.enhancementCount !== null &&
      attribute.enhancementCount >= 5 &&
      isAuspiciousAttribute(attribute.attributeType),
  );
}

/** +0 代表御魂尚未发生强化，在身份列明确标注，避免与已强化但属性为空混淆。 */
function isUnenhancedSoul(soul: MySoul): boolean {
  return soul.level === 0;
}

/** 只读取当前页的评分摘要；按钮状态由分页响应中的 scoreCount 决定。 */
async function loadScoreSummaries(
  soulKeys: string[],
  requestId: number,
  merge = false,
): Promise<void> {
  try {
    const summaries = await desktopApi.listMySoulScores(soulKeys);
    if (requestId === soulPageRequestId) {
      if (!merge) {
        scoreSummaries.value = summaries;
      } else {
        const merged = new Map(
          [...scoreSummaries.value, ...summaries].map((summary) => [
            summary.soulKey,
            summary,
          ]),
        );
        scoreSummaries.value = [...merged.values()];
      }
    }
  } catch (caught) {
    setScoreError(caught);
  }
}

/** 处理评分任务事件；只有当前页面发起的任务才更新进度条和评分结果。 */
function handleScoreProgress(progress: TaskProgress): void {
  if (!scoreBusy.value || progress.phase !== "rule-recalculation") return;
  // 任务 ID 在极快完成时可能晚于第一条事件返回，因此先以页面忙碌状态接住首条事件。
  if (scoreTaskId.value && scoreTaskId.value !== progress.taskId) return;
  scoreTaskId.value = progress.taskId;
  scoreProgress.value = progress;
  if (progress.status === "running") return;

  scoreBusy.value = false;
  scoreTaskId.value = null;
  if (progress.status === "completed") {
    // 后端已提交事务后再读取当前页和总评分数量，确保摘要与数据库同一版本。
    scoreRefreshing.value = true;
    void loadSoulPage()
      .catch((caught) => setError(caught))
      .finally(() => {
        scoreRefreshing.value = false;
      });
    return;
  }
  scoreError.value = progress.error
    ? `${progress.error.code} · ${progress.error.message}`
    : progress.message;
  // 失败或取消不会提交新结果；重新读取旧摘要并恢复“标准已更新”提示，便于用户再次重试。
  scoreRefreshing.value = true;
  void loadSoulPage()
    .catch((caught) => setError(caught))
    .finally(() => {
      scoreRefreshing.value = false;
    });
}

/** 从评分标准启动后台重算；结果由 Rust 在一批事务中写入分析待办表。 */
async function calculateScores(): Promise<void> {
  if (scoreBusy.value || !inventoryTotal.value) return;
  scoreError.value = "";
  try {
    if (!selectedProfileId.value) {
      scoreError.value = "当前库存缺少对应的游戏档案，请刷新页面后重试";
      return;
    }
    scoreBusy.value = true;
    // 计算事务提交前不继续展示旧标准分数，避免长任务期间误以为后台仍在使用旧规则。
    scoreSummaries.value = [];
    scoreTaskId.value = null;
    scoreProgress.value = {
      taskId: "pending",
      phase: "rule-recalculation",
      completed: 0,
      total: inventoryTotal.value,
      message: "正在准备评分标准…",
      status: "running",
      error: null,
      result: null,
    };
    const accepted = await desktopApi.startRuleRecalculationTask({
      profileId: selectedProfileId.value,
    });
    scoreTaskId.value = accepted.taskId;
  } catch (caught) {
    scoreBusy.value = false;
    scoreTaskId.value = null;
    setScoreError(caught);
    // 后台任务未能启动时恢复刚才清空的旧摘要，并继续提示用户按新标准重试。
    scoreRefreshing.value = true;
    void loadSoulPage()
      .catch((loadError) => setError(loadError))
      .finally(() => {
        scoreRefreshing.value = false;
      });
  }
}

/** 允许用户停止长库存计算；任务会在下一枚御魂的安全边界退出，不提交半批结果。 */
async function cancelScoreCalculation(): Promise<void> {
  if (!scoreTaskId.value || !scoreBusy.value) return;
  try {
    await desktopApi.cancelBackgroundTask(scoreTaskId.value);
  } catch (caught) {
    setScoreError(caught);
  }
}

/** 将评分任务错误保持在页面局部，避免覆盖库存读取状态。 */
function setScoreError(caught: unknown): void {
  const commandError = normalizeCommandError(caught);
  scoreError.value = commandError.code + " · " + commandError.message;
}

/** 从后端读取当前页或数值筛选的下一批；查询、排序和批次边界都限制在数据库侧。 */
async function loadSoulPage(append = false): Promise<void> {
  const requestId = ++soulPageRequestId;
  soulPageLoading.value = true;
  // 标准评分上下限分别采用严格大于和严格小于，可单独使用或组合为开区间。
  const scoreMin = Number(filters.value.standardScoreMin);
  const scoreMax = Number(filters.value.standardScoreMax);
  const offset =
    hasAttributeValueFilter.value && append
      ? souls.value.length
      : (page.value - 1) * PAGE_SIZE;
  const request = {
    offset,
    limit: PAGE_SIZE,
    setId: filters.value.setId || undefined,
    setCategory:
      selectedSetCategory.value !== "all" ? selectedSetCategory.value : undefined,
    slot: filters.value.slot ? Number(filters.value.slot) : undefined,
    quality: filters.value.quality ? Number(filters.value.quality) : undefined,
    level: filters.value.level !== "" ? Number(filters.value.level) : undefined,
    mainAttrType: filters.value.mainAttrType || undefined,
    subAttrType: filters.value.subAttrType || undefined,
    attributeType: filters.value.attributeType || undefined,
    attributeOperator:
      filters.value.attributeType && filters.value.attributeValue !== ""
        ? filters.value.attributeOperator
        : undefined,
    attributeValue:
      filters.value.attributeType && filters.value.attributeValue !== ""
        ? (attributeFilterStoredValue(
            filters.value.attributeType,
            filters.value.attributeValue,
          ) ?? undefined)
        : undefined,
    standardScoreMin:
      filters.value.standardScoreMin !== "" && Number.isFinite(scoreMin)
        ? scoreMin
        : undefined,
    standardScoreMax:
      filters.value.standardScoreMax !== "" && Number.isFinite(scoreMax)
        ? scoreMax
        : undefined,
    auspiciousOnly: filters.value.auspiciousOnly,
  };

  try {
    const result = await desktopApi.listMySouls(request);
    if (requestId !== soulPageRequestId) return;

    // 后端以当前激活角色读取库存；评分必须使用完全相同的档案 ID。
    selectedProfileId.value = result.profileId;
    inventoryTotal.value = result.inventoryTotal;
    soulTotal.value = result.total;
    soulPageHasMore.value = result.hasMore;
    sourceKinds.value = result.sourceKinds;
    filterOptions.value = result.filterOptions;
    scoreCount.value = result.scoreCount;
    const resultPageCount = Math.max(1, Math.ceil(result.total / PAGE_SIZE));
    if (!append && !hasAttributeValueFilter.value && page.value > resultPageCount) {
      page.value = resultPageCount;
      await loadSoulPage();
      return;
    }

    if (append) {
      souls.value = [...souls.value, ...result.items];
    } else {
      souls.value = result.items;
      // 翻页或筛选后清理上一页的展开行，避免详情指向已不可见的御魂。
      expandedSoulKey.value = null;
    }
    await loadScoreSummaries(
      result.items.map((soul) => soul.soulKey),
      requestId,
      append,
    );
  } finally {
    if (requestId === soulPageRequestId) {
      soulPageLoading.value = false;
    }
  }
}

/** 读取全局已启用的评分标准；失败时保留页面已有评分摘要，不阻断御魂库存展示。 */
async function loadActiveScoreStandard(): Promise<void> {
  try {
    const standards = await desktopApi.listScoreStandards();
    const enabled = standards.find((standard) => standard.enabled);
    if (enabled?.title) {
      activeScoreStandardTitle.value = enabled.title;
      activeScoreStandardHash.value = enabled.normalizedHash;
      scoreColorThresholds.value = enabled.scoreColorThresholds;
    }
  } catch (caught) {
    // 标准名称只是展示增强，评分摘要仍可提供名称，因此错误只保留在评分区域。
    setScoreError(caught);
  }
}

/** 读取目录和当前页库存；档案和目录准备好后才发起分页查询。 */
async function loadPage(): Promise<void> {
  loading.value = true;
  error.value = "";
  try {
    let catalog = await desktopApi.getCatalogStatus();
    if (
      !catalog ||
      catalog.setCount < EXPECTED_CATALOG_SET_COUNT ||
      catalog.iconCoverage < EXPECTED_CATALOG_SET_COUNT ||
      catalog.mechanicsCoverage < EXPECTED_CATALOG_SET_COUNT ||
      !["verified", "partial"].includes(catalog.validationStatus)
    ) {
      catalog = await desktopApi.installBuiltinCatalog();
    }
    let catalogSets = await desktopApi.listSoulSets(catalog.version);
    // 旧数据库可能已经是当前目录版本，但分类列仍为空；重新安装内置目录只更新目录，不改写玩家库存快照。
    const hasCompleteSetCategories =
      catalogSets.length >= EXPECTED_CATALOG_SET_COUNT &&
      catalogSets.every((set) => Boolean(set.specialCategory ?? set.category));
    if (!hasCompleteSetCategories) {
      catalog = await desktopApi.installBuiltinCatalog();
      catalogSets = await desktopApi.listSoulSets(catalog.version);
    }
    catalogStatus.value = catalog;
    sets.value = catalogSets;
    await loadActiveScoreStandard();
    page.value = 1;
    await loadSoulPage();
  } catch (caught) {
    setError(caught);
  } finally {
    loading.value = false;
  }
}

/** 将后端命令错误转换成页面稳定文案，刷新按钮可以直接重试整个目录与库存读取。 */
function setError(caught: unknown): void {
  const commandError = normalizeCommandError(caught);
  error.value = commandError.code + " · " + commandError.message;
}

/** 清空所有下拉条件，恢复完整库存列表。 */
function clearFilters(): void {
  selectedSetCategory.value = "all";
  filters.value = {
    setId: "",
    slot: "",
    quality: "",
    level: "",
    mainAttrType: "",
    subAttrType: "",
    attributeType: "",
    attributeOperator: "gt",
    attributeValue: "",
    standardScoreMin: "",
    standardScoreMax: "",
    auspiciousOnly: false,
  };
}

/** 空库存引导进入藏宝阁导入。 */
function openDataReadingGuide(): void {
  window.dispatchEvent(new CustomEvent("open-desktop-reading"));
}

/** 从库存页跳转到独立模拟强化工作台；模拟结果不与库存筛选状态耦合。 */
function openSimulation(): void {
  window.dispatchEvent(new CustomEvent("open-simulation"));
}

/** 翻页只重新请求 10 枚御魂，避免在前端保留整份库存。 */
function changePage(delta: number): void {
  const nextPage = Math.min(pageCount.value, Math.max(1, page.value + delta));
  if (nextPage === page.value || soulPageLoading.value) return;
  page.value = nextPage;
  void loadSoulPage().catch((caught) => setError(caught));
}

/** 数值筛选按当前已加载数量请求下一批，追加到列表尾部并保留已有评分摘要。 */
function loadMoreSouls(): void {
  if (!hasAttributeValueFilter.value || !soulPageHasMore.value || soulPageLoading.value) {
    return;
  }
  void loadSoulPage(true).catch((caught) => setError(caught));
}

/** 合并连续筛选输入，只提交最后一次查询，避免输入阈值时堆积多个数据库请求。 */
function scheduleSoulPageReload(): void {
  soulPageRequestId += 1;
  page.value = 1;
  soulPageHasMore.value = false;
  if (soulFilterReloadTimer !== null) {
    window.clearTimeout(soulFilterReloadTimer);
  }
  soulFilterReloadTimer = window.setTimeout(() => {
    soulFilterReloadTimer = null;
    void loadSoulPage().catch((caught) => setError(caught));
  }, 180);
}

watch(
  [filters, selectedSetCategory],
  scheduleSoulPageReload,
  { deep: true },
);

onMounted(async () => {
  stopTaskProgress = await desktopApi.onTaskProgress((progress) => {
    handleScoreProgress(progress);
  });
  stopActiveCharacterChanged = onActiveCharacterChanged(() => {
    void loadPage().catch((caught) => setError(caught));
  });
  void loadPage();
});

onBeforeUnmount(() => {
  if (soulFilterReloadTimer !== null) {
    window.clearTimeout(soulFilterReloadTimer);
  }
  stopTaskProgress?.();
  stopActiveCharacterChanged?.();
});
</script>

<template>
  <main class="workspace my-souls">
    <header class="page-header my-souls__header">
      <div>
        <p class="eyebrow">MY SOULS / 02</p>
        <div class="my-souls__title-row">
          <h1>御魂分析</h1>
          <div
            v-if="sourceKindLabels().length"
            class="my-souls__source-badges"
            aria-label="库存数据来源"
          >
            <span class="my-souls__source-label">来源：</span>
            <span
              v-for="source in sourceKindLabels()"
              :key="source"
              class="status-chip status-chip--safe"
            >
              {{ source }}
            </span>
          </div>
        </div>
        <p class="lede">
          这里展示最近一次读取的实际库存；套装名称和图标按御魂档案的稳定目录编号匹配。
        </p>
      </div>
      <div
        v-if="
          activeSubPage !== 'speed' &&
          activeSubPage !== 'pve' &&
          activeSubPage !== 'head-tail' &&
          activeSubPage !== 'growth-quality' &&
          activeSubPage !== 'embryo-decision'
        "
        class="my-souls__header-actions"
      >
        <CharacterArchiveSwitcher />
        <div class="health-badge" :class="{ 'health-badge--error': error }">
          <span class="health-badge__pulse" aria-hidden="true"></span>
          <span>{{ error ? "库存读取需要处理" : catalogHealthLabel }}</span>
        </div>
        <button
          class="button button--primary"
          type="button"
          :disabled="
            loading ||
            scoreBusy ||
            scoreRefreshing ||
            !inventoryTotal
          "
          :title="
            scoreCount ? '按当前评分标准重新计算全部御魂' : '按当前评分标准计算全部御魂'
          "
          @click="calculateScores"
        >
          {{
            scoreBusy
              ? "正在计算…"
              : scoreRefreshing
                ? "正在刷新结果…"
                : scoreCount
                  ? "重新计算评分"
                  : "计算评分"
          }}
        </button>
        <button
          class="button button--quiet"
          type="button"
          :disabled="loading"
          @click="openSimulation"
        >
          模拟强化未强化御魂
        </button>
      </div>
      <div v-else class="my-souls__header-actions">
        <CharacterArchiveSwitcher />
        <div class="health-badge">
          <span class="health-badge__pulse" aria-hidden="true"></span>
          <span>{{
            activeSubPage === 'head-tail'
              ? "头尾原型预览"
              : activeSubPage === 'growth-quality'
                ? "成长质量"
                : activeSubPage === 'embryo-decision'
                  ? "胚子强化决策"
                  : "静态页面预览"
          }}</span>
        </div>
      </div>
    </header>

    <nav class="my-souls-tabs" aria-label="御魂分析子页面">
      <button
        class="my-souls-tabs__item"
        :class="{ 'my-souls-tabs__item--active': activeSubPage === 'inventory' }"
        type="button"
        :aria-current="activeSubPage === 'inventory' ? 'page' : undefined"
        @click="activeSubPage = 'inventory'"
      >
        <span>库存列表</span>
        <small>逐枚查看与筛选</small>
      </button>
      <button
        class="my-souls-tabs__item"
        :class="{ 'my-souls-tabs__item--active': activeSubPage === 'speed' }"
        type="button"
        :aria-current="activeSubPage === 'speed' ? 'page' : undefined"
        @click="activeSubPage = 'speed'"
      >
        <span>一速御魂</span>
        <small>散件与套装要求</small>
      </button>
      <button
        class="my-souls-tabs__item"
        :class="{ 'my-souls-tabs__item--active': activeSubPage === 'pve' }"
        type="button"
        :aria-current="activeSubPage === 'pve' ? 'page' : undefined"
        @click="activeSubPage = 'pve'"
      >
        <span>PVE 面板</span>
        <small>须佐之男输出计算</small>
      </button>
      <button
        class="my-souls-tabs__item"
        :class="{ 'my-souls-tabs__item--active': activeSubPage === 'head-tail' }"
        type="button"
        :aria-current="activeSubPage === 'head-tail' ? 'page' : undefined"
        @click="activeSubPage = 'head-tail'"
      >
        <span>头尾分析</span>
        <small>速度为王</small>
      </button>
      <button
        class="my-souls-tabs__item"
        :class="{ 'my-souls-tabs__item--active': activeSubPage === 'radar' }"
        type="button"
        :aria-current="activeSubPage === 'radar' ? 'page' : undefined"
        @click="activeSubPage = 'radar'"
      >
        <span>御魂雷达</span>
        <small>六个号位横向比较</small>
      </button>
    </nav>

    <MySoulRadarView v-if="activeSubPage === 'radar'" />
    <Plus15GrowthQualityView
      v-else-if="activeSubPage === 'growth-quality'"
      :sets="sets"
    />
    <FourLegEmbryoDecisionView
      v-else-if="activeSubPage === 'embryo-decision'"
      :sets="sets"
    />
    <SpeedSoulView v-else-if="activeSubPage === 'speed'" :sets="sets" />
    <PvePanelView v-else-if="activeSubPage === 'pve'" />
    <HeadTailAnalysisPrototype
      v-else-if="activeSubPage === 'head-tail'"
      :sets="sets"
    />

    <template v-else>
      <section v-if="error" class="error-panel" role="alert">
        <div>
          <p class="error-panel__title">御魂分析暂时无法读取</p>
          <p>{{ error }}</p>
        </div>
        <button class="button button--quiet" type="button" @click="loadPage">
          重新读取
        </button>
      </section>

      <section v-if="loading" class="placeholder-card">正在读取已导入的御魂…</section>

      <template v-else>
        <section v-if="scoreError" class="error-panel" role="alert">
          <div>
            <p class="error-panel__title">评分计算未完成</p>
            <p>{{ scoreError }}</p>
          </div>
          <button class="button button--quiet" type="button" @click="calculateScores">
            重试计算
          </button>
        </section>

        <section
          v-if="scoreBusy || scoreProgress"
          class="my-souls-score-progress"
          aria-label="御魂评分进度"
        >
          <div class="my-souls-score-progress__header">
            <div>
              <span
                class="my-souls-score-progress__standard"
                aria-label="使用的御魂评分标准"
              >
                <span>使用的御魂评分标准</span>
                <strong>{{ activeScoreStandardLabel }}</strong>
              </span>
              <strong>{{ scoreStatusLabel }}</strong>
            </div>
            <div class="my-souls-score-progress__meta">
              <b>{{ scorePercent }}%</b>
              <button
                v-if="scoreBusy"
                class="button button--quiet button--small"
                type="button"
                @click="cancelScoreCalculation"
              >
                停止计算
              </button>
            </div>
          </div>
          <div
            class="my-souls-score-progress__track"
            role="progressbar"
            :aria-valuenow="scorePercent"
            aria-valuemin="0"
            aria-valuemax="100"
            :aria-label="scoreProgress?.message ?? '御魂评分进度'"
          >
            <i :style="{ width: `${scorePercent}%` }"></i>
          </div>
          <div class="my-souls-score-progress__footer">
            <span>{{ scoreProgress?.message ?? "等待开始" }}</span>
            <span v-if="scoreProgress && scoreProgress.total > 0">
              {{ scoreProgress.completed }} / {{ scoreProgress.total }} 枚
            </span>
          </div>
        </section>

        <!-- 库存未评分或标准正文已更新时，提供清晰但不打断浏览的计算入口。 -->
        <section
          v-if="shouldPromptScoreCalculation"
          class="my-souls-score-prompt"
          role="status"
          aria-live="polite"
        >
          <div class="my-souls-score-prompt__mark" aria-hidden="true">评</div>
          <div class="my-souls-score-prompt__copy">
            <strong>{{
              hasStaleScoreStandard
                ? "评分标准已更新，当前仍是旧评分"
                : "当前库存尚未计算标准评分"
            }}</strong>
            <p>
              将使用“{{ activeScoreStandardLabel }}”计算当前角色的
              {{ inventoryTotal }} 枚御魂；{{
                hasStaleScoreStandard
                  ? "重新计算完成后才会替换旧结果。"
                  : "进入页面不会自动开始。"
              }}
            </p>
          </div>
          <button class="button button--primary" type="button" @click="calculateScores">
            {{ hasStaleScoreStandard ? "按新标准重新计算" : "开始计算评分" }}
          </button>
        </section>

        <section class="my-souls-summary" aria-label="库存摘要">
          <div class="my-souls-stat my-souls-stat--primary">
            <span>当前库存</span>
            <strong>{{ inventoryTotal }}</strong>
            <small>枚已导入御魂</small>
          </div>
          <div class="my-souls-stat my-souls-stat--score">
            <span>评分覆盖</span>
            <strong>{{ scoreCoveragePercent }}%</strong>
            <small>{{ scoredSoulCount }} / {{ inventoryTotal }} 枚已计算</small>
            <small class="my-souls-stat__standard">
              使用标准：<b>{{ activeScoreStandardLabel }}</b>
            </small>
          </div>
          <div class="my-souls-stat">
            <span>当前页已匹配</span>
            <strong>{{ matchedSoulCount }}</strong>
            <small>本页套装名称与图标可用</small>
          </div>
          <div
            class="my-souls-stat"
            :class="{ 'my-souls-stat--warning': unmatchedSoulCount }"
          >
            <span>当前页待核对</span>
            <strong>{{ unmatchedSoulCount }}</strong>
            <small>{{
              unmatchedSoulCount ? "本页存在未匹配套装编号" : "本页全部已匹配"
            }}</small>
          </div>
        </section>

        <section class="my-souls-toolbar" aria-label="筛选御魂分析">
          <div class="my-souls-filters">
            <label class="my-souls-filter">
              <span>套装效果</span>
              <select v-model="selectedSetCategory" @change="handleSetCategoryChange">
                <option
                  v-for="category in setCategories"
                  :key="category.key"
                  :value="category.key"
                >
                  {{ category.label }}
                </option>
              </select>
            </label>
            <label class="my-souls-filter">
              <span>御魂</span>
              <SoulSetPicker
                :model-value="filters.setId"
                :options="filteredSetOptions"
                empty-label="全部御魂"
                :show-categories="false"
                @update:model-value="handleSetSelectionChange"
              />
            </label>
            <label class="my-souls-filter">
              <span>号位</span>
              <select v-model="filters.slot">
                <option value="">全部号位</option>
                <option v-for="slot in slotOptions" :key="slot" :value="slot">
                  {{ slot }}号位
                </option>
              </select>
            </label>
            <label class="my-souls-filter">
              <span>星级</span>
              <select v-model="filters.quality">
                <option value="">全部星级</option>
                <option
                  v-for="quality in qualityOptions"
                  :key="quality"
                  :value="quality"
                >
                  {{ quality }}星
                </option>
              </select>
            </label>
            <label class="my-souls-filter">
              <span>等级</span>
              <select v-model="filters.level">
                <option value="">全部等级</option>
                <option v-for="level in levelOptions" :key="level" :value="level">
                  +{{ level }}
                </option>
              </select>
            </label>
            <label class="my-souls-filter">
              <span>主属性</span>
              <select v-model="filters.mainAttrType">
                <option value="">全部主属性</option>
                <option
                  v-for="attributeType in mainAttributeOptions"
                  :key="attributeType"
                  :value="attributeType"
                >
                  {{ attributeLabel(attributeType) }}
                </option>
              </select>
            </label>
            <label class="my-souls-filter">
              <span>副属性包含</span>
              <select v-model="filters.subAttrType">
                <option value="">不限副属性</option>
                <option
                  v-for="attributeType in subAttributeOptions"
                  :key="attributeType"
                  :value="attributeType"
                >
                  {{ attributeLabel(attributeType) }}
                </option>
              </select>
            </label>
            <label class="my-souls-filter my-souls-filter--attribute-value">
              <span>副属性数值</span>
              <div class="my-souls-filter__attribute-control">
                <select v-model="filters.attributeType" aria-label="选择属性">
                  <option value="">不限属性</option>
                  <option
                    v-for="attributeType in attributeValueOptions"
                    :key="attributeType"
                    :value="attributeType"
                  >
                    {{ attributeLabel(attributeType) }}
                  </option>
                </select>
                <select
                  v-model="filters.attributeOperator"
                  aria-label="选择属性比较方式"
                  :disabled="!filters.attributeType"
                >
                  <option value="gt">大于</option>
                  <option value="lt">小于</option>
                </select>
                <div class="my-souls-filter__input-wrap">
                  <input
                    v-model="filters.attributeValue"
                    type="number"
                    min="0"
                    step="0.1"
                    :placeholder="attributeValuePlaceholder"
                    aria-label="属性数值阈值"
                    :disabled="!filters.attributeType"
                  />
                  <small>{{ attributeValueUnit }}</small>
                </div>
              </div>
            </label>
            <label class="my-souls-filter my-souls-filter--score">
              <span>标准评分大于</span>
              <div class="my-souls-filter__input-wrap">
                <input
                  v-model="filters.standardScoreMin"
                  type="number"
                  min="0"
                  max="100"
                  step="0.1"
                  placeholder="不限"
                  aria-label="标准评分大于多少分"
                />
                <small>分</small>
              </div>
            </label>
            <label class="my-souls-filter my-souls-filter--score">
              <span>标准评分小于</span>
              <div class="my-souls-filter__input-wrap">
                <input
                  v-model="filters.standardScoreMax"
                  type="number"
                  min="0"
                  max="100"
                  step="0.1"
                  placeholder="不限"
                  aria-label="标准评分小于多少分"
                />
                <small>分</small>
              </div>
            </label>
            <label class="my-souls-filter my-souls-filter--check">
              <span>特殊标识</span>
              <span class="my-souls-filter__check-control">
                <input v-model="filters.auspiciousOnly" type="checkbox" />
                <b>只看大吉御魂</b>
              </span>
            </label>
          </div>
          <div class="my-souls-toolbar__meta">
            <span v-if="soulPageLoading">正在加载当前页…</span>
            <span v-else>显示 {{ rangeLabel }}</span>
            <button
              v-if="hasFilters"
              class="button button--quiet"
              type="button"
              @click="clearFilters"
            >
              清除条件
            </button>
            <button class="button button--quiet" type="button" @click="loadPage">
              刷新库存
            </button>
          </div>
        </section>

        <section v-if="!inventoryTotal" class="empty-state-card">
          <div class="empty-state-card__mark" aria-hidden="true">读</div>
          <strong>先读取一份御魂数据</strong>
          <p>
            当前列表为空。请从藏宝阁导入一份公开商品，成功后这里会显示实际库存。
          </p>
          <button
            class="button button--primary"
            type="button"
            @click="openDataReadingGuide"
          >
            去藏宝阁导入
          </button>
        </section>
        <section v-else-if="!soulTotal" class="empty-state-card">
          <strong>没有找到匹配的御魂</strong>
          <p>可以清除部分条件，或重新组合套装、号位、星级和属性筛选。</p>
        </section>
        <section v-else class="my-souls-table-shell" aria-label="御魂分析列表">
          <div class="my-souls-table-scroll">
            <div class="my-souls-table" role="table" aria-label="御魂分析评分表">
              <div class="my-souls-table__head" role="row">
                <span
                  class="my-souls-table__cell my-souls-table__cell--index"
                  role="columnheader"
                  >#</span
                >
                <span class="my-souls-table__cell" role="columnheader">御魂</span>
                <span
                  class="my-souls-table__cell my-souls-table__cell--center"
                  role="columnheader"
                  >等级</span
                >
                <span
                  class="my-souls-table__cell my-souls-table__cell--center"
                  role="columnheader"
                  >星级</span
                >
                <span
                  class="my-souls-table__cell my-souls-table__cell--center"
                  role="columnheader"
                  >位置</span
                >
                <span class="my-souls-table__cell" role="columnheader"
                  >主属性 | 固有属性</span
                >
                <span class="my-souls-table__cell" role="columnheader">副属性</span>
                <span
                  class="my-souls-table__cell my-souls-table__cell--center my-souls-table__score-header"
                  role="columnheader"
                >
                  <span class="my-souls-table__score-header-label">
                    标准评分
                    <button
                      class="my-souls-table__help"
                      type="button"
                      aria-label="查看标准评分计算规则"
                      :aria-expanded="standardScoreHelpOpen"
                      @click.stop="standardScoreHelpOpen = !standardScoreHelpOpen"
                    >
                      ?
                    </button>
                  </span>
                  <span
                    v-if="standardScoreHelpOpen"
                    class="my-souls-table__help-popover"
                    role="tooltip"
                  >
                    <strong>标准评分怎么算？</strong>
                    <span>先判断御魂命中的评分规则卡片，再读取最终副属性。</span>
                    <span
                      >每条属性按规则卡片的基础分与强化分计算，所有贡献直接相加。</span
                    >
                    <span>综合评分待开发。</span>
                  </span>
                </span>
                <span
                  class="my-souls-table__cell my-souls-table__cell--center"
                  role="columnheader"
                  >综合评分（待开发）</span
                >
                <span
                  class="my-souls-table__cell my-souls-table__cell--center"
                  role="columnheader"
                  >操作</span
                >
              </div>

              <div
                v-for="(soul, index) in visibleSouls"
                :key="soul.soulKey"
                class="my-souls-table__row"
                :class="{ 'my-souls-table__row--unmatched': !setById.get(soul.setId) }"
                role="row"
              >
                <span
                  class="my-souls-table__cell my-souls-table__cell--index"
                  role="cell"
                >
                  {{ (page - 1) * PAGE_SIZE + index + 1 }}
                </span>
                <span
                  class="my-souls-table__cell my-souls-table__identity"
                  :class="{
                    'my-souls-table__identity--auspicious': isAuspiciousSoul(soul),
                  }"
                  role="cell"
                >
                  <span class="my-souls-table__icon" aria-hidden="true">
                    <img
                      v-if="soulIcon(setById.get(soul.setId))"
                      :src="soulIcon(setById.get(soul.setId))!"
                      alt=""
                    />
                    <span v-else>{{
                      setById.get(soul.setId)?.name?.slice(0, 1) ?? "?"
                    }}</span>
                  </span>
                  <span class="my-souls-table__identity-copy">
                    <strong class="my-souls-table__identity-title">
                      <span>{{ setById.get(soul.setId)?.name ?? "未匹配套装" }}</span>
                      <span
                        v-if="isAuspiciousSoul(soul)"
                        class="my-souls-table__auspicious"
                        title="重点属性强化满 5 手：大吉"
                      >
                        ✦ 大吉
                      </span>
                      <span
                        v-if="isUnenhancedSoul(soul)"
                        class="my-souls-table__unenhanced"
                        title="这枚御魂尚未强化"
                      >
                        未强化
                      </span>
                    </strong>
                    <small
                      >{{ soul.initialSubstatCount ?? "—" }}腿 ·
                      {{ soul.equippedState ?? "未装备" }}</small
                    >
                  </span>
                </span>
                <span
                  class="my-souls-table__cell my-souls-table__cell--center my-souls-table__level"
                  role="cell"
                  >+{{ soul.level }}</span
                >
                <span
                  class="my-souls-table__cell my-souls-table__cell--center"
                  role="cell"
                  >{{ soul.quality }}</span
                >
                <span
                  class="my-souls-table__cell my-souls-table__cell--center"
                  role="cell"
                  >{{ soul.slot }}号</span
                >
                <span
                  class="my-souls-table__cell my-souls-table__attribute-column"
                  role="cell"
                >
                  <span class="my-souls-table__main-attribute">
                    {{ attributeLabel(soul.mainAttrType) }}
                    {{ formatAttributeValue(soul.mainAttrType, soul.mainAttrValue) }}
                  </span>
                  <span
                    v-if="fixedAttributes(soul).length"
                    class="my-souls-table__attribute-list"
                  >
                    <span
                      v-for="attribute in fixedAttributes(soul)"
                      :key="attribute.attributeIndex"
                      class="my-souls-table__attribute my-souls-table__attribute--fixed"
                    >
                      {{ attributeLabel(attribute.attributeType) }}
                      {{
                        formatAttributeValue(attribute.attributeType, attribute.value)
                      }}
                      <em>固有</em>
                    </span>
                  </span>
                </span>
                <span
                  class="my-souls-table__cell my-souls-table__attribute-column"
                  role="cell"
                >
                  <span
                    v-if="subAttributes(soul).length"
                    class="my-souls-table__attribute-list"
                  >
                    <span
                      v-for="attribute in subAttributes(soul)"
                      :key="attribute.attributeIndex"
                      class="my-souls-table__attribute"
                      :class="{
                        'my-souls-table__attribute--max-roll':
                          attribute.enhancementCount !== null &&
                          attribute.enhancementCount >= 5,
                      }"
                    >
                      <span
                        >{{ attributeLabel(attribute.attributeType) }}
                        {{
                          formatAttributeValue(attribute.attributeType, attribute.value)
                        }}</span
                      >
                      <b
                        v-if="attribute.enhancementCount !== null"
                        :class="{
                          'my-souls-table__roll--hit':
                            attribute.enhancementCount > 0 &&
                            attribute.enhancementCount < 4,
                          'my-souls-table__roll--four':
                            attribute.enhancementCount === 4,
                          'my-souls-table__roll--five': attribute.enhancementCount >= 5,
                        }"
                        >+{{ attribute.enhancementCount }}</b
                      >
                      <b
                        v-else-if="attribute.enhancementCount === null"
                        class="my-souls-table__roll--unknown"
                        >?</b
                      >
                    </span>
                  </span>
                  <span v-else class="my-souls-table__muted">没有副属性记录</span>
                </span>
                <span
                  class="my-souls-table__cell my-souls-table__score"
                  :class="`my-souls-table__score--${scoreTone(scoreBySoulKey.get(soul.soulKey)?.standardScore ?? null)}`"
                  :aria-label="standardScoreTooltip(soul)"
                  :tabindex="scoreBySoulKey.has(soul.soulKey) ? 0 : undefined"
                  :aria-describedby="
                    scoreBySoulKey.has(soul.soulKey)
                      ? `score-tooltip-${soul.soulKey}`
                      : undefined
                  "
                  role="cell"
                  @mouseenter="showScoreTooltip(soul.soulKey, $event)"
                  @mouseleave="hideScoreTooltip"
                  @focusin="showScoreTooltip(soul.soulKey, $event)"
                  @focusout="hideScoreTooltip"
                >
                  <strong>{{
                    formatScore(scoreBySoulKey.get(soul.soulKey)?.standardScore ?? null)
                  }}</strong>
                  <small>{{
                    scoreBySoulKey.get(soul.soulKey)?.standardScoreRule
                      ? `规则卡片：${scoreBySoulKey.get(soul.soulKey)?.standardScoreRule}`
                      : "尚未计算"
                  }}</small>
                  <small
                    v-if="scoreBySoulKey.get(soul.soulKey)?.standardScoreFormula"
                    class="my-souls-table__score-formula"
                  >
                    {{ scoreBySoulKey.get(soul.soulKey)?.standardScoreFormula }}
                  </small>
                </span>
                <span
                  class="my-souls-table__cell my-souls-table__score my-souls-table__score--planned"
                  role="cell"
                >
                  <strong>待开发</strong>
                </span>
                <span class="my-souls-table__cell my-souls-table__action" role="cell">
                  <button
                    class="button button--quiet button--small"
                    type="button"
                    :aria-expanded="expandedSoulKey === soul.soulKey"
                    @click="toggleSoulDetails(soul.soulKey)"
                  >
                    {{ expandedSoulKey === soul.soulKey ? "收起详情" : "评分详情" }}
                  </button>
                </span>

                <div
                  v-if="expandedSoulKey === soul.soulKey"
                  class="my-souls-table__detail"
                  role="cell"
                >
                  <div class="my-souls-table__detail-block">
                    <span class="section-kicker">SCORING EXPLANATION</span>
                    <strong
                      >标准评分
                      {{
                        formatScore(
                          scoreBySoulKey.get(soul.soulKey)?.standardScore ?? null,
                        )
                      }}</strong
                    >
                    <p>
                      规则卡片：{{
                        scoreBySoulKey.get(soul.soulKey)?.standardScoreRule ??
                        "尚未命中"
                      }}<br />
                      {{
                        scoreBySoulKey.get(soul.soulKey)?.standardScoreFormula ??
                        "尚未计算，点击顶部按钮后生成评分结果。"
                      }}
                    </p>
                  </div>
                  <div class="my-souls-table__detail-block">
                    <span class="section-kicker">BEST USE</span>
                    <strong>综合评分 · 待开发</strong>
                  </div>
                  <div class="my-souls-table__detail-block">
                    <span class="section-kicker">ROLLS</span>
                    <strong>强化命中记录</strong>
                    <p v-if="subAttributes(soul).length">
                      <span
                        v-for="attribute in subAttributes(soul)"
                        :key="attribute.attributeIndex"
                        class="my-souls-table__roll-summary"
                      >
                        {{ attributeLabel(attribute.attributeType) }}：{{
                          attribute.enhancementCount === null
                            ? "未解析"
                            : `${attribute.enhancementCount} 手`
                        }}
                      </span>
                    </p>
                    <p v-else>暂无可展示的强化副属性。</p>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </section>

        <aside
          v-if="hoveredScoreSummary"
          :id="`score-tooltip-${hoveredScoreSummary.soulKey}`"
          class="my-souls-score-tooltip"
          :class="`my-souls-score-tooltip--${scoreTone(hoveredScoreSummary.standardScore)}`"
          :style="scoreTooltipStyle"
          role="tooltip"
        >
          <div class="my-souls-score-tooltip__heading">
            <span>标准评分</span>
            <strong>{{ formatScore(hoveredScoreSummary.standardScore) }}</strong>
          </div>
          <div class="my-souls-score-tooltip__rule">
            规则卡片：<b>{{ hoveredScoreSummary.standardScoreRule ?? "尚未命中" }}</b>
          </div>
          <div
            v-if="hoveredScoreSummary.standardScoreContributions.length"
            class="my-souls-score-tooltip__contributions"
          >
            <span
              v-for="contribution in hoveredScoreSummary.standardScoreContributions"
              :key="`${contribution.attributeType}-${contribution.kind}`"
              class="my-souls-score-tooltip__contribution"
              :class="`my-souls-score-tooltip__contribution--${contribution.kind}`"
            >
              <b>{{ contribution.attributeName }}</b>
              <span>{{ formatScoreContribution(contribution) }}</span>
            </span>
          </div>
          <small
            v-if="standardScoreRuleNote(hoveredScoreSummary)"
            class="my-souls-score-tooltip__note"
          >
            {{ standardScoreRuleNote(hoveredScoreSummary) }}
          </small>
          <small v-else class="my-souls-score-tooltip__note">
            {{ hoveredScoreSummary.standardScoreFormula ?? "尚未生成计算公式" }}
          </small>
        </aside>

        <div
          v-if="hasAttributeValueFilter && soulPageHasMore"
          class="my-souls-load-more"
          aria-live="polite"
        >
          <button
            class="button button--quiet"
            type="button"
            :disabled="soulPageLoading"
            @click="loadMoreSouls"
          >
            {{ soulPageLoading ? "正在加载…" : "加载更多" }}
          </button>
          <span>已加载 {{ souls.length }} 条</span>
        </div>

        <nav
          v-if="pageCount > 1 && !hasAttributeValueFilter"
          class="my-souls-pagination"
          aria-label="御魂分析分页"
        >
          <button
            class="button button--quiet"
            type="button"
            :disabled="page <= 1 || soulPageLoading"
            @click="changePage(-1)"
          >
            上一页
          </button>
          <span>第 {{ page }} / {{ pageCount }} 页</span>
          <button
            class="button button--quiet"
            type="button"
            :disabled="page >= pageCount || soulPageLoading"
            @click="changePage(1)"
          >
            下一页
          </button>
        </nav>
      </template>
    </template>
  </main>
</template>

<style scoped>
.my-souls__header {
  margin-bottom: 24px;
}

/* 来源标志紧贴“我的御魂”标题；库存混合来源时允许并排显示多个标志。 */
.my-souls__title-row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}

.my-souls__title-row h1 {
  margin: 0;
}

.my-souls__source-badges {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.my-souls__source-label {
  color: var(--ink-soft);
  font-size: 12px;
  font-weight: 700;
}

.my-souls__header-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
  flex-wrap: wrap;
}

/* 子页面导航与主导航保持同一套低饱和暖色体系，明确当前是在“我的御魂”内部切换视图。 */
.my-souls-tabs {
  display: flex;
  gap: 6px;
  margin: -14px 0 22px;
  overflow-x: auto;
  padding-bottom: 8px;
  border-bottom: 1px solid var(--line);
}

.my-souls-tabs__item {
  display: grid;
  gap: 3px;
  min-width: 142px;
  padding: 9px 13px 10px;
  border: 1px solid transparent;
  border-radius: 8px 8px 0 0;
  color: var(--ink-soft);
  background: transparent;
  cursor: pointer;
  text-align: left;
}

.my-souls-tabs__item:hover {
  color: var(--ink);
  background: rgb(255 250 240 / 62%);
}

.my-souls-tabs__item--active {
  border-color: var(--line);
  border-bottom-color: var(--paper);
  color: var(--ink);
  background: var(--paper);
  box-shadow: 0 -2px 0 var(--cinnabar) inset;
}

.my-souls-tabs__item span {
  font-size: 13px;
  font-weight: 700;
}

.my-souls-tabs__item small {
  color: var(--ink-soft);
  font-size: 10px;
}

.my-souls-summary {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 12px;
  margin-bottom: 16px;
}

.my-souls-stat {
  display: grid;
  gap: 4px;
  min-height: 112px;
  padding: 16px 18px;
  border: 1px solid var(--line);
  border-radius: 10px;
  background: rgb(255 250 240 / 74%);
}

.my-souls-stat--primary {
  border-color: rgb(83 123 111 / 36%);
  background: linear-gradient(135deg, rgb(230 241 233 / 92%), rgb(255 250 240 / 92%));
}

.my-souls-stat--warning {
  border-color: rgb(179 67 62 / 32%);
  background: rgb(179 67 62 / 5%);
}

.my-souls-stat--score {
  border-color: rgb(174 118 83 / 34%);
  background: linear-gradient(135deg, rgb(250 238 213 / 92%), rgb(255 250 240 / 92%));
}

.my-souls-stat span,
.my-souls-stat small {
  color: var(--ink-soft);
  font-size: 11px;
}

.my-souls-stat strong {
  color: var(--ink);
  font-size: 30px;
  font-variant-numeric: tabular-nums;
  line-height: 1.2;
}

/* 用暖金色徽章强调真正参与本次计算的标准名称，避免和普通覆盖说明混在一起。 */
.my-souls-stat__standard {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  width: fit-content;
  max-width: 100%;
  padding: 3px 7px;
  overflow: hidden;
  border: 1px solid rgb(174 118 83 / 38%);
  border-radius: 999px;
  color: var(--accent-deep) !important;
  background: rgb(255 239 198 / 70%);
  font-size: 10px !important;
  font-weight: 700;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.my-souls-stat__standard b {
  overflow: hidden;
  color: var(--cinnabar);
  font-weight: 900;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.my-souls-toolbar {
  display: grid;
  gap: 12px;
  margin-bottom: 16px;
  padding: 14px 16px;
  border: 1px solid var(--line);
  border-radius: 10px;
  background: var(--paper);
}

.my-souls-filters {
  display: grid;
  grid-template-columns: repeat(8, minmax(0, 1fr));
  gap: 8px;
}

.my-souls-filter {
  display: grid;
  min-width: 0;
  gap: 5px;
  color: var(--ink-soft);
  font-size: 11px;
}

.my-souls-filter select {
  min-width: 0;
  min-height: 34px;
  padding: 0 8px;
  border: 1px solid var(--line);
  border-radius: 6px;
  background: #fff;
  color: var(--ink);
  font-size: 11px;
}

.my-souls-filter--attribute-value {
  grid-column: span 2;
}

.my-souls-filter__attribute-control {
  display: grid;
  grid-template-columns: minmax(0, 1.25fr) minmax(66px, 0.75fr) minmax(0, 1fr);
  gap: 5px;
}

.my-souls-filter__attribute-control select,
.my-souls-filter__attribute-control .my-souls-filter__input-wrap {
  min-width: 0;
}

.my-souls-filter__attribute-control select:disabled,
.my-souls-filter__attribute-control input:disabled {
  cursor: not-allowed;
  opacity: 0.55;
}

.my-souls-filter__input-wrap {
  display: flex;
  min-height: 34px;
  align-items: center;
  border: 1px solid var(--line);
  border-radius: 6px;
  background: #fff;
}

.my-souls-filter__input-wrap input {
  width: 100%;
  min-width: 0;
  height: 32px;
  padding: 0 6px 0 8px;
  border: 0;
  outline: 0;
  color: var(--ink);
  background: transparent;
  font-size: 11px;
}

.my-souls-filter__input-wrap small {
  padding-right: 8px;
  color: var(--ink-soft);
  font-size: 10px;
}

.my-souls-filter__check-control {
  display: flex;
  min-height: 34px;
  align-items: center;
  gap: 7px;
  padding: 0 8px;
  border: 1px solid rgb(174 118 83 / 32%);
  border-radius: 6px;
  color: var(--ink);
  background: rgb(250 238 213 / 42%);
}

.my-souls-filter__check-control input {
  accent-color: var(--cinnabar);
}

.my-souls-toolbar__meta {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
  color: var(--ink-soft);
  font-size: 12px;
  white-space: nowrap;
}

/* 计算进度独立成一张窄卡片，让大量御魂计算时的数量百分比始终可见。 */
.my-souls-score-progress {
  display: grid;
  gap: 10px;
  margin-bottom: 16px;
  padding: 14px 16px;
  border: 1px solid rgb(83 123 111 / 30%);
  border-radius: 10px;
  background: linear-gradient(110deg, rgb(235 246 237 / 94%), rgb(255 250 240 / 92%));
}

.my-souls-score-progress__header,
.my-souls-score-progress__footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.my-souls-score-progress__header > div:first-child {
  display: grid;
  gap: 5px;
}

.my-souls-score-progress__standard {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  width: fit-content;
  max-width: 100%;
  padding: 4px 9px;
  overflow: hidden;
  border: 1px solid rgb(174 118 83 / 48%);
  border-radius: 999px;
  color: var(--accent-deep);
  background: linear-gradient(100deg, rgb(255 239 198 / 92%), rgb(255 248 226 / 78%));
  box-shadow: 0 0 0 2px rgb(255 214 105 / 16%);
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.04em;
  white-space: nowrap;
}

.my-souls-score-progress__standard strong {
  overflow: hidden;
  color: var(--cinnabar);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.my-souls-score-progress__header strong {
  color: var(--ink);
  font-size: 13px;
}

.my-souls-score-progress__meta {
  display: flex;
  align-items: center;
  gap: 10px;
}

.my-souls-score-progress__meta b {
  color: var(--accent-deep);
  font-size: 22px;
  font-variant-numeric: tabular-nums;
}

.my-souls-score-progress__track {
  height: 8px;
  overflow: hidden;
  border-radius: 999px;
  background: rgb(83 123 111 / 14%);
}

.my-souls-score-progress__track i {
  display: block;
  height: 100%;
  border-radius: inherit;
  background: linear-gradient(90deg, var(--jade), var(--accent));
  transition: width 180ms ease;
}

.my-souls-score-progress__footer {
  color: var(--ink-soft);
  font-size: 11px;
}

/* 未评分提示沿用御魂印章语汇，以单一“评”字标出需要用户确认的计算边界。 */
.my-souls-score-prompt {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr) auto;
  align-items: center;
  gap: 14px;
  margin-bottom: 16px;
  padding: 14px 16px;
  border: 1px solid rgb(174 118 83 / 38%);
  border-radius: 10px;
  background: linear-gradient(110deg, rgb(255 239 198 / 72%), rgb(255 250 240 / 94%));
}

.my-souls-score-prompt__mark {
  display: grid;
  width: 38px;
  height: 38px;
  place-items: center;
  border: 1px solid rgb(179 67 62 / 46%);
  border-radius: 50%;
  color: var(--cinnabar);
  background: rgb(255 250 240 / 82%);
  font-family: STKaiti, KaiTi, serif;
  font-size: 18px;
  transform: rotate(-5deg);
}

.my-souls-score-prompt__copy {
  display: grid;
  gap: 4px;
}

.my-souls-score-prompt__copy strong {
  color: var(--ink);
  font-size: 14px;
}

.my-souls-score-prompt__copy p {
  margin: 0;
  color: var(--ink-soft);
  font-size: 11px;
  line-height: 1.55;
}

/* 采用流式列网格复刻参考页的信息密度；列宽随可用区域收缩，避免普通桌面分辨率把评分列推到屏幕外。 */
.my-souls-table-shell {
  min-width: 0;
  overflow: hidden;
  border: 1px solid var(--line);
  border-radius: 9px;
  background: rgb(255 255 255 / 78%);
  box-shadow: 0 10px 28px rgb(96 71 44 / 5%);
}

.my-souls-table-scroll {
  overflow-x: auto;
}

.my-souls-table {
  width: 100%;
  color: var(--ink);
  font-size: 11px;
}

.my-souls-table__head,
.my-souls-table__row {
  display: grid;
  grid-template-columns:
    32px minmax(120px, 1.15fr) 44px 44px 44px minmax(150px, 1.2fr)
    minmax(210px, 1.7fr) minmax(84px, 0.7fr) minmax(88px, 0.75fr) 84px;
  align-items: stretch;
}

.my-souls-table__head {
  min-height: 48px;
  color: var(--ink-soft);
  background: linear-gradient(180deg, rgb(247 241 229 / 96%), rgb(241 235 224 / 88%));
  font-weight: 800;
}

.my-souls-table__row {
  position: relative;
  min-height: 112px;
  border-top: 1px solid var(--line-soft);
  background: rgb(255 255 255 / 75%);
  transition: background 120ms ease;
}

.my-souls-table__row:hover {
  background: #fffdf8;
}

.my-souls-table__row--unmatched {
  background: rgb(179 67 62 / 4%);
}

.my-souls-table__cell {
  display: flex;
  min-width: 0;
  align-items: center;
  padding: 10px 10px;
  border-right: 1px solid var(--line-soft);
}

.my-souls-table__head .my-souls-table__cell {
  justify-content: flex-start;
  padding-top: 12px;
  padding-bottom: 12px;
}

.my-souls-table__head .my-souls-table__cell--center,
.my-souls-table__cell--center {
  justify-content: center;
  text-align: center;
}

.my-souls-table__score-header {
  position: relative;
}

.my-souls-table__score-header-label {
  display: inline-flex;
  align-items: center;
  gap: 5px;
}

.my-souls-table__help {
  display: inline-grid;
  width: 17px;
  height: 17px;
  place-items: center;
  padding: 0;
  border: 1px solid var(--line);
  border-radius: 50%;
  color: var(--ink-soft);
  background: rgb(255 255 255 / 78%);
  font-size: 11px;
  font-weight: 800;
  cursor: pointer;
}

.my-souls-table__help:hover,
.my-souls-table__help:focus-visible {
  color: var(--cinnabar);
  border-color: var(--cinnabar);
}

.my-souls-table__help-popover {
  position: absolute;
  z-index: 5;
  top: calc(100% - 4px);
  right: 8px;
  display: grid;
  width: 260px;
  gap: 6px;
  padding: 12px;
  border: 1px solid var(--line);
  border-radius: 10px;
  color: var(--ink-soft);
  background: #fffdf8;
  box-shadow: 0 12px 26px rgb(75 57 33 / 16%);
  font-size: 11px;
  font-weight: 500;
  line-height: 1.5;
  text-align: left;
}

.my-souls-table__help-popover strong {
  color: var(--ink);
  font-size: 12px;
}

.my-souls-table__cell--index {
  justify-content: center;
  color: var(--ink-soft);
  font-family: Consolas, "Cascadia Mono", monospace;
  text-align: center;
}

.my-souls-table__identity {
  gap: 9px;
  transition:
    background 180ms ease,
    box-shadow 180ms ease;
}

.my-souls-table__identity--auspicious {
  position: relative;
  isolation: isolate;
  background: linear-gradient(90deg, rgb(255 246 215 / 72%), rgb(255 255 255 / 0%));
  box-shadow: inset 3px 0 0 #d89a2b;
}

.my-souls-table__identity--auspicious::after {
  position: absolute;
  z-index: -1;
  top: 12px;
  bottom: 12px;
  left: -15%;
  width: 22%;
  background: linear-gradient(100deg, transparent, rgb(255 255 255 / 82%), transparent);
  content: "";
  transform: skewX(-18deg);
  animation: identity-auspicious-sweep 3.6s ease-in-out infinite;
}

.my-souls-table__icon {
  display: grid;
  width: 38px;
  height: 38px;
  flex: 0 0 38px;
  place-items: center;
  overflow: hidden;
  border: 1px solid var(--line);
  border-radius: 50%;
  color: var(--accent-deep);
  background: var(--paper);
  font-family: STKaiti, KaiTi, serif;
  font-size: 17px;
}

.my-souls-table__icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.my-souls-table__identity-copy {
  display: grid;
  min-width: 0;
  gap: 4px;
}

.my-souls-table__identity-copy strong,
.my-souls-table__identity-copy small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.my-souls-table__identity-copy strong {
  font-size: 13px;
}

.my-souls-table__identity-title {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 6px;
  overflow: visible !important;
  white-space: normal !important;
}

.my-souls-table__identity-title > span:first-child {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.my-souls-table__identity-copy small,
.my-souls-table__muted,
.my-souls-table__score small {
  color: var(--ink-soft);
  font-size: 10px;
}

.my-souls-table__level {
  color: var(--accent-deep);
  font-size: 15px;
  font-variant-numeric: tabular-nums;
  font-weight: 800;
}

.my-souls-table__attribute-column {
  display: grid;
  align-content: center;
  gap: 5px;
}

.my-souls-table__main-attribute {
  color: var(--accent-deep);
  font-weight: 800;
}

.my-souls-table__attribute-list {
  display: grid;
  gap: 4px;
}

.my-souls-table__attribute {
  display: flex;
  min-width: 0;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  padding: 3px 5px;
  border: 1px solid var(--line-soft);
  border-radius: 4px;
  color: var(--ink-soft);
  background: var(--paper);
  white-space: nowrap;
  transition:
    border-color 160ms ease,
    background 160ms ease,
    box-shadow 160ms ease;
}

.my-souls-table__attribute--max-roll {
  position: relative;
  overflow: hidden;
  border-color: rgb(184 121 35 / 58%);
  background: linear-gradient(100deg, rgb(255 243 198 / 90%), rgb(255 251 229 / 96%));
  box-shadow:
    inset 3px 0 0 #c13f2f,
    0 2px 8px rgb(184 121 35 / 16%);
  color: #7d3024;
  font-weight: 800;
}

.my-souls-table__attribute--max-roll::after {
  position: absolute;
  top: -20%;
  bottom: -20%;
  left: -35%;
  width: 24%;
  background: linear-gradient(100deg, transparent, rgb(255 255 255 / 92%), transparent);
  content: "";
  transform: skewX(-18deg);
  animation: max-roll-sweep 2.6s ease-in-out infinite;
}

.my-souls-table__attribute--max-roll > span {
  position: relative;
  z-index: 1;
}

.my-souls-table__attribute > span {
  overflow: hidden;
  text-overflow: ellipsis;
}

.my-souls-table__attribute--fixed {
  border-style: dashed;
  color: var(--accent-deep);
  background: rgb(174 118 83 / 7%);
}

.my-souls-table__attribute em {
  flex: 0 0 auto;
  color: var(--accent-deep);
  font-size: 9px;
  font-style: normal;
}

/* 强化次数角标直接呈现“+几手”，让用户能快速核对极品御魂的强化落点。 */
.my-souls-table__attribute b {
  display: inline-grid;
  min-width: 24px;
  min-height: 19px;
  flex: 0 0 auto;
  box-sizing: border-box;
  place-items: center;
  border: 1px solid var(--line);
  border-radius: 999px;
  color: var(--ink-soft);
  background: #fff;
  font-family: Consolas, "Cascadia Mono", monospace;
  font-size: 10px;
  line-height: 1;
}

.my-souls-table__roll--hit {
  border-color: rgb(83 123 111 / 42%);
  color: #fff;
  background: var(--jade);
}

/* +4 使用琥珀色，和 +5 的朱红金色拉开层级，便于快速判断强化落点。 */
.my-souls-table__roll--four {
  border-color: rgb(180 121 35 / 58%);
  color: #fff;
  background: #b87923;
  box-shadow: 0 2px 5px rgb(180 121 35 / 22%);
}

.my-souls-table__roll--five {
  min-width: 32px;
  min-height: 24px;
  border-width: 2px;
  border-color: rgb(164 54 38 / 70%);
  color: #fffaf0;
  background: linear-gradient(145deg, #c13f2f, #9f2f25);
  box-shadow:
    0 2px 7px rgb(164 54 38 / 35%),
    0 0 0 2px rgb(255 214 105 / 36%);
  font-size: 11px;
  font-weight: 900;
  transform: scale(1.08);
}

.my-souls-table__roll--unknown {
  border-style: dashed;
}

/* 大吉是本页唯一的庆祝性装饰，只给重点属性的 +5 使用，避免整张表变得嘈杂。 */
.my-souls-table__auspicious {
  display: inline-flex;
  position: relative;
  align-items: center;
  gap: 2px;
  flex: 0 0 auto;
  min-height: 24px;
  padding: 3px 9px;
  border: 2px solid #a83225;
  border-radius: 999px;
  color: #9f2f25;
  background: linear-gradient(
    120deg,
    rgb(255 238 185 / 96%) 0%,
    rgb(255 249 224 / 96%) 34%,
    rgb(255 216 112 / 98%) 50%,
    rgb(255 249 224 / 96%) 66%,
    rgb(255 238 185 / 96%) 100%
  );
  background-size: 240% 100%;
  box-shadow:
    0 2px 7px rgb(179 67 62 / 25%),
    0 0 0 2px rgb(255 214 105 / 32%);
  font-size: 11px;
  font-weight: 900;
  letter-spacing: 0.02em;
  white-space: nowrap;
  animation:
    auspicious-shimmer 2.8s ease-in-out infinite,
    auspicious-pulse 2.8s ease-in-out infinite;
}

/* 未强化标签使用冷色静态徽章，与大吉的庆祝性动效区分，便于快速识别 +0 御魂。 */
.my-souls-table__unenhanced {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  min-height: 22px;
  padding: 3px 7px;
  border: 1px solid rgb(76 118 108 / 42%);
  border-radius: 999px;
  color: #356b61;
  background: rgb(227 242 235 / 92%);
  box-shadow: inset 0 0 0 1px rgb(255 255 255 / 68%);
  font-size: 10px;
  font-weight: 800;
  line-height: 1;
  letter-spacing: 0.02em;
  white-space: nowrap;
}

/* 用身份列扫光、徽章扫光和满级属性扫光表达稀有强化，三种动画节奏错开避免刺眼。 */
@keyframes identity-auspicious-sweep {
  0%,
  22% {
    left: -25%;
  }

  68%,
  100% {
    left: 118%;
  }
}

@keyframes max-roll-sweep {
  0%,
  28% {
    left: -35%;
  }

  78%,
  100% {
    left: 125%;
  }
}

@keyframes auspicious-shimmer {
  0%,
  30% {
    background-position: 180% 0;
  }

  70%,
  100% {
    background-position: -80% 0;
  }
}

@keyframes auspicious-pulse {
  0%,
  100% {
    box-shadow: 0 1px 4px rgb(179 67 62 / 15%);
  }

  50% {
    box-shadow:
      0 1px 7px rgb(179 67 62 / 28%),
      0 0 0 2px rgb(255 214 105 / 20%);
  }
}

@keyframes score-tooltip-in {
  from {
    opacity: 0;
    transform: translateY(4px) scale(0.98);
  }

  to {
    opacity: 1;
    transform: translateY(0) scale(1);
  }
}

@keyframes score-legendary-sweep {
  0%,
  22% {
    left: -38%;
  }

  68%,
  100% {
    left: 132%;
  }
}

.my-souls-table__score {
  position: relative;
  isolation: isolate;
  display: grid;
  align-content: center;
  justify-items: center;
  gap: 5px;
  overflow: hidden;
  text-align: center;
  cursor: help;
  transition:
    background 180ms ease,
    box-shadow 180ms ease;
}

.my-souls-table__score:not(.my-souls-table__score--planned):hover,
.my-souls-table__score:not(.my-souls-table__score--planned):focus-visible {
  z-index: 2;
}

.my-souls-table__score strong {
  color: var(--ink-soft);
  font-size: 21px;
  font-variant-numeric: tabular-nums;
  line-height: 1.2;
}

.my-souls-table__score small {
  max-width: 90px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.my-souls-table__score-formula {
  color: var(--ink-faint);
}

.my-souls-table__score--planned strong {
  color: var(--ink-soft);
  font-family: inherit;
  font-size: 12px;
}

.my-souls-table__score--planned a,
.my-souls-table__detail-block a {
  color: var(--cinnabar);
  font-size: 10px;
  font-weight: 800;
  text-decoration: none;
}

.my-souls-table__score--planned a:hover,
.my-souls-table__detail-block a:hover {
  text-decoration: underline;
}

.my-souls-table__score--high strong {
  color: var(--jade);
}

/* 分数越高，色彩从青玉、金橙推进到炫彩虹光，让列表优先突出真正的高价值御魂。 */
.my-souls-table__score--high {
  background: linear-gradient(145deg, rgb(227 242 235 / 82%), rgb(255 250 231 / 84%));
  box-shadow: inset 0 0 0 1px rgb(83 123 111 / 18%);
}

.my-souls-table__score--epic {
  background: linear-gradient(145deg, rgb(220 242 238 / 92%), rgb(255 237 194 / 94%));
  box-shadow:
    inset 0 0 0 1px rgb(83 123 111 / 26%),
    0 0 14px rgb(217 155 44 / 16%);
}

.my-souls-table__score--epic strong {
  color: #b06c16;
  text-shadow: 0 1px 0 rgb(255 255 255 / 78%);
}

.my-souls-table__score--legendary {
  background: linear-gradient(
    125deg,
    rgb(214 241 237 / 96%),
    rgb(255 232 166 / 96%) 43%,
    rgb(244 197 219 / 92%) 72%,
    rgb(218 226 255 / 94%)
  );
  box-shadow:
    inset 0 0 0 1px rgb(163 111 198 / 32%),
    0 0 18px rgb(191 127 212 / 24%);
}

.my-souls-table__score--legendary::after {
  position: absolute;
  z-index: -1;
  top: -40%;
  left: -38%;
  width: 32%;
  height: 180%;
  background: linear-gradient(100deg, transparent, rgb(255 255 255 / 84%), transparent);
  content: "";
  transform: rotate(16deg);
  animation: score-legendary-sweep 3.8s ease-in-out infinite;
}

.my-souls-table__score--legendary strong {
  color: #8f4ba9;
  text-shadow:
    0 1px 0 rgb(255 255 255 / 90%),
    0 0 9px rgb(205 133 222 / 48%);
}

.my-souls-table__score--medium strong {
  color: #a36a18;
}

.my-souls-table__score--low strong {
  color: var(--cinnabar);
}

/* 标准评分悬浮卡片采用固定定位，不受表格横向滚动区域裁剪；核心贡献使用暖金色，一般贡献使用青玉色。 */
.my-souls-score-tooltip {
  position: fixed;
  z-index: 30;
  display: grid;
  width: min(330px, calc(100vw - 24px));
  max-height: min(320px, calc(100vh - 24px));
  gap: 9px;
  overflow-y: auto;
  padding: 14px;
  border: 1px solid rgb(166 126 55 / 32%);
  border-radius: 12px;
  color: var(--ink-soft);
  background: rgb(255 253 247 / 97%);
  box-shadow:
    0 16px 34px rgb(75 57 33 / 22%),
    0 0 0 3px rgb(255 255 255 / 54%);
  font-size: 11px;
  line-height: 1.45;
  pointer-events: none;
  animation: score-tooltip-in 140ms ease-out both;
}

.my-souls-score-tooltip--high {
  border-color: rgb(83 123 111 / 45%);
  background: linear-gradient(145deg, rgb(247 255 249 / 98%), rgb(255 251 231 / 98%));
}

.my-souls-score-tooltip--epic {
  border-color: rgb(199 143 44 / 54%);
  background: linear-gradient(145deg, rgb(244 255 249 / 98%), rgb(255 244 208 / 98%));
  box-shadow:
    0 16px 34px rgb(75 57 33 / 22%),
    0 0 18px rgb(217 155 44 / 24%);
}

.my-souls-score-tooltip--legendary {
  border-color: rgb(159 104 181 / 56%);
  background: linear-gradient(
    145deg,
    rgb(242 255 252 / 98%),
    rgb(255 245 205 / 98%) 46%,
    rgb(255 235 250 / 98%)
  );
  box-shadow:
    0 16px 34px rgb(75 57 33 / 22%),
    0 0 24px rgb(191 127 212 / 34%);
}

.my-souls-score-tooltip__heading {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
  color: var(--ink-soft);
  font-weight: 800;
}

.my-souls-score-tooltip__heading strong {
  color: var(--jade);
  font-size: 24px;
  font-variant-numeric: tabular-nums;
  line-height: 1.2;
}

.my-souls-score-tooltip--epic .my-souls-score-tooltip__heading strong {
  color: #b06c16;
}

.my-souls-score-tooltip--legendary .my-souls-score-tooltip__heading strong {
  color: #8f4ba9;
  text-shadow: 0 0 9px rgb(205 133 222 / 48%);
}

.my-souls-score-tooltip__rule {
  padding-bottom: 8px;
  border-bottom: 1px solid rgb(166 126 55 / 20%);
}

.my-souls-score-tooltip__rule b {
  color: var(--ink);
}

.my-souls-score-tooltip__contributions {
  display: grid;
  gap: 5px;
}

.my-souls-score-tooltip__contribution {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 9px;
  padding: 5px 7px;
  border-radius: 7px;
  background: rgb(83 123 111 / 8%);
}

.my-souls-score-tooltip__contribution b {
  flex: 0 0 auto;
}

.my-souls-score-tooltip__contribution span {
  color: var(--ink-soft);
  text-align: right;
}

.my-souls-score-tooltip__contribution--core {
  border-left: 3px solid #c47b22;
  background: linear-gradient(90deg, rgb(255 224 154 / 58%), rgb(255 255 255 / 34%));
}

.my-souls-score-tooltip__contribution--core b {
  color: #a55c13;
}

.my-souls-score-tooltip__contribution--general {
  border-left: 3px solid #4e8c7d;
  background: linear-gradient(90deg, rgb(208 238 229 / 62%), rgb(255 255 255 / 34%));
}

.my-souls-score-tooltip__contribution--general b {
  color: #356b61;
}

.my-souls-score-tooltip__note {
  color: var(--ink-faint);
}

.my-souls-table__action {
  justify-content: center;
  border-right: 0;
}

.my-souls-table__detail {
  grid-column: 1 / -1;
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 16px;
  padding: 14px 18px 16px 48px;
  border-top: 1px dashed var(--line);
  color: var(--ink-soft);
  background: linear-gradient(110deg, rgb(235 246 237 / 70%), rgb(255 250 240 / 90%));
}

.my-souls-table__detail-block {
  display: grid;
  align-content: start;
  gap: 5px;
}

.my-souls-table__detail-block strong {
  color: var(--ink);
  font-size: 12px;
}

.my-souls-table__detail-block p {
  margin: 0;
  font-size: 11px;
  line-height: 1.55;
}

.my-souls-table__roll-summary {
  display: inline-block;
  margin-right: 10px;
}

.empty-state-card {
  display: grid;
  gap: 8px;
  padding: 30px;
  border: 1px dashed var(--line);
  border-radius: 10px;
  color: var(--ink-soft);
  background: rgb(255 250 240 / 58%);
  text-align: center;
}

/* 空库存使用“读”字印章作为行动提示，和数据读取入口的本地工作台气质保持一致。 */
.empty-state-card__mark {
  display: grid;
  width: 42px;
  height: 42px;
  margin: 0 auto 2px;
  place-items: center;
  border: 1px solid rgb(174 118 83 / 42%);
  border-radius: 50%;
  color: var(--cinnabar);
  background: rgb(255 239 198 / 62%);
  font-family: STKaiti, KaiTi, serif;
  font-size: 20px;
  transform: rotate(-6deg);
}

.empty-state-card strong {
  color: var(--ink);
  font-size: 15px;
}

.empty-state-card p {
  margin: 0;
  font-size: 12px;
}

.my-souls-pagination {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 14px;
  margin-top: 18px;
  color: var(--ink-soft);
  font-size: 12px;
}

/* 数值筛选的追加入口与普通页码分开，明确用户正在浏览排序后的结果流。 */
.my-souls-load-more {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  margin-top: 18px;
  color: var(--ink-soft);
  font-size: 12px;
}

@media (max-width: 760px) {
  .my-souls-summary {
    grid-template-columns: 1fr;
  }

  .my-souls-score-prompt {
    grid-template-columns: auto minmax(0, 1fr);
  }

  .my-souls-score-prompt .button {
    grid-column: 1 / -1;
    width: 100%;
  }

  .my-souls__header-actions {
    align-items: stretch;
    justify-content: stretch;
  }

  .my-souls__header-actions .button,
  .my-souls__header-actions .health-badge {
    justify-content: center;
    width: 100%;
  }

  .my-souls-filters {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .my-souls-filter--attribute-value {
    grid-column: 1 / -1;
  }

  /* 手机视口进一步压缩列宽，让评分列仍留在同一屏，不把整页推到横向滚动区。 */
  .my-souls-table {
    min-width: 0;
  }

  .my-souls-table__head,
  .my-souls-table__row {
    grid-template-columns:
      24px minmax(82px, 1.05fr) 36px 36px 36px minmax(100px, 1.1fr)
      minmax(150px, 1.55fr) 64px 68px 64px;
  }

  .my-souls-table__detail {
    grid-template-columns: 1fr;
    padding-left: 18px;
  }

  .my-souls-toolbar {
    align-items: stretch;
  }

  .my-souls-toolbar__meta {
    justify-content: space-between;
  }
}

/* 尊重系统的减少动态效果设置，保留喜庆配色但关闭扫光和呼吸动画。 */
@media (prefers-reduced-motion: reduce) {
  .my-souls-table__identity--auspicious::after,
  .my-souls-table__attribute--max-roll::after,
  .my-souls-table__auspicious,
  .my-souls-table__score--legendary::after,
  .my-souls-score-tooltip {
    animation: none;
    background-position: center;
  }
}
</style>
