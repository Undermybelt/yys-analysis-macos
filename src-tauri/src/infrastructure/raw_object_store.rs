//! 内容寻址原始对象存储。
//!
//! 原始快照字节经过 zstd 压缩后按 SHA-256 分片保存，路径为
//! `snapshots/sha256/<ab>/<cd>/<full-hash>.json.zst`。相同内容只保存一份。
//! 读取时重新解压并校验哈希，不匹配的文件移入隔离区，避免进入御魂库存投影。

use crate::application::error::AppError;
use crate::application::security::{
    MAX_COMPRESSED_OBJECT_BYTES, MAX_RAW_OBJECT_BYTES, decode_zstd_limited, ensure_bytes,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 原始对象存储，负责内容寻址文件的写入、读取、校验与隔离。
#[derive(Clone, Debug)]
pub struct RawObjectStore {
    snapshot_root: PathBuf,
    quarantine_directory: PathBuf,
    temp_directory: PathBuf,
    // 进程内串行化同一对象的写入，避免两个导入同时操作同一临时文件。
    write_lock: Arc<Mutex<()>>,
}

/// 成功落盘后的对象事实，供数据库记录使用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredObject {
    pub raw_size: u64,
    pub stored_size: u64,
}

/// 校验结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// 文件存在且哈希一致。
    Verified,
    /// 文件不存在（对象行存在但文件缺失）。
    Missing,
}

impl RawObjectStore {
    /// 创建原始对象存储。
    pub fn new(
        snapshot_root: PathBuf,
        temp_directory: PathBuf,
        quarantine_directory: PathBuf,
    ) -> Self {
        Self {
            snapshot_root,
            quarantine_directory,
            temp_directory,
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    /// 计算字节的 SHA-256 十六进制摘要。
    pub fn content_hash(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex::encode(hasher.finalize())
    }

    /// 由哈希推导分片相对路径，保证路径始终在快照根目录内。
    pub fn relative_path_for(hash: &str) -> String {
        format!("{}/{}/{}.json.zst", &hash[0..2], &hash[2..4], hash)
    }

    /// 校验外部传入的 SHA-256，并生成安全的相对路径。
    ///
    /// 该边界必须先校验长度和十六进制字符，避免验证命令因切片越界崩溃，
    /// 也避免把用户输入当成路径片段参与文件访问。
    fn checked_relative_path_for(hash: &str) -> Result<String, AppError> {
        let valid = hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !valid {
            return Err(AppError::invalid_argument(
                "sha256",
                "SHA-256 必须是 64 位十六进制字符串",
            ));
        }
        Ok(Self::relative_path_for(hash))
    }

    /// 保存原始字节：压缩后原子写入内容寻址位置。
    /// 相同内容已存在时直接复用，不重复写入。
    pub fn store(
        &self,
        bytes: &[u8],
        media_type: &str,
    ) -> Result<(String, StoredObject), AppError> {
        // 原始对象来自导入或读取适配器；先限制输入规模，避免压缩本身成为资源消耗点。
        ensure_bytes("rawObjectBytes", bytes.len(), MAX_RAW_OBJECT_BYTES)?;
        // 内容寻址写入需要跨“检查已有文件—写临时文件—原子改名”保持一致。
        let _write_guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let hash = Self::content_hash(bytes);
        let relative_path = Self::relative_path_for(&hash);
        let destination = self.snapshot_root.join(&relative_path);

        if destination.exists() {
            // 不能只相信路径存在：崩溃或外部篡改可能留下同名损坏文件。
            // 校验失败时先隔离旧文件，再用当前原始字节重建对象。
            match self.read_verified(&destination, &hash) {
                Ok(stored_size) => {
                    return Ok((
                        hash,
                        StoredObject {
                            raw_size: bytes.len() as u64,
                            stored_size,
                        },
                    ));
                }
                Err(_) => {
                    self.quarantine(&destination)?;
                }
            }
        }

        // 压缩（zstd 级别 3，兼顾速度与压缩率）；encode_all 直接返回压缩字节。
        let compressed = zstd::stream::encode_all(bytes, 3)
            .map_err(|error| AppError::internal(format!("zstd 压缩失败：{error}")))?;

        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| AppError::io("创建对象分片目录", &error))?;
        }

        // 先写临时文件再原子改名，避免半成品文件被当作有效对象。
        // 临时文件带随机后缀，避免上次异常退出的残留文件或并发导入互相覆盖。
        let temp_path = self
            .temp_directory
            .join(format!("raw-{hash}-{}.zst", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&self.temp_directory)
            .map_err(|error| AppError::io("创建临时目录", &error))?;
        if let Err(error) = std::fs::write(&temp_path, &compressed) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(AppError::io("写入临时对象文件", &error));
        }
        if let Err(error) = std::fs::rename(&temp_path, &destination) {
            // 其他进程可能已经先写入相同内容；确认其有效后即可复用。
            if destination.exists() {
                let _ = std::fs::remove_file(&temp_path);
                if let Ok(stored_size) = self.read_verified(&destination, &hash) {
                    return Ok((
                        hash,
                        StoredObject {
                            raw_size: bytes.len() as u64,
                            stored_size,
                        },
                    ));
                }
            }
            let _ = std::fs::remove_file(&temp_path);
            return Err(AppError::io("落盘内容寻址对象", &error));
        }

        tracing::info!(
            sha256 = %hash,
            raw_size = bytes.len(),
            stored_size = compressed.len(),
            media_type,
            "原始对象已保存"
        );

        Ok((
            hash,
            StoredObject {
                raw_size: bytes.len() as u64,
                stored_size: compressed.len() as u64,
            },
        ))
    }

    /// 读取并解压原始字节，校验哈希；不匹配时移入隔离区并返回错误。
    pub fn load(&self, sha256: &str) -> Result<Vec<u8>, AppError> {
        let _write_guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let relative_path = Self::checked_relative_path_for(sha256)?;
        let path = self.snapshot_root.join(&relative_path);

        if !path.exists() {
            return Err(AppError::internal(format!(
                "原始对象文件缺失：{relative_path}"
            )));
        }

        let compressed = match self.read_compressed(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                // 超大或无法读取的活动对象不应继续留在可用目录中，保留现场并停止使用。
                let _ = self.quarantine(&path);
                return Err(error);
            }
        };

        let raw = match decode_zstd_limited(&compressed, MAX_RAW_OBJECT_BYTES) {
            Ok(raw) => raw,
            Err(error) => {
                // 压缩流损坏同样不能留在可用对象目录中，保留现场后统一上报校验错误。
                self.quarantine(&path)?;
                return Err(AppError::checksum_mismatch(
                    &relative_path,
                    sha256,
                    &format!("invalid-zstd: {error}"),
                ));
            }
        };

        let actual = Self::content_hash(&raw);
        if actual != sha256 {
            self.quarantine(&path)?;
            return Err(AppError::checksum_mismatch(&relative_path, sha256, &actual));
        }

        Ok(raw)
    }

    /// 校验已存在对象的哈希完整性。
    pub fn verify(&self, sha256: &str) -> Result<VerifyOutcome, AppError> {
        let _write_guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let relative_path = Self::checked_relative_path_for(sha256)?;
        let path = self.snapshot_root.join(&relative_path);

        if !path.exists() {
            return Ok(VerifyOutcome::Missing);
        }

        let compressed = match self.read_compressed(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = self.quarantine(&path);
                return Err(error);
            }
        };
        let raw = match decode_zstd_limited(&compressed, MAX_RAW_OBJECT_BYTES) {
            Ok(raw) => raw,
            Err(error) => {
                self.quarantine(&path)?;
                return Err(AppError::checksum_mismatch(
                    &relative_path,
                    sha256,
                    &format!("invalid-zstd: {error}"),
                ));
            }
        };
        let actual = Self::content_hash(&raw);

        if actual != sha256 {
            self.quarantine(&path)?;
            return Err(AppError::checksum_mismatch(&relative_path, sha256, &actual));
        }

        Ok(VerifyOutcome::Verified)
    }

    /// 将损坏文件移入隔离区，保留现场供诊断。
    fn quarantine(&self, path: &Path) -> Result<(), AppError> {
        std::fs::create_dir_all(&self.quarantine_directory)
            .map_err(|error| AppError::io("创建隔离目录", &error))?;

        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown.zst");
        // 隔离目录可能已有同名现场，使用随机后缀确保原文件一定会离开活动目录。
        let quarantined = self
            .quarantine_directory
            .join(format!("{file_name}.{}.quarantine", uuid::Uuid::new_v4()));

        if let Err(error) = std::fs::rename(path, &quarantined) {
            // 隔离失败不阻断错误上报，但记录日志。
            tracing::warn!(
                source = %path.display(),
                target = %quarantined.display(),
                reason = %error,
                "隔离损坏对象文件失败"
            );
        } else {
            tracing::error!(
                quarantined = %quarantined.display(),
                "对象文件哈希不匹配，已移入隔离区"
            );
        }
        Ok(())
    }

    /// 读取压缩对象并同时校验原始内容哈希，成功时返回压缩文件大小。
    /// 调用方负责在失败后隔离文件；该方法不改变文件系统状态。
    fn read_verified(&self, path: &Path, expected_hash: &str) -> Result<u64, AppError> {
        let compressed = self.read_compressed(path)?;
        let raw = decode_zstd_limited(&compressed, MAX_RAW_OBJECT_BYTES)?;
        let actual = Self::content_hash(&raw);
        if actual != expected_hash {
            return Err(AppError::checksum_mismatch(
                &path.display().to_string(),
                expected_hash,
                &actual,
            ));
        }
        Ok(compressed.len() as u64)
    }

    /// 校验压缩文件大小后再读取，避免文件系统中的异常大文件直接进入内存。
    fn read_compressed(&self, path: &Path) -> Result<Vec<u8>, AppError> {
        let size = std::fs::metadata(path)
            .map_err(|error| AppError::io("读取原始对象元数据", &error))?
            .len();
        if size > MAX_COMPRESSED_OBJECT_BYTES {
            return Err(AppError::resource_limit(
                "compressedObjectBytes",
                size,
                MAX_COMPRESSED_OBJECT_BYTES,
            ));
        }
        let bytes =
            std::fs::read(path).map_err(|error| AppError::io("读取原始对象文件", &error))?;
        if bytes.len() as u64 > MAX_COMPRESSED_OBJECT_BYTES {
            return Err(AppError::resource_limit(
                "compressedObjectBytes",
                bytes.len() as u64,
                MAX_COMPRESSED_OBJECT_BYTES,
            ));
        }
        Ok(bytes)
    }

    /// 统计当前内容寻址目录中的对象数量与字节数。
    pub fn stats(&self) -> Result<(u64, u64), AppError> {
        let mut count = 0u64;
        let mut bytes = 0u64;
        if !self.snapshot_root.exists() {
            return Ok((0, 0));
        }
        for entry in walkdir_lite(&self.snapshot_root)
            .map_err(|error| AppError::io("遍历对象目录", &error))?
        {
            // `Path::ends_with` 比较路径组件而非文件扩展名；这里必须按扩展名统计对象文件。
            if entry
                .extension()
                .is_some_and(|extension| extension == "zst")
            {
                count += 1;
                if let Ok(meta) = std::fs::metadata(&entry) {
                    bytes += meta.len();
                }
            }
        }
        Ok((count, bytes))
    }
}

/// 简单递归遍历，避免引入额外依赖。
fn walkdir_lite(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                result.push(path);
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{RawObjectStore, VerifyOutcome};

    fn store(dir: &str) -> RawObjectStore {
        let root = std::env::temp_dir().join("yys-analysis-test-raw").join(dir);
        let _ = std::fs::remove_dir_all(&root);
        RawObjectStore::new(
            root.join("snapshots/sha256"),
            root.join("temp"),
            root.join("quarantine"),
        )
    }

    #[test]
    fn 相同内容只落盘一份() {
        let store = store("dedup");
        let payload = br#"{"version":1,"hero_equips":[]}"#;

        let (hash1, _) = store.store(payload, "application/json").expect("首次保存");
        let (hash2, _) = store.store(payload, "application/json").expect("再次保存");

        assert_eq!(hash1, hash2, "相同内容哈希应一致");
        let (count, bytes) = store.stats().expect("统计");
        assert_eq!(count, 1, "相同内容应只保存一个对象文件");
        assert!(bytes > 0);
    }

    #[test]
    fn 不同内容生成不同哈希与文件() {
        let store = store("distinct");
        let (hash1, _) = store
            .store(b"payload-one", "application/json")
            .expect("保存一");
        let (hash2, _) = store
            .store(b"payload-two", "application/json")
            .expect("保存二");

        assert_ne!(hash1, hash2);
        let (count, _) = store.stats().expect("统计");
        assert_eq!(count, 2);
    }

    #[test]
    fn 读取往返保持字节一致() {
        let store = store("roundtrip");
        let payload: Vec<u8> = (0..=255u8).collect(); // 覆盖所有字节值
        let (hash, _) = store.store(&payload, "application/json").expect("保存");

        let loaded = store.load(&hash).expect("读取");
        assert_eq!(loaded, payload, "解压后应恢复原始字节");
    }

    #[test]
    fn 损坏文件被隔离并上报校验失败() {
        let store = store("corrupt");
        let payload = b"intact-payload";
        let (hash, _) = store.store(payload, "application/json").expect("保存");

        // 篡改文件内容
        let path = store
            .snapshot_root
            .join(RawObjectStore::relative_path_for(&hash));
        std::fs::write(&path, b"corrupted-bytes").expect("篡改文件");

        let result = store.load(&hash);
        assert!(result.is_err(), "损坏对象应读取失败");

        // 文件已经进入隔离区，后续校验应明确报告“对象文件缺失”。
        let verify = store.verify(&hash);
        assert!(
            matches!(verify, Ok(VerifyOutcome::Missing)),
            "隔离后的对象应报告缺失"
        );

        // 文件应已移入隔离区
        let (count, _) = store.stats().expect("统计");
        assert_eq!(count, 0, "损坏文件应不再位于内容寻址目录");
    }

    #[test]
    fn 未损坏对象校验通过() {
        let store = store("verify-ok");
        let payload = b"verify-me";
        let (hash, _) = store.store(payload, "application/json").expect("保存");

        match store.verify(&hash).expect("校验") {
            VerifyOutcome::Verified => {}
            other => panic!("应校验通过，实际 {other:?}"),
        }
    }

    /// 外部哈希格式错误时返回参数错误，而不是因路径切片越界导致进程崩溃。
    #[test]
    fn 非法哈希返回参数错误() {
        let store = store("invalid-hash");
        let result = store.verify("not-a-sha256");
        assert!(result.is_err(), "非法哈希应被拒绝");
        let error = result.expect_err("应返回结构化参数错误");
        assert!(matches!(
            error.code,
            crate::application::error::ErrorCode::InvalidArgument
        ));
    }
}
