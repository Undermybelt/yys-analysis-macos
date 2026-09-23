use crate::application::error::AppError;
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use uuid::Uuid;

/// 后台任务的标准终态，后续导入、分析和更新任务复用同一协议。
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}

/// 应用层进度对象，不包含任何 WebView 或前端框架类型。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgress {
    pub task_id: String,
    pub phase: String,
    pub completed: u32,
    pub total: u32,
    pub message: String,
    pub status: TaskStatus,
    pub error: Option<AppError>,
    /// 业务任务完成时携带可选摘要；没有摘要的任务保持为空。
    pub result: Option<serde_json::Value>,
}

/// 统一保存任务取消令牌；克隆仅共享登记表，不复制任务状态。
#[derive(Clone, Default)]
pub struct TaskRegistry {
    cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

impl TaskRegistry {
    /// 登记新任务并返回稳定 ID 与取消令牌。
    pub fn register(&self) -> (String, Arc<AtomicBool>) {
        let task_id = Uuid::new_v4().to_string();
        let cancellation = Arc::new(AtomicBool::new(false));
        self.lock().insert(task_id.clone(), cancellation.clone());
        (task_id, cancellation)
    }

    /// 请求取消指定任务；已结束任务返回结构化错误，避免虚假成功。
    pub fn cancel(&self, task_id: &str) -> Result<(), AppError> {
        let cancellation = self
            .lock()
            .get(task_id)
            .cloned()
            .ok_or_else(|| AppError::task_not_found(task_id))?;
        cancellation.store(true, Ordering::Release);
        Ok(())
    }

    /// 任务到达任一终态后移除登记，防止长期运行产生内存泄漏。
    pub fn complete(&self, task_id: &str) {
        self.lock().remove(task_id);
    }

    /// 更新安装前只查询后台任务登记表；任何导入、分析或批次任务存在时都延迟激活。
    pub fn has_running(&self) -> bool {
        !self.lock().is_empty()
    }

    /// 应用退出时请求取消全部任务，并返回受影响的任务数量。
    pub fn cancel_all(&self) -> usize {
        let mut tasks = self.lock();
        let count = tasks.len();
        for cancellation in tasks.values() {
            cancellation.store(true, Ordering::Release);
        }
        tasks.clear();
        count
    }

    /// 处理互斥锁中毒时仍取回状态，保证退出清理优先于传播 panic。
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<AtomicBool>>> {
        self.cancellations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::TaskRegistry;

    #[test]
    fn 已登记任务可以统一取消并清空() {
        let registry = TaskRegistry::default();
        let (first, _) = registry.register();
        let (_second, _) = registry.register();

        registry.cancel(&first).expect("已登记任务应允许取消");
        assert_eq!(registry.cancel_all(), 2);
        assert!(registry.cancel(&first).is_err());
    }
}
