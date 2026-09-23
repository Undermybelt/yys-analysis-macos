//! 领域实体，表示御魂分析业务中的核心概念。
//!
//! 所有实体使用不可变或半不可变模式，通过构造函数或 builder 创建。
//! 日期时间使用 ISO 8601 字符串（TEXT），避免引入时间库依赖。

use serde::{Deserialize, Serialize};

/// 角色平台在快照和档案中的稳定值；界面再把这些值翻译成中文名称。
pub const PLATFORM_ANDROID: &str = "android";
pub const PLATFORM_IOS: &str = "ios";

// ─── 游戏档案 ─────────────────────────────────────────────────────────────────

/// 游戏档案的成熟度模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MaturityMode {
    Auto,
    Manual,
}

impl MaturityMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }
}

/// 库存成熟度等级。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum MaturityLevel {
    Starter,
    Growth,
    Formed,
    Deep,
}

impl MaturityLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starter => "starter",
            Self::Growth => "growth",
            Self::Formed => "formed",
            Self::Deep => "deep",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "starter" => Some(Self::Starter),
            "growth" => Some(Self::Growth),
            "formed" => Some(Self::Formed),
            "deep" => Some(Self::Deep),
            _ => None,
        }
    }
}

/// 游戏档案：一个独立阴阳师账号或服务器身份的本地分析空间。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameProfile {
    pub id: String,
    pub display_name: String,
    pub source_identity: Option<String>,
    pub server_label: Option<String>,
    pub maturity_mode: MaturityMode,
    pub maturity_level: Option<MaturityLevel>,
    pub revision: u32,
    pub created_at: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
}

/// 创建新档案的请求参数。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewGameProfile {
    pub display_name: String,
    pub source_identity: Option<String>,
    pub server_label: Option<String>,
}

/// 成熟度阈值：系统默认或档案覆盖的六星 +15 御魂数量边界。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaturityThreshold {
    pub scope_type: String, // "system" | "profile"
    pub profile_id: String,
    pub level: MaturityLevel,
    pub min_count: u64,
}

// ─── 原始对象 ──────────────────────────────────────────────────────────────────

/// 原始对象：内容寻址的压缩原始快照文件。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawObject {
    pub sha256: String,
    pub relative_path: String,
    pub compression: String,
    pub media_type: String,
    pub raw_size: u64,
    pub stored_size: u64,
    pub verified_at: Option<String>,
    pub created_at: String,
}

// ─── 采集事件与快照 ────────────────────────────────────────────────────────────

/// 采集事件：每次成功接收或导入的记录。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionEvent {
    pub id: String,
    pub profile_id: String,
    pub source_kind: String, // "native" | "importer" | "mumu" | "desktop" | "cbg"
    pub source_format: String,
    pub raw_sha256: Option<String>,
    pub captured_at: Option<String>,
    pub received_at: String,
    pub game_version: Option<String>,
    pub adapter_version: Option<String>,
    pub parser_version: String,
    pub completeness: String, // "complete" | "partial" | "unknown"
    pub scope_json: Option<String>,
    pub result_kind: String, // "new_snapshot" | "unchanged" | "forced" | "error_free"
    pub snapshot_id: Option<String>,
}

/// 采集快照：不可变的御魂记录。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: String,
    pub profile_id: String,
    pub raw_sha256: String,
    pub content_fingerprint: Option<String>,
    /// 规范化范围声明的 SHA-256；与内容指纹分离，避免筛选范围变化被去重吞掉。
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

/// 快照中的单枚御魂。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSoul {
    pub snapshot_id: String,
    pub internal_id: String,
    pub source_stable_id: Option<String>,
    pub identity_quality: String, // "stable" | "derived" | "missing"
    pub set_id: String,
    pub slot: u8,    // 1–6
    pub quality: u8, // 星级
    pub level: u8,   // 0–15
    pub main_attr_type: String,
    pub main_attr_value: f64,
    pub initial_substat_count: Option<u8>,
    pub locked_in_source: Option<bool>,
    pub equipped_state: Option<String>,
    pub source_json: Option<String>,
}

/// 副属性记录。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoulAttribute {
    pub snapshot_id: String,
    pub soul_internal_id: String,
    pub attribute_index: u8,
    pub attribute_type: String,
    pub value: f64,
    pub enhancement_count: Option<u8>,
    pub count_provenance: String, // "source" | "estimated" | "user" | "unknown"
    pub fixed_attribute: bool,
}

/// 当前库存投影项。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryItem {
    pub profile_id: String,
    pub soul_key: String,
    pub snapshot_id: String,
    pub soul_internal_id: String,
    pub first_seen_snapshot_id: String,
    pub last_seen_snapshot_id: String,
    pub presence_state: String, // "present" | "unconfirmed" | "removed"
    pub updated_at: String,
}

/// “我的御魂”分页查询条件；过滤和分页都在 SQLite 侧完成，避免前端先加载完整库存再切片。
#[derive(Clone, Debug, Default)]
pub struct InventoryPageQuery {
    pub set_id: Option<String>,
    /// 一级套装效果分类；特殊分类优先于普通二件套分类。
    pub set_category: Option<String>,
    pub slot: Option<u8>,
    pub quality: Option<u8>,
    pub level: Option<u8>,
    pub main_attr_type: Option<String>,
    pub sub_attr_type: Option<String>,
    /// 只按指定副属性的原始数值比较；百分比属性由界面先转换为 0~1。
    pub attribute_type: Option<String>,
    pub attribute_operator: Option<String>,
    pub attribute_value: Option<f64>,
    /// 只返回标准评分严格大于该数值的御魂；评分尚未计算时不命中。
    pub standard_score_min: Option<f64>,
    /// 只返回标准评分严格小于该数值的御魂；可与最低分组合成开区间。
    pub standard_score_max: Option<f64>,
    /// 只返回重点属性强化次数达到 5 次且不是固有属性的“大吉”御魂。
    pub auspicious_only: bool,
    pub limit: u32,
    pub offset: u32,
}

/// 御魂筛选下拉框使用的轻量选项；只返回去重后的稳定值，不搬运御魂正文。
#[derive(Clone, Debug, Default)]
pub struct InventoryFilterOptions {
    pub set_ids: Vec<String>,
    pub slots: Vec<u8>,
    pub qualities: Vec<u8>,
    pub levels: Vec<u8>,
    pub main_attr_types: Vec<String>,
    pub sub_attr_types: Vec<String>,
}

/// 分页查询结果；普通筛选返回精确总数，副属性数值筛选按批次返回并标记是否还有更多。
#[derive(Clone, Debug, Default)]
pub struct InventoryPage {
    pub items: Vec<InventoryItem>,
    pub total: u32,
    /// 副属性数值筛选还有下一批结果时为 true。
    pub has_more: bool,
    pub inventory_total: u32,
    /// 当前库存实际包含的读取来源；用于“我的御魂”标题旁的来源标志。
    pub source_kinds: Vec<String>,
    pub limit: u32,
    pub offset: u32,
    pub filter_options: InventoryFilterOptions,
}

/// 雷达图支持的稳定副属性类型；固定属性不参与“每个号位最高副属性”统计。
pub const SOUL_RADAR_METRIC_TYPES: &[&str] = &[
    "attack_rate",
    "attack_flat",
    "hp_rate",
    "hp_flat",
    "defense_rate",
    "defense_flat",
    "speed",
    "crit_rate",
    "crit_damage",
    "effect_hit",
    "effect_resist",
];

/// 单个套装、指标和号位的雷达最高值；主属性来自拥有该最高副属性的御魂。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SoulRadarPoint {
    pub set_id: String,
    pub slot: u8,
    pub metric_type: String,
    pub value: f64,
    pub main_attr_type: Option<String>,
}

/// 已持久化的雷达计算结果；当前库存状态用于判断缓存是否需要用户主动重算。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SoulRadarCache {
    pub profile_id: String,
    pub calculated_at: String,
    pub inventory_revision: String,
    pub inventory_count: u32,
    pub current_inventory_revision: String,
    pub current_inventory_count: u32,
    pub points: Vec<SoulRadarPoint>,
}

/// 头尾卡片中的副属性事实；保存原始数值和强化次数，前端只负责格式化展示。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HeadTailAttribute {
    pub attribute_type: String,
    pub value: f64,
    pub enhancement_count: Option<u8>,
    pub fixed_attribute: bool,
}

/// 头或尾的最佳御魂结果；卡片所需的完整事实随派生结果一起保存，避免页面再次扫描库存。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HeadTailCard {
    pub soul_key: String,
    pub set_id: String,
    pub slot: u8,
    pub quality: u8,
    pub level: u8,
    pub main_attr_type: String,
    pub main_attr_value: f64,
    pub speed: f64,
    pub speed_rolls: u8,
    pub attributes: Vec<HeadTailAttribute>,
}

/// 已保存的头尾分析结果；库存版本用于判断结果是否需要用户主动重算，头尾数组已按候选优先级排序。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HeadTailCache {
    pub profile_id: String,
    pub calculated_at: String,
    pub inventory_revision: String,
    pub inventory_count: u32,
    pub current_inventory_revision: String,
    pub current_inventory_count: u32,
    pub heads: Vec<HeadTailCard>,
    pub tails: Vec<HeadTailCard>,
    pub head_candidate_count: u32,
    pub tail_candidate_count: u32,
}

/// 一次导入提交所需的完整事实集合；仓库会将事件、快照和库存投影放入同一事务。
#[derive(Clone, Debug)]
pub struct ImportCommitRequest {
    pub event: AcquisitionEvent,
    pub snapshot: Snapshot,
    pub souls: Vec<SnapshotSoul>,
    pub attributes: Vec<SoulAttribute>,
    pub force_snapshot: bool,
}

/// 导入提交结果，供导入进度和完成摘要使用。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCommitResult {
    pub snapshot_id: Option<String>,
    pub result_kind: String,
    pub duplicate_of: Option<String>,
    pub added_count: u32,
    pub updated_count: u32,
    pub unchanged_count: u32,
    pub removed_count: u32,
    pub skipped_unstable_count: u32,
}

// ─── 御魂目录与分组 ────────────────────────────────────────────────────────────

/// 已安装的目录包版本。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPackage {
    pub version: String,
    pub installed_at: String,
    pub source: Option<String>,
    pub sha256: Option<String>,
    pub parser_version: Option<String>,
    pub validation_status: String,
    pub validation_errors_json: Option<String>,
    pub icon_coverage: u64,
    pub mechanics_coverage: u64,
    /// 式神目录条数及其覆盖率随包保存，升级失败时旧包的校验摘要仍然可读。
    pub shikigami_count: u64,
    pub panel_coverage: u64,
    pub skill_coverage: u64,
    pub signature_text: Option<String>,
    pub compatible_game_version: Option<String>,
    pub min_app_version: Option<String>,
    pub active: bool,
}

/// 目录条目的证据引用；只描述来源，不把远程内容当作运行时依赖。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEvidence {
    pub source: String,
    pub url: Option<String>,
    pub evidence: String,
}

/// 内置目录安装前的完整性校验结果，错误集合用于界面明确展示未知状态。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogValidationResult {
    pub valid: bool,
    pub errors: Vec<String>,
    pub set_count: u64,
    pub icon_coverage: u64,
    pub mechanics_coverage: u64,
    /// 式神资料校验摘要；面板和技能覆盖率为条目级计数。
    pub shikigami_count: u64,
    pub panel_coverage: u64,
    pub skill_coverage: u64,
    pub content_sha256: String,
}

/// 强化机制的稳定结构；数据库仍保存规范 JSON，类型用于校验和跨端契约。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EnhancementRule {
    pub mechanics_version: String,
    pub checkpoints: Vec<u8>,
    pub four_substat_mode: EnhancementFourSubstatMode,
    pub incomplete_mode: EnhancementIncompleteMode,
    pub roll_benchmarks: Vec<SoulRollBenchmark>,
    pub probability_note: String,
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EnhancementFourSubstatMode {
    pub kind: String,
    pub per_attribute_probability: f64,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EnhancementIncompleteMode {
    pub kind: String,
    pub description: String,
    pub categories: Vec<EnhancementCategoryProbability>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EnhancementCategoryProbability {
    pub id: String,
    pub label: String,
    pub probability: f64,
    pub attributes: Vec<String>,
}

/// 六星副属性高档成长基准；数值单位由 `unit` 明确区分为点数或比例。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SoulRollBenchmark {
    pub attribute_id: String,
    pub label: String,
    pub high_value: f64,
    pub unit: String,
}

/// 御魂套装定义。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoulSet {
    pub catalog_version: String,
    pub set_id: String,
    pub name: String,
    pub icon_asset_id: Option<u32>,
    pub category: Option<String>,
    pub special_category: Option<String>,
    pub rarity_scope: Option<String>,
    pub two_piece_effect_json: Option<String>,
    pub four_piece_effect_json: Option<String>,
    pub enhancement_rule_json: Option<String>,
    pub source_refs_json: Option<String>,
    pub effective_from: Option<String>,
    pub effective_to: Option<String>,
}

/// 式神目录记录；复杂结构保持为已校验 JSON，确保历史目录版本可原样复现。
/// 可培养条目的 `panel_level` 表示 40 级；素材条目不具备培养面板，相关 JSON 为空结构。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shikigami {
    pub catalog_version: String,
    pub shikigami_id: String,
    pub name: String,
    pub rarity: String,
    pub icon_asset_id: Option<String>,
    pub sort_order: u32,
    pub panel_level: u8,
    pub awakened: bool,
    pub panel_json: String,
    pub skills_json: String,
    pub skill_investment_json: String,
    pub employment_scenes_json: String,
    pub game_version: String,
    pub source_refs_json: String,
}

/// 玩家当前账号持有的一只式神；这是桌面版 heroes 的归一化投影，不携带御魂装备关系。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedShikigami {
    pub instance_id: String,
    pub shikigami_id: String,
    pub star: u8,
    pub level: Option<u8>,
    pub exp: Option<u64>,
    pub locked: Option<bool>,
    pub awakened: Option<bool>,
    pub skin_id: Option<u64>,
    pub skills: Vec<OwnedShikigamiSkill>,
    pub selected_skill_ids: Vec<u64>,
}

/// 玩家式神的单个技能等级；技能 ID 与式神目录中的技能资料保持松耦合。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedShikigamiSkill {
    pub skill_id: u64,
    pub level: u8,
}

/// 式神仓库中的一组素材式神（达摩、素材怪等）。
///
/// 游戏把这类式神压缩成「分组 → 数量」存储，没有逐只实例，因此不能表达为
/// `OwnedShikigami`：它们没有实例 ID、等级、御魂和技能。游戏式神录顶部的
/// 总数把这部分计入，所以展示时需要与培养明细合并计数。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShikigamiBagGroup {
    pub shikigami_id: String,
    /// 上游分组键末三段（如 `2_1_0`）；含义尚未完全确认，保留原值便于回溯。
    pub group_key: String,
    pub count: u32,
}

/// 玩家持有的目标式神碎片；只包含 SSR、SP、UR 和御行达摩。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShikigamiShard {
    pub shikigami_id: String,
    pub shard_count: u32,
    pub max_shard_count: u32,
}

/// 御魂用途分组。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YuhunGroup {
    pub id: String,
    pub origin: String, // "system" | "custom"
    pub name: String,
    pub description: Option<String>,
    pub parent_id: Option<String>,
    pub revision: u32,
}

/// 分组与套装的关联，包括来源和用户覆盖状态。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMember {
    pub group_id: String,
    pub set_id: String,
    pub catalog_version: String,
    pub origin: String,
    pub user_override: bool,
}

/// 套装常用度默认值。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetCommonness {
    pub catalog_version: String,
    pub set_id: String,
    pub scenario: String, // "PVE" | "PVP"
    pub value: CommonnessValue,
}

/// 常用度取值。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommonnessValue {
    Common,
    Uncommon,
}

impl CommonnessValue {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Common => "common",
            Self::Uncommon => "uncommon",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "common" => Some(Self::Common),
            "uncommon" => Some(Self::Uncommon),
            _ => None,
        }
    }
}

/// 档案级常用度覆盖。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCommonness {
    pub profile_id: String,
    pub set_id: String,
    pub scenario: String,
    pub value: CommonnessValue,
}

/// 有效常用度（合并默认值与档案覆盖后的结果）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveCommonness {
    pub set_id: String,
    pub scenario: String,
    pub value: CommonnessValue,
    pub is_override: bool,
}

// ─── 备份 ──────────────────────────────────────────────────────────────────────

/// 备份记录。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRecord {
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

// ─── 安装包（用于目录包管理） ──────────────────────────────────────────────────

/// 已安装的数据包记录。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPackage {
    pub id: String,
    pub package_type: String, // "application" | "catalog" | "rules" | "adapter"
    pub version: String,
    pub sha256: Option<String>,
    pub signature_fingerprint: Option<String>,
    pub active: bool,
    pub source: Option<String>,
    pub content_path: Option<String>,
    pub installed_at: String,
}

// ─── 规则版本与档案启用状态 ──────────────────────────────────────────────────

/// 已保存的规则版本；规则正文保存为已校验的规范 JSON，避免数据库成为未验证脚本容器。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleVersionRecord {
    pub id: String,
    pub preset_id: String,
    pub version: String,
    pub schema_version: u32,
    pub title: String,
    pub author: String,
    pub status: String,
    pub origin: String,
    pub canonical_json: String,
    pub canonical_sha256: String,
    pub parent_version_id: Option<String>,
    pub read_only: bool,
    pub created_at: String,
}

/// 某个游戏档案对规则版本的启用状态；规则版本本身不携带档案状态。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleActivation {
    pub profile_id: String,
    pub rule_version_id: String,
    pub enabled: bool,
    pub position: u32,
    pub note: Option<String>,
    pub enabled_at: String,
}

// ─── 目录与包的种子数据 ─────────────────────────────────────────────────────────

/// 内置御魂套装记录；编号同时作为离线图标文件名和 MuMu 压缩编码的稳定映射。
#[derive(Clone, Copy, Debug)]
pub struct BuiltinSoulSet {
    pub code: u32,
    pub set_id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub four_piece_effect: Option<&'static str>,
}

/// 根据普通二件套分类返回展示文本；首领和星痕御魂由套装名称映射到各自的特殊二件套效果。
pub fn two_piece_effect_for_category(category: &str) -> Option<&'static str> {
    match category {
        "攻击加成" => Some("攻击加成+15%"),
        "生命加成" => Some("生命加成+15%"),
        "防御加成" => Some("防御加成+30%"),
        "暴击" => Some("暴击+15%"),
        "暴击伤害" => Some("暴击伤害+20%"),
        "效果命中" => Some("效果命中+15%"),
        "效果抵抗" => Some("效果抵抗+15%"),
        _ => None,
    }
}

/// 返回不适用标准四件套判定的特殊类别；普通御魂始终走标准四件套校验。
pub fn builtin_special_category(name: &str) -> Option<&'static str> {
    match name {
        "土蜘蛛" | "胧车" | "荒骷髅" | "地震鲶" | "蜃气楼" | "鬼灵歌伎" | "夜荒魂" => {
            Some("首领御魂")
        }
        "八咫镜" | "天羽羽斩" | "预言星盘" | "月之石" | "纺缘锤" | "稻荷穗箭" => {
            Some("星痕御魂")
        }
        _ => None,
    }
}

/// 返回首领和星痕御魂的二件套特殊效果；这些类别没有四件套效果。
pub fn builtin_special_two_piece_effect(name: &str) -> Option<&'static str> {
    match name {
        "土蜘蛛" => Some(
            "唯一被动：对怪物造成伤害时，为其附加1层土蜘蛛印记，降低10%速度，并造成10%间接伤害，持续1回合，上限3层。",
        ),
        "胧车" => {
            Some("唯一被动：受到怪物伤害时，有50%概率增加30%行动条，单次攻击内最多触发1次。")
        }
        "荒骷髅" => {
            Some("唯一被动：提升10%对怪物伤害；若行动前受到过怪物伤害，本次伤害提升至25%。")
        }
        "地震鲶" => Some(
            "唯一被动：怪物战斗开始时获得60%伤害减免；每次受到攻击时，将6%伤害减免转化为1.5%伤害增益，多段攻击单次触发1次。",
        ),
        "蜃气楼" => {
            Some("唯一被动：怪物战斗开始时生成可抵挡1次控制效果的护盾；护盾消失5回合后再次生成。")
        }
        "鬼灵歌伎" => Some(
            "唯一被动：每对怪物造成5次伤害后，下一次对怪物造成伤害时造成其生命上限20%的无视防御伤害，最高不超过攻击的255%。",
        ),
        "夜荒魂" => Some(
            "唯一被动：携带者回合内对怪物造成控制效果或放逐时，每命中1个目标，使全体非召唤物友方提升15点速度；最多提升90点，持续1回合。",
        ),
        // 八咫镜二件套仅在与怪物战斗时触发，第二次施加光灼会转化为伤害提升。
        "八咫镜" => Some("唯一效果：与怪物战斗时，造成伤害施加光灼；再次施加光灼时移除并提升20%伤害。"),
        "天羽羽斩" => Some("战斗开始时，获得50点无视防御。"),
        "预言星盘" => Some("唯一效果：施放妖术技能后，提升阴阳师暴击，提升值为自身暴击的15%。"),
        "月之石" => Some(
            "唯一效果：每回合友方首次受到来自敌方的减益和控制效果时，降低来源20%效果命中，持续1回合。",
        ),
        "纺缘锤" => Some("受到伤害后提升20%减伤，持续1回合。"),
        "稻荷穗箭" => {
            Some("唯一效果：回合结束后，提升阴阳师10%行动条（阴阳师回合开始前限1次）。")
        }
        _ => None,
    }
}

/// 生成目录中的二件套说明；特殊类别只保留首领与星痕的专属效果，普通套装使用分类属性。
pub fn builtin_effect_description(name: &str, category: &str) -> Option<String> {
    builtin_special_two_piece_effect(name)
        .or_else(|| two_piece_effect_for_category(category))
        .map(str::to_owned)
}

/// 统一返回完整编号；第三方页面和本地离线图标均使用 300000 + 压缩编码。
pub fn soul_icon_asset_id(code: u32) -> u32 {
    300_000 + code
}

/// 按 MuMu 压缩编码查询稳定套装名，导入器与图鉴共用一份目录，避免两套映射漂移。
pub fn builtin_soul_set_name_by_code(code: u64) -> Option<&'static str> {
    BUILTIN_SOUL_SETS
        .iter()
        .find(|record| u64::from(record.code) == code)
        .map(|record| record.set_id)
}

/// 内置御魂套装种子。旧套装说明来自公开游戏数据快照；未完成当前版本复核的说明保持为空。
pub const BUILTIN_SOUL_SETS: &[BuiltinSoulSet] = &[
    BuiltinSoulSet {
        code: 2,
        set_id: "雪幽魂",
        name: "雪幽魂",
        category: "防御加成",
        four_piece_effect: Some(
            "造成伤害时，有15%（目标带有减速效果时为30%）基础概率冰冻1回合；受到攻击时，使攻击者减速30点，持续1回合。",
        ),
    },
    BuiltinSoulSet {
        code: 3,
        set_id: "地藏像",
        name: "地藏像",
        category: "生命加成",
        four_piece_effect: Some(
            "受到暴击时，自身100%、友方30%概率获得1回合护盾，能吸收10%生命上限的伤害。",
        ),
    },
    BuiltinSoulSet {
        code: 4,
        set_id: "蝠翼",
        name: "蝠翼",
        category: "攻击加成",
        four_piece_effect: Some("造成伤害时，附带20%吸血。"),
    },
    BuiltinSoulSet {
        code: 6,
        set_id: "涅槃之火",
        name: "涅槃之火",
        category: "生命加成",
        four_piece_effect: Some("回合结束时，若生命比例低于30%，治疗生命上限15%的生命。"),
    },
    BuiltinSoulSet {
        code: 7,
        set_id: "三味",
        name: "三味",
        category: "暴击",
        four_piece_effect: Some(
            "任一友方承受控制效果时，使其提升30点速度，持续2回合；该增益不可驱散，可叠加2层。",
        ),
    },
    BuiltinSoulSet {
        code: 8,
        set_id: "魍魉之匣",
        name: "魍魉之匣",
        category: "效果抵抗",
        four_piece_effect: Some(
            "造成伤害时，有25%基础概率随机附加眩晕、沉默、减疗或混乱，持续1回合。",
        ),
    },
    BuiltinSoulSet {
        code: 9,
        set_id: "被服",
        name: "被服",
        category: "生命加成",
        four_piece_effect: Some("受到的伤害降低30%。"),
    },
    BuiltinSoulSet {
        code: 10,
        set_id: "招财猫",
        name: "招财猫",
        category: "防御加成",
        four_piece_effect: Some("回合开始时，有50%概率获得2点鬼火。"),
    },
    BuiltinSoulSet {
        code: 11,
        set_id: "反枕",
        name: "反枕",
        category: "防御加成",
        four_piece_effect: Some("造成伤害时，有23%基础概率使目标沉睡1回合。"),
    },
    BuiltinSoulSet {
        code: 12,
        set_id: "轮入道",
        name: "轮入道",
        category: "攻击加成",
        four_piece_effect: Some("回合结束时，有20%概率获得新的回合。"),
    },
    BuiltinSoulSet {
        code: 13,
        set_id: "日女巳时",
        name: "日女巳时",
        category: "防御加成",
        four_piece_effect: Some(
            "造成伤害时，有20%概率击退目标30%行动条；目标带有增益或印记时，触发概率提升10%。",
        ),
    },
    BuiltinSoulSet {
        code: 14,
        set_id: "镜姬",
        name: "镜姬",
        category: "生命加成",
        four_piece_effect: Some("受到伤害时，有30%概率造成100%反伤。"),
    },
    BuiltinSoulSet {
        code: 15,
        set_id: "钟灵",
        name: "钟灵",
        category: "生命加成",
        four_piece_effect: Some(
            "造成伤害时，有10%基础概率使目标眩晕1回合；敌方无人眩晕时基础概率改为20%。",
        ),
    },
    BuiltinSoulSet {
        code: 18,
        set_id: "狰",
        name: "狰",
        category: "攻击加成",
        four_piece_effect: Some("受到敌方伤害时，有35%概率反击。"),
    },
    BuiltinSoulSet {
        code: 19,
        set_id: "火灵",
        name: "火灵",
        category: "效果命中",
        four_piece_effect: Some("战斗开始时，获得3点鬼火。"),
    },
    BuiltinSoulSet {
        code: 20,
        set_id: "鸣屋",
        name: "鸣屋",
        category: "攻击加成",
        four_piece_effect: Some("攻击时，若目标带有控制效果，提升45%伤害。"),
    },
    BuiltinSoulSet {
        code: 21,
        set_id: "薙魂",
        name: "薙魂",
        category: "生命加成",
        four_piece_effect: Some(
            "友方被攻击时有50%概率守护，使其中的单体伤害降低20%，再分担50%，每次攻击最多触发一次。",
        ),
    },
    BuiltinSoulSet {
        code: 22,
        set_id: "心眼",
        name: "心眼",
        category: "攻击加成",
        four_piece_effect: Some("造成伤害时，目标生命比例每降低15%，提升10%伤害。"),
    },
    BuiltinSoulSet {
        code: 23,
        set_id: "木魅",
        name: "木魅",
        category: "防御加成",
        four_piece_effect: Some(
            "任一友方受到伤害时，有25%概率削减伤害者1点鬼火，单次攻击最多触发1次。",
        ),
    },
    BuiltinSoulSet {
        code: 24,
        set_id: "树妖",
        name: "树妖",
        category: "生命加成",
        four_piece_effect: Some("治疗时，增加20%基础治疗；目标生命低于20%时改为增加50%。"),
    },
    BuiltinSoulSet {
        code: 26,
        set_id: "网切",
        name: "网切",
        category: "暴击",
        four_piece_effect: Some("攻击时，50%概率无视45%防御。"),
    },
    BuiltinSoulSet {
        code: 27,
        set_id: "阴摩罗",
        name: "阴摩罗",
        category: "攻击加成",
        four_piece_effect: Some("击杀目标时，获得3点鬼火。"),
    },
    BuiltinSoulSet {
        code: 29,
        set_id: "伤魂鸟",
        name: "伤魂鸟",
        category: "暴击",
        four_piece_effect: Some(
            "任一非怪物目标阵亡时，治疗生命上限20%的生命，并提升20%伤害（上限120%）直到战斗结束。",
        ),
    },
    BuiltinSoulSet {
        code: 30,
        set_id: "破势",
        name: "破势",
        category: "暴击",
        four_piece_effect: Some("造成伤害时，若目标生命比例高于70%，提升40%伤害。"),
    },
    BuiltinSoulSet {
        code: 31,
        set_id: "镇墓兽",
        name: "镇墓兽",
        category: "暴击",
        four_piece_effect: Some("生命比例每降低1%，提升0.5%暴击伤害。"),
    },
    BuiltinSoulSet {
        code: 32,
        set_id: "珍珠",
        name: "珍珠",
        category: "防御加成",
        four_piece_effect: Some(
            "治疗时，目标获得不可驱散的护盾2回合，能吸收等同基础治疗30%的伤害。",
        ),
    },
    BuiltinSoulSet {
        code: 33,
        set_id: "骰子鬼",
        name: "骰子鬼",
        category: "效果抵抗",
        four_piece_effect: Some(
            "抵抗时反击来源目标，该次反击提升50%伤害；未被控制时承受控制效果，增加25%行动条。",
        ),
    },
    BuiltinSoulSet {
        code: 34,
        set_id: "蚌精",
        name: "蚌精",
        category: "效果命中",
        four_piece_effect: Some(
            "战斗开始时，友方全体获得无法驱散的护盾1回合，能吸收等同生命上限10%的伤害。",
        ),
    },
    BuiltinSoulSet {
        code: 35,
        set_id: "魅妖",
        name: "魅妖",
        category: "防御加成",
        four_piece_effect: Some("造成伤害时，有25%基础概率使目标混乱1回合。"),
    },
    BuiltinSoulSet {
        code: 36,
        set_id: "针女",
        name: "针女",
        category: "暴击",
        four_piece_effect: Some(
            "暴击时，有40%概率对目标造成其生命上限10%的无视防御伤害，最高不超过攻击120%。",
        ),
    },
    BuiltinSoulSet {
        code: 39,
        set_id: "返魂香",
        name: "返魂香",
        category: "效果抵抗",
        four_piece_effect: Some("受到伤害时，有25%基础概率使伤害者眩晕1回合。"),
    },
    BuiltinSoulSet {
        code: 48,
        set_id: "狂骨",
        name: "狂骨",
        category: "攻击加成",
        four_piece_effect: Some("造成伤害时，每拥有1点鬼火，提升8%伤害。"),
    },
    BuiltinSoulSet {
        code: 49,
        set_id: "幽谷响",
        name: "幽谷响",
        category: "效果抵抗",
        four_piece_effect: Some("抵抗控制效果时，有50%概率将该效果反弹给来源目标，且必定命中。"),
    },
    BuiltinSoulSet {
        code: 50,
        set_id: "土蜘蛛",
        name: "土蜘蛛",
        category: "首领御魂",
        four_piece_effect: None,
    },
    BuiltinSoulSet {
        code: 51,
        set_id: "胧车",
        name: "胧车",
        category: "首领御魂",
        four_piece_effect: None,
    },
    BuiltinSoulSet {
        code: 52,
        set_id: "荒骷髅",
        name: "荒骷髅",
        category: "首领御魂",
        four_piece_effect: None,
    },
    BuiltinSoulSet {
        code: 53,
        set_id: "地震鲶",
        name: "地震鲶",
        category: "首领御魂",
        four_piece_effect: None,
    },
    BuiltinSoulSet {
        code: 54,
        set_id: "蜃气楼",
        name: "蜃气楼",
        category: "首领御魂",
        four_piece_effect: None,
    },
    BuiltinSoulSet {
        code: 55,
        set_id: "片叶之苇",
        name: "片叶之苇",
        category: "暴击",
        four_piece_effect: Some("当前生命等于最大生命时，造成伤害提升45%。"),
    },
    BuiltinSoulSet {
        code: 56,
        set_id: "尘冢",
        name: "尘冢",
        category: "攻击加成",
        four_piece_effect: Some(
            "行动时，伤害提升25%；友方非召唤物目标每比敌方多一个，伤害额外提升4%，最多提升至45%。",
        ),
    },
    BuiltinSoulSet {
        code: 57,
        set_id: "油赤子",
        name: "油赤子",
        category: "效果命中",
        four_piece_effect: Some(
            "唯一效果：战斗开始时获得2层灵元；每层提升友方全体8%的防御，友方施放妖术时若鬼火不足，可替代等量鬼火。",
        ),
    },
    BuiltinSoulSet {
        code: 58,
        set_id: "夜啼石",
        name: "夜啼石",
        category: "生命加成",
        four_piece_effect: Some(
            "友方非召唤物目标阵亡时，获得其初始攻击、防御与生命的30%，持续2回合；上限5层，且最多不超过携带者初始属性的150%。",
        ),
    },
    BuiltinSoulSet {
        code: 59,
        set_id: "夜送犬",
        name: "夜送犬",
        category: "效果抵抗",
        four_piece_effect: Some(
            "每回合首次受到来自敌方的控制效果时，获得1层夜行；夜行提升80%抵抗，上限3层；携带者回合开始时，若处于可行动状态，移除此效果。",
        ),
    },
    BuiltinSoulSet {
        code: 60,
        set_id: "雨降",
        name: "雨降",
        category: "防御加成",
        four_piece_effect: Some(
            "生命首次低于30%时，将自身生命上限降低至1点并获得3层雨露；雨露：受到伤害时，消耗1层并吸收此次行动内的所有伤害。",
        ),
    },
    BuiltinSoulSet {
        code: 73,
        set_id: "飞缘魔",
        name: "飞缘魔",
        category: "效果命中",
        four_piece_effect: Some("附加负面状态时，无视30%总效果抵抗。"),
    },
    BuiltinSoulSet {
        code: 74,
        set_id: "兵主部",
        name: "兵主部",
        category: "攻击加成",
        four_piece_effect: Some(
            "回合结束后获得1层兵刃；每层在造成伤害时无视目标75点防御，上限3层。",
        ),
    },
    BuiltinSoulSet {
        code: 75,
        set_id: "青女房",
        name: "青女房",
        category: "暴击",
        four_piece_effect: Some("首次受到致命伤害时恢复生命并冰封自身1回合；每回目仅触发一次。"),
    },
    BuiltinSoulSet {
        code: 76,
        set_id: "涂佛",
        name: "涂佛",
        category: "生命加成",
        four_piece_effect: Some(
            "回合结束时，若本回合普攻或无法动作，使友方全体提升效果抵抗和伤害，持续2回合。",
        ),
    },
    BuiltinSoulSet {
        code: 77,
        set_id: "鬼灵歌伎",
        name: "鬼灵歌伎",
        category: "首领御魂",
        four_piece_effect: None,
    },
    BuiltinSoulSet {
        code: 79,
        set_id: "遗念火",
        name: "遗念火",
        category: "效果命中",
        four_piece_effect: Some(
            "回合开始时获得1层念火，上限3层；施放技能时可消耗念火降低鬼火消耗。",
        ),
    },
    BuiltinSoulSet {
        code: 80,
        set_id: "共潜",
        name: "共潜",
        category: "效果抵抗",
        four_piece_effect: Some("回合结束时，驱散己方全体1个减益效果。"),
    },
    BuiltinSoulSet {
        code: 81,
        set_id: "恶楼",
        name: "恶楼",
        category: "生命加成",
        four_piece_effect: Some(
            "战斗开始时，获得恶楼之力（增伤80%、减伤80%）；在携带者的前8回合，恶楼之力暂时被封禁。",
        ),
    },
    BuiltinSoulSet {
        code: 82,
        set_id: "贝吹坊",
        name: "贝吹坊",
        category: "攻击加成",
        four_piece_effect: Some(
            "携带者回合开始前获得一层可抵挡一次伤害的贝甲；贝甲存在时提升携带者25%攻击。",
        ),
    },
    BuiltinSoulSet {
        code: 83,
        set_id: "海月火玉",
        name: "海月火玉",
        category: "暴击",
        four_piece_effect: Some("施放技能时，若鬼火充足，额外消耗鬼火以提升本次伤害。"),
    },
    BuiltinSoulSet {
        code: 84,
        set_id: "出世螺",
        name: "出世螺",
        category: "防御加成",
        four_piece_effect: Some(
            "每次受到伤害后恢复该伤害10%的生命值；在战斗开始以及自身行动前获得螺壳（上限1层），单次受到超过最大生命值60%的伤害时，将该伤害降低为最大生命值的60%，并移除螺壳。",
        ),
    },
    BuiltinSoulSet {
        code: 85,
        set_id: "火之车",
        name: "火之车",
        category: "防御加成",
        four_piece_effect: Some(
            "携带者回合结束时，获得1层墓火；累计4层时清空层数并获得1个回合，且该回合鬼火消耗减少1点。",
        ),
    },
    BuiltinSoulSet {
        code: 86,
        set_id: "隐念",
        name: "隐念",
        category: "攻击加成",
        four_piece_effect: Some("对同一目标连续造成伤害时，伤害逐次提升。"),
    },
    BuiltinSoulSet {
        code: 87,
        set_id: "叠叩",
        name: "叠叩",
        category: "生命加成",
        four_piece_effect: Some(
            "唯一效果：若自身初始暴击超过120%，全体非召唤物友方减少15%受到的暴击伤害。",
        ),
    },
    BuiltinSoulSet {
        code: 88,
        set_id: "应声虫",
        name: "应声虫",
        category: "暴击",
        four_piece_effect: Some("携带者获得20%协战概率。"),
    },
    BuiltinSoulSet {
        code: 89,
        set_id: "元兴寺",
        name: "元兴寺",
        category: "效果命中",
        four_piece_effect: Some(
            "唯一效果：携带者回合内造成控制效果时，每命中1个目标，使全体非召唤物友方造成的伤害提升5%；单次控制重复命中同目标时仅生效1次，最多提升40%，持续2回合。",
        ),
    },
    BuiltinSoulSet {
        code: 90,
        set_id: "钓瓶火",
        name: "钓瓶火",
        category: "效果抵抗",
        four_piece_effect: Some(
            "回合结束后，额外推进1格鬼火行动条，并为当前生命比例最低的非召唤物友方目标治疗自身防御700%的生命。",
        ),
    },
    BuiltinSoulSet {
        code: 91,
        set_id: "夜荒魂",
        name: "夜荒魂",
        category: "首领御魂",
        four_piece_effect: None,
    },
    BuiltinSoulSet {
        code: 92,
        set_id: "无刀取",
        name: "无刀取",
        category: "暴击伤害",
        four_piece_effect: Some("回合结束后，永久提升15%的伤害，最高不超过45%。"),
    },
    BuiltinSoulSet {
        code: 93,
        set_id: "奉海图",
        name: "奉海图",
        category: "防御加成",
        four_piece_effect: Some(
            "唯一效果：友方受到攻击时获得海图守护，持续1回合；海图守护降低此次伤害的30%（不超过携带者生命上限的35%）并记录，消失时失去等同于记录值的生命。",
        ),
    },
    BuiltinSoulSet {
        code: 94,
        set_id: "八咫镜",
        name: "八咫镜",
        category: "星痕御魂",
        four_piece_effect: None,
    },
    BuiltinSoulSet {
        code: 95,
        set_id: "天羽羽斩",
        name: "天羽羽斩",
        category: "星痕御魂",
        four_piece_effect: None,
    },
    BuiltinSoulSet {
        code: 96,
        set_id: "预言星盘",
        name: "预言星盘",
        category: "星痕御魂",
        four_piece_effect: None,
    },
    BuiltinSoulSet {
        code: 97,
        set_id: "月之石",
        name: "月之石",
        category: "星痕御魂",
        four_piece_effect: None,
    },
    BuiltinSoulSet {
        code: 98,
        set_id: "纺缘锤",
        name: "纺缘锤",
        category: "星痕御魂",
        four_piece_effect: None,
    },
    BuiltinSoulSet {
        code: 99,
        set_id: "稻荷穗箭",
        name: "稻荷穗箭",
        category: "星痕御魂",
        four_piece_effect: None,
    },
];

/// 内置御魂分组种子。
pub const BUILTIN_GROUPS: &[(&str, &str, Option<&str>)] = &[
    ("output", "通用输出", None),
    ("support", "辅助", None),
    ("control", "控制", None),
    ("speed", "速度", None),
    ("defense-output", "防御输出", None),
    ("resist-output", "抵抗输出", None),
    ("special", "特殊式神", None),
    ("survival", "生存", None),
];

/// 内置分组与套装的关联（所属分组）。
pub const BUILTIN_GROUP_MEMBERS: &[(&str, &str)] = &[
    ("output", "破势"),
    ("output", "针女"),
    ("output", "海月火玉"),
    ("output", "隐念"),
    ("support", "招财猫"),
    ("support", "火灵"),
    ("support", "蚌精"),
    ("support", "共潜"),
    ("control", "魅妖"),
    ("control", "雪幽魂"),
    ("speed", "招财猫"),
    ("speed", "遗念火"),
    ("defense-output", "招财猫"),
    ("survival", "蚌精"),
    ("survival", "共潜"),
    ("special", "遗念火"),
];

/// 内置 PVE 套装常用度默认值（待校准，标记为 draft）。
pub const BUILTIN_PVE_COMMONNESS: &[(&str, &str)] = &[
    ("破势", "common"),
    ("针女", "common"),
    ("招财猫", "common"),
    ("火灵", "common"),
    ("蚌精", "common"),
    ("遗念火", "common"),
    ("共潜", "uncommon"),
    ("海月火玉", "common"),
    ("魅妖", "uncommon"),
    ("雪幽魂", "uncommon"),
    ("隐念", "common"),
];

/// 内置 PVP 套装常用度默认值（待校准，标记为 draft）。
pub const BUILTIN_PVP_COMMONNESS: &[(&str, &str)] = &[
    ("破势", "common"),
    ("针女", "common"),
    ("招财猫", "common"),
    ("火灵", "common"),
    ("蚌精", "common"),
    ("遗念火", "common"),
    ("共潜", "common"),
    ("海月火玉", "uncommon"),
    ("魅妖", "common"),
    ("雪幽魂", "common"),
    ("隐念", "uncommon"),
];
