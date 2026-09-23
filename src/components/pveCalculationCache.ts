import type { MySoul } from "../api/contracts";
import type { PveCalculationResult } from "../pveCalculationEngine";

/** PVE 详情弹层实际需要的最小御魂字段；缓存不再重复保存快照和来源元数据。 */
export type PveSoulDetail = Pick<
  MySoul,
  | "soulKey"
  | "setId"
  | "slot"
  | "quality"
  | "level"
  | "mainAttrType"
  | "mainAttrValue"
  | "attributes"
>;

/** 缓存中的输出方案只保存御魂稳定键，详情通过 soulDetails 表恢复。 */
type CachedOutputSelections = Record<string, Record<string, string[]>>;

/** 缓存中的其他方案只保存御魂稳定键，避免同一枚御魂被多个方案重复序列化。 */
type CachedOtherSelections = Record<string, string[]>;

/** PVE 本地缓存结构；数值保持原样，组合详情改为“键引用 + 去重详情表”。 */
export type PveCalculationCache = Omit<
  PveCalculationResult,
  "outputSelections" | "otherSelections"
> & {
  profileId: string;
  soulKeys: string[];
  onlyMaxLevel: boolean;
  outputSelections: CachedOutputSelections;
  otherSelections: CachedOtherSelections;
  soulDetails: Record<string, PveSoulDetail>;
};

/** 页面恢复后继续供详情弹层使用的组合选择结构。 */
export type RestoredPveSelections = {
  outputSelections: Record<string, Record<string, PveSoulDetail[]>>;
  otherSelections: Record<string, PveSoulDetail[]>;
};

/** 复制详情所需字段并断开 Worker 对象引用，保证缓存内容稳定且可序列化。 */
function compactSoul(soul: MySoul): PveSoulDetail {
  return {
    soulKey: soul.soulKey,
    setId: soul.setId,
    slot: soul.slot,
    quality: soul.quality,
    level: soul.level,
    mainAttrType: soul.mainAttrType,
    mainAttrValue: soul.mainAttrValue,
    attributes: soul.attributes.map((attribute) => ({ ...attribute })),
  };
}

/** 将计算结果转换为小体积缓存；同一枚御魂在多个方案中只存储一次详情。 */
export function createPveCalculationCache(
  result: PveCalculationResult,
  profileId: string,
  soulKeys: string[],
  onlyMaxLevel: boolean,
): PveCalculationCache {
  const soulDetails: Record<string, PveSoulDetail> = {};
  const rememberSoul = (soul: MySoul): string => {
    if (!soulDetails[soul.soulKey]) soulDetails[soul.soulKey] = compactSoul(soul);
    return soul.soulKey;
  };
  const outputSelections: CachedOutputSelections = Object.fromEntries(
    Object.entries(result.outputSelections).map(([planId, selections]) => [
      planId,
      Object.fromEntries(
        Object.entries(selections).map(([bossId, selected]) => [
          bossId,
          selected.map(rememberSoul),
        ]),
      ),
    ]),
  );
  const otherSelections: CachedOtherSelections = Object.fromEntries(
    Object.entries(result.otherSelections).map(([planId, selected]) => [
      planId,
      selected.map(rememberSoul),
    ]),
  );

  return {
    version: result.version,
    inventoryCount: result.inventoryCount,
    calculatedAt: result.calculatedAt,
    outputValues: result.outputValues,
    otherValues: result.otherValues,
    setIconIds: result.setIconIds,
    profileId,
    soulKeys,
    onlyMaxLevel,
    outputSelections,
    otherSelections,
    soulDetails,
  };
}

/** 按稳定键恢复详情组合；发现损坏或不完整缓存时返回空值并交给页面清理。 */
export function expandPveCalculationSelections(
  cache: PveCalculationCache,
): RestoredPveSelections | null {
  if (
    !cache.soulDetails ||
    typeof cache.soulDetails !== "object" ||
    !cache.outputSelections ||
    typeof cache.outputSelections !== "object" ||
    !cache.otherSelections ||
    typeof cache.otherSelections !== "object"
  ) {
    return null;
  }

  const resolve = (keys: string[]): PveSoulDetail[] | null => {
    if (!Array.isArray(keys)) return null;
    const selected = keys.map((soulKey) => cache.soulDetails[soulKey]);
    return selected.every((soul): soul is PveSoulDetail => Boolean(soul))
      ? selected
      : null;
  };

  const outputSelections: Record<string, Record<string, PveSoulDetail[]>> = {};
  for (const [planId, selections] of Object.entries(cache.outputSelections)) {
    if (!selections || typeof selections !== "object") return null;
    const restored: Record<string, PveSoulDetail[]> = {};
    for (const [bossId, keys] of Object.entries(selections)) {
      const selected = resolve(keys);
      if (!selected) return null;
      restored[bossId] = selected;
    }
    outputSelections[planId] = restored;
  }

  const otherSelections: Record<string, PveSoulDetail[]> = {};
  for (const [planId, keys] of Object.entries(cache.otherSelections)) {
    const selected = resolve(keys);
    if (!selected) return null;
    otherSelections[planId] = selected;
  }

  return { outputSelections, otherSelections };
}

/** 写入本地缓存失败时先清理当前角色旧缓存并重试，让本次计算结果仍可继续展示。 */
export function savePveCalculationCache(
  storage: Pick<Storage, "setItem" | "removeItem">,
  storageKey: string,
  cache: PveCalculationCache,
): boolean {
  try {
    const payload = JSON.stringify(cache);
    try {
      storage.setItem(storageKey, payload);
    } catch {
      // 旧版本缓存可能比当前紧凑结果更大；删除当前角色旧结果后再尝试一次。
      storage.removeItem(storageKey);
      storage.setItem(storageKey, payload);
    }
    return true;
  } catch {
    return false;
  }
}
