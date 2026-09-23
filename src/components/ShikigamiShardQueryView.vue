<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import type {
  CharacterArchive,
  Shikigami,
  ShikigamiShardLookupCharacter,
  ShikigamiShardLookupResult,
  ShikigamiRarity,
} from "../api/contracts";
import {
  CURRENT_BUILTIN_SHIKIGAMI_COUNT,
  getRarityLabel,
  rarityOrder,
} from "../shikigamiCatalog";
import { toLocalDateTime } from "../timeFormat";
import PlatformBadge from "./PlatformBadge.vue";

type ResultFilter = "MATCH" | "OWNED" | "SHARDS" | "SUMMONABLE" | "ALL";
type ShardRarity = Extract<ShikigamiRarity, "UR" | "SP" | "SSR">;

/** 跨角色碎片查询计算可转赠的 UR、SSR、SP，其他稀有度不进入目标候选。 */
const TARGET_RARITIES: ReadonlySet<string> = new Set(["UR", "SP", "SSR"]);
/** 稀有度下拉沿用游戏展示顺序，ALL 表示 UR、SP 与 SSR 的全部候选。 */
const shardRarityOptions: readonly ShardRarity[] = ["UR", "SP", "SSR"];
/** 仅保存查询条件，不写入角色档案或碎片读取结果。 */
const SHARD_QUERY_STORAGE_KEY = "yys-analysis.shikigami-shard-query.v2";

type SavedShardQuery = {
  version: 2;
  selectedRecipientIdentityKey: string;
  selectedShikigamiId: string;
  selectedRarity: ShardRarity | "ALL";
};

/** 校验本地存储里的稀有度，损坏或旧版本值直接回退默认范围。 */
function isShardRarity(value: unknown): value is ShardRarity {
  return (
    typeof value === "string" && shardRarityOptions.some((rarity) => rarity === value)
  );
}

const catalog = ref<Shikigami[]>([]);
const catalogLoading = ref(true);
const catalogError = ref("");
/** 接收碎片的角色档案；仅用于选择接收方，不会切换应用当前角色。 */
const characterArchives = ref<CharacterArchive[]>([]);
const characterArchivesLoading = ref(true);
const characterArchivesError = ref("");
/** 接收方身份键；空值表示尚未完成第一步选择。 */
const selectedRecipientIdentityKey = ref("");
const search = ref("");
/** 当前式神候选的稀有度范围；默认全部，避免新增筛选改变原有查询结果。 */
const selectedRarity = ref<ShardRarity | "ALL">("ALL");
const suggestionsOpen = ref(false);
const selectedShikigami = ref<Shikigami | null>(null);
const lookupResult = ref<ShikigamiShardLookupResult | null>(null);
const lookupLoading = ref(false);
const lookupError = ref("");
const searchMessage = ref("");
const activeFilter = ref<ResultFilter>("MATCH");

/** 游戏内三条供养渠道的理论速率：寮祈愿 1/天、三花好友 1/天、蓝牙/Wi-Fi 1/3天。 */
const DONATION_RATE_NUMERATOR = 7;
const DONATION_RATE_DENOMINATOR = 3;

/** 查询筛选名称固定从用户任务出发，不暴露后端字段名。 */
const filterOptions: ReadonlyArray<{ value: ResultFilter; label: string }> = [
  { value: "MATCH", label: "全部匹配" },
  { value: "OWNED", label: "已拥有" },
  { value: "SHARDS", label: "持有碎片" },
  { value: "SUMMONABLE", label: "可召唤" },
  { value: "ALL", label: "全部档案" },
];

/** 目录先按稀有度收窄，再使用游戏稀有度顺序稳定展示候选。 */
const eligibleCatalog = computed(() =>
  catalog.value
    .filter(
      (entry) =>
        TARGET_RARITIES.has(entry.rarity) &&
        (selectedRarity.value === "ALL" || entry.rarity === selectedRarity.value),
    )
    .sort((left, right) => {
      const rarityDifference = rarityOrder(left.rarity) - rarityOrder(right.rarity);
      return rarityDifference || left.name.localeCompare(right.name, "zh-CN");
    }),
);

/** 搜索建议支持名称或稳定 ID 模糊匹配；输入为空时展示当前稀有度下的完整下拉。 */
const suggestions = computed(() => {
  const query = search.value.trim().toLocaleLowerCase("zh-CN");
  const candidates = query
    ? eligibleCatalog.value.filter(
        (entry) =>
          entry.name.toLocaleLowerCase("zh-CN").includes(query) ||
          entry.shikigamiId.includes(query),
      )
    : eligibleCatalog.value;
  // 下拉自身带滚动条，保留全部模糊匹配，避免候选超过固定条数后无法选择。
  return candidates;
});

const characters = computed(() => lookupResult.value?.characters ?? []);
const selectedRecipient = computed(
  () =>
    characterArchives.value.find(
      (entry) => entry.identityKey === selectedRecipientIdentityKey.value,
    ) ?? null,
);
/** 查询结果中的接收方行；档案被删除或结果过期时保持 null，避免错认其他账号。 */
const recipientCharacter = computed(
  () =>
    characters.value.find(
      (entry) => entry.identityKey === selectedRecipientIdentityKey.value,
    ) ?? null,
);
/** 供养方只包含接收方以外的档案，接收方数据在上方独立卡片中展示。 */
const donorCharacters = computed(() =>
  characters.value.filter(
    (entry) => entry.identityKey !== selectedRecipientIdentityKey.value,
  ),
);
const availableCharacters = computed(() =>
  donorCharacters.value.filter((entry) => entry.dataAvailable),
);
const unavailableCount = computed(
  () => donorCharacters.value.length - availableCharacters.value.length,
);
const matchedCount = computed(
  () =>
    donorCharacters.value.filter(
      (entry) => entry.ownedCount > 0 || entry.shardCount > 0,
    ).length,
);
const totalShardCount = computed(() =>
  donorCharacters.value
    .filter((entry) => entry.dataAvailable)
    .reduce((sum, entry) => sum + entry.shardCount, 0),
);

/** 接收方当前碎片数与召唤上限；未读取时仍显示 0，缺口会在状态区解释。 */
const recipientShardCount = computed(() => recipientCharacter.value?.shardCount ?? 0);
const recipientDataAvailable = computed(
  () => recipientCharacter.value?.dataAvailable ?? false,
);
const recipientRequiredShardCount = computed(() =>
  requiredShardCount(recipientCharacter.value ?? undefined),
);
const recipientShardGap = computed(() =>
  Math.max(recipientRequiredShardCount.value - recipientShardCount.value, 0),
);
/** 其他角色已经读取到的同一式神碎片总量，不自动把黑蛋潜力计入直接供给。 */
const directDonorShardCount = computed(() =>
  donorCharacters.value
    .filter((entry) => entry.dataAvailable)
    .reduce((sum, entry) => sum + entry.shardCount, 0),
);
/** 已拥有但传记二未完成的供养方，完成各自传记规则后预计可额外提供的 10 片。 */
const unlockableDonorShardCount = computed(() =>
  donorCharacters.value.reduce(
    (sum, entry) =>
      sum +
      (entry.dataAvailable && entry.shardUnlocked === false
        ? (entry.unlockableShardCount ?? 0)
        : 0),
    0,
  ),
);
const directSupplyGap = computed(() =>
  Math.max(recipientShardGap.value - directDonorShardCount.value, 0),
);
const potentialSupplyGap = computed(() =>
  Math.max(
    recipientShardGap.value - directDonorShardCount.value - unlockableDonorShardCount.value,
    0,
  ),
);
/** 按用户提供的三条渠道估算送满天数，缺口为 0 时显示当天已满足。 */
const estimatedDonationDays = computed(() =>
  recipientShardGap.value === 0
    ? 0
    : Math.ceil(
        (recipientShardGap.value * DONATION_RATE_DENOMINATOR) /
          DONATION_RATE_NUMERATOR,
      ),
);

/** 没有独立碎片记录时，召唤上限按目录稀有度补齐：SSR 50，UR/SP 60。 */
function requiredShardCount(entry?: ShikigamiShardLookupCharacter): number {
  if (entry?.maxShardCount) return entry.maxShardCount;
  if (selectedShikigami.value?.rarity === "SSR") return 50;
  if (
    selectedShikigami.value?.rarity === "UR" ||
    selectedShikigami.value?.rarity === "SP"
  ) {
    return 60;
  }
  return 0;
}

/** 所有筛选按钮和结果列表共用同一条判定，保证按钮数量与实际行数一致。 */
function matchesFilter(
  entry: ShikigamiShardLookupCharacter,
  filter: ResultFilter,
): boolean {
  if (filter === "ALL") return true;
  if (!entry.dataAvailable) return false;
  if (filter === "OWNED") return entry.ownedCount > 0;
  if (filter === "SHARDS") return entry.shardCount > 0;
  if (filter === "SUMMONABLE") {
    const required = requiredShardCount(entry);
    return required > 0 && entry.shardCount >= required;
  }
  return entry.ownedCount > 0 || entry.shardCount > 0;
}

/** 结果优先显示碎片多的角色，其次是已拥有角色，最后按昵称稳定排序。 */
const filteredCharacters = computed(() =>
  donorCharacters.value
    .filter((entry) => matchesFilter(entry, activeFilter.value))
    .sort((left, right) => {
      if (left.dataAvailable !== right.dataAvailable)
        return left.dataAvailable ? -1 : 1;
      const shardDifference = right.shardCount - left.shardCount;
      if (shardDifference !== 0) return shardDifference;
      const ownedDifference = right.ownedCount - left.ownedCount;
      return (
        ownedDifference || left.displayName.localeCompare(right.displayName, "zh-CN")
      );
    }),
);

/** 筛选徽标数量不受当前筛选影响，方便用户预判切换后的结果规模。 */
function filterCount(filter: ResultFilter): number {
  return donorCharacters.value.filter((entry) => matchesFilter(entry, filter)).length;
}

/** 目录状态异常时安装随应用打包的本地目录；整个过程不访问远程网络。 */
async function loadCatalog(): Promise<void> {
  catalogLoading.value = true;
  catalogError.value = "";
  try {
    let status = await desktopApi.getCatalogStatus();
    if (
      !status ||
      status.validationStatus !== "verified" ||
      status.shikigamiCount < CURRENT_BUILTIN_SHIKIGAMI_COUNT
    ) {
      status = await desktopApi.installBuiltinCatalog();
    }
    catalog.value = await desktopApi.listShikigami(status.version);
  } catch (error) {
    catalog.value = [];
    catalogError.value =
      error instanceof Error ? error.message : "本地式神目录暂时不可用";
  } finally {
    catalogLoading.value = false;
  }
}

/** 读取已保存的角色档案清单；失败时保留空清单并让界面明确提示无法选择接收方。 */
async function loadCharacterArchives(): Promise<void> {
  characterArchivesLoading.value = true;
  characterArchivesError.value = "";
  try {
    const result = await desktopApi.listCharacterArchives();
    characterArchives.value = result.characters;
  } catch (error) {
    characterArchives.value = [];
    characterArchivesError.value =
      error instanceof Error ? error.message : "角色档案暂时不可用";
  } finally {
    characterArchivesLoading.value = false;
  }
}

/** 保存接收方、式神和稀有度，供下次打开页面恢复同一组供养计划。 */
function saveQueryCondition(): void {
  try {
    if (!selectedRecipientIdentityKey.value && !selectedShikigami.value) {
      window.localStorage.removeItem(SHARD_QUERY_STORAGE_KEY);
      return;
    }
    const saved: SavedShardQuery = {
      version: 2,
      selectedRecipientIdentityKey: selectedRecipientIdentityKey.value,
      selectedShikigamiId: selectedShikigami.value?.shikigamiId ?? "",
      selectedRarity: selectedRarity.value,
    };
    window.localStorage.setItem(SHARD_QUERY_STORAGE_KEY, JSON.stringify(saved));
  } catch {
    // 本地存储不可用时仍保留当前页面查询，不把存储异常升级成业务错误。
  }
}

/** 目录和档案加载完成后恢复上次接收方与式神；任一目标失效时回退为空白步骤。 */
function restoreSavedQuery(): void {
  let raw: string | null;
  try {
    raw = window.localStorage.getItem(SHARD_QUERY_STORAGE_KEY);
  } catch {
    return;
  }
  if (!raw) return;

  let saved: Partial<SavedShardQuery>;
  try {
    saved = JSON.parse(raw) as Partial<SavedShardQuery>;
  } catch {
    return;
  }
  if (
    saved.version !== 2 ||
    typeof saved.selectedRecipientIdentityKey !== "string" ||
    typeof saved.selectedShikigamiId !== "string"
  ) {
    return;
  }

  const recipient = characterArchives.value.find(
    (entry) => entry.identityKey === saved.selectedRecipientIdentityKey,
  );
  if (!recipient) return;
  selectedRecipientIdentityKey.value = recipient.identityKey;

  const savedRarity = saved.selectedRarity;
  if (savedRarity === "ALL") selectedRarity.value = "ALL";
  else if (isShardRarity(savedRarity)) selectedRarity.value = savedRarity;
  if (saved.selectedShikigamiId) {
    const target = eligibleCatalog.value.find(
      (entry) => entry.shikigamiId === saved.selectedShikigamiId,
    );
    if (target) void selectShikigami(target);
  }
}

/** 选择接收方后清理旧查询；若已有式神则重新读取以更新供养方缺口。 */
function handleRecipientChange(): void {
  lookupError.value = "";
  lookupResult.value = null;
  activeFilter.value = "MATCH";
  searchMessage.value = "";
  if (!selectedRecipientIdentityKey.value) {
    selectedShikigami.value = null;
    search.value = "";
    saveQueryCondition();
    return;
  }
  saveQueryCondition();
  if (selectedShikigami.value) void selectShikigami(selectedShikigami.value);
}

/** 选择式神后读取全部角色，期间保留式神签但清空旧角色结果。 */
async function selectShikigami(entry: Shikigami): Promise<void> {
  if (!selectedRecipientIdentityKey.value) {
    searchMessage.value = "请先选择接收碎片的角色，再选择式神";
    return;
  }
  selectedShikigami.value = entry;
  search.value = entry.name;
  suggestionsOpen.value = false;
  searchMessage.value = "";
  lookupError.value = "";
  lookupResult.value = null;
  activeFilter.value = "MATCH";
  saveQueryCondition();
  lookupLoading.value = true;
  try {
    const result = await desktopApi.lookupShikigamiShards(entry.shikigamiId);
    // 用户在上一条请求返回前选择了其他式神时，旧结果不得覆盖新的目标签。
    if (selectedShikigami.value?.shikigamiId === entry.shikigamiId) {
      lookupResult.value = result;
    }
  } catch (error) {
    if (selectedShikigami.value?.shikigamiId === entry.shikigamiId) {
      lookupError.value = error instanceof Error ? error.message : "跨角色碎片查询失败";
    }
  } finally {
    if (selectedShikigami.value?.shikigamiId === entry.shikigamiId) {
      lookupLoading.value = false;
    }
  }
}

/** 回车或查询按钮优先使用名称完全匹配项，否则选取当前建议首项。 */
function submitSearch(): void {
  if (!selectedRecipientIdentityKey.value) {
    searchMessage.value = "请先选择接收碎片的角色，再查询式神";
    return;
  }
  const query = search.value.trim();
  if (!query) {
    searchMessage.value = "请先输入式神名称";
    return;
  }
  const exact = eligibleCatalog.value.find(
    (entry) =>
      entry.name.toLocaleLowerCase("zh-CN") === query.toLocaleLowerCase("zh-CN") ||
      entry.shikigamiId === query,
  );
  const target = exact ?? suggestions.value[0];
  if (!target) {
    const rarityScope =
      selectedRarity.value === "ALL" ? "UR、SP、SSR" : selectedRarity.value;
    searchMessage.value = `没有在 ${rarityScope} 式神中找到匹配项`;
    return;
  }
  void selectShikigami(target);
}

/** 切换稀有度会清空旧式神和结果，但保留接收方，避免供养对象被悄悄替换。 */
function handleRarityChange(): void {
  search.value = "";
  suggestionsOpen.value = false;
  selectedShikigami.value = null;
  lookupResult.value = null;
  lookupLoading.value = false;
  lookupError.value = "";
  searchMessage.value = "";
  activeFilter.value = "MATCH";
  saveQueryCondition();
}

/** 用户改写已选名称后立即解除旧目标，避免旧结果被误认为新搜索词的结果。 */
function handleSearchInput(): void {
  suggestionsOpen.value = true;
  searchMessage.value = "";
  if (selectedShikigami.value && search.value !== selectedShikigami.value.name) {
    selectedShikigami.value = null;
    lookupResult.value = null;
    lookupError.value = "";
  }
}

/** 焦点仍在搜索区内部时保持建议列表，移出整个组合框后再关闭。 */
function handleSearchFocusOut(event: FocusEvent): void {
  const container = event.currentTarget;
  const next = event.relatedTarget;
  if (
    container instanceof HTMLElement &&
    next instanceof Node &&
    container.contains(next)
  ) {
    return;
  }
  suggestionsOpen.value = false;
}

/** 返回随应用打包的本地头像，查询页不依赖运行时网络资源。 */
function avatarPath(entry: Shikigami): string {
  return `${import.meta.env.BASE_URL}shikigami-avatars/${entry.shikigamiId}.webp`;
}

/** 头像文件缺失时隐藏图片，让稀有度底色和“式”字继续承担识别。 */
function hideBrokenAvatar(event: Event): void {
  const image = event.currentTarget;
  if (image instanceof HTMLImageElement) image.hidden = true;
}

/** 名册头像使用角色昵称首字，空白旧昵称回退问号。 */
function characterInitial(name: string): string {
  return [...name.trim()][0] ?? "?";
}

/** 碎片进度限制在 0～100%，超过召唤上限时仍保持满格。 */
function shardProgress(entry: ShikigamiShardLookupCharacter): number {
  const required = requiredShardCount(entry);
  if (required <= 0) return 0;
  return Math.min(100, Math.max(0, (entry.shardCount / required) * 100));
}

/** 比较单个供养方与接收方缺口，明确告诉用户“满足”还是还差多少片。 */
function donorSupplyNote(entry: ShikigamiShardLookupCharacter): string {
  if (!entry.dataAvailable) return "无法判断碎片供给，请重新读取该角色的式神数据";
  if (!recipientDataAvailable.value) return "接收方没有可用式神数据，暂时无法计算缺口";
  if (recipientShardGap.value <= 0) return "目标角色碎片已满足，无需继续供养";
  if (entry.shardCount >= recipientShardGap.value) {
    return `碎片满足，可直接供养${recipientShardGap.value}片`;
  }
  return `碎片不满足，缺口${recipientShardGap.value - entry.shardCount}片`;
}

/** 汇总供养渠道和黑蛋潜力，只有直接供给足够时才承诺“预计送满”。 */
function donationEstimateNote(): string {
  if (!recipientDataAvailable.value) return "接收方未读取式神数据，无法计算送满天数";
  if (recipientShardGap.value <= 0) return "目标角色已满足召唤所需碎片";
  if (directDonorShardCount.value >= recipientShardGap.value) {
    return `预计${estimatedDonationDays.value}天可以送满`;
  }
  if (potentialSupplyGap.value === 0) {
    return `直接碎片还缺${directSupplyGap.value}片；完成传记解锁（技能类可用黑蛋）后预计${estimatedDonationDays.value}天可以送满`;
  }
  return `当前供给仍缺${potentialSupplyGap.value}片，暂时无法送满`;
}

/** 公开规则类型对应的人类可读单位；等级条件不能显示成黑蛋数量。 */
function unlockRemainingLabel(entry: ShikigamiShardLookupCharacter): string {
  const remaining = entry.shardUnlockRemaining ?? 0;
  if (entry.shardUnlockProgressKind === "level") return `${remaining}级`;
  if (entry.shardUnlockProgressKind === "skillUpgrade") {
    return `${remaining}次技能提升`;
  }
  if (entry.shardUnlockProgressKind === "shardCollection") return `${remaining}片`;
  if (entry.shardUnlockProgressKind === "clanPrayer") return `${remaining}次祈愿`;
  if (
    entry.shardUnlockProgressKind === "battleWin" ||
    entry.shardUnlockProgressKind === "teamBattle"
  ) {
    return `${remaining}次胜利`;
  }
  return `${remaining}项`;
}

/** 仅对已拥有但传记二未完成的角色给出公开条件提示；旧档案不猜测状态。 */
function shardUnlockNote(entry: ShikigamiShardLookupCharacter): string | null {
  if (
    !entry.dataAvailable ||
    entry.ownedCount <= 0 ||
    entry.shardUnlocked !== false
  ) {
    return null;
  }
  const unlockableShardCount = entry.unlockableShardCount;
  const shardUnlockRemaining = entry.shardUnlockRemaining;
  if (entry.shardUnlockProgressKind === "unknown") {
    return `提升该角色，可以提供${unlockableShardCount ?? 10}枚碎片；传记二进度 ${entry.shardUnlockCurrent ?? 0}/${entry.shardUnlockRequired ?? 0}，公开规则未收录，不能换算御行达摩`;
  }
  if (
    typeof unlockableShardCount !== "number" ||
    typeof shardUnlockRemaining !== "number"
  ) {
    return "已拥有，但碎片解锁进度未读取";
  }
  const condition = entry.shardUnlockCondition ?? "公开规则已收录";
  const targetName = selectedShikigami.value?.name ?? "该式神";
  if (
    entry.shardUnlockProgressKind === "skillUpgrade" &&
    shardUnlockRemaining > 0
  ) {
    return `传记二条件：${condition}；可考虑给${entry.displayName}的${targetName}喂${shardUnlockRemaining}个御行达摩，解锁${unlockableShardCount}枚碎片`;
  }
  return `传记二条件：${condition}；提升${entry.displayName}的${targetName}可以提供${unlockableShardCount}枚碎片，还需${unlockRemainingLabel(entry)}`;
}

/** 无角色档案时跳到现有读取入口，由应用壳决定实际导航页面。 */
function openDataReading(): void {
  window.dispatchEvent(new Event("open-desktop-reading"));
}

/** 页面首次打开先加载本地目录，再恢复上次式神并自动刷新跨角色结果。 */
onMounted(async () => {
  await Promise.all([loadCatalog(), loadCharacterArchives()]);
  restoreSavedQuery();
});
</script>

<template>
  <main class="page-main shard-query-page" aria-labelledby="shard-query-title">
    <header class="shard-query-header">
      <div>
        <p class="eyebrow">CROSS-ARCHIVE / SHARD LOOKUP</p>
        <h1 id="shard-query-title">跨角色碎片查询</h1>
        <p>选一个式神，一次查清本机全部游戏档案的拥有状态与碎片数量。</p>
      </div>
      <span class="shard-query-scope"><i aria-hidden="true"></i>全部游戏档案</span>
    </header>

    <section class="shard-search-ledger" aria-labelledby="shard-search-title">
      <span class="shard-search-ledger__seal" aria-hidden="true">寻</span>
      <div class="shard-search-ledger__body">
        <div class="shard-search-ledger__heading">
          <div>
            <p class="section-kicker">TARGET SHIKIGAMI</p>
            <h2 id="shard-search-title">要找哪位式神？</h2>
          </div>
          <span
            >当前筛选
            {{ selectedRarity === "ALL" ? "UR / SP / SSR" : selectedRarity }}</span
          >
        </div>

        <form class="shard-search-form" @submit.prevent="submitSearch">
          <label class="shard-recipient-select" for="shard-recipient-select">
            <span>接收碎片的角色</span>
            <select
              id="shard-recipient-select"
              v-model="selectedRecipientIdentityKey"
              :disabled="characterArchivesLoading || characterArchives.length === 0"
              @change="handleRecipientChange"
            >
              <option value="">请先选择角色</option>
              <option
                v-for="character in characterArchives"
                :key="character.identityKey"
                :value="character.identityKey"
              >
                {{ character.displayName }} · {{ character.serverLabel ?? "区服未知" }}{{
                  character.isCurrent ? " · 当前" : ""
                }}
              </option>
            </select>
          </label>
          <div class="shard-search-fields">
            <label class="shard-rarity-select" for="shard-rarity-select">
              <span>稀有度</span>
              <select
                id="shard-rarity-select"
                v-model="selectedRarity"
                :disabled="catalogLoading || !selectedRecipientIdentityKey"
                @change="handleRarityChange"
              >
                <option value="ALL">全部稀有度</option>
                <option
                  v-for="rarity in shardRarityOptions"
                  :key="rarity"
                  :value="rarity"
                >
                  {{ getRarityLabel(rarity) }}
                </option>
              </select>
            </label>

            <div
              class="shard-search-combobox"
              @focusin="suggestionsOpen = true"
              @focusout="handleSearchFocusOut"
            >
              <label for="shard-search-input">式神名称</label>
              <div class="shard-search-input-wrap">
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <circle cx="10.5" cy="10.5" r="6.5" />
                  <path d="m16 16 5 5" />
                </svg>
                <input
                  id="shard-search-input"
                  v-model="search"
                  type="search"
                  role="combobox"
                  autocomplete="off"
                  placeholder="点击下拉选择，或输入名称"
                  aria-autocomplete="list"
                  aria-controls="shard-search-suggestions"
                  :aria-expanded="suggestionsOpen && suggestions.length > 0"
                  :disabled="catalogLoading || !selectedRecipientIdentityKey"
                  @input="handleSearchInput"
                  @keydown.esc="suggestionsOpen = false"
                />
              </div>
              <ul
                v-if="suggestionsOpen && suggestions.length > 0"
                id="shard-search-suggestions"
                class="shard-search-suggestions"
                role="listbox"
                aria-label="匹配的式神"
              >
                <li v-for="entry in suggestions" :key="entry.shikigamiId" role="none">
                  <button
                    type="button"
                    role="option"
                    :aria-selected="
                      selectedShikigami?.shikigamiId === entry.shikigamiId
                    "
                    @click="selectShikigami(entry)"
                  >
                    <span
                      :class="[
                        'suggestion-avatar',
                        `rarity-${entry.rarity.toLowerCase()}`,
                      ]"
                    >
                      <img :src="avatarPath(entry)" alt="" @error="hideBrokenAvatar" />
                      <i aria-hidden="true">式</i>
                    </span>
                    <span>
                      <strong>{{ entry.name }}</strong>
                      <small
                        >{{ getRarityLabel(entry.rarity) }} ·
                        {{ entry.shikigamiId }}</small
                      >
                    </span>
                  </button>
                </li>
              </ul>
            </div>
          </div>
          <button
            class="shard-search-submit"
            type="submit"
            :disabled="
              catalogLoading ||
              characterArchivesLoading ||
              !selectedRecipientIdentityKey ||
              lookupLoading
            "
          >
            {{ lookupLoading ? "正在查找" : "查询可供养角色" }}
          </button>
        </form>

        <p v-if="characterArchivesLoading" class="shard-search-note" role="status">
          正在读取角色档案清单…
        </p>
        <p
          v-else-if="characterArchivesError"
          class="shard-search-note shard-search-note--error"
          role="alert"
        >
          角色档案不可用：{{ characterArchivesError }}
        </p>
        <p v-else-if="characterArchives.length === 0" class="shard-search-note shard-search-note--error" role="alert">
          还没有可选角色，请先完成一次数据读取。
        </p>
        <p v-else-if="catalogLoading" class="shard-search-note" role="status">
          正在打开本地式神目录…
        </p>
        <p
          v-else-if="catalogError"
          class="shard-search-note shard-search-note--error"
          role="alert"
        >
          式神目录不可用：{{ catalogError }}
        </p>
        <p
          v-else-if="searchMessage"
          class="shard-search-note shard-search-note--error"
          role="alert"
        >
          {{ searchMessage }}
        </p>
        <p v-else class="shard-search-note">
          先选接收碎片的角色，再选择式神；其他角色的碎片会按“满足/缺口”列出。上次的接收方和式神会自动恢复。
        </p>
      </div>
    </section>

    <section v-if="selectedShikigami" class="shard-target" aria-live="polite">
      <div class="shard-target__identity">
        <span
          :class="[
            'shard-target__avatar',
            `rarity-${selectedShikigami.rarity.toLowerCase()}`,
          ]"
        >
          <img :src="avatarPath(selectedShikigami)" alt="" @error="hideBrokenAvatar" />
          <i aria-hidden="true">式</i>
        </span>
        <div>
          <small>供养目标式神</small>
          <h2>{{ selectedShikigami.name }}</h2>
          <span
            >{{ getRarityLabel(selectedShikigami.rarity) }} · 召唤需要
            {{ recipientRequiredShardCount }} 片</span
          >
        </div>
      </div>
      <div class="shard-target__recipient">
        <span>接收角色</span>
        <strong>{{ selectedRecipient?.displayName ?? "未选择" }}</strong>
        <small>{{ selectedRecipient?.serverLabel ?? "区服未知" }}</small>
      </div>
      <dl v-if="lookupResult" class="shard-target__summary">
        <div>
          <dt>目标现有</dt>
          <dd>
            {{ recipientShardCount }}<small>/{{ recipientRequiredShardCount }}片</small>
          </dd>
        </div>
        <div>
          <dt>目标缺口</dt>
          <dd>{{ recipientShardGap }}<small>片</small></dd>
        </div>
        <div>
          <dt>其他角色可供养</dt>
          <dd>{{ totalShardCount }}<small>片</small></dd>
        </div>
        <div class="shard-target__summary-total">
          <dt>送满估算</dt>
          <dd>
            <template v-if="recipientDataAvailable">{{ estimatedDonationDays }}<small>天</small></template>
            <template v-else><small>待读取</small></template>
          </dd>
        </div>
      </dl>
      <p v-if="lookupResult" class="shard-target__estimate">
        {{ donationEstimateNote() }}；其他角色当前可供养{{ directDonorShardCount }}片，完成传记解锁后可再增加{{ unlockableDonorShardCount }}片（技能类可用御行达摩）；按寮祈愿每天 1 片、三花好友每天 1 片、蓝牙/Wi-Fi 每 3 天 1 片估算。
      </p>
    </section>

    <section
      v-if="selectedShikigami"
      class="shard-results"
      aria-labelledby="shard-results-title"
    >
      <header class="shard-results__header">
        <div>
          <p class="section-kicker">CHARACTER MATCHES</p>
          <h2 id="shard-results-title">其他角色供养</h2>
          <p v-if="lookupResult">
            {{ matchedCount }} 个角色有匹配记录，接收方已单独列出
            <template v-if="unavailableCount > 0">
              ，{{ unavailableCount }} 个档案待补读
            </template>
          </p>
        </div>
        <div v-if="lookupResult" class="shard-filter-row" aria-label="结果筛选">
          <button
            v-for="filter in filterOptions"
            :key="filter.value"
            type="button"
            :class="{ 'shard-filter--active': activeFilter === filter.value }"
            :aria-pressed="activeFilter === filter.value"
            @click="activeFilter = filter.value"
          >
            {{ filter.label }}
            <b>{{ filterCount(filter.value) }}</b>
          </button>
        </div>
      </header>

      <div v-if="lookupLoading" class="shard-query-state" role="status">
        <span aria-hidden="true">寻</span>
        <div>
          <strong>正在翻阅角色档案</strong>
          <p>逐份读取本机式神记录，不改变当前角色。</p>
        </div>
      </div>
      <div
        v-else-if="lookupError"
        class="shard-query-state shard-query-state--error"
        role="alert"
      >
        <span aria-hidden="true">!</span>
        <div>
          <strong>碎片查询暂时不可用</strong>
          <p>{{ lookupError }}</p>
          <button type="button" @click="selectShikigami(selectedShikigami)">
            重新查询
          </button>
        </div>
      </div>
      <div
        v-else-if="lookupResult && characters.length === 0"
        class="shard-query-state"
        role="status"
      >
        <span aria-hidden="true">零</span>
        <div>
          <strong>还没有游戏档案</strong>
          <p>先读取至少一个角色，再回来横向查询式神与碎片。</p>
          <button type="button" @click="openDataReading">前往数据读取</button>
        </div>
      </div>
      <div
        v-else-if="lookupResult && filteredCharacters.length === 0"
        class="shard-query-state"
        role="status"
      >
        <span aria-hidden="true">无</span>
        <div>
          <strong>当前筛选没有匹配角色</strong>
          <p>可以切换“全部档案”检查哪些角色还没有读取式神数据。</p>
        </div>
      </div>

      <article v-if="lookupResult" class="shard-recipient-card">
        <div class="shard-recipient-card__heading">
          <span class="shard-recipient-card__seal" aria-hidden="true">收</span>
          <div>
            <p class="section-kicker">RECEIVER</p>
            <h3>{{ selectedRecipient?.displayName ?? "接收方" }}</h3>
            <p>{{ selectedRecipient?.serverLabel ?? "区服未知" }} · {{ selectedShikigami.name }}</p>
          </div>
          <span class="shard-badge shard-badge--owned">接收方</span>
        </div>
        <div v-if="recipientCharacter && recipientDataAvailable" class="shard-recipient-card__progress">
          <div>
            <span>自己的碎片</span>
            <strong>{{ recipientShardCount }}<small>/{{ recipientRequiredShardCount }}</small></strong>
          </div>
          <span class="shard-progress-track" aria-hidden="true">
            <i :style="{ width: `${shardProgress(recipientCharacter)}%` }"></i>
          </span>
          <p v-if="recipientShardGap > 0">还缺{{ recipientShardGap }}片；其他角色可在下方供养。</p>
          <p v-else>自己的碎片已经满足召唤需求。</p>
          <small v-if="shardUnlockNote(recipientCharacter)" class="shard-character__unlock-note">
            {{ shardUnlockNote(recipientCharacter) }}
          </small>
        </div>
        <div v-else class="shard-character__warning">
          <strong>接收方无法判断持有状态</strong>
          <p>{{ recipientCharacter?.warning ?? "请重新读取接收方的式神数据" }}</p>
        </div>
      </article>

      <div
        v-if="lookupResult && filteredCharacters.length > 0"
        class="shard-character-list"
        role="list"
      >
        <article
          v-for="character in filteredCharacters"
          :key="character.identityKey"
          :class="[
            'shard-character',
            { 'shard-character--unavailable': !character.dataAvailable },
          ]"
          role="listitem"
        >
          <span class="shard-character__initial" aria-hidden="true">
            {{ characterInitial(character.displayName) }}
          </span>
          <div class="shard-character__identity">
            <div>
              <h3>{{ character.displayName }}</h3>
              <span
                v-if="character.ownedCount > 0"
                class="shard-badge shard-badge--owned"
              >
                已拥有 {{ character.ownedCount }} 只
              </span>
              <span
                v-if="character.dataAvailable && character.shardCount > 0"
                class="shard-badge shard-badge--shards"
              >
                有碎片
              </span>
              <span
                v-if="
                  character.dataAvailable &&
                  character.ownedCount > 0 &&
                  character.shardUnlocked === false
                "
                class="shard-badge shard-badge--unlock-pending"
              >
                碎片未解锁
              </span>
              <span
                v-if="
                  character.dataAvailable &&
                  requiredShardCount(character) > 0 &&
                  character.shardCount >= requiredShardCount(character)
                "
                class="shard-badge shard-badge--summonable"
              >
                可召唤
              </span>
              <span
                v-if="!character.dataAvailable"
                class="shard-badge shard-badge--unknown"
              >
                未读取
              </span>
            </div>
            <p>
              {{ character.serverLabel ?? "区服未知" }}
              <PlatformBadge
                v-if="character.platform"
                :platform="character.platform"
                compact
              />
              <template v-if="character.playerId">
                · ID {{ character.playerId }}
              </template>
            </p>
            <small
              >最后更新 {{ toLocalDateTime(character.lastUpdatedAt) ?? "未知" }}</small
            >
          </div>

          <div v-if="character.dataAvailable" class="shard-character__progress">
            <div>
              <span>碎片</span>
              <strong>
                {{ character.shardCount
                }}<small>/{{ requiredShardCount(character) }}</small>
              </strong>
            </div>
            <span class="shard-progress-track" aria-hidden="true">
              <i :style="{ width: `${shardProgress(character)}%` }"></i>
            </span>
            <small
              v-if="shardUnlockNote(character)"
              class="shard-character__unlock-note"
              >{{ shardUnlockNote(character) }}</small
            >
            <small class="shard-character__supply-note">
              {{ donorSupplyNote(character) }}
            </small>
          </div>
          <div v-else class="shard-character__warning">
            <strong>无法判断持有状态</strong>
            <p>{{ character.warning ?? "请重新读取该角色的式神数据" }}</p>
          </div>
        </article>
      </div>
    </section>

    <section v-else class="shard-query-empty" aria-label="等待选择式神">
      <span aria-hidden="true">签</span>
      <div>
        <p class="section-kicker">WAITING FOR A TARGET</p>
        <h2>先选接收角色和式神</h2>
        <p>先选择接收碎片的角色，再输入式神名称；这里会列出其他角色的供养情况。</p>
      </div>
    </section>

    <footer class="shard-query-footnote">
      <span
        ><b>数据说明</b>
        “已拥有”只表示最近一次本地读取记录了该式神；预计天数按寮祈愿、三花好友各 1 片/天及蓝牙/Wi-Fi 1 片/3 天估算，实际资格与次数仍以游戏内提示为准。</span
      >
      <span>SHARD LEDGER / S-01</span>
    </footer>
  </main>
</template>

<style scoped>
/* 跨角色碎片页使用“寻物簿”意象：靛青区分档案范围，朱砂标记命中，青玉表示可用数据。 */
.shard-query-page {
  --query-indigo: #384d73;
  --query-cinnabar: #a94c43;
  --query-jade: #52776b;
  --query-paper: #fffaf0;
  --query-muted: #827465;
  position: relative;
  max-width: 1480px;
  min-height: 100vh;
  padding: 38px clamp(24px, 4.3vw, 72px) 90px;
  background:
    radial-gradient(circle at 92% 7%, rgb(255 255 255 / 72%), transparent 25%),
    linear-gradient(90deg, transparent 0 97.5%, rgb(56 77 115 / 5%) 97.5% 100%),
    var(--fog);
}

.shard-query-header {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 30px;
}

.shard-query-header h1 {
  margin: 7px 0 8px;
}

.shard-query-header p:last-child {
  margin: 0;
  color: var(--ink-soft);
  font-size: 13px;
}

.shard-query-scope {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding-bottom: 10px;
  color: var(--query-indigo);
  font-family: Consolas, monospace;
  font-size: 11px;
  letter-spacing: 0.07em;
}

.shard-query-scope i {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--query-jade);
  box-shadow: 0 0 0 4px rgb(82 119 107 / 14%);
}

/* 搜索区像一张抽出的寻物签；左侧朱砂字是整页唯一的大装饰。 */
.shard-search-ledger {
  position: relative;
  display: grid;
  grid-template-columns: 78px minmax(0, 1fr);
  margin-top: 30px;
  border: 1px solid rgb(56 77 115 / 24%);
  background: rgb(255 250 240 / 86%);
  box-shadow: 0 16px 42px rgb(73 55 39 / 8%);
}

.shard-search-ledger::after {
  position: absolute;
  right: 18px;
  bottom: 12px;
  width: 46px;
  border-bottom: 1px solid rgb(169 76 67 / 32%);
  content: "";
  transform: rotate(-3deg);
}

.shard-search-ledger__seal {
  display: grid;
  min-height: 194px;
  place-items: center;
  color: #fff8ee;
  background: var(--query-cinnabar);
  font-family: STKaiti, KaiTi, serif;
  font-size: 34px;
  writing-mode: vertical-rl;
  letter-spacing: 0.15em;
}

.shard-search-ledger__body {
  min-width: 0;
  padding: 24px 28px 20px;
}

.shard-search-ledger__heading {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 20px;
}

.shard-search-ledger__heading h2 {
  margin: 4px 0 0;
  font-family: STKaiti, KaiTi, serif;
  font-size: 24px;
  font-weight: 600;
}

.shard-search-ledger__heading > span {
  padding: 4px 8px;
  border: 1px solid rgb(56 77 115 / 24%);
  color: var(--query-indigo);
  font-family: Consolas, monospace;
  font-size: 10px;
  letter-spacing: 0.06em;
}

.shard-search-form {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 12px;
  margin-top: 19px;
}

.shard-recipient-select {
  grid-column: 1 / -1;
}

.shard-search-fields {
  display: grid;
  grid-template-columns: minmax(132px, 0.34fr) minmax(0, 1fr);
  gap: 10px;
}

.shard-rarity-select,
.shard-search-combobox {
  position: relative;
}

.shard-rarity-select > span,
.shard-recipient-select > span,
.shard-search-combobox > label {
  display: block;
  margin-bottom: 7px;
  color: var(--ink-soft);
  font-family: Consolas, monospace;
  font-size: 10px;
  letter-spacing: 0.06em;
}

.shard-rarity-select select {
  width: 100%;
  min-height: 49px;
  padding: 0 30px 0 13px;
  border: 1px solid var(--line);
  border-radius: 2px;
  outline: none;
  color: var(--ink);
  background: #fffdf8;
  cursor: pointer;
  font-size: 14px;
  transition:
    border-color 150ms ease,
    box-shadow 150ms ease;
}

.shard-recipient-select select {
  width: 100%;
  min-height: 49px;
  padding: 0 30px 0 13px;
  border: 1px solid var(--query-cinnabar);
  border-radius: 2px;
  outline: none;
  color: var(--ink);
  background: #fffdf8;
  cursor: pointer;
  font-size: 14px;
  transition:
    border-color 150ms ease,
    box-shadow 150ms ease;
}

.shard-recipient-select select:focus-visible {
  border-color: var(--query-cinnabar);
  box-shadow: 0 0 0 3px rgb(169 76 67 / 12%);
}

.shard-recipient-select select:disabled {
  cursor: wait;
  opacity: 0.65;
}

.shard-rarity-select select:focus-visible {
  border-color: var(--query-indigo);
  box-shadow: 0 0 0 3px rgb(56 77 115 / 12%);
}

.shard-rarity-select select:disabled {
  cursor: wait;
  opacity: 0.65;
}

.shard-search-input-wrap {
  position: relative;
}

.shard-search-input-wrap svg {
  position: absolute;
  top: 50%;
  left: 15px;
  width: 19px;
  fill: none;
  stroke: var(--query-indigo);
  stroke-linecap: round;
  stroke-width: 1.8;
  transform: translateY(-50%);
}

.shard-search-input-wrap input {
  width: 100%;
  min-height: 49px;
  padding: 0 16px 0 46px;
  border: 1px solid var(--line);
  border-radius: 2px;
  outline: none;
  color: var(--ink);
  background: #fffdf8;
  font-size: 15px;
  transition:
    border-color 150ms ease,
    box-shadow 150ms ease;
}

.shard-search-input-wrap input:focus-visible {
  border-color: var(--query-indigo);
  box-shadow: 0 0 0 3px rgb(56 77 115 / 12%);
}

.shard-search-input-wrap input:disabled {
  cursor: wait;
  opacity: 0.65;
}

.shard-search-submit {
  min-width: 142px;
  min-height: 49px;
  padding: 0 20px;
  border: 1px solid var(--query-indigo);
  border-radius: 2px;
  color: #fff;
  background: var(--query-indigo);
  cursor: pointer;
  font-size: 13px;
  font-weight: 700;
  letter-spacing: 0.04em;
  transition:
    transform 150ms ease,
    background 150ms ease;
}

.shard-search-submit:hover:not(:disabled) {
  background: #2f4163;
  transform: translateY(-1px);
}

.shard-search-submit:disabled {
  cursor: wait;
  opacity: 0.58;
}

.shard-search-suggestions {
  position: absolute;
  z-index: 8;
  top: calc(100% + 6px);
  right: 0;
  left: 0;
  max-height: 360px;
  margin: 0;
  padding: 5px;
  overflow-y: auto;
  border: 1px solid rgb(56 77 115 / 28%);
  background: #fffdf8;
  box-shadow: 0 18px 40px rgb(52 45 40 / 16%);
  list-style: none;
}

.shard-search-suggestions button {
  display: flex;
  width: 100%;
  align-items: center;
  gap: 11px;
  padding: 8px 10px;
  border: 0;
  color: var(--ink);
  background: transparent;
  cursor: pointer;
  text-align: left;
}

.shard-search-suggestions button:hover,
.shard-search-suggestions button:focus-visible {
  background: rgb(56 77 115 / 8%);
}

.shard-search-suggestions strong,
.shard-search-suggestions small {
  display: block;
}

.shard-search-suggestions strong {
  font-size: 13px;
}

.shard-search-suggestions small {
  margin-top: 3px;
  color: var(--ink-soft);
  font-family: Consolas, monospace;
  font-size: 10px;
}

.suggestion-avatar,
.shard-target__avatar {
  position: relative;
  display: grid;
  flex: 0 0 auto;
  place-items: center;
  overflow: hidden;
  border: 1px solid rgb(255 255 255 / 42%);
  color: rgb(255 255 255 / 88%);
  background: var(--query-indigo);
  font-family: STKaiti, KaiTi, serif;
}

.suggestion-avatar {
  width: 38px;
  height: 38px;
}

.suggestion-avatar img,
.shard-target__avatar img {
  position: absolute;
  z-index: 1;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.suggestion-avatar i,
.shard-target__avatar i {
  font-style: normal;
}

.rarity-ur {
  background: linear-gradient(145deg, #846c42, #bc9959);
}

.rarity-sp {
  background: linear-gradient(145deg, #73383f, #ad5c59);
}

.rarity-ssr {
  background: linear-gradient(145deg, #5d527e, #8a78aa);
}

.shard-search-note {
  min-height: 18px;
  margin: 10px 0 0;
  color: var(--query-muted);
  font-size: 11px;
  line-height: 1.55;
}

.shard-search-note--error {
  color: var(--query-cinnabar);
}

/* 目标签将式神身份与跨档案汇总放在同一横向阅读线上，不使用通用仪表盘卡片。 */
.shard-target {
  display: flex;
  flex-wrap: wrap;
  align-items: stretch;
  justify-content: space-between;
  gap: 28px;
  margin-top: 22px;
  border: 1px solid rgb(169 76 67 / 28%);
  border-left: 4px solid var(--query-cinnabar);
  background: rgb(255 250 240 / 65%);
}

.shard-target__identity {
  display: flex;
  min-width: 260px;
  align-items: center;
  gap: 14px;
  padding: 15px 20px;
}

.shard-target__recipient {
  display: flex;
  min-width: 170px;
  flex: 0 1 190px;
  flex-direction: column;
  justify-content: center;
  padding: 13px 18px;
  border-left: 1px solid rgb(169 76 67 / 18%);
}

.shard-target__recipient span {
  color: var(--query-cinnabar);
  font-family: Consolas, monospace;
  font-size: 10px;
  letter-spacing: 0.07em;
}

.shard-target__recipient strong {
  margin-top: 5px;
  color: var(--ink);
  font-family: STKaiti, KaiTi, serif;
  font-size: 18px;
}

.shard-target__recipient small {
  margin-top: 3px;
  color: var(--ink-soft);
  font-size: 10px;
}

.shard-target__avatar {
  width: 58px;
  height: 58px;
  font-size: 20px;
}

.shard-target__identity small,
.shard-target__identity span {
  display: block;
  color: var(--ink-soft);
  font-family: Consolas, monospace;
  font-size: 10px;
}

.shard-target__identity small {
  color: var(--query-cinnabar);
  letter-spacing: 0.09em;
}

.shard-target__identity h2 {
  margin: 3px 0 4px;
  font-family: STKaiti, KaiTi, serif;
  font-size: 24px;
}

.shard-target__summary {
  display: grid;
  min-width: min(610px, 58vw);
  grid-template-columns: repeat(4, minmax(105px, 1fr));
  margin: 0;
  border-left: 1px solid rgb(169 76 67 / 18%);
}

.shard-target__summary > div {
  display: flex;
  flex-direction: column;
  justify-content: center;
  padding: 13px 18px;
  border-right: 1px solid rgb(169 76 67 / 13%);
}

.shard-target__summary dt {
  color: var(--ink-soft);
  font-size: 10px;
  letter-spacing: 0.07em;
}

.shard-target__summary dd {
  margin: 3px 0 0;
  color: var(--query-indigo);
  font-family: Consolas, monospace;
  font-size: 20px;
}

.shard-target__summary dd small {
  margin-left: 4px;
  color: var(--ink-soft);
  font-size: 9px;
}

.shard-target__summary-total dd {
  color: var(--query-cinnabar);
}

.shard-target__estimate {
  flex-basis: 100%;
  margin: 0;
  padding: 9px 20px 11px;
  border-top: 1px solid rgb(169 76 67 / 14%);
  color: var(--ink-soft);
  font-size: 10px;
  line-height: 1.5;
}

.shard-results {
  margin-top: 30px;
}

.shard-results__header {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 24px;
  padding-bottom: 13px;
  border-bottom: 1px solid var(--line);
}

.shard-results__header h2 {
  margin: 4px 0 3px;
  font-family: STKaiti, KaiTi, serif;
  font-size: 25px;
}

.shard-results__header p:last-child {
  margin: 0;
  color: var(--ink-soft);
  font-size: 11px;
}

.shard-filter-row {
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: 6px;
}

.shard-filter-row button {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  min-height: 32px;
  padding: 0 10px;
  border: 1px solid var(--line);
  border-radius: 16px;
  color: var(--ink-soft);
  background: rgb(255 250 240 / 62%);
  cursor: pointer;
  font-size: 11px;
}

.shard-filter-row b {
  color: var(--query-indigo);
  font-family: Consolas, monospace;
  font-size: 10px;
}

.shard-filter-row .shard-filter--active {
  border-color: var(--query-indigo);
  color: #fff;
  background: var(--query-indigo);
}

.shard-filter-row .shard-filter--active b {
  color: #fff;
}

.shard-character-list {
  display: grid;
  gap: 9px;
  margin-top: 13px;
}

.shard-recipient-card {
  display: grid;
  grid-template-columns: minmax(250px, 1fr) minmax(260px, 0.8fr);
  gap: 18px;
  margin-top: 14px;
  padding: 14px 17px;
  border: 1px solid rgb(169 76 67 / 28%);
  border-left: 3px solid var(--query-cinnabar);
  background: rgb(255 250 240 / 86%);
}

.shard-recipient-card__heading {
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
}

.shard-recipient-card__heading h3 {
  margin: 3px 0 0;
  font-family: STKaiti, KaiTi, serif;
  font-size: 19px;
}

.shard-recipient-card__heading p:last-child {
  margin: 4px 0 0;
  color: var(--ink-soft);
  font-size: 10px;
}

.shard-recipient-card__heading .shard-badge {
  margin-left: auto;
  flex: 0 0 auto;
}

.shard-recipient-card__seal {
  display: grid;
  width: 42px;
  height: 42px;
  flex: 0 0 auto;
  place-items: center;
  border: 1px solid rgb(169 76 67 / 35%);
  color: var(--query-cinnabar);
  background: rgb(169 76 67 / 7%);
  font-family: STKaiti, KaiTi, serif;
  font-size: 19px;
}

.shard-recipient-card__progress {
  min-width: 0;
  padding-left: 18px;
  border-left: 1px solid var(--line-soft);
}

.shard-recipient-card__progress > div {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
}

.shard-recipient-card__progress > div > span {
  color: var(--ink-soft);
  font-size: 10px;
}

.shard-recipient-card__progress strong {
  color: var(--query-cinnabar);
  font-family: Consolas, monospace;
  font-size: 18px;
}

.shard-recipient-card__progress strong small {
  color: var(--ink-soft);
  font-size: 10px;
}

.shard-recipient-card__progress p {
  margin: 5px 0 0;
  color: var(--ink-soft);
  font-size: 10px;
}

.shard-recipient-card__progress .shard-character__unlock-note {
  display: block;
  margin-top: 5px;
  color: #a56523;
  font-size: 10px;
  line-height: 1.5;
}

.shard-character__supply-note {
  display: block;
  color: var(--query-indigo) !important;
  line-height: 1.5;
}

.shard-character {
  display: grid;
  grid-template-columns: 46px minmax(260px, 1.25fr) minmax(240px, 0.75fr);
  align-items: center;
  gap: 15px;
  padding: 14px 17px;
  border: 1px solid var(--line-soft);
  border-left: 3px solid var(--query-jade);
  background: rgb(255 250 240 / 76%);
  transition:
    border-color 150ms ease,
    transform 150ms ease;
}

.shard-character:hover {
  border-color: rgb(56 77 115 / 28%);
  transform: translateX(2px);
}

.shard-character--unavailable {
  border-left-color: var(--query-muted);
  opacity: 0.76;
}

.shard-character__initial {
  display: grid;
  width: 42px;
  height: 42px;
  place-items: center;
  border: 1px solid rgb(56 77 115 / 28%);
  color: var(--query-indigo);
  background: rgb(56 77 115 / 7%);
  font-family: STKaiti, KaiTi, serif;
  font-size: 18px;
}

.shard-character__identity {
  min-width: 0;
}

.shard-character__identity > div {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.shard-character__identity h3 {
  margin: 0 5px 0 0;
  font-family: STKaiti, KaiTi, serif;
  font-size: 18px;
  font-weight: 600;
}

.shard-character__identity p,
.shard-character__identity > small {
  margin: 4px 0 0;
  color: var(--ink-soft);
  font-size: 10px;
  overflow-wrap: anywhere;
}

.shard-character__identity > small {
  display: block;
  font-family: Consolas, monospace;
}

.shard-badge {
  padding: 3px 7px;
  border: 1px solid currentColor;
  font-size: 9px;
  line-height: 1;
}

.shard-badge--owned {
  color: var(--query-cinnabar);
  background: rgb(169 76 67 / 6%);
}

.shard-badge--shards {
  color: var(--query-indigo);
  background: rgb(56 77 115 / 6%);
}

.shard-badge--unlock-pending {
  color: #b4772f;
  background: rgb(180 119 47 / 8%);
}

.shard-badge--summonable {
  color: var(--query-jade);
  background: rgb(82 119 107 / 7%);
}

.shard-badge--unknown {
  color: var(--query-muted);
  background: rgb(130 116 101 / 8%);
}

.shard-character__progress {
  min-width: 0;
  padding-left: 18px;
  border-left: 1px solid var(--line-soft);
}

.shard-character__progress > div {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
}

.shard-character__progress span,
.shard-character__progress > small {
  color: var(--ink-soft);
  font-size: 9px;
}

.shard-character__progress > .shard-character__unlock-note {
  display: block;
  color: #a56523;
  line-height: 1.5;
}

.shard-character__progress strong {
  color: var(--query-cinnabar);
  font-family: Consolas, monospace;
  font-size: 18px;
}

.shard-character__progress strong small {
  color: var(--ink-soft);
  font-size: 10px;
}

.shard-progress-track {
  display: block;
  height: 4px;
  margin: 7px 0 5px;
  overflow: hidden;
  background: rgb(56 77 115 / 10%);
}

.shard-progress-track i {
  display: block;
  height: 100%;
  background: linear-gradient(90deg, var(--query-indigo), var(--query-cinnabar));
}

.shard-character__warning {
  padding-left: 18px;
  border-left: 1px solid var(--line-soft);
}

.shard-character__warning strong {
  color: var(--query-muted);
  font-size: 11px;
}

.shard-character__warning p {
  margin: 4px 0 0;
  color: var(--ink-soft);
  font-size: 10px;
  line-height: 1.5;
}

.shard-query-state,
.shard-query-empty {
  display: flex;
  align-items: center;
  gap: 18px;
  margin-top: 14px;
  padding: 26px;
  border: 1px dashed rgb(56 77 115 / 28%);
  background: rgb(255 250 240 / 48%);
}

.shard-query-state > span,
.shard-query-empty > span {
  display: grid;
  width: 52px;
  height: 52px;
  flex: 0 0 auto;
  place-items: center;
  border: 1px solid rgb(56 77 115 / 28%);
  color: var(--query-indigo);
  font-family: STKaiti, KaiTi, serif;
  font-size: 22px;
  transform: rotate(-2deg);
}

.shard-query-state strong {
  font-family: STKaiti, KaiTi, serif;
  font-size: 18px;
}

.shard-query-state p,
.shard-query-empty p:last-child {
  margin: 5px 0 0;
  color: var(--ink-soft);
  font-size: 11px;
  line-height: 1.6;
}

.shard-query-state button {
  margin-top: 10px;
  padding: 6px 11px;
  border: 1px solid var(--query-indigo);
  color: var(--query-indigo);
  background: transparent;
  cursor: pointer;
  font-size: 11px;
}

.shard-query-state--error {
  border-color: rgb(169 76 67 / 34%);
}

.shard-query-state--error > span {
  border-color: rgb(169 76 67 / 38%);
  color: var(--query-cinnabar);
}

.shard-query-empty {
  margin-top: 28px;
  min-height: 170px;
}

.shard-query-empty h2 {
  margin: 5px 0 0;
  font-family: STKaiti, KaiTi, serif;
  font-size: 24px;
}

.shard-query-footnote {
  display: flex;
  justify-content: space-between;
  gap: 20px;
  margin-top: 34px;
  padding-top: 13px;
  border-top: 1px solid var(--line);
  color: var(--ink-soft);
  font-size: 10px;
  line-height: 1.6;
}

.shard-query-footnote b {
  margin-right: 6px;
  color: var(--query-cinnabar);
}

.shard-query-footnote span:last-child {
  flex: 0 0 auto;
  font-family: Consolas, monospace;
  letter-spacing: 0.08em;
}

@media (max-width: 1050px) {
  .shard-target {
    display: block;
  }

  .shard-target__summary {
    min-width: 0;
    border-top: 1px solid rgb(169 76 67 / 18%);
    border-left: 0;
  }

  .shard-target__recipient {
    border-top: 1px solid rgb(169 76 67 / 18%);
    border-left: 0;
  }
}

@media (max-width: 760px) {
  .shard-query-page {
    padding: 26px 18px 70px;
  }

  .shard-query-header,
  .shard-results__header {
    align-items: flex-start;
    flex-direction: column;
  }

  .shard-query-scope {
    padding-bottom: 0;
  }

  .shard-search-ledger {
    grid-template-columns: 48px minmax(0, 1fr);
  }

  .shard-search-ledger__seal {
    min-height: 230px;
    font-size: 27px;
  }

  .shard-search-ledger__body {
    padding: 19px 16px 16px;
  }

  .shard-search-ledger__heading > span {
    display: none;
  }

  .shard-search-form {
    grid-template-columns: 1fr;
  }

  .shard-search-fields {
    grid-template-columns: 1fr;
  }

  .shard-search-submit {
    width: 100%;
  }

  .shard-target__summary {
    grid-template-columns: repeat(2, 1fr);
  }

  .shard-recipient-card {
    grid-template-columns: 1fr;
  }

  .shard-recipient-card__progress {
    padding-top: 12px;
    padding-left: 0;
    border-top: 1px solid var(--line-soft);
    border-left: 0;
  }

  .shard-filter-row {
    justify-content: flex-start;
  }

  .shard-character {
    grid-template-columns: 40px minmax(0, 1fr);
  }

  .shard-character__progress,
  .shard-character__warning {
    grid-column: 1 / -1;
    padding: 12px 0 0;
    border-top: 1px solid var(--line-soft);
    border-left: 0;
  }

  .shard-query-footnote {
    flex-direction: column;
  }
}

@media (prefers-reduced-motion: reduce) {
  .shard-search-input-wrap input,
  .shard-search-submit,
  .shard-character {
    transition: none;
  }
}
</style>
