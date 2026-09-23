//! 角色档案列表。
//!
//! macOS 版不再扫描 Windows 桌面版游戏目录。角色档案只来自藏宝阁导入
//! 或本地导入写入的摘要，不读取游戏进程，也不携带御魂正文。

use crate::application::error::AppError;
use crate::infrastructure::character_archives::CharacterArchiveStore;
use serde::Serialize;

/// 一个本地角色的档案摘要；展示区服、昵称、ID 与最后更新时间，不携带御魂正文。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterArchive {
    /// 角色档案身份键；删除单个角色时使用该稳定键。
    pub identity_key: String,
    pub account_id: String,
    /// 该角色绑定的数据档案 ID；切换角色时前端直接使用该 ID。
    pub profile_id: Option<String>,
    /// 角色昵称；缺失时使用不暴露 UID 的友好标签。
    pub display_name: String,
    /// 角色平台：android 或 ios；旧档案没有该字段时为空。
    pub platform: Option<String>,
    /// 游戏内玩家 ID（UID）；快照未携带时为空。
    pub player_id: Option<String>,
    /// 区服名；快照未携带时为空。
    pub server_label: Option<String>,
    /// 最后一次成功导入的时间。
    pub last_updated_at: Option<String>,
    /// 是否与当前导入快照是同一角色。
    pub is_current: bool,
    pub readable: bool,
    pub warning: Option<String>,
    /// 六星御魂数。
    pub soul_count: u32,
    /// 式神数。
    pub shikigami_count: u32,
    /// 档案来源标记：cbg（藏宝阁）/ native（本地导入）等。
    pub source_kind: String,
    /// 角色来源展示名：藏宝阁 / 本地导入等。
    pub source_label: String,
}

/// 角色档案扫描结果；本机没有档案时返回空列表和引导提示，而不是错误。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterArchiveScanResult {
    pub characters: Vec<CharacterArchive>,
    pub message: String,
}

/// 将导入来源标记转换成角色档案页的展示名。
pub fn archive_source_label(source_kind: &str) -> String {
    match source_kind {
        "cbg" => "藏宝阁".to_owned(),
        _ => "本地导入".to_owned(),
    }
}

/// 扫描本机全部角色档案：只读取导入写入的档案摘要，不再扫描游戏目录缓存。
///
/// 档案只由导入新增或更新，重复导入同一角色覆盖旧信息；没有档案时返回空列表
/// 和引导提示，而不是错误。角色档案页不读取、识别或展示游戏目录。
pub fn scan_character_archives(
    archives: &CharacterArchiveStore,
    active_profile_id: Option<&str>,
) -> Result<CharacterArchiveScanResult, AppError> {
    let characters = archives
        .list()?
        .into_iter()
        .map(|entry| CharacterArchive {
            identity_key: entry.identity_key,
            is_current: entry.profile_id.as_deref() == active_profile_id,
            account_id: entry.account_id,
            profile_id: entry.profile_id,
            display_name: entry.display_name,
            platform: entry.platform,
            player_id: entry.player_id,
            server_label: entry.server_label,
            last_updated_at: Some(entry.last_updated_at),
            readable: true,
            warning: None,
            soul_count: entry.soul_count,
            shikigami_count: entry.shikigami_count,
            source_kind: entry.source_kind,
            source_label: entry.source_label,
        })
        .collect::<Vec<_>>();
    let character_count = characters.len();
    tracing::info!(character_count, "角色档案扫描完成");
    Ok(CharacterArchiveScanResult {
        characters,
        message: format!("已找到 {character_count} 个角色档案"),
    })
}

