//! 读取载荷应用服务：识别 MuMu 与桌面版的内部格式、规范化御魂事实并提交快照。
//!
//! 解析阶段不触碰数据库，提交阶段由导入仓库保证事件、快照和库存投影的事务边界；
//! 因此单个读取载荷失败不会留下半个快照，正式读取入口仍可复用同一套事务边界。

use crate::application::{
    character_archives::archive_source_label,
    error::AppError,
    security::{
        MAX_ATTRIBUTES_PER_SOUL, MAX_IMPORT_FILE_BYTES, MAX_IMPORT_FILES, MAX_IMPORT_SHIKIGAMI,
        MAX_IMPORT_SOULS, ensure_bytes, resource_limit,
    },
    services::AppServices,
    tasks::{TaskProgress, TaskStatus},
};
use crate::domain::{
    AcquisitionEvent, ImportCommitRequest, ImportCommitResult, NewGameProfile, OwnedShikigami,
    OwnedShikigamiSkill, Snapshot, SnapshotSoul, SoulAttribute, PLATFORM_ANDROID, PLATFORM_IOS,
};
use crate::infrastructure::character_archives::StoredCharacterArchive;
use crate::infrastructure::current_data::{CURRENT_DATA_VERSION, CurrentDataDocument};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use uuid::Uuid;

/// 桌面版 heroes 中代表阴阳师主角等特殊角色的目录 ID；这些记录不是普通式神录条目，等级可超过 40。
const DESKTOP_SPECIAL_HERO_IDS: &[&str] = &["10", "11", "12", "13", "15", "16"];

/// 判断桌面版 heroes 记录是否属于不应进入普通式神录的特殊角色。
pub(crate) fn is_desktop_special_hero_id(shikigami_id: &str) -> bool {
    DESKTOP_SPECIAL_HERO_IDS.contains(&shikigami_id)
}

/// 前端提交的单个待采集文件；载荷保持原始字节，便于原始对象哈希与回溯。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFileInput {
    pub file_name: String,
    pub payload: Vec<u8>,
}

/// 导入选项；完整性覆盖必须由用户显式传入，默认沿用来源声明或局部语义。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOptions {
    pub completeness_override: Option<String>,
    #[serde(default)]
    pub force_snapshot: bool,
}

/// 单文件导入结果，保存错误详情和快照/投影变化摘要。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFileResult {
    pub file_name: String,
    pub format: Option<String>,
    pub soul_count: u32,
    pub commit: Option<ImportCommitResult>,
    pub error: Option<AppError>,
}

/// 批量导入最终摘要；已成功文件的提交不会因其他文件失败而回滚。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportBatchSummary {
    pub total_files: u32,
    pub succeeded_files: u32,
    pub failed_files: u32,
    pub cancelled: bool,
    pub total_souls: u32,
    pub added_count: u32,
    pub updated_count: u32,
    pub removed_count: u32,
    pub unchanged_files: u32,
    pub results: Vec<ImportFileResult>,
}

/// 当前数据覆盖结果；只报告旧数据数量和新数据数量，不再返回快照历史。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentImportSummary {
    pub file_name: String,
    pub source_kind: String,
    pub completeness: String,
    pub imported_at: String,
    pub previous_soul_count: u32,
    pub current_soul_count: u32,
    pub previous_shikigami_count: u32,
    pub current_shikigami_count: u32,
    pub replaced_count: u32,
}

/// 已经通过预检的规范导入中间态；内部 ID 只服务于当前快照，不作为跨快照身份。
#[derive(Clone, Debug)]
struct ParsedImport {
    format: String,
    source_kind: String,
    completeness: String,
    captured_at: Option<String>,
    game_version: Option<String>,
    adapter_version: Option<String>,
    scope_json: Option<String>,
    scope_fingerprint: String,
    content_fingerprint: String,
    souls: Vec<NormalizedSoul>,
    owned_shikigami: Vec<OwnedShikigami>,
    /// 归一化的资源/道具数量键值对象；非桌面类格式为空对象。
    items: Value,
    /// 归一化的结界卡列表；非桌面类格式为空数组。
    realm_cards: Value,
    warnings: Vec<String>,
}

/// 尚未绑定快照 ID 的规范御魂。
#[derive(Clone, Debug, Serialize)]
struct NormalizedSoul {
    source_stable_id: Option<String>,
    identity_quality: String,
    set_id: String,
    slot: u8,
    quality: u8,
    level: u8,
    main_attr_type: String,
    main_attr_value: f64,
    initial_substat_count: Option<u8>,
    locked_in_source: Option<bool>,
    equipped_state: Option<String>,
    attributes: Vec<NormalizedAttribute>,
    source_json: String,
}

/// 尚未绑定快照 ID 的规范副属性。
#[derive(Clone, Debug, Serialize)]
struct NormalizedAttribute {
    attribute_index: u8,
    attribute_type: String,
    value: f64,
    enhancement_count: Option<u8>,
    fixed_attribute: bool,
}

/// 导入应用用例，统一编排格式解析、原始对象保存和快照事务提交。
pub struct ImportUseCase {
    services: Arc<AppServices>,
}

impl ImportUseCase {
    /// 创建导入用例；服务集合由组合根持有，避免导入层自行创建数据库连接。
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 覆盖当前角色的数据；先按文件身份归属角色（已存在角色复用其档案并覆盖，
    /// 新角色创建档案），再复用既有规范化与库存提交链路，最后提交该角色的数据正文。
    pub fn import_current_file<F>(
        &self,
        task_id: &str,
        file: &ImportFileInput,
        cancellation: &AtomicBool,
        publish: F,
    ) -> Result<CurrentImportSummary, AppError>
    where
        F: Fn(TaskProgress) -> Result<(), AppError>,
    {
        let parsed = parse_file(file, None)?;
        let source_payload: Value = serde_json::from_slice(&file.payload)
            .map_err(|error| AppError::internal(format!("读取当前源 JSON 失败：{error}")))?;
        let normalized_souls = serde_json::to_value(&parsed.souls)
            .map_err(|error| AppError::internal(format!("序列化规范化御魂失败：{error}")))?;
        let imported_at = AppServices::now_iso();
        let document = CurrentDataDocument {
            version: CURRENT_DATA_VERSION,
            file_name: file.file_name.clone(),
            source_kind: parsed.source_kind.clone(),
            completeness: parsed.completeness.clone(),
            imported_at: imported_at.clone(),
            captured_at: parsed.captured_at.clone(),
            raw_sha256: hash_text(std::str::from_utf8(&file.payload).unwrap_or_default()),
            soul_count: parsed.souls.len() as u32,
            normalized_souls,
            normalized_shikigami: parsed.owned_shikigami.clone(),
            normalized_items: parsed.items.clone(),
            normalized_realm_cards: parsed.realm_cards.clone(),
            source_payload,
        };
        // 按文件身份归属角色：已存在档案复用其绑定的数据档案，新角色创建档案。
        // 档案条目在导入成功后才会落盘，避免失败导入留下幽灵角色卡。
        let mut archive_entry = archive_entry_from_import(&document, &parsed);
        let identity_key = archive_entry.identity_key.clone();
        let previous_active = self.services.active_profile_id();
        let (profile_id, created_profile) = self.resolve_character(&archive_entry, &document, &parsed)?;
        archive_entry.profile_id = Some(profile_id.clone());
        // 激活目标角色后再读取历史摘要，前后对比才是该角色自己的新旧数据。
        self.services.set_active_profile(profile_id.clone());
        let previous_summary = self.services.current_data.summary()?;
        let previous_soul_count = previous_summary.soul_count;
        let previous_shikigami_count = previous_summary.shikigami_count;

        // 旧批量导入会自行发送完成事件；这里暂存完成事件，待单文件正文成功替换后再发送最终摘要。
        let pending_completion = std::sync::Arc::new(std::sync::Mutex::new(None));
        let pending_completion_for_publish = pending_completion.clone();
        let batch = self.import_files(
            task_id,
            &profile_id,
            std::slice::from_ref(file),
            &ImportOptions {
                completeness_override: None,
                force_snapshot: true,
            },
            cancellation,
            |progress| {
                if matches!(progress.status, TaskStatus::Completed) {
                    *pending_completion_for_publish
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(progress);
                    Ok(())
                } else {
                    publish(progress)
                }
            },
        );
        // 失败路径回滚：撤销新创建的数据档案与角色档案条目，并恢复此前的激活角色。
        let batch = batch.map_err(|error| {
            if created_profile {
                let _ = self.services.character_archives.delete(&identity_key);
                let _ = self.services.profiles.set_archived(
                    &profile_id,
                    true,
                    0,
                    &AppServices::now_iso(),
                );
            }
            self.restore_active(previous_active.clone());
            error
        })?;
        if batch.succeeded_files != 1 || batch.failed_files != 0 {
            let error = batch
                .results
                .first()
                .and_then(|result| result.error.clone())
                .unwrap_or_else(|| AppError::internal("当前数据覆盖未成功完成"));
            if created_profile {
                let _ = self.services.character_archives.delete(&identity_key);
                let _ = self.services
                    .profiles
                    .set_archived(&profile_id, true, 0, &AppServices::now_iso());
            }
            self.restore_active(previous_active.clone());
            return Err(error);
        }
        if cancellation.load(Ordering::Acquire) {
            if created_profile {
                let _ = self.services.character_archives.delete(&identity_key);
                let _ = self.services
                    .profiles
                    .set_archived(&profile_id, true, 0, &AppServices::now_iso());
            }
            self.restore_active(previous_active);
            return Err(AppError::invalid_argument(
                "currentData",
                "当前数据覆盖已取消",
            ));
        }

        // 只有正文完整写入后才报告完成，前端不会把半成品误认为当前数据。
        self.services.current_data.replace(&document)?;
        // 持有式神镜像同步进 SQLite，供数据库侧查询与后续分析。
        self.services.owned_shikigami.replace_all(
            &profile_id,
            &parsed.owned_shikigami,
            &parsed.source_kind,
            &imported_at,
        )?;
        // 完成提示要指名角色：同一台机器常有多个角色，只报数量无法确认覆盖的是哪一个。
        // archive_entry 随后会被 upsert 取走，先在这里取出展示所需的字段。
        let character_label = format_character_label(&archive_entry);
        // 角色档案只由导入写入：同身份键的角色覆盖旧信息，新角色追加卡片。
        if let Err(error) = self.services.character_archives.upsert(archive_entry) {
            tracing::warn!(error = %error.message, "角色档案更新失败，不影响本次导入");
        }
        let result = CurrentImportSummary {
            file_name: file.file_name.clone(),
            source_kind: parsed.source_kind.clone(),
            completeness: parsed.completeness.clone(),
            imported_at: imported_at.clone(),
            previous_soul_count,
            current_soul_count: parsed.souls.len() as u32,
            previous_shikigami_count,
            current_shikigami_count: parsed.owned_shikigami.len() as u32,
            replaced_count: previous_soul_count,
        };
        let _ = pending_completion
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        publish(TaskProgress {
            task_id: task_id.to_owned(),
            phase: "complete".to_owned(),
            completed: 1,
            total: 1,
            message: format!(
                "{character_label} 当前数据已覆盖：{} 枚御魂、{} 个式神实例，旧数据 {} 枚御魂、{} 个式神实例",
                result.current_soul_count,
                result.current_shikigami_count,
                result.previous_soul_count,
                result.previous_shikigami_count
            ),
            status: TaskStatus::Completed,
            error: None,
            result: Some(
                serde_json::to_value(&result)
                    .map_err(|error| AppError::internal(format!("序列化导入摘要失败：{error}")))?,
            ),
        })?;
        Ok(result)
    }

    /// 按档案身份归属角色并返回数据档案 ID；新角色会先创建 SQLite 档案
    /// 并暂存档案条目，导入成功后条目才会最终落盘。
    fn resolve_character(
        &self,
        archive_entry: &StoredCharacterArchive,
        document: &CurrentDataDocument,
        parsed: &ParsedImport,
    ) -> Result<(String, bool), AppError> {
        if let Some(existing) = self
            .services
            .character_archives
            .find(&archive_entry.identity_key)?
        {
            let profile_id = existing.profile_id.ok_or_else(|| {
                AppError::internal("角色档案缺少数据档案绑定，请清空档案后重新导入")
            })?;
            return Ok((profile_id, false));
        }
        let new_profile = self.services.profiles.create(
            &NewGameProfile {
                display_name: archive_entry.display_name.clone(),
                source_identity: Some(archive_entry.identity_key.clone()),
                server_label: archive_entry.server_label.clone(),
            },
            &document.imported_at,
        )?;
        let mut entry = archive_entry_from_import(document, parsed);
        entry.profile_id = Some(new_profile.id.clone());
        self.services.character_archives.upsert(entry)?;
        Ok((new_profile.id, true))
    }

    /// 导入失败时恢复此前的激活角色；此前没有角色则保持未激活。
    fn restore_active(&self, previous_active: Option<String>) {
        match previous_active {
            Some(profile_id) => self.services.set_active_profile(profile_id),
            None => self.services.clear_active_profile(),
        }
    }

    /// 执行批量导入；进度按文件预检和规范御魂数量发布，取消只会阻止尚未提交的文件。
    pub fn import_files<F>(
        &self,
        task_id: &str,
        profile_id: &str,
        files: &[ImportFileInput],
        options: &ImportOptions,
        cancellation: &AtomicBool,
        publish: F,
    ) -> Result<ImportBatchSummary, AppError>
    where
        F: Fn(TaskProgress) -> Result<(), AppError>,
    {
        self.ensure_profile(profile_id)?;
        if files.len() > MAX_IMPORT_FILES {
            return Err(resource_limit(
                "fileCount",
                files.len() as u64,
                MAX_IMPORT_FILES as u64,
            ));
        }
        let total_files = files.len() as u32;
        let mut parsed_files: Vec<(&ImportFileInput, ParsedImport)> = Vec::new();
        let mut summary = ImportBatchSummary {
            total_files,
            ..ImportBatchSummary::default()
        };

        // 先逐个预检，保证单文件格式错误不会阻断其他文件的规范化与提交。
        for (index, file) in files.iter().enumerate() {
            if cancellation.load(Ordering::Acquire) {
                summary.cancelled = true;
                publish(progress(
                    task_id,
                    "preflight",
                    index as u32,
                    total_files,
                    "导入已取消",
                    TaskStatus::Cancelled,
                    Some(
                        serde_json::to_value(&summary)
                            .map_err(|error| AppError::internal(error.to_string()))?,
                    ),
                ))?;
                return Ok(summary);
            }
            match parse_file(file, options.completeness_override.as_deref()) {
                Ok(parsed) => parsed_files.push((file, parsed)),
                Err(error) => {
                    let error = self.prepare_mumu_failure(file, "preflight", error);
                    summary.failed_files += 1;
                    summary.results.push(ImportFileResult {
                        file_name: file.file_name.clone(),
                        format: None,
                        soul_count: 0,
                        commit: None,
                        error: Some(error),
                    });
                }
            }
            publish(progress(
                task_id,
                "preflight",
                (index + 1) as u32,
                total_files.max(1),
                format!("已预检 {}/{} 个文件", index + 1, total_files),
                TaskStatus::Running,
                None,
            ))?;
        }

        let total_souls = parsed_files
            .iter()
            .map(|(_, parsed)| parsed.souls.len() as u32)
            .sum::<u32>();
        summary.total_souls = total_souls;
        let mut normalized_souls = 0u32;

        for (file, parsed) in parsed_files {
            if cancellation.load(Ordering::Acquire) {
                summary.cancelled = true;
                publish(progress(
                    task_id,
                    "normalize",
                    normalized_souls,
                    total_souls.max(1),
                    "导入已取消，尚未提交当前文件",
                    TaskStatus::Cancelled,
                    None,
                ))?;
                break;
            }

            let snapshot_id = Uuid::new_v4().to_string();
            let now = AppServices::now_iso();
            let (raw_sha256, raw_object) = match self.store_raw_object(&file.payload) {
                Ok(value) => value,
                Err(error) => {
                    let error = self.prepare_mumu_failure(file, "store_raw_object", error);
                    summary.failed_files += 1;
                    summary.results.push(ImportFileResult {
                        file_name: file.file_name.clone(),
                        format: Some(parsed.format.clone()),
                        soul_count: parsed.souls.len() as u32,
                        commit: None,
                        error: Some(error),
                    });
                    continue;
                }
            };
            let raw_existing = match self.services.raw_objects.get(&raw_sha256) {
                Ok(value) => value,
                Err(error) => {
                    let error = self.prepare_mumu_failure(file, "lookup_raw_object", error);
                    summary.failed_files += 1;
                    summary.results.push(ImportFileResult {
                        file_name: file.file_name.clone(),
                        format: Some(parsed.format.clone()),
                        soul_count: parsed.souls.len() as u32,
                        commit: None,
                        error: Some(error),
                    });
                    continue;
                }
            };
            if raw_existing.is_none() {
                if let Err(error) = self.services.raw_objects.upsert(&raw_object) {
                    let error = self.prepare_mumu_failure(file, "save_raw_object", error);
                    summary.failed_files += 1;
                    summary.results.push(ImportFileResult {
                        file_name: file.file_name.clone(),
                        format: Some(parsed.format.clone()),
                        soul_count: parsed.souls.len() as u32,
                        commit: None,
                        error: Some(error),
                    });
                    continue;
                }
            }

            let (souls, attributes) = build_snapshot_facts(&snapshot_id, &parsed.souls);
            for _ in &souls {
                normalized_souls += 1;
                // 每枚记录均检查取消，10,000 枚数据不会把取消请求拖到下一个文件。
                if cancellation.load(Ordering::Acquire) {
                    summary.cancelled = true;
                    break;
                }
                // 进度事件按 100 枚节流，既能覆盖 10,000 枚导入，又避免 WebView 事件风暴。
                if should_publish_normalize_progress(normalized_souls, total_souls) {
                    publish(progress(
                        task_id,
                        "normalize",
                        normalized_souls,
                        total_souls.max(1),
                        format!("正在规范化御魂 {normalized_souls}/{total_souls}"),
                        TaskStatus::Running,
                        None,
                    ))?;
                }
            }
            if summary.cancelled {
                break;
            }

            let catalog_version = self
                .services
                .catalog
                .active_status()?
                .map(|status| status.version)
                .unwrap_or_else(|| "unresolved".to_owned());
            let event = AcquisitionEvent {
                id: Uuid::new_v4().to_string(),
                profile_id: profile_id.to_owned(),
                source_kind: parsed.source_kind.clone(),
                source_format: parsed.format.clone(),
                raw_sha256: Some(raw_sha256.clone()),
                captured_at: parsed.captured_at.clone(),
                received_at: now.clone(),
                game_version: parsed.game_version.clone(),
                adapter_version: parsed.adapter_version.clone(),
                parser_version: env!("CARGO_PKG_VERSION").to_owned(),
                completeness: parsed.completeness.clone(),
                scope_json: parsed.scope_json.clone(),
                result_kind: "new_snapshot".to_owned(),
                snapshot_id: None,
            };
            let snapshot = Snapshot {
                id: snapshot_id,
                profile_id: profile_id.to_owned(),
                raw_sha256,
                content_fingerprint: Some(parsed.content_fingerprint.clone()),
                scope_fingerprint: Some(parsed.scope_fingerprint.clone()),
                source_kind: parsed.source_kind,
                completeness: parsed.completeness,
                scope_json: parsed.scope_json,
                captured_at: parsed.captured_at,
                game_version: parsed.game_version,
                adapter_version: parsed.adapter_version,
                parser_version: env!("CARGO_PKG_VERSION").to_owned(),
                catalog_version,
                parent_snapshot_id: None,
                forced: options.force_snapshot,
                created_at: now,
            };
            let commit = match self.services.imports.commit_import(&ImportCommitRequest {
                event,
                snapshot,
                souls,
                attributes,
                force_snapshot: options.force_snapshot,
            }) {
                Ok(commit) => commit,
                Err(error) => {
                    let error = self.prepare_mumu_failure(file, "commit_import", error);
                    summary.failed_files += 1;
                    summary.results.push(ImportFileResult {
                        file_name: file.file_name.clone(),
                        format: Some(parsed.format.clone()),
                        soul_count: parsed.souls.len() as u32,
                        commit: None,
                        error: Some(error),
                    });
                    continue;
                }
            };
            summary.succeeded_files += 1;
            summary.added_count += commit.added_count;
            summary.updated_count += commit.updated_count;
            summary.removed_count += commit.removed_count;
            if commit.result_kind == "unchanged" {
                summary.unchanged_files += 1;
            }
            summary.results.push(ImportFileResult {
                file_name: file.file_name.clone(),
                format: Some(parsed.format),
                soul_count: parsed.souls.len() as u32,
                commit: Some(commit),
                error: None,
            });

            // 导入只负责提交库存事实；标准评分由用户进入库存列表后明确启动，避免读取过程额外占用计算资源。
        }

        let status = if summary.cancelled {
            TaskStatus::Cancelled
        } else {
            TaskStatus::Completed
        };
        publish(progress(
            task_id,
            "complete",
            if summary.cancelled {
                normalized_souls
            } else {
                total_souls.max(1)
            },
            total_souls.max(1),
            format!(
                "导入完成：成功 {} 个，失败 {} 个，新增 {} 枚，更新 {} 枚，移除 {} 枚",
                summary.succeeded_files,
                summary.failed_files,
                summary.added_count,
                summary.updated_count,
                summary.removed_count
            ),
            status,
            Some(
                serde_json::to_value(&summary)
                    .map_err(|error| AppError::internal(error.to_string()))?,
            ),
        ))?;
        Ok(summary)
    }

    /// 记录 MuMu 导入失败并保存原始批次，使用户可以直接定位实际返回的 JSON 结构。
    /// 普通读取载荷不写诊断副本；保存失败也只追加日志，不改变原始业务错误。
    fn prepare_mumu_failure(
        &self,
        file: &ImportFileInput,
        stage: &str,
        mut error: AppError,
    ) -> AppError {
        if file.file_name.starts_with("mumu-") {
            match self
                .services
                .save_mumu_failed_payload(&file.file_name, &file.payload)
            {
                Ok(path) => {
                    if let Some(details) = error.details.as_mut().and_then(Value::as_object_mut) {
                        details.insert(
                            "diagnosticPath".to_owned(),
                            Value::String(path.display().to_string()),
                        );
                    }
                    tracing::info!(
                        file_name = %file.file_name,
                        diagnostic_path = %path.display(),
                        "MuMu 失败批次原始 JSON 已保存"
                    );
                }
                Err(save_error) => {
                    tracing::error!(
                        file_name = %file.file_name,
                        error_code = ?save_error.code,
                        error_message = %save_error.message,
                        "MuMu 失败批次原始 JSON 保存失败"
                    );
                }
            }
            log_mumu_import_failure(&file.file_name, stage, &error);
        }
        error
    }

    /// 校验档案存在，所有导入数据必须显式归属某个游戏档案。
    fn ensure_profile(&self, profile_id: &str) -> Result<(), AppError> {
        self.services
            .profiles
            .get(profile_id)?
            .ok_or_else(|| AppError::not_found("game_profile", profile_id))?;
        Ok(())
    }

    /// 保存原始 JSON 并生成数据库对象记录；内容寻址文件可被重复导入复用。
    fn store_raw_object(
        &self,
        payload: &[u8],
    ) -> Result<(String, crate::domain::RawObject), AppError> {
        let (sha256, stored) = self
            .services
            .raw_object_store
            .store(payload, "application/json")?;
        let now = AppServices::now_iso();
        Ok((
            sha256.clone(),
            crate::domain::RawObject {
                sha256: sha256.clone(),
                relative_path:
                    crate::infrastructure::raw_object_store::RawObjectStore::relative_path_for(
                        &sha256,
                    ),
                compression: "zstd".to_owned(),
                media_type: "application/json".to_owned(),
                raw_size: stored.raw_size,
                stored_size: stored.stored_size,
                verified_at: Some(now.clone()),
                created_at: now,
            },
        ))
    }
}

/// 解析单个文件并应用用户完整性覆盖；未证明完整时统一默认局部。
fn parse_file(
    file: &ImportFileInput,
    completeness_override: Option<&str>,
) -> Result<ParsedImport, AppError> {
    // 先检查采集载荷原始字节，再交给 JSON 解析器，避免超大输入在语法校验阶段占满内存。
    ensure_bytes("fileBytes", file.payload.len(), MAX_IMPORT_FILE_BYTES)?;
    let text = std::str::from_utf8(&file.payload).map_err(|error| {
        AppError::import_field(
            &file.file_name,
            "$",
            "UTF-8 JSON",
            &error.to_string(),
            "文件不是有效的 UTF-8 JSON",
        )
    })?;
    let value: Value = serde_json::from_str(text).map_err(|error| {
        AppError::import_field(
            &file.file_name,
            "$",
            "合法 JSON 对象",
            &error.to_string(),
            "JSON 解析失败",
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        AppError::import_field(
            &file.file_name,
            "$",
            "JSON 对象",
            value_kind(&value),
            "采集载荷顶层必须是 JSON 对象",
        )
    })?;

    // 痒痒鼠魔方直接保存游戏函数返回的压缩映射，不带 equips 外壳；先按其稳定字段签名识别并本地解码。
    if looks_like_mumu_compact_batch(object) {
        return parse_mumu_compact_batch(file, object, completeness_override);
    }

    // 原始采集脚本会在压缩映射外再包一层 batches；先合并批次，再复用同一套严格解码逻辑。
    if object
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|format| format == "mumu-raw-capture-v1")
    {
        return parse_mumu_raw_capture(file, object, completeness_override);
    }

    let (format, source_kind, souls_value, souls_path, metadata) = if object
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|format| {
            matches!(
                format,
                "yys-analysis-snapshot"
                    | "yys-analysis-snapshot-v1"
                    | "yys-snapshot"
                    | "yys-desktop-cache-v1"
                    | "mumu-adapter-v1"
                    | "mumu-snapshot-v1"
                    | "cbg-equip-v1"
            )
    }) {
        // 本工具和 MuMu 适配器格式只接受版本化快照约定的 souls 字段，避免把未知 JSON 猜测成库存。
        let explicit_format = object.get("format").and_then(Value::as_str);
        let (souls, souls_path) = if explicit_format == Some("mumu-snapshot-v1") {
            (object.get("hero_equips"), "$.hero_equips")
        } else {
            (object.get("souls"), "$.souls")
        };
        let is_mumu = object
            .get("format")
            .and_then(Value::as_str)
            .is_some_and(|format| matches!(format, "mumu-adapter-v1" | "mumu-snapshot-v1"));
        let is_desktop = object
            .get("format")
            .and_then(Value::as_str)
            .is_some_and(|format| format == "yys-desktop-cache-v1");
        let is_cbg = object
            .get("format")
            .and_then(Value::as_str)
            .is_some_and(|format| format == "cbg-equip-v1");
        (
            if explicit_format == Some("mumu-snapshot-v1") {
                "mumu-snapshot-v1"
            } else if is_mumu {
                "mumu-adapter-v1"
            } else if is_desktop {
                "yys-desktop-cache-v1"
            } else if is_cbg {
                "cbg-equip-v1"
            } else {
                "yys-analysis-snapshot-v1"
            }
            .to_owned(),
            if is_mumu {
                "mumu"
            } else if is_desktop {
                "desktop"
            } else if is_cbg {
                "cbg"
            } else {
                "native"
            }
            .to_owned(),
            souls,
            souls_path,
            object,
        )
    } else if object.get("version").is_some()
        && object.get("timestamp").is_some()
        && object
            .get("data")
            .and_then(|data| data.get("hero_equips"))
            .is_some()
    {
        (
            "yyshub-v1".to_owned(),
            "importer".to_owned(),
            object.get("data").and_then(|data| data.get("hero_equips")),
            "$.data.hero_equips",
            object,
        )
    } else if object.get("hero_equips").is_some() {
        let souls = object
            .get("hero_equips")
            .expect("格式分支已确认存在 hero_equips 字段");
        // 参考 yyx-snapshot 会同时输出 player/currency/heroes/realm_cards；带这些完整字段时按全量 MuMu 快照处理。
        let is_reference_snapshot = object.get("player").is_some()
            || object.get("currency").is_some()
            || object.get("heroes").is_some()
            || object.get("realm_cards").is_some()
            || object.get("hero_equip_presets").is_some();
        let souls_path = "$.hero_equips";
        (
            if is_reference_snapshot {
                "mumu-snapshot-v1".to_owned()
            } else {
                "raw-exporter-v1".to_owned()
            },
            if is_reference_snapshot {
                "mumu".to_owned()
            } else {
                "importer".to_owned()
            },
            Some(souls),
            souls_path,
            object,
        )
    } else if object.get("equips").is_some()
        || object
            .get("data")
            .and_then(|data| data.get("equips"))
            .is_some()
    {
        // 参考痒痒鼠导出器可能直接返回 equips，桥接层也用该数组统计批次数量。
        let (souls, souls_path) = if let Some(souls) = object.get("equips") {
            (souls, "$.equips")
        } else {
            (
                object
                    .get("data")
                    .and_then(|data| data.get("equips"))
                    .expect("格式分支已确认存在 data.equips 字段"),
                "$.data.equips",
            )
        };
        (
            "mumu-adapter-v1".to_owned(),
            "mumu".to_owned(),
            Some(souls),
            souls_path,
            object,
        )
    } else {
        let mut error = AppError::import_field(
            &file.file_name,
            "$",
            "本工具快照、hero_equips/equips 原始导出或 yyshub data.hero_equips",
            "未找到可识别的格式标记",
            "无法识别读取载荷格式，请重新执行 MuMu 或桌面版读取",
        );
        if file.file_name.starts_with("mumu-") {
            // 只把键名、类型和数组长度放进错误详情，禁止把御魂原始内容写入日志或界面。
            let shape = json_shape(&value, 0);
            if let Some(details) = error.details.as_mut().and_then(Value::as_object_mut) {
                details.insert("jsonShape".to_owned(), Value::String(shape.clone()));
            }
            tracing::warn!(
                file_name = %file.file_name,
                json_shape = %shape,
                "MuMu 批次 JSON 外壳未识别"
            );
        }
        return Err(error);
    };

    let souls_value = souls_value.ok_or_else(|| {
        AppError::import_field(
            &file.file_name,
            souls_path,
            "JSON 数组",
            "缺失",
            "导入格式缺少御魂数组",
        )
    })?;
    let souls = souls_value.as_array().ok_or_else(|| {
        AppError::import_field(
            &file.file_name,
            souls_path,
            "JSON 数组",
            value_kind(souls_value),
            "御魂字段必须是数组",
        )
    })?;
    if souls.len() > MAX_IMPORT_SOULS {
        return Err(AppError::import_field(
            &file.file_name,
            souls_path,
            &format!("不超过 {MAX_IMPORT_SOULS} 枚"),
            &souls.len().to_string(),
            "单文件御魂数量超过安全上限",
        ));
    }

    let mut normalized = Vec::with_capacity(souls.len());
    for (index, soul) in souls.iter().enumerate() {
        normalized.push(parse_soul(&file.file_name, souls_path, index, soul)?);
    }

    // 桌面版与藏宝阁都明确提供玩家 heroes；其他导入格式没有玩家式神事实，避免误把任意字段猜成式神库存。
    let owned_shikigami = if matches!(
        format.as_str(),
        "yys-desktop-cache-v1" | "cbg-equip-v1" | "mumu-snapshot-v1"
    ) {
        parse_owned_shikigami(&file.file_name, metadata.get("heroes"))?
    } else {
        Vec::new()
    };

    // 参考快照的 currency/realm_cards 与桌面缓存同样写入当前数据；字段名同时兼容两种 JSON 命名风格。
    let (items, realm_cards) = if matches!(
        format.as_str(),
        "yys-desktop-cache-v1" | "cbg-equip-v1" | "mumu-snapshot-v1"
    ) {
        let items = metadata
            .get("currency")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        let realm_cards = metadata
            .get("realmCards")
            .or_else(|| metadata.get("realm_cards"))
            .filter(|value| value.is_array())
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        (items, realm_cards)
    } else {
        (
            Value::Object(serde_json::Map::new()),
            Value::Array(Vec::new()),
        )
    };

    let detected_completeness =
        detect_completeness(metadata).unwrap_or_else(|| "partial".to_owned());
    let completeness = validate_completeness_override(
        &file.file_name,
        completeness_override,
        &detected_completeness,
    )?;
    let scope_value = metadata
        .get("scope")
        .or_else(|| metadata.get("filter"))
        .or_else(|| metadata.get("range"));
    let scope_json = scope_value
        .map(canonical_json)
        .transpose()
        .map_err(|error| AppError::internal(error.to_string()))?;
    let scope_fingerprint = hash_text(scope_json.as_deref().unwrap_or(""));
    let content_fingerprint = fingerprint_souls(&normalized);
    let mut warnings = Vec::new();
    if detected_completeness != "complete" {
        warnings.push("来源未证明包含完整库存，已按局部快照处理".to_owned());
    }
    if normalized
        .iter()
        .any(|soul| soul.source_stable_id.is_none())
    {
        warnings.push("部分御魂没有稳定来源 ID；局部导入只会独立保存，不会自动合并".to_owned());
    }

    Ok(ParsedImport {
        format,
        source_kind,
        completeness,
        captured_at: get_string(
            metadata,
            &["capturedAt", "captured_at", "timestamp", "time"],
        ),
        game_version: get_string(metadata, &["gameVersion", "game_version"]),
        adapter_version: get_string(metadata, &["adapterVersion", "adapter_version"]),
        scope_json,
        scope_fingerprint,
        content_fingerprint,
        souls: normalized,
        owned_shikigami,
        items,
        realm_cards,
        warnings,
    })
}

/// 解析桌面版或藏宝阁 heroes 映射，保留式神等级、锁定、觉醒、皮肤和技能等级，但忽略经验与 equips 关系。
///
/// heroes 的键通常是玩家实例 ID，参考工具也可能返回带 id 的数组；二者统一转换为实例 ID，
/// 既能展示重复式神，也能在后续需要时稳定定位同一实例。缺少可选字段按未知处理。
fn parse_owned_shikigami(
    file_name: &str,
    heroes_value: Option<&Value>,
) -> Result<Vec<OwnedShikigami>, AppError> {
    let Some(heroes_value) = heroes_value else {
        return Ok(Vec::new());
    };
    let entries: Vec<(String, &Value)> = match heroes_value {
        Value::Object(heroes) => heroes
            .iter()
            .map(|(instance_id, value)| (instance_id.clone(), value))
            .collect(),
        Value::Array(heroes) => heroes
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let instance_id = value
                    .as_object()
                    .and_then(|record| {
                        optional_string(record, &["id", "instanceId", "instance_id", "uid"])
                    })
                    .unwrap_or_else(|| format!("hero-{index}"));
                (instance_id, value)
            })
            .collect(),
        _ => {
            return Err(AppError::import_field(
                file_name,
                "$.heroes",
                "JSON 对象或数组",
                value_kind(heroes_value),
                "heroes 字段必须是对象或数组",
            ));
        }
    };
    if entries.len() > MAX_IMPORT_SHIKIGAMI {
        return Err(AppError::import_field(
            file_name,
            "$.heroes",
            &format!("不超过 {MAX_IMPORT_SHIKIGAMI} 个"),
            &entries.len().to_string(),
            "单文件式神数量超过安全上限",
        ));
    }

    let mut normalized = Vec::with_capacity(entries.len());
    for (index, (instance_id, value)) in entries.iter().enumerate() {
        let path = if heroes_value.is_array() {
            format!("$.heroes[{index}]")
        } else {
            format!("$.heroes[\"{instance_id}\"]")
        };
        // 式神资料只作为附加展示信息；单条记录损坏时跳过该条，不能阻断御魂导入。
        let parsed = (|| -> Result<Option<OwnedShikigami>, AppError> {
            let record = value.as_object().ok_or_else(|| {
                AppError::import_field(
                    file_name,
                    &path,
                    "式神对象",
                    value_kind(value),
                    "heroes 记录必须是对象",
                )
            })?;
            let shikigami_id =
                required_string(file_name, &format!("{path}.heroId"), record, &["heroId"])?;
            if is_desktop_special_hero_id(&shikigami_id) {
                // 主角/特殊角色会复用 heroes 结构并出现 60 级等普通式神不适用的值；跳过整条记录。
                return Ok(None);
            }
            let star = required_u8(file_name, &format!("{path}.star"), record, &["star"])?;
            if !(1..=6).contains(&star) {
                return Err(AppError::import_field(
                    file_name,
                    &format!("{path}.star"),
                    "1 到 6 的整数",
                    &star.to_string(),
                    "式神星级超出支持范围",
                ));
            }
            let level = optional_u8_checked(file_name, &format!("{path}.level"), record, &["level"])?;
            if level.is_some_and(|value| value > 40) {
                return Err(AppError::import_field(
                    file_name,
                    &format!("{path}.level"),
                    "不超过 40 的整数",
                    &level.unwrap_or_default().to_string(),
                    "式神等级超出支持范围",
                ));
            }
            // 桌面版经验值可能带小数，且不参与当前分析；直接忽略，避免无关字段阻断整次导入。
            let exp = None;
            let locked =
                optional_bool_checked(file_name, &format!("{path}.lock"), record, &["lock"])?;
            let awakened =
                optional_bool_checked(file_name, &format!("{path}.awake"), record, &["awake"])?;
            let skin_id =
                optional_u64_checked(file_name, &format!("{path}.skinid"), record, &["skinid"])?;
            let skills = parse_owned_shikigami_skills(file_name, &path, record.get("skinfo"))?;
            let selected_skill_ids =
                parse_selected_skill_ids(file_name, &path, record.get("selectSkills"))?;

            Ok(Some(OwnedShikigami {
                instance_id: instance_id.clone(),
                shikigami_id,
                star,
                level,
                exp,
                locked,
                awakened,
                skin_id,
                skills,
                selected_skill_ids,
            }))
        })();
        match parsed {
            Ok(Some(hero)) => normalized.push(hero),
            Ok(None) => {}
            Err(error) => {
                // 只记录实例 ID 和错误文本，不把 heroes 原始内容写入日志；御魂导入继续进行。
                tracing::debug!(
                    file_name,
                    instance_id,
                    reason = %error.message,
                    "跳过结构异常的式神记录"
                );
            }
        }
    }
    Ok(normalized)
}

/// 解析 heroes.skinfo 的技能等级二元组；兼容藏宝阁单技能记录的扁平 `[技能 ID, 等级]` 形状，装备字段故意不进入归一化结构。
fn parse_owned_shikigami_skills(
    file_name: &str,
    hero_path: &str,
    value: Option<&Value>,
) -> Result<Vec<OwnedShikigamiSkill>, AppError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let raw_items = value.as_array().ok_or_else(|| {
        AppError::import_field(
            file_name,
            &format!("{hero_path}.skinfo"),
            "技能二元组数组",
            value_kind(value),
            "式神技能信息必须是数组",
        )
    })?;
    // 多技能记录是二元组数组；藏宝阁只有一条技能时会直接返回一层二元数组。
    let is_flat_pair = raw_items.first().is_some_and(|item| !item.is_array());
    let items: Vec<&Value> = if is_flat_pair {
        vec![value]
    } else {
        raw_items.iter().collect()
    };
    let mut skills = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let path = if is_flat_pair {
            format!("{hero_path}.skinfo")
        } else {
            format!("{hero_path}.skinfo[{index}]")
        };
        let item = *item;
        let pair = item.as_array().ok_or_else(|| {
            AppError::import_field(
                file_name,
                &path,
                "[技能 ID, 技能等级]",
                value_kind(item),
                "式神技能条目必须是二元数组",
            )
        })?;
        if pair.len() < 2 {
            return Err(AppError::import_field(
                file_name,
                &path,
                "至少包含技能 ID 和技能等级",
                &pair.len().to_string(),
                "式神技能条目长度不正确",
            ));
        }
        skills.push(OwnedShikigamiSkill {
            skill_id: parse_u64_value(file_name, &format!("{path}[0]"), &pair[0])?,
            level: parse_u8_value(file_name, &format!("{path}[1]"), &pair[1])?,
        });
    }
    Ok(skills)
}

/// 解析已选择技能 ID；它只用于展示培养选择，不与御魂装备关系混合。
fn parse_selected_skill_ids(
    file_name: &str,
    hero_path: &str,
    value: Option<&Value>,
) -> Result<Vec<u64>, AppError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value.as_array().ok_or_else(|| {
        AppError::import_field(
            file_name,
            &format!("{hero_path}.selectSkills"),
            "技能 ID 数组",
            value_kind(value),
            "式神已选技能必须是数组",
        )
    })?;
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            parse_u64_value(
                file_name,
                &format!("{hero_path}.selectSkills[{index}]"),
                item,
            )
        })
        .collect()
}

/// 解包 `mumu-raw-capture-v1` 原始采集文件，并按稳定御魂 ID 合并批次。
///
/// 采集脚本可能因为 Hook 回调收到重复批次；相同 ID 且内容完全一致时只保留一份，
/// 同一 ID 对应不同内容时直接报错，避免把不确定的新旧数据静默覆盖进当前库存。
fn parse_mumu_raw_capture(
    file: &ImportFileInput,
    object: &serde_json::Map<String, Value>,
    completeness_override: Option<&str>,
) -> Result<ParsedImport, AppError> {
    let batches_value = object.get("batches").ok_or_else(|| {
        AppError::import_field(
            &file.file_name,
            "$.batches",
            "原始采集批次数组",
            "缺失",
            "MuMu 原始采集文件缺少 batches",
        )
    })?;
    let batches = batches_value.as_array().ok_or_else(|| {
        AppError::import_field(
            &file.file_name,
            "$.batches",
            "原始采集批次数组",
            value_kind(batches_value),
            "MuMu 原始采集文件的 batches 必须是数组",
        )
    })?;
    if batches.is_empty() {
        return Err(AppError::import_field(
            &file.file_name,
            "$.batches",
            "至少 1 批原始数据",
            "空数组",
            "MuMu 原始采集文件还没有可导入的御魂批次",
        ));
    }

    let mut merged = serde_json::Map::new();
    for (batch_index, batch_value) in batches.iter().enumerate() {
        let batch_path = format!("$.batches[{batch_index}]");
        let batch = batch_value.as_object().ok_or_else(|| {
            AppError::import_field(
                &file.file_name,
                &batch_path,
                "批次对象",
                value_kind(batch_value),
                "MuMu 原始采集批次必须是对象",
            )
        })?;
        let payload_path = format!("{batch_path}.payload");
        let payload = batch.get("payload").ok_or_else(|| {
            AppError::import_field(
                &file.file_name,
                &payload_path,
                "压缩御魂对象",
                "缺失",
                "MuMu 原始采集批次缺少 payload",
            )
        })?;
        let payload_object = payload.as_object().ok_or_else(|| {
            AppError::import_field(
                &file.file_name,
                &payload_path,
                "压缩御魂对象",
                value_kind(payload),
                "MuMu 原始采集批次的 payload 必须是对象",
            )
        })?;
        if !looks_like_mumu_compact_batch(payload_object) {
            return Err(AppError::import_field(
                &file.file_name,
                &payload_path,
                "带 base_r/base_rindex/others/rattr/single_attr 的压缩御魂对象",
                "字段签名不匹配",
                "MuMu 原始采集批次不是当前已验证的压缩格式",
            ));
        }

        for (stable_id, record) in payload_object {
            if let Some(existing) = merged.get(stable_id) {
                if existing != record {
                    return Err(AppError::import_field(
                        &file.file_name,
                        &format!("{payload_path}[\"{stable_id}\"]"),
                        "与已合并批次内容一致的御魂记录",
                        "同一 ID 内容不同",
                        "MuMu 原始采集批次存在冲突记录，已停止导入以保护当前库存",
                    ));
                }
                continue;
            }
            if merged.len() >= MAX_IMPORT_SOULS {
                return Err(AppError::import_field(
                    &file.file_name,
                    &payload_path,
                    &format!("合并后不超过 {MAX_IMPORT_SOULS} 枚御魂"),
                    &(merged.len() + 1).to_string(),
                    "MuMu 原始采集批次合并后超过安全上限",
                ));
            }
            merged.insert(stable_id.clone(), record.clone());
        }
    }

    let mut parsed = parse_mumu_compact_batch(file, &merged, completeness_override)?;
    parsed.format = "mumu-raw-capture-v1".to_owned();
    parsed.captured_at = get_string(object, &["capturedAt", "captured_at"]);
    parsed.game_version = get_string(object, &["gameVersion", "game_version"]);
    // Hook 读到的是游戏本次计算返回的筛选结果，不等同于完整背包；提醒用户检查 2/4/6 号位筛选范围。
    parsed.warnings.push(
        "MuMu 原始批次来自游戏计算结果；请确认已选择全部主属性，否则未返回的速度、命中/抵抗或暴击/暴伤不会出现在库存中"
            .to_owned(),
    );
    if batches.len() > 1 {
        parsed.warnings.push(format!(
            "原始采集包含 {} 批数据，已按御魂 ID 合并重复记录，最终导入 {} 枚",
            batches.len(),
            merged.len()
        ));
    }
    Ok(parsed)
}

/// 判断顶层对象是否符合痒痒鼠魔方内部压缩批次的稳定字段签名；这里只识别，不在此处容忍字段错误。
fn looks_like_mumu_compact_batch(object: &serde_json::Map<String, Value>) -> bool {
    if object.is_empty()
        || !object
            .keys()
            .all(|id| id.len() == 24 && id.bytes().all(|character| character.is_ascii_hexdigit()))
    {
        return false;
    }

    // 至少一条记录同时带有五个压缩字段，才把整个对象交给严格解码器，避免猜测任意 ID 映射。
    object.values().any(|value| {
        value.as_object().is_some_and(|record| {
            ["base_r", "base_rindex", "others", "rattr", "single_attr"]
                .iter()
                .all(|field| record.contains_key(*field))
        })
    })
}

/// 本地解码痒痒鼠魔方压缩批次；该路径不调用第三方服务器，所有字段均按已验证的位布局转换。
fn parse_mumu_compact_batch(
    file: &ImportFileInput,
    object: &serde_json::Map<String, Value>,
    completeness_override: Option<&str>,
) -> Result<ParsedImport, AppError> {
    if object.len() > MAX_IMPORT_SOULS {
        return Err(AppError::import_field(
            &file.file_name,
            "$",
            &format!("不超过 {MAX_IMPORT_SOULS} 枚"),
            &object.len().to_string(),
            "单文件御魂数量超过安全上限",
        ));
    }

    let mut souls = Vec::with_capacity(object.len());
    for (stable_id, value) in object {
        let path = format!("$[\"{stable_id}\"]");
        if stable_id.len() != 24
            || !stable_id
                .bytes()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Err(AppError::import_field(
                &file.file_name,
                "$",
                "24 位十六进制御魂 ID",
                stable_id,
                "痒痒鼠魔方御魂 ID 不合法",
            ));
        }
        let record = value.as_object().ok_or_else(|| {
            AppError::import_field(
                &file.file_name,
                &path,
                "压缩御魂对象",
                value_kind(value),
                "痒痒鼠魔方御魂记录必须是对象",
            )
        })?;

        // base_r=1 是当前导出器六星御魂格式标记；其他值尚未被参考实现验证，拒绝静默误算。
        let base_r_path = format!("{path}.base_r");
        let base_r_value = record.get("base_r").ok_or_else(|| {
            AppError::import_field(
                &file.file_name,
                &base_r_path,
                "数字 1",
                "缺失",
                "压缩御魂缺少星级格式标记",
            )
        })?;
        let base_r = base_r_value.as_f64().ok_or_else(|| {
            AppError::import_field(
                &file.file_name,
                &base_r_path,
                "数字 1",
                value_kind(base_r_value),
                "压缩御魂星级格式标记类型不正确",
            )
        })?;
        if (base_r - 1.0).abs() > f64::EPSILON {
            return Err(AppError::import_field(
                &file.file_name,
                &base_r_path,
                "当前已验证的六星格式值 1",
                &base_r.to_string(),
                "暂不支持该星级的痒痒鼠魔方压缩御魂",
            ));
        }

        let base_index = compact_required_u64(&file.file_name, &path, record, "base_rindex")?;
        let others = compact_required_u64(&file.file_name, &path, record, "others")?;
        let format_marker = (others >> 32) & 0xffff;
        if others >> 52 != 0 || format_marker != 0xc249 {
            return Err(AppError::import_field(
                &file.file_name,
                &format!("{path}.others"),
                "已验证的痒痒鼠魔方位布局（标记 0xC249）",
                &format!("0x{format_marker:04X}"),
                "压缩御魂版本标记不受支持",
            ));
        }

        let equip_template_id = others & 0xfffff;
        let slot = mumu_slot_from_template(equip_template_id).ok_or_else(|| {
            AppError::import_field(
                &file.file_name,
                &format!("{path}.others"),
                "已识别的御魂模板 ID",
                &equip_template_id.to_string(),
                "暂不支持该痒痒鼠魔方御魂模板",
            )
        })?;
        let raw_suit_code = (others >> 20) & 0xfff;
        let suit_code = raw_suit_code.checked_sub(992).ok_or_else(|| {
            AppError::import_field(
                &file.file_name,
                &format!("{path}.others"),
                "不小于 992 的套装位编码",
                &raw_suit_code.to_string(),
                "压缩御魂套装位编码不合法",
            )
        })?;
        let set_id = mumu_set_name(suit_code).ok_or_else(|| {
            AppError::import_field(
                &file.file_name,
                &format!("{path}.others"),
                "已识别的御魂套装编码",
                &suit_code.to_string(),
                "暂不支持该痒痒鼠魔方御魂套装",
            )
        })?;
        let level = ((others >> 48) & 0xf) as u8;
        let (main_attr_type, main_attr_value) =
            mumu_main_attribute(&file.file_name, &path, slot, base_index, level)?;
        let attributes = parse_mumu_compact_attributes(&file.file_name, &path, record)?;

        souls.push(NormalizedSoul {
            source_stable_id: Some(stable_id.clone()),
            identity_quality: "stable".to_owned(),
            set_id: set_id.to_owned(),
            slot,
            quality: 6,
            level,
            main_attr_type: main_attr_type.to_owned(),
            main_attr_value,
            // 压缩格式只保存最终随机属性倍率，无法可靠反推出初始条数和每条强化次数。
            initial_substat_count: None,
            locked_in_source: None,
            equipped_state: None,
            attributes,
            source_json: serde_json::to_string(value)
                .map_err(|error| AppError::internal(error.to_string()))?,
        });
    }

    let detected_completeness = "partial".to_owned();
    let completeness = validate_completeness_override(
        &file.file_name,
        completeness_override,
        &detected_completeness,
    )?;
    let scope_fingerprint = hash_text("");
    let content_fingerprint = fingerprint_souls(&souls);
    Ok(ParsedImport {
        format: "mumu-compact-v1".to_owned(),
        source_kind: "mumu".to_owned(),
        completeness,
        captured_at: None,
        game_version: None,
        adapter_version: None,
        scope_json: None,
        scope_fingerprint,
        content_fingerprint,
        souls,
        owned_shikigami: Vec::new(),
        items: Value::Object(serde_json::Map::new()),
        realm_cards: Value::Array(Vec::new()),
        warnings: vec![
            "游戏内计算器可能受当前筛选条件影响，已按局部快照处理".to_owned(),
            "压缩格式不包含锁定、装备式神和精确强化次数，这些字段将保留为未知".to_owned(),
        ],
    })
}

/// 从压缩记录读取必填无符号整数，并把缺失或类型错误定位到具体御魂字段。
fn compact_required_u64(
    file_name: &str,
    path: &str,
    record: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<u64, AppError> {
    let field_path = format!("{path}.{field}");
    let value = record.get(field).ok_or_else(|| {
        AppError::import_field(
            file_name,
            &field_path,
            "非负整数",
            "缺失",
            "压缩御魂缺少必填字段",
        )
    })?;
    value.as_u64().ok_or_else(|| {
        AppError::import_field(
            file_name,
            &field_path,
            "非负整数",
            value_kind(value),
            "压缩御魂字段类型不正确",
        )
    })
}

/// 从模板编号还原 1—6 号位；常规模板按编号族计算，活动模板仅接受已在真实批次中验证的编号。
fn mumu_slot_from_template(template_id: u64) -> Option<u8> {
    let template_family = template_id / 10_000;
    if (11..=16).contains(&template_family) {
        return Some((template_family - 10) as u8);
    }
    // 目前已核验的 190xxx 活动模板均以末位 1—6 直接表示号位，按该稳定规则覆盖同组新增模板。
    if (190_000..200_000).contains(&template_id) {
        let slot = template_id % 10;
        if (1..=6).contains(&slot) {
            return Some(slot as u8);
        }
    }
    match template_id {
        180_001 | 180_013 => Some(1),
        180_020 => Some(2),
        180_003 | 180_015 => Some(3),
        180_016 => Some(4),
        180_005 | 180_011 => Some(5),
        180_027 => Some(6),
        _ => None,
    }
}

/// 按阴阳师号位规则和原始主属性索引还原主属性；数值公式来自参考转换器的 0—15 级合成探针结果。
/// 1/3/5 号位分别固定为攻击/防御/生命；2 号位允许攻击、防御、生命、速度；
/// 4 号位允许攻击、防御、生命、效果命中、效果抵抗；6 号位允许攻击、防御、生命、暴击、暴伤。
fn mumu_main_attribute(
    file_name: &str,
    path: &str,
    slot: u8,
    base_index: u64,
    level: u8,
) -> Result<(&'static str, f64), AppError> {
    let attribute_type = match (slot, base_index) {
        (1, 0) => "attack_flat",
        (2, 0) => "attack_rate",
        (2, 1) => "defense_rate",
        (2, 2) => "hp_rate",
        (2, 3) => "speed",
        (3, 1) => "defense_flat",
        (4, 0) => "attack_rate",
        (4, 1) => "defense_rate",
        (4, 2) => "hp_rate",
        (4, 3) => "effect_hit",
        (4, 4) => "effect_resist",
        (5, 2) => "hp_flat",
        (6, 0) => "attack_rate",
        (6, 1) => "defense_rate",
        (6, 2) => "hp_rate",
        (6, 3) => "crit_damage",
        (6, 4) => "crit_rate",
        _ => {
            return Err(AppError::import_field(
                file_name,
                &format!("{path}.base_rindex"),
                "与号位匹配的主属性索引",
                &base_index.to_string(),
                "压缩御魂主属性索引与号位不匹配",
            ));
        }
    };
    let value = match attribute_type {
        "attack_flat" => 81.0 + f64::from(level) * 27.0,
        "defense_flat" => 14.0 + f64::from(level) * 6.0,
        "hp_flat" => 342.0 + f64::from(level) * 114.0,
        "speed" => 12.0 + f64::from(level) * 3.0,
        "crit_damage" => 0.14 + f64::from(level) * 0.05,
        _ => 0.10 + f64::from(level) * 0.03,
    };
    Ok((attribute_type, value))
}

/// 解码随机属性和固有属性；随机倍率保留原始精度，固有属性单独标记为 fixed。
fn parse_mumu_compact_attributes(
    file_name: &str,
    path: &str,
    record: &serde_json::Map<String, Value>,
) -> Result<Vec<NormalizedAttribute>, AppError> {
    let rattr_path = format!("{path}.rattr");
    let rattr_value = record.get("rattr").ok_or_else(|| {
        AppError::import_field(
            file_name,
            &rattr_path,
            "随机属性数组",
            "缺失",
            "压缩御魂缺少随机属性",
        )
    })?;
    let rattrs = rattr_value.as_array().ok_or_else(|| {
        AppError::import_field(
            file_name,
            &rattr_path,
            "随机属性数组",
            value_kind(rattr_value),
            "压缩御魂随机属性必须是数组",
        )
    })?;
    if rattrs.len() + 1 > MAX_ATTRIBUTES_PER_SOUL {
        return Err(AppError::import_field(
            file_name,
            &rattr_path,
            &format!("不超过 {} 条随机属性", MAX_ATTRIBUTES_PER_SOUL - 1),
            &rattrs.len().to_string(),
            "单枚压缩御魂属性数量超过安全上限",
        ));
    }

    let mut attributes = Vec::with_capacity(rattrs.len() + 1);
    for (index, raw_attribute) in rattrs.iter().enumerate() {
        let attribute_path = format!("{rattr_path}[{index}]");
        let pair = raw_attribute.as_array().ok_or_else(|| {
            AppError::import_field(
                file_name,
                &attribute_path,
                "[属性编码, 倍率]",
                value_kind(raw_attribute),
                "压缩御魂随机属性条目必须是二元数组",
            )
        })?;
        if pair.len() != 2 {
            return Err(AppError::import_field(
                file_name,
                &attribute_path,
                "长度为 2 的数组",
                &format!("长度 {}", pair.len()),
                "压缩御魂随机属性条目长度不正确",
            ));
        }
        let code = pair[0].as_u64().ok_or_else(|| {
            AppError::import_field(
                file_name,
                &format!("{attribute_path}[0]"),
                "0 到 10 的整数编码",
                value_kind(&pair[0]),
                "压缩御魂随机属性编码类型不正确",
            )
        })?;
        let ratio = pair[1].as_f64().ok_or_else(|| {
            AppError::import_field(
                file_name,
                &format!("{attribute_path}[1]"),
                "非负数字倍率",
                value_kind(&pair[1]),
                "压缩御魂随机属性倍率类型不正确",
            )
        })?;
        if ratio < 0.0 {
            return Err(AppError::import_field(
                file_name,
                &format!("{attribute_path}[1]"),
                "非负数字倍率",
                &ratio.to_string(),
                "压缩御魂随机属性倍率不能为负数",
            ));
        }
        let (attribute_type, scale) = mumu_random_attribute(code).ok_or_else(|| {
            AppError::import_field(
                file_name,
                &format!("{attribute_path}[0]"),
                "0 到 10 的属性编码",
                &code.to_string(),
                "压缩御魂随机属性编码不受支持",
            )
        })?;
        // 紧凑倍率的整数部分表示该副属性被强化命中的次数；小数部分保留本次属性的成长倍率。
        let enhancement_count = mumu_compact_enhancement_count(ratio);
        attributes.push(NormalizedAttribute {
            attribute_index: index as u8,
            attribute_type: attribute_type.to_owned(),
            value: ratio * scale,
            enhancement_count,
            fixed_attribute: false,
        });
    }

    let single_path = format!("{path}.single_attr");
    let single = record.get("single_attr").ok_or_else(|| {
        AppError::import_field(
            file_name,
            &single_path,
            "null 或 1 到 6 的固有属性编码",
            "缺失",
            "压缩御魂缺少固有属性字段",
        )
    })?;
    if !single.is_null() {
        let code = single.as_u64().ok_or_else(|| {
            AppError::import_field(
                file_name,
                &single_path,
                "null 或 1 到 6 的固有属性编码",
                value_kind(single),
                "压缩御魂固有属性编码类型不正确",
            )
        })?;
        let (attribute_type, value) = mumu_single_attribute(code).ok_or_else(|| {
            AppError::import_field(
                file_name,
                &single_path,
                "1 到 6 的固有属性编码",
                &code.to_string(),
                "压缩御魂固有属性编码不受支持",
            )
        })?;
        attributes.push(NormalizedAttribute {
            attribute_index: attributes.len() as u8,
            attribute_type: attribute_type.to_owned(),
            value,
            enhancement_count: None,
            fixed_attribute: true,
        });
    }
    Ok(attributes)
}

/// 从紧凑倍率提取副属性强化次数；固有属性不经过此函数，避免把固有属性误显示成强化次数。
fn mumu_compact_enhancement_count(ratio: f64) -> Option<u8> {
    if !ratio.is_finite() || ratio < 0.0 {
        return None;
    }
    let count = ratio.floor();
    if count > f64::from(u8::MAX) {
        return None;
    }
    Some(count as u8)
}

/// 将随机属性编码转换为内部属性 ID 和单位倍率。
fn mumu_random_attribute(code: u64) -> Option<(&'static str, f64)> {
    match code {
        0 => Some(("hp_flat", 114.0)),
        1 => Some(("defense_flat", 5.0)),
        2 => Some(("attack_flat", 27.0)),
        3 => Some(("hp_rate", 0.03)),
        4 => Some(("defense_rate", 0.03)),
        5 => Some(("attack_rate", 0.03)),
        6 => Some(("speed", 3.0)),
        7 => Some(("crit_rate", 0.03)),
        8 => Some(("crit_damage", 0.04)),
        9 => Some(("effect_hit", 0.04)),
        10 => Some(("effect_resist", 0.04)),
        _ => None,
    }
}

/// 将固有属性编码转换为内部属性 ID 和固定数值；该表按参考转换器实际输出核验。
fn mumu_single_attribute(code: u64) -> Option<(&'static str, f64)> {
    match code {
        1 => Some(("attack_rate", 0.08)),
        2 => Some(("hp_rate", 0.08)),
        3 => Some(("defense_rate", 0.16)),
        4 => Some(("crit_rate", 0.08)),
        5 => Some(("effect_hit", 0.08)),
        6 => Some(("effect_resist", 0.08)),
        _ => None,
    }
}

/// 将压缩套装编码转换为应用使用的中文稳定套装 ID；编码来自参考网页公开目录。
fn mumu_set_name(code: u64) -> Option<&'static str> {
    crate::domain::builtin_soul_set_name_by_code(code)
}

/// 解析一个御魂对象，并在错误中指出具体文件和数组下标。
fn parse_soul(
    file_name: &str,
    souls_path: &str,
    index: usize,
    value: &Value,
) -> Result<NormalizedSoul, AppError> {
    let path = format!("{souls_path}[{index}]");
    let object = value.as_object().ok_or_else(|| {
        AppError::import_field(
            file_name,
            &path,
            "JSON 对象",
            value_kind(value),
            "单枚御魂必须是 JSON 对象",
        )
    })?;
    let set_id = required_string(
        file_name,
        &format!("{path}.setId"),
        object,
        &["setId", "set_id", "suit", "suitName", "type", "kind"],
    )?;
    let mut slot = required_u8(
        file_name,
        &format!("{path}.slot"),
        object,
        &["slot", "pos", "position", "place"],
    )?;
    // 痒痒鼠标准格式同时带 kind、suit_id 和零基 pos；只在该明确签名下转换为内部 1—6 号位。
    if object.get("slot").is_none()
        && object.get("pos").is_some()
        && object.get("kind").is_some()
        && object.get("suit_id").is_some()
    {
        slot = slot.checked_add(1).ok_or_else(|| {
            AppError::import_field(
                file_name,
                &format!("{path}.pos"),
                "0 到 5",
                &slot.to_string(),
                "痒痒鼠标准格式号位超出范围",
            )
        })?;
    }
    if !(1..=6).contains(&slot) {
        return Err(AppError::import_field(
            file_name,
            &format!("{path}.slot"),
            "1 到 6",
            &slot.to_string(),
            "御魂号位必须在 1 到 6 之间",
        ));
    }
    let quality = required_u8(
        file_name,
        &format!("{path}.quality"),
        object,
        &["quality", "star", "stars", "grade"],
    )?;
    if !(1..=6).contains(&quality) {
        return Err(AppError::import_field(
            file_name,
            &format!("{path}.quality"),
            "1 到 6",
            &quality.to_string(),
            "御魂星级必须在 1 到 6 之间",
        ));
    }
    let level = required_u8(
        file_name,
        &format!("{path}.level"),
        object,
        &["level", "lv", "strengthenLevel"],
    )?;
    if level > 15 {
        return Err(AppError::import_field(
            file_name,
            &format!("{path}.level"),
            "0 到 15",
            &level.to_string(),
            "御魂等级必须在 0 到 15 之间",
        ));
    }

    let (main_attr_type, main_attr_value) = parse_main_attribute(file_name, &path, object)?;
    let raw_attributes = object
        .get("subAttributes")
        .or_else(|| object.get("sub_attributes"))
        .or_else(|| object.get("subAttrs"))
        .or_else(|| object.get("attrs"))
        .or_else(|| object.get("attributes"))
        .or_else(|| object.get("secondary_attributes"))
        .or_else(|| object.get("random_attrs"))
        .or_else(|| object.get("副属性"));
    let mut attributes =
        parse_attributes(file_name, &format!("{path}.subAttributes"), raw_attributes)?;
    if let Some(single_attributes) = object.get("single_attrs") {
        // 痒痒鼠把套装固有属性放在独立数组中；合并后重排索引，并保留 fixed 语义供规则层区分。
        let mut fixed = parse_attributes(
            file_name,
            &format!("{path}.single_attrs"),
            Some(single_attributes),
        )?;
        if attributes.len() + fixed.len() > MAX_ATTRIBUTES_PER_SOUL {
            return Err(AppError::import_field(
                file_name,
                &format!("{path}.single_attrs"),
                &format!("合计不超过 {MAX_ATTRIBUTES_PER_SOUL} 条属性"),
                &(attributes.len() + fixed.len()).to_string(),
                "单枚御魂属性数量超过安全上限",
            ));
        }
        for attribute in &mut fixed {
            attribute.attribute_index = attributes.len() as u8;
            attribute.fixed_attribute = true;
            attributes.push(attribute.clone());
        }
    }
    let explicit_initial_count = optional_u8(
        object,
        &[
            "initialSubstatCount",
            "initial_substat_count",
            "initialAttributeCount",
            "baseAttrCount",
        ],
    );
    let initial_substat_count =
        explicit_initial_count.or_else(|| (level == 0).then_some(attributes.len() as u8));
    let source_stable_id = optional_string(
        object,
        &[
            "id", "equipId", "equip_id", "uuid", "uid", "soulId", "stableId",
        ],
    );
    let identity_quality = if source_stable_id.is_some() {
        "stable"
    } else if optional_string(object, &["derivedId", "fingerprint"]).is_some() {
        "derived"
    } else {
        "missing"
    }
    .to_owned();

    Ok(NormalizedSoul {
        source_stable_id,
        identity_quality,
        set_id,
        slot,
        quality,
        level,
        main_attr_type,
        main_attr_value,
        initial_substat_count,
        locked_in_source: optional_bool(object, &["locked", "isLocked", "lock"]),
        equipped_state: optional_string(
            object,
            &["equippedState", "equipped_state", "equipState", "equipped"],
        ),
        attributes,
        source_json: serde_json::to_string(value)
            .map_err(|error| AppError::internal(error.to_string()))?,
    })
}

/// 解析主属性对象或扁平字段，兼容三类来源常见的对象/字符串表达。
fn parse_main_attribute(
    file_name: &str,
    path: &str,
    object: &serde_json::Map<String, Value>,
) -> Result<(String, f64), AppError> {
    let main = object
        .get("mainAttr")
        .or_else(|| object.get("main_attr"))
        .or_else(|| object.get("mainAttribute"))
        .or_else(|| object.get("baseAttr"))
        .or_else(|| object.get("base_attr"));
    if let Some(main) = main {
        if let Some(main_object) = main.as_object() {
            if let Some((name, value)) = object_attribute_pair(main_object) {
                return Ok((
                    normalize_attribute_type(name),
                    normalize_attribute_value(
                        &normalize_attribute_type(name),
                        parse_number_value(file_name, &format!("{path}.mainAttr.value"), value)?,
                    ),
                ));
            }
        } else if let Some(name) = main.as_str() {
            let value = object
                .get("mainAttrValue")
                .or_else(|| object.get("main_attr_value"))
                .or_else(|| object.get("baseAttrValue"))
                .ok_or_else(|| {
                    AppError::import_field(
                        file_name,
                        &format!("{path}.mainAttrValue"),
                        "数字",
                        "缺失",
                        "主属性缺少数值",
                    )
                })?;
            let normalized_name = normalize_attribute_type(name);
            return Ok((
                normalized_name.clone(),
                normalize_attribute_value(
                    &normalized_name,
                    parse_number_value(file_name, &format!("{path}.mainAttrValue"), value)?,
                ),
            ));
        }
    }

    let name = required_string(
        file_name,
        &format!("{path}.mainAttrType"),
        object,
        &["mainAttrType", "main_attr_type", "mainAttributeType"],
    )?;
    let value = object
        .get("mainAttrValue")
        .or_else(|| object.get("main_attr_value"))
        .or_else(|| object.get("mainAttributeValue"))
        .ok_or_else(|| {
            AppError::import_field(
                file_name,
                &format!("{path}.mainAttrValue"),
                "数字",
                "缺失",
                "主属性缺少数值",
            )
        })?;
    let normalized_name = normalize_attribute_type(&name);
    Ok((
        normalized_name.clone(),
        normalize_attribute_value(
            &normalized_name,
            parse_number_value(file_name, &format!("{path}.mainAttrValue"), value)?,
        ),
    ))
}

/// 解析副属性数组、单个属性对象或“属性名到数值”的对象映射。
fn parse_attributes(
    file_name: &str,
    path: &str,
    value: Option<&Value>,
) -> Result<Vec<NormalizedAttribute>, AppError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let entries: Vec<(String, &Value)> = match value {
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let object = item.as_object().ok_or_else(|| {
                    AppError::import_field(
                        file_name,
                        &format!("{path}[{index}]"),
                        "JSON 对象",
                        value_kind(item),
                        "副属性条目必须是对象",
                    )
                })?;
                let (name, _) = object_attribute_pair(object).ok_or_else(|| {
                    AppError::import_field(
                        file_name,
                        &format!("{path}[{index}].type"),
                        "属性名",
                        "缺失",
                        "副属性缺少属性类型",
                    )
                })?;
                Ok((name.to_owned(), item))
            })
            .collect::<Result<Vec<_>, AppError>>()?,
        Value::Object(object) if object_attribute_pair(object).is_some() => vec![(
            object_attribute_pair(object)
                .expect("前置条件已验证")
                .0
                .to_owned(),
            value,
        )],
        Value::Object(object) => object
            .iter()
            .map(|(name, value)| (name.clone(), value))
            .collect(),
        _ => {
            return Err(AppError::import_field(
                file_name,
                path,
                "JSON 数组或对象",
                value_kind(value),
                "副属性必须是数组或对象",
            ));
        }
    };

    // 副属性数量是固定的小集合；先限制条目数，避免恶意对象映射制造超大中间 Vec。
    if entries.len() > MAX_ATTRIBUTES_PER_SOUL {
        return Err(AppError::import_field(
            file_name,
            path,
            &format!("不超过 {MAX_ATTRIBUTES_PER_SOUL} 条副属性"),
            &entries.len().to_string(),
            "单枚御魂副属性数量超过安全上限",
        ));
    }

    entries
        .into_iter()
        .enumerate()
        .map(|(index, (fallback_name, raw))| {
            let (name, value, enhancement_count, fixed_attribute) = if let Some(object) =
                raw.as_object()
            {
                let (name, value) = object_attribute_pair(object).ok_or_else(|| {
                    AppError::import_field(
                        file_name,
                        &format!("{path}[{index}]"),
                        "属性名和数值",
                        "缺失",
                        "副属性缺少属性名或数值",
                    )
                })?;
                (
                    name.to_owned(),
                    value,
                    optional_u8(
                        object,
                        &[
                            "enhancementCount",
                            "enhancement_count",
                            "enhanceCount",
                            "upCount",
                            "levelUpCount",
                            "times",
                        ],
                    ),
                    optional_bool(object, &["fixed", "fixedAttribute", "isFixed"]).unwrap_or(false),
                )
            } else {
                (fallback_name, raw, None, false)
            };
            let normalized_name = normalize_attribute_type(&name);
            let number = parse_number_value(file_name, &format!("{path}[{index}].value"), value)?;
            Ok(NormalizedAttribute {
                attribute_index: index as u8,
                attribute_type: normalized_name.clone(),
                value: normalize_attribute_value(&normalized_name, number),
                enhancement_count,
                fixed_attribute,
            })
        })
        .collect()
}

/// 将不同来源的字段名映射到规则系统的稳定属性 ID。
fn normalize_attribute_type(value: &str) -> String {
    let compact = value
        .trim()
        .to_ascii_lowercase()
        .replace(['_', '-', ' ', '%'], "");
    match compact.as_str() {
        "攻击" | "攻击加成" | "attackrate" | "atkpercent" | "atk" => "attack_rate".to_owned(),
        "固定攻击" | "攻击力" | "attack" | "attackflat" => "attack_flat".to_owned(),
        "生命" | "生命加成" | "hppercent" | "hprate" => "hp_rate".to_owned(),
        "固定生命" | "hp" | "hpflat" => "hp_flat".to_owned(),
        "防御" | "防御加成" | "defpercent" | "defenserate" => "defense_rate".to_owned(),
        "固定防御" | "defense" | "defflat" | "defenseflat" => "defense_flat".to_owned(),
        "速度" | "spd" | "speed" => "speed".to_owned(),
        "暴击" | "暴击率" | "crit" | "critrate" => "crit_rate".to_owned(),
        "暴伤" | "暴击伤害" | "critpower" | "critdamage" | "cdmg" => "crit_damage".to_owned(),
        "命中" | "效果命中" | "effecthit" | "effecthitrate" | "hit" => {
            "effect_hit".to_owned()
        }
        "抵抗" | "效果抵抗" | "effectresist" | "effectresistrate" | "resist" => {
            "effect_resist".to_owned()
        }
        _ => value.trim().to_owned(),
    }
}

/// 百分比属性统一存为小数；带百分号或大于 1 的百分数按来源展示值转换。
fn normalize_attribute_value(attribute_type: &str, value: f64) -> f64 {
    if matches!(
        attribute_type,
        "attack_rate"
            | "hp_rate"
            | "defense_rate"
            | "crit_rate"
            | "crit_damage"
            | "effect_hit"
            | "effect_resist"
    ) && value.abs() > 1.0
    {
        value / 100.0
    } else {
        value
    }
}

/// 将中间态转换为带快照 ID 的关系事实；属性强化次数未知时保留 NULL。
fn build_snapshot_facts(
    snapshot_id: &str,
    souls: &[NormalizedSoul],
) -> (Vec<SnapshotSoul>, Vec<SoulAttribute>) {
    let mut snapshot_souls = Vec::with_capacity(souls.len());
    let mut attributes = Vec::new();
    for soul in souls {
        let internal_id = Uuid::new_v4().to_string();
        snapshot_souls.push(SnapshotSoul {
            snapshot_id: snapshot_id.to_owned(),
            internal_id: internal_id.clone(),
            source_stable_id: soul.source_stable_id.clone(),
            identity_quality: soul.identity_quality.clone(),
            set_id: soul.set_id.clone(),
            slot: soul.slot,
            quality: soul.quality,
            level: soul.level,
            main_attr_type: soul.main_attr_type.clone(),
            main_attr_value: soul.main_attr_value,
            initial_substat_count: soul.initial_substat_count,
            locked_in_source: soul.locked_in_source,
            equipped_state: soul.equipped_state.clone(),
            source_json: Some(soul.source_json.clone()),
        });
        attributes.extend(soul.attributes.iter().map(|attribute| {
            SoulAttribute {
                snapshot_id: snapshot_id.to_owned(),
                soul_internal_id: internal_id.clone(),
                attribute_index: attribute.attribute_index,
                attribute_type: attribute.attribute_type.clone(),
                value: attribute.value,
                enhancement_count: attribute.enhancement_count,
                count_provenance: if attribute.enhancement_count.is_some() {
                    "source"
                } else {
                    "unknown"
                }
                .to_owned(),
                fixed_attribute: attribute.fixed_attribute,
            }
        }));
    }
    (snapshot_souls, attributes)
}

/// 用不依赖来源字段顺序的规范事实计算内容指纹。
fn fingerprint_souls(souls: &[NormalizedSoul]) -> String {
    let mut fingerprints = souls
        .iter()
        .map(|soul| {
            let mut attributes = soul
                .attributes
                .iter()
                .map(|attribute| {
                    serde_json::json!({
                        "type": attribute.attribute_type,
                        "value": attribute.value,
                        "enhancementCount": attribute.enhancement_count,
                        "fixed": attribute.fixed_attribute,
                    })
                })
                .collect::<Vec<_>>();
            attributes
                .sort_by_key(|attribute| serde_json::to_string(attribute).unwrap_or_default());
            serde_json::json!({
                "id": soul.source_stable_id,
                "identityQuality": soul.identity_quality,
                "setId": soul.set_id,
                "slot": soul.slot,
                "quality": soul.quality,
                "level": soul.level,
                "mainAttrType": soul.main_attr_type,
                "mainAttrValue": soul.main_attr_value,
                "initialSubstatCount": soul.initial_substat_count,
                "locked": soul.locked_in_source,
                "equippedState": soul.equipped_state,
                "attributes": attributes,
            })
        })
        .collect::<Vec<_>>();
    fingerprints.sort_by_key(|fingerprint| serde_json::to_string(fingerprint).unwrap_or_default());
    hash_text(&serde_json::to_string(&fingerprints).unwrap_or_default())
}

/// 生成稳定 SHA-256 文本摘要。
fn hash_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

/// 对 JSON 对象按键排序后序列化，保证范围字段顺序变化不改变指纹。
fn canonical_json(value: &Value) -> Result<String, serde_json::Error> {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(object) => Value::Object(
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), canonical(value)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
            _ => value.clone(),
        }
    }
    serde_json::to_string(&canonical(value))
}

/// 读取对象中的字符串字段，并兼容数字 ID/时间戳。
fn get_string(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| optional_string(object, &[*key]))
}

/// 从对象中读取第一个别名字段的字符串值。
fn optional_string(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| match value {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
}

/// 读取必须存在且必须是字符串/数字的字段。
fn required_string(
    file_name: &str,
    path: &str,
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Result<String, AppError> {
    let value = keys
        .iter()
        .find_map(|key| object.get(*key))
        .ok_or_else(|| AppError::import_field(file_name, path, "字符串", "缺失", "缺少必需字段"))?;
    optional_string(object, keys).ok_or_else(|| {
        AppError::import_field(
            file_name,
            path,
            "字符串或数字",
            value_kind(value),
            "字段类型不正确",
        )
    })
}

/// 读取可选布尔字段，数字 0/1 也按兼容格式转换。
fn optional_bool(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::Number(value) => value.as_i64().map(|number| number != 0),
            Value::String(value) => match value.to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" => Some(true),
                "false" | "no" | "0" => Some(false),
                _ => None,
            },
            _ => None,
        })
}

/// 读取可选非负整数；非法类型按未知处理，让缺省值继续遵循事实优先原则。
fn optional_u8(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<u8> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| match value {
            Value::Number(value) => value.as_u64().and_then(|number| u8::try_from(number).ok()),
            Value::String(value) => value.parse::<u8>().ok(),
            _ => None,
        })
}

/// 严格读取可选的非负整数；字段存在但类型异常时返回导入错误，不静默丢弃玩家培养信息。
fn optional_u64_checked(
    file_name: &str,
    path: &str,
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Result<Option<u64>, AppError> {
    let Some(value) = keys.iter().find_map(|key| object.get(*key)) else {
        return Ok(None);
    };
    match value {
        Value::Number(number) => number.as_u64().map(Some),
        Value::String(text) => text.trim().parse::<u64>().ok().map(Some),
        _ => None,
    }
    .ok_or_else(|| {
        AppError::import_field(
            file_name,
            path,
            "非负整数",
            value_kind(value),
            "字段类型不正确",
        )
    })
}

/// 严格读取可选的 0—255 整数，技能等级和式神等级共用该边界。
fn optional_u8_checked(
    file_name: &str,
    path: &str,
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Result<Option<u8>, AppError> {
    optional_u64_checked(file_name, path, object, keys)?
        .map(|value| {
            u8::try_from(value).map_err(|_| {
                AppError::import_field(
                    file_name,
                    path,
                    "0 到 255 的整数",
                    &value.to_string(),
                    "整数超出范围",
                )
            })
        })
        .transpose()
}

/// 严格读取可选布尔值；桌面版 awake/lock 使用布尔值或 0/1 数字。
fn optional_bool_checked(
    file_name: &str,
    path: &str,
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Result<Option<bool>, AppError> {
    let Some(value) = keys.iter().find_map(|key| object.get(*key)) else {
        return Ok(None);
    };
    let parsed = match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => match value.as_u64() {
            Some(0) => Some(false),
            Some(1) => Some(true),
            _ => None,
        },
        Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    };
    parsed
        .ok_or_else(|| {
            AppError::import_field(
                file_name,
                path,
                "布尔值或 0/1",
                value_kind(value),
                "字段类型不正确",
            )
        })
        .map(Some)
}

/// 读取技能数组中的非负整数，兼容 JSON 数字和数字字符串。
fn parse_u64_value(file_name: &str, path: &str, value: &Value) -> Result<u64, AppError> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    }
    .ok_or_else(|| {
        AppError::import_field(
            file_name,
            path,
            "非负整数",
            value_kind(value),
            "字段类型不正确",
        )
    })
}

/// 读取技能等级并限制为单字节整数，错误路径指向具体技能数组位置。
fn parse_u8_value(file_name: &str, path: &str, value: &Value) -> Result<u8, AppError> {
    let number = parse_u64_value(file_name, path, value)?;
    u8::try_from(number).map_err(|_| {
        AppError::import_field(
            file_name,
            path,
            "0 到 255 的整数",
            &number.to_string(),
            "整数超出范围",
        )
    })
}

/// 读取必需非负整数并在错误中保留字段路径。
fn required_u8(
    file_name: &str,
    path: &str,
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Result<u8, AppError> {
    let value = keys
        .iter()
        .find_map(|key| object.get(*key))
        .ok_or_else(|| {
            AppError::import_field(file_name, path, "0 到 255 的整数", "缺失", "缺少必需字段")
        })?;
    optional_u8(object, keys).ok_or_else(|| {
        AppError::import_field(file_name, path, "整数", value_kind(value), "字段类型不正确")
    })
}

/// 读取属性数值，支持数字、数字字符串和带百分号的字符串。
fn parse_number_value(file_name: &str, path: &str, value: &Value) -> Result<f64, AppError> {
    let parsed = match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.trim().trim_end_matches('%').parse::<f64>().ok(),
        _ => None,
    };
    parsed.filter(|number| number.is_finite()).ok_or_else(|| {
        AppError::import_field(
            file_name,
            path,
            "数字",
            value_kind(value),
            "属性数值类型不正确",
        )
    })
}

/// 找到属性对象中的“名称 + 数值”字段；也支持只有一个键的映射对象。
fn object_attribute_pair(object: &serde_json::Map<String, Value>) -> Option<(&str, &Value)> {
    let name = [
        "type",
        "attributeType",
        "attribute_type",
        "name",
        "attr",
        "key",
        "属性",
    ]
    .iter()
    .find_map(|key| object.get(*key).and_then(Value::as_str));
    let value = ["value", "val", "number", "num", "amount", "数值"]
        .iter()
        .find_map(|key| object.get(*key));
    match (name, value) {
        (Some(name), Some(value)) => Some((name, value)),
        _ if object.len() == 1 => object
            .iter()
            .next()
            .map(|(name, value)| (name.as_str(), value)),
        _ => None,
    }
}

/// 识别来源显式完整性；未知和缺失都不能升级为完整快照。
fn detect_completeness(object: &serde_json::Map<String, Value>) -> Option<String> {
    let value = object
        .get("completeness")
        .or_else(|| object.get("scopeType"))
        .or_else(|| object.get("isComplete"));
    match value {
        Some(Value::String(value))
            if matches!(value.as_str(), "complete" | "partial" | "unknown") =>
        {
            Some(value.clone())
        }
        Some(Value::Bool(true)) => Some("complete".to_owned()),
        Some(Value::Bool(false)) => Some("partial".to_owned()),
        _ => None,
    }
}

/// 校验用户完整性覆盖，只允许三种数据库枚举值。
fn validate_completeness_override(
    file_name: &str,
    value: Option<&str>,
    detected: &str,
) -> Result<String, AppError> {
    match value {
        None => Ok(if detected == "complete" {
            "complete"
        } else {
            "partial"
        }
        .to_owned()),
        Some(value) if matches!(value, "complete" | "partial" | "unknown") => Ok(value.to_owned()),
        Some(value) => Err(AppError::import_field(
            file_name,
            "completenessOverride",
            "complete/partial/unknown",
            value,
            "完整性覆盖值不正确",
        )),
    }
}

/// 判断规范化阶段是否需要发布进度；每百枚和最终枚必须可见，其他枚数不产生事件风暴。
fn should_publish_normalize_progress(completed: u32, total: u32) -> bool {
    completed.is_multiple_of(100) || completed == total
}

/// 生成统一任务进度载荷；错误文件不改变整个批次的任务终态。
fn progress(
    task_id: &str,
    phase: &str,
    completed: u32,
    total: u32,
    message: impl Into<String>,
    status: TaskStatus,
    result: Option<Value>,
) -> TaskProgress {
    TaskProgress {
        task_id: task_id.to_owned(),
        phase: phase.to_owned(),
        completed,
        total,
        message: message.into(),
        status,
        error: None,
        result,
    }
}

/// 返回 JSON 值的类型名，错误详情不回显完整载荷。
fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 生成不含字段值的 JSON 结构摘要，用于定位桥接外壳，不把御魂正文写入诊断日志。
fn json_shape(value: &Value, depth: u8) -> String {
    if depth >= 3 {
        return match value {
            Value::Null => "null".to_owned(),
            Value::Bool(_) => "boolean".to_owned(),
            Value::Number(_) => "number".to_owned(),
            Value::String(_) => "string".to_owned(),
            Value::Array(items) => format!("array(len={})", items.len()),
            Value::Object(object) => format!("object(keys={})", object.len()),
        };
    }
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(_) => "boolean".to_owned(),
        Value::Number(_) => "number".to_owned(),
        Value::String(_) => "string".to_owned(),
        Value::Array(items) => {
            let first = items
                .first()
                .map(|item| format!(", first={}", json_shape(item, depth + 1)))
                .unwrap_or_default();
            format!("array(len={}{})", items.len(), first)
        }
        Value::Object(object) => {
            let mut entries = object
                .iter()
                .take(24)
                .map(|(key, child)| format!("{key}:{}", json_shape(child, depth + 1)))
                .collect::<Vec<_>>();
            if object.len() > 24 {
                entries.push("…".to_owned());
            }
            format!("object{{{}}}", entries.join(","))
        }
    }
}

/// 仅为 MuMu 临时批次记录结构化失败详情，不记录原始 JSON，便于定位预检或提交边界。
fn log_mumu_import_failure(file_name: &str, stage: &str, error: &AppError) {
    if !file_name.starts_with("mumu-") {
        return;
    }
    tracing::warn!(
        stage,
        file_name,
        error_code = ?error.code,
        error_message = %error.message,
        error_details = ?error.details,
        "MuMu 批次导入失败"
    );
}

/// 拼出导入完成提示里的角色标签：「区服·昵称（UID）」。
///
/// 区服名和 UID 都可能缺失（快照未携带、探针未读到），缺哪段就省哪段，
/// 保证提示始终至少带上昵称，不出现「·」或空括号这类残缺文本。
fn format_character_label(entry: &StoredCharacterArchive) -> String {
    let mut label = String::new();
    if let Some(server) = entry
        .server_label
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        label.push_str(server);
        label.push('·');
    }
    label.push_str(entry.display_name.trim());
    if let Some(player_id) = entry
        .player_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        label.push_str(&format!("（UID {player_id}）"));
    }
    label
}

/// 从成功导入的当前数据正文生成角色档案条目；同一角色按身份键覆盖。
fn archive_entry_from_import(
    document: &CurrentDataDocument,
    parsed: &ParsedImport,
) -> StoredCharacterArchive {
    let payload = &document.source_payload;
    let player = payload.get("player").and_then(Value::as_object);
    let player_field = |name: &str| player.and_then(|object| object.get(name));
    let player_id = player_field("shortId").and_then(archive_text_value);
    let display_name = player_field("name")
        .and_then(archive_text_value)
        .or_else(|| payload.get("accountName").and_then(archive_text_value))
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "未命名角色".to_owned());
    let server_label = player_field("serverName")
        .and_then(archive_text_value)
        .or_else(|| payload.get("serverName").and_then(archive_text_value))
        .map(|server| server.trim().to_owned())
        .filter(|server| !server.is_empty());
    // 平台沿用快照中的稳定值；未知或非标准值按缺失处理，避免污染角色档案。
    let platform = player_field("platform")
        .or_else(|| payload.get("platform"))
        .and_then(archive_platform_value);
    let account_id = payload
        .get("accountId")
        .and_then(archive_text_value)
        .or_else(|| player_id.clone())
        .unwrap_or_else(|| document.file_name.clone());
    let six_star_count = parsed
        .souls
        .iter()
        .filter(|soul| soul.quality == 6)
        .count() as u32;
    // 式神总数 = 实例数 + 折叠分组总量（游戏式神录顶部口径）。
    let bag_count = payload
        .get("heroesBagCount")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    StoredCharacterArchive {
        identity_key: archive_identity_key(
            player_id.as_deref(),
            display_name.as_str(),
            server_label.as_deref(),
            payload,
            &document.file_name,
        ),
        profile_id: None,
        account_id,
        display_name,
        platform,
        player_id,
        server_label,
        last_updated_at: document.imported_at.clone(),
        soul_count: six_star_count,
        shikigami_count: document.normalized_shikigami.len() as u32 + bag_count,
        source_kind: document.source_kind.clone(),
        source_label: archive_source_label(&document.source_kind),
    }
}

/// 生成稳定的角色身份键：UID 优先，其次昵称+区服，再次桌面账号 ID，最后回退到文件名。
pub(crate) fn archive_identity_key(
    player_id: Option<&str>,
    display_name: &str,
    server_label: Option<&str>,
    payload: &Value,
    file_name: &str,
) -> String {
    if let Some(player_id) = player_id.map(str::trim).filter(|value| !value.is_empty()) {
        return format!("uid:{player_id}");
    }
    let name = display_name.trim();
    if !name.is_empty() {
        let server = server_label.map(str::trim).unwrap_or_default();
        return format!(
            "name:{}|{}",
            server.to_ascii_lowercase(),
            name.to_ascii_lowercase()
        );
    }
    if let Some(account_id) = payload.get("accountId").and_then(archive_text_value) {
        return format!("account:{account_id}");
    }
    format!("file:{file_name}")
}

/// 兼容字符串和数字的档案文本字段读取；其他类型按缺失处理。
fn archive_text_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

/// 只接受 android/ios 两个受支持的平台值，并统一大小写与空白。
fn archive_platform_value(value: &Value) -> Option<String> {
    let platform = value.as_str()?.trim().to_ascii_lowercase();
    match platform.as_str() {
        PLATFORM_ANDROID | PLATFORM_IOS => Some(platform),
        _ => None,
    }
}

#[cfg(test)]
mod archive_tests {
    use super::{archive_identity_key, archive_platform_value, format_character_label};
    use crate::infrastructure::character_archives::StoredCharacterArchive;
    use serde_json::{Value, json};

    fn archive(
        display_name: &str,
        player_id: Option<&str>,
        server_label: Option<&str>,
    ) -> StoredCharacterArchive {
        StoredCharacterArchive {
            identity_key: "uid:1".to_owned(),
            profile_id: None,
            account_id: "account-1".to_owned(),
            display_name: display_name.to_owned(),
            platform: None,
            player_id: player_id.map(ToOwned::to_owned),
            server_label: server_label.map(ToOwned::to_owned),
            last_updated_at: "2026-08-19T00:00:00Z".to_owned(),
            soul_count: 0,
            shikigami_count: 0,
            source_kind: "desktop".to_owned(),
            source_label: "桌面版".to_owned(),
        }
    }

    #[test]
    fn 角色标签应包含区服昵称和uid() {
        assert_eq!(
            format_character_label(&archive("铁血战士胖虎", Some("4532483"), Some("抢先体验服"))),
            "抢先体验服·铁血战士胖虎（UID 4532483）"
        );
    }

    #[test]
    fn 区服名缺失时角色标签应省略区服段() {
        assert_eq!(
            format_character_label(&archive("铁血战士胖虎", Some("4532483"), None)),
            "铁血战士胖虎（UID 4532483）",
            "不应残留分隔符"
        );
        assert_eq!(
            format_character_label(&archive("铁血战士胖虎", Some("4532483"), Some("   "))),
            "铁血战士胖虎（UID 4532483）",
            "空白区服名应与缺失同等处理"
        );
    }

    #[test]
    fn uid缺失时角色标签应省略括号() {
        assert_eq!(
            format_character_label(&archive("铁血战士胖虎", None, Some("抢先体验服"))),
            "抢先体验服·铁血战士胖虎",
            "不应出现空括号"
        );
    }

    #[test]
    fn 只有昵称时角色标签就是昵称() {
        assert_eq!(
            format_character_label(&archive(" 未命名角色 ", None, None)),
            "未命名角色"
        );
    }

    #[test]
    fn 档案平台只接受安卓和ios() {
        assert_eq!(
            archive_platform_value(&serde_json::json!("ANDROID")),
            Some("android".to_owned())
        );
        assert_eq!(
            archive_platform_value(&serde_json::json!(" iOS ")),
            Some("ios".to_owned())
        );
        assert_eq!(archive_platform_value(&serde_json::json!("windows")), None);
    }

    fn key(
        player_id: Option<&str>,
        name: &str,
        server: Option<&str>,
        payload: Value,
    ) -> String {
        archive_identity_key(player_id, name, server, &payload, "fixture.json")
    }

    #[test]
    fn uid优先作为身份键() {
        assert_eq!(
            key(Some("123456"), "铁血战士", Some("春之樱"), json!({})),
            "uid:123456"
        );
    }

    #[test]
    fn 无uid时按昵称和区服归一化() {
        assert_eq!(
            key(None, "铁血战士", Some("春之樱"), json!({})),
            "name:春之樱|铁血战士"
        );
        assert_eq!(
            key(None, " 铁血战士 ", Some(" 春之樱 "), json!({})),
            "name:春之樱|铁血战士",
            "昵称和区服应去除首尾空白后归一化"
        );
    }

    #[test]
    fn 无身份时回退桌面账号ID或文件名() {
        assert_eq!(
            key(None, " ", None, json!({"accountId": "abc123"})),
            "account:abc123"
        );
        assert_eq!(
            key(None, " ", None, json!({})),
            "file:fixture.json"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(payload: &str) -> ImportFileInput {
        ImportFileInput {
            file_name: "fixture.json".to_owned(),
            payload: payload.as_bytes().to_vec(),
        }
    }

    #[test]
    fn 三类格式都映射为同一规范事实() {
        let soul = r#"{"id":"soul-1","setId":"破势","slot":2,"quality":6,"level":0,"mainAttr":{"type":"速度","value":57},"subAttributes":[{"type":"暴击","value":3}]}"#;
        let native = format!(
            r#"{{"format":"yys-analysis-snapshot","schemaVersion":1,"completeness":"complete","souls":[{soul}]}}"#
        );
        let raw = format!(r#"{{"hero_equips":[{soul}]}}"#);
        let yyshub = format!(
            r#"{{"version":"1","timestamp":"2026-01-01T00:00:00Z","data":{{"hero_equips":[{soul}]}}}}"#
        );

        let native = parse_file(&file(&native), None).expect("本工具格式");
        let raw = parse_file(&file(&raw), None).expect("原始格式");
        let yyshub = parse_file(&file(&yyshub), None).expect("yyshub 格式");

        assert_eq!(native.content_fingerprint, raw.content_fingerprint);
        assert_eq!(raw.content_fingerprint, yyshub.content_fingerprint);
        assert_eq!(native.souls[0].main_attr_type, "speed");
        assert_eq!(native.souls[0].attributes[0].attribute_type, "crit_rate");
        assert_eq!(native.completeness, "complete");
        assert_eq!(raw.completeness, "partial");
    }

    #[test]
    fn 百分比属性转换为小数且强化次数保留来源() {
        let input = file(
            r#"{"hero_equips":[{"id":"1","setId":"针女","slot":6,"quality":6,"level":15,"mainAttrType":"暴击","mainAttrValue":100,"subAttributes":[{"type":"暴伤","value":"40%","enhancementCount":2}]}]}"#,
        );
        let parsed = parse_file(&input, None).expect("解析百分比");
        assert_eq!(parsed.souls[0].main_attr_value, 1.0);
        assert_eq!(parsed.souls[0].attributes[0].value, 0.4);
        assert_eq!(parsed.souls[0].attributes[0].enhancement_count, Some(2));
    }

    #[test]
    fn 桌面版heroes会规范化式神培养信息但忽略御魂装备关系() {
        let payload = r#"{
            "format":"yys-desktop-cache-v1","completeness":"complete",
            "souls":[{"id":"desktop-soul-1","setId":"破势","slot":2,"quality":6,"level":15,
                "mainAttrType":"速度","mainAttrValue":57,"subAttributes":[]}],
            "heroes":{
                "hero-instance-1":{
                "heroId":398,"star":6,"level":40,"exp":12345.6,"lock":true,"awake":1,"skinid":2,
                    "skinfo":[[3981,5],[3982,3]],"selectSkills":[3981,3982],
                    "equips":{"ignored-slot":{"id":"must-not-enter"}}
                }
            }
        }"#;
        let parsed = parse_file(&file(payload), None).expect("桌面版 heroes 应可解析");
        assert_eq!(parsed.owned_shikigami.len(), 1);
        let hero = &parsed.owned_shikigami[0];
        assert_eq!(hero.instance_id, "hero-instance-1");
        assert_eq!(hero.shikigami_id, "398");
        assert_eq!(hero.star, 6);
        assert_eq!(hero.level, Some(40));
        // 式神经验只属于展示性培养信息，导入时忽略其小数值，不应阻断御魂数据写入。
        assert_eq!(hero.exp, None);
        assert_eq!(hero.locked, Some(true));
        assert_eq!(hero.awakened, Some(true));
        assert_eq!(hero.skin_id, Some(2));
        assert_eq!(hero.skills[0].skill_id, 3981);
        assert_eq!(hero.skills[0].level, 5);
        assert_eq!(hero.selected_skill_ids, vec![3981, 3982]);
    }

    #[test]
    fn MuMu参考快照会导入式神资源和结界卡() {
        let payload = r#"{
            "player":{"name":"测试角色","serverName":"春之樱","shortId":"10001"},
            "currency":{"coin":123,"jade":456},
            "hero_equips":[{"id":"mumu-soul-1","setId":"破势","slot":2,"quality":6,"level":15,
                "mainAttrType":"速度","mainAttrValue":57,"subAttributes":[]}],
            "heroes":{"hero-instance-1":{"heroId":398,"star":6,"level":40}},
            "realm_cards":[[900001,123456,3600,150]]
        }"#;
        let parsed = parse_file(&file(payload), None).expect("MuMu 参考快照应可解析");
        assert_eq!(parsed.format, "mumu-snapshot-v1");
        assert_eq!(parsed.source_kind, "mumu");
        assert_eq!(parsed.souls.len(), 1);
        assert_eq!(parsed.owned_shikigami.len(), 1);
        assert_eq!(parsed.items["coin"], 123);
        assert_eq!(parsed.realm_cards.as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn 桌面版道具与结界卡会进入归一化字段() {
        let payload = r#"{
            "format":"yys-desktop-cache-v1","completeness":"complete",
            "souls":[{"id":"desktop-soul-1","setId":"破势","slot":2,"quality":6,"level":15,
                "mainAttrType":"速度","mainAttrValue":57,"subAttributes":[]}],
            "currency":{"coin":123,"jade":456,"action_point":789},
            "realmCards":[
                [900001,123456,3600,150],
                [900002,123456,7200,300],
                [900003,789012,1800,80]
            ]
        }"#;
        let parsed = parse_file(&file(payload), None).expect("桌面版道具与结界卡应可解析");
        assert_eq!(parsed.items["coin"], 123);
        assert_eq!(parsed.items["jade"], 456);
        assert_eq!(parsed.realm_cards.as_array().unwrap().len(), 3);
        assert_eq!(parsed.realm_cards[0][1], 123456);
    }

    #[test]
    fn 非桌面格式不携带道具与结界卡字段() {
        let payload = r#"{
            "format":"mumu-adapter-v1","completeness":"complete",
            "souls":[{"id":"mumu-soul-1","setId":"破势","slot":2,"quality":6,"level":15,
                "mainAttrType":"速度","mainAttrValue":57,"subAttributes":[]}],
            "currency":{"coin":999},
            "realmCards":[[1,2,3,4]]
        }"#;
        let parsed = parse_file(&file(payload), None).expect("MuMu 格式应可解析");
        assert!(parsed.items.as_object().unwrap().is_empty());
        assert!(parsed.realm_cards.as_array().unwrap().is_empty());
    }

    #[test]
    fn 藏宝阁heroes应写入式神录且兼容单技能扁平结构() {
        let payload = r#"{
            "format":"cbg-equip-v1","completeness":"complete",
            "souls":[{"id":"cbg-soul-1","setId":"破势","slot":2,"quality":6,"level":15,
                "mainAttrType":"速度","mainAttrValue":57,"subAttributes":[]}],
            "heroes":{
                "cbg-hero-1":{
                    "heroId":398,"star":6,"level":40,"lock":true,"awake":1,
                    "skinfo":[[3981,5],[3982,3]],"selectSkills":[3981,3982],
                    "equips":[{"id":"must-not-enter"}]
                },
                "cbg-hero-2":{
                    "heroId":900,"star":2,"level":40,
                    "skinfo":[9001,1],"selectSkills":[9001]
                }
            }
        }"#;
        let parsed = parse_file(&file(payload), None).expect("藏宝阁 heroes 应可解析");
        assert_eq!(parsed.owned_shikigami.len(), 2);
        assert_eq!(parsed.owned_shikigami[0].shikigami_id, "398");
        assert_eq!(parsed.owned_shikigami[0].skills.len(), 2);
        assert_eq!(
            parsed.owned_shikigami[0].selected_skill_ids,
            vec![3981, 3982]
        );
        assert_eq!(parsed.owned_shikigami[1].skills[0].skill_id, 9001);
        assert_eq!(parsed.owned_shikigami[1].skills[0].level, 1);
    }

    #[test]
    fn 桌面版heroes中的特殊角色60级记录不应阻断式神导入() {
        // 真实桌面缓存会把阴阳师主角等特殊角色放在 heroes 中；这些记录可为 60 级，不能按普通式神的 40 级上限校验。
        let payload = r#"{
            "format":"yys-desktop-cache-v1","completeness":"complete",
            "souls":[],
            "heroes":{
                "special-hero-10":{"heroId":10,"star":2,"level":60,"skinfo":[[1001,5]]}
            }
        }"#;
        let parsed = parse_file(&file(payload), None).expect("特殊角色记录不应阻断桌面版导入");
        assert!(parsed.owned_shikigami.is_empty());
    }

    /// 桌面版式神缓存不属于御魂分析输入；即使结构异常，也不能阻断御魂导入。
    #[test]
    fn 桌面版heroes字段异常时应直接忽略且不阻断御魂导入() {
        let payload = r#"{
            "format":"yys-desktop-cache-v1","completeness":"complete",
            "souls":[{"id":"desktop-soul-1","setId":"破势","slot":2,"quality":6,"level":15,
                "mainAttrType":"速度","mainAttrValue":57,"subAttributes":[]}],
            "heroes":{
                "broken-hero":{"heroId":{"unexpected":"object"},"exp":13666.4}
            }
        }"#;
        let parsed = parse_file(&file(payload), None).expect("式神缓存异常不应阻断御魂导入");
        assert_eq!(parsed.souls.len(), 1);
        assert!(parsed.owned_shikigami.is_empty());
    }

    #[test]
    fn MuMu适配器批次保留来源版本并在缺少证明时按局部处理() {
        let payload = r#"{
            "format":"mumu-adapter-v1",
            "schemaVersion":1,
            "adapterVersion":"1.2.0",
            "gameVersion":"1.8.0",
            "scope":{"trigger":"calculator"},
            "souls":[{"id":"mumu-1","setId":"破势","slot":2,"quality":6,"level":3,
                "mainAttrType":"速度","mainAttrValue":57,"subAttributes":[]}]
        }"#;
        let parsed = parse_file(&file(payload), None).expect("MuMu 批次应可进入导入解析");
        assert_eq!(parsed.source_kind, "mumu");
        assert_eq!(parsed.format, "mumu-adapter-v1");
        assert_eq!(parsed.completeness, "partial");
        assert_eq!(parsed.adapter_version.as_deref(), Some("1.2.0"));
        assert_eq!(parsed.game_version.as_deref(), Some("1.8.0"));
    }

    #[test]
    fn 痒痒鼠魔方批次的根级equips应可进入解析() {
        // 参考桥接层会直接回传 equips 数组；该样例覆盖批次数量统计与导入解析之间的契约。
        let payload = r#"{
            "equips":[{"uid":"mumu-1","suit":"破势","pos":2,"star":6,"level":3,
                "baseAttr":{"type":"速度","value":57},"attrs":[]}]
        }"#;
        let parsed = parse_file(&file(payload), None).expect("参考工具批次应可进入导入解析");
        assert_eq!(parsed.source_kind, "mumu");
        assert_eq!(parsed.souls.len(), 1);
        assert_eq!(parsed.souls[0].source_stable_id.as_deref(), Some("mumu-1"));
    }

    #[test]
    fn 痒痒鼠魔方内部压缩批次应可进入解析() {
        // 合成记录覆盖火灵五号位、随机属性倍率和固有效果命中，验证完整本地解码而非只识别外壳。
        let payload = r#"{
            "000000003333333333333333":{
                "base_r":1,
                "base_rindex":2,
                "others":4435744499321334,
                "rattr":[[0,1.01],[5,1.06],[6,1.07],[8,1.09]],
                "single_attr":5
            }
        }"#;
        let parsed = parse_file(&file(payload), None).expect("内部压缩批次应可进入导入解析");
        assert_eq!(parsed.format, "mumu-compact-v1");
        assert_eq!(parsed.source_kind, "mumu");
        assert_eq!(parsed.souls.len(), 1);
        let soul = &parsed.souls[0];
        assert_eq!(
            soul.source_stable_id.as_deref(),
            Some("000000003333333333333333")
        );
        assert_eq!(soul.set_id, "火灵");
        assert_eq!(soul.slot, 5);
        assert_eq!(soul.quality, 6);
        assert_eq!(soul.level, 15);
        assert_eq!(soul.main_attr_type, "hp_flat");
        assert!((soul.main_attr_value - 2052.0).abs() < 1e-9);
        assert_eq!(soul.attributes.len(), 5);
        assert_eq!(soul.attributes[0].attribute_type, "hp_flat");
        assert!((soul.attributes[0].value - 115.14).abs() < 1e-9);
        assert_eq!(soul.attributes[1].attribute_type, "attack_rate");
        assert!((soul.attributes[1].value - 0.0318).abs() < 1e-9);
        assert_eq!(soul.attributes[1].enhancement_count, Some(1));
        assert_eq!(soul.attributes[2].attribute_type, "speed");
        assert!((soul.attributes[2].value - 3.21).abs() < 1e-9);
        assert_eq!(soul.attributes[2].enhancement_count, Some(1));
        assert_eq!(soul.attributes[3].attribute_type, "crit_damage");
        assert!((soul.attributes[3].value - 0.0436).abs() < 1e-9);
        assert_eq!(soul.attributes[3].enhancement_count, Some(1));
        assert_eq!(soul.attributes[4].attribute_type, "effect_hit");
        assert!(soul.attributes[4].fixed_attribute);
        assert_eq!(soul.attributes[4].enhancement_count, None);
        assert!((soul.attributes[4].value - 0.08).abs() < 1e-9);
    }

    #[test]
    fn MuMu原始采集包装应合并并去掉重复批次() {
        // 采集脚本的两批重复回调应只导入一枚御魂，并保留外层采集时间和版本信息。
        let payload = r#"{
            "format":"mumu-raw-capture-v1",
            "source":"MuMu6",
            "capturedAt":"2026-08-11T05:39:42Z",
            "gameVersion":"1.8.62",
            "batches":[
                {"receivedAt":"2026-08-11T05:39:55Z","soulCount":1,"payload":{
                    "000000003333333333333333":{
                        "base_r":1,
                        "base_rindex":2,
                        "others":4435744499321334,
                        "rattr":[[0,1.01],[5,1.06],[6,1.07],[8,1.09]],
                        "single_attr":5
                    }
                }},
                {"receivedAt":"2026-08-11T05:39:56Z","soulCount":1,"payload":{
                    "000000003333333333333333":{
                        "base_r":1,
                        "base_rindex":2,
                        "others":4435744499321334,
                        "rattr":[[0,1.01],[5,1.06],[6,1.07],[8,1.09]],
                        "single_attr":5
                    }
                }}
            ]
        }"#;
        let parsed = parse_file(&file(payload), None).expect("原始采集包装应可导入");
        assert_eq!(parsed.format, "mumu-raw-capture-v1");
        assert_eq!(parsed.source_kind, "mumu");
        assert_eq!(parsed.souls.len(), 1);
        assert_eq!(parsed.game_version.as_deref(), Some("1.8.62"));
        assert_eq!(parsed.captured_at.as_deref(), Some("2026-08-11T05:39:42Z"));
        assert!(
            parsed
                .warnings
                .iter()
                .any(|warning| warning.contains("合并重复记录"))
        );
    }

    #[test]
    fn MuMu主属性按号位规则映射() {
        // 锁定固定号位与二四六号位的合法主属性，避免后续调整压缩字段时漏掉速度或特殊属性。
        for (slot, base_index, expected) in [
            (1, 0, "attack_flat"),
            (2, 0, "attack_rate"),
            (2, 1, "defense_rate"),
            (2, 2, "hp_rate"),
            (2, 3, "speed"),
            (3, 1, "defense_flat"),
            (4, 0, "attack_rate"),
            (4, 1, "defense_rate"),
            (4, 2, "hp_rate"),
            (4, 3, "effect_hit"),
            (4, 4, "effect_resist"),
            (5, 2, "hp_flat"),
            (6, 0, "attack_rate"),
            (6, 1, "defense_rate"),
            (6, 2, "hp_rate"),
            (6, 3, "crit_damage"),
            (6, 4, "crit_rate"),
        ] {
            let (attribute, _) =
                mumu_main_attribute("fixture.json", "$.soul", slot, base_index, 15)
                    .expect("合法号位主属性应可解析");
            assert_eq!(attribute, expected, "slot={slot}, base_index={base_index}");
        }

        // 固定号位不允许误接入百分比主属性，避免把 1/3/5 号位显示成加成类属性。
        assert!(mumu_main_attribute("fixture.json", "$.soul", 1, 3, 15).is_err());
        assert!(mumu_main_attribute("fixture.json", "$.soul", 3, 0, 15).is_err());
        assert!(mumu_main_attribute("fixture.json", "$.soul", 5, 4, 15).is_err());
    }

    #[test]
    fn MuMu紧凑倍率整数部分对应强化次数() {
        // 真实采集中的 0.95、1.84 和 5.37 分别应展示为 +0、+1 和 +5。
        assert_eq!(mumu_compact_enhancement_count(0.949721), Some(0));
        assert_eq!(mumu_compact_enhancement_count(1.842758), Some(1));
        assert_eq!(mumu_compact_enhancement_count(5.369659), Some(5));
        assert_eq!(mumu_compact_enhancement_count(-0.1), None);
    }

    #[test]
    fn 活动御魂模板190052应可进入解析() {
        // 新批次出现 190052 模板；使用合成 ID 和属性，仅保留触发模板拒绝所需的最小字段。
        let payload = r#"{
            "000000003333333333333333":{
                "base_r":1,
                "base_rindex":3,
                "others":4435744499361380,
                "rattr":[[0,1],[1,1],[2,1],[3,1]],
                "single_attr":null
            }
        }"#;
        let parsed = parse_file(&file(payload), None).expect("190052 活动御魂模板应可解析");
        assert_eq!(parsed.souls.len(), 1);
        assert_eq!(parsed.souls[0].slot, 2);
        assert_eq!(parsed.souls[0].main_attr_type, "speed");
        assert!((parsed.souls[0].main_attr_value - 57.0).abs() < 1e-9);
    }

    #[test]
    fn 新批次中的190活动模板末位对应号位() {
        // 新文件实际出现的 190041/44/51/52/53 均已由合成转换结果核验，锁定末位号位规则。
        for (template_id, expected_slot) in [
            (190_001, 1),
            (190_005, 5),
            (190_006, 6),
            (190_023, 3),
            (190_025, 5),
            (190_041, 1),
            (190_044, 4),
            (190_051, 1),
            (190_052, 2),
            (190_053, 3),
            (180_027, 6),
        ] {
            assert_eq!(mumu_slot_from_template(template_id), Some(expected_slot));
        }
    }

    #[test]
    fn 痒痒鼠标准JSON应转换零基号位和固有属性() {
        // 第三方在线转换结果使用 kind、零基 pos、random_attrs 和 single_attrs，本地导入需保持同一语义。
        let payload = r#"{
            "version":"1.0","timestamp":"2026-08-10T00:00:00Z","data":{"hero_equips":[{
                "id":"standard-1","suit_id":300019,"kind":"火灵","pos":4,"quality":6,"level":15,
                "base_attr":{"type":"Hp","value":2052},
                "random_attrs":[{"type":"Attack","value":27},{"type":"EffectHitRate","value":0.04}],
                "single_attrs":[{"type":"AttackRate","value":0.08}]
            }]}
        }"#;
        let parsed = parse_file(&file(payload), None).expect("痒痒鼠标准 JSON 应可导入");
        let soul = &parsed.souls[0];
        assert_eq!(parsed.format, "yyshub-v1");
        assert_eq!(soul.set_id, "火灵");
        assert_eq!(soul.slot, 5);
        assert_eq!(soul.main_attr_type, "hp_flat");
        assert_eq!(soul.attributes[0].attribute_type, "attack_flat");
        assert_eq!(soul.attributes[1].attribute_type, "effect_hit");
        assert_eq!(soul.attributes[2].attribute_type, "attack_rate");
        assert!(soul.attributes[2].fixed_attribute);
    }

    #[test]
    fn 结构错误包含文件名和字段路径() {
        let error = parse_file(&file(r#"{"hero_equips":[{"id":"1","slot":2}]}"#), None)
            .expect_err("缺字段应失败");
        let details = error.details.expect("错误详情");
        assert_eq!(details["fileName"], "fixture.json");
        assert!(
            details["jsonPath"]
                .as_str()
                .unwrap_or_default()
                .contains("setId")
        );
    }

    #[test]
    fn 单枚御魂副属性超出预算时拒绝解析() {
        let attributes = (0..=MAX_ATTRIBUTES_PER_SOUL)
            .map(|index| format!(r#"{{"type":"速度","value":{index}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let payload = format!(
            r#"{{"hero_equips":[{{"id":"1","setId":"破势","slot":2,"quality":6,"level":0,"mainAttrType":"速度","mainAttrValue":57,"subAttributes":[{attributes}]}}]}}"#
        );
        let error = parse_file(&file(&payload), None).expect_err("应拒绝异常副属性数量");
        assert!(error.message.contains("副属性数量超过安全上限"));
    }

    #[test]
    fn 一万枚御魂进度按百枚节流且保留最终进度() {
        let published = (1..=10_000)
            .filter(|completed| should_publish_normalize_progress(*completed, 10_000))
            .count();
        assert_eq!(published, 100, "一万枚数据只应发布一百个规范化进度事件");
        assert!(should_publish_normalize_progress(10_000, 10_000));
        assert!(should_publish_normalize_progress(37, 37));
        assert!(!should_publish_normalize_progress(37, 10_000));
    }
}
