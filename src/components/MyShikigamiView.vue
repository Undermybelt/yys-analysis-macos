<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import type {
  CatalogStatus,
  MyShikigami,
  Shikigami,
  ShikigamiBagGroup,
  ShikigamiShardEntry,
  ShikigamiStoryProgress,
} from "../api/contracts";
import { desktopApi } from "../api/client";
import {
  CURRENT_BUILTIN_SHIKIGAMI_COUNT,
  getMaterialShikigamiName,
  getRarityLabel,
  isMaterialShikigamiRarity,
  rarityOrder,
} from "../shikigamiCatalog";
import { getShikigamiWikiUrl } from "../shikigamiWiki";
import CharacterArchiveSwitcher from "./CharacterArchiveSwitcher.vue";
import { onActiveCharacterChanged } from "../state/activeCharacter";

type CollectionFilter = "ALL" | "SIX_STAR" | "MAX_LEVEL";
type SortMode = "RARITY" | "STAR" | "LEVEL";
type CollectionView = "ROSTER" | "UNSIX_STAR" | "UNOWNED";

// 来源数据缺少等级时按游戏录入页的最低可用等级展示；只影响式神录视图，不覆盖原始快照事实。
const DEFAULT_SHIKIGAMI_LEVEL = 1;
interface OwnedShikigamiGroup {
  shikigamiId: string;
  catalog: Shikigami | null;
  records: MyShikigami[];
  /** HeroData 保留每只式神的实例明细；这个数量是前端按 heroId 折叠后的拥有数量。 */
  count: number;
  /**
   * 式神仓库里同 heroId 的素材数量（达摩等）。游戏只存分组计数、没有逐只实例，
   * 因此这部分不会出现在 records 中，只能作为数量展示。
   */
  bagCount: number;
}

/** 未获得式神条目附带当前快照中的碎片记录与进度；没有碎片记录时按 0 片计算。 */
type UnownedShikigamiEntry = Shikigami & {
  shardProgress: ShardProgress | null;
};

/** 未获得式神右侧显示的碎片进度；上限沿用快照，缺记录时按稀有度规则补 50/60。 */
interface ShardProgress {
  current: number;
  required: number;
}

const owned = ref<MyShikigami[]>([]);
const bagGroups = ref<ShikigamiBagGroup[]>([]);
// 碎片接口只返回目标式神的碎片记录，页面按式神 ID 合并到未获得名单。
const shikigamiShards = ref<ShikigamiShardEntry[]>([]);
// 只有碎片接口成功返回后才展示 0/50、0/60，避免读取失败被误报成零碎片。
const shikigamiShardsLoaded = ref(false);
// 式神总数（含折叠重复）；null 表示摘要不可用，退回实例数。
const shikigamiTotal = ref<number | null>(null);
const catalog = ref<Shikigami[]>([]);
const catalogStatus = ref<CatalogStatus | null>(null);
// 当前账号按式神保存的全部传记进度；旧快照缺少字段时为空，页面不擅自推断解锁状态。
const storyProgress = ref<ShikigamiStoryProgress[]>([]);
const selectedId = ref<string | null>(null);
const search = ref("");
// 分析视图使用独立搜索词，避免用户从分析返回式神录时丢失原来的列表筛选。
const analysisSearch = ref("");
// 未获得名单使用独立搜索词，避免切回式神录时污染已拥有式神的搜索条件。
const unownedSearch = ref("");
// 式神录默认展示培养明细；两个页头按钮分别切换未六星分析和未获得清单。
const collectionView = ref<CollectionView>("ROSTER");
const collectionFilter = ref<CollectionFilter>("ALL");
const sortMode = ref<SortMode>("RARITY");
const loading = ref(true);
const errorMessage = ref<string | null>(null);
const catalogWarning = ref<string | null>(null);

const catalogById = computed(
  () => new Map(catalog.value.map((entry) => [entry.shikigamiId, entry])),
);

/** 提供稳定的碎片索引，避免未获得列表渲染时反复扫描全部碎片记录。 */
const shikigamiShardsById = computed(
  () => new Map(shikigamiShards.value.map((entry) => [entry.shikigamiId, entry])),
);

/** 将传记进度索引为式神 ID，列表和详情都复用同一份读取结果。 */
const storyProgressById = computed(
  () => new Map(storyProgress.value.map((entry) => [entry.shikigamiId, entry])),
);

/** 传记碎片解锁对 UR、SSR、SP 计算，其他稀有度保留式神信息但不生成解锁结果。 */
const SHARD_CALCULATION_RARITIES = new Set(["UR", "SSR", "SP"]);

/** 根据式神稀有度确定召唤碎片上限；SSR 为 50，UR/SP 为 60。 */
function defaultShardRequiredCount(rarity: string): number | null {
  if (rarity === "SSR") return 50;
  if (rarity === "UR" || rarity === "SP") return 60;
  return null;
}

/** 判断当前目录条目是否属于需要计算传记碎片的 UR、SSR、SP 范围。 */
function isShardCalculationRarity(rarity: string | null | undefined): boolean {
  return rarity !== null && rarity !== undefined && SHARD_CALCULATION_RARITIES.has(rarity);
}

/** 合并实际碎片记录；快照没有该式神记录时，目标稀有度仍显示 0/50 或 0/60。 */
function getShardProgress(
  entry: Shikigami,
  shard: ShikigamiShardEntry | undefined,
): ShardProgress | null {
  if (!shikigamiShardsLoaded.value) return null;
  if (!isShardCalculationRarity(entry.rarity)) return null;
  const required = shard?.maxShardCount || defaultShardRequiredCount(entry.rarity);
  if (!required) return null;
  return {
    current: shard?.shardCount ?? 0,
    required,
  };
}

/**
 * 将 HeroData 导入的同一 heroId 实例收束为一行，详情区再展开每一只。
 *
 * 导入层故意保留 instanceId、星级和培养信息，前端只负责展示折叠结果，
 * 这样不会因为合并同名式神而丢失重复式神的独立状态。
 *
 * 式神仓库的素材式神（达摩等）按同一 heroId 并入对应行；只有素材、没有培养
 * 实例的 heroId 也会独立成行，保证列表合计与游戏式神录顶部一致。
 */
const allGroups = computed<OwnedShikigamiGroup[]>(() => {
  const grouped = new Map<string, MyShikigami[]>();
  for (const record of owned.value) {
    const records = grouped.get(record.shikigamiId) ?? [];
    records.push(record);
    grouped.set(record.shikigamiId, records);
  }

  const bagById = new Map<string, number>();
  for (const group of bagGroups.value) {
    bagById.set(group.shikigamiId, (bagById.get(group.shikigamiId) ?? 0) + group.count);
    if (!grouped.has(group.shikigamiId)) grouped.set(group.shikigamiId, []);
  }

  return [...grouped.entries()].map(([shikigamiId, records]) => ({
    shikigamiId,
    catalog: catalogById.value.get(shikigamiId) ?? null,
    records,
    count: records.length,
    bagCount: bagById.get(shikigamiId) ?? 0,
  }));
});

/** 按式神录原有搜索、筛选和排序规则展示培养明细，不影响分析视图的完整名单。 */
const groups = computed<OwnedShikigamiGroup[]>(() => {
  const normalizedSearch = search.value.trim().toLocaleLowerCase("zh-CN");
  const result = allGroups.value.filter((group) => {
    const name =
      group.catalog?.name ??
      getMaterialShikigamiName(group.shikigamiId) ??
      group.shikigamiId;
    const matchesSearch = name.toLocaleLowerCase("zh-CN").includes(normalizedSearch);
    const matchesFilter =
      collectionFilter.value === "ALL" ||
      (collectionFilter.value === "SIX_STAR" &&
        group.records.some((record) => record.star >= 6)) ||
      (collectionFilter.value === "MAX_LEVEL" &&
        group.records.some((record) => displayLevel(record) === 40));
    return matchesSearch && matchesFilter;
  });

  return result.sort((left, right) => {
    const leftName = groupName(left);
    const rightName = groupName(right);
    if (sortMode.value === "STAR") {
      const starDifference = maxStar(right) - maxStar(left);
      return starDifference || leftName.localeCompare(rightName, "zh-CN");
    }
    if (sortMode.value === "LEVEL") {
      const levelDifference = maxLevel(right) - maxLevel(left);
      return levelDifference || leftName.localeCompare(rightName, "zh-CN");
    }
    const rarityDifference =
      rarityOrder(left.catalog?.rarity ?? "UNKNOWN") -
      rarityOrder(right.catalog?.rarity ?? "UNKNOWN");
    return rarityDifference || leftName.localeCompare(rightName, "zh-CN");
  });
});

/** 素材式神没有逐只星级事实，不能被判定为“未六星培养对象”。 */
function isMaterialGroup(group: OwnedShikigamiGroup): boolean {
  return (
    Boolean(group.catalog && isMaterialShikigamiRarity(group.catalog.rarity)) ||
    Boolean(getMaterialShikigamiName(group.shikigamiId))
  );
}

/** 集中维护未六星名单的准入条件，保证按钮数量和展开后的实际列表使用同一条规则。 */
function shouldIncludeInUnstarredAnalysis(group: OwnedShikigamiGroup): boolean {
  return group.count > 0 && !isMaterialGroup(group) && maxStar(group) < 6;
}

/**
 * 未六星分析按同一 shikigamiId 合并判断：只要同类式神任意一只达到六星，
 * 该类就视为已满足条件；只有完全没有六星实例的已拥有式神才进入名单。
 */
const unstarredGroups = computed<OwnedShikigamiGroup[]>(() => {
  const normalizedSearch = analysisSearch.value.trim().toLocaleLowerCase("zh-CN");
  return allGroups.value
    .filter(shouldIncludeInUnstarredAnalysis)
    .filter((group) =>
      groupName(group).toLocaleLowerCase("zh-CN").includes(normalizedSearch),
    )
    .sort((left, right) => {
      const starDifference = maxStar(left) - maxStar(right);
      if (starDifference !== 0) return starDifference;
      const rarityDifference =
        rarityOrder(left.catalog?.rarity ?? "UNKNOWN") -
        rarityOrder(right.catalog?.rarity ?? "UNKNOWN");
      return (
        rarityDifference || groupName(left).localeCompare(groupName(right), "zh-CN")
      );
    });
});

/** 未六星分析按钮上的数量与实际名单保持一致，避免用户先点开才发现空结果。 */
const unstarredCount = computed(
  () => allGroups.value.filter(shouldIncludeInUnstarredAnalysis).length,
);

/** 以静态式神目录为全集，排除玩家实例和素材仓库条目，得到未获得的可培养式神。 */
const unownedCatalogEntries = computed<Shikigami[]>(() => {
  const ownedIds = new Set([
    ...owned.value.map((record) => record.shikigamiId),
    ...bagGroups.value.map((group) => group.shikigamiId),
  ]);
  return catalog.value
    .filter(
      (entry) =>
        !isMaterialShikigamiRarity(entry.rarity) && !ownedIds.has(entry.shikigamiId),
    )
    .sort((left, right) => {
      const rarityDifference = rarityOrder(left.rarity) - rarityOrder(right.rarity);
      return rarityDifference || left.name.localeCompare(right.name, "zh-CN");
    });
});

/** 未获得列表只按名称搜索，数量徽章使用未搜索前的完整名单。 */
const unownedEntries = computed<UnownedShikigamiEntry[]>(() => {
  const normalizedSearch = unownedSearch.value.trim().toLocaleLowerCase("zh-CN");
  const entries = !normalizedSearch
    ? unownedCatalogEntries.value
    : unownedCatalogEntries.value.filter((entry) =>
        entry.name.toLocaleLowerCase("zh-CN").includes(normalizedSearch),
      );
  return entries.map((entry) => ({
    ...entry,
    shardProgress: getShardProgress(
      entry,
      shikigamiShardsById.value.get(entry.shikigamiId),
    ),
  }));
});

/** 页头按钮展示完整未获得数量，避免搜索后按钮数字跟着变化造成认知跳动。 */
const unownedCount = computed(() => unownedCatalogEntries.value.length);

const selected = computed(
  () => groups.value.find((group) => group.shikigamiId === selectedId.value) ?? null,
);
const selectedCatalog = computed(() => selected.value?.catalog ?? null);
const selectedStoryProgress = computed(() =>
  selected.value ? storyProgressFor(selected.value) : null,
);
/** 列出没有匹配公开规则的传记二进度，避免把未知单位误报为御行达摩。 */
const storyProgressAnomalies = computed(() =>
  storyProgress.value
    .filter(
      (entry) =>
        isShardCalculationRarity(catalogById.value.get(entry.shikigamiId)?.rarity) &&
        entry.progressKind === "unknown" &&
        !entry.unlocked,
    )
    .map((entry) => ({
      ...entry,
      name: catalogById.value.get(entry.shikigamiId)?.name ?? `式神 ${entry.shikigamiId}`,
    }))
    .sort((left, right) => left.name.localeCompare(right.name, "zh-CN")),
);
const totalSpecies = computed(
  () =>
    new Set([
      ...owned.value.map((item) => item.shikigamiId),
      ...bagGroups.value.map((group) => group.shikigamiId),
    ]).size,
);
const sixStarCount = computed(
  () => owned.value.filter((item) => item.star >= 6).length,
);
const maxLevelCount = computed(
  () => owned.value.filter((item) => displayLevel(item) === 40).length,
);
const awakenedCount = computed(
  () => owned.value.filter((item) => item.awakened === true).length,
);

/**
 * 式神仓库里的素材式神总数（大吉/奉为/招福/御行达摩等）。
 *
 * 游戏把这类式神压缩成「分组 → 数量」存储，没有逐只实例，因此它们在列表里
 * 只能以数量呈现。游戏式神录顶部的总数包含这部分。
 */
const bagTotal = computed(() =>
  bagGroups.value.reduce((sum, group) => sum + group.count, 0),
);

/** 总数卡的构成说明；有素材式神时写明算式，让用户能对上游戏式神录顶部的数字。 */
const totalBreakdown = computed(() => {
  if (shikigamiTotal.value === null) return "仅本地实例，未读到游戏总数";
  if (bagTotal.value === 0) return "与游戏式神录一致";
  return `培养 ${owned.value.length.toLocaleString()} + 素材 ${bagTotal.value.toLocaleString()}`;
});

/** 筛选后保持一个确定的详情对象；空数据时不制造虚假的默认式神。 */
watch(groups, (next) => {
  if (next.length === 0) {
    selectedId.value = null;
  } else if (!next.some((group) => group.shikigamiId === selectedId.value)) {
    selectedId.value = next[0]?.shikigamiId ?? null;
  }
});

/** 空状态按钮通过应用壳事件跳转藏宝阁导入。 */
function openDesktopReading(): void {
  window.dispatchEvent(new CustomEvent("open-desktop-reading"));
}

/** 从分析名单回到对应详情；清除旧筛选，保证用户能立即看到这类式神的全部实例。 */
function openGroupFromAnalysis(group: OwnedShikigamiGroup): void {
  collectionView.value = "ROSTER";
  search.value = "";
  collectionFilter.value = "ALL";
  sortMode.value = "RARITY";
  selectedId.value = group.shikigamiId;
}

/** 首次打开补齐离线目录，再读取当前导入的玩家式神列表；目录失败不会吞掉已导入的原始式神 ID。 */
async function load(): Promise<void> {
  loading.value = true;
  errorMessage.value = null;
  catalogWarning.value = null;
  shikigamiShardsLoaded.value = false;
  storyProgress.value = [];
  try {
    owned.value = await desktopApi.listMyShikigami();
    try {
      bagGroups.value = await desktopApi.listShikigamiBag();
    } catch {
      // 素材式神只影响数量展示；读取失败时退回纯培养明细，不阻断式神录。
      bagGroups.value = [];
    }
    try {
      // 碎片读取失败时只隐藏“还缺多少”信息，不阻断式神持有与培养数据展示。
      shikigamiShards.value = await desktopApi.listShikigamiShards();
      shikigamiShardsLoaded.value = true;
    } catch {
      shikigamiShards.value = [];
    }
    try {
      // 传记进度读取失败只隐藏解锁状态，不阻断式神录已有的持有和培养信息。
      storyProgress.value = await desktopApi.listShikigamiStoryProgress();
    } catch {
      storyProgress.value = [];
    }
    try {
      // 式神总数（含折叠重复）以摘要口径为准，与游戏式神录顶部一致；摘要不可用时不阻断明细。
      const summary = await desktopApi.getCurrentDataSummary();
      shikigamiTotal.value = summary?.exists ? summary.shikigami_count : null;
    } catch {
      shikigamiTotal.value = null;
    }
    try {
      let current = await desktopApi.getCatalogStatus();
      if (
        !current ||
        current.validationStatus !== "verified" ||
        current.shikigamiCount < CURRENT_BUILTIN_SHIKIGAMI_COUNT ||
        current.panelCoverage === 0 ||
        current.skillCoverage === 0
      ) {
        current = await desktopApi.installBuiltinCatalog();
      }
      catalog.value = await desktopApi.listShikigami(current.version);
      catalogStatus.value = current;
    } catch (error) {
      catalog.value = [];
      catalogStatus.value = null;
      catalogWarning.value =
        error instanceof Error
          ? error.message
          : "静态式神图鉴暂时不可用，将显示原始式神 ID。";
    }
  } catch (error) {
    errorMessage.value =
      error instanceof Error
        ? error.message
        : "式神录读取失败，请确认当前在桌面应用中。";
  } finally {
    loading.value = false;
  }
}

/** 返回随应用打包的本地头像，式神录不依赖运行时网络图片。 */
function avatarPath(entry: Shikigami | null): string {
  return entry
    ? `${import.meta.env.BASE_URL}shikigami-avatars/${entry.shikigamiId}.webp`
    : "";
}

/** 头像缺失时保留稀有度色块和名称首字，避免详情区出现破图图标。 */
function hideBrokenAvatar(event: Event): void {
  const image = event.currentTarget;
  if (image instanceof HTMLImageElement) image.hidden = true;
}

function maxStar(group: OwnedShikigamiGroup): number {
  return Math.max(...group.records.map((record) => record.star), 0);
}

function maxLevel(group: OwnedShikigamiGroup): number {
  return Math.max(...group.records.map(displayLevel), DEFAULT_SHIKIGAMI_LEVEL);
}

/** 统一所有式神录中的等级展示与排序，避免同一条缺失等级的记录在不同区域显示不一致。 */
function displayLevel(record: Pick<MyShikigami, "level">): number {
  return record.level ?? DEFAULT_SHIKIGAMI_LEVEL;
}

function rarityClass(group: OwnedShikigamiGroup): string {
  const rarity = group.catalog?.rarity ?? "unknown";
  return isMaterialShikigamiRarity(rarity) ? "material" : rarity.toLowerCase();
}

function groupName(group: OwnedShikigamiGroup): string {
  return (
    group.catalog?.name ??
    getMaterialShikigamiName(group.shikigamiId) ??
    `式神 ${group.shikigamiId}`
  );
}

/** 返回式神录某一行的传记状态；没有读取到进度时保持空，不误报为未解锁。 */
function storyProgressFor(group: OwnedShikigamiGroup): ShikigamiStoryProgress | null {
  if (!isShardCalculationRarity(group.catalog?.rarity)) return null;
  return storyProgressById.value.get(group.shikigamiId) ?? null;
}

/** 只有 UR、SSR、SP 才展示传记二碎片解锁卡片，其他稀有度不参与本功能计算。 */
function shouldShowShardUnlock(group: OwnedShikigamiGroup): boolean {
  return isShardCalculationRarity(group.catalog?.rarity);
}

/** 列表中的短状态文案；完整的解锁规则在详情卡片中展开。 */
function storyProgressLabelFor(group: OwnedShikigamiGroup): string | null {
  const progress = storyProgressFor(group);
  if (!progress) return null;
  if (progress.unlocked) return "传记二已完成 · 碎片已解锁";
  if (progress.progressKind === "unknown") {
    return `传记二 ${progress.current}/${progress.required} · 公开条件待核对`;
  }
  return `传记二 ${progress.current}/${progress.required} · ${progress.unlockCondition ?? "条件已收录"}`;
}

/** 将规则目录内部来源键翻译成用户可读标签，避免页面暴露实现字段。 */
function storyRuleSourceLabel(source: string | null): string | null {
  if (source === "大神-2026-07") return "大神 2026 年 7 月整理";
  if (source === "zzzqiuchan-legacy-table") return "旧版传记条件表";
  return source;
}

/** 按公开条件类型补上剩余量单位，避免把等级进度读成黑蛋次数。 */
function storyProgressRemainingLabel(progress: ShikigamiStoryProgress): string {
  const remaining = progress.remaining;
  if (progress.progressKind === "level") return `${remaining}级`;
  if (progress.progressKind === "skillUpgrade") return `${remaining}次技能提升`;
  if (progress.progressKind === "shardCollection") return `${remaining}片`;
  if (progress.progressKind === "clanPrayer") return `${remaining}次祈愿`;
  if (
    progress.progressKind === "battleWin" ||
    progress.progressKind === "teamBattle"
  ) {
    return `${remaining}次胜利`;
  }
  return `${remaining}项`;
}

/** 调用 Tauri opener 将当前式神 Wiki 交给系统默认浏览器，避免 WebView 内打开失败。 */
async function openWiki(entry: Shikigami | null): Promise<void> {
  if (!entry) return;
  await desktopApi.openExternalUrl(getShikigamiWikiUrl(entry));
}

function recordTitle(record: MyShikigami): string {
  return `${record.star} 星 · ${displayLevel(record)} 级`;
}

function skillName(skillId: number): string {
  const skill = selectedCatalog.value?.skills.find(
    (item) => Number(item.skillId) === skillId,
  );
  return skill?.name ?? `技能 ${skillId}`;
}

function statusText(value: boolean | null, positive: string, negative: string): string {
  return value === null ? "未知" : value ? positive : negative;
}

onMounted(() => {
  void load();
  stopActiveCharacterChanged = onActiveCharacterChanged(() => {
    // 切换游戏档案后回到式神录主视图，避免把上一个档案的分析状态带到新档案。
    collectionView.value = "ROSTER";
    analysisSearch.value = "";
    unownedSearch.value = "";
    void load();
  });
});

let stopActiveCharacterChanged: (() => void) | null = null;
onBeforeUnmount(() => {
  stopActiveCharacterChanged?.();
  stopActiveCharacterChanged = null;
});
</script>

<template>
  <main class="page-main my-shikigami-page" aria-labelledby="my-shikigami-title">
    <header class="page-header my-shikigami-header">
      <div>
        <p class="eyebrow">MY DATA / SHIKIGAMI RECORD</p>
        <h1 id="my-shikigami-title">式神录</h1>
        <p class="page-lede">当前账号的式神持有与培养状态</p>
      </div>
      <div class="my-shikigami-header__meta" aria-live="polite">
        <CharacterArchiveSwitcher />
        <button
          v-if="
            !loading &&
            !errorMessage &&
            (owned.length > 0 || bagGroups.length > 0 || shikigamiShards.length > 0) &&
            catalog.length > 0
          "
          class="unowned-trigger"
          :class="{ 'unowned-trigger--active': collectionView === 'UNOWNED' }"
          type="button"
          :aria-pressed="collectionView === 'UNOWNED'"
          @click="collectionView = collectionView === 'UNOWNED' ? 'ROSTER' : 'UNOWNED'"
        >
          <span>未获得式神</span>
          <strong>{{ unownedCount }}</strong>
        </button>
        <button
          v-if="!loading && !errorMessage && owned.length > 0"
          class="unstarred-analysis-trigger"
          :class="{
            'unstarred-analysis-trigger--active': collectionView === 'UNSIX_STAR',
          }"
          type="button"
          :aria-pressed="collectionView === 'UNSIX_STAR'"
          @click="
            collectionView = collectionView === 'UNSIX_STAR' ? 'ROSTER' : 'UNSIX_STAR'
          "
        >
          <span>未6星式神分析</span>
          <strong>{{ unstarredCount }}</strong>
        </button>
        <span class="record-chip record-chip--accent">
          {{ (shikigamiTotal ?? owned.length).toLocaleString() }} 个式神
        </span>
        <span class="record-chip">{{ owned.length.toLocaleString() }} 只培养</span>
        <span v-if="bagTotal > 0" class="record-chip">
          {{ bagTotal.toLocaleString() }} 个素材
        </span>
        <span class="record-chip">{{ totalSpecies }} 种</span>
        <span v-if="catalogStatus" class="record-chip record-chip--quiet">
          图鉴 {{ catalogStatus.version }}
        </span>
      </div>
    </header>

    <div v-if="loading" class="my-shikigami-state" role="status">
      <span class="state-seal" aria-hidden="true">录</span>
      <div>
        <strong>正在打开式神录</strong>
        <p>读取当前数据中的玩家式神，不会重新扫描游戏文件。</p>
      </div>
    </div>
    <div
      v-else-if="errorMessage"
      class="my-shikigami-state my-shikigami-state--error"
      role="alert"
    >
      <span class="state-seal" aria-hidden="true">!</span>
      <div>
        <strong>式神录暂时不可用</strong>
        <p>{{ errorMessage }}</p>
        <button class="my-shikigami-text-button" type="button" @click="load">
          重新读取
        </button>
      </div>
    </div>
    <div
      v-else-if="
        owned.length === 0 && bagGroups.length === 0 && shikigamiShards.length === 0
      "
      class="my-shikigami-empty"
      role="status"
    >
      <div class="empty-seal" aria-hidden="true">式</div>
      <div>
        <p class="section-kicker">NO PLAYER RECORD</p>
        <h2>还没有导入式神数据</h2>
        <p>
          从藏宝阁导入公开商品，确认后会同时写入御魂与式神录。
          导入完成后回到这里查看自己的式神。
        </p>
        <button
          class="button button--primary"
          type="button"
          @click="openDesktopReading"
        >
          去藏宝阁导入
        </button>
      </div>
    </div>
    <div v-else class="my-shikigami-content">
      <section class="collection-metrics" aria-label="式神统计">
        <div class="collection-metric">
          <span>式神总数</span>
          <strong>{{ (shikigamiTotal ?? owned.length).toLocaleString() }}</strong>
          <small>{{ totalBreakdown }}</small>
        </div>
        <div class="collection-metric collection-metric--warm">
          <span>六星式神</span>
          <strong>{{ sixStarCount.toLocaleString() }}</strong>
          <small>已升至六星</small>
        </div>
        <div class="collection-metric collection-metric--jade">
          <span>满级式神</span>
          <strong>{{ maxLevelCount.toLocaleString() }}</strong>
          <small>等级 40</small>
        </div>
        <div class="collection-metric">
          <span>已觉醒</span>
          <strong>{{ awakenedCount.toLocaleString() }}</strong>
          <small>缓存中明确标记</small>
        </div>
      </section>

      <div v-if="bagTotal > 0" class="bag-notice" role="status">
        <span aria-hidden="true">材</span>
        <p>
          其中
          <strong>{{ bagTotal.toLocaleString() }}</strong>
          个是式神仓库里的素材式神（达摩等）。游戏对这类式神只记数量、不记单只信息，
          所以列表中它们没有等级和御魂明细。
        </p>
      </div>

      <div v-if="catalogWarning" class="catalog-warning" role="status">
        <span aria-hidden="true">!</span>{{ catalogWarning }}
      </div>

      <section
        v-if="storyProgressAnomalies.length > 0"
        class="story-anomaly-notice"
        aria-labelledby="story-anomaly-title"
      >
        <div class="story-anomaly-notice__heading">
          <span aria-hidden="true">核</span>
          <div>
            <strong id="story-anomaly-title">待核对的传记二进度</strong>
            <p>
              以下式神没有匹配到公开规则表，保留原始进度，不能直接换算御行达摩数量。
            </p>
          </div>
        </div>
        <ul class="story-anomaly-list" aria-label="待核对式神名单">
          <li v-for="entry in storyProgressAnomalies" :key="entry.shikigamiId">
            <strong>{{ entry.name }}</strong>
            <span>传记二 {{ entry.current }}/{{ entry.required }}</span>
            <small>可提供 10 枚碎片 · 公开条件待核对，请以游戏内传记提示为准</small>
          </li>
        </ul>
      </section>

      <section
        v-if="collectionView === 'UNSIX_STAR'"
        class="unstarred-analysis"
        aria-labelledby="unstarred-analysis-title"
      >
        <header class="unstarred-analysis__header">
          <div>
            <p class="section-kicker">DEVELOPMENT GAP / SIX STAR CHECK</p>
            <h2 id="unstarred-analysis-title">未6星式神分析</h2>
            <p>按式神种类合并判断；同类式神只要有一只达到 6 星，就不会重复列入这里。</p>
          </div>
          <button
            class="analysis-back-button"
            type="button"
            @click="collectionView = 'ROSTER'"
          >
            返回式神录
          </button>
        </header>

        <div class="unstarred-analysis__toolbar">
          <label class="analysis-search">
            <span class="sr-only">搜索未六星式神</span>
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <circle cx="10.5" cy="10.5" r="6.5" />
              <path d="m16 16 5 5" />
            </svg>
            <input
              v-model="analysisSearch"
              type="search"
              placeholder="搜索未六星式神"
            />
          </label>
          <span class="analysis-result-count">
            {{ unstarredGroups.length }} 类待提升
          </span>
        </div>

        <div v-if="unstarredGroups.length === 0" class="unstarred-analysis__empty">
          <span class="analysis-empty-seal" aria-hidden="true">✓</span>
          <div>
            <strong>{{
              analysisSearch ? "没有找到匹配的式神" : "当前没有未六星式神"
            }}</strong>
            <p>
              {{
                analysisSearch
                  ? "试试清空搜索，查看完整的待提升名单。"
                  : "已拥有的可培养式神中，每一类至少有一只已经达到 6 星。"
              }}
            </p>
          </div>
        </div>

        <div v-else class="unstarred-list" role="list" aria-label="未六星式神名单">
          <div
            v-for="group in unstarredGroups"
            :key="group.shikigamiId"
            role="listitem"
          >
            <button
              class="unstarred-entry"
              type="button"
              :aria-label="`查看${groupName(group)}的式神录详情`"
              @click="openGroupFromAnalysis(group)"
            >
              <span
                :class="[
                  'unstarred-entry__avatar',
                  `roster-avatar--${rarityClass(group)}`,
                ]"
              >
                <img
                  v-if="group.catalog"
                  class="roster-avatar__image"
                  :src="avatarPath(group.catalog)"
                  alt=""
                  @error="hideBrokenAvatar"
                />
                <span v-else aria-hidden="true">式</span>
              </span>
              <span class="unstarred-entry__copy">
                <strong>{{ groupName(group) }}</strong>
                <small>
                  {{
                    group.catalog ? getRarityLabel(group.catalog.rarity) : "未知稀有度"
                  }}
                  · 拥有 {{ group.count }} 只 · 最高 {{ maxStar(group) }} 星
                </small>
              </span>
              <span class="unstarred-entry__status">
                <b>{{ 6 - maxStar(group) }}</b>
                <small>星待补</small>
              </span>
              <span class="unstarred-entry__arrow" aria-hidden="true">→</span>
            </button>
          </div>
        </div>
      </section>

      <section
        v-else-if="collectionView === 'UNOWNED'"
        class="unstarred-analysis unowned-panel"
        aria-labelledby="unowned-title"
      >
        <header class="unstarred-analysis__header">
          <div>
            <p class="section-kicker">COLLECTION GAP / SHIKIGAMI CATALOG</p>
            <h2 id="unowned-title">未获得式神</h2>
            <p>以当前式神图鉴为全集，列出当前游戏档案中还没有记录的可培养式神。</p>
          </div>
          <button
            class="analysis-back-button"
            type="button"
            @click="collectionView = 'ROSTER'"
          >
            返回式神录
          </button>
        </header>

        <div class="unstarred-analysis__toolbar">
          <label class="analysis-search">
            <span class="sr-only">搜索未获得式神</span>
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <circle cx="10.5" cy="10.5" r="6.5" />
              <path d="m16 16 5 5" />
            </svg>
            <input v-model="unownedSearch" type="search" placeholder="搜索未获得式神" />
          </label>
          <span class="analysis-result-count unowned-result-count">
            {{ unownedEntries.length }} / {{ unownedCount }} 只未获得
          </span>
        </div>

        <div v-if="unownedEntries.length === 0" class="unstarred-analysis__empty">
          <span class="analysis-empty-seal" aria-hidden="true">✓</span>
          <div>
            <strong>{{
              unownedSearch
                ? "没有找到匹配的式神"
                : catalog.length === 0
                  ? "式神图鉴暂时不可用"
                  : "已获得全部可培养式神"
            }}</strong>
            <p>
              {{
                unownedSearch
                  ? "试试清空搜索，查看完整的未获得名单。"
                  : catalog.length === 0
                    ? "请先重新读取式神录，待离线图鉴加载完成后再查看。"
                    : "当前游戏档案已经记录了图鉴中的全部可培养式神。"
              }}
            </p>
          </div>
        </div>

        <div v-else class="unstarred-list" role="list" aria-label="未获得式神名单">
          <article
            v-for="entry in unownedEntries"
            :key="entry.shikigamiId"
            class="unstarred-entry unowned-entry"
            role="listitem"
          >
            <span
              :class="[
                'unstarred-entry__avatar',
                `roster-avatar--${entry.rarity.toLowerCase()}`,
              ]"
            >
              <img
                class="roster-avatar__image"
                :src="avatarPath(entry)"
                alt=""
                @error="hideBrokenAvatar"
              />
            </span>
            <span class="unstarred-entry__copy">
              <strong>{{ entry.name }}</strong>
              <small
                >{{ getRarityLabel(entry.rarity) }} · {{ entry.shikigamiId }}</small
              >
            </span>
            <span v-if="entry.shardProgress" class="unowned-entry__shards">
              <strong>
                还缺
                {{
                  Math.max(
                    entry.shardProgress.required - entry.shardProgress.current,
                    0,
                  )
                }}
                个碎片
              </strong>
              <small>
                {{ entry.shardProgress.current }}/{{ entry.shardProgress.required }}
              </small>
            </span>
            <span v-else class="unowned-entry__mark">未获得</span>
          </article>
        </div>
      </section>

      <div v-else class="my-shikigami-layout">
        <aside class="roster-panel" aria-label="我的式神列表">
          <div class="roster-toolbar">
            <label class="roster-search">
              <span class="sr-only">搜索式神名称</span>
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <circle cx="10.5" cy="10.5" r="6.5" />
                <path d="m16 16 5 5" />
              </svg>
              <input v-model="search" type="search" placeholder="搜索式神名称" />
            </label>
            <div class="roster-filter-row" aria-label="式神筛选">
              <button
                v-for="item in [
                  ['ALL', '全部'],
                  ['SIX_STAR', '六星'],
                  ['MAX_LEVEL', '满级'],
                ] as const"
                :key="item[0]"
                class="roster-filter"
                :class="{ 'roster-filter--active': collectionFilter === item[0] }"
                type="button"
                @click="collectionFilter = item[0]"
              >
                {{ item[1] }}
              </button>
            </div>
            <label class="roster-sort">
              <span>排序</span>
              <select v-model="sortMode" aria-label="式神排序方式">
                <option value="RARITY">稀有度</option>
                <option value="STAR">星级</option>
                <option value="LEVEL">等级</option>
              </select>
            </label>
          </div>

          <div v-if="groups.length === 0" class="roster-empty" role="status">
            没有匹配的式神<br /><small>试试清空搜索或切换筛选。</small>
          </div>
          <div v-else class="roster-list" role="listbox" aria-label="已拥有式神">
            <button
              v-for="group in groups"
              :key="group.shikigamiId"
              class="roster-entry"
              :class="{ 'roster-entry--selected': selectedId === group.shikigamiId }"
              type="button"
              role="option"
              :aria-selected="selectedId === group.shikigamiId"
              @click="selectedId = group.shikigamiId"
            >
              <span :class="['roster-avatar', `roster-avatar--${rarityClass(group)}`]">
                <img
                  v-if="group.catalog"
                  class="roster-avatar__image"
                  :src="avatarPath(group.catalog)"
                  alt=""
                  @error="hideBrokenAvatar"
                />
                <span v-else aria-hidden="true">{{
                  getMaterialShikigamiName(group.shikigamiId) ? "达" : "式"
                }}</span>
              </span>
              <span class="roster-entry__copy">
                <strong>{{ groupName(group) }}</strong>
                <small v-if="group.count > 0">
                  {{ group.count }} 只 · {{ maxStar(group) }} 星最高
                  <template v-if="group.bagCount > 0">
                    · 素材 {{ group.bagCount.toLocaleString() }}
                  </template>
                </small>
                <small v-else
                  >素材 {{ group.bagCount.toLocaleString() }} 个 · 无培养</small
                >
                <small
                  v-if="storyProgressLabelFor(group)"
                  class="roster-entry__story"
                  :class="{
                    'roster-entry__story--pending':
                      storyProgressFor(group)?.unlocked === false,
                  }"
                >
                  {{ storyProgressLabelFor(group) }}
                </small>
              </span>
              <span class="roster-entry__mark" aria-label="拥有数量">{{
                (group.count + group.bagCount).toLocaleString()
              }}</span>
            </button>
          </div>
        </aside>

        <section v-if="selected" class="record-detail" aria-live="polite">
          <header class="record-hero">
            <div :class="['record-avatar', `record-avatar--${rarityClass(selected)}`]">
              <img
                v-if="selectedCatalog"
                class="record-avatar__image"
                :src="avatarPath(selectedCatalog)"
                alt=""
                @error="hideBrokenAvatar"
              />
              <span v-else aria-hidden="true">式</span>
            </div>
            <div class="record-hero__copy">
              <div class="record-kicker">
                <span
                  :class="['rarity-label', `rarity-label--${rarityClass(selected)}`]"
                >
                  {{
                    selectedCatalog
                      ? getRarityLabel(selectedCatalog.rarity)
                      : getMaterialShikigamiName(selected.shikigamiId)
                        ? "素材"
                        : "未知稀有度"
                  }}
                </span>
                <span>{{ selected.shikigamiId }}</span>
              </div>
              <h2>{{ groupName(selected) }}</h2>
              <p v-if="selected.count > 0">
                拥有 {{ selected.count }} 只 · 最高 {{ maxStar(selected) }} 星 · 最高
                {{ maxLevel(selected) }} 级
                <template v-if="selected.bagCount > 0">
                  · 素材 {{ selected.bagCount.toLocaleString() }} 个
                </template>
              </p>
              <p v-else>
                式神仓库素材 {{ selected.bagCount.toLocaleString() }} 个 ·
                游戏未记录单只信息
              </p>
              <a
                v-if="selectedCatalog"
                class="record-wiki-link"
                :href="getShikigamiWikiUrl(selectedCatalog)"
                rel="noopener noreferrer"
                :aria-label="`查看${groupName(selected)} Wiki`"
                @click.prevent="openWiki(selectedCatalog)"
              >
                查看wiki
              </a>
            </div>
            <div class="record-seal" aria-label="玩家数据">玩家<br />数据</div>
          </header>

          <section
            v-if="
              selected.count > 0 &&
              !isMaterialGroup(selected) &&
              shouldShowShardUnlock(selected)
            "
            class="record-card record-card--story"
            aria-labelledby="record-story-title"
          >
            <div class="record-card__heading">
              <div>
                <span class="section-index">01</span>
                <h3 id="record-story-title">碎片解锁</h3>
              </div>
              <span class="record-card__caption">全部传记进度</span>
            </div>
            <div
              v-if="selectedStoryProgress"
              class="story-unlock-status"
              :class="{
                'story-unlock-status--unlocked': selectedStoryProgress.unlocked,
                'story-unlock-status--pending': !selectedStoryProgress.unlocked,
              }"
            >
              <strong>{{
                selectedStoryProgress.unlocked ? "碎片已解锁" : "碎片尚未解锁"
              }}</strong>
              <p v-if="selectedStoryProgress.unlocked">
                传记二已完成（{{ selectedStoryProgress.current }}/{{
                  selectedStoryProgress.required
                }}），碎片已解锁，该角色可以提供 10 枚式神碎片。
              </p>
              <p v-else-if="selectedStoryProgress.progressKind !== 'unknown'">
                传记二进度 {{ selectedStoryProgress.current }}/{{
                  selectedStoryProgress.required
                }}；提升该角色可以提供
                {{ selectedStoryProgress.unlockableShardCount ?? 10 }} 枚碎片；规则目标还需
                {{ storyProgressRemainingLabel(selectedStoryProgress) }}。
                <template v-if="selectedStoryProgress.progressKind === 'skillUpgrade'">
                  技能提升通常可用御行达摩或同名式神完成。
                </template>
              </p>
              <p v-else>
                传记二进度 {{ selectedStoryProgress.current }}/{{
                  selectedStoryProgress.required
                }}；提升该角色可以提供
                {{ selectedStoryProgress.unlockableShardCount ?? 10 }} 枚碎片，
                公开规则表未收录该式神，不能将剩余进度换算成御行达摩；请以游戏内传记提示为准。
              </p>
              <small v-if="selectedStoryProgress.unlockRuleSource" class="story-rule-source">
                规则来源：{{ storyRuleSourceLabel(selectedStoryProgress.unlockRuleSource) }}
              </small>
            </div>
            <div
              v-if="selectedStoryProgress"
              class="story-biography-list"
              aria-label="全部传记进度"
            >
              <div
                v-for="biography in selectedStoryProgress.biographies"
                :key="biography.index"
                class="story-biography-row"
                :class="{
                  'story-biography-row--completed': biography.completed,
                }"
              >
                <span>传记{{ biography.index }}</span>
                <strong>{{ biography.current }}/{{ biography.required }}</strong>
                <!-- 传记二单位由公开规则表决定；未收录时只展示原始进度，不换算黑蛋。 -->
                <small>{{
                  biography.completed
                    ? "已完成"
                    : biography.index === 2 &&
                        selectedStoryProgress.progressKind === "unknown"
                      ? "公开条件待核对"
                      : "还需" + biography.remaining + "项"
                }}</small>
              </div>
            </div>
            <p v-else class="story-unlock-status story-unlock-status--unknown">
              当前快照没有传记进度；重启应用不会重新读取游戏，请重新执行一次读取并勾选“式神”数据。
            </p>
          </section>

          <section
            v-if="selected.count > 0"
            class="record-card"
            aria-labelledby="record-status-title"
          >
            <div class="record-card__heading">
              <div>
                <span class="section-index">02</span>
                <h3 id="record-status-title">培养状态</h3>
              </div>
              <span class="record-card__caption">不含御魂装备关系</span>
            </div>
            <div class="record-status-grid">
              <div>
                <span>六星数量</span>
                <strong>{{
                  selected.records.filter((record) => record.star >= 6).length
                }}</strong>
              </div>
              <div>
                <span>满级数量</span>
                <strong>{{
                  selected.records.filter((record) => displayLevel(record) === 40)
                    .length
                }}</strong>
              </div>
              <div>
                <span>锁定数量</span>
                <strong>{{
                  selected.records.filter((record) => record.locked === true).length
                }}</strong>
              </div>
              <div>
                <span>觉醒数量</span>
                <strong>{{
                  selected.records.filter((record) => record.awakened === true).length
                }}</strong>
              </div>
            </div>
          </section>

          <section
            v-if="selected.count > 0"
            class="record-card"
            aria-labelledby="record-instances-title"
          >
            <div class="record-card__heading">
              <div>
                <span class="section-index">03</span>
                <h3 id="record-instances-title">持有实例</h3>
              </div>
              <span class="record-card__caption">{{ selected.count }} 条记录</span>
            </div>
            <div class="instance-list">
              <article
                v-for="record in selected.records"
                :key="record.instanceId"
                class="instance-row"
              >
                <div class="instance-row__title">
                  <strong>{{ recordTitle(record) }}</strong>
                  <code>{{ record.instanceId }}</code>
                </div>
                <div class="instance-row__tags">
                  <span>{{ statusText(record.locked, "已锁定", "未锁定") }}</span>
                  <span>{{ statusText(record.awakened, "已觉醒", "未觉醒") }}</span>
                  <span v-if="record.skinId !== null">皮肤 {{ record.skinId }}</span>
                  <span v-if="record.exp !== null"
                    >经验 {{ record.exp.toLocaleString() }}</span
                  >
                </div>
                <div class="skill-list" aria-label="技能等级">
                  <span
                    v-for="skill in record.skills"
                    :key="skill.skillId"
                    class="skill-pill"
                  >
                    {{ skillName(skill.skillId) }} · {{ skill.level }} 级
                  </span>
                  <span
                    v-if="record.skills.length === 0"
                    class="skill-pill skill-pill--muted"
                    >技能未知</span
                  >
                </div>
              </article>
            </div>
          </section>

          <section
            v-if="selected.count === 0"
            class="record-card"
            aria-labelledby="record-material-title"
          >
            <div class="record-card__heading">
              <div>
                <span class="section-index">01</span>
                <h3 id="record-material-title">素材式神</h3>
              </div>
              <span class="record-card__caption">仅数量</span>
            </div>
            <p class="record-material-note">
              这类式神存放在式神仓库，游戏按「分组 + 数量」压缩记录，不保存每一只的
              等级、御魂和技能，因此没有实例明细可展开。
            </p>
          </section>

          <footer class="record-source">
            <span v-if="catalogStatus">资料：式神图鉴 {{ catalogStatus.version }}</span>
            <span>可通过藏宝阁导入更新</span>
          </footer>
        </section>
      </div>
    </div>
  </main>
</template>

<style scoped>
.my-shikigami-page {
  max-width: 1440px;
  /* 页面目前直接挂载在应用壳网格中，单独保留页内边距，避免标题贴住窗口边缘。 */
  padding: 34px clamp(24px, 4vw, 64px) 28px;
}

.my-shikigami-header {
  align-items: flex-end;
  /* 标题操作和下方统计卡之间保持稳定的垂直层次。 */
  margin-bottom: 42px;
}

.my-shikigami-header__meta {
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
  justify-content: flex-end;
}

.record-chip {
  padding: 7px 11px;
  border: 1px solid var(--line);
  border-radius: 999px;
  color: var(--ink-soft);
  background: rgb(255 255 255 / 58%);
  font-size: 11px;
  letter-spacing: 0.06em;
}

.record-chip--accent {
  color: var(--cinnabar);
  border-color: rgb(164 70 53 / 28%);
  background: rgb(164 70 53 / 6%);
}

.record-chip--quiet {
  color: var(--accent-deep);
  border-color: rgb(31 119 103 / 20%);
}

/* 分析入口升级为页头主操作：实色朱砂、较大的点击区域和数量徽章共同拉开与统计标签的层级。 */
.unstarred-analysis-trigger {
  display: inline-flex;
  gap: 10px;
  align-items: center;
  min-height: 38px;
  padding: 8px 12px 8px 14px;
  border: 1px solid var(--cinnabar);
  border-radius: 7px;
  color: white;
  background: var(--cinnabar);
  box-shadow: 0 7px 16px rgb(164 70 53 / 18%);
  cursor: pointer;
  font: inherit;
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0.05em;
  white-space: nowrap;
  transition:
    box-shadow 160ms ease,
    transform 160ms ease;
}

.unstarred-analysis-trigger strong {
  display: grid;
  min-width: 22px;
  height: 22px;
  place-items: center;
  border-radius: 6px;
  color: var(--cinnabar);
  background: white;
  font-size: 11px;
  font-weight: 700;
}

.unstarred-analysis-trigger:hover,
.unstarred-analysis-trigger:focus-visible {
  box-shadow: 0 9px 20px rgb(164 70 53 / 28%);
  transform: translateY(-1px);
}

.unstarred-analysis-trigger--active strong {
  color: var(--cinnabar);
  background: white;
}

.unstarred-analysis-trigger--active {
  box-shadow:
    inset 0 0 0 2px rgb(255 255 255 / 35%),
    0 7px 16px rgb(164 70 53 / 18%);
}

/* 未获得入口与培养分析并列，但使用青绿色区分“查看缺少条目”和“检查培养进度”。 */
.unowned-trigger {
  display: inline-flex;
  gap: 10px;
  align-items: center;
  min-height: 38px;
  padding: 8px 12px 8px 14px;
  border: 1px solid var(--accent-deep);
  border-radius: 7px;
  color: white;
  background: var(--accent-deep);
  box-shadow: 0 7px 16px rgb(31 119 103 / 18%);
  cursor: pointer;
  font: inherit;
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0.05em;
  white-space: nowrap;
  transition:
    box-shadow 160ms ease,
    transform 160ms ease;
}

.unowned-trigger strong {
  display: grid;
  min-width: 22px;
  height: 22px;
  place-items: center;
  border-radius: 6px;
  color: var(--accent-deep);
  background: white;
  font-size: 11px;
  font-weight: 700;
}

.unowned-trigger:hover,
.unowned-trigger:focus-visible {
  box-shadow: 0 9px 20px rgb(31 119 103 / 28%);
  transform: translateY(-1px);
}

.unowned-trigger--active {
  box-shadow:
    inset 0 0 0 2px rgb(255 255 255 / 35%),
    0 7px 16px rgb(31 119 103 / 18%);
}

.collection-metrics {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 10px;
  margin-bottom: 15px;
  padding: 14px 18px;
  border: 1px solid var(--line-soft);
  background: rgb(255 255 255 / 60%);
}

.collection-metric {
  display: grid;
  gap: 4px;
}

.collection-metric span,
.collection-metric small {
  color: var(--ink-faint);
  font-size: 12px;
}

.collection-metric strong {
  color: var(--ink);
  font-size: 18px;
  font-weight: 600;
}

.collection-metric--warm strong {
  color: var(--cinnabar);
}

.collection-metric--jade strong {
  color: var(--accent-deep);
}

.record-material-note {
  margin: 0;
  padding: 14px 18px;
  color: var(--ink-soft);
  font-size: 12px;
  line-height: 1.75;
}

.bag-notice {
  display: flex;
  gap: 9px;
  align-items: flex-start;
  margin-bottom: 15px;
  padding: 10px 13px;
  border: 1px solid rgb(83 123 111 / 22%);
  color: var(--ink-soft);
  background: rgb(83 123 111 / 6%);
  font-size: 11.5px;
  line-height: 1.65;
}

.bag-notice span {
  display: grid;
  flex: 0 0 19px;
  place-items: center;
  width: 19px;
  height: 19px;
  border: 1px solid rgb(83 123 111 / 45%);
  border-radius: 50%;
  color: var(--jade);
  font-size: 10px;
}

.bag-notice p {
  margin: 0;
}

.bag-notice strong {
  color: var(--jade);
}

.catalog-warning {
  display: flex;
  gap: 8px;
  align-items: center;
  margin-bottom: 15px;
  padding: 9px 12px;
  border: 1px solid rgb(164 70 53 / 20%);
  color: var(--cinnabar);
  background: rgb(164 70 53 / 5%);
  font-size: 11px;
}

.catalog-warning span {
  display: grid;
  flex: 0 0 18px;
  place-items: center;
  width: 18px;
  height: 18px;
  border: 1px solid currentcolor;
  border-radius: 50%;
}

/* 未匹配公开规则的传记二单独列出，避免用户把未知进度误认为黑蛋数量。 */
.story-anomaly-notice {
  display: grid;
  gap: 12px;
  margin-bottom: 15px;
  padding: 14px 16px;
  border: 1px solid rgb(180 119 47 / 28%);
  border-radius: 10px;
  background: rgb(180 119 47 / 7%);
}

.story-anomaly-notice__heading {
  display: flex;
  gap: 10px;
  align-items: flex-start;
}

.story-anomaly-notice__heading > span {
  display: grid;
  flex: 0 0 26px;
  height: 26px;
  place-items: center;
  border: 1px solid rgb(180 119 47 / 45%);
  border-radius: 50%;
  color: #a56523;
  font-size: 12px;
}

.story-anomaly-notice__heading strong {
  color: #8d591e;
  font-size: 13px;
}

.story-anomaly-notice__heading p {
  margin: 3px 0 0;
  color: var(--ink-faint);
  font-size: 11px;
  line-height: 1.6;
}

.story-anomaly-list {
  display: grid;
  gap: 6px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.story-anomaly-list li {
  display: grid;
  grid-template-columns: minmax(92px, 1fr) auto;
  gap: 2px 12px;
  padding: 8px 10px;
  border: 1px solid rgb(180 119 47 / 18%);
  background: rgb(255 252 244 / 68%);
  font-size: 11px;
}

.story-anomaly-list li > strong {
  color: var(--ink);
}

.story-anomaly-list li > span {
  color: #a56523;
  font-variant-numeric: tabular-nums;
}

.story-anomaly-list li > small {
  grid-column: 1 / -1;
  color: var(--ink-faint);
}

/* 分析卡片刻意采用“待补账单”结构：规则说明在上，名单和缺口数字在下。 */
.unstarred-analysis {
  border: 1px solid rgb(164 70 53 / 22%);
  background: rgb(255 255 255 / 64%);
  box-shadow: 0 12px 30px rgb(61 53 44 / 5%);
}

.unstarred-analysis__header {
  display: flex;
  gap: 18px;
  align-items: flex-start;
  justify-content: space-between;
  padding: 24px 26px 20px;
  border-bottom: 1px solid var(--line-soft);
}

.unstarred-analysis__header h2 {
  margin: 7px 0 5px;
  color: var(--ink);
  font-family: "Noto Serif SC", serif;
  font-size: clamp(22px, 3vw, 30px);
}

.unstarred-analysis__header p:not(.section-kicker) {
  max-width: 640px;
  margin: 0;
  color: var(--ink-soft);
  font-size: 12px;
  line-height: 1.7;
}

.analysis-back-button {
  flex: 0 0 auto;
  padding: 7px 11px;
  border: 1px solid var(--line);
  color: var(--ink-soft);
  background: rgb(255 255 255 / 58%);
  cursor: pointer;
  font: inherit;
  font-size: 11px;
}

.analysis-back-button:hover,
.analysis-back-button:focus-visible {
  border-color: rgb(31 119 103 / 30%);
  color: var(--accent-deep);
  background: rgb(31 119 103 / 7%);
}

.unstarred-analysis__toolbar {
  display: flex;
  gap: 12px;
  align-items: center;
  justify-content: space-between;
  padding: 14px 26px;
  border-bottom: 1px solid var(--line-soft);
  background: rgb(252 249 241 / 52%);
}

.analysis-search {
  display: flex;
  gap: 8px;
  align-items: center;
  width: min(360px, 100%);
  padding: 8px 10px;
  border: 1px solid var(--line);
  background: var(--paper);
}

.analysis-search svg {
  width: 16px;
  height: 16px;
  fill: none;
  stroke: var(--ink-soft);
  stroke-width: 1.6;
}

.analysis-search input {
  width: 100%;
  border: 0;
  outline: 0;
  color: var(--ink);
  background: transparent;
  font: inherit;
  font-size: 12px;
}

.analysis-result-count {
  color: var(--cinnabar);
  font-size: 11px;
  white-space: nowrap;
}

.unstarred-list {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px;
  padding: 18px 26px 26px;
}

.unstarred-entry {
  display: flex;
  gap: 11px;
  align-items: center;
  min-width: 0;
  padding: 11px 12px;
  border: 1px solid var(--line-soft);
  color: var(--ink);
  background: rgb(252 249 241 / 70%);
  text-align: left;
  cursor: pointer;
  font: inherit;
  transition:
    border-color 160ms ease,
    background 160ms ease,
    transform 160ms ease;
}

.unstarred-entry:hover,
.unstarred-entry:focus-visible {
  border-color: rgb(164 70 53 / 34%);
  background: rgb(164 70 53 / 6%);
  transform: translateY(-1px);
}

.unstarred-entry__avatar {
  position: relative;
  display: grid;
  flex: 0 0 40px;
  width: 40px;
  height: 40px;
  place-items: center;
  overflow: hidden;
  border-radius: 10px;
  color: white;
  font-family: "Noto Serif SC", serif;
  font-size: 15px;
}

.unstarred-entry__copy {
  display: grid;
  flex: 1;
  min-width: 0;
  gap: 4px;
}

.unstarred-entry__copy strong {
  overflow: hidden;
  color: var(--ink);
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.unstarred-entry__copy small {
  overflow: hidden;
  color: var(--ink-faint);
  font-size: 10px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.unstarred-entry__status {
  display: grid;
  flex: 0 0 42px;
  gap: 1px;
  justify-items: end;
  color: var(--cinnabar);
}

.unstarred-entry__status b {
  font-size: 17px;
  font-variant-numeric: tabular-nums;
  font-weight: 600;
}

.unstarred-entry__status small {
  font-size: 9px;
  white-space: nowrap;
}

.unstarred-entry__arrow {
  flex: 0 0 auto;
  color: var(--ink-faint);
  font-size: 15px;
}

.unstarred-analysis__empty {
  display: flex;
  gap: 13px;
  align-items: center;
  margin: 18px 26px 26px;
  padding: 22px;
  border: 1px dashed rgb(31 119 103 / 24%);
  background: rgb(31 119 103 / 5%);
}

.analysis-empty-seal {
  display: grid;
  flex: 0 0 34px;
  width: 34px;
  height: 34px;
  place-items: center;
  border: 1px solid var(--jade);
  border-radius: 50%;
  color: var(--jade);
  font-size: 16px;
}

.unstarred-analysis__empty strong {
  color: var(--ink);
  font-family: "Noto Serif SC", serif;
  font-size: 15px;
}

.unstarred-analysis__empty p {
  margin: 4px 0 0;
  color: var(--ink-soft);
  font-size: 11px;
  line-height: 1.6;
}

/* 未获得视图复用分析列表的结构，只覆盖边框色并取消静态条目的可点击暗示。 */
.unowned-panel {
  border-color: rgb(31 119 103 / 22%);
}

.unowned-entry {
  cursor: default;
}

.unowned-entry:hover {
  border-color: var(--line-soft);
  background: rgb(252 249 241 / 70%);
  transform: none;
}

.unowned-result-count {
  color: var(--accent-deep);
}

.unowned-entry__mark {
  flex: 0 0 auto;
  color: var(--accent-deep);
  font-size: 10px;
  white-space: nowrap;
}

/* 把缺口和当前进度分层展示，避免只显示总数让用户误解为已拥有式神。 */
.unowned-entry__shards {
  display: grid;
  flex: 0 0 auto;
  gap: 2px;
  justify-items: end;
  text-align: right;
}

.unowned-entry__shards strong {
  color: var(--accent-deep);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.unowned-entry__shards small {
  color: var(--ink-faint);
  font-size: 9px;
  white-space: nowrap;
}

.my-shikigami-layout {
  display: grid;
  grid-template-columns: minmax(270px, 320px) minmax(0, 1fr);
  gap: 18px;
  align-items: start;
}

.roster-panel,
.record-hero,
.record-card {
  border: 1px solid var(--line-soft);
  background: rgb(255 255 255 / 62%);
  box-shadow: 0 12px 30px rgb(61 53 44 / 5%);
}

.roster-panel {
  position: sticky;
  top: 18px;
  max-height: calc(100vh - 150px);
  overflow: auto;
}

.roster-toolbar {
  padding: 13px;
  border-bottom: 1px solid var(--line-soft);
}

.roster-search {
  display: flex;
  gap: 8px;
  align-items: center;
  padding: 8px 10px;
  border: 1px solid var(--line);
  background: var(--paper);
}

.roster-search svg {
  width: 16px;
  height: 16px;
  fill: none;
  stroke: var(--ink-soft);
  stroke-width: 1.6;
}

.roster-search input {
  width: 100%;
  border: 0;
  outline: 0;
  color: var(--ink);
  background: transparent;
  font: inherit;
  font-size: 12px;
}

.roster-filter-row {
  display: flex;
  gap: 5px;
  margin-top: 10px;
}

.roster-filter {
  padding: 5px 9px;
  border: 1px solid transparent;
  border-radius: 999px;
  color: var(--ink-soft);
  background: transparent;
  cursor: pointer;
  font: inherit;
  font-size: 11px;
}

.roster-filter:hover,
.roster-filter--active {
  border-color: rgb(31 119 103 / 20%);
  color: var(--accent-deep);
  background: rgb(31 119 103 / 8%);
}

.roster-sort {
  display: flex;
  gap: 8px;
  align-items: center;
  margin-top: 10px;
  color: var(--ink-faint);
  font-size: 11px;
}

.roster-sort select {
  flex: 1;
  padding: 5px 7px;
  border: 1px solid var(--line-soft);
  color: var(--ink-soft);
  background: var(--paper);
  font: inherit;
  font-size: 11px;
}

.roster-list {
  display: grid;
  gap: 2px;
  padding: 8px;
}

.roster-entry {
  display: flex;
  gap: 9px;
  align-items: center;
  width: 100%;
  padding: 7px;
  border: 1px solid transparent;
  border-radius: 7px;
  color: var(--ink);
  background: transparent;
  text-align: left;
  cursor: pointer;
  font: inherit;
}

.roster-entry:hover,
.roster-entry--selected {
  border-color: rgb(31 119 103 / 18%);
  background: rgb(31 119 103 / 8%);
}

.roster-entry--selected .roster-entry__copy strong {
  color: var(--accent-deep);
}

.roster-avatar,
.record-avatar {
  position: relative;
  display: grid;
  place-items: center;
  overflow: hidden;
  color: white;
  background: #78736b;
  font-family: "Noto Serif SC", serif;
}

.roster-avatar {
  flex: 0 0 30px;
  width: 30px;
  height: 30px;
  border-radius: 50%;
  font-size: 12px;
}

.roster-avatar--ur,
.record-avatar--ur {
  background: linear-gradient(135deg, #8c5bb0, #4c3774);
}

.roster-avatar--sp,
.record-avatar--sp {
  background: #87506f;
}

.roster-avatar--ssr,
.record-avatar--ssr {
  background: #a3752f;
}

.roster-avatar--sr,
.record-avatar--sr {
  background: #4e8777;
}

.roster-avatar--r,
.record-avatar--r {
  background: #6e8fb4;
}

.roster-avatar--material,
.record-avatar--material {
  background: linear-gradient(135deg, #b87945, #765033);
}

.roster-avatar__image,
.record-avatar__image {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.roster-entry__copy {
  display: grid;
  flex: 1;
  min-width: 0;
  gap: 2px;
}

.roster-entry__copy strong {
  overflow: hidden;
  color: var(--ink);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.roster-entry__copy small,
.roster-entry__mark {
  color: var(--ink-faint);
  font-size: 10px;
}

.roster-entry__copy .roster-entry__story {
  color: var(--accent-deep);
}

.roster-entry__copy .roster-entry__story--pending {
  color: #a56523;
}

.roster-entry__mark {
  min-width: 20px;
  text-align: right;
}

.record-detail {
  display: grid;
  gap: 15px;
  min-width: 0;
}

.record-hero {
  display: flex;
  gap: 15px;
  align-items: center;
  padding: 19px 22px;
}

.record-avatar {
  flex: 0 0 78px;
  width: 78px;
  height: 78px;
  border-radius: 14px;
  font-size: 34px;
}

.record-hero__copy {
  flex: 1;
  min-width: 0;
}

.record-kicker {
  display: flex;
  gap: 8px;
  align-items: center;
  color: var(--ink-faint);
  font-size: 11px;
}

.rarity-label {
  padding: 3px 7px;
  border-radius: 5px;
  color: white;
  background: #79756d;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.08em;
}

.rarity-label--ur {
  background: #6b478b;
}

.rarity-label--sp {
  background: #87506f;
}

.rarity-label--ssr {
  background: #a3752f;
}

.rarity-label--sr {
  background: #4e8777;
}

.rarity-label--r {
  background: #6e8fb4;
}

.rarity-label--material {
  background: linear-gradient(135deg, #b87945, #765033);
}

.record-hero h2 {
  margin: 7px 0 3px;
  color: var(--ink);
  font-family: "Noto Serif SC", serif;
  font-size: clamp(23px, 3vw, 33px);
}

.record-hero p {
  margin: 0;
  color: var(--ink-soft);
  font-size: 12px;
}

/* Wiki 链接跟随式神摘要放置，保持可见但不抢占名称和培养信息的视觉焦点。 */
.record-wiki-link {
  display: inline-flex;
  width: fit-content;
  margin-top: 9px;
  padding: 4px 9px;
  border: 1px solid rgb(31 119 103 / 24%);
  border-radius: 999px;
  color: var(--accent-deep);
  background: rgb(31 119 103 / 7%);
  font-size: 11px;
  text-decoration: none;
  transition:
    color 160ms ease,
    background 160ms ease,
    border-color 160ms ease;
}

.record-wiki-link:hover,
.record-wiki-link:focus-visible {
  border-color: rgb(31 119 103 / 42%);
  color: white;
  background: var(--accent-deep);
}

.record-seal,
.empty-seal,
.state-seal {
  display: grid;
  place-items: center;
  border: 1px solid var(--cinnabar);
  color: var(--cinnabar);
  font-family: "Noto Serif SC", serif;
}

.record-seal {
  flex: 0 0 48px;
  padding: 7px 3px;
  font-size: 11px;
  line-height: 1.35;
  transform: rotate(3deg);
}

.record-card {
  padding: 18px 20px;
}

/* 传记解锁卡用暖色强调待完成状态，已完成状态沿用式神录的青绿色。 */
.record-card--story {
  border-color: rgb(31 119 103 / 22%);
}

.story-unlock-status {
  display: grid;
  gap: 6px;
  padding: 13px 14px;
  border-left: 3px solid var(--accent-deep);
  color: var(--ink-soft);
  background: rgb(31 119 103 / 6%);
}

.story-unlock-status strong {
  color: var(--accent-deep);
  font-size: 14px;
}

.story-unlock-status p {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
}

/* 规则来源使用低对比度小字，说明条件版本但不抢占传记进度的主视觉。 */
.story-rule-source {
  display: block;
  margin-top: 6px;
  color: var(--ink-faint);
  font-size: 10px;
}

.story-unlock-status--pending {
  border-left-color: #b4772f;
  background: rgb(180 119 47 / 7%);
}

.story-unlock-status--pending strong {
  color: #a56523;
}

.story-unlock-status--unknown {
  display: block;
  border: 1px dashed var(--line);
  border-left-width: 1px;
  color: var(--ink-faint);
  background: transparent;
  font-size: 12px;
  line-height: 1.7;
}

.story-biography-list {
  display: grid;
  gap: 6px;
  margin-top: 10px;
}

.story-biography-row {
  display: grid;
  grid-template-columns: minmax(52px, auto) minmax(62px, auto) 1fr;
  gap: 10px;
  align-items: center;
  padding: 8px 10px;
  border: 1px solid var(--line-soft);
  color: var(--ink-soft);
  background: rgb(252 249 241 / 62%);
  font-size: 11px;
}

.story-biography-row strong {
  color: #a56523;
  font-variant-numeric: tabular-nums;
  font-weight: 600;
}

.story-biography-row small {
  color: var(--ink-faint);
  text-align: right;
}

.story-biography-row--completed {
  border-color: rgb(31 119 103 / 18%);
  background: rgb(31 119 103 / 5%);
}

.story-biography-row--completed strong,
.story-biography-row--completed small {
  color: var(--accent-deep);
}

.record-card__heading {
  display: flex;
  gap: 12px;
  align-items: flex-start;
  justify-content: space-between;
  margin-bottom: 14px;
}

.record-card__heading > div {
  display: flex;
  gap: 9px;
  align-items: center;
}

.section-index {
  color: var(--cinnabar);
  font-family: ui-monospace, monospace;
  font-size: 11px;
}

.record-card h3 {
  margin: 0;
  color: var(--ink);
  font-family: "Noto Serif SC", serif;
  font-size: 17px;
}

.record-card__caption {
  color: var(--ink-faint);
  font-size: 11px;
}

.record-status-grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 8px;
}

.record-status-grid > div {
  display: grid;
  gap: 7px;
  padding: 11px 12px;
  border: 1px solid var(--line-soft);
  background: rgb(252 249 241 / 62%);
}

.record-status-grid span {
  color: var(--ink-faint);
  font-size: 11px;
}

.record-status-grid strong {
  color: var(--ink);
  font-size: 20px;
  font-variant-numeric: tabular-nums;
  font-weight: 600;
}

.instance-list {
  display: grid;
  gap: 8px;
}

.instance-row {
  display: grid;
  gap: 8px;
  padding: 12px;
  border-left: 2px solid rgb(31 119 103 / 36%);
  background: rgb(252 249 241 / 64%);
}

.instance-row__title {
  display: flex;
  gap: 10px;
  align-items: baseline;
  justify-content: space-between;
}

.instance-row__title strong {
  color: var(--ink);
  font-size: 13px;
}

.instance-row__title code {
  overflow: hidden;
  color: var(--ink-faint);
  font-size: 10px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.instance-row__tags,
.skill-list {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}

.instance-row__tags span,
.skill-pill {
  padding: 4px 7px;
  border: 1px solid var(--line-soft);
  border-radius: 999px;
  color: var(--ink-soft);
  background: rgb(255 255 255 / 58%);
  font-size: 10px;
}

.skill-pill {
  border-color: rgb(31 119 103 / 18%);
  color: var(--accent-deep);
  background: rgb(31 119 103 / 6%);
}

.skill-pill--muted {
  color: var(--ink-faint);
  background: transparent;
}

.record-source {
  display: flex;
  gap: 6px 15px;
  flex-wrap: wrap;
  color: var(--ink-faint);
  font-size: 10px;
  line-height: 1.5;
}

.my-shikigami-state,
.my-shikigami-empty {
  display: flex;
  gap: 14px;
  align-items: center;
  justify-content: center;
  min-height: 240px;
  border: 1px dashed var(--line);
  color: var(--ink);
}

.my-shikigami-state strong,
.my-shikigami-empty h2 {
  font-family: "Noto Serif SC", serif;
}

.my-shikigami-state p,
.my-shikigami-empty > div > p:not(.section-kicker) {
  max-width: 520px;
  margin: 5px 0 0;
  color: var(--ink-soft);
  font-size: 12px;
  line-height: 1.7;
}

.state-seal,
.empty-seal {
  flex: 0 0 44px;
  width: 44px;
  height: 44px;
  border-radius: 50%;
}

.my-shikigami-state--error {
  border-color: rgb(164 70 53 / 35%);
}

.my-shikigami-text-button {
  margin-top: 10px;
  padding: 0;
  border: 0;
  color: var(--accent-deep);
  background: none;
  cursor: pointer;
  font: inherit;
  font-size: 12px;
}

.my-shikigami-empty {
  justify-content: flex-start;
  padding: 38px 7vw;
}

.empty-seal {
  flex: 0 0 70px;
  width: 70px;
  height: 70px;
  border-radius: 12px;
  font-size: 28px;
  transform: rotate(-4deg);
}

.my-shikigami-empty h2 {
  margin: 7px 0 0;
  font-size: 25px;
}

.my-shikigami-empty .button {
  margin-top: 18px;
}

.roster-empty {
  padding: 30px 18px;
  color: var(--ink-soft);
  text-align: center;
  line-height: 1.8;
}

.roster-empty small {
  color: var(--ink-faint);
}

.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}

@media (max-width: 980px) {
  .my-shikigami-layout {
    grid-template-columns: minmax(230px, 270px) minmax(0, 1fr);
    gap: 12px;
  }

  .record-status-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@media (max-width: 720px) {
  .collection-metrics {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .unstarred-analysis__header {
    display: block;
    padding: 20px 18px 17px;
  }

  .analysis-back-button {
    margin-top: 14px;
  }

  .unstarred-analysis__toolbar {
    display: block;
    padding: 12px 18px;
  }

  .analysis-search {
    width: 100%;
  }

  .analysis-result-count {
    display: block;
    margin-top: 9px;
  }

  .unstarred-list {
    grid-template-columns: minmax(0, 1fr);
    padding: 14px 18px 18px;
  }

  .unstarred-analysis__empty {
    margin: 14px 18px 18px;
    padding: 18px;
  }

  .my-shikigami-layout {
    display: block;
  }

  .roster-panel {
    position: static;
    max-height: 390px;
    margin-bottom: 12px;
  }

  .my-shikigami-header {
    display: block;
  }

  .my-shikigami-header__meta {
    justify-content: flex-start;
    margin-top: 12px;
  }

  .my-shikigami-page {
    padding: 30px 18px 22px;
  }

  .record-hero {
    padding: 15px;
  }

  .record-avatar {
    flex-basis: 60px;
    width: 60px;
    height: 60px;
  }

  .record-seal {
    display: none;
  }

  .record-card {
    padding: 15px;
  }

  .record-card__caption {
    display: none;
  }

  .my-shikigami-empty {
    display: block;
    padding: 30px 22px;
  }

  .empty-seal {
    margin-bottom: 20px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .roster-entry,
  .roster-filter,
  .unstarred-analysis-trigger,
  .unstarred-entry,
  .unowned-trigger {
    transition: none;
  }
}
</style>
