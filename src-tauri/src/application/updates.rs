//! 签名更新应用边界。
//!
//! 更新流程严格分成“拉取候选、校验、暂存、备份、激活、记录”六步。镜像地址只负责
//! 分发，清单签名和载荷哈希才是信任边界；任何一步失败都不会覆盖活动版本。

use crate::application::{
    error::AppError,
    security::{
        MAX_UPDATE_ARTIFACT_BYTES, MAX_UPDATE_MANIFEST_BYTES, resource_limit,
        trusted_release_public_key,
    },
    services::AppServices,
    use_cases::{BackupUseCase, CatalogUseCase},
};
use crate::domain::InstalledPackage;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

/// 历史适配器包的能力声明；macOS 版不再执行适配器，只保留清单字段以便校验旧更新包。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdapterCapabilities {
    pub read_soul_data: bool,
    #[serde(default)]
    pub write_memory: bool,
    #[serde(default)]
    pub simulate_input: bool,
    #[serde(default)]
    pub execute_remote_code: bool,
}

/// 历史适配器包清单；字段保持旧协议，避免已签名更新包无法通过校验。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdapterManifest {
    pub package_id: String,
    pub version: String,
    pub min_app_version: String,
    pub supported_game_versions: Vec<String>,
    pub content_sha256: String,
    pub signature: String,
    pub signer_key_id: String,
    pub capabilities: AdapterCapabilities,
}


/// 发布清单协议版本；变更签名字段时必须升级协议，避免新旧客户端误解字段。
pub const UPDATE_PROTOCOL_VERSION: u32 = 1;
/// 正式构建默认只检查稳定通道；测试构建通过编译环境切换到测试通道。
const STABLE_CHANNEL: &str = "stable";
/// 测试包使用独立通道，防止测试 Release 被正式用户误检测到。
const TEST_CHANNEL: &str = "test";
/// 式神目录增量索引的协议版本；索引本身只负责选择候选清单，最终信任仍来自签名清单。
const CATALOG_INDEX_PROTOCOL_VERSION: u32 = 1;
/// 式神目录增量载荷的协议版本；变更操作语义时必须升级，避免旧客户端误应用补丁。
pub const SHIKIGAMI_PATCH_PROTOCOL_VERSION: u32 = 1;
/// 目录补丁清单使用的载荷标记。
const CATALOG_DELTA_KIND_PATCH: &str = "patch";
/// 清单和构件的网络请求最长等待时间，防止更新检查长期占用后台资源。
const UPDATE_NETWORK_TIMEOUT_SECONDS: u64 = 20;
/// 更新助手等待旧进程退出的最长时间，超时后回退启动旧版本，避免界面永久停在“重启中”。
const UPDATE_HELPER_WAIT_TIMEOUT_SECONDS: u64 = 10;
/// 维护者发布源的默认 Gitee Release 列表；客户端按清单通道和版本自行选择候选。
const DEFAULT_GITEE_RELEASE_API_URL: &str =
    "https://gitee.com/api/v5/repos/tigerdonotcry/yys-analysis/releases?per_page=20";

/// 可发布的更新通道；通道同时进入签名清单和客户端构建配置。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    /// 面向普通用户的稳定发布通道。
    #[default]
    Stable,
    /// 面向更新链路验证的测试发布通道。
    Test,
}

impl UpdateChannel {
    /// 返回清单和前端接口使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => STABLE_CHANNEL,
            Self::Test => TEST_CHANNEL,
        }
    }

    /// 解析外部清单中的通道值，未知值必须拒绝而不能降级到稳定通道。
    fn parse(value: &str) -> Option<Self> {
        match value {
            STABLE_CHANNEL => Some(Self::Stable),
            TEST_CHANNEL => Some(Self::Test),
            _ => None,
        }
    }
}

/// 读取当前构建注入的通道；未注入时默认 stable，保证旧构建行为安全。
pub fn build_update_channel() -> UpdateChannel {
    match option_env!("YYS_UPDATE_CHANNEL") {
        Some(TEST_CHANNEL) => UpdateChannel::Test,
        _ => UpdateChannel::Stable,
    }
}

/// 更新源策略固定为 Gitee；保留来源枚举中的旧值仅用于读取历史安装记录。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateSourceMode {
    /// 检查维护者 Gitee Release 中的最新稳定版。
    #[default]
    Gitee,
}

/// 发布镜像身份；它只用于展示和故障报告，不参与信任判断。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateMirror {
    Github,
    Gitee,
}

impl UpdateMirror {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitee => "gitee",
        }
    }
}

/// 可独立更新的载荷类型；应用包和三类数据包不共享活动指针。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageType {
    Application,
    Catalog,
    Rules,
    Adapter,
}

impl PackageType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Catalog => "catalog",
            Self::Rules => "rules",
            Self::Adapter => "adapter",
        }
    }

    /// 将磁盘目录固定映射为受控的四个子目录，拒绝把清单字段当作任意路径。
    fn directory_name(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Catalog => "catalog",
            Self::Rules => "rules",
            Self::Adapter => "adapters",
        }
    }
}

/// 维护者离线签名的发布清单；清单不得包含私钥、脚本或任意待执行命令。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseManifest {
    pub format: String,
    pub protocol_version: u32,
    pub channel: String,
    pub package_type: PackageType,
    pub package_id: String,
    pub version: String,
    pub min_app_version: String,
    pub compatible_game_version: Option<String>,
    pub content_sha256: String,
    pub content_size: u64,
    pub artifact_name: String,
    pub signer_key_id: String,
    pub signature: String,
    /// 读取适配器额外声明只读能力；其他包类型必须保持为空。
    #[serde(default)]
    pub adapter_capabilities: Option<AdapterCapabilities>,
    #[serde(default)]
    pub release_notes: String,
    /// 只有目录补丁填写；完整目录包保持为空以兼容既有清单签名语义。
    #[serde(default)]
    pub delta_kind: Option<String>,
    /// 增量目录要求的活动基础版本；客户端必须严格匹配后才允许安装。
    #[serde(default)]
    pub base_version: Option<String>,
    /// 可选的基础目录规范哈希，用于阻止同版本不同内容被错误套用。
    #[serde(default)]
    pub base_content_sha256: Option<String>,
    /// 可选的应用补丁后目录规范哈希；提供时必须在数据库安装前复核。
    #[serde(default)]
    pub result_content_sha256: Option<String>,
}

/// 目录更新索引只描述相邻补丁和完整包的文件名，不承载任何可执行内容。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogUpdateIndex {
    format: String,
    protocol_version: u32,
    latest_version: String,
    releases: Vec<CatalogIndexEntry>,
}

/// 目录索引中的一个候选清单；文件名必须是同一镜像目录下的单文件名。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogIndexEntry {
    version: String,
    kind: String,
    base_version: Option<String>,
    manifest_name: String,
}

/// Gitee Release API 返回的附件摘要；只读取名称和 HTTPS 下载地址，不信任其余字段。
#[derive(Clone, Debug, Deserialize)]
struct GiteeReleaseMetadata {
    assets: Vec<GiteeReleaseAsset>,
}

/// Gitee Release 的单个附件；下载地址必须再次经过 HTTPS 校验。
#[derive(Clone, Debug, Deserialize)]
struct GiteeReleaseAsset {
    name: String,
    browser_download_url: String,
}

impl GiteeReleaseMetadata {
    /// 按附件文件名定位清单或载荷，避免依赖 Gitee 页面 HTML 和附件顺序。
    fn asset_url(&self, name: &str) -> Result<String, AppError> {
        let asset = self
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .ok_or_else(|| {
                AppError::update_source_unavailable("gitee", format!("Release 缺少附件：{name}"))
            })?;
        if !asset.browser_download_url.starts_with("https://") {
            return Err(AppError::update_rejected("Gitee 附件地址不是 HTTPS"));
        }
        Ok(asset.browser_download_url.clone())
    }
}

/// 镜像下载后的候选对象；在进入选择器前不得写入活动目录。
#[derive(Clone, Debug)]
pub struct MirrorCandidate {
    pub source: UpdateMirror,
    pub manifest: ReleaseManifest,
    pub payload: Vec<u8>,
    /// 已校验的远程构件地址；启动检查只保存它，用户确认后才下载载荷。
    pub artifact_url: Option<String>,
}

/// 已通过全部安全校验的候选对象；此类型才允许进入暂存器。
#[derive(Clone, Debug)]
pub struct VerifiedRelease {
    pub source: UpdateMirror,
    pub manifest: ReleaseManifest,
    pub payload: Vec<u8>,
    pub signature_fingerprint: String,
    /// 应用包启动检查阶段保留的构件地址；完整下载后仍必须重新校验清单哈希。
    pub artifact_url: Option<String>,
}

/// 更新中心用于展示每个镜像结果的摘要，不暴露清单签名正文和载荷内容。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorCheckSummary {
    pub source: UpdateMirror,
    pub status: String,
    pub version: Option<String>,
    pub content_sha256: Option<String>,
    pub signer_key_id: Option<String>,
    pub message: Option<String>,
}

/// 被选中的更新摘要；实际载荷只保存在 Rust 暂存目录。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCandidateSummary {
    pub source: UpdateMirror,
    pub channel: UpdateChannel,
    pub package_type: PackageType,
    pub package_id: String,
    pub version: String,
    pub content_sha256: String,
    pub content_size: u64,
    pub signer_key_id: String,
    pub signature_fingerprint: String,
    pub release_notes: String,
    pub staged_update_id: String,
}

/// 更新检查与暂存结果；无更新时 selected 为空，镜像状态仍可用于诊断故障切换。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub source_mode: UpdateSourceMode,
    pub channel: UpdateChannel,
    pub package_type: PackageType,
    pub current_version: String,
    pub selected: Option<UpdateCandidateSummary>,
    pub mirrors: Vec<MirrorCheckSummary>,
}

/// 暂存更新安装后的结果；应用包由独立助手等待主进程退出后安装，数据包仍按页面手动激活。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInstallResult {
    pub package_type: PackageType,
    pub channel: UpdateChannel,
    pub version: String,
    pub state: String,
    pub backup_id: Option<String>,
    pub content_sha256: String,
    pub message: String,
}

/// 从配置目录读取的 Gitee 发布地址；旧双源字段只为兼容历史配置，当前不会被使用。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSourceConfig {
    /// Gitee Release 列表 API，返回清单与构件附件的下载地址。
    #[serde(default)]
    pub gitee_release_api_url: Option<String>,
    /// 测试通道 Release API；默认与稳定通道共用仓库列表，再由签名通道筛选。
    #[serde(default)]
    pub gitee_test_release_api_url: Option<String>,
    /// 旧版直接清单地址；仅作为旧配置兼容字段，新的默认配置不再写入。
    #[serde(default)]
    pub gitee_manifest_url: Option<String>,
    /// 旧版目录索引地址；新的 Gitee Release 流程从附件中读取 catalog-index.json。
    #[serde(default)]
    pub gitee_catalog_index_url: Option<String>,
    /// 已停用的旧镜像配置，保留字段避免升级后因旧配置反序列化失败。
    #[serde(default)]
    pub github_manifest_url: Option<String>,
    #[serde(default)]
    pub github_catalog_index_url: Option<String>,
}

impl UpdateSourceConfig {
    /// 读取用户配置；配置文件损坏时拒绝更新检查，避免悄悄访问未知地址。
    pub fn load(config_directory: &Path) -> Result<Self, AppError> {
        let path = config_directory.join("update-sources.json");
        if !path.exists() {
            return Ok(Self {
                gitee_release_api_url: Some(DEFAULT_GITEE_RELEASE_API_URL.to_owned()),
                gitee_test_release_api_url: Some(DEFAULT_GITEE_RELEASE_API_URL.to_owned()),
                ..Self::default()
            });
        }
        let bytes = fs::read(&path).map_err(|error| AppError::io("读取更新源配置", &error))?;
        let mut config: Self = serde_json::from_slice(&bytes)
            .map_err(|error| AppError::update_rejected(format!("更新源配置格式无效：{error}")))?;
        // 升级旧版本后自动补上 Gitee 列表 API，避免旧的 latest 地址把测试包挡住。
        if config.gitee_release_api_url.is_none()
            || config
                .gitee_release_api_url
                .as_deref()
                .is_some_and(|url| url.ends_with("/releases/latest"))
        {
            config.gitee_release_api_url = Some(DEFAULT_GITEE_RELEASE_API_URL.to_owned());
        }
        if config.gitee_test_release_api_url.is_none() {
            config.gitee_test_release_api_url = Some(DEFAULT_GITEE_RELEASE_API_URL.to_owned());
        }
        Ok(config)
    }

    /// 当前所有包类型共用 Gitee Release 列表；通道只决定清单筛选范围。
    fn url(
        &self,
        mirror: UpdateMirror,
        _package_type: PackageType,
        channel: UpdateChannel,
    ) -> Option<&str> {
        if mirror != UpdateMirror::Gitee {
            return None;
        }
        match channel {
            UpdateChannel::Stable => self
                .gitee_release_api_url
                .as_deref()
                .or(self.gitee_manifest_url.as_deref()),
            UpdateChannel::Test => self
                .gitee_test_release_api_url
                .as_deref()
                .or(self.gitee_release_api_url.as_deref()),
        }
    }
}

/// 更新下载边界；生产实现使用系统 HTTPS 客户端，测试可注入 file:// 镜像。
pub trait UpdateSourceClient: Send + Sync {
    /// 下载并解析一个镜像的清单与载荷，返回前不写活动目录。
    fn fetch(&self, mirror: UpdateMirror, manifest_url: &str) -> Result<MirrorCandidate, AppError>;

    /// 根据包类型选择普通清单或目录索引；测试下载器默认复用普通清单行为。
    fn fetch_for(
        &self,
        mirror: UpdateMirror,
        manifest_url: &str,
        package_type: PackageType,
        current_version: &str,
        channel: UpdateChannel,
    ) -> Result<MirrorCandidate, AppError> {
        let _ = (package_type, current_version, channel);
        self.fetch(mirror, manifest_url)
    }

    /// 只获取并解析清单元数据；生产 Gitee 实现不下载大体积构件，启动检查使用此入口。
    fn fetch_for_metadata(
        &self,
        mirror: UpdateMirror,
        manifest_url: &str,
        package_type: PackageType,
        current_version: &str,
        channel: UpdateChannel,
    ) -> Result<MirrorCandidate, AppError> {
        self.fetch_for(mirror, manifest_url, package_type, current_version, channel)
    }
}

/// Windows 生产下载器；只接受 HTTPS 或测试用 file://，不允许明文 HTTP 和重定向降级。
#[derive(Clone, Copy, Debug, Default)]
pub struct CurlUpdateSourceClient;

impl UpdateSourceClient for CurlUpdateSourceClient {
    /// 先下载清单取得固定 artifact_name，再从同一镜像下载载荷并交给签名校验。
    fn fetch(&self, mirror: UpdateMirror, manifest_url: &str) -> Result<MirrorCandidate, AppError> {
        fetch_manifest_and_payload(mirror, manifest_url)
    }

    /// 读取目录索引，优先选择当前版本对应的相邻补丁；没有匹配补丁时退回最新完整包。
    fn fetch_for(
        &self,
        mirror: UpdateMirror,
        manifest_url: &str,
        package_type: PackageType,
        current_version: &str,
        channel: UpdateChannel,
    ) -> Result<MirrorCandidate, AppError> {
        // Gitee 不提供当前项目使用的 latest/download 固定附件路径，因此先读公开 API，
        // 再按附件名定位同一个 Release 中的清单和载荷。
        if mirror == UpdateMirror::Gitee && is_gitee_release_api_url(manifest_url) {
            return fetch_gitee_release_package(
                mirror,
                manifest_url,
                package_type,
                current_version,
                channel,
            );
        }
        if package_type != PackageType::Catalog {
            return self.fetch(mirror, manifest_url);
        }

        let index_bytes = fetch_bytes(manifest_url, mirror, MAX_UPDATE_MANIFEST_BYTES)?;
        let index: CatalogUpdateIndex = serde_json::from_slice(&index_bytes).map_err(|error| {
            AppError::update_rejected(format!("{} 目录索引格式无效：{error}", mirror.as_str()))
        })?;
        if index.format != "yys-catalog-update-index"
            || index.protocol_version != CATALOG_INDEX_PROTOCOL_VERSION
            || parse_version(&index.latest_version).is_none()
        {
            return Err(AppError::update_rejected("目录索引格式或协议版本不受支持"));
        }

        let patch = index.releases.iter().filter(|entry| {
            entry.kind == CATALOG_DELTA_KIND_PATCH
                && entry.base_version.as_deref() == Some(current_version)
                && compare_versions(&entry.version, current_version) == std::cmp::Ordering::Greater
        });
        let full = index.releases.iter().filter(|entry| {
            entry.kind == "full"
                && entry.base_version.is_none()
                && compare_versions(&entry.version, current_version) == std::cmp::Ordering::Greater
        });
        let selected = patch
            .max_by(|left, right| compare_versions(&left.version, &right.version))
            .or_else(|| full.max_by(|left, right| compare_versions(&left.version, &right.version)))
            .ok_or_else(|| {
                AppError::update_source_unavailable(mirror.as_str(), "目录索引没有可用更新")
            })?;
        validate_asset_name(&selected.manifest_name)?;
        let selected_manifest_url = artifact_url(manifest_url, &selected.manifest_name)?;
        fetch_manifest_and_payload(mirror, &selected_manifest_url)
    }

    /// 启动检查只下载签名清单；安装包地址写入受控待下载目录，点击更新时再读取。
    fn fetch_for_metadata(
        &self,
        mirror: UpdateMirror,
        manifest_url: &str,
        package_type: PackageType,
        current_version: &str,
        channel: UpdateChannel,
    ) -> Result<MirrorCandidate, AppError> {
        if mirror == UpdateMirror::Gitee && is_gitee_release_api_url(manifest_url) {
            return fetch_gitee_release_metadata_candidate(
                mirror,
                manifest_url,
                package_type,
                current_version,
                channel,
            );
        }
        let candidate = fetch_manifest_only(mirror, manifest_url)?;
        Ok(candidate)
    }
}

/// 只保存受信发布公钥；私钥不进入仓库、安装包、日志或清单。
#[derive(Clone, Debug)]
pub struct TrustedUpdateSignatureVerifier {
    public_keys: BTreeMap<String, VerifyingKey>,
}

impl Default for TrustedUpdateSignatureVerifier {
    fn default() -> Self {
        let mut public_keys = BTreeMap::new();
        // 正式公钥只能由发布构建注入；未配置时默认拒绝所有外部更新包。
        if let Some(public_key) = trusted_release_public_key() {
            public_keys.insert("yys-release-v1".to_owned(), public_key);
        } else {
            tracing::warn!("未配置发行公钥，外部更新包将全部拒绝");
        }
        Self { public_keys }
    }
}

/// 签名校验注入边界，便于测试镜像切换而不生成或保存私钥。
pub trait UpdateSignatureVerifier: Send + Sync {
    fn verify(&self, manifest: &ReleaseManifest) -> bool;
}

impl UpdateSignatureVerifier for TrustedUpdateSignatureVerifier {
    /// Ed25519 只验证规范清单文本；载荷完整性由清单中的 SHA-256 同时约束。
    fn verify(&self, manifest: &ReleaseManifest) -> bool {
        let Some(public_key) = self.public_keys.get(&manifest.signer_key_id) else {
            return false;
        };
        let Ok(signature_bytes) = hex::decode(&manifest.signature) else {
            return false;
        };
        let Ok(signature) = Signature::from_slice(&signature_bytes) else {
            return false;
        };
        public_key
            .verify(canonical_manifest_payload(manifest).as_bytes(), &signature)
            .is_ok()
    }
}

/// 兼容旧调用方的稳定通道校验入口；新代码必须显式传入构建通道。
pub fn validate_release(
    candidate: MirrorCandidate,
    expected_type: PackageType,
    app_version: &str,
    game_version: Option<&str>,
    verifier: &dyn UpdateSignatureVerifier,
) -> Result<VerifiedRelease, AppError> {
    validate_release_for_channel(
        candidate,
        expected_type,
        UpdateChannel::Stable,
        app_version,
        game_version,
        verifier,
    )
}

/// 只校验清单本身；启动检查使用它，避免在用户确认前下载大体积安装包。
fn validate_manifest_for_channel(
    manifest: &ReleaseManifest,
    expected_type: PackageType,
    expected_channel: UpdateChannel,
    app_version: &str,
    game_version: Option<&str>,
    verifier: &dyn UpdateSignatureVerifier,
) -> Result<(), AppError> {
    if manifest.format != "yys-signed-update-manifest"
        || manifest.protocol_version != UPDATE_PROTOCOL_VERSION
    {
        return Err(AppError::update_rejected("更新清单格式或协议版本不受支持"));
    }
    if UpdateChannel::parse(&manifest.channel) != Some(expected_channel) {
        return Err(AppError::update_rejected(format!(
            "更新包通道与当前构建不匹配：需要 {}",
            expected_channel.as_str()
        )));
    }
    if manifest.package_type != expected_type {
        return Err(AppError::update_rejected("更新包类型与当前请求不一致"));
    }
    if manifest.package_id.trim().is_empty()
        || manifest.version.trim().is_empty()
        || manifest.min_app_version.trim().is_empty()
        || manifest.signer_key_id.trim().is_empty()
    {
        return Err(AppError::update_rejected(
            "更新清单缺少包 ID、版本或签名身份",
        ));
    }
    if manifest.artifact_name.is_empty()
        || manifest.artifact_name.len() > 255
        || manifest.artifact_name.contains("..")
        || manifest.artifact_name.contains('/')
        || manifest.artifact_name.contains('\\')
    {
        return Err(AppError::update_rejected("更新清单包含非法构件文件名"));
    }
    if expected_type == PackageType::Adapter {
        let Some(capabilities) = manifest.adapter_capabilities.as_ref() else {
            return Err(AppError::update_rejected("读取适配器清单缺少能力声明"));
        };
        if !capabilities.read_soul_data
            || capabilities.write_memory
            || capabilities.simulate_input
            || capabilities.execute_remote_code
        {
            return Err(AppError::update_rejected("读取适配器超出只读能力边界"));
        }
    } else if manifest.adapter_capabilities.is_some() {
        return Err(AppError::update_rejected(
            "非适配器更新包不能携带适配器能力声明",
        ));
    }
    if !is_version_at_least(app_version, &manifest.min_app_version) {
        return Err(AppError::update_rejected("当前应用版本低于更新包要求"));
    }
    if let Some(required_game_version) = manifest.compatible_game_version.as_deref()
        && required_game_version != "*"
        && game_version != Some(required_game_version)
    {
        return Err(AppError::update_rejected("更新包不兼容当前游戏版本"));
    }
    if parse_version(&manifest.version).is_none()
        || parse_version(&manifest.min_app_version).is_none()
    {
        return Err(AppError::update_rejected("更新清单版本必须是有效 SemVer"));
    }
    if manifest.content_sha256.len() != 64
        || !manifest
            .content_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(AppError::update_rejected("更新清单中的 SHA-256 格式无效"));
    }
    if manifest.signature.len() != 128
        || !manifest
            .signature
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || manifest.signer_key_id == "embedded"
    {
        return Err(AppError::update_rejected("更新签名格式或签名身份无效"));
    }
    if manifest.content_size > MAX_UPDATE_ARTIFACT_BYTES {
        return Err(resource_limit(
            "updateArtifactBytes",
            manifest.content_size,
            MAX_UPDATE_ARTIFACT_BYTES,
        ));
    }

    validate_delta_metadata(manifest, expected_type)?;
    if !verifier.verify(manifest) {
        return Err(AppError::update_rejected("更新清单签名无效或不受信任"));
    }
    Ok(())
}

/// 通过全部清单和载荷约束后返回可暂存对象；该函数不访问网络，也不写磁盘。
pub fn validate_release_for_channel(
    candidate: MirrorCandidate,
    expected_type: PackageType,
    expected_channel: UpdateChannel,
    app_version: &str,
    game_version: Option<&str>,
    verifier: &dyn UpdateSignatureVerifier,
) -> Result<VerifiedRelease, AppError> {
    let manifest = &candidate.manifest;
    validate_manifest_for_channel(
        manifest,
        expected_type,
        expected_channel,
        app_version,
        game_version,
        verifier,
    )?;
    if manifest.content_size != candidate.payload.len() as u64 {
        return Err(AppError::update_rejected("更新包大小与清单不一致"));
    }
    let actual_hash = sha256_hex(&candidate.payload);
    if !manifest.content_sha256.eq_ignore_ascii_case(&actual_hash) {
        return Err(AppError::update_rejected("更新包 SHA-256 校验失败"));
    }

    Ok(VerifiedRelease {
        source: candidate.source,
        manifest: manifest.clone(),
        payload: candidate.payload,
        signature_fingerprint: sha256_hex(manifest.signature.as_bytes()),
        artifact_url: candidate.artifact_url,
    })
}

/// 通过清单安全校验后返回待下载对象；载荷哈希会在用户点击更新后复核。
fn validate_manifest_only_for_channel(
    candidate: MirrorCandidate,
    expected_type: PackageType,
    expected_channel: UpdateChannel,
    app_version: &str,
    game_version: Option<&str>,
    verifier: &dyn UpdateSignatureVerifier,
) -> Result<VerifiedRelease, AppError> {
    validate_manifest_for_channel(
        &candidate.manifest,
        expected_type,
        expected_channel,
        app_version,
        game_version,
        verifier,
    )?;
    let signature_fingerprint = sha256_hex(candidate.manifest.signature.as_bytes());
    let manifest = candidate.manifest;
    Ok(VerifiedRelease {
        source: candidate.source,
        manifest,
        payload: Vec::new(),
        signature_fingerprint,
        artifact_url: candidate.artifact_url,
    })
}

/// 兼容旧调用方的稳定通道选择入口。
pub fn choose_release(
    candidates: &[VerifiedRelease],
    _source_mode: UpdateSourceMode,
) -> Result<Option<VerifiedRelease>, AppError> {
    choose_release_for_channel(candidates, UpdateChannel::Stable)
}

/// 从已验证的候选中选择当前通道的最新版本；历史镜像记录不会参与当前更新选择。
pub fn choose_release_for_channel(
    candidates: &[VerifiedRelease],
    channel: UpdateChannel,
) -> Result<Option<VerifiedRelease>, AppError> {
    // 新版本只接受 Gitee 候选；旧镜像类型保留在模型中，仅用于读取历史版本。
    let allowed = |source: UpdateMirror| source == UpdateMirror::Gitee;
    let mut allowed_candidates = candidates
        .iter()
        .filter(|candidate| {
            allowed(candidate.source)
                && UpdateChannel::parse(&candidate.manifest.channel) == Some(channel)
        })
        .cloned()
        .collect::<Vec<_>>();
    if allowed_candidates.is_empty() {
        return Ok(None);
    }

    allowed_candidates.sort_by(|left, right| {
        compare_versions(&left.manifest.version, &right.manifest.version)
            .then_with(|| left.source.as_str().cmp(right.source.as_str()))
    });
    Ok(allowed_candidates.pop())
}

/// 更新用例；协调网络、暂存、自动备份、指针激活和历史记录。
pub struct UpdateUseCase {
    services: Arc<AppServices>,
    packages_root: PathBuf,
    temp_root: PathBuf,
    config_directory: PathBuf,
    client: Arc<dyn UpdateSourceClient>,
    verifier: Arc<dyn UpdateSignatureVerifier>,
}

impl UpdateUseCase {
    /// 创建生产更新用例；路径只来自 Rust 的应用目录解析器。
    pub fn new(
        services: Arc<AppServices>,
        packages_root: PathBuf,
        temp_root: PathBuf,
        config_directory: PathBuf,
    ) -> Self {
        Self::new_with_dependencies(
            services,
            packages_root,
            temp_root,
            config_directory,
            Arc::new(CurlUpdateSourceClient),
            Arc::new(TrustedUpdateSignatureVerifier::default()),
        )
    }

    /// 注入下载器和验证器，隔离网络与密码学信任根，便于镜像故障和安全分支测试。
    pub fn new_with_dependencies(
        services: Arc<AppServices>,
        packages_root: PathBuf,
        temp_root: PathBuf,
        config_directory: PathBuf,
        client: Arc<dyn UpdateSourceClient>,
        verifier: Arc<dyn UpdateSignatureVerifier>,
    ) -> Self {
        Self {
            services,
            packages_root,
            temp_root,
            config_directory,
            client,
            verifier,
        }
    }

    /// 检查镜像并把候选写入受控目录；启动模式只保存清单，用户确认后才下载载荷。
    pub fn check_and_stage(
        &self,
        source_mode: UpdateSourceMode,
        channel: UpdateChannel,
        package_type: PackageType,
        app_version: &str,
        game_version: Option<&str>,
        stage_payload: bool,
    ) -> Result<UpdateCheckResult, AppError> {
        let config = UpdateSourceConfig::load(&self.config_directory)?;
        let current_version = self.current_version(package_type, app_version)?;
        let mut candidates = Vec::new();
        let mut mirror_summaries = Vec::new();
        for source in mirrors_for(source_mode) {
            let Some(url) = config.url(source, package_type, channel) else {
                mirror_summaries.push(MirrorCheckSummary {
                    source,
                    status: "unconfigured".to_owned(),
                    version: None,
                    content_sha256: None,
                    signer_key_id: None,
                    message: Some("镜像地址未配置".to_owned()),
                });
                continue;
            };
            let candidate_result = if stage_payload {
                self.client
                    .fetch_for(source, url, package_type, &current_version, channel)
            } else {
                self.client
                    .fetch_for_metadata(source, url, package_type, &current_version, channel)
            };
            match candidate_result {
                Ok(candidate) => {
                    let validation = if stage_payload {
                        validate_release_for_channel(
                            candidate,
                            package_type,
                            channel,
                            app_version,
                            game_version,
                            self.verifier.as_ref(),
                        )
                    } else {
                        validate_manifest_only_for_channel(
                            candidate,
                            package_type,
                            channel,
                            app_version,
                            game_version,
                            self.verifier.as_ref(),
                        )
                    };
                    match validation.and_then(|verified| {
                        validate_catalog_base(&verified.manifest, &current_version)?;
                        Ok(verified)
                    }) {
                        Ok(verified) => {
                            mirror_summaries.push(MirrorCheckSummary {
                                source,
                                status: "valid".to_owned(),
                                version: Some(verified.manifest.version.clone()),
                                content_sha256: Some(verified.manifest.content_sha256.clone()),
                                signer_key_id: Some(verified.manifest.signer_key_id.clone()),
                                message: None,
                            });
                            candidates.push(verified);
                        }
                        Err(error) => mirror_summaries.push(MirrorCheckSummary {
                            source,
                            status: "rejected".to_owned(),
                            version: None,
                            content_sha256: None,
                            signer_key_id: None,
                            message: Some(error.message),
                        }),
                    }
                }
                Err(error) => mirror_summaries.push(MirrorCheckSummary {
                    source,
                    status: "unavailable".to_owned(),
                    version: None,
                    content_sha256: None,
                    signer_key_id: None,
                    message: Some(error.message),
                }),
            }
        }

        let selected = choose_release_for_channel(&candidates, channel)?;
        let Some(selected) = selected else {
            return Err(AppError::update_source_unavailable(
                source_mode_label(source_mode),
                "没有可用且通过验证的镜像清单",
            ));
        };
        if compare_versions(&selected.manifest.version, &current_version)
            != std::cmp::Ordering::Greater
        {
            for summary in &mut mirror_summaries {
                if summary.status == "valid" {
                    summary.status = "current".to_owned();
                }
            }
            return Ok(UpdateCheckResult {
                source_mode,
                channel,
                package_type,
                current_version,
                selected: None,
                mirrors: mirror_summaries,
            });
        }

        let staged_update_id = if stage_payload {
            self.stage(&selected)?
        } else {
            self.stage_metadata(&selected)?
        };
        Ok(UpdateCheckResult {
            source_mode,
            channel,
            package_type,
            current_version,
            selected: Some(UpdateCandidateSummary {
                source: selected.source,
                channel,
                package_type,
                package_id: selected.manifest.package_id,
                version: selected.manifest.version,
                content_sha256: selected.manifest.content_sha256,
                content_size: selected.manifest.content_size,
                signer_key_id: selected.manifest.signer_key_id,
                signature_fingerprint: selected.signature_fingerprint,
                release_notes: selected.manifest.release_notes,
                staged_update_id,
            }),
            mirrors: mirror_summaries,
        })
    }

    /// 安装已经暂存的更新；活动读取会话或后台批次运行时只返回 deferred，不触碰磁盘指针。
    pub fn install_staged(
        &self,
        staged_update_id: &str,
        channel: UpdateChannel,
        app_version: &str,
        game_version: Option<&str>,
        runtime_busy: bool,
        auto_restart: bool,
    ) -> Result<UpdateInstallResult, AppError> {
        let mut ignore_progress = |_: &str, _: u64, _: u64| {};
        self.install_staged_with_progress(
            staged_update_id,
            channel,
            app_version,
            game_version,
            runtime_busy,
            auto_restart,
            &mut ignore_progress,
        )
    }

    /// 安装暂存更新并报告下载、校验和安装阶段；下载中断时只保留清单，不覆盖旧版本。
    pub fn install_staged_with_progress(
        &self,
        staged_update_id: &str,
        channel: UpdateChannel,
        app_version: &str,
        game_version: Option<&str>,
        runtime_busy: bool,
        auto_restart: bool,
        progress: &mut dyn FnMut(&str, u64, u64),
    ) -> Result<UpdateInstallResult, AppError> {
        let staged = self.staged_directory(staged_update_id)?;
        let manifest = read_manifest(&staged.join("release-manifest.json"))?;
        let source = read_source(&staged.join("source"))?;
        let package_type = manifest.package_type;
        let payload_path = staged.join("payload.bin");
        // 启动检查阶段没有下载正文；若运行中的读取任务占用资源，先延迟而不浪费下载流量。
        if runtime_busy && !payload_path.exists() {
            validate_manifest_for_channel(
                &manifest,
                package_type,
                channel,
                app_version,
                game_version,
                self.verifier.as_ref(),
            )?;
            return Ok(UpdateInstallResult {
                package_type,
                channel,
                version: manifest.version,
                state: "deferred".to_owned(),
                backup_id: None,
                content_sha256: manifest.content_sha256,
                message: "读取会话或后台批次仍在运行，请结束后重试更新".to_owned(),
            });
        }
        let payload = if payload_path.is_file() {
            fs::read(&payload_path).map_err(|error| AppError::io("读取暂存更新载荷", &error))?
        } else {
            let artifact_url = read_artifact_url(&staged)?;
            progress("downloading", 0, manifest.content_size);
            let mut report_download = |completed: u64, total: u64| {
                // 签名清单中的 contentSize 比 HTTP 头更可信，避免分块响应把百分比基准放大到安全上限。
                let _ = total;
                progress(
                    "downloading",
                    completed.min(manifest.content_size),
                    manifest.content_size,
                );
            };
            let payload = fetch_bytes_with_progress(
                &artifact_url,
                source,
                MAX_UPDATE_ARTIFACT_BYTES,
                &mut report_download,
            )?;
            let download_path = staged.join("payload.bin.download");
            fs::write(&download_path, &payload)
                .map_err(|error| AppError::io("保存下载中的更新载荷", &error))?;
            fs::rename(&download_path, &payload_path)
                .map_err(|error| AppError::io("发布已下载更新载荷", &error))?;
            payload
        };
        progress("verifying", 0, 1);
        let verified = validate_release_for_channel(
            MirrorCandidate {
                source,
                manifest,
                payload,
                artifact_url: read_artifact_url(&staged).ok(),
            },
            package_type,
            channel,
            app_version,
            game_version,
            self.verifier.as_ref(),
        )?;
        progress("verifying", 1, 1);
        if runtime_busy {
            return Ok(UpdateInstallResult {
                package_type: verified.manifest.package_type,
                channel,
                version: verified.manifest.version,
                state: "deferred".to_owned(),
                backup_id: None,
                content_sha256: verified.manifest.content_sha256,
                message: "读取会话或后台批次仍在运行，更新已保留并延迟安装".to_owned(),
            });
        }
        let current_version = self.current_version(verified.manifest.package_type, app_version)?;
        validate_catalog_base(&verified.manifest, &current_version)?;
        if compare_versions(&verified.manifest.version, &current_version)
            != std::cmp::Ordering::Greater
        {
            return Err(AppError::update_rejected("稳定通道禁止自动降级或重复安装"));
        }

        progress("installing", 0, 1);
        // 新备份校验成功后即可替代更早的自动更新备份；安装失败时仍能恢复到本次更新前状态。
        let backup = BackupUseCase::new(self.services.clone())
            .create_backup_retaining_latest("更新前自动备份")?;
        if verified.manifest.package_type == PackageType::Application {
            // 应用进程不能替换自身；先保存待安装包，再由独立 PowerShell 助手等待本进程退出。
            let version_directory = self.install_version(&verified)?;
            let pending = self.pending_application_directory(&verified.manifest.version)?;
            write_version_files(&pending, &verified.manifest, &verified.payload)?;
            write_source(&pending.join("source"), verified.source)?;
            self.services.packages.install(&InstalledPackage {
                id: package_record_id(&verified.manifest),
                package_type: PackageType::Application.as_str().to_owned(),
                version: verified.manifest.version.clone(),
                sha256: Some(verified.manifest.content_sha256.clone()),
                signature_fingerprint: Some(verified.signature_fingerprint.clone()),
                active: false,
                source: Some(verified.source.as_str().to_owned()),
                content_path: Some(version_directory.display().to_string()),
                installed_at: AppServices::now_iso(),
            })?;
            let _ = fs::remove_dir_all(staged);
            if auto_restart {
                self.schedule_application_restart(&pending, &verified.manifest.artifact_name)?;
            }
            progress("installing", 1, 1);
            return Ok(UpdateInstallResult {
                package_type: PackageType::Application,
                channel,
                version: verified.manifest.version,
                state: if auto_restart {
                    "restarting".to_owned()
                } else {
                    "pending_restart".to_owned()
                },
                backup_id: Some(backup.id),
                content_sha256: verified.manifest.content_sha256,
                message: if auto_restart {
                    "更新已验证，应用即将退出并自动安装后重启".to_owned()
                } else {
                    "更新已下载并完成验证，请确认后重启安装".to_owned()
                },
            });
        }

        let package_type = verified.manifest.package_type;
        let previous_version = self.active_version(package_type)?;
        // 通用更新指针可能还没有记录内置种子目录，因此目录回退必须单独保存数据库活动版本。
        let previous_catalog_version = if package_type == PackageType::Catalog {
            CatalogUseCase::new(self.services.clone())
                .active_status()?
                .map(|status| status.version)
        } else {
            None
        };
        let version_directory = self.install_version(&verified)?;

        // 式神与御魂目录必须先通过领域校验并写入 SQLite，再发布通用活动指针。
        // 目录用例内部使用事务，失败时旧目录仍保持活动状态。
        if package_type == PackageType::Catalog {
            let catalog = CatalogUseCase::new(self.services.clone());
            if let Err(error) = catalog.install_external_package(
                &verified.payload,
                &verified.manifest.version,
                verified.source.as_str(),
                &verified.manifest.content_sha256,
                &verified.manifest.signature,
                verified.manifest.compatible_game_version.as_deref(),
                &verified.manifest.min_app_version,
                verified.manifest.delta_kind.as_deref(),
                verified.manifest.base_version.as_deref(),
                verified.manifest.base_content_sha256.as_deref(),
                verified.manifest.result_content_sha256.as_deref(),
            ) {
                let _ = fs::remove_dir_all(&version_directory);
                return Err(error);
            }
        }
        if let Err(error) = self.set_active_version(package_type, &verified.manifest.version) {
            if package_type == PackageType::Catalog {
                if let Some(version) = previous_catalog_version.as_deref() {
                    let _ = CatalogUseCase::new(self.services.clone()).activate_version(version);
                }
            }
            return Err(error);
        }
        let package = InstalledPackage {
            id: package_record_id(&verified.manifest),
            package_type: package_type.as_str().to_owned(),
            version: verified.manifest.version.clone(),
            sha256: Some(verified.manifest.content_sha256.clone()),
            signature_fingerprint: Some(verified.signature_fingerprint.clone()),
            active: true,
            source: Some(verified.source.as_str().to_owned()),
            content_path: Some(version_directory.display().to_string()),
            installed_at: AppServices::now_iso(),
        };
        if let Err(error) = self.services.packages.install(&package) {
            // 数据库记录失败时恢复旧活动指针，避免文件系统和事实层出现不同版本。
            if package_type == PackageType::Catalog {
                if let Some(version) = previous_catalog_version.as_deref() {
                    let _ = CatalogUseCase::new(self.services.clone()).activate_version(version);
                }
            }
            if let Err(restore_error) =
                self.restore_active_version(package_type, previous_version.as_deref())
            {
                return Err(AppError::internal(format!(
                    "更新记录失败且无法恢复旧活动指针：{error}；恢复失败：{restore_error}"
                )));
            }
            return Err(error);
        }
        let _ = fs::remove_dir_all(staged);
        progress("installing", 1, 1);
        Ok(UpdateInstallResult {
            package_type,
            channel,
            version: package.version,
            state: "activated".to_owned(),
            backup_id: Some(backup.id),
            content_sha256: package.sha256.unwrap_or_default(),
            message: "更新已验证并切换到活动版本".to_owned(),
        })
    }

    /// 用户明确选择历史版本回退；该入口不允许自动调用，也不受稳定通道升级判断限制。
    pub fn rollback(
        &self,
        package_type: PackageType,
        channel: UpdateChannel,
        version: &str,
        app_version: &str,
        game_version: Option<&str>,
        runtime_busy: bool,
    ) -> Result<UpdateInstallResult, AppError> {
        if runtime_busy {
            return Ok(UpdateInstallResult {
                package_type,
                channel,
                version: version.to_owned(),
                state: "deferred".to_owned(),
                backup_id: None,
                content_sha256: String::new(),
                message: "读取会话或后台批次仍在运行，用户回退已延迟".to_owned(),
            });
        }
        let version_directory = self.version_directory(package_type, version)?;
        let manifest = read_manifest(&version_directory.join("release-manifest.json"))?;
        let payload = fs::read(version_directory.join("payload.bin"))
            .map_err(|error| AppError::io("读取历史更新载荷", &error))?;
        let verified = validate_release_for_channel(
            MirrorCandidate {
                source: read_source(&version_directory.join("source"))?,
                manifest,
                payload,
                artifact_url: None,
            },
            package_type,
            channel,
            app_version,
            game_version,
            self.verifier.as_ref(),
        )?;
        if verified.manifest.version != version {
            return Err(AppError::update_rejected("回退目录版本与清单版本不一致"));
        }
        let known = self.services.packages.list()?.into_iter().any(|package| {
            package.package_type == package_type.as_str() && package.version == version
        });
        if !known {
            return Err(AppError::not_found("installed_package", version));
        }
        let backup =
            BackupUseCase::new(self.services.clone()).create_backup("用户主动回退前自动备份")?;
        let previous_version = self.active_version(package_type)?;
        // 回退前保存数据库活动目录，确保通用文件指针或安装记录失败时能恢复图鉴状态。
        let previous_catalog_version = if package_type == PackageType::Catalog {
            CatalogUseCase::new(self.services.clone())
                .active_status()?
                .map(|status| status.version)
        } else {
            None
        };

        // 目录回退必须同步切换 SQLite 的活动目录版本，否则图鉴和通用更新指针会分叉。
        if package_type == PackageType::Catalog {
            CatalogUseCase::new(self.services.clone()).activate_version(version)?;
        }
        if let Err(error) = self.set_active_version(package_type, version) {
            if package_type == PackageType::Catalog {
                if let Some(old) = previous_catalog_version.as_deref() {
                    let _ = CatalogUseCase::new(self.services.clone()).activate_version(old);
                }
            }
            return Err(error);
        }
        if let Err(error) = self
            .services
            .packages
            .activate(package_type.as_str(), version)
        {
            if package_type == PackageType::Catalog {
                if let Some(old) = previous_catalog_version.as_deref() {
                    let _ = CatalogUseCase::new(self.services.clone()).activate_version(old);
                }
            }
            if let Err(restore_error) =
                self.restore_active_version(package_type, previous_version.as_deref())
            {
                return Err(AppError::internal(format!(
                    "回退记录失败且无法恢复旧活动指针：{error}；恢复失败：{restore_error}"
                )));
            }
            return Err(error);
        }
        Ok(UpdateInstallResult {
            package_type,
            channel,
            version: verified.manifest.version,
            state: "rolled_back".to_owned(),
            backup_id: Some(backup.id),
            content_sha256: verified.manifest.content_sha256,
            message: "已按用户选择回退到历史版本".to_owned(),
        })
    }

    /// 查询更新历史；所有路径和签名身份由 Rust 整理后再返回界面。
    pub fn list_installed(&self) -> Result<Vec<InstalledPackage>, AppError> {
        self.services.packages.list()
    }

    fn current_version(
        &self,
        package_type: PackageType,
        app_version: &str,
    ) -> Result<String, AppError> {
        if package_type == PackageType::Application {
            return Ok(app_version.to_owned());
        }
        if package_type == PackageType::Catalog {
            // 首次安装的内置目录可能尚未进入通用更新历史，优先读取目录仓库的真实活动版本。
            if let Some(status) = CatalogUseCase::new(self.services.clone()).active_status()? {
                return Ok(status.version);
            }
        }
        Ok(self
            .services
            .packages
            .list()?
            .into_iter()
            .find(|package| package.package_type == package_type.as_str() && package.active)
            .map(|package| package.version)
            .unwrap_or_else(|| "0.0.0".to_owned()))
    }

    /// 写入唯一版本目录和清单来源标记，后续激活只移动小型指针文件。
    fn install_version(&self, release: &VerifiedRelease) -> Result<PathBuf, AppError> {
        let directory =
            self.version_directory(release.manifest.package_type, &release.manifest.version)?;
        let parent = directory
            .parent()
            .ok_or_else(|| AppError::internal("更新版本目录缺少父目录"))?;
        fs::create_dir_all(parent).map_err(|error| AppError::io("创建更新版本目录", &error))?;
        let staging = parent.join(format!(".staging-{}", Uuid::new_v4()));
        write_version_files(&staging, &release.manifest, &release.payload)?;
        write_source(&staging.join("source"), release.source)?;
        if directory.exists() {
            let existing_manifest = read_manifest(&directory.join("release-manifest.json"))?;
            let existing_payload = fs::read(directory.join("payload.bin"))
                .map_err(|error| AppError::io("读取已存在更新版本", &error))?;
            if existing_manifest.content_sha256 != release.manifest.content_sha256
                || sha256_hex(&existing_payload) != release.manifest.content_sha256
            {
                let _ = fs::remove_dir_all(&staging);
                return Err(AppError::update_rejected(
                    "同版本更新目录内容与已验证清单不一致",
                ));
            }
            fs::remove_dir_all(&staging)
                .map_err(|error| AppError::io("清理重复更新暂存", &error))?;
        } else if let Err(error) = fs::rename(&staging, &directory) {
            let _ = fs::remove_dir_all(&staging);
            return Err(AppError::io("发布更新版本目录", &error));
        }
        Ok(directory)
    }

    /// 将已验证对象写入随机暂存目录，完成后目录重命名，下载中断不会污染候选列表。
    fn stage(&self, release: &VerifiedRelease) -> Result<String, AppError> {
        let stage_id = Uuid::new_v4().to_string();
        let root = self.temp_root.join("updates").join("staged");
        fs::create_dir_all(&root).map_err(|error| AppError::io("创建更新暂存目录", &error))?;
        let staging = root.join(format!("{stage_id}.staging"));
        let target = root.join(&stage_id);
        write_version_files(&staging, &release.manifest, &release.payload)?;
        write_source(&staging.join("source"), release.source)?;
        fs::rename(&staging, &target).map_err(|error| {
            let _ = fs::remove_dir_all(&staging);
            AppError::io("发布更新暂存对象", &error)
        })?;
        Ok(stage_id)
    }

    /// 保存启动检查得到的清单和构件地址；目录中没有 payload.bin，确保启动检查不会下载安装包。
    fn stage_metadata(&self, release: &VerifiedRelease) -> Result<String, AppError> {
        let artifact_url = release
            .artifact_url
            .as_deref()
            .filter(|url| url.starts_with("https://"))
            .ok_or_else(|| AppError::update_rejected("更新清单缺少安全的构件下载地址"))?;
        let stage_id = Uuid::new_v4().to_string();
        let root = self.temp_root.join("updates").join("staged");
        fs::create_dir_all(&root).map_err(|error| AppError::io("创建更新暂存目录", &error))?;
        let staging = root.join(format!("{stage_id}.staging"));
        let target = root.join(&stage_id);
        fs::create_dir_all(&staging)
            .map_err(|error| AppError::io("创建更新清单暂存目录", &error))?;
        let result = (|| {
            let manifest_bytes = serde_json::to_vec_pretty(&release.manifest)
                .map_err(|error| AppError::internal(format!("序列化更新清单失败：{error}")))?;
            fs::write(staging.join("release-manifest.json"), manifest_bytes)
                .map_err(|error| AppError::io("写入待下载更新清单", &error))?;
            write_source(&staging.join("source"), release.source)?;
            fs::write(staging.join("artifact-url"), artifact_url)
                .map_err(|error| AppError::io("写入待下载更新地址", &error))?;
            fs::rename(&staging, &target)
                .map_err(|error| AppError::io("发布待下载更新清单", &error))?;
            Ok::<(), AppError>(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        Ok(stage_id)
    }

    fn staged_directory(&self, stage_id: &str) -> Result<PathBuf, AppError> {
        validate_identifier(stage_id, "stagedUpdateId")?;
        let directory = self.temp_root.join("updates").join("staged").join(stage_id);
        if !directory.is_dir() {
            return Err(AppError::not_found("staged_update", stage_id));
        }
        Ok(directory)
    }

    fn pending_application_directory(&self, version: &str) -> Result<PathBuf, AppError> {
        validate_version_path(version)?;
        Ok(self
            .packages_root
            .join(PackageType::Application.directory_name())
            .join("pending")
            .join(version))
    }

    /// 生成固定逻辑的临时更新助手；外部清单只能提供已校验的安装包文件名，不能注入命令。
    fn schedule_application_restart(
        &self,
        pending_directory: &Path,
        artifact_name: &str,
    ) -> Result<(), AppError> {
        #[cfg(not(windows))]
        {
            let _ = (pending_directory, artifact_name);
            return Err(AppError::update_rejected(
                "应用自动安装仅支持 Windows 安装包",
            ));
        }

        #[cfg(windows)]
        {
            validate_asset_name(artifact_name)?;
            // 自动助手只执行 NSIS 可执行安装包，拒绝把任意扩展名交给 PowerShell。
            if !artifact_name.to_ascii_lowercase().ends_with(".exe") {
                return Err(AppError::update_rejected(
                    "应用自动更新构件必须是 NSIS .exe 安装包",
                ));
            }
            let installer = pending_directory.join(artifact_name);
            fs::copy(pending_directory.join("payload.bin"), &installer)
                .map_err(|error| AppError::io("准备应用安装程序", &error))?;
            let target = std::env::current_exe()
                .map_err(|error| AppError::io("定位当前应用程序", &error))?;
            let script_directory = self.temp_root.join("updates");
            fs::create_dir_all(&script_directory)
                .map_err(|error| AppError::io("创建自动更新助手目录", &error))?;
            let helper_id = Uuid::new_v4();
            let script_path = script_directory.join(format!("apply-{helper_id}.ps1"));
            let log_path = script_directory.join(format!("restart-{helper_id}.log"));
            let script = build_restart_script(
                std::process::id(),
                &installer,
                &target,
                &log_path,
            );
            // Windows PowerShell 5.1 只有识别 UTF-8 BOM 才会按 UTF-8 读取中文路径，否则会把安装器路径解析成乱码。
            fs::write(&script_path, powershell_script_bytes(&script))
                .map_err(|error| AppError::io("写入自动更新助手", &error))?;

            use std::os::windows::process::CommandExt;
            let mut command = Command::new("powershell.exe");
            command
                .creation_flags(0x0800_0000)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ])
                .arg(&script_path);
            command
                .spawn()
                .map_err(|error| AppError::io("启动自动更新助手", &error))?;
            Ok(())
        }
    }

    fn version_directory(
        &self,
        package_type: PackageType,
        version: &str,
    ) -> Result<PathBuf, AppError> {
        validate_version_path(version)?;
        Ok(self
            .packages_root
            .join(package_type.directory_name())
            .join("versions")
            .join(version))
    }

    fn active_version(&self, package_type: PackageType) -> Result<Option<String>, AppError> {
        let path = self
            .packages_root
            .join(package_type.directory_name())
            .join("active.json");
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|error| AppError::io("读取活动更新指针", &error))?;
        let pointer: ActivePointer = serde_json::from_slice(&bytes)
            .map_err(|error| AppError::update_rejected(format!("活动更新指针损坏：{error}")))?;
        validate_version_path(&pointer.version)?;
        Ok(Some(pointer.version))
    }

    /// 原子指针切换使用临时文件和旧指针备份；任何发布失败都会尝试恢复旧版本。
    fn set_active_version(&self, package_type: PackageType, version: &str) -> Result<(), AppError> {
        validate_version_path(version)?;
        let directory = self.packages_root.join(package_type.directory_name());
        fs::create_dir_all(&directory).map_err(|error| AppError::io("创建活动更新目录", &error))?;
        let pointer = directory.join("active.json");
        let staging = directory.join(format!("active.{}.staging", Uuid::new_v4()));
        let previous = directory.join("active.previous.json");
        let bytes = serde_json::to_vec(&ActivePointer {
            version: version.to_owned(),
        })
        .map_err(|error| AppError::internal(format!("序列化活动更新指针失败：{error}")))?;
        fs::write(&staging, bytes).map_err(|error| AppError::io("写入活动更新指针暂存", &error))?;

        // Windows 的 rename 不覆盖已有文件，因此先保留旧指针；新指针发布失败时立即恢复。
        if pointer.exists() {
            let _ = fs::remove_file(&previous);
            if let Err(error) = fs::rename(&pointer, &previous) {
                let _ = fs::remove_file(&staging);
                return Err(AppError::io("隔离旧活动更新指针", &error));
            }
        }
        if let Err(error) = fs::rename(&staging, &pointer) {
            let _ = fs::remove_file(&staging);
            let _ = fs::rename(&previous, &pointer);
            return Err(AppError::io("发布活动更新指针", &error));
        }
        let _ = fs::remove_file(previous);
        Ok(())
    }

    /// 当目标类型没有旧版本时删除活动指针，恢复“未安装”状态而不是留下孤儿版本。
    fn restore_active_version(
        &self,
        package_type: PackageType,
        previous_version: Option<&str>,
    ) -> Result<(), AppError> {
        if let Some(previous_version) = previous_version {
            return self.set_active_version(package_type, previous_version);
        }
        let pointer = self
            .packages_root
            .join(package_type.directory_name())
            .join("active.json");
        match fs::remove_file(pointer) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::io("清理失败更新活动指针", &error)),
        }
    }
}

/// 活动版本指针只包含经过路径校验的版本号，不允许携带目录或命令参数。
#[derive(Clone, Debug, Deserialize, Serialize)]
struct ActivePointer {
    version: String,
}

/// 将版本、清单和来源写入同一版本目录；写入失败时由调用方删除整个目录。
fn write_version_files(
    directory: &Path,
    manifest: &ReleaseManifest,
    payload: &[u8],
) -> Result<(), AppError> {
    fs::create_dir_all(directory).map_err(|error| AppError::io("创建更新版本暂存目录", &error))?;
    let manifest_bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|error| AppError::internal(format!("序列化更新清单失败：{error}")))?;
    fs::write(directory.join("release-manifest.json"), manifest_bytes)
        .map_err(|error| AppError::io("写入更新清单", &error))?;
    if manifest.package_type == PackageType::Adapter {
        // MuMu 读取边界仍消费既有 AdapterManifest；发行清单单独保存，防止两种签名协议互相覆盖。
        let capabilities = manifest
            .adapter_capabilities
            .clone()
            .ok_or_else(|| AppError::update_rejected("适配器清单缺少能力声明"))?;
        let adapter_manifest = AdapterManifest {
            package_id: manifest.package_id.clone(),
            version: manifest.version.clone(),
            min_app_version: manifest.min_app_version.clone(),
            supported_game_versions: vec![
                manifest
                    .compatible_game_version
                    .clone()
                    .unwrap_or_else(|| "*".to_owned()),
            ],
            content_sha256: manifest.content_sha256.clone(),
            signature: manifest.signature.clone(),
            signer_key_id: manifest.signer_key_id.clone(),
            capabilities,
        };
        let adapter_bytes = serde_json::to_vec_pretty(&adapter_manifest)
            .map_err(|error| AppError::internal(format!("序列化适配器清单失败：{error}")))?;
        fs::write(directory.join("manifest.json"), adapter_bytes)
            .map_err(|error| AppError::io("写入适配器清单", &error))?;
    }
    fs::write(directory.join("payload.bin"), payload)
        .map_err(|error| AppError::io("写入更新载荷", &error))?;
    Ok(())
}

/// 来源标记写入独立小文件，便于离线回退时继续展示来源而不读取网络。
fn write_source(path: &Path, source: UpdateMirror) -> Result<(), AppError> {
    fs::write(path, source.as_str()).map_err(|error| AppError::io("写入更新来源", &error))
}

/// 将受控本地路径写入 PowerShell 单引号字符串，避免空格和引号导致助手路径截断。
#[cfg(windows)]
fn powershell_literal(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

/// 为 PowerShell 5.1 添加 UTF-8 BOM，确保中文用户名、安装目录和安装包名不被系统代码页破坏。
#[cfg(windows)]
fn powershell_script_bytes(script: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(3 + script.len());
    bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    bytes.extend_from_slice(script.as_bytes());
    bytes
}

/// 构造独立更新助手脚本；助手必须可诊断、可超时，并在安装失败时恢复启动旧版本。
#[cfg(windows)]
fn build_restart_script(parent_pid: u32, installer: &Path, target: &Path, log: &Path) -> String {
    format!(
        r#"$ErrorActionPreference = 'Stop'
$parentPid = {parent_pid}
$installer = '{installer}'
$target = '{target}'
$log = '{log}'

function Write-UpdateLog([string]$message) {{
    try {{
        $stamp = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
        Add-Content -LiteralPath $log -Value "[$stamp] $message" -Encoding UTF8
    }} catch {{
        # 日志不可写时不能阻断回退启动。
    }}
}}

try {{
    Write-UpdateLog "更新助手已启动，等待主进程 $parentPid 退出。"
    $deadline = (Get-Date).AddSeconds({wait_timeout})
    while ($true) {{
        $parent = Get-Process -Id $parentPid -ErrorAction SilentlyContinue
        if ($null -eq $parent) {{ break }}
        if ((Get-Date) -gt $deadline) {{
            throw "等待旧版本进程退出超时：PID=$parentPid"
        }}
        Start-Sleep -Milliseconds 100
    }}

    if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {{
        throw "安装包不存在：$installer"
    }}
    Write-UpdateLog "开始以管理员权限运行安装包。"
    $process = Start-Process -FilePath $installer -ArgumentList @('/S') -Verb RunAs -Wait -PassThru
    if ($null -eq $process) {{
        throw '无法启动安装器，可能未通过 UAC 权限确认。'
    }}
    if ($process.ExitCode -ne 0) {{
        throw "安装器失败，退出码=$($process.ExitCode)"
    }}
    if (-not (Test-Path -LiteralPath $target -PathType Leaf)) {{
        throw "安装完成后找不到应用程序：$target"
    }}
    Start-Process -FilePath $target -ErrorAction Stop | Out-Null
    Write-UpdateLog "安装成功，已启动新版本。"
}} catch {{
    Write-UpdateLog ("自动更新失败，保留旧版本：" + $_.Exception.Message)
    try {{
        if (Test-Path -LiteralPath $target -PathType Leaf) {{
            Start-Process -FilePath $target -ErrorAction Stop | Out-Null
            Write-UpdateLog "已回退启动旧版本。"
        }}
    }} catch {{
        Write-UpdateLog ("回退启动旧版本也失败：" + $_.Exception.Message)
    }}
}} finally {{
    Start-Sleep -Milliseconds 500
    Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
}}
"#,
        parent_pid = parent_pid,
        installer = powershell_literal(installer),
        target = powershell_literal(target),
        log = powershell_literal(log),
        wait_timeout = UPDATE_HELPER_WAIT_TIMEOUT_SECONDS,
    )
}

/// 读取并限制来源标记集合，防止历史目录伪造为任意外部来源。
fn read_source(path: &Path) -> Result<UpdateMirror, AppError> {
    let source = fs::read_to_string(path).map_err(|error| AppError::io("读取更新来源", &error))?;
    match source.trim() {
        "github" => Ok(UpdateMirror::Github),
        "gitee" => Ok(UpdateMirror::Gitee),
        _ => Err(AppError::update_rejected("更新来源标记无效")),
    }
}

/// 读取启动检查保存的下载地址；地址只允许 HTTPS，不能从清单字段拼接任意本地路径。
fn read_artifact_url(directory: &Path) -> Result<String, AppError> {
    let url = fs::read_to_string(directory.join("artifact-url"))
        .map_err(|error| AppError::io("读取待下载更新地址", &error))?;
    let url = url.trim().to_owned();
    if !url.starts_with("https://") {
        return Err(AppError::update_rejected("待下载更新地址不是 HTTPS"));
    }
    Ok(url)
}

fn read_manifest(path: &Path) -> Result<ReleaseManifest, AppError> {
    let bytes = fs::read(path).map_err(|error| AppError::io("读取更新清单", &error))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| AppError::update_rejected(format!("更新清单格式无效：{error}")))
}

fn package_record_id(manifest: &ReleaseManifest) -> String {
    format!(
        "{}:{}:{}",
        manifest.package_type.as_str(),
        manifest.version,
        manifest.content_sha256.to_ascii_lowercase()
    )
}

/// SemVer 预发布标识；数字标识必须按数值比较且低于文本标识。
#[derive(Clone, Debug, PartialEq, Eq)]
enum PreReleaseIdentifier {
    Numeric(u64),
    Text(String),
}

/// 应用更新使用的最小 SemVer 模型；构建元数据不参与版本优先级比较。
#[derive(Clone, Debug, PartialEq, Eq)]
struct SemanticVersion {
    core: [u64; 3],
    pre_release: Vec<PreReleaseIdentifier>,
}

/// 解析数字核心、预发布标识和可忽略的构建元数据，兼容 beta/test 发布版本。
fn parse_version(value: &str) -> Option<SemanticVersion> {
    let (without_build, _) = value.split_once('+').unwrap_or((value, ""));
    let (core_text, pre_text) = without_build
        .split_once('-')
        .map_or((without_build, None), |(core, pre)| (core, Some(pre)));
    let core_parts = core_text.split('.').collect::<Vec<_>>();
    if core_parts.is_empty()
        || core_parts.len() > 3
        || core_parts.iter().any(|part| part.is_empty())
    {
        return None;
    }
    let mut core = [0_u64; 3];
    for (index, part) in core_parts.iter().enumerate() {
        if part.len() > 1 && part.starts_with('0') {
            return None;
        }
        core[index] = part.parse::<u64>().ok()?;
    }

    let pre_release = match pre_text {
        None => Vec::new(),
        Some(pre) if pre.is_empty() => return None,
        Some(pre) => pre
            .split('.')
            .map(|identifier| {
                if identifier.is_empty()
                    || !identifier
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                {
                    return None;
                }
                if identifier.bytes().all(|byte| byte.is_ascii_digit()) {
                    if identifier.len() > 1 && identifier.starts_with('0') {
                        return None;
                    }
                    Some(PreReleaseIdentifier::Numeric(identifier.parse().ok()?))
                } else {
                    Some(PreReleaseIdentifier::Text(identifier.to_owned()))
                }
            })
            .collect::<Option<Vec<_>>>()?,
    };

    Some(SemanticVersion { core, pre_release })
}

/// 按 SemVer 规则比较版本；非法版本排序到最前，仅供选择器继续筛掉无效清单。
fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let Some(left) = parse_version(left) else {
        return if parse_version(right).is_some() {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        };
    };
    let Some(right) = parse_version(right) else {
        return std::cmp::Ordering::Greater;
    };
    for (left_core, right_core) in left.core.iter().zip(right.core.iter()) {
        match left_core.cmp(right_core) {
            std::cmp::Ordering::Equal => continue,
            ordering => return ordering,
        }
    }
    match (left.pre_release.is_empty(), right.pre_release.is_empty()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        (false, false) => {
            for (left_identifier, right_identifier) in
                left.pre_release.iter().zip(right.pre_release.iter())
            {
                let ordering = match (left_identifier, right_identifier) {
                    (PreReleaseIdentifier::Numeric(left), PreReleaseIdentifier::Numeric(right)) => {
                        left.cmp(right)
                    }
                    (PreReleaseIdentifier::Numeric(_), PreReleaseIdentifier::Text(_)) => {
                        std::cmp::Ordering::Less
                    }
                    (PreReleaseIdentifier::Text(_), PreReleaseIdentifier::Numeric(_)) => {
                        std::cmp::Ordering::Greater
                    }
                    (PreReleaseIdentifier::Text(left), PreReleaseIdentifier::Text(right)) => {
                        left.cmp(right)
                    }
                };
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            }
            left.pre_release.len().cmp(&right.pre_release.len())
        }
    }
}

fn is_version_at_least(actual: &str, required: &str) -> bool {
    compare_versions(actual, required) != std::cmp::Ordering::Less
}

fn validate_version_path(version: &str) -> Result<(), AppError> {
    if parse_version(version).is_none()
        || version
            .chars()
            .any(|character| character == '/' || character == '\\')
    {
        return Err(AppError::update_rejected("更新版本不能作为安全目录名"));
    }
    Ok(())
}

fn validate_identifier(value: &str, field: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 128
        || value
            .chars()
            .any(|character| character == '/' || character == '\\')
        || value.contains("..")
    {
        return Err(AppError::invalid_argument(field, "更新标识格式无效"));
    }
    Ok(())
}

fn mirrors_for(mode: UpdateSourceMode) -> Vec<UpdateMirror> {
    let _ = mode;
    vec![UpdateMirror::Gitee]
}

fn source_mode_label(_mode: UpdateSourceMode) -> &'static str {
    "Gitee"
}

/// 签名覆盖的字段采用 JSON 标量逐行编码，避免键顺序和空白变化改变签名语义。
pub fn canonical_manifest_payload(manifest: &ReleaseManifest) -> String {
    if manifest.package_type == PackageType::Adapter {
        let capabilities = manifest.adapter_capabilities.clone().unwrap_or_default();
        return [
            manifest.package_id.clone(),
            manifest.version.clone(),
            // 适配器也必须把通道纳入签名，防止 stable 清单被复制到 test Release。
            manifest.channel.clone(),
            manifest.min_app_version.clone(),
            manifest
                .compatible_game_version
                .clone()
                .unwrap_or_else(|| "*".to_owned()),
            manifest.content_sha256.clone(),
            manifest.signer_key_id.clone(),
            capabilities.read_soul_data.to_string(),
            capabilities.write_memory.to_string(),
            capabilities.simulate_input.to_string(),
            capabilities.execute_remote_code.to_string(),
        ]
        .join("\n");
    }
    let mut fields = vec![
        manifest.format.clone(),
        manifest.protocol_version.to_string(),
        manifest.channel.clone(),
        manifest.package_type.as_str().to_owned(),
        manifest.package_id.clone(),
        manifest.version.clone(),
        manifest.min_app_version.clone(),
        manifest.compatible_game_version.clone().unwrap_or_default(),
        manifest.content_sha256.clone().to_ascii_lowercase(),
        manifest.content_size.to_string(),
        manifest.artifact_name.clone(),
        manifest.signer_key_id.clone(),
        manifest.release_notes.clone(),
    ];
    // 增量字段只追加到目录补丁签名，完整包继续使用原有字段顺序，兼容已经发布的完整包。
    if manifest.delta_kind.is_some() {
        fields.extend([
            manifest.delta_kind.clone().unwrap_or_default(),
            manifest.base_version.clone().unwrap_or_default(),
            manifest.base_content_sha256.clone().unwrap_or_default(),
            manifest.result_content_sha256.clone().unwrap_or_default(),
        ]);
    }
    fields
        .into_iter()
        .map(|value| serde_json::to_string(&value).unwrap_or_else(|_| "\"\"".to_owned()))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn sha256_hex(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    hex::encode(hasher.finalize())
}

/// 校验增量清单自身的字段组合；基础版本匹配在读取本地活动目录后由调用方完成。
fn validate_delta_metadata(
    manifest: &ReleaseManifest,
    expected_type: PackageType,
) -> Result<(), AppError> {
    let has_base_fields = manifest.base_version.is_some()
        || manifest.base_content_sha256.is_some()
        || manifest.result_content_sha256.is_some();
    match manifest.delta_kind.as_deref() {
        None if has_base_fields => Err(AppError::update_rejected(
            "完整更新清单不能携带增量基础字段",
        )),
        None => Ok(()),
        Some(CATALOG_DELTA_KIND_PATCH) if expected_type != PackageType::Catalog => {
            Err(AppError::update_rejected("只有目录更新包允许使用增量补丁"))
        }
        Some(CATALOG_DELTA_KIND_PATCH) => {
            let Some(base_version) = manifest.base_version.as_deref() else {
                return Err(AppError::update_rejected("目录补丁缺少基础版本"));
            };
            if base_version.trim().is_empty() {
                return Err(AppError::update_rejected("目录补丁基础版本不能为空"));
            }
            for (name, value) in [
                ("baseContentSha256", manifest.base_content_sha256.as_deref()),
                (
                    "resultContentSha256",
                    manifest.result_content_sha256.as_deref(),
                ),
            ] {
                if let Some(value) = value {
                    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                        return Err(AppError::update_rejected(format!(
                            "目录补丁 {name} 格式无效"
                        )));
                    }
                }
            }
            Ok(())
        }
        Some(other) => Err(AppError::update_rejected(format!(
            "不支持的目录更新类型：{other}"
        ))),
    }
}

/// 检查目录补丁是否针对当前活动版本；完整包没有基础版本约束。
fn validate_catalog_base(
    manifest: &ReleaseManifest,
    current_version: &str,
) -> Result<(), AppError> {
    if manifest.package_type != PackageType::Catalog {
        return Ok(());
    }
    if let Some(base_version) = manifest.base_version.as_deref()
        && base_version != current_version
    {
        return Err(AppError::update_rejected(format!(
            "目录补丁需要基础版本 {base_version}，当前是 {current_version}"
        )));
    }
    Ok(())
}

/// 清单和索引只允许引用同一目录下的单文件，拒绝路径穿越和隐藏目录跳转。
fn validate_asset_name(name: &str) -> Result<(), AppError> {
    if name.trim().is_empty()
        || name.len() > 255
        || name.contains("..")
        || name.contains('/')
        || name.contains('\\')
    {
        return Err(AppError::update_rejected("更新清单包含非法构件文件名"));
    }
    Ok(())
}

/// 清单 URL 必须是 HTTPS 或测试 file://，构件名由已解析清单提供且已限制为单文件名。
fn artifact_url(manifest_url: &str, artifact_name: &str) -> Result<String, AppError> {
    if manifest_url.starts_with("file://") {
        let path = manifest_url
            .strip_prefix("file://")
            .unwrap_or_default()
            .replace("/", "\\");
        let path = PathBuf::from(path);
        let parent = path
            .parent()
            .ok_or_else(|| AppError::update_rejected("file:// 清单路径缺少父目录"))?;
        return Ok(format!("file://{}", parent.join(artifact_name).display()));
    }
    if !manifest_url.starts_with("https://") {
        return Err(AppError::update_rejected("更新源必须使用 HTTPS"));
    }
    let (parent, _) = manifest_url
        .rsplit_once('/')
        .ok_or_else(|| AppError::update_rejected("更新清单地址格式无效"))?;
    Ok(format!("{parent}/{artifact_name}"))
}

/// 识别 Gitee Release API 地址；只允许仓库 releases 或 releases/latest API。
fn is_gitee_release_api_url(url: &str) -> bool {
    if !url.starts_with("https://gitee.com/api/v5/repos/") {
        return false;
    }
    let path = url.split('?').next().unwrap_or_default();
    path.ends_with("/releases") || path.ends_with("/releases/latest")
}

/// 按包类型约定清单附件名；应用兼容早期的通用 release-manifest.json 命名。
fn gitee_manifest_asset_names(package_type: PackageType) -> &'static [&'static str] {
    match package_type {
        PackageType::Application => &["release-manifest.json", "application-release-manifest.json"],
        PackageType::Rules => &["rules-release-manifest.json"],
        PackageType::Adapter => &["adapter-release-manifest.json"],
        PackageType::Catalog => &["catalog-index.json"],
    }
}

/// 从 Gitee Release API 读取 Release 列表；旧 latest 对象响应也统一包装成单元素列表。
fn fetch_gitee_release_metadata(api_url: &str) -> Result<Vec<GiteeReleaseMetadata>, AppError> {
    let bytes = fetch_bytes(api_url, UpdateMirror::Gitee, MAX_UPDATE_MANIFEST_BYTES)?;
    if api_url
        .split('?')
        .next()
        .unwrap_or_default()
        .ends_with("/latest")
    {
        return serde_json::from_slice::<GiteeReleaseMetadata>(&bytes)
            .map(|release| vec![release])
            .map_err(|error| {
                AppError::update_rejected(format!("Gitee Release API 响应格式无效：{error}"))
            });
    }
    serde_json::from_slice::<Vec<GiteeReleaseMetadata>>(&bytes)
        .map_err(|error| AppError::update_rejected(format!("Gitee Release 列表格式无效：{error}")))
}

/// 只从单个 Release 读取清单元数据，载荷要等候选版本选定后再下载。
fn fetch_gitee_manifest(manifest_url: &str) -> Result<ReleaseManifest, AppError> {
    let manifest_bytes = fetch_bytes(manifest_url, UpdateMirror::Gitee, MAX_UPDATE_MANIFEST_BYTES)?;
    let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| AppError::update_rejected(format!("Gitee 镜像清单格式无效：{error}")))?;
    Ok(manifest)
}

/// 按当前包类型从 Release 中定位签名清单；目录包先读取索引再选择补丁或完整包。
fn find_gitee_manifest_name(
    release: &GiteeReleaseMetadata,
    package_type: PackageType,
    current_version: &str,
) -> Result<Option<String>, AppError> {
    let Some(index_asset) = gitee_manifest_asset_names(package_type)
        .iter()
        .find(|name| release.assets.iter().any(|asset| asset.name == **name))
    else {
        return Ok(None);
    };
    if package_type != PackageType::Catalog {
        return Ok(Some((*index_asset).to_owned()));
    }

    let index_url = release.asset_url(index_asset)?;
    let index_bytes = fetch_bytes(&index_url, UpdateMirror::Gitee, MAX_UPDATE_MANIFEST_BYTES)?;
    let index: CatalogUpdateIndex = serde_json::from_slice(&index_bytes)
        .map_err(|error| AppError::update_rejected(format!("Gitee 目录索引格式无效：{error}")))?;
    if index.format != "yys-catalog-update-index"
        || index.protocol_version != CATALOG_INDEX_PROTOCOL_VERSION
        || parse_version(&index.latest_version).is_none()
    {
        return Err(AppError::update_rejected("目录索引格式或协议版本不受支持"));
    }
    let patch = index.releases.iter().filter(|entry| {
        entry.kind == CATALOG_DELTA_KIND_PATCH
            && entry.base_version.as_deref() == Some(current_version)
            && compare_versions(&entry.version, current_version) == std::cmp::Ordering::Greater
    });
    let full = index.releases.iter().filter(|entry| {
        entry.kind == "full"
            && entry.base_version.is_none()
            && compare_versions(&entry.version, current_version) == std::cmp::Ordering::Greater
    });
    let selected = patch
        .max_by(|left, right| compare_versions(&left.version, &right.version))
        .or_else(|| full.max_by(|left, right| compare_versions(&left.version, &right.version)));
    let Some(selected) = selected else {
        return Ok(None);
    };
    validate_asset_name(&selected.manifest_name)?;
    Ok(Some(selected.manifest_name.clone()))
}

/// 在已读出的 Release 清单中筛掉通道、版本、文件名和附件地址不合法的对象，选择最高有效版本。
/// 清单正文仍会在调用方进入签名、哈希和兼容性完整校验；这里不提前下载低优先级载荷。
fn choose_gitee_release(
    candidates: Vec<(GiteeReleaseMetadata, ReleaseManifest)>,
    package_type: PackageType,
    channel: UpdateChannel,
) -> Option<(GiteeReleaseMetadata, ReleaseManifest)> {
    candidates
        .into_iter()
        .filter(|(release, manifest)| {
            manifest.package_type == package_type
                && UpdateChannel::parse(&manifest.channel) == Some(channel)
                && parse_version(&manifest.version).is_some()
                && validate_asset_name(&manifest.artifact_name).is_ok()
                && release.assets.iter().any(|asset| {
                    asset.name == manifest.artifact_name
                        && asset.browser_download_url.starts_with("https://")
                })
        })
        .max_by(|(_, left), (_, right)| compare_versions(&left.version, &right.version))
}

/// 读取 Gitee Release 列表并选择当前通道最高版本，最后才下载其载荷。
fn fetch_gitee_release_package(
    mirror: UpdateMirror,
    api_url: &str,
    package_type: PackageType,
    current_version: &str,
    channel: UpdateChannel,
) -> Result<MirrorCandidate, AppError> {
    if mirror != UpdateMirror::Gitee {
        return Err(AppError::update_source_unavailable(
            mirror.as_str(),
            "当前只支持 Gitee 更新源",
        ));
    }
    let releases = fetch_gitee_release_metadata(api_url)?;
    let mut candidates = Vec::new();
    for release in releases {
        // Release 列表可能同时包含历史残缺附件；单个 Release 无效时跳过，不能阻断同通道有效版本。
        let Some(manifest_name) =
            (match find_gitee_manifest_name(&release, package_type, current_version) {
                Ok(name) => name,
                Err(_) => continue,
            })
        else {
            continue;
        };
        let Ok(manifest_url) = release.asset_url(&manifest_name) else {
            continue;
        };
        let Ok(manifest) = fetch_gitee_manifest(&manifest_url) else {
            continue;
        };
        candidates.push((release, manifest));
    }
    let selected = choose_gitee_release(candidates, package_type, channel);
    let Some((release, manifest)) = selected else {
        return Err(AppError::update_source_unavailable(
            "gitee",
            format!("没有找到高于当前版本的 {} 通道更新", channel.as_str()),
        ));
    };
    let artifact_url = release.asset_url(&manifest.artifact_name)?;
    let payload = fetch_bytes(
        &artifact_url,
        UpdateMirror::Gitee,
        MAX_UPDATE_ARTIFACT_BYTES,
    )?;
    Ok(MirrorCandidate {
        source: UpdateMirror::Gitee,
        manifest,
        payload,
        artifact_url: Some(artifact_url),
    })
}

/// 只遍历 Gitee Release 的清单并定位最高版本；启动检查不会读取选中安装包的正文。
fn fetch_gitee_release_metadata_candidate(
    mirror: UpdateMirror,
    api_url: &str,
    package_type: PackageType,
    current_version: &str,
    channel: UpdateChannel,
) -> Result<MirrorCandidate, AppError> {
    if mirror != UpdateMirror::Gitee {
        return Err(AppError::update_source_unavailable(
            mirror.as_str(),
            "当前只支持 Gitee 更新源",
        ));
    }
    let releases = fetch_gitee_release_metadata(api_url)?;
    let mut candidates = Vec::new();
    for release in releases {
        // 历史 Release 可能缺少清单或构件；无效项跳过，继续寻找同通道有效版本。
        let Some(manifest_name) =
            (match find_gitee_manifest_name(&release, package_type, current_version) {
                Ok(name) => name,
                Err(_) => continue,
            })
        else {
            continue;
        };
        let Ok(manifest_url) = release.asset_url(&manifest_name) else {
            continue;
        };
        let Ok(manifest) = fetch_gitee_manifest(&manifest_url) else {
            continue;
        };
        candidates.push((release, manifest));
    }
    let selected = choose_gitee_release(candidates, package_type, channel);
    let Some((release, manifest)) = selected else {
        return Err(AppError::update_source_unavailable(
            "gitee",
            format!("没有找到高于当前版本的 {} 通道更新", channel.as_str()),
        ));
    };
    let artifact_url = release.asset_url(&manifest.artifact_name)?;
    Ok(MirrorCandidate {
        source: UpdateMirror::Gitee,
        manifest,
        payload: Vec::new(),
        artifact_url: Some(artifact_url),
    })
}

/// 下载一个已定位的清单及其构件；清单索引和普通更新都复用同一下载边界。
fn fetch_manifest_and_payload(
    mirror: UpdateMirror,
    manifest_url: &str,
) -> Result<MirrorCandidate, AppError> {
    let manifest_bytes = fetch_bytes(manifest_url, mirror, MAX_UPDATE_MANIFEST_BYTES)?;
    let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        AppError::update_rejected(format!("{} 镜像清单格式无效：{error}", mirror.as_str()))
    })?;
    validate_asset_name(&manifest.artifact_name)?;
    let artifact_url = artifact_url(manifest_url, &manifest.artifact_name)?;
    let payload = fetch_bytes(&artifact_url, mirror, MAX_UPDATE_ARTIFACT_BYTES)?;
    Ok(MirrorCandidate {
        source: mirror,
        manifest,
        payload,
        artifact_url: Some(artifact_url),
    })
}

/// 读取单清单地址的元数据；构件正文留给用户点击更新后的安装阶段。
fn fetch_manifest_only(
    mirror: UpdateMirror,
    manifest_url: &str,
) -> Result<MirrorCandidate, AppError> {
    let manifest_bytes = fetch_bytes(manifest_url, mirror, MAX_UPDATE_MANIFEST_BYTES)?;
    let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        AppError::update_rejected(format!("{} 镜像清单格式无效：{error}", mirror.as_str()))
    })?;
    validate_asset_name(&manifest.artifact_name)?;
    let artifact_url = artifact_url(manifest_url, &manifest.artifact_name)?;
    Ok(MirrorCandidate {
        source: mirror,
        manifest,
        payload: Vec::new(),
        artifact_url: Some(artifact_url),
    })
}

/// 执行受限 HTTPS 下载；使用应用内客户端避免依赖外部 curl.exe 和 Windows 命令窗口。
///
/// 远程响应会同时受到状态码、Content-Length、读取上限和 HTTPS 重定向约束；失败时只返回
/// 不包含响应正文和完整 URL 的镜像故障，避免把服务器返回内容或查询参数泄漏到界面日志。
fn fetch_bytes(url: &str, source: UpdateMirror, limit: u64) -> Result<Vec<u8>, AppError> {
    let mut ignore_progress = |_: u64, _: u64| {};
    fetch_bytes_with_progress(url, source, limit, &mut ignore_progress)
}

/// 流式下载并报告已读字节；安装阶段用它把真实百分比传给启动更新弹窗。
fn fetch_bytes_with_progress(
    url: &str,
    source: UpdateMirror,
    limit: u64,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Vec<u8>, AppError> {
    if let Some(path) = url.strip_prefix("file://") {
        let path = Path::new(path);
        let size = fs::metadata(path)
            .map_err(|error| {
                AppError::update_source_unavailable(
                    source.as_str(),
                    format!("读取本地镜像失败：{error}"),
                )
            })?
            .len();
        if size > limit {
            return Err(resource_limit("updatePayloadBytes", size, limit));
        }
        let bytes = fs::read(path).map_err(|error| {
            AppError::update_source_unavailable(
                source.as_str(),
                format!("读取本地镜像失败：{error}"),
            )
        })?;
        if bytes.len() as u64 > limit {
            return Err(resource_limit(
                "updatePayloadBytes",
                bytes.len() as u64,
                limit,
            ));
        }
        progress(bytes.len() as u64, bytes.len() as u64);
        return Ok(bytes);
    }
    if !url.starts_with("https://") {
        return Err(AppError::update_source_unavailable(
            source.as_str(),
            "更新源不是 HTTPS 地址",
        ));
    }
    // reqwest 默认使用应用内 TLS 栈，避免调用外部 curl.exe 时触发 Windows Schannel 凭据错误。
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(UPDATE_NETWORK_TIMEOUT_SECONDS))
        // 重定向仍必须保持 HTTPS，不能因为托管平台返回跳转就降级到明文 HTTP。
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.url().scheme() == "https" {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|_| {
            AppError::update_source_unavailable(source.as_str(), "无法创建 HTTPS 客户端")
        })?;
    let response = client
        .get(url)
        .send()
        .map_err(|_| AppError::update_source_unavailable(source.as_str(), "HTTPS 请求失败"))?;
    if !response.status().is_success() {
        return Err(AppError::update_source_unavailable(
            source.as_str(),
            format!("HTTPS 请求返回状态码 {}", response.status()),
        ));
    }
    if let Some(size) = response.content_length()
        && size > limit
    {
        return Err(resource_limit("updatePayloadBytes", size, limit));
    }
    // 多读一个字节用于识别没有 Content-Length 或服务端声明不准确的超大响应。
    let read_limit = limit.saturating_add(1);
    let total = response.content_length().unwrap_or(limit);
    let capacity = response
        .content_length()
        .and_then(|size| usize::try_from(size).ok())
        .unwrap_or(8 * 1024)
        .min(usize::try_from(limit).unwrap_or(usize::MAX));
    let mut bytes = Vec::with_capacity(capacity);
    let mut response = response.take(read_limit);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = response.read(&mut buffer).map_err(|_| {
            AppError::update_source_unavailable(source.as_str(), "HTTPS 响应读取失败")
        })?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        progress(bytes.len() as u64, total);
    }
    if bytes.len() as u64 > limit {
        return Err(resource_limit(
            "updatePayloadBytes",
            bytes.len() as u64,
            limit,
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试验证器只替代密码学边界，不改变生产校验的格式、哈希和版本约束。
    #[derive(Default)]
    struct AcceptVerifier;

    impl UpdateSignatureVerifier for AcceptVerifier {
        fn verify(&self, _manifest: &ReleaseManifest) -> bool {
            true
        }
    }

    fn manifest(source: UpdateMirror, version: &str, payload: &[u8]) -> MirrorCandidate {
        MirrorCandidate {
            source,
            manifest: ReleaseManifest {
                format: "yys-signed-update-manifest".to_owned(),
                protocol_version: UPDATE_PROTOCOL_VERSION,
                channel: STABLE_CHANNEL.to_owned(),
                package_type: PackageType::Rules,
                package_id: "rules-default".to_owned(),
                version: version.to_owned(),
                min_app_version: "0.1.0".to_owned(),
                compatible_game_version: None,
                content_sha256: sha256_hex(payload),
                content_size: payload.len() as u64,
                artifact_name: "rules.bin".to_owned(),
                signer_key_id: "test".to_owned(),
                signature: "0".repeat(128),
                adapter_capabilities: None,
                release_notes: "测试更新".to_owned(),
                delta_kind: None,
                base_version: None,
                base_content_sha256: None,
                result_content_sha256: None,
            },
            payload: payload.to_vec(),
            artifact_url: None,
        }
    }

    /// 构造 Release 附件摘要；选择器测试只关心清单与安装包附件是否同时存在。
    fn gitee_release(manifest: &ReleaseManifest, include_artifact: bool) -> GiteeReleaseMetadata {
        let mut assets = vec![GiteeReleaseAsset {
            name: "release-manifest.json".to_owned(),
            browser_download_url: "https://gitee.com/test/release-manifest.json".to_owned(),
        }];
        if include_artifact {
            assets.push(GiteeReleaseAsset {
                name: manifest.artifact_name.clone(),
                browser_download_url: format!("https://gitee.com/test/{}", manifest.artifact_name),
            });
        }
        GiteeReleaseMetadata { assets }
    }

    #[test]
    fn 版本比较补齐缺失段且拒绝非法版本路径() {
        assert_eq!(compare_versions("1.2", "1.2.0"), std::cmp::Ordering::Equal);
        assert_eq!(
            compare_versions("1.3.0", "1.2.9"),
            std::cmp::Ordering::Greater
        );
        assert!(validate_version_path("1.2.3").is_ok());
        assert!(validate_version_path("../payload").is_err());
    }

    #[test]
    fn SemVer预发布版本低于正式版且按数字标识比较() {
        assert_eq!(
            compare_versions("0.1.0-beta.2", "0.1.0-beta.3"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("0.1.0-beta.3", "0.1.0"),
            std::cmp::Ordering::Less
        );
        assert!(is_version_at_least("0.1.0-beta.2", "0.1.0-beta.2"));
        assert_eq!(
            compare_versions("0.1.0-test.10", "0.1.0-test.2"),
            std::cmp::Ordering::Greater
        );
        assert!(parse_version("0.1.0-beta.2").is_some());
        assert!(parse_version("0.1.0-beta.02").is_none());
    }

    #[test]
    fn 稳定和测试清单不能跨通道选择() {
        let stable = validate_release(
            manifest(UpdateMirror::Gitee, "1.1.0", b"stable"),
            PackageType::Rules,
            "0.1.0",
            None,
            &AcceptVerifier,
        )
        .expect("稳定候选有效");
        let mut test_candidate = manifest(UpdateMirror::Gitee, "1.2.0", b"test");
        test_candidate.manifest.channel = TEST_CHANNEL.to_owned();
        assert!(
            validate_release(
                test_candidate.clone(),
                PackageType::Rules,
                "0.1.0",
                None,
                &AcceptVerifier,
            )
            .is_err()
        );
        let test = validate_release_for_channel(
            test_candidate,
            PackageType::Rules,
            UpdateChannel::Test,
            "0.1.0",
            None,
            &AcceptVerifier,
        )
        .expect("测试候选有效");
        assert_eq!(
            choose_release_for_channel(&[stable.clone(), test.clone()], UpdateChannel::Stable)
                .expect("稳定通道可选")
                .expect("应有稳定候选")
                .manifest
                .version,
            stable.manifest.version
        );
        assert_eq!(
            choose_release_for_channel(&[stable, test], UpdateChannel::Test)
                .expect("测试通道可选")
                .expect("应有测试候选")
                .manifest
                .version,
            "1.2.0"
        );
    }

    #[test]
    fn gitee_source_selects_only_gitee_candidate() {
        let first = validate_release(
            manifest(UpdateMirror::Gitee, "1.1.0", b"gitee"),
            PackageType::Rules,
            "0.1.0",
            None,
            &AcceptVerifier,
        )
        .expect("Gitee 候选有效");
        let selected = choose_release(&[first], UpdateSourceMode::Gitee)
            .expect("Gitee 候选可选")
            .expect("应有候选");
        assert_eq!(selected.manifest.version, "1.1.0");
    }

    #[test]
    fn gitee_source_ignores_legacy_github_candidate() {
        let github = validate_release(
            manifest(UpdateMirror::Github, "1.2.0", b"same"),
            PackageType::Rules,
            "0.1.0",
            None,
            &AcceptVerifier,
        )
        .expect("GitHub 候选有效");
        assert!(
            choose_release(&[github], UpdateSourceMode::Gitee)
                .expect("旧镜像不应阻断 Gitee")
                .is_none()
        );
    }

    #[test]
    fn Gitee多个Release选择最高有效版本并跳过缺少附件的版本() {
        let low = manifest(UpdateMirror::Gitee, "1.1.0", b"low");
        let middle = manifest(UpdateMirror::Gitee, "1.2.0", b"middle");
        let high = manifest(UpdateMirror::Gitee, "1.3.0", b"high");
        let selected = choose_gitee_release(
            vec![
                (gitee_release(&low.manifest, true), low.manifest),
                (gitee_release(&middle.manifest, true), middle.manifest),
                (gitee_release(&high.manifest, false), high.manifest),
            ],
            PackageType::Rules,
            UpdateChannel::Stable,
        )
        .expect("应选择有效 Release");
        assert_eq!(selected.1.version, "1.2.0");
    }

    #[test]
    fn 不满足哈希或兼容范围时绝不返回已验证候选() {
        let mut candidate = manifest(UpdateMirror::Github, "1.1.0", b"payload");
        candidate.manifest.content_sha256 = "f".repeat(64);
        assert!(
            validate_release(
                candidate,
                PackageType::Rules,
                "0.1.0",
                None,
                &AcceptVerifier,
            )
            .is_err()
        );

        let mut incompatible = manifest(UpdateMirror::Github, "1.1.0", b"payload");
        incompatible.manifest.compatible_game_version = Some("9.9.9".to_owned());
        assert!(
            validate_release(
                incompatible,
                PackageType::Rules,
                "0.1.0",
                Some("1.0.0"),
                &AcceptVerifier,
            )
            .is_err()
        );
    }

    #[test]
    fn 签名载荷字段顺序稳定且不包含私钥() {
        let candidate = manifest(UpdateMirror::Github, "1.1.0", b"payload");
        let payload = canonical_manifest_payload(&candidate.manifest);
        assert!(payload.starts_with("\"yys-signed-update-manifest\"\n"));
        assert!(!payload.contains("private_key"));
    }

    #[test]
    fn 目录补丁字段进入签名且通过完整清单校验() {
        let mut candidate = manifest(UpdateMirror::Github, "1.2.0", b"patch-payload");
        candidate.manifest.package_type = PackageType::Catalog;
        candidate.manifest.package_id = "yys-catalog".to_owned();
        candidate.manifest.artifact_name = "catalog.patch.json".to_owned();
        candidate.manifest.delta_kind = Some("patch".to_owned());
        candidate.manifest.base_version = Some("1.1.0".to_owned());
        candidate.manifest.base_content_sha256 = Some("a".repeat(64));
        candidate.manifest.result_content_sha256 = Some("b".repeat(64));
        let payload = canonical_manifest_payload(&candidate.manifest);
        assert_eq!(payload.lines().count(), 17);
        assert!(
            validate_release(
                candidate,
                PackageType::Catalog,
                "0.1.0",
                None,
                &AcceptVerifier,
            )
            .is_ok()
        );
    }

    #[test]
    fn 目录补丁基础版本不匹配时拒绝应用() {
        let mut candidate = manifest(UpdateMirror::Github, "1.2.0", b"patch-payload");
        candidate.manifest.package_type = PackageType::Catalog;
        candidate.manifest.delta_kind = Some("patch".to_owned());
        candidate.manifest.base_version = Some("1.1.0".to_owned());
        assert!(validate_catalog_base(&candidate.manifest, "1.0.0").is_err());
        assert!(validate_catalog_base(&candidate.manifest, "1.1.0").is_ok());
    }

    #[test]
    fn 非目录包不能伪装成增量补丁() {
        let mut candidate = manifest(UpdateMirror::Github, "1.2.0", b"patch-payload");
        candidate.manifest.delta_kind = Some("patch".to_owned());
        candidate.manifest.base_version = Some("1.1.0".to_owned());
        assert!(
            validate_release(
                candidate,
                PackageType::Rules,
                "0.1.0",
                None,
                &AcceptVerifier,
            )
            .is_err()
        );
    }

    #[test]
    fn 适配器发行清单沿用只读适配器签名字段顺序() {
        let mut candidate = manifest(UpdateMirror::Github, "1.1.0", b"payload");
        candidate.manifest.package_type = PackageType::Adapter;
        candidate.manifest.adapter_capabilities = Some(AdapterCapabilities {
            read_soul_data: true,
            write_memory: false,
            simulate_input: false,
            execute_remote_code: false,
        });
        let payload = canonical_manifest_payload(&candidate.manifest);
        assert_eq!(payload.lines().count(), 11);
        assert!(payload.starts_with("rules-default\n1.1.0\nstable\n0.1.0\n*\n"));
    }

    #[test]
    fn 固定下载器错误不应泄漏响应正文() {
        let error = fetch_bytes(
            "https://127.0.0.1:1/unreachable",
            UpdateMirror::Github,
            MAX_UPDATE_ARTIFACT_BYTES,
        )
        .expect_err("连接应失败");
        assert!(!error.message.contains("private"));
    }

    #[cfg(windows)]
    #[test]
    fn 自动更新助手必须有超时提权退出码校验和失败回退() {
        let script = build_restart_script(
            1234,
            Path::new(r"C:\update dir\installer.exe"),
            Path::new(r"C:\app dir\平安志.exe"),
            Path::new(r"C:\update dir\restart.log"),
        );
        assert!(script.contains("AddSeconds(10)"));
        assert!(script.contains("-Verb RunAs"));
        assert!(script.contains("$process.ExitCode -ne 0"));
        assert!(script.contains("自动更新失败，保留旧版本"));
        assert!(script.contains("等待旧版本进程退出超时"));
        assert!(!script.contains("$ErrorActionPreference = 'SilentlyContinue'"));
        assert_eq!(
            &powershell_script_bytes(&script)[..3],
            &[0xEF, 0xBB, 0xBF],
            "PowerShell 脚本必须带 UTF-8 BOM，才能正确解析中文路径"
        );
        let script_path = std::env::temp_dir().join(format!("yys-update-script-{}.ps1", Uuid::new_v4()));
        fs::write(&script_path, powershell_script_bytes(&script)).expect("写入 PowerShell 测试脚本");
        // PowerShell 5.1 的 -Command 不会可靠地把后置参数映射到 $args，测试必须把脚本路径直接写入解析命令。
        let parse_command = format!(
            "$tokens=$null;$errors=$null;[System.Management.Automation.Language.Parser]::ParseFile('{}',[ref]$tokens,[ref]$errors)|Out-Null;if($errors.Count -gt 0){{$errors|ForEach-Object{{Write-Error $_.Message}};exit 1}}",
            powershell_literal(&script_path)
        );
        let output = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &parse_command,
            ])
            .output()
            .expect("运行 PowerShell 脚本解析器");
        let _ = fs::remove_file(&script_path);
        assert!(
            output.status.success(),
            "更新助手脚本语法无效：{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
