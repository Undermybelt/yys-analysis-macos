import { describe, expect, it } from "vitest";
import {
  getMiracleBasePanelItems,
  getMiracleTargetInputFromPanel,
  parseOptionalMetricMaximum,
} from "./miracleConchPresentation";

describe("神奇海螺基础面板摘要", () => {
  it("应展示八项属性，并将命中与抵抗默认设为 0", () => {
    const items = getMiracleBasePanelItems({
      attack: 4154,
      hp: 11734.76,
      defense: 493.92,
      speed: 112,
      critRate: 0.1,
      critDamage: 0.5,
      grades: { attack: "S", hp: "S", defense: "S", speed: "S", critRate: "S" },
    });

    expect(items.map((item) => item.label)).toEqual([
      "攻击",
      "生命",
      "防御",
      "速度",
      "暴击",
      "暴伤",
      "效果命中",
      "效果抵抗",
    ]);
    expect(items.find((item) => item.key === "critDamage")?.value).toBe(1.5);
    expect(items.find((item) => item.key === "effectHit")?.value).toBe(0);
    expect(items.find((item) => item.key === "effectResist")?.value).toBe(0);
  });

  it("应把式神基础面板转换成可直接编辑的目标输入", () => {
    const target = getMiracleTargetInputFromPanel({
      attack: 4154,
      hp: 11734.76,
      defense: 493.92,
      speed: 112,
      critRate: 0.1,
      critDamage: 0.5,
      grades: { attack: "S", hp: "S", defense: "S", speed: "S", critRate: "S" },
    });

    expect(target).toEqual({
      attack: "4154",
      hp: "11734.76",
      defense: "493.92",
      speed: "112",
      critRate: "10",
      critDamage: "150",
      effectHit: "0",
      effectResist: "0",
    });
  });
});

describe("神奇海螺属性区间输入", () => {
  it("应保留数字输入框产生的上限数值", () => {
    expect(parseOptionalMetricMaximum(122)).toBe(122);
    expect(parseOptionalMetricMaximum("122")).toBe(122);
    expect(parseOptionalMetricMaximum("")).toBeNull();
  });
});
