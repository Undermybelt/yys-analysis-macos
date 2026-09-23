//! 角色档案的可移植导入导出用例。
//!
//! 导出文件只保存角色当前导入的原始 JSON；导入时直接交给既有导入事务，
//! 保证角色归属、御魂规范化和数据库投影继续共用同一条链路，避免重复保存格式化数据。

use crate::application::{
    error::AppError,
    importer::{ImportFileInput, ImportUseCase},
    security::{MAX_IMPORT_FILE_BYTES, ensure_bytes},
    services::AppServices,
    tasks::TaskProgress,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// 前端收到的导出结果；路径由 Rust 直接写入系统下载目录后返回，避免浏览器默认下载位置不可见。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterArchiveExportResult {
    pub character_name: String,
    pub json_path: String,
    pub zip_path: String,
    pub record_count: u32,
}

/// 角色档案导入请求；JSON 正文从文件选择器读取后以字节数组交给 Rust 校验。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterArchiveImportRequest {
    pub file_name: String,
    pub payload: Vec<u8>,
}

/// 角色档案导出请求；只允许按已存在的数据档案 ID 导出，避免把当前激活状态当成隐式参数。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterArchiveExportRequest {
    pub profile_id: String,
}

/// 前端收到的导入结果；不回传御魂正文，只反馈本次替换的角色和数量。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterArchiveImportResult {
    pub character_name: String,
    pub profile_id: String,
    pub soul_count: u32,
    pub shikigami_count: u32,
}

/// 角色档案导入导出服务；所有文件都在本地完成解析和写入。
pub struct CharacterArchiveTransfer;

impl CharacterArchiveTransfer {
    /// 为指定角色生成 JSON 和 ZIP 两个文件；读取角色正文时不改变当前激活角色。
    pub fn export(
        services: &Arc<AppServices>,
        profile_id: &str,
        download_directory: &Path,
    ) -> Result<CharacterArchiveExportResult, AppError> {
        services
            .profiles
            .get(profile_id)?
            .ok_or_else(|| AppError::not_found("game_profile", profile_id))?;
        let archive = services
            .character_archives
            .list()?
            .into_iter()
            .find(|entry| entry.profile_id.as_deref() == Some(profile_id))
            .ok_or_else(|| AppError::not_found("角色档案", profile_id))?;
        let current_data = services
            .current_data
            .read_document_for_profile(profile_id)?;

        // 原始对象存储保留的是首次导入的精确字节；优先从这里读取，避免把 sourcePayload、
        // normalizedSouls、库存和分析结果重复拼进一个交换文件。旧数据缺少原始对象时，
        // 才退回序列化当前正文中的 sourcePayload，保证导出仍可完成。
        let content = match services.raw_object_store.load(&current_data.raw_sha256) {
            Ok(raw_payload) => raw_payload,
            Err(error) => {
                tracing::warn!(
                    profile_id,
                    error = %error.message,
                    "原始对象缺失，使用当前数据中的原始 JSON 结构导出"
                );
                serde_json::to_vec_pretty(&current_data.source_payload).map_err(
                    |serialize_error| {
                        AppError::internal(format!("序列化原始角色数据失败：{serialize_error}"))
                    },
                )?
            }
        };

        // 文件名主体使用角色昵称而不是内部 profileId，时间戳用于区分同一角色的多次导出。
        let stem = format!(
            "{}-{}",
            safe_file_part(&archive.display_name),
            Utc::now().format("%Y%m%d-%H%M%S-%3f")
        );
        let json_path = download_directory.join(format!("{stem}.json"));
        let zip_path = download_directory.join(format!("{stem}.zip"));
        fs::create_dir_all(download_directory)
            .map_err(|error| AppError::io("创建下载目录", &error))?;
        fs::write(&json_path, &content)
            .map_err(|error| AppError::io("写入角色档案 JSON", &error))?;

        // ZIP 只包含同一份 JSON，采用无压缩存储格式，避免新增原生依赖并确保 Windows 可直接打开。
        let zip_content = build_stored_zip(&format!("{stem}.json"), &content)?;
        fs::write(&zip_path, zip_content)
            .map_err(|error| AppError::io("写入角色档案 ZIP", &error))?;

        Ok(CharacterArchiveExportResult {
            character_name: archive.display_name,
            json_path: json_path.display().to_string(),
            zip_path: zip_path.display().to_string(),
            record_count: archive.soul_count.saturating_add(archive.shikigami_count),
        })
    }

    /// 校验 JSON 并原样交给既有导入事务；格式、身份和完整性仍由统一导入器负责校验。
    pub fn prepare_import(
        request: CharacterArchiveImportRequest,
    ) -> Result<ImportFileInput, AppError> {
        let CharacterArchiveImportRequest { file_name, payload } = request;
        // 扩展名只做用户体验层提示，正文会在既有导入器中继续做格式识别和安全校验。
        if !file_name.trim().to_ascii_lowercase().ends_with(".json") {
            return Err(AppError::invalid_argument(
                "characterArchiveFile",
                "角色档案导入只支持 JSON 文件",
            ));
        }
        ensure_bytes("characterArchiveFile", payload.len(), MAX_IMPORT_FILE_BYTES)?;
        serde_json::from_slice::<Value>(&payload).map_err(|error| {
            AppError::invalid_argument(
                "characterArchiveFile",
                format!("角色档案 JSON 格式错误：{error}"),
            )
        })?;

        Ok(ImportFileInput {
            // 使用固定文件名，避免用户选择的文件名包含路径分隔符或控制字符；原始内容不改写。
            file_name: "yys-character-archive-import.json".to_owned(),
            payload,
        })
    }

    /// 导入角色档案并复用现有当前数据覆盖事务；导入失败时原角色数据保持不变。
    ///
    /// 进度由既有导入器产生并通过调用方转发，角色档案页面因此可以展示预检、
    /// 御魂规范化和最终提交的真实阶段，而不需要在前端猜测处理时间。
    pub fn import<F>(
        services: &Arc<AppServices>,
        file: &ImportFileInput,
        task_id: &str,
        cancellation: &AtomicBool,
        publish: F,
    ) -> Result<CharacterArchiveImportResult, AppError>
    where
        F: Fn(TaskProgress) -> Result<(), AppError>,
    {
        let summary = ImportUseCase::new(services.clone()).import_current_file(
            task_id,
            file,
            cancellation,
            publish,
        )?;
        let profile_id = services
            .active_profile_id()
            .ok_or_else(|| AppError::internal("角色档案导入完成但没有激活数据档案"))?;
        let character = services
            .character_archives
            .list()?
            .into_iter()
            .find(|entry| entry.profile_id.as_deref() == Some(profile_id.as_str()))
            .ok_or_else(|| AppError::internal("角色档案导入完成但没有找到档案卡片"))?;
        Ok(CharacterArchiveImportResult {
            character_name: character.display_name,
            profile_id,
            soul_count: summary.current_soul_count,
            shikigami_count: summary.current_shikigami_count,
        })
    }
}

/// 清理 Windows 不允许的文件名字符，同时保留中文昵称、空格和常用标点。
fn safe_file_part(value: &str) -> String {
    let safe = value
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
        })
        .collect::<String>();
    let safe = safe.trim_matches([' ', '.']).to_owned();
    if safe.is_empty() {
        "profile".to_owned()
    } else {
        safe
    }
}

/// 构造一个只包含单个 JSON 文件的 ZIP；无压缩存储足以满足备份和跨设备传输需求。
fn build_stored_zip(file_name: &str, content: &[u8]) -> Result<Vec<u8>, AppError> {
    let name = file_name.as_bytes();
    if name.len() > u16::MAX as usize || content.len() > u32::MAX as usize {
        return Err(AppError::resource_limit(
            "characterArchiveZip",
            content.len() as u64,
            u32::MAX as u64,
        ));
    }
    let crc = crc32(content);
    let size = content.len() as u32;
    let name_len = name.len() as u16;
    let mut zip = Vec::with_capacity(content.len() + 128);

    put_u32(&mut zip, 0x0403_4b50);
    put_u16(&mut zip, 20);
    put_u16(&mut zip, 0);
    put_u16(&mut zip, 0);
    put_u16(&mut zip, 0);
    put_u16(&mut zip, 0);
    put_u32(&mut zip, crc);
    put_u32(&mut zip, size);
    put_u32(&mut zip, size);
    put_u16(&mut zip, name_len);
    put_u16(&mut zip, 0);
    zip.extend_from_slice(name);
    zip.extend_from_slice(content);

    let local_offset = 0_u32;
    let central_offset = zip.len() as u32;
    put_u32(&mut zip, 0x0201_4b50);
    put_u16(&mut zip, 20);
    put_u16(&mut zip, 20);
    put_u16(&mut zip, 0);
    put_u16(&mut zip, 0);
    put_u16(&mut zip, 0);
    put_u16(&mut zip, 0);
    put_u32(&mut zip, crc);
    put_u32(&mut zip, size);
    put_u32(&mut zip, size);
    put_u16(&mut zip, name_len);
    put_u16(&mut zip, 0);
    put_u16(&mut zip, 0);
    put_u16(&mut zip, 0);
    put_u16(&mut zip, 0);
    put_u32(&mut zip, 0);
    put_u32(&mut zip, local_offset);
    zip.extend_from_slice(name);

    let central_size = (zip.len() as u32) - central_offset;
    put_u32(&mut zip, 0x0605_4b50);
    put_u16(&mut zip, 0);
    put_u16(&mut zip, 0);
    put_u16(&mut zip, 1);
    put_u16(&mut zip, 1);
    put_u32(&mut zip, central_size);
    put_u32(&mut zip, central_offset);
    put_u16(&mut zip, 0);
    Ok(zip)
}

/// ZIP 使用 CRC-32 校验单个文件；位运算实现避免新增压缩依赖。
fn crc32(content: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in content {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// 以 ZIP 约定的小端序写入 16 位整数。
fn put_u16(buffer: &mut Vec<u8>, value: u16) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

/// 以 ZIP 约定的小端序写入 32 位整数。
fn put_u32(buffer: &mut Vec<u8>, value: u32) {
    buffer.extend_from_slice(&value.to_le_bytes());
}
