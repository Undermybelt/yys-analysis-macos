//! 发布加固共用的资源预算与受限读取工具。
//!
//! 所有来自文件、网络或读取适配器的载荷都必须先经过这里的大小边界；
//! 这样业务用例不会各自维护一套容易漂移的安全上限，也便于验收测试覆盖
//! 压缩炸弹、超大导入和下载中断等异常路径。

use crate::application::error::AppError;
use ed25519_dalek::VerifyingKey;
use std::io::Read;
use std::path::Path;

/// 单次采集允许的文件数，避免前端一次性提交过多 Vec 造成内存峰值。
pub const MAX_IMPORT_FILES: usize = 64;
/// 单个 JSON 采集载荷的原始字节上限；覆盖 10,000 枚库存的正常导出余量。
pub const MAX_IMPORT_FILE_BYTES: usize = 64 * 1024 * 1024;
/// 单个文件允许的御魂数量上限，防止异常数组让规范化任务长期占用界面线程。
pub const MAX_IMPORT_SOULS: usize = 100_000;
/// 单个文件允许的式神数量上限；桌面版当前缓存远低于该值，避免异常对象制造内存峰值。
pub const MAX_IMPORT_SHIKIGAMI: usize = 20_000;
/// 单枚御魂允许的副属性条目上限，游戏事实不会超过该边界。
pub const MAX_ATTRIBUTES_PER_SOUL: usize = 16;
/// 原始对象解压后的最大字节数，防止 zstd 压缩炸弹耗尽内存。
pub const MAX_RAW_OBJECT_BYTES: usize = 64 * 1024 * 1024;
/// 原始对象压缩文件本身的最大字节数，避免校验前一次性读取异常大文件。
pub const MAX_COMPRESSED_OBJECT_BYTES: u64 = 64 * 1024 * 1024;
/// 更新清单允许的最大大小；清单只包含元数据，不应接近该值。
pub const MAX_UPDATE_MANIFEST_BYTES: u64 = 1024 * 1024;
/// 更新构件允许的最大大小；应用安装包通常远小于该上限。
pub const MAX_UPDATE_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;

/// 统一返回资源预算错误，调用方可据此展示“载荷过大”而不是模糊的内部异常。
pub fn resource_limit(field: &str, actual: u64, limit: u64) -> AppError {
    AppError::resource_limit(field, actual, limit)
}

/// 校验字节长度；所有外部载荷在解析、压缩或写入前都应调用此方法。
pub fn ensure_bytes(field: &str, actual: usize, limit: usize) -> Result<(), AppError> {
    if actual > limit {
        return Err(resource_limit(field, actual as u64, limit as u64));
    }
    Ok(())
}

/// 读取受限文件，先检查元数据再分配 Vec，避免超大文件在校验前进入内存。
pub fn read_file_limited(path: &Path, field: &str, limit: u64) -> Result<Vec<u8>, AppError> {
    let size = std::fs::metadata(path)
        .map_err(|error| AppError::io("读取载荷元数据", &error))?
        .len();
    if size > limit {
        return Err(resource_limit(field, size, limit));
    }
    let bytes = std::fs::read(path).map_err(|error| AppError::io("读取载荷文件", &error))?;
    if bytes.len() as u64 > limit {
        return Err(resource_limit(field, bytes.len() as u64, limit));
    }
    Ok(bytes)
}

/// 解压 zstd 并限制输出长度；多读一个字节以识别刚好越界的压缩流。
pub fn decode_zstd_limited(compressed: &[u8], limit: usize) -> Result<Vec<u8>, AppError> {
    let mut decoder = zstd::stream::Decoder::new(compressed)
        .map_err(|error| AppError::internal(format!("zstd 解压初始化失败：{error}")))?;
    let mut raw = Vec::new();
    decoder
        .by_ref()
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut raw)
        .map_err(|error| AppError::internal(format!("zstd 解压失败：{error}")))?;
    ensure_bytes("decompressedBytes", raw.len(), limit)?;
    Ok(raw)
}

/// 内置发行公钥是安装包验证签名的公开信任根；私钥仍只保留在离线目录。
const BUILTIN_RELEASE_PUBLIC_KEY_HEX: &str = include_str!("../../release-public-key.hex");

/// 解析 32 字节 Ed25519 公钥；长度或编码非法时拒绝加载。
fn parse_release_public_key(encoded: &str) -> Option<VerifyingKey> {
    let bytes = hex::decode(encoded.trim()).ok()?;
    let bytes: [u8; 32] = bytes.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

/// 优先使用构建时注入的公钥，遗漏或填错时回退到仓库内置公钥。
/// 这保证直接构建和标准发布构建都能验证同一条签名更新链。
pub fn trusted_release_public_key() -> Option<VerifyingKey> {
    option_env!("YYS_RELEASE_ED25519_PUBLIC_KEY_HEX")
        .and_then(parse_release_public_key)
        .or_else(|| parse_release_public_key(BUILTIN_RELEASE_PUBLIC_KEY_HEX))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归约束：直接构建和发布构建都必须内置同一个发行信任根，
    /// 避免遗漏环境变量时生成永远无法自动更新的安装包。
    #[test]
    fn 未注入环境变量时仍有发行公钥() {
        assert!(trusted_release_public_key().is_some());
    }

    #[test]
    fn 压缩流解压超过预算时拒绝继续读取() {
        let compressed = zstd::stream::encode_all(&b"0123456789"[..], 1).expect("压缩夹具");
        let error = decode_zstd_limited(&compressed, 4).expect_err("应触发解压预算");
        assert!(matches!(
            error.code,
            crate::application::error::ErrorCode::ResourceLimitExceeded
        ));
    }

    #[test]
    fn 文件字节预算在读取前生效() {
        let directory = std::env::temp_dir().join("yys-analysis-security-budget");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("创建测试目录");
        let path = directory.join("oversized.bin");
        std::fs::write(&path, b"0123456789").expect("写入测试文件");

        let error = read_file_limited(&path, "fixtureBytes", 4).expect_err("应拒绝超大文件");
        assert!(matches!(
            error.code,
            crate::application::error::ErrorCode::ResourceLimitExceeded
        ));
        let _ = std::fs::remove_dir_all(&directory);
    }
}
