//! 四腿胚子强化决策用例：读取当前档案快照事实并委托领域模型计算。

use crate::application::error::AppError;
use crate::application::services::AppServices;
use crate::domain::{EmbryoDecisionReport, analyze_embryo_decision};
use std::sync::Arc;

/// 四腿胚子决策用例；界面层只通过该用例访问当前档案的只读事实。
pub struct EmbryoDecisionUseCase {
    services: Arc<AppServices>,
}

impl EmbryoDecisionUseCase {
    /// 创建用例并复用应用级服务集合，避免 Tauri 命令直接编排数据库读取。
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 分析当前档案的六星 `+0` 四腿胚子；没有激活档案时返回空报告。
    pub fn analyze(&self, profile_id: Option<&str>) -> Result<EmbryoDecisionReport, AppError> {
        self.analyze_with_legs(profile_id, false, true)
    }

    /// 按界面勾选的腿数分析六星 `+0` 胚子；未勾选的腿数不会进入领域计算。
    pub fn analyze_with_legs(
        &self,
        profile_id: Option<&str>,
        include_three_leg: bool,
        include_four_leg: bool,
    ) -> Result<EmbryoDecisionReport, AppError> {
        let Some(profile_id) = profile_id else {
            return Ok(analyze_embryo_decision(
                &[],
                &[],
                &[],
                include_three_leg,
                include_four_leg,
            ));
        };
        let (inventory, souls, attributes, excluded_unconfirmed_count) =
            self.services.snapshots.list_inventory_facts(profile_id)?;
        let mut report = analyze_embryo_decision(
            &inventory,
            &souls,
            &attributes,
            include_three_leg,
            include_four_leg,
        );
        // 局部快照中的未确认御魂不是“停止强化”，需要把排除数量传给页面保持不确定性可见。
        report.excluded_unconfirmed_count = excluded_unconfirmed_count;
        Ok(report)
    }
}
