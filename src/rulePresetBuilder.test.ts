import { describe, expect, it } from "vitest";
import {
  buildManualPreset,
  createManualRuleCard,
  manualRuleToPayload,
  parseManualRuleCards,
  parseScoreColorThresholds,
  stripCanonicalHash,
  validateScoreColorThresholds,
  validateManualCards,
} from "./rulePresetBuilder";

describe("手动御魂评分规则构造", () => {
  it("将多选套装、号位主属性和副属性写入可分享规则", () => {
    const card = createManualRuleCard(1);
    card.name = "输出两件套";
    card.setIds = ["破势", "针女"];
    card.slots = [2, 4, 6];
    card.slotMainPairs = [
      { slot: 2, mainAttribute: "speed" },
      { slot: 4, mainAttribute: "attack_rate" },
      { slot: 6, mainAttribute: "crit_rate" },
    ];
    card.coreSubstats = ["crit_rate", "crit_damage"];
    card.generalSubstats = ["speed"];

    const rule = manualRuleToPayload(card);
    const selector = rule.selector as Record<string, unknown>;

    expect(selector.setIds).toEqual(["破势", "针女"]);
    expect(selector.slotMainPairs).toEqual(card.slotMainPairs);
    expect(selector.requiredSubstats).toEqual({
      allOf: ["crit_rate", "crit_damage"],
      anyOf: ["speed"],
    });
    expect(rule).not.toHaveProperty("soulKeys");
  });

  it("核心属性和一般属性不会重复，且评分阈值按顺序校验", () => {
    const card = createManualRuleCard(1);
    card.coreSubstats = ["speed"];
    card.generalSubstats = ["speed"];
    expect(validateManualCards([card])).toContain("核心属性和一般属性不能重复");

    card.generalSubstats = ["crit_rate"];
    card.scoring.keepScore = 40;
    card.scoring.observeScore = 60;
    expect(validateManualCards([card])).toContain("弃置 ≤ 观察 ≤ 保留");
  });

  it("能从旧版 selector 读取多条规则卡片并保留主属性配对", () => {
    const parsed = {
      rules: [
        {
          id: "manual-one",
          name: "速度输出",
          selector: {
            setIds: ["招财猫"],
            slotMainPairs: [{ slot: 2, mainAttribute: "speed" }],
            requiredSubstats: { allOf: ["speed"], anyOf: ["crit_rate"] },
          },
          candidateUses: ["manual-rule"],
        },
      ],
    };

    const cards = parseManualRuleCards(parsed);
    // 该输入明确包含一条规则，断言前固定取出首条，避免测试噪声掩盖规则解析失败。
    const card = cards[0]!;
    expect(cards).toHaveLength(1);
    expect(card.setIds).toEqual(["招财猫"]);
    expect(card.slots).toEqual([2]);
    expect(card.slotMainPairs).toEqual([{ slot: 2, mainAttribute: "speed" }]);
    expect(card.generalSubstats).toEqual(["crit_rate"]);
  });

  it("生成的标准只含规则条件，不含具体御魂身份", () => {
    const preset = buildManualPreset(
      {},
      {
        id: "manual.score.test",
        version: "0.1.0",
        title: "测试标准",
        author: "测试者",
        description: "手动规则测试",
      },
      [createManualRuleCard(1)],
    );

    expect(preset).toHaveProperty("rules");
    expect(preset).not.toHaveProperty("soulKeys");
    expect(JSON.stringify(preset)).not.toContain("snapshotId");
  });

  it("编辑已导出的规则时应移除旧的规范哈希", () => {
    const source = { canonicalSha256: "旧规则内容的哈希", title: "原始标题" };
    const preset = buildManualPreset(
      source,
      {
        id: "manual.score.edited",
        version: "0.1.0",
        title: "编辑后的标准",
        author: "测试者",
        description: "编辑导出文件回归测试",
      },
      [createManualRuleCard(1)],
    );

    expect(preset).not.toHaveProperty("canonicalSha256");
    expect(stripCanonicalHash(source)).toEqual({ title: "原始标题" });
  });

  it("保存并读取评分颜色阈值，旧标准使用默认分数线", () => {
    const preset = buildManualPreset(
      {},
      {
        id: "manual.score.colors",
        version: "0.1.0",
        title: "颜色测试标准",
        author: "测试者",
        description: "颜色阈值测试",
      },
      [createManualRuleCard(1)],
      { jadeScore: 55, goldScore: 68, rainbowScore: 82 },
    );

    expect(preset.scoreColorThresholds).toEqual({
      jadeScore: 55,
      goldScore: 68,
      rainbowScore: 82,
    });
    expect(parseScoreColorThresholds(preset)).toEqual({
      jadeScore: 55,
      goldScore: 68,
      rainbowScore: 82,
    });
    expect(parseScoreColorThresholds({})).toEqual({
      jadeScore: 60,
      goldScore: 70,
      rainbowScore: 75,
    });
    expect(
      validateScoreColorThresholds({ jadeScore: 75, goldScore: 70, rainbowScore: 80 }),
    ).toContain("青玉 ≤ 金橙 ≤ 彩虹");
  });
});
