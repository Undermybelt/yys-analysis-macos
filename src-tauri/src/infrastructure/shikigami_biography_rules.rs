//! 式神传记解锁规则的本地只读目录。
//!
//! 规则来源于公开整理表，不能替代游戏快照里的实际进度；快照目标与规则目标不一致时，
//! 本模块返回未知规则，调用方必须展示原始进度而不能擅自换算为御行达摩。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

/// 当前随应用打包的规则表版本；更新规则时同步修改 JSON 的 version/updatedAt。
pub(crate) const SHIKIGAMI_BIOGRAPHY_RULES_VERSION: &str = "2026-08";

/// 传记二的公开解锁条件类型；名称直接作为前后端稳定契约的序列化值。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ShikigamiBiographyUnlockKind {
    SkillUpgrade,
    Level,
    BattleWin,
    TeamBattle,
    ShardCollection,
    ClanPrayer,
    Awakening,
    Other,
    /// 规则表新增类型时按未知处理，避免整份目录因一个新值失效。
    #[serde(other)]
    Unknown,
}

/// 规则表中的一条传记二条件；required 为空表示条件是特殊任务，或原表没有给出可解析的数字目标。
///
/// 特殊任务的条件文本可能包含“9 种”“第 9 层”等业务目标，但快照分母是游戏内部进度，
/// 两者不一定相等，因此这类规则不做分母校验。
#[derive(Clone, Debug, Deserialize)]
struct RawBiographyRule {
    kind: ShikigamiBiographyUnlockKind,
    required: Option<u32>,
    condition: String,
    source: String,
}

/// 规则目录根对象；sources 只用于保留审计信息，查询逻辑只读取 rules。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BiographyRulesDocument {
    version: String,
    #[allow(dead_code)]
    updated_at: String,
    #[allow(dead_code)]
    sources: Vec<serde_json::Value>,
    rules: HashMap<String, RawBiographyRule>,
}

/// 给业务层使用的最小规则摘要，不把整份规则表复制到每个传记条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShikigamiBiographyRule {
    pub kind: ShikigamiBiographyUnlockKind,
    pub condition: String,
    pub source: String,
}

static RULES_DOCUMENT: OnceLock<Option<BiographyRulesDocument>> = OnceLock::new();

/// 解析内置规则表一次；文件损坏时按“未收录”处理，不能阻断已有快照读取。
fn rules_document() -> Option<&'static BiographyRulesDocument> {
    RULES_DOCUMENT
        .get_or_init(|| {
            let document = serde_json::from_str::<BiographyRulesDocument>(include_str!(
                "../data/shikigami-biography-rules.json"
            ))
            .ok()?;
            (document.version == SHIKIGAMI_BIOGRAPHY_RULES_VERSION).then_some(document)
        })
        .as_ref()
}

/// 按式神 ID 和快照分母匹配公开规则；分母不一致时返回 None，避免套用过期条件。
pub(crate) fn biography_rule_for(
    shikigami_id: &str,
    actual_required: u32,
) -> Option<ShikigamiBiographyRule> {
    let Some(raw) = rules_document()?.rules.get(shikigami_id) else {
        // 公开整理表中“升 8/12 次技能”是稳定的通用条件；未收录式神按用户确认的默认规则解释，
        // 其他分母仍保持 unknown，避免把等级或任务进度误当成技能次数。
        return match actual_required {
            8 | 12 => Some(ShikigamiBiographyRule {
                kind: ShikigamiBiographyUnlockKind::SkillUpgrade,
                condition: format!("技能升级{actual_required}次"),
                source: "default-skill-upgrade".to_owned(),
            }),
            _ => None,
        };
    };
    if raw.required.is_some_and(|required| required != actual_required) {
        return None;
    }
    Some(ShikigamiBiographyRule {
        kind: raw.kind,
        condition: raw.condition.clone(),
        source: raw.source.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 荒使用公开表中的升至四十级条件() {
        let rule = biography_rule_for("283", 40).expect("应找到荒的传记规则");
        assert_eq!(rule.kind, ShikigamiBiographyUnlockKind::Level);
        assert_eq!(rule.condition, "将荒升至40级");
        assert_eq!(rule.source, "灰机Wiki-传记解锁条件");
    }

    #[test]
    fn 快照目标与规则不一致时不套用旧条件() {
        assert!(biography_rule_for("283", 12).is_none());
    }

    #[test]
    fn 未收录式神的八或十二次目标默认按技能升级() {
        let eight = biography_rule_for("999", 8).expect("8 次应使用默认技能条件");
        assert_eq!(eight.kind, ShikigamiBiographyUnlockKind::SkillUpgrade);
        assert_eq!(eight.condition, "技能升级8次");
        assert_eq!(eight.source, "default-skill-upgrade");

        let twelve = biography_rule_for("999", 12).expect("12 次应使用默认技能条件");
        assert_eq!(twelve.kind, ShikigamiBiographyUnlockKind::SkillUpgrade);
        assert!(biography_rule_for("999", 40).is_none());
    }

    #[test]
    fn 用户核对的白藏主和两只四十级式神规则应优先命中() {
        let bai_cang_zhu = biography_rule_for("316", 10).expect("应找到白藏主的传记规则");
        assert_eq!(bai_cang_zhu.kind, ShikigamiBiographyUnlockKind::TeamBattle);
        assert_eq!(
            bai_cang_zhu.condition,
            "白藏主与携带神乐的其他玩家取得10次组队胜利"
        );

        for shikigami_id in ["327", "328", "339", "334"] {
            let rule = biography_rule_for(shikigami_id, 40).expect("40 级条件应命中");
            assert_eq!(rule.kind, ShikigamiBiographyUnlockKind::Level);
            assert!(rule.condition.ends_with("升至40级"));
        }
    }

    #[test]
    fn 用户补充的六条传记条件应按指定类型和目标命中() {
        let lian_yu = biography_rule_for("322", 10).expect("应找到炼狱茨木童子的传记规则");
        assert_eq!(lian_yu.kind, ShikigamiBiographyUnlockKind::BattleWin);
        assert_eq!(
            lian_yu.condition,
            "炼狱茨木童子在斗技、练习或切磋中取得10次对抗酒吞童子的胜利"
        );

        let shan_feng = biography_rule_for("296", 1).expect("应找到山风的传记规则");
        assert_eq!(shan_feng.kind, ShikigamiBiographyUnlockKind::Awakening);
        assert_eq!(shan_feng.condition, "将山风进行觉醒");

        for shikigami_id in ["343", "399"] {
            let rule = biography_rule_for(shikigami_id, 10).expect("技能升级 10 次条件应命中");
            assert_eq!(rule.kind, ShikigamiBiographyUnlockKind::SkillUpgrade);
            assert!(rule.condition.ends_with("技能升级10次"));
        }

        let ling_yan_ji = biography_rule_for("602", 12).expect("天火命铃彦姬技能升级 12 次条件应命中");
        assert_eq!(ling_yan_ji.kind, ShikigamiBiographyUnlockKind::SkillUpgrade);
        assert_eq!(ling_yan_ji.condition, "将天火命铃彦姬技能升级12次");

        let yu_zao_qian = biography_rule_for("300", 1).expect("应找到玉藻前的传记规则");
        assert_eq!(yu_zao_qian.kind, ShikigamiBiographyUnlockKind::Other);
        assert_eq!(yu_zao_qian.condition, "玉藻前出战下通关御魂副本第9层");
        // 任务文本中的“第 9 层”不是快照分母，内部进度改为其他分母时仍应命中。
        assert!(biography_rule_for("300", 9).is_some());

        let yu_yuan_ban_ruo = biography_rule_for("331", 30).expect("应找到御怨般若的传记规则");
        assert_eq!(yu_yuan_ban_ruo.kind, ShikigamiBiographyUnlockKind::BattleWin);
        assert_eq!(yu_yuan_ban_ruo.condition, "御怨般若参战取得30场胜利");
    }

    #[test]
    fn 用户重新确认的九条传记条件应覆盖旧规则() {
        let gui_deng = biography_rule_for("308", 1).expect("应找到鬼灯的传记规则");
        assert_eq!(gui_deng.kind, ShikigamiBiographyUnlockKind::Other);
        assert_eq!(gui_deng.condition, "在阴阳寮中捐赠1次鬼灯碎片");

        let hui_ye_ji = biography_rule_for("280", 60).expect("辉夜姬应使用结界突破条件");
        assert_eq!(hui_ye_ji.kind, ShikigamiBiographyUnlockKind::BattleWin);
        assert_eq!(hui_ye_ji.condition, "辉夜姬参与60次结界突破进攻");

        let nu_liang = biography_rule_for("294", 30).expect("应找到奴良陆生的传记规则");
        assert_eq!(nu_liang.kind, ShikigamiBiographyUnlockKind::Level);
        assert_eq!(nu_liang.condition, "将奴良陆生升至30级");

        for (shikigami_id, condition) in [
            (
                "319",
                "桔梗在斗技、练习或切磋中取得10次对抗犬夜叉的胜利",
            ),
            (
                "313",
                "犬夜叉在斗技、练习或切磋中取得10次对抗杀生丸的胜利",
            ),
            (
                "314",
                "杀生丸在斗技、练习或切磋中取得10次对抗犬夜叉的胜利",
            ),
        ] {
            let rule = biography_rule_for(shikigami_id, 10).expect("斗技条件应命中");
            assert_eq!(rule.kind, ShikigamiBiographyUnlockKind::BattleWin);
            assert_eq!(rule.condition, condition);
        }

        let ichigo = biography_rule_for("337", 10).expect("应找到黑崎一护的传记规则");
        assert_eq!(ichigo.kind, ShikigamiBiographyUnlockKind::TeamBattle);
        assert_eq!(
            ichigo.condition,
            "黑崎一护与携带朽木露琪亚的其他玩家取得10次组队胜利"
        );

        let tanjiro = biography_rule_for("359", 1).expect("应找到灶门炭治郎的传记规则");
        assert_eq!(tanjiro.kind, ShikigamiBiographyUnlockKind::Other);
        assert_eq!(
            tanjiro.condition,
            "使用9种水之呼吸和火之神神乐在斗技或切磋中达成击败并取得胜利"
        );
        // 任务文本中的“9 种”不是快照分母，内部进度改为其他分母时仍应命中。
        assert!(biography_rule_for("359", 9).is_some());
    }
}
