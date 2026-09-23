import type {
  Shikigami,
  ShikigamiEmploymentScene,
  ShikigamiRarity,
} from "./api/contracts";

/** 当前内置目录包含 273 个可培养式神与 4 个素材式神；用于让旧本地目录自动升级。 */
export const CURRENT_BUILTIN_SHIKIGAMI_COUNT = 277;

/** 图鉴固定稀有度顺序；未知值放在末尾，避免破坏已安装旧目录的可读性。 */
export const RARITY_ORDER: readonly ShikigamiRarity[] = [
  "UR",
  "SP",
  "SSR",
  "SR",
  "R",
  "N",
  "素材",
];

/** 场景筛选值；ALL 表示不限制 PVE/PVP。 */
export type SceneModeFilter = "ALL" | "PVE" | "PVP";

/** 分组结果统一携带未知稀有度分组，便于数据异常时仍然可见并提示用户。 */
export interface ShikigamiRarityGroup {
  rarity: ShikigamiRarity | "UNKNOWN";
  label: string;
  entries: Shikigami[];
}

const RARITY_LABELS: Record<string, string> = {
  UR: "UR",
  SP: "SP",
  SSR: "SSR",
  SR: "SR",
  R: "R",
  N: "N",
  素材: "素材",
  // 兼容已经安装过的旧目录；新目录和界面统一使用中文稀有度值。
  MATERIAL: "素材",
};

/**
 * 式神仓库里的素材式神名称。
 *
 * 达摩现在也进入静态图鉴，目录条目使用“素材”稀有度；这里仍保留映射，兼容旧
 * 快照或目录安装失败时的素材名称回退，避免页面退回原始 heroId。
 *
 * 对应关系由实测数据确认：仓库数 + 实例数与游戏内数量逐个吻合
 * （410=1518 招福、411=36 御行、412=1253 奉为、413=2299 大吉）。
 */
const MATERIAL_SHIKIGAMI_NAMES: Record<string, string> = {
  "410": "招福达摩",
  "411": "御行达摩",
  "412": "奉为达摩",
  "413": "大吉达摩",
};

/** 返回素材式神名称；不是素材式神时返回 null，由调用方回退到图鉴或原始 ID。 */
export function getMaterialShikigamiName(shikigamiId: string): string | null {
  return MATERIAL_SHIKIGAMI_NAMES[shikigamiId] ?? null;
}

/** 返回稳定的稀有度展示文本；未知值不参与排序但保留可读提示。 */
export function getRarityLabel(rarity: string): string {
  return RARITY_LABELS[rarity] ?? "未知稀有度";
}

/** 将旧目录的英文素材值归一化为中文值，避免升级期间出现两个素材分组。 */
export function normalizeShikigamiRarity(rarity: string): string {
  return rarity === "MATERIAL" ? "素材" : rarity;
}

/** 判断目录条目是否为无培养面板的素材式神，同时兼容旧目录英文值。 */
export function isMaterialShikigamiRarity(rarity: string): boolean {
  return normalizeShikigamiRarity(rarity) === "素材";
}

/** 返回稀有度排序位置；未知稀有度排在标准六档之后。 */
export function rarityOrder(rarity: string): number {
  const normalized = normalizeShikigamiRarity(rarity);
  const index = RARITY_ORDER.indexOf(normalized as ShikigamiRarity);
  return index < 0 ? RARITY_ORDER.length : index;
}

/** 在不改变目录内顺序的前提下按稀有度排序，未知值稳定地排在最后。 */
export function sortShikigamiByRarity(entries: Shikigami[]): Shikigami[] {
  return entries
    .map((entry, index) => ({ entry, index }))
    .sort((left, right) => {
      const rarityDifference =
        rarityOrder(left.entry.rarity) - rarityOrder(right.entry.rarity);
      return rarityDifference || left.index - right.index;
    })
    .map(({ entry }) => entry);
}

/** 按 UR、SP、SSR、SR、R、N、素材分组；未知稀有度单独收纳，不静默丢弃条目。 */
export function groupShikigamiByRarity(entries: Shikigami[]): ShikigamiRarityGroup[] {
  const buckets = new Map<string, Shikigami[]>();
  for (const rarity of RARITY_ORDER) buckets.set(rarity, []);
  const unknown: Shikigami[] = [];

  for (const entry of sortShikigamiByRarity(entries)) {
    const bucket = buckets.get(normalizeShikigamiRarity(entry.rarity));
    if (bucket) bucket.push(entry);
    else unknown.push(entry);
  }

  const groups: ShikigamiRarityGroup[] = RARITY_ORDER.map((rarity) => ({
    rarity,
    label: getRarityLabel(rarity),
    entries: buckets.get(rarity) ?? [],
  }));
  if (unknown.length > 0) {
    groups.push({ rarity: "UNKNOWN", label: "未知稀有度", entries: unknown });
  }
  return groups;
}

/** 将名称、技能、技能效果和场景文本合并为统一搜索字段。 */
function searchText(entry: Shikigami): string {
  const skills = entry.skills.flatMap((skill) => [
    skill.name,
    ...skill.levels.flatMap((level) => [level.effect, level.upgradeEffect ?? ""]),
  ]);
  const scenes = entry.employmentScenes.flatMap((scene) => [
    scene.mode,
    scene.scene,
    scene.recommendation,
  ]);
  // 同时加入稀有度展示文本，让用户搜索“素材”时能命中四个达摩条目。
  return [entry.name, entry.rarity, getRarityLabel(entry.rarity), ...skills, ...scenes]
    .join(" ")
    .toLocaleLowerCase("zh-CN");
}

/** 按名称、技能、场景和 PVE/PVP 标签筛选，空查询只应用其他筛选条件。 */
export function filterShikigami(
  entries: Shikigami[],
  query: string,
  sceneMode: SceneModeFilter = "ALL",
  rarity: ShikigamiRarity | "ALL" = "ALL",
): Shikigami[] {
  const normalizedQuery = query.trim().toLocaleLowerCase("zh-CN");
  return sortShikigamiByRarity(
    entries.filter((entry) => {
      const matchesRarity =
        rarity === "ALL" || normalizeShikigamiRarity(entry.rarity) === rarity;
      const matchesScene =
        sceneMode === "ALL" ||
        entry.employmentScenes.some((scene) => scene.mode === sceneMode);
      const matchesQuery =
        !normalizedQuery || searchText(entry).includes(normalizedQuery);
      return matchesRarity && matchesScene && matchesQuery;
    }),
  );
}

/** 计算从 111 目标技能等级开始所需的理论黑蛋数量，非法值按边界安全处理。 */
export function calculateBlackEggsFrom111(targetLevels: number[]): number {
  return targetLevels.reduce(
    (total, target) => total + Math.max(0, Math.min(5, Math.floor(target)) - 1),
    0,
  );
}

/** 选取目录首条作为默认详情；空列表返回 null，避免模板访问未定义对象。 */
export function getDefaultShikigami(entries: Shikigami[]): Shikigami | null {
  return sortShikigamiByRarity(entries)[0] ?? null;
}

/** 聚合场景标签，供详情页展示而不让模板重复处理去重规则。 */
export function uniqueSceneModes(scenes: ShikigamiEmploymentScene[]): string[] {
  return [...new Set(scenes.map((scene) => scene.mode))];
}
