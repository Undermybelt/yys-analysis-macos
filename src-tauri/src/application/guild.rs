//! 寮管理视图：从当前导入快照的 source_payload.guild 解析寮概况与成员列表。
//!
//! 上游寮对象由内存读取快照构建（见 desktop_memory_read::build_guild），成员是
//! 15 字段数组：id、duty、donate_times、last_login_time、join_time、offline_time、
//! weekly_feats、history_donate、nickname、dg_times、name、level、receive_times、
//! total_feats、pvp_score。
//!
//! 其中 donate_times、last_login_time、receive_times、dg_times 经真实快照核对为
//! 服务器不下发（全员恒 0），成员活跃度改用 offline_time 与 history_donate 表达。
//! 寮概况的排名取自寮对象顶层，活跃度/建设度等取自 extra。

use crate::application::error::AppError;
use chrono::{DateTime, Local};
use serde::Serialize;
use serde_json::Value;

/// 寮成员条目；只读展示，不携带任何敏感正文。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuildMemberView {
    pub id: String,
    /// 成员昵称；昵称为空时回退到 name 字段。
    pub name: String,
    /// 游戏内职务编码；未知编码保留原始值便于后续校准。
    pub duty_code: u32,
    /// 职务展示名：会长 / 副会长 / 普通成员。
    pub duty_label: String,
    pub level: u32,
    /// 加入时间（Unix 秒）；缺失为 0。
    pub joined_at: u64,
    /// 加入日期（本地时区 YYYY-MM-DD）；缺失为空串。
    pub joined_date: String,
    /// 本周功绩。
    pub weekly_feats: u32,
    /// 总功绩。
    pub total_feats: u32,
    /// 累计捐献。
    pub history_donate: u32,
    /// 离线时间（Unix 秒）；为 0 表示该成员当前在线。
    pub offline_at: u64,
    /// 离线时刻（本地时区 YYYY-MM-DD HH:MM）；当前在线时为空串。
    pub offline_at_label: String,
    pub pvp_score: u32,
}

/// 寮管理页数据：寮概况与全部成员。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuildOverview {
    pub id: String,
    pub name: String,
    pub level: u32,
    /// 寮资金；快照为浮点数，缺失为 0。
    pub funds: f64,
    /// 活跃成员数。
    pub active_member_count: u32,
    /// 成员总数（extra.member_count）；缺失时与成员列表长度一致。
    pub member_count: u32,
    pub server_id: u32,
    /// 创建时间（Unix 秒）。
    pub create_time: u64,
    /// 创建日期（本地时区 YYYY-MM-DD）。
    pub create_date: String,
    pub badge: u32,
    pub pvp_score: u32,
    /// 现任会长名；无会长成员时回退到 extra.creator（建寮者）。
    pub leader_name: String,
    /// 寮宣言；未设置为空串。
    pub declare: String,
    /// 活跃排名；-1 表示未上榜。
    pub active_rank: i64,
    /// 寮战排名；-1 表示未上榜。
    pub pvp_rank: i64,
    /// 活跃度。
    pub active_score: f64,
    /// 建设度。
    pub construction: u32,
    /// 勋章总数。
    pub insignia_count: u32,
    /// 寮战赛季序号。
    pub pvp_season: u32,
    /// 寮突破已达最高难度。
    pub max_gve_difficulty: u32,
    /// 狭间当前难度。
    pub crevice_difficulty: u32,
    /// 狭间已通关难度。
    pub crevice_defeated_difficulty: u32,
    pub members: Vec<GuildMemberView>,
}

/// 会长职务编码；用于挑出现任会长补全寮概况。
const GUILD_DUTY_LEADER: u32 = 1;
/// 副会长职务编码。
const GUILD_DUTY_VICE_LEADER: u32 = 2;

/// 寮职务编码转展示名；未知编码兜底为普通成员。
pub fn guild_duty_label(duty: u32) -> String {
    match duty {
        GUILD_DUTY_LEADER => "会长".to_owned(),
        GUILD_DUTY_VICE_LEADER => "副会长".to_owned(),
        _ => "普通成员".to_owned(),
    }
}

/// 解析当前导入快照中的寮数据；无寮数据或快照未携带时返回 None。
pub fn parse_guild(guild: Option<&Value>) -> Result<Option<GuildOverview>, AppError> {
    let Some(guild) = guild else {
        return Ok(None);
    };
    if guild.is_null() {
        return Ok(None);
    }
    let field = |key: &str| guild.get(key).cloned().unwrap_or(Value::Null);
    let extra = |key: &str| {
        guild
            .get("extra")
            .and_then(|extra| extra.get(key))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let name = guild
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| extra("name").as_str().map(str::to_owned))
        .unwrap_or_default();
    let extra_member_count = extra("member_count").as_u64().unwrap_or(0) as u32;
    let members_value = guild.get("members").cloned().unwrap_or(Value::Array(Vec::new()));
    let members_array = members_value.as_array().ok_or_else(|| {
        AppError::invalid_argument("guild.members", "寮成员字段应为数组")
    })?;
    let members = members_array.iter().map(parse_guild_member).collect::<Vec<_>>();
    // 会长以现任职务为准；寮内无会长时回退到建寮者名，两者通常一致但转让后会分叉。
    let leader_name = members
        .iter()
        .find(|member| member.duty_code == GUILD_DUTY_LEADER)
        .map(|member| member.name.clone())
        .filter(|name| !name.is_empty())
        .or_else(|| extra("creator").as_str().map(str::to_owned))
        .unwrap_or_default();
    Ok(Some(GuildOverview {
        id: field("id")
            .as_str()
            .map(str::to_owned)
            .unwrap_or_default(),
        name,
        level: field("level").as_u64().unwrap_or(0) as u32,
        funds: field("funds").as_f64().unwrap_or(0.0),
        active_member_count: field("activeMemberCount").as_u64().unwrap_or(0) as u32,
        member_count: if extra_member_count > 0 {
            extra_member_count
        } else {
            members.len() as u32
        },
        server_id: field("serverId").as_u64().unwrap_or(0) as u32,
        create_time: field("createTime").as_u64().unwrap_or(0),
        create_date: format_local_date(field("createTime").as_u64().unwrap_or(0)),
        badge: field("guildBadge").as_u64().unwrap_or(0) as u32,
        pvp_score: field("pvpScore").as_u64().unwrap_or(0) as u32,
        leader_name,
        declare: extra("declare").as_str().map(str::to_owned).unwrap_or_default(),
        active_rank: field("activeRank").as_i64().unwrap_or(0),
        pvp_rank: field("pvpRank").as_i64().unwrap_or(0),
        active_score: extra("active_score").as_f64().unwrap_or(0.0),
        construction: extra("construction").as_u64().unwrap_or(0) as u32,
        insignia_count: extra("sum_insignia_count").as_u64().unwrap_or(0) as u32,
        pvp_season: extra("season_pvp_season").as_u64().unwrap_or(0) as u32,
        max_gve_difficulty: extra("max_gve_difficulty").as_u64().unwrap_or(0) as u32,
        crevice_difficulty: extra("guild_crevice_difficulty").as_u64().unwrap_or(0) as u32,
        crevice_defeated_difficulty: extra("guild_crevice_defeated_difficulty")
            .as_u64()
            .unwrap_or(0) as u32,
        members,
    }))
}

/// 把单个 15 字段成员数组解析为成员视图；缺失字段按 0/空串兜底，不拒绝整条记录。
fn parse_guild_member(member: &Value) -> GuildMemberView {
    let values = member.as_array().map(|items| items.as_slice()).unwrap_or(&[]);
    let field = |position: usize| values.get(position).cloned().unwrap_or(Value::Null);
    let number = |position: usize| field(position).as_u64().unwrap_or(0);
    let nickname = field(8).as_str().map(str::to_owned).unwrap_or_default();
    let name = field(10).as_str().map(str::to_owned).unwrap_or_default();
    let duty = number(1) as u32;
    let joined_at = number(4);
    let offline_at = number(5);
    GuildMemberView {
        id: field(0).as_str().map(str::to_owned).unwrap_or_default(),
        name: if nickname.is_empty() { name } else { nickname },
        duty_code: duty,
        duty_label: guild_duty_label(duty),
        level: number(11) as u32,
        joined_at,
        joined_date: format_local_date(joined_at),
        weekly_feats: number(6) as u32,
        total_feats: number(13) as u32,
        history_donate: number(7) as u32,
        offline_at,
        offline_at_label: format_local_date_time(offline_at),
        pvp_score: number(14) as u32,
    }
}

/// Unix 秒转本地时区 YYYY-MM-DD；时间非法或缺失时返回空串。
fn format_local_date(unix_seconds: u64) -> String {
    DateTime::from_timestamp(unix_seconds as i64, 0)
        .map(|time| time.with_timezone(&Local).format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// Unix 秒转本地时区 YYYY-MM-DD HH:MM；0 与非法时间返回空串。
///
/// 成员离线时刻为 0 表示当前在线，与「时间缺失」共用空串由调用方按 offline_at 区分。
fn format_local_date_time(unix_seconds: u64) -> String {
    if unix_seconds == 0 {
        return String::new();
    }
    DateTime::from_timestamp(unix_seconds as i64, 0)
        .map(|time| time.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 与上游内存读取快照同构的寮成员数组；字段顺序见模块顶部注释。
    ///
    /// donate_times / last_login_time / receive_times 按真实快照恒为 0 填写，
    /// 确认解析层不依赖这些服务器不下发的字段。
    #[allow(clippy::too_many_arguments)]
    fn member(
        id: &str,
        duty: u32,
        nickname: &str,
        name: &str,
        level: u32,
        join_time: u64,
        offline_time: u64,
        weekly: u32,
        history_donate: u32,
        total: u32,
    ) -> Value {
        json!([
            id,
            duty,
            0,
            0,
            join_time,
            offline_time,
            weekly,
            history_donate,
            nickname,
            0,
            name,
            level,
            0,
            total,
            1800
        ])
    }

    #[test]
    fn 完整寮数据应解析出概况与成员() {
        let guild = json!({
            "id": "69520452fa2353e33ccea79f",
            "creatorId": "59a18a901a322107c8cf120c",
            "shortId": 18,
            "activeRank": 137,
            "pvpRank": -1,
            "activeMemberCount": 11,
            "funds": 8647734.0,
            "guildBadge": 0,
            "createTime": 1766982738,
            "level": 2,
            "pvpScore": 15000,
            "serverId": 15049,
            "name": "胖虎的小屋",
            "members": [
                member("m1", 1, "", "捉鼠大师小叮当", 60, 1767006667, 0, 180, 129172, 29250),
                member("m2", 2, "铁血战士胖虎", "", 60, 1766982738, 1787094086, 744, 208517, 46915),
                member("m3", 3, "寻汐", "", 60, 1766983248, 1787065707, 130, 31130, 11112),
                member("m4", 4, "新成员", "", 40, 1787094000, 1787094086, 0, 0, 10),
                member("m5", 99, "未知职务", "", 60, 1787094000, 1787094086, 0, 0, 0)
            ],
            "extra": {
                "name": "胖虎的小屋",
                "creator": "捉鼠大师小叮当",
                "declare": "",
                "member_count": 13,
                "active_score": 16644.0,
                "construction": 13200,
                "sum_insignia_count": 7,
                "season_pvp_season": 9,
                "max_gve_difficulty": 7,
                "guild_crevice_difficulty": 2,
                "guild_crevice_defeated_difficulty": 1,
                "funds": 8647734.0
            }
        });
        let parsed = parse_guild(Some(&guild))
            .expect("解析寮数据")
            .expect("应有寮数据");
        assert_eq!(parsed.id, "69520452fa2353e33ccea79f");
        assert_eq!(parsed.name, "胖虎的小屋");
        assert_eq!(parsed.level, 2);
        assert_eq!(parsed.funds, 8647734.0);
        assert_eq!(parsed.active_member_count, 11);
        assert_eq!(parsed.member_count, 13);
        assert_eq!(parsed.server_id, 15049);
        assert_eq!(parsed.create_time, 1766982738);
        assert_eq!(parsed.leader_name, "捉鼠大师小叮当");
        assert_eq!(parsed.declare, "");
        assert_eq!(parsed.active_rank, 137);
        assert_eq!(parsed.pvp_rank, -1);
        assert_eq!(parsed.active_score, 16644.0);
        assert_eq!(parsed.construction, 13200);
        assert_eq!(parsed.insignia_count, 7);
        assert_eq!(parsed.pvp_season, 9);
        assert_eq!(parsed.max_gve_difficulty, 7);
        assert_eq!(parsed.crevice_difficulty, 2);
        assert_eq!(parsed.crevice_defeated_difficulty, 1);
        assert_eq!(parsed.members.len(), 5);
        assert_eq!(parsed.members[0].name, "捉鼠大师小叮当");
        assert_eq!(parsed.members[0].duty_label, "会长");
        assert_eq!(parsed.members[1].name, "铁血战士胖虎");
        assert_eq!(parsed.members[1].duty_label, "副会长");
        assert_eq!(parsed.members[1].weekly_feats, 744);
        assert_eq!(parsed.members[1].total_feats, 46915);
        assert_eq!(parsed.members[1].history_donate, 208517);
        assert_eq!(parsed.members[1].offline_at, 1787094086);
        assert!(!parsed.members[1].offline_at_label.is_empty());
        assert_eq!(parsed.members[2].duty_label, "普通成员");
        assert_eq!(parsed.members[3].duty_label, "普通成员");
        assert_eq!(parsed.members[4].duty_label, "普通成员");
        assert_eq!(parsed.members[0].joined_at, 1767006667);
        assert!(!parsed.members[0].joined_date.is_empty());
    }

    #[test]
    fn 离线时刻为0应视为当前在线() {
        let guild = json!({
            "id": "g-1",
            "members": [member("m1", 1, "", "在线会长", 60, 1766982738, 0, 10, 20, 30)],
        });
        let parsed = parse_guild(Some(&guild)).expect("解析寮数据").expect("应有寮数据");
        assert_eq!(parsed.members[0].offline_at, 0);
        assert!(parsed.members[0].offline_at_label.is_empty());
    }

    #[test]
    fn 寮内无会长时应回退到extra的建寮者名() {
        let guild = json!({
            "id": "g-1",
            "members": [member("m1", 3, "", "普通成员", 60, 1766982738, 0, 0, 0, 0)],
            "extra": { "creator": "已退寮的建寮者" }
        });
        let parsed = parse_guild(Some(&guild)).expect("解析寮数据").expect("应有寮数据");
        assert_eq!(parsed.leader_name, "已退寮的建寮者");
    }

    #[test]
    fn 未上榜排名应保留负一而非归零() {
        let guild = json!({
            "id": "g-1",
            "activeRank": -1,
            "pvpRank": -1,
            "members": []
        });
        let parsed = parse_guild(Some(&guild)).expect("解析寮数据").expect("应有寮数据");
        assert_eq!(parsed.active_rank, -1);
        assert_eq!(parsed.pvp_rank, -1);
    }

    #[test]
    fn 寮名缺失时应回退到extra字段() {
        let guild = json!({
            "id": "g-1",
            "members": [],
            "extra": { "name": "备选寮名", "member_count": 0 }
        });
        let parsed = parse_guild(Some(&guild)).expect("解析寮数据").expect("应有寮数据");
        assert_eq!(parsed.name, "备选寮名");
        assert_eq!(parsed.member_count, 0);
    }

    #[test]
    fn 无寮数据或空寮字段应返回None() {
        assert!(parse_guild(None).expect("无寮数据").is_none());
        assert!(parse_guild(Some(&Value::Null)).expect("空寮字段").is_none());
    }

    #[test]
    fn 职务编码未知时兜底为普通成员() {
        assert_eq!(guild_duty_label(0), "普通成员");
        assert_eq!(guild_duty_label(1), "会长");
        assert_eq!(guild_duty_label(2), "副会长");
        assert_eq!(guild_duty_label(3), "普通成员");
        assert_eq!(guild_duty_label(4), "普通成员");
        assert_eq!(guild_duty_label(99), "普通成员");
    }
}
