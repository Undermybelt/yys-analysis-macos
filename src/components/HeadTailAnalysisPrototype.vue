<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import { normalizeCommandError } from "../api/errors";
import type { HeadTailCache, HeadTailCard, SoulSet } from "../api/contracts";
import { MY_SOULS_INVENTORY_CLEARED_EVENT } from "./mySoulsDerivedStorage";
import { onActiveCharacterChanged } from "../state/activeCharacter";
import emptyResultImage from "../assets/head-tail-empty.png";

// 头尾卡片直接复用目录中的离线套装图标，避免计算结果依赖网络图片。
const iconModules = import.meta.glob("../assets/soul-icons/*.png", {
  eager: true,
  import: "default",
  query: "?url",
});

const props = defineProps<{
  sets: SoulSet[];
}>();

type CardKey = "head" | "tail";
type SpeedLevel = "normal" | "charged" | "legendary";

/** 卡片属性的结构化展示字段，速度单独带标记以便使用专属视觉色。 */
type DisplayAttribute = {
  label: string;
  value: string;
  rolls: string;
  isSpeed: boolean;
};

type DisplayCard = {
  key: CardKey;
  label: string;
  title: string;
  subtitle: string;
  slot: string;
  mainAttribute: string;
  setLabel: string;
  setIcon: string | null;
  soulKey: string;
  rolls: number;
  rollsKnown: boolean;
  rollsLabel: string;
  attributes: DisplayAttribute[];
  note: string;
  accent: "coral" | "violet";
  speed: number;
  level: SpeedLevel;
};

type CardGroup = {
  key: CardKey;
  title: string;
  subtitle: string;
  cards: DisplayCard[];
};

// 后端只返回当前档案的已保存结果；页面加载和角色切换都不重新扫描完整库存。
const cache = ref<HeadTailCache | null>(null);
const loading = ref(true);
const calculating = ref(false);
const errorMessage = ref("");
const calculationPhase = ref<"reading" | "matching" | "saving">("reading");
let calculationTimer: number | null = null;
let stopActiveCharacterChanged: (() => void) | null = null;

const setById = computed(() => new Map(props.sets.map((set) => [set.setId, set])));

const cacheIsStale = computed(
  () =>
    cache.value !== null &&
    (cache.value.inventoryCount !== cache.value.currentInventoryCount ||
      cache.value.inventoryRevision !== cache.value.currentInventoryRevision),
);
// 只有缓存存在且仍对应当前库存时才展示卡片，避免用户继续查看已失效的头尾结果。
const hasCalculation = computed(() => cache.value !== null && !cacheIsStale.value);

const headCards = computed(() =>
  makeDisplayCards("head", cache.value?.heads ?? []),
);
const tailCards = computed(() =>
  makeDisplayCards("tail", cache.value?.tails ?? []),
);
const resultCards = computed<DisplayCard[]>(() => [
  ...headCards.value,
  ...tailCards.value,
]);
const cardGroups = computed<CardGroup[]>(() => [
  {
    key: "head",
    title: "头榜 · 2号位速度",
    subtitle: "速度主属性，按副属性速度从高到低",
    cards: headCards.value,
  },
  {
    key: "tail",
    title: "尾榜 · 4号位命中 / 抵抗",
    subtitle: "功能主属性，按副属性速度从高到低",
    cards: tailCards.value,
  },
// 过滤空榜单时显式保留 CardGroup 类型，避免 TypeScript 将字面量 key 宽化为 string。
].filter((group): group is CardGroup => group.cards.length > 0));

const combinedSpeed = computed(() => {
  const topPair = [headCards.value[0], tailCards.value[0]].filter(
    (card): card is DisplayCard => card !== undefined,
  );
  if (!topPair.length) return 0;
  return (
    topPair.reduce((total, card) => total + card.speed, 0) / topPair.length
  );
});

const calculationProgress = computed(() => {
  switch (calculationPhase.value) {
    case "reading":
      return 32;
    case "matching":
      return 68;
    case "saving":
      return 88;
  }
});

const calculationMessage = computed(() => {
  switch (calculationPhase.value) {
    case "reading":
      return "正在读取全部已确认御魂…";
    case "matching":
      return "正在匹配 2 号位速度头与 4 号位命中/抵抗尾…";
    case "saving":
      return "正在保存头尾结果…";
  }
});

const statusLabel = computed(() => {
  if (loading.value) return "正在读取已保存结果…";
  if (calculating.value) return calculationMessage.value;
  if (!cache.value) return "尚未计算";
  if (cacheIsStale.value) return "库存已更新，请重新计算";
  return `已保存 ${formatCalculatedAt(cache.value.calculatedAt)}`;
});

/** 按严格大于阈值分层：17.0 不触发，17.1 进入金色，17.6 进入金红动画层。 */
function speedLevel(speed: number): SpeedLevel {
  if (speed > 17.5) return "legendary";
  if (speed > 17) return "charged";
  return "normal";
}

/** 速度条只用于视觉比较，20 是展示上限，不代表游戏内速度封顶。 */
function speedProgress(speed: number): number {
  return Math.min(100, Math.max(0, (speed / 20) * 100));
}

/** 将稳定属性编号转换成卡片上的玩家可读名称，未知属性保留原编号避免丢失事实。 */
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

/** 按后端约定格式化比例和平坦属性；比例属性在数据库中以小数保存。 */
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

/** 两位小数展示速度，确保 16.76、17.18 等速度值不被截断。 */
function formatSpeed(speed: number): string {
  return speed.toFixed(2);
}

/** 根据目录套装返回离线图标；没有覆盖的套装由卡片文字首字回退。 */
function soulIcon(set: SoulSet | undefined): string | null {
  if (!set?.iconAssetId) return null;
  return iconModules[`../assets/soul-icons/${set.iconAssetId}.png`] ?? null;
}

/** 将后端真实卡片映射为 A 版所需的视觉字段；套装名称和图标优先从当前目录读取。 */
function makeDisplayCard(key: CardKey, card: HeadTailCard | null): DisplayCard | null {
  if (!card) return null;
  const mainAttribute = attributeLabel(card.mainAttrType);
  const set = setById.value.get(card.setId);
  const setLabel = set?.name ?? `套装 ${card.setId}`;
  // 速度强化次数沿用后端保存的原始属性事实；次数缺失时只保留卡面结构，不展示推断文案。
  const speedAttribute = card.attributes.find(
    (attribute) => attribute.attributeType === "speed" && !attribute.fixedAttribute,
  );
  const rollsKnown = speedAttribute?.enhancementCount != null;
  const attributes = card.attributes
    .slice()
    .sort((left, right) => Number(left.fixedAttribute) - Number(right.fixedAttribute))
    .map((attribute) => {
      const rolls =
        attribute.enhancementCount !== null ? ` · ${attribute.enhancementCount}手` : "";
      return {
        label: attributeLabel(attribute.attributeType),
        value: formatAttributeValue(attribute.attributeType, attribute.value),
        rolls,
        isSpeed: attribute.attributeType === "speed",
      };
    });

  return {
    key,
    label: key === "head" ? "头" : "尾",
    title: key === "head" ? "速度头" : `${mainAttribute}尾`,
    subtitle: `${card.slot}号位 · ${mainAttribute}主属性`,
    slot: String(card.slot).padStart(2, "0"),
    mainAttribute,
    setLabel,
    setIcon: soulIcon(set),
    soulKey: card.soulKey,
    rolls: card.speedRolls,
    rollsKnown,
    rollsLabel: rollsKnown ? `${card.speedRolls}手` : "",
    attributes,
    note:
      key === "head"
        ? rollsKnown
          ? "5手速度全中 · PVE 先手优先"
          : ""
        : `${mainAttribute}功能位兼顾出手`,
    accent: key === "head" ? "coral" : "violet",
    speed: card.speed,
    level: speedLevel(card.speed),
  };
}

/** 将一组后端候选映射并再次按速度倒序，保障页面即使面对旧缓存也保持用户要求的顺序。 */
function makeDisplayCards(key: CardKey, cards: HeadTailCard[]): DisplayCard[] {
  return cards
    .map((card) => makeDisplayCard(key, card))
    .filter((card): card is DisplayCard => card !== null)
    .sort((left, right) => right.speed - left.speed);
}

/** 读取头尾缓存；此入口只拿 SQLite 派生结果，不触发库存重算。 */
async function loadPage(): Promise<void> {
  loading.value = true;
  errorMessage.value = "";
  try {
    cache.value = await desktopApi.getHeadTailCache();
  } catch (caught) {
    const error = normalizeCommandError(caught);
    errorMessage.value = `${error.code} · ${error.message}`;
  } finally {
    loading.value = false;
  }
}

/** 计算完成后再次读取 SQLite，确保页面展示的是最终落盘结果，而不是命令返回的临时对象。 */
async function reloadSavedCache(): Promise<void> {
  cache.value = await desktopApi.getHeadTailCache();
}

/** 用户显式点击后才计算头尾，并由后端在 SQLite 中覆盖保存当前档案的结果。 */
async function calculateHeadTail(): Promise<void> {
  if (calculating.value) return;
  calculating.value = true;
  calculationPhase.value = "reading";
  startCalculationProgress();
  errorMessage.value = "";
  try {
    cache.value = await desktopApi.calculateHeadTail();
    // 计算命令成功只代表后端完成写入；再读一次缓存让卡片和保存状态走同一条读取链路。
    await reloadSavedCache();
  } catch (caught) {
    const error = normalizeCommandError(caught);
    errorMessage.value = `${error.code} · ${error.message}`;
  } finally {
    stopCalculationProgress();
    calculating.value = false;
  }
}

/** 用阶段提示表达后端一次性命令的等待位置；命令完成前不会虚报 100%。 */
function startCalculationProgress(): void {
  stopCalculationProgress();
  calculationTimer = window.setTimeout(() => {
    calculationPhase.value = "matching";
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

/** 库存清空后立即丢弃头尾缓存，避免短暂展示已删除的御魂。 */
function handleInventoryCleared(): void {
  stopCalculationProgress();
  cache.value = null;
  errorMessage.value = "";
}

/** 角色切换时重新读取对应档案缓存，避免不同角色的头尾结果串用。 */
function handleActiveCharacterChanged(profileId: string | null): void {
  stopCalculationProgress();
  cache.value = null;
  errorMessage.value = "";
  if (profileId) void loadPage();
}

/** 计算时间只用于状态提示；后端时间异常不阻断卡片展示。 */
function formatCalculatedAt(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString("zh-CN");
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
  <section
    class="head-tail-prototype head-tail-prototype--cards"
    aria-labelledby="head-tail-title"
  >
    <header class="head-tail-prototype__header">
      <div>
        <p class="head-tail-prototype__eyebrow">HEAD / TAIL ANALYSIS</p>
        <div class="head-tail-prototype__title-row">
          <h2 id="head-tail-title">头尾分析</h2>
          <span
            v-if="cache && !cacheIsStale"
            class="head-tail-prototype__prototype-badge"
            >计算完成后 · 已保存头尾结果</span
          >
        </div>
        <p class="head-tail-prototype__lede">
          从当前库存里找出最值得看的两类速度御魂：二号位速度主属性为“头”，四号位命中/抵抗为“尾”。
        </p>
      </div>
      <div class="head-tail-prototype__snapshot" aria-label="头尾分析保存状态">
        <span class="head-tail-prototype__snapshot-mark" aria-hidden="true">御</span>
        <div>
          <strong>{{
            cache && !cacheIsStale ? "已保存头尾结果" : "当前库存快照"
          }}</strong>
          <small>{{
            cache ? `${cache.inventoryCount} 枚库存 · ${statusLabel}` : statusLabel
          }}</small>
        </div>
      </div>
    </header>

    <section class="head-tail-prototype__controls" aria-label="头尾分析计算控制">
      <div class="head-tail-prototype__control-copy">
        <span>头尾计算</span>
        <small>仅保留导入数据中明确记录为 5 手速度强化的御魂</small>
      </div>
      <div class="head-tail-prototype__calculation-action">
        <span
          class="head-tail-prototype__calculation-status"
          :class="{
            'head-tail-prototype__calculation-status--stale': cacheIsStale,
          }"
        >
          {{ statusLabel }}
        </span>
        <button
          type="button"
          class="head-tail-prototype__calculate-button"
          :disabled="loading || calculating"
          @click="calculateHeadTail"
        >
          {{ calculating ? "正在计算…" : cache ? "重新计算头尾" : "计算头尾数据" }}
        </button>
      </div>
      <div class="head-tail-prototype__thresholds" aria-label="速度分级说明">
        <span><i class="threshold-dot threshold-dot--normal"></i>≤17 常规</span>
        <span><i class="threshold-dot threshold-dot--charged"></i>&gt;17 金色</span>
        <span
          ><i class="threshold-dot threshold-dot--legendary"></i>&gt;17.5 金红极意</span
        >
      </div>
    </section>

    <p v-if="errorMessage" class="head-tail-prototype__error" role="alert">
      {{ errorMessage }}
    </p>

    <section v-if="loading" class="head-tail-precalculate">
      <span class="head-tail-precalculate__mark" aria-hidden="true">御</span>
      <strong>正在读取已保存的头尾结果…</strong>
      <p>页面只读取 SQLite 缓存，不会自动重新扫描库存。</p>
    </section>

    <section
      v-else-if="!hasCalculation || !resultCards.length"
      class="head-tail-precalculate"
    >
      <template v-if="!hasCalculation">
        <span class="head-tail-precalculate__mark" aria-hidden="true">速</span>
        <strong>{{
          cacheIsStale ? "库存已更新，请重新计算头尾" : "尚未计算头尾数据"
        }}</strong>
        <p>
          点击右上角“{{ cache ? "重新计算头尾" : "计算头尾数据" }}”，系统会筛选 2
          号位速度头和 4 号位命中/抵抗尾，并将结果保存到 SQLite。
        </p>
      </template>
      <template v-else>
        <img
          class="head-tail-precalculate__empty-image"
          :src="emptyResultImage"
          alt="无符合条件的头尾卡片"
        />
        <strong>计算已完成，但没有符合条件的头尾卡片</strong>
        <p>
          已保存 {{ cache?.inventoryCount ?? 0 }} 枚库存；头候选
          {{ cache?.headCandidateCount ?? 0 }} 个、尾候选
          {{
            cache?.tailCandidateCount ?? 0
          }}
          个。导入数据未提供速度强化次数的御魂不会纳入头尾候选。
        </p>
      </template>
    </section>

    <section v-else class="head-tail-cards">
      <div class="head-tail-cards__heading">
        <div>
          <span class="head-tail-cards__kicker">A / HEAD &amp; TAIL RANKINGS</span>
          <h3>头尾候选榜</h3>
        </div>
        <p>全部候选已保存，按副属性速度倒序排列</p>
      </div>
      <div class="head-tail-rankings">
        <section
          v-for="group in cardGroups"
          :key="group.key"
          class="head-tail-ranking"
          :aria-label="group.title"
        >
          <header class="head-tail-ranking__heading">
            <div>
              <span>{{ group.key === "head" ? "HEAD / TOP SPEED" : "TAIL / FUNCTION SPEED" }}</span>
              <h4>{{ group.title }}</h4>
            </div>
            <p>{{ group.subtitle }}</p>
          </header>
          <div class="head-tail-card-grid" role="list">
            <article
              v-for="card in group.cards"
              :key="`${card.key}-${card.soulKey}`"
              class="head-tail-card"
              :class="[
                `head-tail-card--${card.key}`,
                `head-tail-card--${card.level}`,
                `head-tail-card--${card.accent}`,
              ]"
              role="listitem"
            >
              <!-- A 版卡牌的光影层只承担氛围，不承载数据；数据仍保持在普通文案层，避免动画影响阅读。 -->
              <div class="head-tail-card__ambient" aria-hidden="true">
                <span class="head-tail-card__orb head-tail-card__orb--one"></span>
                <span class="head-tail-card__orb head-tail-card__orb--two"></span>
                <span class="head-tail-card__constellation"></span>
              </div>
              <div class="head-tail-card__foil" aria-hidden="true"></div>
              <div class="head-tail-card__topline">
                <span class="head-tail-card__label">{{ card.label }}</span>
                <span class="head-tail-card__slot">{{ card.slot }}号位</span>
                <span class="head-tail-card__rarity">
                  {{
                    card.level === "legendary"
                      ? "UR · 极意"
                      : card.level === "charged"
                        ? "SSR · 发光"
                        : "SR · 速度"
                  }}
                </span>
                <span v-if="card.level === 'legendary'" class="head-tail-card__spark"
                  >✦ 极意</span
                >
                <span v-else-if="card.level === 'charged'" class="head-tail-card__spark"
                  >✦ 发光</span
                >
              </div>
              <div class="head-tail-card__identity">
                <span class="head-tail-card__set-icon" :title="`${card.setLabel}图标`">
                  <img v-if="card.setIcon" :src="card.setIcon" :alt="`${card.setLabel}图标`" />
                  <span v-else>{{ card.setLabel.slice(0, 1) }}</span>
                </span>
                <div>
                  <h4>{{ card.title }}</h4>
                  <p>{{ card.subtitle }}</p>
                </div>
              </div>
              <div class="head-tail-card__speed-block">
                <div>
                  <small>
                    副属性速度<span v-if="card.rollsLabel"> · {{ card.rollsLabel }}</span>
                  </small>
                  <strong>{{ formatSpeed(card.speed) }}</strong>
                </div>
                <span class="head-tail-card__speed-grade">{{
                  card.level === "legendary" ? "SSS" : card.level === "charged" ? "SS" : "A"
                }}</span>
              </div>
              <div class="head-tail-card__meter" aria-hidden="true">
                <span :style="{ width: `${speedProgress(card.speed)}%` }"></span>
              </div>
              <div
                class="head-tail-card__pips"
                :aria-label="
                  card.rollsKnown ? `${card.rolls} 手速度强化` : '速度强化次数未知'
                "
              >
                <span
                  v-for="pip in 5"
                  :key="pip"
                  :class="{ 'is-filled': pip <= card.rolls, 'is-empty': pip > card.rolls }"
                ></span>
                <small v-if="card.rollsKnown">{{ card.rolls }} / 5 速度强化</small>
              </div>
              <div class="head-tail-card__facts">
                <span class="head-tail-card__main-attribute"
                  >主属性 · {{ card.mainAttribute }}</span
                >
                <span class="head-tail-card__set">{{ card.setLabel }}</span>
              </div>
              <div class="head-tail-card__attributes">
                <span
                  v-for="(attribute, index) in card.attributes"
                  :key="`${card.key}-${card.soulKey}-${index}-${attribute.label}`"
                  :class="{ 'head-tail-card__attribute--speed': attribute.isSpeed }"
                >
                  <span>{{ attribute.label }}</span>
                  <strong>{{ attribute.value }}</strong>
                  <small v-if="attribute.rolls">{{ attribute.rolls }}</small>
                </span>
              </div>
              <footer v-if="card.note" class="head-tail-card__footer">
                <span>{{ card.note }}</span>
              </footer>
            </article>
          </div>
        </section>
      </div>
      <div class="head-tail-insight">
        <span class="head-tail-insight__icon" aria-hidden="true">↗</span>
        <div>
          <strong>头尾合看：平均副属性速度 {{ formatSpeed(combinedSpeed) }}</strong>
          <p>
            头负责把速度上限拉高，尾负责在命中/抵抗功能位里保留出手权；当前已保存
            {{ cache?.headCandidateCount ?? 0 }} 个头候选、{{
              cache?.tailCandidateCount ?? 0
            }}
            个尾候选。
          </p>
        </div>
        <span class="head-tail-insight__tag">SQLite 已保存</span>
      </div>
    </section>

    <footer class="head-tail-prototype__note">
      <span class="head-tail-prototype__note-icon" aria-hidden="true">i</span>
      <p>
        计算口径：头 = 2号位主属性速度；尾 =
        4号位主属性效果命中/抵抗；两者都要求速度副属性满足五手速度口径。
        只有明确记录为 5 手速度强化才纳入；次数缺失时不推断，结果按当前角色和库存版本保存，库存更新后需要重新计算。
      </p>
    </footer>

    <Teleport to="body">
      <div
        v-if="calculating"
        class="head-tail-calculation-mask"
        role="dialog"
        aria-modal="true"
        aria-labelledby="head-tail-calculation-title"
      >
        <div class="head-tail-calculation-dialog">
          <div class="head-tail-calculation-dialog__mark" aria-hidden="true">头</div>
          <span class="head-tail-calculation-dialog__eyebrow"
            >HEAD / TAIL · CALCULATING</span
          >
          <h3 id="head-tail-calculation-title">正在计算头尾数据</h3>
          <p>正在读取当前角色库存、匹配明确的五手速度，并保存头尾候选。</p>
          <div
            class="head-tail-calculation-progress"
            role="progressbar"
            aria-valuemin="0"
            aria-valuemax="100"
            :aria-valuenow="calculationProgress"
            :aria-valuetext="calculationMessage"
          >
            <span
              class="head-tail-calculation-progress__fill"
              :style="{ width: `${calculationProgress}%` }"
            ></span>
          </div>
          <div class="head-tail-calculation-dialog__meta">
            <span>{{ calculationMessage }}</span>
            <strong>{{ calculationProgress }}%</strong>
          </div>
          <span class="head-tail-calculation-dialog__hint">请保持当前页面打开</span>
        </div>
      </div>
    </Teleport>
  </section>
</template>

<style scoped>
.head-tail-prototype {
  --ht-ink: #243348;
  --ht-muted: #708095;
  --ht-faint: #a2afbd;
  --ht-line: #dae4ed;
  --ht-paper: #f8fbfd;
  --ht-blue: #3e8de4;
  --ht-aqua: #43c9bd;
  --ht-coral: #e6785c;
  --ht-violet: #8062db;
  position: relative;
  display: grid;
  gap: 18px;
  min-width: 0;
  padding-bottom: 78px;
  color: var(--ht-ink);
}

.head-tail-prototype__header,
.head-tail-prototype__controls,
.head-tail-cards__heading,
.head-tail-prototype__note {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 18px;
}

.head-tail-prototype__header {
  padding-bottom: 16px;
  border-bottom: 1px solid var(--ht-line);
}

.head-tail-prototype__eyebrow,
.head-tail-cards__kicker {
  margin: 0;
  color: var(--ht-blue);
  font-size: 10px;
  font-weight: 900;
  letter-spacing: 0.15em;
}

.head-tail-prototype__title-row {
  display: flex;
  align-items: center;
  gap: 9px;
  margin: 7px 0 5px;
}

.head-tail-prototype h2,
.head-tail-cards h3 {
  margin: 0;
  font-family: Georgia, "Noto Serif SC", serif;
  letter-spacing: -0.03em;
}

.head-tail-prototype h2 {
  font-size: clamp(24px, 3vw, 32px);
}

.head-tail-prototype__prototype-badge {
  padding: 4px 7px;
  border: 1px solid rgb(62 141 228 / 30%);
  border-radius: 999px;
  color: var(--ht-blue);
  background: rgb(62 141 228 / 8%);
  font-size: 10px;
  font-weight: 800;
}

.head-tail-prototype__lede,
.head-tail-cards__heading p {
  margin: 0;
  color: var(--ht-muted);
  font-size: 12px;
  line-height: 1.6;
}

.head-tail-prototype__lede {
  max-width: 650px;
}

.head-tail-prototype__snapshot {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 11px 13px;
  border: 1px solid var(--ht-line);
  border-radius: 10px;
  background: rgb(255 255 255 / 70%);
  box-shadow: 0 8px 20px rgb(45 74 108 / 6%);
}

.head-tail-prototype__snapshot-mark {
  display: grid;
  width: 34px;
  height: 34px;
  place-items: center;
  border: 1px solid rgb(62 141 228 / 28%);
  border-radius: 50%;
  color: var(--ht-blue);
  background: #edf6ff;
  font-family: STKaiti, KaiTi, serif;
  font-size: 18px;
}

.head-tail-prototype__snapshot div {
  display: grid;
  gap: 3px;
}

.head-tail-prototype__snapshot strong {
  font-size: 12px;
}

.head-tail-prototype__snapshot small,
.head-tail-prototype__control-copy small {
  color: var(--ht-muted);
  font-size: 10px;
}

.head-tail-prototype__controls {
  flex-wrap: wrap;
  padding: 12px 14px;
  border: 1px solid var(--ht-line);
  border-radius: 10px;
  background: linear-gradient(105deg, #fff, #f5faff);
}

.head-tail-prototype__control-copy {
  display: grid;
  gap: 3px;
  margin-right: auto;
}

.head-tail-prototype__control-copy > span {
  font-size: 12px;
  font-weight: 800;
}

.head-tail-prototype__calculation-action {
  display: flex;
  align-items: center;
  gap: 9px;
  margin-left: auto;
}

.head-tail-prototype__calculation-status {
  color: var(--ht-muted);
  font-size: 10px;
  white-space: nowrap;
}

.head-tail-prototype__calculation-status--stale {
  color: #b87425;
  font-weight: 800;
}

.head-tail-prototype__calculate-button {
  min-height: 31px;
  padding: 6px 11px;
  border: 1px solid #d29331;
  border-radius: 7px;
  color: #fffaf0;
  background: linear-gradient(135deg, #b87923, #d9972f);
  box-shadow: 0 4px 10px rgb(184 121 35 / 20%);
  font-size: 10px;
  font-weight: 900;
  cursor: pointer;
}

.head-tail-prototype__calculate-button:hover:not(:disabled) {
  background: linear-gradient(135deg, #9f641b, #c68122);
}

.head-tail-prototype__calculate-button:disabled {
  cursor: wait;
  opacity: 0.6;
}

.head-tail-prototype__error {
  margin: 0;
  padding: 9px 11px;
  border: 1px solid rgb(186 80 65 / 32%);
  border-radius: 8px;
  color: #a14b3e;
  background: rgb(255 240 236 / 86%);
  font-size: 11px;
  line-height: 1.5;
}

.head-tail-precalculate {
  display: grid;
  justify-items: center;
  gap: 8px;
  padding: 58px 24px 66px;
  border: 1px dashed rgb(122 164 205 / 48%);
  border-radius: 15px;
  color: var(--ht-muted);
  background:
    radial-gradient(circle at 50% 0%, rgb(255 202 89 / 13%), transparent 34%),
    linear-gradient(145deg, rgb(255 255 255 / 84%), rgb(240 247 255 / 80%));
  text-align: center;
}

/* 仅在计算完成但没有候选时展示用户提供的表情，避免与未计算和过期状态混淆。 */
.head-tail-precalculate__empty-image {
  width: 80px;
  height: 80px;
  margin: 0 auto 12px;
}

.head-tail-precalculate__mark {
  display: grid;
  width: 43px;
  height: 43px;
  place-items: center;
  border: 1px solid rgb(213 153 47 / 55%);
  border-radius: 50%;
  color: #aa6a19;
  background: rgb(255 231 168 / 55%);
  font-family: STKaiti, KaiTi, serif;
  font-size: 22px;
}

.head-tail-precalculate strong {
  color: var(--ht-ink);
  font-size: 14px;
}

.head-tail-precalculate p {
  max-width: 520px;
  margin: 0;
  font-size: 11px;
  line-height: 1.6;
}

.head-tail-prototype__thresholds {
  display: flex;
  flex-wrap: wrap;
  gap: 9px;
  color: var(--ht-muted);
  font-size: 10px;
}

.head-tail-prototype__thresholds span {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.threshold-dot {
  display: inline-block;
  width: 6px;
  height: 6px;
  border-radius: 50%;
}

.threshold-dot--normal {
  background: #aab6c0;
}

.threshold-dot--charged {
  background: #f4bd49;
  box-shadow: 0 0 0 3px rgb(244 189 73 / 16%);
}

.threshold-dot--legendary {
  background: #ed6647;
  box-shadow: 0 0 0 3px rgb(237 102 71 / 16%);
}

.head-tail-cards {
  display: grid;
  gap: 14px;
}

.head-tail-cards__heading {
  align-items: end;
}

.head-tail-cards__heading h3 {
  margin-top: 5px;
  font-size: 21px;
}

.head-tail-card-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 14px;
}

/* 头榜和尾榜分组展示，组内卡片保持速度倒序，避免大量候选挤成一张无层次的长卡组。 */
.head-tail-rankings {
  display: grid;
  gap: 24px;
}

.head-tail-ranking {
  display: grid;
  gap: 10px;
}

.head-tail-ranking__heading {
  display: flex;
  align-items: end;
  justify-content: space-between;
  gap: 14px;
  padding-bottom: 8px;
  border-bottom: 1px solid var(--ht-line);
}

.head-tail-ranking__heading span {
  color: var(--ht-blue);
  font-size: 10px;
  font-weight: 900;
  letter-spacing: 0.12em;
}

.head-tail-ranking__heading h4 {
  margin: 3px 0 0;
  color: var(--ht-ink);
  font-size: 17px;
}

.head-tail-ranking__heading p {
  margin: 0;
  color: var(--ht-muted);
  font-size: 11px;
}

.head-tail-card {
  position: relative;
  display: grid;
  gap: 14px;
  min-width: 0;
  overflow: hidden;
  padding: 17px;
  border: 1px solid var(--ht-line);
  border-radius: 15px;
  background: linear-gradient(150deg, #fff, #f7fafc);
  box-shadow: 0 13px 28px rgb(44 75 104 / 7%);
}

.head-tail-card::before {
  position: absolute;
  top: 0;
  right: 0;
  left: 0;
  height: 4px;
  background: var(--ht-accent, var(--ht-blue));
  content: "";
}

.head-tail-card--coral {
  --ht-accent: var(--ht-coral);
}

.head-tail-card--violet {
  --ht-accent: var(--ht-violet);
}

.head-tail-card--charged {
  border-color: rgb(62 141 228 / 44%);
  background: linear-gradient(145deg, #fff, #f1f8ff);
  box-shadow: 0 15px 32px rgb(62 141 228 / 12%);
}

.head-tail-card--charged .head-tail-card__speed-grade {
  color: var(--ht-blue);
  background: rgb(62 141 228 / 10%);
}

.head-tail-card--legendary {
  border-color: rgb(155 103 222 / 54%);
  background:
    radial-gradient(circle at 90% 8%, rgb(223 197 255 / 45%), transparent 28%),
    linear-gradient(135deg, #fff, #f6f0ff 62%, #fff8e6);
  box-shadow:
    0 16px 36px rgb(135 89 203 / 17%),
    0 0 0 3px rgb(181 125 232 / 12%);
}

/* >17.5 的极意卡片使用缓慢扫光，表达“更酷”但不抢过速度数字。 */
.head-tail-card--legendary::after {
  position: absolute;
  top: -35%;
  bottom: -35%;
  left: -35%;
  width: 20%;
  background: linear-gradient(100deg, transparent, rgb(255 255 255 / 80%), transparent);
  content: "";
  transform: skewX(-16deg);
  animation: head-tail-sweep 3.8s ease-in-out infinite;
}

.head-tail-card__topline,
.head-tail-card__facts,
.head-tail-card__footer {
  display: flex;
  align-items: center;
  gap: 8px;
}

.head-tail-card__topline {
  color: var(--ht-muted);
  font-size: 10px;
  font-weight: 800;
}

.head-tail-card__label {
  display: inline-grid;
  width: 27px;
  height: 27px;
  place-items: center;
  border-radius: 8px;
  color: #fff;
  background: var(--ht-accent, var(--ht-blue));
  font-size: 15px;
  font-variant-numeric: tabular-nums;
}

.head-tail-card__slot {
  margin-right: auto;
}

.head-tail-card__spark {
  color: #8851c1;
  font-size: 10px;
}

.head-tail-card__identity {
  display: flex;
  align-items: center;
  gap: 11px;
}

.head-tail-card__set-icon {
  display: grid;
  width: 46px;
  height: 46px;
  flex: 0 0 46px;
  place-items: center;
  overflow: hidden;
  border: 1px solid var(--ht-line);
  border-radius: 13px;
  color: var(--ht-accent, var(--ht-blue));
  background: rgb(255 255 255 / 84%);
  font-family: STKaiti, KaiTi, serif;
  font-size: 18px;
  font-weight: 800;
}

.head-tail-card__set-icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.head-tail-card__identity h4 {
  margin: 0 0 3px;
  font-size: 18px;
}

.head-tail-card__identity p {
  margin: 0;
  color: var(--ht-muted);
  font-size: 11px;
}

.head-tail-card__speed-block {
  display: flex;
  align-items: end;
  justify-content: space-between;
  gap: 10px;
}

.head-tail-card__speed-block div {
  display: grid;
  gap: 4px;
}

.head-tail-card__speed-block small {
  color: var(--ht-muted);
  font-size: 10px;
}

.head-tail-card__speed-block strong {
  color: var(--ht-ink);
  font-size: 42px;
  font-variant-numeric: tabular-nums;
  line-height: 1.1;
  letter-spacing: -0.05em;
}

.head-tail-card--charged .head-tail-card__speed-block strong {
  color: var(--ht-blue);
}

.head-tail-card--legendary .head-tail-card__speed-block strong {
  color: #8851c1;
  text-shadow: 0 0 12px rgb(175 115 230 / 38%);
}

.head-tail-card__speed-grade {
  padding: 5px 7px;
  border-radius: 6px;
  color: var(--ht-muted);
  background: #eef3f6;
  font-size: 15px;
  font-variant-numeric: tabular-nums;
  font-weight: 800;
}

.head-tail-card--legendary .head-tail-card__speed-grade {
  color: #8851c1;
  background: rgb(162 98 230 / 12%);
}

.head-tail-card__meter {
  height: 5px;
  overflow: hidden;
  border-radius: 999px;
  background: #e8eff3;
}

.head-tail-card__meter span {
  display: block;
  height: 100%;
  border-radius: inherit;
  background: linear-gradient(90deg, var(--ht-aqua), var(--ht-blue));
}

.head-tail-card--legendary .head-tail-card__meter span {
  background: linear-gradient(90deg, var(--ht-blue), #b16be8, #efbd54);
}

.head-tail-card__facts {
  flex-wrap: wrap;
  color: var(--ht-muted);
  font-size: 10px;
}

.head-tail-card__main-attribute {
  color: var(--ht-ink);
  font-weight: 800;
}

.head-tail-card__set {
  margin-left: auto;
  padding: 4px 7px;
  border: 1px solid var(--ht-line);
  border-radius: 999px;
  background: #fff;
}

.head-tail-card__attributes {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.head-tail-card__attributes > span {
  display: inline-flex;
  align-items: baseline;
  gap: 4px;
  padding: 5px 7px;
  border-radius: 6px;
  color: var(--ht-muted);
  background: #eef4f7;
  font-size: 10px;
}

.head-tail-card__attributes > span > strong {
  color: inherit;
  font-weight: 800;
}

.head-tail-card__attributes > span > small {
  color: inherit;
  font-size: 9px;
  opacity: 0.78;
}

.head-tail-card--legendary .head-tail-card__attributes > span:first-child {
  color: #8851c1;
  background: rgb(162 98 230 / 10%);
  font-weight: 800;
}

.head-tail-card__footer {
  justify-content: space-between;
  padding-top: 10px;
  border-top: 1px dashed var(--ht-line);
  color: var(--ht-muted);
  font-size: 10px;
}

.head-tail-card__footer b {
  color: var(--ht-blue);
  font-size: 10px;
}

.head-tail-insight,
.head-tail-prototype__note {
  border: 1px solid var(--ht-line);
  border-radius: 10px;
  background: rgb(255 255 255 / 74%);
}

.head-tail-insight {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 14px;
}

.head-tail-insight__icon {
  display: grid;
  width: 27px;
  height: 27px;
  place-items: center;
  border-radius: 8px;
  color: #fff;
  background: var(--ht-aqua);
  font-weight: 900;
}

.head-tail-insight div {
  display: grid;
  gap: 3px;
  min-width: 0;
}

.head-tail-insight strong {
  font-size: 12px;
}

.head-tail-insight p {
  margin: 0;
  color: var(--ht-muted);
  font-size: 10px;
}

.head-tail-insight__tag {
  margin-left: auto;
  padding: 5px 8px;
  border-radius: 999px;
  color: #2c7f75;
  background: #e7f7f2;
  font-size: 10px;
  font-weight: 800;
  white-space: nowrap;
}

/* A 版是当前首选方向：把两张卡做成深色稀有卡牌，光影集中在卡面而不是整页乱闪。 */
.head-tail-prototype--cards .head-tail-card {
  min-height: 445px;
  border: 1px solid rgb(122 164 205 / 42%);
  border-radius: 18px;
  color: #eef6ff;
  background:
    radial-gradient(circle at 80% 16%, rgb(69 125 197 / 22%), transparent 30%),
    linear-gradient(145deg, #1b3150 0%, #111d35 48%, #0a1327 100%);
  box-shadow:
    0 22px 42px rgb(20 40 66 / 26%),
    inset 0 0 0 1px rgb(255 255 255 / 5%),
    inset 0 -20px 40px rgb(3 10 24 / 34%);
  isolation: isolate;
  transform-style: preserve-3d;
  transition:
    transform 360ms cubic-bezier(0.2, 0.8, 0.2, 1),
    box-shadow 360ms ease,
    border-color 360ms ease;
}

.head-tail-prototype--cards .head-tail-card:hover {
  border-color: rgb(152 213 255 / 72%);
  box-shadow:
    0 28px 54px rgb(20 40 66 / 34%),
    0 0 28px var(--ht-card-glow, rgb(67 159 255 / 24%)),
    inset 0 0 0 1px rgb(255 255 255 / 12%),
    inset 0 -20px 40px rgb(3 10 24 / 34%);
  transform: perspective(900px) rotateX(1.5deg) rotateY(-1.5deg) translateY(-8px);
}

.head-tail-prototype--cards .head-tail-card--head {
  --ht-card-glow: rgb(255 126 98 / 48%);
}

.head-tail-prototype--cards .head-tail-card--tail {
  --ht-card-glow: rgb(145 103 255 / 48%);
}

.head-tail-prototype--cards .head-tail-card::before {
  z-index: 3;
  top: 0;
  right: 11%;
  left: 11%;
  height: 1px;
  background: linear-gradient(90deg, transparent, rgb(204 237 255 / 88%), transparent);
  box-shadow: 0 0 16px rgb(129 205 255 / 64%);
  pointer-events: none;
}

.head-tail-prototype--cards
  .head-tail-card
  > :not(.head-tail-card__ambient):not(.head-tail-card__foil) {
  position: relative;
  z-index: 2;
}

.head-tail-card__ambient,
.head-tail-card__foil {
  position: absolute;
  z-index: 0;
  inset: 0;
  overflow: hidden;
  border-radius: inherit;
  pointer-events: none;
}

.head-tail-card__orb {
  position: absolute;
  display: block;
  width: 220px;
  height: 220px;
  border-radius: 50%;
  opacity: 0.68;
  filter: blur(4px);
  transform: translate3d(0, 0, 0);
}

.head-tail-card__orb--one {
  top: -106px;
  right: -54px;
  background: radial-gradient(
    circle,
    var(--ht-card-glow, rgb(67 159 255 / 58%)),
    transparent 68%
  );
  animation: head-tail-orb-one 8s ease-in-out infinite alternate;
}

.head-tail-card__orb--two {
  bottom: -150px;
  left: -84px;
  width: 240px;
  height: 240px;
  background: radial-gradient(circle, rgb(48 213 204 / 34%), transparent 67%);
  animation: head-tail-orb-two 10s ease-in-out infinite alternate;
}

.head-tail-card__constellation {
  position: absolute;
  inset: 18px;
  display: block;
  opacity: 0.18;
  background-image:
    radial-gradient(circle at 12% 18%, #fff 0 1px, transparent 2px),
    radial-gradient(circle at 76% 28%, #fff 0 1px, transparent 2px),
    radial-gradient(circle at 35% 74%, #fff 0 1px, transparent 2px),
    radial-gradient(circle at 89% 82%, #fff 0 1px, transparent 2px),
    linear-gradient(
      28deg,
      transparent 47%,
      rgb(185 226 255 / 42%) 48%,
      transparent 49%
    ),
    linear-gradient(
      142deg,
      transparent 48%,
      rgb(185 226 255 / 30%) 49%,
      transparent 50%
    );
  background-size: 100% 100%;
  animation: head-tail-constellation 12s ease-in-out infinite alternate;
}

.head-tail-card__foil {
  z-index: 1;
  inset: -42%;
  opacity: 0.12;
  background: conic-gradient(
    from 120deg,
    transparent 0deg,
    rgb(255 255 255 / 50%) 38deg,
    transparent 76deg,
    rgb(104 218 255 / 38%) 120deg,
    transparent 170deg,
    rgb(255 206 114 / 34%) 218deg,
    transparent 270deg,
    rgb(192 129 255 / 34%) 316deg,
    transparent 360deg
  );
  filter: blur(12px);
  mix-blend-mode: screen;
  transform: rotate(0deg);
  animation: head-tail-card-foil 10s linear infinite;
}

.head-tail-prototype--cards .head-tail-card--charged {
  /* >17 是金色档，表达速度已经越过第一道稀有阈值。 */
  --ht-card-glow: rgb(255 190 54 / 58%);
  border-color: rgb(255 211 103 / 72%);
  background:
    radial-gradient(circle at 83% 17%, rgb(255 201 71 / 30%), transparent 31%),
    linear-gradient(145deg, #493d24 0%, #1e2938 48%, #0b182d 100%);
}

.head-tail-prototype--cards .head-tail-card--legendary {
  /* >17.5 是金红极意档，金色负责稀有感，朱红负责爆发感。 */
  --ht-card-glow: rgb(255 91 54 / 72%);
  border-color: rgb(255 178 104 / 82%);
  background:
    radial-gradient(circle at 83% 15%, rgb(255 77 49 / 42%), transparent 32%),
    radial-gradient(circle at 18% 88%, rgb(255 190 66 / 24%), transparent 30%),
    linear-gradient(145deg, #5a2d2a 0%, #2c202b 45%, #0b132d 100%);
  box-shadow:
    0 24px 50px rgb(91 34 29 / 42%),
    0 0 26px rgb(255 93 51 / 34%),
    inset 0 0 0 1px rgb(255 255 255 / 13%),
    inset 0 -22px 45px rgb(3 10 24 / 38%);
}

.head-tail-prototype--cards .head-tail-card--charged .head-tail-card__foil {
  opacity: 0.24;
  animation-duration: 8s;
  background: conic-gradient(
    from 120deg,
    transparent 0deg,
    rgb(255 244 177 / 55%) 38deg,
    transparent 76deg,
    rgb(255 191 55 / 42%) 120deg,
    transparent 170deg,
    rgb(255 222 123 / 40%) 218deg,
    transparent 270deg,
    rgb(255 155 63 / 34%) 316deg,
    transparent 360deg
  );
}

.head-tail-prototype--cards .head-tail-card--legendary .head-tail-card__ambient {
  opacity: 1;
}

.head-tail-prototype--cards .head-tail-card--legendary .head-tail-card__foil {
  opacity: 0.38;
  animation-duration: 5.6s;
  background: conic-gradient(
    from 120deg,
    transparent 0deg,
    rgb(255 236 157 / 68%) 35deg,
    transparent 72deg,
    rgb(255 116 63 / 54%) 120deg,
    transparent 166deg,
    rgb(255 196 83 / 54%) 218deg,
    transparent 270deg,
    rgb(255 83 51 / 50%) 316deg,
    transparent 360deg
  );
}

.head-tail-prototype--cards .head-tail-card__topline {
  color: #91a9c3;
}

.head-tail-prototype--cards .head-tail-card__label {
  border: 1px solid rgb(255 255 255 / 24%);
  box-shadow: 0 0 12px var(--ht-card-glow, rgb(67 159 255 / 22%));
}

.head-tail-prototype--cards .head-tail-card__slot {
  color: #b1c4d8;
}

.head-tail-card__rarity {
  margin-left: auto;
  padding: 4px 6px;
  border: 1px solid rgb(186 221 255 / 22%);
  border-radius: 999px;
  color: #b5ccec;
  background: rgb(125 183 237 / 10%);
  font-family: Consolas, monospace;
  font-size: 9px;
  letter-spacing: 0.04em;
  white-space: nowrap;
}

.head-tail-prototype--cards .head-tail-card--charged .head-tail-card__rarity {
  border-color: rgb(255 215 116 / 62%);
  color: #ffdc81;
  background: rgb(255 190 54 / 14%);
}

.head-tail-prototype--cards .head-tail-card--legendary .head-tail-card__rarity {
  border-color: rgb(255 190 119 / 72%);
  color: #ffd5a7;
  background: linear-gradient(90deg, rgb(255 190 54 / 16%), rgb(255 81 51 / 18%));
  box-shadow: 0 0 11px rgb(255 94 52 / 28%);
}

.head-tail-prototype--cards .head-tail-card__spark {
  color: #f6cb73;
  text-shadow: 0 0 10px rgb(255 190 80 / 58%);
}

.head-tail-prototype--cards .head-tail-card--legendary .head-tail-card__spark {
  color: #ffbd71;
  text-shadow: 0 0 10px rgb(255 93 50 / 72%);
}

.head-tail-prototype--cards .head-tail-card__identity {
  min-height: 70px;
}

.head-tail-prototype--cards .head-tail-card__identity h4 {
  color: #f5f9ff;
  font-size: 20px;
  text-shadow: 0 2px 10px rgb(1 8 20 / 32%);
}

.head-tail-prototype--cards .head-tail-card__identity p,
.head-tail-prototype--cards .head-tail-card__speed-block small {
  color: #9fb6ce;
}

.head-tail-prototype--cards .head-tail-card__speed-block strong {
  color: #f4f8ff;
  font-size: 50px;
  text-shadow: 0 4px 16px rgb(0 7 18 / 38%);
}

.head-tail-prototype--cards
  .head-tail-card--charged
  .head-tail-card__speed-block
  strong {
  color: #ffd975;
  text-shadow: 0 0 17px rgb(255 193 55 / 68%);
}

.head-tail-prototype--cards
  .head-tail-card--legendary
  .head-tail-card__speed-block
  strong {
  color: #ffd5a0;
  text-shadow:
    0 0 9px rgb(255 151 71 / 94%),
    0 0 22px rgb(255 74 48 / 62%);
}

.head-tail-prototype--cards .head-tail-card__speed-grade {
  border: 1px solid rgb(185 220 255 / 30%);
  color: #c3d8ee;
  background: rgb(126 183 235 / 13%);
  box-shadow: inset 0 0 0 1px rgb(255 255 255 / 5%);
}

.head-tail-prototype--cards .head-tail-card--charged .head-tail-card__speed-grade {
  border-color: rgb(255 214 112 / 64%);
  color: #ffdb7d;
  background: rgb(255 190 54 / 15%);
}

.head-tail-prototype--cards .head-tail-card--legendary .head-tail-card__speed-grade {
  border-color: rgb(255 192 126 / 74%);
  color: #ffd6aa;
  background: linear-gradient(90deg, rgb(255 190 54 / 17%), rgb(255 78 49 / 19%));
  box-shadow: 0 0 12px rgb(255 88 50 / 30%);
}

.head-tail-prototype--cards .head-tail-card__meter {
  background: rgb(152 190 224 / 15%);
  box-shadow: inset 0 1px 2px rgb(0 5 16 / 35%);
}

.head-tail-prototype--cards .head-tail-card__meter span {
  background: linear-gradient(90deg, #4db7d5, #7ce9db);
  box-shadow: 0 0 9px rgb(82 219 224 / 62%);
}

.head-tail-prototype--cards .head-tail-card--charged .head-tail-card__meter span {
  background: linear-gradient(90deg, #d9972f, #ffdc70);
  box-shadow: 0 0 11px rgb(255 193 55 / 72%);
}

.head-tail-prototype--cards .head-tail-card--legendary .head-tail-card__meter span {
  background: linear-gradient(90deg, #ffb642, #ff754e, #e94236);
  box-shadow: 0 0 12px rgb(255 102 59 / 78%);
}

.head-tail-card__pips {
  display: flex;
  align-items: center;
  gap: 5px;
}

.head-tail-card__pips > span {
  display: block;
  width: 12px;
  height: 4px;
  border-radius: 999px;
  background: rgb(152 190 224 / 20%);
}

.head-tail-card__pips .is-filled {
  background: #61d9d5;
  box-shadow: 0 0 7px rgb(74 219 214 / 60%);
}

.head-tail-card__pips small {
  margin-left: 4px;
  color: #8ca7c1;
  font-family: Consolas, monospace;
  font-size: 9px;
  letter-spacing: 0.02em;
}

.head-tail-prototype--cards
  .head-tail-card--legendary
  .head-tail-card__pips
  .is-filled {
  background: linear-gradient(90deg, #ffd36b, #ff684b);
  box-shadow: 0 0 8px rgb(255 101 59 / 82%);
}

.head-tail-prototype--cards .head-tail-card--charged .head-tail-card__pips .is-filled {
  background: #ffd36b;
  box-shadow: 0 0 8px rgb(255 194 54 / 80%);
}

.head-tail-prototype--cards .head-tail-card__facts {
  color: #9bb2cb;
}

.head-tail-prototype--cards .head-tail-card__main-attribute {
  color: #e6f2ff;
}

.head-tail-prototype--cards .head-tail-card__set {
  border-color: rgb(173 211 246 / 24%);
  color: #b9cfe5;
  background: rgb(132 181 225 / 10%);
}

.head-tail-prototype--cards .head-tail-card__attributes > span {
  border: 1px solid rgb(174 211 246 / 14%);
  color: #abc3db;
  background: rgb(135 183 226 / 9%);
}

.head-tail-prototype--cards
  .head-tail-card--legendary
  .head-tail-card__attributes
  > span:first-child {
  border-color: rgb(255 188 125 / 46%);
  color: #ffd6a7;
  background: linear-gradient(90deg, rgb(255 190 54 / 15%), rgb(255 79 49 / 16%));
  box-shadow: 0 0 10px rgb(255 94 52 / 21%);
}

.head-tail-prototype--cards
  .head-tail-card--charged
  .head-tail-card__attributes
  > span:first-child {
  border-color: rgb(255 215 115 / 38%);
  color: #ffdc83;
  background: rgb(255 190 54 / 14%);
  box-shadow: 0 0 10px rgb(255 194 55 / 17%);
}

/* 速度是头尾卡片的核心指标，固定使用青绿色高亮，与金色/金红稀有档形成对比。 */
.head-tail-prototype--cards .head-tail-card__attributes > .head-tail-card__attribute--speed {
  border-color: rgb(88 235 224 / 62%);
  color: #7cf0e7;
  background: linear-gradient(90deg, rgb(34 182 192 / 19%), rgb(117 244 216 / 14%));
  box-shadow: 0 0 11px rgb(67 224 218 / 28%);
}

.head-tail-prototype--cards
  .head-tail-card--charged
  .head-tail-card__attributes
  > .head-tail-card__attribute--speed,
.head-tail-prototype--cards
  .head-tail-card--legendary
  .head-tail-card__attributes
  > .head-tail-card__attribute--speed {
  border-color: rgb(102 241 228 / 72%);
  color: #9bfff0;
  background: linear-gradient(90deg, rgb(25 164 181 / 20%), rgb(255 192 72 / 10%));
  box-shadow: 0 0 13px rgb(72 232 221 / 36%);
}

.head-tail-prototype--cards .head-tail-card__footer {
  border-top-color: rgb(169 209 246 / 20%);
  color: #8ea8c2;
}

@keyframes head-tail-card-foil {
  from {
    transform: rotate(0deg) scale(1);
  }

  to {
    transform: rotate(360deg) scale(1.08);
  }
}

@keyframes head-tail-orb-one {
  from {
    transform: translate3d(0, 0, 0) scale(0.92);
  }

  to {
    transform: translate3d(-22px, 18px, 0) scale(1.12);
  }
}

@keyframes head-tail-orb-two {
  from {
    transform: translate3d(0, 0, 0) scale(1);
  }

  to {
    transform: translate3d(26px, -18px, 0) scale(0.84);
  }
}

@keyframes head-tail-constellation {
  from {
    opacity: 0.12;
    transform: translate3d(0, 0, 0) rotate(0deg);
  }

  to {
    opacity: 0.28;
    transform: translate3d(8px, -4px, 0) rotate(2deg);
  }
}

.head-tail-prototype__note {
  align-items: flex-start;
  justify-content: flex-start;
  padding: 10px 12px;
  color: var(--ht-muted);
  font-size: 10px;
  line-height: 1.55;
}

.head-tail-prototype__note-icon {
  display: grid;
  width: 17px;
  height: 17px;
  flex: 0 0 17px;
  place-items: center;
  border: 1px solid rgb(112 128 149 / 35%);
  border-radius: 50%;
  font-weight: 900;
}

.head-tail-prototype__note p {
  margin: 0;
}

/* 计算期间锁住当前页面，避免重复点击并明确提示 SQLite 保存阶段。 */
.head-tail-calculation-mask {
  position: fixed;
  inset: 0;
  z-index: 1000;
  display: grid;
  place-items: center;
  padding: 24px;
  background: rgb(12 25 43 / 46%);
  backdrop-filter: blur(5px);
}

.head-tail-calculation-dialog {
  width: min(440px, 100%);
  padding: 30px 32px 27px;
  border: 1px solid rgb(255 255 255 / 72%);
  border-radius: 15px;
  color: #34465b;
  background: rgb(250 253 255 / 97%);
  box-shadow: 0 24px 64px rgb(9 23 42 / 30%);
}

.head-tail-calculation-dialog__mark {
  display: grid;
  width: 40px;
  height: 40px;
  place-items: center;
  margin-bottom: 15px;
  border: 1px solid rgb(255 193 62 / 60%);
  border-radius: 50%;
  color: #fff8df;
  background: linear-gradient(145deg, #b87923, #e19a36);
  font-family: STKaiti, KaiTi, serif;
  font-size: 18px;
  font-weight: 800;
  box-shadow: 0 7px 16px rgb(184 121 35 / 24%);
}

.head-tail-calculation-dialog__eyebrow {
  color: #8a98a5;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.14em;
}

.head-tail-calculation-dialog h3 {
  margin: 8px 0 9px;
  color: #25384d;
  font-size: 23px;
  letter-spacing: 0.03em;
}

.head-tail-calculation-dialog p {
  margin: 0 0 22px;
  color: #718092;
  font-size: 13px;
  line-height: 1.7;
}

.head-tail-calculation-progress {
  position: relative;
  height: 9px;
  overflow: hidden;
  border-radius: 999px;
  background: #e8eef4;
}

.head-tail-calculation-progress__fill {
  position: relative;
  display: block;
  height: 100%;
  border-radius: inherit;
  background: linear-gradient(90deg, #d9972f, #ffdc70, #ed6747);
  transition: width 0.5s ease;
}

.head-tail-calculation-progress__fill::after {
  position: absolute;
  inset: 0;
  background: linear-gradient(
    105deg,
    transparent 20%,
    rgb(255 255 255 / 70%) 50%,
    transparent 80%
  );
  content: "";
  animation: head-tail-progress-shine 1.35s linear infinite;
}

.head-tail-calculation-dialog__meta {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  margin-top: 12px;
  color: #6f7d89;
  font-size: 12px;
}

.head-tail-calculation-dialog__meta strong {
  color: #c37a23;
  font-variant-numeric: tabular-nums;
}

.head-tail-calculation-dialog__hint {
  display: block;
  margin-top: 21px;
  color: #a4adb5;
  font-size: 11px;
}

@keyframes head-tail-progress-shine {
  from {
    transform: translateX(-100%);
  }

  to {
    transform: translateX(100%);
  }
}

@keyframes head-tail-sweep {
  0%,
  25% {
    left: -35%;
  }

  75%,
  100% {
    left: 130%;
  }
}

@media (max-width: 680px) {
  .head-tail-prototype__header,
  .head-tail-prototype__controls,
  .head-tail-cards__heading,
  .head-tail-ranking__heading {
    align-items: stretch;
    flex-direction: column;
  }

  .head-tail-prototype__snapshot {
    align-self: stretch;
  }

  .head-tail-prototype__control-copy {
    margin-right: 0;
  }

  .head-tail-prototype__calculation-action {
    align-items: stretch;
    flex-direction: column;
    width: 100%;
    margin-left: 0;
  }

  .head-tail-prototype__calculation-status,
  .head-tail-prototype__calculate-button {
    width: 100%;
    text-align: center;
  }

  .head-tail-card-grid {
    grid-template-columns: 1fr;
  }

  .head-tail-insight {
    align-items: flex-start;
    flex-wrap: wrap;
  }

  .head-tail-insight__tag {
    margin-left: 37px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .head-tail-card--legendary::after {
    animation: none;
  }

  .head-tail-calculation-progress__fill {
    transition: none;
  }

  .head-tail-calculation-progress__fill::after {
    animation: none;
  }
}
</style>
