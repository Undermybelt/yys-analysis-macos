import type { MySoul } from "../api/contracts";
import { describe, expect, it } from "vitest";
import type { PveCalculationResult } from "../pveCalculationEngine";
import {
  createPveCalculationCache,
  expandPveCalculationSelections,
  savePveCalculationCache,
} from "./pveCalculationCache";

/** 构造缓存测试所需的最小御魂对象，覆盖详情弹层读取的全部字段。 */
function createSoul(soulKey: string): MySoul {
  return {
    soulKey,
    snapshotId: "snapshot-test",
    soulInternalId: soulKey,
    sourceStableId: null,
    identityQuality: "stable",
    setId: "狂骨",
    slot: 1,
    quality: 6,
    level: 15,
    mainAttrType: "attack_flat",
    mainAttrValue: 100,
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

/** 生成含重复方案引用的最小计算结果，验证缓存只保存一份御魂详情。 */
function createResult(first: MySoul, second: MySoul): PveCalculationResult {
  return {
    version: 28,
    inventoryCount: 2,
    calculatedAt: "2026-08-24T00:00:00.000Z",
    outputValues: { 狂骨: [1, 2, 3, 4] },
    otherValues: { "test-plan": 5 },
    outputSelections: {
      狂骨: { 土蜘蛛: [first, second] },
    },
    otherSelections: { "test-plan": [first, second] },
    setIconIds: { 狂骨: 300048 },
  };
}

describe("PVE 计算缓存", () => {
  it("去重保存御魂详情，并能恢复方案选择", () => {
    const first = createSoul("soul-1");
    const second = createSoul("soul-2");
    const cache = createPveCalculationCache(
      createResult(first, second),
      "profile-1",
      [first.soulKey, second.soulKey],
      true,
    );

    expect(Object.keys(cache.soulDetails)).toEqual(["soul-1", "soul-2"]);
    expect(cache.outputSelections["狂骨"]?.["土蜘蛛"]).toEqual(["soul-1", "soul-2"]);
    const restored = expandPveCalculationSelections(cache);
    expect(
      restored?.outputSelections["狂骨"]?.["土蜘蛛"]?.map((soul) => soul.soulKey),
    ).toEqual(["soul-1", "soul-2"]);
    expect(restored?.otherSelections["test-plan"]?.[0]?.setId).toBe("狂骨");
  });

  it("本地存储配额异常时返回失败而不抛出异常", () => {
    const first = createSoul("soul-1");
    const cache = createPveCalculationCache(
      createResult(first, first),
      "profile-1",
      [first.soulKey],
      true,
    );
    const quotaStorage = {
      setItem: (): void => {
        throw new Error("QuotaExceededError");
      },
      removeItem: (): void => undefined,
    };

    expect(savePveCalculationCache(quotaStorage, "pve-result", cache)).toBe(false);
  });
});
