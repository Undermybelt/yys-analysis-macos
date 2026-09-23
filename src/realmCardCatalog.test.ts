import { describe, expect, it } from "vitest";
import type { RealmCardEntry } from "./api/contracts";
import {
  REALM_CARD_STARS,
  aggregateRealmCards,
  findRealmCard,
} from "./realmCardCatalog";

/** 构造一张结界卡；attrs 语义未确认，取值不参与聚合。 */
function card(itemId: number, id = `card-${itemId}`): RealmCardEntry {
  return [id, itemId, 5, [2400, 0]];
}

describe("findRealmCard", () => {
  it("itemId 末位是星级，同名卡跨 6 个 itemId", () => {
    expect(findRealmCard(200001)).toEqual({
      id: 200001,
      name: "太阴符咒",
      star: 1,
    });
    expect(findRealmCard(200005)).toEqual({
      id: 200005,
      name: "太阴符咒",
      star: 5,
    });
  });

  it("未收录的 itemId 返回 null", () => {
    expect(findRealmCard(999999)).toBeNull();
  });
});

describe("aggregateRealmCards", () => {
  it("同名卡的不同星级归到一行，按星级分列计数", () => {
    const { groups, total } = aggregateRealmCards([
      card(200005),
      card(200005, "another"),
      card(200001),
    ]);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.name).toBe("太阴符咒");
    // REALM_CARD_STARS 是 [6,5,4,3,2,1]：5 星两张、1 星一张。
    expect(groups[0]?.starCounts).toEqual([0, 2, 0, 0, 0, 1]);
    expect(groups[0]?.total).toBe(3);
    expect(total).toBe(3);
  });

  it("不同卡名分成多行，并按目录顺序稳定排列", () => {
    const { groups } = aggregateRealmCards([card(200031), card(200005), card(200021)]);
    expect(groups.map((group) => group.name)).toEqual(["太阴符咒", "斗鱼", "太鼓"]);
  });

  it("同名同星的多个 itemId 合并计数", () => {
    // 目录里 200005 与 200397 都是太阴符咒 5 星。
    const { groups } = aggregateRealmCards([card(200005), card(200397)]);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.starCounts[REALM_CARD_STARS.indexOf(5)]).toBe(2);
  });

  it("目录未收录的 itemId 计入 unknownCount 而不静默丢弃", () => {
    const { groups, total, unknownCount } = aggregateRealmCards([
      card(200005),
      card(999999),
    ]);
    expect(groups).toHaveLength(1);
    expect(total).toBe(1);
    expect(unknownCount).toBe(1);
  });

  it("空快照返回空库存", () => {
    expect(aggregateRealmCards([])).toEqual({
      groups: [],
      total: 0,
      unknownCount: 0,
    });
  });

  it("折叠存储的合成键与真实例 ObjectId 都只按 itemId 计数", () => {
    const { total, groups } = aggregateRealmCards([
      card(200005, "200005_2400_0_24|0"),
      card(200005, "6a83aae617ff2e58ff6e6231"),
    ]);
    expect(total).toBe(2);
    expect(groups[0]?.starCounts[REALM_CARD_STARS.indexOf(5)]).toBe(2);
  });
});
