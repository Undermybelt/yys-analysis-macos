import { describe, expect, it } from "vitest";
import type { MySoul } from "./api/contracts";
import {
  calculateSpeedPreview,
  type SpeedTargetSet,
} from "./speedSoulCalculationEngine";

/** 构造满足各号位基本要求的最小御魂样本；套装参数用于覆盖套装排行组合。 */
function soul(slot: number, level: number, key: string, setId = "火灵"): MySoul {
  return {
    soulKey: key,
    snapshotId: "snapshot",
    soulInternalId: key,
    sourceStableId: key,
    identityQuality: "verified",
    setId,
    slot,
    quality: 6,
    level,
    mainAttrType: slot === 2 ? "speed" : "attack_rate",
    mainAttrValue: slot === 2 ? 57 : 0,
    initialSubstatCount: 4,
    lockedInSource: false,
    equippedState: null,
    presenceState: "present",
    attributes: [
      {
        attributeIndex: 0,
        attributeType: "speed",
        value: 3,
        enhancementCount: 5,
        countProvenance: "imported",
        fixedAttribute: false,
      },
    ],
  };
}

// 模拟页面从目录筛出的普通四件套，验证引擎不再依赖固定的少数套装名单。
const targetSetIds = [
  "薙魂",
  "钟灵",
  "火灵",
  "招财猫",
  "木魅",
  "魅妖",
  "日女巳时",
  "共潜",
  "蚌精",
  "狂骨",
  "雪幽魂",
  "魍魉之匣",
  "骰子鬼",
  "轮入道",
];
const targetSets: SpeedTargetSet[] = targetSetIds.map((setId) => ({
  setId,
  label: setId,
}));

describe("一速计算引擎", () => {
  it("默认只纳入满级御魂，关闭开关后纳入未满级御魂", () => {
    const maxLevelSouls = Array.from({ length: 6 }, (_, index) =>
      soul(index + 1, 15, `max-${index + 1}`),
    );
    const lowLevelSoul = soul(1, 12, "low-level");

    expect(calculateSpeedPreview([...maxLevelSouls, lowLevelSoul], true)).toMatchObject(
      {
        soulCount: 7,
        qualifiedSoulCount: 6,
      },
    );
    expect(
      calculateSpeedPreview([...maxLevelSouls, lowLevelSoul], false),
    ).toMatchObject({
      soulCount: 7,
      qualifiedSoulCount: 7,
    });
  });

  it("六个号位齐全时生成散件一速结果", () => {
    const result = calculateSpeedPreview(
      Array.from({ length: 6 }, (_, index) => soul(index + 1, 15, `soul-${index + 1}`)),
      true,
    );

    expect(result.sections[0]?.title).toBe("散件一速");
    expect(result.sections[0]?.rows[0]?.speed).toBe("75");
    expect(
      result.sections.every((section) =>
        section.rows.every((row) => row.selected?.length === 6),
      ),
    ).toBe(true);
  });

  it("套装一速展示十四种指定套装并按一速倒序排列", () => {
    const souls = targetSetIds.flatMap((setId, setIndex) =>
      Array.from({ length: 6 }, (_, index) => {
        const item = soul(index + 1, 15, `${setId}-${index + 1}`, setId);
        item.attributes[0]!.value = 3 + setIndex;
        return item;
      }),
    );

    const result = calculateSpeedPreview(souls, true, targetSets);
    const rows = result.sections[3]?.rows ?? [];

    expect(rows).toHaveLength(targetSets.length);
    expect(rows.map((row) => row.speed)).toEqual([
      "153",
      "149",
      "145",
      "141",
      "137",
      "133",
      "129",
      "125",
      "121",
      "117",
      "113",
      "109",
      "105",
      "101",
    ]);
    expect(rows.map((row) => row.label)).toEqual([...targetSetIds].reverse());
    expect(rows.every((row) => row.selected?.length === 6)).toBe(true);
  });

  it("每个目标只要求四件套，剩余两个位置可以使用任意套装", () => {
    const targetSouls = Array.from({ length: 4 }, (_, index) =>
      soul(index + 1, 15, `薙魂-${index + 1}`, "薙魂"),
    );
    // 首领御魂和星痕御魂没有标准四件套，但仍允许占用两个自由搭配位置。
    const flexibleSouls = [
      soul(5, 15, "土蜘蛛-5", "土蜘蛛"),
      soul(6, 15, "八咫镜-6", "八咫镜"),
    ];

    const result = calculateSpeedPreview(
      [...targetSouls, ...flexibleSouls],
      true,
      targetSets,
    );
    const row = result.sections[3]?.rows.find((item) => item.label === "薙魂");

    expect(row?.speed).toBe("75");
    expect(row?.selected?.map((item) => item.setId)).toEqual([
      "薙魂",
      "薙魂",
      "薙魂",
      "薙魂",
      "土蜘蛛",
      "八咫镜",
    ]);
  });

  it("没有完整套装组合时仍保留十四种套装类型占位行", () => {
    const result = calculateSpeedPreview([], true, targetSets);

    expect(result.sections[3]?.rows.map((row) => row.label)).toEqual(targetSetIds);
    expect(result.sections[3]?.rows.every((row) => row.speed === "—")).toBe(true);
  });
});
