/**
 * 御魂评分标准的手动配置模型与规则文件转换器。
 *
 * 这里把 UI 的多选状态转换成 Rust 规则引擎能够校验的声明式选择器；
 * 具体御魂 ID 永远不会写入规则文件，规则只保存套装、号位、主属性和副属性条件。
 */

export type RuleScenario = "all" | "pve" | "pvp";

export interface SlotMainPairDraft {
  slot: number;
  mainAttribute: string;
}

export interface RuleScoringDraft {
  coreAttributeWeight: number;
  generalAttributeWeight: number;
  enhancementCoreWeight: number;
  enhancementGeneralWeight: number;
  keepScore: number;
  observeScore: number;
  discardScore: number;
}

/** 御魂列表的评分颜色分级；只影响视觉层级，不改变标准评分数值。 */
export interface ScoreColorThresholdsDraft {
  jadeScore: number;
  goldScore: number;
  rainbowScore: number;
}

export interface ManualRuleCardDraft {
  id: string;
  name: string;
  setIds: string[];
  slots: number[];
  slotMainPairs: SlotMainPairDraft[];
  initialSubstatCounts: number[];
  coreSubstats: string[];
  generalSubstats: string[];
  scenario: RuleScenario;
  candidateUses: string[];
  scoring: RuleScoringDraft;
}

export interface ManualPresetMeta {
  id: string;
  version: string;
  title: string;
  author: string;
  description: string;
}

/** 移除导出文件专用的规范哈希；规则正文被编辑后必须由后端重新计算哈希。 */
export function stripCanonicalHash(
  value: Record<string, unknown>,
): Record<string, unknown> {
  const editable = { ...value };
  delete editable.canonicalSha256;
  return editable;
}

/** 规则引擎认可的规范属性；显示名称由页面单独负责，避免分享文件携带界面文案。 */
export const ATTRIBUTE_OPTIONS = [
  { id: "speed", label: "速度" },
  { id: "crit_rate", label: "暴击" },
  { id: "crit_damage", label: "暴伤" },
  { id: "attack_rate", label: "攻击%" },
  { id: "hp_rate", label: "生命%" },
  { id: "defense_rate", label: "防御%" },
  { id: "effect_hit", label: "效果命中" },
  { id: "effect_resist", label: "效果抵抗" },
  { id: "attack_flat", label: "攻击" },
  { id: "hp_flat", label: "生命" },
  { id: "defense_flat", label: "防御" },
] as const;

/** 按游戏号位限制可选主属性，避免玩家在普通模式中遇到无效组合。 */
export const MAIN_ATTRIBUTES_BY_SLOT: Record<number, string[]> = {
  1: ["attack_flat"],
  2: ["speed", "attack_rate", "hp_rate", "defense_rate"],
  3: ["defense_flat"],
  4: ["attack_rate", "hp_rate", "defense_rate", "effect_hit", "effect_resist"],
  5: ["hp_flat"],
  6: ["attack_rate", "hp_rate", "defense_rate", "crit_rate", "crit_damage"],
};

export const ATTRIBUTE_LABELS: Record<string, string> = Object.fromEntries(
  ATTRIBUTE_OPTIONS.map((item) => [item.id, item.label]),
);

const DEFAULT_SCORING: RuleScoringDraft = {
  coreAttributeWeight: 10,
  generalAttributeWeight: 5,
  enhancementCoreWeight: 10,
  enhancementGeneralWeight: 5,
  keepScore: 70,
  observeScore: 50,
  discardScore: 0,
};

const DEFAULT_SCORE_COLOR_THRESHOLDS: ScoreColorThresholdsDraft = {
  jadeScore: 60,
  goldScore: 70,
  rainbowScore: 75,
};

/** 返回去重且不包含空值的稳定字符串数组，保证规范 JSON 的顺序可预测。 */
export function uniqueStrings(values: string[]): string[] {
  return [...new Set(values.map((value) => value.trim()).filter(Boolean))];
}

/** 复制评分配置，避免多个卡片意外共享同一个可变对象。 */
export function defaultScoring(): RuleScoringDraft {
  return { ...DEFAULT_SCORING };
}

/** 返回默认的青玉、金橙和彩虹分数线，兼容旧版规则文件。 */
export function defaultScoreColorThresholds(): ScoreColorThresholdsDraft {
  return { ...DEFAULT_SCORE_COLOR_THRESHOLDS };
}

/** 创建一张新的手动规则卡片；默认号位按 2/4/6 展开，玩家可以直接删改。 */
export function createManualRuleCard(index: number): ManualRuleCardDraft {
  const slots = [2, 4, 6];
  return {
    id: `manual-rule-${index}`,
    name: `手动规则 ${index}`,
    setIds: [],
    slots,
    slotMainPairs: slots.flatMap((slot) =>
      (MAIN_ATTRIBUTES_BY_SLOT[slot] ?? []).map((mainAttribute) => ({
        slot,
        mainAttribute,
      })),
    ),
    initialSubstatCounts: [3, 4],
    coreSubstats: ["speed", "crit_rate", "crit_damage"],
    generalSubstats: ["attack_rate"],
    scenario: "all",
    candidateUses: ["manual-rule"],
    scoring: defaultScoring(),
  };
}

function asStringArray(value: unknown): string[] {
  return Array.isArray(value)
    ? uniqueStrings(value.filter((item): item is string => typeof item === "string"))
    : [];
}

function asNumberArray(value: unknown): number[] {
  return Array.isArray(value)
    ? [...new Set(value.filter((item): item is number => Number.isInteger(item)))]
    : [];
}

function asScore(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

/** 从规则正文读取列表颜色配置；缺字段的旧标准自动使用当前默认分数线。 */
export function parseScoreColorThresholds(
  parsed: Record<string, unknown>,
): ScoreColorThresholdsDraft {
  const raw = (parsed.scoreColorThresholds ?? {}) as Record<string, unknown>;
  return {
    jadeScore: asScore(raw.jadeScore, DEFAULT_SCORE_COLOR_THRESHOLDS.jadeScore),
    goldScore: asScore(raw.goldScore, DEFAULT_SCORE_COLOR_THRESHOLDS.goldScore),
    rainbowScore: asScore(
      raw.rainbowScore,
      DEFAULT_SCORE_COLOR_THRESHOLDS.rainbowScore,
    ),
  };
}

function selectorPairs(
  selector: Record<string, unknown>,
  slots: number[],
  mainAttributes: string[],
): SlotMainPairDraft[] {
  const pairs = Array.isArray(selector.slotMainPairs)
    ? selector.slotMainPairs
        .map((item) => item as Record<string, unknown>)
        .filter(
          (item) =>
            Number.isInteger(item.slot) && typeof item.mainAttribute === "string",
        )
        .map((item) => ({
          slot: item.slot as number,
          mainAttribute: item.mainAttribute as string,
        }))
    : [];
  if (pairs.length) return pairs;

  const allowedSlots = slots.length ? slots : [1, 2, 3, 4, 5, 6];
  const allowedMainAttributes = mainAttributes.length
    ? mainAttributes
    : allowedSlots.flatMap((slot) => MAIN_ATTRIBUTES_BY_SLOT[slot] ?? []);
  return allowedSlots.flatMap((slot) =>
    allowedMainAttributes
      .filter((mainAttribute) =>
        (MAIN_ATTRIBUTES_BY_SLOT[slot] ?? []).includes(mainAttribute),
      )
      .map((mainAttribute) => ({ slot, mainAttribute })),
  );
}

/** 从已有规则文件读取全部规则卡片，旧版规则会自动映射为手动选择状态。 */
export function parseManualRuleCards(
  parsed: Record<string, unknown>,
): ManualRuleCardDraft[] {
  const rules = Array.isArray(parsed.rules) ? parsed.rules : [];
  const cards = rules.map((item, index) => {
    const rule = (item ?? {}) as Record<string, unknown>;
    const selector = (rule.selector ?? {}) as Record<string, unknown>;
    const required = (selector.requiredSubstats ?? {}) as Record<string, unknown>;
    const rawScoring = (rule.scoring ?? {}) as Record<string, unknown>;
    const scenarioValue = asStringArray(selector.scenarios)[0];
    const slots = asNumberArray(selector.slots);
    const pairs = selectorPairs(
      selector,
      slots,
      asStringArray(selector.mainAttributes),
    );
    const resolvedSlots = [...new Set(pairs.map((pair) => pair.slot))];
    const card: ManualRuleCardDraft = {
      id: typeof rule.id === "string" ? rule.id : `manual-rule-${index + 1}`,
      name:
        typeof rule.name === "string" && rule.name.trim()
          ? rule.name
          : `手动规则 ${index + 1}`,
      setIds: asStringArray(selector.setIds),
      slots: resolvedSlots,
      slotMainPairs: pairs,
      initialSubstatCounts: asNumberArray(selector.initialSubstatCounts),
      coreSubstats: asStringArray(required.allOf),
      generalSubstats: asStringArray(required.anyOf),
      scenario:
        scenarioValue === "pve" || scenarioValue === "pvp" ? scenarioValue : "all",
      candidateUses: asStringArray(rule.candidateUses),
      scoring: {
        coreAttributeWeight: asScore(
          rawScoring.coreAttributeWeight,
          DEFAULT_SCORING.coreAttributeWeight,
        ),
        generalAttributeWeight: asScore(
          rawScoring.generalAttributeWeight,
          DEFAULT_SCORING.generalAttributeWeight,
        ),
        enhancementCoreWeight: asScore(
          rawScoring.enhancementCoreWeight,
          DEFAULT_SCORING.enhancementCoreWeight,
        ),
        enhancementGeneralWeight: asScore(
          rawScoring.enhancementGeneralWeight,
          DEFAULT_SCORING.enhancementGeneralWeight,
        ),
        keepScore: asScore(rawScoring.keepScore, DEFAULT_SCORING.keepScore),
        observeScore: asScore(rawScoring.observeScore, DEFAULT_SCORING.observeScore),
        discardScore: asScore(rawScoring.discardScore, DEFAULT_SCORING.discardScore),
      },
    };
    card.candidateUses = card.candidateUses.length
      ? card.candidateUses
      : [card.scenario === "all" ? "manual-rule" : `manual-${card.scenario}`];
    card.slots = resolvedSlots;
    return card;
  });
  return cards.length ? cards : [createManualRuleCard(1)];
}

function ruleLabel(card: ManualRuleCardDraft): string {
  const sets = card.setIds.length ? card.setIds.join("、") : "全部套装";
  const slots = card.slots.length ? card.slots.join("、") : "全部号位";
  return `${sets} · ${slots}号位 · ${card.scenario.toUpperCase()}`;
}

/** 将一张 UI 卡片转换成后端可校验、可分享的声明式规则。 */
export function manualRuleToPayload(
  card: ManualRuleCardDraft,
): Record<string, unknown> {
  const pairs = card.slotMainPairs.filter(
    (pair) =>
      Number.isInteger(pair.slot) &&
      typeof pair.mainAttribute === "string" &&
      (MAIN_ATTRIBUTES_BY_SLOT[pair.slot] ?? []).includes(pair.mainAttribute),
  );
  const scenario = card.scenario === "all" ? [] : [card.scenario];
  return {
    id: card.id,
    stage: "admission",
    rawLabel: ruleLabel(card),
    name: card.name.trim() || "未命名规则",
    status: "draft",
    evidenceLevel: "author",
    sourceRefs: [],
    selector: {
      setIds: uniqueStrings(card.setIds),
      slots: [...new Set(pairs.map((pair) => pair.slot))],
      mainAttributes: [],
      slotMainPairs: pairs,
      initialSubstatCounts: [...new Set(card.initialSubstatCounts)].sort(
        (a, b) => a - b,
      ),
      scenarios: scenario,
      requiredSubstats: {
        allOf: uniqueStrings(card.coreSubstats),
        anyOf: uniqueStrings(card.generalSubstats),
      },
    },
    candidateUses:
      card.candidateUses.length > 0
        ? uniqueStrings(card.candidateUses)
        : [card.scenario === "all" ? "manual-rule" : `manual-${card.scenario}`],
    scoring: {
      coreAttributes: uniqueStrings(card.coreSubstats),
      generalAttributes: uniqueStrings(card.generalSubstats),
      ...card.scoring,
    },
  };
}

/** 构造完整规则预设；保留原始元数据，替换规则正文，方便复制默认标准后保存。 */
export function buildManualPreset(
  base: Record<string, unknown>,
  meta: ManualPresetMeta,
  cards: ManualRuleCardDraft[],
  scoreColorThresholds: ScoreColorThresholdsDraft = defaultScoreColorThresholds(),
): Record<string, unknown> {
  // 导出文件的完整性字段只对未修改的原始文件有效，编辑时不能沿用旧哈希。
  const editableBase = stripCanonicalHash(base);
  return {
    ...editableBase,
    format: "yys-rule-preset",
    schemaVersion: 1,
    id: meta.id,
    version: meta.version,
    title: meta.title.trim() || "未命名评分标准",
    author: meta.author.trim() || "本机用户",
    status: "draft",
    rawDescription: meta.description.trim() || undefined,
    sourceRefs: Array.isArray(base.sourceRefs) ? base.sourceRefs : [],
    sourceText: cards.map((card) => ruleLabel(card)),
    defaults: base.defaults ?? { quality: [6], level: [0] },
    scoreColorThresholds: { ...scoreColorThresholds },
    rules: cards.map(manualRuleToPayload),
  };
}

/** 校验列表颜色分级，确保用户不会配置出反向或超出百分制的颜色区间。 */
export function validateScoreColorThresholds(
  thresholds: ScoreColorThresholdsDraft,
): string | null {
  const values = [thresholds.jadeScore, thresholds.goldScore, thresholds.rainbowScore];
  if (values.some((value) => !Number.isFinite(value) || value < 0 || value > 100)) {
    return "列表颜色阈值必须位于 0 到 100";
  }
  if (
    thresholds.jadeScore > thresholds.goldScore ||
    thresholds.goldScore > thresholds.rainbowScore
  ) {
    return "列表颜色阈值需要满足：青玉 ≤ 金橙 ≤ 彩虹";
  }
  return null;
}

/** 校验卡片是否具备后端准入规则必需字段，并返回面向用户的错误。 */
export function validateManualCards(cards: ManualRuleCardDraft[]): string | null {
  if (!cards.length) return "至少需要一条评分规则";
  for (const [index, card] of cards.entries()) {
    if (!card.name.trim()) return `第 ${index + 1} 条规则需要填写名称`;
    if (!card.slotMainPairs.length)
      return `第 ${index + 1} 条规则至少选择一个号位和主属性`;
    if (!card.coreSubstats.length && !card.generalSubstats.length) {
      return `第 ${index + 1} 条规则至少选择一个有效副属性`;
    }
    if (
      card.coreSubstats.some((attribute) => card.generalSubstats.includes(attribute))
    ) {
      return `第 ${index + 1} 条规则的核心属性和一般属性不能重复`;
    }
    if (
      card.scoring.discardScore > card.scoring.observeScore ||
      card.scoring.observeScore > card.scoring.keepScore
    ) {
      return `第 ${index + 1} 条规则的评分阈值需要满足：弃置 ≤ 观察 ≤ 保留`;
    }
  }
  return null;
}
