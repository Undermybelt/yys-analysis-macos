//! 分析待办、用户决定与行动批次的领域契约。
//!
//! 该模块只描述可持久化的业务事实和公共查询边界，不负责 SQLite 查询或界面状态。
//! 系统建议保存在分析待办中，用户决定单独保存；因此规则重算可以替换前者，
//! 但不能静默覆盖后者。

use crate::application::error::AppError;
use crate::domain::rules::StandardScoreContribution;
use serde::{Deserialize, Serialize};

/// 待办分类，值是前后端之间稳定的筛选协议。
pub const TODO_CATEGORIES: &[&str] = &[
    "new_embryo",
    "continue",
    "await_review",
    "stop",
    "cleanup",
    "conflict",
    "uncertain",
    "changed",
];

/// 用户决定允许的稳定值。
pub const DECISION_KINDS: &[&str] = &[
    "keep",
    "observe",
    "plan_strengthen",
    "plan_recycle",
    "ignore",
];

/// 强化或清理批次允许的稳定值。
pub const BATCH_KINDS: &[&str] = &["strengthen", "cleanup"];

/// 待办分页查询参数；分页在 Rust/SQLite 侧完成，避免前端加载一万枚库存后再筛选。
#[derive(Clone, Debug, Default)]
pub struct TodoQuery<'a> {
    pub category: Option<&'a str>,
    pub search: Option<&'a str>,
    /// 分析中心用途下钻使用精确用途 ID，避免 LIKE 子串匹配造成图表与列表计数漂移。
    pub use_id: Option<&'a str>,
    pub limit: u32,
    pub offset: u32,
}

/// 当前分析待办；`detail_json` 保存完整事实、逐用途评估和解释树。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisTodo {
    pub id: String,
    pub profile_id: String,
    pub soul_key: String,
    pub snapshot_id: String,
    pub soul_internal_id: String,
    pub set_id: String,
    pub slot: u8,
    pub quality: u8,
    pub level: u8,
    pub main_attribute: String,
    pub main_value: f64,
    /// 重算时直接落库的标准评分，便于分页筛选和列表摘要读取。
    pub standard_score: Option<f64>,
    /// 当前综合评分结果；页面暂不展示，但先保留在分析结果中供后续功能使用。
    pub composite_score: Option<f64>,
    pub category: String,
    pub recommendation: String,
    pub reason_summary: String,
    pub evidence_level: String,
    pub data_quality: String,
    pub presence_state: String,
    pub is_new: bool,
    pub is_changed: bool,
    pub detail_json: String,
    pub explanation_hash: String,
    pub generated_at: String,
    pub revision: u32,
    pub user_decision: Option<String>,
    pub user_decision_note: Option<String>,
    pub decision_revision: Option<u32>,
}

/// 分页待办结果；`total` 用于界面显示筛选后的总数和分页器。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisTodoPage {
    pub items: Vec<AnalysisTodo>,
    pub total: u32,
    pub limit: u32,
    pub offset: u32,
}

/// “我的御魂”只需要读取评分摘要，不把完整解释树重复搬到列表接口。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoulScoreSummary {
    pub soul_key: String,
    pub standard_score: Option<f64>,
    pub standard_score_rule: Option<String>,
    pub standard_score_formula: Option<String>,
    /// 标准评分的逐属性贡献；列表悬浮详情据此区分核心属性和一般属性。
    pub standard_score_contributions: Vec<StandardScoreContribution>,
    pub composite_score: Option<f64>,
    pub best_score: Option<f64>,
    pub best_use_id: Option<String>,
    pub best_use_title: Option<String>,
    pub effective_growth_count: Option<f64>,
    pub scored_use_count: u32,
    pub recommendation: String,
    pub category: String,
    pub generated_at: String,
    pub standard_id: Option<String>,
    pub standard_version: Option<String>,
    pub standard_title: Option<String>,
    /// 实际参与本次评分的规范规则正文哈希；同名同版本正文变化时仍可准确判旧。
    pub standard_hash: Option<String>,
}

/// 单个用户决定变化；`decision = None` 表示清除我的决定。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionChange {
    pub soul_key: String,
    pub decision: Option<String>,
    pub note: Option<String>,
}

/// 用户决定写入前的预览统计。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionPreview {
    pub selected_count: u32,
    pub added_count: u32,
    pub overwritten_count: u32,
    pub skipped_count: u32,
    pub protected_count: u32,
    pub conflict_count: u32,
}

/// 一次决定事务的结果；同一 operation ID 可被撤销。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionApplyResult {
    pub operation_id: String,
    pub added_count: u32,
    pub overwritten_count: u32,
    pub skipped_count: u32,
    pub protected_count: u32,
    pub conflict_count: u32,
}

/// 用户决定历史条目。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionHistoryEntry {
    pub id: String,
    pub operation_id: String,
    pub profile_id: String,
    pub soul_key: String,
    pub previous_decision: Option<String>,
    pub next_decision: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
}

/// 创建行动批次的请求；批次只引用本地待办，不写回游戏。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBatchRequest {
    pub profile_id: String,
    pub kind: String,
    pub target_level: Option<u8>,
    pub soul_keys: Vec<String>,
}

/// 强化/清理批次中的分组，分组键对应游戏内最少筛选切换字段。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchGroup {
    pub group_key: String,
    pub set_id: String,
    pub slot: u8,
    pub main_attribute: String,
    pub level: u8,
    pub item_count: u32,
}

/// 批次条目的状态。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItem {
    pub batch_id: String,
    pub soul_key: String,
    pub group_key: String,
    pub set_id: String,
    pub slot: u8,
    pub main_attribute: String,
    pub level: u8,
    pub status: String,
    pub sort_order: u32,
    pub note: Option<String>,
    pub completed_at: Option<String>,
}

/// 行动批次摘要和分组；详情命令再返回具体条目。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBatch {
    pub id: String,
    pub profile_id: String,
    pub kind: String,
    pub status: String,
    pub target_level: Option<u8>,
    pub snapshot_id: Option<String>,
    pub group_count: u32,
    pub item_count: u32,
    pub completed_count: u32,
    pub skipped_count: u32,
    pub not_found_count: u32,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub groups: Vec<BatchGroup>,
}

/// 批次详情。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBatchDetail {
    pub batch: ActionBatch,
    pub items: Vec<BatchItem>,
}

/// 分析、决定和批次的统一持久化边界。
pub trait ActionRepository: Send + Sync {
    /// 以当前御魂稳定键更新分析结果，并保留独立的用户决定表。
    fn upsert_todos(&self, todos: &[AnalysisTodo]) -> Result<(), AppError>;

    /// 根据库存投影同步存在状态；只有完整快照产生的 `removed` 才能显示为确认消失。
    fn sync_todo_presence(&self, profile_id: &str) -> Result<(), AppError>;

    /// 按分类、搜索关键字和分页读取待办，并返回筛选总数。
    fn list_todos(
        &self,
        profile_id: &str,
        query: &TodoQuery<'_>,
    ) -> Result<AnalysisTodoPage, AppError>;

    /// 读取当前库存的轻量评分摘要；完整解释仍通过分析待办详情按需读取。
    fn list_score_summaries(&self, profile_id: &str) -> Result<Vec<SoulScoreSummary>, AppError>;

    /// 只读取指定分页御魂的评分摘要，避免列表打开时解析整份分析待办详情。
    fn list_score_summaries_by_keys(
        &self,
        profile_id: &str,
        soul_keys: &[String],
    ) -> Result<Vec<SoulScoreSummary>, AppError>;

    /// 统计当前档案已经生成的评分数量；按钮状态只需要这个轻量计数。
    fn count_score_summaries(&self, profile_id: &str) -> Result<u32, AppError>;

    /// 读取某枚御魂的当前用户决定历史。
    fn list_decision_history(
        &self,
        profile_id: &str,
        soul_key: &str,
    ) -> Result<Vec<DecisionHistoryEntry>, AppError>;

    /// 预览决定变化；`respect_protection` 为真时不覆盖保护项。
    fn preview_decisions(
        &self,
        profile_id: &str,
        changes: &[DecisionChange],
        respect_protection: bool,
    ) -> Result<DecisionPreview, AppError>;

    /// 在一个事务中应用批量决定并写入操作历史。
    fn apply_decisions(
        &self,
        profile_id: &str,
        operation_id: &str,
        changes: &[DecisionChange],
        respect_protection: bool,
    ) -> Result<DecisionApplyResult, AppError>;

    /// 按 operation ID 撤销最近一次决定事务。
    fn undo_decision(&self, operation_id: &str) -> Result<(), AppError>;

    /// 创建强化或清理批次；仓库负责批次和条目原子写入。
    fn create_batch(&self, batch: &ActionBatch, items: &[BatchItem]) -> Result<(), AppError>;

    /// 列出档案批次，最近创建的排在前面。
    fn list_batches(&self, profile_id: &str) -> Result<Vec<ActionBatch>, AppError>;

    /// 读取批次详情。
    fn get_batch(
        &self,
        profile_id: &str,
        batch_id: &str,
    ) -> Result<Option<ActionBatchDetail>, AppError>;

    /// 标记批次条目状态；完成态由仓库统一重新汇总。
    fn update_batch_item(
        &self,
        profile_id: &str,
        batch_id: &str,
        soul_key: &str,
        status: &str,
        note: Option<&str>,
    ) -> Result<ActionBatchDetail, AppError>;
}

/// 校验外部请求中的稳定枚举值，拒绝未知值而不是静默降级。
pub fn validate_action_value(field: &str, value: &str, allowed: &[&str]) -> Result<(), AppError> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(AppError::invalid_argument(
            field,
            format!("不支持的值：{value}"),
        ))
    }
}

/// 判断系统建议是否保护该御魂免受默认清理批量操作影响。
pub fn is_protected_todo(todo: &AnalysisTodo) -> bool {
    todo.recommendation == "keep"
        || todo.recommendation == "observe"
        || todo.category == "conflict"
        || todo.data_quality != "complete"
        || todo.evidence_level == "draft"
}

/// 以套装、号位、主属性和当前等级构造稳定的游戏内筛选分组键。
pub fn batch_group_key(set_id: &str, slot: u8, main_attribute: &str, level: u8) -> String {
    format!("{set_id}|{slot}|{main_attribute}|{level}")
}
