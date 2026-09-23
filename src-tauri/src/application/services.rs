//! 应用服务组合根：将领域仓库、基础设施和用例编排为统一的服务集合。

use crate::application::error::AppError;
use crate::domain::{
    AcquisitionEventRepository, ActionRepository, BackupRecordRepository, CatalogRepository,
    CommonnessRepository, HeadTailRepository, HistoryAnalysisRepository, ImportRepository,
    PackageRepository, ProfileRepository, RawObjectRepository, RuleRepository, SnapshotRepository,
    SoulRadarRepository,
};
use crate::infrastructure::character_archives::CharacterArchiveStore;
use crate::infrastructure::current_data::CurrentDataStore;
use crate::infrastructure::database::backup::BackupManager;
use crate::infrastructure::database::connection::Database;
use crate::infrastructure::database::migrations::MigrationRunner;
use crate::infrastructure::raw_object_store::RawObjectStore;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// 应用所需全部依赖的集合。
pub struct AppServices {
    pub db: Arc<Database>,
    /// 当前激活角色的数据档案 ID；None 表示尚未导入任何角色。
    /// 与 CurrentDataStore 共享同一把锁，切换角色时两者原子一致。
    pub active_profile: Arc<Mutex<Option<String>>>,
    /// 当前激活角色的数据文件；导入覆盖只更新该角色的一份正文。
    pub current_data: CurrentDataStore,
    /// 本机角色档案摘要列表；只有导入写入，重复导入同一角色覆盖旧信息。
    pub character_archives: CharacterArchiveStore,
    pub runner: MigrationRunner,
    pub profiles: Arc<dyn ProfileRepository>,
    pub raw_objects: Arc<dyn RawObjectRepository>,
    pub events: Arc<dyn AcquisitionEventRepository>,
    pub snapshots: Arc<dyn SnapshotRepository>,
    /// 御魂雷达派生结果仓库；页面进入时只读取这里的聚合缓存。
    pub radar: Arc<dyn SoulRadarRepository>,
    /// 头尾分析派生结果仓库；页面进入时只读取当前档案已保存的头尾候选缓存。
    pub head_tail: Arc<dyn HeadTailRepository>,
    /// 玩家持有式神实例的 SQLite 镜像；当前数据覆盖时整表替换。
    pub owned_shikigami: crate::infrastructure::repositories::SqliteOwnedShikigamiRepository,
    /// 历史时间线、分析中心和对象完整性共用的只读查询边界。
    pub history: Arc<dyn HistoryAnalysisRepository>,
    /// 导入专用事务仓库，与快照仓库共享同一 SQLite 写入队列。
    pub imports: Arc<dyn ImportRepository>,
    /// 分析待办、用户决定和行动批次的事务仓库。
    pub actions: Arc<dyn ActionRepository>,
    pub catalog: Arc<dyn CatalogRepository>,
    pub commonness: Arc<dyn CommonnessRepository>,
    /// 规则版本与档案级启用状态仓库。
    pub rules: Arc<dyn RuleRepository>,
    /// 签名更新安装历史与活动版本仓库。
    pub packages: Arc<dyn PackageRepository>,
    pub backup_records: Arc<dyn BackupRecordRepository>,
    pub raw_object_store: RawObjectStore,
    pub backup_manager: BackupManager,
    /// MuMu 预检失败时保存原始批次的诊断目录，避免虚拟文件名无法回溯实际载荷。
    mumu_diagnostics_directory: PathBuf,
}

impl AppServices {
    /// 当前激活角色的数据档案 ID；未导入任何角色时为 None。
    pub fn active_profile_id(&self) -> Option<String> {
        self.active_profile
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// 切换当前激活角色；调用方需要保证档案 ID 真实存在。
    pub fn set_active_profile(&self, profile_id: String) {
        *self
            .active_profile
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(profile_id);
    }

    /// 清空当前激活角色；清空全部档案时调用。
    pub fn clear_active_profile(&self) {
        *self
            .active_profile
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// 在指定路径上初始化数据库并运行迁移，构建全部服务。
    /// `backups_directory` 和 `temp_directory` 用于备份管理。
    pub fn initialize(
        database_path: PathBuf,
        snapshot_root: PathBuf,
        quarantine_directory: PathBuf,
        backups_directory: PathBuf,
        temp_directory: PathBuf,
    ) -> Result<Self, AppError> {
        let db = Arc::new(Database::open(&database_path)?);
        let runner = MigrationRunner::builtin();

        // 诊断文件跟随数据库位于应用数据目录，不能放入会在会话清理时删除的临时目录。
        let mumu_diagnostics_directory = database_path
            .parent()
            .map(|parent| parent.join("diagnostics").join("mumu-failed"))
            .unwrap_or_else(|| PathBuf::from("diagnostics/mumu-failed"));

        // 运行迁移
        db.write("migrate", |conn| runner.run(conn))?;

        let raw_object_store = RawObjectStore::new(
            snapshot_root.clone(),
            temp_directory.clone(),
            quarantine_directory,
        );
        let packages_directory = database_path
            .parent()
            .map(|parent| parent.join("packages"))
            .unwrap_or_else(|| PathBuf::from("packages"));
        let backup_manager = BackupManager::with_snapshot_root_and_packages(
            database_path.clone(),
            backups_directory,
            temp_directory,
            snapshot_root,
            packages_directory,
        );

        // 内置评分标准在数据库初始化时一次性落盘，页面读取时不再临时创建，
        // 这样内置规则和用户导入规则都遵循同一条 SQL 持久化链路。
        let rules: Arc<dyn RuleRepository> =
            Arc::new(crate::infrastructure::repositories::SqliteRuleRepository::new(db.clone()));
        let builtin_rule = crate::application::rule_use_cases::builtin_record()?;
        if rules.get_version(&builtin_rule.id)?.is_none() {
            rules.insert_version(&builtin_rule)?;
        }
        // 旧数据库可能仍指向“默认标准”；若已存在用户的铁血战士胖虎标准，启动时迁移全局指针，
        // 让正式分析和规则库页面都使用同一份用户标准。没有该标准时才保留内置兜底。
        let active_id = rules.active_score_standard_id()?;
        let preferred_id = latest_preferred_score_standard_id(rules.as_ref())?;
        if active_id.is_none() {
            let target_id = preferred_id.unwrap_or_else(|| builtin_rule.id.clone());
            let note = if target_id == builtin_rule.id {
                "系统默认评分标准"
            } else {
                "启动时迁移到铁血战士胖虎标准"
            };
            rules.set_active_score_standard(&target_id, Some(note))?;
        }
        // 处理已有数据库中仍指向内置标准的情况；新标准导入后也可由页面/计算入口即时触发同一迁移。
        promote_preferred_score_standard_if_needed(rules.as_ref())?;

        let active_profile = Arc::new(Mutex::new(None::<String>));
        let data_directory = database_path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));

        Ok(Self {
            db: db.clone(),
            active_profile: active_profile.clone(),
            current_data: CurrentDataStore::new(data_directory.clone(), active_profile),
            character_archives: CharacterArchiveStore::new(
                data_directory.join("character-archives.json"),
            ),
            runner,
            profiles: Arc::new(
                crate::infrastructure::repositories::SqliteProfileRepository::new(db.clone()),
            ),
            raw_objects: Arc::new(
                crate::infrastructure::repositories::SqliteRawObjectRepository::new(db.clone()),
            ),
            events: Arc::new(
                crate::infrastructure::repositories::SqliteAcquisitionEventRepository::new(
                    db.clone(),
                ),
            ),
            snapshots: Arc::new(
                crate::infrastructure::repositories::SqliteSnapshotRepository::new(db.clone()),
            ),
            radar: Arc::new(
                crate::infrastructure::repositories::SqliteSoulRadarRepository::new(db.clone()),
            ),
            head_tail: Arc::new(
                crate::infrastructure::repositories::SqliteHeadTailRepository::new(db.clone()),
            ),
            owned_shikigami:
                crate::infrastructure::repositories::SqliteOwnedShikigamiRepository::new(db.clone()),
            history: Arc::new(
                crate::infrastructure::history_repository::SqliteHistoryAnalysisRepository::new(
                    db.clone(),
                ),
            ),
            imports: Arc::new(
                crate::infrastructure::repositories::SqliteSnapshotRepository::new(db.clone()),
            ),
            actions: Arc::new(
                crate::infrastructure::repositories::SqliteActionRepository::new(db.clone()),
            ),
            catalog: Arc::new(
                crate::infrastructure::repositories::SqliteCatalogRepository::new(db.clone()),
            ),
            commonness: Arc::new(
                crate::infrastructure::repositories::SqliteCommonnessRepository::new(db.clone()),
            ),
            rules,
            packages: Arc::new(
                crate::infrastructure::repositories::SqlitePackageRepository::new(db.clone()),
            ),
            backup_records: Arc::new(
                crate::infrastructure::repositories::SqliteBackupRecordRepository::new(db.clone()),
            ),
            raw_object_store,
            backup_manager,
            mumu_diagnostics_directory,
        })
    }

    /// 保存 MuMu 失败批次的原始 JSON，并返回用户可以直接打开的本地路径。
    /// 文件名经过白名单过滤，防止外部批次名穿越到诊断目录之外；保存失败不会覆盖原始导入错误。
    pub fn save_mumu_failed_payload(
        &self,
        file_name: &str,
        payload: &[u8],
    ) -> Result<PathBuf, AppError> {
        let safe_file_name = sanitize_mumu_diagnostic_file_name(file_name);
        std::fs::create_dir_all(&self.mumu_diagnostics_directory)
            .map_err(|error| AppError::io("创建 MuMu 诊断目录", &error))?;
        let path = self.mumu_diagnostics_directory.join(safe_file_name);
        std::fs::write(&path, payload)
            .map_err(|error| AppError::io("保存 MuMu 失败批次", &error))?;
        Ok(path)
    }

    /// 返回当前 ISO 8601 时间戳字符串。
    pub fn now_iso() -> String {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }
}

/// 当前用户要求优先使用的评分标准名称关键词；标题带后缀时仍可通过 contains 匹配。
pub(crate) const PREFERRED_SCORE_STANDARD_KEYWORD: &str = "铁血战士胖虎";

/// 判断规则版本是否是用户指定的铁血战士胖虎评分标准。
/// 标准标题或作者包含该关键词即可匹配，兼容“铁血战士胖虎的标准”等现有命名。
pub(crate) fn is_preferred_score_standard(version: &crate::domain::RuleVersionRecord) -> bool {
    version.origin != "builtin"
        && (version.title.contains(PREFERRED_SCORE_STANDARD_KEYWORD)
            || version.author.contains(PREFERRED_SCORE_STANDARD_KEYWORD))
}

/// 当全局仍指向内置标准或旧版铁血战士胖虎标准时，切换到最近保存的同名新版。
/// 用户明确选择的其他评分标准不属于自动升级范围，必须保持原有全局指针。
pub(crate) fn promote_preferred_score_standard_if_needed(
    rules: &dyn RuleRepository,
) -> Result<Option<String>, AppError> {
    let active_id = rules.active_score_standard_id()?;
    let Some(latest_preferred_id) = latest_preferred_score_standard_id(rules)? else {
        return Ok(active_id);
    };

    // 指针缺失或引用已不存在的记录时也要恢复到新版，避免计算入口静默回退内置正文。
    let active_version = if let Some(active_id) = active_id.as_deref() {
        rules.get_version(active_id)?
    } else {
        None
    };
    let should_promote = match active_version.as_ref() {
        None => true,
        Some(version) => {
            version.origin == "builtin"
                || (is_preferred_score_standard(version) && version.id != latest_preferred_id)
        }
    };
    if !should_promote {
        return Ok(active_id);
    }
    rules.set_active_score_standard(
        &latest_preferred_id,
        Some("迁移到最新版铁血战士胖虎标准"),
    )?;
    Ok(Some(latest_preferred_id))
}

/// 从最近保存的用户标准中找出铁血战士胖虎标准，供启动迁移和规则库回退共用。
fn latest_preferred_score_standard_id(
    rules: &dyn RuleRepository,
) -> Result<Option<String>, AppError> {
    Ok(rules
        .list_versions()?
        .into_iter()
        .find(|version| is_preferred_score_standard(version))
        .map(|version| version.id))
}

/// 仅保留诊断文件名允许使用的字符，确保批次文件始终落在固定目录内。
fn sanitize_mumu_diagnostic_file_name(file_name: &str) -> String {
    let sanitized = file_name
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "mumu-failed-batch.json".to_owned()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::AppServices;
    use crate::application::rule_use_cases::RuleUseCase;
    use crate::domain::{load_default_preset, rule_sharing::export_rule_file};
    use std::sync::Arc;

    /// 回归约束：确认导入后关闭并重新初始化服务，规则版本仍应从同一数据库读出。
    #[test]
    fn 用户导入规则重启后仍然存在() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("读取测试时间")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("yys-rule-persistence-{suffix}"));
        let database_path = root.join("current-analysis.sqlite3");
        let snapshot_root = root.join("current-data/raw");
        let quarantine_directory = root.join("current-data/quarantine");
        let backups_directory = root.join("backups");
        let temp_directory = root.join("temp");

        let preset = load_default_preset().expect("默认规则预设");
        let payload = export_rule_file(&preset).expect("导出规则文件");
        let first_services = Arc::new(
            AppServices::initialize(
                database_path.clone(),
                snapshot_root.clone(),
                quarantine_directory.clone(),
                backups_directory.clone(),
                temp_directory.clone(),
            )
            .expect("首次初始化服务"),
        );
        let initial_versions = RuleUseCase::new(first_services.clone())
            .list_score_standards()
            .expect("读取初始化后的评分标准");
        assert!(
            initial_versions
                .iter()
                .any(|version| version.origin == "builtin" && version.enabled),
            "数据库初始化后应已有系统内置评分标准"
        );
        assert_eq!(
            initial_versions
                .iter()
                .filter(|version| version.enabled)
                .count(),
            1,
            "全局评分标准同时只能启用一条"
        );
        let imported = RuleUseCase::new(first_services.clone())
            .import("file", payload.as_bytes())
            .expect("确认导入规则");
        let imported_id = imported.id.clone();
        let saved = RuleUseCase::new(first_services.clone())
            .save_editable("file", payload.as_bytes(), Some(&imported_id))
            .expect("保存已选评分标准");
        assert_eq!(
            saved.id, imported_id,
            "保存已选评分标准应更新原记录而不是新增记录"
        );
        let saved_versions = RuleUseCase::new(first_services.clone())
            .list_score_standards()
            .expect("读取保存后的评分标准");
        assert_eq!(
            saved_versions
                .iter()
                .filter(|version| version.id == imported_id)
                .count(),
            1,
            "保存后原标准只能保留一条记录"
        );
        // 模拟旧版保存产生的子记录；删除导入记录时不应被历史父子引用卡住。
        let copied = RuleUseCase::new(first_services.clone())
            .copy(&imported_id)
            .expect("创建导入标准副本");
        let copied_id = copied.id.clone();
        RuleUseCase::new(first_services.clone())
            .set_active_score_standard(&imported_id)
            .expect("切换全局评分标准");
        drop(first_services);

        // 模拟应用退出后再次启动，必须复用同一个数据库文件而不是依赖内存状态。
        let second_services = Arc::new(
            AppServices::initialize(
                database_path,
                snapshot_root,
                quarantine_directory,
                backups_directory,
                temp_directory,
            )
            .expect("再次初始化服务"),
        );
        let versions = RuleUseCase::new(second_services.clone())
            .list_score_standards()
            .expect("读取规则版本");
        assert!(
            versions
                .iter()
                .any(|version| version.id == imported_id && version.enabled),
            "重启后应能读取已确认导入且已启用的评分标准"
        );
        assert_eq!(
            versions.iter().filter(|version| version.enabled).count(),
            1,
            "重启后全局评分标准仍只能启用一条"
        );

        // 删除当前用户标准后，仍应从剩余用户标准中选择当前标准，避免页面回到旧默认项。
        let second_use_case = RuleUseCase::new(second_services);
        second_use_case
            .delete_score_standard(&imported_id)
            .expect("删除用户评分标准");
        let after_delete = second_use_case
            .list_score_standards()
            .expect("读取删除后的评分标准");
        assert!(!after_delete.iter().any(|version| version.id == imported_id));
        assert!(
            after_delete.iter().any(|version| version.id == copied_id),
            "删除父标准不应误删用户副本"
        );
        assert_eq!(
            after_delete
                .iter()
                .filter(|version| version.enabled)
                .count(),
            1,
            "删除当前标准后仍应保留一条启用标准"
        );
        assert!(
            after_delete
                .iter()
                .any(|version| version.id == copied_id && version.enabled),
            "删除当前用户标准后应优先启用剩余用户标准"
        );
        assert!(
            !after_delete
                .iter()
                .any(|version| version.origin == "builtin"),
            "存在用户标准时不应再展示系统默认评分标准"
        );
        let reimported = second_use_case
            .import("file", payload.as_bytes())
            .expect("删除后重新导入同一离线规则");
        assert_eq!(
            reimported.id, imported_id,
            "删除后的离线规则应允许使用同一身份重新导入"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    /// 回归约束：已有“铁血战士胖虎”标准时，重启不能继续把系统默认标准当作全局评分标准。
    #[test]
    fn 初始化后优先使用铁血战士胖虎评分标准() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("读取测试时间")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("yys-preferred-score-standard-{suffix}"));
        let database_path = root.join("current-analysis.sqlite3");
        let snapshot_root = root.join("current-data/raw");
        let quarantine_directory = root.join("current-data/quarantine");
        let backups_directory = root.join("backups");
        let temp_directory = root.join("temp");

        let mut preset = load_default_preset().expect("默认规则预设").preset;
        // 测试使用与用户导入标准相同的标题，验证旧数据库中的默认指针会被迁移。
        preset.id = "community.tiexue.panghu".to_owned();
        preset.version = "1.0.0".to_owned();
        preset.title = "铁血战士胖虎".to_owned();
        preset.author = "铁血战士胖虎".to_owned();
        let payload =
            export_rule_file(&preset.validate().expect("校验用户标准")).expect("导出用户标准");

        let first_services = Arc::new(
            AppServices::initialize(
                database_path.clone(),
                snapshot_root.clone(),
                quarantine_directory.clone(),
                backups_directory.clone(),
                temp_directory.clone(),
            )
            .expect("首次初始化服务"),
        );
        RuleUseCase::new(first_services.clone())
            .import("file", payload.as_bytes())
            .expect("导入铁血战士胖虎标准");
        // 旧版本启动时会保留内置标准的 active 指针；这里故意不手动切换以复现问题。
        drop(first_services);

        let restarted_services = Arc::new(
            AppServices::initialize(
                database_path,
                snapshot_root,
                quarantine_directory,
                backups_directory,
                temp_directory,
            )
            .expect("再次初始化服务"),
        );
        let versions = RuleUseCase::new(restarted_services)
            .list_score_standards()
            .expect("读取重启后的评分标准");
        let enabled = versions
            .iter()
            .find(|version| version.enabled)
            .expect("重启后应存在启用的评分标准");
        assert_eq!(enabled.title, "铁血战士胖虎");

        let _ = std::fs::remove_dir_all(root);
    }
}
