use serde::Serialize;
use serde_json::{Value, json};
use std::fmt::{Display, Formatter};

/// 稳定错误码是前后端分支判断的唯一依据，错误文本只用于向用户解释。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidArgument,
    PathUnavailable,
    TaskNotFound,
    IoError,
    EventEmitFailed,
    InternalError,
    DatabaseError,
    NotFound,
    Conflict,
    ChecksumMismatch,
    MigrationFailed,
    BackupFailed,
    ConstraintViolation,
    MumuUnavailable,
    DesktopUnavailable,
    AdapterRejected,
    ReadSessionConflict,
    UpdateRejected,
    UpdateSourceUnavailable,
    ResourceLimitExceeded,
}

/// 所有 Tauri 命令统一返回的结构化错误。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    pub details: Option<Value>,
    pub retryable: bool,
}

impl AppError {
    /// 创建参数校验错误，并附带字段级详情供界面定位。
    pub fn invalid_argument(field: &str, message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InvalidArgument,
            message: message.into(),
            details: Some(json!({ "field": field })),
            retryable: false,
        }
    }

    /// 创建带文件名、JSON 路径、期望类型和实际值的导入校验错误。
    pub fn import_field(
        file_name: &str,
        json_path: &str,
        expected: &str,
        actual: &str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: ErrorCode::InvalidArgument,
            message: message.into(),
            details: Some(json!({
                "fileName": file_name,
                "jsonPath": json_path,
                "expected": expected,
                "actual": actual,
            })),
            retryable: false,
        }
    }

    /// 创建目录解析错误；重试通常无法修复系统目录缺失，因此不可重试。
    pub fn path_unavailable(path_kind: &str, message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::PathUnavailable,
            message: message.into(),
            details: Some(json!({ "pathKind": path_kind })),
            retryable: false,
        }
    }

    /// 创建任务不存在错误，避免取消请求误伤其他或已完成任务。
    pub fn task_not_found(task_id: &str) -> Self {
        Self {
            code: ErrorCode::TaskNotFound,
            message: "任务不存在或已经结束".to_owned(),
            details: Some(json!({ "taskId": task_id })),
            retryable: false,
        }
    }

    /// 将本地 I/O 失败包装为稳定错误，不向前端暴露 Rust 类型。
    pub fn io(operation: &str, error: &std::io::Error) -> Self {
        Self {
            code: ErrorCode::IoError,
            message: format!("本地操作失败：{operation}"),
            details: Some(json!({ "reason": error.to_string() })),
            retryable: true,
        }
    }

    /// 事件发送失败表示 WebView 通道不可用，任务应停止继续产生无消费者事件。
    pub fn event_emit(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::EventEmitFailed,
            message: "无法向界面发送任务进度".to_owned(),
            details: Some(json!({ "reason": message.into() })),
            retryable: true,
        }
    }

    /// 收敛无法归入已知类别的内部错误。
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InternalError,
            message: message.into(),
            details: None,
            retryable: false,
        }
    }

    /// 数据库连接、查询或事务失败。
    pub fn database(operation: &str, error: &(impl std::error::Error + ToString)) -> Self {
        Self {
            code: ErrorCode::DatabaseError,
            message: format!("数据库操作失败：{operation}"),
            details: Some(serde_json::json!({ "reason": error.to_string() })),
            retryable: true,
        }
    }

    /// 查询的实体不存在。
    pub fn not_found(entity: &str, id: &str) -> Self {
        Self {
            code: ErrorCode::NotFound,
            message: format!("{entity} 不存在：{id}"),
            details: Some(serde_json::json!({ "entity": entity, "id": id })),
            retryable: false,
        }
    }

    /// 乐观并发修订号冲突。
    pub fn conflict(entity: &str, id: &str, expected: u32, actual: u32) -> Self {
        Self {
            code: ErrorCode::Conflict,
            message: format!("{entity}({id}) 已被其他操作修改"),
            details: Some(serde_json::json!({
                "entity": entity, "id": id,
                "expectedRevision": expected, "actualRevision": actual,
            })),
            retryable: false,
        }
    }

    /// 原始对象文件哈希不匹配，已隔离到隔离区。
    pub fn checksum_mismatch(path: &str, expected: &str, actual: &str) -> Self {
        Self {
            code: ErrorCode::ChecksumMismatch,
            message: format!("原始快照文件哈希校验失败：{path}"),
            details: Some(serde_json::json!({
                "path": path, "expectedHash": expected, "actualHash": actual,
                "quarantined": true,
            })),
            retryable: true,
        }
    }

    /// 数据库迁移失败，包含版本号和失败原因。
    pub fn migration_failed(version: u32, error: &str) -> Self {
        Self {
            code: ErrorCode::MigrationFailed,
            message: format!("数据库迁移（v{version}）失败：{error}"),
            details: Some(serde_json::json!({ "version": version, "reason": error })),
            retryable: false,
        }
    }

    /// 备份或恢复操作失败。
    pub fn backup_failed(operation: &str, error: &str) -> Self {
        Self {
            code: ErrorCode::BackupFailed,
            message: format!("备份操作失败：{operation}"),
            details: Some(serde_json::json!({ "operation": operation, "reason": error })),
            retryable: true,
        }
    }

    /// 数据库约束违反（唯一约束、外键等）。
    pub fn constraint_violation(entity: &str, error: &(impl std::error::Error + ToString)) -> Self {
        Self {
            code: ErrorCode::ConstraintViolation,
            message: format!("{entity} 违反数据约束"),
            details: Some(serde_json::json!({ "reason": error.to_string() })),
            retryable: false,
        }
    }

    /// MuMu 参考快照工具、模拟器或游戏进程不满足读取前置条件。
    pub fn mumu_unavailable(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::MumuUnavailable,
            message: message.into(),
            details: None,
            retryable: true,
        }
    }

    /// 桌面版客户端未登录、缓存不可读或缓存快照不稳定；不触碰当前已导入数据。
    pub fn desktop_unavailable(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::DesktopUnavailable,
            message: message.into(),
            details: None,
            retryable: true,
        }
    }

    /// 适配器签名、哈希、版本或能力边界校验失败，禁止执行适配器载荷。
    pub fn adapter_rejected(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::AdapterRejected,
            message: message.into(),
            details: None,
            retryable: false,
        }
    }

    /// 同时存在另一个活动读取会话，避免两个附加周期交叉清理资源。
    pub fn read_session_conflict() -> Self {
        Self {
            code: ErrorCode::ReadSessionConflict,
            message: "已有一个 MuMu 读取会话正在运行".to_owned(),
            details: None,
            retryable: false,
        }
    }

    /// 更新清单、签名、哈希或兼容范围不满足约束时拒绝安装，避免错误载荷进入活动目录。
    pub fn update_rejected(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::UpdateRejected,
            message: message.into(),
            details: None,
            retryable: false,
        }
    }

    /// GitHub 或 Gitee 镜像不可访问时返回可重试错误；自动模式可继续尝试另一个镜像。
    pub fn update_source_unavailable(source: &str, message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::UpdateSourceUnavailable,
            message: format!("{source} 更新源不可用"),
            details: Some(json!({ "source": source, "reason": message.into() })),
            retryable: true,
        }
    }

    /// 外部载荷超过资源预算时拒绝继续解析、解压或保存，避免进程被异常输入拖垮。
    pub fn resource_limit(field: &str, actual: u64, limit: u64) -> Self {
        Self {
            code: ErrorCode::ResourceLimitExceeded,
            message: format!("输入载荷超过安全上限：{field}"),
            details: Some(serde_json::json!({
                "field": field,
                "actual": actual,
                "limit": limit,
            })),
            retryable: false,
        }
    }

}

impl Display for AppError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}
