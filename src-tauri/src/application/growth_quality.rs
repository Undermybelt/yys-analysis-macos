//! 成品成长质量用例：读取当前档案快照事实并委托领域模型计算可解释结果。

use crate::application::error::AppError;
use crate::application::services::AppServices;
use crate::domain::{GrowthQualityReport, analyze_plus15_growth_quality};
use std::sync::Arc;

/// 成品成长质量用例；界面层只通过该用例访问快照查询和领域分析。
pub struct GrowthQualityUseCase {
    services: Arc<AppServices>,
}

impl GrowthQualityUseCase {
    /// 创建用例并复用应用级服务集合，避免 Tauri 命令直接编排数据库事实。
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 分析指定档案的六星 +15 成品成长质量；没有激活档案时返回空报告。
    ///
    /// 快照事实只读加载一次，随后交给领域函数完成筛选、归一化、加权和排序，
    /// 因此应用层不复制成长模型规则，也不写入库存或派生缓存。
    pub fn analyze(&self, profile_id: Option<&str>) -> Result<GrowthQualityReport, AppError> {
        let Some(profile_id) = profile_id else {
            return Ok(analyze_plus15_growth_quality(&[], &[], &[]));
        };
        let (inventory, souls, attributes, excluded_unconfirmed_count) =
            self.services.snapshots.list_inventory_facts(profile_id)?;
        let mut report = analyze_plus15_growth_quality(&inventory, &souls, &attributes);
        // 局部快照中的未确认御魂不是“低分”，必须把排除数量传给页面保持数据不确定性可见。
        report.excluded_unconfirmed_count = excluded_unconfirmed_count;
        Ok(report)
    }
}
