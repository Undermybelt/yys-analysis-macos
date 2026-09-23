//! 角色档案持久化存储。
//!
//! 档案列表只由导入写入：每次成功导入新增或覆盖一个角色条目，
//! 重复导入同一角色（按身份键匹配）时更新信息而不是追加新条目。

use crate::application::error::{AppError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// 档案文件的格式版本；升级格式时通过版本号拒绝静默读取不兼容内容。
pub const CHARACTER_ARCHIVE_VERSION: u32 = 1;

/// 一个已导入角色的档案摘要；只保存身份与统计，不保存御魂、式神正文。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredCharacterArchive {
    /// 角色身份键；重复导入同一角色时用该键定位覆盖。
    pub identity_key: String,
    /// 该角色绑定的 SQLite 数据档案 ID；旧版本档案条目没有该字段时按未绑定处理。
    #[serde(default)]
    pub profile_id: Option<String>,
    /// 来源账号 ID；没有稳定账号 ID 时回退为导入文件名。
    pub account_id: String,
    /// 角色昵称；身份缺失时使用“未命名角色”友好标签。
    pub display_name: String,
    /// 角色平台：android 或 ios；旧档案没有该字段时为空。
    #[serde(default)]
    pub platform: Option<String>,
    /// 游戏内玩家 ID（UID）；快照未携带时为空。
    pub player_id: Option<String>,
    /// 区服名；快照未携带时为空。
    pub server_label: Option<String>,
    /// 最后一次成功导入的时间。
    pub last_updated_at: String,
    /// 六星御魂数。
    pub soul_count: u32,
    /// 式神数。
    pub shikigami_count: u32,
    /// 导入来源标记（desktop/cbg/mumu/native 等）。
    pub source_kind: String,
    /// 来源展示名：桌面版 / 藏宝阁 / MuMu 等。
    pub source_label: String,
}

/// 角色档案列表正文。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterArchiveDocument {
    pub version: u32,
    pub characters: Vec<StoredCharacterArchive>,
}

/// 负责原子读写角色档案 JSON；实例本身不缓存正文，避免多个入口看到旧列表。
#[derive(Clone, Debug)]
pub struct CharacterArchiveStore {
    path: PathBuf,
}

impl CharacterArchiveStore {
    /// 创建角色档案存储；目录由应用启动阶段提前创建。
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// 返回当前文件路径，供测试和诊断使用。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读取全部档案摘要，最近导入的排在最前面；文件不存在视为空列表。
    pub fn list(&self) -> Result<Vec<StoredCharacterArchive>, AppError> {
        let document = match self.read_document() {
            Ok(document) => document,
            Err(error) if error.code == ErrorCode::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };        let mut characters = document.characters;
        characters.sort_by(|left, right| right.last_updated_at.cmp(&left.last_updated_at));
        Ok(characters)
    }

    /// 按身份键查找档案条目；文件不存在或未找到时返回 None。
    pub fn find(&self, identity_key: &str) -> Result<Option<StoredCharacterArchive>, AppError> {
        Ok(self
            .list()?
            .into_iter()
            .find(|entry| entry.identity_key == identity_key))
    }

    /// 删除指定身份键的档案条目；返回是否删除了条目。
    pub fn delete(&self, identity_key: &str) -> Result<bool, AppError> {
        let mut document = match self.read_document() {
            Ok(document) => document,
            Err(error) if error.code == ErrorCode::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        let before = document.characters.len();
        document.characters.retain(|entry| entry.identity_key != identity_key);
        if document.characters.len() == before {
            return Ok(false);
        }
        self.write_document(&document)?;
        Ok(true)
    }

    /// 按身份键新增或覆盖一个档案条目；同一角色只保留最新一次导入的信息。
    pub fn upsert(&self, entry: StoredCharacterArchive) -> Result<(), AppError> {
        // 正文缺失视为首次写入；格式损坏时记录警告并重建，避免导入流程被次要档案卡住。
        let mut document = match self.read_document() {
            Ok(document) => document,
            Err(error) if error.code == ErrorCode::NotFound => CharacterArchiveDocument {
                version: CHARACTER_ARCHIVE_VERSION,
                characters: Vec::new(),
            },
            Err(error) => {
                tracing::warn!(error = %error.message, "角色档案正文损坏，重建列表");
                CharacterArchiveDocument {
                    version: CHARACTER_ARCHIVE_VERSION,
                    characters: Vec::new(),
                }
            }
        };
        document.version = CHARACTER_ARCHIVE_VERSION;
        if let Some(existing) = document
            .characters
            .iter_mut()
            .find(|item| item.identity_key == entry.identity_key)
        {
            *existing = entry;
        } else {
            document.characters.push(entry);
        }
        self.write_document(&document)
    }

    /// 清空全部档案；文件不存在视为已经清空，返回删除的条目数。
    pub fn clear(&self) -> Result<u32, AppError> {
        let count = self.list()?.len() as u32;
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(count),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(count),
            Err(error) => Err(AppError::io("清空角色档案", &error)),
        }
    }

    /// 读取档案正文；文件不存在时返回 NotFound，格式损坏时返回明确错误。
    fn read_document(&self) -> Result<CharacterArchiveDocument, AppError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(AppError::not_found("角色档案", "尚未创建"));
            }
            Err(error) => return Err(AppError::io("读取角色档案", &error)),
        };
        let document: CharacterArchiveDocument =
            serde_json::from_slice(&bytes).map_err(|error| {
                AppError::invalid_argument(
                    "characterArchives",
                    format!("角色档案文件格式错误：{error}"),
                )
            })?;
        if document.version != CHARACTER_ARCHIVE_VERSION {
            return Err(AppError::invalid_argument(
                "characterArchives.version",
                format!("不支持的角色档案版本：{}", document.version),
            ));
        }
        Ok(document)
    }

    /// 将完整正文写入临时文件，再替换正式文件；写入失败不会触碰旧档案。
    fn write_document(&self, document: &CharacterArchiveDocument) -> Result<(), AppError> {
        let parent = self.path.parent().ok_or_else(|| {
            AppError::invalid_argument("characterArchivesPath", "角色档案路径没有父目录")
        })?;
        fs::create_dir_all(parent).map_err(|error| AppError::io("创建角色档案目录", &error))?;
        let temporary_path = self.path.with_extension("json.tmp");
        let encoded = serde_json::to_vec_pretty(document)
            .map_err(|error| AppError::internal(format!("序列化角色档案失败：{error}")))?;
        fs::write(&temporary_path, encoded)
            .map_err(|error| AppError::io("写入角色档案临时文件", &error))?;

        // Windows 对目标已存在的 rename 行为与 Unix 不同；先移除旧文件，再快速替换。
        if let Err(error) = replace_archive_file(&temporary_path, &self.path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }
        Ok(())
    }
}

/// 在不同平台完成临时文件替换；旧文件只在新文件已经完整写入后才会被替换。
fn replace_archive_file(temporary_path: &Path, target_path: &Path) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        let restore_path = target_path.with_extension("json.previous");
        if restore_path.exists() {
            fs::remove_file(&restore_path)
                .map_err(|error| AppError::io("清理角色档案恢复文件", &error))?;
        }
        if target_path.exists() {
            fs::copy(target_path, &restore_path)
                .map_err(|error| AppError::io("保存角色档案恢复文件", &error))?;
            fs::remove_file(target_path)
                .map_err(|error| AppError::io("替换角色档案旧文件", &error))?;
        }
        return match fs::rename(temporary_path, target_path) {
            Ok(()) => {
                let _ = fs::remove_file(restore_path);
                Ok(())
            }
            Err(error) => {
                if restore_path.exists() {
                    let _ = fs::rename(&restore_path, target_path);
                }
                Err(AppError::io("提交角色档案文件", &error))
            }
        };
    }

    #[cfg(not(windows))]
    fs::rename(temporary_path, target_path)
        .map_err(|error| AppError::io("提交角色档案文件", &error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_path() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("读取测试时间")
            .as_nanos();
        std::env::temp_dir().join(format!("yys-character-archives-{stamp}.json"))
    }

    fn entry(identity_key: &str, name: &str) -> StoredCharacterArchive {
        StoredCharacterArchive {
            identity_key: identity_key.to_owned(),
            profile_id: Some("profile-1".to_owned()),
            account_id: "account-1".to_owned(),
            display_name: name.to_owned(),
            platform: None,
            player_id: None,
            server_label: None,
            last_updated_at: "2026-08-19T00:00:00Z".to_owned(),
            soul_count: 42,
            shikigami_count: 7,
            source_kind: "desktop".to_owned(),
            source_label: "桌面版".to_owned(),
        }
    }

    #[test]
    fn 空文件返回空档案列表() {
        let path = test_path();
        let store = CharacterArchiveStore::new(path.clone());
        assert!(store.list().expect("读取空档案").is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn 重复导入同一角色应覆盖而不是追加() {
        let path = test_path();
        let store = CharacterArchiveStore::new(path.clone());
        store
            .upsert(entry("uid:1001", "角色A"))
            .expect("写入第一条");
        let mut updated = entry("uid:1001", "角色A");
        updated.soul_count = 99;
        updated.last_updated_at = "2026-08-19T01:00:00Z".to_owned();
        store.upsert(updated).expect("覆盖同一条");
        let mut second = entry("uid:1002", "角色B");
        second.last_updated_at = "2026-08-19T02:00:00Z".to_owned();
        store.upsert(second).expect("写入另一条");
        let characters = store.list().expect("读取覆盖结果");
        assert_eq!(characters.len(), 2, "同角色覆盖不应新增条目");
        let first = characters
            .iter()
            .find(|item| item.identity_key == "uid:1001")
            .expect("找到角色A");
        assert_eq!(first.soul_count, 99, "覆盖后应保留最新信息");
        assert_eq!(characters[0].identity_key, "uid:1002", "最近导入排在最前");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn 清空档案应返回清除数量并支持重复清空() {
        let path = test_path();
        let store = CharacterArchiveStore::new(path.clone());
        store.upsert(entry("uid:1001", "角色A")).expect("写入");
        store.upsert(entry("uid:1002", "角色B")).expect("写入");
        assert_eq!(store.clear().expect("清空"), 2);
        assert_eq!(store.clear().expect("重复清空"), 0);
        assert!(store.list().expect("读取清空结果").is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn 按身份键查找与删除档案条目() {
        let path = test_path();
        let store = CharacterArchiveStore::new(path.clone());
        assert!(store.find("uid:1001").expect("空档案查找").is_none());
        store.upsert(entry("uid:1001", "角色A")).expect("写入");
        let found = store.find("uid:1001").expect("查找已有条目");
        assert_eq!(
            found.and_then(|entry| entry.profile_id).as_deref(),
            Some("profile-1"),
            "条目应携带绑定的档案 ID"
        );
        assert!(store.find("uid:9999").expect("查找缺失条目").is_none());
        assert!(store.delete("uid:1001").expect("删除条目"));
        assert!(!store.delete("uid:1001").expect("重复删除"), "重复删除应返回 false");
        assert!(store.list().expect("删除后列表").is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn 旧档案缺少平台字段时按空值兼容读取() {
        let legacy = r#"{
            "identityKey":"uid:1001",
            "profileId":null,
            "accountId":"account-1",
            "displayName":"角色A",
            "playerId":null,
            "serverLabel":null,
            "lastUpdatedAt":"2026-08-19T00:00:00Z",
            "soulCount":0,
            "shikigamiCount":0,
            "sourceKind":"desktop",
            "sourceLabel":"桌面版"
        }"#;
        let entry: StoredCharacterArchive = serde_json::from_str(legacy).expect("旧档案应可读取");
        assert_eq!(entry.platform, None);
    }
}
