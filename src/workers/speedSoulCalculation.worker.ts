import {
  calculateSpeedPreview,
  type SpeedCalculationResult,
  type SpeedTargetSet,
} from "../speedSoulCalculationEngine";
import type { MySoul } from "../api/contracts";

/** 后台计算请求；完整御魂正文只在当前一次计算中跨线程传递，不写入额外存储。 */
type SpeedWorkerRequest = {
  souls: MySoul[];
  onlyMaxLevel: boolean;
  targetSets: SpeedTargetSet[];
};

/** Worker 只返回最终结果或错误，页面继续负责缓存、展示和完成通知。 */
type SpeedWorkerResponse =
  | { type: "completed"; result: SpeedCalculationResult }
  | { type: "failed"; message: string };

type SpeedWorkerScope = {
  onmessage: ((event: MessageEvent<SpeedWorkerRequest>) => void) | null;
  postMessage: (message: SpeedWorkerResponse) => void;
};

// tsconfig 只启用了 DOM 类型，这里通过最小接口约束 Worker 全局对象。
const workerScope = self as unknown as SpeedWorkerScope;

workerScope.onmessage = (event) => {
  try {
    workerScope.postMessage({
      type: "completed",
      result: calculateSpeedPreview(
        event.data.souls,
        event.data.onlyMaxLevel,
        event.data.targetSets,
      ),
    });
  } catch (error) {
    workerScope.postMessage({
      type: "failed",
      message: error instanceof Error ? error.message : "一速后台计算线程异常结束",
    });
  }
};
