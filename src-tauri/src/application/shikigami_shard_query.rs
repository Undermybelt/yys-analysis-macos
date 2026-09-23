//! 跨游戏档案的式神与碎片查询。
//!
//! 查询只读取角色档案及其 current-data 正文，不改变全局激活角色，也不把原始快照
//! 传给 WebView。单个旧档案缺失或损坏时保留为不可用结果，避免阻断其他角色。

use crate::application::error::AppError;
use crate::infrastructure::character_archives::CharacterArchiveStore;
use crate::infrastructure::current_data::{
    CurrentDataStore, SHIKIGAMI_UNLOCKABLE_SHARD_COUNT, ShikigamiUnlockProgressKind,
    parse_shikigami_shards, parse_shikigami_story_unlock,
};
use serde::Serialize;

/// 单个游戏档案对目标式神的持有与碎片事实。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShikigamiShardLookupCharacter {
    pub identity_key: String,
    pub profile_id: Option<String>,
    pub display_name: String,
    pub platform: Option<String>,
    pub player_id: Option<String>,
    pub server_label: Option<String>,
    pub last_updated_at: Option<String>,
    /// false 表示该档案没有可判断持有状态的式神正文，前端不得把它展示为“未拥有”。
    pub data_available: bool,
    pub warning: Option<String>,
    /// 同一式神可拥有多只，数量按规范化实例列表聚合。
    pub owned_count: u32,
    pub shard_count: u32,
    /// 快照记录的召唤上限；没有对应碎片条目时为 0，由目录稀有度补齐展示值。
    pub max_shard_count: u32,
    /// 是否已经完成传记二；旧档案没有传记进度时为 None，不能误判为未解锁。
    pub shard_unlocked: Option<bool>,
    /// 传记二对应的 Wiki 条件类型；unknown 表示规则表没有覆盖，不能换算黑蛋。
    pub shard_unlock_progress_kind: Option<ShikigamiUnlockProgressKind>,
    /// Wiki 规则表中的传记二条件，例如“升至40级”或“升12次技能”。
    pub shard_unlock_condition: Option<String>,
    /// 规则来源标识，便于界面区分最新整理和旧表。
    pub shard_unlock_rule_source: Option<String>,
    /// 传记二当前进度；仅在读取到新式神传记字段时提供。
    pub shard_unlock_current: Option<u32>,
    /// 传记二所需总进度；仅在读取到新式神传记字段时提供。
    pub shard_unlock_required: Option<u32>,
    /// 距离传记二完成还差的规则目标量；规则未知时保持 None，不伪造单位。
    pub shard_unlock_remaining: Option<u32>,
    /// 未解锁时完成传记二可获得的碎片数量；已解锁或旧档案为 None。
    pub unlockable_shard_count: Option<u32>,
}

/// 一次目标式神查询结果；汇总指标由前端从角色事实派生，避免重复字段发生漂移。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShikigamiShardLookupResult {
    pub shikigami_id: String,
    pub characters: Vec<ShikigamiShardLookupCharacter>,
}

/// 跨全部角色档案查询一个式神；读取期间不触碰 `active_profile`。
pub fn lookup_shikigami_shards(
    archives: &CharacterArchiveStore,
    current_data: &CurrentDataStore,
    shikigami_id: &str,
) -> Result<ShikigamiShardLookupResult, AppError> {
    let target_id = shikigami_id.trim();
    if target_id.is_empty() || target_id.len() > 64 {
        return Err(AppError::invalid_argument(
            "shikigamiId",
            "式神 ID 不能为空且不能超过 64 个字符",
        ));
    }

    let mut characters = Vec::new();
    for archive in archives.list()? {
        let mut row = ShikigamiShardLookupCharacter {
            identity_key: archive.identity_key,
            profile_id: archive.profile_id.clone(),
            display_name: archive.display_name,
            platform: archive.platform,
            player_id: archive.player_id,
            server_label: archive.server_label,
            last_updated_at: Some(archive.last_updated_at),
            data_available: false,
            warning: None,
            owned_count: 0,
            shard_count: 0,
            max_shard_count: 0,
            shard_unlocked: None,
            shard_unlock_progress_kind: None,
            shard_unlock_condition: None,
            shard_unlock_rule_source: None,
            shard_unlock_current: None,
            shard_unlock_required: None,
            shard_unlock_remaining: None,
            unlockable_shard_count: None,
        };

        let Some(profile_id) = archive.profile_id.as_deref() else {
            row.warning = Some("旧角色档案尚未绑定数据，请重新读取该角色".to_owned());
            characters.push(row);
            continue;
        };

        match current_data.read_document_for_profile(profile_id) {
            Ok(document) => {
                // 有式神实例或碎片正文即可证明本次读取包含式神数据；空的旧档案保持未知，
                // 不能把“没有数据”误判成“没有这个式神”。
                let shard_entries = parse_shikigami_shards(&document.source_payload);
                let story_unlock =
                    parse_shikigami_story_unlock(&document.source_payload, target_id);
                row.data_available = archive.shikigami_count > 0
                    || !document.normalized_shikigami.is_empty()
                    || !shard_entries.is_empty()
                    || story_unlock.is_some();
                row.owned_count = document
                    .normalized_shikigami
                    .iter()
                    .filter(|entry| entry.shikigami_id == target_id)
                    .count()
                    .min(u32::MAX as usize) as u32;
                if let Some(shard) = shard_entries
                    .into_iter()
                    .find(|entry| entry.shikigami_id == target_id)
                {
                    row.shard_count = shard.shard_count;
                    row.max_shard_count = shard.max_shard_count;
                }
                if let Some(progress) = story_unlock {
                    let unlocked = progress.current >= progress.required;
                    row.shard_unlocked = Some(unlocked);
                    row.shard_unlock_progress_kind = Some(progress.progress_kind);
                    row.shard_unlock_condition = progress.unlock_condition;
                    row.shard_unlock_rule_source = progress.unlock_rule_source;
                    row.shard_unlock_current = Some(progress.current);
                    row.shard_unlock_required = Some(progress.required);
                    // 只有规则表明确给出条件时才返回剩余目标；未知规则保留 None，避免伪造单位。
                    row.shard_unlock_remaining = (progress.progress_kind
                        != ShikigamiUnlockProgressKind::Unknown)
                        .then_some(progress.required.saturating_sub(progress.current));
                    row.unlockable_shard_count = (!unlocked)
                        .then_some(SHIKIGAMI_UNLOCKABLE_SHARD_COUNT);
                }
                if !row.data_available {
                    row.warning = Some("该档案未读取式神数据，请重新读取并勾选“式神”".to_owned());
                }
            }
            Err(error) => {
                row.warning = Some(format!("角色数据不可用：{}", error.message));
            }
        }
        characters.push(row);
    }

    Ok(ShikigamiShardLookupResult {
        shikigami_id: target_id.to_owned(),
        characters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{OwnedShikigami, OwnedShikigamiSkill};
    use crate::infrastructure::character_archives::StoredCharacterArchive;
    use crate::infrastructure::current_data::{CURRENT_DATA_VERSION, CurrentDataDocument};
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 每个测试使用独立临时目录，避免并行执行时相互覆盖角色正文。
    fn test_directory() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("读取测试时间")
            .as_nanos();
        std::env::temp_dir().join(format!("yys-shard-lookup-{stamp}"))
    }

    /// 构造最小角色档案；profile_id 为空时用于覆盖旧档案降级路径。
    fn archive(identity: &str, profile_id: Option<&str>, name: &str) -> StoredCharacterArchive {
        StoredCharacterArchive {
            identity_key: identity.to_owned(),
            profile_id: profile_id.map(str::to_owned),
            account_id: identity.to_owned(),
            display_name: name.to_owned(),
            platform: Some("android".to_owned()),
            player_id: Some(identity.to_owned()),
            server_label: Some("春之樱".to_owned()),
            last_updated_at: "2026-08-27T00:00:00Z".to_owned(),
            soul_count: 0,
            shikigami_count: u32::from(profile_id.is_some()),
            source_kind: "desktop".to_owned(),
            source_label: "桌面版".to_owned(),
        }
    }

    /// 构造一只目标式神实例，重复调用可验证同类式神数量聚合。
    fn owned(instance_id: &str, shikigami_id: &str) -> OwnedShikigami {
        OwnedShikigami {
            instance_id: instance_id.to_owned(),
            shikigami_id: shikigami_id.to_owned(),
            star: 6,
            level: Some(40),
            exp: None,
            locked: None,
            awakened: Some(true),
            skin_id: None,
            skills: Vec::<OwnedShikigamiSkill>::new(),
            selected_skill_ids: Vec::new(),
        }
    }

    /// 写入单个档案的当前正文；先切换测试锁只影响测试存储，不经过产品激活角色命令。
    fn write_profile(
        store: &CurrentDataStore,
        active_profile: &Arc<Mutex<Option<String>>>,
        profile_id: &str,
        owned_shikigami: Vec<OwnedShikigami>,
        shards: serde_json::Value,
    ) {
        write_profile_with_story(
            store,
            active_profile,
            profile_id,
            owned_shikigami,
            shards,
            json!({}),
        );
    }

    /// 写入带传记进度的角色正文，验证未解锁提示只依赖读取快照而非技能推算。
    fn write_profile_with_story(
        store: &CurrentDataStore,
        active_profile: &Arc<Mutex<Option<String>>>,
        profile_id: &str,
        owned_shikigami: Vec<OwnedShikigami>,
        shards: serde_json::Value,
        story_progress: serde_json::Value,
    ) {
        *active_profile.lock().expect("锁定测试激活角色") = Some(profile_id.to_owned());
        store
            .replace(&CurrentDataDocument {
                version: CURRENT_DATA_VERSION,
                file_name: format!("{profile_id}.json"),
                source_kind: "desktop".to_owned(),
                completeness: "complete".to_owned(),
                imported_at: "2026-08-27T00:00:00Z".to_owned(),
                captured_at: None,
                raw_sha256: "test".to_owned(),
                soul_count: 0,
                normalized_souls: json!([]),
                normalized_shikigami: owned_shikigami,
                normalized_items: json!({}),
                normalized_realm_cards: json!([]),
                source_payload: json!({
                    "heroBookShards": shards,
                    "heroStoryProgress": story_progress,
                }),
            })
            .expect("写入测试角色正文");
    }

    #[test]
    fn 跨档案查询应聚合拥有数量和碎片且不丢弃旧档案() {
        let directory = test_directory();
        fs::create_dir_all(&directory).expect("创建测试目录");
        let archives = CharacterArchiveStore::new(directory.join("character-archives.json"));
        archives
            .upsert(archive("uid-a", Some("profile-a"), "小号甲"))
            .expect("写入角色甲");
        archives
            .upsert(archive("uid-b", Some("profile-b"), "小号乙"))
            .expect("写入角色乙");
        archives
            .upsert(archive("uid-old", None, "旧档案"))
            .expect("写入旧档案");

        let active_profile = Arc::new(Mutex::new(None));
        let current_data = CurrentDataStore::new(directory.clone(), active_profile.clone());
        write_profile(
            &current_data,
            &active_profile,
            "profile-a",
            vec![owned("hero-a1", "398"), owned("hero-a2", "398")],
            json!([[398, 12, 1, 50]]),
        );
        write_profile(
            &current_data,
            &active_profile,
            "profile-b",
            vec![owned("hero-b1", "315")],
            json!([[398, 50, 1, 50]]),
        );

        let result =
            lookup_shikigami_shards(&archives, &current_data, "398").expect("跨档案查询应成功");
        assert_eq!(result.characters.len(), 3);
        let first = result
            .characters
            .iter()
            .find(|entry| entry.identity_key == "uid-a")
            .expect("找到角色甲");
        assert_eq!(first.owned_count, 2);
        assert_eq!(first.shard_count, 12);
        assert_eq!(first.max_shard_count, 50);
        assert!(first.data_available);

        let second = result
            .characters
            .iter()
            .find(|entry| entry.identity_key == "uid-b")
            .expect("找到角色乙");
        assert_eq!(second.owned_count, 0);
        assert_eq!(second.shard_count, 50);

        let legacy = result
            .characters
            .iter()
            .find(|entry| entry.identity_key == "uid-old")
            .expect("旧档案仍应返回");
        assert!(!legacy.data_available);
        assert!(legacy.warning.is_some());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn 跨档案查询应返回未解锁传记进度和可解锁碎片() {
        let directory = test_directory();
        fs::create_dir_all(&directory).expect("创建测试目录");
        let archives = CharacterArchiveStore::new(directory.join("character-archives.json"));
        archives
            .upsert(archive("uid-pending", Some("profile-pending"), "待培养小号"))
            .expect("写入待培养角色");
        let active_profile = Arc::new(Mutex::new(None));
        let current_data = CurrentDataStore::new(directory.clone(), active_profile.clone());
        write_profile_with_story(
            &current_data,
            &active_profile,
            "profile-pending",
            vec![owned("hero-pending", "586")],
            json!([[586, 0, 0, 60]]),
            json!({
                "586": [[1813, [40, 40]], [1814, [8, 12]]],
                "283": [[1111, [40, 40]], [1112, [7, 40]]]
            }),
        );

        let result =
            lookup_shikigami_shards(&archives, &current_data, "586").expect("跨档案查询应成功");
        let row = &result.characters[0];
        assert_eq!(row.owned_count, 1);
        assert_eq!(row.shard_unlocked, Some(false));
        assert_eq!(row.shard_unlock_current, Some(8));
        assert_eq!(row.shard_unlock_required, Some(12));
        assert_eq!(row.shard_unlock_remaining, Some(4));
        assert_eq!(
            row.shard_unlock_progress_kind,
            Some(ShikigamiUnlockProgressKind::SkillUpgrade)
        );
        assert_eq!(row.unlockable_shard_count, Some(10));

        let level_progress = lookup_shikigami_shards(&archives, &current_data, "283")
            .expect("等级条件查询应成功");
        let level_row = &level_progress.characters[0];
        assert_eq!(level_row.shard_unlocked, Some(false));
        assert_eq!(level_row.shard_unlock_current, Some(7));
        assert_eq!(level_row.shard_unlock_required, Some(40));
        assert_eq!(level_row.shard_unlock_remaining, Some(33));
        assert_eq!(
            level_row.shard_unlock_progress_kind,
            Some(ShikigamiUnlockProgressKind::Level)
        );
        assert_eq!(level_row.unlockable_shard_count, Some(10));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn 空式神标识应在读取档案前被拒绝() {
        let directory = test_directory();
        let archives = CharacterArchiveStore::new(directory.join("character-archives.json"));
        let current_data = CurrentDataStore::new(directory, Arc::new(Mutex::new(None)));
        let error = lookup_shikigami_shards(&archives, &current_data, "   ")
            .expect_err("空式神标识必须失败");
        assert_eq!(error.message, "式神 ID 不能为空且不能超过 64 个字符");
    }
}
