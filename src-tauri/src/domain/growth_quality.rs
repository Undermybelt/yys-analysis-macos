//! 六星 +15 御魂成品成长质量分析。
//!
//! 该模块只处理副属性实际成长值的归一化，不读取用途模板、评分规则或式神资料。
//! 计算输入来自快照事实，输出保持精确小数，展示层不得用四舍五入后的文本回算。

use super::{InventoryItem, SnapshotSoul, SoulAttribute};
use serde::Serialize;
use std::collections::HashMap;

/// 成长质量模型版本；该模型属于社区经验，不代表官方强化规则或官方档位概率。
pub const GROWTH_QUALITY_MODEL_VERSION: &str = "community-growth-quality-v1";
/// 成长质量模型的依据状态；与强化机制的官方概率依据保持独立。
pub const GROWTH_QUALITY_EVIDENCE_STATUS: &str = "community-experience";
/// 六星 +15 御魂的初始一手加五次强化，共六手。
pub const PLUS15_HAND_COUNT: u8 = 6;

/// 成长质量固定档位的下界；上界采用左闭右开，极高档位没有上限。
pub const GROWTH_QUALITY_TIER_BOUNDARIES: [f64; 4] = [60.0, 70.0, 80.0, 90.0];

/// 单手成长质量分析所需的最小模型信息。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthQualityTierBoundary {
    pub label: String,
    pub min_score: f64,
    pub max_score: Option<f64>,
}

/// 单条可强化副属性的成长质量结果；固定攻击、防御、生命不会进入该列表。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthQualityAttribute {
    pub attribute_type: String,
    pub attribute_label: String,
    /// 保留数据库中的原始精确值；比例属性仍以 0～1 保存。
    pub value: f64,
    /// 强化新增手数；缺少来源事实时保持未知。
    pub enhancement_count: Option<u8>,
    /// 初始一手加新增手数后的实际成长手数。
    pub hand_count: Option<u8>,
    /// 将该属性的平均单手成长归一化到 0～100；缺少精确手数时保持未知。
    pub single_roll_quality: Option<f64>,
}

/// 单枚六星 +15 御魂的成品成长质量结果。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthQualitySoul {
    pub soul_key: String,
    pub set_id: String,
    pub slot: u8,
    pub quality: u8,
    pub level: u8,
    pub main_attr_type: String,
    pub main_attr_value: f64,
    pub attributes: Vec<GrowthQualityAttribute>,
    /// 所有可强化副属性实际成长手数等权后的总成长分。
    pub total_growth_score: Option<f64>,
    /// 总成长分对应的固定档位；数据不完整时保持未知。
    pub tier: Option<String>,
    /// 无法计算时说明缺失的事实，避免页面把未知误显示成低分。
    pub score_note: Option<String>,
}

/// 当前游戏档案的六星 +15 成品成长质量报告。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthQualityReport {
    pub model_version: String,
    pub evidence_status: String,
    pub model_note: String,
    pub hand_count: u8,
    pub tier_boundaries: Vec<GrowthQualityTierBoundary>,
    pub candidate_count: u32,
    pub scoreable_count: u32,
    pub unknown_data_count: u32,
    /// 当前档案中因局部快照语义而未确认、未纳入分析的库存数量。
    pub excluded_unconfirmed_count: u32,
    pub items: Vec<GrowthQualitySoul>,
}

/// 计算当前库存中六星 +15 御魂的副属性成长质量，并按总成长分降序排列。
///
/// 先按库存稳定键关联快照事实，再排除固定属性；每条可强化属性使用“最终精确值 ÷ 实际手数”
/// 得到平均单手成长，按对应区间归一化后以实际手数等权合并。任一可强化属性缺少精确强化手数
/// 或缺少成长基准时，该枚御魂保留在结果中但总分和档位显示未知。
pub fn analyze_plus15_growth_quality(
    inventory: &[InventoryItem],
    souls: &[SnapshotSoul],
    attributes: &[SoulAttribute],
) -> GrowthQualityReport {
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

    let mut items = inventory
        .iter()
        .filter(|item| item.presence_state == "present")
        .filter_map(|item| {
            let soul = souls_by_identity
                .get(&(item.snapshot_id.clone(), item.soul_internal_id.clone()))?;
            if soul.quality != 6 || soul.level != 15 {
                return None;
            }
            let soul_attributes = attributes_by_identity
                .get(&(soul.snapshot_id.clone(), soul.internal_id.clone()))
                .cloned()
                .unwrap_or_default();
            Some(build_soul_result(item, soul, &soul_attributes))
        })
        .collect::<Vec<_>>();

    // 分数未知的御魂排在可比较结果之后；稳定键保证同分排序可复现。
    items.sort_by(
        |left, right| match (left.total_growth_score, right.total_growth_score) {
            (Some(left_score), Some(right_score)) => right_score
                .total_cmp(&left_score)
                .then_with(|| left.soul_key.cmp(&right.soul_key)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.soul_key.cmp(&right.soul_key),
        },
    );

    let scoreable_count = items
        .iter()
        .filter(|item| item.total_growth_score.is_some())
        .count() as u32;
    let candidate_count = items.len() as u32;
    GrowthQualityReport {
        model_version: GROWTH_QUALITY_MODEL_VERSION.to_owned(),
        evidence_status: GROWTH_QUALITY_EVIDENCE_STATUS.to_owned(),
        model_note: "社区经验模型（档位边界为 60/70/80/90）：每条可强化副属性按实际成长手数等权，固定属性不计入；档位为本地比较标签，不代表官方概率。".to_owned(),
        hand_count: PLUS15_HAND_COUNT,
        tier_boundaries: growth_quality_tier_boundaries(),
        candidate_count,
        scoreable_count,
        unknown_data_count: candidate_count.saturating_sub(scoreable_count),
        excluded_unconfirmed_count: 0,
        items,
    }
}

/// 构造单枚御魂结果；保持未知强化次数，不把缺失事实伪装成零手或满六手。
fn build_soul_result(
    item: &InventoryItem,
    soul: &SnapshotSoul,
    attributes: &[&SoulAttribute],
) -> GrowthQualitySoul {
    let mut result_attributes = Vec::new();
    let mut total_weighted_quality = 0.0;
    let mut total_hands = 0_u32;
    let mut has_unknown_data = false;
    let mut has_unmodeled_attribute = false;

    // 适配器已把御魂固有的固定攻防生命标记为 fixed_attribute；不能仅按属性类型排除，
    // 因为同名平坦属性也可能是可强化副属性，必须保留数据源的固定性事实。
    for attribute in attributes
        .iter()
        .filter(|attribute| !attribute.fixed_attribute)
    {
        let benchmark = benchmark_range(&attribute.attribute_type);
        let hand_count = attribute
            .enhancement_count
            .filter(|enhancement_count| *enhancement_count <= PLUS15_HAND_COUNT - 1)
            .map(|enhancement_count| enhancement_count + 1);
        let single_roll_quality = match (benchmark, hand_count) {
            (Some((minimum, maximum)), Some(hand_count)) if attribute.value.is_finite() => {
                let average_roll = attribute.value / f64::from(hand_count);
                let quality = normalize_roll_quality(average_roll, minimum, maximum);
                total_hands += u32::from(hand_count);
                total_weighted_quality += f64::from(hand_count) * quality;
                Some(quality)
            }
            (Some(_), _) => {
                // 已纳入成长模型的属性缺少精确手数时，不能用默认手数推断总分。
                has_unknown_data = true;
                None
            }
            (None, _) => {
                // 平坦攻击、防御、生命暂未定义本模型区间；展示事实但不污染其他属性的总分。
                has_unmodeled_attribute = true;
                None
            }
        };
        result_attributes.push(GrowthQualityAttribute {
            attribute_type: attribute.attribute_type.clone(),
            attribute_label: attribute_label(&attribute.attribute_type).to_owned(),
            value: attribute.value,
            enhancement_count: attribute.enhancement_count,
            hand_count,
            single_roll_quality,
        });
    }

    let total_growth_score = if !has_unknown_data && total_hands > 0 {
        Some(total_weighted_quality / f64::from(total_hands))
    } else {
        None
    };
    let tier = total_growth_score.map(growth_quality_tier);
    let score_note = if total_growth_score.is_none() {
        Some(
            if has_unknown_data {
                "缺少精确强化手数，暂不计算总成长分。"
            } else if has_unmodeled_attribute {
                "当前副属性没有本模型成长基准，暂不计算总成长分。"
            } else {
                "当前没有可评分的成长副属性，暂不计算总成长分。"
            }
            .to_owned(),
        )
    } else {
        None
    };

    GrowthQualitySoul {
        soul_key: item.soul_key.clone(),
        set_id: soul.set_id.clone(),
        slot: soul.slot,
        quality: soul.quality,
        level: soul.level,
        main_attr_type: soul.main_attr_type.clone(),
        main_attr_value: soul.main_attr_value,
        attributes: result_attributes,
        total_growth_score,
        tier,
        score_note,
    }
}

/// 将平均单手成长按属性基准归一化到 0～100，并把越界数据限制在可展示范围内。
fn normalize_roll_quality(value: f64, minimum: f64, maximum: f64) -> f64 {
    ((value - minimum) / (maximum - minimum) * 100.0).clamp(0.0, 100.0)
}

/// 返回八种可强化副属性的经验成长区间；速度使用点数，其余比例属性使用 0～1。
fn benchmark_range(attribute_type: &str) -> Option<(f64, f64)> {
    match attribute_type {
        "speed" => Some((2.4, 3.0)),
        "crit_rate" | "attack_rate" | "hp_rate" | "defense_rate" => Some((0.024, 0.03)),
        "crit_damage" | "effect_hit" | "effect_resist" => Some((0.032, 0.04)),
        _ => None,
    }
}

/// 将固定档位边界转换为前端可直接展示的结构，明确上界是否包含在下一档。
fn growth_quality_tier_boundaries() -> Vec<GrowthQualityTierBoundary> {
    vec![
        GrowthQualityTierBoundary {
            label: "偏低".to_owned(),
            min_score: 0.0,
            max_score: Some(GROWTH_QUALITY_TIER_BOUNDARIES[0]),
        },
        GrowthQualityTierBoundary {
            label: "普通".to_owned(),
            min_score: GROWTH_QUALITY_TIER_BOUNDARIES[0],
            max_score: Some(GROWTH_QUALITY_TIER_BOUNDARIES[1]),
        },
        GrowthQualityTierBoundary {
            label: "良好".to_owned(),
            min_score: GROWTH_QUALITY_TIER_BOUNDARIES[1],
            max_score: Some(GROWTH_QUALITY_TIER_BOUNDARIES[2]),
        },
        GrowthQualityTierBoundary {
            label: "优秀".to_owned(),
            min_score: GROWTH_QUALITY_TIER_BOUNDARIES[2],
            max_score: Some(GROWTH_QUALITY_TIER_BOUNDARIES[3]),
        },
        GrowthQualityTierBoundary {
            label: "极高".to_owned(),
            min_score: GROWTH_QUALITY_TIER_BOUNDARIES[3],
            max_score: None,
        },
    ]
}

/// 根据左闭右开的固定边界返回成长质量档位。
fn growth_quality_tier(score: f64) -> String {
    match score {
        score if score < GROWTH_QUALITY_TIER_BOUNDARIES[0] => "偏低",
        score if score < GROWTH_QUALITY_TIER_BOUNDARIES[1] => "普通",
        score if score < GROWTH_QUALITY_TIER_BOUNDARIES[2] => "良好",
        score if score < GROWTH_QUALITY_TIER_BOUNDARIES[3] => "优秀",
        _ => "极高",
    }
    .to_owned()
}

/// 统一属性显示名；未知属性保留稳定键，方便追溯目录或导入适配器数据。
fn attribute_label(attribute_type: &str) -> &str {
    match attribute_type {
        "attack_flat" => "攻击",
        "defense_flat" => "防御",
        "hp_flat" => "生命",
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

    fn soul(snapshot_id: &str, internal_id: &str, quality: u8, level: u8) -> SnapshotSoul {
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
            initial_substat_count: Some(4),
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
        enhancement_count: Option<u8>,
        fixed_attribute: bool,
    ) -> SoulAttribute {
        SoulAttribute {
            snapshot_id: snapshot_id.to_owned(),
            soul_internal_id: internal_id.to_owned(),
            attribute_index: index,
            attribute_type: attribute_type.to_owned(),
            value,
            enhancement_count,
            count_provenance: if enhancement_count.is_some() {
                "source".to_owned()
            } else {
                "unknown".to_owned()
            },
            fixed_attribute,
        }
    }

    #[test]
    fn 成长区间按属性类别归一化到零到一百() {
        let inventory = vec![inventory_item("soul-mid", "snapshot", "mid")];
        let souls = vec![soul("snapshot", "mid", 6, 15)];
        let attributes = vec![attribute(
            "snapshot",
            "mid",
            0,
            "speed",
            2.7 * 6.0,
            Some(5),
            false,
        )];

        let report = analyze_plus15_growth_quality(&inventory, &souls, &attributes);
        let item = &report.items[0];
        assert!((item.attributes[0].single_roll_quality.unwrap() - 50.0).abs() < 1e-10);
        assert!((item.total_growth_score.unwrap() - 50.0).abs() < 1e-10);
        assert_eq!(item.tier.as_deref(), Some("偏低"));
    }

    #[test]
    fn 成长区间越界时裁剪到零到一百并保留原始值() {
        let inventory = vec![inventory_item("soul-clamped", "snapshot", "clamped")];
        let souls = vec![soul("snapshot", "clamped", 6, 15)];
        let attributes = vec![attribute(
            "snapshot",
            "clamped",
            0,
            "speed",
            18.000001,
            Some(5),
            false,
        )];

        let report = analyze_plus15_growth_quality(&inventory, &souls, &attributes);
        let attribute_result = &report.items[0].attributes[0];
        assert_eq!(attribute_result.value, 18.000001);
        assert_eq!(attribute_result.single_roll_quality, Some(100.0));

        let attributes = vec![attribute(
            "snapshot",
            "clamped",
            0,
            "speed",
            12.0,
            Some(5),
            false,
        )];
        let report = analyze_plus15_growth_quality(&inventory, &souls, &attributes);
        assert_eq!(report.items[0].attributes[0].single_roll_quality, Some(0.0));
    }

    #[test]
    fn 八种可强化副属性的成长区间固定且分为两类() {
        for attribute_type in [
            "speed",
            "crit_rate",
            "attack_rate",
            "hp_rate",
            "defense_rate",
        ] {
            let (minimum, maximum) = benchmark_range(attribute_type).expect("第一类属性基准");
            if attribute_type == "speed" {
                assert_eq!((minimum, maximum), (2.4, 3.0));
            } else {
                assert_eq!((minimum, maximum), (0.024, 0.03));
            }
        }
        for attribute_type in ["crit_damage", "effect_hit", "effect_resist"] {
            assert_eq!(benchmark_range(attribute_type), Some((0.032, 0.04)));
        }
    }

    #[test]
    fn 六手计算和固定属性排除都可复核() {
        let inventory = vec![inventory_item("soul-six", "snapshot", "six")];
        let souls = vec![soul("snapshot", "six", 6, 15)];
        let attributes = vec![
            attribute("snapshot", "six", 0, "speed", 3.0 * 6.0, Some(5), false),
            attribute("snapshot", "six", 1, "attack_flat", 100.0, Some(5), true),
            attribute("snapshot", "six", 2, "defense_flat", 100.0, Some(5), true),
            attribute("snapshot", "six", 3, "hp_flat", 100.0, Some(5), true),
        ];

        let report = analyze_plus15_growth_quality(&inventory, &souls, &attributes);
        assert_eq!(report.hand_count, 6);
        assert_eq!(report.items[0].attributes.len(), 1);
        assert_eq!(report.items[0].attributes[0].hand_count, Some(6));
        assert_eq!(report.items[0].total_growth_score, Some(100.0));
    }

    #[test]
    fn 未建模平坦副属性保留实际手数但不污染其他属性总分() {
        let inventory = vec![inventory_item("soul-flat", "snapshot", "flat")];
        let souls = vec![soul("snapshot", "flat", 6, 15)];
        let attributes = vec![
            attribute("snapshot", "flat", 0, "speed", 3.0 * 6.0, Some(5), false),
            attribute("snapshot", "flat", 1, "attack_flat", 162.0, Some(5), false),
        ];

        let report = analyze_plus15_growth_quality(&inventory, &souls, &attributes);
        let flat = &report.items[0].attributes[1];
        assert_eq!(flat.hand_count, Some(6));
        assert_eq!(flat.single_roll_quality, None);
        assert_eq!(report.items[0].total_growth_score, Some(100.0));
    }

    #[test]
    fn 总成长分按实际成长手数等权平均() {
        let inventory = vec![inventory_item("soul-weighted", "snapshot", "weighted")];
        let souls = vec![soul("snapshot", "weighted", 6, 15)];
        let attributes = vec![
            attribute(
                "snapshot",
                "weighted",
                0,
                "speed",
                2.4 * 6.0,
                Some(5),
                false,
            ),
            attribute(
                "snapshot",
                "weighted",
                1,
                "crit_damage",
                0.04,
                Some(0),
                false,
            ),
        ];

        let report = analyze_plus15_growth_quality(&inventory, &souls, &attributes);
        // 速度 0 分占 6 手，暴伤 100 分占 1 手，总分为 100 / 7。
        assert!((report.items[0].total_growth_score.unwrap() - (100.0 / 7.0)).abs() < 1e-10);
    }

    #[test]
    fn 档位边界采用左闭右开并覆盖五档() {
        assert_eq!(growth_quality_tier(0.0), "偏低");
        assert_eq!(growth_quality_tier(59.999), "偏低");
        assert_eq!(growth_quality_tier(60.0), "普通");
        assert_eq!(growth_quality_tier(70.0), "良好");
        assert_eq!(growth_quality_tier(80.0), "优秀");
        assert_eq!(growth_quality_tier(90.0), "极高");
    }

    #[test]
    fn 只纳入六星加十五并按总成长分降序排序() {
        let mut removed_item = inventory_item("soul-removed", "snapshot", "removed");
        removed_item.presence_state = "removed".to_owned();
        let inventory = vec![
            inventory_item("soul-low", "snapshot", "low"),
            inventory_item("soul-high", "snapshot", "high"),
            inventory_item("soul-not-plus15", "snapshot", "not-plus15"),
            inventory_item("soul-not-six-star", "snapshot", "not-six-star"),
            removed_item,
        ];
        let souls = vec![
            soul("snapshot", "low", 6, 15),
            soul("snapshot", "high", 6, 15),
            soul("snapshot", "not-plus15", 6, 12),
            soul("snapshot", "not-six-star", 5, 15),
            soul("snapshot", "removed", 6, 15),
        ];
        let attributes = vec![
            attribute("snapshot", "low", 0, "speed", 2.4 * 6.0, Some(5), false),
            attribute("snapshot", "high", 0, "speed", 3.0 * 6.0, Some(5), false),
            attribute(
                "snapshot",
                "not-plus15",
                0,
                "speed",
                3.0 * 6.0,
                Some(5),
                false,
            ),
            attribute(
                "snapshot",
                "not-six-star",
                0,
                "speed",
                3.0 * 6.0,
                Some(5),
                false,
            ),
            attribute("snapshot", "removed", 0, "speed", 3.0 * 6.0, Some(5), false),
        ];

        let report = analyze_plus15_growth_quality(&inventory, &souls, &attributes);
        assert_eq!(report.candidate_count, 2);
        assert_eq!(report.items[0].soul_key, "soul-high");
        assert_eq!(report.items[1].soul_key, "soul-low");
    }

    #[test]
    fn 未知强化手数不伪造总成长分() {
        let inventory = vec![inventory_item("soul-unknown", "snapshot", "unknown")];
        let souls = vec![soul("snapshot", "unknown", 6, 15)];
        let attributes = vec![attribute(
            "snapshot", "unknown", 0, "speed", 18.0, None, false,
        )];

        let report = analyze_plus15_growth_quality(&inventory, &souls, &attributes);
        assert_eq!(report.unknown_data_count, 1);
        assert_eq!(report.items[0].total_growth_score, None);
        assert_eq!(report.items[0].attributes[0].hand_count, None);
    }
}
