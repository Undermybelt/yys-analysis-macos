//! Tauri 托管状态：应用服务集合与日志守卫。

use crate::application::services::AppServices;
use crate::infrastructure::logging::LoggingGuard;
use std::sync::Arc;

/// 应用共享状态，命令通过 `State<'_, AppState>` 获取。
pub struct AppState {
    pub services: Arc<AppServices>,
    pub _logging_guard: LoggingGuard,
}
