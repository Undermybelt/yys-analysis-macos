import catalog from "./assets/realm-cards.json";
import type { RealmCardEntry } from "./api/contracts";

/** 结界卡目录条目；itemId 末位是星级，同一卡名跨 1~6 星共 6 个 itemId。 */
interface RealmCardCatalogEntry {
  id: number;
  name: string;
  star: number;
}

/** 结界卡星级固定 1~6 档，矩阵按高星在前展示。 */
export const REALM_CARD_STARS: readonly number[] = [6, 5, 4, 3, 2, 1];

/** 一种结界卡的持有情况；starCounts 按 REALM_CARD_STARS 顺序对齐。 */
export interface RealmCardGroup {
  name: string;
  starCounts: number[];
  total: number;
  /** 该卡任一星级的图标 itemId；目录缺图时为 null。 */
  iconItemId: number | null;
}

/** 结界卡库存汇总；unknownCount 收纳目录未收录的 itemId，避免静默丢弃。 */
export interface RealmCardInventory {
  groups: RealmCardGroup[];
  total: number;
  unknownCount: number;
}

const CATALOG = catalog as RealmCardCatalogEntry[];

const BY_ITEM_ID = new Map<number, RealmCardCatalogEntry>(
  CATALOG.map((entry) => [entry.id, entry]),
);

// 目录顺序即 itemId 升序，用它固定卡名展示顺序，让同一份快照每次渲染结果一致。
const NAME_ORDER = new Map<string, number>();
for (const entry of CATALOG) {
  if (!NAME_ORDER.has(entry.name)) NAME_ORDER.set(entry.name, NAME_ORDER.size);
}

/** 返回结界卡目录条目；itemId 未收录时返回 null。 */
export function findRealmCard(itemId: number): RealmCardCatalogEntry | null {
  return BY_ITEM_ID.get(itemId) ?? null;
}

/**
 * 按「卡名 × 星级」聚合结界卡库存。
 *
 * 游戏把同规格的卡折叠存储，快照里的 id 既可能是真实例 ObjectId，也可能是
 * `itemid_..._t|序号` 形式的合成键；两者都只按 itemId 计数，不参与身份判断。
 *
 * 每张卡的第 4 个字段（上游 yyx-bridge 命名为 attrs）语义未确认，此处不做解读。
 */
export function aggregateRealmCards(entries: RealmCardEntry[]): RealmCardInventory {
  const groups = new Map<string, RealmCardGroup>();
  let unknownCount = 0;

  for (const entry of entries) {
    const itemId = Number(entry[1]);
    const meta = Number.isFinite(itemId) ? findRealmCard(itemId) : null;
    if (!meta) {
      unknownCount += 1;
      continue;
    }
    const starIndex = REALM_CARD_STARS.indexOf(meta.star);
    if (starIndex < 0) {
      unknownCount += 1;
      continue;
    }
    let group = groups.get(meta.name);
    if (!group) {
      group = {
        name: meta.name,
        starCounts: REALM_CARD_STARS.map(() => 0),
        total: 0,
        iconItemId: null,
      };
      groups.set(meta.name, group);
    }
    group.starCounts[starIndex] = (group.starCounts[starIndex] ?? 0) + 1;
    group.total += 1;
    if (group.iconItemId === null) group.iconItemId = meta.id;
  }

  const ordered = [...groups.values()].sort(
    (left, right) =>
      (NAME_ORDER.get(left.name) ?? Number.MAX_SAFE_INTEGER) -
      (NAME_ORDER.get(right.name) ?? Number.MAX_SAFE_INTEGER),
  );
  return {
    groups: ordered,
    total: ordered.reduce((sum, group) => sum + group.total, 0),
    unknownCount,
  };
}
