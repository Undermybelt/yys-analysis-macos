//! Tauri 命令：Vue 前端可调用的唯一入口。
//!
//! 每个命令只做参数校验与 DTO 映射，业务逻辑委托给应用层用例。

use crate::infrastructure::current_data::{CurrentDataSummary, ShikigamiStoryProgress};
use crate::infrastructure::character_archives::StoredCharacterArchive;
use crate::{
    application::{
        cbg_read::CbgReadUseCase,
        character_archive_transfer::CharacterArchiveTransfer,
        character_archives::scan_character_archives,
        embryo_decision::EmbryoDecisionUseCase,
        error::AppError,
        growth_quality::GrowthQualityUseCase,
        guild::parse_guild,
        head_tail::HeadTailUseCase,
        history_analysis::HistoryAnalysisUseCase,
        importer::ImportUseCase,
        miracle_conch::MiracleConchUseCase,
        rule_use_cases::RuleUseCase,
        services::AppServices,
        shikigami_shard_query::lookup_shikigami_shards,
        simulation::SimulationUseCase,
        soul_radar::SoulRadarUseCase,
        tasks::{TaskProgress, TaskRegistry, TaskStatus},
        updates::{build_update_channel, UpdateUseCase},
        use_cases::{
            AnalysisUseCase, BackupUseCase, CatalogUseCase, ProfileUseCase, StorageUseCase,
        },
    },
    domain::{
        builtin_use_templates, load_default_preset, CreateBatchRequest, InventoryPageQuery,
        NewGameProfile, RuleEngine,
    },
    infrastructure::paths::AppPaths,
    interface::{dto::*, state::AppState},
};
use std::sync::atomic::Ordering;
use std::{collections::HashMap, fs, path::PathBuf, time::Instant};

/// 神奇海螺页面配置文件名；配置与御魂库存分离，只保存用户的页面选择。
const MIRACLE_CONCH_CONFIGURATION_FILE: &str = "miracle-conch.json";
use tauri::{AppHandle, Emitter, Manager, State};

/// 所有后台用例共用的事件名，载荷通过任务 ID 隔离。
const TASK_PROGRESS_EVENT: &str = "task://progress";
/// 更新安装阶段专用事件；与通用任务中心分离，避免启动更新污染用户任务列表。
const UPDATE_PROGRESS_EVENT: &str = "update://progress";
/// 返回当前激活角色的数据档案 ID；尚未导入任何角色时为 None。
/// 角色只由导入自动创建，不再存在隐式“默认数据”档案。
fn active_profile(
    services: &std::sync::Arc<crate::application::services::AppServices>,
) -> Option<String> {
    services.active_profile_id()
}

// ─── 当前库存 ────────────────────────────────────────────────

/// 清空当前激活角色的库存正文及所有依赖库存的派生结果；目录、规则和原始对象历史保持不变。
#[tauri::command]
pub fn clear_current_inventory(state: State<'_, AppState>) -> Result<(), AppError> {
    let Some(profile_id) = active_profile(&state.services) else {
        return Ok(());
    };
    state.services.snapshots.clear_inventory(&profile_id)?;
    // 持有式神镜像同步清空，避免数据库残留与当前正文不一致。
    state.services.owned_shikigami.clear(&profile_id)?;
    // SQLite 事务成功后再删除当前正文，避免数据库清理失败时先丢掉可恢复的导入文件。
    state.services.current_data.clear()?;
    Ok(())
}

/// 列出本机角色档案：只读取导入写入的档案摘要，不扫描游戏目录缓存，也不携带御魂正文。
#[tauri::command]
pub fn list_character_archives(
    state: State<'_, AppState>,
) -> Result<CharacterArchiveScanResultDto, AppError> {
    let active = active_profile(&state.services);
    Ok(scan_character_archives(
        &state.services.character_archives,
        active.as_deref(),
    )?)
}

/// 导出指定角色的完整档案；JSON 和 ZIP 都直接写入系统下载目录并返回绝对路径。
#[tauri::command]
pub fn export_character_archive(
    app: AppHandle,
    state: State<'_, AppState>,
    request: CharacterArchiveExportRequestDto,
) -> Result<CharacterArchiveExportResultDto, AppError> {
    let download_directory = app
        .path()
        .download_dir()
        .map_err(|error| AppError::path_unavailable("download", error.to_string()))?;
    CharacterArchiveTransfer::export(&state.services, &request.profile_id, &download_directory)
}

/// 启动角色档案交换 JSON 导入任务；校验通过后在后台复用当前数据覆盖事务，
/// 前端通过统一任务事件通道接收真实进度，不允许前端直接写入数据库或文件。
#[tauri::command]
pub fn import_character_archive(
    app: AppHandle,
    registry: State<'_, TaskRegistry>,
    request: CharacterArchiveImportRequestDto,
) -> Result<TaskAcceptedDto, AppError> {
    let file = CharacterArchiveTransfer::prepare_import(request)?;
    let services = app.state::<AppState>().services.clone();
    let registry = registry.inner().clone();
    let (task_id, cancellation) = registry.register();
    let worker_task_id = task_id.clone();

    // 文件解析和数据库写入可能持续数秒，放入阻塞线程后主界面仍可响应并持续更新进度条。
    let _worker = tauri::async_runtime::spawn(async move {
        let event_app = app.clone();
        let progress_app = event_app.clone();
        let task_id_for_worker = worker_task_id.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            CharacterArchiveTransfer::import(
                &services,
                &file,
                &task_id_for_worker,
                &cancellation,
                |progress| {
                    progress_app
                        .emit(TASK_PROGRESS_EVENT, progress)
                        .map_err(|error| AppError::event_emit(error.to_string()))
                },
            )
        })
        .await;

        match result {
            Ok(Ok(summary)) => {
                tracing::info!(
                    task_id = %worker_task_id,
                    soul_count = summary.soul_count,
                    shikigami_count = summary.shikigami_count,
                    "角色档案导入完成"
                );
            }
            Ok(Err(error)) => {
                tracing::error!(
                    task_id = %worker_task_id,
                    code = ?error.code,
                    reason = %error.message,
                    "角色档案导入任务失败"
                );
                let _ = event_app.emit(
                    TASK_PROGRESS_EVENT,
                    TaskProgress {
                        task_id: worker_task_id.clone(),
                        phase: "character-archive".to_owned(),
                        completed: 0,
                        total: 1,
                        message: "角色档案导入失败，当前数据保持不变".to_owned(),
                        status: TaskStatus::Failed,
                        error: Some(error),
                        result: None,
                    },
                );
            }
            Err(error) => {
                tracing::error!(
                    task_id = %worker_task_id,
                    reason = %error,
                    "角色档案导入后台任务异常结束"
                );
                let _ = event_app.emit(
                    TASK_PROGRESS_EVENT,
                    TaskProgress {
                        task_id: worker_task_id.clone(),
                        phase: "character-archive".to_owned(),
                        completed: 0,
                        total: 1,
                        message: "角色档案导入后台任务异常结束".to_owned(),
                        status: TaskStatus::Failed,
                        error: Some(AppError::internal(error.to_string())),
                        result: None,
                    },
                );
            }
        }
        registry.complete(&worker_task_id);
    });

    Ok(TaskAcceptedDto { task_id })
}

/// 删除单个角色档案；级联清理该角色的数据档案、库存正文、式神镜像和分析派生结果。
#[tauri::command]
pub fn delete_character_archive(
    state: State<'_, AppState>,
    request: DeleteCharacterArchiveRequestDto,
) -> Result<(), AppError> {
    let identity_key = request.identity_key.trim();
    if identity_key.is_empty() {
        return Err(AppError::invalid_argument(
            "identityKey",
            "角色身份键不能为空",
        ));
    }
    let entry = state
        .services
        .character_archives
        .find(identity_key)?
        .ok_or_else(|| AppError::not_found("characterArchive", identity_key))?;
    clear_character_archive_data(&state.services, &entry)?;
    if !state.services.character_archives.delete(identity_key)? {
        return Err(AppError::not_found("characterArchive", identity_key));
    }
    Ok(())
}

/// 切换当前激活角色；档案 ID 必须是已导入角色绑定的数据档案。
#[tauri::command]
pub fn set_active_character(
    state: State<'_, AppState>,
    request: SetActiveCharacterRequest,
) -> Result<(), AppError> {
    let found = state
        .services
        .character_archives
        .list()?
        .into_iter()
        .any(|entry| entry.profile_id.as_deref() == Some(request.profile_id.as_str()));
    if !found {
        return Err(AppError::not_found(
            "profileId",
            "档案 ID 不属于任何已导入的角色",
        ));
    }
    state.services.set_active_profile(request.profile_id);
    Ok(())
}

/// 清空本机保存的全部角色档案；删除每个角色绑定的库存、分析派生结果、雷达缓存与式神镜像，
/// 再归档角色身份并清空当前激活角色。
#[tauri::command]
pub fn clear_character_archives(state: State<'_, AppState>) -> Result<u32, AppError> {
    let entries = state.services.character_archives.list()?;
    let count = entries.len() as u32;
    for entry in &entries {
        clear_character_archive_data(&state.services, entry)?;
    }
    state.services.clear_active_profile();
    state.services.character_archives.clear()?;
    Ok(count)
}

/// 清理单个角色绑定的数据档案；角色身份采用归档保留，避免破坏历史外键关系。
fn clear_character_archive_data(
    services: &AppServices,
    entry: &StoredCharacterArchive,
) -> Result<(), AppError> {
    let Some(profile_id) = entry.profile_id.as_deref() else {
        return Ok(());
    };
    services.snapshots.clear_inventory(profile_id)?;
    services.owned_shikigami.clear(profile_id)?;
    if let Err(error) = services.profiles.set_archived(profile_id, true, 0, &AppServices::now_iso()) {
        tracing::warn!(error = %error.message, profile_id, "归档角色数据档案失败");
    }
    services.current_data.clear_profile(profile_id)?;
    if services.active_profile_id().as_deref() == Some(profile_id) {
        services.clear_active_profile();
    }
    Ok(())
}

/// 预检藏宝阁商品链接；网络读取放入阻塞线程，避免公开接口响应较慢时卡住 WebView。
#[tauri::command]
pub async fn inspect_cbg_read(request: CbgReadRequest) -> Result<CbgReadPreviewDto, AppError> {
    if request.source_url.trim().is_empty() {
        return Err(AppError::invalid_argument(
            "sourceUrl",
            "藏宝阁链接不能为空",
        ));
    }
    tauri::async_runtime::spawn_blocking(move || CbgReadUseCase::inspect(&request.source_url))
        .await
        .map_err(|error| AppError::internal(format!("藏宝阁预检任务异常结束：{error}")))?
}

/// 启动藏宝阁导入任务；预检与确认均重新读取公开详情，确认前不会覆盖当前库存。
#[tauri::command]
pub fn start_cbg_import_task(
    app: AppHandle,
    registry: State<'_, TaskRegistry>,
    state: State<'_, AppState>,
    request: CbgReadRequest,
) -> Result<TaskAcceptedDto, AppError> {
    if request.source_url.trim().is_empty() {
        return Err(AppError::invalid_argument(
            "sourceUrl",
            "藏宝阁链接不能为空",
        ));
    }
    let services = state.services.clone();
    let source_url = request.source_url;
    let registry = registry.inner().clone();
    let (task_id, cancellation) = registry.register();
    let worker_task_id = task_id.clone();

    // 藏宝阁返回的是整份公开御魂与式神快照，抓取和规范化都放到阻塞线程，保证界面仍能显示进度。
    let _worker = tauri::async_runtime::spawn(async move {
        let event_app = app.clone();
        let progress_app = event_app.clone();
        let task_id_for_worker = worker_task_id.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let prepared = CbgReadUseCase::prepare(&source_url)?;
            let uc = ImportUseCase::new(services);
            uc.import_current_file(
                &task_id_for_worker,
                &prepared.file,
                &cancellation,
                |progress| {
                    progress_app
                        .emit(TASK_PROGRESS_EVENT, progress)
                        .map_err(|error| AppError::event_emit(error.to_string()))
                },
            )
        })
        .await;

        match result {
            Ok(Err(error)) => {
                let _ = event_app.emit(
                    TASK_PROGRESS_EVENT,
                    TaskProgress {
                        task_id: worker_task_id.clone(),
                        phase: "cbg-import".to_owned(),
                        completed: 0,
                        total: 1,
                        message: "藏宝阁导入失败，当前御魂与式神数据保持不变".to_owned(),
                        status: TaskStatus::Failed,
                        error: Some(error),
                        result: None,
                    },
                );
            }
            Err(error) => {
                let _ = event_app.emit(
                    TASK_PROGRESS_EVENT,
                    TaskProgress {
                        task_id: worker_task_id.clone(),
                        phase: "cbg-import".to_owned(),
                        completed: 0,
                        total: 1,
                        message: "藏宝阁导入后台任务异常结束".to_owned(),
                        status: TaskStatus::Failed,
                        error: Some(AppError::internal(error.to_string())),
                        result: None,
                    },
                );
            }
            Ok(Ok(_summary)) => {}
        }
        registry.complete(&worker_task_id);
    });

    Ok(TaskAcceptedDto { task_id })
}

/// 列出档案的不可变快照历史，最近创建的快照排在前面。
#[tauri::command]
pub fn list_snapshots(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<Vec<SnapshotDto>, AppError> {
    Ok(state
        .services
        .snapshots
        .list_by_profile(&profile_id)?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 列出档案的轻量采集事件，包括重复导入事件。
#[tauri::command]
pub fn list_acquisition_events(
    state: State<'_, AppState>,
    profile_id: String,
    limit: Option<u32>,
) -> Result<Vec<AcquisitionEventDto>, AppError> {
    Ok(state
        .services
        .events
        .list_by_profile(&profile_id, limit.unwrap_or(100).min(1_000))?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 列出当前库存投影，保留局部快照下的未确认/已移除状态。
#[tauri::command]
pub fn list_inventory(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<Vec<InventoryItemDto>, AppError> {
    Ok(state
        .services
        .snapshots
        .list_inventory(&profile_id)?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 分页列出当前激活角色的实际御魂；数据库和快照事实查询都只处理当前页，避免打开页面时加载整份库存。
#[tauri::command]
pub fn list_my_souls(
    state: State<'_, AppState>,
    request: Option<ListMySoulsRequest>,
) -> Result<MySoulPageDto, AppError> {
    let request = request.unwrap_or_default();
    let Some(profile_id) = active_profile(&state.services) else {
        return Ok(MySoulPageDto {
            profile_id: None,
            items: Vec::new(),
            total: 0,
            has_more: false,
            inventory_total: 0,
            source_kinds: Vec::new(),
            limit: request.limit,
            offset: request.offset,
            filter_options: MySoulFilterOptionsDto {
                set_ids: Vec::new(),
                slots: Vec::new(),
                qualities: Vec::new(),
                levels: Vec::new(),
                main_attr_types: Vec::new(),
                sub_attr_types: Vec::new(),
            },
            score_count: 0,
        });
    };
    let page = state.services.snapshots.list_inventory_page(
        &profile_id,
        &InventoryPageQuery {
            set_id: request.set_id,
            set_category: request.set_category,
            slot: request.slot,
            quality: request.quality,
            level: request.level,
            main_attr_type: request.main_attr_type,
            sub_attr_type: request.sub_attr_type,
            attribute_type: request.attribute_type,
            attribute_operator: request.attribute_operator,
            attribute_value: request.attribute_value,
            standard_score_min: request.standard_score_min,
            standard_score_max: request.standard_score_max,
            auspicious_only: request.auspicious_only,
            limit: request.limit,
            offset: request.offset,
        },
    )?;

    // 同一页可能跨多个快照；每个快照只查询本页涉及的内部 ID，不把整份快照事实搬入内存。
    let mut ids_by_snapshot: HashMap<String, Vec<String>> = HashMap::new();
    for item in &page.items {
        ids_by_snapshot
            .entry(item.snapshot_id.clone())
            .or_default()
            .push(item.soul_internal_id.clone());
    }
    let mut facts_by_snapshot = HashMap::new();
    for (snapshot_id, internal_ids) in ids_by_snapshot {
        facts_by_snapshot.insert(
            snapshot_id.clone(),
            state
                .services
                .imports
                .list_snapshot_souls_by_ids(&snapshot_id, &internal_ids)?,
        );
    }

    let mut result = Vec::with_capacity(page.items.len());

    for item in page.items {
        let (souls, attributes) = facts_by_snapshot
            .get(&item.snapshot_id)
            .expect("已插入当前库存对应的快照事实");
        let soul = souls
            .iter()
            .find(|soul| soul.internal_id == item.soul_internal_id)
            .ok_or_else(|| {
                AppError::internal(format!("当前库存缺少对应快照事实：{}", item.soul_key))
            })?;

        let soul_attributes = attributes
            .iter()
            .filter(|attribute| {
                attribute.snapshot_id == soul.snapshot_id
                    && attribute.soul_internal_id == soul.internal_id
            })
            .map(|attribute| MySoulAttributeDto {
                attribute_index: attribute.attribute_index,
                attribute_type: attribute.attribute_type.clone(),
                value: attribute.value,
                enhancement_count: attribute.enhancement_count,
                count_provenance: attribute.count_provenance.clone(),
                fixed_attribute: attribute.fixed_attribute,
            })
            .collect();

        result.push(MySoulDto {
            soul_key: item.soul_key,
            snapshot_id: soul.snapshot_id.clone(),
            soul_internal_id: soul.internal_id.clone(),
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
            presence_state: item.presence_state,
            attributes: soul_attributes,
        });
    }

    let score_count =
        AnalysisUseCase::new(state.services.clone()).count_score_summaries(&profile_id)?;
    Ok(MySoulPageDto {
        profile_id: Some(profile_id),
        items: result,
        total: page.total,
        has_more: page.has_more,
        inventory_total: page.inventory_total,
        source_kinds: page.source_kinds,
        limit: page.limit,
        offset: page.offset,
        filter_options: page.filter_options.into(),
        score_count,
    })
}

/// 分析当前激活档案中的六星 +15 御魂成长质量；只读取快照事实，不写入库存或分析缓存。
#[tauri::command]
pub fn analyze_plus15_growth_quality(
    state: State<'_, AppState>,
) -> Result<GrowthQualityReportDto, AppError> {
    let use_case = GrowthQualityUseCase::new(state.services.clone());
    Ok(use_case
        .analyze(active_profile(&state.services).as_deref())?
        .into())
}

/// 分析当前激活档案中的六星 `+0` 四腿胚子；只读取快照事实，不写入库存或分析缓存。
#[tauri::command]
pub fn analyze_four_leg_embryo_decision(
    state: State<'_, AppState>,
) -> Result<EmbryoDecisionReportDto, AppError> {
    let use_case = EmbryoDecisionUseCase::new(state.services.clone());
    Ok(use_case
        .analyze(active_profile(&state.services).as_deref())?
        .into())
}

/// 按用户勾选的腿数分析六星 `+0` 胚子；未勾选腿数不读取为候选，也不写入分析结果。
#[tauri::command]
pub fn analyze_embryo_decision(
    state: State<'_, AppState>,
    request: Option<EmbryoDecisionRequestDto>,
) -> Result<EmbryoDecisionReportDto, AppError> {
    let request = request.unwrap_or_default();
    let use_case = EmbryoDecisionUseCase::new(state.services.clone());
    Ok(use_case
        .analyze_with_legs(
            active_profile(&state.services).as_deref(),
            request.include_three_leg,
            request.include_four_leg,
        )?
        .into())
}

/// 列出当前导入账号的式神；原始 heroes 只在 Rust 端解析，前端得到的是安全的归一化列表。
#[tauri::command]
pub fn list_my_shikigami(state: State<'_, AppState>) -> Result<Vec<OwnedShikigamiDto>, AppError> {
    Ok(state
        .services
        .current_data
        .list_owned_shikigami()?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 列出式神仓库中的素材式神分组；游戏只保存分组数量，没有逐只实例。
#[tauri::command]
pub fn list_shikigami_bag(
    state: State<'_, AppState>,
) -> Result<Vec<ShikigamiBagGroupDto>, AppError> {
    Ok(state
        .services
        .current_data
        .list_shikigami_bag()?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 列出当前导入的 SSR、SP、UR 与御行达摩碎片；原始快照字段只在 Rust 端解析。
#[tauri::command]
pub fn list_shikigami_shards(
    state: State<'_, AppState>,
) -> Result<Vec<ShikigamiShardDto>, AppError> {
    Ok(state
        .services
        .current_data
        .list_shikigami_shards()?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 列出当前账号按式神归一化的全部传记进度；没有新字段时返回空列表，不推断未解锁状态。
#[tauri::command]
pub fn list_shikigami_story_progress(
    state: State<'_, AppState>,
) -> Result<Vec<ShikigamiStoryProgress>, AppError> {
    state.services.current_data.list_shikigami_story_progress()
}

/// 查询全部游戏档案中的目标式神与碎片；不切换当前激活角色，单个旧档案按不可用行返回。
#[tauri::command]
pub fn lookup_shikigami_shards_across_archives(
    state: State<'_, AppState>,
    request: ShikigamiShardLookupRequestDto,
) -> Result<ShikigamiShardLookupResultDto, AppError> {
    lookup_shikigami_shards(
        &state.services.character_archives,
        &state.services.current_data,
        &request.shikigami_id,
    )
}

/// 读取当前导入的资源/道具数量（21 项键值对象）；没有当前数据时返回空对象。
#[tauri::command]
pub fn get_current_items(state: State<'_, AppState>) -> Result<serde_json::Value, AppError> {
    state.services.current_data.list_items()
}

/// 读取当前导入的结界卡列表；没有当前数据时返回空数组。
#[tauri::command]
pub fn get_current_realm_cards(state: State<'_, AppState>) -> Result<serde_json::Value, AppError> {
    state.services.current_data.list_realm_cards()
}

/// 读取当前数据摘要（存在性、来源、导入时间、御魂/式神数量）；首次使用时返回不存在的摘要。
#[tauri::command]
pub fn get_current_data_summary(
    state: State<'_, AppState>,
) -> Result<CurrentDataSummary, AppError> {
    state.services.current_data.summary()
}

/// 读取当前导入快照的寮管理数据（寮概况 + 成员列表）；无寮数据时返回 None。
#[tauri::command]
pub fn get_current_guild(state: State<'_, AppState>) -> Result<Option<GuildOverviewDto>, AppError> {
    let guild = state.services.current_data.guild()?;
    parse_guild(guild.as_ref())
}

/// 返回“我的御魂”列表所需的轻量评分摘要；没有完成计算的御魂不会伪造分数。
#[tauri::command]
pub fn list_my_soul_scores(
    state: State<'_, AppState>,
    request: Option<ListMySoulScoresRequest>,
) -> Result<Vec<SoulScoreSummaryDto>, AppError> {
    let Some(profile_id) = active_profile(&state.services) else {
        return Ok(Vec::new());
    };
    let analysis = AnalysisUseCase::new(state.services.clone());
    let summaries = match request {
        Some(request) => analysis.list_score_summaries_by_keys(&profile_id, &request.soul_keys)?,
        None => analysis.list_score_summaries(&profile_id)?,
    };
    Ok(summaries.into_iter().map(Into::into).collect())
}

/// 读取已保存的御魂雷达结果；进入页面时只读取聚合缓存，不重新扫描全部御魂。
#[tauri::command]
pub fn get_soul_radar_cache(
    state: State<'_, AppState>,
) -> Result<Option<SoulRadarCacheDto>, AppError> {
    let Some(profile_id) = active_profile(&state.services) else {
        return Ok(None);
    };
    Ok(SoulRadarUseCase::new(state.services.clone())
        .get(&profile_id)?
        .map(Into::into))
}

/// 显式计算并保存御魂雷达；首次点击或库存版本变化后才读取全部已确认御魂。
#[tauri::command]
pub fn calculate_soul_radar(state: State<'_, AppState>) -> Result<SoulRadarCacheDto, AppError> {
    let profile_id = active_profile(&state.services)
        .ok_or_else(|| AppError::not_found("activeProfile", "尚未导入任何角色数据，请先导入"))?;
    Ok(SoulRadarUseCase::new(state.services.clone())
        .calculate(&profile_id)?
        .into())
}

/// 读取已保存的头尾分析结果；进入页面时只读取 SQLite 派生缓存，不重新扫描库存事实。
#[tauri::command]
pub fn get_head_tail_cache(
    state: State<'_, AppState>,
) -> Result<Option<HeadTailCacheDto>, AppError> {
    let Some(profile_id) = active_profile(&state.services) else {
        return Ok(None);
    };
    Ok(HeadTailUseCase::new(state.services.clone())
        .get(&profile_id)?
        .map(Into::into))
}

/// 显式计算并保存当前角色的全部头尾候选；库存版本变化后由用户再次点击重算。
#[tauri::command]
pub fn calculate_head_tail(state: State<'_, AppState>) -> Result<HeadTailCacheDto, AppError> {
    let profile_id = active_profile(&state.services)
        .ok_or_else(|| AppError::not_found("activeProfile", "尚未导入任何角色数据，请先导入"))?;
    Ok(HeadTailUseCase::new(state.services.clone())
        .calculate(&profile_id)?
        .into())
}

/// 读取当前库存中的六星未强化御魂并在内存中模拟到+15；绝不写回库存或分析结果。
#[tauri::command]
pub fn simulate_enhancement(
    state: State<'_, AppState>,
    request: SimulateEnhancementRequest,
) -> Result<SimulateEnhancementResultDto, AppError> {
    let profile_id = active_profile(&state.services)
        .ok_or_else(|| AppError::not_found("activeProfile", "尚未导入任何角色数据，请先导入"))?;
    SimulationUseCase::new(state.services.clone()).simulate(&profile_id, request.into())
}

/// 按当前库存计算神奇海螺目标面板；结果只存在前端当前页面，不写回库存或行动批次。
#[tauri::command]
pub fn calculate_miracle_conch(
    state: State<'_, AppState>,
    request: MiracleConchRequestDto,
) -> Result<MiracleConchResultDto, AppError> {
    // 记录 Tauri 命令边界，区分“前端未发起/参数未反序列化”和“应用层或领域搜索卡住”。
    let command_started_at = Instant::now();
    tracing::info!(
        target: "miracle_conch",
        request = ?request,
        "神奇海螺 Tauri 命令已收到"
    );
    let result = MiracleConchUseCase::new(state.services.clone()).calculate(request);
    match &result {
        Ok(result) => tracing::info!(
            target: "miracle_conch",
            status = %result.status,
            search_complete = result.search_complete,
            candidate_count = result.candidate_count,
            solution_count = result.solutions.len(),
            enhancement_count = result.enhancement_candidates.len(),
            elapsed_ms = command_started_at.elapsed().as_millis() as u64,
            "神奇海螺 Tauri 命令已返回"
        ),
        Err(error) => tracing::error!(
            target: "miracle_conch",
            error = %error,
            elapsed_ms = command_started_at.elapsed().as_millis() as u64,
            "神奇海螺 Tauri 命令返回错误"
        ),
    }
    result
}

/// 读取神奇海螺页面配置；文件不存在时返回空值，由前端使用默认配置。
#[tauri::command]
pub fn get_miracle_conch_configuration(
    paths: State<'_, AppPaths>,
) -> Result<Option<String>, AppError> {
    let path = paths
        .config_directory
        .join(MIRACLE_CONCH_CONFIGURATION_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    fs::read_to_string(&path)
        .map(Some)
        .map_err(|error| AppError::io("读取神奇海螺页面配置", &error))
}

/// 保存神奇海螺页面配置；先校验 JSON，避免损坏内容覆盖下一次打开时的配置恢复。
#[tauri::command]
pub fn set_miracle_conch_configuration(
    paths: State<'_, AppPaths>,
    configuration: String,
) -> Result<(), AppError> {
    serde_json::from_str::<serde_json::Value>(&configuration).map_err(|error| {
        AppError::invalid_argument(
            "miracleConchConfiguration",
            format!("神奇海螺页面配置格式错误：{error}"),
        )
    })?;
    fs::create_dir_all(&paths.config_directory)
        .map_err(|error| AppError::io("创建神奇海螺配置目录", &error))?;
    let path = paths
        .config_directory
        .join(MIRACLE_CONCH_CONFIGURATION_FILE);
    fs::write(&path, configuration.as_bytes())
        .map_err(|error| AppError::io("保存神奇海螺页面配置", &error))?;
    Ok(())
}

// ─── 历史、分析中心与数据恢复辅助 ─────────────────────────────────────────────

/// 返回快照时间线、采集事件和当前基线；局部基线的删除确认能力由后端明确计算。
#[tauri::command]
pub fn get_history_overview(
    state: State<'_, AppState>,
    request: HistoryOverviewRequest,
) -> Result<HistoryOverviewDto, AppError> {
    HistoryAnalysisUseCase::new(state.services.clone())
        .history_overview(&request.profile_id, request.event_limit)
}

/// 返回分析中心指标，所有图表数量来自当前档案的同一份事实查询。
#[tauri::command]
pub fn get_analysis_center(
    state: State<'_, AppState>,
    request: AnalysisCenterRequest,
) -> Result<AnalysisCenterDto, AppError> {
    HistoryAnalysisUseCase::new(state.services.clone()).analysis_center(&request.profile_id)
}

/// 返回单枚御魂到快照、目录、规则、练度和解释树的完整追溯链。
#[tauri::command]
pub fn get_analysis_trace(
    state: State<'_, AppState>,
    request: AnalysisTraceRequest,
) -> Result<AnalysisTraceDto, AppError> {
    HistoryAnalysisUseCase::new(state.services.clone())
        .analysis_trace(&request.profile_id, &request.soul_key)
}

/// 校验档案引用的原始对象，并把损坏/缺失状态关联回受影响快照。
#[tauri::command]
pub fn verify_profile_objects(
    state: State<'_, AppState>,
    request: VerifyProfileObjectsRequest,
) -> Result<ProfileObjectVerificationDto, AppError> {
    HistoryAnalysisUseCase::new(state.services.clone()).verify_profile_objects(&request.profile_id)
}

/// 生成档案、快照或筛选结果导出内容；WebView 只接收当前档案范围内的 JSON。
#[tauri::command]
pub fn export_data(
    state: State<'_, AppState>,
    request: ExportDataRequest,
) -> Result<ExportPayloadDto, AppError> {
    HistoryAnalysisUseCase::new(state.services.clone()).export_data(
        &request.profile_id,
        &request.kind,
        request.snapshot_id.as_deref(),
        request.category.as_deref(),
        request.search.as_deref(),
    )
}

// ─── 分析待办与行动批次 ───────────────────────────────────────────────────────

/// 按用户操作重算档案分析；导入命令只更新库存，不会自动触发评分。
#[tauri::command]
pub fn recalculate_analysis(
    state: State<'_, AppState>,
    request: RecalculateAnalysisRequest,
) -> Result<u32, AppError> {
    AnalysisUseCase::new(state.services.clone())
        .recalculate(&request.profile_id, request.snapshot_id.as_deref())
}

/// 分页读取分析待办；筛选、搜索和排序由 Rust/SQLite 完成。
#[tauri::command]
pub fn list_analysis_todos(
    state: State<'_, AppState>,
    request: ListAnalysisTodosRequest,
) -> Result<AnalysisTodoPageDto, AppError> {
    AnalysisUseCase::new(state.services.clone())
        .list_todos(
            &request.profile_id,
            request.category.as_deref(),
            request.search.as_deref(),
            request.use_id.as_deref(),
            request.limit,
            request.offset,
        )
        .map(Into::into)
}

/// 预览用户决定批量变化，明确新增、覆盖、跳过、保护和冲突数量。
#[tauri::command]
pub fn preview_decisions(
    state: State<'_, AppState>,
    request: PreviewDecisionsRequest,
) -> Result<DecisionPreviewDto, AppError> {
    AnalysisUseCase::new(state.services.clone()).preview_decisions(
        &request.profile_id,
        &request.soul_keys,
        &request.decision,
        request.respect_protection,
    )
}

/// 在一个事务中写入单个或批量用户决定。
#[tauri::command]
pub fn apply_decisions(
    state: State<'_, AppState>,
    request: ApplyDecisionsRequest,
) -> Result<DecisionApplyResultDto, AppError> {
    AnalysisUseCase::new(state.services.clone()).apply_decisions(
        &request.profile_id,
        request.changes,
        request.respect_protection,
    )
}

/// 撤销一组用户决定；不会回滚之后产生的新决定。
#[tauri::command]
pub fn undo_decision(
    state: State<'_, AppState>,
    request: UndoDecisionRequest,
) -> Result<(), AppError> {
    AnalysisUseCase::new(state.services.clone()).undo_decision(&request.operation_id)
}

/// 读取单枚御魂的用户决定历史。
#[tauri::command]
pub fn list_decision_history(
    state: State<'_, AppState>,
    request: DecisionHistoryRequest,
) -> Result<Vec<DecisionHistoryEntryDto>, AppError> {
    AnalysisUseCase::new(state.services.clone())
        .list_decision_history(&request.profile_id, &request.soul_key)
}

/// 创建按套装、号位、主属性和等级分组的强化/清理批次。
#[tauri::command]
pub fn create_action_batch(
    state: State<'_, AppState>,
    request: CreateActionBatchRequest,
) -> Result<ActionBatchDto, AppError> {
    AnalysisUseCase::new(state.services.clone()).create_batch(&CreateBatchRequest {
        profile_id: request.profile_id,
        kind: request.kind,
        target_level: request.target_level,
        soul_keys: request.soul_keys,
    })
}

/// 列出档案强化/清理批次。
#[tauri::command]
pub fn list_action_batches(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<Vec<ActionBatchDto>, AppError> {
    AnalysisUseCase::new(state.services.clone()).list_batches(&profile_id)
}

/// 读取行动批次详情和逐枚状态。
#[tauri::command]
pub fn get_action_batch(
    state: State<'_, AppState>,
    profile_id: String,
    batch_id: String,
) -> Result<ActionBatchDetailDto, AppError> {
    AnalysisUseCase::new(state.services.clone()).get_batch(&profile_id, &batch_id)
}

/// 更新批次条目为完成、跳过或找不到。
#[tauri::command]
pub fn update_action_batch_item(
    state: State<'_, AppState>,
    request: UpdateBatchItemRequest,
) -> Result<ActionBatchDetailDto, AppError> {
    AnalysisUseCase::new(state.services.clone()).update_batch_item(
        &request.profile_id,
        &request.batch_id,
        &request.soul_key,
        &request.status,
        request.note.as_deref(),
    )
}

/// 请求取消指定后台任务，取消后的最终进度由统一事件通道确认。
#[tauri::command]
pub fn cancel_background_task(
    registry: State<'_, TaskRegistry>,
    task_id: String,
) -> Result<(), AppError> {
    registry.cancel(&task_id)
}

// ─── 数据库状态 ───────────────────────────────────────────────────────────────

/// 返回数据库引擎的元数据与统计。
#[tauri::command]
pub fn get_database_status(state: State<'_, AppState>) -> Result<DatabaseStatusDto, AppError> {
    let services = &state.services;

    let schema_version = services.runner.latest_version();
    let migrations_applied: u32 = services.db.read("migrations_count", |conn| {
        conn.query_row("SELECT count(*) FROM schema_migration", [], |row| {
            row.get(0)
        })
        .map_err(|e| AppError::database("migrations", &e))
    })?;

    let wal_enabled: bool = services.db.read("wal_check", |conn| {
        conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .map_err(|e| AppError::database("wal_check", &e))
            .map(|mode| mode == "wal")
    })?;

    let profile_count: u32 = services.db.read("profile_count", |conn| {
        conn.query_row("SELECT count(*) FROM game_profile", [], |row| row.get(0))
            .map_err(|e| AppError::database("profile_count", &e))
    })?;

    let raw_stats = services.raw_objects.stats()?;
    let active_catalog = services
        .catalog
        .active_status()
        .ok()
        .flatten()
        .map(|s| s.version);
    let backup_count: u32 = services.db.read("backup_count", |conn| {
        conn.query_row("SELECT count(*) FROM backup_record", [], |row| row.get(0))
            .map_err(|e| AppError::database("backup_count", &e))
    })?;

    Ok(DatabaseStatusDto {
        schema_version,
        migrations_applied,
        wal_enabled,
        profile_count,
        raw_object_count: raw_stats.object_count,
        raw_object_raw_bytes: raw_stats.raw_bytes,
        raw_object_stored_bytes: raw_stats.stored_bytes,
        active_catalog_version: active_catalog,
        backup_count,
    })
}

// ─── 游戏档案 ─────────────────────────────────────────────────────────────────

/// 创建新游戏档案。
#[tauri::command]
pub fn create_profile(
    state: State<'_, AppState>,
    request: CreateProfileRequest,
) -> Result<ProfileCreatedDto, AppError> {
    let uc = ProfileUseCase::new(state.services.clone());
    let new = NewGameProfile {
        display_name: request.display_name,
        source_identity: request.source_identity,
        server_label: request.server_label,
    };
    let profile = uc.create(new)?;
    Ok(ProfileCreatedDto {
        profile: profile.into(),
    })
}

/// 列出游戏档案。
#[tauri::command]
pub fn list_profiles(
    state: State<'_, AppState>,
    include_archived: Option<bool>,
) -> Result<Vec<GameProfileDto>, AppError> {
    let uc = ProfileUseCase::new(state.services.clone());
    Ok(uc
        .list(include_archived.unwrap_or(false))?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 重命名游戏档案。
#[tauri::command]
pub fn rename_profile(
    state: State<'_, AppState>,
    request: RenameProfileRequest,
) -> Result<GameProfileDto, AppError> {
    let uc = ProfileUseCase::new(state.services.clone());
    uc.rename(
        &request.profile_id,
        &request.new_name,
        request.expected_revision,
    )
    .map(Into::into)
}

/// 归档或取消归档游戏档案。
#[tauri::command]
pub fn archive_profile(
    state: State<'_, AppState>,
    request: ArchiveProfileRequest,
) -> Result<GameProfileDto, AppError> {
    let uc = ProfileUseCase::new(state.services.clone());
    uc.set_archived(
        &request.profile_id,
        request.archived,
        request.expected_revision,
    )
    .map(Into::into)
}

// ─── 目录 ─────────────────────────────────────────────────────────────────────

/// 安装内置示例目录。
#[tauri::command]
pub fn install_builtin_catalog(state: State<'_, AppState>) -> Result<CatalogStatusDto, AppError> {
    let uc = CatalogUseCase::new(state.services.clone());
    uc.install_builtin().map(Into::into)
}

/// 当前活跃目录状态。
#[tauri::command]
pub fn get_catalog_status(
    state: State<'_, AppState>,
) -> Result<Option<CatalogStatusDto>, AppError> {
    let uc = CatalogUseCase::new(state.services.clone());
    // 目录状态是所有目录页面的共同入口；先刷新旧的内置版本，避免页面继续读取数据库中的旧文案。
    uc.ensure_current_builtin()?;
    Ok(uc.active_status()?.map(Into::into))
}

/// 列出套装。
#[tauri::command]
pub fn list_soul_sets(
    state: State<'_, AppState>,
    catalog_version: String,
) -> Result<Vec<SoulSetDto>, AppError> {
    let uc = CatalogUseCase::new(state.services.clone());
    Ok(uc
        .list_sets(&catalog_version)?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 按目录版本列出式神详情；所有嵌套面板与技能字段由 DTO 边界统一解码。
#[tauri::command]
pub fn list_shikigami(
    state: State<'_, AppState>,
    catalog_version: String,
) -> Result<Vec<ShikigamiDto>, AppError> {
    let uc = CatalogUseCase::new(state.services.clone());
    Ok(uc
        .list_shikigami(&catalog_version)?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 获取档案的有效常用度。
#[tauri::command]
pub fn get_effective_commonness(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<Vec<EffectiveCommonnessDto>, AppError> {
    let uc = CatalogUseCase::new(state.services.clone());
    Ok(uc
        .effective_commonness(&profile_id)?
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 设置或清除档案常用度覆盖。
#[tauri::command]
pub fn set_commonness_override(
    state: State<'_, AppState>,
    request: CommonnessOverrideRequest,
) -> Result<(), AppError> {
    let uc = CatalogUseCase::new(state.services.clone());
    uc.set_commonness_override(
        &request.profile_id,
        &request.set_id,
        &request.scenario,
        &request.value,
    )
}

/// 复制常用度覆盖到另一档案。
#[tauri::command]
pub fn copy_commonness(
    state: State<'_, AppState>,
    request: CopyCommonnessRequest,
) -> Result<u32, AppError> {
    let uc = CatalogUseCase::new(state.services.clone());
    uc.copy_commonness(&request.from_profile_id, &request.to_profile_id)
        .map(|c| c as u32)
}

// ─── 原始对象 ─────────────────────────────────────────────────────────────────

/// 保存原始字节。
#[tauri::command]
pub fn store_raw_object(
    state: State<'_, AppState>,
    request: StoreRawRequest,
) -> Result<StoredRawObjectDto, AppError> {
    let uc = StorageUseCase::new(state.services.clone());
    let summary = uc.store_raw(
        &request.profile_id,
        &request.source_kind,
        &request.source_format,
        &request.media_type,
        &request.payload,
        &request.received_at,
    )?;
    Ok(StoredRawObjectDto {
        sha256: summary.raw_object.sha256,
        raw_size: summary.stored.raw_size,
        stored_size: summary.stored.stored_size,
        is_new: summary.is_new,
    })
}

/// 校验原始对象哈希。
#[tauri::command]
pub fn verify_raw_object(state: State<'_, AppState>, sha256: String) -> Result<(), AppError> {
    let uc = StorageUseCase::new(state.services.clone());
    uc.verify_object(&sha256)
}

// ─── 备份 ─────────────────────────────────────────────────────────────────────

/// 创建备份。
#[tauri::command]
pub fn create_backup(
    state: State<'_, AppState>,
    request: CreateBackupRequest,
) -> Result<BackupRecordDto, AppError> {
    let uc = BackupUseCase::new(state.services.clone());
    uc.create_backup(&request.reason).map(Into::into)
}

/// 列出备份记录。
#[tauri::command]
pub fn list_backups(state: State<'_, AppState>) -> Result<Vec<BackupRecordDto>, AppError> {
    let uc = BackupUseCase::new(state.services.clone());
    Ok(uc.list_backups()?.into_iter().map(Into::into).collect())
}

/// 校验备份。
#[tauri::command]
pub fn validate_backup(state: State<'_, AppState>, backup_id: String) -> Result<(), AppError> {
    let uc = BackupUseCase::new(state.services.clone());
    uc.validate_backup(&backup_id)?;
    Ok(())
}

/// 恢复备份。
#[tauri::command]
pub fn restore_backup(state: State<'_, AppState>, backup_id: String) -> Result<(), AppError> {
    let uc = BackupUseCase::new(state.services.clone());
    uc.restore_backup(&backup_id)
}

// ─── 签名更新 ─────────────────────────────────────────────────────────────────

/// 检查 Gitee 最新 Release；启动模式只校验并保存清单，手动页面默认同时暂存载荷。
#[tauri::command]
pub async fn check_updates(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: State<'_, AppPaths>,
    request: CheckUpdatesRequest,
) -> Result<crate::application::updates::UpdateCheckResult, AppError> {
    // 通道由构建产物决定，前端参数只用于一致性检查，防止正式版请求测试 Release。
    let build_channel = build_update_channel();
    if request.channel != build_channel {
        return Err(AppError::update_rejected(format!(
            "更新通道与当前构建不匹配：当前为 {}",
            build_channel.as_str()
        )));
    }
    let app_version = app.package_info().version.to_string();
    let use_case = UpdateUseCase::new(
        state.services.clone(),
        paths.data_directory.join("packages"),
        paths.temp_directory.clone(),
        paths.config_directory.clone(),
    );
    let source_mode = request.source_mode;
    let package_type = request.package_type;
    let game_version = request.game_version;
    let stage_payload = request.stage_payload;
    tauri::async_runtime::spawn_blocking(move || {
        use_case.check_and_stage(
            source_mode,
            build_channel,
            package_type,
            &app_version,
            game_version.as_deref(),
            stage_payload,
        )
    })
    .await
    .map_err(|error| AppError::internal(format!("更新检查后台任务异常结束：{error}")))?
}

/// 安装暂存更新；读取会话或任一后台批次运行时只返回 deferred 状态。
#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: State<'_, AppPaths>,
    registry: State<'_, TaskRegistry>,
    request: InstallUpdateRequest,
) -> Result<crate::application::updates::UpdateInstallResult, AppError> {
    let app_version = app.package_info().version.to_string();
    let channel = build_update_channel();
    let use_case = UpdateUseCase::new(
        state.services.clone(),
        paths.data_directory.join("packages"),
        paths.temp_directory.clone(),
        paths.config_directory.clone(),
    );
    let staged_update_id = request.staged_update_id;
    let game_version = request.game_version;
    let auto_restart = request.auto_restart;
    let runtime_busy = registry.has_running();
    let progress_app = app.clone();
    let progress_stage_id = staged_update_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut emit_progress = |phase: &str, completed: u64, total: u64| {
            // 安装阶段可能因系统权限确认和文件替换耗时较久，需要明确提示用户耐心等待。
            let message = match phase {
                "downloading" => "正在下载更新安装包",
                "verifying" => "正在校验更新安装包",
                "installing" => "正在准备安装并重启，此过程耗时较久，属于正常情况，请耐心等待",
                _ => "正在处理更新",
            };
            let _ = progress_app.emit(
                UPDATE_PROGRESS_EVENT,
                UpdateProgressDto {
                    staged_update_id: progress_stage_id.clone(),
                    phase: phase.to_owned(),
                    completed,
                    total,
                    message: message.to_owned(),
                },
            );
        };
        use_case.install_staged_with_progress(
            &staged_update_id,
            channel,
            &app_version,
            game_version.as_deref(),
            runtime_busy,
            auto_restart,
            &mut emit_progress,
        )
    })
    .await
    .map_err(|error| AppError::internal(format!("更新安装后台任务异常结束：{error}")))??;
    if result.state == "restarting" {
        // 先让前端收到“重启中”结果，再强制结束旧进程，避免 Tauri 事件循环迟迟不退出。
        let exit_app = app.clone();
        let exit_task = std::thread::Builder::new()
            .name("update-process-exit".to_owned())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let _ = exit_app.exit(0);
                // 更新助手依赖父进程退出后释放安装目录文件锁；这里保证退出不会被窗口或插件阻塞。
                std::process::exit(0);
            });
        if exit_task.is_err() {
            // 极端情况下线程创建失败仍尝试走 Tauri 的正常退出流程。
            let _ = app.exit(0);
        }
    }
    Ok(result)
}

/// 列出数据包安装历史，供更新中心展示来源、签名身份和手动回退入口。
#[tauri::command]
pub fn list_installed_packages(
    state: State<'_, AppState>,
) -> Result<Vec<InstalledPackageDto>, AppError> {
    let use_case = UpdateUseCase::new(
        state.services.clone(),
        PathBuf::new(),
        PathBuf::new(),
        PathBuf::new(),
    );
    use_case.list_installed()
}

/// 用户明确选择历史版本后执行回退；稳定通道不会自动触发此命令。
#[tauri::command]
pub async fn rollback_update(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: State<'_, AppPaths>,
    registry: State<'_, TaskRegistry>,
    request: RollbackUpdateRequest,
) -> Result<crate::application::updates::UpdateInstallResult, AppError> {
    let app_version = app.package_info().version.to_string();
    let channel = build_update_channel();
    let use_case = UpdateUseCase::new(
        state.services.clone(),
        paths.data_directory.join("packages"),
        paths.temp_directory.clone(),
        paths.config_directory.clone(),
    );
    let package_type = request.package_type;
    let version = request.version;
    let game_version = request.game_version;
    let runtime_busy = registry.has_running();
    tauri::async_runtime::spawn_blocking(move || {
        use_case.rollback(
            package_type,
            channel,
            &version,
            &app_version,
            game_version.as_deref(),
            runtime_busy,
        )
    })
    .await
    .map_err(|error| AppError::internal(format!("更新回退后台任务异常结束：{error}")))?
}

// ─── 规则引擎 ─────────────────────────────────────────────────────────────────

/// 返回内置默认预设的稳定元数据；加载过程会重复执行结构和复杂度校验。
#[tauri::command]
pub fn get_default_rule_preset() -> Result<RulePresetSummaryDto, AppError> {
    let preset = load_default_preset()
        .map_err(|error| AppError::invalid_argument("preset", error.to_string()))?;
    Ok(RulePresetSummaryDto {
        id: preset.preset.id,
        version: preset.preset.version,
        title: preset.preset.title,
        author: preset.preset.author,
        status: preset.preset.status.as_str().to_owned(),
        rule_count: preset.preset.rules.len(),
        normalized_hash: preset.normalized_hash,
    })
}

/// 评估默认胚子准入，返回每条规则的命中细节和候选用途。
#[tauri::command]
pub fn evaluate_default_rule_admission(
    request: EvaluateDefaultAdmissionRequest,
) -> Result<crate::domain::AdmissionResult, AppError> {
    let preset = load_default_preset()
        .map_err(|error| AppError::invalid_argument("preset", error.to_string()))?;
    Ok(RuleEngine::new().evaluate_admission(&preset, &request.facts))
}

/// 返回七个特殊用途模板的版本和证据状态，供分析入口展示未核验提醒。
#[tauri::command]
pub fn list_builtin_use_templates() -> Vec<BuiltinUseTemplateDto> {
    builtin_use_templates()
        .into_iter()
        .map(|template| BuiltinUseTemplateDto {
            id: template.id,
            title: template.title,
            status: template.status.as_str().to_owned(),
            evidence_level: serde_json::to_value(template.evidence_level)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned()),
        })
        .collect()
}

// ─── 规则编辑、版本与离线分享 ─────────────────────────────────────────────────

/// 读取规则库版本和当前档案的启用状态。
#[tauri::command]
pub fn list_rule_versions(
    state: State<'_, AppState>,
    request: ListRuleVersionsRequest,
) -> Result<Vec<RuleVersionDto>, AppError> {
    RuleUseCase::new(state.services.clone()).list_versions(request.profile_id.as_deref())
}

/// 读取不绑定游戏档案的评分标准，并返回 SQL 中保存的全局启用状态。
#[tauri::command]
pub fn list_score_standards(state: State<'_, AppState>) -> Result<Vec<RuleVersionDto>, AppError> {
    RuleUseCase::new(state.services.clone()).list_score_standards()
}

/// 导入前仅做校验和影响信息准备，不写入数据库。
#[tauri::command]
pub fn preview_rule_preset(
    state: State<'_, AppState>,
    request: RulePayloadRequest,
) -> Result<RulePresetPreviewDto, AppError> {
    RuleUseCase::new(state.services.clone()).preview(&request.source_kind, &request.payload)
}

/// 确认导入规则；用户导入版本可直接编辑，但默认不启用。
#[tauri::command]
pub fn import_rule_preset(
    state: State<'_, AppState>,
    request: RulePayloadRequest,
) -> Result<RuleVersionDto, AppError> {
    RuleUseCase::new(state.services.clone()).import(&request.source_kind, &request.payload)
}

/// 从只读版本创建个人可编辑副本。
#[tauri::command]
pub fn copy_rule_version(
    state: State<'_, AppState>,
    version_id: String,
) -> Result<RuleVersionDto, AppError> {
    RuleUseCase::new(state.services.clone()).copy(&version_id)
}

/// 保存完整的个人规则版本；版本保存后由前端触发可取消的分析重算任务。
#[tauri::command]
pub fn save_rule_version(
    state: State<'_, AppState>,
    request: SaveRuleVersionRequest,
) -> Result<RuleVersionDto, AppError> {
    RuleUseCase::new(state.services.clone()).save_editable(
        &request.source_kind,
        &request.payload,
        request.parent_version_id.as_deref(),
    )
}

/// 生成当前版本的规范文件和离线分享码。
#[tauri::command]
pub fn export_rule_version(
    state: State<'_, AppState>,
    version_id: String,
) -> Result<RuleExportDto, AppError> {
    let (file_content, share_code) =
        RuleUseCase::new(state.services.clone()).export(&version_id)?;
    Ok(RuleExportDto {
        file_content,
        share_code,
    })
}

/// 写入档案级启用状态；禁用不会删除版本或历史正文。
#[tauri::command]
pub fn set_rule_activation(
    state: State<'_, AppState>,
    request: SetRuleActivationRequest,
) -> Result<(), AppError> {
    RuleUseCase::new(state.services.clone()).set_activation(
        &request.profile_id,
        &request.rule_version_id,
        request.enabled,
        request.position,
        request.note,
    )
}

/// 将一个评分标准设为全局当前标准；同一时间只保留一个启用项。
#[tauri::command]
pub fn set_active_score_standard(
    state: State<'_, AppState>,
    version_id: String,
) -> Result<(), AppError> {
    RuleUseCase::new(state.services.clone()).set_active_score_standard(&version_id)
}

/// 删除用户自己的评分标准；删除当前标准时由业务层优先切换到其他用户标准。
#[tauri::command]
pub fn delete_score_standard(
    state: State<'_, AppState>,
    version_id: String,
) -> Result<(), AppError> {
    RuleUseCase::new(state.services.clone()).delete_score_standard(&version_id)
}

/// 预览规则对当前档案分析缓存的命中、样本和潜在变化。
#[tauri::command]
pub fn preview_rule_impact(
    state: State<'_, AppState>,
    request: RuleImpactRequest,
) -> Result<RuleImpactPreviewDto, AppError> {
    RuleUseCase::new(state.services.clone()).impact_preview(
        &request.profile_id,
        &request.source_kind,
        &request.payload,
    )
}

/// 规则版本变化后的可取消分析重算；终态复用统一任务进度事件协议。
#[tauri::command]
pub fn start_rule_recalculation_task(
    app: AppHandle,
    registry: State<'_, TaskRegistry>,
    request: StartRuleRecalculationRequest,
) -> Result<TaskAcceptedDto, AppError> {
    if request.profile_id.trim().is_empty() {
        return Err(AppError::invalid_argument("profileId", "档案 ID 不能为空"));
    }
    let registry = registry.inner().clone();
    let services = app.state::<AppState>().services.clone();
    let (task_id, cancellation) = registry.register();
    let worker_task_id = task_id.clone();
    let profile_id = request.profile_id;
    let snapshot_id = request.snapshot_id;
    let cancellation_for_worker = cancellation.clone();

    let _worker = tauri::async_runtime::spawn(async move {
        let event_app = app.clone();
        let _ = event_app.emit(
            TASK_PROGRESS_EVENT,
            TaskProgress {
                task_id: worker_task_id.clone(),
                phase: "rule-recalculation".to_owned(),
                completed: 0,
                total: 1,
                message: "规则版本已保存，正在重新计算分析待办".to_owned(),
                status: TaskStatus::Running,
                error: None,
                result: None,
            },
        );
        let progress_app = event_app.clone();
        let progress_task_id = worker_task_id.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            if cancellation_for_worker.load(Ordering::Acquire) {
                return Ok(None);
            }
            let mut last_percent = u32::MAX;
            AnalysisUseCase::new(services).recalculate_with_progress(
                &profile_id,
                snapshot_id.as_deref(),
                |completed, total| {
                    if cancellation_for_worker.load(Ordering::Acquire) {
                        return false;
                    }
                    let percent = if total == 0 {
                        100
                    } else {
                        completed.saturating_mul(100) / total
                    };
                    // 最多每个百分点评估一次事件，避免大量库存把 WebView 刷屏。
                    if percent != last_percent || completed == total {
                        last_percent = percent;
                        let _ = progress_app.emit(
                            TASK_PROGRESS_EVENT,
                            TaskProgress {
                                task_id: progress_task_id.clone(),
                                phase: "rule-recalculation".to_owned(),
                                completed,
                                total: total.max(1),
                                message: if total == 0 {
                                    "库存为空，正在保存计算结果".to_owned()
                                } else {
                                    format!("正在计算御魂评分：{completed} / {total} 枚")
                                },
                                status: TaskStatus::Running,
                                error: None,
                                result: None,
                            },
                        );
                    }
                    true
                },
            )
        })
        .await;
        let progress = if cancellation.load(Ordering::Acquire) {
            TaskProgress {
                task_id: worker_task_id.clone(),
                phase: "rule-recalculation".to_owned(),
                completed: 0,
                total: 1,
                message: "规则重算已取消".to_owned(),
                status: TaskStatus::Cancelled,
                error: None,
                result: None,
            }
        } else {
            match result {
                Ok(Ok(Some(count))) => TaskProgress {
                    task_id: worker_task_id.clone(),
                    phase: "rule-recalculation".to_owned(),
                    completed: 1,
                    total: 1,
                    message: format!("规则重算完成，共更新 {count} 条待办"),
                    status: TaskStatus::Completed,
                    error: None,
                    result: Some(serde_json::json!({ "todoCount": count })),
                },
                Ok(Ok(None)) => TaskProgress {
                    task_id: worker_task_id.clone(),
                    phase: "rule-recalculation".to_owned(),
                    completed: 0,
                    total: 1,
                    message: "规则重算已取消".to_owned(),
                    status: TaskStatus::Cancelled,
                    error: None,
                    result: None,
                },
                Ok(Err(error)) => TaskProgress {
                    task_id: worker_task_id.clone(),
                    phase: "rule-recalculation".to_owned(),
                    completed: 0,
                    total: 1,
                    message: "规则重算失败".to_owned(),
                    status: TaskStatus::Failed,
                    error: Some(error),
                    result: None,
                },
                Err(error) => TaskProgress {
                    task_id: worker_task_id.clone(),
                    phase: "rule-recalculation".to_owned(),
                    completed: 0,
                    total: 1,
                    message: "规则重算后台线程异常结束".to_owned(),
                    status: TaskStatus::Failed,
                    error: Some(AppError::internal(error.to_string())),
                    result: None,
                },
            }
        };
        let _ = event_app.emit(TASK_PROGRESS_EVENT, progress);
        registry.complete(&worker_task_id);
    });

    Ok(TaskAcceptedDto { task_id })
}
