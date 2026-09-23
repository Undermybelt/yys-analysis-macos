//! “神奇海螺”领域模块。
//!
//! 该模块把目标面板、套装配方和库存事实收敛为一个纯内存计算接口。
//! 页面与 Tauri 命令不需要了解套装计数、面板换算、候选剪枝或缺口排序规则。

use super::simulation::ENHANCEMENT_CHECKPOINTS;
use super::{Shikigami, SnapshotSoul, SoulAttribute, SoulSet};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Instant;

/// 散件在请求中的稳定伪套装 ID；它只参与配方匹配，不产生套装效果。
pub const SCATTERED_SET_ID: &str = "__scattered__";
/// 分类未指定具体套装时的稳定占位值；实际匹配由配方行的 category 决定。
pub const UNRESTRICTED_SET_ID: &str = "__unrestricted__";
/// 速度和暴击是首版缺口排序中最重要的属性。
const SPEED_WEIGHT: f64 = 8.0;
const CRIT_RATE_WEIGHT: f64 = 8.0;
const CRIT_DAMAGE_WEIGHT: f64 = 5.0;
const HIT_RESIST_WEIGHT: f64 = 3.0;
const BASIC_STAT_WEIGHT: f64 = 1.0;
/// 强化建议只做有限数量的替换评估，避免大库存和区间上限把主计算请求拖到不可用。
const MAX_ENHANCEMENT_CANDIDATE_EVALUATIONS: usize = 2_000;

/// 用户输入的目标面板；所有非空字段均解释为最低值。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleTarget {
    pub attack: Option<f64>,
    pub hp: Option<f64>,
    pub defense: Option<f64>,
    pub speed: Option<f64>,
    pub crit_rate: Option<f64>,
    pub crit_damage: Option<f64>,
    pub effect_hit: Option<f64>,
    pub effect_resist: Option<f64>,
}

/// 套装配方的一行；散件使用 `SCATTERED_SET_ID`，分类通配使用 `UNRESTRICTED_SET_ID`。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleRecipeLine {
    pub set_id: String,
    pub count: u8,
    /// 具体套装未选择时的一级分类约束；具体套装行可省略。
    #[serde(default)]
    pub category: Option<String>,
}

/// 前端传给 Tauri 的目标面板计算请求。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleConchRequest {
    pub shikigami_id: String,
    pub target: MiracleTarget,
    /// 只计算六星 +15 御魂；旧请求未携带该字段时也保持默认的严格筛选。
    #[serde(default = "default_only_max_level")]
    pub only_max_level: bool,
    /// 组合指标列表；每一项都必须达标，空列表时沿用八项面板最低值模式。
    #[serde(default)]
    pub metrics: Vec<MiracleMetric>,
    /// 2、4、6 号位的主属性限制；未列出的号位不限制主属性。
    #[serde(default)]
    pub main_attributes: Vec<MiracleMainAttributeConstraint>,
    pub recipe: Vec<MiracleRecipeLine>,
}

/// 为兼容旧版前端请求，缺少“仅满级”字段时沿用界面的默认严格规则。
fn default_only_max_level() -> bool {
    true
}

/// 一项可编辑的目标指标；公式由预设给出，属性限制表达为最低值和可选最高值。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleMetric {
    pub preset_id: String,
    pub metric_type: String,
    pub label: String,
    pub operands: Vec<String>,
    pub threshold: f64,
    pub percentage: bool,
    #[serde(default)]
    pub constraints: Vec<MiracleMetricConstraint>,
}

/// 组合指标中的面板属性区间，例如暴击 100%～120%、速度 152～180。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleMetricConstraint {
    pub attribute: String,
    pub minimum: f64,
    /// 未传入时不限制上限；保留 Option 以兼容旧版只携带 minimum 的请求。
    #[serde(default)]
    pub maximum: Option<f64>,
}

/// 2/4/6 号位主属性限制。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleMainAttributeConstraint {
    pub slot: u8,
    pub main_attr_type: String,
}

/// 组合方案的实际指标、目标和缺口。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleMetricResult {
    pub preset_id: String,
    pub label: String,
    pub expression: String,
    pub actual: f64,
    pub threshold: f64,
    pub gap: f64,
    pub met: bool,
    pub percentage: bool,
    pub constraints: Vec<MiracleMetricConstraintResult>,
}

/// 组合指标中的属性限制实际值与缺口。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleMetricConstraintResult {
    pub attribute: String,
    pub actual: f64,
    pub minimum: f64,
    /// 未设置上限时序列化为 null，前端据此只展示下限。
    pub maximum: Option<f64>,
    pub gap: f64,
    pub met: bool,
}

/// 计算出的八项静态面板；百分比字段均保存为小数。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiraclePanel {
    pub attack: f64,
    pub hp: f64,
    pub defense: f64,
    pub speed: f64,
    pub crit_rate: f64,
    pub crit_damage: f64,
    pub effect_hit: f64,
    pub effect_resist: f64,
}

/// 目标差值；正数表示尚未达到目标，零表示达标。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiraclePanelGap {
    pub attack: f64,
    pub hp: f64,
    pub defense: f64,
    pub speed: f64,
    pub crit_rate: f64,
    pub crit_damage: f64,
    pub effect_hit: f64,
    pub effect_resist: f64,
}

/// 参与计算的单枚御魂；应用层负责把库存投影与快照事实在进入领域层前合并。
#[derive(Clone, Debug)]
pub struct MiracleSoul {
    pub soul: SnapshotSoul,
    pub attributes: Vec<SoulAttribute>,
}

/// 已激活但没有换算成静态面板的套装效果说明。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleSetEffectNote {
    pub set_id: String,
    pub set_name: String,
    pub piece_count: u8,
    pub kind: String,
    pub effect: String,
    pub counted_in_panel: bool,
}

/// 一套六件组合及其可解释面板结果。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleSolution {
    pub souls: Vec<MiracleSoulView>,
    pub set_counts: BTreeMap<String, u8>,
    pub panel: MiraclePanel,
    pub gap: MiraclePanelGap,
    pub met_attributes: Vec<String>,
    pub unmet_attributes: Vec<String>,
    pub metric: Option<MiracleMetricResult>,
    pub metrics: Vec<MiracleMetricResult>,
    pub counted_set_effects: Vec<MiracleSetEffectNote>,
    pub uncounted_set_effects: Vec<MiracleSetEffectNote>,
    pub approximate: bool,
}

/// 前端展示的御魂摘要；不把整份快照原始 JSON 暴露给 WebView。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleSoulView {
    pub soul_key: String,
    pub set_id: String,
    pub slot: u8,
    pub quality: u8,
    pub level: u8,
    pub main_attr_type: String,
    pub main_attr_value: f64,
    pub equipped_state: Option<String>,
    pub initial_substat_count: Option<u8>,
    pub attributes: Vec<MiracleAttributeView>,
}

/// 御魂属性展示摘要；保留原始顺序、固有标记和未知强化次数，供前端复用“我的御魂”展示口径。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleAttributeView {
    pub attribute_index: u8,
    pub attribute_type: String,
    pub value: f64,
    pub enhancement_count: Option<u8>,
    pub count_provenance: String,
    pub fixed_attribute: bool,
}

/// 单枚胚子强化后的副属性期望展示；数值是平均投影，不代表某一次随机结果。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleExpectedAttributeView {
    pub attribute_index: u8,
    pub attribute_type: String,
    pub current_value: f64,
    pub expected_value: f64,
    pub expected_added_value: f64,
    /// 典型强化轨迹中会落在该属性上的强化手数；始终为整数，不展示概率均摊手数。
    pub expected_enhancement_count: u8,
    pub fixed_attribute: bool,
    pub is_new_attribute: bool,
    /// 三腿新增属性的归一化类别概率；已有属性没有该字段。
    pub probability: Option<f64>,
}

/// 某个位置在最佳组合中的相对短板。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleSlotGap {
    pub slot: u8,
    pub impact_score: f64,
    pub missing_attributes: Vec<String>,
    pub reason: String,
}

/// 可继续强化的库存胚子建议。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleEnhancementCandidate {
    /// 当前方案中被替换的原始御魂，用于和推荐胚子并排对照。
    pub original_soul: MiracleSoulView,
    pub soul: MiracleSoulView,
    pub slot: u8,
    pub current_gap_score: f64,
    pub theoretical_gap_score: f64,
    pub possible_improvement: f64,
    /// 候选御魂替换到当前方案后的面板，用于前端展示强化前后的理论收益。
    pub current_panel: MiraclePanel,
    pub current_gap: MiraclePanelGap,
    /// 理论强化上限对应的面板缺口，用于前端展示预期收益。
    pub theoretical_gap: MiraclePanelGap,
    pub theoretical_panel: MiraclePanel,
    /// 与理论面板同一条最有利强化路径上的副属性明细，避免前端根据汇总面板猜测高亮属性。
    pub theoretical_attributes: Vec<MiracleExpectedAttributeView>,
    /// 按新增属性类别概率和后续四腿均匀强化计算的平均面板，仅用于期望收益展示。
    pub expected_improvement: f64,
    pub expected_gap: MiraclePanelGap,
    pub expected_panel: MiraclePanel,
    pub expected_attributes: Vec<MiracleExpectedAttributeView>,
    pub current_metric: Option<MiracleMetricResult>,
    pub theoretical_metric: Option<MiracleMetricResult>,
    pub expected_metric: Option<MiracleMetricResult>,
    pub current_metrics: Vec<MiracleMetricResult>,
    pub theoretical_metrics: Vec<MiracleMetricResult>,
    pub expected_metrics: Vec<MiracleMetricResult>,
    /// 记录该候选在稳定排序中的具体依据，避免页面只显示无法复核的名次。
    pub rank_explanation: String,
    pub reason: String,
}

/// 库存不足时的购买方向；只描述位置、主属性和副属性方向，不虚构商店来源。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiraclePurchaseSuggestion {
    pub slot: u8,
    pub set_direction: String,
    pub main_attribute: String,
    pub priority_sub_attributes: Vec<String>,
    pub missing_attribute: String,
    pub priority_score: f64,
    pub reason: String,
}

/// 神奇海螺完整结果；状态限定为 matched、closest 或 no_candidate。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MiracleConchResult {
    pub status: String,
    pub search_complete: bool,
    pub candidate_count: u32,
    pub excluded_unconfirmed_count: u32,
    pub excluded_non_six_star_count: u32,
    pub solutions: Vec<MiracleSolution>,
    pub slot_gaps: Vec<MiracleSlotGap>,
    pub enhancement_candidates: Vec<MiracleEnhancementCandidate>,
    pub purchase_suggestions: Vec<MiraclePurchaseSuggestion>,
    pub quality_notes: Vec<String>,
}

/// 计算目标面板组合；该函数是领域模块对应用层暴露的唯一主接口。
pub fn calculate_miracle_conch(
    request: &MiracleConchRequest,
    shikigami: &Shikigami,
    sets: &[SoulSet],
    inventory: &[MiracleSoul],
    excluded_unconfirmed_count: u32,
    excluded_non_six_star_count: u32,
) -> Result<MiracleConchResult, String> {
    // 领域计算不依赖应用层日志；这里单独记录关键阶段，便于区分组合搜索和强化建议的耗时。
    let calculation_started_at = Instant::now();
    validate_request(request, shikigami, sets)?;
    let set_map = sets
        .iter()
        .map(|set| (set.set_id.clone(), set.clone()))
        .collect::<HashMap<_, _>>();
    let base_panel = parse_shikigami_panel(shikigami)?;
    let candidates = inventory
        .iter()
        // 六星是首版配装范围；勾选“仅满级”时进一步排除所有未达到 +15 的御魂。
        .filter(|item| item.soul.quality == 6 && (!request.only_max_level || item.soul.level == 15))
        .cloned()
        .collect::<Vec<_>>();
    tracing::info!(
        target: "miracle_conch",
        candidate_count = candidates.len(),
        inventory_count = inventory.len(),
        elapsed_ms = calculation_started_at.elapsed().as_millis() as u64,
        "神奇海螺六星候选筛选完成"
    );
    let mut quality_notes = Vec::new();
    if excluded_unconfirmed_count > 0 {
        quality_notes.push(format!(
            "已排除 {} 枚局部快照未确认御魂；它们不能作为当前库存方案依据。",
            excluded_unconfirmed_count
        ));
    }
    // 号位候选会在这里完成套装相关剪枝；只有确实丢弃候选时才标记为近似搜索。
    let (slot_candidates, _is_approximate) =
        build_slot_candidates(&candidates, request, &set_map, &base_panel);
    tracing::info!(
        target: "miracle_conch",
        slot_counts = ?slot_candidates.iter().map(Vec::len).collect::<Vec<_>>(),
        elapsed_ms = calculation_started_at.elapsed().as_millis() as u64,
        "神奇海螺号位候选构建完成"
    );
    // 使用三号位+三号位 Meet-in-the-middle；两侧各自做无损前沿剪枝后再按配方余量合并。
    let search = search_meet_in_middle(&slot_candidates, request, &set_map, &base_panel);
    tracing::info!(
        target: "miracle_conch",
        search_nodes = search.nodes,
        search_complete = search.complete,
        solution_count = search.solutions.len(),
        elapsed_ms = calculation_started_at.elapsed().as_millis() as u64,
        "神奇海螺组合搜索完成"
    );

    let mut solutions = search.solutions;
    solutions.sort_by(solution_order);
    solutions.truncate(3);
    if !search.complete {
        for solution in &mut solutions {
            solution.approximate = true;
        }
    }
    let status = if solutions.is_empty() {
        "no_candidate"
    } else if is_target_met(&solutions[0].panel, &request.target, &request.metrics) {
        "matched"
    } else {
        "closest"
    };

    let best = solutions.first().cloned();
    // 已达标时没有需要替换的位置，号位缺口与购买、强化建议保持一致地返回空列表。
    let slot_gaps = if status == "matched" {
        Vec::new()
    } else {
        best.as_ref()
            .map(|solution| build_slot_gaps(solution, &request.target, &request.metrics))
            .unwrap_or_default()
    };
    // 已达标时当前方案已经满足目标，不再扫描未满级库存生成强化建议，避免返回无意义的替换方向。
    let enhancement_analysis = if status == "matched" {
        EnhancementCandidateBuild {
            candidates: Vec::new(),
            truncated: false,
        }
    } else {
        best.as_ref()
            .map(|solution| {
                // “仅满级”只控制毕业组合搜索；未达标时强化建议仍需查看库存中的六星未满级胚子。
                build_enhancement_candidates(solution, inventory, request, &set_map, &base_panel)
            })
            .unwrap_or_else(|| EnhancementCandidateBuild {
                candidates: Vec::new(),
                truncated: false,
            })
    };
    let enhancement_candidates = enhancement_analysis.candidates;
    if enhancement_analysis.truncated {
        quality_notes.push(format!(
            "当前库存较大，强化建议仅评估前 {} 枚候选；配装主方案仍已完成完整搜索。",
            MAX_ENHANCEMENT_CANDIDATE_EVALUATIONS
        ));
    }
    tracing::info!(
        target: "miracle_conch",
        enhancement_count = enhancement_candidates.len(),
        elapsed_ms = calculation_started_at.elapsed().as_millis() as u64,
        "神奇海螺强化建议整理完成"
    );
    // 已达标时不展示购买方向；未达标或无方案时才给出补齐目标的购买建议。
    let purchase_suggestions = if status == "matched" {
        Vec::new()
    } else {
        best.as_ref()
            .map(|solution| build_purchase_suggestions(solution, request, &set_map))
            .unwrap_or_else(|| build_purchase_suggestions_without_solution(request, &set_map))
    };

    Ok(MiracleConchResult {
        status: status.to_owned(),
        search_complete: search.complete,
        candidate_count: candidates.len() as u32,
        excluded_unconfirmed_count,
        excluded_non_six_star_count,
        solutions,
        slot_gaps,
        enhancement_candidates,
        purchase_suggestions,
        quality_notes,
    })
}

/// 校验请求的可计算前置条件，防止空目标或非法件数进入组合搜索。
fn validate_request(
    request: &MiracleConchRequest,
    shikigami: &Shikigami,
    sets: &[SoulSet],
) -> Result<(), String> {
    if request.shikigami_id != shikigami.shikigami_id {
        return Err("请求的式神不在当前目录中".to_owned());
    }
    if !has_target(&request.target, &request.metrics) {
        return Err("至少填写一项目标面板或组合指标后才能计算".to_owned());
    }
    for metric in &request.metrics {
        validate_metric(metric)?;
    }
    validate_main_attributes(&request.main_attributes)?;
    if request.recipe.is_empty() {
        return Err("至少添加一行套装配方".to_owned());
    }
    let known_ids = sets
        .iter()
        .map(|set| set.set_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut total = 0_u8;
    for line in &request.recipe {
        if line.count == 0 {
            return Err("套装件数必须大于 0".to_owned());
        }
        if line.set_id == UNRESTRICTED_SET_ID {
            let Some(category) = line.category.as_deref().filter(|value| !value.is_empty()) else {
                return Err("分类通配配方必须提供套装方向".to_owned());
            };
            if !sets
                .iter()
                .any(|set| catalog_set_category(set) == Some(category))
            {
                return Err(format!("目录中不存在套装方向：{category}"));
            }
        } else if line.set_id != SCATTERED_SET_ID
            && line.set_id != "散件"
            && !known_ids.contains(line.set_id.as_str())
        {
            return Err(format!("目录中不存在套装：{}", line.set_id));
        }
        let duplicate_key = if line.set_id == UNRESTRICTED_SET_ID {
            format!(
                "{UNRESTRICTED_SET_ID}:{}",
                line.category.as_deref().unwrap_or_default()
            )
        } else {
            line.set_id.clone()
        };
        if !seen.insert(duplicate_key) {
            return Err(format!(
                "套装配方不能重复填写：{}",
                line.category.as_deref().unwrap_or(&line.set_id)
            ));
        }
        total = total.saturating_add(line.count);
    }
    if total != 6 {
        return Err(format!("套装配方总件数必须为 6，当前为 {total}"));
    }
    Ok(())
}

/// 返回目录采用的一级套装方向；特殊套装优先于普通属性分类，与前端两级选择保持一致。
fn catalog_set_category(set: &SoulSet) -> Option<&str> {
    set.special_category.as_deref().or(set.category.as_deref())
}

/// 判断单枚御魂是否满足一行配方，支持具体套装、分类通配和完全不限制三种语义。
fn recipe_line_matches_soul(
    line: &MiracleRecipeLine,
    soul: &MiracleSoul,
    set_map: &HashMap<String, SoulSet>,
) -> bool {
    if line.set_id == SCATTERED_SET_ID || line.set_id == "散件" {
        return true;
    }
    if line.set_id == UNRESTRICTED_SET_ID {
        return line
            .category
            .as_deref()
            .map(|category| {
                set_map
                    .get(&soul.soul.set_id)
                    .and_then(catalog_set_category)
                    == Some(category)
            })
            .unwrap_or(true);
    }
    soul.soul.set_id == line.set_id
}

/// 读取目录中已核验的满级基础面板，并把暴伤转换为游戏展示口径的总暴伤。
fn parse_shikigami_panel(shikigami: &Shikigami) -> Result<MiraclePanel, String> {
    let value: Value = serde_json::from_str(&shikigami.panel_json)
        .map_err(|error| format!("式神基础面板数据损坏：{error}"))?;
    let number = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_f64)
            .ok_or_else(|| format!("式神基础面板缺少字段：{key}"))
    };
    Ok(MiraclePanel {
        attack: number("attack")?,
        hp: number("hp")?,
        defense: number("defense")?,
        speed: number("speed")?,
        crit_rate: number("critRate")?,
        // 目录保存的是暴伤额外值，面板展示还包含基础 100%。
        crit_damage: 1.0 + number("critDamage")?,
        effect_hit: 0.0,
        effect_resist: 0.0,
    })
}

/// 为每个号位按目标相关度排序，并只做能证明不影响最优解的配方过滤与支配剪枝。
fn build_slot_candidates(
    inventory: &[MiracleSoul],
    request: &MiracleConchRequest,
    set_map: &HashMap<String, SoulSet>,
    base_panel: &MiraclePanel,
) -> ([Vec<MiracleSoul>; 6], bool) {
    let mut grouped: [Vec<MiracleSoul>; 6] = std::array::from_fn(|_| Vec::new());
    for item in inventory {
        if !(1..=6).contains(&item.soul.slot) {
            continue;
        }
        grouped[usize::from(item.soul.slot - 1)].push(item.clone());
    }
    for (slot_index, candidates) in grouped.iter_mut().enumerate() {
        // 先过滤号位主属性，再做支配剪枝，避免允许的主属性被不允许的御魂淘汰。
        let allowed_main_attributes = request
            .main_attributes
            .iter()
            .filter(|constraint| constraint.slot == slot_index as u8 + 1)
            .map(|constraint| constraint.main_attr_type.as_str())
            .collect::<BTreeSet<_>>();
        if !allowed_main_attributes.is_empty() {
            // 同一号位多选时使用 OR 语义：御魂主属性命中任一用户选择即可保留。
            candidates
                .retain(|item| allowed_main_attributes.contains(item.soul.main_attr_type.as_str()));
        }
        // 先排除无法匹配任何配方行的套装，避免无关套装占用分类通配的搜索空间。
        // 这是语义过滤而不是近似截断：最终组合中的每一枚御魂都必须匹配某一行配方。
        candidates.retain(|item| {
            request
                .recipe
                .iter()
                .any(|line| line.count > 0 && recipe_line_matches_soul(line, item, set_map))
        });
        candidates.sort_by(|left, right| {
            candidate_relevance(right, &request.target, &request.metrics, set_map)
                .partial_cmp(&candidate_relevance(
                    left,
                    &request.target,
                    &request.metrics,
                    set_map,
                ))
                .unwrap_or(Ordering::Equal)
                .then_with(|| soul_key(left).cmp(&soul_key(right)))
        });
        // 只删除同套装、同号位下被全面支配的御魂；该剪枝不会改变任何可达的最优面板。
        *candidates = prune_dominated_candidates(std::mem::take(candidates), request, base_panel);
    }
    // 候选过滤和支配剪枝都是无损操作，不能因此把结果标记成“近似方案”。
    (grouped, false)
}

/// 前沿状态保存配方行已使用件数、套装二件套计数、面板贡献和具体御魂选择。
///
/// 同一个“配方计数 + 套装计数”下，如果一个状态在全部面板贡献上都不差，
/// 另一个状态就不可能生成更优答案，因此可以安全合并。这是精确的 Pareto 剪枝，
/// 与“每个号位只取前 N 枚”不同，不会因候选排序改变答案。
#[derive(Clone)]
struct SearchFrontierState {
    used_recipe: Vec<u8>,
    set_counts: BTreeMap<String, u8>,
    /// 已激活的二件套面板贡献；同类套装的具体 ID 不影响静态面板结果。
    set_bonus: Contribution,
    /// 用固定长度计数记录各类静态二件套，避免把浮点贡献格式化进状态键。
    set_bonus_signature: [u8; 7],
    contribution: Contribution,
    /// 搜索中只保存每个号位候选数组的索引；完整御魂详情在生成最终方案时再还原。
    selected_indices: [usize; 6],
}

/// 使用三号位+三号位拆分搜索；每一侧只保留可由相同后缀/前缀补全的 Pareto 状态。
fn search_meet_in_middle(
    slot_candidates: &[Vec<MiracleSoul>; 6],
    request: &MiracleConchRequest,
    set_map: &HashMap<String, SoulSet>,
    base_panel: &MiraclePanel,
) -> SearchState {
    let relevant_attributes = target_attribute_order_for_request(&request.target, &request.metrics);
    let (left_states, left_nodes) = enumerate_mitm_half(
        slot_candidates,
        0,
        3,
        request,
        set_map,
        base_panel,
        &relevant_attributes,
    );
    let (right_states, right_nodes) = enumerate_mitm_half(
        slot_candidates,
        3,
        6,
        request,
        set_map,
        base_panel,
        &relevant_attributes,
    );
    let mut search = SearchState {
        nodes: left_nodes.saturating_add(right_nodes),
        complete: true,
        ..SearchState::default()
    };

    // 右半部分按配方件数建立索引；左半部分只查找恰好补齐六件配方的桶。
    let mut right_by_recipe: HashMap<Vec<u8>, Vec<SearchFrontierState>> = HashMap::new();
    for state in right_states {
        right_by_recipe
            .entry(state.used_recipe.clone())
            .or_default()
            .push(state);
    }
    // 合并阶段只需要最终排序最优的 12 个状态；旧实现先保存数百万个三维状态，
    // 再对每个桶执行平方级 Pareto 比较，狂骨 + 土蜘蛛会因此长期停在前端 94%。
    let mut best_merged_states = Vec::<RankedMergedState>::new();
    // 正常库存中的御魂只属于一个号位；仅在异常重复稳定 ID确实跨半区出现时执行逐配对检查。
    let check_cross_half_duplicates = has_cross_half_duplicate_keys(slot_candidates);
    for left in left_states {
        let required_right = left
            .used_recipe
            .iter()
            .enumerate()
            .map(|(index, used)| request.recipe[index].count.saturating_sub(*used))
            .collect::<Vec<_>>();
        let Some(right_bucket) = right_by_recipe.get(&required_right) else {
            continue;
        };
        for right in right_bucket {
            let Some(merged) = merge_mitm_pair(
                &left,
                right,
                slot_candidates,
                set_map,
                check_cross_half_duplicates,
            ) else {
                continue;
            };
            search.nodes = search.nodes.saturating_add(1);
            retain_best_merged_state(
                &mut best_merged_states,
                ranked_merged_state(merged, request, base_panel),
                slot_candidates,
            );
        }
    }

    // 排名键与 solution_order 完全一致；这里只为入选状态还原御魂详情和展示文案。
    for ranked in best_merged_states {
        let selected = ranked
            .selected_indices
            .iter()
            .enumerate()
            .filter_map(|(slot_index, candidate_index)| {
                slot_candidates
                    .get(slot_index)
                    .and_then(|candidates| candidates.get(*candidate_index))
                    .cloned()
            })
            .collect::<Vec<_>>();
        // 左右桶只在配方件数互为补集时进入合并，因此这里仅需防御索引还原失败。
        if selected.len() != 6 {
            continue;
        }
        search.solutions.push(make_solution(
            &selected,
            set_map,
            base_panel,
            &request.target,
            &request.metrics,
            false,
        ));
    }
    search.solutions.sort_by(solution_order);
    search.solutions.truncate(12);
    search
}

/// 合并状态的轻量排序事实；不提前创建完整方案，避免每个左右配对都克隆六枚御魂。
struct RankedMergedState {
    selected_indices: [usize; 6],
    missing_count: usize,
    metric_gap_score: f64,
    weighted_gap_score: f64,
    excess_score: f64,
}

/// 从已含二件套加成的合并贡献计算最终面板，口径必须与 calculate_panel 保持一致。
fn panel_from_contribution(base: &MiraclePanel, contribution: &Contribution) -> MiraclePanel {
    MiraclePanel {
        attack: base.attack * (1.0 + contribution.attack_rate) + contribution.attack_flat,
        hp: base.hp * (1.0 + contribution.hp_rate) + contribution.hp_flat,
        defense: base.defense * (1.0 + contribution.defense_rate) + contribution.defense_flat,
        speed: base.speed + contribution.speed,
        crit_rate: base.crit_rate + contribution.crit_rate,
        crit_damage: base.crit_damage + contribution.crit_damage,
        effect_hit: base.effect_hit + contribution.effect_hit,
        effect_resist: base.effect_resist + contribution.effect_resist,
    }
}

/// 计算与最终方案排序相同的轻量排名，不分配指标结果、御魂视图或套装说明。
fn ranked_merged_state(
    state: MergedSearchState,
    request: &MiracleConchRequest,
    base_panel: &MiraclePanel,
) -> RankedMergedState {
    let panel = panel_from_contribution(base_panel, &state.contribution);
    let gap = panel_gap(&panel, &request.target);
    let mut missing_count = [
        gap.attack,
        gap.hp,
        gap.defense,
        gap.speed,
        gap.crit_rate,
        gap.crit_damage,
        gap.effect_hit,
        gap.effect_resist,
    ]
    .into_iter()
    .filter(|value| *value > 1e-9)
    .count();
    let mut metric_score = 0.0;
    for metric in &request.metrics {
        let actual = match metric.metric_type.as_str() {
            "minimum" => metric
                .operands
                .iter()
                .filter_map(|operand| panel_attribute_value(&panel, operand))
                .fold(f64::INFINITY, f64::min),
            _ => metric
                .operands
                .iter()
                .filter_map(|operand| panel_attribute_value(&panel, operand))
                .product(),
        };
        let formula_gap = (metric.threshold - actual).max(0.0);
        let mut metric_met = formula_gap <= 1e-9;
        metric_score += formula_gap;
        for constraint in &metric.constraints {
            let actual = panel_attribute_value(&panel, &constraint.attribute).unwrap_or(0.0);
            let lower_gap = (constraint.minimum - actual).max(0.0);
            let upper_gap = constraint
                .maximum
                .map(|maximum| (actual - maximum).max(0.0))
                .unwrap_or(0.0);
            let constraint_gap = lower_gap.max(upper_gap);
            metric_score += constraint_gap;
            metric_met &= constraint_gap <= 1e-9;
        }
        if !metric_met {
            missing_count += 1;
        }
    }
    RankedMergedState {
        selected_indices: state.selected_indices,
        missing_count,
        metric_gap_score: metric_score,
        weighted_gap_score: weighted_gap(&gap),
        excess_score: excess_score(&panel, &gap),
    }
}

/// 比较两个轻量状态；最后仍使用六枚御魂稳定键，保证同分结果不随遍历顺序抖动。
fn ranked_merged_state_order(
    left: &RankedMergedState,
    right: &RankedMergedState,
    slot_candidates: &[Vec<MiracleSoul>; 6],
) -> Ordering {
    left.missing_count
        .cmp(&right.missing_count)
        .then_with(|| {
            left.metric_gap_score
                .partial_cmp(&right.metric_gap_score)
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| {
            left.weighted_gap_score
                .partial_cmp(&right.weighted_gap_score)
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| {
            left.excess_score
                .partial_cmp(&right.excess_score)
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| {
            merged_solution_key(&left.selected_indices, slot_candidates).cmp(&merged_solution_key(
                &right.selected_indices,
                slot_candidates,
            ))
        })
}

/// 只在完全同分时生成连接后的稳定键；正常数值比较路径不产生字符串分配。
fn merged_solution_key(
    selected_indices: &[usize; 6],
    slot_candidates: &[Vec<MiracleSoul>; 6],
) -> String {
    selected_indices
        .iter()
        .enumerate()
        .map(|(slot_index, candidate_index)| {
            soul_key_at(slot_candidates, slot_index, *candidate_index)
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// 在线维护固定 12 个最佳合并状态，避免物化完整笛卡尔积及其后续平方级剪枝。
fn retain_best_merged_state(
    best: &mut Vec<RankedMergedState>,
    candidate: RankedMergedState,
    slot_candidates: &[Vec<MiracleSoul>; 6],
) {
    const SEARCH_SOLUTION_LIMIT: usize = 12;
    if best.len() < SEARCH_SOLUTION_LIMIT {
        best.push(candidate);
        best.sort_by(|left, right| ranked_merged_state_order(left, right, slot_candidates));
        return;
    }
    let Some(worst) = best.last() else {
        return;
    };
    if ranked_merged_state_order(&candidate, worst, slot_candidates) != Ordering::Less {
        return;
    }
    best.pop();
    best.push(candidate);
    best.sort_by(|left, right| ranked_merged_state_order(left, right, slot_candidates));
}

/// 枚举一个三号位半区，并按当前已知的配方/套装/面板状态做无损前沿剪枝。
fn enumerate_mitm_half(
    slot_candidates: &[Vec<MiracleSoul>; 6],
    start_slot: usize,
    end_slot: usize,
    request: &MiracleConchRequest,
    set_map: &HashMap<String, SoulSet>,
    base_panel: &MiraclePanel,
    relevant_attributes: &[String],
) -> (Vec<SearchFrontierState>, usize) {
    let mut frontier = vec![SearchFrontierState {
        used_recipe: vec![0; request.recipe.len()],
        set_counts: BTreeMap::new(),
        set_bonus: Contribution::default(),
        set_bonus_signature: [0; 7],
        contribution: Contribution::default(),
        selected_indices: [usize::MAX; 6],
    }];
    let mut nodes = 0usize;
    for slot_index in start_slot..end_slot {
        let mut next: HashMap<String, Vec<SearchFrontierState>> = HashMap::new();
        for state in frontier {
            for (candidate_index, candidate) in slot_candidates[slot_index].iter().enumerate() {
                // 保持与旧搜索一致的唯一御魂约束；跨半区的重复在合并阶段再次检查。
                if state.selected_indices.iter().enumerate().any(
                    |(previous_slot, selected_index)| {
                        *selected_index != usize::MAX
                            && soul_key_at(slot_candidates, previous_slot, *selected_index)
                                == soul_key(candidate)
                    },
                ) {
                    continue;
                }
                let matching_lines = request
                    .recipe
                    .iter()
                    .enumerate()
                    .filter(|(index, line)| {
                        state.used_recipe[*index] < line.count
                            && recipe_line_matches_soul(line, candidate, set_map)
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                for line_index in matching_lines {
                    let mut used_recipe = state.used_recipe.clone();
                    used_recipe[line_index] += 1;
                    let selected_count = used_recipe
                        .iter()
                        .map(|count| usize::from(*count))
                        .sum::<usize>();
                    let remaining_slots = 6usize.saturating_sub(selected_count);
                    // 剩余号位连某一配方行都无法补齐时，半区状态不可能在另一侧完成。
                    if used_recipe.iter().enumerate().any(|(index, count)| {
                        usize::from(*count) + remaining_slots
                            < usize::from(request.recipe[index].count)
                    }) {
                        continue;
                    }
                    let mut set_counts = state.set_counts.clone();
                    let count = set_counts.entry(candidate.soul.set_id.clone()).or_insert(0);
                    let was_two_piece_active = *count >= 2;
                    *count = (*count + 1).min(2);
                    let mut set_bonus = state.set_bonus;
                    let mut set_bonus_signature = state.set_bonus_signature;
                    let mut contribution = state.contribution;
                    add_soul_contribution(&mut contribution, candidate);
                    if !was_two_piece_active && *count >= 2 {
                        if let Some((attribute_type, value)) = set_map
                            .get(&candidate.soul.set_id)
                            .and_then(catalog_set_category)
                            .and_then(category_bonus)
                        {
                            add_attribute(&mut set_bonus, attribute_type, value);
                            add_attribute(&mut contribution, attribute_type, value);
                            if let Some(index) = static_bonus_signature_index(attribute_type) {
                                set_bonus_signature[index] =
                                    set_bonus_signature[index].saturating_add(1);
                            }
                        }
                    }
                    let mut selected_indices = state.selected_indices;
                    selected_indices[slot_index] = candidate_index;
                    let next_state = SearchFrontierState {
                        used_recipe,
                        set_counts,
                        set_bonus,
                        set_bonus_signature,
                        contribution,
                        selected_indices,
                    };
                    nodes = nodes.saturating_add(1);
                    next.entry(frontier_state_key(&next_state))
                        .or_default()
                        .push(next_state);
                }
            }
        }
        frontier = next
            .into_values()
            .flat_map(|states| prune_frontier_bucket(states, relevant_attributes, base_panel))
            .collect();
        tracing::info!(
            target: "miracle_conch",
            half_start = start_slot + 1,
            slot = slot_index + 1,
            candidate_count = slot_candidates[slot_index].len(),
            frontier_count = frontier.len(),
            search_nodes = nodes,
            "神奇海螺 Meet-in-the-middle 半区搜索完成一个号位"
        );
        if frontier.is_empty() {
            break;
        }
    }
    (frontier, nodes)
}

/// 读取指定号位候选的稳定 ID，统一处理半区状态中的空索引。
fn soul_key_at(
    slot_candidates: &[Vec<MiracleSoul>; 6],
    slot_index: usize,
    candidate_index: usize,
) -> String {
    slot_candidates[slot_index][candidate_index]
        .soul
        .source_stable_id
        .clone()
        .unwrap_or_else(|| {
            slot_candidates[slot_index][candidate_index]
                .soul
                .internal_id
                .clone()
        })
}

/// 在合并热路径借用稳定 ID，避免为每一次跨半区重复检查分配字符串。
fn soul_key_ref(item: &MiracleSoul) -> &str {
    item.soul
        .source_stable_id
        .as_deref()
        .unwrap_or(&item.soul.internal_id)
}

/// 预先判断稳定 ID 是否异常地同时出现在左右半区；正常库存可跳过数百万次重复检查。
fn has_cross_half_duplicate_keys(slot_candidates: &[Vec<MiracleSoul>; 6]) -> bool {
    let left_keys = slot_candidates[..3]
        .iter()
        .flatten()
        .map(soul_key_ref)
        .collect::<HashSet<_>>();
    slot_candidates[3..]
        .iter()
        .flatten()
        .map(soul_key_ref)
        .any(|key| left_keys.contains(key))
}

/// 合并热路径只保留排名所需的面板贡献和六个索引，避免每个配对复制映射与配方数组。
struct MergedSearchState {
    contribution: Contribution,
    selected_indices: [usize; 6],
}

/// 合并左右半区，并补上恰好跨越两个半区形成的二件套静态属性。
fn merge_mitm_pair(
    left: &SearchFrontierState,
    right: &SearchFrontierState,
    slot_candidates: &[Vec<MiracleSoul>; 6],
    set_map: &HashMap<String, SoulSet>,
    check_cross_half_duplicates: bool,
) -> Option<MergedSearchState> {
    if check_cross_half_duplicates {
        // 同一枚御魂异常地出现在不同号位时，合并阶段不能让它被使用两次。
        for left_slot in 0..3 {
            let left_index = left.selected_indices[left_slot];
            if left_index == usize::MAX {
                continue;
            }
            let left_key = soul_key_ref(&slot_candidates[left_slot][left_index]);
            for right_slot in 3..6 {
                let right_index = right.selected_indices[right_slot];
                if right_index != usize::MAX
                    && left_key == soul_key_ref(&slot_candidates[right_slot][right_index])
                {
                    return None;
                }
            }
        }
    }

    let mut contribution = left.contribution;
    add_contribution(&mut contribution, &right.contribution);
    for (set_id, right_count) in &right.set_counts {
        let left_count = left.set_counts.get(set_id).copied().unwrap_or(0);
        // 左右各自只有一件时，二件套在任一半区都尚未激活，需要在合并时补一次。
        if left_count == 1 && *right_count == 1 {
            if let Some((attribute_type, value)) = set_map
                .get(set_id)
                .and_then(catalog_set_category)
                .and_then(category_bonus)
            {
                add_attribute(&mut contribution, attribute_type, value);
            }
        }
    }
    let mut selected_indices = left.selected_indices;
    for slot_index in 3..6 {
        selected_indices[slot_index] = right.selected_indices[slot_index];
    }
    Some(MergedSearchState {
        contribution,
        selected_indices,
    })
}

/// 累加两个面板贡献，供半区状态合并时复用，避免重新遍历六枚御魂。
fn add_contribution(target: &mut Contribution, source: &Contribution) {
    target.attack_flat += source.attack_flat;
    target.attack_rate += source.attack_rate;
    target.hp_flat += source.hp_flat;
    target.hp_rate += source.hp_rate;
    target.defense_flat += source.defense_flat;
    target.defense_rate += source.defense_rate;
    target.speed += source.speed;
    target.crit_rate += source.crit_rate;
    target.crit_damage += source.crit_damage;
    target.effect_hit += source.effect_hit;
    target.effect_resist += source.effect_resist;
}

/// 按六个号位逐层扩展精确状态，并在每层按等价状态做 Pareto 剪枝。
///
/// 该实现只保留给本地排查历史结果使用，生产计算统一走上面的 Meet-in-the-middle。
#[cfg(test)]
fn search_frontier(
    slot_candidates: &[Vec<MiracleSoul>; 6],
    request: &MiracleConchRequest,
    set_map: &HashMap<String, SoulSet>,
    base_panel: &MiraclePanel,
) -> SearchState {
    let mut search = SearchState {
        complete: true,
        ..SearchState::default()
    };
    // Pareto 只比较用户真正填写的目标和指标属性；无关面板字段不会影响是否达标，
    // 继续纳入会把大量等价状态错误地保留到前沿中。目标字段为空时由校验提前拒绝。
    let relevant_attributes = target_attribute_order_for_request(&request.target, &request.metrics);
    let mut frontier = vec![SearchFrontierState {
        used_recipe: vec![0; request.recipe.len()],
        set_counts: BTreeMap::new(),
        set_bonus: Contribution::default(),
        set_bonus_signature: [0; 7],
        contribution: Contribution::default(),
        selected_indices: [usize::MAX; 6],
    }];
    // 固定号位的稳定御魂 ID只预计算一次，异常重复快照检查不再为每个状态重复创建字符串。
    let candidate_keys = slot_candidates
        .iter()
        .map(|candidates| candidates.iter().map(soul_key).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    // 搜索阶段可能在前端长期显示 94%；按号位记录前沿规模和累计节点，便于定位具体爆炸层。
    let search_started_at = Instant::now();

    for (slot_index, candidates) in slot_candidates.iter().enumerate() {
        let mut next: HashMap<String, Vec<SearchFrontierState>> = HashMap::new();
        for state in frontier {
            for (candidate_index, candidate) in candidates.iter().enumerate() {
                // 保持旧搜索的“同一枚库存御魂只能使用一次”约束，兼容异常重复快照数据。
                if state.selected_indices.iter().enumerate().any(
                    |(previous_slot, selected_index)| {
                        *selected_index != usize::MAX
                            && candidate_keys[previous_slot][*selected_index]
                                == candidate_keys[slot_index][candidate_index]
                    },
                ) {
                    continue;
                }
                // 同一枚御魂可能同时满足具体套装行和分类通配行，逐行尝试才能避免贪心分配导致误报无解。
                let matching_lines = request
                    .recipe
                    .iter()
                    .enumerate()
                    .filter(|(index, line)| {
                        state.used_recipe[*index] < line.count
                            && recipe_line_matches_soul(line, candidate, set_map)
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                for line_index in matching_lines {
                    let mut used_recipe = state.used_recipe.clone();
                    used_recipe[line_index] += 1;
                    let selected_count = used_recipe
                        .iter()
                        .map(|count| usize::from(*count))
                        .sum::<usize>();
                    let remaining_slots = 6usize.saturating_sub(selected_count);
                    // 剩余号位连某一行尚缺的件数都填不满时，提前丢弃该状态。
                    if used_recipe.iter().enumerate().any(|(index, count)| {
                        usize::from(*count) + remaining_slots
                            < usize::from(request.recipe[index].count)
                    }) {
                        continue;
                    }
                    let mut set_counts = state.set_counts.clone();
                    let count = set_counts.entry(candidate.soul.set_id.clone()).or_insert(0);
                    // 面板计算只关心二件套是否激活；四件套展示仍由最终六枚御魂重新精确统计。
                    let was_two_piece_active = *count >= 2;
                    *count = (*count + 1).min(2);
                    let mut set_bonus = state.set_bonus;
                    let mut set_bonus_signature = state.set_bonus_signature;
                    // 只有从 1 件变为 2 件时才新增一次静态二件套效果；四件套战斗效果不参与面板搜索。
                    if !was_two_piece_active && *count >= 2 {
                        if let Some((attribute_type, value)) = set_map
                            .get(&candidate.soul.set_id)
                            .and_then(catalog_set_category)
                            .and_then(category_bonus)
                        {
                            add_attribute(&mut set_bonus, attribute_type, value);
                            if let Some(index) = static_bonus_signature_index(attribute_type) {
                                set_bonus_signature[index] =
                                    set_bonus_signature[index].saturating_add(1);
                            }
                        }
                    }
                    let mut contribution = state.contribution;
                    add_soul_contribution(&mut contribution, candidate);
                    if !was_two_piece_active && *count >= 2 {
                        if let Some((attribute_type, value)) = set_map
                            .get(&candidate.soul.set_id)
                            .and_then(catalog_set_category)
                            .and_then(category_bonus)
                        {
                            add_attribute(&mut contribution, attribute_type, value);
                        }
                    }
                    let mut selected_indices = state.selected_indices;
                    selected_indices[slot_index] = candidate_index;
                    let next_state = SearchFrontierState {
                        used_recipe,
                        set_counts,
                        set_bonus,
                        set_bonus_signature,
                        contribution,
                        selected_indices,
                    };
                    search.nodes += 1;
                    next.entry(frontier_state_key(&next_state))
                        .or_default()
                        .push(next_state);
                }
            }
        }
        frontier = next
            .into_values()
            .flat_map(|states| prune_frontier_bucket(states, &relevant_attributes, base_panel))
            .collect();
        tracing::info!(
            target: "miracle_conch",
            slot = slot_index + 1,
            candidate_count = candidates.len(),
            frontier_count = frontier.len(),
            search_nodes = search.nodes,
            elapsed_ms = search_started_at.elapsed().as_millis() as u64,
            "神奇海螺组合搜索完成一个号位"
        );
        if frontier.is_empty() {
            break;
        }
    }

    for state in frontier {
        if state
            .used_recipe
            .iter()
            .enumerate()
            .all(|(index, count)| *count == request.recipe[index].count)
        {
            let selected = state
                .selected_indices
                .iter()
                .enumerate()
                .filter_map(|(slot_index, candidate_index)| {
                    slot_candidates
                        .get(slot_index)
                        .and_then(|candidates| candidates.get(*candidate_index))
                        .cloned()
                })
                .collect::<Vec<_>>();
            if selected.len() != 6 {
                continue;
            }
            search.solutions.push(make_solution(
                &selected,
                set_map,
                base_panel,
                &request.target,
                &request.metrics,
                false,
            ));
        }
    }
    search.solutions.sort_by(solution_order);
    search.solutions.truncate(12);
    search
}

/// 为同一配方/静态面板状态生成稳定键。
///
/// 已经达到二件套的具体套装 ID 不再进入键，因为它们只会产生相同的静态面板贡献；
/// 仍保留一件套 ID，防止后续放入同套装御魂时丢失“下一枚会激活二件套”的可能性。
fn frontier_state_key(state: &SearchFrontierState) -> String {
    let recipe_key = state
        .used_recipe
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let open_set_key = state
        .set_counts
        .iter()
        .filter(|(_, count)| **count == 1)
        .map(|(set_id, count)| format!("{set_id}:{count}"))
        .collect::<Vec<_>>()
        .join("|");
    // 数组签名只保存静态二件套类别计数，避免为每个扩展状态格式化整组浮点字段。
    format!(
        "{recipe_key};{open_set_key};{:?}",
        state.set_bonus_signature
    )
}

/// 把可计入静态面板的二件套属性映射为紧凑签名下标；战斗型效果不进入搜索状态。
fn static_bonus_signature_index(attribute_type: &str) -> Option<usize> {
    match attribute_type {
        "attack_rate" => Some(0),
        "hp_rate" => Some(1),
        "defense_rate" => Some(2),
        "crit_rate" => Some(3),
        "crit_damage" => Some(4),
        "effect_hit" => Some(5),
        "effect_resist" => Some(6),
        _ => None,
    }
}

/// 把一枚御魂的主属性和副属性累加到前沿状态的面板贡献中。
fn add_soul_contribution(contribution: &mut Contribution, soul: &MiracleSoul) {
    add_attribute(
        contribution,
        &soul.soul.main_attr_type,
        soul.soul.main_attr_value,
    );
    for attribute in &soul.attributes {
        add_attribute(contribution, &attribute.attribute_type, attribute.value);
    }
}

/// 删除同一等价状态内被目标属性贡献支配的分支，并为完全相同贡献保留稳定代表。
///
/// 用户当前的火灵 + 暴击配置只有暴击率、暴击伤害两个有效维度，因此可以排序扫描；
/// 多目标配置仍使用通用 Pareto 比较，但不会再为每次比较临时分配属性数组和稳定键字符串。
fn prune_frontier_bucket(
    mut states: Vec<SearchFrontierState>,
    relevant_attributes: &[String],
    base_panel: &MiraclePanel,
) -> Vec<SearchFrontierState> {
    if relevant_attributes.len() <= 2 {
        states.sort_by(|left, right| {
            contribution_value(&right.contribution, &relevant_attributes[0], base_panel)
                .partial_cmp(&contribution_value(
                    &left.contribution,
                    &relevant_attributes[0],
                    base_panel,
                ))
                .unwrap_or(Ordering::Equal)
                .then_with(|| {
                    if relevant_attributes.len() < 2 {
                        Ordering::Equal
                    } else {
                        contribution_value(&right.contribution, &relevant_attributes[1], base_panel)
                            .partial_cmp(&contribution_value(
                                &left.contribution,
                                &relevant_attributes[1],
                                base_panel,
                            ))
                            .unwrap_or(Ordering::Equal)
                    }
                })
                .then_with(|| left.selected_indices.cmp(&right.selected_indices))
        });
        let mut retained = Vec::new();
        let mut highest_second = f64::NEG_INFINITY;
        let mut first_value = f64::NEG_INFINITY;
        for state in states {
            let current_first =
                contribution_value(&state.contribution, &relevant_attributes[0], base_panel);
            if relevant_attributes.len() == 1 {
                if current_first + 1e-9 < first_value {
                    continue;
                }
                if first_value.is_finite() {
                    continue;
                }
                first_value = current_first;
                retained.push(state);
                continue;
            }
            let current_second =
                contribution_value(&state.contribution, &relevant_attributes[1], base_panel);
            if current_second <= highest_second + 1e-9 {
                continue;
            }
            highest_second = current_second;
            retained.push(state);
        }
        return retained;
    }

    let mut retained: Vec<SearchFrontierState> = Vec::new();
    for candidate in states {
        let mut dominated = false;
        retained.retain(|existing| {
            if contribution_not_worse_for_attributes(
                &existing.contribution,
                &candidate.contribution,
                relevant_attributes,
                base_panel,
            ) && (contribution_strictly_better_for_attributes(
                &existing.contribution,
                &candidate.contribution,
                relevant_attributes,
                base_panel,
            ) || existing.selected_indices <= candidate.selected_indices)
            {
                dominated = true;
                return true;
            }
            if contribution_not_worse_for_attributes(
                &candidate.contribution,
                &existing.contribution,
                relevant_attributes,
                base_panel,
            ) && (contribution_strictly_better_for_attributes(
                &candidate.contribution,
                &existing.contribution,
                relevant_attributes,
                base_panel,
            ) || candidate.selected_indices < existing.selected_indices)
            {
                return false;
            }
            true
        });
        if !dominated {
            retained.push(candidate);
        }
    }
    retained
}

/// 读取指定面板属性对应的贡献；攻击、生命、防御按 PVE 共用的面板公式换算固定值和倍率。
///
/// 固定值不会再次乘百分比：最终面板统一使用“基础面板 × (1 + 百分比) + 白字固定值”。
/// 这里必须与 `calculate_panel` 保持一致，否则前沿剪枝会按另一套攻击口径误判状态。
fn contribution_value(
    contribution: &Contribution,
    attribute: &str,
    base_panel: &MiraclePanel,
) -> f64 {
    match attribute {
        "attack" => base_panel.attack * contribution.attack_rate + contribution.attack_flat,
        "hp" => base_panel.hp * contribution.hp_rate + contribution.hp_flat,
        "defense" => base_panel.defense * contribution.defense_rate + contribution.defense_flat,
        "speed" => contribution.speed,
        "crit_rate" => contribution.crit_rate,
        "crit_damage" => contribution.crit_damage,
        "effect_hit" => contribution.effect_hit,
        "effect_resist" => contribution.effect_resist,
        _ => 0.0,
    }
}

/// 判断左侧贡献在所有用户相关属性上都不低于右侧。
fn contribution_not_worse_for_attributes(
    left: &Contribution,
    right: &Contribution,
    attributes: &[String],
    base_panel: &MiraclePanel,
) -> bool {
    attributes.iter().all(|attribute| {
        contribution_value(left, attribute, base_panel) + 1e-9
            >= contribution_value(right, attribute, base_panel)
    })
}

/// 判断左侧贡献在所有用户相关属性上至少有一项严格更高。
fn contribution_strictly_better_for_attributes(
    left: &Contribution,
    right: &Contribution,
    attributes: &[String],
    base_panel: &MiraclePanel,
) -> bool {
    attributes.iter().any(|attribute| {
        contribution_value(left, attribute, base_panel)
            > contribution_value(right, attribute, base_panel) + 1e-9
    })
}

#[derive(Default)]
struct SearchState {
    nodes: usize,
    complete: bool,
    approximate: bool,
    solutions: Vec<MiracleSolution>,
}

/// 强化建议分析结果；主方案必须完整，强化候选允许在大库存下有界返回。
struct EnhancementCandidateBuild {
    candidates: Vec<MiracleEnhancementCandidate>,
    truncated: bool,
}

/// 同号位、同套装且目标属性全部不优于另一枚的御魂不会影响最优解，可安全删除。
fn prune_dominated_candidates(
    candidates: Vec<MiracleSoul>,
    request: &MiracleConchRequest,
    base_panel: &MiraclePanel,
) -> Vec<MiracleSoul> {
    let mut retained = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let dominated = candidates.iter().enumerate().any(|(other_index, other)| {
            index != other_index
                && other.soul.set_id == candidate.soul.set_id
                && dominates(
                    other,
                    candidate,
                    &request.target,
                    &request.metrics,
                    base_panel,
                )
        });
        if !dominated {
            retained.push(candidate.clone());
        }
    }
    retained
}

/// 判断另一枚御魂是否在同一位置、同一套装下对所有面板属性都不差且至少一项更好。
///
/// 必须比较完整面板字段，而不能只比较用户当前填写的目标；否则会把速度等
/// 未设为硬约束、但仍参与最终并列排序的属性更高的候选错误删除。
fn dominates(
    left: &MiracleSoul,
    right: &MiracleSoul,
    _target: &MiracleTarget,
    _metrics: &[MiracleMetric],
    base_panel: &MiraclePanel,
) -> bool {
    let mut strictly_better = false;
    const PANEL_ATTRIBUTES: [&str; 8] = [
        "attack",
        "hp",
        "defense",
        "speed",
        "crit_rate",
        "crit_damage",
        "effect_hit",
        "effect_resist",
    ];
    for attribute in PANEL_ATTRIBUTES {
        let left_value = soul_attribute_value(left, &attribute, base_panel);
        let right_value = soul_attribute_value(right, &attribute, base_panel);
        if left_value + 1e-9 < right_value {
            return false;
        }
        if left_value > right_value + 1e-9 {
            strictly_better = true;
        }
    }
    strictly_better
}

/// 将目标属性映射到一枚御魂的主副属性贡献，固定值和倍率在同一属性内比较。
fn soul_attribute_value(soul: &MiracleSoul, target: &str, base_panel: &MiraclePanel) -> f64 {
    let mut flat = 0.0;
    let mut rate = 0.0;
    let mut direct = 0.0;
    let mut add = |attribute_type: &str, value: f64| match attribute_type {
        "attack_flat" if target == "attack" => flat += value,
        "attack_rate" if target == "attack" => rate += value,
        "hp_flat" if target == "hp" => flat += value,
        "hp_rate" if target == "hp" => rate += value,
        "defense_flat" if target == "defense" => flat += value,
        "defense_rate" if target == "defense" => rate += value,
        actual if actual == target => direct += value,
        _ => {}
    };
    add(&soul.soul.main_attr_type, soul.soul.main_attr_value);
    for attribute in &soul.attributes {
        add(&attribute.attribute_type, attribute.value);
    }
    match target {
        "attack" => flat + base_panel.attack * rate,
        "hp" => flat + base_panel.hp * rate,
        "defense" => flat + base_panel.defense * rate,
        _ => direct,
    }
}

/// 生成组合结果，并把套装效果分成“计入面板”和“仅展示”两类。
fn make_solution(
    selected: &[MiracleSoul],
    set_map: &HashMap<String, SoulSet>,
    base_panel: &MiraclePanel,
    target: &MiracleTarget,
    metrics: &[MiracleMetric],
    approximate: bool,
) -> MiracleSolution {
    let set_counts = selected.iter().fold(BTreeMap::new(), |mut counts, item| {
        *counts.entry(item.soul.set_id.clone()).or_insert(0) += 1;
        counts
    });
    let (panel, counted_set_effects, uncounted_set_effects) =
        calculate_panel(base_panel, selected, &set_counts, set_map);
    let gap = panel_gap(&panel, target);
    let (mut met_attributes, mut unmet_attributes) = attribute_status(&panel, target);
    let metrics_result = metrics
        .iter()
        .map(|metric| metric_result(&panel, metric))
        .collect::<Vec<_>>();
    for result in &metrics_result {
        if result.met {
            met_attributes.push(result.label.clone());
        } else {
            unmet_attributes.push(result.label.clone());
        }
    }
    MiracleSolution {
        souls: selected.iter().map(soul_view).collect(),
        set_counts,
        panel,
        gap,
        met_attributes,
        unmet_attributes,
        metric: metrics_result.first().cloned(),
        metrics: metrics_result,
        counted_set_effects,
        uncounted_set_effects,
        approximate,
    }
}

/// 叠加御魂主副属性和可计算的二件套面板加成。
fn calculate_panel(
    base: &MiraclePanel,
    selected: &[MiracleSoul],
    set_counts: &BTreeMap<String, u8>,
    set_map: &HashMap<String, SoulSet>,
) -> (
    MiraclePanel,
    Vec<MiracleSetEffectNote>,
    Vec<MiracleSetEffectNote>,
) {
    let mut contribution = Contribution::default();
    for item in selected {
        add_attribute(
            &mut contribution,
            &item.soul.main_attr_type,
            item.soul.main_attr_value,
        );
        for attribute in &item.attributes {
            add_attribute(
                &mut contribution,
                &attribute.attribute_type,
                attribute.value,
            );
        }
    }
    let mut counted = Vec::new();
    let mut uncounted = Vec::new();
    for (set_id, count) in set_counts {
        let Some(set) = set_map.get(set_id) else {
            continue;
        };
        if *count >= 2 {
            if let Some(category) = set.category.as_deref().and_then(category_bonus) {
                add_attribute(&mut contribution, category.0, category.1);
                counted.push(MiracleSetEffectNote {
                    set_id: set_id.clone(),
                    set_name: set.name.clone(),
                    piece_count: 2,
                    kind: "two_piece_panel".to_owned(),
                    effect: set.two_piece_effect_json.clone().unwrap_or_default(),
                    counted_in_panel: true,
                });
            } else if let Some(effect) = set.two_piece_effect_json.clone() {
                uncounted.push(MiracleSetEffectNote {
                    set_id: set_id.clone(),
                    set_name: set.name.clone(),
                    piece_count: 2,
                    kind: "two_piece_battle_effect".to_owned(),
                    effect,
                    counted_in_panel: false,
                });
            }
        }
        if *count >= 4 {
            if let Some(effect) = set.four_piece_effect_json.clone() {
                uncounted.push(MiracleSetEffectNote {
                    set_id: set_id.clone(),
                    set_name: set.name.clone(),
                    piece_count: 4,
                    kind: "four_piece_battle_effect".to_owned(),
                    effect,
                    counted_in_panel: false,
                });
            }
        }
    }
    // 面板公式与前端 PVE 引擎保持一致：百分比只作用于基础面板，白字固定值最后相加。
    let panel = MiraclePanel {
        attack: base.attack * (1.0 + contribution.attack_rate) + contribution.attack_flat,
        hp: base.hp * (1.0 + contribution.hp_rate) + contribution.hp_flat,
        defense: base.defense * (1.0 + contribution.defense_rate) + contribution.defense_flat,
        speed: base.speed + contribution.speed,
        crit_rate: base.crit_rate + contribution.crit_rate,
        crit_damage: base.crit_damage + contribution.crit_damage,
        effect_hit: base.effect_hit + contribution.effect_hit,
        effect_resist: base.effect_resist + contribution.effect_resist,
    };
    (panel, counted, uncounted)
}

#[derive(Clone, Copy, Debug, Default)]
struct Contribution {
    attack_flat: f64,
    attack_rate: f64,
    hp_flat: f64,
    hp_rate: f64,
    defense_flat: f64,
    defense_rate: f64,
    speed: f64,
    crit_rate: f64,
    crit_damage: f64,
    effect_hit: f64,
    effect_resist: f64,
}

/// 把规范属性 ID写入面板中间态；未知属性保留在原始御魂视图但不冒充面板贡献。
fn add_attribute(contribution: &mut Contribution, attribute_type: &str, value: f64) {
    match attribute_type {
        "attack_flat" => contribution.attack_flat += value,
        "attack_rate" => contribution.attack_rate += value,
        "hp_flat" => contribution.hp_flat += value,
        "hp_rate" => contribution.hp_rate += value,
        "defense_flat" => contribution.defense_flat += value,
        "defense_rate" => contribution.defense_rate += value,
        "speed" => contribution.speed += value,
        "crit_rate" => contribution.crit_rate += value,
        "crit_damage" => contribution.crit_damage += value,
        "effect_hit" => contribution.effect_hit += value,
        "effect_resist" => contribution.effect_resist += value,
        _ => {}
    }
}

/// 把普通二件套分类映射为面板型属性；首领/星痕的特殊效果不在此处伪造数值。
fn category_bonus(category: &str) -> Option<(&'static str, f64)> {
    match category {
        "攻击加成" => Some(("attack_rate", 0.15)),
        "生命加成" => Some(("hp_rate", 0.15)),
        "防御加成" => Some(("defense_rate", 0.30)),
        "暴击" => Some(("crit_rate", 0.15)),
        "暴击伤害" => Some(("crit_damage", 0.20)),
        "效果命中" => Some(("effect_hit", 0.15)),
        "效果抵抗" => Some(("effect_resist", 0.15)),
        _ => None,
    }
}

/// 为目标属性生成缺口，并且只把填写过的属性加入达标状态。
fn panel_gap(panel: &MiraclePanel, target: &MiracleTarget) -> MiraclePanelGap {
    MiraclePanelGap {
        attack: gap(panel.attack, target.attack),
        hp: gap(panel.hp, target.hp),
        defense: gap(panel.defense, target.defense),
        speed: gap(panel.speed, target.speed),
        crit_rate: gap(panel.crit_rate, target.crit_rate),
        crit_damage: gap(panel.crit_damage, target.crit_damage),
        effect_hit: gap(panel.effect_hit, target.effect_hit),
        effect_resist: gap(panel.effect_resist, target.effect_resist),
    }
}

fn gap(actual: f64, target: Option<f64>) -> f64 {
    target
        .map(|target| (target - actual).max(0.0))
        .unwrap_or(0.0)
}

fn is_target_met(panel: &MiraclePanel, target: &MiracleTarget, metrics: &[MiracleMetric]) -> bool {
    panel_gap(panel, target) == MiraclePanelGap::default()
        && metrics
            .iter()
            .all(|metric| metric_result(panel, metric).met)
}

/// 输出属性名称列表，供前端不依赖字段名做文案分支。
fn attribute_status(panel: &MiraclePanel, target: &MiracleTarget) -> (Vec<String>, Vec<String>) {
    let fields = [
        ("攻击", panel.attack, target.attack),
        ("生命", panel.hp, target.hp),
        ("防御", panel.defense, target.defense),
        ("速度", panel.speed, target.speed),
        ("暴击", panel.crit_rate, target.crit_rate),
        ("暴伤", panel.crit_damage, target.crit_damage),
        ("效果命中", panel.effect_hit, target.effect_hit),
        ("效果抵抗", panel.effect_resist, target.effect_resist),
    ];
    let mut met = Vec::new();
    let mut unmet = Vec::new();
    for (label, actual, required) in fields {
        let Some(required) = required else { continue };
        if actual + 1e-9 >= required {
            met.push(label.to_owned());
        } else {
            unmet.push(label.to_owned());
        }
    }
    (met, unmet)
}

/// 接近度排序先比较未达标属性数，再比较加权缺口，最后用稳定键消除同分抖动。
fn solution_order(left: &MiracleSolution, right: &MiracleSolution) -> Ordering {
    let left_missing = left.unmet_attributes.len();
    let right_missing = right.unmet_attributes.len();
    left_missing
        .cmp(&right_missing)
        .then_with(|| {
            metric_gap_score(left)
                .partial_cmp(&metric_gap_score(right))
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| {
            weighted_gap(&left.gap)
                .partial_cmp(&weighted_gap(&right.gap))
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| {
            excess_score(&left.panel, &left.gap)
                .partial_cmp(&excess_score(&right.panel, &right.gap))
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| solution_key(left).cmp(&solution_key(right)))
}

/// 组合指标的缺口作为面板缺口之后的稳定排序依据，避免只看面板字段时指标模式全部同分。
fn metric_gap_score(solution: &MiracleSolution) -> f64 {
    solution
        .metrics
        .iter()
        .map(|metric| {
            metric.gap
                + metric
                    .constraints
                    .iter()
                    .map(|constraint| constraint.gap)
                    .sum::<f64>()
        })
        .sum()
}

fn weighted_gap(gap: &MiraclePanelGap) -> f64 {
    gap.attack * BASIC_STAT_WEIGHT
        + gap.hp * BASIC_STAT_WEIGHT / 1000.0
        + gap.defense * BASIC_STAT_WEIGHT
        + gap.speed * SPEED_WEIGHT
        + gap.crit_rate * CRIT_RATE_WEIGHT * 100.0
        + gap.crit_damage * CRIT_DAMAGE_WEIGHT * 100.0
        + gap.effect_hit * HIT_RESIST_WEIGHT * 100.0
        + gap.effect_resist * HIT_RESIST_WEIGHT * 100.0
}

fn excess_score(panel: &MiraclePanel, gap: &MiraclePanelGap) -> f64 {
    (panel.attack - gap.attack).max(0.0)
        + (panel.hp - gap.hp).max(0.0) / 1000.0
        + (panel.defense - gap.defense).max(0.0)
        + (panel.speed - gap.speed).max(0.0)
}

fn solution_key(solution: &MiracleSolution) -> String {
    solution
        .souls
        .iter()
        .map(|soul| soul.soul_key.as_str())
        .collect::<Vec<_>>()
        .join("|")
}

/// 通过组合中各号位对当前缺口的贡献，找出更值得查找的号位。
fn build_slot_gaps(
    solution: &MiracleSolution,
    target: &MiracleTarget,
    _metrics: &[MiracleMetric],
) -> Vec<MiracleSlotGap> {
    let missing = solution.unmet_attributes.clone();
    let mut gaps = solution
        .souls
        .iter()
        .map(|soul| {
            let impact_score = if missing.is_empty() {
                0.0
            } else {
                missing
                    .iter()
                    .map(|attribute| attribute_contribution_for_view(soul, attribute))
                    .sum::<f64>()
            };
            MiracleSlotGap {
                slot: soul.slot,
                impact_score,
                missing_attributes: missing.clone(),
                reason: if missing.is_empty() {
                    "当前组合已达到填写的目标，可优先比较该号位的替代御魂。".to_owned()
                } else {
                    format!(
                        "该号位当前对{}的贡献较少，优先查找更高主属性或副属性。",
                        missing.join("、")
                    )
                },
            }
        })
        .collect::<Vec<_>>();
    gaps.sort_by(|left, right| {
        right
            .impact_score
            .partial_cmp(&left.impact_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.slot.cmp(&right.slot))
    });
    let _ = target;
    gaps
}

/// 估算单枚御魂对目标属性的直接贡献，用于展示“哪个位置查得比较多”。
fn attribute_contribution_for_view(soul: &MiracleSoulView, label: &str) -> f64 {
    let wanted = match label {
        "攻击" => ["attack_flat", "attack_rate"].as_slice(),
        "生命" => ["hp_flat", "hp_rate"].as_slice(),
        "防御" => ["defense_flat", "defense_rate"].as_slice(),
        "速度" => ["speed"].as_slice(),
        "暴击" => ["crit_rate"].as_slice(),
        "暴伤" => ["crit_damage"].as_slice(),
        "效果命中" => ["effect_hit"].as_slice(),
        "效果抵抗" => ["effect_resist"].as_slice(),
        _ => [].as_slice(),
    };
    soul.attributes
        .iter()
        .filter(|attribute| wanted.contains(&attribute.attribute_type.as_str()))
        .map(|attribute| attribute.value)
        .sum::<f64>()
        + if wanted.contains(&soul.main_attr_type.as_str()) {
            soul.main_attr_value
        } else {
            0.0
        }
}

/// 获取强化候选用于排序和准入判断的先天副属性条数。
///
/// 优先使用导入数据记录的初始条数；缺少事实时才使用当前可见副属性数作为保守下界，
/// 这样不会把“后来新增的一条腿”误认为御魂出生时就有完整副属性池。
fn enhancement_initial_leg_count(soul: &MiracleSoul) -> usize {
    soul.soul
        .initial_substat_count
        .map(|count| usize::from(count))
        .unwrap_or_else(|| {
            soul.attributes
                .iter()
                .filter(|attribute| !attribute.fixed_attribute)
                .count()
        })
}

/// 判断下一强化节点是否仍处于“必然新增第四腿”的阶段。
///
/// 初始条数是事实优先级更高的字段；如果导入记录显示先天三腿但当前已经出现第四条，
/// 说明新增腿已经发生，后续应按四腿均匀强化处理。
fn is_incomplete_enhancement_stage(soul: &MiracleSoul) -> bool {
    let initial_leg_count = enhancement_initial_leg_count(soul);
    let current_leg_count = soul
        .attributes
        .iter()
        .filter(|attribute| !attribute.fixed_attribute)
        .count();
    initial_leg_count < 4 && current_leg_count < 4
}

/// 对每个可继续强化的替换候选计算当前缺口、理论上限和期望收益，不把任一结果解释成概率承诺。
///
/// 两腿胚子不进入建议列表；三腿胚子下一次强化必然新增副属性，但新增的具体属性未知，
/// 因此同时给出最有利分支的理论上限和按目录类别概率计算的期望收益。
fn build_enhancement_candidates(
    solution: &MiracleSolution,
    inventory: &[MiracleSoul],
    request: &MiracleConchRequest,
    set_map: &HashMap<String, SoulSet>,
    base_panel: &MiraclePanel,
) -> EnhancementCandidateBuild {
    // 最佳组合中的六枚御魂先一次性从库存建立索引；原实现每个强化候选都线性查找六次，
    // 大库存下会把“整理建议”阶段放大成候选数 × 库存数的重复扫描。
    let selected_inventory = solution
        .souls
        .iter()
        .filter_map(|selected| find_inventory_soul(selected, inventory))
        .collect::<Vec<_>>();
    if selected_inventory.len() != solution.souls.len() {
        return EnhancementCandidateBuild {
            candidates: Vec::new(),
            truncated: false,
        };
    }
    let inventory_by_slot: [Vec<&MiracleSoul>; 6] = std::array::from_fn(|slot_index| {
        inventory
            .iter()
            .filter(|item| item.soul.slot == slot_index as u8 + 1)
            .collect()
    });
    let base_set_counts: BTreeMap<String, u8> =
        selected_inventory
            .iter()
            .fold(BTreeMap::new(), |mut counts, item| {
                *counts.entry(item.soul.set_id.clone()).or_insert(0_u8) += 1;
                counts
            });
    // 同一套装、同一号位的多个候选共享配方匹配结果；缓存键只包含六个套装 ID，
    // 不包含副属性，因此不会把不同御魂错误合并到面板计算中。
    let mut recipe_match_cache = HashMap::<String, bool>::new();
    let mut result = Vec::new();
    let mut evaluated_candidates = 0_usize;
    let mut truncated = false;
    'selected_slots: for (selected_index, selected) in solution.souls.iter().enumerate() {
        let current_slot = selected.slot;
        let Some(slot_candidates) = inventory_by_slot.get(usize::from(current_slot - 1)) else {
            continue;
        };
        for candidate in slot_candidates.iter().copied() {
            if !(item_is_enhancement_candidate(candidate, current_slot, selected, solution)) {
                continue;
            }
            // 强化建议不是主方案的一部分；达到上限后立即返回已有结果，避免用户被低价值的
            // 全库存理论投影长时间阻塞。结果会通过 quality_notes 明确标注为部分评估。
            if evaluated_candidates >= MAX_ENHANCEMENT_CANDIDATE_EVALUATIONS {
                truncated = true;
                break 'selected_slots;
            }
            evaluated_candidates += 1;
            // 先天缺两条副属性时，后续需要连续赌中新增腿，概率过低，不作为当前强化建议。
            let initial_leg_count = enhancement_initial_leg_count(candidate);
            if initial_leg_count < 3 {
                continue;
            }
            let mut replacement = selected_inventory.clone();
            if candidate.soul_key() != selected.soul_key
                && replacement
                    .iter()
                    .any(|item| soul_key(item) == soul_key(candidate))
            {
                continue;
            }
            replacement[selected_index] = candidate.clone();
            let old_set_id = selected_inventory[selected_index].soul.set_id.clone();
            let new_set_id = candidate.soul.set_id.clone();
            let mut set_counts = base_set_counts.clone();
            if old_set_id != new_set_id {
                if let Some(count) = set_counts.get_mut(&old_set_id) {
                    *count = (*count).saturating_sub(1);
                }
                if set_counts.get(&old_set_id) == Some(&0) {
                    set_counts.remove(&old_set_id);
                }
                *set_counts.entry(new_set_id).or_insert(0) += 1;
            }
            // 强化建议必须保持当前套装配方，否则它其实是换套装而不是强化当前胚子。
            let recipe_key = selection_set_key(&replacement);
            let matches_recipe = *recipe_match_cache
                .entry(recipe_key)
                .or_insert_with(|| selection_matches_recipe(&replacement, request, set_map));
            if !matches_recipe {
                continue;
            }
            let (current_panel, _, _) =
                calculate_panel(base_panel, &replacement, &set_counts, set_map);
            let current_gap = panel_gap(&current_panel, &request.target);
            let current_score =
                request_gap_score(&current_panel, &request.target, &request.metrics);
            let (theoretical_soul, theoretical_panel) = theoretical_projection(
                candidate,
                &replacement,
                &set_counts,
                set_map,
                base_panel,
                &request.target,
                &request.metrics,
            )
            .unwrap_or_else(|| (candidate.clone(), current_panel.clone()));
            let theoretical_score =
                request_gap_score(&theoretical_panel, &request.target, &request.metrics);
            let possible_improvement = (current_score - theoretical_score).max(0.0);
            let expected_panel =
                expected_panel(candidate, &replacement, &set_counts, set_map, base_panel);
            let expected_score =
                request_gap_score(&expected_panel, &request.target, &request.metrics);
            let expected_improvement = (current_score - expected_score).max(0.0);
            // 没有现有属性可成长、且新增属性的平均收益也无法改善目标时，不占用建议名额。
            if possible_improvement <= 1e-9 && expected_improvement <= 1e-9 {
                continue;
            }
            let reason = if is_incomplete_enhancement_stage(candidate) {
                format!(
                    "这是{}腿胚子；下一次强化必然新增第4条副属性，但具体属性未知。期望收益按目录类别概率与类别内均分估算，只有命中目标属性才会有效；理论上限取最有利新增属性分支。",
                    initial_leg_count
                )
            } else {
                "当前已有完整副属性池；理论收益按剩余节点全部命中最有利属性估算，期望收益按四条副属性均匀命中和平均成长估算。".to_owned()
            };
            result.push(MiracleEnhancementCandidate {
                original_soul: selected.clone(),
                soul: soul_view(candidate),
                slot: current_slot,
                current_gap_score: current_score,
                theoretical_gap_score: theoretical_score,
                possible_improvement,
                current_panel: current_panel.clone(),
                current_gap,
                theoretical_gap: panel_gap(&theoretical_panel, &request.target),
                theoretical_panel: theoretical_panel.clone(),
                theoretical_attributes: build_theoretical_attributes(
                    candidate,
                    &theoretical_soul,
                    set_map,
                ),
                expected_improvement,
                expected_gap: panel_gap(&expected_panel, &request.target),
                expected_panel: expected_panel.clone(),
                expected_attributes: build_expected_attributes(candidate, set_map),
                current_metric: request
                    .metrics
                    .first()
                    .map(|metric| metric_result(&current_panel, metric)),
                theoretical_metric: request
                    .metrics
                    .first()
                    .map(|metric| metric_result(&theoretical_panel, metric)),
                expected_metric: request
                    .metrics
                    .first()
                    .map(|metric| metric_result(&expected_panel, metric)),
                current_metrics: request
                    .metrics
                    .iter()
                    .map(|metric| metric_result(&current_panel, metric))
                    .collect(),
                theoretical_metrics: request
                    .metrics
                    .iter()
                    .map(|metric| metric_result(&theoretical_panel, metric))
                    .collect(),
                expected_metrics: request
                    .metrics
                    .iter()
                    .map(|metric| metric_result(&expected_panel, metric))
                    .collect(),
                rank_explanation: String::new(),
                reason,
            });
        }
    }
    result.sort_by(|left, right| {
        // 胚子质量优先于纸面理论收益：先天四腿胚子更可控，先天三腿胚子只作为次级建议。
        let left_legs = left
            .soul
            .initial_substat_count
            .map(|count| usize::from(count))
            .unwrap_or_else(|| {
                left.soul
                    .attributes
                    .iter()
                    .filter(|attribute| !attribute.fixed_attribute)
                    .count()
            });
        let right_legs = right
            .soul
            .initial_substat_count
            .map(|count| usize::from(count))
            .unwrap_or_else(|| {
                right
                    .soul
                    .attributes
                    .iter()
                    .filter(|attribute| !attribute.fixed_attribute)
                    .count()
            });
        right_legs
            .cmp(&left_legs)
            .then_with(|| {
                right
                    .expected_improvement
                    .partial_cmp(&left.expected_improvement)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| {
                right
                    .possible_improvement
                    .partial_cmp(&left.possible_improvement)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.slot.cmp(&right.slot))
            .then_with(|| left.soul.soul_key.cmp(&right.soul.soul_key))
    });
    // 排序完成后再写入解释，确保页面展示的名次和实际比较链完全一致。
    for (index, candidate) in result.iter_mut().enumerate() {
        let leg_count = candidate
            .soul
            .initial_substat_count
            .map(|count| usize::from(count))
            .unwrap_or_else(|| {
                candidate
                    .soul
                    .attributes
                    .iter()
                    .filter(|attribute| !attribute.fixed_attribute)
                    .count()
            });
        candidate.rank_explanation = format!(
            "把它放到当前方案的{}号位、其余五枚保持不变后重算：当前候选中排第{}，先比较先天副属性条数（{}腿优先），再比较期望加权缺口改善（{}），随后比较理论上限改善（{}）；最后用号位和御魂稳定ID打破并列。",
            candidate.slot,
            index + 1,
            leg_count,
            format!("{:.2}", candidate.expected_improvement),
            format!("{:.2}", candidate.possible_improvement),
        );
    }
    result.truncate(10);
    EnhancementCandidateBuild {
        candidates: result,
        truncated,
    }
}

/// 判断御魂是否进入强化建议扫描，集中维护号位、等级、星级和重复使用约束。
fn item_is_enhancement_candidate(
    item: &MiracleSoul,
    current_slot: u8,
    selected: &MiracleSoulView,
    solution: &MiracleSolution,
) -> bool {
    item.soul.slot == current_slot
        && item.soul.level < 15
        && item.soul.quality == 6
        // 当前最佳方案里的低等级御魂也必须作为“继续强化”候选返回。
        && (item.soul.soul_key() == selected.soul_key
            || !solution
                .souls
                .iter()
                .any(|soul| soul.soul_key == item.soul.soul_key()))
}

/// 计算理论上限使用的最有利强化路径，同时返回该路径上的御魂副属性和面板。
///
/// 三腿胚子会把“下一节点必出新腿”的所有可能属性分别计算，再选出最有利分支；
/// 四腿胚子则尝试把剩余节点全部投入每一条已有副属性。面板和副属性必须由同一条路径产生，
/// 这样前端才能可靠地把真正承担理论收益的属性高亮出来。
fn theoretical_projection(
    candidate: &MiracleSoul,
    selected: &[MiracleSoul],
    set_counts: &BTreeMap<String, u8>,
    set_map: &HashMap<String, SoulSet>,
    base_panel: &MiraclePanel,
    target: &MiracleTarget,
    metrics: &[MiracleMetric],
) -> Option<(MiracleSoul, MiraclePanel)> {
    // 候选一定来自当前方案，但仍保留显式查找，避免领域函数在异常输入下索引越界。
    let candidate_index = selected
        .iter()
        .position(|item| soul_key(item) == soul_key(candidate))?;
    let current_panel = calculate_panel(base_panel, selected, set_counts, set_map).0;
    let rolls = remaining_enhancement_nodes(candidate.soul.level);
    if rolls == 0 {
        return Some((selected[candidate_index].clone(), current_panel));
    }

    // 强化上限必须基于当前副属性继续成长，而不是把“高档成长 × 剩余节点”当成最终值。
    // 对攻击、生命、防御分别尝试固定值和百分比两条成长路径，取加权缺口最小者。
    let mut best_soul = selected[candidate_index].clone();
    let mut best_panel = current_panel.clone();
    let mut best_score = request_gap_score(&current_panel, target, metrics);
    if is_incomplete_enhancement_stage(candidate) {
        // 三腿时下一节点必然新增一腿；对每种可能的新腿分别取最有利的后续强化路径。
        for (new_attribute, _) in new_attribute_distribution(candidate, set_map) {
            let benchmark = benchmark_for(candidate, set_map, &new_attribute);
            if benchmark <= 0.0 {
                continue;
            }
            let mut projected_soul = candidate.clone();
            projected_soul.attributes.push(SoulAttribute {
                snapshot_id: projected_soul.soul.snapshot_id.clone(),
                soul_internal_id: projected_soul.soul.internal_id.clone(),
                attribute_index: 255,
                attribute_type: new_attribute,
                value: benchmark,
                // 新增腿的具体强化次数是理论投影，不覆盖真实御魂的未知事实。
                enhancement_count: None,
                count_provenance: "estimated".to_owned(),
                fixed_attribute: false,
            });
            let mut projected = selected.to_vec();
            projected[candidate_index] = projected_soul.clone();
            let mut branch_panel = calculate_panel(base_panel, &projected, set_counts, set_map).0;
            let mut branch_score = request_gap_score(&branch_panel, target, metrics);
            let remaining_rolls = rolls.saturating_sub(1);

            // 新增属性后的剩余节点仍按“全部投入一条最有利属性”计算，允许继续强化新增腿。
            if remaining_rolls > 0 {
                // 每个候选属性都从“只新增一腿”的同一基线试算，不能把前一个失败分支的成长带入下一个分支。
                let branch_soul = projected_soul.clone();
                for attribute in target_attribute_order_for_request(target, metrics) {
                    for roll_attribute in enhancement_attribute_types(&attribute) {
                        let benchmark = benchmark_for(candidate, set_map, roll_attribute);
                        if benchmark <= 0.0 {
                            continue;
                        }
                        let Some(rolled_soul) = project_existing_soul(
                            &branch_soul,
                            roll_attribute,
                            benchmark,
                            remaining_rolls,
                        ) else {
                            continue;
                        };
                        projected[candidate_index] = rolled_soul.clone();
                        let rolled_panel =
                            calculate_panel(base_panel, &projected, set_counts, set_map).0;
                        let score = request_gap_score(&rolled_panel, target, metrics);
                        if score + 1e-9 < branch_score {
                            branch_score = score;
                            branch_panel = rolled_panel;
                            projected_soul = rolled_soul;
                        }
                    }
                }
            }

            if branch_score + 1e-9 < best_score {
                best_score = branch_score;
                best_soul = projected_soul;
                best_panel = branch_panel;
            }
        }
    } else {
        for attribute in target_attribute_order_for_request(target, metrics) {
            for roll_attribute in enhancement_attribute_types(&attribute) {
                let benchmark = benchmark_for(candidate, set_map, roll_attribute);
                if benchmark <= 0.0 {
                    continue;
                }
                let Some(projected_soul) =
                    project_existing_soul(candidate, roll_attribute, benchmark, rolls)
                else {
                    continue;
                };
                let mut projected = selected.to_vec();
                projected[candidate_index] = projected_soul.clone();
                let projected_panel =
                    calculate_panel(base_panel, &projected, set_counts, set_map).0;
                let score = request_gap_score(&projected_panel, target, metrics);
                if score + 1e-9 < best_score {
                    best_score = score;
                    best_soul = projected_soul;
                    best_panel = projected_panel;
                }
            }
        }
    }
    Some((best_soul, best_panel))
}

/// 把剩余理论强化节点全部叠加到指定副属性；只用于理论投影，不修改库存事实。
fn project_existing_soul(
    candidate: &MiracleSoul,
    roll_attribute: &str,
    benchmark: f64,
    rolls: usize,
) -> Option<MiracleSoul> {
    let mut projected = candidate.clone();
    let attribute = projected.attributes.iter_mut().find(|attribute| {
        !attribute.fixed_attribute && attribute.attribute_type == roll_attribute
    })?;
    attribute.value += benchmark * rolls as f64;
    Some(projected)
}

/// 计算强化后的典型面板：三腿先按概率新增一条属性，再按均衡的整数轨迹分配剩余节点；
/// 四腿直接按均衡的整数轨迹分配剩余节点。成长值使用六星区间中位值，不使用高档上限。
fn expected_panel(
    candidate: &MiracleSoul,
    selected: &[MiracleSoul],
    set_counts: &BTreeMap<String, u8>,
    set_map: &HashMap<String, SoulSet>,
    base_panel: &MiraclePanel,
) -> MiraclePanel {
    let current_panel = calculate_panel(base_panel, selected, set_counts, set_map).0;
    let rolls = remaining_enhancement_nodes(candidate.soul.level);
    if rolls == 0 {
        return current_panel;
    }
    if is_incomplete_enhancement_stage(candidate) {
        let mut weighted_panel = MiraclePanel::default();
        let mut total_weight = 0.0;
        for (new_attribute, probability) in new_attribute_distribution(candidate, set_map) {
            let benchmark = benchmark_for(candidate, set_map, &new_attribute);
            if benchmark <= 0.0 {
                continue;
            }
            let Some(projected) = typical_new_attribute_rolls(
                candidate,
                selected,
                &new_attribute,
                benchmark,
                rolls,
                set_counts,
                set_map,
                base_panel,
            ) else {
                continue;
            };
            add_weighted_panel(&mut weighted_panel, &projected, probability);
            total_weight += probability;
        }
        if total_weight > 0.0 {
            divide_panel(&mut weighted_panel, total_weight);
            return weighted_panel;
        }
    }
    expected_existing_rolls(candidate, selected, rolls, set_counts, set_map, base_panel)
}

/// 生成强化卡片中的“典型强化后副属性”列表：手数保持整数，成长值使用六星区间中位值。
/// 三腿新增属性仍按概率列出候选，但不会把概率均摊成 1.25 手之类的展示值。
fn build_expected_attributes(
    candidate: &MiracleSoul,
    set_map: &HashMap<String, SoulSet>,
) -> Vec<MiracleExpectedAttributeView> {
    let rolls = remaining_enhancement_nodes(candidate.soul.level);
    let attributes = candidate
        .attributes
        .iter()
        .filter(|attribute| !attribute.fixed_attribute)
        .collect::<Vec<_>>();
    let incomplete = is_incomplete_enhancement_stage(candidate);
    let mut result = attributes
        .iter()
        .map(|attribute| MiracleExpectedAttributeView {
            attribute_index: attribute.attribute_index,
            attribute_type: attribute.attribute_type.clone(),
            current_value: attribute.value,
            expected_value: attribute.value,
            expected_added_value: 0.0,
            expected_enhancement_count: 0,
            fixed_attribute: false,
            is_new_attribute: false,
            probability: None,
        })
        .collect::<Vec<_>>();

    if incomplete && rolls > 0 {
        // 三腿的第一手必然新增属性；按每个新属性分支生成一个概率加权摘要，避免伪造确定属性。
        let distribution = new_attribute_distribution(candidate, set_map);
        let total_weight = distribution.iter().map(|(_, weight)| *weight).sum::<f64>();
        if total_weight > 0.0 {
            // 第一手新增后进入四腿均匀命中；剩余节点按典型整数轨迹分到四条副属性。
            let roll_counts = typical_roll_counts(attributes.len() + 1, rolls.saturating_sub(1));
            for (attribute_type, weight) in distribution {
                let benchmark = benchmark_for(candidate, set_map, &attribute_type);
                let probability = weight / total_weight;
                let new_roll_count = *roll_counts.last().unwrap_or(&0);
                let expected_value =
                    expected_roll_benchmark(benchmark) * f64::from(1 + new_roll_count);
                result.push(MiracleExpectedAttributeView {
                    attribute_index: 255,
                    attribute_type,
                    current_value: 0.0,
                    expected_value,
                    expected_added_value: expected_value,
                    expected_enhancement_count: 1 + new_roll_count,
                    fixed_attribute: false,
                    is_new_attribute: true,
                    probability: Some(probability),
                });
            }
            // 已有属性按同一条典型轨迹取整数手数；多出的节点按原始副属性顺序分配。
            for (index, item) in result
                .iter_mut()
                .filter(|item| !item.is_new_attribute)
                .enumerate()
            {
                let benchmark = benchmark_for(candidate, set_map, &item.attribute_type);
                let count = *roll_counts.get(index).unwrap_or(&0);
                let added_value = expected_roll_benchmark(benchmark) * f64::from(count);
                item.expected_added_value += added_value;
                item.expected_value += added_value;
                item.expected_enhancement_count = count;
            }
            return result;
        }
    }

    // 四腿胚子按已有副属性均衡分配剩余节点，手数和数值都保持离散可读。
    let roll_counts = typical_roll_counts(attributes.len().max(1), rolls);
    for (index, item) in result.iter_mut().enumerate() {
        let benchmark = benchmark_for(candidate, set_map, &item.attribute_type);
        let count = *roll_counts.get(index).unwrap_or(&0);
        item.expected_enhancement_count = count;
        item.expected_added_value = expected_roll_benchmark(benchmark) * f64::from(count);
        item.expected_value += item.expected_added_value;
    }
    result
}

/// 把理论最有利路径投影回副属性列表，供前端与“一般属性”并排展示。
///
/// 理论路径只改变可强化副属性；原始属性仍按库存顺序保留，三腿分支产生的新增腿使用 255 作为
/// 前端稳定的临时索引。强化手数按高档成长基准反推为整数，仅用于阅读，不覆盖真实强化次数。
fn build_theoretical_attributes(
    candidate: &MiracleSoul,
    projected: &MiracleSoul,
    set_map: &HashMap<String, SoulSet>,
) -> Vec<MiracleExpectedAttributeView> {
    let current_attributes = candidate
        .attributes
        .iter()
        .filter(|attribute| !attribute.fixed_attribute)
        .collect::<Vec<_>>();
    projected
        .attributes
        .iter()
        .filter(|attribute| !attribute.fixed_attribute)
        .map(|attribute| {
            let current = current_attributes.iter().find(|item| {
                item.attribute_index == attribute.attribute_index
                    && item.attribute_type == attribute.attribute_type
            });
            let current_value = current.map(|item| item.value).unwrap_or(0.0);
            let expected_added_value = (attribute.value - current_value).max(0.0);
            let benchmark = benchmark_for(candidate, set_map, &attribute.attribute_type);
            let expected_enhancement_count = if expected_added_value > 1e-9 && benchmark > 0.0 {
                (expected_added_value / benchmark).round() as u8
            } else {
                0
            };
            MiracleExpectedAttributeView {
                attribute_index: attribute.attribute_index,
                attribute_type: attribute.attribute_type.clone(),
                current_value,
                expected_value: attribute.value,
                expected_added_value,
                expected_enhancement_count,
                fixed_attribute: false,
                is_new_attribute: current.is_none(),
                probability: None,
            }
        })
        .collect()
}

/// 读取不足四腿时的新增属性分布；只保留当前尚未出现的属性，避免把新增腿错误地算成重复属性。
/// 返回值使用未归一化权重，调用方会在可用属性范围内重新归一化。
fn new_attribute_distribution(
    candidate: &MiracleSoul,
    set_map: &HashMap<String, SoulSet>,
) -> Vec<(String, f64)> {
    let existing = candidate
        .attributes
        .iter()
        .filter(|attribute| !attribute.fixed_attribute)
        .map(|attribute| attribute.attribute_type.as_str())
        .collect::<BTreeSet<_>>();
    let mut distribution = Vec::new();
    if let Some(categories) = set_map
        .get(&candidate.soul.set_id)
        .and_then(|set| set.enhancement_rule_json.as_deref())
        .and_then(|json| serde_json::from_str::<Value>(json).ok())
        .and_then(|rule| rule.get("incompleteMode").cloned())
        .and_then(|mode| mode.get("categories").cloned())
        .and_then(|categories| categories.as_array().cloned())
    {
        for category in categories {
            let probability = category
                .get("probability")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let attributes = category
                .get("attributes")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|attribute| !existing.contains(*attribute))
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if probability <= 0.0 || attributes.is_empty() {
                continue;
            }
            let per_attribute_weight = probability / attributes.len() as f64;
            distribution.extend(
                attributes
                    .into_iter()
                    .map(|attribute| (attribute, per_attribute_weight)),
            );
        }
    }
    if distribution.is_empty() {
        // 目录缺失时沿用模拟强化模块的类别概率；这只是期望值兜底，不伪造具体属性概率来源。
        let fallback_categories = [
            (
                0.36,
                vec!["attack_flat", "attack_rate", "crit_rate", "crit_damage"],
            ),
            (
                0.36,
                vec!["hp_flat", "defense_flat", "hp_rate", "defense_rate"],
            ),
            (0.28, vec!["effect_resist", "effect_hit", "speed"]),
        ];
        for (probability, attributes) in fallback_categories {
            let available = attributes
                .into_iter()
                .filter(|attribute| !existing.contains(*attribute))
                .collect::<Vec<_>>();
            if available.is_empty() {
                continue;
            }
            let per_attribute_weight = probability / available.len() as f64;
            distribution.extend(
                available
                    .into_iter()
                    .map(|attribute| (attribute.to_owned(), per_attribute_weight)),
            );
        }
    }
    distribution
}

/// 以六星成长区间的中位值作为典型成长；区间为高档值的 80%—100%，所以速度高档 3 点对应 2.7 点。
fn expected_roll_benchmark(benchmark: f64) -> f64 {
    benchmark * 0.9
}

/// 将剩余强化节点分配成一条可读的整数轨迹；多出的节点从原始副属性顺序开始分配。
/// 例如四条腿、五次强化会得到 [2, 1, 1, 1]，不展示概率平均值 1.25 手。
fn typical_roll_counts(attribute_count: usize, rolls: usize) -> Vec<u8> {
    if attribute_count == 0 {
        return Vec::new();
    }
    let base = rolls / attribute_count;
    let remainder = rolls % attribute_count;
    (0..attribute_count)
        .map(|index| (base + if index < remainder { 1 } else { 0 }) as u8)
        .collect()
}

/// 计算三腿新增属性后的典型面板；新增腿使用一次中位成长，剩余节点使用整数强化轨迹。
fn typical_new_attribute_rolls(
    candidate: &MiracleSoul,
    selected: &[MiracleSoul],
    new_attribute: &str,
    new_attribute_benchmark: f64,
    rolls: usize,
    set_counts: &BTreeMap<String, u8>,
    set_map: &HashMap<String, SoulSet>,
    base_panel: &MiraclePanel,
) -> Option<MiraclePanel> {
    let mut projected = selected.to_vec();
    let index = projected
        .iter()
        .position(|item| soul_key(item) == soul_key(candidate))?;
    let mut clone = candidate.clone();
    clone.attributes.push(SoulAttribute {
        snapshot_id: clone.soul.snapshot_id.clone(),
        soul_internal_id: clone.soul.internal_id.clone(),
        attribute_index: 255,
        attribute_type: new_attribute.to_owned(),
        value: expected_roll_benchmark(new_attribute_benchmark),
        enhancement_count: None,
        count_provenance: "estimated".to_owned(),
        fixed_attribute: false,
    });
    let remaining_rolls = rolls.saturating_sub(1);
    // 新腿第一次出现时先吃一手中位成长，后续节点按四条腿得到整数命中次数。
    let leg_count = clone
        .attributes
        .iter()
        .filter(|attribute| !attribute.fixed_attribute)
        .count();
    let roll_counts = typical_roll_counts(leg_count, remaining_rolls);
    for (index, attribute) in clone
        .attributes
        .iter_mut()
        .filter(|attribute| !attribute.fixed_attribute)
        .enumerate()
    {
        let initial_roll = if index + 1 == leg_count { 1 } else { 0 };
        let count = initial_roll + usize::from(*roll_counts.get(index).unwrap_or(&0));
        attribute.value +=
            expected_roll_benchmark(benchmark_for(candidate, set_map, &attribute.attribute_type))
                * count as f64;
    }
    projected[index] = clone;
    Some(calculate_panel(base_panel, &projected, set_counts, set_map).0)
}

/// 四腿胚子按典型整数轨迹命中已有副属性，不参与随机模拟。
fn expected_existing_rolls(
    candidate: &MiracleSoul,
    selected: &[MiracleSoul],
    rolls: usize,
    set_counts: &BTreeMap<String, u8>,
    set_map: &HashMap<String, SoulSet>,
    base_panel: &MiraclePanel,
) -> MiraclePanel {
    if rolls == 0 {
        return calculate_panel(base_panel, selected, set_counts, set_map).0;
    }
    let mut projected = selected.to_vec();
    let Some(index) = projected
        .iter()
        .position(|item| soul_key(item) == soul_key(candidate))
    else {
        return calculate_panel(base_panel, selected, set_counts, set_map).0;
    };
    let mut clone = candidate.clone();
    let leg_count = clone
        .attributes
        .iter()
        .filter(|attribute| !attribute.fixed_attribute)
        .count();
    if leg_count == 0 {
        return calculate_panel(base_panel, selected, set_counts, set_map).0;
    }
    let roll_counts = typical_roll_counts(leg_count, rolls);
    for (index, attribute) in clone
        .attributes
        .iter_mut()
        .filter(|attribute| !attribute.fixed_attribute)
        .enumerate()
    {
        let count = *roll_counts.get(index).unwrap_or(&0);
        attribute.value +=
            expected_roll_benchmark(benchmark_for(candidate, set_map, &attribute.attribute_type))
                * f64::from(count);
    }
    projected[index] = clone;
    calculate_panel(base_panel, &projected, set_counts, set_map).0
}

/// 在面板上累加一个带权分支，用于合成三腿胚子的期望面板。
fn add_weighted_panel(target: &mut MiraclePanel, source: &MiraclePanel, weight: f64) {
    target.attack += source.attack * weight;
    target.hp += source.hp * weight;
    target.defense += source.defense * weight;
    target.speed += source.speed * weight;
    target.crit_rate += source.crit_rate * weight;
    target.crit_damage += source.crit_damage * weight;
    target.effect_hit += source.effect_hit * weight;
    target.effect_resist += source.effect_resist * weight;
}

/// 将带权累加后的面板除以总权重，得到概率归一化的期望值。
fn divide_panel(panel: &mut MiraclePanel, total_weight: f64) {
    panel.attack /= total_weight;
    panel.hp /= total_weight;
    panel.defense /= total_weight;
    panel.speed /= total_weight;
    panel.crit_rate /= total_weight;
    panel.crit_damage /= total_weight;
    panel.effect_hit /= total_weight;
    panel.effect_resist /= total_weight;
}

/// 读取当前等级之后还会触发的强化节点；节点定义与模拟强化共用，避免 +0/+3 计算漂移。
fn remaining_enhancement_nodes(level: u8) -> usize {
    ENHANCEMENT_CHECKPOINTS
        .iter()
        .filter(|checkpoint| **checkpoint > level)
        .count()
}

/// 将面板目标映射为可由御魂强化直接成长的一条或两条规范属性路径。
fn enhancement_attribute_types(attribute: &str) -> &'static [&'static str] {
    match attribute {
        "attack" => &["attack_flat", "attack_rate"],
        "hp" => &["hp_flat", "hp_rate"],
        "defense" => &["defense_flat", "defense_rate"],
        "speed" => &["speed"],
        "crit_rate" => &["crit_rate"],
        "crit_damage" => &["crit_damage"],
        "effect_hit" => &["effect_hit"],
        "effect_resist" => &["effect_resist"],
        _ => &[],
    }
}

/// 按模拟强化的“命中已有腿继续成长”规则投影理论上限。
///
/// 这里故意不模拟新增副属性：新增一条腿需要先命中低概率事件，不能作为当前强化收益承诺。
fn project_candidate_rolls(
    candidate: &MiracleSoul,
    selected: &[MiracleSoul],
    roll_attribute: &str,
    benchmark: f64,
    rolls: usize,
    set_counts: &BTreeMap<String, u8>,
    set_map: &HashMap<String, SoulSet>,
    base_panel: &MiraclePanel,
) -> Option<MiraclePanel> {
    let mut projected = selected.to_vec();
    let index = projected
        .iter()
        .position(|item| soul_key(item) == soul_key(candidate))?;
    let mut clone = candidate.clone();
    if let Some(attribute) = clone
        .attributes
        .iter_mut()
        .find(|attribute| !attribute.fixed_attribute && attribute.attribute_type == roll_attribute)
    {
        // 初始副属性值必须保留；这里仅叠加未来节点的理论最高成长。
        attribute.value += benchmark * rolls as f64;
    } else {
        // 缺少该已有副属性时，不把新增腿视为可靠收益，交给购买建议提示更合适的胚子方向。
        return None;
    }
    projected[index] = clone;
    Some(calculate_panel(base_panel, &projected, set_counts, set_map).0)
}

/// 从套装强化规则读取高档成长基准，目录缺失时使用保守的六星默认上界。
fn benchmark_for(
    candidate: &MiracleSoul,
    set_map: &HashMap<String, SoulSet>,
    attribute: &str,
) -> f64 {
    if let Some(rule) = set_map
        .get(&candidate.soul.set_id)
        .and_then(|set| set.enhancement_rule_json.as_deref())
        .and_then(|json| serde_json::from_str::<Value>(json).ok())
    {
        if let Some(value) = rule
            .get("rollBenchmarks")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("attributeId").and_then(Value::as_str) == Some(attribute))
            })
            .and_then(|item| item.get("highValue"))
            .and_then(Value::as_f64)
        {
            return value;
        }
    }
    match attribute {
        "speed" => 3.0,
        "crit_rate" | "attack_rate" | "hp_rate" | "defense_rate" => 0.03,
        "crit_damage" | "effect_hit" | "effect_resist" => 0.04,
        // 兜底值与模拟强化领域的六星高档成长基准保持一致。
        "attack_flat" => 27.0,
        "hp_flat" => 114.0,
        "defense_flat" => 5.0,
        _ => 0.0,
    }
}

/// 根据目标缺口生成购买方向；副属性建议采用可执行的方向而非虚假的精确数值。
fn build_purchase_suggestions(
    solution: &MiracleSolution,
    request: &MiracleConchRequest,
    set_map: &HashMap<String, SoulSet>,
) -> Vec<MiraclePurchaseSuggestion> {
    let mut suggestions = Vec::new();
    // 组合指标没有单项阈值；只有公式下限或属性下限未满足时才给出补强建议，避免上限超出时继续推荐堆叠该属性。
    for (metric, metric_value) in request.metrics.iter().zip(solution.metrics.iter()) {
        let needs_more_attribute = metric_value.gap > 1e-9
            || metric_value
                .constraints
                .iter()
                .zip(metric.constraints.iter())
                .any(|(result, constraint)| result.actual + 1e-9 < constraint.minimum);
        if needs_more_attribute {
            for attribute in &metric.operands {
                let (slot, main_attribute, subs) = purchase_template(attribute);
                suggestions.push(MiraclePurchaseSuggestion {
                    slot,
                    set_direction: request
                        .recipe
                        .iter()
                        .filter(|line| line.set_id != SCATTERED_SET_ID && line.set_id != "散件")
                        .max_by_key(|line| line.count)
                        .and_then(|line| set_map.get(&line.set_id))
                        .map(|set| set.name.clone())
                        .unwrap_or_else(|| "配方中的任意套装".to_owned()),
                    main_attribute: main_attribute.to_owned(),
                    priority_sub_attributes: subs.iter().map(|item| (*item).to_owned()).collect(),
                    missing_attribute: format!(
                        "{}（参与{}）",
                        attribute_label(attribute),
                        metric.label
                    ),
                    priority_score: metric_value.gap * attribute_weight(attribute),
                    reason: format!(
                        "组合指标 {} 当前为 {}，目标为 {}；优先补强{}相关属性。",
                        metric_value.expression,
                        metric_value.actual,
                        metric_value.threshold,
                        attribute_label(attribute),
                    ),
                });
            }
        }
    }
    for attribute in target_attribute_order_for_request(&request.target, &request.metrics) {
        let score = gap_for_attribute(&solution.panel, &request.target, &attribute);
        if score <= 0.0 {
            continue;
        }
        let (slot, main_attribute, subs) = purchase_template(&attribute);
        let set_direction = request
            .recipe
            .iter()
            .filter(|line| line.set_id != SCATTERED_SET_ID && line.set_id != "散件")
            .max_by_key(|line| line.count)
            .and_then(|line| set_map.get(&line.set_id))
            .map(|set| set.name.clone())
            .unwrap_or_else(|| "配方中的任意套装".to_owned());
        suggestions.push(MiraclePurchaseSuggestion {
            slot,
            set_direction,
            main_attribute: main_attribute.to_owned(),
            priority_sub_attributes: subs.iter().map(|item| (*item).to_owned()).collect(),
            missing_attribute: attribute_label(&attribute).to_owned(),
            priority_score: score * attribute_weight(&attribute),
            reason: format!(
                "当前组合仍缺少{}，优先寻找该号位的主属性和相关副属性。",
                attribute_label(&attribute)
            ),
        });
    }
    suggestions.sort_by(|left, right| {
        right
            .priority_score
            .partial_cmp(&left.priority_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.slot.cmp(&right.slot))
    });
    suggestions.truncate(6);
    suggestions
}

fn build_purchase_suggestions_without_solution(
    request: &MiracleConchRequest,
    set_map: &HashMap<String, SoulSet>,
) -> Vec<MiraclePurchaseSuggestion> {
    let mut result = Vec::new();
    for attribute in target_attribute_order_for_request(&request.target, &request.metrics)
        .into_iter()
        .take(6)
    {
        let (slot, main_attribute, subs) = purchase_template(&attribute);
        let set_direction = request
            .recipe
            .iter()
            .find(|line| line.set_id != SCATTERED_SET_ID && line.set_id != "散件")
            .and_then(|line| set_map.get(&line.set_id))
            .map(|set| set.name.clone())
            .unwrap_or_else(|| "配方中的任意套装".to_owned());
        result.push(MiraclePurchaseSuggestion {
            slot,
            set_direction,
            main_attribute: main_attribute.to_owned(),
            priority_sub_attributes: subs.iter().map(|item| (*item).to_owned()).collect(),
            missing_attribute: attribute_label(&attribute).to_owned(),
            priority_score: attribute_weight(&attribute),
            reason: format!(
                "当前没有满足配方的六件组合，先从{}号位补齐{}方向。",
                slot,
                attribute_label(&attribute)
            ),
        });
    }
    result
}

/// 判断替换后的六件组合是否仍满足具体套装、分类通配和不限制配方。
fn selection_matches_recipe(
    selected: &[MiracleSoul],
    request: &MiracleConchRequest,
    set_map: &HashMap<String, SoulSet>,
) -> bool {
    fn assign_recipe_line(
        index: usize,
        selected: &[MiracleSoul],
        remaining: &mut [MiracleRecipeLine],
        set_map: &HashMap<String, SoulSet>,
    ) -> bool {
        if index == selected.len() {
            return remaining.iter().all(|line| line.count == 0);
        }
        let matching_lines = remaining
            .iter()
            .enumerate()
            .filter(|(_, line)| {
                line.count > 0 && recipe_line_matches_soul(line, &selected[index], set_map)
            })
            .map(|(line_index, _)| line_index)
            .collect::<Vec<_>>();
        for line_index in matching_lines {
            remaining[line_index].count -= 1;
            if assign_recipe_line(index + 1, selected, remaining, set_map) {
                return true;
            }
            remaining[line_index].count += 1;
        }
        false
    }

    let mut remaining = request.recipe.clone();
    assign_recipe_line(0, selected, &mut remaining, set_map)
}

fn purchase_template(attribute: &str) -> (u8, &'static str, &'static [&'static str]) {
    match attribute {
        "attack" => (4, "攻击加成", &["攻击加成", "暴击", "暴伤"]),
        "hp" => (4, "生命加成", &["生命加成", "速度", "效果抵抗"]),
        "defense" => (4, "防御加成", &["防御加成", "速度", "效果抵抗"]),
        "speed" => (2, "速度", &["速度", "暴击", "攻击加成"]),
        "crit_rate" => (6, "暴击", &["暴击", "速度", "攻击加成"]),
        "crit_damage" => (6, "暴伤", &["暴击", "速度", "攻击加成"]),
        "effect_hit" => (4, "效果命中", &["效果命中", "速度", "生命加成"]),
        "effect_resist" => (4, "效果抵抗", &["效果抵抗", "速度", "生命加成"]),
        _ => (2, "不限", &["速度"]),
    }
}

fn target_attribute_order(target: &MiracleTarget) -> Vec<String> {
    [
        ("attack", target.attack),
        ("hp", target.hp),
        ("defense", target.defense),
        ("speed", target.speed),
        ("crit_rate", target.crit_rate),
        ("crit_damage", target.crit_damage),
        ("effect_hit", target.effect_hit),
        ("effect_resist", target.effect_resist),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|_| name.to_owned()))
    .collect()
}

/// 合并普通面板目标和组合指标操作数，供候选相关度、剪枝和强化上限共同使用。
fn target_attribute_order_for_request(
    target: &MiracleTarget,
    metrics: &[MiracleMetric],
) -> Vec<String> {
    let mut attributes = target_attribute_order(target);
    for metric in metrics {
        for operand in &metric.operands {
            if is_panel_attribute(operand) && !attributes.iter().any(|item| item == operand) {
                attributes.push(operand.clone());
            }
        }
        for constraint in &metric.constraints {
            if is_panel_attribute(&constraint.attribute)
                && !attributes.iter().any(|item| item == &constraint.attribute)
            {
                attributes.push(constraint.attribute.clone());
            }
        }
    }
    attributes
}

/// 只允许使用当前静态面板中的八个字段，避免组合公式读取未计算的战斗状态。
fn is_panel_attribute(attribute: &str) -> bool {
    matches!(
        attribute,
        "attack"
            | "hp"
            | "defense"
            | "speed"
            | "crit_rate"
            | "crit_damage"
            | "effect_hit"
            | "effect_resist"
    )
}

/// 校验组合指标，确保后端不会执行任意表达式，只解释固定的面板字段和乘法运算。
fn validate_metric(metric: &MiracleMetric) -> Result<(), String> {
    if !metric.threshold.is_finite() || metric.threshold <= 0.0 {
        return Err("组合指标目标必须是大于 0 的数字".to_owned());
    }
    if metric.label.trim().is_empty() {
        return Err("组合指标名称不能为空".to_owned());
    }
    let expected_count = match metric.metric_type.as_str() {
        "product" => 2..=4,
        "minimum" => 1..=4,
        _ => return Err("组合指标类型只支持 product 或 minimum".to_owned()),
    };
    if !expected_count.contains(&metric.operands.len()) {
        return Err(match metric.metric_type.as_str() {
            "product" => "乘积指标需要选择 2 到 4 个面板属性".to_owned(),
            _ => "最低值指标需要选择 1 到 4 个面板属性".to_owned(),
        });
    }
    let mut seen = BTreeSet::new();
    for operand in &metric.operands {
        if !is_panel_attribute(operand) {
            return Err(format!("组合指标包含未知面板属性：{operand}"));
        }
        if !seen.insert(operand) {
            return Err("组合指标不能重复选择同一面板属性".to_owned());
        }
    }
    for constraint in &metric.constraints {
        if !is_panel_attribute(&constraint.attribute)
            || !constraint.minimum.is_finite()
            || constraint.minimum < 0.0
            || constraint
                .maximum
                .is_some_and(|maximum| !maximum.is_finite() || maximum < constraint.minimum)
        {
            return Err("组合指标的属性限制无效".to_owned());
        }
    }
    Ok(())
}

/// 校验 2/4/6 号位主属性限制；同一号位允许多个不同主属性，但拒绝重复的同一约束。
fn validate_main_attributes(constraints: &[MiracleMainAttributeConstraint]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for constraint in constraints {
        if !matches!(constraint.slot, 2 | 4 | 6) {
            return Err("主属性限制只支持 2、4、6 号位".to_owned());
        }
        if !valid_main_attribute(constraint.slot, &constraint.main_attr_type) {
            return Err(format!(
                "{}号位不支持主属性 {}",
                constraint.slot, constraint.main_attr_type
            ));
        }
        let key = (constraint.slot, constraint.main_attr_type.clone());
        if !seen.insert(key) {
            return Err(format!(
                "{}号位主属性 {} 重复指定",
                constraint.slot, constraint.main_attr_type
            ));
        }
    }
    Ok(())
}

/// 识别当前游戏目录允许的二四六号位主属性。
fn valid_main_attribute(slot: u8, main_attr_type: &str) -> bool {
    match slot {
        2 => matches!(
            main_attr_type,
            "attack_rate" | "hp_rate" | "defense_rate" | "speed"
        ),
        4 => matches!(
            main_attr_type,
            "attack_rate" | "hp_rate" | "defense_rate" | "effect_hit" | "effect_resist"
        ),
        6 => matches!(
            main_attr_type,
            "attack_rate" | "hp_rate" | "defense_rate" | "crit_rate" | "crit_damage"
        ),
        _ => false,
    }
}

/// 读取面板字段；百分比属性保持内部小数，组合指标结果因此与后端面板口径一致。
fn panel_attribute_value(panel: &MiraclePanel, attribute: &str) -> Option<f64> {
    match attribute {
        "attack" => Some(panel.attack),
        "hp" => Some(panel.hp),
        "defense" => Some(panel.defense),
        "speed" => Some(panel.speed),
        "crit_rate" => Some(panel.crit_rate),
        "crit_damage" => Some(panel.crit_damage),
        "effect_hit" => Some(panel.effect_hit),
        "effect_resist" => Some(panel.effect_resist),
        _ => None,
    }
}

/// 计算固定组合指标；首版不接受脚本或任意运算符，保证结果可解释且可稳定排序。
fn metric_result(panel: &MiraclePanel, metric: &MiracleMetric) -> MiracleMetricResult {
    let values = metric
        .operands
        .iter()
        .filter_map(|operand| panel_attribute_value(panel, operand))
        .collect::<Vec<_>>();
    let actual = match metric.metric_type.as_str() {
        "minimum" => values.iter().copied().fold(f64::INFINITY, f64::min),
        _ => values.iter().copied().product(),
    };
    let gap = (metric.threshold - actual).max(0.0);
    let constraints = metric
        .constraints
        .iter()
        .map(|constraint| {
            let actual = panel_attribute_value(panel, &constraint.attribute).unwrap_or(0.0);
            let lower_gap = (constraint.minimum - actual).max(0.0);
            let upper_gap = constraint
                .maximum
                .map(|maximum| (actual - maximum).max(0.0))
                .unwrap_or(0.0);
            MiracleMetricConstraintResult {
                attribute: constraint.attribute.clone(),
                actual,
                minimum: constraint.minimum,
                maximum: constraint.maximum,
                gap: lower_gap.max(upper_gap),
                met: lower_gap <= 1e-9 && upper_gap <= 1e-9,
            }
        })
        .collect::<Vec<_>>();
    MiracleMetricResult {
        preset_id: metric.preset_id.clone(),
        label: metric.label.clone(),
        expression: metric
            .operands
            .iter()
            .map(|operand| attribute_label(operand))
            .collect::<Vec<_>>()
            .join(if metric.metric_type == "product" {
                " × "
            } else {
                " 取最低 "
            }),
        actual,
        threshold: metric.threshold,
        gap,
        met: gap <= 1e-9 && constraints.iter().all(|constraint| constraint.met),
        percentage: metric.percentage,
        constraints,
    }
}

/// 统一普通面板和组合指标的缺口评分；组合指标模式下仍允许面板目标同时作为附加约束。
fn request_gap_score(
    panel: &MiraclePanel,
    target: &MiracleTarget,
    metrics: &[MiracleMetric],
) -> f64 {
    weighted_gap(&panel_gap(panel, target))
        + metrics
            .iter()
            .map(|metric| {
                let result = metric_result(panel, metric);
                result.gap
                    + result
                        .constraints
                        .iter()
                        .map(|constraint| constraint.gap)
                        .sum::<f64>()
            })
            .sum::<f64>()
}

fn gap_for_attribute(panel: &MiraclePanel, target: &MiracleTarget, attribute: &str) -> f64 {
    match attribute {
        "attack" => gap(panel.attack, target.attack),
        "hp" => gap(panel.hp, target.hp),
        "defense" => gap(panel.defense, target.defense),
        "speed" => gap(panel.speed, target.speed),
        "crit_rate" => gap(panel.crit_rate, target.crit_rate),
        "crit_damage" => gap(panel.crit_damage, target.crit_damage),
        "effect_hit" => gap(panel.effect_hit, target.effect_hit),
        "effect_resist" => gap(panel.effect_resist, target.effect_resist),
        _ => 0.0,
    }
}

fn attribute_label(attribute: &str) -> &'static str {
    match attribute {
        "attack" => "攻击",
        "hp" => "生命",
        "defense" => "防御",
        "speed" => "速度",
        "crit_rate" => "暴击",
        "crit_damage" => "暴伤",
        "effect_hit" => "效果命中",
        "effect_resist" => "效果抵抗",
        _ => "目标属性",
    }
}

fn attribute_weight(attribute: &str) -> f64 {
    match attribute {
        "speed" => SPEED_WEIGHT,
        "crit_rate" => CRIT_RATE_WEIGHT,
        "crit_damage" => CRIT_DAMAGE_WEIGHT,
        "effect_hit" | "effect_resist" => HIT_RESIST_WEIGHT,
        _ => BASIC_STAT_WEIGHT,
    }
}

fn candidate_relevance(
    soul: &MiracleSoul,
    target: &MiracleTarget,
    metrics: &[MiracleMetric],
    _set_map: &HashMap<String, SoulSet>,
) -> f64 {
    let mut score = 0.0;
    for attribute in target_attribute_order_for_request(target, metrics) {
        let (slot, main, _) = purchase_template(&attribute);
        let _ = slot;
        let value = soul
            .attributes
            .iter()
            .filter(|item| purchase_attribute_matches(&attribute, &item.attribute_type))
            .map(|item| item.value)
            .sum::<f64>()
            + if purchase_attribute_matches(&attribute, &soul.soul.main_attr_type) {
                soul.soul.main_attr_value
            } else {
                0.0
            };
        score += value * attribute_weight(&attribute);
        let _ = main;
    }
    score
}

fn purchase_attribute_matches(target: &str, actual: &str) -> bool {
    match target {
        "attack" => matches!(actual, "attack_flat" | "attack_rate"),
        "hp" => matches!(actual, "hp_flat" | "hp_rate"),
        "defense" => matches!(actual, "defense_flat" | "defense_rate"),
        _ => actual == target,
    }
}

fn has_target(target: &MiracleTarget, metrics: &[MiracleMetric]) -> bool {
    !target_attribute_order(target).is_empty() || !metrics.is_empty()
}

fn soul_key(item: &MiracleSoul) -> String {
    item.soul_key()
}

fn soul_view(item: &MiracleSoul) -> MiracleSoulView {
    MiracleSoulView {
        soul_key: item.soul_key(),
        set_id: item.soul.set_id.clone(),
        slot: item.soul.slot,
        quality: item.soul.quality,
        level: item.soul.level,
        main_attr_type: item.soul.main_attr_type.clone(),
        main_attr_value: item.soul.main_attr_value,
        equipped_state: item.soul.equipped_state.clone(),
        initial_substat_count: item.soul.initial_substat_count,
        attributes: item
            .attributes
            .iter()
            .map(|attribute| MiracleAttributeView {
                attribute_index: attribute.attribute_index,
                attribute_type: attribute.attribute_type.clone(),
                value: attribute.value,
                enhancement_count: attribute.enhancement_count,
                count_provenance: attribute.count_provenance.clone(),
                fixed_attribute: attribute.fixed_attribute,
            })
            .collect(),
    }
}

fn find_inventory_soul(view: &MiracleSoulView, inventory: &[MiracleSoul]) -> Option<MiracleSoul> {
    inventory
        .iter()
        .find(|item| item.soul_key() == view.soul_key)
        .cloned()
}

/// 生成只包含套装顺序的稳定键；强化建议中的配方匹配不依赖副属性数值。
fn selection_set_key(selected: &[MiracleSoul]) -> String {
    selected
        .iter()
        .map(|item| item.soul.set_id.as_str())
        .collect::<Vec<_>>()
        .join("\u{0}")
}

impl MiracleSoul {
    fn soul_key(&self) -> String {
        self.soul
            .source_stable_id
            .clone()
            .unwrap_or_else(|| self.soul.internal_id.clone())
    }
}

impl SnapshotSoul {
    fn soul_key(&self) -> String {
        self.source_stable_id
            .clone()
            .unwrap_or_else(|| self.internal_id.clone())
    }
}

impl PartialEq for MiraclePanelGap {
    fn eq(&self, other: &Self) -> bool {
        (self.attack - other.attack).abs() < 1e-9
            && (self.hp - other.hp).abs() < 1e-9
            && (self.defense - other.defense).abs() < 1e-9
            && (self.speed - other.speed).abs() < 1e-9
            && (self.crit_rate - other.crit_rate).abs() < 1e-9
            && (self.crit_damage - other.crit_damage).abs() < 1e-9
            && (self.effect_hit - other.effect_hit).abs() < 1e-9
            && (self.effect_resist - other.effect_resist).abs() < 1e-9
    }
}

impl Eq for MiraclePanelGap {}

#[cfg(test)]
mod tests {
    use super::*;

    fn shikigami() -> Shikigami {
        Shikigami {
            catalog_version: "test".to_owned(),
            shikigami_id: "389".to_owned(),
            name: "须佐之男".to_owned(),
            rarity: "SSR".to_owned(),
            icon_asset_id: None,
            sort_order: 1,
            panel_level: 40,
            awakened: true,
            panel_json: serde_json::json!({
                "attack": 100.0, "hp": 1000.0, "defense": 100.0, "speed": 100.0,
                "critRate": 0.1, "critDamage": 0.5
            })
            .to_string(),
            skills_json: "[]".to_owned(),
            skill_investment_json: "{}".to_owned(),
            employment_scenes_json: "[]".to_owned(),
            game_version: "test".to_owned(),
            source_refs_json: "[]".to_owned(),
        }
    }

    fn set(id: &str, category: Option<&str>) -> SoulSet {
        SoulSet {
            catalog_version: "test".to_owned(),
            set_id: id.to_owned(),
            name: id.to_owned(),
            icon_asset_id: None,
            category: category.map(str::to_owned),
            special_category: None,
            rarity_scope: None,
            two_piece_effect_json: Some("面板效果".to_owned()),
            four_piece_effect_json: Some("战斗效果".to_owned()),
            enhancement_rule_json: None,
            source_refs_json: None,
            effective_from: None,
            effective_to: None,
        }
    }

    fn soul(slot: u8, set_id: &str, attribute_type: &str, value: f64) -> MiracleSoul {
        MiracleSoul {
            soul: SnapshotSoul {
                snapshot_id: "snap".to_owned(),
                internal_id: format!("soul-{slot}-{set_id}"),
                source_stable_id: Some(format!("source-{slot}-{set_id}")),
                identity_quality: "stable".to_owned(),
                set_id: set_id.to_owned(),
                slot,
                quality: 6,
                level: 15,
                main_attr_type: "attack_flat".to_owned(),
                main_attr_value: 0.0,
                initial_substat_count: Some(4),
                locked_in_source: None,
                equipped_state: None,
                source_json: None,
            },
            attributes: vec![SoulAttribute {
                snapshot_id: "snap".to_owned(),
                soul_internal_id: format!("soul-{slot}-{set_id}"),
                attribute_index: 0,
                attribute_type: attribute_type.to_owned(),
                value,
                enhancement_count: Some(1),
                count_provenance: "source".to_owned(),
                fixed_attribute: false,
            }],
        }
    }

    #[test]
    fn calculates_base_panel_and_two_piece_bonus() {
        let sets = vec![set("攻击套", Some("攻击加成"))];
        let inventory = (1..=6)
            .map(|slot| soul(slot, "攻击套", "attack_rate", 0.1))
            .collect::<Vec<_>>();
        let result = calculate_miracle_conch(
            &MiracleConchRequest {
                shikigami_id: "389".to_owned(),
                target: MiracleTarget {
                    attack: Some(170.0),
                    ..Default::default()
                },
                only_max_level: true,
                metrics: Vec::new(),
                main_attributes: Vec::new(),
                recipe: vec![MiracleRecipeLine {
                    set_id: "攻击套".to_owned(),
                    count: 6,
                    category: None,
                }],
            },
            &shikigami(),
            &sets,
            &inventory,
            0,
            0,
        )
        .expect("测试输入合法");
        assert_eq!(result.status, "matched");
        // 已达标方案不再附带购买和强化方向，避免把已经够用的配置继续推向无意义的优化。
        assert!(result.purchase_suggestions.is_empty());
        assert!(result.enhancement_candidates.is_empty());
        // 基础攻击 100 × (1 + 六件各 10% + 二件套 15%) = 175。
        assert!((result.solutions[0].panel.attack - 175.0).abs() < 1e-9);
        assert_eq!(result.solutions[0].counted_set_effects.len(), 1);
        assert_eq!(result.solutions[0].uncounted_set_effects.len(), 1);
    }

    #[test]
    fn fixed_attack_is_added_after_attack_rate() {
        let base = MiraclePanel {
            attack: 100.0,
            hp: 100.0,
            defense: 100.0,
            speed: 100.0,
            crit_rate: 0.1,
            crit_damage: 1.5,
            effect_hit: 0.0,
            effect_resist: 0.0,
        };
        let selected = vec![
            soul(1, "散件", "attack_flat", 100.0),
            soul(2, "散件", "attack_rate", 0.5),
        ];

        // 白字攻击不参与百分比加成：100 × (1 + 50%) + 100 = 250，而不是 300。
        let (panel, _, _) = calculate_panel(&base, &selected, &BTreeMap::new(), &HashMap::new());
        assert!((panel.attack - 250.0).abs() < 1e-9);
    }

    #[test]
    fn supports_product_metric_without_single_panel_minimums() {
        let sets = vec![set("攻击套", Some("攻击加成"))];
        let inventory = (1..=6)
            .map(|slot| soul(slot, "攻击套", "attack_rate", 0.1))
            .collect::<Vec<_>>();
        let result = calculate_miracle_conch(
            &MiracleConchRequest {
                shikigami_id: "389".to_owned(),
                target: MiracleTarget::default(),
                only_max_level: true,
                metrics: vec![MiracleMetric {
                    preset_id: "damage_output".to_owned(),
                    metric_type: "product".to_owned(),
                    label: "最终输出".to_owned(),
                    operands: vec!["attack".to_owned(), "crit_damage".to_owned()],
                    threshold: 260.0,
                    percentage: false,
                    constraints: Vec::new(),
                }],
                main_attributes: Vec::new(),
                recipe: vec![MiracleRecipeLine {
                    set_id: "攻击套".to_owned(),
                    count: 6,
                    category: None,
                }],
            },
            &shikigami(),
            &sets,
            &inventory,
            0,
            0,
        )
        .expect("组合指标请求应合法");
        assert_eq!(result.status, "matched");
        let metric = result.solutions[0]
            .metric
            .as_ref()
            .expect("应返回组合指标结果");
        assert_eq!(metric.expression, "攻击 × 暴伤");
        assert!((metric.actual - 262.5).abs() < 1e-9);
        assert!(metric.met);
    }

    #[test]
    fn supports_metric_attribute_interval() {
        let panel = MiraclePanel {
            speed: 160.0,
            ..Default::default()
        };
        let metric = MiracleMetric {
            preset_id: "speed".to_owned(),
            metric_type: "minimum".to_owned(),
            label: "速度区间".to_owned(),
            operands: vec!["speed".to_owned()],
            threshold: 150.0,
            percentage: false,
            constraints: vec![MiracleMetricConstraint {
                attribute: "speed".to_owned(),
                minimum: 152.0,
                maximum: Some(180.0),
            }],
        };

        // 160 位于 152～180 区间内时，属性限制和整体指标都应达标。
        let matched = metric_result(&panel, &metric);
        assert!(matched.met);
        assert_eq!(matched.constraints[0].gap, 0.0);
        assert_eq!(matched.constraints[0].maximum, Some(180.0));

        // 超过上限时缺口应反映“超出”的数值，而不是继续按最低值补强。
        let over_limit = MiracleMetric {
            constraints: vec![MiracleMetricConstraint {
                attribute: "speed".to_owned(),
                minimum: 152.0,
                maximum: Some(155.0),
            }],
            ..metric
        };
        let result = metric_result(&panel, &over_limit);
        assert!(!result.met);
        assert_eq!(result.constraints[0].gap, 5.0);
        assert_eq!(result.constraints[0].maximum, Some(155.0));
    }

    #[test]
    fn rejects_solution_above_metric_interval_upper_bound() {
        // 复现用户配置：式神基础速度 100，御魂合计增加 31.69，实际速度 131.69。
        // 即使达到最低目标 119，只要超过上限 122，完整计算结果也必须判为未达标。
        let inventory = (1..=6)
            .map(|slot| soul(slot, "散件", "speed", if slot == 1 { 31.69 } else { 0.0 }))
            .collect::<Vec<_>>();
        let result = calculate_miracle_conch(
            &MiracleConchRequest {
                shikigami_id: "389".to_owned(),
                target: MiracleTarget::default(),
                only_max_level: true,
                metrics: vec![MiracleMetric {
                    preset_id: "speed".to_owned(),
                    metric_type: "minimum".to_owned(),
                    label: "速度".to_owned(),
                    operands: vec!["speed".to_owned()],
                    threshold: 119.0,
                    percentage: false,
                    constraints: vec![MiracleMetricConstraint {
                        attribute: "speed".to_owned(),
                        minimum: 119.0,
                        maximum: Some(122.0),
                    }],
                }],
                main_attributes: Vec::new(),
                recipe: vec![MiracleRecipeLine {
                    set_id: SCATTERED_SET_ID.to_owned(),
                    count: 6,
                    category: None,
                }],
            },
            &shikigami(),
            &[],
            &inventory,
            0,
            0,
        )
        .expect("速度区间请求应合法");

        assert_eq!(result.status, "closest");
        let metric = result.solutions[0].metric.as_ref().expect("应返回速度指标");
        assert!((metric.actual - 131.69).abs() < 1e-9);
        assert!(!metric.met);
        assert_eq!(metric.constraints[0].maximum, Some(122.0));
        assert!(!metric.constraints[0].met);
    }

    #[test]
    fn bounds_large_inventory_enhancement_analysis() {
        // 强化建议候选数量可以远大于最终展示的 10 条；这里只验证扫描会有界停止，
        // 主方案搜索仍由 calculate_miracle_conch 负责完整枚举。
        let selected = (1..=6)
            .map(|slot| soul(slot, "散件", "speed", 3.0))
            .collect::<Vec<_>>();
        let request = MiracleConchRequest {
            shikigami_id: "389".to_owned(),
            target: MiracleTarget {
                speed: Some(119.0),
                ..Default::default()
            },
            only_max_level: true,
            metrics: Vec::new(),
            main_attributes: Vec::new(),
            recipe: vec![MiracleRecipeLine {
                set_id: SCATTERED_SET_ID.to_owned(),
                count: 6,
                category: None,
            }],
        };
        let base_panel = parse_shikigami_panel(&shikigami()).expect("测试面板应可解析");
        let set_map = HashMap::new();
        let solution = make_solution(
            &selected,
            &set_map,
            &base_panel,
            &request.target,
            &request.metrics,
            false,
        );
        let mut inventory = selected.clone();
        for index in 0..(MAX_ENHANCEMENT_CANDIDATE_EVALUATIONS + 1) {
            let mut candidate = soul(1, "散件", "speed", 0.2);
            candidate.soul.internal_id = format!("enhancement-budget-{index}");
            candidate.soul.source_stable_id = Some(candidate.soul.internal_id.clone());
            candidate.soul.level = 12;
            candidate.soul.initial_substat_count = Some(3);
            candidate.attributes = vec![
                candidate.attributes[0].clone(),
                SoulAttribute {
                    attribute_type: "attack_rate".to_owned(),
                    value: 0.1,
                    ..candidate.attributes[0].clone()
                },
                SoulAttribute {
                    attribute_type: "crit_damage".to_owned(),
                    value: 0.1,
                    ..candidate.attributes[0].clone()
                },
            ];
            inventory.push(candidate);
        }

        let analysis =
            build_enhancement_candidates(&solution, &inventory, &request, &set_map, &base_panel);

        assert!(analysis.truncated);
        assert!(analysis.candidates.len() <= 10);
    }

    #[test]
    fn validates_empty_target_and_recipe_count() {
        let error = calculate_miracle_conch(
            &MiracleConchRequest {
                shikigami_id: "389".to_owned(),
                target: MiracleTarget::default(),
                only_max_level: true,
                metrics: Vec::new(),
                main_attributes: Vec::new(),
                recipe: vec![],
            },
            &shikigami(),
            &[],
            &[],
            0,
            0,
        )
        .expect_err("空目标必须被拒绝");
        assert!(error.contains("至少填写一项"));
    }

    #[test]
    fn filters_unmaxed_souls_when_only_max_level_is_enabled() {
        let sets = vec![set("攻击套", Some("攻击加成"))];
        let mut inventory = (1..=6)
            .map(|slot| soul(slot, "攻击套", "attack_rate", 0.1))
            .collect::<Vec<_>>();
        // 只把一个位置降到 +14，便于同时验证严格筛选和关闭开关后的完整候选数。
        inventory[5].soul.level = 14;
        let request = |only_max_level| MiracleConchRequest {
            shikigami_id: "389".to_owned(),
            target: MiracleTarget {
                attack: Some(100.0),
                ..Default::default()
            },
            only_max_level,
            metrics: Vec::new(),
            main_attributes: Vec::new(),
            recipe: vec![MiracleRecipeLine {
                set_id: "攻击套".to_owned(),
                count: 6,
                category: None,
            }],
        };

        let strict = calculate_miracle_conch(&request(true), &shikigami(), &sets, &inventory, 0, 0)
            .expect("仅满级请求应合法");
        assert_eq!(strict.candidate_count, 5);
        assert!(strict.solutions.is_empty());

        let relaxed =
            calculate_miracle_conch(&request(false), &shikigami(), &sets, &inventory, 0, 0)
                .expect("关闭仅满级后请求应合法");
        assert_eq!(relaxed.candidate_count, 6);
        assert!(!relaxed.solutions.is_empty());
    }

    #[test]
    fn supports_two_plus_two_plus_two_recipe() {
        let sets = vec![
            set("攻击套", Some("攻击加成")),
            set("暴击套", Some("暴击")),
            set("速度套", Some("速度")),
        ];
        let inventory = (1..=6)
            .map(|slot| {
                let (set_id, attribute) = match slot {
                    1 | 2 => ("攻击套", "attack_rate"),
                    3 | 4 => ("暴击套", "crit_rate"),
                    _ => ("速度套", "speed"),
                };
                soul(slot, set_id, attribute, 0.1)
            })
            .collect::<Vec<_>>();
        let result = calculate_miracle_conch(
            &MiracleConchRequest {
                shikigami_id: "389".to_owned(),
                target: MiracleTarget {
                    attack: Some(100.0),
                    ..Default::default()
                },
                only_max_level: true,
                metrics: Vec::new(),
                main_attributes: Vec::new(),
                recipe: vec![
                    MiracleRecipeLine {
                        set_id: "攻击套".to_owned(),
                        count: 2,
                        category: None,
                    },
                    MiracleRecipeLine {
                        set_id: "暴击套".to_owned(),
                        count: 2,
                        category: None,
                    },
                    MiracleRecipeLine {
                        set_id: "速度套".to_owned(),
                        count: 2,
                        category: None,
                    },
                ],
            },
            &shikigami(),
            &sets,
            &inventory,
            0,
            0,
        )
        .expect("2+2+2 配方应合法");
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(result.solutions[0].set_counts["攻击套"], 2);
        assert_eq!(result.solutions[0].set_counts["暴击套"], 2);
        assert_eq!(result.solutions[0].set_counts["速度套"], 2);
    }

    #[test]
    fn supports_category_wildcard_with_unrestricted_remainder() {
        let sets = vec![
            set("暴击套一", Some("暴击")),
            set("暴击套二", Some("暴击")),
            set("攻击套", Some("攻击加成")),
        ];
        let inventory = (1..=6)
            .map(|slot| {
                let (set_id, attribute) = match slot {
                    1 | 2 => ("暴击套一", "crit_rate"),
                    3 | 4 => ("暴击套二", "crit_rate"),
                    _ => ("攻击套", "attack_rate"),
                };
                soul(slot, set_id, attribute, 0.1)
            })
            .collect::<Vec<_>>();
        let result = calculate_miracle_conch(
            &MiracleConchRequest {
                shikigami_id: "389".to_owned(),
                target: MiracleTarget {
                    attack: Some(100.0),
                    ..Default::default()
                },
                only_max_level: true,
                metrics: Vec::new(),
                main_attributes: Vec::new(),
                recipe: vec![
                    MiracleRecipeLine {
                        set_id: UNRESTRICTED_SET_ID.to_owned(),
                        count: 4,
                        category: Some("暴击".to_owned()),
                    },
                    MiracleRecipeLine {
                        set_id: SCATTERED_SET_ID.to_owned(),
                        count: 2,
                        category: None,
                    },
                ],
            },
            &shikigami(),
            &sets,
            &inventory,
            0,
            0,
        )
        .expect("分类通配和剩余不限制的配方应合法");
        assert_eq!(result.solutions.len(), 1);
        let selected_crit_count = result.solutions[0]
            .souls
            .iter()
            .filter(|soul| soul.set_id == "暴击套一" || soul.set_id == "暴击套二")
            .count();
        assert_eq!(selected_crit_count, 4);
    }

    #[test]
    fn category_wildcard_keeps_all_matching_sets_without_fixed_candidate_cap() {
        // 回归规则：无关套装可以很多，但不能挤掉分类通配真正允许的网切候选。
        let sets = vec![set("火灵", Some("攻击加成")), set("网切", Some("暴击"))];
        let mut inventory = Vec::new();
        for slot in 1..=6 {
            inventory.push(soul(slot, "火灵", "crit_rate", 0.15));
            if matches!(slot, 2 | 4) {
                inventory.push(soul(slot, "网切", "crit_rate", 0.15));
            }
            for index in 0..300 {
                inventory.push(soul(
                    slot,
                    &format!("无关套装-{slot}-{index}"),
                    "attack_flat",
                    1.0,
                ));
            }
        }

        let result = calculate_miracle_conch(
            &MiracleConchRequest {
                shikigami_id: "389".to_owned(),
                target: MiracleTarget {
                    crit_rate: Some(1.0),
                    ..Default::default()
                },
                only_max_level: true,
                metrics: Vec::new(),
                main_attributes: Vec::new(),
                recipe: vec![
                    MiracleRecipeLine {
                        set_id: "火灵".to_owned(),
                        count: 4,
                        category: None,
                    },
                    MiracleRecipeLine {
                        set_id: UNRESTRICTED_SET_ID.to_owned(),
                        count: 2,
                        category: Some("暴击".to_owned()),
                    },
                ],
            },
            &shikigami(),
            &sets,
            &inventory,
            0,
            0,
        )
        .expect("分类通配请求应合法");

        assert_eq!(result.status, "matched");
        assert!(result.search_complete);
        assert!(
            result.solutions[0]
                .souls
                .iter()
                .any(|selected| selected.set_id == "网切")
        );
    }

    #[test]
    fn keeps_unknown_enhancement_count_without_quality_note() {
        let mut unknown = soul(1, "攻击套", "attack_rate", 0.1);
        unknown.attributes[0].enhancement_count = None;
        let inventory = (1..=6)
            .map(|slot| {
                if slot == 1 {
                    unknown.clone()
                } else {
                    soul(slot, "攻击套", "attack_rate", 0.1)
                }
            })
            .collect::<Vec<_>>();
        let result = calculate_miracle_conch(
            &MiracleConchRequest {
                shikigami_id: "389".to_owned(),
                target: MiracleTarget {
                    attack: Some(100.0),
                    ..Default::default()
                },
                only_max_level: true,
                metrics: Vec::new(),
                main_attributes: Vec::new(),
                recipe: vec![MiracleRecipeLine {
                    set_id: SCATTERED_SET_ID.to_owned(),
                    count: 6,
                    category: None,
                }],
            },
            &shikigami(),
            &[],
            &inventory,
            0,
            0,
        )
        .expect("散件配方应合法");
        // 强化次数未知仍由领域逻辑保留为未知，不再把这类实现细节作为结果提示展示。
        assert!(result.quality_notes.is_empty());
    }

    #[test]
    fn theoretical_speed_cap_keeps_initial_value_before_future_rolls() {
        let candidate = soul(2, "攻击套", "speed", 2.1);
        let base_panel = MiraclePanel::default();
        let projected = project_candidate_rolls(
            &candidate,
            std::slice::from_ref(&candidate),
            "speed",
            3.0,
            remaining_enhancement_nodes(0),
            &BTreeMap::from([(String::from("攻击套"), 1)]),
            &HashMap::new(),
            &base_panel,
        )
        .expect("已有速度副属性应允许继续投影");
        // +0 的 2.1 仍然保留，五个节点全部按 3 点上限成长也只有 17.1。
        assert!((projected.speed - 17.1).abs() < 1e-9);
        assert!(projected.speed < 17.5);
    }

    #[test]
    fn includes_low_level_soul_from_best_solution_in_enhancement_advice() {
        let mut inventory = (1..=6)
            .map(|slot| soul(slot, "攻击套", "attack_rate", 0.1))
            .collect::<Vec<_>>();
        inventory[1].soul.level = 0;
        inventory[1].attributes[0].attribute_type = "speed".to_owned();
        inventory[1].attributes[0].value = 2.1;
        let sets = vec![set("攻击套", Some("攻击加成"))];
        let result = calculate_miracle_conch(
            &MiracleConchRequest {
                shikigami_id: "389".to_owned(),
                target: MiracleTarget {
                    // 须佐之男夹具基础速度为 100，因此目标面板要把基础值一并计入。
                    speed: Some(117.5),
                    ..Default::default()
                },
                only_max_level: false,
                metrics: Vec::new(),
                main_attributes: Vec::new(),
                recipe: vec![MiracleRecipeLine {
                    set_id: "攻击套".to_owned(),
                    count: 6,
                    category: None,
                }],
            },
            &shikigami(),
            &sets,
            &inventory,
            0,
            0,
        )
        .expect("目标应能返回最接近方案");
        assert_eq!(result.status, "closest");
        assert!(
            result
                .enhancement_candidates
                .iter()
                .any(|candidate| candidate.soul.soul_key == inventory[1].soul_key())
        );
        let candidate = result
            .enhancement_candidates
            .iter()
            .find(|candidate| candidate.soul.soul_key == inventory[1].soul_key())
            .expect("低等级速度胚子应出现在强化建议");
        // 基础速度 100 + 副属性初始 2.1 + 五个强化节点各 3 点 = 117.1。
        assert!((candidate.theoretical_panel.speed - 117.1).abs() < 1e-9);
        // 理论面板与副属性列表必须来自同一条路径，前端才能逐条展示并准确高亮速度。
        assert_eq!(candidate.theoretical_attributes.len(), 1);
        assert_eq!(candidate.theoretical_attributes[0].attribute_type, "speed");
        assert!((candidate.theoretical_attributes[0].expected_value - 17.1).abs() < 1e-9);
        assert_eq!(
            candidate.theoretical_attributes[0].expected_enhancement_count,
            5
        );
    }

    #[test]
    fn online_meet_in_middle_merge_matches_materialized_reference() {
        // 使用互有取舍的攻击、暴伤、暴击和速度构造多维前沿；在线合并必须与相同
        // 半区前沿的全量合并返回相同前 12 套，避免性能优化改变同分稳定顺序。
        let sets = vec![set("狂骨", Some("攻击加成")), set("土蜘蛛", None)];
        let mut inventory = Vec::new();
        for slot in 1..=6 {
            for index in 0..8_u8 {
                let set_id = if index % 2 == 0 {
                    "狂骨"
                } else {
                    "土蜘蛛"
                };
                let mut item = soul(slot, set_id, "attack_rate", 0.0);
                item.soul.internal_id = format!("comparison-{slot}-{index}");
                item.soul.source_stable_id = Some(item.soul.internal_id.clone());
                item.attributes = vec![
                    SoulAttribute {
                        attribute_type: "attack_rate".to_owned(),
                        value: f64::from((index * 3 + slot) % 8) * 0.03,
                        ..item.attributes[0].clone()
                    },
                    SoulAttribute {
                        attribute_type: "crit_damage".to_owned(),
                        value: f64::from((index * 5 + slot) % 8) * 0.04,
                        ..item.attributes[0].clone()
                    },
                    SoulAttribute {
                        attribute_type: "crit_rate".to_owned(),
                        value: f64::from((index * 7 + slot) % 8) * 0.01,
                        ..item.attributes[0].clone()
                    },
                    SoulAttribute {
                        attribute_type: "speed".to_owned(),
                        value: f64::from((index * 2 + slot) % 8),
                        ..item.attributes[0].clone()
                    },
                ];
                inventory.push(item);
            }
        }
        let request = MiracleConchRequest {
            shikigami_id: "389".to_owned(),
            target: MiracleTarget::default(),
            only_max_level: true,
            metrics: vec![MiracleMetric {
                preset_id: "damage_output".to_owned(),
                metric_type: "product".to_owned(),
                label: "伤害输出".to_owned(),
                operands: vec!["attack".to_owned(), "crit_damage".to_owned()],
                threshold: 260.0,
                percentage: false,
                constraints: vec![
                    MiracleMetricConstraint {
                        attribute: "speed".to_owned(),
                        minimum: 115.0,
                        maximum: None,
                    },
                    MiracleMetricConstraint {
                        attribute: "crit_rate".to_owned(),
                        minimum: 0.3,
                        maximum: None,
                    },
                ],
            }],
            main_attributes: Vec::new(),
            recipe: vec![
                MiracleRecipeLine {
                    set_id: "狂骨".to_owned(),
                    count: 4,
                    category: None,
                },
                MiracleRecipeLine {
                    set_id: "土蜘蛛".to_owned(),
                    count: 2,
                    category: None,
                },
            ],
        };
        let base_panel = parse_shikigami_panel(&shikigami()).expect("测试面板应可解析");
        let set_map = sets
            .into_iter()
            .map(|item| (item.set_id.clone(), item))
            .collect::<HashMap<_, _>>();
        let (slot_candidates, _) =
            build_slot_candidates(&inventory, &request, &set_map, &base_panel);

        // 参考实现物化相同半区前沿的全部合法配对后再排序；它只用于小夹具验证，
        // 生产算法则在线保留前 12 个，二者的选择和稳定顺序必须完全一致。
        let relevant_attributes =
            target_attribute_order_for_request(&request.target, &request.metrics);
        let (left_states, _) = enumerate_mitm_half(
            &slot_candidates,
            0,
            3,
            &request,
            &set_map,
            &base_panel,
            &relevant_attributes,
        );
        let (right_states, _) = enumerate_mitm_half(
            &slot_candidates,
            3,
            6,
            &request,
            &set_map,
            &base_panel,
            &relevant_attributes,
        );
        let mut expected_solutions = Vec::new();
        for left in &left_states {
            for right in &right_states {
                if left
                    .used_recipe
                    .iter()
                    .zip(&right.used_recipe)
                    .enumerate()
                    .any(|(index, (left_count, right_count))| {
                        left_count.saturating_add(*right_count) != request.recipe[index].count
                    })
                {
                    continue;
                }
                let Some(merged) = merge_mitm_pair(left, right, &slot_candidates, &set_map, true)
                else {
                    continue;
                };
                let selected = merged
                    .selected_indices
                    .iter()
                    .enumerate()
                    .map(|(slot_index, candidate_index)| {
                        slot_candidates[slot_index][*candidate_index].clone()
                    })
                    .collect::<Vec<_>>();
                expected_solutions.push(make_solution(
                    &selected,
                    &set_map,
                    &base_panel,
                    &request.target,
                    &request.metrics,
                    false,
                ));
            }
        }
        expected_solutions.sort_by(solution_order);
        expected_solutions.truncate(12);
        let actual = search_meet_in_middle(&slot_candidates, &request, &set_map, &base_panel);
        let expected_keys = expected_solutions
            .iter()
            .map(solution_key)
            .collect::<Vec<_>>();
        let actual_keys = actual
            .solutions
            .iter()
            .map(solution_key)
            .collect::<Vec<_>>();

        assert_eq!(actual_keys, expected_keys);
    }

    #[test]
    #[ignore = "大库存性能基准，按需运行"]
    fn benchmarks_damage_output_with_kyokotsu_and_tsuchigumo() {
        // 使用 3360 枚候选复现“伤害输出 28000、满暴、狂骨 4 + 土蜘蛛 2”；
        // 该测试记录各阶段耗时，避免只看前端停在 94% 而无法判断真正的慢点。
        let sets = vec![set("狂骨", Some("攻击加成")), set("土蜘蛛", None)];
        let mut inventory = Vec::new();
        for slot in 1..=6 {
            for index in 0..560 {
                let set_id = if index % 2 == 0 {
                    "狂骨"
                } else {
                    "土蜘蛛"
                };
                let mut item = soul(slot, set_id, "crit_rate", 0.01 * f64::from(index % 20));
                item.soul.internal_id = format!("benchmark-{slot}-{index}");
                item.soul.source_stable_id = Some(format!("benchmark-{slot}-{index}"));
                item.soul.main_attr_type = match slot {
                    2 => "speed",
                    4 => "attack_rate",
                    6 => {
                        if index % 2 == 0 {
                            "crit_damage"
                        } else {
                            "crit_rate"
                        }
                    }
                    _ => "attack_flat",
                }
                .to_owned();
                item.soul.main_attr_value = match slot {
                    2 => 57.0,
                    4 => 0.55,
                    6 if index % 2 == 0 => 0.89,
                    6 => 0.55,
                    _ => 486.0,
                };
                item.soul.level = if index % 12 == 0 { 12 } else { 15 };
                item.attributes = vec![
                    SoulAttribute {
                        snapshot_id: "benchmark".to_owned(),
                        soul_internal_id: item.soul.internal_id.clone(),
                        attribute_index: 0,
                        attribute_type: "crit_rate".to_owned(),
                        value: f64::from(index % 17) * 0.005,
                        enhancement_count: Some(1),
                        count_provenance: "source".to_owned(),
                        fixed_attribute: false,
                    },
                    SoulAttribute {
                        snapshot_id: "benchmark".to_owned(),
                        soul_internal_id: item.soul.internal_id.clone(),
                        attribute_index: 1,
                        attribute_type: "crit_damage".to_owned(),
                        value: f64::from(index % 19) * 0.01,
                        enhancement_count: Some(1),
                        count_provenance: "source".to_owned(),
                        fixed_attribute: false,
                    },
                    SoulAttribute {
                        snapshot_id: "benchmark".to_owned(),
                        soul_internal_id: item.soul.internal_id.clone(),
                        attribute_index: 2,
                        attribute_type: "speed".to_owned(),
                        value: f64::from(index % 23) * 0.5,
                        enhancement_count: Some(1),
                        count_provenance: "source".to_owned(),
                        fixed_attribute: false,
                    },
                    SoulAttribute {
                        snapshot_id: "benchmark".to_owned(),
                        soul_internal_id: item.soul.internal_id.clone(),
                        attribute_index: 3,
                        attribute_type: "attack_rate".to_owned(),
                        value: f64::from(index % 29) * 0.01,
                        enhancement_count: Some(1),
                        count_provenance: "source".to_owned(),
                        fixed_attribute: false,
                    },
                ];
                inventory.push(item);
            }
        }
        let request = MiracleConchRequest {
            shikigami_id: "389".to_owned(),
            target: MiracleTarget::default(),
            only_max_level: true,
            metrics: vec![MiracleMetric {
                preset_id: "damage_output".to_owned(),
                metric_type: "product".to_owned(),
                label: "伤害输出".to_owned(),
                operands: vec!["attack".to_owned(), "crit_damage".to_owned()],
                threshold: 28_000.0,
                percentage: false,
                constraints: vec![MiracleMetricConstraint {
                    attribute: "crit_rate".to_owned(),
                    minimum: 1.0,
                    maximum: None,
                }],
            }],
            main_attributes: vec![
                MiracleMainAttributeConstraint {
                    slot: 2,
                    main_attr_type: "speed".to_owned(),
                },
                MiracleMainAttributeConstraint {
                    slot: 4,
                    main_attr_type: "attack_rate".to_owned(),
                },
                MiracleMainAttributeConstraint {
                    slot: 6,
                    main_attr_type: "crit_damage".to_owned(),
                },
            ],
            recipe: vec![
                MiracleRecipeLine {
                    set_id: "狂骨".to_owned(),
                    count: 4,
                    category: None,
                },
                MiracleRecipeLine {
                    set_id: "土蜘蛛".to_owned(),
                    count: 2,
                    category: None,
                },
            ],
        };
        let shikigami = shikigami();
        let base_panel = parse_shikigami_panel(&shikigami).expect("基准式神面板应可解析");
        let set_map = sets
            .iter()
            .cloned()
            .map(|set| (set.set_id.clone(), set))
            .collect::<HashMap<_, _>>();
        let candidates = inventory
            .iter()
            .filter(|item| item.soul.quality == 6 && item.soul.level == 15)
            .cloned()
            .collect::<Vec<_>>();
        let start = std::time::Instant::now();
        let (slot_candidates, _) =
            build_slot_candidates(&candidates, &request, &set_map, &base_panel);
        let candidates_elapsed = start.elapsed();
        let search_start = std::time::Instant::now();
        // 基准必须走生产使用的 Meet-in-the-middle，便于直接观察切换后的真实搜索耗时。
        let search = search_meet_in_middle(&slot_candidates, &request, &set_map, &base_panel);
        let search_elapsed = search_start.elapsed();
        eprintln!(
            "damage benchmark candidate slots={:?} search_nodes={} solutions={}",
            slot_candidates.iter().map(Vec::len).collect::<Vec<_>>(),
            search.nodes,
            search.solutions.len(),
        );
        let best = search.solutions.first().expect("基准库存应能组成六件方案");
        let enhancement_start = std::time::Instant::now();
        let enhancement =
            build_enhancement_candidates(best, &inventory, &request, &set_map, &base_panel);
        let enhancement_elapsed = enhancement_start.elapsed();
        eprintln!(
            "damage+kyokotsu+tsuchigumo benchmark: candidates={} build={:?} search_nodes={} search={:?} enhancement_candidates={} enhancement={:?} total={:?}",
            candidates.len(),
            candidates_elapsed,
            search.nodes,
            search_elapsed,
            enhancement.candidates.len(),
            enhancement_elapsed,
            start.elapsed(),
        );
    }
}
