//! 御魂雷达聚合用例：把当前库存事实压缩为“套装 + 号位 + 指标”的最高副属性。

use crate::application::error::AppError;
use crate::application::services::AppServices;
use crate::domain::{SOUL_RADAR_METRIC_TYPES, SoulAttribute, SoulRadarCache, SoulRadarPoint};
use std::collections::HashMap;
use std::sync::Arc;

/// 雷达计算用例；计算动作显式触发，页面进入只读取已保存的派生结果。
pub struct SoulRadarUseCase {
    services: Arc<AppServices>,
}

impl SoulRadarUseCase {
    /// 创建雷达用例并复用应用级仓库，避免页面层直接访问数据库。
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 读取缓存及其当前库存版本；该路径不会加载全部御魂属性事实。
    pub fn get(&self, profile_id: &str) -> Result<Option<SoulRadarCache>, AppError> {
        self.services.radar.get(profile_id)
    }

    /// 读取当前档案全部已确认御魂，计算每套每号位每项副属性最高值并保存结果。
    /// 固定属性不计入副属性最高值；同值时保留先读到的记录，保证结果稳定且不引入额外排序规则。
    pub fn calculate(&self, profile_id: &str) -> Result<SoulRadarCache, AppError> {
        let (inventory, souls, attributes, _excluded_unconfirmed_count) =
            self.services.snapshots.list_inventory_facts(profile_id)?;
        let (_current_inventory_count, inventory_revision) =
            self.services.radar.current_inventory_state(profile_id)?;

        // 先按快照 ID 和御魂内部 ID 建索引，避免每个御魂重复扫描整份属性数组。
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

        let mut highest_points: HashMap<(String, u8, String), SoulRadarPoint> = HashMap::new();
        for soul in &souls {
            let key = (soul.snapshot_id.clone(), soul.internal_id.clone());
            let Some(soul_attributes) = attributes_by_soul.get(&key) else {
                continue;
            };

            for attribute in soul_attributes {
                // 固定属性属于御魂基础属性，不应被当作成长副属性参与雷达峰值。
                if attribute.fixed_attribute
                    || !SOUL_RADAR_METRIC_TYPES.contains(&attribute.attribute_type.as_str())
                {
                    continue;
                }

                let point_key = (
                    soul.set_id.clone(),
                    soul.slot,
                    attribute.attribute_type.clone(),
                );
                let candidate = SoulRadarPoint {
                    set_id: soul.set_id.clone(),
                    slot: soul.slot,
                    metric_type: attribute.attribute_type.clone(),
                    value: attribute.value,
                    main_attr_type: Some(soul.main_attr_type.clone()),
                };
                let should_replace = highest_points
                    .get(&point_key)
                    .map(|current| candidate.value > current.value)
                    .unwrap_or(true);
                if should_replace {
                    highest_points.insert(point_key, candidate);
                }
            }
        }

        let mut points: Vec<SoulRadarPoint> = highest_points.into_values().collect();
        // 持久化前排序，让数据库读取、前端渲染和调试导出都保持固定顺序。
        points.sort_by(|left, right| {
            left.set_id
                .cmp(&right.set_id)
                .then_with(|| left.slot.cmp(&right.slot))
                .then_with(|| left.metric_type.cmp(&right.metric_type))
        });

        let calculated_at = AppServices::now_iso();
        self.services.radar.replace(
            profile_id,
            &calculated_at,
            &inventory_revision,
            inventory.len() as u32,
            &points,
        )?;

        self.services
            .radar
            .get(profile_id)?
            .ok_or_else(|| AppError::internal("雷达计算结果保存后无法读取"))
    }
}
