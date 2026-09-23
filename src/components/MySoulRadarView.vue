<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { desktopApi } from "../api/client";
import { normalizeCommandError } from "../api/errors";
import type {
  CatalogStatus,
  SoulRadarCache,
  SoulRadarPoint,
  SoulSet,
} from "../api/contracts";
import { MY_SOULS_INVENTORY_CLEARED_EVENT } from "./mySoulsDerivedStorage";
import { onActiveCharacterChanged } from "../state/activeCharacter";

// 图标直接复用内置目录资源；雷达只读取目录元数据和缓存，不为展示重新读取御魂正文。
const iconModules = import.meta.glob("../assets/soul-icons/*.png", {
  eager: true,
  import: "default",
  query: "?url",
});

type MetricKey =
  | "speed"
  | "crit_rate"
  | "crit_damage"
  | "effect_hit"
  | "effect_resist"
  | "attack_rate"
  | "hp_rate"
  | "defense_rate"
  | "attack_flat"
  | "hp_flat"
  | "defense_flat";
type SortKey = "desc" | "asc" | "name";
type Metric = {
  key: MetricKey;
  label: string;
  percentage: boolean;
  baseMax: number;
};
type RadarValue = { value: number; mainAttrType: string | null };
type RadarSet = { set: SoulSet; values: Record<MetricKey, RadarValue[]> };

// 百分比副属性在数据库中按小数保存，例如 0.1588 在界面显示为 15.88%。
const metrics: Metric[] = [
  { key: "speed", label: "速度", percentage: false, baseMax: 20 },
  { key: "crit_rate", label: "暴击", percentage: true, baseMax: 0.2 },
  { key: "crit_damage", label: "暴伤", percentage: true, baseMax: 0.3 },
  { key: "effect_hit", label: "效果命中", percentage: true, baseMax: 0.2 },
  { key: "effect_resist", label: "效果抵抗", percentage: true, baseMax: 0.2 },
  { key: "attack_rate", label: "攻击加成", percentage: true, baseMax: 0.2 },
  { key: "hp_rate", label: "生命加成", percentage: true, baseMax: 0.2 },
  { key: "defense_rate", label: "防御加成", percentage: true, baseMax: 0.2 },
  { key: "attack_flat", label: "攻击", percentage: false, baseMax: 100 },
  { key: "hp_flat", label: "生命", percentage: false, baseMax: 1000 },
  { key: "defense_flat", label: "防御", percentage: false, baseMax: 100 },
];

// 图上从左上开始放置 1、6、5、4、3、2 号位，沿图面顺序保持用户确认的逆时针号位展示。
const slots = [1, 6, 5, 4, 3, 2] as const;
const slotLabels: Record<(typeof slots)[number], string> = {
  1: "壹",
  6: "陆",
  5: "伍",
  4: "肆",
  3: "叁",
  2: "贰",
};

// 主属性使用导入事实中的稳定类型，避免把“速度/抵抗/爆伤”写死后无法展示其他主属性。
const attributeLabels: Record<string, string> = {
  attack_flat: "攻击",
  defense_flat: "防御",
  hp_flat: "生命",
  attack_rate: "攻击",
  defense_rate: "防御",
  hp_rate: "生命",
  speed: "速度",
  effect_hit: "命中",
  effect_resist: "抵抗",
  crit_rate: "暴击",
  crit_damage: "爆伤",
};

const chart = { centerX: 115, centerY: 112, radius: 69 };
const selectedMetric = ref<MetricKey>("speed");
const selectedSort = ref<SortKey>("desc");
const selectedEffect = ref("all");
const selectedSetId = ref("");
const setPickerOpen = ref(false);
const catalogStatus = ref<CatalogStatus | null>(null);
const sets = ref<SoulSet[]>([]);
const cache = ref<SoulRadarCache | null>(null);
const loading = ref(true);
const calculating = ref(false);
const errorMessage = ref("");
// 后端聚合命令是一次性返回结果；前端用阶段提示表达当前等待位置，避免用户误以为按钮失效。
const calculationPhase = ref<"reading" | "aggregating" | "saving">("reading");
let calculationTimer: number | null = null;
let stopActiveCharacterChanged: (() => void) | null = null;

const calculationProgress = computed(() => {
  switch (calculationPhase.value) {
    case "reading":
      return 32;
    case "aggregating":
      return 68;
    case "saving":
      return 88;
  }
});

const calculationMessage = computed(() => {
  switch (calculationPhase.value) {
    case "reading":
      return "正在读取全部已确认御魂…";
    case "aggregating":
      return "正在聚合各套装的最高属性…";
    case "saving":
      return "正在保存雷达结果…";
  }
});

const activeMetric = computed(
  () => metrics.find((metric) => metric.key === selectedMetric.value) ?? metrics[0]!,
);
const cacheIsStale = computed(
  () =>
    cache.value !== null &&
    (cache.value.inventoryCount !== cache.value.currentInventoryCount ||
      cache.value.inventoryRevision !== cache.value.currentInventoryRevision),
);
// 只有缓存存在且仍对应当前库存时才展示雷达和筛选器，避免用户继续查看旧数据。
const hasCalculation = computed(() => cache.value !== null && !cacheIsStale.value);
const pointsByKey = computed(() => {
  const result = new Map<string, SoulRadarPoint>();
  for (const point of cache.value?.points ?? []) {
    result.set(`${point.setId}|${point.slot}|${point.metricType}`, point);
  }
  return result;
});

const radarSets = computed<RadarSet[]>(() =>
  sets.value.map((set) => {
    const values = {} as Record<MetricKey, RadarValue[]>;
    for (const metric of metrics) {
      values[metric.key] = slots.map((slot) => {
        const point = pointsByKey.value.get(`${set.setId}|${slot}|${metric.key}`);
        return { value: point?.value ?? 0, mainAttrType: point?.mainAttrType ?? null };
      });
    }
    return { set, values };
  }),
);

/** 从当前目录提取全部真实套装效果类别，避免筛选器遗漏新增或特殊类别。 */
const effectFilters = computed(() => {
  const categories = new Set<string>();
  for (const set of sets.value) {
    const category = set.specialCategory ?? set.category;
    if (category) categories.add(category);
  }
  return [
    { key: "all", label: "全部效果" },
    ...Array.from(categories)
      .sort((left, right) => left.localeCompare(right, "zh-CN"))
      .map((category) => ({ key: category, label: category })),
  ];
});

/** 套装分类来自目录的 category/specialCategory，不再把多个实际效果合并成少数展示项。 */
function matchesEffectFilter(set: SoulSet, filter: string): boolean {
  return filter === "all" || (set.specialCategory ?? set.category) === filter;
}

const filteredRadarSets = computed(() =>
  radarSets.value.filter(
    (soulSet) =>
      matchesEffectFilter(soulSet.set, selectedEffect.value) &&
      (!selectedSetId.value || soulSet.set.setId === selectedSetId.value),
  ),
);

// 二级套装下拉框只展示当前真实效果分类下的套装；切换一级分类时自动清空失效选择。
const availableSetOptions = computed(() =>
  radarSets.value.filter((soulSet) =>
    matchesEffectFilter(soulSet.set, selectedEffect.value),
  ),
);

watch(selectedEffect, () => {
  if (
    !availableSetOptions.value.some(
      (soulSet) => soulSet.set.setId === selectedSetId.value,
    )
  ) {
    selectedSetId.value = "";
  }
});

const chartMax = computed(() => {
  const observedMax = radarSets.value.reduce(
    (maximum, soulSet) =>
      Math.max(
        maximum,
        ...soulSet.values[activeMetric.value.key].map((item) => item.value),
      ),
    0,
  );
  // 保留游戏常见的视觉基准，同时在极端数据下自动扩展，避免点位被裁切到边界之外。
  return Math.max(activeMetric.value.baseMax, observedMax);
});

const sortedSets = computed(() =>
  [...filteredRadarSets.value].sort((left, right) => {
    if (selectedSort.value === "name") {
      return left.set.name.localeCompare(right.set.name, "zh-CN");
    }
    const leftMax = setPeak(left);
    const rightMax = setPeak(right);
    return selectedSort.value === "desc" ? rightMax - leftMax : leftMax - rightMax;
  }),
);

/** 读取目录和已持久化雷达结果；此入口不会调用 listMySouls。 */
async function loadPage(): Promise<void> {
  loading.value = true;
  errorMessage.value = "";
  try {
    catalogStatus.value = await desktopApi.getCatalogStatus();
    if (!catalogStatus.value) {
      // 首次运行没有目录时安装内置目录，保证所有套装仍能展示名称和图标。
      catalogStatus.value = await desktopApi.installBuiltinCatalog();
    }
    sets.value = await desktopApi.listSoulSets(catalogStatus.value.version);
    cache.value = await desktopApi.getSoulRadarCache();
  } catch (caught) {
    const error = normalizeCommandError(caught);
    errorMessage.value = `${error.code} · ${error.message}`;
  } finally {
    loading.value = false;
  }
}

/** 只有用户点击按钮才读取全部已确认御魂，并把聚合结果写入 SQLite 缓存。 */
async function calculateRadar(): Promise<void> {
  if (calculating.value) return;
  calculating.value = true;
  calculationPhase.value = "reading";
  startCalculationProgress();
  errorMessage.value = "";
  try {
    cache.value = await desktopApi.calculateSoulRadar();
  } catch (caught) {
    const error = normalizeCommandError(caught);
    errorMessage.value = `${error.code} · ${error.message}`;
  } finally {
    stopCalculationProgress();
    calculating.value = false;
  }
}

/** 用三个用户可理解的阶段更新遮罩文案；命令完成前不会虚报 100% 已完成。 */
function startCalculationProgress(): void {
  stopCalculationProgress();
  calculationTimer = window.setTimeout(() => {
    calculationPhase.value = "aggregating";
    calculationTimer = window.setTimeout(() => {
      calculationPhase.value = "saving";
    }, 1200);
  }, 900);
}

/** 清理阶段定时器，确保离开页面或计算失败后不会继续修改已卸载组件状态。 */
function stopCalculationProgress(): void {
  if (calculationTimer !== null) {
    window.clearTimeout(calculationTimer);
    calculationTimer = null;
  }
}

/** 以左上角为第一轴绘制六个落点，落点顺序固定对应 1、6、5、4、3、2。 */
function point(index: number, ratio: number): { x: number; y: number } {
  const angle = ((-120 + index * 60) * Math.PI) / 180;
  return {
    x: chart.centerX + Math.cos(angle) * chart.radius * ratio,
    y: chart.centerY + Math.sin(angle) * chart.radius * ratio,
  };
}

/** 生成六边形网格或数据面使用的 SVG 点串。 */
function polygon(ratio: number): string {
  return slots
    .map((_, index) => point(index, ratio))
    .map((item) => `${item.x.toFixed(1)},${item.y.toFixed(1)}`)
    .join(" ");
}

/** 将一个套装六个号位的最高指标值转换为雷达数据面。 */
function dataPolygon(soulSet: RadarSet): string {
  return soulSet.values[activeMetric.value.key]
    .map((item, index) => point(index, Math.min(1, item.value / chartMax.value)))
    .map((item) => `${item.x.toFixed(1)},${item.y.toFixed(1)}`)
    .join(" ");
}

/** 数值标签向外偏移；无数据时仍保留 0，方便用户确认计算已覆盖该号位。 */
function valuePoint(soulSet: RadarSet, index: number): { x: number; y: number } {
  const value = soulSet.values[activeMetric.value.key][index]?.value ?? 0;
  return point(index, Math.min(1.16, Math.max(0.1, value / chartMax.value + 0.08)));
}

/** 按属性单位格式化数值；百分比补充主属性名称，满足二四六号位的标注要求。 */
function formatValue(value: number): string {
  const displayValue = activeMetric.value.percentage ? value * 100 : value;
  const rounded = Number.isInteger(displayValue)
    ? String(displayValue)
    : displayValue.toFixed(2);
  return rounded + (activeMetric.value.percentage ? "%" : "");
}

/** 计算当前套装在选中指标下的峰值，用于排序和标题辅助信息。 */
function setPeak(soulSet: RadarSet): number {
  return Math.max(
    ...soulSet.values[activeMetric.value.key].map((item) => item.value),
    0,
  );
}

/** 给二、四、六号位的最高副属性补充该御魂主属性，其他号位保持标签简洁。 */
function mainAttributeLabel(soulSet: RadarSet, index: number): string {
  const slot = slots[index];
  if (slot !== 2 && slot !== 4 && slot !== 6) return "";
  const mainAttrType = soulSet.values[activeMetric.value.key][index]?.mainAttrType;
  const label = mainAttrType ? (attributeLabels[mainAttrType] ?? mainAttrType) : "";
  return label ? ` ${label}` : "";
}

/** 根据目录套装返回本地离线图标；所有套装标题都走同一映射。 */
function soulSetIcon(set: SoulSet): string | null {
  if (!set.iconAssetId) return null;
  return iconModules[`../assets/soul-icons/${set.iconAssetId}.png`] ?? null;
}

/** 点击具体御魂后关闭自定义选择面板，避免选择完成后面板遮挡雷达内容。 */
function selectSoulSet(setId: string): void {
  selectedSetId.value = setId;
  setPickerOpen.value = false;
}

/** 根据当前选择返回按钮中的御魂图标和名称；未选择时显示全部御魂。 */
const selectedSoulSet = computed(
  () =>
    availableSetOptions.value.find(
      (soulSet) => soulSet.set.setId === selectedSetId.value,
    )?.set ?? null,
);

/** 清空库存后立即丢弃当前组件仍持有的雷达结果，避免短暂展示已失效的图表。 */
function handleInventoryCleared(): void {
  stopCalculationProgress();
  cache.value = null;
  errorMessage.value = "";
}

/** 角色切换时丢弃旧档案雷达并重新读取新档案缓存，避免页面沿用上一角色的图表。 */
function handleActiveCharacterChanged(profileId: string | null): void {
  stopCalculationProgress();
  cache.value = null;
  errorMessage.value = "";
  if (profileId) void loadPage();
}

/** 计算结果的时间只用于状态提示，异常日期不阻断雷达展示。 */
function formattedCalculatedAt(): string {
  if (!cache.value) return "";
  const date = new Date(cache.value.calculatedAt);
  return Number.isNaN(date.getTime())
    ? cache.value.calculatedAt
    : date.toLocaleString("zh-CN");
}

onMounted(() => {
  void loadPage();
  window.addEventListener(MY_SOULS_INVENTORY_CLEARED_EVENT, handleInventoryCleared);
  stopActiveCharacterChanged = onActiveCharacterChanged(handleActiveCharacterChanged);
});

onBeforeUnmount(() => {
  stopCalculationProgress();
  window.removeEventListener(MY_SOULS_INVENTORY_CLEARED_EVENT, handleInventoryCleared);
  stopActiveCharacterChanged?.();
});
</script>

<template>
  <section class="soul-radar-overview" aria-labelledby="soul-radar-title">
    <header class="soul-radar-overview__titlebar">
      <h2 id="soul-radar-title">套装各维度成长全息图</h2>
      <div class="soul-radar-overview__actions">
        <span v-if="cache && !cacheIsStale" class="soul-radar-overview__status"
          >已保存 {{ formattedCalculatedAt() }}</span
        >
        <span
          v-else-if="cacheIsStale"
          class="soul-radar-overview__status soul-radar-overview__status--stale"
          >库存已更新</span
        >
        <span v-else class="soul-radar-overview__status">尚未计算</span>
        <button
          type="button"
          class="soul-radar-overview__calculate"
          :disabled="loading || calculating"
          @click="calculateRadar"
        >
          {{ calculating ? "正在计算…" : cache ? "重新计算" : "计算雷达数据" }}
        </button>
      </div>
    </header>

    <section
      v-if="hasCalculation"
      class="soul-radar-overview__controls"
      aria-label="雷达图筛选与排序"
    >
      <label class="soul-radar-overview__control">
        <span>套装效果</span>
        <select v-model="selectedEffect">
          <option v-for="effect in effectFilters" :key="effect.key" :value="effect.key">
            {{ effect.label }}
          </option>
        </select>
      </label>
      <label class="soul-radar-overview__control">
        <span>御魂</span>
        <div class="soul-radar-set-picker">
          <button
            type="button"
            class="soul-radar-set-picker__trigger"
            :aria-expanded="setPickerOpen"
            aria-haspopup="listbox"
            @click="setPickerOpen = !setPickerOpen"
          >
            <template v-if="selectedSoulSet">
              <img
                v-if="soulSetIcon(selectedSoulSet)"
                :src="soulSetIcon(selectedSoulSet) ?? undefined"
                :alt="selectedSoulSet.name + '图标'"
              />
              <span>{{ selectedSoulSet.name }}</span>
            </template>
            <span v-else>全部御魂（{{ availableSetOptions.length }}）</span>
            <span class="soul-radar-set-picker__chevron" aria-hidden="true">⌄</span>
          </button>
          <div
            v-if="setPickerOpen"
            class="soul-radar-set-picker__menu"
            role="listbox"
            aria-label="选择御魂"
          >
            <button
              type="button"
              class="soul-radar-set-picker__option"
              :class="{ 'soul-radar-set-picker__option--selected': !selectedSetId }"
              role="option"
              :aria-selected="!selectedSetId"
              @click="selectSoulSet('')"
            >
              <span>全部御魂</span>
              <small>{{ availableSetOptions.length }}</small>
            </button>
            <button
              v-for="soulSet in availableSetOptions"
              :key="soulSet.set.setId"
              type="button"
              class="soul-radar-set-picker__option"
              :class="{
                'soul-radar-set-picker__option--selected':
                  selectedSetId === soulSet.set.setId,
              }"
              role="option"
              :aria-selected="selectedSetId === soulSet.set.setId"
              @click="selectSoulSet(soulSet.set.setId)"
            >
              <img
                v-if="soulSetIcon(soulSet.set)"
                :src="soulSetIcon(soulSet.set) ?? undefined"
                :alt="soulSet.set.name + '图标'"
              />
              <span>{{ soulSet.set.name }}</span>
            </button>
          </div>
        </div>
      </label>
      <label class="soul-radar-overview__control">
        <span>指标</span>
        <select v-model="selectedMetric">
          <option v-for="metric in metrics" :key="metric.key" :value="metric.key">
            {{ metric.label }}
          </option>
        </select>
      </label>
      <label class="soul-radar-overview__control">
        <span>排序方式</span>
        <select v-model="selectedSort">
          <option value="desc">指标数值（从高到低）</option>
          <option value="asc">指标数值（从低到高）</option>
          <option value="name">套装名称</option>
        </select>
      </label>
    </section>

    <p v-if="errorMessage" class="soul-radar-overview__error" role="alert">
      {{ errorMessage }}
    </p>
    <div v-if="loading" class="soul-radar-overview__empty">
      正在读取雷达缓存和套装目录…
    </div>
    <div v-else-if="!hasCalculation" class="soul-radar-overview__empty">
      <strong>{{
        cacheIsStale ? "库存已更新，请重新计算" : "尚未计算雷达数据"
      }}</strong>
      <span>点击右上角“计算雷达数据”，完成后才展示雷达图和筛选数据。</span>
    </div>
    <div v-else class="soul-radar-overview__grid">
      <article
        v-for="soulSet in sortedSets"
        :key="soulSet.set.setId"
        class="soul-radar-card"
      >
        <div class="soul-radar-card__heading">
          <h3>{{ soulSet.set.name }}</h3>
          <img
            v-if="soulSetIcon(soulSet.set)"
            class="soul-radar-card__set-icon"
            :src="soulSetIcon(soulSet.set) ?? undefined"
            :alt="soulSet.set.name + '图标'"
          />
          <span>最高 {{ formatValue(setPeak(soulSet)) }}</span>
        </div>
        <svg
          class="soul-radar-card__chart"
          viewBox="0 0 230 214"
          role="img"
          :aria-label="soulSet.set.name + '的' + activeMetric.label + '六号位雷达图'"
        >
          <g class="soul-radar-card__grid" aria-hidden="true">
            <polygon
              v-for="ratio in [0.25, 0.5, 0.75, 1]"
              :key="ratio"
              :points="polygon(ratio)"
            />
            <line
              v-for="(slot, index) in slots"
              :key="slot"
              :x1="chart.centerX"
              :y1="chart.centerY"
              :x2="point(index, 1).x"
              :y2="point(index, 1).y"
            />
          </g>
          <polygon class="soul-radar-card__data" :points="dataPolygon(soulSet)" />
          <g class="soul-radar-card__data-points" aria-hidden="true">
            <circle
              v-for="(_, index) in slots"
              :key="'point-' + slots[index]"
              :cx="
                point(
                  index,
                  Math.min(
                    1,
                    (soulSet.values[activeMetric.key][index]?.value ?? 0) / chartMax,
                  ),
                ).x
              "
              :cy="
                point(
                  index,
                  Math.min(
                    1,
                    (soulSet.values[activeMetric.key][index]?.value ?? 0) / chartMax,
                  ),
                ).y
              "
              r="3.1"
            />
          </g>
          <g class="soul-radar-card__value-labels">
            <text
              v-for="(_, index) in slots"
              :key="'value-' + slots[index]"
              :x="valuePoint(soulSet, index).x"
              :y="valuePoint(soulSet, index).y"
              text-anchor="middle"
            >
              {{ formatValue(soulSet.values[activeMetric.key][index]?.value ?? 0)
              }}{{ mainAttributeLabel(soulSet, index) }}
            </text>
          </g>
          <g class="soul-radar-card__slot-labels">
            <text
              v-for="(slot, index) in slots"
              :key="'slot-' + slot"
              :x="point(index, 1.28).x"
              :y="point(index, 1.28).y + 4"
              text-anchor="middle"
            >
              {{ slotLabels[slot] }}
            </text>
          </g>
        </svg>
      </article>
    </div>

    <!-- 计算期间锁住页面，避免用户重复触发全量读取或误操作筛选条件。 -->
    <Teleport to="body">
      <div
        v-if="calculating"
        class="soul-radar-calculation-mask"
        role="dialog"
        aria-modal="true"
        aria-labelledby="soul-radar-calculation-title"
      >
        <div class="soul-radar-calculation-dialog">
          <div class="soul-radar-calculation-dialog__mark" aria-hidden="true">六</div>
          <span class="soul-radar-calculation-dialog__eyebrow"
            >SOUL RADAR / CALCULATING</span
          >
          <h3 id="soul-radar-calculation-title">正在计算御魂雷达</h3>
          <p>首次计算会读取全部御魂，结果保存后下次进入页面无需重复读取。</p>
          <div
            class="soul-radar-calculation-progress"
            role="progressbar"
            aria-valuemin="0"
            aria-valuemax="100"
            :aria-valuenow="calculationProgress"
            :aria-valuetext="calculationMessage"
          >
            <span
              class="soul-radar-calculation-progress__fill"
              :style="{ width: `${calculationProgress}%` }"
            />
          </div>
          <div class="soul-radar-calculation-dialog__meta">
            <span>{{ calculationMessage }}</span>
            <strong>{{ calculationProgress }}%</strong>
          </div>
          <span class="soul-radar-calculation-dialog__hint">请保持当前页面打开</span>
        </div>
      </div>
    </Teleport>
  </section>
</template>

<style scoped>
.soul-radar-overview {
  display: grid;
  gap: 0;
  min-width: 0;
  margin: -10px -18px 0;
  padding: 0 clamp(18px, 2.8vw, 48px) 38px;
  color: #29323a;
  background: #fff;
}
.soul-radar-overview__titlebar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 18px;
  padding: 22px 0 19px;
  border-bottom: 1px solid #e5e8ed;
}
.soul-radar-overview__titlebar h2 {
  margin: 0;
  color: #202a33;
  font-size: clamp(23px, 2.3vw, 30px);
  font-weight: 800;
  letter-spacing: 0.05em;
}
.soul-radar-overview__actions {
  display: flex;
  align-items: center;
  gap: 12px;
}
.soul-radar-overview__status {
  color: #7b8792;
  font-size: 12px;
  white-space: nowrap;
}
.soul-radar-overview__status--stale {
  color: #c17b36;
}
.soul-radar-overview__calculate {
  height: 36px;
  padding: 0 15px;
  border: 1px solid #4f9bea;
  border-radius: 6px;
  color: #fff;
  background: #5aa8f5;
  font-size: 13px;
  cursor: pointer;
}
.soul-radar-overview__calculate:hover:not(:disabled) {
  background: #438fdd;
}
.soul-radar-overview__calculate:disabled {
  cursor: wait;
  opacity: 0.6;
}
.soul-radar-overview__controls {
  display: grid;
  grid-template-columns:
    minmax(220px, 1fr) minmax(250px, 1.1fr) minmax(220px, 1fr)
    minmax(240px, 1.1fr);
  width: min(100%, 1280px);
  margin-inline: auto;
  gap: 16px clamp(16px, 2.7vw, 42px);
  padding: 35px 0 29px;
}
.soul-radar-overview__control {
  display: flex;
  align-items: center;
  gap: 11px;
  color: #626b75;
  font-size: 16px;
  white-space: nowrap;
}
.soul-radar-overview__control select,
.soul-radar-set-picker__trigger {
  min-width: 0;
  width: 100%;
  height: 42px;
  padding: 0 34px 0 14px;
  border: 1px solid #dce1e8;
  border-radius: 6px;
  color: #66717e;
  background: #fff;
  font-size: 16px;
  outline: none;
}
.soul-radar-overview__control:first-child select {
  border-color: #5aabff;
  box-shadow: 0 0 0 1px rgb(90 171 255 / 8%);
}
.soul-radar-overview__control select:focus-visible {
  border-color: #4799ed;
  box-shadow: 0 0 0 3px rgb(90 171 255 / 18%);
}
.soul-radar-set-picker {
  position: relative;
  min-width: 230px;
}
.soul-radar-set-picker__trigger {
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  padding: 0 13px;
  text-align: left;
  cursor: pointer;
}
.soul-radar-set-picker__trigger:focus-visible,
.soul-radar-set-picker__trigger[aria-expanded="true"] {
  border-color: #4799ed;
  box-shadow: 0 0 0 3px rgb(90 171 255 / 18%);
}
.soul-radar-set-picker__trigger img,
.soul-radar-set-picker__option img {
  width: 23px;
  height: 23px;
  flex: 0 0 23px;
  object-fit: contain;
}
.soul-radar-set-picker__trigger span:not(.soul-radar-set-picker__chevron) {
  overflow: hidden;
  text-overflow: ellipsis;
}
.soul-radar-set-picker__chevron {
  margin-left: auto;
  color: #9ba5af;
  font-size: 19px;
  line-height: 1;
  transform: translateY(-2px);
}
.soul-radar-set-picker__menu {
  position: absolute;
  top: calc(100% + 7px);
  right: 0;
  left: 0;
  z-index: 30;
  max-height: 320px;
  overflow-y: auto;
  padding: 6px;
  border: 1px solid #dce3e9;
  border-radius: 8px;
  background: #fff;
  box-shadow: 0 12px 28px rgb(46 62 78 / 16%);
}
.soul-radar-set-picker__option {
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  min-height: 36px;
  padding: 5px 8px;
  border: 0;
  border-radius: 5px;
  color: #596773;
  background: transparent;
  font-size: 14px;
  text-align: left;
  cursor: pointer;
}
.soul-radar-set-picker__option:hover,
.soul-radar-set-picker__option--selected {
  color: #2f78bb;
  background: #eff7ff;
}
.soul-radar-set-picker__option small {
  margin-left: auto;
  color: #a2adb6;
  font-size: 11px;
}
.soul-radar-overview__error {
  margin: 0 0 18px;
  padding: 10px 13px;
  border: 1px solid #f2c8c3;
  border-radius: 6px;
  color: #a54c43;
  background: #fff7f6;
  font-size: 13px;
}
.soul-radar-overview__empty {
  display: grid;
  justify-items: center;
  gap: 8px;
  padding: 72px 20px 110px;
  color: #84909b;
  font-size: 14px;
  text-align: center;
}
.soul-radar-overview__empty strong {
  color: #4c5964;
  font-size: 18px;
}
.soul-radar-calculation-mask {
  position: fixed;
  inset: 0;
  z-index: 1000;
  display: grid;
  place-items: center;
  padding: 24px;
  background: rgb(26 36 47 / 42%);
  backdrop-filter: blur(5px);
}
.soul-radar-calculation-dialog {
  width: min(440px, 100%);
  padding: 32px 34px 28px;
  border: 1px solid rgb(255 255 255 / 70%);
  border-radius: 12px;
  color: #34414d;
  background: rgb(255 255 255 / 96%);
  box-shadow: 0 22px 60px rgb(28 42 56 / 24%);
}
.soul-radar-calculation-dialog__mark {
  display: grid;
  width: 38px;
  height: 38px;
  place-items: center;
  margin-bottom: 17px;
  border-radius: 50%;
  color: #fff;
  background: #ff8a78;
  font-family: "Noto Serif SC", "Microsoft YaHei", sans-serif;
  font-size: 18px;
  font-weight: 800;
  box-shadow: 0 6px 15px rgb(255 138 120 / 30%);
}
.soul-radar-calculation-dialog__eyebrow {
  color: #8a98a5;
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.14em;
}
.soul-radar-calculation-dialog h3 {
  margin: 8px 0 9px;
  color: #25323d;
  font-size: 23px;
  letter-spacing: 0.03em;
}
.soul-radar-calculation-dialog p {
  margin: 0 0 24px;
  color: #7b8792;
  font-size: 13px;
  line-height: 1.7;
}
.soul-radar-calculation-progress {
  position: relative;
  height: 9px;
  overflow: hidden;
  border-radius: 999px;
  background: #edf1f5;
}
.soul-radar-calculation-progress__fill {
  position: relative;
  display: block;
  height: 100%;
  border-radius: inherit;
  background: linear-gradient(90deg, #ff927e, #f8b06e);
  transition: width 0.5s ease;
}
.soul-radar-calculation-progress__fill::after {
  position: absolute;
  inset: 0;
  background: linear-gradient(
    105deg,
    transparent 20%,
    rgb(255 255 255 / 65%) 50%,
    transparent 80%
  );
  content: "";
  animation: soul-radar-progress-shine 1.35s linear infinite;
}
.soul-radar-calculation-dialog__meta {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  margin-top: 12px;
  color: #6f7d89;
  font-size: 12px;
}
.soul-radar-calculation-dialog__meta strong {
  color: #e67967;
  font-variant-numeric: tabular-nums;
}
.soul-radar-calculation-dialog__hint {
  display: block;
  margin-top: 22px;
  color: #a4adb5;
  font-size: 11px;
}
@keyframes soul-radar-progress-shine {
  from {
    transform: translateX(-100%);
  }
  to {
    transform: translateX(100%);
  }
}
.soul-radar-overview__grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(210px, 1fr));
  column-gap: clamp(10px, 2.4vw, 40px);
  row-gap: 20px;
}
.soul-radar-card {
  display: grid;
  justify-items: center;
  min-width: 0;
}
.soul-radar-card__heading {
  display: flex;
  align-items: baseline;
  justify-content: center;
  gap: 8px;
  width: 100%;
  min-height: 32px;
}
.soul-radar-card__heading h3 {
  margin: 0;
  color: #27313b;
  font-size: 20px;
  font-weight: 800;
  letter-spacing: 0.04em;
}
/* 图标紧跟套装名称，尺寸控制在标题字高附近，避免把雷达图整体向下推移。 */
.soul-radar-card__set-icon {
  width: 21px;
  height: 21px;
  object-fit: contain;
  vertical-align: -3px;
}
.soul-radar-card__heading span {
  color: #89929b;
  font-size: 10px;
}
.soul-radar-card__chart {
  display: block;
  width: min(100%, 280px);
  height: auto;
  overflow: visible;
}
.soul-radar-card__grid polygon,
.soul-radar-card__grid line {
  fill: none;
  stroke: #dce5ef;
  stroke-width: 1.25;
}
.soul-radar-card__grid line {
  stroke: #cfd7df;
  stroke-width: 1;
}
.soul-radar-card__data {
  fill: rgb(255 137 119 / 38%);
  stroke: #ff8777;
  stroke-linejoin: round;
  stroke-width: 2.2;
}
.soul-radar-card__data-points circle {
  fill: #ff8777;
  stroke: #ff8777;
  stroke-width: 2;
}
.soul-radar-card__slot-labels text {
  fill: #c7ccd2;
  font-size: 11px;
  font-weight: 600;
}
.soul-radar-card__value-labels text {
  fill: #303840;
  paint-order: stroke;
  stroke: rgb(255 255 255 / 90%);
  stroke-width: 3px;
  font-size: 10px;
  font-weight: 500;
}
@media (max-width: 1180px) {
  .soul-radar-overview__grid {
    grid-template-columns: repeat(3, minmax(210px, 1fr));
  }
}
@media (max-width: 860px) {
  .soul-radar-overview__grid {
    grid-template-columns: repeat(2, minmax(210px, 1fr));
  }
}
@media (max-width: 600px) {
  .soul-radar-overview {
    margin: -4px -10px 0;
    padding-inline: 12px;
  }
  .soul-radar-overview__titlebar {
    align-items: stretch;
    flex-direction: column;
  }
  .soul-radar-overview__actions {
    justify-content: space-between;
  }
  .soul-radar-overview__controls {
    display: flex;
    align-items: stretch;
    flex-direction: column;
    gap: 12px;
    padding-block: 22px;
  }
  .soul-radar-overview__control {
    justify-content: space-between;
  }
  .soul-radar-overview__control select,
  .soul-radar-set-picker {
    min-width: 0;
    flex: 1;
  }
  .soul-radar-overview__grid {
    grid-template-columns: 1fr;
  }
  .soul-radar-calculation-dialog {
    padding: 28px 24px 24px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .soul-radar-calculation-progress__fill {
    transition: none;
  }
  .soul-radar-calculation-progress__fill::after {
    animation: none;
  }
}
</style>
