import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const catalogPath = path.join(
  root,
  "src-tauri",
  "src",
  "data",
  "shikigami-catalog.json",
);
const attrUrl = (id, awake) =>
  `https://g37simulator.webapp.163.com/get_hero_attr?heroid=${id}&awake=${awake}&level=40&star=6`;
const skillUrl = (id) =>
  `https://g37simulator.webapp.163.com/get_hero_skill?heroid=${id}&awake=0&level=0&star=2`;

/** 官方评分接口用 0–6 表示 D、C、B、A、S、SS、SSS；不截断高档位。 */
function gradeFromScore(value) {
  if (!Number.isFinite(Number(value))) return null;
  return ["D", "C", "B", "A", "S", "SS", "SSS"][
    Math.max(0, Math.min(6, Number(value)))
  ];
}

/** 将属性接口统一成图鉴只读的满级面板；SP/UR 的 hp 字段名是 maxHp，必须显式兼容。 */
function normalizePanel(data, fallback) {
  const panel = {
    attack: Number(data.attack ?? fallback.attack ?? 0),
    hp: Number(data.hp ?? data.maxHp ?? fallback.hp ?? 0),
    defense: Number(data.defense ?? fallback.defense ?? 0),
    speed: Number(data.speed ?? fallback.speed ?? 0),
    critRate: Number(data.critRate ?? fallback.critRate ?? 0),
    critDamage: Number(data.critPower ?? fallback.critDamage ?? 0.5),
    grades: {
      attack: gradeFromScore(data.score?.attack),
      hp: gradeFromScore(data.score?.maxHp),
      defense: gradeFromScore(data.score?.defense),
      speed: gradeFromScore(data.score?.speed),
      critRate: gradeFromScore(data.score?.critRate),
    },
  };
  return panel;
}

/** 官方接口偶发返回空响应；短暂重试后仍为空才回退旧值，并由后续校验阻止 0 面板入包。 */
async function fetchData(url) {
  let lastError;
  for (let attempt = 0; attempt < 3; attempt += 1) {
    try {
      const response = await fetch(url);
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      const payload = await response.json();
      if (payload.success && payload.data && Object.keys(payload.data).length > 0)
        return payload.data;
      throw new Error("官方接口返回空数据");
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 250 * (attempt + 1)));
    }
  }
  throw lastError;
}

/** 以固定并发量刷新所有条目的官方面板，避免一次性请求压垮资料接口。 */
async function mapLimit(items, limit, worker) {
  const result = new Array(items.length);
  let cursor = 0;
  async function consume() {
    while (cursor < items.length) {
      const index = cursor;
      cursor += 1;
      result[index] = await worker(items[index], index);
    }
  }
  await Promise.all(Array.from({ length: limit }, consume));
  return result;
}

/** 将官方技能接口转换成现有离线包的紧凑结构；技能不再展示，但保留字段兼容旧目录快照。 */
function normalizeUrSkills(data, id) {
  return Object.entries(data)
    .filter(([key, value]) => /^\d+$/.test(key) && value && typeof value === "object")
    .sort(([left], [right]) => Number(left) - Number(right))
    .map(([skillId, skill]) => ({
      skillId,
      name: skill.name || `技能${skillId.slice(-1)}`,
      type: skillId.endsWith("1") ? "普攻" : "主动",
      energyCost: Number(skill.consume_val ?? 0),
      baseEffect: skill.normaldesc || skill.normal_simplify_desc || "技能效果待核验",
      upgrades: Array.from(
        { length: 4 },
        (_, index) => skill.desc?.[index] ?? skill.simplify_desc?.[index] ?? null,
      ),
    }));
}

const packageData = JSON.parse(fs.readFileSync(catalogPath, "utf8"));
const existingById = new Map(
  packageData.entries.map((entry) => [entry.shikigamiId, entry]),
);
// 素材达摩没有官方培养面板，刷新时跳过远程面板接口，保留它们的目录资料条目。
const refreshableEntries = [...existingById.values()].filter(
  (entry) => entry.rarity !== "MATERIAL" && entry.rarity !== "素材",
);
const ids = refreshableEntries.map((entry) => entry.shikigamiId);
const refreshed = await mapLimit(ids, 8, async (id, index) => {
  const existing = existingById.get(id);
  const awakeData = await fetchData(attrUrl(id, 1)).catch(() => null);
  const baseData = awakeData ?? (await fetchData(attrUrl(id, 0)));
  const panel = normalizePanel(baseData, existing.panel);
  if (
    [panel.attack, panel.hp, panel.defense, panel.speed].some((value) => value <= 0)
  ) {
    throw new Error(`${existing.name}(${id}) 面板存在非正数：${JSON.stringify(panel)}`);
  }
  if ((index + 1) % 20 === 0) console.log(`已刷新 ${index + 1}/${ids.length} 条属性`);
  return { ...existing, panel };
});

const urId = "593";
const urAttr = await fetchData(attrUrl(urId, 0));
const urSkills = normalizeUrSkills(await fetchData(skillUrl(urId)), urId);
// 目录升级可能已经包含旧版 UR；先按稳定 ID 移除旧条目，再写入最新官方面板。
const urIndex = refreshed.findIndex((entry) => entry.shikigamiId === urId);
if (urIndex >= 0) refreshed.splice(urIndex, 1);
refreshed.push({
  shikigamiId: urId,
  name: "妖刀姬·绯夜猎刃",
  rarity: "UR",
  sortOrder: 0,
  iconAssetId: "pic_ss_ur_yaodaoji",
  panelLevel: 40,
  awakened: false,
  panel: normalizePanel(urAttr, {}),
  skills: urSkills,
  skillInvestment: {
    targetLevels: [1, 5, 5],
    priority: [1, 2, 3],
    blackEggsFrom111: 8,
    note: "资料页仅保留技能等级目标编码。",
  },
});

// 面板刷新只改可培养式神；素材条目原样拼回末尾，避免刷新脚本把达摩从图鉴中删掉。
const materialEntries = [...existingById.values()]
  .filter((entry) => entry.rarity === "MATERIAL" || entry.rarity === "素材")
  .map((entry) => ({ ...entry, rarity: "素材" }));
packageData.version = "builtin-2026.08.8";
packageData.expectedCount = refreshed.length + materialEntries.length;
packageData.entries = [...refreshed, ...materialEntries];
fs.writeFileSync(catalogPath, `${JSON.stringify(packageData, null, 2)}\n`, "utf8");
console.log(`完成：${refreshed.length} 条式神，已保留 SS/SSS 属性评级。`);
