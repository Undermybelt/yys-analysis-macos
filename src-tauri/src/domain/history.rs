//! 历史追溯与分析中心的领域契约。
//!
//! 这些结构只描述可被前端观察的事实和指标，不携带 SQL、文件系统或 Tauri 细节。
//! 分析中心的每个指标都保留可逆下钻信息，避免把统计数字与库存筛选割裂。

use crate::application::error::AppError;
use serde::Serialize;

use super::{AcquisitionEvent, AnalysisTodo, Snapshot};

/// 快照时间线中的一项，包含当前基线和原始对象完整性状态。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotHistoryItem {
    pub snapshot: Snapshot,
    pub soul_count: u32,
    pub inventory_reference_count: u32,
    pub is_current_baseline: bool,
    pub integrity_status: String,
    pub integrity_message: Option<String>,
}

/// 档案的快照与采集事件历史。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryOverview {
    pub profile_id: String,
    pub current_baseline_snapshot_id: Option<String>,
    pub current_completeness: Option<String>,
    pub can_confirm_removals: bool,
    pub snapshots: Vec<SnapshotHistoryItem>,
    pub acquisition_events: Vec<AcquisitionEvent>,
}

/// 用途覆盖指标；分类不一致时不强加分类筛选，避免下钻漏掉同一用途的其他待办。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCoverageMetric {
    pub use_id: String,
    pub title: String,
    pub usable_count: u32,
    pub observing_count: u32,
    pub gap_count: u32,
    pub drill_down_category: Option<String>,
    pub drill_down_search: String,
}

/// 属性分布指标。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributeDistributionMetric {
    pub attribute_type: String,
    pub count: u32,
}

/// 套装结构指标，保留 PVE/PVP 常用度标签。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetStructureMetric {
    pub set_id: String,
    pub count: u32,
    pub pve_common: bool,
    pub pvp_common: bool,
}

/// 强化漏斗单阶段指标。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnhancementFunnelStage {
    pub stage: String,
    pub count: u32,
    pub drill_down_category: Option<String>,
}

/// 清理收益及其确认状态。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupYield {
    pub candidate_count: u32,
    pub confirmed_removed_count: u32,
    pub unconfirmed_count: u32,
    pub pending_count: u32,
}

/// 数据质量指标；所有数量均来自当前档案，不跨档案汇总。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataQualitySummary {
    pub current_baseline_complete: bool,
    pub unknown_enhancement_count: u32,
    pub draft_rule_count: u32,
    pub stale_analysis_count: u32,
    pub affected_snapshot_count: u32,
}

/// 分析中心总览。图表点击时优先使用其中的分类或搜索键回到库存/待办。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisCenter {
    pub profile_id: String,
    pub current_baseline_snapshot_id: Option<String>,
    pub inventory_count: u32,
    pub present_count: u32,
    pub unconfirmed_count: u32,
    pub removed_count: u32,
    pub usage_coverage: Vec<UsageCoverageMetric>,
    pub attribute_distribution: Vec<AttributeDistributionMetric>,
    pub set_structure: Vec<SetStructureMetric>,
    pub enhancement_funnel: Vec<EnhancementFunnelStage>,
    pub cleanup_yield: CleanupYield,
    pub data_quality: DataQualitySummary,
}

/// 单枚御魂的追溯视图，保持分析待办、事实快照和规则版本之间的链路。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisTrace {
    pub profile_id: String,
    pub soul_key: String,
    pub todo: AnalysisTodo,
    pub snapshot: Snapshot,
    pub raw_sha256: String,
    pub catalog_version: String,
    pub maturity_level: Option<String>,
    pub rule_versions: Vec<RuleTraceVersion>,
    pub integrity_status: String,
    pub integrity_message: Option<String>,
}

/// 追溯链中的规则版本摘要；正文仍由规则库按需读取。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleTraceVersion {
    pub id: String,
    pub version: String,
    pub title: String,
    pub normalized_hash: String,
    pub enabled: bool,
}

/// 当前库存中一枚御魂的查询事实，用于指标计算和导出，避免前端重算数据库关联。
#[derive(Clone, Debug)]
pub struct InventoryAnalysisRow {
    pub soul_key: String,
    pub snapshot_id: String,
    pub soul_internal_id: String,
    pub set_id: String,
    pub slot: u8,
    pub level: u8,
    pub initial_substat_count: Option<u8>,
    pub attribute_types: Vec<String>,
    pub presence_state: String,
    pub snapshot_created_at: String,
    pub snapshot_completeness: String,
}

/// 分析待办的轻量查询行，不重复把大解释树加载到每个统计分支。
#[derive(Clone, Debug)]
pub struct AnalysisMetricRow {
    pub soul_key: String,
    pub snapshot_id: String,
    pub category: String,
    pub recommendation: String,
    pub data_quality: String,
    pub evidence_level: String,
    pub level: u8,
    pub presence_state: String,
    pub generated_at: String,
    pub detail_json: String,
}

/// 档案引用的原始对象及其影响快照。
#[derive(Clone, Debug)]
pub struct ProfileRawReference {
    pub sha256: String,
    pub snapshot_ids: Vec<String>,
}

/// 原始对象校验结果；损坏和缺失对象均保留受影响快照列表。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectIntegrityReport {
    pub sha256: String,
    pub status: String,
    pub message: Option<String>,
    pub affected_snapshot_ids: Vec<String>,
    pub repair_suggestion: String,
}

/// 一次档案原始对象校验的汇总。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileObjectVerification {
    pub profile_id: String,
    pub checked_count: u32,
    pub healthy_count: u32,
    pub missing_count: u32,
    pub corrupt_count: u32,
    pub reports: Vec<ObjectIntegrityReport>,
}

/// 受档案隔离约束的导出结果；前端只负责让用户保存这段已生成的内容。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPayload {
    pub file_name: String,
    pub media_type: String,
    pub content: String,
    pub profile_id: String,
    pub snapshot_id: Option<String>,
    pub record_count: u32,
}

/// 历史分析仓库契约；实现只负责读取/记录历史事实，不承担指标业务规则。
pub trait HistoryAnalysisRepository: Send + Sync {
    /// 读取档案快照时间线及快照完整性标记。
    fn list_snapshot_history(
        &self,
        profile_id: &str,
        current_snapshot_id: Option<&str>,
    ) -> Result<Vec<SnapshotHistoryItem>, AppError>;

    /// 读取当前库存指标所需的规范化事实。
    fn list_inventory_analysis_rows(
        &self,
        profile_id: &str,
    ) -> Result<Vec<InventoryAnalysisRow>, AppError>;

    /// 读取当前档案全部待办的轻量解释数据。
    fn list_analysis_metric_rows(
        &self,
        profile_id: &str,
    ) -> Result<Vec<AnalysisMetricRow>, AppError>;

    /// 列出档案引用的原始对象及受影响快照。
    fn list_profile_raw_references(
        &self,
        profile_id: &str,
    ) -> Result<Vec<ProfileRawReference>, AppError>;

    /// 写入单个快照的对象完整性状态，供历史页持续标记异常来源。
    fn record_snapshot_integrity(
        &self,
        snapshot_id: &str,
        status: &str,
        message: Option<&str>,
        checked_at: &str,
    ) -> Result<(), AppError>;
}
