//! 前后端共享的传输对象，统一使用 camelCase 序列化。
//!
//! 每个 DTO 与其对应领域实体保持独立，避免序列化注解污染领域模型。

use crate::application::cbg_read::{CbgReadPreview, CbgSoulPreview};
use crate::application::character_archive_transfer::{
    CharacterArchiveExportRequest, CharacterArchiveExportResult, CharacterArchiveImportRequest,
    CharacterArchiveImportResult,
};
use crate::application::character_archives::CharacterArchiveScanResult;
use crate::application::guild::GuildOverview;
use crate::application::rule_use_cases::{RuleImpactPreview, RulePresetPreview, RuleVersionView};
use crate::application::shikigami_shard_query::ShikigamiShardLookupResult;
use crate::application::simulation::{SimulateEnhancementInput, SimulateEnhancementResult};
use crate::application::updates::{PackageType, UpdateChannel, UpdateSourceMode};
use crate::domain::{
    AcquisitionEvent, ActionBatch, ActionBatchDetail, AnalysisCenter, AnalysisTodo,
    AnalysisTodoPage, AnalysisTrace, BackupRecord, BatchItem, CatalogEvidence, CatalogStatus,
    DecisionApplyResult, DecisionChange, DecisionHistoryEntry, DecisionPreview,
    EffectiveCommonness, EmbryoComparisonItem, EmbryoDecisionPath, EmbryoDecisionReport,
    EmbryoPossibleAttributePath,
    ExportPayload, GameProfile, GrowthQualityAttribute, GrowthQualityReport, GrowthQualitySoul,
    GrowthQualityTierBoundary, HeadTailAttribute, HeadTailCache, HeadTailCard,
    HistoryOverview, InstalledPackage, InventoryFilterOptions, InventoryItem, MaturityThreshold,
    MiracleConchRequest, MiracleConchResult, OwnedShikigami, OwnedShikigamiSkill,
    ProfileObjectVerification, RawObject, Shikigami, ShikigamiBagGroup, ShikigamiShard, Snapshot,
    SoulFacts, SoulRadarCache, SoulRadarPoint, SoulSet, StandardScoreContribution,
};
use serde::{Deserialize, Serialize};

/// 神奇海螺请求和结果沿用领域模型的稳定 camelCase 序列化契约，不在接口层复制嵌套结构。
pub type MiracleConchRequestDto = MiracleConchRequest;
pub type MiracleConchResultDto = MiracleConchResult;

/// 胚子同类 `+15` 对照项；保留目标属性的精确小数值。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbryoComparisonItemDto {
    pub soul_key: String,
    pub value: f64,
}

/// 新增属性类别下的具体可能分支；未知概率和通常期望保持为空。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbryoPossibleAttributePathDto {
    pub attribute_type: String,
    pub attribute_label: String,
    pub theoretical_upper: Option<f64>,
    pub usual_expected: Option<f64>,
    pub hit_probability: Option<f64>,
    pub comparison_value: Option<f64>,
    pub comparison_source: String,
    pub comparison_items: Vec<EmbryoComparisonItemDto>,
    pub comparison_candidate_count: u32,
    pub theoretical_excess: Option<f64>,
    pub usual_excess: Option<f64>,
    pub conclusion: Option<String>,
    pub data_note: String,
}

/// 三腿新增类别下的具体属性分支；类别概率只在父路径展示。
impl From<EmbryoPossibleAttributePath> for EmbryoPossibleAttributePathDto {
    /// 将新增属性分支映射为独立接口对象，不把未知概率填成零。
    fn from(value: EmbryoPossibleAttributePath) -> Self {
        Self {
            attribute_type: value.attribute_type,
            attribute_label: value.attribute_label,
            theoretical_upper: value.theoretical_upper,
            usual_expected: value.usual_expected,
            hit_probability: value.hit_probability,
            comparison_value: value.comparison_value,
            comparison_source: value.comparison_source,
            comparison_items: value.comparison_items.into_iter().map(Into::into).collect(),
            comparison_candidate_count: value.comparison_candidate_count,
            theoretical_excess: value.theoretical_excess,
            usual_excess: value.usual_excess,
            conclusion: value.conclusion,
            data_note: value.data_note,
        }
    }
}

/// 三腿或四腿胚子的一条独立强化路径；接口保留结论和未四舍五入的比较差值。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbryoDecisionPathDto {
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
    pub theoretical_upper: Option<f64>,
    pub usual_expected: Option<f64>,
    pub hit_probability: Option<f64>,
    pub comparison_value: Option<f64>,
    pub comparison_source: String,
    pub comparison_items: Vec<EmbryoComparisonItemDto>,
    pub comparison_candidate_count: u32,
    pub theoretical_excess: Option<f64>,
    pub usual_excess: Option<f64>,
    pub conclusion: String,
    pub possible_attributes: Vec<EmbryoPossibleAttributePathDto>,
    pub category_probability: Option<f64>,
    pub data_note: String,
}

/// 当前档案所选腿数的 `+0` 胚子强化决策报告。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbryoDecisionReportDto {
    pub model_version: String,
    pub evidence_status: String,
    pub model_note: String,
    pub remaining_hand_count: u8,
    pub selected_leg_counts: Vec<u8>,
    pub candidate_count: u32,
    pub path_count: u32,
    pub excluded_unconfirmed_count: u32,
    pub paths: Vec<EmbryoDecisionPathDto>,
}

impl From<EmbryoComparisonItem> for EmbryoComparisonItemDto {
    /// 将同类对照项映射为前端展示对象，不损失原始精确值。
    fn from(value: EmbryoComparisonItem) -> Self {
        Self {
            soul_key: value.soul_key,
            value: value.value,
        }
    }
}

impl From<EmbryoDecisionPath> for EmbryoDecisionPathDto {
    /// 将单条胚子路径逐字段映射为稳定的 camelCase 接口契约。
    fn from(value: EmbryoDecisionPath) -> Self {
        Self {
            path_id: value.path_id,
            soul_key: value.soul_key,
            set_id: value.set_id,
            slot: value.slot,
            quality: value.quality,
            level: value.level,
            main_attr_type: value.main_attr_type,
            leg_count: value.leg_count,
            path_kind: value.path_kind,
            attribute_index: value.attribute_index,
            attribute_type: value.attribute_type,
            attribute_label: value.attribute_label,
            initial_value: value.initial_value,
            theoretical_upper: value.theoretical_upper,
            usual_expected: value.usual_expected,
            hit_probability: value.hit_probability,
            comparison_value: value.comparison_value,
            comparison_source: value.comparison_source,
            comparison_items: value.comparison_items.into_iter().map(Into::into).collect(),
            comparison_candidate_count: value.comparison_candidate_count,
            theoretical_excess: value.theoretical_excess,
            usual_excess: value.usual_excess,
            conclusion: value.conclusion,
            possible_attributes: value
                .possible_attributes
                .into_iter()
                .map(Into::into)
                .collect(),
            category_probability: value.category_probability,
            data_note: value.data_note,
        }
    }
}

impl From<EmbryoDecisionReport> for EmbryoDecisionReportDto {
    /// 将完整胚子决策报告映射为独立的前后端传输结构。
    fn from(value: EmbryoDecisionReport) -> Self {
        Self {
            model_version: value.model_version,
            evidence_status: value.evidence_status,
            model_note: value.model_note,
            remaining_hand_count: value.remaining_hand_count,
            selected_leg_counts: value.selected_leg_counts,
            candidate_count: value.candidate_count,
            path_count: value.path_count,
            excluded_unconfirmed_count: value.excluded_unconfirmed_count,
            paths: value.paths.into_iter().map(Into::into).collect(),
        }
    }
}

/// 胚子分析的腿数选择；默认只分析四腿，三腿必须由用户主动勾选。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbryoDecisionRequestDto {
    #[serde(default)]
    pub include_three_leg: bool,
    #[serde(default = "default_include_four_leg")]
    pub include_four_leg: bool,
}

fn default_include_four_leg() -> bool {
    true
}

impl Default for EmbryoDecisionRequestDto {
    fn default() -> Self {
        Self {
            include_three_leg: false,
            include_four_leg: true,
        }
    }
}

/// 成品成长质量的档位边界传输对象；接口字段独立于领域结构，便于后续调整传输契约。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthQualityTierBoundaryDto {
    pub label: String,
    pub min_score: f64,
    pub max_score: Option<f64>,
}

/// 成品成长质量的副属性传输对象；保留未知强化次数和未知单手分数。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthQualityAttributeDto {
    pub attribute_type: String,
    pub attribute_label: String,
    pub value: f64,
    pub enhancement_count: Option<u8>,
    pub hand_count: Option<u8>,
    pub single_roll_quality: Option<f64>,
}

/// 成品成长质量的单枚御魂传输对象；不携带用途价值或式神建议。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthQualitySoulDto {
    pub soul_key: String,
    pub set_id: String,
    pub slot: u8,
    pub quality: u8,
    pub level: u8,
    pub main_attr_type: String,
    pub main_attr_value: f64,
    pub attributes: Vec<GrowthQualityAttributeDto>,
    pub total_growth_score: Option<f64>,
    pub tier: Option<String>,
    pub score_note: Option<String>,
}

/// 成品成长质量报告传输对象；模型版本和证据状态随结果一起返回。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthQualityReportDto {
    pub model_version: String,
    pub evidence_status: String,
    pub model_note: String,
    pub hand_count: u8,
    pub tier_boundaries: Vec<GrowthQualityTierBoundaryDto>,
    pub candidate_count: u32,
    pub scoreable_count: u32,
    pub unknown_data_count: u32,
    pub excluded_unconfirmed_count: u32,
    pub items: Vec<GrowthQualitySoulDto>,
}

impl From<GrowthQualityTierBoundary> for GrowthQualityTierBoundaryDto {
    /// 将领域档位边界逐字段映射为接口对象，避免暴露领域类型别名。
    fn from(value: GrowthQualityTierBoundary) -> Self {
        Self {
            label: value.label,
            min_score: value.min_score,
            max_score: value.max_score,
        }
    }
}

impl From<GrowthQualityAttribute> for GrowthQualityAttributeDto {
    /// 将领域副属性成长结果映射为前端展示所需的精确字段。
    fn from(value: GrowthQualityAttribute) -> Self {
        Self {
            attribute_type: value.attribute_type,
            attribute_label: value.attribute_label,
            value: value.value,
            enhancement_count: value.enhancement_count,
            hand_count: value.hand_count,
            single_roll_quality: value.single_roll_quality,
        }
    }
}

impl From<GrowthQualitySoul> for GrowthQualitySoulDto {
    /// 将单枚御魂结果映射为接口对象，并保留未知分数说明。
    fn from(value: GrowthQualitySoul) -> Self {
        Self {
            soul_key: value.soul_key,
            set_id: value.set_id,
            slot: value.slot,
            quality: value.quality,
            level: value.level,
            main_attr_type: value.main_attr_type,
            main_attr_value: value.main_attr_value,
            attributes: value.attributes.into_iter().map(Into::into).collect(),
            total_growth_score: value.total_growth_score,
            tier: value.tier,
            score_note: value.score_note,
        }
    }
}

impl From<GrowthQualityReport> for GrowthQualityReportDto {
    /// 将完整成长质量报告映射为独立的前后端传输结构。
    fn from(value: GrowthQualityReport) -> Self {
        Self {
            model_version: value.model_version,
            evidence_status: value.evidence_status,
            model_note: value.model_note,
            hand_count: value.hand_count,
            tier_boundaries: value.tier_boundaries.into_iter().map(Into::into).collect(),
            candidate_count: value.candidate_count,
            scoreable_count: value.scoreable_count,
            unknown_data_count: value.unknown_data_count,
            excluded_unconfirmed_count: value.excluded_unconfirmed_count,
            items: value.items.into_iter().map(Into::into).collect(),
        }
    }
}

// ─── 通用 ─────────────────────────────────────────────────────────────────────

/// 用于诊断页的数据库与数据底座状态。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseStatusDto {
    pub schema_version: u32,
    pub migrations_applied: u32,
    pub wal_enabled: bool,
    pub profile_count: u32,
    pub raw_object_count: u64,
    pub raw_object_raw_bytes: u64,
    pub raw_object_stored_bytes: u64,
    pub active_catalog_version: Option<String>,
    pub backup_count: u32,
}

/// 后台任务接收确认；返回后前端通过统一事件通道监听进度。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAcceptedDto {
    pub task_id: String,
}

/// 切换当前激活角色；档案 ID 必须是已导入角色绑定的数据档案。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetActiveCharacterRequest {
    pub profile_id: String,
}

/// 删除单个角色档案；使用身份键定位，避免旧档案缺少 profileId 时无法删除。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteCharacterArchiveRequest {
    pub identity_key: String,
}

pub type CharacterArchiveScanResultDto = CharacterArchiveScanResult;
pub type CharacterArchiveExportRequestDto = CharacterArchiveExportRequest;
pub type CharacterArchiveExportResultDto = CharacterArchiveExportResult;
pub type CharacterArchiveImportRequestDto = CharacterArchiveImportRequest;
pub type CharacterArchiveImportResultDto = CharacterArchiveImportResult;
pub type DeleteCharacterArchiveRequestDto = DeleteCharacterArchiveRequest;

/// 跨角色碎片查询请求；界面先用本地式神目录选出稳定 ID，后端仍负责边界校验。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShikigamiShardLookupRequestDto {
    pub shikigami_id: String,
}

/// 跨角色查询结果沿用应用层的只读事实结构，不复制原始快照字段。
pub type ShikigamiShardLookupResultDto = ShikigamiShardLookupResult;

pub type GuildOverviewDto = GuildOverview;
/// 藏宝阁链接预检结果；样例字段来自公开商品详情，原始御魂与式神库存只在 Rust 导入链路内流转。
pub type CbgReadPreviewDto = CbgReadPreview;
pub type CbgSoulPreviewDto = CbgSoulPreview;

/// 藏宝阁读取请求；后端会再次校验域名和商品详情路径，避免前端传入任意远程地址。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CbgReadRequest {
    pub source_url: String,
}

/// 当前账号式神及其技能等级的传输对象；不暴露 heroes 原始缓存，也不包含御魂装备关系。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedShikigamiSkillDto {
    pub skill_id: u64,
    pub level: u8,
}

/// 式神录使用的玩家持有数据；目录名称和头像由前端再与静态式神目录合并。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedShikigamiDto {
    pub instance_id: String,
    pub shikigami_id: String,
    pub star: u8,
    pub level: Option<u8>,
    pub exp: Option<u64>,
    pub locked: Option<bool>,
    pub awakened: Option<bool>,
    pub skin_id: Option<u64>,
    pub skills: Vec<OwnedShikigamiSkillDto>,
    pub selected_skill_ids: Vec<u64>,
}

impl From<OwnedShikigamiSkill> for OwnedShikigamiSkillDto {
    fn from(skill: OwnedShikigamiSkill) -> Self {
        Self {
            skill_id: skill.skill_id,
            level: skill.level,
        }
    }
}

impl From<OwnedShikigami> for OwnedShikigamiDto {
    fn from(shikigami: OwnedShikigami) -> Self {
        Self {
            instance_id: shikigami.instance_id,
            shikigami_id: shikigami.shikigami_id,
            star: shikigami.star,
            level: shikigami.level,
            exp: shikigami.exp,
            locked: shikigami.locked,
            awakened: shikigami.awakened,
            skin_id: shikigami.skin_id,
            skills: shikigami.skills.into_iter().map(Into::into).collect(),
            selected_skill_ids: shikigami.selected_skill_ids,
        }
    }
}

/// 式神仓库中的一组素材式神；只有数量，没有实例明细。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShikigamiBagGroupDto {
    pub shikigami_id: String,
    pub group_key: String,
    pub count: u32,
}

impl From<ShikigamiBagGroup> for ShikigamiBagGroupDto {
    fn from(group: ShikigamiBagGroup) -> Self {
        Self {
            shikigami_id: group.shikigami_id,
            group_key: group.group_key,
            count: group.count,
        }
    }
}

/// 当前目标式神碎片 DTO；只传递碎片数量和完成所需上限，不暴露原始货币键。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShikigamiShardDto {
    pub shikigami_id: String,
    pub shard_count: u32,
    pub max_shard_count: u32,
}

impl From<ShikigamiShard> for ShikigamiShardDto {
    fn from(shard: ShikigamiShard) -> Self {
        Self {
            shikigami_id: shard.shikigami_id,
            shard_count: shard.shard_count,
            max_shard_count: shard.max_shard_count,
        }
    }
}

/// 单枚导入文件的历史事实摘要。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotDto {
    pub id: String,
    pub profile_id: String,
    pub raw_sha256: String,
    pub content_fingerprint: Option<String>,
    pub scope_fingerprint: Option<String>,
    pub source_kind: String,
    pub completeness: String,
    pub scope_json: Option<String>,
    pub captured_at: Option<String>,
    pub game_version: Option<String>,
    pub adapter_version: Option<String>,
    pub parser_version: String,
    pub catalog_version: String,
    pub parent_snapshot_id: Option<String>,
    pub forced: bool,
    pub created_at: String,
}

impl From<Snapshot> for SnapshotDto {
    fn from(snapshot: Snapshot) -> Self {
        Self {
            id: snapshot.id,
            profile_id: snapshot.profile_id,
            raw_sha256: snapshot.raw_sha256,
            content_fingerprint: snapshot.content_fingerprint,
            scope_fingerprint: snapshot.scope_fingerprint,
            source_kind: snapshot.source_kind,
            completeness: snapshot.completeness,
            scope_json: snapshot.scope_json,
            captured_at: snapshot.captured_at,
            game_version: snapshot.game_version,
            adapter_version: snapshot.adapter_version,
            parser_version: snapshot.parser_version,
            catalog_version: snapshot.catalog_version,
            parent_snapshot_id: snapshot.parent_snapshot_id,
            forced: snapshot.forced,
            created_at: snapshot.created_at,
        }
    }
}

/// 采集事件历史 DTO。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionEventDto {
    pub id: String,
    pub profile_id: String,
    pub source_kind: String,
    pub source_format: String,
    pub raw_sha256: Option<String>,
    pub captured_at: Option<String>,
    pub received_at: String,
    pub game_version: Option<String>,
    pub adapter_version: Option<String>,
    pub parser_version: String,
    pub completeness: String,
    pub scope_json: Option<String>,
    pub result_kind: String,
    pub snapshot_id: Option<String>,
}

impl From<AcquisitionEvent> for AcquisitionEventDto {
    fn from(event: AcquisitionEvent) -> Self {
        Self {
            id: event.id,
            profile_id: event.profile_id,
            source_kind: event.source_kind,
            source_format: event.source_format,
            raw_sha256: event.raw_sha256,
            captured_at: event.captured_at,
            received_at: event.received_at,
            game_version: event.game_version,
            adapter_version: event.adapter_version,
            parser_version: event.parser_version,
            completeness: event.completeness,
            scope_json: event.scope_json,
            result_kind: event.result_kind,
            snapshot_id: event.snapshot_id,
        }
    }
}

/// 当前库存投影项 DTO。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryItemDto {
    pub profile_id: String,
    pub soul_key: String,
    pub snapshot_id: String,
    pub soul_internal_id: String,
    pub first_seen_snapshot_id: String,
    pub last_seen_snapshot_id: String,
    pub presence_state: String,
    pub updated_at: String,
}

/// 我的御魂页面使用的副属性 DTO；保留强化次数来源，避免前端把未知值误显示成零次。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MySoulAttributeDto {
    pub attribute_index: u8,
    pub attribute_type: String,
    pub value: f64,
    pub enhancement_count: Option<u8>,
    pub count_provenance: String,
    pub fixed_attribute: bool,
}

/// 当前导入库存中的完整御魂事实；set_id 与御魂目录共用稳定编号，由页面负责关联名称和图标。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MySoulDto {
    pub soul_key: String,
    pub snapshot_id: String,
    pub soul_internal_id: String,
    pub source_stable_id: Option<String>,
    pub identity_quality: String,
    pub set_id: String,
    pub slot: u8,
    pub quality: u8,
    pub level: u8,
    pub main_attr_type: String,
    pub main_attr_value: f64,
    pub initial_substat_count: Option<u8>,
    pub locked_in_source: Option<bool>,
    pub equipped_state: Option<String>,
    pub presence_state: String,
    pub attributes: Vec<MySoulAttributeDto>,
}

/// “我的御魂”分页请求；默认只读取 10 枚，筛选条件由数据库侧执行。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMySoulsRequest {
    #[serde(default)]
    pub offset: u32,
    #[serde(default = "default_my_soul_page_limit")]
    pub limit: u32,
    pub set_id: Option<String>,
    /// 一级套装效果分类；具体御魂未选择时由数据库直接按目录分类过滤。
    pub set_category: Option<String>,
    pub slot: Option<u8>,
    pub quality: Option<u8>,
    pub level: Option<u8>,
    pub main_attr_type: Option<String>,
    pub sub_attr_type: Option<String>,
    /// 属性数值筛选只检查副属性；值已按数据库原始单位传入。
    pub attribute_type: Option<String>,
    pub attribute_operator: Option<String>,
    pub attribute_value: Option<f64>,
    #[serde(default)]
    pub standard_score_min: Option<f64>,
    /// 标准评分严格小于该数值；与最低分同时提供时形成开区间。
    #[serde(default)]
    pub standard_score_max: Option<f64>,
    #[serde(default)]
    pub auspicious_only: bool,
}

fn default_my_soul_page_limit() -> u32 {
    10
}

impl Default for ListMySoulsRequest {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: default_my_soul_page_limit(),
            set_id: None,
            set_category: None,
            slot: None,
            quality: None,
            level: None,
            main_attr_type: None,
            sub_attr_type: None,
            attribute_type: None,
            attribute_operator: None,
            attribute_value: None,
            standard_score_min: None,
            standard_score_max: None,
            auspicious_only: false,
        }
    }
}

/// “我的御魂”筛选选项；只传递稳定编号和属性类型，不传递额外御魂记录。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MySoulFilterOptionsDto {
    pub set_ids: Vec<String>,
    pub slots: Vec<u8>,
    pub qualities: Vec<u8>,
    pub levels: Vec<u8>,
    pub main_attr_types: Vec<String>,
    pub sub_attr_types: Vec<String>,
}

impl From<InventoryFilterOptions> for MySoulFilterOptionsDto {
    fn from(options: InventoryFilterOptions) -> Self {
        Self {
            set_ids: options.set_ids,
            slots: options.slots,
            qualities: options.qualities,
            levels: options.levels,
            main_attr_types: options.main_attr_types,
            sub_attr_types: options.sub_attr_types,
        }
    }
}

/// 我的御魂分页结果；数值筛选通过 `hasMore` 支持追加读取，当前批次评分另行按键读取。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MySoulPageDto {
    /// 当前页实际读取的游戏档案；前端发起评分任务时必须复用该 ID，不能自行猜测列表第一项。
    pub profile_id: Option<String>,
    pub items: Vec<MySoulDto>,
    pub total: u32,
    /// 副属性数值筛选还有下一批结果时为 true。
    pub has_more: bool,
    pub inventory_total: u32,
    /// 当前库存来源汇总；页面放在标题右侧展示，不在每枚御魂行内重复显示。
    pub source_kinds: Vec<String>,
    pub limit: u32,
    pub offset: u32,
    pub filter_options: MySoulFilterOptionsDto,
    pub score_count: u32,
}

/// 单个套装、号位和指标的雷达最高值；二、四、六号位同时返回该御魂主属性。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoulRadarPointDto {
    pub set_id: String,
    pub slot: u8,
    pub metric_type: String,
    pub value: f64,
    pub main_attr_type: Option<String>,
}

impl From<SoulRadarPoint> for SoulRadarPointDto {
    fn from(point: SoulRadarPoint) -> Self {
        Self {
            set_id: point.set_id,
            slot: point.slot,
            metric_type: point.metric_type,
            value: point.value,
            main_attr_type: point.main_attr_type,
        }
    }
}

/// 已保存的御魂雷达结果；当前库存版本用于前端提示用户是否需要重新计算。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoulRadarCacheDto {
    pub profile_id: String,
    pub calculated_at: String,
    pub inventory_revision: String,
    pub inventory_count: u32,
    pub current_inventory_revision: String,
    pub current_inventory_count: u32,
    pub points: Vec<SoulRadarPointDto>,
}

impl From<SoulRadarCache> for SoulRadarCacheDto {
    fn from(cache: SoulRadarCache) -> Self {
        Self {
            profile_id: cache.profile_id,
            calculated_at: cache.calculated_at,
            inventory_revision: cache.inventory_revision,
            inventory_count: cache.inventory_count,
            current_inventory_revision: cache.current_inventory_revision,
            current_inventory_count: cache.current_inventory_count,
            points: cache.points.into_iter().map(Into::into).collect(),
        }
    }
}

/// 头尾卡片中的副属性事实；保留强化次数供卡片显示“几手速度”。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadTailAttributeDto {
    pub attribute_type: String,
    pub value: f64,
    pub enhancement_count: Option<u8>,
    pub fixed_attribute: bool,
}

impl From<HeadTailAttribute> for HeadTailAttributeDto {
    fn from(attribute: HeadTailAttribute) -> Self {
        Self {
            attribute_type: attribute.attribute_type,
            value: attribute.value,
            enhancement_count: attribute.enhancement_count,
            fixed_attribute: attribute.fixed_attribute,
        }
    }
}

/// 单枚头尾候选御魂；只有明确记录速度强化 5 次才入选，结果已经由后端筛选并排序。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadTailCardDto {
    pub soul_key: String,
    pub set_id: String,
    pub slot: u8,
    pub quality: u8,
    pub level: u8,
    pub main_attr_type: String,
    pub main_attr_value: f64,
    pub speed: f64,
    pub speed_rolls: u8,
    pub attributes: Vec<HeadTailAttributeDto>,
}

impl From<HeadTailCard> for HeadTailCardDto {
    fn from(card: HeadTailCard) -> Self {
        Self {
            soul_key: card.soul_key,
            set_id: card.set_id,
            slot: card.slot,
            quality: card.quality,
            level: card.level,
            main_attr_type: card.main_attr_type,
            main_attr_value: card.main_attr_value,
            speed: card.speed,
            speed_rolls: card.speed_rolls,
            attributes: card.attributes.into_iter().map(Into::into).collect(),
        }
    }
}

/// 已保存的全部头尾分析结果；当前库存版本供前端判断是否需要重算。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadTailCacheDto {
    pub profile_id: String,
    pub calculated_at: String,
    pub inventory_revision: String,
    pub inventory_count: u32,
    pub current_inventory_revision: String,
    pub current_inventory_count: u32,
    pub heads: Vec<HeadTailCardDto>,
    pub tails: Vec<HeadTailCardDto>,
    pub head_candidate_count: u32,
    pub tail_candidate_count: u32,
}

impl From<HeadTailCache> for HeadTailCacheDto {
    fn from(cache: HeadTailCache) -> Self {
        Self {
            profile_id: cache.profile_id,
            calculated_at: cache.calculated_at,
            inventory_revision: cache.inventory_revision,
            inventory_count: cache.inventory_count,
            current_inventory_revision: cache.current_inventory_revision,
            current_inventory_count: cache.current_inventory_count,
            heads: cache.heads.into_iter().map(Into::into).collect(),
            tails: cache.tails.into_iter().map(Into::into).collect(),
            head_candidate_count: cache.head_candidate_count,
            tail_candidate_count: cache.tail_candidate_count,
        }
    }
}

/// 当前页评分摘要请求；空键列表表示页面还没有可查询的御魂。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMySoulScoresRequest {
    #[serde(default)]
    pub soul_keys: Vec<String>,
}

/// 一键模拟强化请求；seed 可选，便于用户回放相同的娱乐结果。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulateEnhancementRequest {
    #[serde(default = "default_simulation_threshold")]
    pub threshold: f64,
    pub seed: Option<u64>,
}

fn default_simulation_threshold() -> f64 {
    70.0
}

impl From<SimulateEnhancementRequest> for SimulateEnhancementInput {
    fn from(request: SimulateEnhancementRequest) -> Self {
        Self {
            threshold: request.threshold,
            seed: request.seed,
        }
    }
}

/// 模拟强化结果 DTO；应用层已经完成评分和排序，界面只负责呈现。
pub type SimulateEnhancementResultDto = SimulateEnhancementResult;

/// 历史、分析中心和对象完整性均以领域 DTO 原样传输，避免前端二次拼装跨表事实。
pub type HistoryOverviewDto = HistoryOverview;
pub type AnalysisCenterDto = AnalysisCenter;
pub type AnalysisTraceDto = AnalysisTrace;
pub type ProfileObjectVerificationDto = ProfileObjectVerification;
pub type ExportPayloadDto = ExportPayload;

/// 历史时间线请求；limit 只影响采集事件数量，快照历史始终返回完整档案链。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryOverviewRequest {
    pub profile_id: String,
    #[serde(default = "default_history_event_limit")]
    pub event_limit: u32,
}

fn default_history_event_limit() -> u32 {
    200
}

/// 分析中心请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisCenterRequest {
    pub profile_id: String,
}

/// 单枚御魂追溯请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisTraceRequest {
    pub profile_id: String,
    pub soul_key: String,
}

/// 档案原始对象完整性校验请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyProfileObjectsRequest {
    pub profile_id: String,
}

/// 档案、快照或筛选结果导出请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportDataRequest {
    pub profile_id: String,
    pub kind: String,
    #[serde(default)]
    pub snapshot_id: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
}

impl From<InventoryItem> for InventoryItemDto {
    fn from(item: InventoryItem) -> Self {
        Self {
            profile_id: item.profile_id,
            soul_key: item.soul_key,
            snapshot_id: item.snapshot_id,
            soul_internal_id: item.soul_internal_id,
            first_seen_snapshot_id: item.first_seen_snapshot_id,
            last_seen_snapshot_id: item.last_seen_snapshot_id,
            presence_state: item.presence_state,
            updated_at: item.updated_at,
        }
    }
}

// ─── 分析待办、用户决定与行动批次 ─────────────────────────────────────────────

/// 重新计算分析的请求；可带导入刚生成的快照以定位新增/变化待办。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecalculateAnalysisRequest {
    pub profile_id: String,
    #[serde(default)]
    pub snapshot_id: Option<String>,
}

/// 待办分页和筛选请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAnalysisTodosRequest {
    pub profile_id: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub use_id: Option<String>,
    #[serde(default = "default_page_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

fn default_page_limit() -> u32 {
    100
}

/// 单枚或批量决定预览请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewDecisionsRequest {
    pub profile_id: String,
    pub soul_keys: Vec<String>,
    pub decision: String,
    #[serde(default = "default_respect_protection")]
    pub respect_protection: bool,
}

fn default_respect_protection() -> bool {
    true
}

/// 批量决定应用请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyDecisionsRequest {
    pub profile_id: String,
    pub changes: Vec<DecisionChange>,
    #[serde(default = "default_respect_protection")]
    pub respect_protection: bool,
}

/// 决定撤销请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoDecisionRequest {
    pub operation_id: String,
}

/// 读取某枚御魂决定历史的请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionHistoryRequest {
    pub profile_id: String,
    pub soul_key: String,
}

/// 创建行动批次请求。
pub type CreateActionBatchRequest = crate::domain::CreateBatchRequest;

/// 更新批次条目状态请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBatchItemRequest {
    pub profile_id: String,
    pub batch_id: String,
    pub soul_key: String,
    pub status: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// 分析待办传输对象；解释详情从安全 JSON 字符串解码，不执行其中任何内容。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisTodoDto {
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
    pub standard_score: Option<f64>,
    pub composite_score: Option<f64>,
    pub category: String,
    pub recommendation: String,
    pub reason_summary: String,
    pub evidence_level: String,
    pub data_quality: String,
    pub presence_state: String,
    pub is_new: bool,
    pub is_changed: bool,
    pub detail: serde_json::Value,
    pub explanation_hash: String,
    pub generated_at: String,
    pub revision: u32,
    pub user_decision: Option<String>,
    pub user_decision_note: Option<String>,
    pub decision_revision: Option<u32>,
}

/// 我的御魂列表使用的评分摘要；完整解释树仍由分析待办详情单独传输。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoulScoreSummaryDto {
    pub soul_key: String,
    pub standard_score: Option<f64>,
    pub standard_score_rule: Option<String>,
    pub standard_score_formula: Option<String>,
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
    /// 实际参与评分的规范规则正文哈希，用于识别标准保存后的历史旧结果。
    pub standard_hash: Option<String>,
}

impl From<crate::domain::SoulScoreSummary> for SoulScoreSummaryDto {
    fn from(summary: crate::domain::SoulScoreSummary) -> Self {
        Self {
            soul_key: summary.soul_key,
            standard_score: summary.standard_score,
            standard_score_rule: summary.standard_score_rule,
            standard_score_formula: summary.standard_score_formula,
            standard_score_contributions: summary.standard_score_contributions,
            composite_score: summary.composite_score,
            best_score: summary.best_score,
            best_use_id: summary.best_use_id,
            best_use_title: summary.best_use_title,
            effective_growth_count: summary.effective_growth_count,
            scored_use_count: summary.scored_use_count,
            recommendation: summary.recommendation,
            category: summary.category,
            generated_at: summary.generated_at,
            standard_id: summary.standard_id,
            standard_version: summary.standard_version,
            standard_title: summary.standard_title,
            standard_hash: summary.standard_hash,
        }
    }
}

impl From<AnalysisTodo> for AnalysisTodoDto {
    fn from(todo: AnalysisTodo) -> Self {
        let detail = serde_json::from_str(&todo.detail_json).unwrap_or(serde_json::Value::Null);
        Self {
            id: todo.id,
            profile_id: todo.profile_id,
            soul_key: todo.soul_key,
            snapshot_id: todo.snapshot_id,
            soul_internal_id: todo.soul_internal_id,
            set_id: todo.set_id,
            slot: todo.slot,
            quality: todo.quality,
            level: todo.level,
            main_attribute: todo.main_attribute,
            main_value: todo.main_value,
            standard_score: todo.standard_score,
            composite_score: todo.composite_score,
            category: todo.category,
            recommendation: todo.recommendation,
            reason_summary: todo.reason_summary,
            evidence_level: todo.evidence_level,
            data_quality: todo.data_quality,
            presence_state: todo.presence_state,
            is_new: todo.is_new,
            is_changed: todo.is_changed,
            detail,
            explanation_hash: todo.explanation_hash,
            generated_at: todo.generated_at,
            revision: todo.revision,
            user_decision: todo.user_decision,
            user_decision_note: todo.user_decision_note,
            decision_revision: todo.decision_revision,
        }
    }
}

/// 待办分页传输对象。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisTodoPageDto {
    pub items: Vec<AnalysisTodoDto>,
    pub total: u32,
    pub limit: u32,
    pub offset: u32,
}

impl From<AnalysisTodoPage> for AnalysisTodoPageDto {
    fn from(page: AnalysisTodoPage) -> Self {
        Self {
            items: page.items.into_iter().map(Into::into).collect(),
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        }
    }
}

/// 领域批量决定/撤销结果的传输对象。
pub type DecisionPreviewDto = DecisionPreview;
pub type DecisionApplyResultDto = DecisionApplyResult;
pub type DecisionHistoryEntryDto = DecisionHistoryEntry;

/// 行动批次摘要与分组传输对象。
pub type ActionBatchDto = ActionBatch;
pub type ActionBatchDetailDto = ActionBatchDetail;
pub type BatchItemDto = BatchItem;

// ─── 游戏档案 ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameProfileDto {
    pub id: String,
    pub display_name: String,
    pub source_identity: Option<String>,
    pub server_label: Option<String>,
    pub maturity_mode: String,
    pub maturity_level: Option<String>,
    pub revision: u32,
    pub created_at: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
}

impl From<GameProfile> for GameProfileDto {
    fn from(p: GameProfile) -> Self {
        Self {
            id: p.id,
            display_name: p.display_name,
            source_identity: p.source_identity,
            server_label: p.server_label,
            maturity_mode: p.maturity_mode.as_str().to_owned(),
            maturity_level: p.maturity_level.map(|m| m.as_str().to_owned()),
            revision: p.revision,
            created_at: p.created_at,
            updated_at: p.updated_at,
            archived_at: p.archived_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaturityInfoDto {
    pub system: Vec<MaturityThreshold>,
    pub profile: Vec<MaturityThreshold>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCreatedDto {
    pub profile: GameProfileDto,
}

// ─── 目录 ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogStatusDto {
    pub version: String,
    pub set_count: u64,
    pub group_count: u64,
    pub source: Option<String>,
    pub sha256: Option<String>,
    pub parser_version: Option<String>,
    pub validation_status: String,
    pub validation_errors: Vec<String>,
    pub icon_coverage: u64,
    pub mechanics_coverage: u64,
    pub shikigami_count: u64,
    pub panel_coverage: u64,
    pub skill_coverage: u64,
}

impl From<CatalogStatus> for CatalogStatusDto {
    fn from(s: CatalogStatus) -> Self {
        Self {
            version: s.version,
            set_count: s.set_count,
            group_count: s.group_count,
            source: s.source,
            sha256: s.sha256,
            parser_version: s.parser_version,
            validation_status: s.validation_status,
            validation_errors: s.validation_errors,
            icon_coverage: s.icon_coverage,
            mechanics_coverage: s.mechanics_coverage,
            shikigami_count: s.shikigami_count,
            panel_coverage: s.panel_coverage,
            skill_coverage: s.skill_coverage,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoulSetDto {
    pub catalog_version: String,
    pub set_id: String,
    pub name: String,
    pub icon_asset_id: Option<u32>,
    pub category: Option<String>,
    pub special_category: Option<String>,
    pub two_piece_effect: Option<String>,
    pub four_piece_effect: Option<String>,
    pub enhancement_rule: Option<serde_json::Value>,
    pub evidence: Vec<CatalogEvidence>,
}

impl From<SoulSet> for SoulSetDto {
    fn from(s: SoulSet) -> Self {
        Self {
            catalog_version: s.catalog_version,
            set_id: s.set_id,
            name: s.name,
            icon_asset_id: s.icon_asset_id,
            category: s.category,
            special_category: s.special_category,
            two_piece_effect: decode_json_text(s.two_piece_effect_json),
            four_piece_effect: decode_json_text(s.four_piece_effect_json),
            enhancement_rule: s
                .enhancement_rule_json
                .and_then(|value| serde_json::from_str(&value).ok()),
            evidence: decode_evidence_json(s.source_refs_json),
        }
    }
}

/// 式神详情 DTO；嵌套 JSON 在边界层一次性解码，前端无需理解 SQLite 存储形式。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShikigamiDto {
    pub catalog_version: String,
    pub shikigami_id: String,
    pub name: String,
    pub rarity: String,
    pub icon_asset_id: Option<String>,
    pub panel_level: u8,
    pub awakened: bool,
    pub panel: serde_json::Value,
    pub skills: serde_json::Value,
    pub skill_investment: serde_json::Value,
    pub employment_scenes: serde_json::Value,
    pub game_version: String,
    pub evidence: Vec<CatalogEvidence>,
}

impl From<Shikigami> for ShikigamiDto {
    fn from(s: Shikigami) -> Self {
        Self {
            catalog_version: s.catalog_version,
            shikigami_id: s.shikigami_id,
            name: s.name,
            rarity: s.rarity,
            icon_asset_id: s.icon_asset_id,
            panel_level: s.panel_level,
            awakened: s.awakened,
            panel: decode_json_value(&s.panel_json),
            skills: decode_json_value(&s.skills_json),
            skill_investment: decode_json_value(&s.skill_investment_json),
            employment_scenes: decode_json_value(&s.employment_scenes_json),
            game_version: s.game_version,
            evidence: decode_evidence_json(Some(s.source_refs_json)),
        }
    }
}

/// 兼容旧目录中保存的 JSON 字符串标量，也允许未来目录直接保存普通文本。
fn decode_json_text(value: Option<String>) -> Option<String> {
    value.map(|text| serde_json::from_str::<String>(&text).unwrap_or(text))
}

/// 将目录包中的证据数组解析为只读 DTO；损坏证据只显示为空，不阻断已完成的套装事实。
fn decode_evidence_json(value: Option<String>) -> Vec<CatalogEvidence> {
    value
        .and_then(|text| serde_json::from_str::<Vec<CatalogEvidence>>(&text).ok())
        .unwrap_or_default()
}

/// 解码目录中的嵌套 JSON；异常时返回 null，让前端进入未核验状态而不是崩溃。
fn decode_json_value(value: &str) -> serde_json::Value {
    serde_json::from_str(value).unwrap_or(serde_json::Value::Null)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveCommonnessDto {
    pub set_id: String,
    pub scenario: String,
    pub value: String,
    pub is_override: bool,
}

impl From<EffectiveCommonness> for EffectiveCommonnessDto {
    fn from(e: EffectiveCommonness) -> Self {
        Self {
            set_id: e.set_id,
            scenario: e.scenario,
            value: e.value.as_str().to_owned(),
            is_override: e.is_override,
        }
    }
}

// ─── 原始对象 ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawObjectDto {
    pub sha256: String,
    pub relative_path: String,
    pub compression: String,
    pub media_type: String,
    pub raw_size: u64,
    pub stored_size: u64,
    pub verified_at: Option<String>,
    pub created_at: String,
}

impl From<RawObject> for RawObjectDto {
    fn from(r: RawObject) -> Self {
        Self {
            sha256: r.sha256,
            relative_path: r.relative_path,
            compression: r.compression,
            media_type: r.media_type,
            raw_size: r.raw_size,
            stored_size: r.stored_size,
            verified_at: r.verified_at,
            created_at: r.created_at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredRawObjectDto {
    pub sha256: String,
    pub raw_size: u64,
    pub stored_size: u64,
    pub is_new: bool,
}

// ─── 备份 ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRecordDto {
    pub id: String,
    pub path: String,
    pub schema_version: u32,
    pub file_count: u32,
    pub total_size: u64,
    pub reason: String,
    pub checksum: Option<String>,
    pub created_at: String,
    pub restored_at: Option<String>,
    pub verified: bool,
}

impl From<BackupRecord> for BackupRecordDto {
    fn from(r: BackupRecord) -> Self {
        Self {
            id: r.id,
            path: r.path,
            schema_version: r.schema_version,
            file_count: r.file_count,
            total_size: r.total_size,
            reason: r.reason,
            checksum: r.checksum,
            created_at: r.created_at,
            restored_at: r.restored_at,
            verified: r.verified,
        }
    }
}

// ─── 请求参数 ──────────────────────────────────────────────────────────────────

/// 创建档案请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProfileRequest {
    pub display_name: String,
    #[serde(default)]
    pub source_identity: Option<String>,
    #[serde(default)]
    pub server_label: Option<String>,
}

/// 重命名档案请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameProfileRequest {
    pub profile_id: String,
    pub new_name: String,
    pub expected_revision: u32,
}

/// 归档/取消归档请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveProfileRequest {
    pub profile_id: String,
    pub archived: bool,
    pub expected_revision: u32,
}

/// 设置成熟度阈值请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetMaturityRequest {
    pub profile_id: String,
    pub thresholds: Vec<MaturityThresholdDto>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaturityThresholdDto {
    pub level: String,
    pub min_count: u64,
}

/// 常用度覆盖请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommonnessOverrideRequest {
    pub profile_id: String,
    pub set_id: String,
    pub scenario: String,
    pub value: String,
}

/// 复制常用度请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyCommonnessRequest {
    pub from_profile_id: String,
    pub to_profile_id: String,
}

/// 存储原始对象请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreRawRequest {
    pub profile_id: String,
    pub source_kind: String,
    pub source_format: String,
    pub media_type: String,
    pub payload: Vec<u8>,
    pub received_at: String,
}

/// 创建备份请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBackupRequest {
    pub reason: String,
}

// ─── 签名更新 ─────────────────────────────────────────────────────────────────

/// 更新检查请求；镜像地址由 Rust 配置目录解析，前端只能选择来源和包类型。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckUpdatesRequest {
    pub source_mode: UpdateSourceMode,
    /// 前端声明当前构建通道；命令层会再次与编译期通道核对，不能靠前端自选跨通道。
    pub channel: UpdateChannel,
    pub package_type: PackageType,
    #[serde(default)]
    pub game_version: Option<String>,
    /// 启动检查只保存签名清单；手动检查默认继续下载并暂存完整载荷，保持旧页面行为。
    #[serde(default = "default_stage_update_payload")]
    pub stage_payload: bool,
}

fn default_stage_update_payload() -> bool {
    true
}

/// 安装已验证暂存对象请求；暂存 ID 不接受任意文件路径。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallUpdateRequest {
    pub staged_update_id: String,
    /// 应用包由用户点击更新后自动安装并重启，数据包仍保持原有手动安装行为。
    #[serde(default)]
    pub auto_restart: bool,
    #[serde(default)]
    pub game_version: Option<String>,
}

/// 应用包下载进度；只发送阶段、字节数和暂存 ID，不暴露下载地址或载荷内容。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgressDto {
    pub staged_update_id: String,
    pub phase: String,
    pub completed: u64,
    pub total: u64,
    pub message: String,
}

/// 用户主动选择历史版本回退请求；版本只能来自已安装数据包列表。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackUpdateRequest {
    pub package_type: PackageType,
    pub version: String,
    #[serde(default)]
    pub game_version: Option<String>,
}

/// 已安装包 DTO 保留来源、签名指纹和活动状态，不把清单私密字段暴露给 WebView。
pub type InstalledPackageDto = InstalledPackage;

// ─── 规则引擎 ─────────────────────────────────────────────────────────────────

/// 默认规则预设摘要；界面展示元数据和哈希，不直接执行未经校验的 JSON。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePresetSummaryDto {
    pub id: String,
    pub version: String,
    pub title: String,
    pub author: String,
    pub status: String,
    pub rule_count: usize,
    pub normalized_hash: String,
}

/// 默认规则胚子准入请求；事实由 Issue 03 的规范库存投影构建。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateDefaultAdmissionRequest {
    pub facts: SoulFacts,
}

/// 规则引擎内置用途模板的摘要。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinUseTemplateDto {
    pub id: String,
    pub title: String,
    pub status: String,
    pub evidence_level: String,
}

/// 规则库版本列表请求；档案为空时只读取版本，不读取启用状态。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListRuleVersionsRequest {
    #[serde(default)]
    pub profile_id: Option<String>,
}

/// 规则载荷请求；载荷可以是 UTF-8 JSON 文件或分享码文本的字节数组。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePayloadRequest {
    pub source_kind: String,
    pub payload: Vec<u8>,
}

/// 保存可编辑版本请求；父版本仅用于来源追溯，不改变规则正文。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveRuleVersionRequest {
    pub source_kind: String,
    pub payload: Vec<u8>,
    #[serde(default)]
    pub parent_version_id: Option<String>,
}

/// 档案级规则启用状态请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRuleActivationRequest {
    pub profile_id: String,
    pub rule_version_id: String,
    pub enabled: bool,
    #[serde(default)]
    pub position: u32,
    #[serde(default)]
    pub note: Option<String>,
}

/// 规则影响预览请求；使用完整载荷保证预览内容可重复验证。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleImpactRequest {
    pub profile_id: String,
    pub source_kind: String,
    pub payload: Vec<u8>,
}

/// 保存规则后启动分析重算的后台任务请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRuleRecalculationRequest {
    pub profile_id: String,
    #[serde(default)]
    pub snapshot_id: Option<String>,
}

/// 规则导出结果，同时提供文件正文和离线分享码。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleExportDto {
    pub file_content: String,
    pub share_code: String,
}

/// 应用层规则视图的 DTO 别名，保持前端契约和业务用例解耦。
pub type RuleVersionDto = RuleVersionView;
pub type RulePresetPreviewDto = RulePresetPreview;
pub type RuleImpactPreviewDto = RuleImpactPreview;
