<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { desktopApi } from "../api/client";
import type {
  EmbryoPossibleAttributePath,
  EmbryoDecisionPath,
  EmbryoDecisionReport,
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

// 比例属性在后端按 0～1 保存；这里只把精确事实转换成易读的百分数。
const PERCENTAGE_ATTRIBUTE_TYPES = new Set([
  "attack_rate",
  "hp_rate",
  "defense_rate",
  "crit_rate",
  "crit_damage",
  "effect_hit",
  "effect_resist",
]);

const report = ref<EmbryoDecisionReport | null>(null);
// 默认只勾选四腿；三腿属于额外探索路径，必须由用户主动加入分析。
const includeThreeLeg = ref(false);
const includeFourLeg = ref(true);
// 计算必须由用户手动触发；进入页签和切换角色都不会偷偷扫描全量库存。
const loading = ref(false);
const error = ref("");
const expandedPathId = ref<string | null>(null);
let stopActiveCharacterChanged: (() => void) | null = null;
let requestId = 0;

const setById = computed(() => new Map(props.sets.map((set) => [set.setId, set])));

/** 手动读取当前档案勾选腿数的胚子决策；响应过期时不覆盖更新后的页面状态。 */
async function calculate(): Promise<void> {
  const currentRequestId = ++requestId;
  loading.value = true;
  error.value = "";
  try {
    const nextReport = await desktopApi.analyzeEmbryoDecision({
      includeThreeLeg: includeThreeLeg.value,
      includeFourLeg: includeFourLeg.value,
    });
    if (currentRequestId === requestId) {
      report.value = nextReport;
      expandedPathId.value = null;
    }
  } catch (caught) {
    if (currentRequestId === requestId) {
      const commandError = normalizeCommandError(caught);
      error.value = commandError.code + " · " + commandError.message;
    }
  } finally {
    if (currentRequestId === requestId) loading.value = false;
  }
}

/** 切换角色后清空旧报告，等待用户确认后重新计算，避免把上一角色结果误认为当前结果。 */
function clearReportForCharacterChange(): void {
  requestId += 1;
  report.value = null;
  expandedPathId.value = null;
  error.value = "";
  loading.value = false;
}

/** 目录未覆盖时回退稳定套装编号，保证结果仍然可读。 */
function setName(path: EmbryoDecisionPath): string {
  return setById.value.get(path.setId)?.name ?? path.setId;
}

/** 将稳定属性编号转换成页面短标签；未知属性保留原始编号，避免主属性显示内部编码。 */
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

/** 根据胚子路径中的套装编号返回离线图标；缺图时由模板回退到套装首字。 */
function soulIcon(path: EmbryoDecisionPath): string | null {
  const iconAssetId = setById.value.get(path.setId)?.iconAssetId;
  if (!iconAssetId) return null;
  return iconModules[`../assets/soul-icons/${iconAssetId}.png`] ?? null;
}

/** 仅用于展示的精确短文本；排序和结论已经由后端原始小数完成，统一固定四位小数。 */
function formatExact(value: number): string {
  return value.toFixed(4);
}

/** 按属性类型展示点数或百分数，避免把速度误显示为百分比。 */
function formatValue(value: number, attributeType: string): string {
  return PERCENTAGE_ATTRIBUTE_TYPES.has(attributeType)
    ? `${formatExact(value * 100)}%`
    : formatExact(value);
}

/** 将可选理论值转换为诚实的未知文案，避免把未知属性当成零值展示。 */
function formatOptionalValue(value: number | null, attributeType: string): string {
  return value === null ? "未知" : formatValue(value, attributeType);
}

/** 显示带正负号的超越差值；差值本身保持目标属性的原始单位。 */
function formatExcess(path: EmbryoDecisionPath, value: number | null): string {
  if (value === null) return "未知";
  const prefix = value > 0 ? "+" : "";
  return `${prefix}${formatValue(value, path.attributeType)}`;
}

/** 格式化新增属性分支的理论超越差值；空值表示没有可比较的成长基准。 */
function formatBranchExcess(branch: EmbryoPossibleAttributePath): string {
  if (branch.theoreticalExcess === null) return "未知";
  const prefix = branch.theoreticalExcess > 0 ? "+" : "";
  return `${prefix}${formatValue(branch.theoreticalExcess, branch.attributeType)}`;
}

/** 路径命中概率是五手全中目标属性的概率，接口按 0～1 保存，页面按百分比展示。 */
function formatProbability(value: number): string {
  return `${formatExact(value * 100)}%`;
}

/** 类别概率同样固定保留四位小数，避免页面不同卡片的百分比精度不一致。 */
function formatCategoryProbability(value: number): string {
  return `${formatExact(value * 100)}%`;
}

/** 将后端稳定结论转换成面向玩家的三档文案。 */
function conclusionLabel(conclusion: EmbryoDecisionPath["conclusion"]): string {
  if (conclusion === "worth-it") return "值得强化";
  if (conclusion === "high-risk") return "高风险可赌";
  return "停止强化";
}

/** 为三档结论提供稳定视觉层级，不改变后端排序。 */
function conclusionClass(conclusion: EmbryoDecisionPath["conclusion"]): string {
  if (conclusion === "worth-it") return "embryo-decision-conclusion--worth";
  if (conclusion === "high-risk") return "embryo-decision-conclusion--risk";
  return "embryo-decision-conclusion--stop";
}

/** 只展开当前路径的前三枚对照，避免长库存让页面一次性失去扫描节奏。 */
function toggleComparison(pathId: string): void {
  expandedPathId.value = expandedPathId.value === pathId ? null : pathId;
}

/** 同类对照来源使用中文解释，明确空库存时的平均期望门槛。 */
function comparisonSourceLabel(path: EmbryoDecisionPath): string {
  return path.comparisonSource === "inventory"
    ? `同类 +15 最高值（共 ${path.comparisonCandidateCount} 枚）`
    : "无同类库存，使用属性类别平均期望门槛";
}

/** 新增类别路径默认折叠，展开按钮只影响当前御魂的三类新增分支。 */
function isNewCategoryPath(path: EmbryoDecisionPath): boolean {
  return path.pathKind === "new-category";
}

/** 将分支结论转换为三档文案；具体属性基准不足时明确显示未知。 */
function branchConclusionLabel(
  conclusion: EmbryoPossibleAttributePath["conclusion"],
): string {
  if (conclusion === "worth-it") return "值得强化";
  if (conclusion === "high-risk") return "高风险可赌";
  if (conclusion === "stop") return "停止强化";
  return "数据不足";
}

/** 分支对照来源沿用四腿库存规则，并单独标记固定属性无法比较的情况。 */
function branchComparisonSourceLabel(branch: EmbryoPossibleAttributePath): string {
  if (branch.comparisonSource === "inventory") {
    return `同类 +15 最高值（共 ${branch.comparisonCandidateCount} 枚）`;
  }
  if (branch.comparisonSource === "average-threshold") {
    return "无同类库存，使用属性类别平均期望门槛";
  }
  return "没有可用成长基准";
}

/** 切换腿数后清空旧报告，避免用户看到与当前勾选条件不一致的路径。 */
function clearReportForLegSelectionChange(): void {
  requestId += 1;
  report.value = null;
  expandedPathId.value = null;
  error.value = "";
  loading.value = false;
}

onMounted(() => {
  stopActiveCharacterChanged = onActiveCharacterChanged(clearReportForCharacterChange);
});

watch([includeThreeLeg, includeFourLeg], clearReportForLegSelectionChange);

onBeforeUnmount(() => {
  stopActiveCharacterChanged?.();
});
</script>

<template>
  <section class="embryo-decision-panel">
    <header class="embryo-decision-panel__intro">
      <div>
        <p class="section-kicker">PLUS0 / EMBRYO DECISION</p>
        <h2>胚子强化决策</h2>
        <p>
          只分析勾选的六星 +0
          胚子。已有副属性独立计算理论上限、通常期望，三腿新增属性按类别路径展示，
          再与同套装、同号位、同主属性的 +15 御魂比较。
        </p>
      </div>
      <div class="embryo-decision-controls">
        <fieldset class="embryo-decision-leg-picker">
          <legend>分析腿数</legend>
          <label>
            <input v-model="includeThreeLeg" type="checkbox" />
            <span>三腿</span>
          </label>
          <label>
            <input v-model="includeFourLeg" type="checkbox" />
            <span>四腿</span>
          </label>
        </fieldset>
        <button
          class="button button--quiet"
          type="button"
          :disabled="loading || (!includeThreeLeg && !includeFourLeg)"
          @click="calculate"
        >
          {{ loading ? "正在计算…" : report ? "重新计算" : "计算所选胚子" }}
        </button>
      </div>
    </header>

    <section v-if="!report && !loading && !error" class="embryo-decision-start">
      <strong>准备好后手动开始</strong>
      <p>
        计算只读取当前档案的库存快照，不会强化、锁定或回收御魂；未确认库存不会被当作停止强化。
      </p>
      <button
        class="button"
        type="button"
        :disabled="!includeThreeLeg && !includeFourLeg"
        @click="calculate"
      >
        开始计算所选胚子
      </button>
    </section>

    <section v-if="error" class="error-panel" role="alert">
      <div>
        <p class="error-panel__title">胚子决策暂时无法读取</p>
        <p>{{ error }}</p>
      </div>
      <button class="button button--quiet" type="button" @click="calculate">
        重新计算
      </button>
    </section>

    <section v-if="loading" class="placeholder-card">正在读取六星 +0 四腿胚子…</section>

    <template v-else-if="report">
      <section class="embryo-decision-model" aria-label="胚子决策模型说明">
        <div class="embryo-decision-model__facts">
          <span
            >模型版本 <b>{{ report.modelVersion }}</b></span
          >
          <span
            >依据状态
            <b>{{
              report.evidenceStatus === "community-experience"
                ? "社区经验模型"
                : report.evidenceStatus
            }}</b></span
          >
          <span
            >剩余强化 <b>{{ report.remainingHandCount }} 手</b></span
          >
          <span
            >已选腿数 <b>{{ report.selectedLegCounts.join("、") }}腿</b></span
          >
          <span v-if="report.excludedUnconfirmedCount > 0"
            >待确认库存 <b>{{ report.excludedUnconfirmedCount }} 枚</b></span
          >
        </div>
        <p>{{ report.modelNote }}</p>
      </section>

      <section class="embryo-decision-summary" aria-label="胚子决策摘要">
        <div class="embryo-decision-summary__stat">
          <span>所选胚子</span>
          <strong>{{ report.candidateCount }}</strong>
          <small>六星 +0，未勾选腿数已排除</small>
        </div>
        <div class="embryo-decision-summary__stat embryo-decision-summary__stat--good">
          <span>独立路径</span>
          <strong>{{ report.pathCount }}</strong>
          <small>每条已有可强化副属性一条</small>
        </div>
        <div class="embryo-decision-summary__stat">
          <span>排序规则</span>
          <strong>3 档</strong>
          <small>结论优先，再按通常期望差值</small>
        </div>
      </section>

      <section v-if="!report.paths.length" class="empty-state-card">
        <strong>当前没有可分析的四腿路径</strong>
        <p>需要导入六星 +0、初始四腿，并且至少有一条可强化副属性。</p>
      </section>

      <section v-else class="embryo-decision-list" aria-label="胚子强化路径">
        <article
          v-for="path in report.paths"
          :key="path.pathId"
          class="embryo-decision-card"
        >
          <header class="embryo-decision-card__header">
            <div class="embryo-decision-card__identity">
              <span class="embryo-decision-card__icon" aria-hidden="true">
                <img v-if="soulIcon(path)" :src="soulIcon(path)!" alt="" />
                <span v-else>{{ setName(path).slice(0, 1) }}</span>
              </span>
              <div>
                <span class="embryo-decision-card__eyebrow">
                  {{ path.legCount }}腿 · {{ path.slot }}号位 · {{ path.quality }}星 ·
                  +{{ path.level }}
                </span>
                <h3>{{ setName(path) }} · {{ path.attributeLabel }}路径</h3>
                <small
                  >主属性：{{ attributeLabel(path.mainAttrType)
                  }}<template v-if="path.initialValue !== null">
                    · 初始值 {{ formatValue(path.initialValue, path.attributeType) }}
                  </template></small
                >
              </div>
            </div>
            <div
              class="embryo-decision-conclusion"
              :class="conclusionClass(path.conclusion)"
            >
              <span>强化结论</span>
              <strong>{{ conclusionLabel(path.conclusion) }}</strong>
            </div>
          </header>

          <div v-if="!isNewCategoryPath(path)" class="embryo-decision-metrics">
            <div>
              <span>理论上限</span>
              <strong>{{
                formatOptionalValue(path.theoreticalUpper, path.attributeType)
              }}</strong>
              <small>五手全中</small>
            </div>
            <div>
              <span>通常期望</span>
              <strong>{{
                formatOptionalValue(path.usualExpected, path.attributeType)
              }}</strong>
              <small>{{
                path.legCount === 3 ? "后四手均匀命中" : "五手均匀命中"
              }}</small>
            </div>
            <div>
              <span>路径命中概率</span>
              <strong>{{
                path.hitProbability === null
                  ? "未知"
                  : formatProbability(path.hitProbability)
              }}</strong>
              <small>{{
                path.legCount === 3 ? "后四手全中目标属性" : "五手全中目标属性"
              }}</small>
            </div>
            <div>
              <span>同类对照值</span>
              <strong>{{
                formatOptionalValue(path.comparisonValue, path.attributeType)
              }}</strong>
              <small>{{ comparisonSourceLabel(path) }}</small>
            </div>
          </div>

          <div v-else class="embryo-decision-category-summary">
            <div>
              <span>新增类别概率</span>
              <strong>{{
                path.categoryProbability === null
                  ? "未知"
                  : formatCategoryProbability(path.categoryProbability)
              }}</strong>
              <small>第一手抽中该类别</small>
            </div>
            <div>
              <span>具体属性概率</span>
              <strong>未声明</strong>
              <small>不把类别概率拆分</small>
            </div>
            <div>
              <span>理论上限 / 对照值</span>
              <strong>见展开分支</strong>
              <small>按具体属性分别比较</small>
            </div>
            <div>
              <span>通常期望</span>
              <strong>不计算</strong>
              <small>具体属性概率未知</small>
            </div>
          </div>

          <div v-if="!isNewCategoryPath(path)" class="embryo-decision-deltas">
            <span
              >理论超越差值
              <b>{{ formatExcess(path, path.theoreticalExcess) }}</b></span
            >
            <span
              >通常超越差值 <b>{{ formatExcess(path, path.usualExcess) }}</b></span
            >
          </div>

          <p v-if="isNewCategoryPath(path)" class="embryo-decision-data-note">
            {{ path.dataNote }}
          </p>

          <button
            v-if="path.comparisonItems.length || path.possibleAttributes.length"
            class="embryo-decision-comparison-toggle"
            type="button"
            :aria-expanded="expandedPathId === path.pathId"
            @click="toggleComparison(path.pathId)"
          >
            <template v-if="isNewCategoryPath(path)">
              {{ expandedPathId === path.pathId ? "收起" : "展开" }}全部
              {{ path.possibleAttributes.length }} 个具体属性分支
            </template>
            <template v-else>
              {{ expandedPathId === path.pathId ? "收起" : "展开" }}前
              {{ path.comparisonItems.length }} 枚同类对照
            </template>
          </button>
          <div v-if="expandedPathId === path.pathId" class="embryo-decision-comparison">
            <template v-if="isNewCategoryPath(path)">
              <article
                v-for="branch in path.possibleAttributes"
                :key="branch.attributeType"
                class="embryo-decision-branch"
              >
                <header>
                  <strong>{{ branch.attributeLabel }}</strong>
                  <span>{{ branchConclusionLabel(branch.conclusion) }}</span>
                </header>
                <div class="embryo-decision-branch__metrics">
                  <span
                    >理论上限
                    <b>{{
                      formatOptionalValue(branch.theoreticalUpper, branch.attributeType)
                    }}</b></span
                  >
                  <span>具体属性概率 <b>未声明</b></span>
                  <span
                    >对照值
                    <b>{{
                      formatOptionalValue(branch.comparisonValue, branch.attributeType)
                    }}</b>
                    <small>{{ branchComparisonSourceLabel(branch) }}</small></span
                  >
                  <span
                    >理论超越差值 <b>{{ formatBranchExcess(branch) }}</b></span
                  >
                </div>
                <p>{{ branch.dataNote }}</p>
                <div
                  v-if="branch.comparisonItems.length"
                  class="embryo-decision-comparison"
                >
                  <span v-for="item in branch.comparisonItems" :key="item.soulKey">
                    {{ formatValue(item.value, branch.attributeType) }}
                  </span>
                </div>
              </article>
            </template>
            <template v-else>
              <span v-for="item in path.comparisonItems" :key="item.soulKey">
                {{ formatValue(item.value, path.attributeType) }}
              </span>
            </template>
          </div>
        </article>
      </section>
    </template>
  </section>
</template>

<style scoped>
.embryo-decision-panel {
  display: grid;
  gap: 18px;
}

.embryo-decision-panel__intro {
  display: flex;
  align-items: end;
  justify-content: space-between;
  gap: 20px;
}

.embryo-decision-panel__intro h2 {
  margin: 4px 0 7px;
  color: var(--ink);
  font-size: clamp(22px, 3vw, 31px);
}

.embryo-decision-panel__intro p:not(.section-kicker) {
  max-width: 740px;
  margin: 0;
  color: var(--ink-soft);
  font-size: 12px;
  line-height: 1.6;
}

/* 腿数选择和计算按钮放在同一操作区；三腿默认关闭，减少首次查看的干扰。 */
.embryo-decision-controls {
  display: grid;
  justify-items: end;
  gap: 10px;
}

.embryo-decision-leg-picker {
  display: flex;
  gap: 12px;
  margin: 0;
  padding: 8px 10px;
  border: 1px solid var(--line);
  border-radius: 8px;
  color: var(--ink-soft);
  font-size: 11px;
}

.embryo-decision-leg-picker legend {
  padding: 0 4px;
  color: var(--ink);
  font-size: 10px;
  font-weight: 700;
}

.embryo-decision-leg-picker label {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  cursor: pointer;
}

.embryo-decision-start,
.embryo-decision-model,
.embryo-decision-card,
.embryo-decision-summary__stat {
  border: 1px solid var(--line);
  border-radius: 10px;
  background: rgb(255 255 255 / 76%);
  box-shadow: 0 10px 28px rgb(96 71 44 / 5%);
}

.embryo-decision-start {
  display: grid;
  justify-items: start;
  gap: 8px;
  padding: 22px;
}

.embryo-decision-start strong {
  color: var(--ink);
  font-size: 15px;
}

.embryo-decision-start p {
  max-width: 680px;
  margin: 0 0 4px;
  color: var(--ink-soft);
  font-size: 12px;
  line-height: 1.6;
}

.embryo-decision-model {
  display: grid;
  gap: 9px;
  padding: 14px 16px;
  color: var(--ink-soft);
  font-size: 11px;
  line-height: 1.5;
}

.embryo-decision-model p {
  margin: 0;
}

.embryo-decision-model__facts {
  display: flex;
  flex-wrap: wrap;
  gap: 8px 18px;
}

.embryo-decision-model__facts b {
  color: var(--ink);
}

.embryo-decision-summary {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 12px;
}

.embryo-decision-summary__stat {
  display: grid;
  gap: 4px;
  padding: 14px 16px;
}

.embryo-decision-summary__stat span,
.embryo-decision-summary__stat small {
  color: var(--ink-soft);
  font-size: 11px;
}

.embryo-decision-summary__stat strong {
  color: var(--ink);
  font-size: 24px;
  font-variant-numeric: tabular-nums;
}

.embryo-decision-summary__stat--good strong {
  color: var(--jade);
}

.embryo-decision-list {
  display: grid;
  gap: 12px;
}

.embryo-decision-card {
  overflow: hidden;
}

.embryo-decision-card__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 18px;
  padding: 14px 16px;
  background: linear-gradient(110deg, rgb(247 241 229 / 92%), rgb(255 255 255 / 62%));
}

.embryo-decision-card__identity {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 10px;
}

.embryo-decision-card__identity > div {
  min-width: 0;
}

.embryo-decision-card__icon {
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

.embryo-decision-card__icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.embryo-decision-card__eyebrow {
  color: var(--accent-deep);
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.05em;
}

.embryo-decision-card h3 {
  margin: 4px 0;
  color: var(--ink);
  font-size: 15px;
}

.embryo-decision-card__header small {
  color: var(--ink-soft);
  font-size: 10px;
}

.embryo-decision-conclusion {
  display: grid;
  min-width: 104px;
  gap: 3px;
  padding: 8px 12px;
  border-radius: 8px;
  text-align: center;
}

.embryo-decision-conclusion span {
  font-size: 10px;
}

.embryo-decision-conclusion strong {
  font-size: 14px;
}

.embryo-decision-conclusion--worth {
  color: var(--jade);
  background: rgb(83 123 111 / 10%);
}

.embryo-decision-conclusion--risk {
  color: #9b6617;
  background: rgb(216 154 43 / 12%);
}

.embryo-decision-conclusion--stop {
  color: var(--cinnabar);
  background: rgb(179 67 62 / 8%);
}

.embryo-decision-metrics {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 1px;
  background: var(--line-soft);
}

/* 新增类别父路径不伪造具体数值，先展示类别概率，再把数值核对放进折叠分支。 */
.embryo-decision-category-summary {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 1px;
  background: var(--line-soft);
}

.embryo-decision-category-summary > div {
  display: grid;
  gap: 4px;
  min-width: 0;
  padding: 13px 14px;
  background: rgb(255 255 255 / 88%);
}

.embryo-decision-category-summary span,
.embryo-decision-category-summary small {
  color: var(--ink-soft);
  font-size: 10px;
}

.embryo-decision-category-summary strong {
  color: var(--ink);
  font-size: 18px;
  font-variant-numeric: tabular-nums;
}

.embryo-decision-category-summary small {
  min-height: 24px;
  line-height: 1.35;
}

.embryo-decision-data-note,
.embryo-decision-branch p {
  margin: 0;
  padding: 10px 16px;
  color: var(--ink-soft);
  font-size: 11px;
  line-height: 1.5;
}

.embryo-decision-metrics > div {
  display: grid;
  gap: 4px;
  min-width: 0;
  padding: 13px 14px;
  background: rgb(255 255 255 / 88%);
}

.embryo-decision-metrics span,
.embryo-decision-metrics small {
  color: var(--ink-soft);
  font-size: 10px;
}

.embryo-decision-metrics strong {
  color: var(--ink);
  font-size: 18px;
  font-variant-numeric: tabular-nums;
}

.embryo-decision-metrics small {
  min-height: 24px;
  line-height: 1.35;
}

.embryo-decision-deltas {
  display: flex;
  flex-wrap: wrap;
  gap: 8px 22px;
  padding: 11px 16px;
  border-top: 1px solid var(--line-soft);
  color: var(--ink-soft);
  font-size: 11px;
}

.embryo-decision-deltas b {
  color: var(--ink);
  font-variant-numeric: tabular-nums;
}

.embryo-decision-comparison-toggle {
  padding: 10px 16px 12px;
  border: 0;
  border-top: 1px dashed var(--line);
  color: var(--accent-deep);
  background: transparent;
  cursor: pointer;
  font-size: 11px;
  text-align: left;
}

.embryo-decision-comparison-toggle:hover {
  color: var(--ink);
}

.embryo-decision-comparison {
  display: flex;
  flex-wrap: wrap;
  gap: 7px;
  padding: 0 16px 13px;
}

.embryo-decision-branch {
  flex: 1 1 100%;
  border-bottom: 1px dashed var(--line);
}

.embryo-decision-branch:last-child {
  border-bottom: 0;
}

.embryo-decision-branch header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 0 4px;
  color: var(--ink);
  font-size: 11px;
}

.embryo-decision-branch header span {
  color: var(--accent-deep);
  font-size: 10px;
}

.embryo-decision-branch__metrics {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 8px;
  color: var(--ink-soft);
  font-size: 10px;
}

.embryo-decision-branch__metrics span {
  display: grid;
  gap: 3px;
}

.embryo-decision-branch__metrics b {
  color: var(--ink);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}

.embryo-decision-branch__metrics small {
  color: var(--ink-soft);
  font-size: 9px;
}

.embryo-decision-branch > .embryo-decision-comparison {
  padding: 0 0 11px;
}

.embryo-decision-comparison span {
  padding: 4px 8px;
  border-radius: 999px;
  color: var(--ink-soft);
  background: rgb(247 241 229 / 90%);
  font-size: 10px;
  font-variant-numeric: tabular-nums;
}

@media (max-width: 800px) {
  .embryo-decision-metrics {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .embryo-decision-category-summary,
  .embryo-decision-branch__metrics {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@media (max-width: 700px) {
  .embryo-decision-panel__intro,
  .embryo-decision-card__header {
    align-items: stretch;
    flex-direction: column;
  }

  .embryo-decision-summary {
    grid-template-columns: 1fr;
  }

  .embryo-decision-controls {
    justify-items: stretch;
  }

  .embryo-decision-leg-picker {
    justify-content: space-between;
  }

  .embryo-decision-conclusion {
    align-self: start;
  }
}
</style>
