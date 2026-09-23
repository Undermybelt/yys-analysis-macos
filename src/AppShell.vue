<script setup lang="ts">
import { computed, defineAsyncComponent, onBeforeUnmount, onMounted, ref } from "vue";
import { desktopApi, type DesktopApi } from "./api/client";
import { taskScheduler } from "./tasks/taskScheduler";
import { syncActiveCharacter } from "./state/activeCharacter";
import { normalizeCommandError } from "./api/errors";
import type { DeferredPageKey } from "./pageLoaders";
import { APP_UPDATE_CHANNEL, IS_TEST_BUILD } from "./updateChannel";

// 启动更新状态直接从 API 接口推导，避免应用壳为几个字段加载完整的业务契约文件。
type UpdateCheckResult = Awaited<ReturnType<DesktopApi["checkUpdates"]>>;
type UpdateCandidateSummary = NonNullable<UpdateCheckResult["selected"]>;
type UpdateProgress = Parameters<Parameters<DesktopApi["onUpdateProgress"]>[0]>[0];

// 应用壳只保留默认页的直接边界；其他页面的 import 表放到二级懒加载模块中，避免首屏转换整个业务模块图。
function loadDeferredPage(key: DeferredPageKey) {
  return defineAsyncComponent(async () => {
    const { deferredPageLoaders } = await import("./pageLoaders");
    return deferredPageLoaders[key]();
  });
}

const AnalysisInboxView = loadDeferredPage("analysisInbox");
const AnalysisCenterView = loadDeferredPage("analysisCenter");
const SoulCatalogView = loadDeferredPage("soulCatalog");
const ShikigamiCatalogView = loadDeferredPage("shikigamiCatalog");
const MyShikigamiView = loadDeferredPage("myShikigami");
const ShikigamiShardQueryView = loadDeferredPage("shardQuery");
const GameAssetsView = loadDeferredPage("gameAssets");
const MySoulsView = loadDeferredPage("mySouls");
const RuleLibraryView = loadDeferredPage("ruleLibrary");
const CharacterArchivesView = defineAsyncComponent(
  () => import("./components/CharacterArchivesView.vue"),
);
const GuildView = loadDeferredPage("guild");
const CbgReadView = loadDeferredPage("cbgRead");
const UpdateView = loadDeferredPage("update");
const UpdatePrompt = defineAsyncComponent(
  () => import("./components/UpdatePrompt.vue"),
);
const FeedbackView = loadDeferredPage("feedback");
const SimulationView = loadDeferredPage("simulation");
const MiracleConchView = loadDeferredPage("miracleConch");
const SupportView = loadDeferredPage("support");
const TaskCenter = defineAsyncComponent(() => import("./components/TaskCenter.vue"));

// 一级导航：应用启动后默认进入“角色档案”，其他业务面板保留为应用壳入口。
const activeTab = ref<
  | "analysis"
  | "analysis-center"
  | "my-souls"
  | "soul-catalog"
  | "shikigami-catalog"
  | "my-shikigami"
  | "shard-query"
  | "game-assets"
  | "rules"
  | "character-archives"
  | "guild"
  | "cbg"
  | "help"
  | "test"
  | "feedback"
  | "simulation"
  | "miracle-conch"
  | "support"
>("character-archives");
// 我的御魂分组默认展开，保证模拟强化和神奇海螺子菜单可见。
const soulsExpanded = ref(true);
// 数据读取入口默认收起，减少首次进入工作台时的导航噪声。
const dataReadingExpanded = ref(false);
const analysisFilterCategory = ref("");
const analysisFilterSearch = ref("");
const analysisFilterUseId = ref("");
// 应用壳只维护一次桌面端任务事件监听，页面组件不再各自承担任务中心职责。
let stopTaskProgress: (() => void) | null = null;
let stopUpdateProgress: (() => void) | null = null;
let appUnmounted = false;
// 启动检查只允许执行一次，避免热切换页面或重复挂载触发多次下载。
const startupUpdate = ref<UpdateCheckResult | null>(null);
const startupUpdateError = ref("");
const startupUpdateProgress = ref<UpdateProgress | null>(null);
const isStartupUpdateInstalling = ref(false);
let startupUpdateCheckStarted = false;

// 页面区域统一使用当前组件，避免模板同时展开全部业务组件标签；切换菜单时只挂载当前入口对应的异步页面。
const activePageComponent = computed(() => {
  switch (activeTab.value) {
    case "analysis":
      return AnalysisInboxView;
    case "analysis-center":
      return AnalysisCenterView;
    case "soul-catalog":
      return SoulCatalogView;
    case "shikigami-catalog":
      return ShikigamiCatalogView;
    case "my-shikigami":
      return MyShikigamiView;
    case "shard-query":
      return ShikigamiShardQueryView;
    case "game-assets":
      return GameAssetsView;
    case "my-souls":
      return MySoulsView;
    case "simulation":
      return SimulationView;
    case "miracle-conch":
      return MiracleConchView;
    case "rules":
      return RuleLibraryView;
    case "character-archives":
      return CharacterArchivesView;
    case "guild":
      return GuildView;
    case "cbg":
      return CbgReadView;
    case "help":
    case "test":
      return UpdateView;
    case "feedback":
      return FeedbackView;
    case "support":
      return SupportView;
    default:
      return null;
  }
});

// 只有分析收件箱和测试更新页需要额外属性，其余页面保持原有无参数挂载行为。
const activePageProps = computed<Record<string, unknown>>(() => {
  if (activeTab.value === "analysis") {
    return {
      initialCategory: analysisFilterCategory.value,
      initialSearch: analysisFilterSearch.value,
      initialUseId: analysisFilterUseId.value,
    };
  }
  if (activeTab.value === "test") return { testOnly: true };
  return {};
});

/** 页面子组件通过事件请求导航，避免子组件直接依赖应用壳状态。 */
function openMySouls(): void {
  soulsExpanded.value = true;
  activeTab.value = "my-souls";
}

/** 藏宝阁和其他导入页完成后可直接打开玩家式神录，复用应用壳的一级导航。 */
function openMyShikigami(): void {
  activeTab.value = "my-shikigami";
}

/** 库存页和其他入口共用模拟强化导航事件；进入子页时保持我的御魂分组展开。 */
function openSimulation(): void {
  soulsExpanded.value = true;
  activeTab.value = "simulation";
}

/** 切换我的御魂分组；展开状态只影响导航，不改变当前页面。 */
function toggleSouls(): void {
  soulsExpanded.value = !soulsExpanded.value;
}

/** 切换数据读取分组；分组本身只负责展开渠道，不改变当前页面。 */
function toggleDataReading(): void {
  dataReadingExpanded.value = !dataReadingExpanded.value;
}

/**
 * 进入具体读取渠道时保持分组展开，确保当前入口始终可见。
 * macOS 版只有藏宝阁导入。
 */
function openDataReading(tab: "cbg"): void {
  dataReadingExpanded.value = true;
  activeTab.value = tab;
}

/** 空库存引导打开藏宝阁导入；macOS 版不读取阴阳师客户端。 */
function openDesktopReadingFromGuide(): void {
  openDataReading("cbg");
}

const isDataReadingActive = computed(() => activeTab.value === "cbg");

/** 分析中心图表通过浏览器事件把后端给出的分类传回待办页，保持下钻可逆。 */
function openAnalysisFilter(event: Event): void {
  const detail = (
    event as CustomEvent<{
      category?: string | null;
      search?: string;
      useId?: string;
    }>
  ).detail;
  if (!detail?.category && !detail?.search && !detail?.useId) return;
  analysisFilterCategory.value = detail.category ?? "";
  analysisFilterSearch.value = detail.search ?? "";
  analysisFilterUseId.value = detail.useId ?? "";
  activeTab.value = "analysis";
}

/** 应用启动后异步检查当前构建通道的应用包，不阻塞主页面和本地数据加载。 */
async function checkStartupUpdate(): Promise<void> {
  if (startupUpdateCheckStarted) return;
  startupUpdateCheckStarted = true;
  try {
    const result = await desktopApi.checkUpdates({
      sourceMode: "gitee",
      channel: APP_UPDATE_CHANNEL,
      packageType: "application",
      // 启动只读取清单并保存待下载记录，避免弹窗等待完整安装包下载。
      stagePayload: false,
    });
    if (result.selected) {
      startupUpdate.value = result;
    }
  } catch {
    // 启动检查失败不打扰用户；帮助页和测试菜单仍可显示主动检查错误。
  }
}

/** 用户在启动弹窗确认后自动安装应用包；安装助手启动后由后端退出并重启进程。 */
async function installStartupUpdate(): Promise<void> {
  const candidate: UpdateCandidateSummary | null =
    startupUpdate.value?.selected ?? null;
  if (!candidate || isStartupUpdateInstalling.value) return;
  isStartupUpdateInstalling.value = true;
  startupUpdateError.value = "";
  startupUpdateProgress.value = null;
  try {
    const result = await desktopApi.installUpdate({
      stagedUpdateId: candidate.stagedUpdateId,
      autoRestart: true,
    });
    if (result.state === "deferred") {
      startupUpdateError.value = result.message;
      return;
    }
    // restarting 状态通常会立即关闭当前窗口；保留状态可兼容安装器启动稍有延迟的机器。
    startupUpdate.value = null;
  } catch (error) {
    const normalized = normalizeCommandError(error);
    startupUpdateError.value = normalized.code + " · " + normalized.message;
  } finally {
    isStartupUpdateInstalling.value = false;
  }
}

/** 用户暂不更新时只关闭本次弹窗，下次启动仍会重新检查。 */
function dismissStartupUpdate(): void {
  if (!isStartupUpdateInstalling.value) {
    startupUpdate.value = null;
    startupUpdateError.value = "";
    startupUpdateProgress.value = null;
  }
}

onMounted(() => {
  window.addEventListener("open-analysis-filter", openAnalysisFilter);
  window.addEventListener("open-my-souls", openMySouls);
  window.addEventListener("open-my-shikigami", openMyShikigami);
  window.addEventListener("open-simulation", openSimulation);
  window.addEventListener("open-cbg-reading", openDesktopReadingFromGuide);
  window.addEventListener("open-desktop-reading", openDesktopReadingFromGuide);
  // 应用启动时恢复上次激活角色；各页面挂载后自行跟随当前激活状态。
  void syncActiveCharacter();
  // 注入后端取消适配器；调度器本身不依赖 Tauri，便于单元测试和未来替换执行端。
  taskScheduler.configureBackendCancellation((taskId) =>
    desktopApi.cancelBackgroundTask(taskId),
  );
  // Rust 任务事件由应用壳统一接收，保证切换页面后任务仍会进入左下角任务中心。
  void desktopApi
    .onTaskProgress((progress) => taskScheduler.adoptBackendProgress(progress))
    .then((unlisten) => {
      if (appUnmounted) {
        unlisten();
        return;
      }
      stopTaskProgress = unlisten;
    })
    .catch(() => {
      // 浏览器预览模式没有 Tauri 事件桥时保持静默，页面内的本地任务仍可正常运行。
    });
  // 更新下载进度只绑定到当前启动候选；应用重启前保留最后一个校验和安装状态。
  void desktopApi
    .onUpdateProgress((progress) => {
      const stagedUpdateId = startupUpdate.value?.selected?.stagedUpdateId;
      if (stagedUpdateId && progress.stagedUpdateId === stagedUpdateId) {
        startupUpdateProgress.value = progress;
      }
    })
    .then((unlisten) => {
      if (appUnmounted) {
        unlisten();
        return;
      }
      stopUpdateProgress = unlisten;
    })
    .catch(() => {
      // 浏览器预览模式没有 Tauri 更新事件桥，安装状态仍由命令结果反馈。
    });
  void checkStartupUpdate();
});

onBeforeUnmount(() => {
  appUnmounted = true;
  stopTaskProgress?.();
  stopTaskProgress = null;
  stopUpdateProgress?.();
  stopUpdateProgress = null;
  window.removeEventListener("open-analysis-filter", openAnalysisFilter);
  window.removeEventListener("open-my-souls", openMySouls);
  window.removeEventListener("open-my-shikigami", openMyShikigami);
  window.removeEventListener("open-simulation", openSimulation);
  window.removeEventListener("open-cbg-reading", openDesktopReadingFromGuide);
  window.removeEventListener("open-desktop-reading", openDesktopReadingFromGuide);
});
</script>

<template>
  <div class="app-shell">
    <aside class="side-rail" aria-label="应用导航">
      <div class="brand-lockup">
        <div class="brand-mark" aria-hidden="true">志</div>
        <div>
          <p class="brand-name">平安志</p>
          <p class="brand-caption">本地数据工作台</p>
        </div>
      </div>

      <nav class="nav-stack">
        <p class="nav-section-label nav-section-label--spaced">工作台</p>
        <div class="nav-group">
          <button
            class="nav-item nav-group__toggle"
            :class="{ 'nav-item--active': isDataReadingActive }"
            type="button"
            :aria-expanded="dataReadingExpanded"
            aria-controls="data-reading-submenu"
            @click="toggleDataReading"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M5 5h14v14H5zM8 12h8M12 8v8" />
            </svg>
            数据读取
            <span
              class="nav-group__chevron"
              :class="{ 'nav-group__chevron--expanded': dataReadingExpanded }"
              aria-hidden="true"
            >
              ⌄
            </span>
          </button>
          <div
            v-if="dataReadingExpanded"
            id="data-reading-submenu"
            class="nav-substack"
            aria-label="数据读取渠道"
          >
            <!-- 藏宝阁读取是用户粘贴公开商品链接的入口，仍遵循先预检后覆盖的只读边界。 -->
            <a
              class="nav-item nav-item--nested"
              :class="{ 'nav-item--active': activeTab === 'cbg' }"
              href="#cbg"
              :aria-current="activeTab === 'cbg' ? 'page' : undefined"
              @click="openDataReading('cbg')"
            >
              藏宝阁导入
            </a>
          </div>
        </div>
        <!-- 角色档案展示本机全部账号角色，属于工作台数据读取下方的独立一级入口。 -->
        <a
          class="nav-item"
          :class="{ 'nav-item--active': activeTab === 'character-archives' }"
          href="#character-archives"
          :aria-current="activeTab === 'character-archives' ? 'page' : undefined"
          @click="activeTab = 'character-archives'"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <circle cx="12" cy="8" r="3.2" />
            <path d="M5.5 19c.9-3.2 3.3-4.8 6.5-4.8s5.6 1.6 6.5 4.8" />
          </svg>
          角色档案
        </a>
        <!-- 我的御魂分组默认展开；模拟强化和神奇海螺属于御魂工作区子菜单。 -->
        <div class="nav-group">
          <button
            class="nav-item nav-group__toggle"
            :class="{
              'nav-item--active':
                activeTab === 'my-souls' ||
                activeTab === 'simulation' ||
                activeTab === 'miracle-conch',
            }"
            type="button"
            :aria-expanded="soulsExpanded"
            aria-controls="souls-submenu"
            @click="toggleSouls"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M12 3l7 4v8l-7 6-7-6V7l7-4zM8.5 9.5h7M8.5 13h5" />
            </svg>
            御魂分析
            <span
              class="nav-group__chevron"
              :class="{ 'nav-group__chevron--expanded': soulsExpanded }"
              aria-hidden="true"
            >
              ⌄
            </span>
          </button>
          <div
            v-if="soulsExpanded"
            id="souls-submenu"
            class="nav-substack"
            aria-label="御魂分析相关"
          >
            <a
              class="nav-item nav-item--nested"
              :class="{ 'nav-item--active': activeTab === 'my-souls' }"
              href="#my-souls"
              :aria-current="activeTab === 'my-souls' ? 'page' : undefined"
              @click="activeTab = 'my-souls'"
            >
              御魂分析
            </a>
            <a
              class="nav-item nav-item--nested"
              :class="{ 'nav-item--active': activeTab === 'simulation' }"
              href="#simulation"
              :aria-current="activeTab === 'simulation' ? 'page' : undefined"
              @click="activeTab = 'simulation'"
            >
              模拟强化
            </a>
            <a
              class="nav-item nav-item--nested"
              :class="{ 'nav-item--active': activeTab === 'miracle-conch' }"
              href="#miracle-conch"
              :aria-current="activeTab === 'miracle-conch' ? 'page' : undefined"
              @click="activeTab = 'miracle-conch'"
            >
              神奇海螺
            </a>
          </div>
        </div>
        <!-- 式神录只展示当前角色；跨角色能力由紧随其后的碎片查询独立承载。 -->
        <div class="nav-group">
          <a
            class="nav-item"
            :class="{ 'nav-item--active': activeTab === 'my-shikigami' }"
            href="#my-shikigami"
            :aria-current="activeTab === 'my-shikigami' ? 'page' : undefined"
            @click="activeTab = 'my-shikigami'"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M5 4h14v16H5zM8 8h8M8 12h5M8 16h6" />
            </svg>
            式神录
          </a>
        </div>
        <!-- 碎片查询横向读取全部游戏档案，紧跟当前角色式神录但保持独立数据范围。 -->
        <a
          class="nav-item"
          :class="{ 'nav-item--active': activeTab === 'shard-query' }"
          href="#shard-query"
          :aria-current="activeTab === 'shard-query' ? 'page' : undefined"
          @click="activeTab = 'shard-query'"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M6.5 5.5h11l-1 13h-9zM9 9h6M8.5 13h7" />
            <path d="M9 5.5c.4-1.7 1.4-2.5 3-2.5s2.6.8 3 2.5" />
          </svg>
          碎片查询
        </a>
        <a
          class="nav-item"
          :class="{ 'nav-item--active': activeTab === 'game-assets' }"
          href="#game-assets"
          :aria-current="activeTab === 'game-assets' ? 'page' : undefined"
          @click="activeTab = 'game-assets'"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M5 4h14v16H5zM8 8h8M8 12h5M8 16h6" />
          </svg>
          游戏资产
        </a>
        <!-- 寮管理展示当前导入快照中的阴阳寮成员名册，位于游戏资产下方的一级入口。 -->
        <a
          class="nav-item"
          :class="{ 'nav-item--active': activeTab === 'guild' }"
          href="#guild"
          :aria-current="activeTab === 'guild' ? 'page' : undefined"
          @click="activeTab = 'guild'"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 3l8 3.5v5c0 4.6-3.4 8-8 9.5-4.6-1.5-8-4.9-8-9.5v-5z" />
            <path d="M9.5 12.2l1.7 1.7 3.4-3.6" />
          </svg>
          寮管理
        </a>
        <div class="nav-group">
          <a
            class="nav-item"
            :class="{
              'nav-item--active':
                activeTab === 'help' ||
                activeTab === 'test' ||
                activeTab === 'feedback',
            }"
            href="#help"
            :aria-current="activeTab === 'help' ? 'page' : undefined"
            @click="activeTab = 'help'"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18Z" />
              <path
                d="M9.7 9a2.4 2.4 0 1 1 4.5 1.2c-.8 1-2.2 1.2-2.2 2.8M12 16.5h.01"
              />
            </svg>
            帮助
          </a>
          <!-- 问题反馈归入帮助分组，但保留独立页面，方便用户直达反馈流程。 -->
          <div class="nav-substack" aria-label="帮助相关">
            <a
              v-if="IS_TEST_BUILD"
              class="nav-item nav-item--nested"
              :class="{ 'nav-item--active': activeTab === 'test' }"
              href="#test"
              :aria-current="activeTab === 'test' ? 'page' : undefined"
              @click="activeTab = 'test'"
            >
              测试更新
              <span class="nav-item__status">test</span>
            </a>
            <a
              class="nav-item nav-item--nested"
              :class="{ 'nav-item--active': activeTab === 'feedback' }"
              href="#feedback"
              :aria-current="activeTab === 'feedback' ? 'page' : undefined"
              @click="activeTab = 'feedback'"
            >
              问题反馈
            </a>
          </div>
        </div>
        <p class="nav-section-label nav-section-label--spaced">基本数据</p>
        <!-- 基本数据入口保持同一级，目录、评分标准和未来资源页面可以直接切换。 -->
        <a
          class="nav-item"
          :class="{ 'nav-item--active': activeTab === 'soul-catalog' }"
          href="#soul-catalog"
          :aria-current="activeTab === 'soul-catalog' ? 'page' : undefined"
          @click="activeTab = 'soul-catalog'"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 3l7 4v8l-7 6-7-6V7l7-4zM8.5 9.5h7M8.5 13h5" />
          </svg>
          御魂数据
        </a>
        <a
          class="nav-item"
          :class="{ 'nav-item--active': activeTab === 'rules' }"
          href="#rules"
          :aria-current="activeTab === 'rules' ? 'page' : undefined"
          @click="activeTab = 'rules'"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M6 4h12v16H6zM9 8h6M9 12h6M9 16h4" />
          </svg>
          御魂评分标准
        </a>
        <!-- 式神目录是只读基础数据入口，页面内部负责版本校验与离线安装。 -->
        <a
          class="nav-item"
          :class="{ 'nav-item--active': activeTab === 'shikigami-catalog' }"
          href="#shikigami-catalog"
          :aria-current="activeTab === 'shikigami-catalog' ? 'page' : undefined"
          @click="activeTab = 'shikigami-catalog'"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path
              d="M12 3c4 0 7 2.7 7 6.2 0 4.8-4.2 8.8-7 11.8-2.8-3-7-7-7-11.8C5 5.7 8 3 12 3zM9 10.5c1.8 1.3 4.2 1.3 6 0M9.5 14h5"
            />
          </svg>
          式神图鉴
        </a>
        <span class="nav-item nav-item--disabled" aria-disabled="true">
          <span>库存资源</span>
          <span class="nav-item__status">待开发</span>
        </span>
        <!-- 支持入口保持为单一菜单，支持页内再展示收款码、捐赠人和反馈信息。 -->
        <a
          class="nav-item"
          :class="{ 'nav-item--active': activeTab === 'support' }"
          href="#support"
          :aria-current="activeTab === 'support' ? 'page' : undefined"
          @click="activeTab = 'support'"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path
              d="M12 20.2 4.8 13a4.8 4.8 0 0 1 6.8-6.8L12 6.6l.4-.4A4.8 4.8 0 0 1 19.2 13L12 20.2Z"
            />
            <path d="M8.8 11.5h6.4M12 8.3v6.4" />
          </svg>
          支持我
        </a>
      </nav>

      <div class="rail-note">
        <span class="rail-note__seal">只读</span>
        <p>数据、日志与任务均留在本机。</p>
      </div>
    </aside>

    <component
      :is="activePageComponent"
      v-if="activePageComponent"
      :key="activeTab"
      v-bind="activePageProps"
    />

    <footer class="page-footer">
      <span>所有路径由 Rust 核心解析</span>
      <span aria-hidden="true">·</span>
      <span>WebView 不直接访问文件系统</span>
    </footer>
  </div>
  <UpdatePrompt
    v-if="startupUpdate?.selected"
    :candidate="startupUpdate.selected"
    :current-version="startupUpdate.currentVersion"
    :channel="startupUpdate.channel"
    :installing="isStartupUpdateInstalling"
    :progress="startupUpdateProgress"
    :error-message="startupUpdateError"
    @update="installStartupUpdate"
    @dismiss="dismissStartupUpdate"
  />
  <TaskCenter />
</template>
