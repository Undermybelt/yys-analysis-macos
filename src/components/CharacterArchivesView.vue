<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { desktopApi, type DesktopApi } from "../api/client";
import { normalizeCommandError } from "../api/errors";
import { resetActiveCharacter, syncActiveCharacter } from "../state/activeCharacter";
import { taskScheduler } from "../tasks/taskScheduler";
import { toLocalDateTime } from "../timeFormat";
import type { TaskProgress } from "../api/contracts";
import { clearAllMySoulsDerivedResults } from "./mySoulsDerivedStorage";
import PlatformBadge from "./PlatformBadge.vue";

// 首屏档案页只从 API 接口推导扫描结果，避免直接触发完整业务契约文件的开发期转换。
type CharacterArchiveScanResult = Awaited<
  ReturnType<DesktopApi["listCharacterArchives"]>
>;

// 浏览器静态预览没有 Tauri 事件桥，保持静默空态，避免 invoke 报错刷屏。
const isTauriRuntime = computed(
  () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window,
);

// 头像底色按账号 ID 稳定取色，同一角色每次进入页面颜色不变。
const AVATAR_PALETTE: readonly [
  string,
  string,
  string,
  string,
  string,
  string,
  string,
  string,
] = [
  "#8a4f3c",
  "#3f6d5c",
  "#5a6b9f",
  "#9a6b45",
  "#6f5b8a",
  "#a05a4e",
  "#4f7a8f",
  "#7c8a4f",
];

// 与 Rust 导入安全上限保持一致；先在文件选择层拦截旧版超大导出，避免 Array.from 制造数亿个 JS 数组元素。
const MAX_CHARACTER_ARCHIVE_IMPORT_BYTES = 64 * 1024 * 1024;

const scanResult = ref<CharacterArchiveScanResult | null>(null);
const loadError = ref("");
const loaded = ref(false);
const clearing = ref(false);
const importing = ref(false);
const importTaskId = ref<string | null>(null);
const importReadPercent = ref(0);
const importProgress = ref<TaskProgress | null>(null);
// 命令返回任务 ID 前可能已经收到首个事件，先暂存最后一条避免短任务的完成事件丢失。
const pendingImportProgress = ref<TaskProgress | null>(null);
const exportingProfileId = ref<string | null>(null);
const deletingIdentityKey = ref<string | null>(null);
const successMessage = ref("");
const importInput = ref<HTMLInputElement | null>(null);
let stopImportProgress: (() => void) | null = null;

const characters = computed(() => scanResult.value?.characters ?? []);
const currentCount = computed(
  () => characters.value.filter((character) => character.isCurrent).length,
);

function avatarColor(accountId: string): string {
  let hash = 0;
  for (const character of accountId) {
    hash = (hash * 31 + character.codePointAt(0)!) >>> 0;
  }
  return AVATAR_PALETTE[hash % AVATAR_PALETTE.length] ?? AVATAR_PALETTE[0];
}

function avatarInitial(displayName: string): string {
  return [...displayName.trim()][0] ?? "?";
}

function formatTime(value: string | null): string {
  // 后端存的是 UTC；这里转成本地时区（如北京时间 +8）再展示。
  return toLocalDateTime(value) ?? "未知";
}

function formatCount(value: number): string {
  return value.toLocaleString("zh-CN");
}

function openDesktopReading(): void {
  window.dispatchEvent(new Event("open-desktop-reading"));
}

/** 触发隐藏文件选择器；导入格式由后端再次校验，前端只限制常见的 JSON 扩展名。 */
function openImportPicker(): void {
  if (!isTauriRuntime.value || importing.value) return;
  importInput.value?.click();
}

// 将后端阶段映射到单调递增的页面进度：文件读取占前 25%，解析和规范化占后 75%。
const importProgressPercent = computed(() => {
  const progress = importProgress.value;
  if (!progress) return Math.round(importReadPercent.value * 0.25);
  const ratio =
    progress.total > 0
      ? Math.min(1, Math.max(0, progress.completed / progress.total))
      : 0;
  if (progress.phase === "preflight") return Math.round(25 + ratio * 15);
  if (progress.phase === "normalize") return Math.round(40 + ratio * 50);
  if (progress.phase === "complete") {
    return progress.status === "completed" ? 100 : 90;
  }
  return 25;
});

const importProgressMessage = computed(() => {
  if (importProgress.value?.message) return importProgress.value.message;
  if (importReadPercent.value < 100) {
    return `正在读取角色档案文件（${importReadPercent.value}%）…`;
  }
  return "正在准备导入任务，请稍候…";
});

const importProgressPhase = computed(() => {
  if (!importProgress.value) {
    return importReadPercent.value < 100 ? "读取文件" : "提交导入";
  }
  if (importProgress.value.phase === "preflight") return "校验数据结构";
  if (importProgress.value.phase === "normalize") return "处理御魂数据";
  if (importProgress.value.phase === "complete") return "完成写入";
  return "处理角色档案";
});

/** 使用 FileReader 的读取事件反馈大档案读取百分比，避免界面在准备字节数组时失去响应。 */
function readImportFile(file: File): Promise<number[]> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onprogress = (event) => {
      if (event.lengthComputable && event.total > 0) {
        importReadPercent.value = Math.min(
          100,
          Math.round((event.loaded / event.total) * 100),
        );
      }
    };
    reader.onerror = () => reject(reader.error ?? new Error("读取角色档案文件失败"));
    reader.onabort = () => reject(new Error("读取角色档案文件已取消"));
    reader.onload = () => {
      if (!(reader.result instanceof ArrayBuffer)) {
        reject(new Error("读取角色档案文件结果无效"));
        return;
      }
      importReadPercent.value = 100;
      resolve(Array.from(new Uint8Array(reader.result)));
    };
    reader.readAsArrayBuffer(file);
  });
}

/** 处理角色档案后台任务的真实进度，并在终态同步激活角色和档案卡片。 */
async function handleImportProgress(progress: TaskProgress): Promise<void> {
  if (!importTaskId.value && importing.value) {
    pendingImportProgress.value = progress;
    return;
  }
  if (progress.taskId !== importTaskId.value) return;
  importProgress.value = progress;
  if (progress.status === "running") return;

  importTaskId.value = null;
  importing.value = false;
  if (progress.status === "completed") {
    try {
      clearAllMySoulsDerivedResults();
      await syncActiveCharacter();
      await loadArchives();
      successMessage.value = progress.message || "角色档案导入完成。";
    } catch (error) {
      const commandError = normalizeCommandError(error);
      loadError.value = `导入完成但刷新页面失败：${commandError.message}`;
    }
  } else {
    const commandError = progress.error
      ? normalizeCommandError(progress.error)
      : null;
    loadError.value = `导入失败：${commandError?.message || progress.message}`;
  }
}

/** 导出卡片对应的数据档案，不切换当前角色，避免影响用户正在查看的其他页面。 */
async function exportCharacter(profileId: string | null): Promise<void> {
  if (!isTauriRuntime.value || !profileId || exportingProfileId.value) return;
  exportingProfileId.value = profileId;
  loadError.value = "";
  successMessage.value = "";
  try {
    const result = await desktopApi.exportCharacterArchive(profileId);
    successMessage.value = `“${result.characterName}”已导出。JSON：${result.jsonPath}；ZIP：${result.zipPath}`;
  } catch (error) {
    const commandError = normalizeCommandError(error);
    loadError.value = `导出失败：${commandError.message}`;
  } finally {
    exportingProfileId.value = null;
  }
}

/** 删除单个角色及其绑定数据；确认后复用后端级联清理，并同步全局激活角色状态。 */
async function deleteCharacter(
  identityKey: string,
  displayName: string,
): Promise<void> {
  if (
    !isTauriRuntime.value ||
    !identityKey ||
    clearing.value ||
    importing.value ||
    exportingProfileId.value ||
    deletingIdentityKey.value
  ) {
    return;
  }
  const confirmed = window.confirm(
    `删除“${displayName}”将同时删除该角色的数据档案、库存正文、式神镜像和分析数据，此操作不可恢复，是否继续？`,
  );
  if (!confirmed) return;
  deletingIdentityKey.value = identityKey;
  loadError.value = "";
  successMessage.value = "";
  try {
    cancelSoulCalculationTasks();
    await desktopApi.deleteCharacterArchive(identityKey);
    clearAllMySoulsDerivedResults();
    await syncActiveCharacter();
    await loadArchives();
    successMessage.value = `“${displayName}”已删除`;
  } catch (error) {
    const commandError = normalizeCommandError(error);
    loadError.value = `删除失败：${commandError.message}`;
  } finally {
    deletingIdentityKey.value = null;
  }
}

/** 读取用户选择的 JSON 字节并交给 Rust 导入事务，避免前端解析或重建角色数据。 */
async function importCharacter(event: Event): Promise<void> {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file || importing.value || !isTauriRuntime.value) return;
  if (file.size > MAX_CHARACTER_ARCHIVE_IMPORT_BYTES) {
    const sizeInMiB = Math.ceil(file.size / 1024 / 1024);
    loadError.value = `导入失败：文件约 ${sizeInMiB} MiB，超过 64 MiB 安全上限；请重新导出角色原始数据后再导入。`;
    successMessage.value = "";
    return;
  }
  importing.value = true;
  importTaskId.value = null;
  importReadPercent.value = 0;
  importProgress.value = null;
  pendingImportProgress.value = null;
  loadError.value = "";
  successMessage.value = "";
  try {
    // 覆盖角色库存前先取消仍引用旧数据的计算任务，避免旧结果在导入完成后回写缓存。
    cancelSoulCalculationTasks();
    const payload = await readImportFile(file);
    const accepted = await desktopApi.importCharacterArchive({
      fileName: file.name,
      payload,
    });
    // 命令只负责接收任务，真正完成后由统一进度事件触发档案刷新，避免页面过早显示成功。
    importTaskId.value = accepted.taskId;
    const pending = pendingImportProgress.value as TaskProgress | null;
    pendingImportProgress.value = null;
    if (pending && pending.taskId === accepted.taskId) {
      void handleImportProgress(pending);
    }
  } catch (error) {
    const commandError = normalizeCommandError(error);
    loadError.value = `导入失败：${commandError.message}`;
    importing.value = false;
  }
}

/** 删除角色前取消仍引用旧库存的计算任务，避免任务完成后又把旧结果写回本地缓存。 */
function cancelSoulCalculationTasks(): void {
  for (const kind of ["speed-calculation", "pve-calculation"] as const) {
    const task = taskScheduler.findRunning(kind);
    if (task) taskScheduler.cancel(task.id);
  }
}

async function loadArchives(): Promise<void> {
  if (!isTauriRuntime.value) {
    loaded.value = true;
    return;
  }
  try {
    scanResult.value = await desktopApi.listCharacterArchives();
  } catch (error) {
    loadError.value =
      typeof error === "object" && error !== null && "message" in error
        ? String(error.message)
        : "读取角色档案失败";
  } finally {
    loaded.value = true;
  }
}

/** 清空本机全部角色档案；级联删除每个角色的数据档案、库存正文与式神镜像，并清空激活角色。 */
async function clearArchives(): Promise<void> {
  if (clearing.value) return;
  const count = characters.value.length;
  if (count === 0) return;
  const confirmed = window.confirm(
    `清空角色档案将删除本机保存的 ${count} 个角色档案，以及每个角色绑定的库存正文、式神镜像和分析数据。此操作不可恢复，是否继续？`,
  );
  if (!confirmed) return;
  clearing.value = true;
  loadError.value = "";
  try {
    cancelSoulCalculationTasks();
    await desktopApi.clearCharacterArchives();
    // 角色档案删除后不再存在可恢复这些结果的库存，浏览器缓存也必须同步清空。
    clearAllMySoulsDerivedResults();
    resetActiveCharacter();
    await loadArchives();
  } catch (error) {
    loadError.value =
      typeof error === "object" && error !== null && "message" in error
        ? String(error.message)
        : "清空角色档案失败";
  } finally {
    clearing.value = false;
  }
}

onMounted(async () => {
  if (isTauriRuntime.value) {
    try {
      stopImportProgress = await desktopApi.onTaskProgress((progress) => {
        void handleImportProgress(progress);
      });
    } catch (error) {
      const commandError = normalizeCommandError(error);
      loadError.value = `导入进度通道连接失败：${commandError.message}`;
    }
  }
  await loadArchives();
});
onBeforeUnmount(() => {
  importInput.value = null;
  stopImportProgress?.();
  stopImportProgress = null;
});
</script>

<template>
  <main class="page-main archives-page" aria-labelledby="archives-title">
    <header class="archives-header">
      <div>
        <p class="eyebrow">WORKBENCH / CHARACTER ARCHIVES</p>
        <div class="archives-title-row">
          <h1 id="archives-title">角色档案</h1>
          <span v-if="currentCount > 0" class="archives-current-count"
            >{{ currentCount }} 个当前角色</span
          >
        </div>
      </div>
      <div class="archives-header__meta">
        <button
          class="archives-import"
          type="button"
          :disabled="
            clearing ||
            importing ||
            Boolean(exportingProfileId) ||
            Boolean(deletingIdentityKey) ||
            !isTauriRuntime
          "
          title="导入本工具导出的角色档案 JSON"
          @click="openImportPicker"
        >
          {{ importing ? "正在导入…" : "导入 JSON" }}
        </button>
        <input
          ref="importInput"
          class="archives-file-input"
          type="file"
          accept=".json,application/json"
          aria-label="选择角色档案 JSON 文件"
          @change="importCharacter"
        />
        <span class="archives-readonly"><i aria-hidden="true"></i>只读档案</span>
        <button
          v-if="characters.length > 0"
          class="archives-clear"
          type="button"
          :disabled="
            clearing ||
            importing ||
            Boolean(exportingProfileId) ||
            Boolean(deletingIdentityKey)
          "
          title="清空本机保存的全部角色档案摘要"
          @click="clearArchives"
        >
          {{ clearing ? "正在清空…" : "清空档案" }}
        </button>
      </div>
    </header>

    <section
      v-if="importing"
      class="archives-import-progress"
      role="status"
      aria-live="polite"
      aria-label="角色档案导入进度"
    >
      <div class="archives-import-progress__heading">
        <div>
          <span class="archives-import-progress__eyebrow"
            >IMPORT / CHARACTER ARCHIVE</span
          >
          <strong>正在导入角色档案</strong>
        </div>
        <b>{{ importProgressPercent }}%</b>
      </div>
      <p>{{ importProgressMessage }}</p>
      <div
        class="archives-import-progress__track"
        role="progressbar"
        :aria-valuenow="importProgressPercent"
        aria-valuemin="0"
        aria-valuemax="100"
        :aria-label="importProgressMessage"
      >
        <span :style="{ width: `${importProgressPercent}%` }"></span>
      </div>
      <div class="archives-import-progress__footer">
        <span>{{ importProgressPhase }}</span>
        <span>数据仅在本机处理</span>
      </div>
    </section>

    <p v-if="loadError" class="archives-load-error" role="alert">{{ loadError }}</p>
    <p v-if="successMessage" class="archives-success" role="status">
      {{ successMessage }}
    </p>

    <section
      v-if="characters.length > 0"
      class="archives-grid"
      aria-label="角色档案卡片"
    >
      <article
        v-for="character in characters"
        :key="character.identityKey"
        :class="[
          'archive-card',
          {
            'archive-card--current': character.isCurrent,
            'archive-card--unreadable': !character.readable,
          },
        ]"
      >
        <div class="archive-card__top">
          <div
            class="archive-card__avatar"
            :style="{ background: avatarColor(character.accountId) }"
            aria-hidden="true"
          >
            <span>{{ avatarInitial(character.displayName) }}</span>
          </div>
          <div class="archive-card__identity">
            <div class="archive-card__name-row">
              <h2>{{ character.displayName }}</h2>
              <span class="archive-card__badge archive-card__badge--source">{{
                character.sourceLabel
              }}</span>
              <span v-if="character.isCurrent" class="archive-card__badge">当前</span>
              <PlatformBadge
                v-if="character.platform"
                :platform="character.platform"
              />
            </div>
            <p class="archive-card__server">
              <span aria-hidden="true">◈</span>
              {{ character.serverLabel ?? "区服未知" }}
            </p>
          </div>
        </div>

        <dl class="archive-card__facts">
          <div>
            <dt>角色 ID</dt>
            <dd>{{ character.playerId ?? character.accountId }}</dd>
          </div>
          <div>
            <dt>最后更新</dt>
            <dd>{{ formatTime(character.lastUpdatedAt) }}</dd>
          </div>
          <div>
            <dt>六星御魂</dt>
            <dd>{{ formatCount(character.soulCount) }}</dd>
          </div>
          <div>
            <dt>式神</dt>
            <dd>{{ formatCount(character.shikigamiCount) }}</dd>
          </div>
        </dl>

        <p v-if="character.warning" class="archive-card__warning">
          <span aria-hidden="true">!</span>{{ character.warning }}
        </p>
        <p v-else class="archive-card__note">
          最近一次导入的信息；重新导入该角色会覆盖本卡内容。
        </p>
        <div class="archive-card__actions">
          <div class="archive-card__buttons">
            <button
              class="archive-card__export"
              type="button"
              :disabled="
                !character.profileId ||
                clearing ||
                Boolean(exportingProfileId) ||
                importing ||
                Boolean(deletingIdentityKey)
              "
              @click="exportCharacter(character.profileId)"
            >
              {{ exportingProfileId === character.profileId ? "导出中…" : "导出" }}
            </button>
            <button
              class="archive-card__delete"
              type="button"
              :disabled="
                clearing ||
                importing ||
                Boolean(exportingProfileId) ||
                Boolean(deletingIdentityKey)
              "
              title="删除该角色及其本机数据"
              @click="deleteCharacter(character.identityKey, character.displayName)"
            >
              {{ deletingIdentityKey === character.identityKey ? "删除中…" : "删除" }}
            </button>
          </div>
        </div>
      </article>
    </section>

    <section v-else-if="loaded" class="archives-empty" aria-label="暂无角色档案">
      <p class="archives-empty__mark" aria-hidden="true">零</p>
      <h2>{{ scanResult?.message ?? "还没有角色档案" }}</h2>
      <p>
        角色档案只在本机保存，数据来自导入。每次成功导入会新增或更新一个角色，
        重复导入同一角色会覆盖该角色的信息。
      </p>
      <button class="archives-empty__action" type="button" @click="openDesktopReading">
        前往藏宝阁导入
      </button>
    </section>

    <footer class="archives-footnote">
      <span
        ><b>读取边界</b>
        只展示角色身份与最后更新时间，不把御魂、式神正文或账号登录信息发送到任何远程服务；清空或删除档案会同步清理对应的本机数据。</span
      >
      <span class="archives-footnote__stamp">ARCHIVES / A-02</span>
    </footer>
  </main>
</template>

<style scoped>
/* 角色档案页使用“名册 + 印章红”作为专属视觉层，与数据读取入口保持同一工作台体系。 */
.archives-page {
  --archive-red: #a64b3f;
  --archive-indigo: #344b75;
  position: relative;
  max-width: 1480px;
  min-height: 100vh;
  padding: 38px clamp(24px, 4.3vw, 72px) 104px;
  background:
    radial-gradient(circle at 83% 3%, rgb(255 255 255 / 78%), transparent 28%),
    var(--fog);
}

.archives-header {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 28px;
}

.archives-title-row {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}

.archives-title-row h1 {
  margin: 7px 0 8px;
}

.archives-current-count {
  padding: 4px 9px;
  border: 1px solid rgb(166 75 63 / 42%);
  color: var(--archive-red);
  font-family: Consolas, monospace;
  font-size: 10px;
  letter-spacing: 0.06em;
}

.archives-header__meta {
  display: flex;
  align-items: center;
  gap: 12px;
  padding-bottom: 9px;
  color: var(--ink-soft);
  font-family: Consolas, monospace;
  font-size: 10px;
  letter-spacing: 0.07em;
}

.archives-file-input {
  display: none;
}

.archives-import,
.archives-clear {
  padding: 5px 12px;
  border: 1px solid rgb(52 75 117 / 42%);
  color: var(--archive-indigo);
  background: rgb(255 250 240 / 70%);
  cursor: pointer;
  font-size: 11px;
  letter-spacing: 0.06em;
  transition: background 0.15s ease;
}

.archives-import:hover:not(:disabled) {
  background: rgb(52 75 117 / 9%);
}

.archives-import:disabled {
  cursor: default;
  opacity: 0.55;
}

.archives-readonly {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  color: var(--jade);
}

.archives-readonly i {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: currentColor;
  box-shadow: 0 0 0 4px rgb(82 121 111 / 14%);
}

.archives-clear {
  border: 1px solid rgb(166 75 63 / 45%);
  color: var(--archive-red);
}

.archives-clear:hover:not(:disabled) {
  background: rgb(166 75 63 / 9%);
}

.archives-clear:disabled {
  cursor: default;
  opacity: 0.55;
}

.archives-load-error {
  margin: 0 0 14px;
  padding: 10px 14px;
  border: 1px solid rgb(200 80 70 / 45%);
  background: rgb(200 80 70 / 8%);
  color: var(--archive-red);
  font-size: 12px;
}

/* 导入进度采用档案页的印章红，阶段文案和百分比共同说明当前处理边界。 */
.archives-import-progress {
  margin: 22px 0 14px;
  padding: 15px 17px 13px;
  border: 1px solid rgb(166 75 63 / 34%);
  background: rgb(255 250 240 / 76%);
  box-shadow: 0 10px 26px rgb(73 55 39 / 6%);
}

.archives-import-progress__heading,
.archives-import-progress__footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}

.archives-import-progress__heading strong {
  display: block;
  margin-top: 5px;
  color: var(--ink);
  font-family: STKaiti, KaiTi, serif;
  font-size: 18px;
  font-weight: 600;
}

.archives-import-progress__heading b {
  color: var(--archive-red);
  font-family: Consolas, monospace;
  font-size: 18px;
  font-weight: 500;
}

.archives-import-progress__eyebrow,
.archives-import-progress__footer {
  color: var(--ink-soft);
  font-family: Consolas, monospace;
  font-size: 10px;
  letter-spacing: 0.07em;
}

.archives-import-progress__eyebrow {
  color: var(--archive-red);
}

.archives-import-progress p {
  margin: 12px 0 9px;
  color: var(--ink-soft);
  font-size: 12px;
  line-height: 1.6;
  overflow-wrap: anywhere;
}

.archives-import-progress__track {
  height: 7px;
  overflow: hidden;
  background: rgb(166 75 63 / 12%);
}

.archives-import-progress__track span {
  display: block;
  height: 100%;
  background: linear-gradient(90deg, var(--archive-red), #d18b64);
  transition: width 180ms ease;
}

.archives-import-progress__footer {
  margin-top: 8px;
  letter-spacing: 0.04em;
}

.archives-success {
  margin: 0 0 14px;
  padding: 10px 14px;
  border: 1px solid rgb(82 121 111 / 42%);
  background: rgb(82 121 111 / 8%);
  color: var(--jade);
  font-size: 12px;
  line-height: 1.6;
  overflow-wrap: anywhere;
}

/* 档案卡采用纵向名册布局：头像承担第一识别层，身份与事实各占一段。 */
.archives-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: 14px;
  margin-top: 34px;
}

.archive-card {
  position: relative;
  display: flex;
  min-height: 300px;
  flex-direction: column;
  padding: 20px 20px 16px;
  border: 1px solid var(--line);
  background: rgb(255 250 240 / 78%);
  box-shadow: 0 16px 40px rgb(73 55 39 / 7%);
  overflow: hidden;
}

.archive-card::before {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  height: 3px;
  background: linear-gradient(90deg, var(--archive-red), var(--line) 55%, transparent);
  content: "";
}

.archive-card--current::before {
  background: linear-gradient(
    90deg,
    var(--archive-indigo),
    var(--line) 55%,
    transparent
  );
}

.archive-card__top {
  display: flex;
  align-items: center;
  gap: 14px;
}

/* 游戏没有本地头像素材：先用昵称首字 + 账号稳定色生成印章式头像，补齐后替换为 img。 */
.archive-card__avatar {
  display: grid;
  width: 62px;
  height: 62px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 50%;
  color: #fffaf0;
  font-family: STKaiti, KaiTi, serif;
  font-size: 27px;
  box-shadow:
    inset 0 0 0 3px rgb(255 250 240 / 32%),
    0 8px 18px rgb(52 45 40 / 18%);
}

.archive-card__name-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.archive-card__name-row h2 {
  margin: 0;
  font-family: STKaiti, KaiTi, serif;
  font-size: 21px;
  letter-spacing: -0.02em;
}

.archive-card__badge {
  padding: 2px 7px;
  border: 1px solid rgb(52 75 117 / 45%);
  color: var(--archive-indigo);
  font-size: 10px;
  letter-spacing: 0.05em;
}

.archive-card__badge--source {
  border-color: rgb(83 123 111 / 42%);
  color: var(--jade);
}

.archive-card__server {
  margin: 6px 0 0;
  color: var(--ink-soft);
  font-size: 12px;
}

.archive-card__server span {
  margin-right: 5px;
  color: var(--archive-red);
  font-size: 9px;
}

.archive-card__facts {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 1px;
  margin: 18px 0 0;
  border: 1px solid var(--line-soft);
  background: var(--line-soft);
}

.archive-card__facts div {
  padding: 10px 12px;
  background: rgb(255 255 255 / 58%);
}

.archive-card__facts dt {
  color: var(--ink-soft);
  font-size: 10px;
  letter-spacing: 0.06em;
}

.archive-card__facts dd {
  margin: 4px 0 0;
  font-family: Consolas, monospace;
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.archive-card__warning,
.archive-card__note {
  display: flex;
  align-items: flex-start;
  gap: 7px;
  margin: auto 0 0;
  padding-top: 14px;
  color: var(--ink-soft);
  font-size: 11px;
  line-height: 1.6;
}

.archive-card__actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
  margin-top: 14px;
  padding-top: 12px;
  border-top: 1px dashed var(--line-soft);
  color: var(--ink-soft);
  font-size: 10px;
  letter-spacing: 0.03em;
}

.archive-card__buttons {
  display: flex;
  flex: 0 0 auto;
  gap: 8px;
}

.archive-card__export,
.archive-card__delete {
  flex: 0 0 auto;
  padding: 6px 15px;
  background: rgb(255 250 240 / 80%);
  cursor: pointer;
  font-size: 11px;
  letter-spacing: 0.08em;
  transition: background 0.15s ease;
}

.archive-card__export {
  border: 1px solid var(--archive-red);
  color: var(--archive-red);
}

.archive-card__export:hover:not(:disabled) {
  background: rgb(166 75 63 / 10%);
}

.archive-card__delete {
  border: 1px solid rgb(166 75 63 / 40%);
  color: var(--archive-red);
}

.archive-card__delete:hover:not(:disabled) {
  background: rgb(166 75 63 / 10%);
}

.archive-card__export:disabled,
.archive-card__delete:disabled {
  cursor: default;
  opacity: 0.55;
}

.archive-card__warning {
  color: var(--archive-red);
}

.archive-card__warning span {
  display: grid;
  width: 14px;
  height: 14px;
  flex: 0 0 auto;
  place-items: center;
  border: 1px solid currentColor;
  border-radius: 50%;
  font-size: 10px;
}

.archive-card--unreadable .archive-card__avatar {
  filter: grayscale(0.7);
  opacity: 0.75;
}

.archives-empty {
  max-width: 560px;
  margin: 40px auto 0;
  padding: 44px 34px 38px;
  border: 1px dashed var(--line);
  background: rgb(255 250 240 / 55%);
  text-align: center;
}

.archives-empty__mark {
  margin: 0 0 10px;
  color: var(--archive-red);
  font-family: STKaiti, KaiTi, serif;
  font-size: 44px;
  line-height: 1;
}

.archives-empty h2 {
  margin: 0;
  font-family: STKaiti, KaiTi, serif;
  font-size: 22px;
}

.archives-empty p:not(.archives-empty__mark) {
  margin: 12px auto 0;
  color: var(--ink-soft);
  font-size: 12px;
  line-height: 1.8;
}

.archives-empty__action {
  margin-top: 20px;
  padding: 9px 18px;
  border: 1px solid var(--archive-red);
  color: var(--archive-red);
  background: rgb(255 250 240 / 70%);
  cursor: pointer;
  font-size: 12px;
  transition: background 0.15s ease;
}

.archives-empty__action:hover {
  background: rgb(166 75 63 / 9%);
}

.archives-footnote {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
  margin-top: 26px;
  padding-top: 13px;
  border-top: 1px dashed var(--line);
  color: var(--ink-soft);
  font-size: 11px;
  line-height: 1.5;
}

.archives-footnote b {
  color: var(--ink);
}

.archives-footnote__stamp {
  flex: 0 0 auto;
  color: var(--archive-red);
  font-family: Consolas, monospace;
  font-size: 10px;
  letter-spacing: 0.1em;
}

@media (max-width: 720px) {
  .archives-page {
    padding: 27px 18px 96px;
  }

  .archives-header {
    align-items: flex-start;
    flex-direction: column;
  }

  .archives-header__meta {
    align-items: flex-start;
    flex-wrap: wrap;
    padding-bottom: 0;
  }

  .archives-grid {
    grid-template-columns: 1fr;
  }

  .archives-footnote {
    align-items: flex-start;
    flex-direction: column;
    gap: 8px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .archives-empty__action,
  .archives-import,
  .archives-clear,
  .archive-card__export,
  .archive-card__delete,
  .archives-import-progress__track span {
    transition: none;
  }
}
</style>
