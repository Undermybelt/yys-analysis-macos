<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import type { MySoul, SoulSet } from "../api/contracts";
import {
  calculateSpeedPreview as calculateSpeedPreviewEngine,
  type SpeedCalculationResult,
  type SpeedSection,
  type SpeedTargetSet,
} from "../speedSoulCalculationEngine";
import {
  MY_SOULS_INVENTORY_CLEARED_EVENT,
  speedPreviewStorageKey,
} from "./mySoulsDerivedStorage";
import { activeCharacterId, onActiveCharacterChanged } from "../state/activeCharacter";
import {
  taskScheduler,
  type ScheduledTask,
  type TaskExecutionContext,
} from "../tasks/taskScheduler";

// 一速表格复用应用内置的离线御魂图标；目录缺图时保留纯文字，不影响排行阅读。
const iconModules = import.meta.glob("../assets/soul-icons/*.png", {
  eager: true,
  import: "default",
  query: "?url",
});

const props = defineProps<{
  /** 父级已经读取的御魂目录；图标通过稳定套装 ID 与排行结果关联。 */
  sets: SoulSet[];
}>();

// 分页读取仍沿用后端单页上限；排行计算已移到独立引擎，便于主线程和 Worker 共用。
const INVENTORY_PAGE_SIZE = 100;
// 后端等级筛选使用游戏数据中的 +15 数值；开启“只计算满级”时直接下推此条件。
const MAX_SOUL_LEVEL = 15;
// 首次只展示十条套装排行；点击“加载更多”后每次继续展开十条，计算结果本身仍保留全量。
const DEFAULT_VISIBLE_SET_COUNT = 10;
const SET_SECTION_TITLE = "套装一速";

// 一速目标列表只接收有标准四件套效果的普通御魂；首领/星痕不会成为目标，但仍保留在两件自由位候选池中。
const speedSetConfigs = computed<SpeedTargetSet[]>(() =>
  props.sets
    .filter((set) => !set.specialCategory && Boolean(set.fourPieceEffect))
    .map((set) => ({ setId: set.setId, label: set.name })),
);

// 缓存保存表格结果和套装详情所需的六枚御魂，不保存未入选的整份库存；打开页面无需再次扫描库存。
type SpeedCalculationSnapshot = {
  profileId: string;
  /** 计算时使用的普通四件套目录 ID；目录变化后旧缓存必须重新计算。 */
  targetSetIds: string[];
  soulKeys: string[];
  calculatedAt: string;
  onlyMaxLevel: boolean;
  soulCount: number;
  qualifiedSoulCount: number;
  sections: SpeedSection[];
};

type SpeedSoulDetail = {
  /** 当前详情对应的套装排行名称，用于弹层标题和无障碍标识。 */
  title: string;
  /** 当前详情的规则说明，明确四件套与两件自由位口径。 */
  subtitle: string;
  /** 当前排行实际选择的六个号位御魂。 */
  selected: MySoul[];
};

const souls = ref<MySoul[]>([]);
const calculation = ref<SpeedCalculationSnapshot | null>(null);
const calculating = ref(false);
const restoredFromCache = ref(false);
const error = ref("");
// 满级是默认口径；取消后会把六星未满级御魂也纳入一速候选。
const onlyMaxLevel = ref(true);
const calculationMode = ref<"foreground" | "background" | null>(null);
const calculationNotice = ref("");
// 套装排行默认只展开十条；展开数量只影响显示，不会改变已缓存的全量排序结果。
const visibleSetCount = ref(DEFAULT_VISIBLE_SET_COUNT);
// 计算期间持续反馈分页读取进度；总数在第一次后端响应后才确定。
const progressLoaded = ref(0);
const progressTotal = ref(0);
const calculationStage = ref<"reading" | "ranking" | "background">("reading");
// 当前展开的一速套装组合详情；关闭后不保留上一次组合，避免切换角色时误读旧数据。
const soulDetail = ref<SpeedSoulDetail | null>(null);
// 详情按 PVE 面板习惯排列，便于用户逐个核对六个号位。
const detailSlots = [1, 6, 2, 5, 3, 4] as const;
// Worker 由统一任务调度器持有；页面销毁只解除订阅，不会中断后台计算。
let speedCalculationWorker: Worker | null = null;
let rejectBackgroundCalculation: ((error: Error) => void) | null = null;
let noticeTimer: number | null = null;
let activeSpeedTaskId: string | null = null;
let stopTaskListening: (() => void) | null = null;
let stopActiveCharacterChanged: (() => void) | null = null;

// 用稳定套装 ID 建立图标编号索引，目录更新后由 Vue 自动刷新表格图标。
// “招财”是排行中的短名，目录稳定 ID 为“招财猫”，这里同时登记目录名称和显示名称。
const setIconAssetIds = computed(() => {
  const iconIds = new Map<string, number | null>();
  for (const set of props.sets) {
    iconIds.set(set.setId, set.iconAssetId);
    iconIds.set(set.name, set.iconAssetId);
  }
  if (!iconIds.has("招财") && iconIds.has("招财猫")) {
    iconIds.set("招财", iconIds.get("招财猫") ?? null);
  }
  return iconIds;
});

/** 根据排行行中的套装 ID 返回离线图标 URL；缺少编号或资源时返回空值。 */
function soulIcon(setId: string): string | null {
  const iconAssetId = setIconAssetIds.value.get(setId);
  if (!iconAssetId) return null;
  return iconModules[`../assets/soul-icons/${iconAssetId}.png`] ?? null;
}

// 速度阈值只作用于各位置的速度副属性；一速总值是 100 以上的组合结果，不参与 17/17.5 判断。
function speedTone(value: string): "normal" | "hot" | "rainbow" {
  const numericValue = Number.parseFloat(value);
  if (numericValue > 17.5) return "rainbow";
  if (numericValue > 17) return "hot";
  return "normal";
}

/** 按当前筛选口径分页读取库存；只计算满级时由数据库直接过滤 level=15，避免读取未满级正文。 */
async function loadAllSouls(
  context: TaskExecutionContext,
  selectedOnlyMaxLevel: boolean,
): Promise<void> {
  error.value = "";
  try {
    const loaded: MySoul[] = [];
    let offset = 0;
    let total = Number.POSITIVE_INFINITY;
    progressLoaded.value = 0;
    progressTotal.value = 0;

    while (loaded.length < total) {
      if (context.isCancelled()) return;
      const page = await desktopApi.listMySouls(
        selectedOnlyMaxLevel
          ? { offset, limit: INVENTORY_PAGE_SIZE, level: MAX_SOUL_LEVEL }
          : { offset, limit: INVENTORY_PAGE_SIZE },
      );
      total = page.total;
      progressTotal.value = page.total;
      loaded.push(...page.items);
      progressLoaded.value = Math.min(loaded.length, page.total);
      if (context.isCancelled()) return;
      context.report({
        phase: "读取库存",
        completed: loaded.length,
        total: page.total,
        message: `正在读取${selectedOnlyMaxLevel ? " +15" : "全部等级"}御魂… ${loaded.length} / ${page.total}`,
      });
      if (!page.items.length) break;
      offset += page.items.length;
    }
    souls.value = loaded;
  } catch (caught) {
    error.value = caught instanceof Error ? caught.message : "御魂库存读取失败";
    throw caught instanceof Error ? caught : new Error(error.value);
  }
}

/** 从本地缓存恢复最近一次计算结果；缓存损坏时静默清理，页面仍可手动重新计算。 */
function restoreCalculation(): void {
  const profileId = activeCharacterId.value;
  const storageKey = speedPreviewStorageKey(profileId);
  calculation.value = null;
  visibleSetCount.value = DEFAULT_VISIBLE_SET_COUNT;
  restoredFromCache.value = false;
  if (!storageKey || !profileId) return;
  try {
    const raw = localStorage.getItem(storageKey);
    if (!raw) return;
    const parsed = JSON.parse(raw) as SpeedCalculationSnapshot;
    if (
      !parsed ||
      parsed.profileId !== profileId ||
      !Array.isArray(parsed.targetSetIds) ||
      !parsed.targetSetIds.every((setId) => typeof setId === "string") ||
      !Array.isArray(parsed.soulKeys) ||
      typeof parsed.calculatedAt !== "string" ||
      typeof parsed.onlyMaxLevel !== "boolean" ||
      !Array.isArray(parsed.sections) ||
      typeof parsed.soulCount !== "number" ||
      typeof parsed.qualifiedSoulCount !== "number"
    ) {
      localStorage.removeItem(storageKey);
      return;
    }
    // 旧版本没有目录 ID；目录新增/删除四件套后清除旧缓存，避免排行数量与详情入口不一致。
    const currentSetIds = speedSetConfigs.value.map((set) => set.setId);
    const hasCurrentTargetSets =
      parsed.targetSetIds.length === currentSetIds.length &&
      currentSetIds.every((setId) => parsed.targetSetIds.includes(setId));
    const setSection = parsed.sections.find(
      (section) => section.title === SET_SECTION_TITLE,
    );
    // 旧版本只给套装行保存详情；现在所有速度行都有详情入口，缺少任一行时要求重新计算。
    const hasRowDetails = parsed.sections.every((section) =>
      section.rows.every((row) => row.speed === "—" || Array.isArray(row.selected)),
    );
    if (!hasCurrentTargetSets || !setSection || !hasRowDetails) {
      localStorage.removeItem(storageKey);
      return;
    }
    calculation.value = parsed;
    onlyMaxLevel.value = parsed.onlyMaxLevel;
    restoredFromCache.value = true;
  } catch {
    localStorage.removeItem(storageKey);
  }
}

/** 将本次完整扫描得到的排行榜和筛选口径写入本地，后续只恢复轻量快照。 */
function saveCalculation(
  result: SpeedCalculationResult,
  selectedOnlyMaxLevel: boolean,
  profileId: string,
): void {
  const storageKey = speedPreviewStorageKey(profileId);
  if (!storageKey) return;
  const snapshot: SpeedCalculationSnapshot = {
    profileId,
    targetSetIds: speedSetConfigs.value.map((set) => set.setId),
    soulKeys: result.soulKeys,
    calculatedAt: new Date().toISOString(),
    onlyMaxLevel: selectedOnlyMaxLevel,
    soulCount: result.soulCount,
    qualifiedSoulCount: result.qualifiedSoulCount,
    sections: result.sections,
  };
  calculation.value = snapshot;
  restoredFromCache.value = false;
  localStorage.setItem(storageKey, JSON.stringify(snapshot));
}

/** 展示短暂完成通知；同时保留 aria-live，确保键盘和辅助技术用户也能收到结果。 */
function showCompletionNotice(message: string): void {
  calculationNotice.value = message;
  if (noticeTimer !== null) window.clearTimeout(noticeTimer);
  noticeTimer = window.setTimeout(() => {
    calculationNotice.value = "";
    noticeTimer = null;
  }, 6000);
}

/** 在 Worker 中运行纯排行计算；读取库存仍由页面负责，完成后结果回到页面统一缓存。 */
function runBackgroundCalculation(
  soulList: MySoul[],
  selectedOnlyMaxLevel: boolean,
  context: TaskExecutionContext,
  targetSets: readonly SpeedTargetSet[],
): Promise<SpeedCalculationResult> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(
      new URL("../workers/speedSoulCalculation.worker.ts", import.meta.url),
      { type: "module" },
    );
    let settled = false;

    const cleanup = (): void => {
      worker.terminate();
      if (speedCalculationWorker === worker) speedCalculationWorker = null;
      if (rejectBackgroundCalculation) rejectBackgroundCalculation = null;
    };
    const finishResolve = (result: SpeedCalculationResult): void => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(result);
    };
    const finishReject = (caught: Error): void => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(caught);
    };

    rejectBackgroundCalculation = () => finishReject(new Error("一速后台计算已停止"));
    worker.onmessage = (event: MessageEvent<unknown>): void => {
      const response = event.data as
        | { type: "completed"; result: SpeedCalculationResult }
        | { type: "failed"; message: string };
      if (response.type === "completed") {
        finishResolve(response.result);
      } else {
        finishReject(new Error(response.message));
      }
    };
    worker.onerror = (event): void => {
      finishReject(new Error(event.message || "一速后台计算线程异常结束"));
    };
    speedCalculationWorker = worker;
    context.report({ phase: "整理排行", message: "已开始后台整理一速排行…" });
    // Vue 的 ref 数组和其中的御魂对象可能是 Proxy，不能直接参与结构化克隆；逐层展开成纯对象后再跨线程传递。
    const transferableSouls = soulList.map((soul) => ({
      ...soul,
      attributes: soul.attributes.map((attribute) => ({ ...attribute })),
    }));
    worker.postMessage({
      souls: transferableSouls,
      onlyMaxLevel: selectedOnlyMaxLevel,
      targetSets: targetSets.map((set) => ({ ...set })),
    });
  });
}

/** 只终止当前实例注册的 Worker；任务状态由调度器统一收敛。 */
function stopSpeedWorker(): void {
  const cancel = rejectBackgroundCalculation;
  rejectBackgroundCalculation = null;
  cancel?.(new Error("一速后台计算已停止"));
  speedCalculationWorker?.terminate();
  speedCalculationWorker = null;
}

/** 重新进入一速页面时把调度器中的运行任务映射回页面状态。 */
function handleSpeedTaskSnapshot(tasks: ScheduledTask[]): void {
  const running = tasks.find(
    (task) => task.kind === "speed-calculation" && task.status === "running",
  );
  if (!activeSpeedTaskId && running) {
    activeSpeedTaskId = running.id;
    if (typeof running.metadata.onlyMaxLevel === "boolean") {
      onlyMaxLevel.value = running.metadata.onlyMaxLevel;
    }
  }
  if (!activeSpeedTaskId) return;
  const current = tasks.find((task) => task.id === activeSpeedTaskId);
  if (!current) return;
  progressLoaded.value = current.completed;
  progressTotal.value = current.total;
  if (current.message)
    calculationStage.value = current.phase === "整理排行" ? "background" : "reading";
  if (current.status === "running") {
    calculating.value = true;
    calculationMode.value =
      current.metadata.mode === "background" ? "background" : "foreground";
    return;
  }
  calculating.value = false;
  calculationMode.value = null;
  if (current.status === "completed") {
    restoreCalculation();
    showCompletionNotice(
      current.metadata.mode === "background"
        ? "后台计算完成，结果已保存。"
        : "一速计算完成，结果已保存。",
    );
  } else if (current.status === "failed") {
    error.value = current.error ?? current.message;
  }
  activeSpeedTaskId = null;
}

/** 调度器执行的一速任务；分页读取和 Worker 都由任务上下文报告进度与日志。 */
async function executeSpeedPreview(
  mode: "foreground" | "background",
  context: TaskExecutionContext,
  selectedOnlyMaxLevel: boolean,
  profileId: string,
): Promise<void> {
  calculating.value = true;
  calculationMode.value = mode;
  calculationStage.value = "reading";
  progressLoaded.value = 0;
  progressTotal.value = 0;
  error.value = "";
  calculationNotice.value = "";
  // 后台重算保留上一份结果，让用户可以继续查看；前台重算仍使用原来的遮罩式体验。
  if (mode === "foreground") {
    calculation.value = null;
    visibleSetCount.value = DEFAULT_VISIBLE_SET_COUNT;
  }
  context.onCancel(stopSpeedWorker);
  context.report({
    phase: "读取库存",
    message: `正在读取${selectedOnlyMaxLevel ? " +15" : "全部等级"}御魂…`,
  });
  context.log(`计算前剔除${selectedOnlyMaxLevel ? "非 +15" : "不剔除非 +15"}御魂`);
  try {
    if (context.isCancelled()) return;
    await loadAllSouls(context, selectedOnlyMaxLevel);
    if (context.isCancelled()) return;
    let result: SpeedCalculationResult;
    if (mode === "background") {
      calculationStage.value = "background";
      result = await runBackgroundCalculation(
        souls.value,
        selectedOnlyMaxLevel,
        context,
        speedSetConfigs.value,
      );
    } else {
      // 让遮罩先绘制出“读取完成、正在整理”的阶段，再执行同步排行计算。
      calculationStage.value = "ranking";
      context.report({ phase: "整理排行", message: "库存读取完成，正在整理一速排行…" });
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      result = calculateSpeedPreviewEngine(
        souls.value,
        selectedOnlyMaxLevel,
        speedSetConfigs.value,
      );
    }
    if (context.isCancelled()) return;
    saveCalculation(result, selectedOnlyMaxLevel, profileId);
    context.report({
      phase: "已完成",
      completed: 100,
      total: 100,
      message: "一速排行计算完成，结果已保存。",
    });
    context.log("一速排行计算完成，结果已保存。", "success");
    showCompletionNotice(
      mode === "background"
        ? "后台计算完成，结果已保存。"
        : "一速计算完成，结果已保存。",
    );
  } catch (caught) {
    // Worker 异常不能静默丢失；后台模式保留旧结果，但明确提示本次计算未完成。
    error.value = caught instanceof Error ? caught.message : "一速计算失败";
    if (!context.isCancelled()) {
      context.log(error.value, "error");
      throw caught instanceof Error ? caught : new Error(error.value);
    }
  } finally {
    calculating.value = false;
    calculationMode.value = null;
  }
}

/** 用户点击计算时只登记任务；执行、进度、日志和取消动作统一交给任务调度器。 */
function calculateSpeedPreview(mode: "foreground" | "background" = "foreground"): void {
  if (calculating.value || taskScheduler.findRunning("speed-calculation")) return;
  const profileId = activeCharacterId.value;
  if (!profileId) {
    error.value = "尚未选择角色档案，无法保存一速结果";
    return;
  }
  const selectedOnlyMaxLevel = onlyMaxLevel.value;
  calculating.value = true;
  calculationMode.value = mode;
  activeSpeedTaskId = taskScheduler.start({
    kind: "speed-calculation",
    title: mode === "background" ? "一速预览（后台）" : "一速预览计算",
    metadata: { mode, onlyMaxLevel: selectedOnlyMaxLevel, profileId },
    run: (context) =>
      executeSpeedPreview(mode, context, selectedOnlyMaxLevel, profileId),
  });
  handleSpeedTaskSnapshot(taskScheduler.snapshot());
}

/** 将分页读取数量转换为遮罩层进度百分比；总数未知时保持不确定进度。 */
const progressPercent = computed(() => {
  if (!progressTotal.value) return 0;
  return Math.min(100, Math.round((progressLoaded.value / progressTotal.value) * 100));
});

const progressLabel = computed(() => {
  if (!progressTotal.value) return "正在获取库存总数…";
  if (calculationStage.value === "ranking") return "库存读取完成，正在整理排行…";
  if (calculationStage.value === "background") return "库存读取完成，后台整理排行…";
  return `已读取 ${progressLoaded.value} / ${progressTotal.value} 枚`;
});

/** 在点击计算前明确展示实际筛选动作，避免用户只在结果页才知道哪些御魂被排除。 */
const calculationFilterDescription = computed(() =>
  onlyMaxLevel.value
    ? "计算前剔除未满级御魂，仅统计六星 +15。"
    : "不剔除未满级御魂，统计全部六星。",
);

/** 让缓存时间以本地语言显示，提醒用户结果来自哪一次主动计算。 */
function formatCalculatedAt(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "时间未知";
  return date.toLocaleString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** 将属性稳定键转换为玩家熟悉的中文名称；目录未来增加属性时仍保留原键可核对。 */
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

/** 按“我的御魂”和 PVE 详情的单位规则展示主属性、副属性数值。 */
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

/** 按 PVE 详情相同的 1、6、2、5、3、4 顺序返回当前组合，缺失号位保留空卡位。 */
function detailSoulsBySlot(): Array<MySoul | undefined> {
  const selected = soulDetail.value?.selected ?? [];
  return detailSlots.map((slot) => selected.find((soul) => soul.slot === slot));
}

// 详情卡片只依赖当前打开的组合，使用计算值避免模板重复执行号位匹配。
const detailSlotSouls = computed(detailSoulsBySlot);

/** 打开某个套装排行的实际六枚御魂详情；占位行没有组合时按钮不会触发。 */
function openSoulDetails(row: SpeedSection["rows"][number]): void {
  if (!row.selected?.length) return;
  soulDetail.value = {
    title: `${row.label} · 一速组合`,
    subtitle: "查看当前排行行实际选中的六个位置御魂",
    selected: row.selected,
  };
}

/** 关闭当前六位置详情，供遮罩、关闭按钮和 Escape 键复用。 */
function closeSoulDetails(): void {
  soulDetail.value = null;
}

/** 详情浮层只响应自己的 Escape，不影响页面其他键盘交互。 */
function handleSoulDetailKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape" && soulDetail.value) closeSoulDetails();
}

/** 从完整计算结果中截取当前已展开的套装行；散件、命中和抵抗排行保持原有展示数量。 */
const visibleSetRows = computed(() => {
  const setSection = calculation.value?.sections.find(
    (section) => section.title === SET_SECTION_TITLE,
  );
  return setSection?.rows.slice(0, visibleSetCount.value) ?? [];
});

/** 判断是否还有未展示的四件套，供“加载更多”按钮和剩余数量提示共用。 */
const hasMoreSetRows = computed(() => {
  const setSection = calculation.value?.sections.find(
    (section) => section.title === SET_SECTION_TITLE,
  );
  return visibleSetRows.value.length < (setSection?.rows.length ?? 0);
});

/** 当前目录中参与计算的四件套总数，用于提示用户加载进度。 */
const totalSetRowCount = computed(
  () =>
    calculation.value?.sections.find((section) => section.title === SET_SECTION_TITLE)
      ?.rows.length ?? 0,
);

/** 每次展开十条，直到显示完整个目录计算出的套装排行。 */
function loadMoreSetRows(): void {
  visibleSetCount.value += DEFAULT_VISIBLE_SET_COUNT;
}

/** 将“默认十条”应用到套装分组，同时保留其他分组原有结果结构。 */
const displayedSections = computed(() =>
  (calculation.value?.sections ?? []).map((section) =>
    section.title === SET_SECTION_TITLE
      ? { ...section, rows: visibleSetRows.value }
      : section,
  ),
);

// 首次进入只恢复轻量结果，不自动扫描库存；数据更新后由用户点击按钮重新计算。
function handleInventoryCleared(): void {
  if (activeSpeedTaskId) taskScheduler.cancel(activeSpeedTaskId);
  calculation.value = null;
  visibleSetCount.value = DEFAULT_VISIBLE_SET_COUNT;
  soulDetail.value = null;
  restoredFromCache.value = false;
  souls.value = [];
  error.value = "";
}

/** 角色切换时取消旧角色的计算并读取新角色缓存，避免后台结果写入错误的角色空间。 */
function handleActiveCharacterChanged(profileId: string | null): void {
  if (activeSpeedTaskId) taskScheduler.cancel(activeSpeedTaskId);
  calculation.value = null;
  visibleSetCount.value = DEFAULT_VISIBLE_SET_COUNT;
  soulDetail.value = null;
  restoredFromCache.value = false;
  souls.value = [];
  error.value = "";
  if (profileId) restoreCalculation();
}

onMounted(() => {
  restoreCalculation();
  // 调度器单例跨页面保留任务；页面只订阅当前快照，不在卸载时销毁 Worker。
  stopTaskListening = taskScheduler.subscribe(handleSpeedTaskSnapshot);
  window.addEventListener(MY_SOULS_INVENTORY_CLEARED_EVENT, handleInventoryCleared);
  window.addEventListener("keydown", handleSoulDetailKeydown);
  stopActiveCharacterChanged = onActiveCharacterChanged(handleActiveCharacterChanged);
});

onBeforeUnmount(() => {
  stopTaskListening?.();
  stopTaskListening = null;
  window.removeEventListener(MY_SOULS_INVENTORY_CLEARED_EVENT, handleInventoryCleared);
  window.removeEventListener("keydown", handleSoulDetailKeydown);
  stopActiveCharacterChanged?.();
  if (noticeTimer !== null) window.clearTimeout(noticeTimer);
});
</script>

<template>
  <main class="speed-soul-page" aria-label="一速御魂页面">
    <div
      v-if="calculating && calculationMode === 'foreground'"
      class="speed-preview-overlay"
      role="status"
      aria-live="polite"
      aria-label="正在计算一速排行"
    >
      <div class="speed-preview-overlay__panel">
        <span class="speed-preview-overlay__mark" aria-hidden="true">速</span>
        <p class="speed-preview-overlay__eyebrow">SPEED ARCHIVE</p>
        <h2>{{ calculationStage === "ranking" ? "正在整理一速" : "正在读取御魂" }}</h2>
        <p class="speed-preview-overlay__message">{{ progressLabel }}</p>
        <div
          class="speed-preview-progress"
          :class="{ 'speed-preview-progress--indeterminate': !progressTotal }"
          role="progressbar"
          :aria-valuenow="progressTotal ? progressLoaded : undefined"
          :aria-valuemin="progressTotal ? 0 : undefined"
          :aria-valuemax="progressTotal || undefined"
          :aria-label="progressLabel"
        >
          <span
            class="speed-preview-progress__bar"
            :style="{ width: progressTotal ? `${progressPercent}%` : undefined }"
          ></span>
        </div>
        <span v-if="progressTotal" class="speed-preview-overlay__percent">
          {{ progressPercent }}%
        </span>
        <p class="speed-preview-overlay__hint">请稍候，计算完成后结果会自动保存</p>
      </div>
    </div>

    <div class="speed-preview-titlebar">
      <span class="speed-preview-titlebar__meta">一速档案</span>
      <h2 class="speed-preview-title">一速预览</h2>
      <div class="speed-preview-titlebar__actions">
        <span class="speed-preview-titlebar__meta speed-preview-titlebar__meta--right">
          <span class="speed-preview-legend" aria-label="速度颜色规则">
            <i
              class="speed-preview-legend__swatch speed-preview-legend__swatch--hot"
            ></i>
            &gt;17
            <i
              class="speed-preview-legend__swatch speed-preview-legend__swatch--rainbow"
            ></i>
            &gt;17.5
          </span>
        </span>
        <button
          class="speed-preview-calculate speed-preview-calculate--background"
          type="button"
          :disabled="calculating"
          @click="calculateSpeedPreview('background')"
        >
          {{
            calculating && calculationMode === "background" ? "后台计算中…" : "后台计算"
          }}
        </button>
        <button
          class="speed-preview-calculate"
          type="button"
          :disabled="calculating"
          @click="calculateSpeedPreview('foreground')"
        >
          {{
            calculating && calculationMode === "foreground"
              ? "计算中…"
              : calculation
                ? "重新计算"
                : "计算一速"
          }}
        </button>
      </div>
    </div>

    <div class="speed-preview-options" aria-label="一速计算范围">
      <label class="speed-preview-max-level-toggle">
        <input v-model="onlyMaxLevel" type="checkbox" :disabled="calculating" />
        <span>只计算满级</span>
      </label>
      <small>{{ calculationFilterDescription }}</small>
    </div>

    <p
      v-if="calculationNotice"
      class="speed-preview-notice"
      role="status"
      aria-live="polite"
    >
      {{ calculationNotice }}
    </p>
    <p
      v-if="calculating && calculationMode === 'background'"
      class="speed-preview-state"
    >
      正在后台计算一速排行 · {{ calculationFilterDescription }} · {{ progressLabel }} ·
      完成后会通知并自动保存结果。
    </p>
    <p v-else-if="calculating" class="speed-preview-state">
      正在读取当前御魂库存并计算一速排行…
    </p>
    <p v-if="error" class="speed-preview-state speed-preview-state--error" role="alert">
      {{ error }}
      <button type="button" @click="calculateSpeedPreview('foreground')">
        重新计算
      </button>
    </p>
    <p
      v-if="!calculating && !calculation && !error"
      class="speed-preview-state speed-preview-state--empty"
    >
      尚未计算一速排行。点击右上角“计算一速”或“后台计算”读取当前库存，结果会保存到本机。
    </p>

    <template v-if="calculation">
      <p class="speed-preview-source">
        {{ restoredFromCache ? "已恢复本机缓存" : "刚刚完成计算" }} ·
        {{ formatCalculatedAt(calculation.calculatedAt) }} · 已读取
        {{ calculation.soulCount }} 枚，符合条件 {{ calculation.qualifiedSoulCount }} 枚
        · {{ calculation.onlyMaxLevel ? "只计算满级" : "包含未满级" }}；套装一速按四件套
        + 两件任意计算。
      </p>
      <section
        v-for="(section, sectionIndex) in displayedSections"
        :key="section.title"
        class="speed-preview-block"
        :aria-labelledby="`speed-section-${section.title}`"
      >
        <div class="speed-preview-block__heading">
          <span class="speed-preview-block__index" aria-hidden="true">
            {{ String(sectionIndex + 1).padStart(2, "0") }}
          </span>
          <h3 :id="`speed-section-${section.title}`">{{ section.title }}</h3>
          <span class="speed-preview-block__kind">
            {{
              section.title === SET_SECTION_TITLE
                ? "套装排名"
                : section.extraHeader
                  ? "用途分组"
                  : "散件排名"
            }}
          </span>
        </div>

        <div v-if="section.rows.length" class="speed-preview-table-wrap">
          <table class="speed-preview-table">
            <colgroup>
              <col class="speed-preview-table__type" />
              <col class="speed-preview-table__speed" />
              <col class="speed-preview-table__positions" />
              <col v-if="section.extraHeader" class="speed-preview-table__extra" />
              <col class="speed-preview-table__action" />
            </colgroup>
            <thead>
              <tr>
                <th scope="col">套装类型</th>
                <th scope="col">一速</th>
                <th scope="col">{{ section.positionHeader }}</th>
                <th v-if="section.extraHeader" scope="col">
                  {{ section.extraHeader }}
                </th>
                <th scope="col">操作</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="row in section.rows" :key="`${section.title}-${row.label}`">
                <td class="speed-preview-table__label">
                  <span class="speed-preview-set-label">
                    <img
                      v-if="soulIcon(row.label)"
                      :src="soulIcon(row.label) ?? undefined"
                      :alt="`${row.label}图标`"
                    />
                    <span
                      v-else-if="section.title === '套装一速'"
                      class="speed-preview-set-fallback"
                      aria-hidden="true"
                    >
                      {{ row.label.slice(0, 1) }}
                    </span>
                    <span>{{ row.label }}</span>
                  </span>
                </td>
                <td class="speed-preview-table__speed-value">{{ row.speed }}</td>
                <td class="speed-preview-table__positions-value">
                  <span
                    v-for="(position, index) in row.positions"
                    :key="`${row.label}-${position.value}-${index}`"
                    class="speed-preview-metric"
                  >
                    <span
                      class="speed-preview-metric__value"
                      :class="`speed-preview-metric__value--${speedTone(position.value)}`"
                    >
                      {{ position.value }}
                    </span>
                    <span v-if="position.tail" class="speed-preview-metric__tail">
                      [{{ position.tail }}]
                    </span>
                    <span
                      v-if="index < row.positions.length - 1"
                      class="speed-preview-metric__separator"
                      aria-hidden="true"
                    >
                      |
                    </span>
                  </span>
                </td>
                <td v-if="section.extraHeader" class="speed-preview-table__extra-value">
                  {{ row.extra }}
                </td>
                <td class="speed-preview-table__action-cell">
                  <button
                    class="speed-preview-details-button"
                    type="button"
                    :disabled="!row.selected?.length"
                    :aria-label="`查看${row.label}的一速六位置御魂`"
                    @click="openSoulDetails(row)"
                  >
                    <svg
                      class="speed-preview-details-icon"
                      viewBox="0 0 24 24"
                      aria-hidden="true"
                    >
                      <path
                        d="M2.5 12s3.4-5.5 9.5-5.5 9.5 5.5 9.5 5.5-3.4 5.5-9.5 5.5S2.5 12 2.5 12Z"
                      />
                      <circle cx="12" cy="12" r="2.4" />
                    </svg>
                    查看详情
                  </button>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <div
          v-if="section.title === '套装一速' && hasMoreSetRows"
          class="speed-preview-load-more"
        >
          <button type="button" @click="loadMoreSetRows">加载更多</button>
          <span
            >已显示 {{ section.rows.length }} / {{ totalSetRowCount }} 套四件套</span
          >
        </div>
        <p v-if="!section.rows.length" class="speed-preview-empty">
          当前库存没有满足这一组规则的完整组合。
        </p>
      </section>
    </template>

    <div
      v-if="soulDetail"
      class="speed-soul-detail-overlay"
      role="presentation"
      @click.self="closeSoulDetails"
    >
      <section
        class="speed-soul-detail-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="speed-soul-detail-title"
      >
        <header class="speed-soul-detail-dialog__header">
          <div>
            <p class="speed-soul-detail-dialog__eyebrow">SELECTED SOULS / 6 SLOTS</p>
            <h3 id="speed-soul-detail-title">{{ soulDetail.title }}</h3>
            <p>{{ soulDetail.subtitle }}</p>
          </div>
          <button
            class="speed-soul-detail-dialog__close"
            type="button"
            aria-label="关闭御魂详情"
            @click="closeSoulDetails"
          >
            ×
          </button>
        </header>

        <div class="speed-soul-detail-list">
          <article
            v-for="(soul, index) in detailSlotSouls"
            :key="soul?.soulKey ?? `empty-${index}`"
            class="speed-soul-detail-item"
            :class="{ 'speed-soul-detail-item--empty': !soul }"
          >
            <span class="speed-soul-detail-item__slot">{{ detailSlots[index] }}号</span>
            <template v-if="soul">
              <span class="speed-soul-detail-item__icon" :title="`${soul.setId}图标`">
                <img v-if="soulIcon(soul.setId)" :src="soulIcon(soul.setId)!" alt="" />
                <span v-else>{{ soul.setId.slice(0, 1) }}</span>
              </span>
              <div class="speed-soul-detail-item__identity">
                <b>{{ soul.setId }}</b>
                <small
                  >{{ soul.quality }}星 · +{{ soul.level }} · {{ soul.soulKey }}</small
                >
              </div>
              <div class="speed-soul-detail-item__attributes">
                <strong>
                  主：{{ attributeLabel(soul.mainAttrType) }}
                  {{ formatSoulAttributeValue(soul.mainAttrType, soul.mainAttrValue) }}
                </strong>
                <div
                  v-if="soul.attributes.length"
                  class="speed-soul-detail-item__subattributes"
                >
                  <div
                    v-for="attribute in soul.attributes"
                    :key="`${soul.soulKey}-${attribute.attributeIndex}`"
                    class="speed-soul-detail-item__subattribute"
                  >
                    <span>
                      {{ attributeLabel(attribute.attributeType) }}
                      {{
                        formatSoulAttributeValue(
                          attribute.attributeType,
                          attribute.value,
                        )
                      }}
                    </span>
                    <strong v-if="attribute.enhancementCount !== null">
                      强化 {{ attribute.enhancementCount }} 次
                    </strong>
                    <small v-else>强化次数未知</small>
                  </div>
                </div>
                <span v-else>副：暂无副属性记录</span>
              </div>
            </template>
            <span v-else class="speed-soul-detail-item__empty-copy"
              >该位置没有满足条件的御魂</span
            >
          </article>
        </div>

        <footer class="speed-soul-detail-dialog__footer">
          <span>只展示本次一速计算实际选中的御魂</span>
          <button type="button" @click="closeSoulDetails">完成</button>
        </footer>
      </section>
    </div>
  </main>
</template>

<style scoped>
/* 保留参考图的白底表格骨架，只用朱砂短线、浅暖底和编号增强页面识别度。 */
.speed-soul-page {
  --speed-text: #4d5866;
  --speed-heading: #2f3740;
  --speed-muted: #8c96a2;
  --speed-line: #e8edf1;
  --speed-accent: #c4855e;
  --speed-warm: #faf6ef;
  position: relative;
  padding: 26px 22px 52px;
  color: var(--speed-text);
  background:
    linear-gradient(
      90deg,
      transparent 0,
      transparent 6px,
      rgb(196 133 94 / 9%) 6px,
      rgb(196 133 94 / 9%) 7px,
      transparent 7px
    ),
    #fff;
}

/* 全量读取期间锁住页面操作，遮罩中心只保留阶段、进度和结果保存提示。 */
.speed-preview-overlay {
  position: fixed;
  z-index: 20;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 24px;
  background: rgb(38 47 57 / 32%);
  backdrop-filter: blur(2px);
}

.speed-preview-overlay__panel {
  width: min(390px, 100%);
  padding: 30px 32px 25px;
  border: 1px solid rgb(255 255 255 / 72%);
  border-radius: 6px;
  color: var(--speed-heading);
  background: rgb(255 255 255 / 96%);
  box-shadow: 0 18px 48px rgb(38 47 57 / 18%);
  text-align: center;
}

.speed-preview-overlay__mark {
  display: grid;
  place-items: center;
  width: 44px;
  height: 44px;
  margin: 0 auto 15px;
  border: 1px solid rgb(196 133 94 / 50%);
  border-radius: 50%;
  color: var(--speed-accent);
  font-family: "Noto Serif SC", Georgia, serif;
  font-size: 22px;
  font-weight: 900;
}

.speed-preview-overlay__eyebrow {
  margin: 0 0 8px;
  color: var(--speed-accent);
  font-size: 9px;
  font-weight: 800;
  letter-spacing: 0.16em;
}

.speed-preview-overlay__panel h2 {
  margin: 0;
  color: var(--speed-heading);
  font-size: 22px;
  letter-spacing: -0.05em;
}

.speed-preview-overlay__message {
  min-height: 18px;
  margin: 11px 0 15px;
  color: var(--speed-text);
  font-size: 12px;
}

.speed-preview-progress {
  position: relative;
  width: 100%;
  height: 8px;
  overflow: hidden;
  border-radius: 999px;
  background: #edf0f2;
}

.speed-preview-progress__bar {
  display: block;
  height: 100%;
  min-width: 2px;
  border-radius: inherit;
  background: linear-gradient(90deg, #c4855e, #e4a35e);
  transition: width 180ms ease;
}

.speed-preview-progress--indeterminate .speed-preview-progress__bar {
  position: absolute;
  width: 42%;
  animation: speed-preview-progress-slide 1.15s ease-in-out infinite;
}

.speed-preview-overlay__percent {
  display: block;
  margin-top: 9px;
  color: var(--speed-accent);
  font-size: 12px;
  font-weight: 800;
  font-variant-numeric: tabular-nums;
}

.speed-preview-overlay__hint {
  margin: 16px 0 0;
  color: var(--speed-muted);
  font-size: 10px;
}

@keyframes speed-preview-progress-slide {
  0% {
    transform: translateX(-110%);
  }

  100% {
    transform: translateX(255%);
  }
}

.speed-preview-titlebar {
  display: grid;
  grid-template-columns: 1fr auto 1fr;
  align-items: center;
  gap: 16px;
  margin: 0 0 28px;
  padding: 0 5px 28px;
  border-bottom: 1px solid var(--speed-line);
}

.speed-preview-title {
  margin: 0;
  color: var(--speed-heading);
  font-size: 28px;
  font-weight: 800;
  letter-spacing: -0.06em;
  line-height: 1;
  text-align: center;
}

.speed-preview-titlebar__meta {
  position: relative;
  padding-left: 14px;
  color: var(--speed-muted);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.08em;
}

.speed-preview-titlebar__meta::before {
  position: absolute;
  top: 50%;
  left: 0;
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: var(--speed-accent);
  content: "";
  transform: translateY(-50%);
}

.speed-preview-titlebar__meta--right {
  padding-right: 14px;
  padding-left: 0;
  text-align: right;
}

.speed-preview-titlebar__meta--right::before {
  right: 0;
  left: auto;
}

.speed-preview-titlebar__actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
}

.speed-preview-calculate {
  min-width: 78px;
  padding: 7px 12px;
  border: 1px solid var(--speed-accent);
  border-radius: 4px;
  color: #fff;
  background: var(--speed-accent);
  cursor: pointer;
  font-size: 11px;
  font-weight: 800;
}

.speed-preview-calculate:hover:not(:disabled) {
  background: #ae6d4d;
}

/* 后台按钮使用浅色边框，和立即计算区分动作但保持相同可发现性。 */
.speed-preview-calculate--background {
  color: var(--speed-accent);
  background: #fff;
}

.speed-preview-calculate--background:hover:not(:disabled) {
  color: #ae6d4d;
  background: var(--speed-warm);
}

.speed-preview-calculate:focus-visible,
.speed-preview-state button:focus-visible {
  outline: 2px solid #6c82ed;
  outline-offset: 2px;
}

.speed-preview-calculate:disabled {
  cursor: wait;
  opacity: 0.62;
}

.speed-preview-legend {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  color: var(--speed-muted);
  font-size: 9px;
  font-weight: 700;
  letter-spacing: 0.02em;
}

.speed-preview-legend__swatch {
  width: 7px;
  height: 7px;
  margin-left: 2px;
  border-radius: 50%;
}

.speed-preview-legend__swatch--hot {
  background: var(--speed-accent);
}

.speed-preview-legend__swatch--rainbow {
  margin-left: 6px;
  background: linear-gradient(
    135deg,
    #ff7d68,
    #ffca58 28%,
    #54c7a7 54%,
    #6988ff 78%,
    #c477ed
  );
}

/* 计算范围单独成行，避免复选框被标题栏的排行按钮挤到次要位置。 */
.speed-preview-options {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-wrap: wrap;
  gap: 8px 12px;
  margin: -10px 0 18px;
  color: var(--speed-muted);
  font-size: 11px;
}

.speed-preview-max-level-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--speed-heading);
  font-weight: 800;
  cursor: pointer;
}

.speed-preview-max-level-toggle input {
  accent-color: var(--speed-accent);
}

.speed-preview-options small {
  font-size: 10px;
}

.speed-preview-notice {
  margin: 0 auto 14px;
  padding: 8px 12px;
  border: 1px solid rgb(196 133 94 / 30%);
  color: #8e6048;
  background: rgb(250 246 239 / 72%);
  font-size: 11px;
  text-align: center;
}

.speed-preview-source,
.speed-preview-state,
.speed-preview-empty {
  margin: 0 0 20px;
  color: var(--speed-muted);
  font-size: 11px;
  line-height: 1.6;
  text-align: center;
}

.speed-preview-state {
  padding: 28px 12px;
  border: 1px dashed var(--speed-line);
  background: var(--speed-warm);
}

.speed-preview-state--error {
  color: #a65b4a;
}

.speed-preview-state button {
  margin-left: 8px;
  border: 0;
  color: var(--speed-accent);
  background: transparent;
  cursor: pointer;
  font: inherit;
  font-weight: 700;
}

.speed-preview-block {
  margin: 0 0 32px;
  padding-bottom: 32px;
  border-bottom: 1px solid var(--speed-line);
}

.speed-preview-block:last-child {
  margin-bottom: 0;
  padding-bottom: 0;
  border-bottom: 0;
}

.speed-preview-block__heading {
  display: grid;
  grid-template-columns: 28px 1fr auto;
  align-items: center;
  gap: 10px;
  width: fit-content;
  min-width: 260px;
  margin: 0 auto 15px;
  padding: 0 12px 9px;
  border-bottom: 2px solid var(--speed-accent);
}

.speed-preview-block__index {
  color: var(--speed-accent);
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.1em;
}

.speed-preview-block h3 {
  margin: 0;
  color: var(--speed-heading);
  font-size: 22px;
  font-weight: 800;
  letter-spacing: -0.05em;
  line-height: 1;
  text-align: center;
}

.speed-preview-block__kind {
  color: var(--speed-muted);
  font-size: 9px;
  letter-spacing: 0.06em;
}

.speed-preview-table-wrap {
  overflow-x: auto;
}

.speed-preview-table {
  width: max-content;
  min-width: 100%;
  border-collapse: collapse;
  font-variant-numeric: tabular-nums;
}

.speed-preview-table__type {
  width: 164px;
}

.speed-preview-table__speed {
  width: 96px;
}

.speed-preview-table__positions {
  width: 700px;
}

.speed-preview-table__extra {
  width: 164px;
}

.speed-preview-table__action {
  width: 118px;
}

.speed-preview-table th,
.speed-preview-table td {
  padding: 9px 16px;
  border-bottom: 1px solid var(--speed-line);
  text-align: left;
  vertical-align: middle;
  white-space: nowrap;
}

.speed-preview-table th {
  color: var(--speed-muted);
  background: linear-gradient(90deg, var(--speed-warm), transparent 75%);
  font-size: 14px;
  font-weight: 700;
}

.speed-preview-table td {
  color: var(--speed-text);
  font-size: 14px;
  line-height: 1.25;
}

.speed-preview-table tbody tr:hover {
  background: #fffaf5;
}

.speed-preview-table tbody tr:first-child {
  background: rgb(250 246 239 / 54%);
}

.speed-preview-table__label {
  color: #596575 !important;
}

/* 套装图标作为类型列的第一识别层，尺寸固定避免不同资源撑高表格行。 */
.speed-preview-set-label {
  display: inline-flex;
  align-items: center;
  gap: 8px;
}

.speed-preview-set-label img {
  width: 26px;
  height: 26px;
  flex: 0 0 26px;
  object-fit: contain;
}

.speed-preview-set-fallback {
  display: inline-grid;
  width: 26px;
  height: 26px;
  flex: 0 0 26px;
  place-items: center;
  border: 1px solid rgb(196 133 94 / 42%);
  border-radius: 50%;
  color: var(--speed-accent);
  background: var(--speed-warm);
  font-size: 12px;
  font-weight: 800;
}

.speed-preview-table__speed-value {
  color: #475363 !important;
  font-size: 16px !important;
}

.speed-preview-table__positions-value {
  min-width: 700px;
  color: #596575 !important;
}

.speed-preview-metric {
  display: inline-flex;
  align-items: baseline;
}

.speed-preview-metric__value--hot {
  color: var(--speed-accent);
  font-weight: 800;
}

.speed-preview-metric__value--rainbow {
  color: transparent;
  background: linear-gradient(
    100deg,
    #f06d5d 0%,
    #e8b34d 23%,
    #4dbd9e 48%,
    #6c82ed 72%,
    #c270e5 100%
  );
  background-clip: text;
  -webkit-background-clip: text;
  font-weight: 900;
  text-shadow: 0 0 12px rgb(97 137 224 / 13%);
}

.speed-preview-metric__tail {
  margin-left: 4px;
  color: #65707c;
}

.speed-preview-metric__separator {
  margin: 0 8px;
  color: #b4bdc6;
}

.speed-preview-table__extra-value {
  min-width: 164px;
  padding-right: 22px !important;
  padding-left: 22px !important;
  color: #596575 !important;
  background: rgb(250 246 239 / 70%);
  font-size: 15px !important;
  font-weight: 700;
}

/* 套装目录较长时用独立操作行继续展开，避免首屏被几十条四件套淹没。 */
.speed-preview-load-more {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  margin-top: 14px;
  color: var(--speed-muted);
  font-size: 10px;
}

.speed-preview-load-more button {
  padding: 7px 18px;
  border: 1px solid rgb(196 133 94 / 45%);
  border-radius: 999px;
  color: #9b684d;
  background: #fffdf9;
  cursor: pointer;
  font-size: 10px;
  font-weight: 800;
}

.speed-preview-load-more button:hover {
  color: #fff;
  background: var(--speed-accent);
}

.speed-preview-load-more button:focus-visible {
  outline: 2px solid #6c82ed;
  outline-offset: 2px;
}

/* 套装排行的详情入口保持轻量线框，避免抢过总速度数字的视觉优先级。 */
.speed-preview-table__action-cell {
  min-width: 118px;
  padding-right: 12px !important;
  padding-left: 12px !important;
  text-align: center !important;
}

.speed-preview-details-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  min-width: 88px;
  padding: 6px 9px;
  border: 1px solid rgb(196 133 94 / 42%);
  border-radius: 999px;
  color: #9b684d;
  background: #fffdf9;
  cursor: pointer;
  font-size: 10px;
  font-weight: 800;
}

.speed-preview-details-button:hover:not(:disabled) {
  color: #fff;
  background: var(--speed-accent);
}

.speed-preview-details-button:focus-visible,
.speed-soul-detail-dialog__close:focus-visible,
.speed-soul-detail-dialog__footer button:focus-visible {
  outline: 2px solid #6c82ed;
  outline-offset: 2px;
}

.speed-preview-details-button:disabled {
  border-color: var(--speed-line);
  color: #b8bec5;
  background: #f7f8f9;
  cursor: not-allowed;
}

.speed-preview-details-icon {
  width: 14px;
  height: 14px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.7;
}

/* 详情弹层延续 PVE 的六位置核对结构，但用暖纸色和朱砂线强调一速档案身份。 */
.speed-soul-detail-overlay {
  position: fixed;
  z-index: 40;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 18px;
  background: rgb(38 47 57 / 42%);
  backdrop-filter: blur(3px);
}

.speed-soul-detail-dialog {
  display: grid;
  width: min(940px, 100%);
  max-height: min(760px, calc(100vh - 36px));
  overflow: hidden;
  border: 1px solid rgb(196 133 94 / 35%);
  border-top: 4px solid var(--speed-accent);
  border-radius: 8px;
  color: var(--speed-text);
  background: #fffdf9;
  box-shadow: 0 24px 70px rgb(38 47 57 / 25%);
}

.speed-soul-detail-dialog__header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 18px;
  padding: 22px 24px 16px;
  border-bottom: 1px solid var(--speed-line);
}

.speed-soul-detail-dialog__eyebrow {
  margin: 0 0 7px;
  color: var(--speed-accent);
  font-size: 9px;
  font-weight: 800;
  letter-spacing: 0.16em;
}

.speed-soul-detail-dialog__header h3 {
  margin: 0;
  color: var(--speed-heading);
  font-size: 22px;
  letter-spacing: -0.04em;
}

.speed-soul-detail-dialog__header p:last-child {
  margin: 7px 0 0;
  color: var(--speed-muted);
  font-size: 11px;
}

.speed-soul-detail-dialog__close {
  display: grid;
  width: 30px;
  height: 30px;
  flex: 0 0 30px;
  place-items: center;
  border: 1px solid var(--speed-line);
  border-radius: 50%;
  color: var(--speed-muted);
  background: #fff;
  cursor: pointer;
  font-size: 22px;
  line-height: 1;
}

.speed-soul-detail-dialog__close:hover {
  border-color: rgb(196 133 94 / 45%);
  color: var(--speed-accent);
}

.speed-soul-detail-list {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
  overflow-y: auto;
  padding: 16px 20px;
}

.speed-soul-detail-item {
  display: grid;
  grid-template-columns: 42px 38px minmax(90px, 0.7fr) minmax(220px, 1.65fr);
  align-items: center;
  gap: 9px;
  min-width: 0;
  padding: 10px;
  border: 1px solid rgb(196 133 94 / 17%);
  border-radius: 6px;
  background: linear-gradient(110deg, rgb(250 246 239 / 82%), #fff);
}

.speed-soul-detail-item--empty {
  color: var(--speed-muted);
  background: #fafbfc;
}

.speed-soul-detail-item__slot {
  display: grid;
  min-height: 26px;
  place-items: center;
  border-radius: 4px;
  color: #fff;
  background: var(--speed-accent);
  font-size: 11px;
  font-weight: 800;
}

.speed-soul-detail-item__icon {
  display: grid;
  width: 34px;
  height: 34px;
  place-items: center;
  overflow: hidden;
  border: 1px solid rgb(196 133 94 / 28%);
  border-radius: 50%;
  color: var(--speed-accent);
  background: var(--speed-warm);
  font-size: 14px;
  font-weight: 800;
}

.speed-soul-detail-item__icon img {
  width: 100%;
  height: 100%;
  object-fit: contain;
}

.speed-soul-detail-item__identity,
.speed-soul-detail-item__attributes {
  display: grid;
  min-width: 0;
  gap: 4px;
}

.speed-soul-detail-item__identity b {
  overflow: hidden;
  color: var(--speed-heading);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.speed-soul-detail-item__identity small,
.speed-soul-detail-item__empty-copy,
.speed-soul-detail-item__attributes > span {
  overflow: hidden;
  color: var(--speed-muted);
  font-size: 10px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.speed-soul-detail-item__attributes > strong {
  overflow: hidden;
  color: #596575;
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.speed-soul-detail-item__subattributes {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

.speed-soul-detail-item__subattribute {
  display: inline-flex;
  align-items: baseline;
  gap: 4px;
  padding: 3px 5px;
  border: 1px solid var(--speed-line);
  border-radius: 3px;
  color: #697482;
  background: #fff;
  font-size: 10px;
  white-space: nowrap;
}

.speed-soul-detail-item__subattribute strong {
  color: var(--speed-accent);
  font-size: 9px;
}

.speed-soul-detail-item__subattribute small {
  color: var(--speed-muted);
  font-size: 9px;
}

.speed-soul-detail-dialog__footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 12px 20px 14px;
  border-top: 1px solid var(--speed-line);
  color: var(--speed-muted);
  font-size: 10px;
}

.speed-soul-detail-dialog__footer button {
  padding: 6px 14px;
  border: 1px solid var(--speed-accent);
  border-radius: 4px;
  color: #fff;
  background: var(--speed-accent);
  cursor: pointer;
  font-size: 10px;
  font-weight: 800;
}

.speed-soul-detail-dialog__footer button:hover {
  background: #ae6d4d;
}

@media (max-width: 720px) {
  .speed-soul-page {
    padding: 22px 12px 40px;
  }

  .speed-preview-titlebar {
    grid-template-columns: 1fr;
    gap: 11px;
    margin-bottom: 24px;
    padding-bottom: 24px;
  }

  .speed-preview-titlebar__meta,
  .speed-preview-titlebar__meta--right {
    justify-self: center;
    padding: 0 0 0 14px;
    text-align: center;
  }

  .speed-preview-titlebar__meta--right {
    padding-right: 14px;
    padding-left: 0;
  }

  .speed-preview-titlebar__actions {
    justify-content: center;
    flex-wrap: wrap;
  }

  .speed-preview-title {
    font-size: 25px;
  }

  .speed-preview-overlay__panel {
    padding: 26px 22px 22px;
  }

  .speed-preview-block {
    margin-bottom: 27px;
    padding-bottom: 27px;
  }

  .speed-preview-block__heading {
    min-width: 0;
  }

  .speed-preview-block h3 {
    font-size: 20px;
  }

  .speed-preview-table th,
  .speed-preview-table td {
    padding: 8px 10px;
  }

  .speed-soul-detail-overlay {
    padding: 10px;
  }

  .speed-soul-detail-dialog {
    max-height: calc(100vh - 20px);
  }

  .speed-soul-detail-dialog__header {
    padding: 18px 16px 14px;
  }

  .speed-soul-detail-list {
    grid-template-columns: 1fr;
    padding: 12px;
  }

  .speed-soul-detail-item {
    grid-template-columns: 38px 34px minmax(0, 1fr);
  }

  .speed-soul-detail-item__attributes {
    grid-column: 3 / -1;
  }

  .speed-soul-detail-dialog__footer {
    padding-right: 12px;
    padding-left: 12px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .speed-preview-progress__bar {
    transition: none;
  }

  .speed-preview-progress--indeterminate .speed-preview-progress__bar {
    animation: none;
    width: 42%;
    transform: translateX(70%);
  }
}
</style>
