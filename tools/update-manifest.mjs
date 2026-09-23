#!/usr/bin/env node

/**
 * 生成 Gitee Release 使用的签名更新清单。
 * 私钥只从命令行指定的离线文件读取，不写入仓库、产物或日志。
 */

import { createHash, createPrivateKey, createPublicKey, sign } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";

const PROTOCOL_VERSION = 1;
const MANIFEST_FORMAT = "yys-signed-update-manifest";
const CATALOG_PATCH_KIND = "patch";
const UPDATE_CHANNELS = new Set(["stable", "test"]);

/** 将参数列表转为键值对象，拒绝未知的隐式位置参数。 */
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

/** 与 Rust 更新模块保持逐行 JSON 标量编码一致，避免发布端与客户端签名语义漂移。 */
function canonicalPayload(manifest) {
  const fields =
    manifest.packageType === "adapter"
      ? [
          manifest.packageId,
          manifest.version,
          manifest.channel,
          manifest.minAppVersion,
          manifest.compatibleGameVersion ?? "*",
          manifest.contentSha256,
          manifest.signerKeyId,
          manifest.adapterCapabilities.readSoulData,
          manifest.adapterCapabilities.writeMemory,
          manifest.adapterCapabilities.simulateInput,
          manifest.adapterCapabilities.executeRemoteCode,
        ]
      : [
          manifest.format,
          manifest.protocolVersion,
          manifest.channel,
          manifest.packageType,
          manifest.packageId,
          manifest.version,
          manifest.minAppVersion,
          manifest.compatibleGameVersion ?? "",
          manifest.contentSha256.toLowerCase(),
          manifest.contentSize,
          manifest.artifactName,
          manifest.signerKeyId,
          manifest.releaseNotes ?? "",
        ];
  // 完整包沿用既有字段顺序；目录补丁才追加增量字段，避免旧清单签名失效。
  if (manifest.packageType !== "adapter" && manifest.deltaKind) {
    fields.push(
      manifest.deltaKind,
      manifest.baseVersion ?? "",
      manifest.baseContentSha256 ?? "",
      manifest.resultContentSha256 ?? "",
    );
  }
  return manifest.packageType === "adapter"
    ? fields.map((value) => String(value)).join("\n")
    : fields.map((value) => JSON.stringify(String(value))).join("\n");
}

/** 生成单包清单；签名使用离线 Ed25519 私钥文件，私钥内容不会打印。 */
async function createManifest(args) {
  const payload = await readFile(required(args, "payload"));
  const privateKeyBytes = await readFile(required(args, "private-key-file"));
  const privateKey = createPrivateKey(privateKeyBytes);
  const publicKey = createPublicKey(privateKey).export({ format: "der", type: "spki" });
  const signerKeyId = required(args, "signer-key-id");
  const packageType = required(args, "package-type");
  const channel = args.channel ?? "stable";
  if (!UPDATE_CHANNELS.has(channel)) {
    throw new Error("--channel 必须是 stable 或 test");
  }
  const deltaKind = args["delta-kind"] ?? null;
  const baseVersion = args["base-version"] ?? null;
  const baseContentSha256 = args["base-sha256"] ?? null;
  const resultContentSha256 = args["result-sha256"] ?? null;
  validateDeltaArgs({
    packageType,
    deltaKind,
    baseVersion,
    baseContentSha256,
    resultContentSha256,
  });
  const manifest = {
    format: MANIFEST_FORMAT,
    protocolVersion: PROTOCOL_VERSION,
    channel,
    packageType,
    packageId: required(args, "package-id"),
    version: required(args, "version"),
    minAppVersion: required(args, "min-app-version"),
    compatibleGameVersion: args["game-version"] ?? null,
    contentSha256: createHash("sha256").update(payload).digest("hex"),
    contentSize: payload.length,
    artifactName: required(args, "artifact-name"),
    signerKeyId,
    signature: "",
    adapterCapabilities:
      packageType === "adapter"
        ? {
            readSoulData: true,
            writeMemory: false,
            simulateInput: false,
            executeRemoteCode: false,
          }
        : null,
    releaseNotes: args["release-notes"] ?? "",
    deltaKind,
    baseVersion,
    baseContentSha256,
    resultContentSha256,
  };
  manifest.signature = sign(
    null,
    Buffer.from(canonicalPayload(manifest)),
    privateKey,
  ).toString("hex");
  // 只在清单合法时写出，公钥指纹用于发布日志之外的人工核对，不包含私钥。
  const publicKeyFingerprint = createHash("sha256")
    .update(publicKey)
    .digest("hex")
    .slice(0, 16);
  await writeFile(
    required(args, "output"),
    `${JSON.stringify(manifest, null, 2)}\n`,
    "utf8",
  );
  process.stdout.write(
    `已生成清单：${args.output} · 公钥指纹 ${publicKeyFingerprint}\n`,
  );
}

function required(args, name) {
  const value = args[name];
  if (!value) throw new Error(`缺少参数 --${name}`);
  return value;
}

/** 校验目录增量清单参数，防止生成客户端无法应用或签名语义不完整的发布文件。 */
function validateDeltaArgs({
  packageType,
  deltaKind,
  baseVersion,
  baseContentSha256,
  resultContentSha256,
}) {
  const hasHash = baseContentSha256 || resultContentSha256;
  if (!deltaKind && (baseVersion || hasHash)) {
    throw new Error("完整更新不能携带目录增量字段");
  }
  if (!deltaKind) return;
  if (packageType !== "catalog" || deltaKind !== CATALOG_PATCH_KIND) {
    throw new Error("只有 catalog 包可以使用 --delta-kind patch");
  }
  if (!baseVersion?.trim()) throw new Error("目录补丁必须提供 --base-version");
  for (const [name, value] of [
    ["base-sha256", baseContentSha256],
    ["result-sha256", resultContentSha256],
  ]) {
    if (value && !/^[0-9a-f]{64}$/i.test(value)) {
      throw new Error(`--${name} 必须是 64 位十六进制 SHA-256`);
    }
  }
}

const [command, ...rest] = process.argv.slice(2);
try {
  if (command === "create") await createManifest(parseArgs(rest));
  else throw new Error("用法：create");
} catch (error) {
  // 错误只输出操作摘要，不输出私钥、清单签名原文或载荷内容。
  process.stderr.write(
    `更新清单工具失败：${error instanceof Error ? error.message : String(error)}\n`,
  );
  process.exitCode = 1;
}
