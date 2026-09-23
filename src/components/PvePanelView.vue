<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import type { MySoul, SoulSet } from "../api/contracts";
import { normalizeCommandError } from "../api/errors";
import {
  type PveCalculationProgress,
  type PveCalculationResult,
} from "../pveCalculationEngine";
import type { PveWorkerResponse } from "../workers/pveCalculation.worker";
import {
  taskScheduler,
  type ScheduledTask,
  type TaskExecutionContext,
} from "../tasks/taskScheduler";
import {
  createPveCalculationCache,
  expandPveCalculationSelections,
  savePveCalculationCache,
  type PveCalculationCache,
  type PveSoulDetail,
} from "./pveCalculationCache";
import {
  MY_SOULS_INVENTORY_CLEARED_EVENT,
  pveCalculationStorageKey,
} from "./mySoulsDerivedStorage";
import { activeCharacterId, onActiveCharacterChanged } from "../state/activeCharacter";

// 静态原型复用应用内置的离线御魂图标，页面不发起网络请求。
const iconModules = import.meta.glob("../assets/soul-icons/*.png", {
  eager: true,
  import: "default",
  query: "?url",
});

type BossSoul = {
  name: string;
  shortName: string;
  tone: "blue" | "green" | "orange" | "violet";
  iconAssetId: number;
  subtitle: string;
};

type OutputSoul = {
  setId: string;
  name: string;
  mark: string;
  tone: "red" | "violet" | "jade" | "gold" | "blue" | "ink" | "rose";
  iconAssetId: number;
  note: string;
  values: [number, number, number, number];
};

type OtherMetric = "damage" | "percent" | "attack" | "defense";

type OtherPlan = {
  id: string;
  shikigami: string;
  shikigamiId: number;
  shikigamiMark: string;
  soul: string;
  soulIconAssetIds: number[];
  statLabel: string;
  requirement: string;
  tone: "blue" | "gold" | "jade" | "violet";
  metric: OtherMetric;
  value: number;
};

// 默认使用成品御魂口径；关闭“只计算 +15”后会放宽读取条件，便于比较未满级库存。
const PVE_MAX_SOUL_LEVEL = 15;

type OutputValues = Record<string, [number, number, number, number]>;

type OutputSelections = Record<string, Record<string, PveSoulDetail[]>>;

type SoulDetailSelection = {
  title: string;
  subtitle: string;
  selected: PveSoulDetail[];
  tone: string;
};

// 特殊两件套对应表头四列；编号遵循后端的 300000 + 目录编码规则。
const bossSouls: BossSoul[] = [
  {
    name: "土蜘蛛",
    shortName: "土蜘蛛",
    tone: "blue",
    iconAssetId: 300050,
    subtitle: "持续伤害",
  },
  {
    name: "荒骷髅",
    shortName: "荒骷髅",
    tone: "green",
    iconAssetId: 300052,
    subtitle: "目标增伤",
  },
  {
    name: "鬼灵歌伎",
    shortName: "鬼灵歌伎",
    tone: "orange",
    iconAssetId: 300077,
    subtitle: "多段攻击",
  },
  {
    name: "天羽羽斩",
    shortName: "天羽羽斩",
    tone: "violet",
    iconAssetId: 300095,
    subtitle: "无视防御",
  },
];

// 每行代表一套输出四件套；无缓存时显示静态占位，首次计算后替换为当前库存结果。
const outputSouls: OutputSoul[] = [
  {
    setId: "狂骨",
    name: "狂骨",
    mark: "狂",
    tone: "red",
    iconAssetId: 300048,
    note: "鬼火越多，伤害越高",
    values: [20124, 19890, 19960, 0],
  },
  {
    setId: "隐念",
    name: "隐念",
    mark: "隐",
    tone: "violet",
    iconAssetId: 300086,
    note: "多段连续攻击增伤",
    values: [19380, 19870, 20120, 0],
  },
  {
    setId: "片叶",
    name: "片叶",
    mark: "片",
    tone: "jade",
    iconAssetId: 300055,
    note: "满血状态下增伤",
    values: [18840, 19120, 19090, 0],
  },
  {
    setId: "破势",
    name: "破势",
    mark: "破",
    tone: "gold",
    iconAssetId: 300030,
    note: "高血目标先手增伤",
    values: [19680, 19430, 19390, 0],
  },
  {
    setId: "心眼",
    name: "心眼",
    mark: "心",
    tone: "rose",
    iconAssetId: 300022,
    note: "目标生命越低，伤害越高",
    values: [19480, 19390, 19560, 0],
  },
  {
    setId: "针女",
    name: "针女",
    mark: "针",
    tone: "blue",
    iconAssetId: 300036,
    note: "多段攻击追加伤害",
    values: [19120, 20340, 20480, 0],
  },
  {
    setId: "兵主部",
    name: "兵主部",
    mark: "兵",
    tone: "ink",
    iconAssetId: 300074,
    note: "叠层后无视防御",
    values: [19420, 19280, 19710, 0],
  },
  {
    setId: "网切",
    name: "网切",
    mark: "网",
    tone: "rose",
    iconAssetId: 300026,
    note: "概率无视部分防御",
    values: [18960, 19040, 19330, 0],
  },
  {
    setId: "伤魂鸟",
    name: "伤魂鸟",
    mark: "伤",
    tone: "rose",
    iconAssetId: 300029,
    note: "击杀后持续提升伤害",
    values: [18740, 19080, 19260, 0],
  },
  {
    setId: "镇墓兽",
    name: "镇墓兽",
    mark: "镇",
    tone: "gold",
    iconAssetId: 300031,
    note: "生命降低时提高暴伤",
    values: [19460, 19220, 19380, 0],
  },
  {
    setId: "无刀取",
    name: "无刀取",
    mark: "无",
    tone: "ink",
    iconAssetId: 300092,
    note: "回合结束后持续提升伤害",
    values: [19540, 19460, 19620, 0],
  },
  {
    setId: "恶楼",
    name: "恶楼",
    mark: "恶",
    tone: "violet",
    iconAssetId: 300081,
    note: "前八回合封禁，之后大幅增伤",
    values: [19520, 19480, 19640, 0],
  },
  {
    setId: "海月火玉",
    name: "海月火玉",
    mark: "海",
    tone: "blue",
    iconAssetId: 300083,
    note: "消耗鬼火换取伤害",
    values: [19820, 20040, 20110, 0],
  },
];

// 其他方案使用独立卡片展示，避免与四列特殊两件套矩阵混淆；无缓存时保留静态占位。
const otherPlans: OtherPlan[] = [
  {
    id: "inaba-scattered",
    shikigami: "因幡辉夜姬（SP辉夜姬）",
    shikigamiId: 372,
    shikigamiMark: "因",
    soul: "散件",
    soulIconAssetIds: [],
    statLabel: "任意组合 · 爆伤 ≥380%",
    tone: "blue",
    requirement: "散件或套装组合 · 自动计入两件套属性 · 6号位爆伤",
    metric: "percent",
    value: 19804,
  },
  {
    id: "inaba-fire",
    shikigami: "因幡辉夜姬（SP辉夜姬）",
    shikigamiId: 372,
    shikigamiMark: "因",
    soul: "火灵",
    soulIconAssetIds: [300019],
    statLabel: "爆伤 ≥360%",
    tone: "gold",
    requirement: "6号位爆伤 · 暴击不限 · 主要承担打火和爆伤辅助",
    metric: "percent",
    value: 19632,
  },
  {
    id: "miketsu-mountain",
    shikigami: "不见岳",
    shikigamiId: 379,
    shikigamiMark: "不",
    soul: "防御 2+2+2",
    soulIconAssetIds: [],
    statLabel: "防御 ≥2400",
    tone: "jade",
    requirement: "2/4/6号位防御 · 2+2+2 · 三组两件套各+30%防御",
    metric: "defense",
    value: 12840,
  },
  {
    id: "sp-yuan-fire",
    shikigami: "纺愿缘结神（SP）",
    shikigamiId: 554,
    shikigamiMark: "缘",
    soul: "火灵 4 + 任意 2",
    soulIconAssetIds: [300019],
    statLabel: "满暴 · 最高爆伤",
    tone: "gold",
    requirement: "火灵 4 件 + 其他 2 件不限定套装 · 2号位速度 · 全库存搜索",
    metric: "percent",
    value: 19742,
  },
  {
    id: "sp-yuan-reminiscence",
    shikigami: "纺愿缘结神（SP）",
    shikigamiId: 554,
    shikigamiMark: "缘",
    soul: "遗念火",
    soulIconAssetIds: [300079],
    statLabel: "满暴 · 275%爆伤",
    tone: "blue",
    requirement: "遗念火 4 件 · 2号位速度 · 速度按阵容配速",
    metric: "percent",
    value: 19586,
  },
  {
    id: "sijin-attack",
    shikigami: "思金神",
    shikigamiId: 601,
    shikigamiMark: "思",
    soul: "隐念 + 歌姬",
    soulIconAssetIds: [300086, 300077],
    statLabel: "纯攻击",
    tone: "violet",
    requirement: "隐念四件套 · 歌姬首领御魂",
    metric: "attack",
    value: 20416,
  },
];

// 结果缓存包含版本号；规则、展示结构或详情数据升级时通过递增版本自动丢弃旧数据。
const SOUL_PAGE_SIZE = 100;
// PVE 计算必须使用已校验的活动目录，避免目录更新后仍按旧版本分类漏算两件套。
const EXPECTED_CATALOG_SET_COUNT = 70;
// 须佐之男的头像 ID 沿用式神图鉴目录的稳定编号，避免页面重复维护图片路径。
const MAIN_SHIKIGAMI_ID = 389;
const hasCalculated = ref(false);
const calculationBusy = ref(false);
// 默认只计算 +15；关闭后读取并搜索全部六星御魂。
const onlyMaxLevel = ref(true);
const calculationMode = ref<"foreground" | "background" | null>(null);
const calculationProgress = ref(0);
const calculationMessage = ref("准备读取已导入御魂…");
const calculationError = ref("");
const calculationNotice = ref("");
const calculatedOutputValues = ref<OutputValues | null>(null);
const calculatedOtherValues = ref<Record<string, number> | null>(null);
const calculatedOutputSelections = ref<OutputSelections | null>(null);
const calculatedOtherSelections = ref<Record<string, PveSoulDetail[]> | null>(null);
const calculatedSetIconIds = ref<Record<string, number> | null>(null);
// Worker 生命周期和运行编号用于取消旧计算，避免旧任务完成后覆盖新任务的页面结果。
let calculationWorker: Worker | null = null;
let rejectActiveCalculation: (() => void) | null = null;
let calculationRunId = 0;
// 任务 ID 属于调度器，不随当前页面实例销毁；页面只保存自己订阅的 ID。
let activePveTaskId: string | null = null;
let stopTaskListening: (() => void) | null = null;
let stopActiveCharacterChanged: (() => void) | null = null;
// 已完成项只保存短消息，控制遮罩层展示体积；完整组合仍保存在增量结果和最终缓存中。
const calculationCompletedItems = ref<string[]>([]);
const calculationTotalItems = ref(0);
let noticeTimer: number | null = null;
// 当前打开的六位置详情；同一时间只展示一个方案，避免多个浮层互相遮挡。
const soulDetailSelection = ref<SoulDetailSelection | null>(null);
// 详情阅读顺序按游戏面板排布：1、6、2、5、3、4，而不是数字自然顺序。
const soulSlots = [1, 6, 2, 5, 3, 4];
const detailSlotSouls = computed(() =>
  selectedSoulsBySlot(soulDetailSelection.value?.selected ?? []),
);

/** 从离线资源表读取御魂图标；资源缺失时由模板回退到单字印记。 */
function soulIcon(iconAssetId: number): string | null {
  return iconModules[`../assets/soul-icons/${iconAssetId}.png`] ?? null;
}

/** 读取详情卡片中的套装图标；缓存映射优先，当前计算目录映射作为兜底。 */
function selectedSoulIcon(setId: string): string | null {
  const iconAssetId = calculatedSetIconIds.value?.[setId];
  return iconAssetId === undefined ? null : soulIcon(iconAssetId);
}

/** 复用式神图鉴的 public 头像目录，头像编号直接对应图鉴的稳定 shikigamiId。 */
function shikigamiAvatar(shikigamiId: number): string {
  return `${import.meta.env.BASE_URL}shikigami-avatars/${shikigamiId}.webp`;
}

/** 头像文件异常时隐藏图片，让卡片继续显示已有的单字兜底标记。 */
function hideBrokenAvatar(event: Event): void {
  const image = event.currentTarget;
  if (image instanceof HTMLImageElement) image.hidden = true;
}

/** 将结果格式化为不带千位分隔符的紧凑整数，避免长数字被逗号切断视觉节奏。 */
function formatMetric(value: number): string {
  return String(Math.round(value));
}

/** 只用有结果的特殊两件套计算平均值；无结果时返回 0，避免无效结果稀释有效方案。 */
function averageValue(values: [number, number, number, number]): number {
  const availableValues = values.filter((value) => value !== 0);
  if (availableValues.length === 0) return 0;
  return Math.round(
    availableValues.reduce((total, value) => total + value, 0) / availableValues.length,
  );
}

const averageMetric = computed(() => {
  // 没有任何有效组合的输出套装不参与总平均，保持总平均与行平均的除数口径一致。
  const availableAverages = outputSouls
    .map((soul) => averageValue(outputValuesFor(soul)))
    .filter((value) => value !== 0);
  if (availableAverages.length === 0) return 0;
  return Math.round(
    availableAverages.reduce((total, value) => total + value, 0) /
      availableAverages.length,
  );
});

/** 已计算时优先使用缓存/本次结果，未计算时保留原型中的静态占位值。 */
function outputValuesFor(soul: OutputSoul): [number, number, number, number] {
  return calculatedOutputValues.value?.[soul.name] ?? soul.values;
}

/** 返回输出矩阵某一列对应的实际六件御魂；列名必须与计算缓存使用的特殊两件套名称一致。 */
function outputSelectionFor(
  soul: OutputSoul,
  boss: BossSoul | undefined,
): PveSoulDetail[] | undefined {
  if (!boss) return undefined;
  return calculatedOutputSelections.value?.[soul.name]?.[boss.name];
}

/** 其他卡片的数值在计算完成后来自真实库存组合，进入页面前只显示静态占位。 */
function otherValueFor(plan: OtherPlan): number {
  return calculatedOtherValues.value?.[plan.id] ?? plan.value;
}

/** 返回“其他”方案卡片对应的实际六件御魂，缓存缺失时按钮保持禁用而不伪造组合。 */
function otherSelectionFor(plan: OtherPlan): PveSoulDetail[] | undefined {
  return calculatedOtherSelections.value?.[plan.id];
}

/** 展示方案规则本身；第二套由搜索器从剩余御魂中择优，不把某次结果误显示成固定条件。 */
function otherSoulLabel(plan: OtherPlan): string {
  return plan.soul;
}

/** 将属性稳定键转换成玩家熟悉的中文名称；未知键保留原值，方便核对异常导入数据。 */
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

/** 按“我的御魂”页面的单位规则格式化主属性和副属性数值。 */
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

/** 按详情面板约定的 1、6、2、5、3、4 顺序排列；缺少位置时保留空位。 */
function selectedSoulsBySlot(
  selected: PveSoulDetail[],
): Array<PveSoulDetail | undefined> {
  return soulSlots.map((slot) => selected.find((soul) => soul.slot === slot));
}

/** 展示详情按钮对应的六个位置，标题中明确这是哪套首领/辅助方案的实际组合。 */
function openSoulDetails(
  title: string,
  subtitle: string,
  selected: PveSoulDetail[] | undefined,
  tone: string,
): void {
  soulDetailSelection.value = {
    title,
    subtitle,
    selected: selected ?? [],
    tone,
  };
}

/** 关闭当前六位置详情，供遮罩、关闭按钮和 Escape 键复用。 */
function closeSoulDetails(): void {
  soulDetailSelection.value = null;
}

/** 详情浮层只响应自己的 Escape，不影响页面其他键盘交互。 */
function handleSoulDetailKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape" && soulDetailSelection.value) closeSoulDetails();
}

/** 百分比型毕业指标在卡片中保留百分号，攻击/防御仍按整数面板显示。 */
function formatOtherValue(plan: OtherPlan): string {
  return plan.metric === "percent"
    ? `${formatMetric(otherValueFor(plan))}%`
    : formatMetric(otherValueFor(plan));
}

/** 将 Worker 上报的毫秒耗时转换为更适合任务日志阅读的秒数，并固定保留一位小数。 */
function formatCalculationDuration(durationMs: number): string {
  return `${(durationMs / 1000).toFixed(1)} 秒`;
}

/** 计算前明确展示筛选口径；勾选时后端直接排除非 +15 御魂。 */
const calculationFilterDescription = computed(() =>
  onlyMaxLevel.value
    ? "计算前剔除非 +15 御魂，仅统计六星 +15。"
    : "不剔除非 +15 御魂，统计全部六星。",
);

/** 计算完成后提供短暂页面通知，并通过 aria-live 让辅助技术用户同步获知。 */
function showCalculationNotice(message: string): void {
  calculationNotice.value = message;
  if (noticeTimer !== null) window.clearTimeout(noticeTimer);
  noticeTimer = window.setTimeout(() => {
    calculationNotice.value = "";
    noticeTimer = null;
  }, 6000);
}

/** 将后台 Worker 的单项结果合并到页面已有结果，刷新时也能持续看到新完成的方案。 */
function applyPveProgress(
  progress: PveCalculationProgress,
  context: TaskExecutionContext,
): void {
  // 组合完成消息同时展示实际候选数量和耗时，便于区分筛选慢还是组合枚举慢。
  const detailMessage = `${progress.message} · 候选御魂 ${progress.candidateCount} 枚 · 耗时 ${formatCalculationDuration(progress.durationMs)}`;
  calculationTotalItems.value = progress.total;
  calculationProgress.value =
    56 + Math.round((progress.completed / Math.max(1, progress.total)) * 42);
  calculationMessage.value = detailMessage;
  context.report({
    phase: "组合搜索",
    completed: progress.completed,
    total: progress.total,
    message: detailMessage,
  });
  context.log(detailMessage);
  if (!calculationCompletedItems.value.includes(detailMessage)) {
    calculationCompletedItems.value = [
      ...calculationCompletedItems.value,
      detailMessage,
    ];
  }

  if ("bossId" in progress.item) {
    const item = progress.item;
    const values = calculatedOutputValues.value
      ? { ...calculatedOutputValues.value }
      : {};
    const current = [...(values[item.planId] ?? [0, 0, 0, 0])] as [
      number,
      number,
      number,
      number,
    ];
    const bossIndex = bossSouls.findIndex((boss) => boss.name === item.bossId);
    if (bossIndex >= 0) current[bossIndex] = item.value;
    values[item.planId] = current;
    calculatedOutputValues.value = values;

    const selections = calculatedOutputSelections.value
      ? { ...calculatedOutputSelections.value }
      : {};
    selections[item.planId] = {
      ...(selections[item.planId] ?? {}),
      [item.bossId]: item.selection,
    };
    calculatedOutputSelections.value = selections;
    return;
  }

  const item = progress.item;
  calculatedOtherValues.value = {
    ...(calculatedOtherValues.value ?? {}),
    [item.planId]: item.value,
  };
  if (item.selection) {
    calculatedOtherSelections.value = {
      ...(calculatedOtherSelections.value ?? {}),
      [item.planId]: item.selection,
    };
  }
}

/** 运行一次 Worker 计算；主线程只接收进度，不执行组合枚举。 */
function runPveWorker(
  souls: MySoul[],
  catalogSets: SoulSet[],
  runId: number,
  context: TaskExecutionContext,
): Promise<PveCalculationResult> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(
      new URL("../workers/pveCalculation.worker.ts", import.meta.url),
      { type: "module" },
    );
    let settled = false;

    const cleanup = (): void => {
      worker.terminate();
      if (calculationWorker === worker) calculationWorker = null;
      if (rejectActiveCalculation) rejectActiveCalculation = null;
    };
    const finishResolve = (result: PveCalculationResult): void => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(result);
    };
    const finishReject = (error: Error): void => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(error);
    };
    rejectActiveCalculation = () =>
      finishReject(new Error("PVE_CALCULATION_CANCELLED"));

    worker.onmessage = (event: MessageEvent<PveWorkerResponse>): void => {
      if (runId !== calculationRunId) return;
      if (event.data.type === "progress") {
        applyPveProgress(event.data.progress, context);
      } else if (event.data.type === "log") {
        // Worker 只负责搜索；组合开始日志交回调度器，确保任务中心保留完整的阶段轨迹。
        context.log(event.data.message);
      } else if (event.data.type === "completed") {
        finishResolve(event.data.result);
      } else {
        finishReject(new Error(event.data.message));
      }
    };
    worker.onerror = (event): void => {
      finishReject(new Error(event.message || "PVE 计算线程异常结束"));
    };
    calculationWorker = worker;
    // Worker 只接收普通对象；逐层展开御魂和属性，避免 Vue 响应式代理触发结构化克隆失败。
    const transferableSouls = souls.map((soul) => ({
      ...soul,
      attributes: soul.attributes.map((attribute) => ({ ...attribute })),
    }));
    const transferableCatalogSets = catalogSets.map((set) => ({ ...set }));
    worker.postMessage({
      type: "calculate",
      souls: transferableSouls,
      catalogSets: transferableCatalogSets,
    });
  });
}

/** 只负责终止当前实例创建的 Worker；任务终态和取消入口由统一调度器收敛。 */
function stopPveWorker(): void {
  calculationRunId += 1;
  const cancel = rejectActiveCalculation;
  rejectActiveCalculation = null;
  cancel?.();
  calculationWorker?.terminate();
  calculationWorker = null;
  calculationBusy.value = false;
  calculationMode.value = null;
  calculationMessage.value = "计算已取消";
  calculationProgress.value = 0;
}

/** 取消当前 PVE 任务；任务中心和库存清空入口都复用同一个调度器动作。 */
function cancelPveCalculation(): void {
  if (activePveTaskId) {
    taskScheduler.cancel(activePveTaskId);
    return;
  }
  if (calculationBusy.value) stopPveWorker();
}

/** 同步任务中心快照，让重新进入 PVE 页面时能恢复正在进行的任务提示。 */
function handlePveTaskSnapshot(tasks: ScheduledTask[]): void {
  const running = tasks.find(
    (task) => task.kind === "pve-calculation" && task.status === "running",
  );
  if (!activePveTaskId && running) {
    activePveTaskId = running.id;
    if (typeof running.metadata.onlyMaxLevel === "boolean") {
      onlyMaxLevel.value = running.metadata.onlyMaxLevel;
    }
  }
  if (!activePveTaskId) return;
  const current = tasks.find((task) => task.id === activePveTaskId);
  if (!current) return;
  calculationProgress.value = current.total
    ? Math.min(100, Math.round((current.completed / current.total) * 42) + 56)
    : calculationProgress.value;
  calculationTotalItems.value = current.total;
  calculationMessage.value = current.message;
  if (current.status === "running") {
    calculationBusy.value = true;
    calculationMode.value =
      current.metadata.mode === "background" ? "background" : "foreground";
    return;
  }
  calculationBusy.value = false;
  calculationMode.value = null;
  if (current.status === "completed") {
    // 缓存写入失败时保留当前页面内存中的新结果，不能因恢复不到缓存而清空刚完成的计算。
    const restored = restoreCachedResult(activeCharacterId.value, true);
    showCalculationNotice(
      restored
        ? current.metadata.mode === "background"
          ? "PVE 后台计算完成，结果已保存。"
          : "PVE 计算完成，结果已保存。"
        : "PVE 计算完成，但本地缓存空间不足，本次结果仅保留在当前页面。",
    );
  } else if (current.status === "failed") {
    calculationError.value = current.error ?? current.message;
  }
  activePveTaskId = null;
}

/** 调度器真正执行的 PVE 任务：读取已过滤库存、准备目录、运行 Worker 并写入轻量缓存。 */
async function executePveCalculation(
  mode: "foreground" | "background",
  context: TaskExecutionContext,
  selectedOnlyMaxLevel: boolean,
  profileId: string,
): Promise<void> {
  const runId = ++calculationRunId;
  calculationBusy.value = true;
  calculationMode.value = mode;
  calculationError.value = "";
  calculationNotice.value = "";
  calculationProgress.value = 4;
  calculationMessage.value = `正在读取${selectedOnlyMaxLevel ? " +15" : "全部等级"}御魂…`;
  calculationCompletedItems.value = [];
  calculationTotalItems.value = 0;
  context.onCancel(stopPveWorker);
  context.report({ phase: "读取库存", message: calculationMessage.value });
  context.log(calculationMessage.value);

  try {
    if (context.isCancelled()) return;
    // 六星条件始终下推给数据库；关闭“只计算 +15”时仅放宽等级，不放宽星级。
    const listSoulsPage = (offset: number) =>
      desktopApi.listMySouls(
        selectedOnlyMaxLevel
          ? {
              limit: SOUL_PAGE_SIZE,
              offset,
              quality: 6,
              level: PVE_MAX_SOUL_LEVEL,
            }
          : { limit: SOUL_PAGE_SIZE, offset, quality: 6 },
      );
    const firstPage = await listSoulsPage(0);
    const candidateSouls = [...firstPage.items];
    context.report({
      phase: "读取库存",
      completed: candidateSouls.length,
      total: firstPage.total,
      message: `正在读取${selectedOnlyMaxLevel ? " +15" : "全部等级"}御魂… ${candidateSouls.length} / ${firstPage.total}`,
    });
    const totalPages = Math.max(1, Math.ceil(firstPage.total / SOUL_PAGE_SIZE));
    for (let page = 1; page < totalPages; page += 1) {
      if (context.isCancelled()) return;
      const result = await listSoulsPage(page * SOUL_PAGE_SIZE);
      if (context.isCancelled()) return;
      candidateSouls.push(...result.items);
      calculationProgress.value = Math.min(
        52,
        4 + Math.round((page / totalPages) * 48),
      );
      calculationMessage.value = `正在读取${selectedOnlyMaxLevel ? " +15" : "全部等级"}御魂… ${candidateSouls.length} / ${firstPage.total}`;
      context.report({
        phase: "读取库存",
        completed: candidateSouls.length,
        total: firstPage.total,
        message: calculationMessage.value,
      });
    }

    if (runId !== calculationRunId || context.isCancelled()) return;
    // 分页读取结束后写入实际交给组合搜索的数量，避免只看到“读取完成”却无法判断计算范围。
    context.log(
      `已读取 ${candidateSouls.length} 个${selectedOnlyMaxLevel ? " +15" : ""}御魂`,
    );
    calculationMessage.value = "正在读取御魂套装目录…";
    calculationProgress.value = 54;
    context.log(calculationMessage.value);
    context.report({ phase: "准备目录", message: calculationMessage.value });
    let catalog = await desktopApi.getCatalogStatus();
    if (
      !catalog ||
      catalog.setCount < EXPECTED_CATALOG_SET_COUNT ||
      !["verified", "partial"].includes(catalog.validationStatus)
    ) {
      catalog = await desktopApi.installBuiltinCatalog();
    }
    let catalogSets = await desktopApi.listSoulSets(catalog.version);
    if (
      catalogSets.length < EXPECTED_CATALOG_SET_COUNT ||
      catalogSets.some((set) => !set.specialCategory && !set.category)
    ) {
      catalog = await desktopApi.installBuiltinCatalog();
      catalogSets = await desktopApi.listSoulSets(catalog.version);
    }

    calculationMessage.value = "已开始后台组合搜索…";
    calculationProgress.value = 56;
    context.log(calculationMessage.value);
    context.report({ phase: "组合搜索", message: calculationMessage.value });
    const result = await runPveWorker(candidateSouls, catalogSets, runId, context);
    if (runId !== calculationRunId || context.isCancelled()) return;

    calculatedOutputValues.value = result.outputValues;
    calculatedOtherValues.value = result.otherValues;
    calculatedOutputSelections.value = result.outputSelections;
    calculatedOtherSelections.value = result.otherSelections;
    calculatedSetIconIds.value = result.setIconIds;
    // PVE 的结果依赖本次参与搜索的御魂集合；缓存只保存稳定键和去重后的详情，避免重复写入完整御魂对象。
    const cacheResult = createPveCalculationCache(
      result,
      profileId,
      candidateSouls.map((soul) => soul.soulKey),
      selectedOnlyMaxLevel,
    );
    const storageKey = pveCalculationStorageKey(profileId);
    const cacheSaved = storageKey
      ? savePveCalculationCache(window.localStorage, storageKey, cacheResult)
      : false;
    for (const outputSoul of outputSouls) {
      const values = result.outputValues[outputSoul.name];
      if (values) outputSoul.values = values;
    }
    for (const plan of otherPlans) plan.value = result.otherValues[plan.id] ?? 0;
    calculationProgress.value = 100;
    calculationMessage.value = cacheSaved
      ? `计算完成，已保存 ${candidateSouls.length} 枚${selectedOnlyMaxLevel ? " +15" : ""}御魂的结果`
      : `计算完成，但本地缓存空间不足；${candidateSouls.length} 枚${selectedOnlyMaxLevel ? " +15" : ""}御魂的结果仅保留在当前页面`;
    context.report({
      phase: "已完成",
      completed: 100,
      total: 100,
      message: calculationMessage.value,
    });
    context.log(calculationMessage.value, cacheSaved ? "success" : "warning");
    hasCalculated.value = true;
    showCalculationNotice(
      cacheSaved
        ? mode === "background"
          ? "PVE 后台计算完成，结果已保存。"
          : "PVE 计算完成，结果已保存。"
        : "PVE 计算完成，但本地缓存空间不足，本次结果仅保留在当前页面。",
    );
  } catch (caught) {
    if (runId !== calculationRunId) return;
    if (caught instanceof Error && caught.message === "PVE_CALCULATION_CANCELLED") {
      calculationMessage.value = "计算已取消";
      return;
    }
    const message = normalizeCommandError(caught).message;
    calculationError.value = message;
    calculationMessage.value = "计算失败，请稍后重试";
    context.log(message, "error");
    throw new Error(message, { cause: caught });
  } finally {
    if (runId === calculationRunId) {
      calculationBusy.value = false;
      calculationMode.value = null;
    }
  }
}

/** 用户点击计算时只创建调度任务；执行 Promise、Worker 和取消动作均由调度器持有。 */
function calculatePvePanel(mode: "foreground" | "background" = "foreground"): void {
  if (calculationBusy.value || taskScheduler.findRunning("pve-calculation")) return;
  const profileId = activeCharacterId.value;
  if (!profileId) {
    calculationError.value = "尚未选择角色档案，无法保存 PVE 结果";
    return;
  }
  const selectedOnlyMaxLevel = onlyMaxLevel.value;
  calculationBusy.value = true;
  calculationMode.value = mode;
  activePveTaskId = taskScheduler.start({
    kind: "pve-calculation",
    title: mode === "background" ? "PVE 建议（后台）" : "PVE 建议计算",
    metadata: { mode, onlyMaxLevel: selectedOnlyMaxLevel, profileId },
    run: (context) =>
      executePveCalculation(mode, context, selectedOnlyMaxLevel, profileId),
  });
  handlePveTaskSnapshot(taskScheduler.snapshot());
}

/** 读取当前角色带等级口径的缓存结果；缓存存在时不再访问御魂接口。 */
function restoreCachedResult(
  profileId: string | null = activeCharacterId.value,
  preserveExisting = false,
): boolean {
  const storageKey = pveCalculationStorageKey(profileId);
  if (!preserveExisting) clearCachedResult();
  if (!storageKey || !profileId) return false;
  try {
    const raw = window.localStorage.getItem(storageKey);
    if (!raw) return false;
    const cached = JSON.parse(raw) as PveCalculationCache;
    if (
      cached.version !== 28 ||
      cached.profileId !== profileId ||
      !Array.isArray(cached.soulKeys) ||
      typeof cached.onlyMaxLevel !== "boolean"
    ) {
      window.localStorage.removeItem(storageKey);
      return false;
    }
    const restoredSelections = expandPveCalculationSelections(cached);
    if (!restoredSelections) {
      window.localStorage.removeItem(storageKey);
      return false;
    }
    calculatedOutputValues.value = cached.outputValues;
    calculatedOtherValues.value = cached.otherValues;
    calculatedOutputSelections.value = restoredSelections.outputSelections;
    calculatedOtherSelections.value = restoredSelections.otherSelections;
    calculatedSetIconIds.value = cached.setIconIds;
    for (const outputSoul of outputSouls) {
      const values = cached.outputValues[outputSoul.name];
      if (values?.length === 4) outputSoul.values = values;
    }
    for (const plan of otherPlans) {
      const cachedValue = cached.otherValues[plan.id];
      if (typeof cachedValue === "number") plan.value = cachedValue;
    }
    hasCalculated.value = true;
    onlyMaxLevel.value = cached.onlyMaxLevel;
    calculationMessage.value = `已读取缓存 · ${cached.inventoryCount} 枚${cached.onlyMaxLevel ? " +15" : ""}御魂 · ${new Date(cached.calculatedAt).toLocaleString("zh-CN")}`;
    return true;
  } catch {
    window.localStorage.removeItem(storageKey);
    return false;
  }
}

/** 清理当前页面仍持有的旧角色结果，切换角色或库存后先回到可重新计算的空态。 */
function clearCachedResult(): void {
  hasCalculated.value = false;
  calculatedOutputValues.value = null;
  calculatedOtherValues.value = null;
  calculatedOutputSelections.value = null;
  calculatedOtherSelections.value = null;
  calculatedSetIconIds.value = null;
  closeSoulDetails();
  calculationError.value = "";
  calculationNotice.value = "";
}

// 进入页面只读本地缓存；没有缓存时保留“计算建议”按钮，不触发御魂全量扫描。
function handleInventoryCleared(): void {
  cancelPveCalculation();
  clearCachedResult();
}

/** 角色切换时停止旧角色计算，并恢复新角色自己的 PVE 缓存。 */
function handleActiveCharacterChanged(profileId: string | null): void {
  if (activePveTaskId) taskScheduler.cancel(activePveTaskId);
  clearCachedResult();
  if (profileId) restoreCachedResult(profileId);
}

onMounted(() => {
  restoreCachedResult();
  // 调度器单例跨页面保留任务；当前实例只订阅快照，不接管任务生命周期。
  stopTaskListening = taskScheduler.subscribe(handlePveTaskSnapshot);
  window.addEventListener(MY_SOULS_INVENTORY_CLEARED_EVENT, handleInventoryCleared);
  stopActiveCharacterChanged = onActiveCharacterChanged(handleActiveCharacterChanged);
  window.addEventListener("keydown", handleSoulDetailKeydown);
});

onBeforeUnmount(() => {
  stopTaskListening?.();
  stopTaskListening = null;
  stopActiveCharacterChanged?.();
  if (noticeTimer !== null) window.clearTimeout(noticeTimer);
  window.removeEventListener(MY_SOULS_INVENTORY_CLEARED_EVENT, handleInventoryCleared);
  window.removeEventListener("keydown", handleSoulDetailKeydown);
});
</script>

<template>
  <section class="pve-panel" aria-labelledby="pve-panel-title">
    <header class="pve-panel__header">
      <div>
        <p class="pve-panel__eyebrow">PVE DAMAGE PREVIEW</p>
        <h2 id="pve-panel-title">PVE 数据预览 <span aria-hidden="true">?</span></h2>
        <p class="pve-panel__lede">须佐之男 · 满暴 · 仅展示攻击 × 暴伤</p>
      </div>
      <div class="pve-panel__header-meta">
        <span class="pve-panel__shikigami-mark">
          <img
            :src="shikigamiAvatar(MAIN_SHIKIGAMI_ID)"
            alt=""
            @error="hideBrokenAvatar"
          />
          <span>须</span>
        </span>
        <span><b>须佐之男</b><small>固定计算式神</small></span>
      </div>
    </header>

    <div class="pve-panel__toolbar">
      <div class="pve-panel__rule-tags" aria-label="计算规则">
        <span>暴击 100%</span>
        <span>攻击 × 暴伤</span>
        <span>2/4号位：攻击</span>
        <span>6号位：暴伤 / 暴击</span>
        <span>首次计算后缓存</span>
      </div>
      <div class="pve-panel__toolbar-action">
        <button
          class="button button--quiet pve-panel__background-button"
          type="button"
          :disabled="calculationBusy"
          aria-label="后台计算 PVE 建议"
          title="后台计算 PVE 建议"
          @click="calculatePvePanel('background')"
        >
          {{
            calculationBusy && calculationMode === "background"
              ? "后台计算中…"
              : "后台计算"
          }}
        </button>
        <template v-if="hasCalculated">
          <span class="pve-panel__saved-state">已计算 · 自动保留</span>
          <!-- 已有缓存时允许用户主动重新读取当前库存，按钮沿用计算入口避免出现两套规则。 -->
          <button
            class="pve-panel__refresh-button"
            type="button"
            :disabled="calculationBusy"
            aria-label="重新计算 PVE 建议"
            title="重新计算 PVE 建议"
            @click="calculatePvePanel('foreground')"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M20 11a8 8 0 0 0-14.9-3.9L3 9m0 0V4m0 5h5" />
              <path d="M4 13a8 8 0 0 0 14.9 3.9L21 15m0 0v5m0-5h-5" />
            </svg>
          </button>
        </template>
        <button
          v-else
          class="button button--primary pve-panel__calculate-button"
          type="button"
          :disabled="calculationBusy"
          @click="calculatePvePanel('foreground')"
        >
          计算
        </button>
      </div>
    </div>

    <div class="pve-panel__calculation-options" aria-label="PVE 御魂计算范围">
      <label>
        <input v-model="onlyMaxLevel" type="checkbox" :disabled="calculationBusy" />
        <span>只计算 +15</span>
      </label>
      <small>{{ calculationFilterDescription }}</small>
    </div>

    <p
      v-if="calculationNotice"
      class="pve-panel__calculation-notice"
      role="status"
      aria-live="polite"
    >
      {{ calculationNotice }}
    </p>

    <div v-if="calculationError" class="pve-panel__calculation-error" role="alert">
      {{ calculationError }}
    </div>

    <div
      v-if="!hasCalculated && !calculationBusy"
      class="pve-panel__precalculate"
      role="status"
    >
      <span class="pve-panel__precalculate-mark" aria-hidden="true">御</span>
      <strong>尚未计算 PVE 建议</strong>
      <p>
        点击上方“计算建议”或“后台计算”读取当前库存，完成后才展示输出和其他方案数据。
      </p>
    </div>

    <p
      v-if="calculationBusy && calculationMode === 'background'"
      class="pve-panel__background-status"
      role="status"
    >
      正在后台计算 PVE 建议 · {{ calculationFilterDescription }} ·
      {{ calculationMessage }}
    </p>

    <template v-if="hasCalculated">
      <section class="pve-preview-card" aria-labelledby="pve-matrix-title">
        <div class="pve-preview-card__heading">
          <div>
            <span class="pve-panel__section-kicker"
              >SET COMPARISON / 13 OUTPUT SETS</span
            >
            <h3 id="pve-matrix-title">御魂方案对比</h3>
          </div>
          <p>横向比较特殊两件套，纵向比较输出四件套</p>
        </div>

        <div class="pve-matrix-scroll">
          <div class="pve-matrix" role="table" aria-label="PVE 御魂方案对比表">
            <div class="pve-matrix__row pve-matrix__row--head" role="row">
              <div class="pve-matrix__corner" role="columnheader">
                <span>输出御魂</span>
                <small>攻击 × 暴伤</small>
              </div>
              <div
                v-for="boss in bossSouls"
                :key="boss.name"
                class="pve-matrix__boss-head"
                :class="`pve-matrix__boss-head--${boss.tone}`"
                role="columnheader"
              >
                <span class="pve-matrix__boss-icon">
                  <img
                    v-if="soulIcon(boss.iconAssetId)"
                    :src="soulIcon(boss.iconAssetId)!"
                    alt=""
                  />
                  <span v-else>首</span>
                </span>
                <span class="pve-matrix__boss-name"
                  ><b>{{ boss.shortName }}</b
                  ><small>{{ boss.subtitle }}</small></span
                >
              </div>
              <div class="pve-matrix__average-head" role="columnheader">
                <span>平均值</span>
                <small>四套特殊御魂</small>
              </div>
            </div>

            <div
              v-for="soul in outputSouls"
              :key="soul.name"
              class="pve-matrix__row pve-matrix__row--body"
              role="row"
            >
              <div class="pve-matrix__soul-label" role="rowheader">
                <span
                  class="pve-matrix__soul-icon"
                  :class="`pve-matrix__soul-icon--${soul.tone}`"
                >
                  <img
                    v-if="soulIcon(soul.iconAssetId)"
                    :src="soulIcon(soul.iconAssetId)!"
                    alt=""
                  />
                  <span v-else>{{ soul.mark }}</span>
                </span>
                <span class="pve-matrix__soul-copy"
                  ><b>{{ soul.name }}</b
                  ><small>四件套 · {{ soul.note }}</small></span
                >
              </div>

              <div
                v-for="(value, index) in outputValuesFor(soul)"
                :key="`${soul.name}-${bossSouls[index]?.name}`"
                class="pve-matrix__metric-card"
                role="cell"
              >
                <span class="pve-matrix__metric-label">{{
                  hasCalculated ? "攻击 × 暴伤" : "指标"
                }}</span>
                <strong>{{ hasCalculated ? formatMetric(value) : "暂无" }}</strong>
                <button
                  class="pve-matrix__details-button"
                  type="button"
                  :disabled="!outputSelectionFor(soul, bossSouls[index])"
                  :aria-label="`查看${soul.name} · ${bossSouls[index]?.shortName ?? '首领'}的御魂`"
                  @click="
                    openSoulDetails(
                      `${soul.name} · ${bossSouls[index]?.shortName ?? '首领'}`,
                      '输出方案 · 当前库存快照',
                      outputSelectionFor(soul, bossSouls[index]),
                      soul.tone,
                    )
                  "
                >
                  <svg class="pve-details-icon" viewBox="0 0 24 24" aria-hidden="true">
                    <path
                      d="M2.5 12s3.4-5.5 9.5-5.5 9.5 5.5 9.5 5.5-3.4 5.5-9.5 5.5S2.5 12 2.5 12Z"
                    />
                    <circle cx="12" cy="12" r="2.4" />
                  </svg>
                  查看御魂
                </button>
              </div>

              <div
                class="pve-matrix__metric-card pve-matrix__metric-card--average"
                role="cell"
              >
                <span class="pve-matrix__metric-label">{{
                  hasCalculated ? "综合参考" : "指标"
                }}</span>
                <strong>{{
                  hasCalculated
                    ? formatMetric(averageValue(outputValuesFor(soul)))
                    : "暂无"
                }}</strong>
              </div>
            </div>
          </div>
        </div>

        <div class="pve-preview-card__footer">
          <span class="pve-preview-card__legend"><i></i>结果为当前库存快照</span>
          <span v-if="hasCalculated" class="pve-preview-card__overall"
            >全套平均参考值：<b>{{ formatMetric(averageMetric) }}</b></span
          >
          <span v-else>首次进入请先点击上方“计算建议”</span>
        </div>
      </section>

      <section class="pve-other-section" aria-labelledby="pve-other-title">
        <div class="pve-other-section__heading">
          <div>
            <span class="pve-panel__section-kicker">OTHER BUILDS / SPECIAL USE</span>
            <h3 id="pve-other-title">其他</h3>
          </div>
          <p>不参与输出套装横向平均，单独展示辅助式神方案</p>
        </div>

        <div class="pve-other-grid">
          <article
            v-for="plan in otherPlans"
            :key="`${plan.shikigami}-${plan.soul}`"
            class="pve-other-card"
            :class="`pve-other-card--${plan.tone}`"
          >
            <div class="pve-other-card__title">
              <span class="pve-other-card__soul-icons" aria-hidden="true">
                <span
                  v-for="iconAssetId in plan.soulIconAssetIds"
                  :key="iconAssetId"
                  class="pve-other-card__soul-icon"
                >
                  <img
                    v-if="soulIcon(iconAssetId)"
                    :src="soulIcon(iconAssetId)!"
                    alt=""
                  />
                </span>
                <span
                  v-if="!plan.soulIconAssetIds.length"
                  class="pve-other-card__soul-icon pve-other-card__soul-icon--fallback"
                >
                  {{ plan.soul === "散件" ? "散" : plan.soul.slice(0, 1) }}
                </span>
              </span>
              <strong
                >{{ plan.shikigami }} · {{ otherSoulLabel(plan) }} ·
                {{ plan.statLabel }}</strong
              >
            </div>
            <div class="pve-other-card__body">
              <div class="pve-other-card__shikigami">
                <span class="pve-other-card__shikigami-mark">
                  <img
                    :src="shikigamiAvatar(plan.shikigamiId)"
                    :alt="`${plan.shikigami}头像`"
                    @error="hideBrokenAvatar"
                  />
                  <span>{{ plan.shikigamiMark }}</span>
                </span>
                <span
                  ><small>式神</small><b>{{ plan.shikigami }}</b></span
                >
              </div>
              <div class="pve-other-card__metric">
                <small>{{ plan.requirement }}</small>
                <strong>{{ hasCalculated ? formatOtherValue(plan) : "暂无" }}</strong>
              </div>
            </div>
            <button
              class="pve-other-card__details-button"
              type="button"
              :disabled="!otherSelectionFor(plan)"
              :aria-label="`查看${plan.shikigami} · ${otherSoulLabel(plan)}的御魂`"
              @click="
                openSoulDetails(
                  `${plan.shikigami} · ${otherSoulLabel(plan)}`,
                  `其他方案 · ${plan.statLabel}`,
                  otherSelectionFor(plan),
                  plan.tone,
                )
              "
            >
              <svg class="pve-details-icon" viewBox="0 0 24 24" aria-hidden="true">
                <path
                  d="M2.5 12s3.4-5.5 9.5-5.5 9.5 5.5 9.5 5.5-3.4 5.5-9.5 5.5S2.5 12 2.5 12Z"
                />
                <circle cx="12" cy="12" r="2.4" />
              </svg>
              查看御魂
            </button>
          </article>
        </div>
      </section>

      <aside class="pve-panel__note" aria-label="计算说明">
        <span class="pve-panel__note-icon">i</span>
        <p>
          首次点击“计算建议”后会读取当前库存中的 +15
          御魂并保存结果；后续进入页面直接读取缓存，不再重复扫描。输出方案按满暴条件计算须佐之男的攻击
          × 暴伤。
        </p>
      </aside>
    </template>

    <div
      v-if="soulDetailSelection"
      class="pve-soul-detail-overlay"
      role="presentation"
      @click.self="closeSoulDetails"
    >
      <section
        class="pve-soul-detail-dialog"
        :class="`pve-soul-detail-dialog--${soulDetailSelection.tone}`"
        role="dialog"
        aria-modal="true"
        aria-labelledby="pve-soul-detail-title"
      >
        <header class="pve-soul-detail-dialog__header">
          <div>
            <p class="pve-panel__section-kicker">SELECTED SOULS / 6 SLOTS</p>
            <h3 id="pve-soul-detail-title">{{ soulDetailSelection.title }}</h3>
            <p>{{ soulDetailSelection.subtitle }}</p>
          </div>
          <button
            class="pve-soul-detail-dialog__close"
            type="button"
            aria-label="关闭御魂详情"
            @click="closeSoulDetails"
          >
            ×
          </button>
        </header>

        <div class="pve-soul-detail-list">
          <article
            v-for="(soul, index) in detailSlotSouls"
            :key="soul?.soulKey ?? `empty-${index}`"
            class="pve-soul-detail-item"
            :class="{ 'pve-soul-detail-item--empty': !soul }"
          >
            <span class="pve-soul-detail-item__slot">{{ soulSlots[index] }}号</span>
            <template v-if="soul">
              <span class="pve-soul-detail-item__icon" :title="`${soul.setId}图标`">
                <img
                  v-if="selectedSoulIcon(soul.setId)"
                  :src="selectedSoulIcon(soul.setId)!"
                  alt=""
                />
                <span v-else>{{ soul.setId.slice(0, 1) }}</span>
              </span>
              <div class="pve-soul-detail-item__identity">
                <b class="pve-soul-detail-item__soul-name">{{ soul.setId }}</b>
                <small
                  >{{ soul.quality }}星 · +{{ soul.level }} · {{ soul.soulKey }}</small
                >
              </div>
              <div class="pve-soul-detail-item__attributes">
                <strong
                  >主：{{ attributeLabel(soul.mainAttrType) }}
                  {{
                    formatSoulAttributeValue(soul.mainAttrType, soul.mainAttrValue)
                  }}</strong
                >
                <div
                  v-if="soul.attributes.length"
                  class="pve-soul-detail-item__subattributes"
                >
                  <div
                    v-for="attribute in soul.attributes"
                    :key="`${soul.soulKey}-${attribute.attributeIndex}`"
                    class="pve-soul-detail-item__subattribute"
                  >
                    <span class="pve-soul-detail-item__subattribute-name">
                      {{ attributeLabel(attribute.attributeType) }}
                      {{
                        formatSoulAttributeValue(
                          attribute.attributeType,
                          attribute.value,
                        )
                      }}
                    </span>
                    <strong
                      v-if="attribute.enhancementCount !== null"
                      class="pve-soul-detail-item__enhancement-count"
                    >
                      强化 {{ attribute.enhancementCount }} 次
                    </strong>
                    <small v-else class="pve-soul-detail-item__enhancement-unknown">
                      强化次数未知
                    </small>
                  </div>
                </div>
                <span v-else>副：暂无副属性记录</span>
              </div>
            </template>
            <span v-else class="pve-soul-detail-item__empty-copy"
              >该位置没有满足条件的御魂</span
            >
          </article>
        </div>

        <footer class="pve-soul-detail-dialog__footer">
          <span>只展示本次计算实际选中的御魂</span>
          <button type="button" @click="closeSoulDetails">完成</button>
        </footer>
      </section>
    </div>

    <div
      v-if="calculationBusy && calculationMode === 'foreground'"
      class="pve-calculation-overlay"
      role="status"
      aria-live="polite"
    >
      <div class="pve-calculation-dialog">
        <div class="pve-calculation-dialog__sigil" aria-hidden="true">御</div>
        <p class="pve-calculation-dialog__eyebrow">PVE RECOMMENDATION</p>
        <h3>正在计算毕业建议</h3>
        <p>{{ calculationMessage }}</p>
        <div class="pve-calculation-progress" aria-hidden="true">
          <span :style="{ width: `${calculationProgress}%` }"></span>
        </div>
        <div class="pve-calculation-dialog__meta">
          <span>
            御魂读取与组合搜索
            <template v-if="calculationTotalItems">
              · {{ calculationCompletedItems.length }} / {{ calculationTotalItems }} 项
            </template>
          </span>
          <b>{{ calculationProgress }}%</b>
        </div>
        <ul
          v-if="calculationCompletedItems.length"
          class="pve-calculation-dialog__items"
        >
          <li v-for="item in calculationCompletedItems.slice(-6)" :key="item">
            <span aria-hidden="true">✓</span>{{ item }}
          </li>
        </ul>
        <button
          class="pve-calculation-dialog__cancel"
          type="button"
          @click="cancelPveCalculation"
        >
          取消计算
        </button>
      </div>
    </div>
  </section>
</template>

<style scoped>
/* 页面采用参考图的“列头 + 行卡片 + 平均值”结构，把对比关系放在第一视觉层。 */
.pve-panel {
  --pve-ink: #26374d;
  --pve-muted: #6f7d8d;
  --pve-line: #e0e7ef;
  --pve-soft: #f7f9fc;
  --pve-blue: #3c97f3;
  display: grid;
  gap: 16px;
  padding-bottom: 42px;
  color: var(--pve-ink);
}

.pve-panel__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 24px;
  padding: 6px 4px 19px;
  border-bottom: 1px solid #d9e1ea;
}

.pve-panel__eyebrow,
.pve-panel__section-kicker {
  margin: 0;
  color: var(--pve-blue);
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.14em;
}

.pve-panel__header h2 {
  margin: 7px 0 6px;
  color: var(--pve-ink);
  font-family: Georgia, "Noto Serif SC", serif;
  font-size: clamp(27px, 3vw, 38px);
  letter-spacing: -0.04em;
  line-height: 1.05;
}

.pve-panel__header h2 span {
  display: inline-grid;
  width: 16px;
  height: 16px;
  margin-left: 4px;
  place-items: center;
  border-radius: 50%;
  color: #fff;
  background: #738398;
  font-family: "Microsoft YaHei UI", sans-serif;
  font-size: 10px;
  vertical-align: 6px;
}

.pve-panel__lede {
  margin: 0;
  color: var(--pve-muted);
  font-size: 12px;
}

.pve-panel__header-meta {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 8px 11px 8px 8px;
  border: 1px solid #e0e7ef;
  border-radius: 11px;
  background: #fff;
  box-shadow: 0 5px 16px rgb(65 86 108 / 7%);
}

.pve-panel__header-meta > span:last-child {
  display: grid;
  gap: 3px;
}

.pve-panel__header-meta b {
  font-size: 12px;
}

.pve-panel__header-meta small {
  color: var(--pve-muted);
  font-size: 10px;
}

.pve-panel__shikigami-mark {
  position: relative;
  display: grid;
  width: 38px;
  height: 38px;
  place-items: center;
  border: 1px solid rgb(60 151 243 / 35%);
  border-radius: 10px;
  color: var(--pve-blue);
  background: #eef7ff;
  font-family: "Noto Serif SC", Georgia, serif;
  font-size: 22px;
  font-weight: 900;
  overflow: hidden;
}

.pve-panel__shikigami-mark img,
.pve-other-card__shikigami-mark img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.pve-panel__shikigami-mark > span,
.pve-other-card__shikigami-mark > span {
  display: none;
}

.pve-panel__shikigami-mark:has(img[hidden]) > span,
.pve-other-card__shikigami-mark:has(img[hidden]) > span {
  display: grid;
}

.pve-panel__toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}

.pve-panel__rule-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 7px;
}

.pve-panel__rule-tags span {
  padding: 6px 9px;
  border: 1px solid #dce6f1;
  border-radius: 999px;
  color: #55718e;
  background: #f7fbff;
  font-size: 10px;
}

.pve-panel__toolbar-action {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 8px;
}

.pve-panel__calculate-button {
  min-height: 38px;
  box-shadow: 0 7px 15px rgb(60 151 243 / 17%);
}

/* 后台计算沿用同一个 Worker，但不显示遮罩，让用户可以继续浏览已有 PVE 结果。 */
.pve-panel__background-button {
  min-height: 38px;
  border-color: #b9d4eb;
  color: #3d78a9;
  background: #f5faff;
  font-size: 11px;
  font-weight: 800;
}

.pve-panel__background-button:hover:not(:disabled) {
  border-color: #8eb9dd;
  color: #286391;
  background: #eaf5ff;
}

.pve-panel__saved-state {
  display: inline-flex;
  align-items: center;
  flex: 1 1 auto;
  min-height: 38px;
  padding: 0 12px;
  border: 1px solid rgb(67 156 122 / 27%);
  border-radius: 7px;
  color: #39846a;
  background: #f1fbf6;
  font-size: 11px;
  font-weight: 800;
}

/* 刷新按钮保持与保存状态同高，用图标表达“重新计算”这个低频但明确的动作。 */
.pve-panel__refresh-button {
  display: inline-flex;
  flex: 0 0 38px;
  align-items: center;
  justify-content: center;
  width: 38px;
  height: 38px;
  padding: 0;
  border: 1px solid rgb(67 156 122 / 27%);
  border-radius: 7px;
  color: #39846a;
  background: #f1fbf6;
  cursor: pointer;
  transition:
    color 160ms ease,
    background-color 160ms ease,
    border-color 160ms ease;
}

.pve-panel__refresh-button svg {
  width: 16px;
  height: 16px;
  fill: none;
  stroke: currentcolor;
  stroke-linecap: round;
  stroke-linejoin: round;
  stroke-width: 1.8;
}

.pve-panel__refresh-button:hover:not(:disabled) {
  border-color: rgb(67 156 122 / 46%);
  color: #236d54;
  background: #e3f6ed;
}

.pve-panel__refresh-button:focus-visible {
  outline: 3px solid rgb(60 151 243 / 24%);
  outline-offset: 2px;
}

.pve-panel__refresh-button:disabled {
  cursor: wait;
  opacity: 0.55;
}

/* 计算范围提前展示，避免用户只能从结果消息判断本次是否排除了非 +15 御魂。 */
.pve-panel__calculation-options {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px 12px;
  padding: 9px 11px;
  border: 1px solid #dceaf6;
  border-radius: 9px;
  color: var(--pve-muted);
  background: #f8fcff;
  font-size: 11px;
}

.pve-panel__calculation-options label {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--pve-ink);
  font-weight: 800;
  cursor: pointer;
}

.pve-panel__calculation-options input {
  accent-color: var(--pve-blue);
}

.pve-panel__calculation-options small {
  font-size: 10px;
}

.pve-panel__calculation-notice,
.pve-panel__background-status {
  margin: 0;
  padding: 9px 12px;
  border-radius: 8px;
  font-size: 11px;
  line-height: 1.5;
}

.pve-panel__calculation-notice {
  border: 1px solid rgb(67 156 122 / 27%);
  color: #39846a;
  background: #f1fbf6;
}

.pve-panel__background-status {
  border: 1px dashed #c7dff1;
  color: #55718e;
  background: #f8fcff;
}

/* 计算失败保持在工具栏下方，错误不覆盖卡片内容，方便用户直接重试。 */
.pve-panel__calculation-error {
  padding: 10px 13px;
  border: 1px solid #f2caca;
  border-left: 3px solid #e87979;
  border-radius: 8px;
  color: #a24c4c;
  background: #fff7f7;
  font-size: 11px;
}

/* 未计算时只保留明确的行动入口，避免静态占位卡片被误认为是真实库存结果。 */
.pve-panel__precalculate {
  display: grid;
  justify-items: center;
  gap: 8px;
  padding: 42px 24px;
  border: 1px dashed #cfe0ef;
  border-radius: 14px;
  color: var(--pve-muted);
  background: linear-gradient(145deg, #fbfdff, #f5faff);
  text-align: center;
}

.pve-panel__precalculate-mark {
  display: grid;
  width: 48px;
  height: 48px;
  place-items: center;
  border: 1px solid rgb(60 151 243 / 30%);
  border-radius: 15px;
  color: var(--pve-blue);
  background: #edf7ff;
  font-family: "Noto Serif SC", Georgia, serif;
  font-size: 25px;
  font-weight: 900;
}

.pve-panel__precalculate strong {
  color: var(--pve-ink);
  font-size: 15px;
}

.pve-panel__precalculate p {
  max-width: 360px;
  margin: 0;
  font-size: 11px;
  line-height: 1.6;
}

.pve-preview-card {
  padding: 19px 20px 15px;
  border: 1px solid #e2e8ef;
  border-radius: 14px;
  background: #fff;
  box-shadow: 0 12px 30px rgb(61 79 102 / 7%);
}

.pve-preview-card__heading {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 18px;
  margin-bottom: 16px;
  padding: 0 2px 13px;
  border-bottom: 1px solid #e6ebf0;
}

.pve-preview-card__heading h3 {
  margin: 5px 0 0;
  color: var(--pve-ink);
  font-size: 20px;
}

.pve-preview-card__heading p {
  margin: 0;
  color: var(--pve-muted);
  font-size: 11px;
}

.pve-matrix-scroll {
  overflow-x: auto;
  padding: 1px 1px 7px;
}

.pve-matrix {
  display: grid;
  min-width: 1120px;
  gap: 12px;
}

.pve-matrix__row {
  display: grid;
  grid-template-columns: minmax(220px, 1.25fr) repeat(4, minmax(170px, 1fr)) minmax(
      170px,
      1fr
    );
  align-items: stretch;
  gap: 10px;
}

.pve-matrix__corner,
.pve-matrix__boss-head,
.pve-matrix__average-head {
  min-height: 68px;
  padding: 12px 14px;
  border: 1px solid var(--head-line);
  border-radius: 9px;
  background: var(--head-bg);
}

.pve-matrix__corner {
  display: grid;
  align-content: center;
  gap: 4px;
  --head-line: #dce6f0;
  --head-bg: #f7fafe;
}

.pve-matrix__corner span {
  font-size: 13px;
  font-weight: 800;
}

.pve-matrix__corner small {
  color: var(--pve-muted);
  font-size: 10px;
}

.pve-matrix__boss-head {
  display: flex;
  align-items: center;
  gap: 9px;
  --head-line: #cfe3fb;
  --head-bg: #edf6ff;
}

.pve-matrix__boss-head--green {
  --head-line: #d8ebcb;
  --head-bg: #f0faeb;
}

.pve-matrix__boss-head--orange {
  --head-line: #f1dfc6;
  --head-bg: #fff8ee;
}

.pve-matrix__boss-head--violet {
  --head-line: #ded5f2;
  --head-bg: #f6f2ff;
}

.pve-matrix__boss-icon {
  display: grid;
  width: 43px;
  height: 43px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 11px;
  background: rgb(255 255 255 / 78%);
  box-shadow: 0 4px 9px rgb(63 84 107 / 9%);
}

.pve-matrix__boss-icon img,
.pve-matrix__soul-icon img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: contain;
}

.pve-matrix__boss-name,
.pve-matrix__soul-copy {
  display: grid;
  min-width: 0;
  gap: 4px;
}

.pve-matrix__boss-name b {
  font-size: 13px;
}

.pve-matrix__boss-name small,
.pve-matrix__soul-copy small {
  overflow: hidden;
  color: var(--pve-muted);
  font-size: 10px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pve-matrix__average-head {
  display: grid;
  align-content: center;
  gap: 4px;
  border-color: #ffc8c8;
  background: #fff1f1;
  color: #f05e5e;
  text-align: center;
}

.pve-matrix__average-head span {
  font-size: 14px;
  font-weight: 900;
}

.pve-matrix__average-head small {
  color: #c47c7c;
  font-size: 9px;
}

.pve-matrix__row--body > * {
  min-height: 92px;
}

.pve-matrix__soul-label,
.pve-matrix__metric-card {
  box-sizing: border-box;
  height: 100%;
  border: 1px solid var(--pve-line);
  border-radius: 10px;
  background: #fff;
  box-shadow: 0 4px 12px rgb(61 79 102 / 5%);
}

.pve-matrix__soul-label {
  display: flex;
  align-items: center;
  min-height: 92px;
  gap: 10px;
  min-width: 0;
  padding: 10px 12px;
}

.pve-matrix__soul-icon {
  display: grid;
  width: 49px;
  height: 49px;
  flex: 0 0 auto;
  place-items: center;
  border: 1px solid var(--soul-accent);
  border-radius: 13px;
  color: var(--soul-accent);
  background: color-mix(in srgb, var(--soul-accent) 8%, white);
  font-family: "Noto Serif SC", Georgia, serif;
  font-size: 21px;
  font-weight: 900;
  box-shadow: 0 4px 11px color-mix(in srgb, var(--soul-accent) 13%, transparent);
}

.pve-matrix__soul-icon--red {
  --soul-accent: #bd5c50;
}
.pve-matrix__soul-icon--violet {
  --soul-accent: #806ea8;
}
.pve-matrix__soul-icon--jade {
  --soul-accent: #4c9580;
}
.pve-matrix__soul-icon--gold {
  --soul-accent: #bc8b42;
}
.pve-matrix__soul-icon--blue {
  --soul-accent: #5789bc;
}
.pve-matrix__soul-icon--ink {
  --soul-accent: #53687b;
}
.pve-matrix__soul-icon--rose {
  --soul-accent: #a76777;
}

.pve-matrix__soul-copy b {
  color: var(--pve-ink);
  font-size: 13px;
}

.pve-matrix__metric-card {
  display: grid;
  align-content: center;
  justify-items: start;
  min-height: 92px;
  gap: 7px;
  padding: 11px 14px;
}

/* 每个首领结果格独立携带详情入口，确保看到的六件御魂与该格分数一一对应。 */
.pve-matrix__details-button,
.pve-other-card__details-button {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  border: 0;
  color: var(--pve-blue);
  background: transparent;
  cursor: pointer;
  font-size: 10px;
  font-weight: 800;
  line-height: 1.2;
  padding: 0;
  text-align: left;
}

/* 查看图标使用线性眼睛轮廓，不增加额外文案宽度，帮助用户快速识别这是预览入口。 */
.pve-details-icon {
  width: 13px;
  height: 13px;
  flex: 0 0 auto;
  fill: none;
  stroke: currentcolor;
  stroke-linecap: round;
  stroke-linejoin: round;
  stroke-width: 1.7;
}

.pve-matrix__details-button:hover:not(:disabled),
.pve-other-card__details-button:hover:not(:disabled) {
  color: #176fc8;
  text-decoration: underline;
  text-underline-offset: 3px;
}

.pve-matrix__details-button:focus-visible,
.pve-other-card__details-button:focus-visible,
.pve-soul-detail-dialog__close:focus-visible,
.pve-soul-detail-dialog__footer button:focus-visible {
  outline: 3px solid rgb(60 151 243 / 24%);
  outline-offset: 2px;
}

.pve-matrix__details-button:disabled,
.pve-other-card__details-button:disabled {
  color: #aab5c1;
  cursor: not-allowed;
}

.pve-matrix__metric-label {
  color: var(--pve-muted);
  font-size: 10px;
}

.pve-matrix__metric-card strong {
  color: #8290a0;
  font-family: Consolas, "Microsoft YaHei UI", sans-serif;
  font-size: 20px;
  font-style: normal;
  font-variant-numeric: tabular-nums;
  letter-spacing: -0.03em;
  line-height: 1;
}

.pve-matrix__metric-card--average {
  border-color: #ffcaca;
  background: #fffafa;
}

.pve-matrix__metric-card--average strong {
  color: #f15d5d;
}

.pve-panel:has(.pve-panel__saved-state)
  .pve-matrix__metric-card:not(.pve-matrix__metric-card--average)
  strong {
  color: #388ff0;
}

.pve-preview-card__footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 14px;
  margin-top: 12px;
  padding: 12px 3px 0;
  border-top: 1px solid #e8edf2;
  color: var(--pve-muted);
  font-size: 10px;
}

.pve-preview-card__legend {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.pve-preview-card__legend i {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #7da4c9;
}

.pve-preview-card__overall b {
  color: #f15d5d;
  font-size: 15px;
  font-style: italic;
  font-variant-numeric: tabular-nums;
}

.pve-other-section {
  padding-top: 5px;
}

.pve-other-section__heading {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 18px;
  margin-bottom: 13px;
  padding: 14px 3px 12px;
  border-top: 1px solid #dfe6ee;
  border-bottom: 1px solid var(--pve-blue);
}

.pve-other-section__heading h3 {
  margin: 5px 0 0;
  color: var(--pve-ink);
  font-size: 20px;
}

.pve-other-section__heading p {
  margin: 0;
  color: var(--pve-muted);
  font-size: 11px;
}

.pve-other-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  align-items: stretch;
  gap: 16px;
}

.pve-other-card {
  display: grid;
  grid-template-rows: 68px 1fr auto;
  overflow: hidden;
  border: 1px solid #e0e7ef;
  border-radius: 11px;
  background: #fff;
  box-shadow: 0 5px 14px rgb(61 79 102 / 5%);
}

.pve-other-card__title {
  display: flex;
  align-items: center;
  gap: 9px;
  min-height: 68px;
  padding: 9px 12px;
  border-bottom: 1px solid #e5ebf1;
  background: var(--other-title-bg);
}

.pve-other-card--blue {
  --other-accent: #3c97f3;
  --other-title-bg: #eef7ff;
}

.pve-other-card--gold {
  --other-accent: #d5983d;
  --other-title-bg: #fff8ed;
}

.pve-other-card--jade {
  --other-accent: #4c9780;
  --other-title-bg: #f1faf5;
}

.pve-other-card--violet {
  --other-accent: #806ea8;
  --other-title-bg: #f6f2ff;
}

.pve-other-card__soul-icons {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  min-width: 36px;
}

.pve-other-card__soul-icons .pve-other-card__soul-icon + .pve-other-card__soul-icon {
  margin-left: -8px;
}

.pve-other-card__soul-icon {
  display: grid;
  display: grid;
  width: 36px;
  height: 36px;
  flex: 0 0 auto;
  place-items: center;
  border: 1px solid color-mix(in srgb, var(--other-accent) 32%, white);
  border-radius: 9px;
  color: var(--other-accent);
  background: rgb(255 255 255 / 72%);
  font-family: "Noto Serif SC", Georgia, serif;
  font-size: 17px;
  font-weight: 900;
}

.pve-other-card__soul-icon--fallback {
  padding-bottom: 1px;
}

.pve-other-card__soul-icon img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: contain;
}

.pve-other-card__title strong {
  display: -webkit-box;
  overflow: hidden;
  color: var(--pve-ink);
  font-size: 12px;
  line-height: 1.4;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}

.pve-other-card__body {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 12px;
  min-height: 104px;
  padding: 13px 12px;
}

/* 其他方案把详情入口放在卡片底部，避免压缩式神和指标的主要信息。 */
.pve-other-card__details-button {
  align-self: end;
  margin: 0 12px 11px;
  padding-top: 9px;
  border-top: 1px solid #edf1f5;
}

.pve-other-card__shikigami {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.pve-other-card__shikigami-mark {
  position: relative;
  display: grid;
  width: 30px;
  height: 30px;
  flex: 0 0 auto;
  place-items: center;
  border: 1px solid color-mix(in srgb, var(--other-accent) 35%, white);
  border-radius: 50%;
  color: var(--other-accent);
  background: color-mix(in srgb, var(--other-accent) 9%, white);
  font-family: "Noto Serif SC", Georgia, serif;
  font-size: 16px;
  font-weight: 900;
  overflow: hidden;
}

.pve-other-card__shikigami span:last-child {
  display: grid;
  min-width: 0;
  gap: 4px;
}

.pve-other-card__shikigami small,
.pve-other-card__metric small {
  color: var(--pve-muted);
  font-size: 10px;
  line-height: 1.35;
  text-align: right;
}

.pve-other-card__shikigami b {
  overflow: hidden;
  color: var(--pve-ink);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pve-other-card__metric {
  display: grid;
  justify-items: end;
  gap: 5px;
  min-width: 74px;
}

.pve-other-card__metric strong {
  color: #8290a0;
  font-family: Consolas, "Microsoft YaHei UI", sans-serif;
  font-size: 19px;
  font-style: normal;
  font-variant-numeric: tabular-nums;
  letter-spacing: -0.03em;
}

.pve-panel:has(.pve-panel__saved-state) .pve-other-card__metric strong {
  color: var(--other-accent);
}

.pve-panel__note {
  display: flex;
  align-items: flex-start;
  gap: 9px;
  padding: 11px 13px;
  border: 1px solid #dce9f8;
  border-left: 3px solid var(--pve-blue);
  border-radius: 8px;
  color: #61748b;
  background: #f7fbff;
  font-size: 10px;
  line-height: 1.6;
}

.pve-panel__note p {
  margin: 0;
}

.pve-panel__note-icon {
  display: grid;
  width: 16px;
  height: 16px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 50%;
  color: #fff;
  background: var(--pve-blue);
  font-family: Georgia, serif;
  font-size: 11px;
  font-weight: 900;
}

/* 六位置详情使用固定遮罩，信息按号位分组，方便从 1 到 6 顺序核对。 */
.pve-soul-detail-overlay {
  position: fixed;
  z-index: 90;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 24px;
  background: rgb(24 38 57 / 42%);
  backdrop-filter: blur(5px);
}

.pve-soul-detail-dialog {
  width: min(960px, 100%);
  max-height: min(820px, calc(100vh - 48px));
  overflow: auto;
  border: 1px solid #dce7f1;
  border-top: 4px solid var(--detail-accent, var(--pve-blue));
  border-radius: 16px;
  background: #fff;
  box-shadow: 0 24px 70px rgb(20 40 66 / 28%);
}

.pve-soul-detail-dialog--red {
  --detail-accent: #bd5c50;
}

.pve-soul-detail-dialog--violet {
  --detail-accent: #806ea8;
}

.pve-soul-detail-dialog--jade {
  --detail-accent: #4c9580;
}

.pve-soul-detail-dialog--gold {
  --detail-accent: #bc8b42;
}

.pve-soul-detail-dialog--blue {
  --detail-accent: #5789bc;
}

.pve-soul-detail-dialog--ink {
  --detail-accent: #53687b;
}

.pve-soul-detail-dialog--rose {
  --detail-accent: #a76777;
}

.pve-soul-detail-dialog--green {
  --detail-accent: #4c9780;
}

.pve-soul-detail-dialog__header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 20px;
  padding: 22px 24px 16px;
  border-bottom: 1px solid #e8edf2;
}

.pve-soul-detail-dialog__header h3 {
  margin: 5px 0 4px;
  color: var(--pve-ink);
  font-size: 21px;
}

.pve-soul-detail-dialog__header p:last-child {
  margin: 0;
  color: var(--pve-muted);
  font-size: 11px;
}

.pve-soul-detail-dialog__close {
  display: grid;
  width: 30px;
  height: 30px;
  flex: 0 0 auto;
  place-items: center;
  border: 1px solid #dce6ef;
  border-radius: 8px;
  color: #6b7f94;
  background: #f7fbff;
  cursor: pointer;
  font-size: 23px;
  line-height: 1;
}

.pve-soul-detail-dialog__close:hover {
  border-color: #bfd3e6;
  color: var(--pve-ink);
  background: #eef7ff;
}

.pve-soul-detail-list {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 14px;
  padding: 22px 28px;
}

.pve-soul-detail-item {
  display: grid;
  grid-template-columns: 42px 46px minmax(126px, 0.72fr) minmax(0, 1.28fr);
  align-items: center;
  gap: 14px;
  min-height: 104px;
  padding: 14px 16px;
  border: 1px solid color-mix(in srgb, var(--detail-accent) 22%, #e0e7ef);
  border-radius: 10px;
  background: color-mix(in srgb, var(--detail-accent) 5%, white);
}

.pve-soul-detail-item--empty {
  border-style: dashed;
  background: #fafcfe;
}

.pve-soul-detail-item__slot {
  display: grid;
  width: 36px;
  height: 36px;
  place-items: center;
  border-radius: 9px;
  color: #fff;
  background: var(--detail-accent);
  font-size: 11px;
  font-weight: 900;
}

/* 每个位置单独展示套装图标，图标缺失时由模板回退为套装首字。 */
.pve-soul-detail-item__icon {
  display: grid;
  width: 38px;
  height: 38px;
  place-items: center;
  overflow: hidden;
  border: 1px solid color-mix(in srgb, var(--detail-accent) 28%, #dce7ef);
  border-radius: 9px;
  color: var(--detail-accent);
  background: rgb(255 255 255 / 72%);
  font-family: "Noto Serif SC", Georgia, serif;
  font-size: 16px;
  font-weight: 900;
}

.pve-soul-detail-item__icon img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: contain;
}

.pve-soul-detail-item__identity,
.pve-soul-detail-item__attributes {
  display: grid;
  min-width: 0;
  gap: 5px;
}

.pve-soul-detail-item__identity b {
  overflow: hidden;
  color: var(--pve-ink);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 御魂名称使用独立色块高亮，帮助用户在较宽详情卡中快速定位套装。 */
.pve-soul-detail-item__soul-name {
  display: inline-flex;
  width: fit-content;
  max-width: 100%;
  padding: 4px 8px;
  border: 1px solid color-mix(in srgb, var(--detail-accent) 26%, #dce7ef);
  border-radius: 6px;
  color: var(--detail-accent) !important;
  background: color-mix(in srgb, var(--detail-accent) 9%, white);
}

.pve-soul-detail-item__identity small,
.pve-soul-detail-item__attributes span,
.pve-soul-detail-item__empty-copy {
  overflow: hidden;
  color: var(--pve-muted);
  font-size: 10px;
  line-height: 1.45;
  text-overflow: ellipsis;
}

.pve-soul-detail-item__attributes strong {
  overflow: hidden;
  color: var(--detail-accent);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 副属性逐条排列，右侧用醒目的徽标展示已经强化了几次，避免多条文字连成一团。 */
.pve-soul-detail-item__subattributes {
  display: grid;
  gap: 5px;
  margin-top: 1px;
}

.pve-soul-detail-item__subattribute {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  min-width: 0;
  padding: 4px 6px;
  border: 1px solid color-mix(in srgb, var(--detail-accent) 15%, #e6edf3);
  border-radius: 6px;
  background: color-mix(in srgb, var(--detail-accent) 4%, white);
}

.pve-soul-detail-item__subattribute-name {
  overflow: visible !important;
  min-width: 0;
  color: var(--pve-ink) !important;
  font-size: 10px !important;
  text-overflow: clip !important;
  white-space: nowrap;
}

.pve-soul-detail-item__enhancement-count {
  flex: 0 0 auto;
  padding: 3px 6px;
  border: 1px solid color-mix(in srgb, var(--detail-accent) 28%, #dce7ef);
  border-radius: 999px;
  color: var(--detail-accent) !important;
  background: color-mix(in srgb, var(--detail-accent) 11%, white);
  font-size: 10px !important;
  font-weight: 800;
  letter-spacing: 0.02em;
}

.pve-soul-detail-item__enhancement-unknown {
  flex: 0 0 auto;
  color: #9aa7b4;
  font-size: 9px;
  white-space: nowrap;
}

.pve-soul-detail-item__empty-copy {
  grid-column: 2 / -1;
}

.pve-soul-detail-dialog__footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 13px 24px 18px;
  border-top: 1px solid #e8edf2;
  color: var(--pve-muted);
  font-size: 10px;
}

.pve-soul-detail-dialog__footer button {
  min-width: 56px;
  padding: 7px 12px;
  border: 1px solid #c9dced;
  border-radius: 7px;
  color: var(--pve-blue);
  background: #f5faff;
  cursor: pointer;
  font-size: 11px;
  font-weight: 800;
}

/* 遮罩层只在用户主动计算期间出现，阻止重复点击并明确全量读取仍在进行。 */
.pve-calculation-overlay {
  position: fixed;
  z-index: 80;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 24px;
  background: rgb(24 38 57 / 34%);
  backdrop-filter: blur(4px);
}

.pve-calculation-dialog {
  display: grid;
  width: min(390px, 100%);
  gap: 10px;
  padding: 28px 30px 24px;
  border: 1px solid rgb(255 255 255 / 68%);
  border-radius: 18px;
  background: linear-gradient(145deg, #fff, #f8fbff);
  box-shadow: 0 24px 70px rgb(20 40 66 / 28%);
  text-align: center;
}

.pve-calculation-dialog__sigil {
  display: grid;
  width: 48px;
  height: 48px;
  margin: 0 auto 2px;
  place-items: center;
  border: 1px solid rgb(60 151 243 / 30%);
  border-radius: 15px;
  color: var(--pve-blue);
  background: #edf7ff;
  font-family: "Noto Serif SC", Georgia, serif;
  font-size: 26px;
  font-weight: 900;
  box-shadow: 0 8px 20px rgb(60 151 243 / 15%);
}

.pve-calculation-dialog__eyebrow {
  margin: 0;
  color: var(--pve-blue);
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.16em;
}

.pve-calculation-dialog h3 {
  margin: 0;
  color: var(--pve-ink);
  font-size: 20px;
}

.pve-calculation-dialog > p:not(.pve-calculation-dialog__eyebrow) {
  min-height: 18px;
  margin: 0;
  color: var(--pve-muted);
  font-size: 11px;
}

.pve-calculation-progress {
  height: 8px;
  overflow: hidden;
  margin-top: 5px;
  border-radius: 999px;
  background: #e8eff6;
}

.pve-calculation-progress span {
  display: block;
  height: 100%;
  border-radius: inherit;
  background: linear-gradient(90deg, #53a9f3, #70d1b0);
  transition: width 180ms ease-out;
}

.pve-calculation-dialog__meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
  color: #8090a1;
  font-size: 10px;
}

.pve-calculation-dialog__meta b {
  color: var(--pve-blue);
  font-size: 12px;
}

/* 增量结果最多展示最近六项，既让用户知道任务在前进，也避免长库存撑高遮罩。 */
.pve-calculation-dialog__items {
  display: grid;
  gap: 4px;
  max-height: 122px;
  margin: 2px 0 0;
  padding: 0;
  overflow: hidden;
  color: #63778c;
  font-size: 10px;
  list-style: none;
  text-align: left;
}

.pve-calculation-dialog__items li {
  display: flex;
  align-items: center;
  gap: 6px;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.pve-calculation-dialog__items li span {
  color: #39ad86;
  font-weight: 900;
}

/* 计算在独立 Worker 中运行，取消按钮可以立即终止当前线程，不必等待当前搜索完成。 */
.pve-calculation-dialog__cancel {
  justify-self: center;
  min-width: 92px;
  padding: 7px 14px;
  border: 1px solid #c9dced;
  border-radius: 8px;
  color: #6b7f94;
  background: #f5faff;
  cursor: pointer;
  font-size: 11px;
  font-weight: 800;
}

.pve-calculation-dialog__cancel:hover {
  border-color: #a9c8e1;
  color: var(--pve-ink);
  background: #edf7ff;
}

@media (max-width: 720px) {
  .pve-panel__header,
  .pve-panel__toolbar,
  .pve-preview-card__heading,
  .pve-preview-card__footer,
  .pve-other-section__heading {
    align-items: flex-start;
    flex-direction: column;
  }

  .pve-panel__header-meta,
  .pve-panel__toolbar-action {
    align-self: stretch;
  }

  .pve-panel__toolbar-action {
    flex-wrap: wrap;
  }

  .pve-panel__background-button,
  .pve-panel__calculate-button,
  .pve-panel__saved-state {
    justify-content: center;
    width: 100%;
  }

  .pve-panel__saved-state {
    width: auto;
  }

  .pve-preview-card {
    padding: 16px 12px 13px;
  }

  .pve-other-grid {
    grid-template-columns: 1fr;
  }

  .pve-soul-detail-overlay {
    padding: 12px;
  }

  .pve-soul-detail-dialog {
    max-height: calc(100vh - 24px);
  }

  .pve-soul-detail-dialog__header,
  .pve-soul-detail-list,
  .pve-soul-detail-dialog__footer {
    padding-left: 16px;
    padding-right: 16px;
  }

  .pve-soul-detail-list {
    grid-template-columns: 1fr;
  }

  .pve-soul-detail-item {
    grid-template-columns: 40px 40px minmax(0, 1fr);
  }

  .pve-soul-detail-item__attributes {
    grid-column: 1 / -1;
  }

  .pve-soul-detail-dialog__footer {
    align-items: flex-start;
    flex-direction: column;
  }

  .pve-soul-detail-dialog__footer button {
    align-self: flex-end;
  }
}

@media (prefers-reduced-motion: reduce) {
  .pve-panel button,
  .pve-calculation-progress span {
    transition: none;
  }
}
</style>
