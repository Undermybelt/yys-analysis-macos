use crate::application::error::AppError;
use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// 持有异步日志线程；字段只用于生命周期管理，不对外暴露写入器。
pub struct LoggingGuard {
    _worker_guard: WorkerGuard,
}

/// 初始化按日滚动的 JSON Lines 本地日志，并允许开发者用 RUST_LOG 调整级别。
///
/// 日志目录不可写时回退到标准错误输出，不能因为诊断日志权限问题阻断桌面应用启动；
/// 正常安装环境仍优先写入应用数据目录，只有创建滚动文件失败时才触发回退。
pub fn initialize(log_directory: &Path) -> Result<LoggingGuard, AppError> {
    // 使用 builder 的 Result 版本，避免 rolling::daily 在目录权限不足时直接 panic。
    let log_writer: Box<dyn std::io::Write + Send> = match RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("yys-analysis.jsonl")
        .build(log_directory)
    {
        Ok(file_appender) => Box::new(file_appender),
        Err(error) => {
            eprintln!(
                "结构化日志目录不可写，回退到标准错误输出：{}，原因：{error}",
                log_directory.display()
            );
            Box::new(std::io::stderr())
        }
    };
    let (non_blocking, worker_guard) = tracing_appender::non_blocking(log_writer);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .json()
                .with_current_span(false)
                .with_span_list(false)
                .with_writer(non_blocking),
        )
        .try_init()
        .map_err(|error| AppError::internal(format!("初始化结构化日志失败：{error}")))?;

    Ok(LoggingGuard {
        _worker_guard: worker_guard,
    })
}
