use crate::application::error::AppError;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// 应用唯一认可的本地目录集合；数据库和快照后续只能从数据目录派生。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppPaths {
    pub data_directory: PathBuf,
    pub config_directory: PathBuf,
    pub log_directory: PathBuf,
    pub temp_directory: PathBuf,
}

impl AppPaths {
    /// 使用 Tauri 平台路径解析器构造目录，兼容中文用户名和空格路径。
    pub fn resolve(app: &AppHandle) -> Result<Self, AppError> {
        let data_directory = app
            .path()
            .app_data_dir()
            .map_err(|error| AppError::path_unavailable("data", error.to_string()))?;
        let config_directory = app
            .path()
            .app_config_dir()
            .map_err(|error| AppError::path_unavailable("config", error.to_string()))?;

        // 日志和临时产物固定归入应用数据目录，便于备份时明确排除和退出清理。
        let log_directory = data_directory.join("logs");
        let temp_directory = data_directory.join("temp");

        Ok(Self {
            data_directory,
            config_directory,
            log_directory,
            temp_directory,
        })
    }

    /// 创建基础目录及架构约定的快照、包和备份占位目录。
    pub fn ensure_required_directories(&self) -> Result<(), AppError> {
        let required = [
            self.data_directory.as_path(),
            self.config_directory.as_path(),
            self.log_directory.as_path(),
            self.temp_directory.as_path(),
        ];
        for directory in required {
            create_directory(directory)?;
        }

        for relative in ["snapshots/sha256", "packages", "backups"] {
            create_directory(&self.data_directory.join(relative))?;
        }
        Ok(())
    }
}

/// 统一包装目录创建错误，使前端始终获得结构化 I/O 错误。
fn create_directory(path: &Path) -> Result<(), AppError> {
    std::fs::create_dir_all(path).map_err(|error| AppError::io("创建应用目录", &error))
}
