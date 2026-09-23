<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import type {
  ScoreColorThresholds,
  SimulateEnhancementResult,
  SimulationAttribute,
  SimulationRoll,
  SimulatedSoulResult,
  SoulSet,
} from "../api/contracts";
import { normalizeCommandError } from "../api/errors";
import SoulSetPicker from "./SoulSetPicker.vue";
import CharacterArchiveSwitcher from "./CharacterArchiveSwitcher.vue";
import { onActiveCharacterChanged } from "../state/activeCharacter";

// 图标沿用御魂目录的离线资源，模拟页面不复制或上传玩家库存数据。
const iconModules = import.meta.glob("../assets/soul-icons/*.png", {
  eager: true,
  import: "default",
  query: "?url",
});

// 主属性筛选顺序与“我的御魂”保持一致，固定属性即使本批次没有结果也不会从选项中消失。
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
// 评分配置缺失时沿用规则引擎的默认彩虹色阈值；正常情况会在页面加载时被启用标准覆盖。
const DEFAULT_SCORE_COLOR_THRESHOLDS: ScoreColorThresholds = {
  jadeScore: 60,
  goldScore: 70,
  rainbowScore: 75,
};
// 烟花粒子使用固定方向和配色，保证同一批模拟结果可复现且不会引入额外随机状态。
const FIREWORK_COLORS = ["#f2b84b", "#d75b42", "#e4cf72"] as const;
const FIREWORK_SPARKS = Array.from({ length: 12 }, (_, index) => {
  const angle = (index / 12) * Math.PI * 2;
  const distance = 24 + (index % 3) * 7;
  return {
    key: index,
    angle: `${Math.round((angle * 180) / Math.PI)}deg`,
    x: `${Math.round(Math.cos(angle) * distance)}px`,
    y: `${Math.round(Math.sin(angle) * distance)}px`,
    delay: `${(index % 4) * 35}ms`,
    color: FIREWORK_COLORS[index % FIREWORK_COLORS.length] ?? FIREWORK_COLORS[0],
  };
});
const sets = ref<SoulSet[]>([]);
const scoreColorThresholds = ref<ScoreColorThresholds>({
  ...DEFAULT_SCORE_COLOR_THRESHOLDS,
});
const activeScoreStandardTitle = ref("内置评分标准");
const threshold = ref(DEFAULT_SCORE_COLOR_THRESHOLDS.rainbowScore);
const loading = ref(true);
const simulating = ref(false);
const error = ref("");
const notice = ref("");
const simulation = ref<SimulateEnhancementResult | null>(null);
const expandedSoulKey = ref<string | null>(null);
// 模拟结果已经全部在当前页面内存中，查询条件只做本地 AND 过滤，不触碰真实库存。
const simulationFilters = ref({
  keyword: "",
  setId: "",
  slot: "",
  mainAttrType: "",
  subAttrType: "",
  standardScoreMin: "",
  fireworkOnly: false,
});

const setById = computed(() => new Map(sets.value.map((set) => [set.setId, set])));
const hasResults = computed(() => Boolean(simulation.value?.results.length));
const qualityResults = computed(() =>
  [...(simulation.value?.results ?? [])].sort(
    (left, right) =>
      right.standardScore - left.standardScore ||
      (right.initialSubstatCount ?? 0) - (left.initialSubstatCount ?? 0) ||
      left.soulKey.localeCompare(right.soulKey),
  ),
);
const simulationSetOptions = computed(() => {
  const options = new Map<string, string>();
  for (const result of simulation.value?.results ?? []) {
    options.set(result.setId, result.setName);
  }
  return [...options.entries()]
    .map(([setId, name]) => {
      const set = setById.value.get(setId);
      return {
        setId,
        name,
        iconAssetId: set?.iconAssetId ?? null,
        category: set?.category ?? null,
        specialCategory: set?.specialCategory ?? null,
      };
    })
    .sort((left, right) => left.name.localeCompare(right.name, "zh-CN"));
});
// 一级分类只负责收窄二级御魂列表；模拟结果仍由具体 setId 进行精确筛选。
const selectedSetCategory = ref("all");
const simulationSetCategories = computed(() => {
  const categories = new Set<string>();
  for (const option of simulationSetOptions.value) {
    const category = option.specialCategory ?? option.category;
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
const filteredSimulationSetOptions = computed(() => {
  if (selectedSetCategory.value === "all") return simulationSetOptions.value;
  return simulationSetOptions.value.filter(
    (option) =>
      (option.specialCategory ?? option.category) === selectedSetCategory.value,
  );
});

/** 切换一级分类时清除失效的二级选择，保证候选套装和当前值始终属于同一分类。 */
function handleSimulationSetCategoryChange(): void {
  if (
    simulationFilters.value.setId &&
    !filteredSimulationSetOptions.value.some(
      (option) => option.setId === simulationFilters.value.setId,
    )
  ) {
    simulationFilters.value.setId = "";
  }
}

/** 选择具体御魂时同步一级套装效果，避免两个筛选条件出现交叉或空结果。 */
function handleSimulationSetSelectionChange(setId: string): void {
  simulationFilters.value.setId = setId;
  if (!setId) return;
  const selected = setById.value.get(setId);
  const category = selected?.specialCategory ?? selected?.category;
  if (category) selectedSetCategory.value = category;
}
const simulationSlotOptions = computed(() =>
  [...new Set((simulation.value?.results ?? []).map((result) => result.slot))].sort(
    (left, right) => left - right,
  ),
);
const simulationSubAttributeOptions = computed(() => {
  const attributeTypes = new Set(
    (simulation.value?.results ?? []).flatMap((result) =>
      result.finalAttributes.map((attribute) => attribute.attributeType),
    ),
  );
  return [...attributeTypes].sort((left, right) =>
    attributeLabel(left).localeCompare(attributeLabel(right), "zh-CN"),
  );
});
const hasSimulationFilters = computed(
  () =>
    Object.values(simulationFilters.value).some(Boolean) ||
    selectedSetCategory.value !== "all",
);
const filteredSimulationResults = computed(() => {
  const filters = simulationFilters.value;
  const keyword = filters.keyword.trim().toLocaleLowerCase("zh-CN");
  const scoreMin = Number(filters.standardScoreMin);
  const categorySetIds = new Set(
    filteredSimulationSetOptions.value.map((option) => option.setId),
  );

  return qualityResults.value.filter((result) => {
    if (selectedSetCategory.value !== "all" && !categorySetIds.has(result.setId)) {
      return false;
    }
    if (filters.setId && result.setId !== filters.setId) return false;
    if (filters.slot && result.slot !== Number(filters.slot)) return false;
    if (filters.mainAttrType && result.mainAttrType !== filters.mainAttrType) {
      return false;
    }
    if (
      filters.subAttrType &&
      !result.finalAttributes.some(
        (attribute) => attribute.attributeType === filters.subAttrType,
      )
    ) {
      return false;
    }
    if (
      filters.standardScoreMin !== "" &&
      (!Number.isFinite(scoreMin) || result.standardScore <= scoreMin)
    ) {
      return false;
    }
    if (filters.fireworkOnly && !result.firework) return false;
    if (keyword) {
      const searchableText = [
        result.soulKey,
        result.setName,
        `${result.slot}号位`,
        attributeLabel(result.mainAttrType),
        ...result.finalAttributes.flatMap((attribute) => [
          attribute.attributeLabel,
          attribute.attributeType,
        ]),
      ]
        .join(" ")
        .toLocaleLowerCase("zh-CN");
      if (!searchableText.includes(keyword)) return false;
    }
    return true;
  });
});
const averageScore = computed(() => {
  const results = simulation.value?.results ?? [];
  if (!results.length) return 0;
  return (
    results.reduce((sum, result) => sum + result.standardScore, 0) / results.length
  );
});
const fireworkCount = computed(
  () => simulation.value?.results.filter((result) => result.firework).length ?? 0,
);

/** 返回套装离线图标；目录没有覆盖时保留文字印章，避免空白身份列。 */
function soulIcon(result: SimulatedSoulResult): string | null {
  const iconAssetId = setById.value.get(result.setId)?.iconAssetId;
  if (!iconAssetId) return null;
  return iconModules[`../assets/soul-icons/${iconAssetId}.png`] ?? null;
}

/** 将稳定属性键转换为玩家熟悉的中文名称。 */
function attributeLabel(attributeType: string): string {
  const labels: Record<string, string> = {
    attack_rate: "攻击加成",
    attack_flat: "攻击",
    hp_rate: "生命加成",
    hp_flat: "生命",
    defense_rate: "防御加成",
    defense_flat: "防御",
    speed: "速度",
    effect_hit: "效果命中",
    effect_resist: "效果抵抗",
    crit_rate: "暴击",
    crit_damage: "暴击伤害",
  };
  return labels[attributeType] ?? attributeType;
}

/** 复用正式御魂页的属性数值规则，百分比属性按百分数、平坦属性按原值展示。 */
function formatAttributeValue(attributeType: string, value: number): string {
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

/** 按属性单位格式化模拟后的最终值；没有成长基准时才提示数值未生成。 */
function formatSimulationAttributeValue(attribute: SimulationAttribute): string {
  return attribute.value === 0 && attribute.valueMin === 0 && attribute.valueMax === 0
    ? "数值未生成"
    : formatAttributeValue(attribute.attributeType, attribute.value);
}

/** 将汇总卡片的大数字压缩为万或亿，避免数字过长挤压卡片布局。 */
function formatNumber(value: number): string {
  const absoluteValue = Math.abs(value);
  if (absoluteValue >= 100_000_000) {
    const compactValue = value / 100_000_000;
    return `${compactValue.toFixed(compactValue >= 10 ? 1 : 2).replace(/\.?0+$/, "")}亿`;
  }
  if (absoluteValue >= 10_000) {
    const compactValue = value / 10_000;
    return `${compactValue.toFixed(compactValue >= 10 ? 1 : 2).replace(/\.?0+$/, "")}万`;
  }
  return new Intl.NumberFormat("zh-CN").format(value);
}

/** 将 PDF 给出的单次最低值至最高值累加区间格式化，帮助玩家区分实抽值和理论运气范围。 */
function formatSimulationAttributeRange(attribute: SimulationAttribute): string {
  if (attribute.valueMin === 0 && attribute.valueMax === 0) return "数值区间未知";
  const min = formatAttributeValue(attribute.attributeType, attribute.valueMin);
  const max = formatAttributeValue(attribute.attributeType, attribute.valueMax);
  return min === max ? min : `${min}～${max}`;
}

/** 展示每个强化节点本次抽到的成长值；节点等级和成长数值分开，避免误读为固定值。 */
function formatSimulationRollValue(roll: SimulationRoll): string {
  return formatAttributeValue(roll.attributeType, roll.valueDelta);
}

/** 将固定粒子方向转换为 CSS 自定义属性，供一次性烟花动画使用。 */
function fireworkSparkStyle(
  spark: (typeof FIREWORK_SPARKS)[number],
): Record<string, string> {
  return {
    "--spark-angle": spark.angle,
    "--spark-x": spark.x,
    "--spark-y": spark.y,
    "--spark-delay": spark.delay,
    "--spark-color": spark.color,
  };
}

/** 评分统一保留一位小数，与“我的御魂”页面的标准评分展示保持一致。 */
function formatScore(value: number): string {
  return value.toFixed(1);
}

/** 让结果详情保持单行可扫描，同时完整保留每个节点的成长轨迹。 */
function rollSummary(result: SimulatedSoulResult): string {
  return result.rolls
    .map(
      (roll) =>
        `+${roll.checkpoint} ${roll.attributeLabel} ${formatSimulationRollValue(roll)} ${
          roll.outcome === "newAttribute" ? "新腿" : "+1"
        }`,
    )
    .join(" · ");
}

/** 只允许打开一枚详情，避免批量结果展开后页面高度失控。 */
function toggleDetails(soulKey: string): void {
  expandedSoulKey.value = expandedSoulKey.value === soulKey ? null : soulKey;
}

/** 清除模拟结果查询条件；只恢复当前页面视图，不重新发起模拟。 */
function clearSimulationFilters(): void {
  selectedSetCategory.value = "all";
  simulationFilters.value = {
    keyword: "",
    setId: "",
    slot: "",
    mainAttrType: "",
    subAttrType: "",
    standardScoreMin: "",
    fireworkOnly: false,
  };
}

/** 在重新计算前断开上一批结果的引用，释放结果数组、轨迹和筛选派生状态，避免连续模拟累积内存。 */
function clearSimulationMemory(): void {
  simulation.value = null;
  expandedSoulKey.value = null;
  clearSimulationFilters();
}

let stopActiveCharacterChanged: (() => void) | null = null;
onBeforeUnmount(() => {
  stopActiveCharacterChanged?.();
  stopActiveCharacterChanged = null;
});

/** 将用户带回“我的御魂”确认读取结果；读取入口统一放在数据读取分组中。 */
function openMySouls(): void {
  window.dispatchEvent(new CustomEvent("open-my-souls"));
}

/** 加载目录状态和套装名称；失败时模拟按钮保持禁用，避免结果缺少身份信息。 */
async function loadCatalog(): Promise<void> {
  loading.value = true;
  error.value = "";
  try {
    let catalog = await desktopApi.getCatalogStatus();
    // 只在没有目录或目录校验不合格时安装内置种子；已安装的 Gitee 目录直接按活动版本读取。
    if (!catalog) {
      catalog = await desktopApi.installBuiltinCatalog();
    }
    sets.value = await desktopApi.listSoulSets(catalog.version);
    await loadScoreThreshold();
  } catch (caught) {
    const commandError = normalizeCommandError(caught);
    error.value = `${commandError.code} · ${commandError.message}`;
  } finally {
    loading.value = false;
  }
}

/** 读取当前启用评分标准的彩虹色阈值，作为本轮烟花阈值的初始值。 */
async function loadScoreThreshold(): Promise<void> {
  try {
    const standards = await desktopApi.listScoreStandards();
    const enabled = standards.find((standard) => standard.enabled);
    if (!enabled) {
      scoreColorThresholds.value = { ...DEFAULT_SCORE_COLOR_THRESHOLDS };
      threshold.value = DEFAULT_SCORE_COLOR_THRESHOLDS.rainbowScore;
      activeScoreStandardTitle.value = "内置评分标准";
      return;
    }
    scoreColorThresholds.value = enabled.scoreColorThresholds;
    threshold.value = enabled.scoreColorThresholds.rainbowScore;
    activeScoreStandardTitle.value = enabled.title;
  } catch (caught) {
    // 评分配置读取失败不阻断模拟；回退值与标准评分引擎默认配置保持一致并明确提示。
    scoreColorThresholds.value = { ...DEFAULT_SCORE_COLOR_THRESHOLDS };
    threshold.value = DEFAULT_SCORE_COLOR_THRESHOLDS.rainbowScore;
    activeScoreStandardTitle.value = "内置评分标准（配置读取失败）";
    const commandError = normalizeCommandError(caught);
    notice.value = `未读取到当前评分光效配置，已使用彩虹色默认阈值 ${threshold.value} 分（${commandError.message}）`;
  }
}

/** 触发整批 +15 模拟；结果只落在组件内存中，不触发正式分析重算。 */
async function simulate(): Promise<void> {
  if (simulating.value) return;
  const normalizedThreshold = Number(threshold.value);
  if (
    !Number.isFinite(normalizedThreshold) ||
    normalizedThreshold < 0 ||
    normalizedThreshold > 100
  ) {
    error.value = "烟花阈值必须在 0 到 100 分之间。";
    return;
  }
  simulating.value = true;
  error.value = "";
  notice.value = "";
  // 先清空旧批次再调用后端；即使本次请求失败，页面也不会继续持有上一批大结果集。
  clearSimulationMemory();
  try {
    simulation.value = await desktopApi.simulateEnhancement({
      threshold: normalizedThreshold,
    });
    notice.value = simulation.value.eligibleCount
      ? `已完成 ${simulation.value.eligibleCount} 枚六星未强化御魂的娱乐模拟。`
      : "当前没有符合条件的六星未强化御魂。";
  } catch (caught) {
    const commandError = normalizeCommandError(caught);
    error.value = `${commandError.code} · ${commandError.message}`;
  } finally {
    simulating.value = false;
  }
}

onMounted(() => {
  void loadCatalog();
  stopActiveCharacterChanged = onActiveCharacterChanged(() => {
    clearSimulationMemory();
    notice.value = "";
    void loadCatalog();
  });
});
</script>

<template>
  <main class="workspace simulation-workbench">
    <header class="page-header simulation-workbench__header">
      <div>
        <p class="eyebrow">SIMULATED FORGE / 03</p>
        <h1>模拟强化</h1>
        <p class="lede">
          把六星未强化御魂一次推演到 +15，看看哪一枚值得期待。
          <span class="simulation-workbench__disclaimer">仅供娱乐，不算实际强化。</span>
        </p>
      </div>
      <div class="simulation-workbench__actions">
        <CharacterArchiveSwitcher />
        <label class="simulation-threshold">
          <span>烟花阈值</span>
          <input
            v-model.number="threshold"
            class="text-input"
            type="number"
            min="0"
            max="100"
            step="1"
          />
          <b>分</b>
          <small
            >{{ activeScoreStandardTitle }} · 彩虹色 ≥
            {{ scoreColorThresholds.rainbowScore }} 分</small
          >
        </label>
        <button
          class="button button--primary simulation-launch"
          type="button"
          :disabled="loading || simulating"
          @click="simulate"
        >
          {{ simulating ? "正在推演…" : "一键强化至 +15" }}
        </button>
      </div>
    </header>

    <section v-if="error" class="error-panel" role="alert">
      <div>
        <p class="error-panel__title">模拟强化未完成</p>
        <p>{{ error }}</p>
      </div>
      <button class="button button--quiet" type="button" @click="loadCatalog">
        重新加载
      </button>
    </section>
    <p v-if="notice" class="success-panel" role="status">{{ notice }}</p>

    <section class="simulation-notice" aria-label="模拟说明">
      <div class="simulation-notice__seal" aria-hidden="true">娱</div>
      <div>
        <strong>这是一次纸上推演</strong>
        <p>
          模拟强化仅供娱乐，不计入实际库存、金币或御魂材料。概率按网易官方公示模拟；结果不会修改御魂库存，也不会扣除真实资源。
        </p>
        <small
          >烟花阈值默认读取当前御魂评分配置的“彩虹色”阈值；手动调整只对本轮模拟生效。</small
        >
      </div>
      <a
        href="https://yys.163.com/news/notice/2017/05/01/25369_665793.html"
        target="_blank"
        rel="noreferrer"
      >
        查看官方概率
      </a>
    </section>

    <section v-if="loading" class="placeholder-card">正在读取御魂目录…</section>

    <template v-else>
      <section v-if="simulation" class="simulation-summary" aria-label="模拟汇总">
        <div class="simulation-stat simulation-stat--primary">
          <span>参与御魂</span>
          <strong>{{ formatNumber(simulation.eligibleCount) }}</strong>
          <small>六星 · +0 · 未移除</small>
        </div>
        <div class="simulation-stat">
          <span>模拟强化次数</span>
          <strong>{{ formatNumber(simulation.totalRolls) }}</strong>
          <small>每枚固定五个节点</small>
        </div>
        <div class="simulation-stat simulation-stat--gold">
          <span>金币估算</span>
          <strong>{{ formatNumber(simulation.totalGold) }}</strong>
          <small>娱乐估算，不会真实扣除</small>
        </div>
        <div class="simulation-stat simulation-stat--jade">
          <span>御魂经验估算</span>
          <strong>{{ formatNumber(simulation.totalExp) }}</strong>
          <small>娱乐估算，不会真实扣除</small>
        </div>
      </section>

      <section v-if="simulation && hasResults" class="simulation-meta-strip">
        <span
          >平均标准评分 <b>{{ formatScore(averageScore) }}</b></span
        >
        <span
          >达到阈值 <b>{{ fireworkCount }}</b> 枚</span
        >
        <span
          >模拟种子 <code>{{ simulation.seed }}</code></span
        >
        <span>规则 {{ simulation.cost.mechanicsVersion }}</span>
      </section>

      <section
        v-if="simulation && hasResults"
        class="simulation-results-shell"
        aria-label="模拟强化结果"
      >
        <div class="simulation-results-heading">
          <div>
            <p class="section-kicker">RESULTS · +15</p>
            <h2>按标准评分排序</h2>
          </div>
          <span class="simulation-results-heading__hint"
            >最高分优先 · 同分按御魂稳定键排序</span
          >
        </div>

        <section class="simulation-query-toolbar" aria-label="查询模拟强化结果">
          <div class="simulation-query-toolbar__filters">
            <label class="simulation-query-filter simulation-query-filter--keyword">
              <span>关键词</span>
              <input
                v-model="simulationFilters.keyword"
                type="search"
                placeholder="套装、属性或御魂键"
              />
            </label>
            <label class="simulation-query-filter">
              <span>套装效果</span>
              <select
                v-model="selectedSetCategory"
                @change="handleSimulationSetCategoryChange"
              >
                <option
                  v-for="category in simulationSetCategories"
                  :key="category.key"
                  :value="category.key"
                >
                  {{ category.label }}
                </option>
              </select>
            </label>
            <label class="simulation-query-filter">
              <span>御魂</span>
              <SoulSetPicker
                :model-value="simulationFilters.setId"
                :options="filteredSimulationSetOptions"
                empty-label="全部御魂"
                :show-categories="false"
                @update:model-value="handleSimulationSetSelectionChange"
              />
            </label>
            <label class="simulation-query-filter">
              <span>号位</span>
              <select v-model="simulationFilters.slot">
                <option value="">全部号位</option>
                <option v-for="slot in simulationSlotOptions" :key="slot" :value="slot">
                  {{ slot }}号位
                </option>
              </select>
            </label>
            <label class="simulation-query-filter">
              <span>主属性</span>
              <select v-model="simulationFilters.mainAttrType">
                <option value="">全部主属性</option>
                <option
                  v-for="attributeType in MAIN_ATTRIBUTE_ORDER"
                  :key="attributeType"
                  :value="attributeType"
                >
                  {{ attributeLabel(attributeType) }}
                </option>
              </select>
            </label>
            <label class="simulation-query-filter">
              <span>副属性包含</span>
              <select v-model="simulationFilters.subAttrType">
                <option value="">不限副属性</option>
                <option
                  v-for="attributeType in simulationSubAttributeOptions"
                  :key="attributeType"
                  :value="attributeType"
                >
                  {{ attributeLabel(attributeType) }}
                </option>
              </select>
            </label>
            <label class="simulation-query-filter simulation-query-filter--score">
              <span>标准评分大于</span>
              <div class="simulation-query-filter__input-wrap">
                <input
                  v-model="simulationFilters.standardScoreMin"
                  type="number"
                  min="0"
                  max="100"
                  step="0.1"
                  placeholder="不限"
                  aria-label="模拟结果标准评分大于多少分"
                />
                <small>分</small>
              </div>
            </label>
            <label class="simulation-query-filter simulation-query-filter--check">
              <span>特殊标识</span>
              <span class="simulation-query-filter__check-control">
                <input v-model="simulationFilters.fireworkOnly" type="checkbox" />
                <b>只看烟花</b>
              </span>
            </label>
          </div>
          <div class="simulation-query-toolbar__meta">
            <span
              >显示 {{ filteredSimulationResults.length }} /
              {{ qualityResults.length }}</span
            >
            <button
              v-if="hasSimulationFilters"
              class="button button--quiet button--small"
              type="button"
              @click="clearSimulationFilters"
            >
              清除条件
            </button>
          </div>
        </section>

        <div
          class="simulation-results-table"
          role="table"
          aria-label="模拟强化排序结果"
        >
          <div class="simulation-results-table__head" role="row">
            <span>#</span>
            <span>御魂</span>
            <span>最终主属性</span>
            <span>强化轨迹</span>
            <span>最终副属性</span>
            <span>标准评分</span>
            <span>状态</span>
          </div>
          <article
            v-for="(result, index) in filteredSimulationResults"
            :key="`${simulation.seed}-${result.soulKey}`"
            class="simulation-result-row"
            :class="{ 'simulation-result-row--firework': result.firework }"
          >
            <span class="simulation-result-row__rank">{{ index + 1 }}</span>
            <div class="simulation-result-row__identity">
              <span class="simulation-result-row__icon" aria-hidden="true">
                <img v-if="soulIcon(result)" :src="soulIcon(result)!" alt="" />
                <span v-else>{{ result.setName.slice(0, 1) }}</span>
              </span>
              <span>
                <strong>{{ result.setName }}</strong>
                <small
                  >{{ result.slot }}号 · {{ result.initialSubstatCount ?? "—" }}腿 ·
                  模拟 +15</small
                >
              </span>
            </div>
            <div class="simulation-result-row__main-attribute">
              <span class="detail-label">最终主属性</span>
              <strong>{{ attributeLabel(result.mainAttrType) }}</strong>
              <small>{{
                formatAttributeValue(result.mainAttrType, result.mainAttrValue)
              }}</small>
            </div>
            <div class="simulation-result-row__rolls" :title="rollSummary(result)">
              <span
                v-for="roll in result.rolls"
                :key="`${result.soulKey}-${roll.checkpoint}`"
                :class="{ 'is-new': roll.outcome === 'newAttribute' }"
              >
                +{{ roll.checkpoint }}
                <b>{{ roll.attributeLabel }}</b>
                <small>{{ formatSimulationRollValue(roll) }}</small>
              </span>
            </div>
            <div class="simulation-result-row__attributes">
              <!-- 最终副属性按“属性名 + 最终数值 / 下一行强化次数”展示，贴近游戏面板阅读习惯。 -->
              <span
                v-for="attribute in result.finalAttributes"
                :key="attribute.attributeType"
                class="simulation-attribute-chip"
              >
                <span class="simulation-attribute-chip__value">
                  {{ attribute.attributeLabel }}
                  <b>{{ formatSimulationAttributeValue(attribute) }}</b>
                </span>
                <strong class="simulation-attribute-chip__roll">
                  +{{ attribute.enhancementCount }}
                </strong>
                <small class="simulation-attribute-chip__range">
                  {{ formatSimulationAttributeRange(attribute) }}
                </small>
              </span>
            </div>
            <div class="simulation-result-row__score">
              <strong>{{ formatScore(result.standardScore) }}</strong>
              <small>{{ result.standardScoreRule }}</small>
            </div>
            <div class="simulation-result-row__status">
              <span v-if="result.firework" class="firework-burst" aria-hidden="true">
                <i
                  v-for="spark in FIREWORK_SPARKS"
                  :key="`${result.soulKey}-spark-${spark.key}`"
                  class="firework-burst__spark"
                  :style="fireworkSparkStyle(spark)"
                ></i>
                <b class="firework-burst__core">✦</b>
              </span>
              <span
                v-if="result.firework"
                class="firework-badge"
                aria-label="达到烟花阈值"
                >✦ 烟花</span
              >
              <button
                class="button button--quiet button--small"
                type="button"
                :aria-expanded="expandedSoulKey === result.soulKey"
                @click="toggleDetails(result.soulKey)"
              >
                {{ expandedSoulKey === result.soulKey ? "收起" : "轨迹" }}
              </button>
            </div>
            <div
              v-if="expandedSoulKey === result.soulKey"
              class="simulation-result-row__detail"
            >
              <div>
                <span class="detail-label">完整节点</span>
                <p>{{ rollSummary(result) }}</p>
              </div>
              <div>
                <span class="detail-label">评分公式</span>
                <p>{{ result.standardScoreFormula }}</p>
              </div>
              <div>
                <span class="detail-label">副属性说明</span>
                <p>
                  <span
                    v-for="attribute in result.finalAttributes"
                    :key="`detail-${attribute.attributeType}`"
                  >
                    {{ attributeLabel(attribute.attributeType) }}：{{
                      formatSimulationAttributeValue(attribute)
                    }}，强化次数 {{ attribute.enhancementCount }}；
                  </span>
                </p>
              </div>
              <div>
                <span class="detail-label">数值模型</span>
                <p>
                  单次成长按参考文档的最低值～最高值区间模拟；当前值：
                  {{
                    result.rolls
                      .map(
                        (roll) =>
                          `+${roll.checkpoint} ${roll.attributeLabel} ${formatSimulationRollValue(roll)}`,
                      )
                      .join(" · ")
                  }}
                </p>
              </div>
            </div>
          </article>
        </div>
        <div v-if="!filteredSimulationResults.length" class="simulation-filter-empty">
          <strong>没有匹配的模拟结果</strong>
          <p>可以清除部分条件，或重新组合套装、号位和属性筛选。</p>
          <button
            v-if="hasSimulationFilters"
            class="button button--quiet"
            type="button"
            @click="clearSimulationFilters"
          >
            清除条件
          </button>
        </div>
      </section>

      <section v-else-if="simulation" class="simulation-empty-card">
        <span class="simulation-empty-card__mark">+0</span>
        <div>
          <h2>没有找到六星未强化御魂</h2>
          <p>请先读取当前库存，或回到“御魂分析”确认是否已有六星 +0 御魂。</p>
          <button class="button button--primary" type="button" @click="openMySouls">
            查看御魂分析
          </button>
        </div>
      </section>

      <section v-else class="simulation-empty-card simulation-empty-card--first">
        <span class="simulation-empty-card__mark">五</span>
        <div>
          <h2>准备好试试手气了吗？</h2>
          <p>
            一键读取所有六星 +0 御魂，按官方节点概率推演到
            +15，再按标准评分找出最漂亮的结果。
          </p>
          <button class="button button--primary" type="button" @click="simulate">
            开始模拟强化
          </button>
        </div>
      </section>

      <details class="simulation-evidence">
        <summary>概率与成本说明</summary>
        <div class="simulation-evidence__grid">
          <div>
            <strong>概率规则</strong>
            <p>
              已有四条副属性时，每条 25%；不足四条时，攻击类 36%、防御类 36%、功能类
              28%。
            </p>
            <p>类别内具体属性按等概率娱乐模拟：攻击/防御类各 9%，功能类各约 9.33%。</p>
            <p>
              数值成长按参考文档的单次最低值～最高值区间模拟，例如速度 2.4～3、攻击
              21.6～27、暴伤 3.2%～4%；具体档位概率未公开，属于娱乐估算。
            </p>
          </div>
          <div>
            <strong>成本配置</strong>
            <p>
              每枚六星御魂到 +15：御魂经验
              {{ simulation?.cost.expPerSoul ?? 236250 }}、金币
              {{ simulation?.cost.goldPerSoul ?? 472500 }}。
            </p>
            <p>
              成本参考：<a
                href="https://ngabbs.com/read.php?tid=25650273"
                target="_blank"
                rel="noreferrer"
                >NGA 文档</a
              >。
            </p>
            <p>
              数值参考：{{
                simulation?.cost.valueModelSource ??
                "用户提供的御魂系统主副属性数值规律与强化模型分析文档"
              }}。
            </p>
          </div>
        </div>
      </details>
    </template>
  </main>
</template>

<style scoped>
.simulation-workbench {
  --simulation-ink: #17243f;
  --simulation-paper: #fbf7ef;
  --simulation-gold: #b6763f;
  --simulation-jade: #297869;
  position: relative;
}

.simulation-workbench__header {
  margin-bottom: 24px;
}

.simulation-workbench__disclaimer {
  display: inline-block;
  margin-left: 6px;
  color: var(--cinnabar);
  font-weight: 700;
}

.simulation-workbench__actions {
  display: flex;
  align-items: end;
  justify-content: flex-end;
  gap: 10px;
  flex-wrap: wrap;
}

.simulation-threshold {
  display: flex;
  align-items: center;
  gap: 7px;
  color: var(--ink-soft);
  font-size: 11px;
  font-weight: 700;
}

.simulation-threshold .text-input {
  width: 64px;
  text-align: center;
}

.simulation-threshold b {
  color: var(--simulation-gold);
}

.simulation-launch {
  min-width: 154px;
}

.simulation-notice {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr) auto;
  gap: 14px;
  align-items: center;
  margin-bottom: 16px;
  padding: 15px 18px;
  border: 1px solid rgb(182 105 58 / 25%);
  background: linear-gradient(100deg, rgb(255 248 228 / 96%), rgb(242 246 237 / 92%));
}

.simulation-notice__seal,
.simulation-empty-card__mark {
  display: grid;
  place-items: center;
  color: var(--cinnabar);
  font-family: STKaiti, KaiTi, serif;
}

.simulation-notice__seal {
  width: 38px;
  height: 38px;
  border: 1px solid rgb(201 70 61 / 40%);
  border-radius: 50%;
  font-size: 22px;
  transform: rotate(-7deg);
}

.simulation-notice strong {
  color: var(--simulation-ink);
  font-size: 13px;
}

.simulation-notice p {
  margin: 3px 0 0;
  color: var(--ink-soft);
  font-size: 11px;
  line-height: 1.55;
}

.simulation-notice a,
.simulation-evidence a {
  color: var(--accent-deep);
  font-size: 11px;
  font-weight: 700;
  text-decoration: none;
}

.simulation-summary {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 12px;
  margin-bottom: 12px;
}

.simulation-stat {
  display: grid;
  gap: 4px;
  min-height: 112px;
  padding: 16px 18px;
  border: 1px solid var(--line);
  background: rgb(255 250 240 / 75%);
}

.simulation-stat--primary {
  border-color: rgb(83 123 111 / 35%);
  background: linear-gradient(135deg, rgb(230 241 233 / 94%), rgb(255 250 240 / 92%));
}

.simulation-stat--gold {
  border-color: rgb(182 105 58 / 35%);
  background: linear-gradient(135deg, rgb(250 238 213 / 94%), rgb(255 250 240 / 92%));
}

.simulation-stat--jade {
  border-color: rgb(41 120 105 / 28%);
  background: linear-gradient(135deg, rgb(232 244 239 / 94%), rgb(255 250 240 / 92%));
}

.simulation-stat span,
.simulation-stat small {
  color: var(--ink-soft);
  font-size: 11px;
}

.simulation-stat strong {
  color: var(--simulation-ink);
  /* 汇总数字统一使用正文无衬线字体和等宽数字，避免不同数字字形造成视觉错落。 */
  font-family: inherit;
  font-size: 28px;
  font-variant-numeric: tabular-nums;
  line-height: 1.1;
}

.simulation-meta-strip {
  display: flex;
  gap: 22px;
  align-items: center;
  flex-wrap: wrap;
  margin-bottom: 16px;
  padding: 11px 14px;
  border-left: 3px solid var(--simulation-jade);
  color: var(--ink-soft);
  background: rgb(41 120 105 / 6%);
  font-size: 11px;
}

.simulation-meta-strip b {
  color: var(--simulation-ink);
  font-family: "Cascadia Mono", Consolas, monospace;
}

.simulation-meta-strip code {
  color: var(--accent-deep);
  font-size: 10px;
}

.simulation-results-shell {
  overflow: hidden;
  border: 1px solid rgb(23 36 63 / 11%);
  background: rgb(248 250 249 / 94%);
  box-shadow: var(--shadow);
}

.simulation-results-heading {
  display: flex;
  align-items: end;
  justify-content: space-between;
  gap: 18px;
  padding: 20px 22px 17px;
  border-bottom: 1px solid var(--line-soft);
  background: var(--simulation-paper);
}

.simulation-results-heading h2 {
  margin-top: 5px;
  color: var(--simulation-ink);
  font-family: STKaiti, KaiTi, serif;
  font-size: 28px;
}

.simulation-results-heading__hint {
  color: var(--ink-soft);
  font-size: 10px;
}

.simulation-query-toolbar {
  display: grid;
  gap: 12px;
  padding: 14px 17px;
  border-bottom: 1px solid var(--line-soft);
  background: rgb(255 250 240 / 68%);
}

.simulation-query-toolbar__filters {
  display: grid;
  grid-template-columns: minmax(150px, 1.4fr) repeat(6, minmax(90px, 1fr));
  gap: 8px;
}

.simulation-query-filter {
  display: grid;
  min-width: 0;
  gap: 5px;
  color: var(--ink-soft);
  font-size: 10px;
}

.simulation-query-filter input,
.simulation-query-filter select {
  width: 100%;
  min-width: 0;
  min-height: 32px;
  padding: 0 8px;
  border: 1px solid var(--line);
  border-radius: 6px;
  outline: 0;
  color: var(--ink);
  background: #fff;
  font-size: 10px;
}

.simulation-query-filter input:focus,
.simulation-query-filter select:focus {
  border-color: var(--accent-deep);
  box-shadow: 0 0 0 2px rgb(182 105 58 / 12%);
}

.simulation-query-filter__input-wrap {
  display: flex;
  min-height: 32px;
  align-items: center;
  border: 1px solid var(--line);
  border-radius: 6px;
  background: #fff;
}

.simulation-query-filter__input-wrap input {
  min-height: 30px;
  border: 0;
  box-shadow: none;
}

.simulation-query-filter__input-wrap small {
  padding-right: 8px;
  color: var(--ink-soft);
  font-size: 9px;
}

.simulation-query-filter__check-control {
  display: flex;
  min-height: 32px;
  align-items: center;
  gap: 6px;
  padding: 0 8px;
  border: 1px solid rgb(174 118 83 / 32%);
  border-radius: 6px;
  color: var(--ink);
  background: rgb(250 238 213 / 42%);
}

.simulation-query-filter__check-control input {
  width: auto;
  min-height: 0;
  accent-color: var(--cinnabar);
}

.simulation-query-toolbar__meta {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
  color: var(--ink-soft);
  font-size: 11px;
}

.simulation-results-table {
  overflow-x: auto;
}

.simulation-filter-empty {
  display: grid;
  place-items: center;
  gap: 6px;
  min-height: 150px;
  padding: 24px;
  border-top: 1px solid var(--line-soft);
  color: var(--ink-soft);
  text-align: center;
}

.simulation-filter-empty strong {
  color: var(--simulation-ink);
  font-size: 13px;
}

.simulation-filter-empty p {
  margin: 0;
  font-size: 11px;
}

.simulation-results-table__head,
.simulation-result-row {
  display: grid;
  grid-template-columns:
    34px minmax(165px, 1.1fr) minmax(110px, 0.75fr) minmax(265px, 1.55fr)
    minmax(190px, 1.1fr) 95px 108px;
  gap: 12px;
  /* 结果表每个单元格从同一条上边线开始，避免不同内容高度导致文字上下错落。 */
  align-items: start;
  min-width: 1130px;
}

.simulation-results-table__head {
  padding: 10px 17px;
  color: var(--ink-soft);
  background: rgb(23 36 63 / 3%);
  font-size: 10px;
  font-weight: 700;
}

.simulation-result-row {
  position: relative;
  min-height: 82px;
  padding: 12px 17px;
  border-top: 1px solid var(--line-soft);
  color: var(--simulation-ink);
  background: rgb(255 255 255 / 56%);
  transition:
    background 180ms ease,
    box-shadow 180ms ease;
}

.simulation-result-row:hover {
  background: rgb(255 250 240 / 88%);
}

.simulation-result-row--firework {
  background: linear-gradient(90deg, rgb(255 248 220 / 92%), rgb(255 255 255 / 56%));
  box-shadow: inset 4px 0 0 #d39a31;
  animation: simulation-firework-glow 1.1s ease-out 1;
}

.simulation-result-row__rank {
  color: var(--accent-deep);
  font-family: "Cascadia Mono", Consolas, monospace;
  font-size: 12px;
  font-weight: 700;
}

.simulation-result-row__identity,
.simulation-result-row__identity > span:last-child {
  min-width: 0;
}

.simulation-result-row__identity {
  display: flex;
  align-items: center;
  gap: 9px;
}

.simulation-result-row__identity > span:last-child {
  display: grid;
  gap: 4px;
}

.simulation-result-row__identity strong,
.simulation-result-row__identity small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.simulation-result-row__identity strong {
  font-size: 12px;
}

.simulation-result-row__identity small {
  color: var(--ink-soft);
  font-size: 10px;
}

.simulation-result-row__main-attribute {
  display: grid;
  gap: 3px;
  min-width: 0;
}

.simulation-result-row__main-attribute strong {
  overflow: hidden;
  color: var(--simulation-ink);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.simulation-result-row__main-attribute small {
  color: var(--ink-soft);
  font-size: 10px;
}

.simulation-result-row__icon {
  display: grid;
  width: 38px;
  height: 38px;
  flex: 0 0 auto;
  place-items: center;
  overflow: hidden;
  border: 1px solid rgb(139 71 36 / 30%);
  border-radius: 50% 50% 44% 44%;
  color: var(--accent-deep);
  background: #e8d9c5;
  font-family: STKaiti, KaiTi, serif;
  font-size: 17px;
}

.simulation-result-row__icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.simulation-result-row__rolls,
.simulation-result-row__attributes {
  display: flex;
  gap: 5px;
  align-items: center;
  flex-wrap: wrap;
}

.simulation-result-row__rolls > span,
/* 每条最终副属性独立成块，强化次数固定落在数值下一行，不与属性值挤在同一行。 */
.simulation-result-row__attributes > .simulation-attribute-chip {
  padding: 4px 5px;
  border: 1px solid rgb(23 36 63 / 10%);
  color: var(--ink-soft);
  background: rgb(255 255 255 / 64%);
  font-size: 10px;
}

.simulation-result-row__attributes > .simulation-attribute-chip {
  display: grid;
  gap: 2px;
}

.simulation-attribute-chip__value {
  display: inline-flex;
  gap: 4px;
  align-items: baseline;
  white-space: nowrap;
}

.simulation-attribute-chip__roll {
  color: var(--accent-deep);
  font-family: "Cascadia Mono", Consolas, monospace;
  font-size: 10px;
  line-height: 1;
}

.simulation-attribute-chip__range {
  color: var(--ink-soft);
  font-size: 9px;
  line-height: 1.2;
  white-space: nowrap;
}

.simulation-result-row__rolls small {
  color: var(--accent-deep);
  font-size: 9px;
}

.simulation-result-row__rolls span.is-new {
  border-color: rgb(182 105 58 / 36%);
  color: var(--accent-deep);
  background: rgb(182 105 58 / 8%);
}

.simulation-result-row__rolls b,
.simulation-result-row__attributes b {
  color: var(--simulation-ink);
}

.simulation-result-row__score {
  display: grid;
  align-content: start;
  gap: 3px;
}

.simulation-result-row__score strong {
  color: var(--cinnabar);
  /* 评分数字与表格正文统一字体，并用等宽数字保持小数点和各行稳定对齐。 */
  font-family: inherit;
  font-size: 24px;
  font-variant-numeric: tabular-nums;
  line-height: 1.1;
}

.simulation-result-row__score small {
  overflow: hidden;
  color: var(--ink-soft);
  font-size: 9px;
  line-height: 1.35;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.simulation-result-row__status {
  position: relative;
  display: grid;
  justify-items: start;
  gap: 7px;
}

/* 达到彩虹色阈值时，在状态列播放一次金橙色粒子爆闪；动画只由结果 key 触发一次。 */
.firework-burst {
  position: absolute;
  top: 50%;
  left: 44px;
  width: 42px;
  height: 42px;
  pointer-events: none;
  transform: translate(-50%, -50%);
  z-index: 2;
}

.firework-burst__core {
  position: absolute;
  top: 50%;
  left: 50%;
  display: grid;
  width: 14px;
  height: 14px;
  place-items: center;
  color: #fff4be;
  font-size: 16px;
  text-shadow:
    0 0 5px #f2b84b,
    0 0 12px #d75b42;
  transform: translate(-50%, -50%);
  animation: firework-core-pop 720ms ease-out 1;
}

.firework-burst__spark {
  position: absolute;
  top: 50%;
  left: 50%;
  width: 2px;
  height: 13px;
  border-radius: 999px;
  background: var(--spark-color);
  box-shadow: 0 0 5px var(--spark-color);
  opacity: 0;
  transform: translate(-50%, -50%) translate(0, 0) scale(0.25)
    rotate(var(--spark-angle));
  animation: firework-spark-burst 760ms cubic-bezier(0.2, 0.8, 0.2, 1) 1;
  animation-delay: var(--spark-delay);
}

.firework-badge {
  color: #a9691c;
  font-size: 11px;
  font-weight: 800;
  animation: firework-badge-pop 560ms cubic-bezier(0.2, 0.8, 0.2, 1) 1;
}

.simulation-result-row__detail {
  grid-column: 2 / -1;
  display: grid;
  grid-template-columns: 1.15fr 1.55fr 1fr;
  gap: 18px;
  padding: 13px 0 3px;
  border-top: 1px dashed rgb(182 105 58 / 25%);
}

.simulation-result-row__detail p {
  margin: 5px 0 0;
  color: var(--ink-soft);
  font-size: 10px;
  line-height: 1.65;
}

.detail-label {
  color: var(--accent-deep);
  font-size: 10px;
  font-weight: 700;
}

.simulation-empty-card {
  display: flex;
  align-items: center;
  gap: 22px;
  min-height: 210px;
  padding: 34px;
  border: 1px solid var(--line);
  background: var(--simulation-paper);
}

.simulation-empty-card__mark {
  width: 76px;
  height: 76px;
  flex: 0 0 auto;
  border: 1px solid rgb(201 70 61 / 35%);
  color: var(--cinnabar);
  font-size: 38px;
  transform: rotate(-5deg);
}

.simulation-empty-card h2 {
  color: var(--simulation-ink);
  font-family: STKaiti, KaiTi, serif;
  font-size: 24px;
}

.simulation-empty-card p {
  max-width: 560px;
  margin: 8px 0 16px;
  color: var(--ink-soft);
  font-size: 12px;
  line-height: 1.7;
}

.simulation-evidence {
  margin-top: 16px;
  padding: 14px 16px;
  border-top: 1px solid var(--line-soft);
  color: var(--ink-soft);
}

.simulation-evidence summary {
  color: var(--accent-deep);
  cursor: pointer;
  font-size: 11px;
  font-weight: 700;
}

.simulation-evidence__grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 20px;
  padding-top: 15px;
}

.simulation-evidence strong {
  color: var(--simulation-ink);
  font-size: 11px;
}

.simulation-evidence p {
  margin: 6px 0 0;
  font-size: 10px;
  line-height: 1.65;
}

@keyframes simulation-firework-glow {
  0% {
    box-shadow:
      inset 4px 0 0 #d39a31,
      0 0 0 rgb(211 154 49 / 0%);
  }
  45% {
    box-shadow:
      inset 4px 0 0 #d39a31,
      0 0 24px rgb(211 154 49 / 48%);
  }
  100% {
    box-shadow:
      inset 4px 0 0 #d39a31,
      0 0 0 rgb(211 154 49 / 0%);
  }
}

@keyframes firework-badge-pop {
  0% {
    opacity: 0;
    transform: scale(0.7) rotate(-6deg);
  }
  70% {
    opacity: 1;
    transform: scale(1.08) rotate(2deg);
  }
  100% {
    transform: scale(1) rotate(0);
  }
}

@keyframes firework-core-pop {
  0% {
    opacity: 0;
    transform: translate(-50%, -50%) scale(0.2) rotate(-30deg);
  }
  35% {
    opacity: 1;
    transform: translate(-50%, -50%) scale(1.35) rotate(8deg);
  }
  100% {
    opacity: 0.92;
    transform: translate(-50%, -50%) scale(1) rotate(0);
  }
}

@keyframes firework-spark-burst {
  0% {
    opacity: 0;
    transform: translate(-50%, -50%) translate(0, 0) scale(0.25)
      rotate(var(--spark-angle));
  }
  28% {
    opacity: 1;
  }
  100% {
    opacity: 0;
    transform: translate(-50%, -50%) translate(var(--spark-x), var(--spark-y)) scale(1)
      rotate(var(--spark-angle));
  }
}

@media (max-width: 900px) {
  .simulation-summary {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .simulation-query-toolbar__filters {
    grid-template-columns: repeat(4, minmax(0, 1fr));
  }

  .simulation-results-heading {
    align-items: flex-start;
    flex-direction: column;
    gap: 7px;
  }

  .simulation-result-row__detail {
    grid-template-columns: 1fr;
    gap: 10px;
  }
}

@media (max-width: 620px) {
  .simulation-workbench__actions {
    align-items: stretch;
    justify-content: stretch;
  }

  .simulation-threshold,
  .simulation-launch {
    width: 100%;
    justify-content: center;
  }

  .simulation-notice {
    grid-template-columns: auto minmax(0, 1fr);
  }

  .simulation-notice a {
    grid-column: 2;
  }

  .simulation-summary,
  .simulation-evidence__grid,
  .simulation-query-toolbar__filters {
    grid-template-columns: 1fr;
  }

  .simulation-empty-card {
    align-items: flex-start;
    flex-direction: column;
    padding: 24px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .simulation-result-row,
  .simulation-result-row--firework,
  .firework-badge,
  .firework-burst__core,
  .firework-burst__spark {
    animation: none;
    transition: none;
  }

  .firework-burst {
    display: none;
  }
}
</style>
