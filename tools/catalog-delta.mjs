#!/usr/bin/env node

/**
 * 根据两个紧凑式神目录生成可按基础版本应用的增量补丁。
 * 补丁只描述新增、删除和发生变化的条目，未变化条目由客户端从本地活动目录保留。
 */

import { readFile, writeFile } from "node:fs/promises";

const PATCH_FORMAT = "yys-shikigami-patch";
const PATCH_PROTOCOL_VERSION = 1;

/** 将命令行参数解析为键值对象，拒绝没有值的位置参数。 */
function parseArgs(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (!argument.startsWith("--")) throw new Error(`参数格式无效：${argument}`);
    const key = argument.slice(2);
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) throw new Error(`参数缺少值：${argument}`);
    values[key] = value;
    index += 1;
  }
  return values;
}

/** 从紧凑目录读取 JSON，并在计算差异前确认发布方没有重复式神 ID。 */
async function readPackage(path, label) {
  const packageData = JSON.parse(await readFile(path, "utf8"));
  if (!packageData.version || !packageData.gameVersion) {
    throw new Error(`${label} 缺少 version 或 gameVersion`);
  }
  if (!Array.isArray(packageData.entries) || packageData.entries.length === 0) {
    throw new Error(`${label} 的 entries 必须是非空数组`);
  }
  if (!Array.isArray(packageData.defaultEmploymentScenes)) {
    throw new Error(`${label} 缺少 defaultEmploymentScenes 数组`);
  }
  if (!Array.isArray(packageData.defaultEvidence)) {
    throw new Error(`${label} 缺少 defaultEvidence 数组`);
  }
  indexEntries(packageData.entries, label);
  if (Number(packageData.expectedCount) !== packageData.entries.length) {
    throw new Error(`${label} 的 expectedCount 与 entries 数量不一致`);
  }
  return packageData;
}

/** 建立 ID 到条目的索引；重复 ID 会让补丁含义不确定，因此直接拒绝。 */
function indexEntries(entries, label) {
  const indexed = new Map();
  for (const entry of entries) {
    if (!entry || typeof entry.shikigamiId !== "string" || !entry.shikigamiId.trim()) {
      throw new Error(`${label} 存在缺少 shikigamiId 的条目`);
    }
    if (indexed.has(entry.shikigamiId)) {
      throw new Error(`${label} 存在重复式神 ID：${entry.shikigamiId}`);
    }
    indexed.set(entry.shikigamiId, entry);
  }
  return indexed;
}

/** 递归排序对象键，保证差异判断不受 JSON 原始字段顺序影响。 */
function canonicalize(value) {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, child]) => [key, canonicalize(child)]),
    );
  }
  return value;
}

/** 判断两个目录元数据是否一致；默认资料变化时必须让所有目标条目重新写入。 */
function sameDefaults(basePackage, targetPackage) {
  return (
    JSON.stringify(canonicalize(basePackage.defaultEmploymentScenes)) ===
      JSON.stringify(canonicalize(targetPackage.defaultEmploymentScenes)) &&
    JSON.stringify(canonicalize(basePackage.defaultEvidence)) ===
      JSON.stringify(canonicalize(targetPackage.defaultEvidence)) &&
    basePackage.gameVersion === targetPackage.gameVersion
  );
}

/** 计算增量条目集合，并按目标目录顺序输出，便于审阅和稳定复现。 */
function buildPatch(basePackage, targetPackage) {
  const baseEntries = indexEntries(basePackage.entries, "基础目录");
  const targetEntries = indexEntries(targetPackage.entries, "目标目录");
  const defaultsUnchanged = sameDefaults(basePackage, targetPackage);
  const upserts = targetPackage.entries.filter((entry) => {
    const baseEntry = baseEntries.get(entry.shikigamiId);
    return !defaultsUnchanged || !baseEntry || !sameJson(baseEntry, entry);
  });
  const deletes = basePackage.entries
    .filter((entry) => !targetEntries.has(entry.shikigamiId))
    .map((entry) => entry.shikigamiId);

  return {
    format: PATCH_FORMAT,
    protocolVersion: PATCH_PROTOCOL_VERSION,
    baseVersion: basePackage.version,
    version: targetPackage.version,
    gameVersion: targetPackage.gameVersion,
    expectedCount: targetPackage.expectedCount,
    defaultEmploymentScenes: targetPackage.defaultEmploymentScenes,
    defaultEvidence: targetPackage.defaultEvidence,
    upserts,
    deletes,
  };
}

/** 用稳定 JSON 比较两个条目的业务内容。 */
function sameJson(left, right) {
  return JSON.stringify(canonicalize(left)) === JSON.stringify(canonicalize(right));
}

function required(args, name) {
  const value = args[name];
  if (!value) throw new Error(`缺少参数 --${name}`);
  return value;
}

try {
  const args = parseArgs(process.argv.slice(2));
  const basePackage = await readPackage(required(args, "base"), "基础目录");
  const targetPackage = await readPackage(required(args, "target"), "目标目录");
  if (basePackage.version === targetPackage.version) {
    throw new Error("基础目录和目标目录版本不能相同");
  }
  const patch = buildPatch(basePackage, targetPackage);
  await writeFile(required(args, "output"), `${JSON.stringify(patch, null, 2)}\n`, "utf8");
  process.stdout.write(
    `已生成目录补丁：${args.output} · ${patch.baseVersion} -> ${patch.version} · ` +
      `更新 ${patch.upserts.length} 条，删除 ${patch.deletes.length} 条\n`,
  );
} catch (error) {
  // 失败时只输出参数和校验摘要，不打印完整目录内容，避免污染发布日志。
  process.stderr.write(
    `目录增量工具失败：${error instanceof Error ? error.message : String(error)}\n`,
  );
  process.exitCode = 1;
}

