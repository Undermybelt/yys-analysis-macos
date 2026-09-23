//! 模拟强化领域逻辑。
//!
//! 该模块只负责根据已核验的概率规则生成娱乐用强化轨迹，不写入库存，也不改变真实御魂。
//! 具体评分仍由既有规则引擎完成，避免模拟功能复制一套评分公式。

use super::{EnhancementRule, SoulAttributeFact};

/// 模拟强化使用的官方概率规则版本；版本变化时应新增版本而不是静默覆盖旧结果。
pub const SIMULATION_MECHANICS_VERSION: &str = "official-2017.05-enhancement-v1";
/// 模拟成本配置版本；数值来自用户提供的 NGA 参考资料，仅用于娱乐估算。
pub const SIMULATION_COST_VERSION: &str = "nga-6star-plus15-v1";
/// 副属性成长数值模型版本；数值上下限来自用户提供的御魂系统分析 PDF。
pub const SIMULATION_VALUE_MODEL_VERSION: &str = "pdf-2026.01-substat-range-v1";
/// 六星御魂从 +0 模拟到 +15 的御魂经验估算。
pub const SIX_STAR_PLUS15_EXP_COST: u64 = 236_250;
/// 六星御魂从 +0 模拟到 +15 的金币估算。
pub const SIX_STAR_PLUS15_GOLD_COST: u64 = 472_500;
/// 强化观察节点固定为游戏内的五个三级节点。
pub const ENHANCEMENT_CHECKPOINTS: [u8; 5] = [3, 6, 9, 12, 15];

/// 文档给出的副属性单次最低值占单次最高值的比例；所有六星副属性表格均为 80%。
const VALUE_MIN_RATIO: f64 = 0.8;

/// 官方公告中列出的副属性类型分类；类别内均匀抽取是模拟假设，不代表官方单属性概率。
pub const ATTACK_ATTRIBUTES: [&str; 4] = ["attack_flat", "attack_rate", "crit_rate", "crit_damage"];
pub const DEFENSE_ATTRIBUTES: [&str; 4] = ["hp_flat", "defense_flat", "hp_rate", "defense_rate"];
pub const UTILITY_ATTRIBUTES: [&str; 3] = ["effect_resist", "effect_hit", "speed"];

/// 一次节点强化命中的展示记录。
#[derive(Clone, Debug, PartialEq)]
pub struct SimulationRoll {
    pub checkpoint: u8,
    pub attribute_type: String,
    pub attribute_label: String,
    pub outcome: SimulationRollOutcome,
    /// 本次节点实际抽到的成长值；仅用于娱乐结果展示，不代表官方档位概率。
    pub value_delta: f64,
    /// 说明本次命中来自四腿均匀选择，或来自官方类别概率下的类别内抽取。
    pub probability_basis: String,
}

/// 节点命中是已有腿成长，还是模拟新增腿。
#[derive(Clone, Debug, PartialEq)]
pub enum SimulationRollOutcome {
    ExistingAttribute,
    NewAttribute,
}

/// 模拟完成后的副属性；最终值和区间都按文档的单次最低/最高值累加。
#[derive(Clone, Debug, PartialEq)]
pub struct SimulationAttribute {
    pub attribute_type: String,
    pub value: f64,
    pub value_min: f64,
    pub value_max: f64,
    pub enhancement_count: u8,
}

/// 单枚御魂的纯随机强化结果。
#[derive(Clone, Debug, PartialEq)]
pub struct SimulationRollout {
    pub attributes: Vec<SimulationAttribute>,
    pub rolls: Vec<SimulationRoll>,
}

/// 无外部依赖的可复现随机数生成器；种子由调用方传入，便于回放测试结果。
#[derive(Clone, Debug)]
pub struct SimulationRng {
    state: u64,
}

impl SimulationRng {
    /// 创建一个不会因为零种子而退化成全零序列的随机状态。
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9e37_79b9_7f4a_7c15
            } else {
                seed
            },
        }
    }

    /// 生成 [0, 1) 区间的随机小数；使用高位避免低位周期影响抽样。
    fn next_unit(&mut self) -> f64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let value = self.state >> 11;
        value as f64 / (1u64 << 53) as f64
    }

    /// 从非空切片中均匀抽取一个元素。
    fn choose_index(&mut self, length: usize) -> usize {
        debug_assert!(length > 0);
        ((self.next_unit() * length as f64) as usize).min(length - 1)
    }
}

/// 对单枚六星未强化御魂执行五个强化节点的纯内存模拟。
pub fn simulate_rollout(
    initial_attributes: &[SoulAttributeFact],
    rule: &EnhancementRule,
    rng: &mut SimulationRng,
) -> SimulationRollout {
    let mut attributes = initial_attributes
        .iter()
        .map(|attribute| SimulationAttribute {
            attribute_type: attribute.attribute_type.clone(),
            value: attribute.value,
            // 初始腿已有真实导入值，区间从该值开始；后续节点才使用文档成长区间。
            value_min: attribute.value,
            value_max: attribute.value,
            // +0 的导入数据没有强化次数；模拟从零开始，不继承未知或旧值。
            enhancement_count: 0,
        })
        .collect::<Vec<_>>();
    let mut rolls = Vec::with_capacity(ENHANCEMENT_CHECKPOINTS.len());

    for checkpoint in ENHANCEMENT_CHECKPOINTS {
        let (attribute_type, probability_basis) = if attributes.len() >= 4 {
            let index = rng.choose_index(attributes.len());
            (
                attributes[index].attribute_type.clone(),
                "已有四条副属性：每条副属性均为25%".to_owned(),
            )
        } else {
            choose_attribute_by_category(rule, rng)
        };
        let existing = attributes
            .iter_mut()
            .find(|attribute| attribute.attribute_type == attribute_type);
        let (outcome, value_delta) = if let Some(attribute) = existing {
            // 命中已有腿时增加一次随机成长，并同步扩大该属性的理论最低/最高区间。
            let value_delta = sample_roll_value(&attribute_type, rule, rng);
            let (value_min, value_max) = benchmark_range(&attribute_type, rule);
            attribute.value += value_delta;
            attribute.value_min += value_min;
            attribute.value_max += value_max;
            attribute.enhancement_count = attribute.enhancement_count.saturating_add(1);
            (SimulationRollOutcome::ExistingAttribute, value_delta)
        } else {
            // 新腿首次出现时也按同一张数值表抽取初始值，避免只显示属性类型而没有数值。
            let value_delta = sample_roll_value(&attribute_type, rule, rng);
            let (value_min, value_max) = benchmark_range(&attribute_type, rule);
            attributes.push(SimulationAttribute {
                attribute_type: attribute_type.clone(),
                value: value_delta,
                value_min,
                value_max,
                enhancement_count: 0,
            });
            (SimulationRollOutcome::NewAttribute, value_delta)
        };
        rolls.push(SimulationRoll {
            checkpoint,
            attribute_label: attribute_display_name(&attribute_type).to_owned(),
            attribute_type,
            outcome,
            value_delta,
            probability_basis,
        });
    }

    SimulationRollout { attributes, rolls }
}

/// 从目录强化规则读取某个属性的单次最低/最高成长值；缺少基准时返回零，保持未知数据诚实。
fn benchmark_range(attribute_type: &str, rule: &EnhancementRule) -> (f64, f64) {
    let high_value = rule
        .roll_benchmarks
        .iter()
        .find(|benchmark| benchmark.attribute_id == attribute_type)
        .map(|benchmark| benchmark.high_value)
        .unwrap_or(0.0);
    (high_value * VALUE_MIN_RATIO, high_value)
}

/// 在文档给出的单次最低值和最高值之间均匀抽取一次成长值；这是娱乐抽样假设。
fn sample_roll_value(attribute_type: &str, rule: &EnhancementRule, rng: &mut SimulationRng) -> f64 {
    let (value_min, value_max) = benchmark_range(attribute_type, rule);
    value_min + (value_max - value_min) * rng.next_unit()
}

/// 按官方类别概率先抽类别，再在类别内等概率抽取具体属性。
/// 官方未公布类别内概率，因此这里将其作为娱乐模拟假设并通过结果文案持续披露。
fn choose_attribute_by_category(
    rule: &EnhancementRule,
    rng: &mut SimulationRng,
) -> (String, String) {
    let categories = &rule.incomplete_mode.categories;
    let roll = rng.next_unit();
    let mut cursor = 0.0;
    let selected = categories
        .iter()
        .find(|category| {
            cursor += category.probability;
            roll < cursor
        })
        .or_else(|| categories.last())
        .expect("内置强化规则必须至少包含一个副属性类别");
    // 目录中的属性字段是 String，而内置兜底常量是 &str；这里统一先取选项再复制成 String，
    // 避免为了随机抽样改变目录规则的所有权或引入两套概率分支。
    let attribute_type = if selected.attributes.is_empty() {
        let attributes: &[&str] = match selected.id.as_str() {
            "attack" => &ATTACK_ATTRIBUTES,
            "defense" => &DEFENSE_ATTRIBUTES,
            _ => &UTILITY_ATTRIBUTES,
        };
        attributes[rng.choose_index(attributes.len())].to_owned()
    } else {
        selected.attributes[rng.choose_index(selected.attributes.len())].clone()
    };
    (
        attribute_type,
        format!(
            "{}：类别概率{}%，类别内等概率（娱乐假设）",
            selected.label,
            trim_probability(selected.probability * 100.0)
        ),
    )
}

/// 统一显示属性名称；未知类型保留原始键，保证目录新增属性仍可追溯。
pub fn attribute_display_name(attribute_type: &str) -> &str {
    match attribute_type {
        "attack_flat" => "攻击",
        "defense_flat" => "防御",
        "hp_flat" => "生命",
        "attack_rate" => "攻击加成",
        "defense_rate" => "防御加成",
        "hp_rate" => "生命加成",
        "speed" => "速度",
        "effect_hit" => "效果命中",
        "effect_resist" => "效果抵抗",
        "crit_rate" => "暴击",
        "crit_damage" => "暴击伤害",
        _ => attribute_type,
    }
}

/// 将概率转换为稳定短文本，避免 28/3 产生过长的小数。
fn trim_probability(value: f64) -> String {
    if (value - value.round()).abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        EnhancementCategoryProbability, EnhancementFourSubstatMode, EnhancementIncompleteMode,
        SoulRollBenchmark,
    };

    fn rule() -> EnhancementRule {
        EnhancementRule {
            mechanics_version: SIMULATION_MECHANICS_VERSION.to_owned(),
            checkpoints: ENHANCEMENT_CHECKPOINTS.to_vec(),
            four_substat_mode: EnhancementFourSubstatMode {
                kind: "uniformExisting".to_owned(),
                per_attribute_probability: 0.25,
                description: "已有四条副属性时每条25%".to_owned(),
            },
            incomplete_mode: EnhancementIncompleteMode {
                kind: "categoryThenAttribute".to_owned(),
                description: "按官方类别概率抽取".to_owned(),
                categories: vec![
                    EnhancementCategoryProbability {
                        id: "attack".to_owned(),
                        label: "攻击类".to_owned(),
                        probability: 0.36,
                        attributes: ATTACK_ATTRIBUTES.iter().map(|v| (*v).to_owned()).collect(),
                    },
                    EnhancementCategoryProbability {
                        id: "defense".to_owned(),
                        label: "防御类".to_owned(),
                        probability: 0.36,
                        attributes: DEFENSE_ATTRIBUTES.iter().map(|v| (*v).to_owned()).collect(),
                    },
                    EnhancementCategoryProbability {
                        id: "utility".to_owned(),
                        label: "功能类".to_owned(),
                        probability: 0.28,
                        attributes: UTILITY_ATTRIBUTES.iter().map(|v| (*v).to_owned()).collect(),
                    },
                ],
            },
            roll_benchmarks: [
                ("hp_flat", 114.0, "生命"),
                ("defense_flat", 5.0, "防御"),
                ("attack_flat", 27.0, "攻击"),
                ("hp_rate", 0.03, "生命加成"),
                ("defense_rate", 0.03, "防御加成"),
                ("attack_rate", 0.03, "攻击加成"),
                ("speed", 3.0, "速度"),
                ("crit_rate", 0.03, "暴击"),
                ("crit_damage", 0.04, "暴击伤害"),
                ("effect_hit", 0.04, "效果命中"),
                ("effect_resist", 0.04, "效果抵抗"),
            ]
            .into_iter()
            .map(|(attribute_id, high_value, label)| SoulRollBenchmark {
                attribute_id: attribute_id.to_owned(),
                label: label.to_owned(),
                high_value,
                unit: if high_value < 1.0 {
                    "rate".to_owned()
                } else {
                    "flat".to_owned()
                },
            })
            .collect(),
            probability_note: String::new(),
            source_refs: Vec::new(),
        }
    }

    fn attrs(names: &[&str]) -> Vec<SoulAttributeFact> {
        names
            .iter()
            .map(|name| SoulAttributeFact {
                attribute_type: (*name).to_owned(),
                value: 0.0,
                enhancement_count: None,
                count_provenance: None,
            })
            .collect()
    }

    #[test]
    fn 四条副属性的五次强化始终命中已有属性() {
        let mut rng = SimulationRng::new(7);
        let result = simulate_rollout(&attrs(&ATTACK_ATTRIBUTES), &rule(), &mut rng);
        assert_eq!(result.attributes.len(), 4);
        assert_eq!(result.rolls.len(), 5);
        assert!(
            result
                .rolls
                .iter()
                .all(|roll| roll.outcome == SimulationRollOutcome::ExistingAttribute)
        );
        assert!(
            result
                .attributes
                .iter()
                .map(|item| item.enhancement_count)
                .sum::<u8>()
                == 5
        );
    }

    #[test]
    fn 少于四条时新腿和已有腿都只在四条以内变化() {
        let mut rng = SimulationRng::new(11);
        let result = simulate_rollout(&attrs(&["speed"]), &rule(), &mut rng);
        assert!(result.attributes.len() <= 4);
        assert_eq!(result.rolls.len(), 5);
        assert!(
            result
                .rolls
                .iter()
                .all(|roll| !roll.attribute_type.is_empty())
        );
    }

    #[test]
    fn 官方类别概率合计为百分之百() {
        let total: f64 = 0.36 + 0.36 + 0.28;
        assert!((total - 1.0).abs() < f64::EPSILON);
        assert_eq!(
            ATTACK_ATTRIBUTES.len() + DEFENSE_ATTRIBUTES.len() + UTILITY_ATTRIBUTES.len(),
            11
        );
    }

    #[test]
    fn 强化节点顺序和六星成本配置固定() {
        assert_eq!(ENHANCEMENT_CHECKPOINTS, [3, 6, 9, 12, 15]);
        assert_eq!(SIX_STAR_PLUS15_EXP_COST, 236_250);
        assert_eq!(SIX_STAR_PLUS15_GOLD_COST, 472_500);
    }

    #[test]
    fn 多批种子下重复命中只增加次数且副属性不超过四条() {
        for seed in 1..=128 {
            let mut rng = SimulationRng::new(seed);
            let initial = attrs(&["speed"]);
            let result = simulate_rollout(&initial, &rule(), &mut rng);
            let new_count = result
                .rolls
                .iter()
                .filter(|roll| roll.outcome == SimulationRollOutcome::NewAttribute)
                .count();
            let existing_count = result
                .rolls
                .iter()
                .filter(|roll| roll.outcome == SimulationRollOutcome::ExistingAttribute)
                .count();
            assert_eq!(result.attributes.len(), initial.len() + new_count);
            assert_eq!(
                result
                    .attributes
                    .iter()
                    .map(|attribute| attribute.enhancement_count as usize)
                    .sum::<usize>(),
                existing_count
            );
            assert!(result.attributes.len() <= 4);
        }
    }

    #[test]
    fn 成长数值始终落在参考文档的单次最低到最高区间() {
        let rule = rule();
        let expected_high_values = [
            ("hp_flat", 114.0),
            ("defense_flat", 5.0),
            ("attack_flat", 27.0),
            ("hp_rate", 0.03),
            ("defense_rate", 0.03),
            ("attack_rate", 0.03),
            ("speed", 3.0),
            ("crit_rate", 0.03),
            ("crit_damage", 0.04),
            ("effect_hit", 0.04),
            ("effect_resist", 0.04),
        ];
        for (attribute_type, high_value) in expected_high_values {
            let (value_min, value_max) = benchmark_range(attribute_type, &rule);
            assert!((value_min - high_value * 0.8).abs() < f64::EPSILON);
            assert!((value_max - high_value).abs() < f64::EPSILON);
            for seed in 1..=16 {
                let mut rng = SimulationRng::new(seed);
                let value = sample_roll_value(attribute_type, &rule, &mut rng);
                assert!((value_min..=value_max).contains(&value));
            }
        }
    }
}
