//! 神奇海螺应用用例。
//!
//! 该用例只负责读取当前数据、目录和当前库存，并把不同 Adapter 提供的事实合并后
//! 交给领域模块计算；它不写入数据库，也不改变用户决定或分析待办。

use crate::application::error::AppError;
use crate::application::services::AppServices;
use crate::domain::{
    MiracleConchRequest, MiracleConchResult, MiracleSoul, calculate_miracle_conch,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// 目标面板配装用例；所有复杂组合规则隐藏在领域模块的单一接口后。
pub struct MiracleConchUseCase {
    services: Arc<AppServices>,
}

impl MiracleConchUseCase {
    /// 注入应用服务集合，保证单元测试和正式运行都使用相同的仓库 seam。
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 解析应用唯一的当前库存，再执行一次纯内存目标面板搜索。
    pub fn calculate(&self, request: MiracleConchRequest) -> Result<MiracleConchResult, AppError> {
        // 计算命令是一次同步调用；记录每个边界的耗时和数量，避免前端停在 94% 时只能猜测后端阶段。
        let calculation_started_at = Instant::now();
        tracing::info!(
            target: "miracle_conch",
            shikigami_id = %request.shikigami_id,
            only_max_level = request.only_max_level,
            recipe_lines = request.recipe.len(),
            metric_count = request.metrics.len(),
            "神奇海螺计算开始"
        );
        // 库存事实始终来自当前激活角色，与页面切换同步。
        let profile_id = self
            .services
            .active_profile_id()
            .ok_or_else(|| AppError::not_found("activeProfile", "尚未导入任何角色数据，请先导入"))?;
        self.services
            .profiles
            .get(&profile_id)?
            .ok_or_else(|| AppError::not_found("current_inventory", &profile_id))?;

        let catalog_version = self
            .services
            .catalog
            .active_status()?
            .ok_or_else(|| AppError::internal("尚未安装御魂与式神目录"))?
            .version;
        let sets = self.services.catalog.list_sets(&catalog_version)?;
        let shikigami = self
            .services
            .catalog
            .list_shikigami(&catalog_version)?
            .into_iter()
            .find(|entry| entry.shikigami_id == request.shikigami_id)
            .ok_or_else(|| AppError::not_found("shikigami", &request.shikigami_id))?;

        let (inventory, snapshot_souls, attributes, excluded_unconfirmed_count) =
            self.services.snapshots.list_inventory_facts(&profile_id)?;
        tracing::info!(
            target: "miracle_conch",
            inventory_rows = inventory.len(),
            snapshot_souls = snapshot_souls.len(),
            attribute_rows = attributes.len(),
            excluded_unconfirmed_count,
            elapsed_ms = calculation_started_at.elapsed().as_millis() as u64,
            "神奇海螺库存事实读取完成"
        );
        let attributes_by_soul = attributes.into_iter().fold(
            HashMap::<(String, String), Vec<_>>::new(),
            |mut grouped, attribute| {
                grouped
                    .entry((
                        attribute.snapshot_id.clone(),
                        attribute.soul_internal_id.clone(),
                    ))
                    .or_default()
                    .push(attribute);
                grouped
            },
        );

        let mut miracle_inventory = Vec::new();
        let mut excluded_non_six_star_count = 0_u32;
        for item in inventory {
            if item.presence_state != "present" {
                continue;
            }
            let soul = snapshot_souls
                .iter()
                .find(|soul| {
                    soul.snapshot_id == item.snapshot_id
                        && soul.internal_id == item.soul_internal_id
                })
                .cloned()
                .ok_or_else(|| {
                    AppError::internal(format!("当前库存缺少御魂事实：{}", item.soul_key))
                })?;
            if soul.quality != 6 {
                excluded_non_six_star_count += 1;
            }
            let soul_attributes = attributes_by_soul
                .get(&(soul.snapshot_id.clone(), soul.internal_id.clone()))
                .cloned()
                .unwrap_or_default();
            miracle_inventory.push(MiracleSoul {
                soul,
                attributes: soul_attributes,
            });
        }

        tracing::info!(
            target: "miracle_conch",
            miracle_inventory = miracle_inventory.len(),
            excluded_non_six_star_count,
            elapsed_ms = calculation_started_at.elapsed().as_millis() as u64,
            "神奇海螺库存转换完成，开始领域搜索"
        );
        let result = calculate_miracle_conch(
            &request,
            &shikigami,
            &sets,
            &miracle_inventory,
            excluded_unconfirmed_count,
            excluded_non_six_star_count,
        );
        match &result {
            Ok(result) => tracing::info!(
                target: "miracle_conch",
                status = %result.status,
                search_complete = result.search_complete,
                candidate_count = result.candidate_count,
                solution_count = result.solutions.len(),
                enhancement_count = result.enhancement_candidates.len(),
                elapsed_ms = calculation_started_at.elapsed().as_millis() as u64,
                "神奇海螺计算完成"
            ),
            Err(error) => tracing::error!(
                target: "miracle_conch",
                error = %error,
                elapsed_ms = calculation_started_at.elapsed().as_millis() as u64,
                "神奇海螺领域计算失败"
            ),
        }
        result.map_err(|message| AppError::invalid_argument("miracleConch", message))
    }
}
