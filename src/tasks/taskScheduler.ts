import type { TaskProgress } from "../api/contracts";

/** 统一任务终态；运行中任务不会因页面组件卸载而自动消失。 */
export type ScheduledTaskStatus = "running" | "completed" | "cancelled" | "failed";

/** 任务日志等级只影响任务中心的显示色彩，不参与任务状态判断。 */
export type ScheduledTaskLogLevel = "info" | "success" | "warning" | "error";

/** 任务中心展示的一条轻量日志，避免把御魂正文或大对象放入全局状态。 */
export interface ScheduledTaskLog {
  at: string;
  level: ScheduledTaskLogLevel;
  message: string;
}

/** 应用级任务快照；这是任务中心与页面恢复状态共同使用的窄接口。 */
export interface ScheduledTask {
  id: string;
  kind: string;
  title: string;
  status: ScheduledTaskStatus;
  phase: string;
  completed: number;
  total: number;
  message: string;
  error: string | null;
  logs: ScheduledTaskLog[];
  startedAt: string;
  finishedAt: string | null;
  cancellable: boolean;
  metadata: Record<string, string | number | boolean | null>;
  /** 后端任务完成摘要；页面可从中读取导入数量等结构化结果。 */
  result: Record<string, unknown> | null;
}

/** 任务执行器可报告进度、写日志并注册取消动作；页面不需要持有 Worker 引用。 */
export interface TaskExecutionContext {
  report(patch: {
    phase?: string;
    completed?: number;
    total?: number;
    message?: string;
  }): void;
  log(message: string, level?: ScheduledTaskLogLevel): void;
  onCancel(callback: () => void | Promise<void>): void;
  isCancelled(): boolean;
}

/** 启动任务时需要提供的业务适配器；调度器负责执行 Promise 和收敛终态。 */
export interface ScheduledTaskDefinition<T = unknown> {
  id?: string;
  kind: string;
  title: string;
  metadata?: Record<string, string | number | boolean | null>;
  run(context: TaskExecutionContext): Promise<T>;
}

/** 订阅函数返回取消订阅动作，任务中心和页面恢复逻辑都通过它保持解耦。 */
export type TaskSchedulerListener = (tasks: ScheduledTask[]) => void;

/** 桌面端取消接口由应用壳注入，避免任务模块反向依赖 Tauri 客户端。 */
export type BackendTaskCanceller = (taskId: string) => Promise<void>;

type InternalTask = ScheduledTask & {
  cancelRequested: boolean;
  cancelCallbacks: Array<() => void | Promise<void>>;
};

const MAX_TASK_COUNT = 40;
const MAX_LOG_COUNT = 80;

/** 生成浏览器进程内唯一任务 ID；后端任务会直接复用 Rust 返回的 taskId。 */
function createTaskId(kind: string): string {
  return `${kind}-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

/** 把未知异常转换成任务中心可读的短消息，避免 Promise rejection 直接丢失。 */
function errorMessage(caught: unknown): string {
  if (caught instanceof Error) return caught.message;
  if (typeof caught === "string") return caught;
  return "任务执行失败";
}

/** 按后端 phase 给未注册任务补充稳定标题，保证全局监听也能覆盖已有导入任务。 */
function titleForBackendPhase(phase: string): string {
  const titles: Record<string, string> = {
    "rule-recalculation": "重算御魂评分",
    "desktop-read-import": "导入桌面版数据",
    "cbg-import": "导入藏宝阁数据",
    "mumu-read": "读取 MuMu 数据",
  };
  return titles[phase] ?? (phase ? `后台任务 · ${phase}` : "后台任务");
}

/**
 * 创建一个不依赖 Vue 的任务调度器。
 * 任务表和执行 Promise 都属于模块级应用状态，因此页面切换只会卸载视图，不会中断任务。
 */
export function createTaskScheduler() {
  const tasks = new Map<string, InternalTask>();
  const listeners = new Set<TaskSchedulerListener>();
  let cancelBackendTask: BackendTaskCanceller | null = null;

  /** 返回排序后的不可变快照，避免消费者修改调度器内部对象。 */
  function snapshot(): ScheduledTask[] {
    return [...tasks.values()]
      .sort((left, right) => {
        const leftTime = Date.parse(left.startedAt);
        const rightTime = Date.parse(right.startedAt);
        return rightTime - leftTime;
      })
      .map((task) => ({
        ...task,
        logs: task.logs.map((log) => ({ ...log })),
        metadata: { ...task.metadata },
        result: task.result ? { ...task.result } : null,
      }));
  }

  /** 通知所有观察者；日志和进度都沿用同一个事件出口。 */
  function notify(): void {
    const current = snapshot();
    for (const listener of listeners) listener(current);
  }

  /** 只保留有限数量的历史任务，避免长期运行的桌面应用无限增长。 */
  function trimHistory(): void {
    if (tasks.size <= MAX_TASK_COUNT) return;
    const removable = [...tasks.values()]
      .filter((task) => task.status !== "running")
      .sort((left, right) => Date.parse(left.startedAt) - Date.parse(right.startedAt));
    while (tasks.size > MAX_TASK_COUNT && removable.length) {
      const task = removable.shift();
      if (task) tasks.delete(task.id);
    }
  }

  /** 向任务追加短日志；同一消息可重复出现，方便定位分页读取或 Worker 阶段。 */
  function appendLog(
    task: InternalTask,
    message: string,
    level: ScheduledTaskLogLevel,
  ): void {
    task.logs.push({ at: new Date().toISOString(), level, message });
    if (task.logs.length > MAX_LOG_COUNT)
      task.logs.splice(0, task.logs.length - MAX_LOG_COUNT);
  }

  /** 将任务收敛到终态，只允许第一次终态写入，避免取消后又被完成事件覆盖。 */
  function finishTask(
    task: InternalTask,
    status: Exclude<ScheduledTaskStatus, "running">,
    message: string,
    error: string | null = null,
  ): void {
    if (task.status !== "running") return;
    task.status = status;
    task.message = message;
    task.error = error;
    task.finishedAt = new Date().toISOString();
    task.cancellable = false;
    appendLog(
      task,
      message,
      status === "completed" ? "success" : status === "cancelled" ? "warning" : "error",
    );
    notify();
    trimHistory();
  }

  /** 启动任务并把执行异常统一收敛到 failed，调用方只需关注自己的业务结果。 */
  function start<T>(definition: ScheduledTaskDefinition<T>): string {
    const id = definition.id ?? createTaskId(definition.kind);
    if (tasks.get(id)?.status === "running") return id;

    const task: InternalTask = {
      id,
      kind: definition.kind,
      title: definition.title,
      status: "running",
      phase: "starting",
      completed: 0,
      total: 0,
      message: "任务已开始",
      error: null,
      logs: [],
      startedAt: new Date().toISOString(),
      finishedAt: null,
      cancellable: true,
      metadata: { ...(definition.metadata ?? {}) },
      result: null,
      cancelRequested: false,
      cancelCallbacks: [],
    };
    tasks.set(id, task);
    appendLog(task, "任务已开始", "info");
    notify();

    const context: TaskExecutionContext = {
      report(patch) {
        if (task.status !== "running") return;
        if (patch.phase !== undefined) task.phase = patch.phase;
        if (patch.completed !== undefined)
          task.completed = Math.max(0, patch.completed);
        if (patch.total !== undefined) task.total = Math.max(0, patch.total);
        if (patch.message !== undefined) task.message = patch.message;
        notify();
      },
      log(message, level = "info") {
        if (task.status !== "running") return;
        task.message = message;
        appendLog(task, message, level);
        notify();
      },
      onCancel(callback) {
        task.cancelCallbacks.push(callback);
        if (task.cancelRequested) void callback();
      },
      isCancelled() {
        return task.cancelRequested;
      },
    };

    void Promise.resolve()
      .then(() => definition.run(context))
      .then(() => {
        finishTask(task, "completed", task.message || "任务完成");
      })
      .catch((caught: unknown) => {
        if (task.cancelRequested) return;
        const message = errorMessage(caught);
        finishTask(task, "failed", message, message);
      });

    return id;
  }

  /** 取消任务时先写入统一终态，再触发 Worker、后端命令等具体取消动作。 */
  function cancel(taskId: string): void {
    const task = tasks.get(taskId);
    if (!task || task.status !== "running") return;
    task.cancelRequested = true;
    finishTask(task, "cancelled", "任务已取消");
    for (const callback of task.cancelCallbacks) {
      void Promise.resolve(callback()).catch((caught: unknown) => {
        appendLog(task, `取消动作失败：${errorMessage(caught)}`, "error");
        notify();
      });
    }
    if (task.kind.startsWith("backend:") && cancelBackendTask) {
      void cancelBackendTask(task.id).catch((caught: unknown) => {
        appendLog(task, `后台任务取消请求失败：${errorMessage(caught)}`, "error");
        notify();
      });
    }
  }

  /** 页面挂载后可通过 kind 找回仍在运行的任务，避免重复启动同一份计算。 */
  function findRunning(kind: string): ScheduledTask | null {
    const task = [...tasks.values()].find(
      (candidate) => candidate.kind === kind && candidate.status === "running",
    );
    return task
      ? (snapshot().find((candidate) => candidate.id === task.id) ?? null)
      : null;
  }

  /** 接入 Rust 全局进度事件；没有显式 start 的后端任务也会自动出现在任务中心。 */
  function adoptBackendProgress(progress: TaskProgress): void {
    const kind = `backend:${progress.phase || "unknown"}`;
    let task = tasks.get(progress.taskId);
    if (!task) {
      task = {
        id: progress.taskId,
        kind,
        title: titleForBackendPhase(progress.phase),
        status: "running",
        phase: progress.phase,
        completed: progress.completed,
        total: progress.total,
        message: progress.message || "后台任务已开始",
        error: null,
        logs: [],
        startedAt: new Date().toISOString(),
        finishedAt: null,
        cancellable: true,
        metadata: {},
        result: null,
        cancelRequested: false,
        cancelCallbacks: [],
      };
      tasks.set(task.id, task);
      appendLog(task, task.message, "info");
    }
    if (task.status !== "running") return;
    task.phase = progress.phase;
    task.completed = Math.max(0, progress.completed);
    task.total = Math.max(0, progress.total);
    if (progress.result) task.result = { ...progress.result };
    if (progress.message) {
      task.message = progress.message;
      const lastLog = task.logs[task.logs.length - 1];
      if (!lastLog || lastLog.message !== progress.message)
        appendLog(task, progress.message, "info");
    }
    if (progress.status === "completed") {
      task.completed = task.total || task.completed;
      finishTask(task, "completed", progress.message || "任务完成");
      return;
    }
    if (progress.status === "cancelled") {
      finishTask(task, "cancelled", progress.message || "任务已取消");
      return;
    }
    if (progress.status === "failed") {
      const message = progress.error?.message || progress.message || "后台任务失败";
      finishTask(task, "failed", message, message);
      return;
    }
    notify();
  }

  return {
    start,
    cancel,
    snapshot,
    findRunning,
    adoptBackendProgress,
    configureBackendCancellation(canceller: BackendTaskCanceller): void {
      cancelBackendTask = canceller;
    },
    subscribe(listener: TaskSchedulerListener): () => void {
      listeners.add(listener);
      listener(snapshot());
      return () => listeners.delete(listener);
    },
  };
}

/** 应用壳共享的单例；所有页面和任务中心通过同一个任务表协作。 */
export const taskScheduler = createTaskScheduler();
