import { describe, expect, it, vi } from "vitest";
import { createTaskScheduler, type TaskExecutionContext } from "./taskScheduler";

/** 等待调度器启动 Promise 执行器，测试不依赖真实计时器或 Worker。 */
async function flushTaskRunner(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

describe("taskScheduler", () => {
  it("执行任务并汇总进度与日志", async () => {
    const scheduler = createTaskScheduler();
    const taskId = scheduler.start({
      kind: "unit-task",
      title: "单元测试任务",
      run: (context) => {
        context.report({
          phase: "读取",
          completed: 2,
          total: 4,
          message: "已读取 2 / 4",
        });
        context.log("进入计算阶段");
        return Promise.resolve();
      },
    });

    await flushTaskRunner();
    const task = scheduler.snapshot().find((candidate) => candidate.id === taskId);
    expect(task?.status).toBe("completed");
    expect(task?.phase).toBe("读取");
    expect(task?.message).toBe("进入计算阶段");
    expect(task?.logs.map((log) => log.message)).toContain("进入计算阶段");
  });

  it("取消任务后不会被迟到的执行结果覆盖", async () => {
    const scheduler = createTaskScheduler();
    let context: TaskExecutionContext | null = null;
    const cancelHandler = vi.fn();
    const taskId = scheduler.start({
      kind: "cancelable-task",
      title: "可取消任务",
      run: (taskContext) => {
        context = taskContext;
        taskContext.onCancel(cancelHandler);
        return new Promise<void>(() => undefined);
      },
    });

    await flushTaskRunner();
    expect(context).not.toBeNull();
    scheduler.cancel(taskId);

    const task = scheduler.snapshot().find((candidate) => candidate.id === taskId);
    expect(task?.status).toBe("cancelled");
    expect(cancelHandler).toHaveBeenCalledTimes(1);
  });

  it("接收后端任务事件并使用注入的取消适配器", () => {
    const scheduler = createTaskScheduler();
    const cancelBackendTask = vi.fn(() => Promise.resolve());
    scheduler.configureBackendCancellation(cancelBackendTask);
    scheduler.adoptBackendProgress({
      taskId: "rust-task-1",
      phase: "rule-recalculation",
      completed: 3,
      total: 10,
      message: "已重算 3 / 10",
      status: "running",
      error: null,
      result: null,
    });

    expect(scheduler.snapshot()[0]).toMatchObject({
      id: "rust-task-1",
      title: "重算御魂评分",
      status: "running",
      completed: 3,
      total: 10,
    });
    scheduler.cancel("rust-task-1");
    expect(cancelBackendTask).toHaveBeenCalledWith("rust-task-1");
  });
});
