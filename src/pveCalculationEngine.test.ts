import { describe, expect, it } from "vitest";
import type { MySoul, SoulSet } from "./api/contracts";
import {
  benchmarkPveExactPair,
  calculatePvePanel,
  PVE_PRODUCTION_EXACT_PAIR_SEARCH_STRATEGY,
  type PveCalculationProgress,
  type PveExactPairSearchStrategy,
} from "./pveCalculationEngine";

/** 构造最小满暴输出组合，验证索引化搜索仍能返回六件实际御魂。 */
function createSoul(
  soulKey: string,
  setId: string,
  slot: number,
  mainAttrType: string,
  mainAttrValue: number,
): MySoul {
  return {
    soulKey,
    snapshotId: "snapshot-test",
    soulInternalId: soulKey,
    sourceStableId: null,
    identityQuality: "stable",
    setId,
    slot,
    quality: 6,
    level: 15,
    mainAttrType,
    mainAttrValue,
    initialSubstatCount: 4,
    lockedInSource: null,
    equippedState: null,
    presenceState: "present",
    attributes: [
      {
        attributeIndex: 0,
        attributeType: "crit_rate",
        value: 0.3,
        enhancementCount: null,
        countProvenance: "source",
        fixedAttribute: false,
      },
    ],
  };
}

/** 为性能/规则回归样例追加指定暴伤副属性，保持原有六星满暴构造器可复用。 */
function withCritDamage(soul: MySoul, value: number): MySoul {
  return {
    ...soul,
    attributes: [
      ...soul.attributes,
      {
        attributeIndex: soul.attributes.length,
        attributeType: "crit_damage",
        value,
        enhancementCount: null,
        countProvenance: "source",
        fixedAttribute: false,
      },
    ],
  };
}

describe("PVE 计算引擎", () => {
  it("生产精确组合搜索固定使用 Meet-in-the-middle", () => {
    expect(PVE_PRODUCTION_EXACT_PAIR_SEARCH_STRATEGY).toBe("meet-in-the-middle");
  });

  it("空库存也会按固定 58 个计算单元发布完整进度", () => {
    const progress: Array<{ completed: number; message: string }> = [];
    const diagnostics: Array<{ candidateCount: number; durationMs: number }> = [];
    // 组合开始日志必须覆盖每个计算单元，确保后台搜索较慢时用户能知道当前目标。
    const started: string[] = [];

    const result = calculatePvePanel(
      [],
      [],
      (event) => {
        progress.push({ completed: event.completed, message: event.message });
        diagnostics.push({
          candidateCount: event.candidateCount,
          durationMs: event.durationMs,
        });
      },
      (message) => started.push(message),
    );

    expect(progress).toHaveLength(58);
    expect(diagnostics).toHaveLength(58);
    expect(diagnostics[0]?.candidateCount).toBe(0);
    expect(typeof diagnostics[0]?.durationMs).toBe("number");
    expect(started).toHaveLength(58);
    expect(started[0]).toBe("开始计算 狂骨 + 土蜘蛛 组合");
    expect(started).toContain("开始计算 狂骨 + 天羽羽斩 组合");
    expect(started).toContain("开始计算 思金神 · 隐念 + 歌姬 组合");
    expect(progress[0]).toEqual({ completed: 1, message: "狂骨 · 土蜘蛛 已完成" });
    expect(progress).toContainEqual({
      completed: 4,
      message: "狂骨 · 天羽羽斩 已完成",
    });
    expect(progress.at(-1)).toEqual({
      completed: 58,
      message: "思金神 · 隐念 + 歌姬 已完成",
    });
    expect(result.version).toBe(28);
    expect(result.outputValues["狂骨"]).toEqual([0, 0, 0, 0]);
    expect(result.outputValues["恶楼"]).toEqual([0, 0, 0, 0]);
    expect(result.otherValues["inaba-scattered"]).toBe(0);
  });

  it("按号位和套装索引搜索时仍能返回满足满暴的六件组合", () => {
    const souls = [
      createSoul("ordinary-1", "狂骨", 1, "attack_flat", 100),
      createSoul("ordinary-2", "狂骨", 2, "attack_rate", 0.55),
      createSoul("boss-3", "土蜘蛛", 3, "attack_flat", 100),
      createSoul("ordinary-4", "狂骨", 4, "attack_rate", 0.55),
      createSoul("ordinary-5", "狂骨", 5, "attack_flat", 100),
      createSoul("boss-6", "土蜘蛛", 6, "crit_damage", 0.89),
    ];

    const progress: PveCalculationProgress[] = [];
    const result = calculatePvePanel(souls, [], (event) => progress.push(event));

    expect(result.outputValues["狂骨"]?.[0]).toBeGreaterThan(0);
    expect(result.outputSelections["狂骨"]?.["土蜘蛛"]).toHaveLength(6);
    expect(progress[0]?.candidateCount).toBe(6);
    expect(progress[0]?.durationMs).toBeGreaterThanOrEqual(0);
    expect(result.outputValues["隐念"]).toEqual([0, 0, 0, 0]);
  });

  it("输出四件套可以搭配天羽羽斩两件套返回六件组合", () => {
    const souls = [
      createSoul("ordinary-1", "狂骨", 1, "attack_flat", 100),
      createSoul("ordinary-2", "狂骨", 2, "attack_rate", 0.55),
      createSoul("tianyu-3", "天羽羽斩", 3, "attack_flat", 100),
      createSoul("ordinary-4", "狂骨", 4, "attack_rate", 0.55),
      createSoul("ordinary-5", "狂骨", 5, "attack_flat", 100),
      createSoul("tianyu-6", "天羽羽斩", 6, "crit_damage", 0.89),
    ];

    const result = calculatePvePanel(souls, [], () => undefined);

    expect(result.outputValues["狂骨"]?.[3]).toBeGreaterThan(0);
    expect(result.outputSelections["狂骨"]?.["天羽羽斩"]).toHaveLength(6);
  });

  it("输出矩阵新增恶楼四件套计算行", () => {
    const souls = [
      createSoul("evil-1", "恶楼", 1, "attack_flat", 100),
      createSoul("evil-2", "恶楼", 2, "attack_rate", 0.55),
      createSoul("boss-3", "土蜘蛛", 3, "attack_flat", 100),
      createSoul("evil-4", "恶楼", 4, "attack_rate", 0.55),
      createSoul("evil-5", "恶楼", 5, "attack_flat", 100),
      createSoul("boss-6", "土蜘蛛", 6, "crit_damage", 0.89),
    ];

    const result = calculatePvePanel(souls, [], () => undefined);

    expect(result.outputValues["恶楼"]?.[0]).toBeGreaterThan(0);
    expect(result.outputSelections["恶楼"]?.["土蜘蛛"]).toHaveLength(6);
  });

  it("输出矩阵新增心眼和无刀取四件套计算行", () => {
    const souls = [
      createSoul("心眼-1", "心眼", 1, "attack_flat", 100),
      createSoul("心眼-2", "心眼", 2, "attack_rate", 0.55),
      createSoul("心眼-4", "心眼", 4, "attack_rate", 0.55),
      createSoul("心眼-5", "心眼", 5, "attack_flat", 100),
      createSoul("无刀取-1", "无刀取", 1, "attack_flat", 100),
      createSoul("无刀取-2", "无刀取", 2, "attack_rate", 0.55),
      createSoul("无刀取-4", "无刀取", 4, "attack_rate", 0.55),
      createSoul("无刀取-5", "无刀取", 5, "attack_flat", 100),
      createSoul("土蜘蛛-3", "土蜘蛛", 3, "attack_flat", 100),
      createSoul("土蜘蛛-6", "土蜘蛛", 6, "crit_damage", 0.89),
    ];

    const result = calculatePvePanel(souls, [], () => undefined);

    // 两套新增输出御魂都必须能与首领两件套组成六件组合，并写入详情选择结果。
    expect(result.outputValues["心眼"]?.[0]).toBeGreaterThan(0);
    expect(result.outputSelections["心眼"]?.["土蜘蛛"]).toHaveLength(6);
    expect(result.outputValues["无刀取"]?.[0]).toBeGreaterThan(0);
    expect(result.outputSelections["无刀取"]?.["土蜘蛛"]).toHaveLength(6);
  });

  it("纺愿缘结神火灵方案只要求四件火灵，不限定其余两件套装", () => {
    const souls = [
      createSoul("fire-1", "火灵", 1, "attack_flat", 100),
      createSoul("fire-2", "火灵", 2, "speed", 57),
      createSoul("needle-3", "针女", 3, "defense_flat", 100),
      createSoul("fire-4", "火灵", 4, "attack_rate", 0.55),
      createSoul("fire-5", "火灵", 5, "hp_flat", 100),
      createSoul("needle-6", "针女", 6, "crit_damage", 0.89),
    ];

    const result = calculatePvePanel(souls, [], () => undefined);
    const selected = result.otherSelections["sp-yuan-fire"] ?? [];

    // 第二套即使是针女也必须可以进入结果，不能由展示名称反推成土蜘蛛条件。
    expect(selected.filter((soul) => soul.setId === "火灵")).toHaveLength(4);
    expect(selected.filter((soul) => soul.setId === "针女")).toHaveLength(2);
    expect(selected.some((soul) => soul.setId === "土蜘蛛")).toBe(false);
  });

  it("纺愿缘结神最高爆伤不能剪掉带暴伤两件套的组合", () => {
    const souls = [
      createSoul("fire-1", "火灵", 1, "attack_flat", 100),
      createSoul("fire-2", "火灵", 2, "speed", 57),
      createSoul("fire-3", "火灵", 3, "defense_flat", 100),
      createSoul("fire-4", "火灵", 4, "attack_rate", 0.55),
      withCritDamage(createSoul("wuda取-5", "无刀取", 5, "hp_flat", 100), 0.4),
      withCritDamage(createSoul("wuda取-6", "无刀取", 6, "crit_damage", 0.89), 0.4),
      withCritDamage(createSoul("needle-5", "针女", 5, "hp_flat", 100), 0.45),
      withCritDamage(createSoul("needle-6", "针女", 6, "crit_damage", 0.89), 0.45),
    ];
    const catalogSets: SoulSet[] = [
      {
        catalogVersion: "test",
        setId: "无刀取",
        name: "无刀取",
        iconAssetId: null,
        category: "暴击伤害",
        specialCategory: null,
        twoPieceEffect: "暴击伤害+20%",
        fourPieceEffect: null,
        enhancementRule: null,
        evidence: [],
      },
    ];

    const result = calculatePvePanel(souls, catalogSets, () => undefined);
    const selected = result.otherSelections["sp-yuan-fire"] ?? [];

    // 无刀取的原始副属性略低，但两件套暴伤加成后应胜过针女组合。
    expect(result.otherValues["sp-yuan-fire"]).toBe(339);
    expect(selected.filter((soul) => soul.setId === "无刀取")).toHaveLength(2);
  });

  it("单测狂骨+土蜘蛛的三种精确搜索策略并打印各自耗时", () => {
    const souls = [
      createSoul("ordinary-1", "狂骨", 1, "attack_flat", 100),
      createSoul("ordinary-2", "狂骨", 2, "attack_rate", 0.55),
      createSoul("boss-3", "土蜘蛛", 3, "attack_flat", 100),
      createSoul("ordinary-4", "狂骨", 4, "attack_rate", 0.55),
      createSoul("ordinary-5", "狂骨", 5, "attack_flat", 100),
      createSoul("boss-6", "土蜘蛛", 6, "crit_damage", 0.89),
    ];
    const strategies: PveExactPairSearchStrategy[] = [
      "branch-and-bound",
      "meet-in-the-middle",
      "pareto",
    ];
    // 三种策略必须在同一库存和同一配方下得到相同最优值，耗时只用于性能比较。
    const results = strategies.map((strategy) =>
      benchmarkPveExactPair(souls, [], strategy),
    );

    console.info(
      `[PVE-BENCHMARK] 狂骨 · 土蜘蛛 ${results
        .map(
          (result) =>
            `${result.strategy}: ${result.durationMs} ms, 候选 ${result.candidateCount} 枚, 结果 ${result.value}`,
        )
        .join(" | ")}`,
    );
    expect(results.map((result) => result.candidateCount)).toEqual([6, 6, 6]);
    expect(results.every((result) => result.durationMs >= 0)).toBe(true);
    expect(results.every((result) => result.selection)).toBe(true);
    expect(new Set(results.map((result) => result.value)).size).toBe(1);
  });

  it("大候选的片叶+荒骷髅搜索会在半区中间层提前剪枝", () => {
    const souls: MySoul[] = [];
    for (let slot = 1; slot <= 6; slot += 1) {
      const mainAttrType =
        slot === 2 || slot === 4
          ? "attack_rate"
          : slot === 6
            ? "crit_damage"
            : "attack_flat";
      for (let index = 0; index < 30; index += 1) {
        for (const [setId, prefix] of [
          ["片叶之苇", "leaf"],
          ["荒骷髅", "boss"],
        ] as const) {
          const soul = createSoul(
            `${prefix}-${slot}-${index}`,
            setId,
            slot,
            mainAttrType,
            slot === 6 ? 0.55 + index * 0.001 : index * 10,
          );
          soul.attributes = [
            {
              ...soul.attributes[0]!,
              value: (30 - index) * 0.01,
            },
            {
              ...soul.attributes[0]!,
              attributeIndex: 1,
              attributeType: "crit_damage",
              value: index * 0.01,
            },
            {
              ...soul.attributes[0]!,
              attributeIndex: 2,
              attributeType: "attack_rate",
              value: (30 - index) * 0.005,
            },
          ];
          souls.push(soul);
        }
      }
    }

    const progress: PveCalculationProgress[] = [];
    calculatePvePanel(souls, [], (event) => progress.push(event));
    const target = progress.find((event) => event.message === "片叶 · 荒骷髅 已完成");

    expect(target?.candidateCount).toBe(360);
    expect(target?.item.selection).toHaveLength(6);
    expect(target?.item.value).toBeGreaterThan(0);
  });
});
