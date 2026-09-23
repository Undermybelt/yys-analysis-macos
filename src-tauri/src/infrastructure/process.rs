use crate::application::error::AppError;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

/// Windows 的 CREATE_NO_WINDOW 标志，确保辅助进程不会弹出额外控制台窗口。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 清理结果用于退出日志和测试，记录进程是否在清理前仍然运行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessCleanup {
    pub process_id: u32,
    pub label: String,
    pub was_running: bool,
}

/// 临时文件清理结果；用于会话完成、取消和应用退出日志，不记录文件内容。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempFileCleanup {
    pub path: PathBuf,
    pub removed: bool,
}

/// 登记读取会话创建的临时文件，避免异常退出后留下适配器载荷或中间 JSON。
#[derive(Clone, Default)]
pub struct TemporaryFileRegistry {
    paths: Arc<Mutex<HashSet<PathBuf>>>,
}

impl TemporaryFileRegistry {
    /// 登记一个临时文件路径；写入方必须仍然使用应用解析出的 temp 目录。
    pub fn register(&self, path: PathBuf) {
        self.paths
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path);
    }

    /// 删除指定临时文件并从登记表移除，文件已经不存在也视为完成清理。
    pub fn cleanup_all(&self) -> Vec<TempFileCleanup> {
        let paths = std::mem::take(
            &mut *self
                .paths
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        paths
            .into_iter()
            .map(|path| {
                let removed = match std::fs::remove_file(&path) {
                    Ok(()) => true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                    Err(error) => {
                        tracing::warn!(path = %path.display(), reason = %error, "清理临时文件失败");
                        false
                    }
                };
                TempFileCleanup { path, removed }
            })
            .collect()
    }
}

/// 统一登记本工具启动的子进程，克隆只共享登记表。
#[derive(Clone, Default)]
pub struct ProcessRegistry {
    children: Arc<Mutex<HashMap<u32, TrackedProcess>>>,
}

struct TrackedProcess {
    label: String,
    /// 子进程由清理线程和自然退出监视线程共享；互斥锁保证 wait/kill 不会并发操作同一个句柄。
    child: Arc<Mutex<Child>>,
    /// 主动清理前先关闭此标志，避免 kill 被自然退出监视器误报成桥接故障。
    exit_monitor_active: Option<Arc<AtomicBool>>,
    /// 进程被强制终止前执行的外部资源清理；用于 sidecar 在异常结束时回收 Android 服务。
    cleanup: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl ProcessRegistry {
    /// 通过登记器启动子进程，后续 ADB、Frida 和适配器不得绕过此入口。
    #[allow(dead_code)] // 门票 07 的读取适配器将成为首个生产调用方。
    pub fn spawn_registered(
        &self,
        label: impl Into<String>,
        command: &mut Command,
    ) -> Result<u32, AppError> {
        configure_hidden_process(command);
        let child = command
            .spawn()
            .map_err(|error| AppError::io("启动受控子进程", &error))?;
        let process_id = child.id();
        self.lock().insert(
            process_id,
            TrackedProcess {
                label: label.into(),
                child: Arc::new(Mutex::new(child)),
                exit_monitor_active: None,
                cleanup: None,
            },
        );
        tracing::info!(process_id, "受控子进程已登记");
        Ok(process_id)
    }

    /// 启动并登记带标准输出的受控子进程；每行输出交给调用方解析，适合读取桥接批次协议。
    ///
    /// 输出读取线程只负责转发和日志，不持有进程登记表，避免桥接进程因管道未消费而阻塞；
    /// 进程仍由同一个登记器负责终止和回收，取消、失败与应用退出都能复用既有清理路径。
    #[allow(dead_code)] // 当前 Frida 桥接需要额外的 Android 清理回调，保留通用输出入口供其他 sidecar 复用。
    pub fn spawn_registered_with_output<F>(
        &self,
        label: impl Into<String>,
        command: &mut Command,
        on_stdout_line: F,
    ) -> Result<u32, AppError>
    where
        F: Fn(String) + Send + 'static,
    {
        self.spawn_registered_with_output_and_cleanup(label, command, on_stdout_line, None)
    }

    /// 启动带输出的受控子进程，并登记其终止前必须执行的一次性清理动作。
    pub fn spawn_registered_with_output_and_cleanup<F>(
        &self,
        label: impl Into<String>,
        command: &mut Command,
        on_stdout_line: F,
        cleanup: Option<Box<dyn FnOnce() + Send + 'static>>,
    ) -> Result<u32, AppError>
    where
        F: Fn(String) + Send + 'static,
    {
        self.spawn_registered_with_output_and_cleanup_and_exit(
            label,
            command,
            on_stdout_line,
            cleanup,
            None,
        )
    }

    /// 启动带输出的受控子进程，并在其未被主动清理而自然退出时回调调用方。
    ///
    /// MuMu 桥接失败时可能没有任何 stdout JSON；自然退出回调让上层能结束“等待触发”假状态，
    /// 同时保留既有清理路径，避免把桥接 stderr 原文直接暴露到前端。
    pub fn spawn_registered_with_output_and_cleanup_and_exit<F>(
        &self,
        label: impl Into<String>,
        command: &mut Command,
        on_stdout_line: F,
        cleanup: Option<Box<dyn FnOnce() + Send + 'static>>,
        on_exit: Option<Box<dyn FnOnce(Option<ExitStatus>) + Send + 'static>>,
    ) -> Result<u32, AppError>
    where
        F: Fn(String) + Send + 'static,
    {
        configure_hidden_process(command);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| AppError::io("启动受控读取桥接", &error))?;
        let process_id = child.id();

        // 标准输出承载结构化事件；单独线程持续读取，避免大批次 JSON 填满操作系统管道。
        if let Some(stdout) = child.stdout.take() {
            thread::spawn(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(stdout).lines() {
                    match line {
                        Ok(line) => on_stdout_line(line),
                        Err(error) => {
                            tracing::warn!(reason = %error, "读取 MuMu 桥接标准输出失败");
                            break;
                        }
                    }
                }
            });
        }

        // 标准错误只进入截断日志，不向前端透传原始内容，避免桥接运行时泄露库存正文。
        if let Some(stderr) = child.stderr.take() {
            thread::spawn(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(stderr).lines() {
                    match line {
                        Ok(line) => tracing::warn!(stderr = %line, "MuMu 桥接运行日志"),
                        Err(error) => {
                            tracing::warn!(reason = %error, "读取 MuMu 桥接错误输出失败");
                            break;
                        }
                    }
                }
            });
        }

        let child = Arc::new(Mutex::new(child));
        let exit_monitor_active = on_exit.as_ref().map(|_| Arc::new(AtomicBool::new(true)));
        self.lock().insert(
            process_id,
            TrackedProcess {
                label: label.into(),
                child: Arc::clone(&child),
                exit_monitor_active: exit_monitor_active.clone(),
                cleanup,
            },
        );
        // 先登记再启动监视线程，避免桥接立即退出时回调抢在登记完成前执行清理。
        if let Some(on_exit) = on_exit {
            let monitored_child = Arc::clone(&child);
            let monitor_active = exit_monitor_active
                .as_ref()
                .expect("存在退出回调时必须创建监视开关")
                .clone();
            thread::spawn(move || {
                // 轮询间隔限制在较短范围内，保证桥接故障能及时反馈，同时避免高频占用 CPU。
                loop {
                    if !monitor_active.load(Ordering::Acquire) {
                        break;
                    }
                    let result = monitored_child
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .try_wait();
                    match result {
                        Ok(Some(status)) => {
                            if monitor_active.load(Ordering::Acquire) {
                                tracing::warn!(process_id, ?status, "受控读取桥接进程自然退出");
                                on_exit(Some(status));
                            }
                            break;
                        }
                        Ok(None) => thread::sleep(Duration::from_millis(250)),
                        Err(error) => {
                            tracing::warn!(process_id, reason = %error, "监视受控读取桥接进程失败");
                            if monitor_active.load(Ordering::Acquire) {
                                on_exit(None);
                            }
                            break;
                        }
                    }
                }
            });
        }
        tracing::info!(process_id, "带输出的受控子进程已登记");
        Ok(process_id)
    }

    /// 应用退出时终止并回收全部登记进程，已自然退出的进程只执行 wait 回收。
    pub fn cleanup_all(&self) -> Vec<ProcessCleanup> {
        let children = std::mem::take(&mut *self.lock());
        cleanup_children(children)
    }

    /// 仅清理指定标签前缀的进程，避免结束一个读取会话时误伤其他受控任务。
    pub fn cleanup_label_prefix(&self, prefix: &str) -> Vec<ProcessCleanup> {
        let mut registered = self.lock();
        let selected_ids = registered
            .iter()
            .filter(|(_, tracked)| tracked.label.starts_with(prefix))
            .map(|(process_id, _)| *process_id)
            .collect::<Vec<_>>();
        let selected = selected_ids
            .into_iter()
            .filter_map(|process_id| {
                registered
                    .remove(&process_id)
                    .map(|tracked| (process_id, tracked))
            })
            .collect::<HashMap<_, _>>();
        drop(registered);
        cleanup_children(selected)
    }
}

/// 执行进程终止与 wait 回收；调用方已经先从登记表中摘除目标进程。
fn cleanup_children(children: HashMap<u32, TrackedProcess>) -> Vec<ProcessCleanup> {
    let mut results = Vec::with_capacity(children.len());

    for (process_id, mut tracked) in children {
        // 先禁止自然退出回调，再终止目标进程；取消或完成读取不应被报告为桥接故障。
        if let Some(monitor_active) = tracked.exit_monitor_active.take() {
            monitor_active.store(false, Ordering::Release);
        }
        let mut child = tracked
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let was_running = match child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => {
                if let Err(error) = child.kill() {
                    tracing::warn!(
                        process_id,
                        reason = %error,
                        "终止受控子进程失败"
                    );
                }
                true
            }
            Err(error) => {
                tracing::warn!(process_id, reason = %error, "查询受控子进程状态失败");
                if let Err(kill_error) = child.kill() {
                    tracing::warn!(
                        process_id,
                        reason = %kill_error,
                        "状态未知时终止受控子进程失败"
                    );
                }
                true
            }
        };

        if let Err(error) = child.wait() {
            tracing::warn!(process_id, reason = %error, "回收受控子进程失败");
        }
        // 等 Windows sidecar 已经结束后再清理 Android 服务，避免它在清理回调之后又重新启动服务。
        if let Some(cleanup) = tracked.cleanup.take() {
            cleanup();
        }
        results.push(ProcessCleanup {
            process_id,
            label: tracked.label,
            was_running,
        });
    }
    results
}

/// 处理互斥锁中毒时仍取回登记表，保证进程清理不会因其他线程 panic 被跳过。
impl ProcessRegistry {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u32, TrackedProcess>> {
        self.children
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Windows 下为所有辅助进程统一关闭控制台窗口，其他平台保持命令默认行为。
#[cfg(windows)]
fn configure_hidden_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_hidden_process(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::{ProcessRegistry, TemporaryFileRegistry};
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn 退出清理会终止已登记测试进程() {
        let registry = ProcessRegistry::default();

        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("cmd");
            command.args(["/C", "ping -n 30 127.0.0.1 > NUL"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 30"]);
            command
        };

        let process_id = registry
            .spawn_registered("cleanup-test", &mut command)
            .expect("测试子进程应能启动");
        let cleanup = registry.cleanup_all();

        assert_eq!(cleanup.len(), 1);
        assert_eq!(cleanup[0].process_id, process_id);
        assert!(cleanup[0].was_running);
        assert!(registry.cleanup_all().is_empty());
    }

    #[test]
    fn 临时文件登记器会删除会话文件并允许重复清理() {
        let registry = TemporaryFileRegistry::default();
        let path =
            std::env::temp_dir().join(format!("yys-mumu-cleanup-{}.tmp", std::process::id()));
        std::fs::write(&path, b"temporary adapter data").expect("写入临时夹具");
        registry.register(PathBuf::from(&path));

        let result = registry.cleanup_all();
        assert_eq!(result.len(), 1);
        assert!(result[0].removed);
        assert!(!path.exists());
        assert!(registry.cleanup_all().is_empty());
    }

    #[test]
    fn 桥接自然退出会触发退出回调() {
        let registry = ProcessRegistry::default();
        let (sender, receiver) = mpsc::channel();

        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("cmd");
            command.args(["/C", "exit 7"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-c", "exit 7"]);
            command
        };

        // 退出回调模拟 MuMu 桥接失败通知；自然退出的进程仍由登记器统一回收。
        registry
            .spawn_registered_with_output_and_cleanup_and_exit(
                "bridge-exit-test",
                &mut command,
                |_| {},
                None,
                Some(Box::new(move |status| {
                    sender.send(status.and_then(|value| value.code())).unwrap();
                })),
            )
            .expect("测试桥接进程应能启动");

        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            Some(7)
        );
        registry.cleanup_all();
    }
}
