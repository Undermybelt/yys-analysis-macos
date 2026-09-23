import type { MySoul, SoulSet } from "./api/contracts";

/** PVE 输出矩阵中的单元格结果；每个首领御魂组合完成后立即发送给界面。 */
export type PveOutputCell = {
  planId: string;
  bossId: string;
  value: number;
  selection: MySoul[];
};

/** PVE 其他方案的单项结果；方案完成后立即发送给界面。 */
export type PveOtherItem = {
  planId: string;
  value: number;
  selection: MySoul[] | null;
};

/** 后台计算的增量进度；item 只携带当前完成项，不发送候选搜索中的中间状态。 */
export type PveCalculationProgress = {
  completed: number;
  total: number;
  message: string;
  /** 当前组合经过号位、套装和主属性筛选后实际进入搜索的候选数量。 */
  candidateCount: number;
  /** 当前组合从开始筛选到得出最优结果的耗时，单位为毫秒。 */
  durationMs: number;
  item: PveOutputCell | PveOtherItem;
};

/** 一个组合搜索的结果和诊断信息；候选数量不再误用整批库存数量。 */
type CombinationSearchResult = {
  selection: MySoul[] | null;
  candidateCount: number;
};

/** 精确组合搜索的三种策略；基准组沿用现有 Pareto 动态规划。 */
export type PveExactPairSearchStrategy =
  "branch-and-bound" | "meet-in-the-middle" | "pareto";

/** 单个精确组合的可比基准结果，供单测和后续性能回归使用。 */
export type PveExactPairBenchmarkResult = {
  strategy: PveExactPairSearchStrategy;
  candidateCount: number;
  durationMs: number;
  value: number;
  selection: MySoul[];
};

// 生产计算固定使用基准中最快的 Meet-in-the-middle；另外两种策略仅用于对比和回归。
export const PVE_PRODUCTION_EXACT_PAIR_SEARCH_STRATEGY: PveExactPairSearchStrategy =
  "meet-in-the-middle";

/** PVE 计算最终结果；结果结构变化通过版本号校验，避免旧缓存误读。 */
export type PveCalculationResult = {
  version: 28;
  inventoryCount: number;
  calculatedAt: string;
  outputValues: Record<string, [number, number, number, number]>;
  otherValues: Record<string, number>;
  outputSelections: Record<string, Record<string, MySoul[]>>;
  otherSelections: Record<string, MySoul[]>;
  setIconIds: Record<string, number>;
};

type OtherMetric = "damage" | "percent" | "attack" | "defense";

type PanelValues = {
  attack: number;
  defense: number;
  speed: number;
  critRate: number;
  critDamage: number;
};

type ShikigamiBase = Pick<
  PanelValues,
  "attack" | "defense" | "speed" | "critRate" | "critDamage"
>;

type SoulContribution = {
  attackFlat: number;
  attackRate: number;
  hpRate: number;
  defenseFlat: number;
  defenseRate: number;
  speed: number;
  critRate: number;
  critDamage: number;
  effectHit: number;
  effectResist: number;
};

type BuildRule = {
  requiredSet?: string[];
  exactPair?: { ordinary: string[]; boss: string[] };
  fullCrit?: boolean;
  scattered?: boolean;
  pairStructure?: { pairCount: number };
  considerSetBonuses?: boolean;
  mainAttributes?: Partial<Record<2 | 4 | 6, string | string[]>>;
};

type ExactPairState = SoulContributionForSearch & {
  selected: MySoul[];
};

type SoulContributionForSearch = Pick<
  SoulContribution,
  "attackFlat" | "attackRate" | "critDamage" | "critRate"
>;

type RequiredSetState = ExactPairState & {
  requiredCount: number;
  secondarySetState: string;
};

type OutputSpec = {
  id: string;
  setId: string;
};

type OtherSpec = {
  id: string;
  metric: OtherMetric;
  shikigamiId: number;
  rule: BuildRule;
};

// 正式目录名称是“片叶之苇”，页面沿用玩家常叫的“片叶”，计算时兼容两种名称。
const SOUL_SET_ALIASES: Record<string, string[]> = {
  片叶: ["片叶", "片叶之苇"],
};

// PVE 输出矩阵使用固定的十三个输出套装；顺序必须与页面矩阵和增量进度一致。
const OUTPUT_SPECS: OutputSpec[] = [
  { id: "狂骨", setId: "狂骨" },
  { id: "隐念", setId: "隐念" },
  { id: "片叶", setId: "片叶" },
  { id: "破势", setId: "破势" },
  { id: "心眼", setId: "心眼" },
  { id: "针女", setId: "针女" },
  { id: "兵主部", setId: "兵主部" },
  { id: "网切", setId: "网切" },
  { id: "伤魂鸟", setId: "伤魂鸟" },
  { id: "镇墓兽", setId: "镇墓兽" },
  { id: "无刀取", setId: "无刀取" },
  { id: "恶楼", setId: "恶楼" },
  { id: "海月火玉", setId: "海月火玉" },
];

// 特殊两件套名称来自目录和页面表头；每个输出套装都要计算这四列。
const BOSS_SPECS = ["土蜘蛛", "荒骷髅", "鬼灵歌伎", "天羽羽斩"];

// “其他”卡片的规则集中在计算引擎中，避免页面线程再次维护一份搜索规则。
const OTHER_SPECS: OtherSpec[] = [
  {
    id: "inaba-scattered",
    metric: "percent",
    shikigamiId: 372,
    rule: { considerSetBonuses: true, mainAttributes: { 6: "crit_damage" } },
  },
  {
    id: "inaba-fire",
    metric: "percent",
    shikigamiId: 372,
    rule: { requiredSet: ["火灵"], mainAttributes: { 6: "crit_damage" } },
  },
  {
    id: "miketsu-mountain",
    metric: "defense",
    shikigamiId: 379,
    rule: {
      pairStructure: { pairCount: 3 },
      mainAttributes: { 2: "defense_rate", 4: "defense_rate", 6: "defense_rate" },
    },
  },
  {
    id: "sp-yuan-fire",
    metric: "percent",
    shikigamiId: 554,
    rule: {
      requiredSet: ["火灵"],
      fullCrit: true,
      mainAttributes: { 2: "speed" },
    },
  },
  {
    id: "sp-yuan-reminiscence",
    metric: "percent",
    shikigamiId: 554,
    rule: {
      requiredSet: ["遗念火"],
      fullCrit: true,
      mainAttributes: { 2: "speed" },
    },
  },
  {
    id: "sijin-attack",
    metric: "attack",
    shikigamiId: 601,
    rule: {
      exactPair: { ordinary: ["隐念"], boss: ["鬼灵歌伎"] },
      mainAttributes: { 2: "attack_rate", 4: "attack_rate", 6: "attack_rate" },
    },
  },
];

// 计算内部使用稳定英文 ID，进度提示必须转换为玩家能直接识别的中文方案名称。
const OTHER_DISPLAY_NAMES: Record<string, string> = {
  "inaba-scattered": "因幡辉夜姬 · 散件",
  "inaba-fire": "因幡辉夜姬 · 火灵",
  "miketsu-mountain": "不见岳 · 防御 2+2+2",
  "sp-yuan-fire": "纺愿缘结神 · 火灵",
  "sp-yuan-reminiscence": "纺愿缘结神 · 遗念火",
  "sijin-attack": "思金神 · 隐念 + 歌姬",
};

// 须佐之男的暴伤已包含游戏面板基础 100%，保持旧版结果口径不变。
const MAIN_SHIKIGAMI_BASE: ShikigamiBase = {
  attack: 4154,
  defense: 493.92,
  speed: 112,
  critRate: 0.12,
  critDamage: 1.5,
};

// “其他”卡片按所属式神的满级基础面板计算，不能复用须佐之男面板。
const SHIKIGAMI_BASES: Record<number, ShikigamiBase> = {
  372: {
    attack: 2949.07,
    defense: 396.9,
    speed: 113,
    critRate: 0.08,
    critDamage: 1.65,
  },
  379: {
    attack: 2492.4,
    defense: 538.02,
    speed: 115,
    critRate: 0.08,
    critDamage: 1.5,
  },
  554: {
    attack: 2224.4,
    defense: 432.18,
    speed: 108,
    critRate: 0.1,
    critDamage: 1.5,
  },
  601: {
    attack: 2144,
    defense: 533.61,
    speed: 110,
    critRate: 0.08,
    critDamage: 1.5,
  },
};

type SoulSetBonus = {
  attribute: keyof SoulContribution;
  value: number;
};

// 普通御魂的两件套类别映射；首领/星痕御魂不会进入普通两件套计算。
const ORDINARY_TWO_PIECE_ATTRIBUTES: Record<string, keyof SoulContribution> = {
  攻击加成: "attackRate",
  生命加成: "hpRate",
  防御加成: "defenseRate",
  暴击: "critRate",
  暴击伤害: "critDamage",
  效果命中: "effectHit",
  效果抵抗: "effectResist",
};

// 目录缺少效果文本时使用游戏普通两件套的稳定兜底值。
const ORDINARY_TWO_PIECE_DEFAULT_VALUES: Record<string, number> = {
  攻击加成: 0.15,
  生命加成: 0.15,
  防御加成: 0.3,
  暴击: 0.15,
  暴击伤害: 0.2,
  效果命中: 0.15,
  效果抵抗: 0.15,
};

// 单次计算上下文只在 Worker 内使用；预处理后所有搜索共享贡献和按号位索引。
let activeSoulSetBonuses = new Map<string, SoulSetBonus>();
let activeSoulSetIcons = new Map<string, number>();
let activeSoulContributions = new Map<string, SoulContribution>();
let activeSoulsBySlot = new Map<number, MySoul[]>();
let activeSoulsBySlotAndSet = new Map<string, MySoul[]>();
// 输出矩阵会重复使用同一号位、套装和主属性条件；缓存剪枝结果，避免 30 个组合反复构造候选数组。
let activeExactPairCandidates = new Map<string, MySoul[]>();

/** 判断是否为不参与普通两件套面板的首领/星痕御魂。 */
function isIndependentSoulSet(set: SoulSet): boolean {
  return (
    set.specialCategory === "首领御魂" ||
    set.specialCategory === "星痕御魂" ||
    set.category === "首领御魂" ||
    set.category === "星痕御魂"
  );
}

/** 读取目录中的两件套效果文本；目录异常时回退到类别默认值。 */
function parseSoulSetBonus(set: SoulSet): SoulSetBonus | undefined {
  if (isIndependentSoulSet(set)) return undefined;
  const attributeType = set.category
    ? ORDINARY_TWO_PIECE_ATTRIBUTES[set.category]
    : undefined;
  if (!attributeType) return undefined;
  let effectText = set.twoPieceEffect?.trim() ?? "";
  try {
    const parsed = JSON.parse(effectText) as unknown;
    if (typeof parsed === "string") effectText = parsed;
  } catch {
    // 目录接口也可能直接返回普通文本，此时继续使用正则解析。
  }
  const percentMatch = effectText.match(/([0-9]+(?:\.[0-9]+)?)%/);
  if (percentMatch?.[1]) {
    const percent = Number(percentMatch[1]);
    if (Number.isFinite(percent))
      return { attribute: attributeType, value: percent / 100 };
  }
  const fallbackValue = ORDINARY_TWO_PIECE_DEFAULT_VALUES[set.category ?? ""];
  return fallbackValue === undefined
    ? undefined
    : { attribute: attributeType, value: fallbackValue };
}

/** 构建目录规则、按号位索引和单枚御魂贡献，避免每个方案重复扫描同一批数据。 */
function configureCalculationContext(souls: MySoul[], sets: SoulSet[]): void {
  activeSoulSetBonuses = new Map();
  activeSoulSetIcons = new Map();
  activeSoulContributions = new Map();
  activeSoulsBySlot = new Map();
  activeSoulsBySlotAndSet = new Map();
  activeExactPairCandidates = new Map();

  for (const set of sets) {
    if (set.iconAssetId !== null) activeSoulSetIcons.set(set.setId, set.iconAssetId);
    const bonus = parseSoulSetBonus(set);
    if (!bonus) continue;
    activeSoulSetBonuses.set(set.setId, bonus);
    for (const [alias, names] of Object.entries(SOUL_SET_ALIASES)) {
      if (names.includes(set.setId)) {
        activeSoulSetBonuses.set(alias, bonus);
        if (set.iconAssetId !== null) activeSoulSetIcons.set(alias, set.iconAssetId);
      }
    }
  }

  for (const soul of souls) {
    const contribution: SoulContribution = {
      attackFlat: 0,
      attackRate: 0,
      hpRate: 0,
      defenseFlat: 0,
      defenseRate: 0,
      speed: 0,
      critRate: 0,
      critDamage: 0,
      effectHit: 0,
      effectResist: 0,
    };
    addSoulValue(contribution, soul.mainAttrType, soul.mainAttrValue);
    for (const attribute of soul.attributes) {
      addSoulValue(contribution, attribute.attributeType, attribute.value);
    }
    activeSoulContributions.set(soul.soulKey, contribution);
    const slotSouls = activeSoulsBySlot.get(soul.slot) ?? [];
    slotSouls.push(soul);
    activeSoulsBySlot.set(soul.slot, slotSouls);
    const setNames = new Set([
      soul.setId,
      ...Object.entries(SOUL_SET_ALIASES)
        .filter(([, names]) => names.includes(soul.setId))
        .map(([alias]) => alias),
    ]);
    for (const setName of setNames) {
      const setSouls = activeSoulsBySlotAndSet.get(`${soul.slot}:${setName}`) ?? [];
      setSouls.push(soul);
      activeSoulsBySlotAndSet.set(`${soul.slot}:${setName}`, setSouls);
    }
  }
}

/** 将一个主属性或副属性累加到有限的面板贡献结构中。 */
function addSoulValue(
  contribution: SoulContribution,
  attributeType: string,
  value: number,
): void {
  switch (attributeType) {
    case "attack_flat":
      contribution.attackFlat += value;
      break;
    case "attack_rate":
      contribution.attackRate += value;
      break;
    case "hp_rate":
      contribution.hpRate += value;
      break;
    case "defense_flat":
      contribution.defenseFlat += value;
      break;
    case "defense_rate":
      contribution.defenseRate += value;
      break;
    case "speed":
      contribution.speed += value;
      break;
    case "crit_rate":
      contribution.critRate += value;
      break;
    case "crit_damage":
      contribution.critDamage += value;
      break;
    case "effect_hit":
      contribution.effectHit += value;
      break;
    case "effect_resist":
      contribution.effectResist += value;
      break;
  }
}

/** 读取预处理贡献；测试或异常输入缺少缓存时仍能按单枚御魂即时回退计算。 */
function contributionOf(soul: MySoul): SoulContribution {
  const cached = activeSoulContributions.get(soul.soulKey);
  if (cached) return cached;
  const contribution: SoulContribution = {
    attackFlat: 0,
    attackRate: 0,
    hpRate: 0,
    defenseFlat: 0,
    defenseRate: 0,
    speed: 0,
    critRate: 0,
    critDamage: 0,
    effectHit: 0,
    effectResist: 0,
  };
  addSoulValue(contribution, soul.mainAttrType, soul.mainAttrValue);
  for (const attribute of soul.attributes) {
    addSoulValue(contribution, attribute.attributeType, attribute.value);
  }
  return contribution;
}

/** 计算六枚御魂的静态面板；普通两件套属性只激活一次。 */
function calculatePanel(
  selected: MySoul[],
  base: ShikigamiBase = MAIN_SHIKIGAMI_BASE,
): PanelValues {
  const contribution: SoulContribution = {
    attackFlat: 0,
    attackRate: 0,
    hpRate: 0,
    defenseFlat: 0,
    defenseRate: 0,
    speed: 0,
    critRate: 0,
    critDamage: 0,
    effectHit: 0,
    effectResist: 0,
  };
  const setCounts = new Map<string, number>();

  for (const soul of selected) {
    const soulContribution = contributionOf(soul);
    contribution.attackFlat += soulContribution.attackFlat;
    contribution.attackRate += soulContribution.attackRate;
    contribution.hpRate += soulContribution.hpRate;
    contribution.defenseFlat += soulContribution.defenseFlat;
    contribution.defenseRate += soulContribution.defenseRate;
    contribution.speed += soulContribution.speed;
    contribution.critRate += soulContribution.critRate;
    contribution.critDamage += soulContribution.critDamage;
    contribution.effectHit += soulContribution.effectHit;
    contribution.effectResist += soulContribution.effectResist;
    setCounts.set(soul.setId, (setCounts.get(soul.setId) ?? 0) + 1);
  }

  for (const [setId, count] of setCounts) {
    if (count < 2) continue;
    const bonus = activeSoulSetBonuses.get(setId);
    if (bonus) contribution[bonus.attribute] += bonus.value;
  }

  return {
    attack: base.attack * (1 + contribution.attackRate) + contribution.attackFlat,
    defense: base.defense * (1 + contribution.defenseRate) + contribution.defenseFlat,
    speed: base.speed + contribution.speed,
    critRate: base.critRate + contribution.critRate,
    critDamage: base.critDamage + contribution.critDamage,
  };
}

/** 判断组合是否满足满暴和 2/4/6 号位主属性约束。 */
function matchesConstraints(
  selected: MySoul[],
  panel: PanelValues,
  rule: BuildRule,
): boolean {
  if (rule.fullCrit && panel.critRate < 1 - 1e-9) return false;
  for (const [slot, mainAttribute] of Object.entries(rule.mainAttributes ?? {})) {
    const soul = selected.find((item) => item.slot === Number(slot));
    const allowedAttributes = Array.isArray(mainAttribute)
      ? mainAttribute
      : [mainAttribute];
    if (!soul || !allowedAttributes.includes(soul.mainAttrType)) return false;
  }
  return true;
}

/** 统计正式目录名与页面简称对应的套装件数。 */
function countSet(setCounts: Map<string, number>, setId: string): number {
  return (SOUL_SET_ALIASES[setId] ?? [setId]).reduce(
    (total, name) => total + (setCounts.get(name) ?? 0),
    0,
  );
}

/** 判断御魂是否命中指定套装，兼容片叶简称。 */
function soulMatchesSet(soul: MySoul, setId: string): boolean {
  return (SOUL_SET_ALIASES[setId] ?? [setId]).includes(soul.setId);
}

/** 按目标属性给单枚御魂预排序；成对收益只作为候选排序提示，不改变最终面板。 */
function candidateScore(
  soul: MySoul,
  metric: OtherMetric | "damage",
  base: ShikigamiBase = MAIN_SHIKIGAMI_BASE,
  includePairBonus = false,
): number {
  const contribution = contributionOf(soul);
  let attack = contribution.attackFlat + contribution.attackRate * base.attack;
  let defense = contribution.defenseFlat + contribution.defenseRate * base.defense;
  let critDamage = contribution.critDamage;
  let critRate = contribution.critRate;
  if (includePairBonus) {
    const setBonus = activeSoulSetBonuses.get(soul.setId);
    if (setBonus?.attribute === "attackRate")
      attack += (setBonus.value * base.attack) / 2;
    if (setBonus?.attribute === "defenseRate")
      defense += (setBonus.value * base.defense) / 2;
    if (setBonus?.attribute === "critDamage") critDamage += setBonus.value / 2;
    if (setBonus?.attribute === "critRate") critRate += setBonus.value / 2;
  }
  if (metric === "defense") return defense;
  if (metric === "attack") return attack;
  if (metric === "percent") return critDamage * 3 + critRate;
  return attack * 0.01 + critDamage * 3 + critRate;
}

/** 提取搜索状态需要的四个贡献维度；贡献已在计算上下文中预先缓存。 */
function exactPairContribution(soul: MySoul): SoulContributionForSearch {
  const contribution = contributionOf(soul);
  return {
    attackFlat: contribution.attackFlat,
    attackRate: contribution.attackRate,
    critDamage: contribution.critDamage,
    critRate: contribution.critRate,
  };
}

/** 删除同一位置、同一套装下被完全支配的御魂。 */
function pruneExactPairCandidates(souls: MySoul[]): MySoul[] {
  // 属性贡献完全相同的御魂对最终面板没有区别，只保留稳定排序中的第一枚，
  // 避免后续四维两两支配比较重复处理同一组数值。
  const uniqueByContribution = new Map<
    string,
    { soul: MySoul; contribution: SoulContributionForSearch }
  >();
  for (const soul of souls) {
    const contribution = exactPairContribution(soul);
    const key = [
      contribution.attackFlat,
      contribution.attackRate,
      contribution.critDamage,
      contribution.critRate,
    ].join(":");
    if (!uniqueByContribution.has(key)) {
      uniqueByContribution.set(key, { soul, contribution });
    }
  }
  const candidates = [...uniqueByContribution.values()];
  return candidates
    .filter((candidate, candidateIndex) => {
      const current = candidate.contribution;
      return !candidates.some((other, otherIndex) => {
        if (candidateIndex === otherIndex) return false;
        const replacement = other.contribution;
        const noWorse =
          replacement.attackFlat >= current.attackFlat &&
          replacement.attackRate >= current.attackRate &&
          replacement.critDamage >= current.critDamage &&
          replacement.critRate >= current.critRate;
        const strictlyBetter =
          replacement.attackFlat > current.attackFlat ||
          replacement.attackRate > current.attackRate ||
          replacement.critDamage > current.critDamage ||
          replacement.critRate > current.critRate;
        return noWorse && strictlyBetter;
      });
    })
    .map(({ soul }) => soul);
}

/**
 * 删除被另一状态全面超过的候选状态，保留精确 Pareto 前沿。
 *
 * 伤害指标中的攻击平面是“基础攻击 × 攻击加成 + 攻击白字”，因此攻击白字和攻击加成
 * 不需要作为两个独立维度保存；合并成最终攻击值后，前沿维度从四维降为攻击、暴伤、暴击。
 * 满暴约束开启时保留暴击维度，否则暴击率不会影响伤害排序。
 */
function pruneExactPairStates(
  states: ExactPairState[],
  metric: OtherMetric | "damage",
  base: ShikigamiBase,
  requireFullCrit: boolean,
): ExactPairState[] {
  const dimensions = (state: ExactPairState): number[] => {
    const attack = base.attack * state.attackRate + state.attackFlat;
    if (metric === "percent") {
      return requireFullCrit ? [state.critDamage, state.critRate] : [state.critDamage];
    }
    if (metric === "attack")
      return requireFullCrit ? [attack, state.critRate] : [attack];
    if (metric === "damage") {
      return requireFullCrit
        ? [attack, state.critDamage, state.critRate]
        : [attack, state.critDamage];
    }
    return [state.attackFlat, state.attackRate, state.critDamage, state.critRate];
  };
  const frontier: ExactPairState[] = [];
  for (const state of states) {
    const stateDimensions = dimensions(state);
    if (
      frontier.some((other) => {
        const otherDimensions = dimensions(other);
        return otherDimensions.every(
          (value, index) =>
            value >= (stateDimensions[index] ?? Number.NEGATIVE_INFINITY),
        );
      })
    ) {
      continue;
    }
    for (let index = frontier.length - 1; index >= 0; index -= 1) {
      const other = frontier[index];
      const otherDimensions = other ? dimensions(other) : [];
      if (
        other &&
        stateDimensions.every(
          (value, dimensionIndex) =>
            value >= (otherDimensions[dimensionIndex] ?? Number.POSITIVE_INFINITY),
        )
      ) {
        frontier.splice(index, 1);
      }
    }
    frontier.push(state);
  }
  return frontier;
}

/** 读取指定号位候选；按号位索引避免每个方案重复遍历全部库存。 */
function soulsForSlot(slot: number): MySoul[] {
  return activeSoulsBySlot.get(slot) ?? [];
}

/** 按号位和套装读取候选；片叶简称会在上下文建立时绑定到正式目录候选。 */
function soulsForSlotAndSet(slot: number, setId: string): MySoul[] {
  return (SOUL_SET_ALIASES[setId] ?? [setId]).flatMap(
    (name) => activeSoulsBySlotAndSet.get(`${slot}:${name}`) ?? [],
  );
}

/** 统计各号位最终进入组合搜索的去重候选，避免把整批库存数量写进组合日志。 */
function countCandidateSouls(candidatesBySlot: MySoul[][]): number {
  return new Set(
    candidatesBySlot.flatMap((candidates) => candidates.map((soul) => soul.soulKey)),
  ).size;
}

/** 缓存单号位、单套装和主属性条件下的精确候选，跨首领/输出组合复用 Pareto 剪枝结果。 */
function exactPairCandidatesForSet(
  slot: number,
  setId: string,
  requiredMainAttribute: string | string[] | undefined,
): MySoul[] {
  const mainAttributeKey = Array.isArray(requiredMainAttribute)
    ? requiredMainAttribute.join("|")
    : (requiredMainAttribute ?? "*");
  const cacheKey = `${slot}:${setId}:${mainAttributeKey}`;
  const cached = activeExactPairCandidates.get(cacheKey);
  if (cached) return cached;
  const allowedAttributes = Array.isArray(requiredMainAttribute)
    ? requiredMainAttribute
    : [requiredMainAttribute];
  const filtered = soulsForSlotAndSet(slot, setId).filter(
    (soul) => !requiredMainAttribute || allowedAttributes.includes(soul.mainAttrType),
  );
  const pruned = pruneExactPairCandidates(filtered);
  activeExactPairCandidates.set(cacheKey, pruned);
  return pruned;
}

/** 构建指定四件套和首领两件套的六个号位候选。 */
function buildExactPairCandidates(
  rule: BuildRule,
  metric: OtherMetric | "damage",
  base: ShikigamiBase,
): MySoul[][] {
  if (!rule.exactPair) return [];
  const ordinarySets = rule.exactPair.ordinary;
  const bossSets = rule.exactPair.boss;
  return Array.from({ length: 6 }, (_, index) => {
    const requiredMainAttribute = rule.mainAttributes?.[(index + 1) as 2 | 4 | 6];
    const candidatesForSet = (setId: string): MySoul[] =>
      exactPairCandidatesForSet(index + 1, setId, requiredMainAttribute);
    const unique = (items: MySoul[]) =>
      items.filter(
        (soul, soulIndex, allSouls) =>
          allSouls.findIndex((candidate) => candidate.soulKey === soul.soulKey) ===
          soulIndex,
      );
    const ordinaryCandidates = unique(
      ordinarySets.flatMap((setId) =>
        pruneExactPairCandidates(candidatesForSet(setId)),
      ),
    ).sort(
      (left, right) =>
        candidateScore(right, metric, base) - candidateScore(left, metric, base),
    );
    const bossCandidates = unique(
      bossSets.flatMap((setId) => pruneExactPairCandidates(candidatesForSet(setId))),
    ).sort(
      (left, right) =>
        candidateScore(right, metric, base) - candidateScore(left, metric, base),
    );
    return [...ordinaryCandidates, ...bossCandidates];
  });
}

/** 统计进入精确组合前的原始候选；日志数量不因内部 Pareto 剪枝而变小。 */
function countExactPairInputCandidates(rule: BuildRule): number {
  if (!rule.exactPair) return 0;
  const candidateKeys = new Set<string>();
  for (let slot = 1; slot <= 6; slot += 1) {
    const requiredMainAttribute = rule.mainAttributes?.[slot as 2 | 4 | 6];
    const allowedAttributes = Array.isArray(requiredMainAttribute)
      ? requiredMainAttribute
      : [requiredMainAttribute];
    for (const setId of [...rule.exactPair.ordinary, ...rule.exactPair.boss]) {
      for (const soul of soulsForSlotAndSet(slot, setId)) {
        if (requiredMainAttribute && !allowedAttributes.includes(soul.mainAttrType)) {
          continue;
        }
        candidateKeys.add(soul.soulKey);
      }
    }
  }
  return candidateKeys.size;
}

/** 使用现有四维 Pareto 状态动态规划，作为正确性基准组。 */
function findBestExactPairCombinationPareto(
  rule: BuildRule,
  metric: OtherMetric | "damage",
  base: ShikigamiBase,
): CombinationSearchResult {
  if (!rule.exactPair) return { selection: null, candidateCount: 0 };
  const ordinarySets = rule.exactPair.ordinary;
  const candidatesBySlot = buildExactPairCandidates(rule, metric, base);
  const candidateCount = countExactPairInputCandidates(rule);

  let states = new Map<string, ExactPairState[]>([
    [
      "0:0",
      [{ attackFlat: 0, attackRate: 0, critDamage: 0, critRate: 0, selected: [] }],
    ],
  ]);
  for (let slotIndex = 0; slotIndex < candidatesBySlot.length; slotIndex += 1) {
    const slotCandidates = candidatesBySlot[slotIndex] ?? [];
    const availableSlots = candidatesBySlot.length - slotIndex;
    const remainingSlots = candidatesBySlot.length - slotIndex - 1;
    const nextStates = new Map<string, ExactPairState[]>();
    for (const [stateKey, currentStates] of states) {
      const [ordinaryCountText, bossCountText] = stateKey.split(":");
      const ordinaryCount = Number(ordinaryCountText);
      const bossCount = Number(bossCountText);
      // 当前计数即使把剩余号位全部补上也无法达到 4+2 时，直接丢弃整桶状态。
      if (
        ordinaryCount > 4 ||
        bossCount > 2 ||
        ordinaryCount + availableSlots < 4 ||
        bossCount + availableSlots < 2
      ) {
        continue;
      }
      for (const state of currentStates) {
        for (const soul of slotCandidates) {
          const isOrdinary = ordinarySets.some((setId) => soulMatchesSet(soul, setId));
          const nextOrdinaryCount = ordinaryCount + (isOrdinary ? 1 : 0);
          const nextBossCount = bossCount + (isOrdinary ? 0 : 1);
          if (nextOrdinaryCount > 4 || nextBossCount > 2) continue;
          // 本次选择后仍无法凑齐目标件数的分支，不进入下一轮状态集合。
          if (
            nextOrdinaryCount + remainingSlots < 4 ||
            nextBossCount + remainingSlots < 2
          ) {
            continue;
          }
          const contribution = exactPairContribution(soul);
          const nextKey = `${nextOrdinaryCount}:${nextBossCount}`;
          const next = nextStates.get(nextKey) ?? [];
          next.push({
            attackFlat: state.attackFlat + contribution.attackFlat,
            attackRate: state.attackRate + contribution.attackRate,
            critDamage: state.critDamage + contribution.critDamage,
            critRate: state.critRate + contribution.critRate,
            selected: [...state.selected, soul],
          });
          nextStates.set(nextKey, next);
        }
      }
    }
    states = new Map(
      [...nextStates.entries()].map(([key, currentStates]) => [
        key,
        pruneExactPairStates(currentStates, metric, base, rule.fullCrit ?? false),
      ]),
    );
  }

  let bestSelected: MySoul[] | null = null;
  let bestScore = -Infinity;
  for (const state of states.get("4:2") ?? []) {
    const panel = calculatePanel(state.selected, base);
    if (!matchesConstraints(state.selected, panel, rule)) continue;
    const score =
      metric === "attack"
        ? panel.attack
        : metric === "percent"
          ? panel.critDamage
          : panel.attack * panel.critDamage;
    if (score > bestScore) {
      bestScore = score;
      bestSelected = state.selected;
    }
  }
  return {
    selection: bestSelected,
    candidateCount,
  };
}

/** 合并两段搜索状态的四维贡献，避免重复解析御魂属性。 */
function addSearchContributions(
  left: SoulContributionForSearch,
  right: SoulContributionForSearch,
): SoulContributionForSearch {
  return {
    attackFlat: left.attackFlat + right.attackFlat,
    attackRate: left.attackRate + right.attackRate,
    critDamage: left.critDamage + right.critDamage,
    critRate: left.critRate + right.critRate,
  };
}

/** 返回一个没有任何御魂贡献的搜索状态，用于分支定界的递归起点。 */
function emptySearchContributions(): SoulContributionForSearch {
  return { attackFlat: 0, attackRate: 0, critDamage: 0, critRate: 0 };
}

/** 为每个后缀号位预计算四维最大贡献，作为分支定界的保守上界。 */
function buildExactPairSuffixMaximums(
  candidatesBySlot: MySoul[][],
): SoulContributionForSearch[] {
  const suffix = Array.from({ length: candidatesBySlot.length + 1 }, () =>
    emptySearchContributions(),
  );
  for (let slotIndex = candidatesBySlot.length - 1; slotIndex >= 0; slotIndex -= 1) {
    const slotMaximum = emptySearchContributions();
    for (const soul of candidatesBySlot[slotIndex] ?? []) {
      const contribution = exactPairContribution(soul);
      slotMaximum.attackFlat = Math.max(
        slotMaximum.attackFlat,
        contribution.attackFlat,
      );
      slotMaximum.attackRate = Math.max(
        slotMaximum.attackRate,
        contribution.attackRate,
      );
      slotMaximum.critDamage = Math.max(
        slotMaximum.critDamage,
        contribution.critDamage,
      );
      slotMaximum.critRate = Math.max(slotMaximum.critRate, contribution.critRate);
    }
    suffix[slotIndex] = addSearchContributions(suffix[slotIndex + 1]!, slotMaximum);
  }
  return suffix;
}

/** 计算普通套装两件套可能提供的最大正向贡献，确保上界不会低估最终面板。 */
function exactPairBonusUpperBound(
  rule: BuildRule,
  attribute: keyof SoulContributionForSearch,
): number {
  return Math.max(
    0,
    ...(rule.exactPair?.ordinary ?? []).map((setId) => {
      const bonus = activeSoulSetBonuses.get(setId);
      return bonus?.attribute === attribute ? bonus.value : 0;
    }),
  );
}

/** 使用当前贡献和剩余号位最大贡献计算攻击、暴伤目标的保守上界。 */
function exactPairUpperScore(
  current: SoulContributionForSearch,
  suffixMaximum: SoulContributionForSearch,
  rule: BuildRule,
  metric: OtherMetric | "damage",
  base: ShikigamiBase,
): number {
  if (metric === "defense") return Number.POSITIVE_INFINITY;
  const attackFlat = current.attackFlat + suffixMaximum.attackFlat;
  const attackRate =
    current.attackRate +
    suffixMaximum.attackRate +
    exactPairBonusUpperBound(rule, "attackRate");
  const critDamage =
    base.critDamage +
    current.critDamage +
    suffixMaximum.critDamage +
    exactPairBonusUpperBound(rule, "critDamage");
  const attack = base.attack * (1 + attackRate) + attackFlat;
  return metric === "attack"
    ? attack
    : metric === "percent"
      ? critDamage
      : attack * critDamage;
}

/** 判断满暴约束在剩余号位的最大贡献下是否仍有机会满足。 */
function exactPairCanReachFullCrit(
  current: SoulContributionForSearch,
  suffixMaximum: SoulContributionForSearch,
  rule: BuildRule,
  base: ShikigamiBase,
): boolean {
  if (!rule.fullCrit) return true;
  return (
    base.critRate +
      current.critRate +
      suffixMaximum.critRate +
      exactPairBonusUpperBound(rule, "critRate") >=
    1 - 1e-9
  );
}

/** 用安全上界递归搜索；只在分支不可能超过当前最优解时剪枝，不牺牲精确性。 */
function findBestExactPairCombinationBranchAndBound(
  rule: BuildRule,
  metric: OtherMetric | "damage",
  base: ShikigamiBase,
): CombinationSearchResult {
  if (!rule.exactPair) return { selection: null, candidateCount: 0 };
  const ordinarySets = rule.exactPair.ordinary;
  const candidatesBySlot = buildExactPairCandidates(rule, metric, base);
  const candidateCount = countExactPairInputCandidates(rule);
  const suffixMaximums = buildExactPairSuffixMaximums(candidatesBySlot);
  let bestSelected: MySoul[] | null = null;
  let bestScore = -Infinity;
  const selected: MySoul[] = [];

  function visit(
    slotIndex: number,
    ordinaryCount: number,
    bossCount: number,
    contribution: SoulContributionForSearch,
  ): void {
    const availableSlots = candidatesBySlot.length - slotIndex;
    if (
      ordinaryCount > 4 ||
      bossCount > 2 ||
      ordinaryCount + availableSlots < 4 ||
      bossCount + availableSlots < 2 ||
      !exactPairCanReachFullCrit(contribution, suffixMaximums[slotIndex]!, rule, base)
    ) {
      return;
    }
    if (
      bestSelected &&
      exactPairUpperScore(
        contribution,
        suffixMaximums[slotIndex]!,
        rule,
        metric,
        base,
      ) <=
        bestScore + 1e-9
    ) {
      return;
    }
    if (slotIndex === candidatesBySlot.length) {
      if (ordinaryCount !== 4 || bossCount !== 2) return;
      const panel = calculatePanel(selected, base);
      if (!matchesConstraints(selected, panel, rule)) return;
      const score =
        metric === "attack"
          ? panel.attack
          : metric === "percent"
            ? panel.critDamage
            : panel.attack * panel.critDamage;
      if (score > bestScore) {
        bestScore = score;
        bestSelected = [...selected];
      }
      return;
    }

    for (const soul of candidatesBySlot[slotIndex] ?? []) {
      const isOrdinary = ordinarySets.some((setId) => soulMatchesSet(soul, setId));
      const nextOrdinaryCount = ordinaryCount + (isOrdinary ? 1 : 0);
      const nextBossCount = bossCount + (isOrdinary ? 0 : 1);
      if (nextOrdinaryCount > 4 || nextBossCount > 2) continue;
      selected.push(soul);
      visit(
        slotIndex + 1,
        nextOrdinaryCount,
        nextBossCount,
        addSearchContributions(contribution, exactPairContribution(soul)),
      );
      selected.pop();
    }
  }

  visit(0, 0, 0, emptySearchContributions());
  return {
    selection: bestSelected,
    candidateCount,
  };
}

/**
 * 生成一半号位的部分状态；每加入一个号位就按配方件数和目标相关贡献做 Pareto 剪枝。
 *
 * 不能等三个号位全部笛卡尔展开后再剪枝，否则片叶+荒骷髅这类大库存会先生成数百万
 * 中间状态，Worker 虽然没有死循环，但主线程看起来会长时间卡住。
 */
function enumerateExactPairHalf(
  candidatesBySlot: MySoul[][],
  start: number,
  end: number,
  ordinarySets: string[],
  metric: OtherMetric | "damage",
  base: ShikigamiBase,
  requireFullCrit: boolean,
): Map<string, ExactPairState[]> {
  let statesByCount = new Map<string, ExactPairState[]>([
    [
      "0:0",
      [
        {
          ...emptySearchContributions(),
          selected: [],
        },
      ],
    ],
  ]);

  for (let slotIndex = start; slotIndex < end; slotIndex += 1) {
    const nextStates = new Map<string, ExactPairState[]>();
    for (const [stateKey, currentStates] of statesByCount) {
      const [ordinaryCountText, bossCountText] = stateKey.split(":");
      const ordinaryCount = Number(ordinaryCountText);
      const bossCount = Number(bossCountText);
      for (const state of currentStates) {
        for (const soul of candidatesBySlot[slotIndex] ?? []) {
          // 正常库存不会跨号位重复，但这里保留稳定 ID 检查，避免异常快照重复使用同一枚御魂。
          if (state.selected.some((item) => item.soulKey === soul.soulKey)) continue;
          const isOrdinary = ordinarySets.some((setId) => soulMatchesSet(soul, setId));
          const nextOrdinaryCount = ordinaryCount + (isOrdinary ? 1 : 0);
          const nextBossCount = bossCount + (isOrdinary ? 0 : 1);
          if (nextOrdinaryCount > 4 || nextBossCount > 2) continue;
          const contribution = exactPairContribution(soul);
          const nextState: ExactPairState = {
            attackFlat: state.attackFlat + contribution.attackFlat,
            attackRate: state.attackRate + contribution.attackRate,
            critDamage: state.critDamage + contribution.critDamage,
            critRate: state.critRate + contribution.critRate,
            selected: [...state.selected, soul],
          };
          const key = `${nextOrdinaryCount}:${nextBossCount}`;
          const bucket = nextStates.get(key) ?? [];
          bucket.push(nextState);
          nextStates.set(key, bucket);
        }
      }
    }

    // 每一层都剪枝，把四维贡献相同且更差的状态挡在继续展开之前。
    statesByCount = new Map(
      [...nextStates.entries()].map(([key, states]) => [
        key,
        pruneExactPairStates(states, metric, base, requireFullCrit),
      ]),
    );
    if (statesByCount.size === 0) break;
  }
  return statesByCount;
}

/** 将两个三号位 Pareto 前沿按 4+2 件数配对，避免直接展开六重笛卡尔积。 */
function findBestExactPairCombinationMeetInMiddle(
  rule: BuildRule,
  metric: OtherMetric | "damage",
  base: ShikigamiBase,
): CombinationSearchResult {
  if (!rule.exactPair) return { selection: null, candidateCount: 0 };
  const candidatesBySlot = buildExactPairCandidates(rule, metric, base);
  const candidateCount = countExactPairInputCandidates(rule);
  const ordinarySets = rule.exactPair.ordinary;
  const left = enumerateExactPairHalf(
    candidatesBySlot,
    0,
    3,
    ordinarySets,
    metric,
    base,
    rule.fullCrit ?? false,
  );
  const right = enumerateExactPairHalf(
    candidatesBySlot,
    3,
    6,
    ordinarySets,
    metric,
    base,
    rule.fullCrit ?? false,
  );
  let bestSelected: MySoul[] | null = null;
  let bestScore = -Infinity;

  for (const [leftKey, leftStates] of left) {
    const [leftOrdinaryText, leftBossText] = leftKey.split(":");
    const rightKey = `${4 - Number(leftOrdinaryText)}:${2 - Number(leftBossText)}`;
    const rightStates = right.get(rightKey) ?? [];
    for (const leftState of leftStates) {
      for (const rightState of rightStates) {
        const selected = [...leftState.selected, ...rightState.selected];
        const panel = calculatePanel(selected, base);
        if (!matchesConstraints(selected, panel, rule)) continue;
        const score =
          metric === "attack"
            ? panel.attack
            : metric === "percent"
              ? panel.critDamage
              : panel.attack * panel.critDamage;
        if (score > bestScore) {
          bestScore = score;
          bestSelected = selected;
        }
      }
    }
  }
  return {
    selection: bestSelected,
    candidateCount,
  };
}

/** 根据策略名分派精确四件套+两件首领御魂搜索，保证三种算法共享同一候选和评分口径。 */
function findBestExactPairCombination(
  rule: BuildRule,
  metric: OtherMetric | "damage",
  base: ShikigamiBase,
  strategy: PveExactPairSearchStrategy = PVE_PRODUCTION_EXACT_PAIR_SEARCH_STRATEGY,
): CombinationSearchResult {
  if (strategy === "branch-and-bound") {
    return findBestExactPairCombinationBranchAndBound(rule, metric, base);
  }
  if (strategy === "meet-in-the-middle") {
    return findBestExactPairCombinationMeetInMiddle(rule, metric, base);
  }
  return findBestExactPairCombinationPareto(rule, metric, base);
}

/** 精确搜索指定四件套加任意两件，并按状态贡献做 Pareto 剪枝。 */
function findBestRequiredSetCombination(
  rule: BuildRule,
  metric: OtherMetric | "damage",
  base: ShikigamiBase,
): CombinationSearchResult {
  const requiredSet = rule.requiredSet?.[0];
  if (!requiredSet) return { selection: null, candidateCount: 0 };
  const candidatesBySlot = Array.from({ length: 6 }, (_, index) => {
    const slotSouls = soulsForSlot(index + 1).filter((soul) => {
      const requiredMainAttribute = rule.mainAttributes?.[(index + 1) as 2 | 4 | 6];
      const allowedAttributes = Array.isArray(requiredMainAttribute)
        ? requiredMainAttribute
        : [requiredMainAttribute];
      return !requiredMainAttribute || allowedAttributes.includes(soul.mainAttrType);
    });
    const soulsBySet = new Map<string, MySoul[]>();
    for (const soul of slotSouls) {
      const sameSet = soulsBySet.get(soul.setId) ?? [];
      sameSet.push(soul);
      soulsBySet.set(soul.setId, sameSet);
    }
    return [...soulsBySet.values()].flatMap((setSouls) =>
      pruneExactPairCandidates(setSouls),
    );
  });

  let states = new Map<string, RequiredSetState[]>([
    [
      "0:",
      [
        {
          attackFlat: 0,
          attackRate: 0,
          critDamage: 0,
          critRate: 0,
          selected: [],
          requiredCount: 0,
          secondarySetState: "",
        },
      ],
    ],
  ]);
  for (let slotIndex = 0; slotIndex < candidatesBySlot.length; slotIndex += 1) {
    const slotCandidates = candidatesBySlot[slotIndex] ?? [];
    const nextStates = new Map<string, RequiredSetState[]>();
    for (const currentStates of states.values()) {
      for (const state of currentStates) {
        for (const soul of slotCandidates) {
          if (state.selected.some((item) => item.soulKey === soul.soulKey)) continue;
          const isRequired = soulMatchesSet(soul, requiredSet);
          const requiredCount = state.requiredCount + (isRequired ? 1 : 0);
          let secondarySetState = state.secondarySetState;
          if (!isRequired) {
            if (secondarySetState === "") {
              secondarySetState = `one:${soul.setId}`;
            } else if (secondarySetState.startsWith("one:")) {
              const firstSetId = secondarySetState.slice("one:".length);
              // 同套两件会激活对应二件套属性，必须保留套装名，避免不同套装在 Pareto 剪枝时互相覆盖。
              secondarySetState =
                firstSetId === soul.setId ? `same:${soul.setId}` : "different";
            } else {
              continue;
            }
          }
          const remainingSlots = candidatesBySlot.length - slotIndex - 1;
          if (requiredCount > 4) continue;
          const secondaryCount =
            secondarySetState === "" ? 0 : secondarySetState.startsWith("one:") ? 1 : 2;
          if (requiredCount + remainingSlots < 4) continue;
          if (secondaryCount + remainingSlots < 2) continue;
          const contribution = exactPairContribution(soul);
          const nextState: RequiredSetState = {
            attackFlat: state.attackFlat + contribution.attackFlat,
            attackRate: state.attackRate + contribution.attackRate,
            critDamage: state.critDamage + contribution.critDamage,
            critRate: state.critRate + contribution.critRate,
            selected: [...state.selected, soul],
            requiredCount,
            secondarySetState,
          };
          const key = requiredCount + ":" + secondarySetState;
          const bucket = nextStates.get(key) ?? [];
          bucket.push(nextState);
          nextStates.set(key, bucket);
        }
      }
    }
    states = new Map(
      [...nextStates.entries()].map(([key, currentStates]) => [
        key,
        pruneRequiredSetStates(currentStates, metric),
      ]),
    );
  }

  let bestSelected: MySoul[] | null = null;
  let bestScore = -Infinity;
  for (const currentStates of states.values()) {
    for (const state of currentStates) {
      if (
        state.requiredCount !== 4 ||
        (state.secondarySetState !== "different" &&
          !state.secondarySetState.startsWith("same:"))
      ) {
        continue;
      }
      const panel = calculatePanel(state.selected, base);
      if (!matchesConstraints(state.selected, panel, rule)) continue;
      const score =
        metric === "attack"
          ? panel.attack
          : metric === "percent"
            ? panel.critDamage
            : panel.attack * panel.critDamage;
      if (score > bestScore) {
        bestScore = score;
        bestSelected = state.selected;
      }
    }
  }
  return {
    selection: bestSelected,
    candidateCount: countCandidateSouls(candidatesBySlot),
  };
}

/** 删除同一套装计数状态中被全面超过的组合。 */
function pruneRequiredSetStates(
  states: RequiredSetState[],
  metric: OtherMetric | "damage",
): RequiredSetState[] {
  if (metric === "percent") {
    const sorted = [...states].sort(
      (left, right) =>
        right.critDamage - left.critDamage || right.critRate - left.critRate,
    );
    const frontier: RequiredSetState[] = [];
    let highestCritRate = -Infinity;
    for (const state of sorted) {
      if (state.critRate <= highestCritRate) continue;
      frontier.push(state);
      highestCritRate = state.critRate;
    }
    return frontier;
  }
  const frontier: RequiredSetState[] = [];
  for (const state of states) {
    if (
      frontier.some(
        (other) =>
          other.attackFlat >= state.attackFlat &&
          other.attackRate >= state.attackRate &&
          other.critDamage >= state.critDamage &&
          other.critRate >= state.critRate,
      )
    ) {
      continue;
    }
    for (let index = frontier.length - 1; index >= 0; index -= 1) {
      const other = frontier[index];
      if (
        other &&
        state.attackFlat >= other.attackFlat &&
        state.attackRate >= other.attackRate &&
        state.critDamage >= other.critDamage &&
        state.critRate >= other.critRate
      ) {
        frontier.splice(index, 1);
      }
    }
    frontier.push(state);
  }
  return frontier;
}

/** 按固定套装配方枚举最优六件套；候选使用按号位索引和必要的前 8 截断。 */
function findBestCombination(
  rule: BuildRule,
  metric: OtherMetric | "damage",
  base: ShikigamiBase = MAIN_SHIKIGAMI_BASE,
  exactPairStrategy: PveExactPairSearchStrategy = PVE_PRODUCTION_EXACT_PAIR_SEARCH_STRATEGY,
): CombinationSearchResult {
  if (rule.requiredSet?.length === 1 && !rule.pairStructure && !rule.scattered) {
    return findBestRequiredSetCombination(rule, metric, base);
  }
  if (rule.exactPair) {
    return findBestExactPairCombination(rule, metric, base, exactPairStrategy);
  }

  const candidatesBySlot = Array.from({ length: 6 }, (_, index) => {
    const slotSouls = soulsForSlot(index + 1)
      .filter((soul) => {
        const requiredMainAttribute = rule.mainAttributes?.[(index + 1) as 2 | 4 | 6];
        const allowedAttributes = Array.isArray(requiredMainAttribute)
          ? requiredMainAttribute
          : [requiredMainAttribute];
        return !requiredMainAttribute || allowedAttributes.includes(soul.mainAttrType);
      })
      .sort(
        (left, right) =>
          candidateScore(
            right,
            metric,
            base,
            Boolean(rule.pairStructure || rule.considerSetBonuses),
          ) -
          candidateScore(
            left,
            metric,
            base,
            Boolean(rule.pairStructure || rule.considerSetBonuses),
          ),
      );
    const requiredSetCandidates = (rule.requiredSet ?? []).flatMap((setId) =>
      slotSouls.filter((soul) => soulMatchesSet(soul, setId)).slice(0, 12),
    );
    const candidateKeys = new Set<string>();
    return [...requiredSetCandidates, ...slotSouls.slice(0, 8)].filter((soul) => {
      if (candidateKeys.has(soul.soulKey)) return false;
      candidateKeys.add(soul.soulKey);
      return true;
    });
  });

  let bestSelected: MySoul[] | null = null;
  let bestScore = -Infinity;
  const selected: MySoul[] = [];

  function visit(slotIndex: number): void {
    if (slotIndex === 6) {
      const setCounts = new Map<string, number>();
      for (const soul of selected) {
        setCounts.set(soul.setId, (setCounts.get(soul.setId) ?? 0) + 1);
      }
      if (
        rule.requiredSet &&
        !rule.requiredSet.every((setId) => countSet(setCounts, setId) === 4)
      ) {
        return;
      }
      if (rule.scattered && [...setCounts.values()].some((count) => count > 1)) return;
      if (rule.pairStructure) {
        const pairCounts = [...setCounts.values()];
        if (
          pairCounts.length !== rule.pairStructure.pairCount ||
          pairCounts.some((count) => count !== 2)
        ) {
          return;
        }
      }
      const panel = calculatePanel(selected, base);
      if (!matchesConstraints(selected, panel, rule)) return;
      const score =
        metric === "defense"
          ? panel.defense
          : metric === "attack"
            ? panel.attack
            : metric === "percent"
              ? panel.critDamage
              : panel.attack * panel.critDamage;
      if (score > bestScore) {
        bestScore = score;
        bestSelected = [...selected];
      }
      return;
    }
    const slotCandidates = candidatesBySlot[slotIndex];
    if (!slotCandidates) return;
    for (const soul of slotCandidates) {
      if (selected.some((item) => item.soulKey === soul.soulKey)) continue;
      selected.push(soul);
      visit(slotIndex + 1);
      selected.pop();
    }
  }

  visit(0);
  return {
    selection: bestSelected,
    candidateCount: countCandidateSouls(candidatesBySlot),
  };
}

/** 查找输出矩阵某个套装与首领御魂的最佳组合。 */
function findOutputCombination(
  ordinarySet: string,
  bossSet: string,
  exactPairStrategy: PveExactPairSearchStrategy = PVE_PRODUCTION_EXACT_PAIR_SEARCH_STRATEGY,
): CombinationSearchResult {
  return findBestCombination(
    {
      exactPair: { ordinary: [ordinarySet], boss: [bossSet] },
      fullCrit: true,
      mainAttributes: {
        2: "attack_rate",
        4: "attack_rate",
        6: ["crit_damage", "crit_rate"],
      },
    },
    "damage",
    MAIN_SHIKIGAMI_BASE,
    exactPairStrategy,
  );
}

/** 查找“其他”方案的最佳组合。 */
function findOtherCombination(spec: OtherSpec): CombinationSearchResult {
  const base = SHIKIGAMI_BASES[spec.shikigamiId] ?? MAIN_SHIKIGAMI_BASE;
  return findBestCombination(spec.rule, spec.metric, base);
}

/**
 * 对指定御魂库存只计算“狂骨+土蜘蛛”一个组合，供单测和性能回归使用。
 * 每次调用都会重建计算上下文，避免前一个策略的候选缓存影响后一个策略的耗时。
 */
export function benchmarkPveExactPair(
  souls: MySoul[],
  catalogSets: SoulSet[],
  strategy: PveExactPairSearchStrategy,
): PveExactPairBenchmarkResult {
  configureCalculationContext(souls, catalogSets);
  const startedAt = performance.now();
  const search = findOutputCombination("狂骨", "土蜘蛛", strategy);
  const selected = search.selection ?? [];
  const panel = search.selection ? calculatePanel(search.selection) : null;
  const value = panel ? Math.round(panel.attack * panel.critDamage) : 0;
  return {
    strategy,
    candidateCount: search.candidateCount,
    durationMs: Math.max(0, Math.round(performance.now() - startedAt)),
    value,
    selection: selected,
  };
}

/**
 * 计算所有 PVE 方案；每个组合开始时通知 Worker 写入日志，每个输出单元完成后再通过进度回调交给前端。
 * 这样任务中心既能说明当前正在计算哪套组合，也能继续展示完成数量。
 */
export function calculatePvePanel(
  souls: MySoul[],
  catalogSets: SoulSet[],
  onProgress: (progress: PveCalculationProgress) => void,
  onCombinationStart: (message: string) => void = () => undefined,
): PveCalculationResult {
  configureCalculationContext(souls, catalogSets);
  const total = OUTPUT_SPECS.length * BOSS_SPECS.length + OTHER_SPECS.length;
  let completed = 0;
  const outputValues: Record<string, [number, number, number, number]> = {};
  const outputSelections: Record<string, Record<string, MySoul[]>> = {};
  const otherValues: Record<string, number> = {};
  const otherSelections: Record<string, MySoul[]> = {};

  for (const output of OUTPUT_SPECS) {
    const selections: Record<string, MySoul[]> = {};
    const values: number[] = [];
    for (const boss of BOSS_SPECS) {
      // 组合筛选和枚举都在 Worker 内完成；开始日志不再冒充当前组合的候选数量。
      const startedAt = performance.now();
      onCombinationStart(`开始计算 ${output.id} + ${boss} 组合`);
      const search = findOutputCombination(output.setId, boss);
      const selected = search.selection;
      const panel = selected ? calculatePanel(selected) : null;
      const value = panel ? Math.round(panel.attack * panel.critDamage) : 0;
      if (selected) selections[boss] = selected;
      values.push(value);
      // 计时覆盖候选筛选、组合枚举和最终面板计算，日志反映完整单项成本。
      const durationMs = Math.max(0, Math.round(performance.now() - startedAt));
      completed += 1;
      onProgress({
        completed,
        total,
        message: `${output.id} · ${boss} 已完成`,
        candidateCount: search.candidateCount,
        durationMs,
        item: { planId: output.id, bossId: boss, value, selection: selected ?? [] },
      });
    }
    outputValues[output.id] = [
      values[0] ?? 0,
      values[1] ?? 0,
      values[2] ?? 0,
      values[3] ?? 0,
    ];
    outputSelections[output.id] = selections;
  }

  for (const spec of OTHER_SPECS) {
    // “其他”方案没有首领列，直接使用面向玩家的方案名称作为组合日志标题。
    const displayName = OTHER_DISPLAY_NAMES[spec.id] ?? spec.id;
    const startedAt = performance.now();
    onCombinationStart(`开始计算 ${displayName} 组合`);
    const search = findOtherCombination(spec);
    const selected = search.selection;
    const base = SHIKIGAMI_BASES[spec.shikigamiId] ?? MAIN_SHIKIGAMI_BASE;
    let value = 0;
    if (selected) {
      const panel = calculatePanel(selected, base);
      value =
        spec.metric === "defense"
          ? Math.round(panel.defense)
          : spec.metric === "attack"
            ? Math.round(panel.attack)
            : Math.round(panel.critDamage * 100);
      otherSelections[spec.id] = selected;
    }
    otherValues[spec.id] = value;
    // “其他”方案同样把面板计算纳入单项耗时，便于和输出组合直接比较。
    const durationMs = Math.max(0, Math.round(performance.now() - startedAt));
    completed += 1;
    onProgress({
      completed,
      total,
      message: `${displayName} 已完成`,
      candidateCount: search.candidateCount,
      durationMs,
      item: { planId: spec.id, value, selection: selected },
    });
  }

  return {
    version: 28,
    inventoryCount: souls.length,
    calculatedAt: new Date().toISOString(),
    outputValues,
    otherValues,
    outputSelections,
    otherSelections,
    setIconIds: Object.fromEntries(activeSoulSetIcons),
  };
}
