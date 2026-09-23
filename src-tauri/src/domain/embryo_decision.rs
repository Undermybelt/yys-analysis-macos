//! 六星四腿 `+0` 御魂胚子强化决策。
//!
//! 本模块只回答“某一条已有可强化副属性是否值得继续投入”这一问题，不混入用途评分、
//! 式神价值或游戏内自动操作。所有比较都保留原始小数，展示层不得用格式化文本回算结论。

use super::{
    ATTACK_ATTRIBUTES, DEFENSE_ATTRIBUTES, InventoryItem, SnapshotSoul, SoulAttribute,
    UTILITY_ATTRIBUTES, attribute_display_name,
};
use serde::Serialize;
use std::collections::HashMap;

/// 胚子决策模型版本；该模型属于社区经验，不代表官方强化概率或官方推荐。
pub const EMBRYO_DECISION_MODEL_VERSION: &str = "community-embryo-decision-v1";
/// 胚子决策模型的依据状态，与数值来源一起展示，避免把经验模型伪装成官方规则。
pub const EMBRYO_DECISION_EVIDENCE_STATUS: &str = "community-experience";
/// 四腿和三腿 `+0` 御魂从 `+0` 到 `+15` 都还剩五次强化节点。
pub const EMBRYO_REMAINING_HAND_COUNT: u8 = 5;
/// 四腿强化时，单次节点均匀落在四条已有副属性中的一条。
pub const EMBRYO_LEG_COUNT: u8 = 4;
/// 三腿在第一手必然新增副属性，因此已有属性实际只参与后四手。
pub const THREE_LEG_COUNT: u8 = 3;
/// 三腿新增属性后的四条副属性，后续每手命中目标属性的概率为四分之一。
pub const THREE_LEG_EXISTING_REMAINING_HAND_COUNT: u8 = 4;
/// 一条路径的理论上限定义为五次强化全部命中当前目标属性。
pub const EMBRYO_PATH_HIT_PROBABILITY: f64 = 1.0 / 1024.0;
/// 三腿已有属性路径的四手全中概率；第一手用于新增第四条副属性。
pub const THREE_LEG_EXISTING_PATH_HIT_PROBABILITY: f64 = 1.0 / 256.0;

/// 一枚用于对照的同类 `+15` 御魂及其目标属性精确值。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbryoComparisonItem {
    pub soul_key: String,
    pub value: f64,
}

/// 新增属性类别下的一个具体可能分支；属性内概率未知时保持为空，不伪造概率或通常期望。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbryoPossibleAttributePath {
    pub attribute_type: String,
    pub attribute_label: String,
    /// 新属性首次出现并在剩余四手全部命中时的理论高档上限；固定属性没有成长基准时为空。
    pub theoretical_upper: Option<f64>,
    /// 具体新增属性的通常期望不由已知类别概率推出，因此始终保持未知。
    pub usual_expected: Option<f64>,
    /// 类别内具体属性概率未公开，不能把类别概率拆分为属性概率。
    pub hit_probability: Option<f64>,
    /// 只要存在可用成长基准，就沿用同套装、同号位、同主属性的对照规则。
    pub comparison_value: Option<f64>,
    pub comparison_source: String,
    pub comparison_items: Vec<EmbryoComparisonItem>,
    pub comparison_candidate_count: u32,
    pub theoretical_excess: Option<f64>,
    pub usual_excess: Option<f64>,
    /// 固定属性或未知属性基准不能强行套用三档结论，因此允许为空。
    pub conclusion: Option<String>,
    pub data_note: String,
}

/// 三腿或四腿胚子的一条独立强化路径；同一枚胚子的不同副属性不会合并成一个上限。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbryoDecisionPath {
    pub path_id: String,
    pub soul_key: String,
    pub set_id: String,
    pub slot: u8,
    pub quality: u8,
    pub level: u8,
    pub main_attr_type: String,
    pub leg_count: u8,
    pub path_kind: String,
    pub attribute_index: Option<u8>,
    pub attribute_type: String,
    pub attribute_label: String,
    pub initial_value: Option<f64>,
    /// 五手全部命中目标属性，并按该属性成长区间高档上限计算的理论值。
    pub theoretical_upper: Option<f64>,
    /// 已有属性路径可以计算通常期望；新增属性具体类型未知时保持为空。
    pub usual_expected: Option<f64>,
    /// 已有属性路径是完整命中概率；新增类别路径只使用 category_probability。
    pub hit_probability: Option<f64>,
    /// 同类库存最高一枚的目标属性值，或无库存时的平均期望门槛。
    pub comparison_value: Option<f64>,
    /// `inventory` 表示来自同类 `+15` 库存，`average-threshold` 表示无对照库存，
    /// `category-branches` 表示对照值需要展开到新增属性分支查看。
    pub comparison_source: String,
    /// 默认保留同类库存前 3 枚，页面可展开查看；排序仍使用全部精确值中的最高值。
    pub comparison_items: Vec<EmbryoComparisonItem>,
    pub comparison_candidate_count: u32,
    /// 理论上限相对对照值的精确差值。
    pub theoretical_excess: Option<f64>,
    /// 通常期望相对对照值的精确差值；默认排序使用该字段。
    pub usual_excess: Option<f64>,
    /// `stop`、`high-risk` 或 `worth-it`。
    pub conclusion: String,
    /// 新增类别路径的具体分支；已有属性路径为空。
    pub possible_attributes: Vec<EmbryoPossibleAttributePath>,
    /// 仅新增类别路径有值，用于明确展示 36%/36%/28% 的类别概率。
    pub category_probability: Option<f64>,
    /// 解释未知属性概率、通常期望或固定属性基准的原因。
    pub data_note: String,
}

/// 当前档案中所选腿数的 `+0` 胚子强化路径报告。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbryoDecisionReport {
    pub model_version: String,
    pub evidence_status: String,
    pub model_note: String,
    pub remaining_hand_count: u8,
    pub selected_leg_counts: Vec<u8>,
    pub candidate_count: u32,
    pub path_count: u32,
    /// 当前档案中因局部快照语义而未确认、未纳入当前分析的库存数量。
    pub excluded_unconfirmed_count: u32,
    pub paths: Vec<EmbryoDecisionPath>,
}

/// 分析当前库存所选腿数的六星 `+0` 胚子，并按结论优先级和通常期望超越差值排序。
///
/// 先按库存稳定键关联快照事实，分开收集六星 `+15` 的同类对照，再为每枚六星 `+0`
/// 四腿御魂的每条可强化副属性构造独立路径。没有同属性对照时，使用该属性类别的六手平均
/// 期望作为门槛；所有数值都在这里用精确小数比较，前端仅负责格式化展示。
pub fn analyze_embryo_decision(
    inventory: &[InventoryItem],
    souls: &[SnapshotSoul],
    attributes: &[SoulAttribute],
    include_three_leg: bool,
    include_four_leg: bool,
) -> EmbryoDecisionReport {
    let souls_by_identity = souls
        .iter()
        .map(|soul| ((soul.snapshot_id.clone(), soul.internal_id.clone()), soul))
        .collect::<HashMap<_, _>>();
    let mut attributes_by_identity = HashMap::<(String, String), Vec<&SoulAttribute>>::new();
    for attribute in attributes {
        attributes_by_identity
            .entry((
                attribute.snapshot_id.clone(),
                attribute.soul_internal_id.clone(),
            ))
            .or_default()
            .push(attribute);
    }

    // 先建立“套装、号位、主属性、目标副属性”的对照池，避免每条胚子路径重复扫描全部库存。
    let mut comparison_pools = HashMap::<ComparisonKey, Vec<EmbryoComparisonItem>>::new();
    for item in inventory
        .iter()
        .filter(|item| item.presence_state == "present")
    {
        let Some(soul) =
            souls_by_identity.get(&(item.snapshot_id.clone(), item.soul_internal_id.clone()))
        else {
            continue;
        };
        if soul.quality != 6 || soul.level != 15 {
            continue;
        }
        let soul_attributes = attributes_by_identity
            .get(&(soul.snapshot_id.clone(), soul.internal_id.clone()))
            .cloned()
            .unwrap_or_default();
        for attribute in soul_attributes
            .into_iter()
            .filter(|attribute| !attribute.fixed_attribute)
            .filter(|attribute| roll_profile(&attribute.attribute_type).is_some())
            .filter(|attribute| attribute.value.is_finite())
        {
            comparison_pools
                .entry(ComparisonKey {
                    set_id: soul.set_id.clone(),
                    slot: soul.slot,
                    main_attr_type: soul.main_attr_type.clone(),
                    attribute_type: attribute.attribute_type.clone(),
                })
                .or_default()
                .push(EmbryoComparisonItem {
                    soul_key: item.soul_key.clone(),
                    value: attribute.value,
                });
        }
    }
    // 最高一枚决定结论；同值时按稳定键排序，确保重复计算的展示顺序一致。
    for comparison_items in comparison_pools.values_mut() {
        comparison_items.sort_by(|left, right| {
            right
                .value
                .total_cmp(&left.value)
                .then_with(|| left.soul_key.cmp(&right.soul_key))
        });
    }

    let mut paths = Vec::new();
    let mut candidate_count = 0_u32;
    let mut selected_leg_counts = Vec::new();
    if include_three_leg {
        selected_leg_counts.push(THREE_LEG_COUNT);
    }
    if include_four_leg {
        selected_leg_counts.push(EMBRYO_LEG_COUNT);
    }
    // 未勾选的腿数完全不参与候选统计和路径构造，避免结果数量与界面选择不一致。
    for item in inventory
        .iter()
        .filter(|item| item.presence_state == "present")
    {
        let Some(soul) =
            souls_by_identity.get(&(item.snapshot_id.clone(), item.soul_internal_id.clone()))
        else {
            continue;
        };
        // 初始腿数是导入事实；不能用当前属性数组长度替代，避免缺失属性时误判胚子类型。
        let Some(leg_count) = soul.initial_substat_count else {
            continue;
        };
        if soul.quality != 6
            || soul.level != 0
            || (leg_count == THREE_LEG_COUNT && !include_three_leg)
            || (leg_count == EMBRYO_LEG_COUNT && !include_four_leg)
            || ![THREE_LEG_COUNT, EMBRYO_LEG_COUNT].contains(&leg_count)
        {
            continue;
        }
        candidate_count += 1;
        let soul_attributes = attributes_by_identity
            .get(&(soul.snapshot_id.clone(), soul.internal_id.clone()))
            .cloned()
            .unwrap_or_default();
        let rollable_attributes = soul_attributes
            .into_iter()
            .filter(|attribute| !attribute.fixed_attribute)
            .filter(|attribute| roll_profile(&attribute.attribute_type).is_some())
            .filter(|attribute| attribute.value.is_finite())
            .collect::<Vec<_>>();

        // 已有副属性路径沿用四腿对照规则；三腿只因第一手必然新增而减少一手可命中次数。
        for attribute in rollable_attributes.iter().copied() {
            let profile =
                roll_profile(&attribute.attribute_type).expect("已过滤的属性必须存在成长档位");
            let comparison_key = ComparisonKey {
                set_id: soul.set_id.clone(),
                slot: soul.slot,
                main_attr_type: soul.main_attr_type.clone(),
                attribute_type: attribute.attribute_type.clone(),
            };
            let comparison_pool = comparison_pools
                .get(&comparison_key)
                .cloned()
                .unwrap_or_default();
            let comparison_candidate_count = comparison_pool.len() as u32;
            let (comparison_value, comparison_source, comparison_items) =
                if let Some(best) = comparison_pool.first() {
                    (
                        best.value,
                        "inventory".to_owned(),
                        comparison_pool.into_iter().take(3).collect::<Vec<_>>(),
                    )
                } else {
                    (
                        profile.average_threshold,
                        "average-threshold".to_owned(),
                        Vec::new(),
                    )
                };
            // 理论值和通常值都从未强化的当前精确值出发，不使用四舍五入后的界面文本。
            let remaining_hit_count = if leg_count == THREE_LEG_COUNT {
                THREE_LEG_EXISTING_REMAINING_HAND_COUNT
            } else {
                EMBRYO_REMAINING_HAND_COUNT
            };
            let existing_attribute_probability = if leg_count == THREE_LEG_COUNT {
                THREE_LEG_EXISTING_PATH_HIT_PROBABILITY
            } else {
                EMBRYO_PATH_HIT_PROBABILITY
            };
            let denominator = f64::from(EMBRYO_LEG_COUNT);
            let theoretical_upper =
                attribute.value + f64::from(remaining_hit_count) * profile.maximum_roll;
            let usual_expected = attribute.value
                + (f64::from(remaining_hit_count) / denominator) * profile.average_roll;
            let theoretical_excess = theoretical_upper - comparison_value;
            let usual_excess = usual_expected - comparison_value;
            // 结论只看精确值：理论也不能超过时停止；只有理论能超过时才进入风险判断。
            let conclusion = if theoretical_upper <= comparison_value {
                "stop"
            } else if usual_expected <= comparison_value {
                "high-risk"
            } else {
                "worth-it"
            };
            paths.push(EmbryoDecisionPath {
                path_id: format!("{}:{}", item.soul_key, attribute.attribute_index),
                soul_key: item.soul_key.clone(),
                set_id: soul.set_id.clone(),
                slot: soul.slot,
                quality: soul.quality,
                level: soul.level,
                main_attr_type: soul.main_attr_type.clone(),
                leg_count,
                path_kind: "existing-attribute".to_owned(),
                attribute_index: Some(attribute.attribute_index),
                attribute_type: attribute.attribute_type.clone(),
                attribute_label: attribute_label(&attribute.attribute_type).to_owned(),
                initial_value: Some(attribute.value),
                theoretical_upper: Some(theoretical_upper),
                usual_expected: Some(usual_expected),
                hit_probability: Some(existing_attribute_probability),
                comparison_value: Some(comparison_value),
                comparison_source,
                comparison_items,
                comparison_candidate_count,
                theoretical_excess: Some(theoretical_excess),
                usual_excess: Some(usual_excess),
                conclusion: conclusion.to_owned(),
                possible_attributes: Vec::new(),
                category_probability: None,
                data_note: if leg_count == THREE_LEG_COUNT {
                    "三腿第一手必然新增第四条副属性；已有属性路径的通常期望只计算后四手按四条均匀命中。".to_owned()
                } else {
                    "四腿五手均按四条已有副属性均匀命中计算。".to_owned()
                },
            });
        }

        if leg_count == THREE_LEG_COUNT {
            // 新增类别只展示三类父路径；具体属性分支不拆分类别概率，避免伪造属性概率。
            let existing_attribute_types = attributes_by_identity
                .get(&(soul.snapshot_id.clone(), soul.internal_id.clone()))
                .into_iter()
                .flatten()
                .map(|attribute| attribute.attribute_type.as_str())
                .collect::<std::collections::HashSet<_>>();
            for (category_id, category_label, category_probability, possible_types) in
                new_attribute_categories()
            {
                let possible_attributes = possible_types
                    .iter()
                    .filter(|attribute_type| !existing_attribute_types.contains(**attribute_type))
                    .map(|attribute_type| {
                        let profile = roll_profile(attribute_type);
                        let comparison_key = ComparisonKey {
                            set_id: soul.set_id.clone(),
                            slot: soul.slot,
                            main_attr_type: soul.main_attr_type.clone(),
                            attribute_type: (*attribute_type).to_owned(),
                        };
                        let comparison_pool = comparison_pools
                            .get(&comparison_key)
                            .cloned()
                            .unwrap_or_default();
                        let comparison_candidate_count = comparison_pool.len() as u32;
                        let (comparison_value, comparison_source, comparison_items) =
                            if let Some(profile) = profile {
                                if let Some(best) = comparison_pool.first() {
                                    (
                                        Some(best.value),
                                        "inventory".to_owned(),
                                        comparison_pool.into_iter().take(3).collect::<Vec<_>>(),
                                    )
                                } else {
                                    (
                                        Some(profile.average_threshold),
                                        "average-threshold".to_owned(),
                                        Vec::new(),
                                    )
                                }
                            } else {
                                (None, "unavailable".to_owned(), Vec::new())
                            };
                        let theoretical_upper = profile.map(|profile| {
                            f64::from(EMBRYO_REMAINING_HAND_COUNT) * profile.maximum_roll
                        });
                        let theoretical_excess = theoretical_upper
                            .zip(comparison_value)
                            .map(|(upper, comparison)| upper - comparison);
                        let conclusion = theoretical_excess.map(|excess| {
                            if excess <= 0.0 {
                                "stop".to_owned()
                            } else {
                                // 具体属性通常期望未知，理论可超越时只能标记高风险。
                                "high-risk".to_owned()
                            }
                        });
                        let data_note = if profile.is_some() {
                            "具体属性可能出现，但类别内具体概率未声明；通常期望不计算。理论上限按新增第一手与后四手全部取该属性高档值，仅作可能分支上界。".to_owned()
                        } else {
                            "该具体属性属于可能分支，但没有可用于成长比较的高档基准；不计算理论上限、通常期望和三档结论。".to_owned()
                        };
                        EmbryoPossibleAttributePath {
                            attribute_type: (*attribute_type).to_owned(),
                            attribute_label: attribute_display_name(attribute_type).to_owned(),
                            theoretical_upper,
                            usual_expected: None,
                            hit_probability: None,
                            comparison_value,
                            comparison_source,
                            comparison_items,
                            comparison_candidate_count,
                            theoretical_excess,
                            usual_excess: None,
                            conclusion,
                            data_note,
                        }
                    })
                    .collect::<Vec<_>>();
                let category_conclusion = possible_attributes
                    .iter()
                    .filter_map(|branch| branch.conclusion.as_deref())
                    .max_by_key(|conclusion| conclusion_rank(conclusion))
                    .unwrap_or("stop")
                    .to_owned();
                paths.push(EmbryoDecisionPath {
                    path_id: format!("{}:new:{}", item.soul_key, category_id),
                    soul_key: item.soul_key.clone(),
                    set_id: soul.set_id.clone(),
                    slot: soul.slot,
                    quality: soul.quality,
                    level: soul.level,
                    main_attr_type: soul.main_attr_type.clone(),
                    leg_count,
                    path_kind: "new-category".to_owned(),
                    attribute_index: None,
                    attribute_type: format!("new_{category_id}"),
                    attribute_label: format!("新增{category_label}"),
                    initial_value: None,
                    theoretical_upper: None,
                    usual_expected: None,
                    hit_probability: None,
                    comparison_value: None,
                    comparison_source: "category-branches".to_owned(),
                    comparison_items: Vec::new(),
                    comparison_candidate_count: 0,
                    theoretical_excess: None,
                    usual_excess: None,
                    conclusion: category_conclusion,
                    possible_attributes,
                    category_probability: Some(category_probability),
                    data_note: "第一手新增属性类别概率已知；类别内具体属性概率和通常期望未声明。展开后查看全部可能属性分支及其可比较的理论上限。".to_owned(),
                });
            }
        }
    }

    // 结论优先级高于数值差值；同分继续使用稳定键和属性索引保持默认顺序可复现。
    paths.sort_by(|left, right| {
        conclusion_rank(&right.conclusion)
            .cmp(&conclusion_rank(&left.conclusion))
            .then_with(|| {
                right
                    .usual_excess
                    .unwrap_or(f64::NEG_INFINITY)
                    .total_cmp(&left.usual_excess.unwrap_or(f64::NEG_INFINITY))
            })
            .then_with(|| {
                right
                    .theoretical_excess
                    .unwrap_or(f64::NEG_INFINITY)
                    .total_cmp(&left.theoretical_excess.unwrap_or(f64::NEG_INFINITY))
            })
            .then_with(|| left.soul_key.cmp(&right.soul_key))
            .then_with(|| left.attribute_index.cmp(&right.attribute_index))
    });

    EmbryoDecisionReport {
        model_version: EMBRYO_DECISION_MODEL_VERSION.to_owned(),
        evidence_status: EMBRYO_DECISION_EVIDENCE_STATUS.to_owned(),
        model_note: if include_three_leg {
            "社区经验模型：分析已勾选的六星 +0 三腿/四腿胚子；三腿第一手必然新增属性，新增类别按攻击类36%、防御类36%、功能类28%展示，类别内具体属性概率与通常期望保持未知；不代表官方概率。".to_owned()
        } else {
            "社区经验模型：只分析六星 +0 四腿胚子；理论上限假设剩余五手全部命中目标属性，通常期望按四条已有副属性均匀命中和属性区间中位值计算；不代表官方概率。".to_owned()
        },
        remaining_hand_count: EMBRYO_REMAINING_HAND_COUNT,
        selected_leg_counts,
        candidate_count,
        path_count: paths.len() as u32,
        excluded_unconfirmed_count: 0,
        paths,
    }
}

/// 保留旧四腿领域入口，保证前两个任务的默认行为和已有调用方不变。
pub fn analyze_four_leg_embryo_decision(
    inventory: &[InventoryItem],
    souls: &[SnapshotSoul],
    attributes: &[SoulAttribute],
) -> EmbryoDecisionReport {
    analyze_embryo_decision(inventory, souls, attributes, false, true)
}

/// 返回强化机制定义的三类新增属性及其具体可能类型；类别内不推导具体概率。
fn new_attribute_categories() -> [(&'static str, &'static str, f64, &'static [&'static str]); 3] {
    [
        ("attack", "攻击类", 0.36, &ATTACK_ATTRIBUTES),
        ("defense", "防御类", 0.36, &DEFENSE_ATTRIBUTES),
        ("utility", "功能类", 0.28, &UTILITY_ATTRIBUTES),
    ]
}

/// 对照池的稳定复合键；主属性必须保留，避免把不同号位用途的成品混在一起。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ComparisonKey {
    set_id: String,
    slot: u8,
    main_attr_type: String,
    attribute_type: String,
}

/// 单条可强化副属性的经验成长参数；比例属性按数据库中的 0～1 计算，速度使用点数。
#[derive(Clone, Copy)]
struct RollProfile {
    maximum_roll: f64,
    average_roll: f64,
    average_threshold: f64,
}

/// 返回八种可强化副属性的高档上限、中位期望和无库存六手平均期望门槛。
fn roll_profile(attribute_type: &str) -> Option<RollProfile> {
    match attribute_type {
        "speed" => Some(RollProfile {
            maximum_roll: 3.0,
            average_roll: 2.7,
            average_threshold: 16.2,
        }),
        "crit_rate" | "attack_rate" | "hp_rate" | "defense_rate" => Some(RollProfile {
            maximum_roll: 0.03,
            average_roll: 0.027,
            average_threshold: 0.162,
        }),
        "crit_damage" | "effect_hit" | "effect_resist" => Some(RollProfile {
            maximum_roll: 0.04,
            average_roll: 0.036,
            average_threshold: 0.216,
        }),
        _ => None,
    }
}

/// 将稳定属性编号转换成用户可读名称；未知编号保留原值以便追溯导入事实。
fn attribute_label(attribute_type: &str) -> &str {
    match attribute_type {
        "attack_rate" => "攻击加成",
        "hp_rate" => "生命加成",
        "defense_rate" => "防御加成",
        "speed" => "速度",
        "crit_rate" => "暴击",
        "crit_damage" => "暴伤",
        "effect_hit" => "效果命中",
        "effect_resist" => "效果抵抗",
        _ => attribute_type,
    }
}

/// 将三档结论映射为默认排序的优先级，值得强化应排在高风险和停止之前。
fn conclusion_rank(conclusion: &str) -> u8 {
    match conclusion {
        "worth-it" => 3,
        "high-risk" => 2,
        "stop" => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory_item(soul_key: &str, snapshot_id: &str, internal_id: &str) -> InventoryItem {
        InventoryItem {
            profile_id: "profile-test".to_owned(),
            soul_key: soul_key.to_owned(),
            snapshot_id: snapshot_id.to_owned(),
            soul_internal_id: internal_id.to_owned(),
            first_seen_snapshot_id: snapshot_id.to_owned(),
            last_seen_snapshot_id: snapshot_id.to_owned(),
            presence_state: "present".to_owned(),
            updated_at: "2026-08-24T00:00:00Z".to_owned(),
        }
    }

    fn soul(
        snapshot_id: &str,
        internal_id: &str,
        quality: u8,
        level: u8,
        initial_substat_count: Option<u8>,
    ) -> SnapshotSoul {
        SnapshotSoul {
            snapshot_id: snapshot_id.to_owned(),
            internal_id: internal_id.to_owned(),
            source_stable_id: Some(internal_id.to_owned()),
            identity_quality: "stable".to_owned(),
            set_id: "set-test".to_owned(),
            slot: 2,
            quality,
            level,
            main_attr_type: "speed".to_owned(),
            main_attr_value: 12.0,
            initial_substat_count,
            locked_in_source: Some(false),
            equipped_state: None,
            source_json: None,
        }
    }

    fn attribute(
        snapshot_id: &str,
        internal_id: &str,
        index: u8,
        attribute_type: &str,
        value: f64,
        fixed_attribute: bool,
    ) -> SoulAttribute {
        SoulAttribute {
            snapshot_id: snapshot_id.to_owned(),
            soul_internal_id: internal_id.to_owned(),
            attribute_index: index,
            attribute_type: attribute_type.to_owned(),
            value,
            enhancement_count: Some(0),
            count_provenance: "source".to_owned(),
            fixed_attribute,
        }
    }

    #[test]
    fn 只纳入六星零级四腿并为每条可强化副属性建立独立路径() {
        let inventory = vec![
            inventory_item("embryo", "snapshot", "embryo"),
            inventory_item("plus15", "snapshot", "plus15"),
            inventory_item("three-leg", "snapshot", "three-leg"),
        ];
        let souls = vec![
            soul("snapshot", "embryo", 6, 0, Some(4)),
            soul("snapshot", "plus15", 6, 15, Some(4)),
            soul("snapshot", "three-leg", 6, 0, Some(3)),
        ];
        let attributes = vec![
            attribute("snapshot", "embryo", 0, "speed", 3.0, false),
            attribute("snapshot", "embryo", 1, "crit_rate", 0.03, false),
            attribute("snapshot", "embryo", 2, "attack_flat", 100.0, true),
            attribute("snapshot", "plus15", 0, "speed", 17.5, false),
            attribute("snapshot", "three-leg", 0, "speed", 3.0, false),
        ];

        let report = analyze_four_leg_embryo_decision(&inventory, &souls, &attributes);
        assert_eq!(report.candidate_count, 1);
        assert_eq!(report.path_count, 2);
        assert_eq!(report.paths[0].comparison_source, "average-threshold");
        assert!(
            report
                .paths
                .iter()
                .any(|path| path.attribute_type == "speed")
        );
        assert!(
            report
                .paths
                .iter()
                .any(|path| path.attribute_type == "crit_rate")
        );
    }

    #[test]
    fn 同类对照按套装号位主属性和目标属性筛选并只保留前三枚() {
        let mut inventory = vec![inventory_item("embryo", "snapshot", "embryo")];
        let mut souls = vec![soul("snapshot", "embryo", 6, 0, Some(4))];
        let mut attributes = vec![attribute("snapshot", "embryo", 0, "speed", 3.0, false)];
        for index in 0..4 {
            let internal_id = format!("plus15-{index}");
            inventory.push(inventory_item(&internal_id, "snapshot", &internal_id));
            souls.push(soul("snapshot", &internal_id, 6, 15, Some(4)));
            attributes.push(attribute(
                "snapshot",
                &internal_id,
                0,
                "speed",
                10.0 + f64::from(index),
                false,
            ));
        }

        let report = analyze_four_leg_embryo_decision(&inventory, &souls, &attributes);
        let path = &report.paths[0];
        assert_eq!(path.comparison_source, "inventory");
        assert_eq!(path.comparison_value, Some(13.0));
        assert_eq!(path.comparison_candidate_count, 4);
        assert_eq!(path.comparison_items.len(), 3);
        assert_eq!(path.comparison_items[0].soul_key, "plus15-3");
    }

    #[test]
    fn 无对照门槛和小数比较不依赖四舍五入文本() {
        let inventory = vec![inventory_item("embryo", "snapshot", "embryo")];
        let souls = vec![soul("snapshot", "embryo", 6, 0, Some(4))];
        let attributes = vec![attribute("snapshot", "embryo", 0, "crit_rate", 0.03, false)];

        let report = analyze_four_leg_embryo_decision(&inventory, &souls, &attributes);
        let path = &report.paths[0];
        assert_eq!(path.comparison_value, Some(0.162));
        assert!((path.usual_expected.unwrap() - 0.06375).abs() < 1e-12);
        assert!((path.theoretical_upper.unwrap() - 0.18).abs() < 1e-12);
        assert_eq!(path.conclusion, "high-risk");
        assert!((path.hit_probability.unwrap() - (1.0 / 1024.0)).abs() < 1e-15);
    }

    #[test]
    fn 三档结论按理论上限和通常期望分层并按结论优先级排序() {
        let inventory = vec![
            inventory_item("embryo", "snapshot", "embryo"),
            inventory_item("weak-plus15", "snapshot", "weak-plus15"),
        ];
        let souls = vec![
            soul("snapshot", "embryo", 6, 0, Some(4)),
            soul("snapshot", "weak-plus15", 6, 15, Some(4)),
        ];
        let attributes = vec![
            attribute("snapshot", "embryo", 0, "speed", 3.0, false),
            attribute("snapshot", "embryo", 1, "crit_rate", 0.03, false),
            attribute("snapshot", "embryo", 2, "effect_hit", 0.0, false),
            attribute("snapshot", "weak-plus15", 0, "crit_rate", 0.04, false),
        ];

        let report = analyze_four_leg_embryo_decision(&inventory, &souls, &attributes);
        assert_eq!(report.paths[0].conclusion, "worth-it");
        assert_eq!(report.paths[1].conclusion, "high-risk");
        assert_eq!(report.paths[2].conclusion, "stop");
    }

    #[test]
    fn 腿数选择只计算勾选的候选并为三腿新增类别建立三条父路径() {
        let inventory = vec![
            inventory_item("three-leg", "snapshot", "three-leg"),
            inventory_item("four-leg", "snapshot", "four-leg"),
        ];
        let souls = vec![
            soul("snapshot", "three-leg", 6, 0, Some(3)),
            soul("snapshot", "four-leg", 6, 0, Some(4)),
        ];
        let attributes = vec![
            attribute("snapshot", "three-leg", 0, "speed", 3.0, false),
            attribute("snapshot", "four-leg", 0, "speed", 3.0, false),
        ];

        let three_only = analyze_embryo_decision(&inventory, &souls, &attributes, true, false);
        assert_eq!(three_only.selected_leg_counts, vec![3]);
        assert_eq!(three_only.candidate_count, 1);
        assert_eq!(three_only.paths.len(), 4);
        assert!(three_only.paths.iter().all(|path| path.leg_count == 3));
        assert_eq!(
            three_only
                .paths
                .iter()
                .filter(|path| path.path_kind == "new-category")
                .count(),
            3
        );

        let four_only = analyze_embryo_decision(&inventory, &souls, &attributes, false, true);
        assert_eq!(four_only.selected_leg_counts, vec![4]);
        assert_eq!(four_only.candidate_count, 1);
        assert!(four_only.paths.iter().all(|path| path.leg_count == 4));
        assert!(
            four_only
                .paths
                .iter()
                .all(|path| path.path_kind == "existing-attribute")
        );
    }

    #[test]
    fn 三腿新增类别保留类别概率并明确具体属性概率和通常期望未知() {
        let inventory = vec![
            inventory_item("three-leg", "snapshot", "three-leg"),
            inventory_item("plus15", "snapshot", "plus15"),
        ];
        let souls = vec![
            soul("snapshot", "three-leg", 6, 0, Some(3)),
            soul("snapshot", "plus15", 6, 15, Some(4)),
        ];
        let attributes = vec![
            attribute("snapshot", "three-leg", 0, "speed", 3.0, false),
            attribute("snapshot", "plus15", 0, "crit_rate", 0.12, false),
        ];

        let report = analyze_embryo_decision(&inventory, &souls, &attributes, true, false);
        let categories = report
            .paths
            .iter()
            .filter(|path| path.path_kind == "new-category")
            .collect::<Vec<_>>();
        assert_eq!(categories.len(), 3);
        let probability_sum = categories
            .iter()
            .map(|path| path.category_probability.unwrap())
            .sum::<f64>();
        assert!((probability_sum - 1.0).abs() < 1e-12);
        assert_eq!(
            categories
                .iter()
                .find(|path| path.attribute_type == "new_attack")
                .unwrap()
                .category_probability,
            Some(0.36)
        );
        let crit_rate_branch = categories
            .iter()
            .find(|path| path.attribute_type == "new_attack")
            .unwrap()
            .possible_attributes
            .iter()
            .find(|branch| branch.attribute_type == "crit_rate")
            .unwrap();
        assert_eq!(crit_rate_branch.usual_expected, None);
        assert_eq!(crit_rate_branch.hit_probability, None);
        assert_eq!(crit_rate_branch.comparison_value, Some(0.12));
        assert!((crit_rate_branch.theoretical_upper.unwrap() - 0.15).abs() < 1e-12);

        let existing = report
            .paths
            .iter()
            .find(|path| path.path_kind == "existing-attribute")
            .unwrap();
        assert_eq!(existing.hit_probability, Some(1.0 / 256.0));
        assert!((existing.theoretical_upper.unwrap() - 15.0).abs() < 1e-12);
        assert!((existing.usual_expected.unwrap() - 5.7).abs() < 1e-12);
    }
}
