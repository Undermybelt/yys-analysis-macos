/**
 * “我的御魂”下三个计算面板共用的本地结果键和清空事件。
 * 派生结果必须先绑定当前角色，再通过 soulKeys 绑定参与计算的御魂，避免角色切换后串用旧结论。
 */
// 缓存键包含角色档案 ID；版本升级时直接废弃旧格式，避免读取没有角色归属的全局缓存。
const SPEED_PREVIEW_CACHE_KEY_PREFIX = "yys-analysis:speed-preview:v3:";
const PVE_CALCULATION_STORAGE_KEY_PREFIX = "yys-pve-panel-result-v13:";
const LEGACY_DERIVED_RESULT_KEYS = [
  "yys-analysis:speed-preview:v2",
  "yys-pve-panel-result-v12",
] as const;
export const MY_SOULS_INVENTORY_CLEARED_EVENT = "my-souls-inventory-cleared";

/** 为指定角色生成一速缓存键；没有激活角色时不允许落盘派生结果。 */
export function speedPreviewStorageKey(profileId: string | null): string | null {
  return profileId ? `${SPEED_PREVIEW_CACHE_KEY_PREFIX}${profileId}` : null;
}

/** 为指定角色生成 PVE 缓存键；角色 ID 是本地结果与御魂库存的第一层归属。 */
export function pveCalculationStorageKey(profileId: string | null): string | null {
  return profileId ? `${PVE_CALCULATION_STORAGE_KEY_PREFIX}${profileId}` : null;
}

/** 清理指定角色的浏览器派生结果，并顺手移除没有角色归属的历史全局缓存。 */
export function clearMySoulsDerivedResults(profileId: string | null): void {
  const keys = [speedPreviewStorageKey(profileId), pveCalculationStorageKey(profileId)];
  for (const key of keys) {
    if (key) localStorage.removeItem(key);
  }
  for (const key of LEGACY_DERIVED_RESULT_KEYS) localStorage.removeItem(key);
}

/** 清理所有角色的浏览器派生结果；清空角色档案时使用，避免残留无法再访问的缓存。 */
export function clearAllMySoulsDerivedResults(): void {
  const keysToRemove: string[] = [];
  for (let index = 0; index < localStorage.length; index += 1) {
    const key = localStorage.key(index);
    if (!key) continue;
    if (
      key.startsWith(SPEED_PREVIEW_CACHE_KEY_PREFIX) ||
      key.startsWith(PVE_CALCULATION_STORAGE_KEY_PREFIX) ||
      (LEGACY_DERIVED_RESULT_KEYS as readonly string[]).includes(key)
    ) {
      keysToRemove.push(key);
    }
  }
  for (const key of keysToRemove) localStorage.removeItem(key);
}
