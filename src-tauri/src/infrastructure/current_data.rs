//! 按角色档案隔离的当前数据文件：每个角色一份 current-data-{profileId}.json，
//! 导入成功后整体替换对应角色的正文。实例不保存快照时间线或重复导入历史。

use crate::application::error::AppError;
use crate::domain::{
    OwnedShikigami, ShikigamiBagGroup, ShikigamiShard, PLATFORM_ANDROID, PLATFORM_IOS,
};
use crate::infrastructure::shikigami_biography_rules::{
    ShikigamiBiographyUnlockKind, biography_rule_for,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 当前数据文件的格式版本；升级格式时通过版本号拒绝静默读取不兼容内容。
pub const CURRENT_DATA_VERSION: u32 = 1;

/// 前端当前数据卡片需要的最小摘要。
// 当前数据摘要直接服务于现有前端读取契约；字段名保持 snake_case，避免导入成功后页面读不到数量。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CurrentDataSummary {
    pub exists: bool,
    pub file_name: Option<String>,
    pub source_kind: Option<String>,
    pub completeness: Option<String>,
    pub imported_at: Option<String>,
    pub soul_count: u32,
    pub shikigami_count: u32,
    /// 非 0 的资源/道具项数。
    pub item_count: u32,
    /// 结界卡张数。
    pub realm_card_count: u32,
    /// 寮名称；未读取到寮数据时为 None。
    pub guild_name: Option<String>,
    /// 寮成员数；未读取到寮数据时为 0。
    pub guild_member_count: u32,
    /// 玩家昵称；快照未携带时为 None。
    pub account_name: Option<String>,
    /// 区服名；快照未携带时为 None。
    pub server_name: Option<String>,
    /// 玩家短 ID（UID）；快照未携带时为 None。
    pub short_id: Option<String>,
    /// 玩家平台；桌面版内存读取从本地 clientconfig 补齐，其他来源按快照值读取。
    pub platform: Option<String>,
}

/// 式神传记 2 的解锁进度；进度来自读取快照中的 activityId，不根据技能等级反推。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShikigamiStoryUnlockProgress {
    pub activity_id: String,
    pub current: u32,
    pub required: u32,
    pub progress_kind: ShikigamiUnlockProgressKind,
    /// 公开规则表中的传记二条件；未收录或分母不一致时为空。
    pub unlock_condition: Option<String>,
    /// 条件来源标识，供界面提示规则时效和可信度。
    pub unlock_rule_source: Option<String>,
}

/// 传记二进度对应的公开条件类型；未知类型禁止换算御行达摩。
pub type ShikigamiUnlockProgressKind = ShikigamiBiographyUnlockKind;

/// 式神录中的单条传记进度；前 3 项分别对应传记一、二、三。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShikigamiBiographyProgress {
    pub index: u8,
    pub activity_id: String,
    pub current: u32,
    pub required: u32,
    pub remaining: u32,
    pub completed: bool,
}

/// 传给式神录的传记解锁事实；状态由已保存的传记进度直接计算，不推断技能培养。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShikigamiStoryProgress {
    pub shikigami_id: String,
    pub activity_id: String,
    pub current: u32,
    pub required: u32,
    /// 原始剩余进度；规则类型决定该数值的单位，未知规则时不能换算御行达摩。
    pub remaining: u32,
    pub unlocked: bool,
    /// 传记二对应的公开规则类型；未知类型不能直接换算黑蛋。
    pub progress_kind: ShikigamiUnlockProgressKind,
    /// 公开规则表中的传记二条件；未收录或分母不一致时为空。
    pub unlock_condition: Option<String>,
    /// 条件来源标识，供前端说明规则时效。
    pub unlock_rule_source: Option<String>,
    /// 当前规则下完成传记二可提供的碎片数；已解锁时为空，避免重复计算。
    pub unlockable_shard_count: Option<u32>,
    /// 该式神已读取到的全部传记进度，避免界面只展示传记二一项。
    pub biographies: Vec<ShikigamiBiographyProgress>,
}

/// 完成传记二后可解锁的碎片数量；与跨角色碎片查询共用同一规则常量。
pub(crate) const SHIKIGAMI_UNLOCKABLE_SHARD_COUNT: u32 = 10;

/// 单份数据文件正文；保留规范化数据和原始 JSON，便于离线排查且不再生成多个对象文件。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentDataDocument {
    pub version: u32,
    pub file_name: String,
    pub source_kind: String,
    pub completeness: String,
    pub imported_at: String,
    pub captured_at: Option<String>,
    pub raw_sha256: String,
    pub soul_count: u32,
    pub normalized_souls: Value,
    /// 归一化的玩家式神列表；旧版本 current-data 没有该字段时按空列表兼容读取。
    #[serde(default)]
    pub normalized_shikigami: Vec<OwnedShikigami>,
    /// 归一化的资源/道具数量（21 项键值对象）；旧版本 current-data 没有该字段时按空对象兼容读取。
    #[serde(default)]
    pub normalized_items: Value,
    /// 归一化的结界卡列表；旧版本 current-data 没有该字段时按空数组兼容读取。
    #[serde(default)]
    pub normalized_realm_cards: Value,
    pub source_payload: Value,
}

/// 兼容字符串和数字的档案文本字段读取；其他类型按缺失处理。
fn current_data_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

/// 式神碎片的游戏内稀有度边界；411 是没有常规上限的御行达摩。
const SSR_SHARD_CAP: u64 = 50;
const SP_UR_SHARD_CAP: u64 = 60;
const YUKO_DARUMA_ID: &str = "411";

/// 从兼容字符串/数字的 JSON 字段读取非负数量，并限制到 DTO 可表达的范围。
fn current_data_count(value: Option<&Value>) -> u32 {
    value
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(u32::MAX as u64) as u32
}

/// 从快照的 `heroBookShards` 解析目标式神碎片；旧脚本的全量碎片在这里再次收敛。
pub(crate) fn parse_shikigami_shards(payload: &Value) -> Vec<ShikigamiShard> {
    let Some(entries) = payload
        .get("heroBookShards")
        .or_else(|| payload.get("hero_book_shards"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    entries
        .iter()
        .filter_map(|entry| {
            let fields = entry.as_array()?;
            let shikigami_id = fields.first().and_then(current_data_text)?;
            let max_shard_count = current_data_count(fields.get(3));
            let is_target = shikigami_id == YUKO_DARUMA_ID
                || matches!(max_shard_count as u64, SSR_SHARD_CAP | SP_UR_SHARD_CAP);
            if !is_target {
                return None;
            }
            Some(ShikigamiShard {
                shikigami_id,
                shard_count: current_data_count(fields.get(1)),
                max_shard_count,
            })
        })
        .collect()
}

/// 从按式神保存的 activityId 进度中读取前三项传记；缺少新字段的旧档案返回 None。
fn parse_shikigami_biographies(
    payload: &Value,
    shikigami_id: &str,
) -> Option<Vec<ShikigamiBiographyProgress>> {
    let entries = payload
        .get("heroStoryProgress")
        .or_else(|| payload.get("hero_story_progress"))
        .and_then(|value| value.get(shikigami_id))
        .and_then(Value::as_array)?;
    // 游戏 DATA_STORY.activityId 的前三项固定对应传记一、二、三，后续项目属于特殊任务。
    let biographies = entries
        .iter()
        .enumerate()
        .take(3)
        .filter_map(|(index, entry)| {
            let fields = entry.as_array()?;
            let activity_id = fields.first().and_then(current_data_text)?;
            let progress = fields.get(1)?.as_array()?;
            let current = current_data_count(progress.first());
            let required = current_data_count(progress.get(1));
            if required == 0 {
                return None;
            }
            let completed = current >= required;
            Some(ShikigamiBiographyProgress {
                index: (index + 1) as u8,
                activity_id,
                current,
                required,
                remaining: required.saturating_sub(current),
                completed,
            })
        })
        .collect::<Vec<_>>();
    (!biographies.is_empty()).then_some(biographies)
}

/// 从按式神保存的 activityId 进度中读取传记 2；缺少新字段的旧档案返回 None。
pub(crate) fn parse_shikigami_story_unlock(
    payload: &Value,
    shikigami_id: &str,
) -> Option<ShikigamiStoryUnlockProgress> {
    let biography_two = parse_shikigami_biographies(payload, shikigami_id)?
        .into_iter()
        .find(|entry| entry.index == 2)?;
    let rule = biography_rule_for(shikigami_id, biography_two.required);
    Some(ShikigamiStoryUnlockProgress {
        activity_id: biography_two.activity_id,
        current: biography_two.current,
        required: biography_two.required,
        progress_kind: rule
            .as_ref()
            .map(|entry| entry.kind)
            .unwrap_or(ShikigamiUnlockProgressKind::Unknown),
        unlock_condition: rule.as_ref().map(|entry| entry.condition.clone()),
        unlock_rule_source: rule.map(|entry| entry.source),
    })
}

/// 从快照的 `heroesBagEntries` 解析素材式神分组。
///
/// 上游键形如 `413_2_1_0`：首段是 heroId，其余为分组键。游戏会保留数量为 0 的
/// 历史分组，这里丢弃 0 项，只返回真正持有的分组。
fn parse_shikigami_bag(payload: &Value) -> Vec<ShikigamiBagGroup> {
    let Some(entries) = payload.get("heroesBagEntries").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut groups = Vec::new();
    for entry in entries {
        let Some(pair) = entry.as_array() else { continue };
        let Some(key) = pair.first().and_then(Value::as_str) else {
            continue;
        };
        let count = pair.get(1).and_then(Value::as_u64).unwrap_or(0) as u32;
        if count == 0 {
            continue;
        }
        let Some((shikigami_id, group_key)) = key.split_once('_') else {
            continue;
        };
        if shikigami_id.is_empty() {
            continue;
        }
        groups.push(ShikigamiBagGroup {
            shikigami_id: shikigami_id.to_owned(),
            group_key: group_key.to_owned(),
            count,
        });
    }
    groups
}

/// 负责按当前激活角色读取和原子替换角色数据文件；实例本身不缓存正文，
/// 避免多个入口看到旧数据。没有激活角色时所有读取返回“无数据”语义。
#[derive(Clone, Debug)]
pub struct CurrentDataStore {
    directory: PathBuf,
    /// 当前激活角色绑定的数据档案 ID；与应用全局激活状态共享同一把锁。
    active_profile: Arc<Mutex<Option<String>>>,
}

impl CurrentDataStore {
    /// 创建当前数据文件存储；目录由应用启动阶段提前创建。
    pub fn new(directory: PathBuf, active_profile: Arc<Mutex<Option<String>>>) -> Self {
        Self {
            directory,
            active_profile,
        }
    }

    /// 当前激活角色的数据文件路径；没有激活角色时返回 None。
    fn active_path(&self) -> Option<PathBuf> {
        self.active_profile
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|profile_id| self.path_for(profile_id))
    }

    /// 指定角色的数据文件路径，供测试和诊断使用。
    pub fn path_for(&self, profile_id: &str) -> PathBuf {
        self.directory.join(format!("current-data-{profile_id}.json"))
    }

    /// 返回当前激活角色的数据文件路径，供测试和诊断使用。
    pub fn path(&self) -> PathBuf {
        self.active_path()
            .unwrap_or_else(|| self.directory.join("current-data-.json"))
    }

    /// 读取当前摘要；文件不存在表示该角色尚未导入，不视为错误。
    pub fn summary(&self) -> Result<CurrentDataSummary, AppError> {
        let Some(path) = self.active_path() else {
            return Ok(CurrentDataSummary {
                exists: false,
                file_name: None,
                source_kind: None,
                completeness: None,
                imported_at: None,
                soul_count: 0,
                shikigami_count: 0,
                item_count: 0,
                realm_card_count: 0,
                guild_name: None,
                guild_member_count: 0,
                account_name: None,
                server_name: None,
                short_id: None,
                platform: None,
            });
        };
        if !path.is_file() {
            return Ok(CurrentDataSummary {
                exists: false,
                file_name: None,
                source_kind: None,
                completeness: None,
                imported_at: None,
                soul_count: 0,
                shikigami_count: 0,
                item_count: 0,
                realm_card_count: 0,
                guild_name: None,
                guild_member_count: 0,
                account_name: None,
                server_name: None,
                short_id: None,
                platform: None,
            });
        }
        let document = self.read_document_at(&path)?;
        let item_count = document
            .normalized_items
            .as_object()
            .map(|items| items.values().filter(|value| value.as_u64().unwrap_or(0) > 0).count() as u32)
            .unwrap_or(0);
        let realm_card_count = document
            .normalized_realm_cards
            .as_array()
            .map(|cards| cards.len() as u32)
            .unwrap_or(0);
        let guild = document.source_payload.get("guild");
        let guild_name = guild
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let guild_member_count = guild
            .and_then(|value| value.get("members"))
            .and_then(Value::as_array)
            .map(|members| members.len() as u32)
            .unwrap_or(0);
        let player = document.source_payload.get("player");
        let player_field = |name: &str| player.and_then(|value| value.get(name));
        let account_name = player_field("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                document
                    .source_payload
                    .get("accountName")
                    .and_then(current_data_text)
            });
        let server_name = player_field("serverName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                document
                    .source_payload
                    .get("serverName")
                    .and_then(current_data_text)
            });
        let short_id = player_field("shortId").and_then(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_u64().map(|value| value.to_string()))
        });
        // 平台只接受内部标准值，避免把来源中的任意文本直接显示成平台。
        let platform = player_field("platform")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .filter(|value| matches!(value.as_str(), PLATFORM_ANDROID | PLATFORM_IOS));
        // 式神总数 = 实例数 + 折叠分组总量（游戏式神录顶部口径）。
        let heroes_bag_count = document
            .source_payload
            .get("heroesBagCount")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        Ok(CurrentDataSummary {
            exists: true,
            file_name: Some(document.file_name),
            source_kind: Some(document.source_kind),
            completeness: Some(document.completeness),
            imported_at: Some(document.imported_at),
            soul_count: document.soul_count,
            shikigami_count: document.normalized_shikigami.len() as u32 + heroes_bag_count,
            item_count,
            realm_card_count,
            guild_name,
            guild_member_count,
            account_name,
            server_name,
            short_id,
            platform,
        })
    }

    /// 读取当前导入的玩家式神；无激活角色或尚未导入时返回空列表。
    pub fn list_owned_shikigami(&self) -> Result<Vec<OwnedShikigami>, AppError> {
        let Some(path) = self.active_path() else {
            return Ok(Vec::new());
        };
        if !path.is_file() {
            return Ok(Vec::new());
        }
        Ok(self.read_document_at(&path)?.normalized_shikigami)
    }

    /// 读取式神仓库中的素材式神分组；旧快照没有该字段或数量为 0 时返回空列表。
    pub fn list_shikigami_bag(&self) -> Result<Vec<ShikigamiBagGroup>, AppError> {
        let Some(path) = self.active_path() else {
            return Ok(Vec::new());
        };
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let document = self.read_document_at(&path)?;
        Ok(parse_shikigami_bag(&document.source_payload))
    }

    /// 读取当前导入的目标式神碎片；旧快照中的低稀有度碎片不会越过接口边界。
    pub fn list_shikigami_shards(&self) -> Result<Vec<ShikigamiShard>, AppError> {
        let Some(path) = self.active_path() else {
            return Ok(Vec::new());
        };
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let document = self.read_document_at(&path)?;
        Ok(parse_shikigami_shards(&document.source_payload))
    }

    /// 读取当前账号按式神归一化的全部传记进度；旧快照缺少字段时返回空列表。
    pub fn list_shikigami_story_progress(&self) -> Result<Vec<ShikigamiStoryProgress>, AppError> {
        let Some(path) = self.active_path() else {
            return Ok(Vec::new());
        };
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let document = self.read_document_at(&path)?;
        let Some(progress_map) = document
            .source_payload
            .get("heroStoryProgress")
            .or_else(|| document.source_payload.get("hero_story_progress"))
            .and_then(Value::as_object)
        else {
            return Ok(Vec::new());
        };
        Ok(progress_map
            .keys()
            .filter_map(|shikigami_id| {
                let biographies = parse_shikigami_biographies(
                    &document.source_payload,
                    shikigami_id,
                )?;
                let progress = parse_shikigami_story_unlock(
                    &document.source_payload,
                    shikigami_id,
                )?;
                let unlocked = progress.current >= progress.required;
                Some(ShikigamiStoryProgress {
                    shikigami_id: shikigami_id.clone(),
                    activity_id: progress.activity_id,
                    current: progress.current,
                    required: progress.required,
                    remaining: progress.required.saturating_sub(progress.current),
                    unlocked,
                    progress_kind: progress.progress_kind,
                    unlock_condition: progress.unlock_condition,
                    unlock_rule_source: progress.unlock_rule_source,
                    unlockable_shard_count: (!unlocked)
                        .then_some(SHIKIGAMI_UNLOCKABLE_SHARD_COUNT),
                    biographies,
                })
            })
            .collect())
    }

    /// 读取当前导入的资源/道具数量（21 项键值对象）；无激活角色时返回空对象。
    pub fn list_items(&self) -> Result<Value, AppError> {
        let Some(path) = self.active_path() else {
            return Ok(Value::Object(serde_json::Map::new()));
        };
        if !path.is_file() {
            return Ok(Value::Object(serde_json::Map::new()));
        }
        Ok(self.read_document_at(&path)?.normalized_items)
    }

    /// 读取当前导入的结界卡列表；无激活角色时返回空数组。
    pub fn list_realm_cards(&self) -> Result<Value, AppError> {
        let Some(path) = self.active_path() else {
            return Ok(Value::Array(Vec::new()));
        };
        if !path.is_file() {
            return Ok(Value::Array(Vec::new()));
        }
        Ok(self.read_document_at(&path)?.normalized_realm_cards)
    }

    /// 读取当前导入快照中的寮原始数据；无激活角色或快照未携带寮数据时返回 None。
    pub fn guild(&self) -> Result<Option<Value>, AppError> {
        let Some(path) = self.active_path() else {
            return Ok(None);
        };
        if !path.is_file() {
            return Ok(None);
        }
        Ok(self
            .read_document_at(&path)?
            .source_payload
            .get("guild")
            .filter(|value| !value.is_null())
            .cloned())
    }

    /// 读取当前激活角色的正文，格式损坏时返回明确错误并阻止覆盖操作继续。
    pub fn read_document(&self) -> Result<CurrentDataDocument, AppError> {
        let Some(path) = self.active_path() else {
            return Err(AppError::not_found("当前角色数据", "尚未激活任何角色"));
        };
        self.read_document_at(&path)
    }

    /// 读取指定角色的完整当前数据正文；导出场景不能改变用户当前激活的角色。
    pub fn read_document_for_profile(
        &self,
        profile_id: &str,
    ) -> Result<CurrentDataDocument, AppError> {
        let path = self.path_for(profile_id);
        if !path.is_file() {
            return Err(AppError::not_found("当前角色数据", profile_id));
        }
        self.read_document_at(&path)
    }

    fn read_document_at(&self, path: &Path) -> Result<CurrentDataDocument, AppError> {
        let bytes = fs::read(path).map_err(|error| AppError::io("读取当前数据", &error))?;
        let document: CurrentDataDocument = serde_json::from_slice(&bytes).map_err(|error| {
            AppError::invalid_argument("currentData", format!("当前数据文件格式错误：{error}"))
        })?;
        if document.version != CURRENT_DATA_VERSION {
            return Err(AppError::invalid_argument(
                "currentData.version",
                format!("不支持的当前数据版本：{}", document.version),
            ));
        }
        Ok(document)
    }

    /// 将完整候选正文写入当前激活角色的临时文件，再替换正式文件；写入失败不会触碰旧正文。
    pub fn replace(&self, document: &CurrentDataDocument) -> Result<(), AppError> {
        let path = self.active_path().ok_or_else(|| {
            AppError::invalid_argument("activeProfile", "尚未激活任何角色，无法写入当前数据")
        })?;
        let parent = path.parent().ok_or_else(|| {
            AppError::invalid_argument("currentDataPath", "当前数据路径没有父目录")
        })?;
        fs::create_dir_all(parent).map_err(|error| AppError::io("创建当前数据目录", &error))?;
        let temporary_path = path.with_extension("json.tmp");
        let encoded = serde_json::to_vec_pretty(document)
            .map_err(|error| AppError::internal(format!("序列化当前数据失败：{error}")))?;
        fs::write(&temporary_path, encoded)
            .map_err(|error| AppError::io("写入当前数据临时文件", &error))?;

        // Windows 对目标已存在的 rename 行为与 Unix 不同；先移除旧文件，再快速替换。
        if let Err(error) = replace_file(&temporary_path, &path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }
        Ok(())
    }

    /// 删除当前激活角色的数据正文；文件不存在视为已经清空，避免重复点击产生无意义错误。
    pub fn clear(&self) -> Result<(), AppError> {
        let Some(path) = self.active_path() else {
            return Ok(());
        };
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::io("清空当前数据", &error)),
        }
    }

    /// 删除指定角色的数据正文；清空角色档案时按档案逐个清理对应文件。
    pub fn clear_profile(&self, profile_id: &str) -> Result<(), AppError> {
        let path = self.path_for(profile_id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::io("清空角色数据", &error)),
        }
    }
}

/// 在不同平台完成临时文件替换；旧文件只在新文件已经完整写入后才会被替换。
fn replace_file(temporary_path: &Path, target_path: &Path) -> Result<(), AppError> {
    // Unix 的 rename 在同一目录内具备原子替换语义；Windows 目标文件不能直接被 rename 覆盖，
    // 因而先留存旧文件、再替换临时文件，移动失败时立即恢复旧文件，保证失败路径不丢当前数据。
    #[cfg(windows)]
    {
        let restore_path = target_path.with_extension("json.previous");
        if restore_path.exists() {
            fs::remove_file(&restore_path)
                .map_err(|error| AppError::io("清理当前数据恢复文件", &error))?;
        }
        if target_path.exists() {
            fs::copy(target_path, &restore_path)
                .map_err(|error| AppError::io("保存当前数据恢复文件", &error))?;
            fs::remove_file(target_path)
                .map_err(|error| AppError::io("替换当前数据旧文件", &error))?;
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
                Err(AppError::io("提交当前数据文件", &error))
            }
        };
    }

    #[cfg(not(windows))]
    fs::rename(temporary_path, target_path)
        .map_err(|error| AppError::io("提交当前数据文件", &error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_path() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("读取测试时间")
            .as_nanos();
        std::env::temp_dir().join(format!("yys-current-data-{stamp}"))
    }

    fn active(profile_id: &str) -> Arc<Mutex<Option<String>>> {
        Arc::new(Mutex::new(Some(profile_id.to_owned())))
    }

    fn document(file_name: &str, soul_count: u32) -> CurrentDataDocument {
        CurrentDataDocument {
            version: CURRENT_DATA_VERSION,
            file_name: file_name.to_owned(),
            source_kind: "importer".to_owned(),
            completeness: "complete".to_owned(),
            imported_at: "2026-08-11T00:00:00Z".to_owned(),
            captured_at: None,
            raw_sha256: "test-hash".to_owned(),
            soul_count,
            normalized_souls: Value::Array(Vec::new()),
            normalized_shikigami: Vec::new(),
            normalized_items: Value::Object(serde_json::Map::new()),
            normalized_realm_cards: Value::Array(Vec::new()),
            source_payload: Value::Object(serde_json::Map::new()),
        }
    }

    #[test]
    fn 空文件返回无数据摘要() {
        let path = test_path();
        let store = CurrentDataStore::new(path.clone(), active("profile-1"));
        let summary = store.summary().expect("读取空摘要");
        assert!(!summary.exists);
        assert_eq!(summary.soul_count, 0);
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn 当前数据摘要序列化字段应与前端契约一致() {
        // 前端现有页面按 snake_case 读取摘要；这里锁定 Tauri 边界字段名，避免数据已写入但展示层拿到 undefined。
        let summary = CurrentDataSummary {
            exists: true,
            file_name: Some("current.json".to_owned()),
            source_kind: Some("mumu".to_owned()),
            completeness: Some("complete".to_owned()),
            imported_at: Some("2026-08-21T00:00:00Z".to_owned()),
            soul_count: 123,
            shikigami_count: 456,
            item_count: 7,
            realm_card_count: 8,
            guild_name: Some("测试寮".to_owned()),
            guild_member_count: 9,
            account_name: Some("测试角色".to_owned()),
            server_name: Some("测试服".to_owned()),
            short_id: Some("10001".to_owned()),
            platform: Some(PLATFORM_ANDROID.to_owned()),
        };
        let encoded = serde_json::to_value(summary).expect("摘要应可序列化");

        assert_eq!(encoded["soul_count"], 123);
        assert_eq!(encoded["shikigami_count"], 456);
        assert_eq!(encoded["item_count"], 7);
        assert_eq!(encoded["platform"], PLATFORM_ANDROID);
        assert!(encoded.get("soulCount").is_none());
    }

    #[test]
    fn 没有激活角色时所有读取按空数据处理() {
        let path = test_path();
        let store = CurrentDataStore::new(path.clone(), Arc::new(Mutex::new(None)));
        assert!(!store.summary().expect("读取空摘要").exists);
        assert!(store.list_owned_shikigami().expect("读取式神").is_empty());
        assert!(store
            .list_items()
            .expect("读取道具")
            .as_object()
            .expect("道具对象")
            .is_empty());
        assert!(store.guild().expect("读取寮").is_none());
        assert!(store.replace(&document("a.json", 2)).is_err(), "无激活角色不允许写入");
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn 新数据替换旧数据且只保留一份正文() {
        let path = test_path();
        let handle = active("profile-1");
        let store = CurrentDataStore::new(path.clone(), handle.clone());
        store.replace(&document("a.json", 2)).expect("写入第一份");
        store.replace(&document("b.json", 5)).expect("覆盖第二份");
        let summary = store.summary().expect("读取覆盖结果");
        assert_eq!(summary.file_name.as_deref(), Some("b.json"));
        assert_eq!(summary.soul_count, 5);
        assert!(!store.path().with_extension("json.tmp").exists());
        assert!(store.path_for("profile-1").is_file());
        assert!(!store.path_for("profile-2").exists(), "其他角色不应共享正文");
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn 切换激活角色后读取对应角色的正文() {
        let path = test_path();
        let handle = active("profile-1");
        let store = CurrentDataStore::new(path.clone(), handle.clone());
        store.replace(&document("a.json", 2)).expect("写入角色A");
        *handle.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some("profile-2".to_owned());
        assert!(!store.summary().expect("读取角色B摘要").exists, "角色B还没有数据");
        store.replace(&document("c.json", 9)).expect("写入角色B");
        let summary = store.summary().expect("读取角色B摘要");
        assert_eq!(summary.file_name.as_deref(), Some("c.json"));
        assert_eq!(summary.soul_count, 9);
        assert!(store.path_for("profile-1").is_file(), "角色A正文应保留");
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn 旧版本正文缺少式神字段时按空列表兼容读取() {
        let path = test_path();
        let store = CurrentDataStore::new(path.clone(), active("profile-1"));
        let old_document = serde_json::json!({
            "version": CURRENT_DATA_VERSION,
            "fileName": "legacy.json",
            "sourceKind": "importer",
            "completeness": "partial",
            "importedAt": "2026-08-11T00:00:00Z",
            "capturedAt": null,
            "rawSha256": "legacy-hash",
            "soulCount": 1,
            "normalizedSouls": [],
            "sourcePayload": {}
        });
        fs::create_dir_all(&path).expect("创建测试目录");
        fs::write(
            &store.path_for("profile-1"),
            serde_json::to_vec(&old_document).expect("编码旧正文"),
        )
        .expect("写入旧正文");

        let document = store.read_document().expect("旧正文应可兼容读取");
        assert!(document.normalized_shikigami.is_empty());
        assert_eq!(store.list_owned_shikigami().expect("读取式神列表").len(), 0);
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn 摘要式神数应加上折叠分组总量() {
        let path = test_path();
        let store = CurrentDataStore::new(path.clone(), active("profile-1"));
        let mut doc = document("bag.json", 1);
        doc.source_payload =
            serde_json::json!({ "heroesBagCount": 5466 });
        store.replace(&doc).expect("写入正文");
        let summary = store.summary().expect("读取摘要");
        assert_eq!(summary.shikigami_count, 5466);
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn 式神碎片只保留目标稀有度和御行达摩() {
        let payload = serde_json::json!({
            "heroBookShards": [
                [398, 12, 1, 50],
                [315, 0, 0, 60],
                [593, 1, 0, 60],
                [411, 16, 0, 25],
                [200, 34, 0, 40],
                [410, 5, 0, 25],
                [499, 0, 0, 99999],
            ]
        });
        let shards = parse_shikigami_shards(&payload);
        assert_eq!(shards.len(), 4);
        assert_eq!(shards[0].shikigami_id, "398");
        assert_eq!(shards[1].shikigami_id, "315");
        assert_eq!(shards[2].shikigami_id, "593");
        assert_eq!(shards[3].shikigami_id, "411");
        assert_eq!(shards[3].max_shard_count, 25);
    }

    #[test]
    fn 式神传记二进度按目标式神读取() {
        let payload = serde_json::json!({
            "heroStoryProgress": {
                "554": [[1693, [40, 40]], [1694, [8, 8]]],
                "586": [[1813, [40, 40]], [1814, [8, 12]]],
                "602": [[1900, [40, 40]], [1901, [7, 12]]],
                "283": [[1111, [40, 40]], [1112, [7, 40]]],
                // 特殊条件的业务数字不等于快照分母，覆盖玉藻前和灶门炭治郎的实际读取形态。
                "300": [[1200, [0, 1]], [1201, [0, 1]]],
                "359": [[1300, [0, 1]], [1301, [0, 9]]],
                "999": [[2000, [1, 1]], [2001, [7, 40]]]
            }
        });
        let complete = parse_shikigami_story_unlock(&payload, "554")
            .expect("应读取已完成的传记二");
        assert_eq!(complete.activity_id, "1694");
        assert_eq!((complete.current, complete.required), (8, 8));
        let pending = parse_shikigami_story_unlock(&payload, "586")
            .expect("应读取未完成的传记二");
        assert_eq!((pending.current, pending.required), (8, 12));
        assert_eq!(pending.progress_kind, ShikigamiUnlockProgressKind::SkillUpgrade);
        let ling_yan_ji = parse_shikigami_story_unlock(&payload, "602")
            .expect("天火命铃彦姬应读取技能升级传记条件");
        assert_eq!(
            ling_yan_ji.progress_kind,
            ShikigamiUnlockProgressKind::SkillUpgrade
        );
        let level_progress = parse_shikigami_story_unlock(&payload, "283")
            .expect("应读取等级条件传记二");
        assert_eq!((level_progress.current, level_progress.required), (7, 40));
        assert_eq!(
            level_progress.progress_kind,
            ShikigamiUnlockProgressKind::Level
        );
        let yu_zao_qian = parse_shikigami_story_unlock(&payload, "300")
            .expect("玉藻前特殊传记条件应读取");
        assert_eq!(yu_zao_qian.progress_kind, ShikigamiUnlockProgressKind::Other);
        let tanjiro = parse_shikigami_story_unlock(&payload, "359")
            .expect("灶门炭治郎特殊传记条件应读取");
        assert_eq!(tanjiro.progress_kind, ShikigamiUnlockProgressKind::Other);
        let unknown = parse_shikigami_story_unlock(&payload, "999")
            .expect("未收录式神仍应返回原始传记进度");
        assert_eq!(unknown.progress_kind, ShikigamiUnlockProgressKind::Unknown);
        assert!(unknown.unlock_condition.is_none());
        assert!(parse_shikigami_story_unlock(&payload, "555").is_none());
    }

    #[test]
    fn 当前账号传记进度应返回解锁状态和剩余条件目标() {
        let path = test_path();
        let store = CurrentDataStore::new(path.clone(), active("profile-1"));
        let mut document = document("story.json", 0);
        document.source_payload = serde_json::json!({
            "heroStoryProgress": {
                "554": [[1693, [40, 40]], [1694, [8, 8]], [1695, [0, 10]]],
                "586": [[1813, [40, 40]], [1814, [8, 12]]],
                "283": [[1111, [40, 40]], [1112, [7, 40]]]
            }
        });
        store.replace(&document).expect("写入传记进度");

        let rows = store
            .list_shikigami_story_progress()
            .expect("读取传记进度");
        let completed = rows
            .iter()
            .find(|entry| entry.shikigami_id == "554")
            .expect("找到已完成式神");
        assert!(completed.unlocked);
        assert_eq!(completed.biographies.len(), 3);
        assert_eq!(completed.biographies[2].index, 3);
        let pending = rows
            .iter()
            .find(|entry| entry.shikigami_id == "586")
            .expect("找到未完成式神");
        assert!(!pending.unlocked);
        assert_eq!(pending.remaining, 4);
        assert_eq!(pending.unlockable_shard_count, Some(10));
        assert_eq!(pending.progress_kind, ShikigamiUnlockProgressKind::SkillUpgrade);
        let level_progress = rows
            .iter()
            .find(|entry| entry.shikigami_id == "283")
            .expect("找到等级条件式神");
        assert_eq!(level_progress.progress_kind, ShikigamiUnlockProgressKind::Level);
        assert_eq!(level_progress.remaining, 33);
        assert_eq!(level_progress.unlockable_shard_count, Some(10));
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn 素材式神分组应丢弃零项并拆出分组键() {
        // 真实快照里 44 个 heroId 只有 14 组非零，且同一 heroId 可跨 2_1_0 / 2_20_0 两组。
        let payload = serde_json::json!({
            "heroesBagEntries": [
                ["413_2_1_0", 2247],
                ["412_2_1_0", 910],
                ["412_2_20_0", 154],
                ["203_2_1_0", 0],
                ["400_2_20_0", 0],
            ]
        });
        let groups = parse_shikigami_bag(&payload);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].shikigami_id, "413");
        assert_eq!(groups[0].group_key, "2_1_0");
        assert_eq!(groups[0].count, 2247);
        assert_eq!(groups[2].shikigami_id, "412");
        assert_eq!(groups[2].group_key, "2_20_0");
        assert_eq!(groups[2].count, 154);
    }

    #[test]
    fn 旧快照缺少素材式神字段时返回空列表() {
        assert!(parse_shikigami_bag(&serde_json::json!({ "heroesBagCount": 5466 })).is_empty());
    }
}
