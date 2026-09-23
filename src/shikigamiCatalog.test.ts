import { describe, expect, it } from "vitest";

import type { Shikigami } from "./api/contracts";
import {
  calculateBlackEggsFrom111,
  filterShikigami,
  getDefaultShikigami,
  getRarityLabel,
  groupShikigamiByRarity,
  normalizeShikigamiRarity,
  sortShikigamiByRarity,
} from "./shikigamiCatalog";

/** 构造可复用的最小图鉴条目；测试只覆盖纯函数规则，不依赖 Tauri 或 SQLite。 */
function entry(overrides: Partial<Shikigami> = {}): Shikigami {
  return {
    catalogVersion: "test",
    shikigamiId: "test-1",
    name: "测试式神",
    rarity: "SR",
    iconAssetId: null,
    panelLevel: 40,
    awakened: true,
    panel: {
      attack: 100,
      hp: 1000,
      defense: 50,
      speed: 110,
      critRate: 0.1,
      critDamage: 0.5,
      grades: { attack: "A", hp: "B", defense: "C", speed: "S", critRate: "A" },
    },
    skills: [
      {
        skillId: "skill-1",
        name: "焰火术",
        type: "主动",
        energyCost: 3,
        levels: [{ level: 1, effect: "造成伤害", upgradeEffect: null }],
      },
    ],
    skillInvestment: {
      targetLevels: [1, 5, 5],
      priority: [1, 2, 3],
      blackEggsFrom111: 8,
      note: "先升级核心技能",
    },
    employmentScenes: [
      { mode: "PVE", scene: "御魂副本", recommendation: "稳定输出" },
      { mode: "PVP", scene: "斗技", recommendation: "先手压制" },
    ],
    gameVersion: "test",
    evidence: [{ source: "test", url: null, evidence: "fixture" }],
    ...overrides,
  };
}

describe("式神图鉴纯函数", () => {
  it("按 UR、SP、SSR、SR、R、N、素材排序", () => {
    const entries = [
      entry({ shikigamiId: "n", name: "N", rarity: "N" }),
      entry({ shikigamiId: "sp", name: "SP", rarity: "SP" }),
      entry({
        shikigamiId: "ur",
        name: "UR",
        rarity: "UR",
      }),
      entry({ shikigamiId: "ssr", name: "SSR", rarity: "SSR" }),
    ];

    expect(sortShikigamiByRarity(entries).map((item) => item.shikigamiId)).toEqual([
      "ur",
      "sp",
      "ssr",
      "n",
    ]);
    expect(groupShikigamiByRarity(entries).map((group) => group.rarity)).toEqual([
      "UR",
      "SP",
      "SSR",
      "SR",
      "R",
      "N",
      "素材",
    ]);
    expect(groupShikigamiByRarity(entries).at(0)?.entries).toHaveLength(1);
  });

  it("素材条目按独立稀有度分组且可以被筛选", () => {
    const material = entry({
      shikigamiId: "411",
      name: "御行达摩",
      rarity: "素材",
    });

    expect(groupShikigamiByRarity([material]).at(-1)).toMatchObject({
      rarity: "素材",
      label: "素材",
      entries: [material],
    });
    expect(filterShikigami([material], "御行达摩", "ALL", "素材")).toEqual([material]);
  });

  it("素材稀有度使用中文并兼容旧英文目录值", () => {
    expect(getRarityLabel("素材")).toBe("素材");
    expect(getRarityLabel("MATERIAL")).toBe("素材");
    expect(normalizeShikigamiRarity("MATERIAL")).toBe("素材");
  });

  it("支持名称、技能、场景和 PVE/PVP 筛选", () => {
    const entries = [
      entry({
        shikigamiId: "pve",
        name: "副本式神",
        employmentScenes: [{ mode: "PVE", scene: "御魂副本", recommendation: "稳定" }],
      }),
      entry({
        shikigamiId: "pvp",
        name: "斗技式神",
        employmentScenes: [{ mode: "PVP", scene: "斗技", recommendation: "控制" }],
      }),
    ];

    expect(filterShikigami(entries, "焰火术").map((item) => item.shikigamiId)).toEqual([
      "pve",
      "pvp",
    ]);
    expect(filterShikigami(entries, "斗技").map((item) => item.shikigamiId)).toEqual([
      "pvp",
    ]);
    expect(filterShikigami(entries, "", "PVE").map((item) => item.shikigamiId)).toEqual(
      ["pve"],
    );
    expect(filterShikigami(entries, "", "PVP").map((item) => item.shikigamiId)).toEqual(
      ["pvp"],
    );
  });

  /** 神奇海螺复用目录筛选规则，先选稀有度后只返回对应的式神。 */
  it("支持按稀有度筛选式神", () => {
    const entries = [
      entry({ shikigamiId: "ssr", name: "SSR", rarity: "SSR" }),
      entry({ shikigamiId: "sr", name: "SR", rarity: "SR" }),
    ];

    expect(
      filterShikigami(entries, "", "ALL", "SSR").map((item) => item.shikigamiId),
    ).toEqual(["ssr"]);
  });

  it("按 111 起算目标等级计算黑蛋，并安全处理边界值", () => {
    expect(calculateBlackEggsFrom111([1, 5, 5])).toBe(8);
    expect(calculateBlackEggsFrom111([0, 6, 2.9])).toBe(5);
  });

  it("空列表没有默认选中项，空查询不会误删条目", () => {
    expect(getDefaultShikigami([])).toBeNull();
    expect(filterShikigami([], "任意关键词")).toEqual([]);
    expect(filterShikigami([entry()], "")).toHaveLength(1);
  });
});
