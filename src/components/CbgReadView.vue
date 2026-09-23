<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import { normalizeCommandError } from "../api/errors";
import type { CbgReadPreview, CbgSoulPreview, TaskProgress } from "../api/contracts";
import { syncActiveCharacter } from "../state/activeCharacter";
import PlatformBadge from "./PlatformBadge.vue";

// 默认留空，由用户粘贴公开商品链接；进入页面后仍需主动点击预检，避免无感触发远程读取。
const sourceUrl = ref("");
const preview = ref<CbgReadPreview | null>(null);
const loading = ref(false);
const importing = ref(false);
const importCompleted = ref(false);
const error = ref("");
const message = ref("");
const taskId = ref<string | null>(null);
const taskProgress = ref<TaskProgress | null>(null);
let stopProgressListening: (() => void) | null = null;

const isTauriRuntime = computed(() =>
  Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__),
);
const hasPreview = computed(() => Boolean(preview.value));
const canInspect = computed(
  () => Boolean(sourceUrl.value.trim()) && !loading.value && !importing.value,
);
const canImport = computed(
  () =>
    Boolean(preview.value) &&
    !loading.value &&
    !importing.value &&
    !importCompleted.value,
);
const progressPercent = computed(() => {
  const progress = taskProgress.value;
  if (!progress) return 0;
  if (progress.total <= 0) return progress.status === "completed" ? 100 : 0;
  return Math.min(100, Math.round((progress.completed / progress.total) * 100));
});

// 浏览器静态预览保留一组真实接口返回中的字段形状，方便不开 Tauri 时先核对布局。
const staticPreview: CbgReadPreview = {
  sourceUrl: "https://yys.cbg.163.com/cgi/mweb/equip/3/…",
  accountName: "何漾（示例）",
  serverName: "网易-两情相悦",
  platform: null,
  inventoryCount: 9309,
  retainedCount: 9309,
  sixStarCount: 1,
  lowerStarCount: 9308,
  shikigamiCount: 355,
  sampleSouls: [
    {
      id: "6a1c0e41f9e6d8f96ddbf23b",
      setName: "招财猫",
      slot: 1,
      quality: 6,
      level: 15,
      mainAttribute: "攻击",
      mainValue: "486.00",
      subAttributes: ["生命 306.00", "速度 5.26", "暴击伤害 7.16%", "效果抵抗 7.18%"],
    },
  ],
  warnings: [
    "静态页面预览：以下数字来自本次链接测试的字段形状，不会写入本机库存。",
    "正式导入会把御魂写入库存、式神写入“式神录”；装备关系、货币和商品价格不会写入本地数据。",
    "藏宝阁没有逐条副属性强化次数，相关字段按未知处理。",
  ],
  message: "这是开发环境静态预览；在桌面应用中点击预检会读取当前链接的御魂与式神。",
};

/** 将任务事件限制在当前藏宝阁导入任务，避免其他后台任务污染本页进度。 */
function handleTaskProgress(progress: TaskProgress): void {
  if (progress.taskId !== taskId.value) return;
  taskProgress.value = progress;
  if (progress.status === "completed") {
    importing.value = false;
    importCompleted.value = true;
    message.value = progress.message;
    void syncActiveCharacter();
  } else if (progress.status === "failed" || progress.status === "cancelled") {
    importing.value = false;
    error.value =
      progress.error?.message ?? "藏宝阁导入没有完成，当前御魂与式神数据保持不变。";
  }
}

/** 在正式桌面应用中调用 Rust 预检；浏览器预览环境使用静态字段样例，不发起跨域请求。 */
async function inspectCbg(): Promise<void> {
  if (!canInspect.value) return;
  loading.value = true;
  error.value = "";
  message.value = "正在读取藏宝阁公开商品详情…";
  preview.value = null;
  importCompleted.value = false;
  taskProgress.value = null;
  try {
    if (!isTauriRuntime.value) {
      preview.value = { ...staticPreview, sourceUrl: sourceUrl.value.trim() };
      message.value = staticPreview.message;
      return;
    }
    preview.value = await desktopApi.inspectCbgRead({
      sourceUrl: sourceUrl.value.trim(),
    });
    message.value = preview.value.message;
  } catch (caught) {
    error.value = normalizeCommandError(caught).message || "无法读取藏宝阁公开商品。";
    message.value = "请确认链接仍然有效，且商品没有下架。";
  } finally {
    loading.value = false;
  }
}

/** 确认后重新抓取并交给现有覆盖事务；只有任务完成事件到达后才显示导入完成。 */
async function importCbg(): Promise<void> {
  if (!canImport.value) return;
  if (!isTauriRuntime.value) {
    message.value = "当前是浏览器静态预览；请在 Tauri 桌面应用中确认导入。";
    return;
  }
  importing.value = true;
  error.value = "";
  message.value = "已提交藏宝阁导入任务，御魂与式神数据将在任务成功后替换。";
  try {
    const accepted = await desktopApi.startCbgImportTask({
      sourceUrl: sourceUrl.value.trim(),
    });
    taskId.value = accepted.taskId;
  } catch (caught) {
    importing.value = false;
    error.value =
      normalizeCommandError(caught).message || "无法启动藏宝阁御魂与式神导入任务。";
  }
}

/** 重新输入链接后清理旧预检，避免用户把旧商品的统计误认为新链接结果。 */
function invalidatePreview(): void {
  preview.value = null;
  taskProgress.value = null;
  taskId.value = null;
  importCompleted.value = false;
  error.value = "";
  message.value = "链接已修改，请重新预检公开数据。";
}

/** 导入成功后导航到“我的御魂”，让用户立即看到来源徽标和新库存。 */
function openMySouls(): void {
  window.dispatchEvent(new Event("open-my-souls"));
}

/** 导入完成后导航到“式神录”，让用户立即核对藏宝阁公开的式神培养数据。 */
function openMyShikigami(): void {
  window.dispatchEvent(new Event("open-my-shikigami"));
}

/** 将当前读取/导入阶段压缩成检查台标题旁的状态标签。 */
function statusLabel(): string {
  if (importing.value) return "正在导入";
  if (importCompleted.value) return "导入完成";
  if (loading.value) return "正在预检";
  if (hasPreview.value) return "已完成预检";
  return "等待预检";
}

/** 返回预检样例，空预检时不渲染虚构数据。 */
function sampleSouls(): CbgSoulPreview[] {
  return preview.value?.sampleSouls ?? [];
}

onMounted(async () => {
  // 浏览器预览只展示静态样例；桌面端才订阅真实导入任务进度事件。
  if (isTauriRuntime.value) {
    stopProgressListening = await desktopApi.onTaskProgress(handleTaskProgress);
  } else {
    preview.value = staticPreview;
  }
});

onBeforeUnmount(() => {
  stopProgressListening?.();
});
</script>

<template>
  <main class="workspace cbg-read-page">
    <header class="page-header cbg-read-header">
      <div>
        <p class="eyebrow">DATA READING · INSPECTION DESK</p>
        <h1>藏宝阁导入</h1>
        <p class="lede">粘贴公开商品链接，先核对御魂与式神，再确认覆盖当前本地数据。</p>
      </div>
      <div class="cbg-read-header__meta">
        <span class="status-chip">{{ statusLabel() }}</span>
        <span class="cbg-read-header__mode">{{
          isTauriRuntime ? "桌面端读取" : "浏览器静态预览"
        }}</span>
      </div>
    </header>

    <section class="cbg-inspection-desk">
      <aside class="cbg-step-rail">
        <p class="section-kicker">READING DESK</p>
        <h2>先验证来源，再接管库存</h2>
        <ol>
          <li class="is-active">
            <b>01</b><span>商品链接<br /><small>确认公开详情地址</small></span>
          </li>
          <li :class="{ 'is-active': hasPreview }">
            <b>02</b><span>字段预览<br /><small>核对御魂属性映射</small></span>
          </li>
          <li :class="{ 'is-active': importCompleted }">
            <b>03</b><span>写入快照<br /><small>覆盖御魂与式神数据</small></span>
          </li>
        </ol>
        <div class="cbg-step-rail__note">
          公开式神会写入式神录；装备关系、货币和商品价格不会进入本地数据。
        </div>
      </aside>
      <div class="cbg-inspection-board">
        <article class="cbg-board-panel cbg-board-panel--url">
          <div class="cbg-board-panel__title">
            <span>商品地址</span><span>{{ statusLabel() }}</span>
          </div>
          <textarea v-model="sourceUrl" rows="4" @input="invalidatePreview"></textarea>
          <button
            class="button button--primary"
            type="button"
            :disabled="!canInspect"
            @click="inspectCbg"
          >
            {{ loading ? "读取中…" : "检查这条链接" }}
          </button>
        </article>
        <article class="cbg-board-panel cbg-board-panel--facts">
          <div class="cbg-board-panel__title">
            <span>公开字段报告</span
            ><span>{{ preview?.accountName || "尚未读取" }}</span>
          </div>
          <div v-if="preview" class="cbg-fact-lines">
            <div>
              <span>服务器</span><strong>{{ preview.serverName || "未知" }}</strong>
            </div>
            <div>
              <span>平台</span>
              <PlatformBadge
                v-if="preview.platform"
                :platform="preview.platform"
                compact
              />
              <strong v-else class="cbg-platform-unknown">未知</strong>
            </div>
            <div>
              <span>御魂库存</span
              ><strong>{{ preview.inventoryCount.toLocaleString() }} 条</strong>
            </div>
            <div>
              <span>导入范围</span
              ><strong
                >全量御魂 · {{ preview.sixStarCount.toLocaleString() }} 条六星</strong
              >
            </div>
            <div>
              <span>式神录</span
              ><strong>{{ preview.shikigamiCount.toLocaleString() }} 个</strong>
            </div>
          </div>
          <div v-else class="cbg-empty-state">
            <strong>报告尚未生成</strong><span>完成检查后再决定是否覆盖。</span>
          </div>
        </article>
        <article class="cbg-board-panel cbg-board-panel--table">
          <div class="cbg-board-panel__title">
            <span>字段映射样例</span><span>最多展示 5 条</span>
          </div>
          <div v-if="preview?.sampleSouls.length" class="cbg-field-table">
            <div
              v-for="soul in sampleSouls()"
              :key="soul.id"
              class="cbg-field-table__row"
            >
              <strong>{{ soul.setName }} {{ soul.slot }}号</strong
              ><span>{{ soul.mainAttribute }} {{ soul.mainValue }}</span
              ><small>{{ soul.subAttributes.join(" · ") }}</small>
            </div>
          </div>
          <div v-else class="cbg-empty-state">
            <strong>还没有样例</strong><span>先检查链接。</span>
          </div>
          <div class="cbg-board-panel__footer">
            <button
              class="button button--primary"
              type="button"
              :disabled="!canImport"
              @click="importCbg"
            >
              {{
                importing
                  ? "正在导入…"
                  : importCompleted
                    ? "导入完成"
                    : "确认写入御魂分析"
              }}
            </button>
            <button
              v-if="importCompleted"
              class="button button--quiet"
              type="button"
              @click="openMySouls"
            >
              打开御魂分析
            </button>
            <button
              v-if="importCompleted && preview?.shikigamiCount"
              class="button button--quiet"
              type="button"
              @click="openMyShikigami"
            >
              打开式神录
            </button>
          </div>
        </article>
      </div>
    </section>

    <section v-if="error" class="error-panel cbg-read-error" role="alert">
      <div>
        <p class="error-panel__title">藏宝阁读取未完成</p>
        <p>{{ error }}</p>
      </div>
      <button class="button button--quiet" type="button" @click="inspectCbg">
        重试预检
      </button>
    </section>
    <section v-if="message" class="cbg-read-message" role="status">
      <span>⌁</span>
      <p>{{ message }}</p>
    </section>
    <section v-if="preview?.warnings.length" class="cbg-read-warnings">
      <strong>导入边界</strong>
      <ul>
        <li v-for="warning in preview.warnings" :key="warning">{{ warning }}</li>
      </ul>
    </section>

    <!-- 导入期间锁住整页，明确提示正在写入御魂与式神录，避免用户重复提交或误改链接。 -->
    <div
      v-if="importing"
      class="cbg-import-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="cbg-import-overlay-title"
    >
      <div class="cbg-import-overlay__panel">
        <span class="cbg-import-overlay__mark" aria-hidden="true">03</span>
        <p class="section-kicker">WRITE TO MY DATA</p>
        <h2 id="cbg-import-overlay-title">正在写入御魂与式神录</h2>
        <p class="cbg-import-overlay__message">
          {{ taskProgress?.message ?? "正在准备导入任务，请稍候…" }}
        </p>
        <div
          class="cbg-import-overlay__progress"
          role="progressbar"
          aria-label="藏宝阁御魂与式神导入进度"
          aria-valuemin="0"
          aria-valuemax="100"
          :aria-valuenow="
            taskProgress && taskProgress.total > 0 ? progressPercent : undefined
          "
        >
          <span
            :class="{
              'is-indeterminate': !taskProgress || taskProgress.total <= 0,
            }"
            :style="{ width: `${progressPercent}%` }"
          ></span>
        </div>
        <div class="cbg-import-overlay__status">
          <span>旧御魂与式神数据将在任务成功后替换</span>
          <strong v-if="taskProgress && taskProgress.total > 0"
            >{{ progressPercent }}%</strong
          >
          <strong v-else>处理中</strong>
        </div>
      </div>
    </div>
  </main>
</template>

<style scoped>
/* 藏宝阁读取页面沿用平安志纸张色，但把“外部公开数据”做成一层可核对的冷色信息面。 */
.cbg-read-page {
  position: relative;
  min-height: 100vh;
  padding-bottom: 92px;
}

.cbg-read-header {
  align-items: flex-start;
}
.cbg-read-header__meta {
  display: grid;
  justify-items: end;
  gap: 8px;
}
.cbg-read-header__mode {
  color: var(--ink-soft);
  font-size: 11px;
}
.cbg-board-panel {
  border: 1px solid var(--line);
  background: rgb(255 250 240 / 86%);
  box-shadow: 0 16px 36px rgb(73 55 39 / 8%);
}
.cbg-board-panel textarea {
  width: 100%;
  resize: vertical;
  border: 1px solid var(--line);
  padding: 12px;
  color: var(--ink);
  background: #fffdf7;
  font:
    11px/1.6 Consolas,
    "Cascadia Mono",
    monospace;
}
.cbg-empty-state {
  display: grid;
  min-height: 132px;
  place-content: center;
  gap: 8px;
  color: var(--ink-soft);
  text-align: center;
}
.cbg-empty-state strong {
  color: var(--ink);
  font-size: 14px;
}
.cbg-empty-state span {
  font-size: 12px;
}
.cbg-inspection-desk {
  display: grid;
  grid-template-columns: 220px minmax(0, 1fr);
  gap: 18px;
}
.cbg-step-rail {
  padding: 24px 18px;
  color: var(--paper);
  background: var(--ink);
}
.cbg-step-rail h2 {
  margin: 8px 0 30px;
  font-family: STKaiti, KaiTi, serif;
  font-size: 25px;
  font-weight: 400;
  line-height: 1.35;
}
.cbg-step-rail ol {
  display: grid;
  gap: 24px;
  padding: 0;
  margin: 0;
  list-style: none;
}
.cbg-step-rail li {
  display: grid;
  grid-template-columns: 32px 1fr;
  gap: 8px;
  color: rgb(255 250 240 / 45%);
}
.cbg-step-rail li b {
  color: rgb(255 250 240 / 32%);
  font:
    12px Consolas,
    monospace;
}
.cbg-step-rail li.is-active {
  color: var(--paper);
}
.cbg-step-rail li.is-active b {
  color: #f49a92;
}
.cbg-step-rail li small {
  color: rgb(255 250 240 / 47%);
  font-size: 10px;
  line-height: 1.6;
}
.cbg-step-rail__note {
  margin-top: 58px;
  padding-top: 14px;
  border-top: 1px solid rgb(255 250 240 / 17%);
  color: rgb(255 250 240 / 58%);
  font-size: 11px;
  line-height: 1.7;
}
.cbg-inspection-board {
  display: grid;
  grid-template-columns: minmax(260px, 0.85fr) minmax(280px, 1.15fr);
  gap: 18px;
}
.cbg-board-panel {
  padding: 20px;
}
.cbg-board-panel--table {
  grid-column: 1 / -1;
}
.cbg-board-panel__title {
  display: flex;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 14px;
  color: var(--ink-soft);
  font-size: 11px;
  letter-spacing: 0.08em;
}
.cbg-board-panel__title span:last-child {
  color: var(--accent);
}
.cbg-board-panel .button {
  margin-top: 12px;
}
.cbg-fact-lines {
  display: grid;
  gap: 10px;
}
.cbg-fact-lines div {
  display: flex;
  justify-content: space-between;
  gap: 20px;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--line-soft);
  font-size: 12px;
}
.cbg-fact-lines > div > span {
  color: var(--ink-soft);
}
.cbg-platform-unknown {
  color: var(--ink-soft);
  font-weight: 600;
}
.cbg-field-table {
  display: grid;
  gap: 0;
  border-top: 1px solid var(--line);
}
.cbg-field-table__row {
  display: grid;
  grid-template-columns: 1.2fr 1fr 2fr;
  gap: 14px;
  padding: 12px 0;
  border-bottom: 1px solid var(--line-soft);
  font-size: 12px;
}
.cbg-field-table__row small {
  overflow: hidden;
  color: var(--ink-soft);
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cbg-board-panel__footer {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 16px;
}
.cbg-read-error,
.cbg-read-message,
.cbg-read-warnings {
  margin-top: 18px;
}
.cbg-read-message {
  display: flex;
  gap: 10px;
  padding: 13px 16px;
  border-left: 3px solid var(--jade);
  color: var(--ink-soft);
  background: rgb(83 123 111 / 8%);
}
.cbg-read-message p {
  margin: 0;
  font-size: 12px;
  line-height: 1.6;
}
.cbg-read-warnings {
  padding: 18px 20px;
  border: 1px solid var(--line-soft);
  color: var(--ink-soft);
  background: rgb(255 250 240 / 60%);
}
.cbg-read-warnings strong {
  color: var(--ink);
  font-size: 12px;
}
.cbg-read-warnings ul {
  display: grid;
  gap: 5px;
  padding-left: 18px;
  margin: 8px 0 0;
  font-size: 11px;
  line-height: 1.6;
}
.cbg-import-overlay {
  position: fixed;
  z-index: 100;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 24px;
  background: rgb(52 45 40 / 74%);
  backdrop-filter: blur(5px);
}
.cbg-import-overlay__panel {
  width: min(100%, 430px);
  padding: 32px;
  border: 1px solid #f49a92;
  color: var(--paper);
  background: var(--ink);
  box-shadow: 0 24px 80px rgb(0 0 0 / 32%);
}
.cbg-import-overlay__mark {
  display: grid;
  width: 42px;
  height: 42px;
  place-items: center;
  margin-bottom: 22px;
  border: 1px solid rgb(244 154 146 / 75%);
  color: #f49a92;
  font:
    12px Consolas,
    monospace;
}
.cbg-import-overlay__panel .section-kicker {
  color: #f49a92;
}
.cbg-import-overlay__panel h2 {
  margin: 8px 0 10px;
  color: var(--paper);
  font-family: STKaiti, KaiTi, serif;
  font-size: 28px;
  font-weight: 400;
}
.cbg-import-overlay__message {
  min-height: 38px;
  margin: 0;
  color: rgb(255 250 240 / 68%);
  font-size: 12px;
  line-height: 1.65;
}
.cbg-import-overlay__progress {
  position: relative;
  height: 8px;
  margin-top: 24px;
  overflow: hidden;
  background: rgb(255 250 240 / 16%);
}
.cbg-import-overlay__progress span {
  display: block;
  height: 100%;
  background: #f49a92;
  transition: width 180ms ease;
}
.cbg-import-overlay__progress span.is-indeterminate {
  width: 35% !important;
  animation: cbg-import-progress 1.35s ease-in-out infinite;
}
.cbg-import-overlay__status {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  margin-top: 10px;
  color: rgb(255 250 240 / 52%);
  font-size: 11px;
}
.cbg-import-overlay__status strong {
  color: #f49a92;
  font:
    700 12px Consolas,
    monospace;
}

@keyframes cbg-import-progress {
  0% {
    transform: translateX(-120%);
  }
  100% {
    transform: translateX(310%);
  }
}

@media (max-width: 900px) {
  .cbg-inspection-desk {
    grid-template-columns: 1fr;
  }
  .cbg-board-panel--table {
    grid-column: auto;
  }
  .cbg-step-rail {
    display: none;
  }
}

@media (max-width: 620px) {
  .cbg-read-header {
    align-items: flex-start;
    flex-direction: column;
  }
  .cbg-read-header__meta {
    justify-items: start;
  }
  .cbg-field-table__row {
    grid-template-columns: 1fr;
    gap: 4px;
  }
  .cbg-import-overlay__panel {
    padding: 24px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .cbg-import-overlay__progress span {
    transition: none;
  }
  .cbg-import-overlay__progress span.is-indeterminate {
    width: 100% !important;
    animation: none;
  }
}
</style>
