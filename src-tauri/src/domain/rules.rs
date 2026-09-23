//! 可解释、确定性的御魂规则引擎。
//!
//! 本模块只依赖领域事实和声明式规则，不读取文件、网络、系统时间或随机数。
//! 规则预设先经过结构与复杂度校验，再进入选择器、表达式、评分和续投评估，
//! 以保证相同输入始终得到相同的处置建议与解释哈希。

use super::models::{MaturityLevel, MaturityMode, MaturityThreshold, SnapshotSoul, SoulAttribute};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};

/// 规则文件的固定格式标识。
pub const RULE_PRESET_FORMAT: &str = "yys-rule-preset";
/// 当前支持的规则文件 schema 版本。
pub const RULE_PRESET_SCHEMA_VERSION: u32 = 1;
/// 首版允许的规则表达式节点数上限。
pub const MAX_EXPRESSION_NODES: usize = 1_000;
/// 首版允许的规则表达式嵌套深度上限。
pub const MAX_EXPRESSION_DEPTH: usize = 32;
/// 首版允许的解压后预设大小上限（5 MiB）。
pub const MAX_PRESET_BYTES: usize = 5 * 1024 * 1024;
/// 首版单个预设允许的规则数量上限。
pub const MAX_RULE_COUNT: usize = 500;

/// 规则表达式可以引用的规范属性 ID。
pub const NORMALIZED_ATTRIBUTE_IDS: &[&str] = &[
    "attack_flat",
    "attack_rate",
    "hp_flat",
    "hp_rate",
    "defense_flat",
    "defense_rate",
    "speed",
    "crit_rate",
    "crit_damage",
    "effect_hit",
    "effect_resist",
];

/// 规则预设的生命周期状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleStatus {
    Draft,
    Verified,
    Deprecated,
}

impl RuleStatus {
    /// 返回分享格式中的稳定状态值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Verified => "verified",
            Self::Deprecated => "deprecated",
        }
    }
}

/// 规则依据的证据等级；证据等级不自动决定规则是否启用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLevel {
    Official,
    OpenSource,
    Author,
    Community,
    Inferred,
    Unverified,
}

/// 预设默认值，决定简单胚子规则使用的基础范围。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RulePresetDefaults {
    #[serde(default)]
    pub quality: Vec<u8>,
    #[serde(default)]
    pub level: Vec<u8>,
    #[serde(default)]
    pub continuation_policy_ref: Option<String>,
}

/// 可执行的规则预设文件。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RulePreset {
    pub format: String,
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    pub title: String,
    pub author: String,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub game_version_range: Option<String>,
    #[serde(default)]
    pub minimum_app_version: Option<String>,
    pub status: RuleStatus,
    #[serde(default)]
    pub source_refs: Vec<String>,
    #[serde(default)]
    pub source_text: Vec<String>,
    #[serde(default)]
    pub raw_description: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub change_summary: Option<String>,
    #[serde(default)]
    pub author_signature: Option<String>,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub defaults: RulePresetDefaults,
    /// 列表评分的颜色分级阈值；不参与标准评分数值，只决定界面视觉层级。
    #[serde(default)]
    pub score_color_thresholds: ScoreColorThresholds,
    pub rules: Vec<RuleDefinition>,
}

/// 标准评分在列表中的三档颜色阈值；旧规则文件缺少该字段时使用 60/70/75。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScoreColorThresholds {
    #[serde(default = "default_jade_score")]
    pub jade_score: f64,
    #[serde(default = "default_gold_score")]
    pub gold_score: f64,
    #[serde(default = "default_rainbow_score")]
    pub rainbow_score: f64,
}

fn default_jade_score() -> f64 {
    60.0
}

fn default_gold_score() -> f64 {
    70.0
}

fn default_rainbow_score() -> f64 {
    75.0
}

impl Default for ScoreColorThresholds {
    fn default() -> Self {
        Self {
            jade_score: default_jade_score(),
            gold_score: default_gold_score(),
            rainbow_score: default_rainbow_score(),
        }
    }
}

/// 规则执行阶段；首版预设以 admission 为主，也允许评分和续投规则进入同一格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleStage {
    Admission,
    Continuation,
    Scoring,
    Template,
}

impl RuleStage {
    /// 解析机器可读的阶段名，未知值必须拒绝而不是静默降级。
    fn parse(value: &str) -> Option<Self> {
        match value {
            "admission" => Some(Self::Admission),
            "continuation" => Some(Self::Continuation),
            "scoring" => Some(Self::Scoring),
            "template" => Some(Self::Template),
            _ => None,
        }
    }
}

/// 预设中的单条规则。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleDefinition {
    pub id: String,
    pub stage: String,
    #[serde(default)]
    pub raw_label: Option<String>,
    pub name: String,
    pub status: RuleStatus,
    #[serde(default)]
    pub evidence_level: Option<EvidenceLevel>,
    #[serde(default)]
    pub source_refs: Vec<String>,
    #[serde(default)]
    pub game_version_range: Option<String>,
    #[serde(default)]
    pub selector: Selector,
    #[serde(default)]
    pub candidate_uses: Vec<String>,
    #[serde(default)]
    pub expression: Option<RuleExpression>,
    /// 用户可编辑的评分权重和阈值；准入选择器仍负责决定规则是否命中。
    #[serde(default)]
    pub scoring: Option<RuleScoring>,
}

/// 一条手动评分规则的可分享配置；数值只描述评分标准，不绑定具体御魂。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleScoring {
    #[serde(default)]
    pub core_attributes: Vec<String>,
    #[serde(default)]
    pub general_attributes: Vec<String>,
    #[serde(default = "default_core_attribute_weight")]
    pub core_attribute_weight: f64,
    #[serde(default = "default_general_attribute_weight")]
    pub general_attribute_weight: f64,
    #[serde(default = "default_enhancement_core_weight")]
    pub enhancement_core_weight: f64,
    #[serde(default = "default_enhancement_general_weight")]
    pub enhancement_general_weight: f64,
    #[serde(default = "default_keep_score")]
    pub keep_score: f64,
    #[serde(default = "default_observe_score")]
    pub observe_score: f64,
    #[serde(default)]
    pub discard_score: f64,
}

fn default_core_attribute_weight() -> f64 {
    10.0
}

fn default_general_attribute_weight() -> f64 {
    5.0
}

fn default_enhancement_core_weight() -> f64 {
    10.0
}

fn default_enhancement_general_weight() -> f64 {
    5.0
}

fn default_keep_score() -> f64 {
    70.0
}

fn default_observe_score() -> f64 {
    50.0
}

/// 号位与主属性的配对条件。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SlotMainPair {
    pub slot: u8,
    pub main_attribute: String,
}

/// 副属性至少命中条件。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AtLeastSubstats {
    pub count: usize,
    #[serde(default)]
    pub from: Vec<String>,
}

/// 副属性组合条件。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequiredSubstats {
    #[serde(default)]
    pub all_of: Vec<String>,
    #[serde(default)]
    pub any_of: Vec<String>,
    #[serde(default)]
    pub at_least: Option<AtLeastSubstats>,
}

/// 选择器的有限布尔组合；不支持循环、脚本或动态字段。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selector {
    #[serde(default)]
    pub quality: Vec<u8>,
    #[serde(default)]
    pub level: Vec<u8>,
    #[serde(default)]
    pub slots: Vec<u8>,
    #[serde(default)]
    pub main_attributes: Vec<String>,
    #[serde(default)]
    pub initial_substat_counts: Vec<u8>,
    #[serde(default)]
    pub set_ids: Vec<String>,
    #[serde(default)]
    pub set_group_ids: Vec<String>,
    #[serde(default)]
    pub set_commonness: Vec<super::models::CommonnessValue>,
    #[serde(default)]
    pub scenarios: Vec<String>,
    #[serde(default)]
    pub slot_main_pairs: Vec<SlotMainPair>,
    #[serde(default)]
    pub required_substats: RequiredSubstats,
    #[serde(default)]
    pub forbidden_substats: Vec<String>,
    #[serde(default)]
    pub all: Vec<Selector>,
    #[serde(default)]
    pub any: Vec<Selector>,
    #[serde(default)]
    pub not: Option<Box<Selector>>,
}

/// 表达式节点。字段采用受限结构，未知字段在反序列化时直接拒绝。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleExpression {
    pub op: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default)]
    pub args: Vec<RuleExpression>,
    #[serde(default)]
    pub left: Option<Box<RuleExpression>>,
    #[serde(default)]
    pub right: Option<Box<RuleExpression>>,
    #[serde(default)]
    pub lower: Option<Box<RuleExpression>>,
    #[serde(default)]
    pub upper: Option<Box<RuleExpression>>,
    #[serde(default)]
    pub condition: Option<Box<RuleExpression>>,
    #[serde(default, rename = "then")]
    pub then_branch: Option<Box<RuleExpression>>,
    #[serde(default, rename = "else")]
    pub else_branch: Option<Box<RuleExpression>>,
    #[serde(default, rename = "onZero")]
    pub on_zero: Option<String>,
    #[serde(default)]
    pub weights: Vec<f64>,
}

/// 可用于评分的归一化尺度。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Normalization {
    pub kind: String,
    pub zero_at: f64,
    pub target_at: f64,
    pub cap_at: f64,
    pub target_value: f64,
    pub cap_value: f64,
}

impl Default for Normalization {
    /// 默认将 0 到 1 的百分比型原始分数线性映射到 0 到 100。
    fn default() -> Self {
        Self {
            kind: "piecewise_linear".to_owned(),
            zero_at: 0.0,
            target_at: 80.0,
            cap_at: 100.0,
            target_value: 1.0,
            cap_value: 1.25,
        }
    }
}

impl Normalization {
    /// 把未归一化分数限制到声明的 0–100 区间。
    pub fn normalize(&self, value: f64) -> f64 {
        if !value.is_finite() {
            return 0.0;
        }
        if value <= self.zero_at {
            return 0.0;
        }
        if value <= self.target_value {
            return interpolate(value, self.zero_at, self.target_value, 0.0, self.target_at);
        }
        if value <= self.cap_value {
            return interpolate(
                value,
                self.target_value,
                self.cap_value,
                self.target_at,
                self.cap_at,
            );
        }
        self.cap_at.clamp(0.0, 100.0)
    }
}

/// 一条用途模板的硬约束。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HardConstraint {
    pub id: String,
    pub label: String,
    pub expression: RuleExpression,
    #[serde(default = "default_required_severity")]
    pub severity: String,
}

fn default_required_severity() -> String {
    "required".to_owned()
}

/// 属性转换：先把源属性按因子转换为目标属性，再参与评分。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttributeConversion {
    pub id: String,
    pub source_attribute: String,
    pub target_attribute: String,
    pub factor: f64,
    #[serde(default)]
    pub cap: Option<f64>,
    #[serde(default)]
    pub evidence_level: Option<EvidenceLevel>,
    #[serde(default)]
    pub source_refs: Vec<String>,
}

/// 评分中的条件分支；分支路径会进入解释树。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScoreBranch {
    pub id: String,
    pub label: String,
    pub condition: RuleExpression,
    #[serde(default)]
    pub when_true_weights: BTreeMap<String, f64>,
    #[serde(default)]
    pub when_false_weights: BTreeMap<String, f64>,
}

/// 代表性用途模板接口，支持特殊式神的转换、封顶、硬约束和条件分支。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UseTemplate {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub scenario: Option<String>,
    pub status: RuleStatus,
    pub evidence_level: EvidenceLevel,
    #[serde(default)]
    pub source_refs: Vec<String>,
    #[serde(default)]
    pub game_version_range: Option<String>,
    #[serde(default)]
    pub selector: Option<Selector>,
    #[serde(default)]
    pub effective_attributes: Vec<String>,
    #[serde(default)]
    pub weights: BTreeMap<String, f64>,
    #[serde(default)]
    pub normalization: Normalization,
    #[serde(default)]
    pub hard_constraints: Vec<HardConstraint>,
    #[serde(default)]
    pub conversions: Vec<AttributeConversion>,
    #[serde(default)]
    pub branches: Vec<ScoreBranch>,
    #[serde(default)]
    pub score_expression: Option<RuleExpression>,
    #[serde(default)]
    pub max_allowed_crooked_points: Option<u8>,
}

/// 用于规则评估的副属性事实。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SoulAttributeFact {
    pub attribute_type: String,
    pub value: f64,
    #[serde(default)]
    pub enhancement_count: Option<u8>,
    #[serde(default)]
    pub count_provenance: Option<String>,
}

/// 规则引擎读取的规范御魂事实；不包含数据库连接或 UI 状态。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SoulFacts {
    pub set_id: String,
    pub slot: u8,
    pub quality: u8,
    pub level: u8,
    pub main_attribute: String,
    pub main_value: f64,
    #[serde(default)]
    pub initial_substat_count: Option<u8>,
    #[serde(default)]
    pub attributes: Vec<SoulAttributeFact>,
    #[serde(default)]
    pub set_group_ids: Vec<String>,
    #[serde(default)]
    pub commonness: BTreeMap<String, super::models::CommonnessValue>,
    #[serde(default)]
    pub scenario: Option<String>,
}

impl SoulFacts {
    /// 从 Issue 03 保存的快照事实构建规则输入，并只读取属于当前御魂的属性。
    pub fn from_snapshot(
        soul: &SnapshotSoul,
        attributes: &[SoulAttribute],
        set_group_ids: Vec<String>,
        commonness: BTreeMap<String, super::models::CommonnessValue>,
        scenario: Option<String>,
    ) -> Self {
        let mut facts = Self {
            set_id: soul.set_id.clone(),
            slot: soul.slot,
            quality: soul.quality,
            level: soul.level,
            main_attribute: soul.main_attr_type.clone(),
            main_value: soul.main_attr_value,
            initial_substat_count: soul.initial_substat_count,
            attributes: attributes
                .iter()
                .filter(|attribute| {
                    attribute.snapshot_id == soul.snapshot_id
                        && attribute.soul_internal_id == soul.internal_id
                })
                .map(|attribute| SoulAttributeFact {
                    attribute_type: attribute.attribute_type.clone(),
                    value: attribute.value,
                    enhancement_count: attribute.enhancement_count,
                    count_provenance: Some(attribute.count_provenance.clone()),
                })
                .collect(),
            set_group_ids,
            commonness,
            scenario,
        };
        facts.sort_for_determinism();
        facts
    }

    /// 创建测试或上层编排使用的单属性输入。
    pub fn with_attributes(
        set_id: impl Into<String>,
        slot: u8,
        quality: u8,
        level: u8,
        main_attribute: impl Into<String>,
        main_value: f64,
        initial_substat_count: Option<u8>,
        attributes: Vec<SoulAttributeFact>,
    ) -> Self {
        let mut facts = Self {
            set_id: set_id.into(),
            slot,
            quality,
            level,
            main_attribute: main_attribute.into(),
            main_value,
            initial_substat_count,
            attributes,
            set_group_ids: Vec::new(),
            commonness: BTreeMap::new(),
            scenario: None,
        };
        facts.sort_for_determinism();
        facts
    }

    /// 设置套装分组和场景常用度，供预设选择器使用。
    pub fn with_catalog_context(
        mut self,
        set_group_ids: Vec<String>,
        commonness: BTreeMap<String, super::models::CommonnessValue>,
        scenario: Option<String>,
    ) -> Self {
        self.set_group_ids = set_group_ids;
        self.commonness = commonness;
        self.scenario = scenario;
        self.sort_for_determinism();
        self
    }

    /// 判断御魂是否拥有某个副属性。
    pub fn has_attribute(&self, attribute_type: &str) -> bool {
        self.attributes
            .iter()
            .any(|attribute| attribute.attribute_type == attribute_type)
    }

    /// 读取某个副属性的原始值；同一属性重复出现时取确定的最大值。
    pub fn attribute_value(&self, attribute_type: &str) -> f64 {
        self.attributes
            .iter()
            .filter(|attribute| attribute.attribute_type == attribute_type)
            .map(|attribute| attribute.value)
            .fold(0.0, f64::max)
    }

    /// 将属性、分组和常用度排序，避免来源数组顺序影响结果哈希。
    fn sort_for_determinism(&mut self) {
        self.attributes.sort_by(|left, right| {
            left.attribute_type
                .cmp(&right.attribute_type)
                .then_with(|| left.value.total_cmp(&right.value))
        });
        self.set_group_ids.sort();
    }
}

/// 表达式引用上下文；所有外部输入必须由调用方显式写入。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationContext {
    #[serde(default)]
    pub values: BTreeMap<String, Value>,
}

impl EvaluationContext {
    /// 将御魂事实映射到固定的 soul 路径。
    pub fn from_soul(facts: &SoulFacts) -> Self {
        let mut context = Self::default();
        context.set("soul.set_id", Value::String(facts.set_id.clone()));
        context.set("soul.slot", number(facts.slot as f64));
        context.set("soul.quality", number(facts.quality as f64));
        context.set("soul.level", number(facts.level as f64));
        context.set(
            "soul.main_attribute",
            Value::String(facts.main_attribute.clone()),
        );
        context.set("soul.main_value", number(facts.main_value));
        if let Some(count) = facts.initial_substat_count {
            context.set("soul.initial_substat_count", number(count as f64));
        }
        for attribute in facts.attributes.iter() {
            context.set(
                &format!("soul.{}", attribute.attribute_type),
                number(attribute.value),
            );
            if let Some(count) = attribute.enhancement_count {
                context.set(
                    &format!("soul.{}_enhancement_count", attribute.attribute_type),
                    number(count as f64),
                );
            }
        }
        context
    }

    /// 写入一个固定路径；路径校验在表达式引用时再次执行。
    pub fn set(&mut self, path: impl Into<String>, value: Value) {
        self.values.insert(path.into(), value);
    }

    /// 合并一组外部输入，用于 hero、team、target、context 和 mechanics 面板。
    pub fn with_values(mut self, values: impl IntoIterator<Item = (String, Value)>) -> Self {
        for (path, value) in values {
            self.set(path, value);
        }
        self
    }

    /// 读取完整路径，未知引用返回明确错误而不是伪造默认值。
    fn get(&self, path: &str) -> Result<&Value, ExpressionError> {
        self.values
            .get(path)
            .ok_or_else(|| ExpressionError::MissingReference(path.to_owned()))
    }
}

/// 规则校验限制，可在测试或受控预览场景缩小预算。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidationLimits {
    pub max_preset_bytes: usize,
    pub max_rules: usize,
    pub max_expression_nodes: usize,
    pub max_expression_depth: usize,
    pub max_string_length: usize,
    pub max_source_refs: usize,
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_preset_bytes: MAX_PRESET_BYTES,
            max_rules: MAX_RULE_COUNT,
            max_expression_nodes: MAX_EXPRESSION_NODES,
            max_expression_depth: MAX_EXPRESSION_DEPTH,
            max_string_length: 4_096,
            max_source_refs: 64,
        }
    }
}

/// 规则预设的结构或安全校验错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleValidationError {
    PayloadTooLarge { actual: usize, limit: usize },
    Json(String),
    InvalidField { field: String, message: String },
    DuplicateId(String),
    UnknownOperator(String),
    ExpressionTooLarge { nodes: usize, limit: usize },
    ExpressionTooDeep { depth: usize, limit: usize },
}

impl Display for RuleValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PayloadTooLarge { actual, limit } => {
                write!(formatter, "规则预设过大：{actual} 字节，限制 {limit} 字节")
            }
            Self::Json(message) => write!(formatter, "规则预设 JSON 无效：{message}"),
            Self::InvalidField { field, message } => {
                write!(formatter, "字段 {field} 无效：{message}")
            }
            Self::DuplicateId(id) => write!(formatter, "规则 ID 重复：{id}"),
            Self::UnknownOperator(op) => write!(formatter, "不允许的表达式操作：{op}"),
            Self::ExpressionTooLarge { nodes, limit } => {
                write!(formatter, "表达式节点数 {nodes} 超过限制 {limit}")
            }
            Self::ExpressionTooDeep { depth, limit } => {
                write!(formatter, "表达式深度 {depth} 超过限制 {limit}")
            }
        }
    }
}

impl std::error::Error for RuleValidationError {}

/// 通过校验后保存规范 JSON 和哈希，后续评估只接受该类型。
#[derive(Clone, Debug)]
pub struct ValidatedRulePreset {
    pub preset: RulePreset,
    pub canonical_json: String,
    pub normalized_hash: String,
}

impl RulePreset {
    /// 按默认安全预算校验预设。
    pub fn validate(self) -> Result<ValidatedRulePreset, RuleValidationError> {
        validate_rule_preset(self, ValidationLimits::default())
    }

    /// 使用自定义预算校验预设，供受控预览和测试使用。
    pub fn validate_with_limits(
        self,
        limits: ValidationLimits,
    ) -> Result<ValidatedRulePreset, RuleValidationError> {
        validate_rule_preset(self, limits)
    }
}

/// 解析规则预设载荷并立即完成大小、类型、操作符和复杂度校验。
pub fn parse_rule_preset(
    payload: &[u8],
    limits: ValidationLimits,
) -> Result<ValidatedRulePreset, RuleValidationError> {
    if payload.len() > limits.max_preset_bytes {
        return Err(RuleValidationError::PayloadTooLarge {
            actual: payload.len(),
            limit: limits.max_preset_bytes,
        });
    }
    let preset: RulePreset = serde_json::from_slice(payload)
        .map_err(|error| RuleValidationError::Json(error.to_string()))?;
    validate_rule_preset(preset, limits)
}

/// 从仓库中随版本编译的默认预设加载 28 条胚子规则。
pub fn load_default_preset() -> Result<ValidatedRulePreset, RuleValidationError> {
    const DEFAULT_PRESET: &str =
        include_str!("../../resources/presets/author-default-v0.1.0.yysrule.json");
    parse_rule_preset(DEFAULT_PRESET.as_bytes(), ValidationLimits::default())
}

/// 校验预设并生成字段顺序稳定的规范 JSON 与 SHA-256。
pub fn validate_rule_preset(
    preset: RulePreset,
    limits: ValidationLimits,
) -> Result<ValidatedRulePreset, RuleValidationError> {
    validate_text("id", &preset.id, limits.max_string_length)?;
    validate_text("version", &preset.version, limits.max_string_length)?;
    validate_text("title", &preset.title, limits.max_string_length)?;
    validate_text("author", &preset.author, limits.max_string_length)?;
    if preset.format != RULE_PRESET_FORMAT {
        return Err(invalid_field("format", "必须为 yys-rule-preset"));
    }
    if preset.schema_version != RULE_PRESET_SCHEMA_VERSION {
        return Err(invalid_field("schemaVersion", "不兼容的规则格式版本"));
    }
    if preset.rules.len() > limits.max_rules {
        return Err(invalid_field("rules", "规则数量超过限制"));
    }
    validate_refs("sourceRefs", &preset.source_refs, &limits)?;
    if preset
        .defaults
        .quality
        .iter()
        .any(|quality| *quality == 0 || *quality > 6)
    {
        return Err(invalid_field("defaults.quality", "星级必须位于 1 到 6"));
    }
    if preset.defaults.level.iter().any(|level| *level > 15) {
        return Err(invalid_field("defaults.level", "等级必须位于 0 到 15"));
    }
    // 颜色阈值必须递增且落在百分制内，避免评分颜色出现反向或无法触达的层级。
    let color_thresholds = [
        ("jadeScore", preset.score_color_thresholds.jade_score),
        ("goldScore", preset.score_color_thresholds.gold_score),
        ("rainbowScore", preset.score_color_thresholds.rainbow_score),
    ];
    if color_thresholds
        .iter()
        .any(|(_, value)| !value.is_finite() || !(0.0..=100.0).contains(value))
    {
        return Err(invalid_field(
            "scoreColorThresholds",
            "颜色阈值必须位于 0 到 100",
        ));
    }
    if !(preset.score_color_thresholds.jade_score <= preset.score_color_thresholds.gold_score
        && preset.score_color_thresholds.gold_score <= preset.score_color_thresholds.rainbow_score)
    {
        return Err(invalid_field(
            "scoreColorThresholds",
            "颜色阈值需要满足：青玉 ≤ 金橙 ≤ 彩虹",
        ));
    }

    let mut ids = BTreeSet::new();
    for rule in &preset.rules {
        if !ids.insert(rule.id.clone()) {
            return Err(RuleValidationError::DuplicateId(rule.id.clone()));
        }
        validate_text("rules[].id", &rule.id, limits.max_string_length)?;
        validate_text("rules[].name", &rule.name, limits.max_string_length)?;
        if RuleStage::parse(&rule.stage).is_none() {
            return Err(invalid_field("rules[].stage", "未知规则阶段"));
        }
        validate_refs("rules[].sourceRefs", &rule.source_refs, &limits)?;
        validate_selector(&rule.selector, &limits, "rules[].selector")?;
        if let Some(scoring) = &rule.scoring {
            validate_scoring(scoring, &limits, "rules[].scoring")?;
        }
        if rule.stage == "admission" && rule.candidate_uses.is_empty() {
            return Err(invalid_field(
                "rules[].candidateUses",
                "胚子准入规则至少需要一个候选用途",
            ));
        }
        if let Some(expression) = &rule.expression {
            validate_expression(expression, &limits)?;
        }
    }

    let json = serde_json::to_value(&preset)
        .map_err(|error| RuleValidationError::Json(error.to_string()))?;
    let canonical_json =
        canonical_json(&json).map_err(|error| RuleValidationError::Json(error.to_string()))?;
    let normalized_hash = sha256_hex(canonical_json.as_bytes());
    Ok(ValidatedRulePreset {
        preset,
        canonical_json,
        normalized_hash,
    })
}

/// 选择器检查的一条可读解释。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectorCheck {
    pub label: String,
    pub matched: bool,
    pub detail: String,
    #[serde(default)]
    pub children: Vec<SelectorCheck>,
}

/// 选择器完整结果，保留每个失败条件而不是只返回布尔值。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectorEvaluation {
    pub matched: bool,
    pub checks: Vec<SelectorCheck>,
}

impl Selector {
    /// 执行基础条件与有限布尔组合。
    pub fn evaluate(&self, facts: &SoulFacts) -> SelectorEvaluation {
        let mut checks = Vec::new();
        let mut matched = true;
        macro_rules! check {
            ($label:expr, $condition:expr, $detail:expr) => {{
                let passed = $condition;
                matched &= passed;
                checks.push(SelectorCheck {
                    label: $label.to_owned(),
                    matched: passed,
                    detail: $detail,
                    children: Vec::new(),
                });
            }};
        }

        if !self.quality.is_empty() {
            check!(
                "星级",
                self.quality.contains(&facts.quality),
                format!("当前 {} 星，允许 {:?}", facts.quality, self.quality)
            );
        }
        if !self.level.is_empty() {
            check!(
                "等级",
                self.level.contains(&facts.level),
                format!("当前 +{}，允许 {:?}", facts.level, self.level)
            );
        }
        if !self.slots.is_empty() {
            check!(
                "号位",
                self.slots.contains(&facts.slot),
                format!("当前 {} 号位，允许 {:?}", facts.slot, self.slots)
            );
        }
        if !self.main_attributes.is_empty() {
            check!(
                "主属性",
                self.main_attributes.contains(&facts.main_attribute),
                format!(
                    "当前 {}，允许 {:?}",
                    facts.main_attribute, self.main_attributes
                )
            );
        }
        if !self.initial_substat_counts.is_empty() {
            let passed = facts
                .initial_substat_count
                .is_some_and(|count| self.initial_substat_counts.contains(&count));
            check!(
                "初始腿数",
                passed,
                format!(
                    "当前 {:?} 腿，允许 {:?}",
                    facts.initial_substat_count, self.initial_substat_counts
                )
            );
        }
        if !self.set_ids.is_empty() {
            check!(
                "御魂套装",
                self.set_ids.iter().any(|set_id| set_id == &facts.set_id),
                format!("当前 {}，允许 {:?}", facts.set_id, self.set_ids)
            );
        }
        if !self.set_group_ids.is_empty() {
            let passed = self
                .set_group_ids
                .iter()
                .any(|group| facts.set_group_ids.iter().any(|item| item == group));
            check!(
                "御魂分组",
                passed,
                format!(
                    "当前 {:?}，要求 {:?}",
                    facts.set_group_ids, self.set_group_ids
                )
            );
        }
        if !self.set_commonness.is_empty() {
            let passed = facts
                .scenario
                .as_deref()
                .and_then(|scenario| facts.commonness.get(scenario))
                .is_some_and(|value| self.set_commonness.contains(value));
            check!(
                "套装常用度",
                passed,
                format!(
                    "场景 {:?} 的常用度应为 {:?}",
                    facts.scenario, self.set_commonness
                )
            );
        }
        if !self.scenarios.is_empty() {
            let passed = facts
                .scenario
                .as_deref()
                .is_some_and(|scenario| self.scenarios.iter().any(|item| item == scenario));
            check!(
                "场景",
                passed,
                format!("当前 {:?}，允许 {:?}", facts.scenario, self.scenarios)
            );
        }
        if !self.slot_main_pairs.is_empty() {
            let passed = self
                .slot_main_pairs
                .iter()
                .any(|pair| pair.slot == facts.slot && pair.main_attribute == facts.main_attribute);
            check!(
                "号位主属性配对",
                passed,
                format!(
                    "当前 ({}, {})，允许配对 {} 项",
                    facts.slot,
                    facts.main_attribute,
                    self.slot_main_pairs.len()
                )
            );
        }

        let required = &self.required_substats;
        if !required.all_of.is_empty() {
            let missing = required
                .all_of
                .iter()
                .filter(|attribute| !facts.has_attribute(attribute))
                .cloned()
                .collect::<Vec<_>>();
            check!(
                "必备副属性",
                missing.is_empty(),
                format!("缺少 {:?}", missing)
            );
        }
        if !required.any_of.is_empty() {
            let passed = required
                .any_of
                .iter()
                .any(|attribute| facts.has_attribute(attribute));
            check!(
                "候选副属性",
                passed,
                format!("至少需要 {:?} 中一项", required.any_of)
            );
        }
        if let Some(at_least) = &required.at_least {
            let count = at_least
                .from
                .iter()
                .filter(|attribute| facts.has_attribute(attribute))
                .count();
            check!(
                "副属性至少命中",
                count >= at_least.count,
                format!("命中 {count} 项，需要至少 {} 项", at_least.count)
            );
        }
        if !self.forbidden_substats.is_empty() {
            let forbidden = self
                .forbidden_substats
                .iter()
                .filter(|attribute| facts.has_attribute(attribute))
                .cloned()
                .collect::<Vec<_>>();
            check!(
                "禁止副属性",
                forbidden.is_empty(),
                format!("命中 {:?}", forbidden)
            );
        }

        for nested in &self.all {
            let result = nested.evaluate(facts);
            matched &= result.matched;
            checks.push(SelectorCheck {
                label: "all".to_owned(),
                matched: result.matched,
                detail: "所有嵌套选择器都必须命中".to_owned(),
                children: result.checks,
            });
        }
        if !self.any.is_empty() {
            let results = self
                .any
                .iter()
                .map(|item| item.evaluate(facts))
                .collect::<Vec<_>>();
            let any_matched = results.iter().any(|result| result.matched);
            matched &= any_matched;
            checks.push(SelectorCheck {
                label: "any".to_owned(),
                matched: any_matched,
                detail: "至少一个嵌套选择器必须命中".to_owned(),
                children: results
                    .into_iter()
                    .flat_map(|result| result.checks)
                    .collect(),
            });
        }
        if let Some(negated) = &self.not {
            let result = negated.evaluate(facts);
            let passed = !result.matched;
            matched &= passed;
            checks.push(SelectorCheck {
                label: "not".to_owned(),
                matched: passed,
                detail: "嵌套选择器不得命中".to_owned(),
                children: result.checks,
            });
        }

        SelectorEvaluation { matched, checks }
    }

    /// 只需要布尔结果时使用的便捷入口。
    pub fn matches(&self, facts: &SoulFacts) -> bool {
        self.evaluate(facts).matched
    }
}

/// 单条胚子规则的命中与解释。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionRuleResult {
    pub rule_id: String,
    pub name: String,
    pub status: RuleStatus,
    pub evidence_level: Option<EvidenceLevel>,
    pub candidate_uses: Vec<String>,
    pub matched: bool,
    pub selector: SelectorEvaluation,
}

/// 胚子准入总结果；重叠规则会保留全部候选用途。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionResult {
    pub preset_id: String,
    pub preset_version: String,
    pub preset_hash: String,
    pub admitted: bool,
    pub candidate_uses: Vec<String>,
    pub rules: Vec<AdmissionRuleResult>,
    pub explanation: ExplanationNode,
    pub explanation_hash: String,
}

/// 当前评分标准对单枚御魂给出的数字评分；详细属性贡献仍保存在分析解释树中。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StandardScore {
    pub score: f64,
    pub matched_rule_id: String,
    pub matched_rule_name: String,
    /// 命中规则卡片的数字加法说明，供“我的御魂”列表直接展示。
    pub formula: String,
    /// 每条参与计算的属性贡献，明确区分核心属性和一般属性，供前端着色展示。
    pub contributions: Vec<StandardScoreContribution>,
}

/// 标准评分中的单条属性贡献；`kind` 只允许 `core` 或 `general`，由规则卡片决定。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StandardScoreContribution {
    pub attribute_type: String,
    pub attribute_name: String,
    pub kind: String,
    pub base_weight: f64,
    pub enhancement_count: u8,
    pub enhancement_weight: f64,
    pub contribution: f64,
}

/// 解释树节点，所有用户可见的结论均从该树生成。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplanationNode {
    pub kind: String,
    pub label: String,
    pub status: String,
    pub detail: String,
    #[serde(default)]
    pub children: Vec<ExplanationNode>,
}

/// 纯函数规则引擎。
#[derive(Clone, Copy, Debug, Default)]
pub struct RuleEngine {
    pub validation_limits: ValidationLimits,
}

impl RuleEngine {
    /// 创建使用首版默认安全预算的引擎。
    pub fn new() -> Self {
        Self {
            validation_limits: ValidationLimits::default(),
        }
    }

    /// 对一枚御魂执行所有启用的胚子准入规则。
    pub fn evaluate_admission(
        &self,
        preset: &ValidatedRulePreset,
        facts: &SoulFacts,
    ) -> AdmissionResult {
        let mut rules = Vec::new();
        let mut candidate_uses = BTreeSet::new();
        let mut children = Vec::new();
        for rule in &preset.preset.rules {
            if rule.stage != "admission" || rule.status == RuleStatus::Deprecated {
                continue;
            }
            let selector = rule.selector.evaluate(facts);
            if selector.matched {
                candidate_uses.extend(rule.candidate_uses.iter().cloned());
            }
            children.push(ExplanationNode {
                kind: "admission-rule".to_owned(),
                label: rule.name.clone(),
                status: if selector.matched {
                    "matched"
                } else {
                    "not-matched"
                }
                .to_owned(),
                detail: rule.raw_label.clone().unwrap_or_else(|| rule.id.clone()),
                children: selector
                    .checks
                    .iter()
                    .map(selector_check_to_explanation)
                    .collect(),
            });
            rules.push(AdmissionRuleResult {
                rule_id: rule.id.clone(),
                name: rule.name.clone(),
                status: rule.status,
                evidence_level: rule.evidence_level,
                candidate_uses: rule.candidate_uses.clone(),
                matched: selector.matched,
                selector,
            });
        }
        let candidate_uses = candidate_uses.into_iter().collect::<Vec<_>>();
        let admitted = !candidate_uses.is_empty();
        let explanation = ExplanationNode {
            kind: "admission".to_owned(),
            label: "胚子准入".to_owned(),
            status: if admitted { "admit" } else { "reject" }.to_owned(),
            detail: if admitted {
                format!("命中 {} 个候选用途", candidate_uses.len())
            } else {
                "没有启用规则命中".to_owned()
            },
            children,
        };
        let explanation_hash = hash_serializable(&explanation);
        AdmissionResult {
            preset_id: preset.preset.id.clone(),
            preset_version: preset.preset.version.clone(),
            preset_hash: preset.normalized_hash.clone(),
            admitted,
            candidate_uses,
            rules,
            explanation,
            explanation_hash,
        }
    }

    /// 按评分标准中的命中规则计算数字分数；同一御魂命中多条规则时取最高分。
    /// 初始属性和已知强化次数分别计入规则卡片定义的基础分和强化分，最后逐条直接相加。
    pub fn evaluate_standard_score(
        &self,
        preset: &ValidatedRulePreset,
        facts: &SoulFacts,
    ) -> Option<StandardScore> {
        let mut best: Option<StandardScore> = None;
        for rule in &preset.preset.rules {
            if rule.stage != "admission" || rule.status == RuleStatus::Deprecated {
                continue;
            }
            let Some(scoring) = &rule.scoring else {
                continue;
            };
            // 初始腿数和有效副属性组合只用于胚子准入；成品评分遍历全部结构匹配的规则卡片，
            // 再按各卡片的核心/一般属性计算实际贡献，避免缺少某个有效属性时把整条高分规则提前排除。
            let mut scoring_selector = rule.selector.clone();
            scoring_selector.initial_substat_counts.clear();
            // 等级范围同样只约束胚子准入。安装包规则把评分卡限制在 +0 到 +2，
            // 成品御魂仍应按套装、号位和属性计分。
            scoring_selector.level.clear();
            scoring_selector.required_substats = RequiredSubstats::default();
            if !scoring_selector.evaluate(facts).matched {
                continue;
            }

            // 同步记录每条实际参与评分的属性，既用于求和，也用于页面详情中的可解释公式。
            let mut contributions = Vec::new();
            let mut contribution_details = Vec::new();
            let raw_score = facts.attributes.iter().fold(0.0, |total, attribute| {
                let is_core = scoring.core_attributes.contains(&attribute.attribute_type);
                let is_general = scoring
                    .general_attributes
                    .contains(&attribute.attribute_type);
                let (base_weight, enhancement_weight) = if is_core {
                    (
                        scoring.core_attribute_weight,
                        scoring.enhancement_core_weight,
                    )
                } else if is_general {
                    (
                        scoring.general_attribute_weight,
                        scoring.enhancement_general_weight,
                    )
                } else {
                    return total;
                };
                let enhancement_count = attribute.enhancement_count.unwrap_or(0);
                let enhancement_count_number = f64::from(enhancement_count);
                let contribution = base_weight + enhancement_count_number * enhancement_weight;
                contribution_details.push(StandardScoreContribution {
                    attribute_type: attribute.attribute_type.clone(),
                    attribute_name: attribute_display_name(&attribute.attribute_type).to_owned(),
                    kind: if is_core { "core" } else { "general" }.to_owned(),
                    base_weight,
                    enhancement_count,
                    enhancement_weight,
                    contribution,
                });
                contributions.push(format!(
                    "{}：{} + {}×{} = {}",
                    attribute_display_name(&attribute.attribute_type),
                    format_rule_number(base_weight),
                    format_rule_number(enhancement_count_number),
                    format_rule_number(enhancement_weight),
                    format_rule_number(contribution),
                ));
                total + contribution
            });
            let candidate = StandardScore {
                score: raw_score.clamp(0.0, 100.0),
                matched_rule_id: rule.id.clone(),
                matched_rule_name: rule.name.clone(),
                formula: format!(
                    "{}；核心属性初始 +{} 分、强化每次 +{} 分；一般属性初始 +{} 分、强化每次 +{} 分",
                    if contributions.is_empty() {
                        "没有属性贡献".to_owned()
                    } else {
                        contributions.join("；")
                    },
                    format_rule_number(scoring.core_attribute_weight),
                    format_rule_number(scoring.enhancement_core_weight),
                    format_rule_number(scoring.general_attribute_weight),
                    format_rule_number(scoring.enhancement_general_weight),
                ),
                contributions: contribution_details,
            };
            if best
                .as_ref()
                .is_none_or(|current| candidate.score > current.score)
            {
                best = Some(candidate);
            }
        }
        // 未命中任何评分卡片也属于一次完整计算：用 0 分和明确的“未匹配”标记落库，
        // 避免重算已经完成但标准评分覆盖率仍被统计为 0%。
        best.or_else(|| {
            Some(StandardScore {
                score: 0.0,
                matched_rule_id: "unmatched".to_owned(),
                matched_rule_name: "未匹配评分规则卡片".to_owned(),
                formula: "未匹配评分规则卡片，评分为 0 分".to_owned(),
                contributions: Vec::new(),
            })
        })
    }
}

/// 将规则卡片中的数字权重格式化为稳定、易读的说明文本。
fn format_rule_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// 将规则中的稳定属性 ID 转为用户能直接核对的中文名称；未知属性保留原 ID。
fn attribute_display_name(attribute_type: &str) -> &str {
    match attribute_type {
        "attack_flat" => "攻击",
        "attack_rate" => "攻击加成",
        "hp_flat" => "生命",
        "hp_rate" => "生命加成",
        "defense_flat" => "防御",
        "defense_rate" => "防御加成",
        "speed" => "速度",
        "crit_rate" => "暴击",
        "crit_damage" => "暴伤",
        "effect_hit" => "效果命中",
        "effect_resist" => "效果抵抗",
        other => other,
    }
}

/// 每个评分方案中的属性贡献。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributeContribution {
    pub attribute: String,
    pub value: f64,
    pub weight: f64,
    pub contribution: f64,
    pub effective: bool,
}

/// 属性转换执行结果。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionResult {
    pub id: String,
    pub source_attribute: String,
    pub source_value: f64,
    pub converted_value: f64,
    pub capped: bool,
    pub contribution: f64,
}

/// 条件分支执行结果。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchResult {
    pub id: String,
    pub label: String,
    pub selected: bool,
}

/// 硬约束执行结果。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardConstraintResult {
    pub id: String,
    pub label: String,
    pub satisfied: bool,
    pub gap: Option<f64>,
    pub detail: String,
}

/// 续投或处置建议。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Recommendation {
    Keep,
    Observe,
    Stop,
    Recycle,
}

/// 单个用途的完整评估。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UseEvaluation {
    pub use_id: String,
    pub title: String,
    pub status: RuleStatus,
    pub evidence_level: EvidenceLevel,
    pub selector: Option<SelectorEvaluation>,
    pub selector_matched: bool,
    pub raw_score: f64,
    pub score: f64,
    pub attribute_contributions: Vec<AttributeContribution>,
    pub conversion_results: Vec<ConversionResult>,
    pub branch_results: Vec<BranchResult>,
    pub hard_constraints: Vec<HardConstraintResult>,
    pub effective_growth_count: Option<f64>,
    pub crooked_points: Option<u8>,
    pub remaining_upper_bound: Option<f64>,
    pub next_checkpoint: Option<u8>,
    pub recommendation: Recommendation,
    pub maturity: String,
    pub explanation: ExplanationNode,
    pub explanation_hash: String,
}

/// 库存练度阈值与续投门槛；分数保持独立，只有该结构参与建议。
#[derive(Clone, Copy, Debug)]
pub struct MaturityPolicy {
    pub keep_score: f64,
    pub observe_score: f64,
    pub min_upper_bound_score: f64,
    pub max_crooked_points: u8,
}

impl MaturityPolicy {
    /// 根据库存练度返回默认处置门槛，实际应用可由档案配置覆盖。
    pub fn for_level(level: MaturityLevel) -> Self {
        match level {
            MaturityLevel::Starter => Self {
                keep_score: 55.0,
                observe_score: 20.0,
                min_upper_bound_score: 55.0,
                max_crooked_points: 3,
            },
            MaturityLevel::Growth => Self {
                keep_score: 65.0,
                observe_score: 35.0,
                min_upper_bound_score: 65.0,
                max_crooked_points: 2,
            },
            MaturityLevel::Formed => Self {
                keep_score: 75.0,
                observe_score: 50.0,
                min_upper_bound_score: 75.0,
                max_crooked_points: 1,
            },
            MaturityLevel::Deep => Self {
                keep_score: 85.0,
                observe_score: 65.0,
                min_upper_bound_score: 85.0,
                max_crooked_points: 0,
            },
        }
    }
}

/// 完整快照中六星 +15 御魂数量自动映射到库存练度。
pub fn infer_maturity(
    complete_snapshot: bool,
    souls: &[SoulFacts],
    thresholds: &[MaturityThreshold],
) -> Option<MaturityLevel> {
    if !complete_snapshot {
        return None;
    }
    let count = souls
        .iter()
        .filter(|soul| soul.quality == 6 && soul.level == 15)
        .count() as u64;
    let mut selected = None;
    for threshold in thresholds {
        if threshold.min_count <= count {
            selected = Some(threshold.level);
        }
    }
    selected.or(Some(MaturityLevel::Starter))
}

/// 按自动或手动模式解析库存练度；局部快照不能触发自动判断。
pub fn resolve_maturity(
    mode: MaturityMode,
    manual_level: Option<MaturityLevel>,
    complete_snapshot: bool,
    souls: &[SoulFacts],
    thresholds: &[MaturityThreshold],
) -> Option<MaturityLevel> {
    match mode {
        MaturityMode::Manual => manual_level,
        MaturityMode::Auto => infer_maturity(complete_snapshot, souls, thresholds),
    }
}

/// 多用途安全合并：保留 > 观察 > 停止 > 回收。
pub fn safe_merge(
    recommendations: impl IntoIterator<Item = (bool, Recommendation)>,
) -> Recommendation {
    let mut has_keep = false;
    let mut has_observe = false;
    let mut has_stop = false;
    for (enabled, recommendation) in recommendations {
        if !enabled {
            continue;
        }
        match recommendation {
            Recommendation::Keep => has_keep = true,
            Recommendation::Observe => has_observe = true,
            Recommendation::Stop => has_stop = true,
            Recommendation::Recycle => {}
        }
    }
    if has_keep {
        Recommendation::Keep
    } else if has_observe {
        Recommendation::Observe
    } else if has_stop {
        Recommendation::Stop
    } else {
        Recommendation::Recycle
    }
}

/// 多用途综合结果，同时保留每个用途的独立判断和解释。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregateUseEvaluation {
    pub recommendation: Recommendation,
    pub uses: Vec<UseEvaluation>,
    pub explanation: ExplanationNode,
    pub explanation_hash: String,
}

/// 合并已启用用途的评估结果；未启用用途完全不参与安全合并。
pub fn aggregate_use_evaluations(uses: Vec<(bool, UseEvaluation)>) -> AggregateUseEvaluation {
    let recommendation = safe_merge(
        uses.iter()
            .map(|(enabled, evaluation)| (*enabled, evaluation.recommendation)),
    );
    let children = uses
        .iter()
        .map(|(enabled, evaluation)| ExplanationNode {
            kind: "use".to_owned(),
            label: evaluation.title.clone(),
            status: if *enabled {
                evaluation.recommendation.as_str().to_owned()
            } else {
                "disabled".to_owned()
            },
            detail: format!(
                "{} 分，剩余上限 {:?}",
                evaluation.score, evaluation.remaining_upper_bound
            ),
            children: vec![evaluation.explanation.clone()],
        })
        .collect::<Vec<_>>();
    let explanation = ExplanationNode {
        kind: "safe-merge".to_owned(),
        label: "逐用途安全合并".to_owned(),
        status: recommendation.as_str().to_owned(),
        detail: "任一启用用途保留或观察即可否决回收".to_owned(),
        children,
    };
    let explanation_hash = hash_serializable(&explanation);
    AggregateUseEvaluation {
        recommendation,
        uses: uses.into_iter().map(|(_, evaluation)| evaluation).collect(),
        explanation,
        explanation_hash,
    }
}

/// 返回首版七类特殊用途模板。未核验公式均明确标记 draft，避免被误认为官方结论。
pub fn builtin_use_templates() -> Vec<UseTemplate> {
    vec![
        simple_template(
            "fuyue-defense",
            "不见岳防御转化",
            &["defense_rate"],
            EvidenceLevel::Unverified,
        ),
        simple_template(
            "inaba-output",
            "因幡辉夜姬输出",
            &["crit_rate", "crit_damage", "attack_rate"],
            EvidenceLevel::Unverified,
        ),
        simple_template(
            "ji-output",
            "季暴伤输出",
            &["crit_damage", "attack_rate"],
            EvidenceLevel::Unverified,
        ),
        conditional_template("pingjiangmen-defense-output", "平将门姿态防御输出"),
        simple_template(
            "sp-cat-resist-output",
            "妙主九命猫抵抗输出",
            &["effect_resist", "crit_rate", "crit_damage"],
            EvidenceLevel::Unverified,
        ),
        simple_template(
            "flower-bird-healing",
            "花鸟卷治疗",
            &["hp_rate", "speed", "effect_resist"],
            EvidenceLevel::Unverified,
        ),
        simple_template(
            "peacock-hit-crit",
            "孔雀明王命中暴击",
            &["effect_hit", "crit_rate", "crit_damage"],
            EvidenceLevel::Unverified,
        ),
    ]
}

/// 构造基础模板；权重和阈值是可编辑的占位接口，不冒充已核验公式。
fn simple_template(
    id: &str,
    title: &str,
    attributes: &[&str],
    evidence_level: EvidenceLevel,
) -> UseTemplate {
    let weights = attributes
        .iter()
        .enumerate()
        .map(|(index, attribute)| ((*attribute).to_owned(), 1.0 / (index as f64 + 1.0)))
        .collect();
    UseTemplate {
        id: id.to_owned(),
        title: title.to_owned(),
        scenario: None,
        status: RuleStatus::Draft,
        evidence_level,
        source_refs: Vec::new(),
        game_version_range: Some("*".to_owned()),
        selector: None,
        effective_attributes: attributes.iter().map(|value| (*value).to_owned()).collect(),
        weights,
        normalization: Normalization::default(),
        hard_constraints: Vec::new(),
        conversions: Vec::new(),
        branches: Vec::new(),
        score_expression: None,
        max_allowed_crooked_points: None,
    }
}

/// 构造平将门的姿态分支模板，分支先判定再选权重。
fn conditional_template(id: &str, title: &str) -> UseTemplate {
    let condition = RuleExpression {
        op: "gt".to_owned(),
        path: None,
        value: None,
        args: vec![
            reference("hero.panel.initial_attack"),
            RuleExpression {
                op: "mul".to_owned(),
                path: None,
                value: None,
                args: vec![reference("hero.panel.initial_defense"), constant(7.0)],
                left: None,
                right: None,
                lower: None,
                upper: None,
                condition: None,
                then_branch: None,
                else_branch: None,
                on_zero: None,
                weights: Vec::new(),
            },
        ],
        left: None,
        right: None,
        lower: None,
        upper: None,
        condition: None,
        then_branch: None,
        else_branch: None,
        on_zero: None,
        weights: Vec::new(),
    };
    let mut template = simple_template(
        id,
        title,
        &["defense_rate", "crit_rate"],
        EvidenceLevel::Unverified,
    );
    template.branches.push(ScoreBranch {
        id: "stance".to_owned(),
        label: "攻击/防御姿态".to_owned(),
        condition,
        when_true_weights: BTreeMap::from([(String::from("attack_rate"), 1.0)]),
        when_false_weights: BTreeMap::from([(String::from("defense_rate"), 1.0)]),
    });
    template
}

impl RuleEngine {
    /// 评估用途模板；评分本身不读取库存练度。
    pub fn evaluate_use(
        &self,
        template: &UseTemplate,
        facts: &SoulFacts,
        context: &EvaluationContext,
        maturity: MaturityLevel,
    ) -> Result<UseEvaluation, ExpressionError> {
        let selector = template
            .selector
            .as_ref()
            .map(|selector| selector.evaluate(facts));
        let selector_matched = selector.as_ref().is_none_or(|result| result.matched);
        let mut evaluation_context =
            EvaluationContext::from_soul(facts).with_values(context.values.clone());
        let mut attribute_contributions = Vec::new();
        let mut contribution_by_attribute = BTreeMap::new();

        for (attribute, weight) in &template.weights {
            let value = facts.attribute_value(attribute);
            let contribution = value * weight;
            contribution_by_attribute.insert(attribute.clone(), contribution);
            attribute_contributions.push(AttributeContribution {
                attribute: attribute.clone(),
                value,
                weight: *weight,
                contribution,
                effective: template.effective_attributes.contains(attribute),
            });
        }

        let mut conversion_results = Vec::new();
        for conversion in &template.conversions {
            let source_value = facts.attribute_value(&conversion.source_attribute);
            let uncapped = source_value * conversion.factor;
            let converted_value = conversion.cap.map_or(uncapped, |cap| uncapped.min(cap));
            let target_weight = template
                .weights
                .get(&conversion.target_attribute)
                .copied()
                .unwrap_or(0.0);
            let contribution = converted_value * target_weight;
            *contribution_by_attribute
                .entry(conversion.target_attribute.clone())
                .or_insert(0.0) += contribution;
            conversion_results.push(ConversionResult {
                id: conversion.id.clone(),
                source_attribute: conversion.source_attribute.clone(),
                source_value,
                converted_value,
                capped: conversion.cap.is_some_and(|cap| uncapped > cap),
                contribution,
            });
        }

        let mut branch_results = Vec::new();
        for branch in &template.branches {
            let selected = branch
                .condition
                .evaluate(&evaluation_context)?
                .as_bool()
                .unwrap_or(false);
            let selected_weights = if selected {
                &branch.when_true_weights
            } else {
                &branch.when_false_weights
            };
            for (attribute, weight) in selected_weights {
                let value = facts.attribute_value(attribute);
                *contribution_by_attribute
                    .entry(attribute.clone())
                    .or_insert(0.0) += value * weight;
            }
            branch_results.push(BranchResult {
                id: branch.id.clone(),
                label: branch.label.clone(),
                selected,
            });
        }

        let raw_score = if let Some(expression) = &template.score_expression {
            evaluation_context.set(
                "score.weighted_sum",
                number(contribution_by_attribute.values().sum()),
            );
            expression
                .evaluate(&evaluation_context)?
                .as_f64()
                .ok_or_else(|| ExpressionError::TypeMismatch {
                    path: "score_expression".to_owned(),
                    expected: "数字",
                    actual: "非数字".to_owned(),
                })?
        } else {
            contribution_by_attribute.values().sum()
        };
        let score = template.normalization.normalize(raw_score);

        let mut hard_constraints = Vec::new();
        for constraint in &template.hard_constraints {
            let value = constraint.expression.evaluate(&evaluation_context)?;
            let satisfied = value.as_bool().unwrap_or(false);
            hard_constraints.push(HardConstraintResult {
                id: constraint.id.clone(),
                label: constraint.label.clone(),
                satisfied,
                gap: constraint_gap(&constraint.expression, &evaluation_context),
                detail: if satisfied {
                    "硬约束已满足".to_owned()
                } else {
                    "硬约束未满足，分数不替代该条件".to_owned()
                },
            });
        }

        let effective_growth_count = effective_growth_count(facts, template);
        let crooked_points = crooked_points(facts, template);
        let next_checkpoint = next_checkpoint(facts.level);
        let remaining_upper_bound = remaining_upper_bound(facts, template, raw_score);
        let policy = MaturityPolicy::for_level(maturity);
        let hard_constraints_satisfied = hard_constraints.iter().all(|item| item.satisfied);
        let recommendation = if selector_matched {
            recommendation(
                facts.level,
                score,
                remaining_upper_bound,
                crooked_points,
                hard_constraints_satisfied,
                policy,
                template.max_allowed_crooked_points,
            )
        } else {
            Recommendation::Recycle
        };
        let explanation = build_use_explanation(
            template,
            selector.as_ref(),
            &attribute_contributions,
            &conversion_results,
            &branch_results,
            &hard_constraints,
            effective_growth_count,
            crooked_points,
            remaining_upper_bound,
            next_checkpoint,
            recommendation,
        );
        let explanation_hash = hash_serializable(&explanation);
        Ok(UseEvaluation {
            use_id: template.id.clone(),
            title: template.title.clone(),
            status: template.status,
            evidence_level: template.evidence_level,
            selector,
            selector_matched,
            raw_score,
            score,
            attribute_contributions,
            conversion_results,
            branch_results,
            hard_constraints,
            effective_growth_count,
            crooked_points,
            remaining_upper_bound,
            next_checkpoint,
            recommendation,
            maturity: maturity.as_str().to_owned(),
            explanation,
            explanation_hash,
        })
    }
}

/// 表达式运行时错误。
#[derive(Clone, Debug, PartialEq)]
pub enum ExpressionError {
    MissingReference(String),
    InvalidOperation(String),
    DivisionByZero,
    TypeMismatch {
        path: String,
        expected: &'static str,
        actual: String,
    },
}

impl Display for ExpressionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingReference(path) => write!(formatter, "缺少表达式引用：{path}"),
            Self::InvalidOperation(operation) => {
                write!(formatter, "表达式运行失败：{operation}")
            }
            Self::DivisionByZero => write!(formatter, "表达式除数为零"),
            Self::TypeMismatch {
                path,
                expected,
                actual,
            } => {
                write!(
                    formatter,
                    "表达式 {path} 类型错误：需要 {expected}，实际 {actual}"
                )
            }
        }
    }
}

impl std::error::Error for ExpressionError {}

impl RuleExpression {
    /// 解释一个表达式节点；操作集合与校验器保持一致。
    pub fn evaluate(&self, context: &EvaluationContext) -> Result<Value, ExpressionError> {
        match self.op.as_str() {
            "const" => self
                .value
                .clone()
                .ok_or_else(|| ExpressionError::InvalidOperation("const 缺少 value".to_owned())),
            "ref" => {
                let path = self
                    .path
                    .as_deref()
                    .ok_or_else(|| ExpressionError::InvalidOperation("ref 缺少 path".to_owned()))?;
                Ok(context.get(path)?.clone())
            }
            "add" => numeric_args(self, context, |values| values.into_iter().sum()),
            "sub" => binary_numeric(self, context, |left, right| left - right),
            "mul" => numeric_args(self, context, |values| values.into_iter().product()),
            "div" => {
                let values = numeric_values(self, context)?;
                if values.len() != 2 {
                    return Err(ExpressionError::InvalidOperation(
                        "div 需要两个 args".to_owned(),
                    ));
                }
                if values[1] == 0.0 {
                    match self.on_zero.as_deref() {
                        Some("zero") => Ok(number(0.0)),
                        Some("error") => Err(ExpressionError::DivisionByZero),
                        _ => Err(ExpressionError::InvalidOperation(
                            "div 缺少有效 onZero 策略".to_owned(),
                        )),
                    }
                } else {
                    Ok(number(values[0] / values[1]))
                }
            }
            "min" => numeric_args(self, context, |values| {
                values.into_iter().fold(f64::INFINITY, f64::min)
            }),
            "max" => numeric_args(self, context, |values| {
                values.into_iter().fold(f64::NEG_INFINITY, f64::max)
            }),
            "clamp" => {
                let values = numeric_values(self, context)?;
                if values.len() != 3 {
                    return Err(ExpressionError::InvalidOperation(
                        "clamp 需要 value、min、max".to_owned(),
                    ));
                }
                Ok(number(values[0].clamp(values[1], values[2])))
            }
            "abs" => unary_numeric(self, context, f64::abs),
            "eq" => comparison(self, context, |left, right| left == right),
            "ne" => comparison(self, context, |left, right| left != right),
            "gt" => binary_numeric_bool(self, context, |left, right| left > right),
            "gte" => binary_numeric_bool(self, context, |left, right| left >= right),
            "lt" => binary_numeric_bool(self, context, |left, right| left < right),
            "lte" => binary_numeric_bool(self, context, |left, right| left <= right),
            "between" => {
                let values = numeric_values(self, context)?;
                if values.len() != 3 {
                    return Err(ExpressionError::InvalidOperation(
                        "between 需要 value、lower、upper".to_owned(),
                    ));
                }
                Ok(Value::Bool(
                    values[0] >= values[1] && values[0] <= values[2],
                ))
            }
            "and" => boolean_args(self, context, |values| {
                values.into_iter().all(|value| value)
            }),
            "or" => boolean_args(self, context, |values| {
                values.into_iter().any(|value| value)
            }),
            "not" => unary_boolean(self, context, |value| !value),
            "if" => {
                let condition = self
                    .condition
                    .as_ref()
                    .ok_or_else(|| {
                        ExpressionError::InvalidOperation("if 缺少 condition".to_owned())
                    })?
                    .evaluate(context)?;
                let branch = if condition.as_bool().unwrap_or(false) {
                    self.then_branch.as_ref()
                } else {
                    self.else_branch.as_ref()
                }
                .ok_or_else(|| ExpressionError::InvalidOperation("if 缺少分支".to_owned()))?;
                branch.evaluate(context)
            }
            "sum" => numeric_args(self, context, |values| values.into_iter().sum()),
            "weighted_sum" => {
                let values = numeric_values(self, context)?;
                if values.len() != self.weights.len() {
                    return Err(ExpressionError::InvalidOperation(
                        "weighted_sum 的 weights 数量必须与 args 一致".to_owned(),
                    ));
                }
                Ok(number(
                    values
                        .into_iter()
                        .zip(self.weights.iter())
                        .map(|(value, weight)| value * weight)
                        .sum(),
                ))
            }
            "product" => numeric_args(self, context, |values| values.into_iter().product()),
            "lexicographic" | "pareto" => {
                let values = self
                    .args
                    .iter()
                    .map(|argument| argument.evaluate(context))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::Array(values))
            }
            operation => Err(ExpressionError::InvalidOperation(operation.to_owned())),
        }
    }
}

/// 构造数值常量，避免直接依赖 JSON 数字构造细节。
fn constant(value: f64) -> RuleExpression {
    RuleExpression {
        op: "const".to_owned(),
        path: None,
        value: Some(number(value)),
        args: Vec::new(),
        left: None,
        right: None,
        lower: None,
        upper: None,
        condition: None,
        then_branch: None,
        else_branch: None,
        on_zero: None,
        weights: Vec::new(),
    }
}

/// 构造引用节点。
fn reference(path: &str) -> RuleExpression {
    RuleExpression {
        op: "ref".to_owned(),
        path: Some(path.to_owned()),
        value: None,
        args: Vec::new(),
        left: None,
        right: None,
        lower: None,
        upper: None,
        condition: None,
        then_branch: None,
        else_branch: None,
        on_zero: None,
        weights: Vec::new(),
    }
}

/// 统一读取表达式数值参数。
fn numeric_values(
    expression: &RuleExpression,
    context: &EvaluationContext,
) -> Result<Vec<f64>, ExpressionError> {
    expression_operands(expression)
        .iter()
        .map(|argument| {
            let value = argument.evaluate(context)?;
            value.as_f64().ok_or_else(|| ExpressionError::TypeMismatch {
                path: expression.op.clone(),
                expected: "数字",
                actual: value_type(&value).to_owned(),
            })
        })
        .collect()
}

fn numeric_args(
    expression: &RuleExpression,
    context: &EvaluationContext,
    combine: impl Fn(Vec<f64>) -> f64,
) -> Result<Value, ExpressionError> {
    Ok(number(combine(numeric_values(expression, context)?)))
}

fn unary_numeric(
    expression: &RuleExpression,
    context: &EvaluationContext,
    operation: impl Fn(f64) -> f64,
) -> Result<Value, ExpressionError> {
    let values = numeric_values(expression, context)?;
    if values.len() != 1 {
        return Err(ExpressionError::InvalidOperation(format!(
            "{} 需要一个参数",
            expression.op
        )));
    }
    Ok(number(operation(values[0])))
}

fn binary_numeric(
    expression: &RuleExpression,
    context: &EvaluationContext,
    operation: impl Fn(f64, f64) -> f64,
) -> Result<Value, ExpressionError> {
    let values = numeric_values(expression, context)?;
    if values.len() != 2 {
        return Err(ExpressionError::InvalidOperation(format!(
            "{} 需要两个参数",
            expression.op
        )));
    }
    Ok(number(operation(values[0], values[1])))
}

fn binary_numeric_bool(
    expression: &RuleExpression,
    context: &EvaluationContext,
    operation: impl Fn(f64, f64) -> bool,
) -> Result<Value, ExpressionError> {
    let values = numeric_values(expression, context)?;
    if values.len() != 2 {
        return Err(ExpressionError::InvalidOperation(format!(
            "{} 需要两个参数",
            expression.op
        )));
    }
    Ok(Value::Bool(operation(values[0], values[1])))
}

fn comparison(
    expression: &RuleExpression,
    context: &EvaluationContext,
    operation: impl Fn(&Value, &Value) -> bool,
) -> Result<Value, ExpressionError> {
    let operands = expression_operands(expression);
    if operands.len() != 2 {
        return Err(ExpressionError::InvalidOperation(format!(
            "{} 需要两个参数",
            expression.op
        )));
    }
    let left = operands[0].evaluate(context)?;
    let right = operands[1].evaluate(context)?;
    Ok(Value::Bool(operation(&left, &right)))
}

/// 同时兼容规范中的 args 形式和示例中的 left/right 形式。
fn expression_operands(expression: &RuleExpression) -> Vec<&RuleExpression> {
    if !expression.args.is_empty() {
        expression.args.iter().collect()
    } else {
        [expression.left.as_deref(), expression.right.as_deref()]
            .into_iter()
            .flatten()
            .collect()
    }
}

fn boolean_args(
    expression: &RuleExpression,
    context: &EvaluationContext,
    combine: impl Fn(Vec<bool>) -> bool,
) -> Result<Value, ExpressionError> {
    let values = expression
        .args
        .iter()
        .map(|argument| {
            let value = argument.evaluate(context)?;
            value
                .as_bool()
                .ok_or_else(|| ExpressionError::TypeMismatch {
                    path: expression.op.clone(),
                    expected: "布尔值",
                    actual: value_type(&value).to_owned(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Value::Bool(combine(values)))
}

fn unary_boolean(
    expression: &RuleExpression,
    context: &EvaluationContext,
    operation: impl Fn(bool) -> bool,
) -> Result<Value, ExpressionError> {
    if expression.args.len() != 1 {
        return Err(ExpressionError::InvalidOperation(format!(
            "{} 需要一个参数",
            expression.op
        )));
    }
    let value = expression.args[0]
        .evaluate(context)?
        .as_bool()
        .ok_or_else(|| ExpressionError::InvalidOperation("not 参数必须是布尔值".to_owned()))?;
    Ok(Value::Bool(operation(value)))
}

/// 校验表达式结构、引用路径、操作数数量和资源预算。
fn validate_expression(
    expression: &RuleExpression,
    limits: &ValidationLimits,
) -> Result<(), RuleValidationError> {
    let (nodes, depth) = expression_shape(expression, 1, limits)?;
    if nodes > limits.max_expression_nodes {
        return Err(RuleValidationError::ExpressionTooLarge {
            nodes,
            limit: limits.max_expression_nodes,
        });
    }
    if depth > limits.max_expression_depth {
        return Err(RuleValidationError::ExpressionTooDeep {
            depth,
            limit: limits.max_expression_depth,
        });
    }
    validate_expression_node(expression, "expression")
}

fn expression_shape(
    expression: &RuleExpression,
    depth: usize,
    limits: &ValidationLimits,
) -> Result<(usize, usize), RuleValidationError> {
    if depth > limits.max_expression_depth {
        return Err(RuleValidationError::ExpressionTooDeep {
            depth,
            limit: limits.max_expression_depth,
        });
    }
    let mut nodes = 1;
    let mut max_depth = depth;
    let children = expression_children(expression);
    for child in children {
        let (child_nodes, child_depth) = expression_shape(child, depth + 1, limits)?;
        nodes += child_nodes;
        max_depth = max_depth.max(child_depth);
    }
    Ok((nodes, max_depth))
}

fn expression_children(expression: &RuleExpression) -> Vec<&RuleExpression> {
    let mut children = expression.args.iter().collect::<Vec<_>>();
    if let Some(value) = &expression.left {
        children.push(value);
    }
    if let Some(value) = &expression.right {
        children.push(value);
    }
    if let Some(value) = &expression.lower {
        children.push(value);
    }
    if let Some(value) = &expression.upper {
        children.push(value);
    }
    if let Some(value) = &expression.condition {
        children.push(value);
    }
    if let Some(value) = &expression.then_branch {
        children.push(value);
    }
    if let Some(value) = &expression.else_branch {
        children.push(value);
    }
    children
}

fn validate_expression_node(
    expression: &RuleExpression,
    path: &str,
) -> Result<(), RuleValidationError> {
    const OPERATORS: &[&str] = &[
        "const",
        "ref",
        "add",
        "sub",
        "mul",
        "div",
        "min",
        "max",
        "clamp",
        "abs",
        "eq",
        "ne",
        "gt",
        "gte",
        "lt",
        "lte",
        "between",
        "and",
        "or",
        "not",
        "if",
        "sum",
        "weighted_sum",
        "product",
        "lexicographic",
        "pareto",
    ];
    if !OPERATORS.contains(&expression.op.as_str()) {
        return Err(RuleValidationError::UnknownOperator(expression.op.clone()));
    }
    match expression.op.as_str() {
        "const" => {
            let value = expression
                .value
                .as_ref()
                .ok_or_else(|| invalid_field(path, "const 必须提供 value"))?;
            if value.is_null() || value.is_array() || value.is_object() {
                return Err(invalid_field(path, "const 只能保存数字、字符串或布尔值"));
            }
        }
        "ref" => {
            let reference = expression
                .path
                .as_deref()
                .ok_or_else(|| invalid_field(path, "ref 必须提供 path"))?;
            validate_reference_path(reference, path)?;
        }
        "div" => {
            if expression.args.len() != 2 {
                return Err(invalid_field(path, "div 必须有两个 args"));
            }
            if !matches!(expression.on_zero.as_deref(), Some("zero" | "error")) {
                return Err(invalid_field(path, "div 必须声明 onZero=zero 或 error"));
            }
        }
        "sub" | "gt" | "gte" | "lt" | "lte" | "eq" | "ne" => {
            if expression_operands(expression).len() != 2 {
                return Err(invalid_field(path, "二元操作必须有两个 args"));
            }
        }
        "clamp" | "between" => {
            if expression_operands(expression).len() != 3 {
                return Err(invalid_field(path, "范围操作必须有三个 args"));
            }
        }
        "not" | "abs" => {
            if expression.args.len() != 1 {
                return Err(invalid_field(path, "一元操作必须有一个 args"));
            }
        }
        "if" => {
            if expression.condition.is_none()
                || expression.then_branch.is_none()
                || expression.else_branch.is_none()
            {
                return Err(invalid_field(path, "if 必须提供 condition、then 和 else"));
            }
        }
        "weighted_sum" => {
            if expression.args.is_empty() || expression.args.len() != expression.weights.len() {
                return Err(invalid_field(
                    path,
                    "weighted_sum 的 weights 必须与 args 数量一致",
                ));
            }
            if expression.weights.iter().any(|weight| !weight.is_finite()) {
                return Err(invalid_field(path, "weighted_sum 权重必须是有限数字"));
            }
        }
        "and" | "or" | "add" | "mul" | "min" | "max" | "sum" | "product" | "lexicographic"
        | "pareto" => {
            if expression.args.is_empty() {
                return Err(invalid_field(path, "聚合操作至少需要一个 args"));
            }
        }
        _ => {}
    }
    for (index, child) in expression_children(expression).into_iter().enumerate() {
        validate_expression_node(child, &format!("{path}.children[{index}]"))?;
    }
    Ok(())
}

fn validate_reference_path(reference: &str, field: &str) -> Result<(), RuleValidationError> {
    let allowed_prefixes = [
        "soul.",
        "hero.base.",
        "hero.panel.",
        "team.",
        "target.",
        "context.",
        "mechanics.",
    ];
    if !allowed_prefixes
        .iter()
        .any(|prefix| reference.starts_with(prefix))
    {
        return Err(invalid_field(field, "ref path 必须使用受限上下文前缀"));
    }
    if reference.split('.').any(|part| {
        part.is_empty()
            || !part
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
    }) {
        return Err(invalid_field(field, "ref path 不能包含动态字段或特殊字符"));
    }
    Ok(())
}

fn validate_selector(
    selector: &Selector,
    limits: &ValidationLimits,
    path: &str,
) -> Result<(), RuleValidationError> {
    validate_selector_at(selector, limits, path, 1)
}

fn validate_selector_at(
    selector: &Selector,
    limits: &ValidationLimits,
    path: &str,
    depth: usize,
) -> Result<(), RuleValidationError> {
    if depth > limits.max_expression_depth {
        return Err(RuleValidationError::ExpressionTooDeep {
            depth,
            limit: limits.max_expression_depth,
        });
    }
    for (field, attributes) in [
        ("mainAttributes", &selector.main_attributes),
        ("requiredSubstats.allOf", &selector.required_substats.all_of),
        ("requiredSubstats.anyOf", &selector.required_substats.any_of),
        ("forbiddenSubstats", &selector.forbidden_substats),
    ] {
        for attribute in attributes {
            if !NORMALIZED_ATTRIBUTE_IDS.contains(&attribute.as_str()) {
                return Err(invalid_field(&format!("{path}.{field}"), "未知规范属性 ID"));
            }
        }
    }
    for set_id in &selector.set_ids {
        validate_text(&format!("{path}.setIds"), set_id, limits.max_string_length)?;
    }
    for pair in &selector.slot_main_pairs {
        if !(1..=6).contains(&pair.slot) {
            return Err(invalid_field(
                &format!("{path}.slotMainPairs"),
                "号位必须位于 1 到 6",
            ));
        }
        if !NORMALIZED_ATTRIBUTE_IDS.contains(&pair.main_attribute.as_str()) {
            return Err(invalid_field(
                &format!("{path}.slotMainPairs"),
                "未知主属性 ID",
            ));
        }
    }
    if let Some(at_least) = &selector.required_substats.at_least {
        if at_least.count == 0 || at_least.from.is_empty() || at_least.count > at_least.from.len() {
            return Err(invalid_field(
                &format!("{path}.requiredSubstats.atLeast"),
                "count 必须在候选属性数量内且大于 0",
            ));
        }
    }
    for nested in selector.all.iter().chain(selector.any.iter()) {
        validate_selector_at(nested, limits, path, depth + 1)?;
    }
    if let Some(nested) = &selector.not {
        validate_selector_at(nested, limits, path, depth + 1)?;
    }
    Ok(())
}

/// 校验手动评分配置，防止分享文件携带无法解释的属性或反向阈值。
fn validate_scoring(
    scoring: &RuleScoring,
    _limits: &ValidationLimits,
    path: &str,
) -> Result<(), RuleValidationError> {
    for (field, attributes) in [
        ("coreAttributes", &scoring.core_attributes),
        ("generalAttributes", &scoring.general_attributes),
    ] {
        for attribute in attributes {
            if !NORMALIZED_ATTRIBUTE_IDS.contains(&attribute.as_str()) {
                return Err(invalid_field(&format!("{path}.{field}"), "未知规范属性 ID"));
            }
        }
    }
    if scoring
        .core_attributes
        .iter()
        .any(|attribute| scoring.general_attributes.contains(attribute))
    {
        return Err(invalid_field(
            &format!("{path}.generalAttributes"),
            "核心属性和一般属性不能重复",
        ));
    }
    let weights = [
        ("coreAttributeWeight", scoring.core_attribute_weight),
        ("generalAttributeWeight", scoring.general_attribute_weight),
        ("enhancementCoreWeight", scoring.enhancement_core_weight),
        (
            "enhancementGeneralWeight",
            scoring.enhancement_general_weight,
        ),
    ];
    if weights
        .iter()
        .any(|(_, value)| !value.is_finite() || *value < 0.0)
    {
        return Err(invalid_field(
            &format!("{path}.weights"),
            "评分权重必须是非负有限数",
        ));
    }
    let scores = [
        ("discardScore", scoring.discard_score),
        ("observeScore", scoring.observe_score),
        ("keepScore", scoring.keep_score),
    ];
    if scores
        .iter()
        .any(|(_, value)| !value.is_finite() || !(*value >= 0.0 && *value <= 100.0))
    {
        return Err(invalid_field(
            &format!("{path}.thresholds"),
            "评分阈值必须位于 0 到 100",
        ));
    }
    if !(scoring.discard_score <= scoring.observe_score
        && scoring.observe_score <= scoring.keep_score)
    {
        return Err(invalid_field(
            &format!("{path}.thresholds"),
            "评分阈值需要满足：弃置 ≤ 观察 ≤ 保留",
        ));
    }
    Ok(())
}

/// 把选择器检查转换为统一解释树节点。
fn selector_check_to_explanation(check: &SelectorCheck) -> ExplanationNode {
    ExplanationNode {
        kind: "selector-check".to_owned(),
        label: check.label.clone(),
        status: if check.matched { "matched" } else { "failed" }.to_owned(),
        detail: check.detail.clone(),
        children: check
            .children
            .iter()
            .map(selector_check_to_explanation)
            .collect(),
    }
}

/// 对常见比较型硬约束给出与目标线的数值差距；无法安全推导时保持未知。
fn constraint_gap(expression: &RuleExpression, context: &EvaluationContext) -> Option<f64> {
    let operands = expression_operands(expression);
    if operands.len() != 2 {
        return None;
    }
    let left = operands[0].evaluate(context).ok()?.as_f64()?;
    let right = operands[1].evaluate(context).ok()?.as_f64()?;
    let gap = match expression.op.as_str() {
        "gte" | "gt" => right - left,
        "lte" | "lt" => left - right,
        "eq" | "ne" => (right - left).abs(),
        _ => return None,
    };
    Some(gap.max(0.0))
}

/// 计算有明确强化次数的有效成长；未知次数保持 None，不能默认为零。
fn effective_growth_count(facts: &SoulFacts, template: &UseTemplate) -> Option<f64> {
    let mut total = 0.0;
    let mut has_known = false;
    for attribute in &facts.attributes {
        if !template
            .effective_attributes
            .contains(&attribute.attribute_type)
        {
            continue;
        }
        if let Some(count) = attribute.enhancement_count {
            has_known = true;
            total += count as f64;
        }
    }
    has_known.then_some(total)
}

/// 歪点仅统计已知成长次数且不属于当前用途有效属性的成长。
fn crooked_points(facts: &SoulFacts, template: &UseTemplate) -> Option<u8> {
    let mut total = 0_u8;
    let mut has_known = false;
    for attribute in &facts.attributes {
        if let Some(count) = attribute.enhancement_count {
            has_known = true;
            if !template
                .effective_attributes
                .contains(&attribute.attribute_type)
            {
                total = total.saturating_add(count);
            }
        }
    }
    has_known.then_some(total)
}

/// 返回 +3/+6/+9/+12/+15 中严格大于当前等级的下一个观察节点。
fn next_checkpoint(level: u8) -> Option<u8> {
    [3, 6, 9, 12, 15]
        .into_iter()
        .find(|checkpoint| *checkpoint > level)
}

/// 以当前用途最有利的单次成长计算理论剩余上限，不代表成功概率。
fn remaining_upper_bound(facts: &SoulFacts, template: &UseTemplate, raw_score: f64) -> Option<f64> {
    let remaining_rolls = if facts.level >= 15 {
        0
    } else {
        ((15_u16 - facts.level as u16) as f64 / 3.0).ceil() as u8
    };
    if template.weights.is_empty() {
        return None;
    }
    let best_single_roll = template
        .weights
        .iter()
        .filter(|(attribute, weight)| {
            template.effective_attributes.contains(attribute) && **weight > 0.0
        })
        .map(|(_, weight)| *weight)
        .fold(0.0, f64::max);
    Some(
        template
            .normalization
            .normalize(raw_score + f64::from(remaining_rolls) * best_single_roll),
    )
}

/// 由分数、硬约束、歪点和剩余上限生成续投建议。
fn recommendation(
    level: u8,
    score: f64,
    upper_bound: Option<f64>,
    crooked_points: Option<u8>,
    hard_constraints_satisfied: bool,
    policy: MaturityPolicy,
    custom_max_crooked_points: Option<u8>,
) -> Recommendation {
    if crooked_points
        .is_some_and(|value| value > custom_max_crooked_points.unwrap_or(policy.max_crooked_points))
    {
        return if level == 15 {
            Recommendation::Recycle
        } else {
            Recommendation::Stop
        };
    }
    if level == 15 {
        return if hard_constraints_satisfied && score >= policy.keep_score {
            Recommendation::Keep
        } else {
            Recommendation::Recycle
        };
    }
    if !hard_constraints_satisfied && upper_bound.unwrap_or(0.0) < policy.min_upper_bound_score {
        return Recommendation::Stop;
    }
    if upper_bound.unwrap_or(0.0) >= policy.min_upper_bound_score && score >= policy.observe_score {
        Recommendation::Observe
    } else {
        Recommendation::Stop
    }
}

/// 构造用途的解释树，按固定顺序保留评分、约束和续投依据。
fn build_use_explanation(
    template: &UseTemplate,
    selector: Option<&SelectorEvaluation>,
    attributes: &[AttributeContribution],
    conversions: &[ConversionResult],
    branches: &[BranchResult],
    constraints: &[HardConstraintResult],
    growth: Option<f64>,
    crooked: Option<u8>,
    upper_bound: Option<f64>,
    next: Option<u8>,
    recommendation: Recommendation,
) -> ExplanationNode {
    let mut children = Vec::new();
    if let Some(selector) = selector {
        children.push(ExplanationNode {
            kind: "selector".to_owned(),
            label: "用途选择器".to_owned(),
            status: if selector.matched {
                "matched"
            } else {
                "failed"
            }
            .to_owned(),
            detail: "用途硬范围检查".to_owned(),
            children: selector
                .checks
                .iter()
                .map(selector_check_to_explanation)
                .collect(),
        });
    }
    children.push(ExplanationNode {
        kind: "score".to_owned(),
        label: "方案评分".to_owned(),
        status: "calculated".to_owned(),
        detail: format!("{} 项属性贡献", attributes.len()),
        children: attributes
            .iter()
            .map(|attribute| ExplanationNode {
                kind: "attribute".to_owned(),
                label: attribute.attribute.clone(),
                status: if attribute.effective {
                    "effective"
                } else {
                    "secondary"
                }
                .to_owned(),
                detail: format!(
                    "{} × {} = {}",
                    attribute.value, attribute.weight, attribute.contribution
                ),
                children: Vec::new(),
            })
            .collect(),
    });
    for conversion in conversions {
        children.push(ExplanationNode {
            kind: "conversion".to_owned(),
            label: conversion.id.clone(),
            status: if conversion.capped {
                "capped"
            } else {
                "converted"
            }
            .to_owned(),
            detail: format!(
                "{} → {}",
                conversion.source_value, conversion.converted_value
            ),
            children: Vec::new(),
        });
    }
    for branch in branches {
        children.push(ExplanationNode {
            kind: "branch".to_owned(),
            label: branch.label.clone(),
            status: if branch.selected { "true" } else { "false" }.to_owned(),
            detail: "按条件分支选择评分权重".to_owned(),
            children: Vec::new(),
        });
    }
    children.push(ExplanationNode {
        kind: "continuation".to_owned(),
        label: "强化观察".to_owned(),
        status: recommendation.as_str().to_owned(),
        detail: format!(
            "有效成长 {:?}，歪点 {:?}，剩余上限 {:?}，下一节点 {:?}",
            growth, crooked, upper_bound, next
        ),
        children: constraints
            .iter()
            .map(|constraint| ExplanationNode {
                kind: "hard-constraint".to_owned(),
                label: constraint.label.clone(),
                status: if constraint.satisfied {
                    "satisfied"
                } else {
                    "failed"
                }
                .to_owned(),
                detail: constraint.detail.clone(),
                children: Vec::new(),
            })
            .collect(),
    });
    ExplanationNode {
        kind: "use".to_owned(),
        label: template.title.clone(),
        status: recommendation.as_str().to_owned(),
        detail: "用途独立评估".to_owned(),
        children,
    }
}

/// 自动推导中间值时统一使用可序列化 JSON 数字。
fn number(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or_else(|| Value::Number(serde_json::Number::from(0)))
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn validate_text(field: &str, value: &str, limit: usize) -> Result<(), RuleValidationError> {
    if value.trim().is_empty() || value.chars().count() > limit {
        return Err(invalid_field(field, "不能为空且长度不能超过限制"));
    }
    Ok(())
}

fn validate_refs(
    field: &str,
    refs: &[String],
    limits: &ValidationLimits,
) -> Result<(), RuleValidationError> {
    if refs.len() > limits.max_source_refs {
        return Err(invalid_field(field, "来源引用数量超过限制"));
    }
    for reference in refs {
        validate_text(field, reference, limits.max_string_length)?;
    }
    Ok(())
}

fn invalid_field(field: &str, message: &str) -> RuleValidationError {
    RuleValidationError::InvalidField {
        field: field.to_owned(),
        message: message.to_owned(),
    }
}

fn interpolate(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    if from_max <= from_min {
        return to_max.clamp(0.0, 100.0);
    }
    (to_min + (value - from_min) / (from_max - from_min) * (to_max - to_min)).clamp(0.0, 100.0)
}

fn canonical_json(value: &Value) -> Result<String, serde_json::Error> {
    fn normalize(value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let mut normalized = Map::new();
                let mut entries = object.iter().collect::<Vec<_>>();
                entries.sort_by(|left, right| left.0.cmp(right.0));
                for (key, child) in entries {
                    normalized.insert(key.clone(), normalize(child));
                }
                Value::Object(normalized)
            }
            Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&normalize(value))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hash_serializable<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| canonical_json(&value).ok())
        .map(|json| sha256_hex(json.as_bytes()))
        .unwrap_or_default()
}

impl Recommendation {
    /// 返回前后端共享的处置建议字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Observe => "observe",
            Self::Stop => "stop",
            Self::Recycle => "recycle",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr(attribute_type: &str, value: f64, enhancement_count: Option<u8>) -> SoulAttributeFact {
        SoulAttributeFact {
            attribute_type: attribute_type.to_owned(),
            value,
            enhancement_count,
            count_provenance: enhancement_count.map(|_| "source".to_owned()),
        }
    }

    fn facts(slot: u8, main_attribute: &str, initial_legs: u8, attributes: &[&str]) -> SoulFacts {
        SoulFacts::with_attributes(
            "破势",
            slot,
            6,
            0,
            main_attribute,
            0.0,
            Some(initial_legs),
            attributes
                .iter()
                .map(|attribute| attr(attribute, 0.05, None))
                .collect(),
        )
    }

    #[test]
    fn 默认预设包含二十八条唯一规则并可生成稳定哈希() {
        let preset = load_default_preset().expect("默认预设应通过校验");
        assert_eq!(preset.preset.rules.len(), 28);
        let ids = preset
            .preset
            .rules
            .iter()
            .map(|rule| rule.id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), 28);
        assert_eq!(preset.normalized_hash.len(), 64);
        let reordered = serde_json::from_str::<Value>(&preset.canonical_json).expect("规范 JSON");
        let reordered_text = serde_json::to_string(&reordered).expect("JSON");
        let parsed = parse_rule_preset(reordered_text.as_bytes(), ValidationLimits::default())
            .expect("重排后仍可校验");
        assert_eq!(preset.normalized_hash, parsed.normalized_hash);
    }

    #[test]
    fn 当前评分标准命中时返回可解释的百分制分数() {
        let preset = load_default_preset().expect("默认预设");
        let engine = RuleEngine::new();
        let scored_facts = SoulFacts::with_attributes(
            "招财猫",
            2,
            6,
            0,
            "speed",
            0.0,
            Some(4),
            vec![attr("speed", 0.05, Some(3))],
        );

        let score = engine
            .evaluate_standard_score(&preset, &scored_facts)
            .expect("四腿速度胚子应命中评分规则");
        assert!((0.0..=100.0).contains(&score.score));
        assert!(!score.matched_rule_id.is_empty());
        assert!(!score.matched_rule_name.is_empty());
    }

    /// 回归约束：即使御魂没有命中任何评分卡片，也必须落下可追踪的 0 分结果，
    /// 不能因为评分卡片未命中而让“评分覆盖”误显示为未计算。
    #[test]
    fn 未命中评分卡片仍返回零分结果() {
        let preset = load_default_preset().expect("默认预设");
        let engine = RuleEngine::new();
        let ordinary_facts = SoulFacts::with_attributes(
            "不在默认输出卡片中的套装",
            1,
            6,
            15,
            "attack_flat",
            81.0,
            None,
            vec![],
        );

        let score = engine
            .evaluate_standard_score(&preset, &ordinary_facts)
            .expect("未命中评分卡片也应生成 0 分结果");

        assert_eq!(score.score, 0.0);
        assert_eq!(score.matched_rule_id, "unmatched");
        assert_eq!(score.matched_rule_name, "未匹配评分规则卡片");
    }

    /// 回归约束：评分卡片匹配不依赖胚子阶段的初始腿数，最终属性满足时仍应返回分数。
    #[test]
    fn 成品评分不因缺少初始腿数而跳过规则卡片() {
        let preset = load_default_preset().expect("默认预设");
        let engine = RuleEngine::new();
        let finished_facts = SoulFacts::with_attributes(
            "招财猫",
            2,
            6,
            15,
            "speed",
            57.0,
            None,
            vec![attr("speed", 0.05, Some(5))],
        );

        let score = engine
            .evaluate_standard_score(&preset, &finished_facts)
            .expect("最终属性满足规则卡片时应生成评分");
        assert!(score.score > 0.0);
    }

    /// 回归约束：有效副属性描述评分贡献，不是成品评分的硬性准入条件。
    /// 同一枚御魂必须遍历全部结构匹配的规则卡片，并采用实际贡献分最高的一条。
    #[test]
    fn 成品遍历全部结构匹配规则并采用最高评分() {
        let mut preset = load_default_preset().expect("默认预设").preset;
        let base_rule = preset
            .rules
            .iter()
            .find(|rule| rule.scoring.is_some())
            .expect("默认预设应包含评分卡片")
            .clone();

        // 第一条生命规则可以完整命中，但只能得到较低的基础分。
        let mut lower_rule = base_rule.clone();
        lower_rule.id = "finished-hp-lower".to_owned();
        lower_rule.name = "成品生命低分规则".to_owned();
        lower_rule.selector = Selector {
            set_ids: vec!["破势".to_owned()],
            slots: vec![1],
            slot_main_pairs: vec![SlotMainPair {
                slot: 1,
                main_attribute: "attack_flat".to_owned(),
            }],
            required_substats: RequiredSubstats {
                all_of: vec!["hp_rate".to_owned()],
                ..RequiredSubstats::default()
            },
            ..Selector::default()
        };
        let mut lower_scoring = lower_rule.scoring.clone().expect("复制低分评分配置");
        lower_scoring.core_attributes = vec!["hp_rate".to_owned()];
        lower_scoring.general_attributes.clear();
        lower_scoring.core_attribute_weight = 10.0;
        lower_scoring.enhancement_core_weight = 10.0;
        lower_rule.scoring = Some(lower_scoring);

        // 第二条双暴规则缺少暴击，但已有暴伤五次强化，仍应参与成品评分并以 60 分胜出。
        let mut higher_rule = base_rule;
        higher_rule.id = "finished-crit-higher".to_owned();
        higher_rule.name = "成品暴伤高分规则".to_owned();
        higher_rule.selector = Selector {
            set_ids: vec!["破势".to_owned()],
            slots: vec![1],
            slot_main_pairs: vec![SlotMainPair {
                slot: 1,
                main_attribute: "attack_flat".to_owned(),
            }],
            initial_substat_counts: vec![3, 4],
            required_substats: RequiredSubstats {
                all_of: vec!["crit_rate".to_owned(), "crit_damage".to_owned()],
                ..RequiredSubstats::default()
            },
            ..Selector::default()
        };
        let mut higher_scoring = higher_rule.scoring.clone().expect("复制高分评分配置");
        higher_scoring.core_attributes = vec!["crit_rate".to_owned(), "crit_damage".to_owned()];
        higher_scoring.general_attributes.clear();
        higher_scoring.core_attribute_weight = 10.0;
        higher_scoring.enhancement_core_weight = 10.0;
        higher_rule.scoring = Some(higher_scoring);
        preset.rules = vec![lower_rule, higher_rule];
        let preset = preset.validate().expect("校验成品多规则评分预设");

        // 使用用户报告的精确属性组合，确保测试覆盖真实问题而不是相邻场景。
        let finished_facts = SoulFacts::with_attributes(
            "破势",
            1,
            6,
            15,
            "attack_flat",
            486.0,
            None,
            vec![
                attr("hp_rate", 0.029525486399542833, Some(0)),
                attr("defense_rate", 0.024731710438677997, Some(0)),
                attr("crit_damage", 0.20512969501709571, Some(5)),
                attr("effect_resist", 0.03745831469277963, Some(0)),
            ],
        );

        let score = RuleEngine::new()
            .evaluate_standard_score(&preset, &finished_facts)
            .expect("成品应生成标准评分");
        assert_eq!(score.matched_rule_id, "finished-crit-higher");
        assert_eq!(score.score, 60.0);
    }

    #[test]
    fn 头胚子和速度胚子的边界严格区分主属性与四腿() {
        let preset = load_default_preset().expect("默认预设");
        let engine = RuleEngine::new();
        let head = engine.evaluate_admission(&preset, &facts(2, "speed", 3, &["speed"]));
        let head_rule = head
            .rules
            .iter()
            .find(|rule| rule.rule_id == "head-embryo")
            .expect("头胚子规则");
        assert!(head_rule.matched);
        let speed_rule = head
            .rules
            .iter()
            .find(|rule| rule.rule_id == "speed-embryo-four")
            .expect("速度胚子规则");
        assert!(!speed_rule.matched);

        let non_speed_main =
            engine.evaluate_admission(&preset, &facts(4, "attack_rate", 4, &["speed"]));
        assert!(
            non_speed_main
                .rules
                .iter()
                .find(|rule| rule.rule_id == "speed-embryo-four")
                .unwrap()
                .matched
        );
        assert!(
            !non_speed_main
                .rules
                .iter()
                .find(|rule| rule.rule_id == "head-embryo")
                .unwrap()
                .matched
        );
        let three_legs =
            engine.evaluate_admission(&preset, &facts(4, "attack_rate", 3, &["speed"]));
        assert!(
            !three_legs
                .rules
                .iter()
                .find(|rule| rule.rule_id == "speed-embryo-four")
                .unwrap()
                .matched
        );
    }

    #[test]
    fn 套装选择器只命中绑定的御魂() {
        // 套装强化配置必须直接绑定稳定套装标识，不能误命中其他套装。
        let selector = Selector {
            set_ids: vec!["破势".to_owned()],
            ..Selector::default()
        };
        let po_shi = selector.evaluate(&facts(4, "attack_rate", 4, &["crit_rate"]));
        assert!(po_shi.matched);
        assert_eq!(po_shi.checks[0].label, "御魂套装");

        let mut zhen_nv = facts(4, "attack_rate", 4, &["crit_rate"]);
        zhen_nv.set_id = "针女".to_owned();
        let rejected = selector.evaluate(&zhen_nv);
        assert!(!rejected.matched);
        assert!(!rejected.checks[0].matched);
    }

    #[test]
    fn DSL拒绝未知操作深度超限和缺少除零策略() {
        let unknown = RuleExpression {
            op: "execute".to_owned(),
            ..constant(1.0)
        };
        assert!(matches!(
            validate_expression(&unknown, &ValidationLimits::default()),
            Err(RuleValidationError::UnknownOperator(_))
        ));
        let div = RuleExpression {
            op: "div".to_owned(),
            args: vec![constant(1.0), constant(0.0)],
            ..constant(1.0)
        };
        assert!(validate_expression(&div, &ValidationLimits::default()).is_err());
        let mut deep = constant(1.0);
        for _ in 0..MAX_EXPRESSION_DEPTH {
            deep = RuleExpression {
                op: "abs".to_owned(),
                args: vec![deep],
                ..constant(0.0)
            };
        }
        assert!(matches!(
            validate_expression(&deep, &ValidationLimits::default()),
            Err(RuleValidationError::ExpressionTooDeep { .. })
        ));
    }

    #[test]
    fn 分数不随库存练度变化但续投门槛会变化() {
        let template = simple_template("output", "输出", &["crit_rate"], EvidenceLevel::Community);
        let item = SoulFacts::with_attributes(
            "破势",
            6,
            6,
            0,
            "crit_rate",
            1.0,
            Some(3),
            vec![attr("crit_rate", 0.8, Some(1))],
        );
        let context = EvaluationContext::from_soul(&item);
        let engine = RuleEngine::new();
        let starter = engine
            .evaluate_use(&template, &item, &context, MaturityLevel::Starter)
            .expect("starter");
        let deep = engine
            .evaluate_use(&template, &item, &context, MaturityLevel::Deep)
            .expect("deep");
        assert_eq!(starter.score, deep.score);
        assert_ne!(starter.recommendation, deep.recommendation);
    }

    #[test]
    fn 未知强化次数保持未知且有效成长与歪点按用途计算() {
        let template = simple_template("output", "输出", &["crit_rate"], EvidenceLevel::Community);
        let item = SoulFacts::with_attributes(
            "破势",
            6,
            6,
            6,
            "crit_damage",
            1.0,
            Some(4),
            vec![
                attr("crit_rate", 0.15, Some(2)),
                attr("speed", 18.0, Some(1)),
            ],
        );
        let result = RuleEngine::new()
            .evaluate_use(
                &template,
                &item,
                &EvaluationContext::from_soul(&item),
                MaturityLevel::Starter,
            )
            .expect("用途评估");
        assert_eq!(result.effective_growth_count, Some(2.0));
        assert_eq!(result.crooked_points, Some(1));
        assert_eq!(result.next_checkpoint, Some(9));
        let unknown = SoulFacts::with_attributes(
            "破势",
            6,
            6,
            6,
            "crit_damage",
            1.0,
            Some(4),
            vec![attr("crit_rate", 0.15, None)],
        );
        let unknown_result = RuleEngine::new()
            .evaluate_use(
                &template,
                &unknown,
                &EvaluationContext::from_soul(&unknown),
                MaturityLevel::Starter,
            )
            .expect("未知成长");
        assert_eq!(unknown_result.effective_growth_count, None);
        assert_eq!(unknown_result.crooked_points, None);
    }

    #[test]
    fn 安全合并只允许启用用途参与且保留可以否决回收() {
        assert_eq!(
            safe_merge([
                (true, Recommendation::Recycle),
                (true, Recommendation::Observe)
            ]),
            Recommendation::Observe
        );
        assert_eq!(
            safe_merge([
                (true, Recommendation::Recycle),
                (false, Recommendation::Keep)
            ]),
            Recommendation::Recycle
        );
        assert_eq!(
            safe_merge([(true, Recommendation::Stop), (true, Recommendation::Keep)]),
            Recommendation::Keep
        );
    }

    #[test]
    fn 代表性用途模板明确标记草案和未核验证据() {
        let templates = builtin_use_templates();
        let ids = templates
            .iter()
            .map(|template| template.id.as_str())
            .collect::<BTreeSet<_>>();
        for expected in [
            "fuyue-defense",
            "inaba-output",
            "ji-output",
            "pingjiangmen-defense-output",
            "sp-cat-resist-output",
            "flower-bird-healing",
            "peacock-hit-crit",
        ] {
            assert!(ids.contains(expected));
        }
        assert!(
            templates
                .iter()
                .all(|template| template.status == RuleStatus::Draft)
        );
        assert!(
            templates
                .iter()
                .all(|template| template.evidence_level == EvidenceLevel::Unverified)
        );
    }

    #[test]
    fn 自动练度只接受完整快照并使用四档默认边界() {
        let thresholds = vec![
            MaturityThreshold {
                scope_type: "system".to_owned(),
                profile_id: String::new(),
                level: MaturityLevel::Starter,
                min_count: 0,
            },
            MaturityThreshold {
                scope_type: "system".to_owned(),
                profile_id: String::new(),
                level: MaturityLevel::Growth,
                min_count: 500,
            },
            MaturityThreshold {
                scope_type: "system".to_owned(),
                profile_id: String::new(),
                level: MaturityLevel::Formed,
                min_count: 1500,
            },
            MaturityThreshold {
                scope_type: "system".to_owned(),
                profile_id: String::new(),
                level: MaturityLevel::Deep,
                min_count: 3000,
            },
        ];
        let souls = (0..500)
            .map(|_| {
                SoulFacts::with_attributes(
                    "破势",
                    1,
                    6,
                    15,
                    "attack_flat",
                    100.0,
                    Some(4),
                    Vec::new(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            infer_maturity(true, &souls, &thresholds),
            Some(MaturityLevel::Growth)
        );
        assert_eq!(infer_maturity(false, &souls, &thresholds), None);
    }
}
