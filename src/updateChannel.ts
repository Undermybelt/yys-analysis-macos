import type { UpdateChannel } from "./api/contracts";

/** 构建时固定更新通道；未知或缺失值回退 stable，避免开发环境意外访问测试源。 */
const configuredChannel = (import.meta.env as Record<string, unknown>)[
  "VITE_UPDATE_CHANNEL"
];

export const APP_UPDATE_CHANNEL: UpdateChannel =
  configuredChannel === "test" ? "test" : "stable";

/** B 版专属入口和测试更新面板只在 test 构建中展示。 */
export const IS_TEST_BUILD = APP_UPDATE_CHANNEL === "test";
