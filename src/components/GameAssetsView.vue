<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import type {
  CurrentDataSummary,
  Shikigami,
  ShikigamiShardEntry,
} from "../api/contracts";
import CharacterArchiveSwitcher from "./CharacterArchiveSwitcher.vue";
import { onActiveCharacterChanged } from "../state/activeCharacter";
import { toLocalDateTime } from "../timeFormat";
import {
  getMaterialShikigamiName,
  getRarityLabel,
  rarityOrder,
} from "../shikigamiCatalog";
import {
  REALM_CARD_STARS,
  aggregateRealmCards,
  type RealmCardInventory,
} from "../realmCardCatalog";
import { formatCoin } from "../assetFormat";

type AssetTab = "core" | "boundary" | "shards";
type ShardRarityFilter = "all" | "UR" | "SP" | "SSR" | "御行达摩";
type AssetTone = "vermilion" | "blue" | "jade" | "gold";

// 图标随应用离线打包，文件名与 currency key 一致；contrib/broken_amulet/realm_raid_pass
// 藏宝阁不展示，暂缺图，由占位块兜住。
const itemIconModules = import.meta.glob("../assets/item-icons/*.png", {
  eager: true,
  import: "default",
  query: "?url",
});

// 道具/资源按游戏内 21 项顺序定义。
interface ItemMeta {
  key: string;
  label: string;
  note: string;
  tone: AssetTone;
}

const ITEM_METAS: readonly ItemMeta[] = [
  { key: "coin", label: "金币", note: "通用货币", tone: "gold" },
  { key: "jade", label: "勾玉", note: "稀有货币", tone: "vermilion" },
  { key: "action_point", label: "体力", note: "副本消耗", tone: "jade" },
  { key: "auto_point", label: "樱饼", note: "自动战斗", tone: "jade" },
  { key: "honor", label: "荣誉", note: "斗技商店", tone: "blue" },
  { key: "medal", label: "勋章", note: "寮商店", tone: "blue" },
  { key: "contrib", label: "功勋", note: "寮商店", tone: "blue" },
  { key: "totem_pass", label: "御灵券", note: "御灵之境", tone: "gold" },
  { key: "s_jade", label: "魂玉", note: "充值货币", tone: "vermilion" },
  { key: "skin_token", label: "皮肤券", note: "皮肤商店", tone: "gold" },
  { key: "realm_raid_pass", label: "突破券", note: "结界突破", tone: "blue" },
  { key: "broken_amulet", label: "破碎的符咒", note: "普通召唤", tone: "gold" },
  { key: "mystery_amulet", label: "神秘的符咒", note: "标准召唤", tone: "vermilion" },
  { key: "ar_amulet", label: "现世符咒", note: "现世召唤", tone: "vermilion" },
  { key: "ofuda", label: "御札", note: "神龛货币", tone: "blue" },
  { key: "gold_ofuda", label: "金御札", note: "神龛货币", tone: "gold" },
  { key: "scale", label: "八岐大蛇鳞片", note: "秘魂屋", tone: "jade" },
  { key: "reverse_scale", label: "逆鳞", note: "秘魂屋", tone: "jade" },
  { key: "demon_soul", label: "逢魔之魂", note: "逢魔之时", tone: "vermilion" },
  { key: "foolery_pass", label: "痴念之卷", note: "业原火挑战", tone: "jade" },
  { key: "sp_skin_token", label: "SP皮肤券", note: "SP皮肤商店", tone: "vermilion" },
];

// 结界卡图标随应用离线打包，文件名与 itemId 一致；同一卡种的 1~6 星共用作者提供的卡面素材。
const realmCardIconModules = import.meta.glob("../assets/realm-card-icons/*.png", {
  eager: true,
  import: "default",
  query: "?url",
});

// 浏览器静态预览没有 Tauri 事件桥，保持静默空态，避免 invoke 报错刷屏。
const isTauriRuntime = computed(
  () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window,
);

// Tab 只存在于当前页面内，暂不引入持久化状态。
const assetTab = ref<AssetTab>("core");
const items = ref<Record<string, number>>({});
const shikigamiShards = ref<ShikigamiShardEntry[]>([]);
const shardCatalog = ref<Shikigami[]>([]);
const activeShardRarity = ref<ShardRarityFilter>("all");
const shardRarityTabs: readonly { key: ShardRarityFilter; label: string }[] = [
  { key: "all", label: "全部" },
  { key: "UR", label: "UR" },
  { key: "SP", label: "SP" },
  { key: "SSR", label: "SSR" },
  { key: "御行达摩", label: "御行达摩" },
];
const realmCards = ref<RealmCardInventory>({
  groups: [],
  total: 0,
  unknownCount: 0,
});
const summary = ref<CurrentDataSummary | null>(null);
const loadError = ref("");
const loaded = ref(false);

// 金币使用紧凑单位避免大数字撑开卡片，其余资产继续显示完整千分位数字。
const coreAssets = computed(() =>
  ITEM_METAS.map((meta) => {
    const rawValue = items.value[meta.key] ?? 0;
    return {
      ...meta,
      rawValue,
      value: meta.key === "coin" ? formatCoin(rawValue) : formatNumber(rawValue),
      icon: itemIcon(meta.key),
    };
  }),
);

// 兑换口径：1000 勾玉 = 11 张蓝票；按当前勾玉余额向下取整计算可购买张数。
const jadeCount = computed(() => items.value.jade ?? 0);
const purchasableBlueTickets = computed(() =>
  Math.floor((jadeCount.value / 1000) * 11),
);

const shardCatalogById = computed(
  () => new Map(shardCatalog.value.map((entry) => [entry.shikigamiId, entry])),
);

// 后端已按稀有度收敛数据；这里仅负责把目录信息合并到展示行并保持固定顺序。
const shardRows = computed(() =>
  shikigamiShards.value
    // 只展示实际持有的碎片，避免大量 0 数量卡片占据视线和页面空间。
    .filter((shard) => shard.shardCount > 0)
    .map((shard) => ({
      ...shard,
      name:
        shardCatalogById.value.get(shard.shikigamiId)?.name ??
        getMaterialShikigamiName(shard.shikigamiId) ??
        `式神 ${shard.shikigamiId}`,
      rarity: shardRarity(shard),
    }))
    .sort((left, right) => {
      const rarityDifference =
        shardRarityOrder(left.rarity) - shardRarityOrder(right.rarity);
      return rarityDifference || left.name.localeCompare(right.name, "zh-CN");
    }),
);

// 二级 Tab 只改变当前视图，不重新读取数据；未归类的目录条目仍在“全部”中保留。
const visibleShardRows = computed(() => {
  if (activeShardRarity.value === "all") return shardRows.value;
  return shardRows.value.filter((shard) => shard.rarity === activeShardRarity.value);
});

const shardRarityCount = (rarity: ShardRarityFilter): number =>
  rarity === "all"
    ? shardRows.value.length
    : shardRows.value.filter((shard) => shard.rarity === rarity).length;

function formatNumber(value: number): string {
  return value.toLocaleString("zh-CN");
}

/** 返回离线道具图标；缺图时回退到占位块，不发起网络请求。 */
function itemIcon(key: string): string | null {
  return (itemIconModules[`../assets/item-icons/${key}.png`] as string) ?? null;
}

/** 返回离线卡面图标；缺图时回退到占位块，避免预览页发起网络请求。 */
function realmCardIcon(itemId: number | null): string | null {
  if (itemId === null) return null;
  return (
    (realmCardIconModules[`../assets/realm-card-icons/${itemId}.png`] as string) ?? null
  );
}

/** 合并式神目录稀有度；目录缺失时用碎片上限把未知的 50 片条目归为 SSR。 */
function shardRarity(shard: ShikigamiShardEntry): string {
  if (shard.shikigamiId === "411") return "御行达摩";
  return (
    shardCatalogById.value.get(shard.shikigamiId)?.rarity ??
    (shard.maxShardCount === 50 ? "SSR" : "未知")
  );
}

/** 将目标碎片按 UR、SP、SSR、御行达摩、未知排序，未知条目仍保留以便发现目录滞后。 */
function shardRarityOrder(rarity: string): number {
  if (rarity === "御行达摩") return 3;
  if (rarity === "未知") return 4;
  return rarityOrder(rarity);
}

/** 碎片卡片统一显示中文稀有度；御行达摩保留其专属名称，避免把目录内部值直接暴露给用户。 */
function shardRarityLabel(rarity: string): string {
  return rarity === "御行达摩" ? rarity : getRarityLabel(rarity);
}

/** 返回随应用发布的离线头像；411 御行达摩虽不在式神目录，也有独立素材头像。 */
function shardAvatarUrl(shikigamiId: string): string | null {
  if (shikigamiId === "411") return "/shikigami-avatars/411.webp";
  const catalogEntry = shardCatalogById.value.get(shikigamiId);
  return catalogEntry?.iconAssetId
    ? `/shikigami-avatars/${encodeURIComponent(shikigamiId)}.webp`
    : null;
}

/** 头像资源缺失时隐藏坏图，让卡片保留纹理占位而不是出现破图图标。 */
function hideBrokenAvatar(event: Event): void {
  if (event.currentTarget instanceof HTMLImageElement) {
    event.currentTarget.hidden = true;
  }
}

function formatSnapshotTime(value: string | null): string {
  if (!value) return "尚未导入";
  // 后端存的是 UTC；这里转成本地时区（如北京时间 +8）再展示。
  return toLocalDateTime(value) ?? "尚未导入";
}

async function loadAssets(): Promise<void> {
  if (!isTauriRuntime.value) {
    loaded.value = true;
    return;
  }
  try {
    const [itemValues, cardEntries, shardEntries, dataSummary] = await Promise.all([
      desktopApi.getCurrentItems(),
      desktopApi.getCurrentRealmCards(),
      desktopApi.listShikigamiShards(),
      desktopApi.getCurrentDataSummary(),
    ]);
    items.value = itemValues;
    realmCards.value = aggregateRealmCards(cardEntries);
    shikigamiShards.value = shardEntries;
    summary.value = dataSummary;
    await loadShardCatalog();
  } catch (error) {
    loadError.value =
      typeof error === "object" && error !== null && "message" in error
        ? String(error.message)
        : "读取当前数据失败";
  } finally {
    loaded.value = true;
  }
}

/** 读取已安装的式神目录，用于把碎片 ID 补齐为名称和稀有度；目录失败不阻断碎片数量展示。 */
async function loadShardCatalog(): Promise<void> {
  try {
    let status = await desktopApi.getCatalogStatus();
    if (
      !status ||
      status.validationStatus !== "verified" ||
      status.shikigamiCount === 0
    ) {
      status = await desktopApi.installBuiltinCatalog();
    }
    shardCatalog.value = await desktopApi.listShikigami(status.version);
  } catch {
    shardCatalog.value = [];
  }
}

onMounted(() => {
  void loadAssets();
  stopActiveCharacterChanged = onActiveCharacterChanged(() => void loadAssets());
});

let stopActiveCharacterChanged: (() => void) | null = null;
onBeforeUnmount(() => {
  stopActiveCharacterChanged?.();
  stopActiveCharacterChanged = null;
});
</script>

<template>
  <main
    class="page-main game-assets-page game-assets-page--a"
    aria-labelledby="game-assets-title"
  >
    <header class="game-assets-header game-assets-header--ledger">
      <div>
        <p class="eyebrow">MY DATA / GAME ASSETS</p>
        <div class="game-assets-title-row">
          <h1 id="game-assets-title">游戏资产</h1>
        </div>
      </div>
      <div class="game-assets-header__meta">
        <CharacterArchiveSwitcher />
        <span class="asset-snapshot"
          >快照
          {{ summary ? formatSnapshotTime(summary.imported_at) : "尚未导入" }}</span
        >
        <span class="asset-readonly"><i aria-hidden="true"></i>只读预览</span>
      </div>
    </header>

    <p v-if="loadError" class="asset-load-error" role="alert">{{ loadError }}</p>

    <div
      class="game-assets-tabs game-assets-tabs--ledger"
      role="tablist"
      aria-label="游戏资产分类"
    >
      <button
        class="game-assets-tab"
        :class="{ 'game-assets-tab--active': assetTab === 'core' }"
        type="button"
        role="tab"
        :aria-selected="assetTab === 'core'"
        @click="assetTab = 'core'"
      >
        <span class="game-assets-tab__index">01</span>
        <span><strong>核心资产</strong><small>21 项资源 · 当前余额</small></span>
      </button>
      <button
        class="game-assets-tab"
        :class="{ 'game-assets-tab--active': assetTab === 'boundary' }"
        type="button"
        role="tab"
        :aria-selected="assetTab === 'boundary'"
        @click="assetTab = 'boundary'"
      >
        <span class="game-assets-tab__index">02</span>
        <span><strong>结界卡</strong><small>卡名 · 星级 · 数量</small></span>
      </button>
      <button
        class="game-assets-tab"
        :class="{ 'game-assets-tab--active': assetTab === 'shards' }"
        type="button"
        role="tab"
        :aria-selected="assetTab === 'shards'"
        @click="assetTab = 'shards'"
      >
        <span class="game-assets-tab__index">03</span>
        <span><strong>式神碎片</strong><small>SSR · SP · UR · 御行达摩</small></span>
      </button>
    </div>

    <section
      v-if="assetTab === 'core'"
      class="ledger-overview"
      aria-label="核心资产总览"
    >
      <div class="ledger-overview__heading">
        <div>
          <p class="section-kicker">CORE ASSETS / SNAPSHOT</p>
          <h2>资产盘点</h2>
        </div>
        <div class="ledger-total">
          <span>当前可用勾玉</span>
          <strong>{{ formatNumber(jadeCount) }}</strong>
          <small>可以购买 {{ purchasableBlueTickets }} 张蓝票</small>
        </div>
      </div>
      <div class="ledger-rule" aria-hidden="true"></div>
      <div class="ledger-asset-grid">
        <article
          v-for="asset in coreAssets"
          :key="asset.key"
          :class="['ledger-asset', `ledger-asset--${asset.tone}`]"
        >
          <div class="ledger-asset__mark" aria-hidden="true">
            <img v-if="asset.icon" :src="asset.icon" :alt="asset.label" />
            <span v-else class="ledger-asset__placeholder"></span>
          </div>
          <div class="ledger-asset__copy">
            <span>{{ asset.label }}</span>
            <strong
              :title="
                asset.key === 'coin'
                  ? `精确数量：${formatNumber(asset.rawValue)}`
                  : undefined
              "
            >
              {{ asset.value }}
            </strong>
            <small>{{ asset.note }}</small>
          </div>
          <span class="ledger-asset__arrow" aria-hidden="true">↗</span>
        </article>
      </div>
      <div class="ledger-footnote">
        <span
          ><b>读取边界</b>
          {{
            loaded
              ? "只展示当前快照确认的资源余额，不推算商店价格和未完成任务奖励。"
              : "正在读取当前数据…"
          }}</span
        >
        <span class="ledger-footnote__stamp">ASSET LEDGER / A-01</span>
      </div>
    </section>

    <section
      v-else-if="assetTab === 'boundary'"
      class="boundary-ledger"
      aria-label="结界卡总览"
    >
      <div class="boundary-ledger__lead">
        <div>
          <p class="section-kicker">BARRIER CARDS / INVENTORY</p>
          <h2>结界卡库存</h2>
          <p>按卡名与星级统计当前持有的结界卡；同名卡的 1~6 星在同一行分列展示。</p>
        </div>
        <div class="boundary-total">
          <strong>{{ realmCards.total }}</strong
          ><span>张结界卡</span>
          <small>{{ realmCards.groups.length }} 种卡</small>
        </div>
      </div>
      <div v-if="realmCards.groups.length === 0 && loaded" class="boundary-empty">
        当前快照没有结界卡数据。macOS 版不读取阴阳师客户端，藏宝阁公开商品也不包含结界卡。
      </div>
      <div v-else class="boundary-card-scroll">
        <table class="boundary-card-table">
          <thead>
            <tr>
              <th scope="col">结界卡</th>
              <th v-for="star in REALM_CARD_STARS" :key="star" scope="col">
                {{ star }} 星
              </th>
              <th scope="col">合计</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="group in realmCards.groups" :key="group.name">
              <th scope="row">
                <span class="boundary-card__icon" aria-hidden="true">
                  <img
                    v-if="realmCardIcon(group.iconItemId)"
                    :src="realmCardIcon(group.iconItemId) ?? undefined"
                    :alt="group.name"
                  />
                  <span v-else class="boundary-card__placeholder"></span>
                </span>
                <span>{{ group.name }}</span>
              </th>
              <td
                v-for="(count, index) in group.starCounts"
                :key="index"
                :class="{ 'boundary-card__zero': count === 0 }"
              >
                {{ count }}
              </td>
              <td class="boundary-card__sum">{{ group.total }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="boundary-ledger__note">
        <span class="note-dot" aria-hidden="true"></span>
        <span v-if="realmCards.unknownCount > 0">
          另有 {{ realmCards.unknownCount }} 张卡的 ID 不在目录内，未计入上表；
        </span>
        当前只统计持有数量，不含寄养状态与产出进度。
      </div>
    </section>

    <section v-else class="shard-ledger" aria-label="式神碎片总览">
      <div class="shard-ledger__lead">
        <div>
          <p class="section-kicker">SHIKIGAMI SHARDS / INVENTORY</p>
          <h2>式神碎片</h2>
          <p>
            只展示 SSR、SP、UR
            与御行达摩碎片；数量来自当前读取快照，不推算召唤或兑换结果。
          </p>
        </div>
      </div>
      <div v-if="shardRows.length === 0 && loaded" class="boundary-empty">
        当前快照没有可展示的式神碎片。藏宝阁公开商品未包含碎片时，这里保持为空。
      </div>
      <div v-else class="shard-collection">
        <!-- 二级筛选只承担稀有度切换，按钮保持轻量，避免与主 Tab 抢视觉层级。 -->
        <div class="shard-rarity-tabs" role="tablist" aria-label="按稀有度筛选式神碎片">
          <button
            v-for="tab in shardRarityTabs"
            :key="tab.key"
            class="shard-rarity-tab"
            :class="{ 'shard-rarity-tab--active': activeShardRarity === tab.key }"
            type="button"
            role="tab"
            :aria-selected="activeShardRarity === tab.key"
            @click="activeShardRarity = tab.key"
          >
            <span>{{ tab.label }}</span>
            <small>{{ shardRarityCount(tab.key) }}</small>
          </button>
        </div>

        <div v-if="visibleShardRows.length === 0" class="boundary-empty">
          当前稀有度没有式神碎片。
        </div>
        <div v-else class="shard-card-grid">
          <article
            v-for="shard in visibleShardRows"
            :key="shard.shikigamiId"
            class="shard-card"
            :class="`shard-card--${shard.rarity.toLowerCase()}`"
          >
            <div class="shard-card__portrait" aria-hidden="true">
              <span class="shard-card__portrait-placeholder">
                {{ shard.rarity === "御行达摩" ? "御" : "式" }}
              </span>
              <img
                v-if="shardAvatarUrl(shard.shikigamiId)"
                :src="shardAvatarUrl(shard.shikigamiId) ?? undefined"
                :alt="`${shard.name}头像`"
                @error="hideBrokenAvatar"
              />
            </div>
            <div class="shard-card__body">
              <span class="shard-card__rarity">{{
                shardRarityLabel(shard.rarity)
              }}</span>
              <span class="shard-card__name">{{ shard.name }}</span>
              <small v-if="shard.name.startsWith('式神 ')" class="shard-card__id">
                ID {{ shard.shikigamiId }}
              </small>
            </div>
            <strong class="shard-card__count">{{
              shard.shardCount.toLocaleString()
            }}</strong>
          </article>
        </div>
      </div>
    </section>
  </main>
</template>

<style scoped>
/* 游戏资产页使用“账册 + 结界靛青”作为专属视觉层，保持与式神录主页面的层级关系。 */
.game-assets-page {
  --asset-blue: #425f89;
  --asset-indigo: #344b75;
  --asset-red: #b64e43;
  --asset-gold: #b48b56;
  --asset-jade: #52796f;
  position: relative;
  max-width: 1480px;
  min-height: 100vh;
  padding: 38px clamp(24px, 4.3vw, 72px) 104px;
  background:
    radial-gradient(circle at 83% 3%, rgb(255 255 255 / 78%), transparent 28%),
    var(--fog);
}

.game-assets-header {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 28px;
}

.game-assets-title-row {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}

.game-assets-title-row h1 {
  margin: 7px 0 8px;
}

.game-assets-header__meta {
  display: flex;
  align-items: center;
  gap: 12px;
  padding-bottom: 9px;
  color: var(--ink-soft);
  font-family: Consolas, monospace;
  font-size: 10px;
  letter-spacing: 0.07em;
}

.asset-readonly {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  color: var(--asset-jade);
}

.asset-readonly i {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: currentColor;
  box-shadow: 0 0 0 4px rgb(82 121 111 / 14%);
}

.game-assets-tabs {
  display: flex;
  gap: 1px;
  margin-top: 37px;
  border-bottom: 1px solid var(--line);
}

.game-assets-tab {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  min-width: min(320px, 50%);
  padding: 15px 20px 13px;
  border: 0;
  border-bottom: 3px solid transparent;
  color: var(--ink-soft);
  text-align: left;
  cursor: pointer;
  background: transparent;
}

.game-assets-tab:hover {
  color: var(--ink);
  background: rgb(255 255 255 / 30%);
}

.game-assets-tab--active {
  border-bottom-color: var(--asset-red);
  color: var(--ink);
  background: rgb(255 255 255 / 42%);
}

.game-assets-tab__index {
  padding-top: 2px;
  color: var(--asset-red);
  font-family: Consolas, monospace;
  font-size: 11px;
}

.game-assets-tab strong,
.game-assets-tab small {
  display: block;
}

.game-assets-tab strong {
  font-size: 14px;
}

.game-assets-tab small {
  margin-top: 4px;
  color: var(--ink-soft);
  font-size: 11px;
  font-weight: 400;
}

.ledger-overview,
.boundary-ledger,
.shard-ledger {
  margin-top: 27px;
  padding: 28px 30px 21px;
  border: 1px solid var(--line);
  background: rgb(255 250 240 / 72%);
  box-shadow: 0 16px 40px rgb(73 55 39 / 8%);
}

.ledger-overview__heading,
.boundary-ledger__lead,
.shard-ledger__lead {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 24px;
}

.ledger-overview h2,
.boundary-ledger h2,
.shard-ledger h2 {
  margin: 7px 0 0;
  font-family: STKaiti, KaiTi, serif;
  font-size: 28px;
  letter-spacing: -0.03em;
}

.ledger-total {
  min-width: 220px;
  padding-left: 20px;
  border-left: 1px solid var(--line);
}

.ledger-total span,
.ledger-total small {
  display: block;
  color: var(--ink-soft);
  font-size: 11px;
}

.ledger-total strong {
  display: block;
  margin: 4px 0 1px;
  color: var(--asset-red);
  font-family: Consolas, monospace;
  font-size: 26px;
  letter-spacing: -0.06em;
}

.ledger-rule {
  height: 1px;
  margin: 21px 0 0;
  background: linear-gradient(90deg, var(--asset-red), var(--line) 35%, transparent);
}

/* 核心资产卡保持紧凑尺寸，图标承担第一识别层，数值承担第二识别层。 */
.ledger-asset-grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 8px;
  padding-top: 18px;
}

.ledger-asset {
  position: relative;
  display: grid;
  min-height: 96px;
  grid-template-columns: 36px minmax(0, 1fr);
  grid-template-rows: 1fr auto;
  column-gap: 10px;
  padding: 11px 12px;
  border: 1px solid rgb(52 45 40 / 11%);
  background: rgb(255 255 255 / 56%);
  overflow: hidden;
}

.ledger-asset::after {
  position: absolute;
  right: -18px;
  bottom: -35px;
  width: 82px;
  height: 82px;
  border: 1px solid currentColor;
  border-radius: 50%;
  opacity: 0.14;
  content: "";
}

.ledger-asset__mark {
  display: grid;
  width: 34px;
  height: 34px;
  grid-row: 1 / -1;
  align-self: center;
  place-items: center;
  color: var(--asset-red);
}

/* 少数道具官方素材缺失：用同色描边占位块占住识别层。 */
.ledger-asset__placeholder {
  display: block;
  width: 24px;
  height: 24px;
  border: 1px solid currentColor;
  border-radius: 5px;
  opacity: 0.55;
}

.asset-load-error {
  margin: 0 0 14px;
  padding: 10px 14px;
  border: 1px solid rgb(200 80 70 / 45%);
  background: rgb(200 80 70 / 8%);
  color: var(--asset-red);
  font-size: 12px;
}

.boundary-empty {
  margin-top: 22px;
  padding: 26px 18px;
  border: 1px dashed var(--line);
  color: var(--ink-soft);
  font-size: 12px;
  line-height: 1.7;
  text-align: center;
}

.ledger-asset--blue .ledger-asset__mark {
  color: var(--asset-blue);
}

.ledger-asset--jade .ledger-asset__mark {
  color: var(--asset-jade);
}

.ledger-asset--gold .ledger-asset__mark {
  color: var(--asset-gold);
}

.ledger-asset__mark img,
.boundary-card__icon img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: contain;
}

.ledger-asset__copy span,
.ledger-asset__copy small {
  display: block;
  color: var(--ink-soft);
  font-size: 11px;
}

.ledger-asset__copy span {
  align-self: end;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ledger-asset__copy strong {
  display: block;
  margin: 2px 0 1px;
  font-family: Consolas, monospace;
  font-size: clamp(18px, 2.1vw, 23px);
  letter-spacing: -0.07em;
}

.ledger-asset__copy small {
  color: var(--asset-jade);
  font-size: 10px;
}

.ledger-asset--blue .ledger-asset__copy small {
  color: var(--asset-blue);
}

.ledger-asset__arrow {
  position: absolute;
  top: 11px;
  right: 13px;
  color: rgb(52 45 40 / 35%);
  font-size: 15px;
}

.ledger-footnote {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
  margin-top: 21px;
  padding-top: 13px;
  border-top: 1px dashed var(--line);
  color: var(--ink-soft);
  font-size: 11px;
  line-height: 1.5;
}

.ledger-footnote b {
  color: var(--ink);
}

.ledger-footnote__stamp {
  flex: 0 0 auto;
  color: var(--asset-red);
  font-family: Consolas, monospace;
  font-size: 10px;
  letter-spacing: 0.1em;
}

.boundary-ledger {
  padding-bottom: 25px;
}

.boundary-ledger__lead p:not(.section-kicker) {
  max-width: 620px;
  margin: 11px 0 0;
  color: var(--ink-soft);
  font-size: 13px;
  line-height: 1.7;
}

.boundary-total {
  display: grid;
  min-width: 150px;
  grid-template-columns: auto 1fr;
  align-items: end;
  column-gap: 8px;
  padding-left: 20px;
  border-left: 1px solid var(--line);
}

.boundary-total strong {
  color: var(--asset-indigo);
  font-family: Consolas, monospace;
  font-size: 38px;
  line-height: 1;
}

.boundary-total span {
  align-self: center;
  font-size: 12px;
}

.boundary-total small {
  grid-column: 2;
  margin-top: 5px;
  color: var(--ink-soft);
  font-size: 10px;
}

/* 结界卡采用一行一种卡、按星级分列的矩阵；窄屏保留横向滚动，避免数字被压扁。 */
.boundary-card-scroll {
  margin-top: 24px;
  overflow-x: auto;
}

.boundary-card-table {
  min-width: 640px;
  border-collapse: collapse;
  table-layout: fixed;
  width: 100%;
}

.boundary-card-table th,
.boundary-card-table td {
  padding: 9px 10px;
  border-top: 1px solid var(--line-soft);
  text-align: center;
}

.boundary-card-table thead th {
  border-top: 1px solid var(--line);
  border-bottom: 1px solid var(--line);
  color: var(--ink-soft);
  font-family: Consolas, monospace;
  font-size: 10px;
  font-weight: 400;
  letter-spacing: 0.06em;
}

.boundary-card-table thead th:first-child {
  text-align: left;
}

.boundary-card-table tbody tr:hover {
  background: rgb(255 255 255 / 78%);
}

.boundary-card-table tbody th {
  display: flex;
  align-items: center;
  gap: 11px;
  font-family: STKaiti, KaiTi, serif;
  font-size: 17px;
  font-weight: 400;
  text-align: left;
}

.boundary-card-table tbody td {
  color: var(--asset-indigo);
  font-family: Consolas, monospace;
  font-size: 16px;
}

/* 0 张压到弱色，让实际持有的星级在矩阵里一眼可见。 */
.boundary-card__zero {
  color: var(--ink-soft);
  opacity: 0.45;
}

.boundary-card__sum {
  border-left: 1px solid var(--line-soft);
  color: var(--asset-red);
}

.boundary-card__icon {
  display: grid;
  width: 45px;
  height: 45px;
  flex: 0 0 auto;
  place-items: center;
  padding: 4px;
  border: 1px solid rgb(52 45 40 / 16%);
  background: rgb(255 250 240 / 68%);
}

/* 缺少卡面素材时用虚线占位块占住识别层，已有离线素材优先展示真实卡面。 */
.boundary-card__placeholder {
  display: block;
  width: 28px;
  height: 28px;
  border: 1px dashed rgb(52 45 40 / 40%);
  border-radius: 5px;
}

.boundary-ledger__note {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 18px;
  color: var(--ink-soft);
  font-size: 11px;
}

/* 式神碎片采用轻量筛选与紧凑卡片，信息焦点只落在式神名和持有数量。 */
.shard-ledger {
  padding-bottom: 25px;
}

.shard-ledger__lead p:not(.section-kicker) {
  max-width: 680px;
  margin: 11px 0 0;
  color: var(--ink-soft);
  font-size: 13px;
  line-height: 1.7;
}

.shard-collection {
  margin-top: 21px;
}

.shard-rarity-tabs {
  display: flex;
  gap: 5px;
  margin-bottom: 14px;
  overflow-x: auto;
  padding-bottom: 7px;
  border-bottom: 1px solid var(--line-soft);
}

.shard-rarity-tab {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: baseline;
  gap: 7px;
  min-height: 29px;
  padding: 5px 10px;
  border: 1px solid transparent;
  background: transparent;
  color: var(--ink-soft);
  font-family: Consolas, monospace;
  font-size: 10px;
  letter-spacing: 0.04em;
  cursor: pointer;
  transition:
    border-color 160ms ease,
    background 160ms ease,
    color 160ms ease;
}

.shard-rarity-tab:hover {
  border-color: rgb(52 45 40 / 20%);
  color: var(--ink);
}

.shard-rarity-tab--active {
  border-color: var(--asset-indigo);
  background: var(--asset-indigo);
  color: #fffaf0;
}

.shard-rarity-tab small {
  opacity: 0.68;
  font-size: 9px;
}

.shard-card-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(132px, 1fr));
  gap: 9px;
}

/* 头像是卡片的视觉锚点，名称紧贴头像下方；数量固定在右下角。 */
.shard-card {
  position: relative;
  display: flex;
  min-height: 122px;
  flex-direction: column;
  align-items: center;
  padding: 9px 8px 10px;
  border: 1px solid var(--line-soft);
  background: rgb(255 250 240 / 78%);
  transition:
    border-color 160ms ease,
    transform 160ms ease,
    background 160ms ease;
}

.shard-card:hover {
  border-color: var(--line);
  background: #fffaf0;
  transform: translateY(-1px);
}

.shard-card__portrait {
  position: relative;
  display: grid;
  width: 62px;
  height: 62px;
  place-items: center;
  overflow: hidden;
  border: 1px solid rgb(52 45 40 / 18%);
  border-radius: 5px;
  background: linear-gradient(135deg, rgb(255 255 255 / 70%), transparent), var(--fog);
}

.shard-card__portrait-placeholder {
  color: var(--asset-indigo);
  font-family: STKaiti, KaiTi, serif;
  font-size: 18px;
}

.shard-card__portrait img {
  position: absolute;
  inset: 0;
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.shard-card__portrait img[hidden] {
  display: none;
}

.shard-card__body {
  width: 100%;
  min-width: 0;
  padding: 0 22px 0;
  text-align: center;
}

.shard-card__rarity {
  display: block;
  margin-top: 7px;
  color: var(--ink-soft);
  font-family: Consolas, monospace;
  font-size: 9px;
  letter-spacing: 0.06em;
  line-height: 1;
}

.shard-card__name {
  display: block;
  max-width: 100%;
  margin-top: 4px;
  padding: 0;
  overflow: hidden;
  color: var(--ink);
  font-family: STKaiti, KaiTi, serif;
  font-size: 15px;
  line-height: 1.2;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.shard-card__id {
  display: block;
  margin-top: 3px;
  color: var(--ink-soft);
  font-family: Consolas, monospace;
  font-size: 9px;
}

.shard-card__count {
  position: absolute;
  right: 9px;
  bottom: 7px;
  color: var(--asset-indigo);
  font-family: Consolas, monospace;
  font-size: 17px;
  line-height: 1;
}

.shard-card--ur .shard-card__rarity {
  color: #714b91;
}

.shard-card--sp .shard-card__rarity {
  color: #a44b69;
}

.shard-card--ssr .shard-card__rarity {
  color: #a06e2e;
}

.shard-card--御行达摩 .shard-card__rarity {
  color: var(--asset-jade);
}

.note-dot {
  width: 6px;
  height: 6px;
  flex: 0 0 auto;
  border-radius: 50%;
  background: var(--asset-red);
}

@media (max-width: 1050px) {
  .ledger-asset-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@media (max-width: 720px) {
  .game-assets-page {
    padding: 27px 18px 96px;
  }

  .game-assets-header,
  .ledger-overview__heading,
  .boundary-ledger__lead,
  .shard-ledger__lead {
    align-items: flex-start;
    flex-direction: column;
  }

  .game-assets-header__meta {
    padding-bottom: 0;
  }

  .game-assets-tabs {
    overflow-x: auto;
  }

  .game-assets-tab {
    min-width: 245px;
    padding-inline: 14px;
  }

  .ledger-overview,
  .boundary-ledger,
  .shard-ledger {
    padding: 21px 17px 18px;
  }

  .ledger-total,
  .boundary-total,
  .shard-total {
    width: 100%;
    padding: 13px 0 0;
    border-top: 1px solid var(--line);
    border-left: 0;
  }

  .ledger-footnote {
    align-items: flex-start;
    flex-direction: column;
    gap: 8px;
  }

  .ledger-asset-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .boundary-card-scroll {
    margin-inline: -17px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .game-assets-tab {
    transition: none;
  }
}
</style>
