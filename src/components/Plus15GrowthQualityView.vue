<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import type {
  GrowthQualityAttribute,
  GrowthQualityReport,
  GrowthQualitySoul,
  SoulSet,
} from "../api/contracts";
import { normalizeCommandError } from "../api/errors";
import { onActiveCharacterChanged } from "../state/activeCharacter";

const props = defineProps<{
  sets: SoulSet[];
}>();

// 御魂图标随应用离线打包；卡片只通过目录提供的稳定图标编号查找资源，不发起网络请求。
const iconModules = import.meta.glob("../assets/soul-icons/*.png", {
  eager: true,
  import: "default",
  query: "?url",
});

// 比例属性在后端按 0～1 保存；页面只负责把精确事实转换成玩家熟悉的百分数。
const PERCENTAGE_ATTRIBUTE_TYPES = new Set([
  "attack_rate",
  "hp_rate",
  "defense_rate",
  "crit_rate",
  "crit_damage",
  "effect_hit",
  "effect_resist",
]);

const report = ref<GrowthQualityReport | null>(null);
// 页面状态只描述手动分析过程和数据不确定性；进入页面时不主动触发后端计算。
const loading = ref(false);
const error = ref("");
let stopActiveCharacterChanged: (() => void) | null = null;
let requestId = 0;

const setById = computed(() => new Map(props.sets.map((set) => [set.setId, set])));

/** 读取当前档案的成长质量报告；分析结果来自后端精确快照事实。 */
async function loadReport(): Promise<void> {
  const currentRequestId = ++requestId;
  loading.value = true;
  error.value = "";
  try {
    const nextReport = await desktopApi.analyzePlus15GrowthQuality();
    if (currentRequestId === requestId) report.value = nextReport;
  } catch (caught) {
    if (currentRequestId === requestId) {
      const commandError = normalizeCommandError(caught);
      error.value = commandError.code + " · " + commandError.message;
    }
  } finally {
    if (currentRequestId === requestId) loading.value = false;
  }
}

/** 将稳定属性编号转换成页面短标签；未知属性保留原始编号，避免丢失追溯线索。 */
function attributeLabel(attributeType: string): string {
  const labels: Record<string, string> = {
    attack_flat: "攻击",
    hp_flat: "生命",
    defense_flat: "防御",
    attack_rate: "攻击加成",
    hp_rate: "生命加成",
    defense_rate: "防御加成",
    speed: "速度",
    crit_rate: "暴击",
    crit_damage: "暴伤",
    effect_hit: "效果命中",
    effect_resist: "效果抵抗",
  };
  return labels[attributeType] ?? attributeType;
}

/** 同时显示属性原始值和便于阅读的单位值；成长质量计算仍使用接口传来的精确数值。 */
function formatAttributeValue(attribute: GrowthQualityAttribute): string {
  // 所有属性数值统一固定保留四位小数；比例属性先换算成百分比再格式化，避免长小数污染卡片。
  const displayValue = PERCENTAGE_ATTRIBUTE_TYPES.has(attribute.attributeType)
    ? attribute.value * 100
    : attribute.value;
  const suffix = PERCENTAGE_ATTRIBUTE_TYPES.has(attribute.attributeType) ? "%" : "";
  return `${displayValue.toFixed(4)}${suffix}`;
}

/** 将分数固定显示一位小数，保留排序使用的后端精确值。 */
function formatScore(score: number | null): string {
  return score === null ? "未知" : score.toFixed(1);
}

/** 统一显示新增强化手数和总实际手数，未知事实不填充默认值。 */
function formatHands(value: number | null): string {
  return value === null ? "未知" : `${value} 手`;
}

/** 获取目录中的套装名称；目录未覆盖时回退稳定编号。 */
function setName(soul: GrowthQualitySoul): string {
  return setById.value.get(soul.setId)?.name ?? soul.setId;
}

/** 根据成长质量卡片中的套装编号返回离线图标；缺图时由模板回退到套装首字。 */
function soulIcon(soul: GrowthQualitySoul): string | null {
  const iconAssetId = setById.value.get(soul.setId)?.iconAssetId;
  if (!iconAssetId) return null;
  return iconModules[`../assets/soul-icons/${iconAssetId}.png`] ?? null;
}

/** 将档位转换成稳定的视觉层级；档位文案本身仍由后端固定。 */
function tierClass(tier: string | null): string {
  if (tier === "极高") return "growth-quality-score--extreme";
  if (tier === "优秀") return "growth-quality-score--excellent";
  if (tier === "良好") return "growth-quality-score--good";
  if (tier === "普通") return "growth-quality-score--ordinary";
  return "growth-quality-score--low";
}

/** 将模型依据状态映射为用户可理解的短文案。 */
function evidenceLabel(status: string): string {
  return status === "community-experience" ? "社区经验模型" : status;
}

onMounted(() => {
  // 角色变化后清空旧档案报告，必须由用户点击按钮重新分析当前档案。
  stopActiveCharacterChanged = onActiveCharacterChanged(() => {
    report.value = null;
    error.value = "";
  });
});

onBeforeUnmount(() => {
  stopActiveCharacterChanged?.();
});
</script>

<template>
  <section class="growth-quality-panel">
    <header class="growth-quality-panel__intro">
      <div>
        <p class="section-kicker">PLUS15 / GROWTH QUALITY</p>
        <h2>成长质量</h2>
        <p>
          只评价六星 +15 御魂的可强化副属性成长，不混入用途价值、式神建议或标准评分。
        </p>
      </div>
      <button
        class="button button--quiet"
        type="button"
        :disabled="loading"
        @click="loadReport"
      >
        {{ loading ? "正在分析…" : report ? "重新分析" : "开始分析" }}
      </button>
    </header>

    <section v-if="report" class="growth-quality-model" aria-label="成长质量模型说明">
      <div class="growth-quality-model__facts">
        <span
          >模型版本 <b>{{ report.modelVersion }}</b></span
        >
        <span
          >依据状态 <b>{{ evidenceLabel(report.evidenceStatus) }}</b></span
        >
        <span
          >计算方式 <b>初始 1 手 + 5 次强化，共 {{ report.handCount }} 手</b></span
        >
        <span v-if="report.excludedUnconfirmedCount > 0"
          >待确认库存 <b>{{ report.excludedUnconfirmedCount }} 枚</b></span
        >
      </div>
      <p>{{ report.modelNote }}</p>
      <div class="growth-quality-tier-legend" aria-label="成长质量档位">
        <span v-for="boundary in report.tierBoundaries" :key="boundary.label">
          {{ boundary.label }} {{ boundary.minScore
          }}<template v-if="boundary.maxScore !== null"
            >–{{ boundary.maxScore }}</template
          ><template v-else>+</template>
        </span>
      </div>
    </section>

    <section v-if="error" class="error-panel" role="alert">
      <div>
        <p class="error-panel__title">成长质量暂时无法读取</p>
        <p>{{ error }}</p>
      </div>
      <button class="button button--quiet" type="button" @click="loadReport">
        重新读取
      </button>
    </section>

    <section v-if="loading" class="placeholder-card">正在读取六星 +15 御魂…</section>

    <section
      v-else-if="!report && !error"
      class="empty-state-card growth-quality-start"
    >
      <strong>尚未开始分析</strong>
      <p>点击右上角“开始分析”，读取当前档案并计算六星 +15 御魂的成长质量。</p>
    </section>

    <template v-else-if="report">
      <section class="growth-quality-summary" aria-label="成长质量摘要">
        <div class="growth-quality-summary__stat">
          <span>候选成品</span>
          <strong>{{ report.candidateCount }}</strong>
          <small>六星 +15 御魂</small>
        </div>
        <div class="growth-quality-summary__stat growth-quality-summary__stat--good">
          <span>可比较</span>
          <strong>{{ report.scoreableCount }}</strong>
          <small>强化手数事实完整</small>
        </div>
        <div
          class="growth-quality-summary__stat"
          :class="{
            'growth-quality-summary__stat--warning': report.unknownDataCount > 0,
          }"
        >
          <span>待补事实</span>
          <strong>{{ report.unknownDataCount }}</strong>
          <small>不伪造总成长分</small>
        </div>
      </section>

      <section v-if="!report.items.length" class="empty-state-card">
        <strong>当前没有六星 +15 御魂</strong>
      <p>导入库存后，这里会按总成长分从高到低展示成长质量。</p>
      </section>

      <section v-else class="growth-quality-list" aria-label="成长质量排序">
        <article
          v-for="soul in report.items"
          :key="soul.soulKey"
          class="growth-quality-card"
        >
          <header class="growth-quality-card__header">
            <div class="growth-quality-card__identity">
              <span class="growth-quality-card__icon" aria-hidden="true">
                <img v-if="soulIcon(soul)" :src="soulIcon(soul)!" alt="" />
                <span v-else>{{ setName(soul).slice(0, 1) }}</span>
              </span>
              <div>
                <span class="growth-quality-card__eyebrow"
                  >{{ soul.slot }}号位 · {{ soul.quality }}星 · +{{ soul.level }}</span
                >
                <h3>{{ setName(soul) }}</h3>
                <small>主属性：{{ attributeLabel(soul.mainAttrType) }}</small>
              </div>
            </div>
            <div class="growth-quality-score" :class="tierClass(soul.tier)">
              <span>总成长分</span>
              <strong>{{ formatScore(soul.totalGrowthScore) }}</strong>
              <b>{{ soul.tier ?? "数据不足" }}</b>
            </div>
          </header>

          <div class="growth-quality-table-wrap">
            <table class="growth-quality-table">
              <thead>
                <tr>
                  <th>可强化副属性</th>
                  <th>原始值</th>
                  <th>强化手数</th>
                  <th>单手成长质量</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="attribute in soul.attributes" :key="attribute.attributeType">
                  <th>
                    {{
                      attribute.attributeLabel ||
                      attributeLabel(attribute.attributeType)
                    }}
                  </th>
                  <td>{{ formatAttributeValue(attribute) }}</td>
                  <td>
                    <span>新增 {{ formatHands(attribute.enhancementCount) }}</span>
                    <small>实际 {{ formatHands(attribute.handCount) }}</small>
                  </td>
                  <td>
                    {{
                      attribute.singleRollQuality === null
                        ? "未知"
                        : `${attribute.singleRollQuality.toFixed(1)} 分`
                    }}
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
          <p v-if="soul.scoreNote" class="growth-quality-card__note">
            {{ soul.scoreNote }}
          </p>
        </article>
      </section>
    </template>
  </section>
</template>

<style scoped>
.growth-quality-panel {
  display: grid;
  gap: 18px;
}

.growth-quality-panel__intro {
  display: flex;
  align-items: end;
  justify-content: space-between;
  gap: 20px;
}

.growth-quality-panel__intro h2 {
  margin: 4px 0 7px;
  color: var(--ink);
  font-size: clamp(22px, 3vw, 31px);
}

.growth-quality-panel__intro p:not(.section-kicker) {
  max-width: 690px;
  margin: 0;
  color: var(--ink-soft);
  font-size: 12px;
  line-height: 1.6;
}

.growth-quality-model,
.growth-quality-card,
.growth-quality-summary__stat {
  border: 1px solid var(--line);
  border-radius: 10px;
  background: rgb(255 255 255 / 76%);
  box-shadow: 0 10px 28px rgb(96 71 44 / 5%);
}

.growth-quality-model {
  display: grid;
  gap: 9px;
  padding: 14px 16px;
  color: var(--ink-soft);
  font-size: 11px;
  line-height: 1.5;
}

.growth-quality-model p {
  margin: 0;
}

.growth-quality-model__facts,
.growth-quality-tier-legend {
  display: flex;
  flex-wrap: wrap;
  gap: 8px 18px;
}

.growth-quality-model__facts b {
  color: var(--ink);
}

.growth-quality-tier-legend span {
  padding: 3px 8px;
  border-radius: 999px;
  color: var(--accent-deep);
  background: rgb(174 118 83 / 9%);
  font-variant-numeric: tabular-nums;
}

.growth-quality-summary {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 12px;
}

.growth-quality-summary__stat {
  display: grid;
  gap: 4px;
  padding: 14px 16px;
}

.growth-quality-summary__stat span,
.growth-quality-summary__stat small {
  color: var(--ink-soft);
  font-size: 11px;
}

.growth-quality-summary__stat strong {
  color: var(--ink);
  font-size: 24px;
  font-variant-numeric: tabular-nums;
}

.growth-quality-summary__stat--good strong {
  color: var(--jade);
}

.growth-quality-summary__stat--warning strong {
  color: var(--cinnabar);
}

/* 首次进入时用“+15”印章强调分析入口，让空状态既能说明现状也能提示下一步动作。 */
.growth-quality-start {
  position: relative;
  min-height: 86px;
  padding: 22px 24px 22px 78px;
  border-color: rgb(174 118 83 / 34%);
  background: linear-gradient(115deg, rgb(255 250 240 / 92%), rgb(247 241 229 / 70%));
  box-shadow: 0 12px 28px rgb(96 71 44 / 7%);
  text-align: left;
}

.growth-quality-start::before {
  position: absolute;
  top: 22px;
  left: 24px;
  display: grid;
  width: 38px;
  height: 38px;
  place-items: center;
  border: 1px solid rgb(174 118 83 / 48%);
  border-radius: 50%;
  color: var(--cinnabar);
  background: rgb(255 239 198 / 64%);
  content: "+15";
  font-family: STKaiti, KaiTi, serif;
  font-size: 15px;
  transform: rotate(-7deg);
}

.growth-quality-start strong {
  color: var(--ink);
  font-size: 15px;
}

.growth-quality-start p {
  max-width: 680px;
  margin: 0;
  color: var(--ink-soft);
  font-size: 12px;
  line-height: 1.6;
}

.growth-quality-list {
  display: grid;
  gap: 12px;
}

.growth-quality-card {
  overflow: hidden;
}

.growth-quality-card__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 18px;
  padding: 14px 16px;
  background: linear-gradient(110deg, rgb(247 241 229 / 92%), rgb(255 255 255 / 62%));
}

.growth-quality-card__identity {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 10px;
}

.growth-quality-card__identity > div {
  min-width: 0;
}

.growth-quality-card__icon {
  display: grid;
  width: 42px;
  height: 42px;
  flex: 0 0 42px;
  place-items: center;
  overflow: hidden;
  border: 1px solid rgb(174 118 83 / 22%);
  border-radius: 10px;
  background: var(--paper);
  color: var(--accent-deep);
  font-family: STKaiti, KaiTi, serif;
  font-size: 18px;
}

.growth-quality-card__icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.growth-quality-card__eyebrow {
  color: var(--accent-deep);
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.05em;
}

.growth-quality-card h3 {
  margin: 4px 0;
  color: var(--ink);
  font-size: 15px;
}

.growth-quality-card__header small {
  color: var(--ink-soft);
  font-size: 10px;
}

.growth-quality-score {
  display: grid;
  min-width: 98px;
  gap: 2px;
  padding: 8px 12px;
  border-radius: 8px;
  text-align: center;
}

.growth-quality-score span,
.growth-quality-score b {
  font-size: 10px;
}

.growth-quality-score strong {
  font-size: 23px;
  font-variant-numeric: tabular-nums;
}

.growth-quality-score--low {
  color: var(--cinnabar);
  background: rgb(179 67 62 / 8%);
}

.growth-quality-score--ordinary {
  color: #9b6617;
  background: rgb(216 154 43 / 12%);
}

.growth-quality-score--good {
  color: var(--jade);
  background: rgb(83 123 111 / 10%);
}

.growth-quality-score--excellent {
  color: #9b6417;
  background: rgb(255 222 143 / 33%);
}

.growth-quality-score--extreme {
  color: #8f4ba9;
  background: linear-gradient(
    125deg,
    rgb(214 241 237 / 72%),
    rgb(255 232 166 / 72%),
    rgb(244 197 219 / 72%)
  );
}

.growth-quality-table-wrap {
  overflow-x: auto;
}

.growth-quality-table {
  width: 100%;
  color: var(--ink-soft);
  font-size: 11px;
}

.growth-quality-table th,
.growth-quality-table td {
  padding: 10px 14px;
  border-top: 1px solid var(--line-soft);
  text-align: left;
  white-space: nowrap;
}

.growth-quality-table thead th {
  color: var(--ink-soft);
  background: rgb(247 241 229 / 58%);
  font-size: 10px;
}

.growth-quality-table tbody th {
  color: var(--ink);
}

.growth-quality-table td {
  font-variant-numeric: tabular-nums;
}

.growth-quality-table td span,
.growth-quality-table td small {
  display: block;
}

.growth-quality-table td small {
  margin-top: 3px;
  color: var(--ink-faint);
  font-size: 10px;
}

.growth-quality-card__note {
  margin: 0;
  padding: 10px 16px 13px;
  border-top: 1px dashed var(--line);
  color: var(--cinnabar);
  font-size: 11px;
}

@media (max-width: 700px) {
  .growth-quality-panel__intro,
  .growth-quality-card__header {
    align-items: stretch;
    flex-direction: column;
  }

  .growth-quality-summary {
    grid-template-columns: 1fr;
  }

  .growth-quality-start {
    padding: 22px 18px 22px 70px;
  }

  .growth-quality-start::before {
    left: 18px;
  }

  .growth-quality-score {
    align-self: start;
  }
}
</style>
