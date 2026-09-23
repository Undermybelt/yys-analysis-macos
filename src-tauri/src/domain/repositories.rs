//! 仓库特质定义领域层所需的数据访问契约。
//!
//! 特质只依赖领域实体与 `AppError`，不暴露 SQL、连接或事务细节。
//! 具体实现位于 `infrastructure::repositories`。

use crate::application::error::AppError;

use super::models::{
    AcquisitionEvent, BackupRecord, GameProfile, GroupMember, HeadTailCache, HeadTailCard,
    ImportCommitRequest, ImportCommitResult, InstalledPackage, InventoryItem, InventoryPage,
    InventoryPageQuery, MaturityThreshold, NewGameProfile, RawObject, RuleActivation,
    RuleVersionRecord, SetCommonness, Snapshot, SnapshotSoul, SoulAttribute, SoulRadarCache,
    SoulRadarPoint, SoulSet, YuhunGroup,
};

/// 游戏档案仓库。
pub trait ProfileRepository: Send + Sync {
    /// 创建新档案并返回完整实体。
    fn create(&self, new: &NewGameProfile, now: &str) -> Result<GameProfile, AppError>;

    /// 列出档案；默认排除已归档档案。
    fn list(&self, include_archived: bool) -> Result<Vec<GameProfile>, AppError>;

    /// 按 ID 查询档案。
    fn get(&self, id: &str) -> Result<Option<GameProfile>, AppError>;

    /// 重命名档案，使用修订号做乐观并发检查。
    fn rename(
        &self,
        id: &str,
        new_name: &str,
        expected_revision: u32,
        now: &str,
    ) -> Result<GameProfile, AppError>;

    /// 归档或取消归档档案。
    fn set_archived(
        &self,
        id: &str,
        archived: bool,
        expected_revision: u32,
        now: &str,
    ) -> Result<GameProfile, AppError>;

    /// 查询系统默认成熟度阈值。
    fn system_thresholds(&self) -> Result<Vec<MaturityThreshold>, AppError>;

    /// 查询档案覆盖的成熟度阈值。
    fn profile_thresholds(&self, profile_id: &str) -> Result<Vec<MaturityThreshold>, AppError>;

    /// 覆盖档案的成熟度阈值（整体替换，scope_type 固定为 profile）。
    fn replace_profile_thresholds(
        &self,
        profile_id: &str,
        thresholds: &[MaturityThreshold],
    ) -> Result<(), AppError>;
}

/// 原始对象仓库。
pub trait RawObjectRepository: Send + Sync {
    /// 写入或复用已存在的原始对象记录。
    fn upsert(&self, object: &RawObject) -> Result<(), AppError>;

    /// 按 SHA-256 查询原始对象。
    fn get(&self, sha256: &str) -> Result<Option<RawObject>, AppError>;

    /// 汇总统计（数量、字节数），供诊断页使用。
    fn stats(&self) -> Result<RawObjectStats, AppError>;
}

/// 原始对象统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawObjectStats {
    pub object_count: u64,
    pub raw_bytes: u64,
    pub stored_bytes: u64,
}

/// 采集事件仓库。
pub trait AcquisitionEventRepository: Send + Sync {
    /// 写入一次采集事件。
    fn insert(&self, event: &AcquisitionEvent) -> Result<(), AppError>;

    /// 按档案列出最近采集事件。
    fn list_by_profile(
        &self,
        profile_id: &str,
        limit: u32,
    ) -> Result<Vec<AcquisitionEvent>, AppError>;
}

/// 采集快照仓库。
pub trait SnapshotRepository: Send + Sync {
    /// 写入快照及其御魂与副属性，整体在一个事务中完成。
    fn insert_snapshot(
        &self,
        snapshot: &Snapshot,
        souls: &[SnapshotSoul],
        attributes: &[SoulAttribute],
    ) -> Result<(), AppError>;

    /// 按 ID 查询快照。
    fn get(&self, id: &str) -> Result<Option<Snapshot>, AppError>;

    /// 按档案列出快照（最近在前）。
    fn list_by_profile(&self, profile_id: &str) -> Result<Vec<Snapshot>, AppError>;

    /// 写入或更新库存投影项。
    fn upsert_inventory(&self, item: &InventoryItem) -> Result<(), AppError>;

    /// 读取档案当前库存投影，按稳定键排序以便界面稳定展示。
    fn list_inventory(&self, profile_id: &str) -> Result<Vec<InventoryItem>, AppError>;

    /// 按筛选条件分页读取当前库存；筛选、计数和 LIMIT/OFFSET 必须在数据库侧完成。
    fn list_inventory_page(
        &self,
        profile_id: &str,
        query: &InventoryPageQuery,
    ) -> Result<InventoryPage, AppError>;

    /// 一次性读取当前档案的所有库存事实，供组合搜索等只读分析使用。
    fn list_inventory_facts(
        &self,
        profile_id: &str,
    ) -> Result<
        (
            Vec<InventoryItem>,
            Vec<SnapshotSoul>,
            Vec<SoulAttribute>,
            u32,
        ),
        AppError,
    >;

    /// 清空档案库存及其评分、分析和雷达派生结果，但保留目录、规则和原始对象历史。
    fn clear_inventory(&self, profile_id: &str) -> Result<(), AppError>;
}

/// 雷达缓存仓库；只保存聚合结果，不把整份御魂正文复制到派生表。
pub trait SoulRadarRepository: Send + Sync {
    /// 覆盖某个档案的旧雷达结果，缓存元数据和点位必须在同一事务中提交。
    fn replace(
        &self,
        profile_id: &str,
        calculated_at: &str,
        inventory_revision: &str,
        inventory_count: u32,
        points: &[SoulRadarPoint],
    ) -> Result<(), AppError>;

    /// 读取已保存结果，并附带当前库存的轻量版本信息，不重新读取全部御魂事实。
    fn get(&self, profile_id: &str) -> Result<Option<SoulRadarCache>, AppError>;

    /// 只查询当前库存数量和更新时间版本，用于计算缓存是否过期。
    fn current_inventory_state(&self, profile_id: &str) -> Result<(u32, String), AppError>;
}

/// 头尾缓存仓库；保存当前档案全部候选头、尾及其库存版本。
pub trait HeadTailRepository: Send + Sync {
    /// 在同一事务内覆盖旧结果和全部头尾卡片，避免只写入一半结果。
    fn replace(
        &self,
        profile_id: &str,
        calculated_at: &str,
        inventory_revision: &str,
        inventory_count: u32,
        heads: &[HeadTailCard],
        tails: &[HeadTailCard],
        head_candidate_count: u32,
        tail_candidate_count: u32,
    ) -> Result<(), AppError>;

    /// 读取已保存的头尾结果，并附带当前库存版本信息。
    fn get(&self, profile_id: &str) -> Result<Option<HeadTailCache>, AppError>;

    /// 只读取当前库存的数量和更新时间，用于判断缓存是否过期。
    fn current_inventory_state(&self, profile_id: &str) -> Result<(u32, String), AppError>;
}

/// 导入事务仓库：保证重复判断、快照、事件和库存投影不会产生半成品。
pub trait ImportRepository: Send + Sync {
    /// 按内容与范围指纹去重，并按完整/局部语义更新库存投影。
    fn commit_import(&self, request: &ImportCommitRequest) -> Result<ImportCommitResult, AppError>;

    /// 列出快照中保存的规范御魂事实，供历史和导入结果摘要使用。
    fn list_snapshot_souls(
        &self,
        snapshot_id: &str,
    ) -> Result<(Vec<SnapshotSoul>, Vec<SoulAttribute>), AppError>;

    /// 只读取分页结果对应的快照事实，禁止为展示 10 枚御魂重新加载整份快照。
    fn list_snapshot_souls_by_ids(
        &self,
        snapshot_id: &str,
        internal_ids: &[String],
    ) -> Result<(Vec<SnapshotSoul>, Vec<SoulAttribute>), AppError>;
}

/// 御魂与式神目录仓库。
pub trait CatalogRepository: Send + Sync {
    /// 安装新目录版本：写入包、分组、套装、式神、分组关系与常用度默认值，并激活该版本。
    fn install(
        &self,
        package: &super::models::CatalogPackage,
        groups: &[YuhunGroup],
        sets: &[SoulSet],
        shikigami: &[super::models::Shikigami],
        group_members: &[GroupMember],
        commonness: &[SetCommonness],
    ) -> Result<(), AppError>;

    /// 激活已经写入数据库的目录版本；只切换活动指针，不复制或修改历史目录事实。
    fn activate_version(&self, catalog_version: &str) -> Result<(), AppError>;

    /// 当前激活的目录版本及其套装和分组数量。
    fn active_status(&self) -> Result<Option<CatalogStatus>, AppError>;

    /// 按目录版本列出套装。
    fn list_sets(&self, catalog_version: &str) -> Result<Vec<SoulSet>, AppError>;

    /// 按目录版本列出式神；调用方负责将版本作为历史快照的只读引用。
    fn list_shikigami(
        &self,
        catalog_version: &str,
    ) -> Result<Vec<super::models::Shikigami>, AppError>;

    /// 按目录版本查询套装。
    fn get_set(&self, catalog_version: &str, set_id: &str) -> Result<Option<SoulSet>, AppError>;

    /// 列出全部用途分组。
    fn list_groups(&self) -> Result<Vec<YuhunGroup>, AppError>;

    /// 列出分组内的套装成员。
    fn group_members(&self, group_id: &str) -> Result<Vec<GroupMember>, AppError>;
}

/// 目录状态摘要。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogStatus {
    pub version: String,
    pub set_count: u64,
    pub group_count: u64,
    pub source: Option<String>,
    pub sha256: Option<String>,
    pub parser_version: Option<String>,
    pub validation_status: String,
    pub validation_errors: Vec<String>,
    pub icon_coverage: u64,
    pub mechanics_coverage: u64,
    pub shikigami_count: u64,
    pub panel_coverage: u64,
    pub skill_coverage: u64,
}

use serde::Serialize;

/// 套装常用度仓库。
pub trait CommonnessRepository: Send + Sync {
    /// 查询目录版本的全局默认常用度。
    fn defaults(&self, catalog_version: &str) -> Result<Vec<SetCommonness>, AppError>;

    /// 查询档案级常用度覆盖。
    fn overrides(
        &self,
        profile_id: &str,
    ) -> Result<Vec<super::models::ProfileCommonness>, AppError>;

    /// 合并默认值与档案覆盖，返回有效常用度。
    fn effective(
        &self,
        profile_id: &str,
        catalog_version: &str,
    ) -> Result<Vec<super::models::EffectiveCommonness>, AppError>;

    /// 设置或更新档案覆盖；恢复默认时调用方可删除该行。
    fn upsert_override(&self, override_: &super::models::ProfileCommonness)
    -> Result<(), AppError>;

    /// 删除档案覆盖，恢复默认值。
    fn delete_override(
        &self,
        profile_id: &str,
        set_id: &str,
        scenario: &str,
    ) -> Result<(), AppError>;

    /// 将源档案的覆盖复制到目标档案（先清空目标覆盖）。
    fn copy_overrides(&self, from_profile: &str, to_profile: &str) -> Result<usize, AppError>;
}

/// 备份记录仓库。
pub trait BackupRecordRepository: Send + Sync {
    /// 写入备份记录。
    fn insert(&self, record: &BackupRecord) -> Result<(), AppError>;

    /// 列出备份记录（最近在前）。
    fn list(&self) -> Result<Vec<BackupRecord>, AppError>;

    /// 删除指定备份记录；实体文件由备份管理器先行清理。
    fn delete(&self, id: &str) -> Result<(), AppError>;

    /// 标记备份为已恢复。
    fn mark_restored(&self, id: &str, now: &str) -> Result<(), AppError>;
}

/// 已安装数据包仓库；只保存可追溯的版本、来源与活动指针，不保存私钥或远程载荷正文。
pub trait PackageRepository: Send + Sync {
    /// 记录一个已完成验证的数据包，并将同类型其他版本置为非活动。
    fn install(&self, package: &InstalledPackage) -> Result<(), AppError>;

    /// 列出所有历史数据包，最近安装的在前。
    fn list(&self) -> Result<Vec<InstalledPackage>, AppError>;

    /// 将用户明确选择的历史版本设为活动版本。
    fn activate(&self, package_type: &str, version: &str) -> Result<(), AppError>;
}

/// 规则版本仓库；只保存已通过领域校验的版本正文和档案级启用关系。
pub trait RuleRepository: Send + Sync {
    /// 列出规则版本，按最近保存时间倒序。
    fn list_versions(&self) -> Result<Vec<RuleVersionRecord>, AppError>;

    /// 按稳定版本 ID 查询规则版本。
    fn get_version(&self, version_id: &str) -> Result<Option<RuleVersionRecord>, AppError>;

    /// 写入新规则版本；同一版本 ID 不允许覆盖已有正文。
    fn insert_version(&self, version: &RuleVersionRecord) -> Result<(), AppError>;

    /// 更新已有的用户规则正文，保留原规则 ID 以避免保存时产生重复标准。
    fn update_version(&self, version: &RuleVersionRecord) -> Result<(), AppError>;

    /// 删除用户拥有的评分标准；系统内置标准由应用层阻止删除。
    fn delete_version(&self, version_id: &str) -> Result<(), AppError>;

    /// 列出档案级启用关系。
    fn list_activations(&self, profile_id: &str) -> Result<Vec<RuleActivation>, AppError>;

    /// 设置档案级启用关系，关闭规则时仍保留历史关系以便恢复。
    fn set_activation(&self, activation: &RuleActivation) -> Result<(), AppError>;

    /// 查询当前全局启用的评分标准；全局最多同时启用一条标准。
    fn active_score_standard_id(&self) -> Result<Option<String>, AppError>;

    /// 切换全局启用的评分标准，并将切换结果持久化到 SQL。
    fn set_active_score_standard(
        &self,
        rule_version_id: &str,
        note: Option<&str>,
    ) -> Result<(), AppError>;
}
