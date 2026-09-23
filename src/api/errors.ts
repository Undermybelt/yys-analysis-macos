import type { CommandErrorPayload, ErrorCode } from "./contracts";

// 未知异常统一映射为内部错误，避免组件散落字符串与类型猜测。
const FALLBACK_CODE: ErrorCode = "INTERNAL_ERROR";

/** 前端可安全展示的命令错误，同时保留可重试标记与结构化详情。 */
export class CommandError extends Error {
  readonly code: ErrorCode;
  readonly details: Record<string, unknown> | null;
  readonly retryable: boolean;

  constructor(payload: CommandErrorPayload) {
    super(payload.message);
    this.name = "CommandError";
    this.code = payload.code;
    this.details = payload.details;
    this.retryable = payload.retryable;
  }
}

/** 将 Tauri 抛出的对象或文本收敛到同一种错误类型。 */
export function normalizeCommandError(error: unknown): CommandError {
  if (error instanceof CommandError) {
    return error;
  }

  if (isCommandErrorPayload(error)) {
    return new CommandError(error);
  }

  return new CommandError({
    code: FALLBACK_CODE,
    message: error instanceof Error ? error.message : String(error),
    details: null,
    retryable: false,
  });
}

/** 仅接受后端约定的完整字段，防止把任意对象误当作可恢复错误。 */
function isCommandErrorPayload(value: unknown): value is CommandErrorPayload {
  if (typeof value !== "object" || value === null) {
    return false;
  }

  const candidate = value as Partial<CommandErrorPayload>;
  return (
    typeof candidate.code === "string" &&
    typeof candidate.message === "string" &&
    typeof candidate.retryable === "boolean" &&
    (candidate.details === null || typeof candidate.details === "object")
  );
}
