//! 备份管理：事务备份创建、数据一致性校验与恢复准备。
//!
//! 备份创建使用 SQLite 在线备份 API，保证一致性。校验包含完整性检查、
//! 外键检查和 schema 版本前缀验证。恢复采用文件级原子替换。

use crate::application::error::AppError;
use crate::infrastructure::database::migrations::MigrationRunner;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// 备份校验结果。
#[derive(Clone, Debug)]
pub struct BackupValidation {
    pub schema_version: u32,
    pub file_count: u32,
    pub total_size: u64,
    pub checksum: Option<String>,
    pub passed: bool,
}

/// 备份管理器，负责创建、校验和恢复数据库备份。
pub struct BackupManager {
    database_path: PathBuf,
    backups_directory: PathBuf,
    temp_directory: PathBuf,
    // 原始对象不可变，备份时复制其目录即可与 SQLite 事实层共同恢复。
    snapshot_root: Option<PathBuf>,
    // 数据包本身由签名更新流程管理；备份保存其活动文件清单，恢复时用于校验版本来源。
    packages_directory: Option<PathBuf>,
}

impl BackupManager {
    /// 创建备份管理器。
    pub fn new(
        database_path: PathBuf,
        backups_directory: PathBuf,
        temp_directory: PathBuf,
    ) -> Self {
        Self {
            database_path,
            backups_directory,
            temp_directory,
            snapshot_root: None,
            packages_directory: None,
        }
    }

    /// 创建同时覆盖 SQLite 和原始快照目录的备份管理器。
    pub fn with_snapshot_root(
        database_path: PathBuf,
        backups_directory: PathBuf,
        temp_directory: PathBuf,
        snapshot_root: PathBuf,
    ) -> Self {
        Self {
            database_path,
            backups_directory,
            temp_directory,
            snapshot_root: Some(snapshot_root),
            packages_directory: None,
        }
    }

    /// 创建同时记录原始对象和活动数据包清单的备份管理器。
    pub fn with_snapshot_root_and_packages(
        database_path: PathBuf,
        backups_directory: PathBuf,
        temp_directory: PathBuf,
        snapshot_root: PathBuf,
        packages_directory: PathBuf,
    ) -> Self {
        Self {
            database_path,
            backups_directory,
            temp_directory,
            snapshot_root: Some(snapshot_root),
            packages_directory: Some(packages_directory),
        }
    }

    /// 创建事务一致性备份，返回备份文件路径。
    /// 使用 SQLite 在线备份 API 逐页复制，允许在备份期间有并发读取。
    pub fn create_backup(&self) -> Result<PathBuf, AppError> {
        std::fs::create_dir_all(&self.backups_directory)
            .map_err(|error| AppError::io("创建备份目录", &error))?;
        std::fs::create_dir_all(&self.temp_directory)
            .map_err(|error| AppError::io("创建备份临时目录", &error))?;

        // 时间戳用于可读性，UUID 防止同一毫秒创建多个备份时覆盖彼此。
        let backup_stem = format!(
            "yuhun-backup-{}-{}",
            format_iso_now_for_filename(),
            uuid::Uuid::new_v4()
        );
        let backup_path = self
            .backups_directory
            .join(format!("{backup_stem}.sqlite3"));
        let staging_database = self
            .temp_directory
            .join(format!("{backup_stem}.sqlite3.staging"));
        let staging_snapshots = self
            .temp_directory
            .join(format!("{backup_stem}.snapshots.staging"));
        let staging_package_manifest = self
            .temp_directory
            .join(format!("{backup_stem}.packages.json.staging"));
        let raw_backup_path = Self::raw_backup_path(&backup_path);
        let package_manifest_path = Self::package_manifest_path(&backup_path);

        let source =
            Connection::open_with_flags(&self.database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|error| AppError::backup_failed("打开源数据库", &error.to_string()))?;

        let mut dest = Connection::open(&staging_database)
            .map_err(|error| AppError::backup_failed("创建备份文件", &error.to_string()))?;

        let backup = rusqlite::backup::Backup::new(&source, &mut dest)
            .map_err(|error| AppError::backup_failed("初始化备份", &error.to_string()))?;

        if let Err(error) =
            backup.run_to_completion(4096, std::time::Duration::from_millis(250), None)
        {
            let _ = std::fs::remove_file(&staging_database);
            return Err(AppError::backup_failed("执行备份", &error.to_string()));
        }
        // 在线备份对象持有源/目标连接借用，必须先释放它再关闭连接。
        drop(backup);
        drop(dest);
        drop(source);

        // SQLite 在线备份保证事实层一致；原始对象是不可变文件，单独复制其当前内容即可。
        if let Some(snapshot_root) = &self.snapshot_root {
            if let Err(error) = validate_snapshot_objects(snapshot_root) {
                let _ = std::fs::remove_file(&staging_database);
                return Err(error);
            }
            if let Err(error) = validate_database_raw_references(&staging_database, snapshot_root) {
                let _ = std::fs::remove_file(&staging_database);
                return Err(error);
            }
            if let Err(error) = copy_directory(snapshot_root, &staging_snapshots) {
                let _ = std::fs::remove_file(&staging_database);
                let _ = std::fs::remove_dir_all(&staging_snapshots);
                return Err(error);
            }
        }

        // 活动数据包可能由更新模块单独管理；备份至少保存其版本、路径和哈希清单。
        if let Some(packages_directory) = &self.packages_directory {
            let manifest = match build_package_manifest(packages_directory) {
                Ok(manifest) => manifest,
                Err(error) => {
                    let _ = std::fs::remove_file(&staging_database);
                    let _ = std::fs::remove_dir_all(&staging_snapshots);
                    return Err(error);
                }
            };
            if let Err(error) = std::fs::write(&staging_package_manifest, manifest) {
                let _ = std::fs::remove_file(&staging_database);
                let _ = std::fs::remove_dir_all(&staging_snapshots);
                return Err(AppError::io("写入活动数据包清单", &error));
            }
        }

        if let Err(error) = std::fs::rename(&staging_database, &backup_path) {
            let _ = std::fs::remove_file(&staging_database);
            let _ = std::fs::remove_dir_all(&staging_snapshots);
            let _ = std::fs::remove_file(&staging_package_manifest);
            return Err(AppError::io("发布数据库备份", &error));
        }
        if self.snapshot_root.is_some() {
            if let Err(error) = std::fs::rename(&staging_snapshots, &raw_backup_path) {
                let _ = std::fs::remove_file(&backup_path);
                let _ = std::fs::remove_dir_all(&staging_snapshots);
                let _ = std::fs::remove_file(&staging_package_manifest);
                return Err(AppError::io("发布原始对象备份", &error));
            }
        }
        if self.packages_directory.is_some() {
            if let Err(error) = std::fs::rename(&staging_package_manifest, &package_manifest_path) {
                let _ = std::fs::remove_file(&backup_path);
                let _ = std::fs::remove_dir_all(&raw_backup_path);
                return Err(AppError::io("发布活动数据包清单", &error));
            }
        }

        tracing::info!(
            backup_path = %backup_path.display(),
            "数据库备份已创建"
        );

        Ok(backup_path)
    }

    /// 校验备份文件的完整性和一致性。
    pub fn validate(
        &self,
        backup_path: &Path,
        runner: &MigrationRunner,
    ) -> Result<BackupValidation, AppError> {
        let conn = Connection::open_with_flags(backup_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| AppError::backup_failed("打开备份文件", &error.to_string()))?;

        // 完整性检查
        let integrity: String = conn
            .pragma_query_value(None, "integrity_check", |row| row.get(0))
            .map_err(|error| AppError::backup_failed("完整性检查", &error.to_string()))?;
        if integrity != "ok" {
            return Err(AppError::backup_failed(
                "完整性检查",
                &format!("备份文件完整性检查失败：{integrity}"),
            ));
        }

        // 外键检查
        let fk_errors: Vec<String> = {
            let mut stmt = conn
                .prepare("PRAGMA foreign_key_check")
                .map_err(|error| AppError::backup_failed("外键检查准备", &error.to_string()))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(format!(
                        "表={} 行={} 外键={}",
                        row.get::<_, String>(0).unwrap_or_default(),
                        row.get::<_, i64>(1).unwrap_or(-1),
                        row.get::<_, String>(2).unwrap_or_default(),
                    ))
                })
                .map_err(|error| AppError::backup_failed("外键检查", &error.to_string()))?;
            let mut errors = Vec::new();
            for row in rows {
                if let Ok(msg) = row {
                    errors.push(msg);
                }
            }
            errors
        };
        if !fk_errors.is_empty() {
            return Err(AppError::backup_failed(
                "外键检查",
                &format!("备份文件存在外键违反：{}条", fk_errors.len()),
            ));
        }

        // schema 版本前缀检查
        let schema_version: u32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migration",
                [],
                |row| row.get(0),
            )
            .map_err(|error| AppError::backup_failed("查询 schema 版本", &error.to_string()))?;

        if schema_version > runner.latest_version() {
            return Err(AppError::backup_failed(
                "schema 版本检查",
                &format!(
                    "备份文件 schema 版本 {schema_version} 高于当前最新版本 {}",
                    runner.latest_version()
                ),
            ));
        }

        let raw_backup_path = Self::raw_backup_path(backup_path);
        if self.snapshot_root.is_some() && !raw_backup_path.is_dir() {
            return Err(AppError::backup_failed(
                "原始对象检查",
                "备份缺少与 SQLite 配套的原始对象目录",
            ));
        }
        if self.snapshot_root.is_some() {
            validate_snapshot_objects(&raw_backup_path)?;
            validate_database_raw_references(backup_path, &raw_backup_path)?;
        }
        if self.packages_directory.is_some() {
            validate_package_manifest(&Self::package_manifest_path(backup_path))?;
        }

        // 清单同时覆盖数据库和原始对象文件，恢复或人工复制后可以重新验证内容。
        let (file_count, total_size, checksum) = build_manifest(
            backup_path,
            self.snapshot_root
                .as_ref()
                .map(|_| raw_backup_path.as_path()),
            self.packages_directory
                .as_ref()
                .map(|_| Self::package_manifest_path(backup_path)),
        )?;

        Ok(BackupValidation {
            schema_version,
            file_count,
            total_size,
            checksum: Some(checksum),
            passed: true,
        })
    }

    /// 恢复备份：校验后原子替换数据库文件，并返回是否成功。
    /// 调用方需确保数据库连接已关闭，并在恢复后重新打开。
    pub fn restore(&self, backup_path: &Path, runner: &MigrationRunner) -> Result<(), AppError> {
        if !backup_path.exists() {
            return Err(AppError::backup_failed("恢复", "备份文件不存在"));
        }

        // 校验备份
        let validation = self.validate(backup_path, runner)?;
        if !validation.passed {
            return Err(AppError::backup_failed("恢复", "备份校验未通过"));
        }

        // 复制到临时目录，确保恢复期间源文件不被占用；目录名带 UUID 避免残留互相覆盖。
        let restore_id = uuid::Uuid::new_v4().to_string();
        let staging = self
            .temp_directory
            .join(format!("restore-{restore_id}.sqlite3"));
        let staging_snapshots = self
            .temp_directory
            .join(format!("restore-{restore_id}.snapshots"));
        std::fs::create_dir_all(&self.temp_directory)
            .map_err(|error| AppError::io("创建临时目录", &error))?;

        std::fs::copy(backup_path, &staging)
            .map_err(|error| AppError::io("复制备份到临时位置", &error))?;
        let raw_backup_path = Self::raw_backup_path(backup_path);
        if self.snapshot_root.is_some() {
            if let Err(error) = copy_directory(&raw_backup_path, &staging_snapshots) {
                let _ = std::fs::remove_file(&staging);
                let _ = std::fs::remove_dir_all(&staging_snapshots);
                return Err(error);
            }
        }

        // 切换当前数据库前，必须在临时副本上完成迁移、完整性和原始对象引用校验。
        // 任一步失败都只清理暂存文件，不触碰当前正在使用的数据库和对象目录。
        if let Err(error) = validate_restore_staging(
            &staging,
            self.snapshot_root
                .as_ref()
                .map(|_| staging_snapshots.as_path()),
            runner,
        ) {
            let _ = std::fs::remove_file(&staging);
            let _ = std::fs::remove_dir_all(&staging_snapshots);
            return Err(error);
        }

        // 原子替换：先将当前数据库移为备份，再将暂存文件移入
        let quarantine = self
            .backups_directory
            .join(format!("pre-restore-{restore_id}.sqlite3"));
        let snapshot_quarantine = self
            .backups_directory
            .join(format!("pre-restore-{restore_id}.snapshots"));

        // 数据库连接由调用方关闭后，清理旧 WAL/SHM，避免它们被新数据库误读。
        remove_sqlite_sidecars(&self.database_path)?;

        if self.database_path.exists() {
            std::fs::rename(&self.database_path, &quarantine)
                .map_err(|error| AppError::io("备份当前数据库", &error))?;
        }

        if let Some(snapshot_root) = &self.snapshot_root {
            if snapshot_root.exists() {
                std::fs::rename(snapshot_root, &snapshot_quarantine).map_err(|error| {
                    // 数据库尚未替换，失败时恢复已隔离的数据库。
                    if quarantine.exists() {
                        let _ = std::fs::rename(&quarantine, &self.database_path);
                    }
                    AppError::io("备份当前原始对象目录", &error)
                })?;
            }
            if let Some(parent) = snapshot_root.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| AppError::io("创建原始对象目录", &error))?;
            }
        }

        std::fs::rename(&staging, &self.database_path).map_err(|error| {
            // 恢复失败时尝试把隔离文件移回
            if quarantine.exists() {
                let _ = std::fs::rename(&quarantine, &self.database_path);
            }
            if let Some(snapshot_root) = &self.snapshot_root {
                if snapshot_quarantine.exists() {
                    let _ = std::fs::rename(&snapshot_quarantine, snapshot_root);
                }
            }
            let _ = std::fs::remove_dir_all(&staging_snapshots);
            AppError::io("替换数据库文件", &error)
        })?;

        if let Some(snapshot_root) = &self.snapshot_root {
            if let Err(error) = std::fs::rename(&staging_snapshots, snapshot_root) {
                // 数据库已经换入，但原始对象换入失败；尽力恢复两部分，避免留下半恢复状态。
                let _ = std::fs::remove_file(&self.database_path);
                if quarantine.exists() {
                    let _ = std::fs::rename(&quarantine, &self.database_path);
                }
                if snapshot_quarantine.exists() {
                    let _ = std::fs::rename(&snapshot_quarantine, snapshot_root);
                }
                return Err(AppError::io("替换原始对象目录", &error));
            }
        }

        tracing::info!(
            backup_path = %backup_path.display(),
            quarantine = %quarantine.display(),
            "数据库已从备份恢复"
        );

        Ok(())
    }

    /// 从备份数据库路径推导配套原始对象目录路径。
    fn raw_backup_path(backup_path: &Path) -> PathBuf {
        backup_path.with_extension("snapshots")
    }

    /// 从备份数据库路径推导活动数据包清单路径。
    fn package_manifest_path(backup_path: &Path) -> PathBuf {
        backup_path.with_extension("packages.json")
    }

    /// 删除一组由本管理器创建的备份文件；路径必须是备份目录直属的标准数据库文件。
    pub fn delete_backup(&self, backup_path: &Path) -> Result<(), AppError> {
        let is_managed_path = backup_path.parent() == Some(self.backups_directory.as_path())
            && backup_path
                .extension()
                .is_some_and(|value| value == "sqlite3")
            && backup_path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.starts_with("yuhun-backup-"));
        if !is_managed_path {
            return Err(AppError::backup_failed(
                "删除旧备份",
                "备份路径不属于应用备份目录",
            ));
        }

        // 辅助目录和 SQLite 边车文件必须与主备份一并删除，避免保留策略仍持续占用磁盘。
        let raw_backup_path = Self::raw_backup_path(backup_path);
        if raw_backup_path.exists() {
            std::fs::remove_dir_all(&raw_backup_path)
                .map_err(|error| AppError::io("删除备份原始对象", &error))?;
        }
        for path in [
            Self::package_manifest_path(backup_path),
            sqlite_sidecar_path(backup_path, "-wal"),
            sqlite_sidecar_path(backup_path, "-shm"),
            backup_path.to_path_buf(),
        ] {
            if path.exists() {
                std::fs::remove_file(&path)
                    .map_err(|error| AppError::io("删除旧备份文件", &error))?;
            }
        }
        Ok(())
    }

    /// 返回备份文件列表（按修改时间倒序）。
    pub fn list_backups(&self) -> Result<Vec<PathBuf>, AppError> {
        if !self.backups_directory.exists() {
            return Ok(Vec::new());
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&self.backups_directory)
            .map_err(|error| AppError::io("读取备份目录", &error))?
            .filter_map(|entry| entry.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |ext| ext == "sqlite3"))
            .filter(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .map_or(false, |s| s.starts_with("yuhun-backup-"))
            })
            .collect();

        // 按修改时间倒序
        entries.sort_by(|a, b| {
            b.metadata()
                .and_then(|m| m.modified())
                .ok()
                .cmp(&a.metadata().and_then(|m| m.modified()).ok())
        });
        Ok(entries)
    }
}

/// 递归复制目录内容；目录不存在时创建空目录，表示当前没有原始对象。
fn copy_directory(source: &Path, destination: &Path) -> Result<(), AppError> {
    std::fs::create_dir_all(destination)
        .map_err(|error| AppError::io("创建目录备份暂存区", &error))?;
    if !source.exists() {
        return Ok(());
    }

    for entry in
        std::fs::read_dir(source).map_err(|error| AppError::io("读取原始对象目录", &error))?
    {
        let entry = entry.map_err(|error| AppError::io("读取原始对象目录项", &error))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else {
            std::fs::copy(&source_path, &destination_path)
                .map_err(|error| AppError::io("复制原始对象文件", &error))?;
        }
    }
    Ok(())
}

/// 收集目录内全部普通文件，排序后计算稳定清单哈希。
fn build_manifest(
    database_path: &Path,
    snapshots_path: Option<&Path>,
    package_manifest_path: Option<PathBuf>,
) -> Result<(u32, u64, String), AppError> {
    let mut files = vec![("database.sqlite3".to_owned(), database_path.to_path_buf())];
    if let Some(root) = snapshots_path {
        let mut nested = Vec::new();
        collect_files(root, root, &mut nested)?;
        files.extend(
            nested
                .into_iter()
                .map(|(relative, path)| (format!("snapshots/{relative}"), path)),
        );
    }
    if let Some(path) = package_manifest_path {
        files.push(("packages.json".to_owned(), path));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut hasher = Sha256::new();
    let mut total_size = 0u64;
    for (label, path) in &files {
        let bytes =
            std::fs::read(path).map_err(|error| AppError::io("读取备份清单文件", &error))?;
        total_size = total_size
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| AppError::backup_failed("计算备份大小", "文件总大小溢出"))?;
        hasher.update(label.as_bytes());
        hasher.update([0]);
        hasher.update(&bytes);
        hasher.update([0]);
    }

    let file_count = u32::try_from(files.len())
        .map_err(|_| AppError::backup_failed("计算备份清单", "文件数量超过支持上限"))?;
    Ok((file_count, total_size, hex::encode(hasher.finalize())))
}

/// 生成活动数据包清单；只保存文件元数据，不把可执行包复制进用户备份。
fn build_package_manifest(root: &Path) -> Result<String, AppError> {
    let mut files = Vec::new();
    if root.exists() {
        let mut entries = Vec::new();
        collect_files(root, root, &mut entries)?;
        for (relative, path) in entries {
            let bytes =
                std::fs::read(&path).map_err(|error| AppError::io("读取活动数据包", &error))?;
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            files.push(serde_json::json!({
                "path": relative,
                "sha256": hex::encode(hasher.finalize()),
                "size": bytes.len(),
            }));
        }
    }
    serde_json::to_string_pretty(&serde_json::json!({
        "format": "yys-analysis-package-manifest",
        "version": 1,
        "files": files,
    }))
    .map_err(|error| AppError::internal(format!("序列化活动数据包清单失败：{error}")))
}

/// 校验清单结构和每个文件的路径/哈希字段，防止恢复时接受伪造的活动包元数据。
fn validate_package_manifest(path: &Path) -> Result<(), AppError> {
    let bytes = std::fs::read(path).map_err(|error| AppError::io("读取活动数据包清单", &error))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::backup_failed("解析活动数据包清单", &error.to_string()))?;
    if value.get("format").and_then(|value| value.as_str()) != Some("yys-analysis-package-manifest")
        || value.get("version").and_then(|value| value.as_u64()) != Some(1)
    {
        return Err(AppError::backup_failed(
            "校验活动数据包清单",
            "清单格式或版本不受支持",
        ));
    }
    let Some(files) = value.get("files").and_then(|value| value.as_array()) else {
        return Err(AppError::backup_failed(
            "校验活动数据包清单",
            "清单缺少 files 数组",
        ));
    };
    for file in files {
        let path_value = file
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let hash_value = file
            .get("sha256")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if path_value.is_empty()
            || path_value.starts_with('/')
            || path_value.contains("..")
            || hash_value.len() != 64
            || !hash_value.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(AppError::backup_failed(
                "校验活动数据包清单",
                "清单包含非法路径或 SHA-256",
            ));
        }
    }
    Ok(())
}

/// 校验原始对象目录中每个 zstd 文件的内容哈希与文件名一致。
/// 备份校验不能只验证 SQLite 文件自身，否则可能把损坏的原始载荷一起恢复回来。
fn validate_snapshot_objects(root: &Path) -> Result<(), AppError> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    for (_, path) in files {
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(expected_hash) = file_name.strip_suffix(".json.zst") else {
            continue;
        };
        if expected_hash.len() != 64 || !expected_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(AppError::backup_failed(
                "原始对象检查",
                &format!("对象文件名不是有效 SHA-256：{file_name}"),
            ));
        }
        let compressed =
            std::fs::read(&path).map_err(|error| AppError::io("读取原始对象备份", &error))?;
        let raw = zstd::stream::decode_all(compressed.as_slice()).map_err(|error| {
            AppError::backup_failed(
                "原始对象检查",
                &format!("zstd 解压失败：{file_name}：{error}"),
            )
        })?;
        let mut hasher = Sha256::new();
        hasher.update(raw);
        let actual_hash = hex::encode(hasher.finalize());
        if actual_hash != expected_hash {
            return Err(AppError::checksum_mismatch(
                &path.display().to_string(),
                expected_hash,
                &actual_hash,
            ));
        }
    }
    Ok(())
}

/// 校验 SQLite 中的原始对象引用在备份目录中均有对应文件。
fn validate_database_raw_references(
    database_path: &Path,
    snapshots_path: &Path,
) -> Result<(), AppError> {
    let conn = Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| AppError::backup_failed("读取原始对象引用", &error.to_string()))?;
    let mut stmt = conn
        .prepare("SELECT sha256, relative_path FROM raw_object")
        .map_err(|error| AppError::backup_failed("读取原始对象引用", &error.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| AppError::backup_failed("读取原始对象引用", &error.to_string()))?;
    for row in rows {
        let (sha256, relative_path) =
            row.map_err(|error| AppError::backup_failed("解析原始对象引用", &error.to_string()))?;
        let expected_path = expected_raw_relative_path(&sha256)?;
        if relative_path != expected_path {
            return Err(AppError::backup_failed(
                "原始对象引用",
                &format!("对象 {sha256} 的相对路径不符合内容寻址约定"),
            ));
        }
        if !snapshots_path.join(&relative_path).is_file() {
            return Err(AppError::backup_failed(
                "原始对象引用",
                &format!("对象文件缺失：{relative_path}"),
            ));
        }
    }
    Ok(())
}

/// 由已验证的 SHA-256 生成对象相对路径，供备份校验复用同一规则。
fn expected_raw_relative_path(sha256: &str) -> Result<String, AppError> {
    if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::backup_failed(
            "原始对象引用",
            "数据库中的 SHA-256 格式无效",
        ));
    }
    Ok(format!(
        "{}/{}/{}.json.zst",
        &sha256[0..2],
        &sha256[2..4],
        sha256
    ))
}

/// 在恢复暂存区执行迁移并验证 SQLite/原始对象一致性，保证原子切换前已具备可启动状态。
fn validate_restore_staging(
    database_path: &Path,
    snapshots_path: Option<&Path>,
    runner: &MigrationRunner,
) -> Result<(), AppError> {
    let mut connection = Connection::open(database_path)
        .map_err(|error| AppError::backup_failed("打开恢复暂存数据库", &error.to_string()))?;
    runner.run(&mut connection)?;
    let integrity: String = connection
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .map_err(|error| AppError::backup_failed("恢复暂存完整性检查", &error.to_string()))?;
    if integrity != "ok" {
        return Err(AppError::backup_failed(
            "恢复暂存完整性检查",
            &format!("完整性检查失败：{integrity}"),
        ));
    }
    // 显式延长 statement 生命周期，确保 query_map 迭代期间底层查询仍然有效。
    let foreign_key_error_count: u32 = {
        let mut statement = connection
            .prepare("PRAGMA foreign_key_check")
            .map_err(|error| AppError::backup_failed("恢复暂存外键检查准备", &error.to_string()))?;
        let rows = statement
            .query_map([], |_| Ok(()))
            .map_err(|error| AppError::backup_failed("恢复暂存外键检查", &error.to_string()))?;
        rows.count() as u32
    };
    if foreign_key_error_count > 0 {
        return Err(AppError::backup_failed(
            "恢复暂存外键检查",
            &format!("发现 {foreign_key_error_count} 条外键错误"),
        ));
    }
    drop(connection);
    // 迁移可能创建 WAL/SHM 旁路文件；暂存区发布前将它们合并/清理，避免污染新数据库。
    remove_sqlite_sidecars(database_path)?;
    if let Some(snapshots_path) = snapshots_path {
        validate_snapshot_objects(snapshots_path)?;
        validate_database_raw_references(database_path, snapshots_path)?;
    }
    Ok(())
}

/// 递归收集文件，并以相对路径参与备份清单，避免目录遍历顺序影响哈希。
fn collect_files(
    root: &Path,
    current: &Path,
    output: &mut Vec<(String, PathBuf)>,
) -> Result<(), AppError> {
    if !current.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(current).map_err(|error| AppError::io("遍历备份目录", &error))?
    {
        let entry = entry.map_err(|error| AppError::io("读取备份目录项", &error))?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, output)?;
        } else {
            let relative = path
                .strip_prefix(root)
                .map_err(|error| AppError::internal(format!("备份路径计算失败：{error}")))?
                .to_string_lossy()
                .replace('\\', "/");
            output.push((relative, path));
        }
    }
    Ok(())
}

/// 删除数据库旁路 WAL/SHM 文件，避免恢复后旧事务日志污染新数据库。
fn remove_sqlite_sidecars(database_path: &Path) -> Result<(), AppError> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{}", database_path.display(), suffix));
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(AppError::io("清理数据库旁路文件", &error)),
        }
    }
    Ok(())
}

/// 生成适合文件名的 ISO 8601 时间戳（不含冒号）。
fn format_iso_now_for_filename() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%3f").to_string()
}

/// 构造 SQLite 与数据库文件同名的 WAL/SHM 边车路径，兼容中文及非 UTF-8 文件名。
fn sqlite_sidecar_path(database_path: &Path, suffix: &str) -> PathBuf {
    let mut value = database_path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::database::connection::Database;
    use crate::infrastructure::database::migrations::MigrationRunner;
    use crate::infrastructure::raw_object_store::RawObjectStore;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("yys-analysis-test-backup")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建测试目录");
        dir
    }

    #[test]
    fn 创建和校验备份() {
        let dir = temp_dir("create-validate");
        let db_path = dir.join("test.sqlite3");
        let db = Database::open(&db_path).expect("打开数据库");
        let runner = MigrationRunner::builtin();

        // 迁移并写入数据
        let current_version = db.write("migrate", |conn| runner.run(conn)).expect("迁移");
        // 备份测试只关心数据库已迁移到当前最新版本，避免每次新增迁移都遗漏这里的版本断言。
        assert_eq!(current_version, runner.latest_version());

        db.write("seed", |conn| {
            conn.execute("INSERT INTO app_meta (key, value, updated_at) VALUES ('k1', 'v1', '2024-01-01T00:00:00Z')", [])
                .map_err(|e| AppError::database("seed", &e))?;
            Ok(())
        })
        .expect("写入种子数据");

        // 创建备份
        let manager = BackupManager::new(db_path, dir.join("backups"), dir.join("temp"));
        let backup_path = manager.create_backup().expect("创建备份");

        // 校验备份
        let validation = manager.validate(&backup_path, &runner).expect("校验备份");
        assert!(validation.passed);
        assert!(validation.schema_version >= 10);
    }

    #[test]
    fn 备份恢复后数据一致() {
        let dir = temp_dir("restore");
        let db_path = dir.join("test.sqlite3");
        let db = Database::open(&db_path).expect("打开数据库");
        let runner = MigrationRunner::builtin();

        db.write("migrate", |conn| runner.run(conn)).expect("迁移");
        db.write("seed", |conn| {
            conn.execute("INSERT INTO app_meta (key, value, updated_at) VALUES ('k1', 'v1', '2024-01-01T00:00:00Z')", [])
                .map_err(|e| AppError::database("seed", &e))?;
            Ok(())
        })
        .expect("写入种子数据");

        let manager = BackupManager::new(db_path.clone(), dir.join("backups"), dir.join("temp"));
        let backup_path = manager.create_backup().expect("创建备份");

        // 写入额外数据（模拟恢复前的变化）
        db.write("extra", |conn| {
            conn.execute("INSERT INTO app_meta (key, value, updated_at) VALUES ('extra', 'lost', '2024-01-02T00:00:00Z')", [])
                .map_err(|e| AppError::database("extra", &e))?;
            Ok(())
        })
        .expect("写入额外数据");

        // 恢复备份
        db.close();
        manager.restore(&backup_path, &runner).expect("恢复备份");
        db.reopen().expect("重新打开数据库");
        // 恢复后需重新迁移（若备份版本不是最新，会补充迁移到最新）
        db.write("migrate-after-restore", |conn| runner.run(conn))
            .expect("恢复后迁移");

        // 验证：额外数据丢失（正确），种子数据存在
        let value: String = db
            .read("verify", |conn| {
                conn.query_row("SELECT value FROM app_meta WHERE key = 'k1'", [], |row| {
                    row.get(0)
                })
                .map_err(|e| AppError::database("verify", &e))
            })
            .expect("查询种子数据");
        assert_eq!(value, "v1", "种子数据应存在");

        let extra_exists: bool = db
            .read("check-extra", |conn| {
                let count: i64 = conn
                    .query_row(
                        "SELECT count(*) FROM app_meta WHERE key = 'extra'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|e| AppError::database("check", &e))?;
                Ok(count > 0)
            })
            .expect("检查额外数据");
        assert!(!extra_exists, "恢复后额外数据应丢失");
    }

    /// SQLite 备份必须与不可变原始对象目录一起恢复，避免事实层和原始载荷脱节。
    #[test]
    fn 备份恢复同时覆盖原始对象目录() {
        let dir = temp_dir("restore-raw-objects");
        let db_path = dir.join("test.sqlite3");
        let snapshot_root = dir.join("snapshots").join("sha256");
        let raw_before = b"raw-before";
        let object_path = snapshot_root.join(RawObjectStore::relative_path_for(
            &RawObjectStore::content_hash(raw_before),
        ));
        std::fs::create_dir_all(object_path.parent().expect("对象父目录")).expect("创建对象目录");
        let compressed_before =
            zstd::stream::encode_all(raw_before.as_slice(), 3).expect("压缩原始对象");
        std::fs::write(&object_path, compressed_before).expect("写入原始对象");

        let db = Database::open(&db_path).expect("打开数据库");
        let runner = MigrationRunner::builtin();
        db.write("migrate", |conn| runner.run(conn)).expect("迁移");

        let manager = BackupManager::with_snapshot_root(
            db_path.clone(),
            dir.join("backups"),
            dir.join("temp"),
            snapshot_root.clone(),
        );
        let backup_path = manager.create_backup().expect("创建带原始对象的备份");
        let validation = manager.validate(&backup_path, &runner).expect("校验备份");
        assert_eq!(validation.file_count, 2, "清单应包含数据库和原始对象");
        assert!(validation.checksum.is_some(), "备份应有清单哈希");

        // 模拟当前版本在备份后被修改，恢复后应回到备份内容。
        let compressed_after =
            zstd::stream::encode_all(b"raw-after".as_slice(), 3).expect("压缩修改内容");
        std::fs::write(&object_path, compressed_after).expect("修改原始对象");
        db.close();
        manager
            .restore(&backup_path, &runner)
            .expect("恢复带原始对象的备份");
        db.reopen().expect("重新打开数据库");

        let restored = zstd::stream::decode_all(
            std::fs::read(&object_path)
                .expect("读取恢复后的原始对象")
                .as_slice(),
        )
        .expect("解压恢复后的原始对象");
        assert_eq!(restored, raw_before, "原始对象应恢复到备份时内容");
    }
}
