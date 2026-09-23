//! 模拟强化应用用例。
//!
//! 用例负责读取当前库存、准备评分所需的目录事实，并把纯内存强化结果交给界面。
//! 它不会调用任何写入仓库的方法，因此模拟结果不会污染真实库存或分析待办。

use crate::application::error::AppError;
use crate::application::services::AppServices;
use crate::domain::{
    EnhancementRule, RuleEngine, SimulationRng, SimulationRollOutcome, SimulationRollout,
    SnapshotSoul, SoulAttribute, SoulAttributeFact, SoulFacts, StandardScore, load_default_preset,
    parse_rule_preset,
};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// 用户触发一次模拟强化时提交的参数；seed 允许用户复现同一批娱乐结果。
#[derive(Clone, Debug)]
pub struct SimulateEnhancementInput {
    pub threshold: f64,
    pub seed: Option<u64>,
}

/// 模拟强化所需的固定成本和规则版本摘要。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulationCostSummary {
    pub cost_version: String,
    pub mechanics_version: String,
    pub value_model_version: String,
    pub exp_per_soul: u64,
    pub gold_per_soul: u64,
    pub probability_note: String,
    pub value_model_note: String,
    pub value_model_source: String,
    pub cost_note: String,
    pub probability_source: String,
    pub cost_source: String,
}

/// 单枚御魂模拟后的可展示结果。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulatedSoulResult {
    pub soul_key: String,
    pub set_id: String,
    pub set_name: String,
    pub slot: u8,
    pub quality: u8,
    pub level: u8,
    pub main_attr_type: String,
    pub main_attr_value: f64,
    pub initial_substat_count: Option<u8>,
    pub final_attributes: Vec<SimulationAttributeView>,
    pub rolls: Vec<SimulationRollView>,
    pub standard_score: f64,
    pub standard_score_rule: String,
    pub standard_score_formula: String,
    pub firework: bool,
}

/// 副属性结果 DTO；返回实际模拟值、理论区间和独立的强化次数，供前端逐行展示。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulationAttributeView {
    pub attribute_type: String,
    pub attribute_label: String,
    pub value: f64,
    pub value_min: f64,
    pub value_max: f64,
    pub enhancement_count: u8,
}

/// 强化节点轨迹 DTO；每个节点同时携带本次抽到的成长值，便于复盘数值变化。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulationRollView {
    pub checkpoint: u8,
    pub attribute_type: String,
    pub attribute_label: String,
    pub value_delta: f64,
    pub outcome: String,
    pub probability_basis: String,
}

/// 一次批量模拟的完整结果；所有字段均来自当前调用，不写入任何持久化表。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulateEnhancementResult {
    pub seed: u64,
    pub threshold: f64,
    pub eligible_count: u32,
    pub total_rolls: u32,
    pub total_exp: u64,
    pub total_gold: u64,
    pub entertainment_note: String,
    pub cost: SimulationCostSummary,
    pub results: Vec<SimulatedSoulResult>,
}

/// 模拟强化用例；依赖只读仓库和规则引擎，不持有页面状态。
pub struct SimulationUseCase {
    services: Arc<AppServices>,
}

impl SimulationUseCase {
    /// 创建模拟强化用例。
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 读取六星 +0 御魂并全部模拟到 +15；评分采用当前启用的标准评分规则。
    pub fn simulate(
        &self,
        profile_id: &str,
        input: SimulateEnhancementInput,
    ) -> Result<SimulateEnhancementResult, AppError> {
        if !input.threshold.is_finite() || !(0.0..=100.0).contains(&input.threshold) {
            return Err(AppError::invalid_argument(
                "threshold",
                "烟花阈值必须在 0 到 100 分之间",
            ));
        }
        let inventory = self.services.snapshots.list_inventory(profile_id)?;
        let eligible = inventory
            .into_iter()
            .filter(|item| item.presence_state != "removed")
            .collect::<Vec<_>>();
        let active_catalog = self
            .services
            .catalog
            .active_status()?
            .ok_or_else(|| AppError::not_found("catalog", "active"))?;
        let sets = self.services.catalog.list_sets(&active_catalog.version)?;
        let sets_by_id = sets
            .iter()
            .map(|set| (set.set_id.as_str(), set))
            .collect::<HashMap<_, _>>();
        let default_rule = sets
            .iter()
            .find_map(|set| set.enhancement_rule_json.as_deref())
            .ok_or_else(|| AppError::invalid_argument("enhancementRule", "目录缺少强化机制"))?;
        let enhancement_rule = serde_json::from_str::<EnhancementRule>(default_rule)
            .map_err(|error| AppError::internal(format!("强化机制解析失败：{error}")))?;

        let (group_map, commonness_map) =
            self.load_catalog_context(profile_id, &active_catalog.version)?;
        let preset = self.load_active_score_preset()?;
        let engine = RuleEngine::new();
        let mut facts_by_snapshot: HashMap<String, (Vec<SnapshotSoul>, Vec<SoulAttribute>)> =
            HashMap::new();
        for item in &eligible {
            if facts_by_snapshot.contains_key(&item.snapshot_id) {
                continue;
            }
            facts_by_snapshot.insert(
                item.snapshot_id.clone(),
                self.services
                    .imports
                    .list_snapshot_souls(&item.snapshot_id)?,
            );
        }

        let seed = input.seed.unwrap_or_else(default_seed);
        let mut rng = SimulationRng::new(seed);
        let mut results = Vec::new();
        for item in eligible {
            let Some((souls, attributes)) = facts_by_snapshot.get(&item.snapshot_id) else {
                continue;
            };
            let Some(soul) = souls
                .iter()
                .find(|soul| soul.internal_id == item.soul_internal_id)
            else {
                continue;
            };
            if soul.quality != 6 || soul.level != 0 {
                continue;
            }
            let Some(set) = sets_by_id.get(soul.set_id.as_str()) else {
                continue;
            };
            let initial_attributes = attributes
                .iter()
                .filter(|attribute| {
                    attribute.snapshot_id == soul.snapshot_id
                        && attribute.soul_internal_id == soul.internal_id
                        && !attribute.fixed_attribute
                })
                .map(|attribute| SoulAttributeFact {
                    attribute_type: attribute.attribute_type.clone(),
                    value: attribute.value,
                    enhancement_count: None,
                    count_provenance: Some("simulation".to_owned()),
                })
                .collect::<Vec<_>>();
            let rollout =
                crate::domain::simulate_rollout(&initial_attributes, &enhancement_rule, &mut rng);
            let score_facts = build_score_facts(
                soul,
                &rollout,
                group_map.get(&soul.set_id).cloned().unwrap_or_default(),
                commonness_map
                    .get(&soul.set_id)
                    .cloned()
                    .unwrap_or_default(),
            );
            let score = engine
                .evaluate_standard_score(&preset, &score_facts)
                .ok_or_else(|| AppError::internal("模拟御魂无法生成标准评分"))?;
            results.push(to_soul_result(
                &item.soul_key,
                soul,
                set.name.clone(),
                rollout,
                score,
                input.threshold,
            ));
        }

        // 评分高的结果优先展示；稳定键保证同分时每次结果顺序一致。
        results.sort_by(|left, right| {
            right
                .standard_score
                .total_cmp(&left.standard_score)
                .then_with(|| {
                    right
                        .initial_substat_count
                        .unwrap_or_default()
                        .cmp(&left.initial_substat_count.unwrap_or_default())
                })
                .then_with(|| left.soul_key.cmp(&right.soul_key))
        });
        let eligible_count = results.len() as u32;
        Ok(SimulateEnhancementResult {
            seed,
            threshold: input.threshold,
            eligible_count,
            total_rolls: eligible_count.saturating_mul(5),
            total_exp: eligible_count as u64 * crate::domain::SIX_STAR_PLUS15_EXP_COST,
            total_gold: eligible_count as u64 * crate::domain::SIX_STAR_PLUS15_GOLD_COST,
            entertainment_note: "模拟强化仅供娱乐，不计入实际库存、金币或御魂材料。".to_owned(),
            cost: SimulationCostSummary {
                cost_version: crate::domain::SIMULATION_COST_VERSION.to_owned(),
                mechanics_version: crate::domain::SIMULATION_MECHANICS_VERSION.to_owned(),
                value_model_version: crate::domain::SIMULATION_VALUE_MODEL_VERSION.to_owned(),
                exp_per_soul: crate::domain::SIX_STAR_PLUS15_EXP_COST,
                gold_per_soul: crate::domain::SIX_STAR_PLUS15_GOLD_COST,
                probability_note:
                    "官方公布类别概率；类别内具体属性采用等概率娱乐模拟。"
                        .to_owned(),
                value_model_note: "副属性单次成长按参考文档给出的最低值至最高值均匀抽样；具体档位概率未公开，因此属于娱乐估算。"
                    .to_owned(),
                value_model_source:
                    "用户提供：《阴阳师》御魂系统主副属性数值规律与强化模型分析.pdf"
                        .to_owned(),
                cost_note: "金币与御魂经验为六星御魂强化到+15的版本化娱乐估算。".to_owned(),
                probability_source: "https://yys.163.com/news/notice/2017/05/01/25369_665793.html"
                    .to_owned(),
                cost_source: "https://ngabbs.com/read.php?tid=25650273".to_owned(),
            },
            results,
        })
    }

    /// 读取套装分组和常用度，保持模拟评分与正式分析使用同一套目录上下文。
    fn load_catalog_context(
        &self,
        profile_id: &str,
        catalog_version: &str,
    ) -> Result<
        (
            HashMap<String, Vec<String>>,
            HashMap<String, BTreeMap<String, crate::domain::CommonnessValue>>,
        ),
        AppError,
    > {
        let mut group_map = HashMap::new();
        for group in self.services.catalog.list_groups()? {
            for member in self.services.catalog.group_members(&group.id)? {
                if member.catalog_version == catalog_version {
                    group_map
                        .entry(member.set_id)
                        .or_insert_with(Vec::new)
                        .push(group.id.clone());
                }
            }
        }
        let commonness_map = self
            .services
            .commonness
            .effective(profile_id, catalog_version)?
            .into_iter()
            .fold(HashMap::new(), |mut map, entry| {
                map.entry(entry.set_id)
                    .or_insert_with(BTreeMap::new)
                    .insert(entry.scenario, entry.value);
                map
            });
        Ok((group_map, commonness_map))
    }

    /// 解析全局当前评分标准；没有有效指针时回退内置标准，与正式分析保持一致。
    fn load_active_score_preset(&self) -> Result<crate::domain::ValidatedRulePreset, AppError> {
        // 模拟入口与正式分析共用旧数据库迁移，避免预览评分和库存评分使用不同标准。
        let active_score_standard_id =
            crate::application::services::promote_preferred_score_standard_if_needed(
                self.services.rules.as_ref(),
            )?;
        if let Some(active_id) = active_score_standard_id {
            if let Some(version) = self.services.rules.get_version(&active_id)? {
                return parse_rule_preset(
                    version.canonical_json.as_bytes(),
                    crate::domain::ValidationLimits::default(),
                )
                .map_err(|error| AppError::invalid_argument("scoreStandard", error.to_string()));
            }
        }
        load_default_preset()
            .map_err(|error| AppError::internal(format!("默认评分标准无法加载：{error}")))
    }
}

/// 把模拟结果转换为标准评分引擎需要的 +15 事实；数值来自同一次娱乐模拟，强化次数仍独立记录。
fn build_score_facts(
    soul: &SnapshotSoul,
    rollout: &SimulationRollout,
    set_group_ids: Vec<String>,
    commonness: BTreeMap<String, crate::domain::CommonnessValue>,
) -> SoulFacts {
    SoulFacts {
        set_id: soul.set_id.clone(),
        slot: soul.slot,
        quality: soul.quality,
        level: 15,
        main_attribute: soul.main_attr_type.clone(),
        main_value: soul.main_attr_value,
        initial_substat_count: soul.initial_substat_count,
        attributes: rollout
            .attributes
            .iter()
            .map(|attribute| SoulAttributeFact {
                attribute_type: attribute.attribute_type.clone(),
                value: attribute.value,
                enhancement_count: Some(attribute.enhancement_count),
                count_provenance: Some("simulation".to_owned()),
            })
            .collect(),
        set_group_ids,
        commonness,
        scenario: Some("PVE".to_owned()),
    }
}

/// 转换单枚御魂结果并只在最终达到阈值时标记一次烟花。
fn to_soul_result(
    soul_key: &str,
    soul: &SnapshotSoul,
    set_name: String,
    rollout: SimulationRollout,
    score: StandardScore,
    threshold: f64,
) -> SimulatedSoulResult {
    SimulatedSoulResult {
        soul_key: soul_key.to_owned(),
        set_id: soul.set_id.clone(),
        set_name,
        slot: soul.slot,
        quality: soul.quality,
        level: 15,
        main_attr_type: soul.main_attr_type.clone(),
        main_attr_value: soul.main_attr_value,
        initial_substat_count: soul.initial_substat_count,
        final_attributes: rollout
            .attributes
            .iter()
            .map(|attribute| SimulationAttributeView {
                attribute_type: attribute.attribute_type.clone(),
                attribute_label: crate::domain::attribute_display_name(&attribute.attribute_type)
                    .to_owned(),
                value: attribute.value,
                value_min: attribute.value_min,
                value_max: attribute.value_max,
                enhancement_count: attribute.enhancement_count,
            })
            .collect(),
        rolls: rollout
            .rolls
            .iter()
            .map(|roll| SimulationRollView {
                checkpoint: roll.checkpoint,
                attribute_type: roll.attribute_type.clone(),
                attribute_label: roll.attribute_label.clone(),
                value_delta: roll.value_delta,
                outcome: match roll.outcome {
                    SimulationRollOutcome::ExistingAttribute => "existingAttribute".to_owned(),
                    SimulationRollOutcome::NewAttribute => "newAttribute".to_owned(),
                },
                probability_basis: roll.probability_basis.clone(),
            })
            .collect(),
        standard_score: score.score,
        standard_score_rule: score.matched_rule_name,
        standard_score_formula: score.formula,
        firework: score.score >= threshold,
    }
}

/// 以纳秒时间戳的安全低位提供默认种子；显式传入 seed 时不会走该分支，便于复现结果。
fn default_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        // 前端以 JavaScript number 展示种子，限制在 53 位以内可避免大整数精度丢失。
        .map(|duration| duration.as_nanos() as u64 & ((1u64 << 53) - 1))
        .unwrap_or(1)
}
