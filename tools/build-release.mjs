#!/usr/bin/env node

/**
 * 使用同一个通道参数驱动 Vite 和 Rust，避免前端显示 test 而后端实际连接 stable。
 * 版本号仍由 npm run version:set 单独管理，构建脚本只负责通道一致性和 NSIS 打包。
 */

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";

const channel = process.argv[2];
if (channel !== "stable" && channel !== "test") {
  throw new Error("用法：node tools/build-release.mjs stable|test");
}

// 公钥是可公开的发行信任根，构建默认从仓库固定文件读取；
// 环境变量只允许显式传入同一公钥，避免误构建出无法验证现有清单的安装包。
const builtInReleasePublicKeyHex = readFileSync(
  new URL("../src-tauri/release-public-key.hex", import.meta.url),
  "utf8",
).trim();
const releasePublicKeyHex =
  process.env.YYS_RELEASE_ED25519_PUBLIC_KEY_HEX?.trim() ?? builtInReleasePublicKeyHex;
if (!releasePublicKeyHex || !/^[0-9a-f]{64}$/i.test(releasePublicKeyHex)) {
  throw new Error(
    "缺少有效的 YYS_RELEASE_ED25519_PUBLIC_KEY_HEX（必须是 64 位十六进制发行公钥）",
  );
}
if (releasePublicKeyHex.toLowerCase() !== builtInReleasePublicKeyHex.toLowerCase()) {
  throw new Error("构建公钥与仓库内置发行公钥不一致");
}

const command = process.platform === "win32" ? "npm.cmd" : "npm";
const result = spawnSync(
  command,
  ["run", "tauri", "--", "build", "--bundles", "nsis"],
  {
    stdio: "inherit",
    // Windows 的 npm.cmd 是命令脚本，必须交给 shell 启动；参数固定且不含用户输入。
    shell: process.platform === "win32",
    env: {
      ...process.env,
      VITE_UPDATE_CHANNEL: channel,
      YYS_UPDATE_CHANNEL: channel,
      YYS_RELEASE_ED25519_PUBLIC_KEY_HEX: releasePublicKeyHex,
    },
  },
);

if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
