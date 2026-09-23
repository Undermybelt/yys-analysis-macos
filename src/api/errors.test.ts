import { describe, expect, it } from "vitest";
import { CommandError, normalizeCommandError } from "./errors";

describe("normalizeCommandError", () => {
  it("保留后端结构化错误码和可重试状态", () => {
    const error = normalizeCommandError({
      code: "TASK_NOT_FOUND",
      message: "任务不存在或已经结束",
      details: { taskId: "missing" },
      retryable: false,
    });

    expect(error).toBeInstanceOf(CommandError);
    expect(error.code).toBe("TASK_NOT_FOUND");
    expect(error.details).toEqual({ taskId: "missing" });
  });

  it("将普通异常收敛为内部错误", () => {
    const error = normalizeCommandError(new Error("bridge unavailable"));

    expect(error.code).toBe("INTERNAL_ERROR");
    expect(error.message).toBe("bridge unavailable");
    expect(error.retryable).toBe(false);
  });
});
