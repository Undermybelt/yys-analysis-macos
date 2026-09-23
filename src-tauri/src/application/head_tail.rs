//! 头尾分析用例：从当前库存中筛选满足五手速度条件的全部头和尾，并保存派生结果。

use crate::application::error::AppError;
use crate::application::services::AppServices;
use crate::domain::{HeadTailAttribute, HeadTailCache, HeadTailCard, SnapshotSoul, SoulAttribute};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

const SPEED_ATTRIBUTE: &str = "speed";
const HEAD_SLOT: u8 = 2;
const TAIL_SLOT: u8 = 4;
const HEAD_MAIN_ATTRIBUTE: &str = "speed";
const TAIL_HIT_ATTRIBUTE: &str = "effect_hit";
const TAIL_RESIST_ATTRIBUTE: &str = "effect_resist";
const REQUIRED_SPEED_ROLLS: u8 = 5;

/// 头尾计算用例；进入页面只读取 SQLite 缓存，点击按钮时才扫描当前库存事实。
pub struct HeadTailUseCase {
    services: Arc<AppServices>,
}

impl HeadTailUseCase {
    /// 创建头尾用例并复用应用级仓库，避免界面层直接访问 SQLite。
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 读取已保存的头尾结果；结果是否过期由返回值中的当前库存版本交给前端判断。
    pub fn get(&self, profile_id: &str) -> Result<Option<HeadTailCache>, AppError> {
        self.services.head_tail.get(profile_id)
    }

    /// 计算并保存全部头尾：头限定 2 号位速度主属性，尾限定 4 号位命中/抵抗主属性，
    /// 两者都必须存在满足五手口径的速度副属性；结果按速度倒序，并以等级、星级和稳定键打破同速并列。
    pub fn calculate(&self, profile_id: &str) -> Result<HeadTailCache, AppError> {
        let (inventory, souls, attributes, _excluded_unconfirmed_count) =
            self.services.snapshots.list_inventory_facts(profile_id)?;
        let (_current_inventory_count, inventory_revision) = self
            .services
            .head_tail
            .current_inventory_state(profile_id)?;

        // 先把库存投影键映射到快照事实键，确保结果能追溯到当前库存中的稳定 soulKey。
        let inventory_keys = inventory
            .into_iter()
            .map(|item| ((item.snapshot_id, item.soul_internal_id), item.soul_key))
            .collect::<HashMap<_, _>>();

        // 按快照 ID 和御魂内部 ID 建索引，避免遍历候选御魂时反复扫描所有副属性。
        let mut attributes_by_soul: HashMap<(String, String), Vec<&SoulAttribute>> = HashMap::new();
        for attribute in &attributes {
            attributes_by_soul
                .entry((
                    attribute.snapshot_id.clone(),
                    attribute.soul_internal_id.clone(),
                ))
                .or_default()
                .push(attribute);
        }

        let mut head_candidate_count = 0u32;
        let mut tail_candidate_count = 0u32;
        let mut heads = Vec::new();
        let mut tails = Vec::new();

        for soul in &souls {
            let key = (soul.snapshot_id.clone(), soul.internal_id.clone());
            let Some(soul_attributes) = attributes_by_soul.get(&key) else {
                continue;
            };
            let Some(soul_key) = inventory_keys.get(&key) else {
                continue;
            };

            let is_head = soul.slot == HEAD_SLOT && soul.main_attr_type == HEAD_MAIN_ATTRIBUTE;
            let is_tail = soul.slot == TAIL_SLOT
                && matches!(
                    soul.main_attr_type.as_str(),
                    TAIL_HIT_ATTRIBUTE | TAIL_RESIST_ATTRIBUTE
                );
            if !is_head && !is_tail {
                continue;
            }

            let Some(card) = build_card(soul, soul_key, soul_attributes) else {
                continue;
            };

            if is_head {
                head_candidate_count += 1;
                heads.push(card);
            } else {
                tail_candidate_count += 1;
                tails.push(card);
            }
        }

        // 先在后端固定排序，前端只负责分组和视觉呈现，避免页面刷新或数据库行序改变卡片顺序。
        heads.sort_by(compare_cards_desc);
        tails.sort_by(compare_cards_desc);

        let calculated_at = AppServices::now_iso();
        self.services.head_tail.replace(
            profile_id,
            &calculated_at,
            &inventory_revision,
            souls.len() as u32,
            &heads,
            &tails,
            head_candidate_count,
            tail_candidate_count,
        )?;

        self.services
            .head_tail
            .get(profile_id)?
            .ok_or_else(|| AppError::internal("头尾计算结果保存后无法读取"))
    }
}

/// 判断速度副属性是否满足五手候选口径：只有数据源明确记录 5 次强化才放行。
/// 次数未知时不能从速度数值反推出“5 次都强化速度”这一事实，因此必须排除。
fn qualifies_as_five_roll_speed(attribute: &SoulAttribute) -> bool {
    if attribute.fixed_attribute || attribute.attribute_type != SPEED_ATTRIBUTE {
        return false;
    }

    match attribute.enhancement_count {
        Some(enhancement_count) => enhancement_count == REQUIRED_SPEED_ROLLS,
        None => false,
    }
}

/// 将库存事实转换成卡片所需的完整副属性；没有满足五手口径的速度时不构成头尾候选。
fn build_card(
    soul: &SnapshotSoul,
    soul_key: &str,
    attributes: &[&SoulAttribute],
) -> Option<HeadTailCard> {
    let speed_attribute = attributes
        .iter()
        .filter(|attribute| qualifies_as_five_roll_speed(attribute))
        .max_by(|left, right| {
            left.value
                .partial_cmp(&right.value)
                .unwrap_or(Ordering::Equal)
        })?;

    let attributes = attributes
        .iter()
        .map(|attribute| HeadTailAttribute {
            attribute_type: attribute.attribute_type.clone(),
            value: attribute.value,
            enhancement_count: attribute.enhancement_count,
            fixed_attribute: attribute.fixed_attribute,
        })
        .collect();

    Some(HeadTailCard {
        soul_key: soul_key.to_owned(),
        set_id: soul.set_id.clone(),
        slot: soul.slot,
        quality: soul.quality,
        level: soul.level,
        main_attr_type: soul.main_attr_type.clone(),
        main_attr_value: soul.main_attr_value,
        speed: speed_attribute.value,
        speed_rolls: REQUIRED_SPEED_ROLLS,
        attributes,
    })
}

/// 按展示顺序比较头尾候选：速度、等级、星级倒序，稳定键正序，确保同值候选可复现。
fn compare_cards_desc(left: &HeadTailCard, right: &HeadTailCard) -> Ordering {
    right
        .speed
        .partial_cmp(&left.speed)
        .unwrap_or(Ordering::Equal)
        .then_with(|| right.level.cmp(&left.level))
        .then_with(|| right.quality.cmp(&left.quality))
        .then_with(|| left.soul_key.cmp(&right.soul_key))
}

/// 判断候选是否应排在当前卡片之前，供业务比较测试复用。
#[cfg(test)]
fn is_better_card(candidate: &HeadTailCard, current: &HeadTailCard) -> bool {
    compare_cards_desc(candidate, current) == Ordering::Less
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造只用于筛选测试的快照御魂，业务测试只关心号位和主属性约束。
    fn soul(slot: u8, main_attr_type: &str) -> SnapshotSoul {
        SnapshotSoul {
            snapshot_id: "snapshot-test".to_owned(),
            internal_id: "soul-test".to_owned(),
            source_stable_id: Some("stable-test".to_owned()),
            identity_quality: "stable".to_owned(),
            set_id: "set-test".to_owned(),
            slot,
            quality: 6,
            level: 15,
            main_attr_type: main_attr_type.to_owned(),
            main_attr_value: 0.55,
            initial_substat_count: Some(4),
            locked_in_source: Some(false),
            equipped_state: Some("bag".to_owned()),
            source_json: None,
        }
    }

    /// 构造速度副属性，覆盖固定属性和明确强化次数边界。
    fn speed_attribute(
        value: f64,
        enhancement_count: Option<u8>,
        fixed_attribute: bool,
    ) -> SoulAttribute {
        SoulAttribute {
            snapshot_id: "snapshot-test".to_owned(),
            soul_internal_id: "soul-test".to_owned(),
            attribute_index: 0,
            attribute_type: SPEED_ATTRIBUTE.to_owned(),
            value,
            enhancement_count,
            count_provenance: "source".to_owned(),
            fixed_attribute,
        }
    }

    /// 构造一条非速度副属性，用于验证“其他属性存在时不能凭速度值推断五手”。
    fn other_attribute(attribute_type: &str, value: f64) -> SoulAttribute {
        SoulAttribute {
            snapshot_id: "snapshot-test".to_owned(),
            soul_internal_id: "soul-test".to_owned(),
            attribute_index: 1,
            attribute_type: attribute_type.to_owned(),
            value,
            enhancement_count: None,
            count_provenance: "source".to_owned(),
            fixed_attribute: false,
        }
    }

    #[test]
    fn 强化次数未知时不能仅凭十四点速度判为五手头() {
        let head = soul(HEAD_SLOT, HEAD_MAIN_ATTRIBUTE);
        let speed = speed_attribute(14.25, None, false);
        let life_rate = other_attribute("hp_rate", 0.0276);

        // 五手速度的业务含义是五次强化都落在速度；次数未知时不能从 14.25 反推这个事实。
        assert!(build_card(&head, "head-unknown-count", &[&life_rate, &speed]).is_none());
    }

    #[test]
    fn 头尾候选按五手速度规则且排除固定速度() {
        let head = soul(HEAD_SLOT, HEAD_MAIN_ATTRIBUTE);
        let four_rolls = speed_attribute(13.6, Some(4), false);
        let fixed_five_rolls = speed_attribute(13.6, Some(REQUIRED_SPEED_ROLLS), true);
        let five_rolls = speed_attribute(17.2, Some(REQUIRED_SPEED_ROLLS), false);

        assert!(build_card(&head, "head-four", &[&four_rolls]).is_none());
        assert!(build_card(&head, "head-fixed", &[&fixed_five_rolls]).is_none());
        let card = build_card(&head, "head-five", &[&five_rolls]).expect("五手速度应成为候选");
        assert_eq!(card.speed, 17.2);
        assert_eq!(card.speed_rolls, REQUIRED_SPEED_ROLLS);
    }

    #[test]
    fn 未提供强化次数时不能推断为五手速度候选() {
        let head = soul(HEAD_SLOT, HEAD_MAIN_ATTRIBUTE);
        let just_above_minimum = speed_attribute(14.01, None, false);
        let below_minimum = speed_attribute(13.99, None, false);
        let unknown_count_speed = speed_attribute(16.76, None, false);

        assert!(build_card(&head, "head-just-above-minimum", &[&just_above_minimum]).is_none());
        assert!(build_card(&head, "head-below-minimum", &[&below_minimum]).is_none());
        assert!(build_card(&head, "head-unknown-count", &[&unknown_count_speed]).is_none());
    }

    #[test]
    fn 全部头尾候选按速度倒序排列() {
        let head = soul(HEAD_SLOT, HEAD_MAIN_ATTRIBUTE);
        let speeds = [16.76, 17.51, 14.01];
        let mut cards = speeds
            .iter()
            .enumerate()
            .map(|(index, speed)| {
                let attribute = speed_attribute(*speed, Some(REQUIRED_SPEED_ROLLS), false);
                build_card(&head, &format!("head-{index}"), &[&attribute]).expect("构造候选")
            })
            .collect::<Vec<_>>();

        cards.sort_by(compare_cards_desc);

        assert_eq!(
            cards.iter().map(|card| card.speed).collect::<Vec<_>>(),
            vec![17.51, 16.76, 14.01]
        );
    }

    #[test]
    fn 同速度候选按等级星级再按稳定键确定性排序() {
        let head = soul(HEAD_SLOT, HEAD_MAIN_ATTRIBUTE);
        let speed = speed_attribute(17.2, Some(REQUIRED_SPEED_ROLLS), false);
        let mut higher_level = build_card(&head, "head-z", &[&speed]).expect("构造候选");
        higher_level.level = 14;
        let mut lower_level = higher_level.clone();
        lower_level.soul_key = "head-a".to_owned();
        lower_level.level = 13;

        assert!(is_better_card(&higher_level, &lower_level));
        assert!(!is_better_card(&lower_level, &higher_level));

        lower_level.level = higher_level.level;
        assert!(is_better_card(&lower_level, &higher_level));
    }
}
