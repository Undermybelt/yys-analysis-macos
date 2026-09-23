//! 规则文件与离线分享码的领域边界。
//!
//! 分享格式只承载已经通过 DSL 校验的规范 JSON，不执行任何外部代码；文件和分享码
//! 最终都回到同一个预设解析入口，确保两条导入路径具有一致的安全约束。

use super::rules::{MAX_PRESET_BYTES, RuleValidationError, ValidatedRulePreset, ValidationLimits};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

/// 离线规则分享码的格式前缀。
pub const RULE_SHARE_PREFIX: &str = "YYSR1";
/// 分享码校验和使用压缩载荷 SHA-256 的前 16 个十六进制字符。
pub const RULE_SHARE_CHECKSUM_LENGTH: usize = 16;

/// 规则文件或分享码解析失败的稳定错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleShareError {
    InvalidFormat(String),
    InvalidBase64,
    ChecksumMismatch,
    Decompression(String),
    PayloadTooLarge { actual: usize, limit: usize },
    InvalidUtf8,
    Validation(RuleValidationError),
}

impl std::fmt::Display for RuleShareError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFormat(message) => write!(formatter, "分享码格式无效：{message}"),
            Self::InvalidBase64 => write!(formatter, "分享码载荷不是合法的 Base64URL"),
            Self::ChecksumMismatch => write!(formatter, "分享码校验和不匹配"),
            Self::Decompression(message) => write!(formatter, "分享码解压失败：{message}"),
            Self::PayloadTooLarge { actual, limit } => {
                write!(
                    formatter,
                    "分享码解压后为 {actual} 字节，超过 {limit} 字节限制"
                )
            }
            Self::InvalidUtf8 => write!(formatter, "规则载荷不是合法 UTF-8"),
            Self::Validation(error) => write!(formatter, "规则预设校验失败：{error}"),
        }
    }
}

impl std::error::Error for RuleShareError {}

/// 将已校验预设编码为离线分享码。
pub fn encode_share_code(preset: &ValidatedRulePreset) -> Result<String, RuleShareError> {
    if preset.canonical_json.len() > MAX_PRESET_BYTES {
        return Err(RuleShareError::PayloadTooLarge {
            actual: preset.canonical_json.len(),
            limit: MAX_PRESET_BYTES,
        });
    }
    // 分享码只压缩规范 JSON，避免不同字段顺序在两台设备上生成不同载荷。
    let mut compressed = Vec::new();
    {
        let mut encoder = zstd::stream::write::Encoder::new(&mut compressed, 3)
            .map_err(|error| RuleShareError::Decompression(error.to_string()))?;
        encoder
            .write_all(preset.canonical_json.as_bytes())
            .map_err(|error| RuleShareError::Decompression(error.to_string()))?;
        encoder
            .finish()
            .map_err(|error| RuleShareError::Decompression(error.to_string()))?;
    }

    let checksum = sha256_hex(&compressed)[..RULE_SHARE_CHECKSUM_LENGTH].to_owned();
    Ok(format!(
        "{RULE_SHARE_PREFIX}.{}.{}",
        encode_base64_url(&compressed),
        checksum
    ))
}

/// 解析并校验离线分享码。
pub fn decode_share_code(
    code: &str,
    limits: ValidationLimits,
) -> Result<ValidatedRulePreset, RuleShareError> {
    // 复制时允许界面自动插入换行，但格式段、校验和和载荷本身不能含空白。
    let normalized = code
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let max_encoded_bytes = limits.max_preset_bytes.saturating_mul(2).max(64);
    if normalized.len() > max_encoded_bytes {
        return Err(RuleShareError::PayloadTooLarge {
            actual: normalized.len(),
            limit: max_encoded_bytes,
        });
    }
    let parts = normalized.split('.').collect::<Vec<_>>();
    if parts.len() != 3 || parts[0] != RULE_SHARE_PREFIX {
        return Err(RuleShareError::InvalidFormat(
            "应为 YYSR1.<载荷>.<校验和>".to_owned(),
        ));
    }
    if parts[2].len() != RULE_SHARE_CHECKSUM_LENGTH
        || !parts[2]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(RuleShareError::InvalidFormat(
            "校验和长度或字符无效".to_owned(),
        ));
    }

    let compressed = decode_base64_url(parts[1]).ok_or(RuleShareError::InvalidBase64)?;
    let actual_checksum = sha256_hex(&compressed)[..RULE_SHARE_CHECKSUM_LENGTH].to_owned();
    if !actual_checksum.eq_ignore_ascii_case(parts[2]) {
        return Err(RuleShareError::ChecksumMismatch);
    }

    // Decoder 直接读取到预算上限 + 1，避免恶意压缩包在完整展开前耗尽内存。
    let decoder = zstd::stream::read::Decoder::new(compressed.as_slice())
        .map_err(|error| RuleShareError::Decompression(error.to_string()))?;
    let output_limit = limits.max_preset_bytes.saturating_add(1);
    let mut payload = Vec::new();
    decoder
        .take(output_limit as u64)
        .read_to_end(&mut payload)
        .map_err(|error| RuleShareError::Decompression(error.to_string()))?;
    if payload.len() > limits.max_preset_bytes {
        return Err(RuleShareError::PayloadTooLarge {
            actual: payload.len(),
            limit: limits.max_preset_bytes,
        });
    }

    let payload = std::str::from_utf8(&payload).map_err(|_| RuleShareError::InvalidUtf8)?;
    super::rules::parse_rule_preset(payload.as_bytes(), limits).map_err(RuleShareError::Validation)
}

/// 对规范 JSON 生成可编辑的规则文件；文件不携带会阻塞用户修改的哈希字段。
pub fn export_rule_file(preset: &ValidatedRulePreset) -> Result<String, RuleShareError> {
    let value: serde_json::Value = serde_json::from_str(&preset.canonical_json)
        .map_err(|error| RuleShareError::InvalidFormat(error.to_string()))?;
    serde_json::to_string_pretty(&value)
        .map_err(|error| RuleShareError::InvalidFormat(error.to_string()))
}

/// 解析规则文件；兼容移除旧版可选哈希字段，不让用户编辑文件被完整性字段阻塞。
pub fn parse_rule_file(
    payload: &[u8],
    limits: ValidationLimits,
) -> Result<ValidatedRulePreset, RuleShareError> {
    if payload.len() > limits.max_preset_bytes {
        return Err(RuleShareError::PayloadTooLarge {
            actual: payload.len(),
            limit: limits.max_preset_bytes,
        });
    }
    let mut value: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|error| RuleShareError::InvalidFormat(error.to_string()))?;
    // 旧版导出文件可能包含 canonicalSha256；它只是历史完整性元数据，导入时直接忽略。
    if let Some(object) = value.as_object_mut() {
        object.remove("canonicalSha256");
    }
    let normalized_payload = serde_json::to_vec(&value)
        .map_err(|error| RuleShareError::InvalidFormat(error.to_string()))?;
    super::rules::parse_rule_preset(&normalized_payload, limits).map_err(RuleShareError::Validation)
}

/// 生成分享载荷校验和；只使用十六进制字符串，便于离线人工核对。
fn sha256_hex(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    hex::encode(hasher.finalize())
}

/// 编码不带填充字符的 URL 安全 Base64，避免分享码复制时出现特殊符号。
fn encode_base64_url(payload: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(payload.len().div_ceil(3) * 4);
    for chunk in payload.chunks(3) {
        let first = chunk[0] as u32;
        let second = chunk.get(1).copied().unwrap_or(0) as u32;
        let third = chunk.get(2).copied().unwrap_or(0) as u32;
        let combined = (first << 16) | (second << 8) | third;
        output.push(ALPHABET[((combined >> 18) & 0x3f) as usize] as char);
        output.push(ALPHABET[((combined >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[((combined >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[(combined & 0x3f) as usize] as char);
        }
    }
    output
}

/// 解码不带填充字符的 URL 安全 Base64，并拒绝非规范字符。
fn decode_base64_url(encoded: &str) -> Option<Vec<u8>> {
    if encoded.is_empty() || encoded.contains('=') || encoded.len() % 4 == 1 {
        return None;
    }
    let mut values = Vec::with_capacity(encoded.len());
    for character in encoded.bytes() {
        let value = match character {
            b'A'..=b'Z' => character - b'A',
            b'a'..=b'z' => character - b'a' + 26,
            b'0'..=b'9' => character - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        values.push(value);
    }

    let mut output = Vec::with_capacity(values.len() * 3 / 4);
    for chunk in values.chunks(4) {
        let first = chunk[0] as u32;
        let second = chunk.get(1).copied().unwrap_or(0) as u32;
        let third = chunk.get(2).copied().unwrap_or(0) as u32;
        let fourth = chunk.get(3).copied().unwrap_or(0) as u32;
        let combined = (first << 18) | (second << 12) | (third << 6) | fourth;
        output.push((combined >> 16) as u8);
        if chunk.len() > 2 {
            output.push((combined >> 8) as u8);
        }
        if chunk.len() > 3 {
            output.push(combined as u8);
        }
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::rules::load_default_preset;

    #[test]
    fn 默认预设可以通过分享码往返并保持规范哈希() {
        let preset = load_default_preset().expect("默认预设应可加载");
        let code = encode_share_code(&preset).expect("应生成分享码");
        let restored = decode_share_code(&code, ValidationLimits::default()).expect("应解析分享码");

        assert_eq!(restored.normalized_hash, preset.normalized_hash);
        assert_eq!(restored.canonical_json, preset.canonical_json);
    }

    #[test]
    fn 分享码校验和损坏时明确拒绝() {
        let preset = load_default_preset().expect("默认预设应可加载");
        let code = encode_share_code(&preset).expect("应生成分享码");
        let mut parts = code.split('.').map(str::to_owned).collect::<Vec<_>>();
        parts[2].replace_range(..1, "0");

        assert!(matches!(
            decode_share_code(&parts.join("."), ValidationLimits::default()),
            Err(RuleShareError::ChecksumMismatch)
        ));
    }

    #[test]
    fn 分享码解压载荷超过预算时拒绝() {
        let preset = load_default_preset().expect("默认预设应可加载");
        let code = encode_share_code(&preset).expect("应生成分享码");
        let limits = ValidationLimits {
            max_preset_bytes: 8,
            ..ValidationLimits::default()
        };

        assert!(matches!(
            decode_share_code(&code, limits),
            Err(RuleShareError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn 规则文件往返保持规范哈希() {
        let preset = load_default_preset().expect("默认预设应可加载");
        let file = export_rule_file(&preset).expect("应导出规则文件");
        let restored = parse_rule_file(file.as_bytes(), ValidationLimits::default())
            .expect("规则文件应可导入");

        assert_eq!(restored.normalized_hash, preset.normalized_hash);
        assert!(!file.contains("canonicalSha256"));
    }

    #[test]
    fn 旧版规则文件哈希变化不阻塞导入() {
        let preset = load_default_preset().expect("默认预设应可加载");
        let mut file = export_rule_file(&preset).expect("应导出规则文件");
        let mut value: serde_json::Value = serde_json::from_str(&file).expect("规则 JSON");
        value["canonicalSha256"] = serde_json::Value::String("旧版哈希".to_owned());
        value["title"] = serde_json::Value::String("用户修改后的标准".to_owned());
        file = serde_json::to_string(&value).expect("生成旧版规则文件");

        let restored = parse_rule_file(file.as_bytes(), ValidationLimits::default())
            .expect("旧版规则文件应允许导入");
        assert_eq!(restored.preset.title, "用户修改后的标准");
    }
}
