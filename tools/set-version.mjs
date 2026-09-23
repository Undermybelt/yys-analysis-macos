#!/usr/bin/env node

/**
 * 同步桌面应用的发布版本号。
 * 版本号同时进入前端展示、Tauri 清单、Rust 包元数据和 npm lock，避免只修改一处导致安装包与帮助页不一致。
 */

import { readFile, writeFile } from "node:fs/promises";

const version = process.argv[2];

if (!version || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error("用法：node tools/set-version.mjs 0.1.0-beta.3");
}

/** 只替换已知文件中的版本字段，保留其他格式和注释不变。 */
async function replaceVersion(path, pattern, replacement, expectedCount = 1) {
  const source = await readFile(path, "utf8");
  const matches = source.match(pattern);
  const matchCount = pattern.global ? (matches?.length ?? 0) : matches ? 1 : 0;
  if (matchCount !== expectedCount) {
    throw new Error(
      path + " 的版本字段数量异常：期望 " + expectedCount + "，实际 " + matchCount,
    );
  }
  const next = source.replace(pattern, replacement);
  if (next !== source) await writeFile(path, next, "utf8");
}

await replaceVersion(
  "package.json",
  /("name": "yys-analysis",\r?\n\s+"version": ")[^"]+(")/,
  "$1" + version + "$2",
);
await replaceVersion(
  "package-lock.json",
  /("name": "yys-analysis",\r?\n\s+"version": ")[^"]+(")/g,
  "$1" + version + "$2",
  2,
);
await replaceVersion(
  "src-tauri/tauri.conf.json",
  /("version": ")[^"]+(")/,
  "$1" + version + "$2",
);
await replaceVersion(
  "src-tauri/Cargo.toml",
  /^(version = ")[^"]+("$)/m,
  `$1${version}$2`,
);

process.stdout.write(`已同步版本：${version}\n`);
