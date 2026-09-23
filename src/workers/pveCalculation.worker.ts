import type { MySoul, SoulSet } from "../api/contracts";
import {
  calculatePvePanel,
  type PveCalculationProgress,
  type PveCalculationResult,
} from "../pveCalculationEngine";

/** Worker 接收的计算请求；御魂正文和目录均来自当前一次读取，避免再次访问 Tauri。 */
type PveWorkerRequest = {
  type: "calculate";
  souls: MySoul[];
  catalogSets: SoulSet[];
};

/** Worker 返回增量进度或最终结果；错误统一在主线程转为页面提示。 */
export type PveWorkerResponse =
  | { type: "progress"; progress: PveCalculationProgress }
  | { type: "log"; message: string }
  | { type: "completed"; result: PveCalculationResult }
  | { type: "failed"; message: string };

type PveWorkerScope = {
  onmessage: ((event: MessageEvent<PveWorkerRequest>) => void) | null;
  postMessage: (message: PveWorkerResponse) => void;
};

// tsconfig 只启用了 DOM 类型，这里通过最小接口约束 Worker 全局对象，避免引入第二套 lib 配置。
const workerScope = self as unknown as PveWorkerScope;

workerScope.onmessage = (event) => {
  if (event.data.type !== "calculate") return;
  try {
    const result = calculatePvePanel(
      event.data.souls,
      event.data.catalogSets,
      (progress) => workerScope.postMessage({ type: "progress", progress }),
      // 组合搜索在 Worker 中执行，开始日志通过独立消息即时返回，主线程才能写入任务日志。
      (message) => workerScope.postMessage({ type: "log", message }),
    );
    workerScope.postMessage({ type: "completed", result });
  } catch (error) {
    workerScope.postMessage({
      type: "failed",
      message: error instanceof Error ? error.message : "PVE 计算线程异常结束",
    });
  }
};
