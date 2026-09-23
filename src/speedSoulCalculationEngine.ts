import type { MySoul } from "./api/contracts";

// 一速排行只使用六星御魂；满级开关只控制是否排除未达到 +15 的记录。
const RANK_ROW_COUNT = 5;
const ELIGIBLE_QUALITY = 6;
const ELIGIBLE_LEVEL = 15;
const REQUIRED_SET_PIECES = 4;
const SLOT_ORDER = [1, 2, 3, 4, 5, 6] as const;
const RANK_LABELS = ["一", "二", "三", "四", "五"] as const;

// 兼容旧快照或旧适配器使用的“招财”短名；新目录和新库存统一使用“招财猫”。
const SET_ID_ALIASES: Record<string, readonly string[]> = {
  招财猫: ["招财猫", "招财"],
};

export type ExtraMetric = "effect_hit" | "effect_resist" | "set" | null;

/** 页面从御魂目录传入的普通四件套；数组顺序仅作为同速度时的稳定排序依据。 */
export type SpeedTargetSet = {
  setId: string;
  label: string;
};

export type SpeedCell = {
  value: string;
  tail?: string;
};

export type SpeedRow = {
  label: string;
  speed: string;
  positions: SpeedCell[];
  extra?: string;
  /** 当前排行行实际选中的六枚御魂；所有速度分组都可通过此字段打开详情。 */
  selected?: MySoul[];
};

export type SpeedSection = {
  title: string;
  positionHeader: string;
  extraHeader?: string;
  rows: SpeedRow[];
};

/** Worker 与页面共用的排行结果；稳定键用于校验输入，每个完整排行行携带六枚入选御魂供详情弹层展示。 */
export type SpeedCalculationResult = {
  soulKeys: string[];
  soulCount: number;
  qualifiedSoulCount: number;
  sections: SpeedSection[];
};

type SetBuildState = {
  selected: MySoul[];
  speed: number;
  targetCount: number;
};

/** 判断御魂是否属于目标套装；只兼容已知历史别名，避免放宽其他套装的精确匹配。 */
function matchesTargetSet(soul: MySoul, setId: string): boolean {
  return (SET_ID_ALIASES[setId] ?? [setId]).includes(soul.setId);
}

/** 速度只统计副属性，二号位主属性速度由主属性值单独加入组合总值。 */
function subSpeed(soul: MySoul): number {
  return soul.attributes
    .filter(
      (attribute) => !attribute.fixedAttribute && attribute.attributeType === "speed",
    )
    .reduce((total, attribute) => total + attribute.value, 0);
}

/** 一速行尾只展示四号位和六号位主属性，保持表格的紧凑表达。 */
function tailAttribute(soul: MySoul): string | undefined {
  if (soul.slot !== 4 && soul.slot !== 6) return undefined;
  if (soul.mainAttrType === "speed") return undefined;
  const labels: Record<string, string> = {
    attack_rate: "攻击",
    attack_flat: "攻击",
    hp_rate: "生命",
    hp_flat: "生命",
    defense_rate: "防御",
    defense_flat: "防御",
    speed: "速度",
    effect_hit: "命中",
    effect_resist: "抵抗",
    crit_rate: "暴击",
    crit_damage: "暴击伤害",
  };
  return labels[soul.mainAttrType] ?? soul.mainAttrType;
}

/** 返回单枚御魂的主属性加副属性目标值，用于命中/抵抗的平局排序和总计。 */
function soulMetric(soul: MySoul, metric: "effect_hit" | "effect_resist"): number {
  const mainValue = soul.mainAttrType === metric ? soul.mainAttrValue : 0;
  const subValue = soul.attributes
    .filter((attribute) => attribute.attributeType === metric)
    .reduce((total, attribute) => total + attribute.value, 0);
  return mainValue + subValue;
}

/** 同速度时用目标属性和稳定 ID 做确定性排序，确保每次计算结果顺序稳定。 */
function sortCandidates(
  left: MySoul,
  right: MySoul,
  metric: ExtraMetric = null,
): number {
  const speedDifference = subSpeed(right) - subSpeed(left);
  if (speedDifference !== 0) return speedDifference;
  if (metric && metric !== "set") {
    const metricDifference = soulMetric(right, metric) - soulMetric(left, metric);
    if (metricDifference !== 0) return metricDifference;
  }
  return left.soulKey.localeCompare(right.soulKey, "zh-CN");
}

/** 按号位建立候选列表；二号位必须是速度主属性，命中/抵抗榜还限制四号位主属性。 */
function candidatesBySlot(
  eligibleSouls: MySoul[],
  metric: ExtraMetric = null,
): Map<number, MySoul[]> {
  const candidates = new Map<number, MySoul[]>();
  for (const slot of SLOT_ORDER) {
    const slotCandidates = eligibleSouls
      .filter((soul) => {
        if (soul.slot !== slot) return false;
        if (slot === 2 && soul.mainAttrType !== "speed") return false;
        if (
          metric === "effect_hit" &&
          slot === 4 &&
          soul.mainAttrType !== "effect_hit"
        ) {
          return false;
        }
        if (
          metric === "effect_resist" &&
          slot === 4 &&
          soul.mainAttrType !== "effect_resist"
        ) {
          return false;
        }
        return true;
      })
      .sort((left, right) => sortCandidates(left, right, metric));
    candidates.set(slot, slotCandidates);
  }
  return candidates;
}

/** 将一组按号位选出的御魂转换为表格行，并计算总速度与目标属性。 */
function makeRow(
  selected: Array<MySoul | null>,
  label: string,
  metric: ExtraMetric,
): SpeedRow {
  const selectedSouls = selected.filter((soul): soul is MySoul => soul !== null);
  const mainSpeed =
    selected.find((soul) => soul?.slot === 2 && soul.mainAttrType === "speed")
      ?.mainAttrValue ?? 0;
  const totalSpeed =
    mainSpeed + selectedSouls.reduce((total, soul) => total + subSpeed(soul), 0);
  const positions = SLOT_ORDER.map((slot) => {
    const soul = selected.find((candidate) => candidate?.slot === slot) ?? null;
    return {
      value: soul ? formatNumber(subSpeed(soul)) : "—",
      tail: soul ? tailAttribute(soul) : undefined,
    };
  });

  let extra: string | undefined;
  if (metric === "effect_hit" || metric === "effect_resist") {
    extra = formatPercent(
      selectedSouls.reduce((total, soul) => total + soulMetric(soul, metric), 0),
    );
  }
  return {
    label,
    speed: formatNumber(totalSpeed),
    positions,
    extra,
    // 所有完整速度行都支持“查看详情”；占位行没有 selected，按钮会保持禁用。
    selected: selectedSouls,
  };
}

/** 生成散件、命中或抵抗榜单；每一行取各号位的同排名速度，最多显示五行。 */
function rankedRows(
  eligibleSouls: MySoul[],
  title: string,
  metric: ExtraMetric,
): SpeedRow[] {
  const candidates = candidatesBySlot(eligibleSouls, metric);
  const rowCount = Math.min(
    RANK_ROW_COUNT,
    ...SLOT_ORDER.map((slot) => candidates.get(slot)?.length ?? 0),
  );
  return Array.from({ length: Math.max(0, rowCount) }, (_, index) => {
    const selected = SLOT_ORDER.map((slot) => candidates.get(slot)?.[index] ?? null);
    const metricLabel =
      metric === "effect_hit" ? "命中" : metric === "effect_resist" ? "抵抗" : "";
    const titleLabel = metricLabel ? `${title}${metricLabel}` : title;
    return makeRow(selected, `${titleLabel}${RANK_LABELS[index]}速`, metric);
  });
}

/** 套装组合优先速度，速度相同再优先目标套装数量，最后按御魂 ID 稳定排序。 */
function compareBuild(
  left: SetBuildState,
  right: SetBuildState,
  setId: string,
): number {
  if (left.speed !== right.speed) return right.speed - left.speed;
  if (left.targetCount !== right.targetCount)
    return right.targetCount - left.targetCount;
  const leftKey = left.selected.map((soul) => soul.soulKey).join("|");
  const rightKey = right.selected.map((soul) => soul.soulKey).join("|");
  return leftKey.localeCompare(rightKey, "zh-CN") || setId.localeCompare(setId);
}

/** 通过动态规划选择每个套装的最高速六件组合，同时满足至少四件同套。 */
function fastestSetBuild(eligibleSouls: MySoul[], setId: string): MySoul[] | null {
  const candidates = candidatesBySlot(eligibleSouls, "set");
  if (SLOT_ORDER.some((slot) => !candidates.get(slot)?.length)) return null;

  let states = new Map<number, SetBuildState>([
    [0, { selected: [], speed: 0, targetCount: 0 }],
  ]);
  for (const slot of SLOT_ORDER) {
    const next = new Map<number, SetBuildState>();
    for (const state of states.values()) {
      for (const soul of candidates.get(slot) ?? []) {
        const targetCount = state.targetCount + (matchesTargetSet(soul, setId) ? 1 : 0);
        const candidate: SetBuildState = {
          selected: [...state.selected, soul],
          speed: state.speed + subSpeed(soul),
          targetCount,
        };
        const previous = next.get(targetCount);
        if (!previous || compareBuild(candidate, previous, setId) < 0) {
          next.set(targetCount, candidate);
        }
      }
    }
    states = next;
  }

  const valid = [...states.values()].filter(
    (state) => state.targetCount >= REQUIRED_SET_PIECES,
  );
  if (!valid.length) return null;
  valid.sort((left, right) => compareBuild(left, right, setId));
  return valid[0]?.selected ?? null;
}

/** 为没有可用完整组合的固定套装保留占位行，避免用户误以为该类型未被支持。 */
function unavailableSetRow(label: string): SpeedRow {
  return {
    label,
    speed: "—",
    positions: SLOT_ORDER.map(() => ({ value: "—" })),
    extra: "暂无完整组合",
  };
}

/** 统一生成页面四个分组；套装行按总速度倒序，同速度沿用用户配置顺序。 */
function buildSections(
  eligibleSouls: MySoul[],
  targetSets: readonly SpeedTargetSet[],
): SpeedSection[] {
  // 先逐套计算满足“四件目标套装 + 两件任意御魂”的最优六件组合，再按一速倒序展示。
  const setRows = targetSets
    .map(({ setId, label }, index) => {
      const selected = fastestSetBuild(eligibleSouls, setId);
      return {
        row: selected ? makeRow(selected, label, "set") : unavailableSetRow(label),
        order: index,
      };
    })
    .sort((left, right) => {
      const leftSpeed = Number.parseFloat(left.row.speed);
      const rightSpeed = Number.parseFloat(right.row.speed);
      // “暂无”没有可比较速度，统一放在列表末尾；有效结果始终按速度从高到低。
      if (Number.isNaN(leftSpeed) && Number.isNaN(rightSpeed))
        return left.order - right.order;
      if (Number.isNaN(leftSpeed)) return 1;
      if (Number.isNaN(rightSpeed)) return -1;
      return rightSpeed - leftSpeed || left.order - right.order;
    })
    .map(({ row }) => row);

  return [
    {
      title: "散件一速",
      positionHeader: "各位置最高速",
      rows: rankedRows(eligibleSouls, "散件", null),
    },
    {
      title: "散件命中一速",
      positionHeader: "各位置最高速",
      extraHeader: "效果命中",
      rows: rankedRows(eligibleSouls, "散件", "effect_hit"),
    },
    {
      title: "散件抵抗一速",
      positionHeader: "各位置最高速",
      extraHeader: "效果抵抗",
      rows: rankedRows(eligibleSouls, "散件", "effect_resist"),
    },
    {
      title: "套装一速",
      positionHeader: "各位置最高速",
      rows: setRows,
    },
  ];
}

/** 按参考图保留最多两位小数，去掉无意义的末尾 0。 */
function formatNumber(value: number): string {
  return value.toFixed(2).replace(/\.?(0+)$/, "");
}

/** 命中与抵抗按数据层比例值转换为百分比显示。 */
function formatPercent(value: number): string {
  return `${(value * 100).toFixed(2)}%`;
}

/** 计算一速排行；只做纯内存运算，页面计算和 Worker 后台计算共用同一套规则。 */
export function calculateSpeedPreview(
  souls: MySoul[],
  onlyMaxLevel: boolean,
  targetSets: readonly SpeedTargetSet[] = [],
): SpeedCalculationResult {
  // 满级开关关闭后仍保留六星、在库和有速度副属性这三个基本约束。
  const eligibleSouls = souls.filter(
    (soul) =>
      soul.presenceState === "present" &&
      soul.quality === ELIGIBLE_QUALITY &&
      (!onlyMaxLevel || soul.level === ELIGIBLE_LEVEL) &&
      subSpeed(soul) > 0,
  );
  return {
    // 缓存通过稳定御魂键记录计算输入，角色切换或库存替换后不会误认旧排行属于当前库存。
    soulKeys: eligibleSouls.map((soul) => soul.soulKey),
    soulCount: souls.length,
    qualifiedSoulCount: eligibleSouls.length,
    sections: buildSections(eligibleSouls, targetSets),
  };
}
