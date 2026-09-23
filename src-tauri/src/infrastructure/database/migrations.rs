//! 数据库迁移定义与迁移运行器。
//!
//! 迁移采用编号顺序执行，已应用的迁移通过校验和检测漂移。
//! 所有待应用迁移在一个 `Immediate` 事务中执行，失败时自动回滚本次升级。

use crate::application::error::AppError;
use rusqlite::{Connection, TransactionBehavior};
use sha2::{Digest, Sha256};

/// 单个迁移版本。
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// 迁移运行器。
pub struct MigrationRunner {
    migrations: Vec<Migration>,
}

impl MigrationRunner {
    /// 使用内置迁移列表创建运行器。
    pub fn builtin() -> Self {
        Self {
            migrations: migration_list(),
        }
    }

    /// 返回当前最新 schema 版本（0 表示无迁移）。
    pub fn latest_version(&self) -> u32 {
        self.migrations.iter().map(|m| m.version).max().unwrap_or(0)
    }

    /// 运行未应用的迁移，并校验已应用迁移的完整性。
    /// 返回迁移后的当前 schema 版本。
    pub fn run(&self, connection: &mut Connection) -> Result<u32, AppError> {
        self.bootstrap(connection)?;

        // 迁移列表是版本化协议，重复版本会让回滚点和校验和失去确定性。
        let mut known_versions = std::collections::HashSet::new();
        for migration in &self.migrations {
            if !known_versions.insert(migration.version) {
                return Err(AppError::migration_failed(
                    migration.version,
                    "当前迁移列表包含重复版本",
                ));
            }
        }

        let applied = self.load_applied(connection)?;
        self.verify_checksums(&applied)?;
        self.verify_no_incompatible_versions(&applied)?;

        let current = applied.iter().map(|r| r.version).max().unwrap_or(0);
        // 不能只取最大版本：中间迁移缺失时，直接继续会把数据库标记成不可解释的状态。
        let applied_versions: std::collections::HashSet<u32> =
            applied.iter().map(|row| row.version).collect();
        for migration in &self.migrations {
            if migration.version <= current && !applied_versions.contains(&migration.version) {
                return Err(AppError::migration_failed(
                    migration.version,
                    "数据库缺少前置迁移版本",
                ));
            }
        }

        let mut pending: Vec<&Migration> = self
            .migrations
            .iter()
            .filter(|m| m.version > current)
            .collect();
        pending.sort_by_key(|migration| migration.version);

        if pending.is_empty() {
            return Ok(current);
        }

        let app_version = env!("CARGO_PKG_VERSION");
        // 所有待应用迁移共用一个事务。这样第 N+1 个迁移失败时，N 及之前的
        // 结构变化和 schema_migration 记录也会一起回滚，数据库仍可从升级前继续启动。
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::migration_failed(
                    pending[0].version,
                    &format!("无法启动迁移事务：{error}"),
                )
            })?;

        for migration in &pending {
            let checksum = compute_checksum(migration.sql);
            let started_at = format_iso_now();

            if let Err(error) = tx.execute_batch(migration.sql) {
                let _ = tx.rollback();
                return Err(AppError::migration_failed(
                    migration.version,
                    &format!("SQL 执行失败：{error}"),
                ));
            }

            let finished_at = format_iso_now();

            tx.execute(
                "INSERT INTO schema_migration (version, checksum, started_at, finished_at, result, applied_by) \
                 VALUES (?1, ?2, ?3, ?4, 'applied', ?5)",
                rusqlite::params![migration.version, checksum, started_at, finished_at, app_version],
            )
            .map_err(|error| {
                AppError::migration_failed(migration.version, &format!("记录迁移失败：{error}"))
            })?;

            tracing::info!(
                version = migration.version,
                name = migration.name,
                "数据库迁移已应用"
            );
        }

        tx.commit().map_err(|error| {
            AppError::migration_failed(
                pending
                    .last()
                    .map(|migration| migration.version)
                    .unwrap_or(current),
                &format!("提交迁移失败：{error}"),
            )
        })?;

        Ok(self.latest_version())
    }

    /// 创建 schema_migration 表（如果不存在）。
    fn bootstrap(&self, connection: &Connection) -> Result<(), AppError> {
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migration (
                    version INTEGER PRIMARY KEY NOT NULL,
                    checksum TEXT NOT NULL,
                    started_at TEXT NOT NULL,
                    finished_at TEXT NOT NULL,
                    result TEXT NOT NULL,
                    applied_by TEXT NOT NULL
                ) STRICT;",
            )
            .map_err(|error| AppError::database("自举迁移表", &error))
    }

    /// 加载已应用的迁移记录。
    fn load_applied(&self, connection: &Connection) -> Result<Vec<AppliedMigration>, AppError> {
        let mut stmt = connection
            .prepare("SELECT version, checksum, started_at, finished_at, result, applied_by FROM schema_migration ORDER BY version")
            .map_err(|error| AppError::database("查询迁移记录", &error))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(AppliedMigration {
                    version: row.get(0)?,
                    checksum: row.get(1)?,
                    _started_at: row.get(2)?,
                    _finished_at: row.get(3)?,
                    _result: row.get(4)?,
                    _applied_by: row.get(5)?,
                })
            })
            .map_err(|error| AppError::database("读取迁移记录", &error))?;

        let mut applied = Vec::new();
        for row in rows {
            applied.push(row.map_err(|error| AppError::database("解析迁移记录", &error))?);
        }
        Ok(applied)
    }

    /// 校验已应用迁移的校验和与当前迁移列表一致。
    fn verify_checksums(&self, applied: &[AppliedMigration]) -> Result<(), AppError> {
        for applied_row in applied {
            if let Some(migration) = self
                .migrations
                .iter()
                .find(|m| m.version == applied_row.version)
            {
                let expected = compute_checksum(migration.sql);
                if applied_row.checksum != expected {
                    return Err(AppError::migration_failed(
                        applied_row.version,
                        &format!(
                            "校验和不匹配：预期 {expected}，记录为 {}",
                            applied_row.checksum
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// 检查已应用迁移与当前版本是否兼容。
    /// 当前最新版本以内的未知迁移仍视为历史漂移并拒绝；只允许比当前程序
    /// 更高的迁移号继续启动，使用户回退到旧版时仍能进入界面并完成自动更新。
    fn verify_no_incompatible_versions(
        &self,
        applied: &[AppliedMigration],
    ) -> Result<(), AppError> {
        let known_versions: std::collections::HashSet<u32> =
            self.migrations.iter().map(|m| m.version).collect();
        let latest_known = self.latest_version();
        for applied_row in applied {
            if !known_versions.contains(&applied_row.version) && applied_row.version <= latest_known
            {
                return Err(AppError::migration_failed(
                    applied_row.version,
                    "数据库中存在当前迁移列表中缺失的历史版本",
                ));
            }
            if !known_versions.contains(&applied_row.version) {
                tracing::warn!(
                    database_version = applied_row.version,
                    supported_version = latest_known,
                    "数据库来自更高版本，当前程序以兼容模式启动以便完成更新"
                );
            }
        }
        Ok(())
    }

    /// 针对测试：允许注入自定义迁移列表。
    #[cfg(test)]
    pub fn new_with_migrations(migrations: Vec<Migration>) -> Self {
        Self { migrations }
    }
}

/// 已应用的迁移记录。
struct AppliedMigration {
    version: u32,
    checksum: String,
    _started_at: String,
    _finished_at: String,
    _result: String,
    _applied_by: String,
}

/// 计算迁移 SQL 的 SHA-256 校验和。
fn compute_checksum(sql: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    hex::encode(hasher.finalize())
}

/// 生成当前 UTC 时间的 ISO 8601 字符串。
fn format_iso_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ─── 迁移 SQL ─────────────────────────────────────────────────────────────────

fn migration_list() -> Vec<Migration> {
    vec![
        Migration {
            version: 1,
            name: "application-meta",
            sql: MIGRATION_1,
        },
        Migration {
            version: 2,
            name: "game-profiles",
            sql: MIGRATION_2,
        },
        Migration {
            version: 3,
            name: "raw-objects-snapshots",
            sql: MIGRATION_3,
        },
        Migration {
            version: 4,
            name: "catalog-and-commonness",
            sql: MIGRATION_4,
        },
        Migration {
            version: 5,
            name: "backup-records",
            sql: MIGRATION_5,
        },
        Migration {
            version: 6,
            name: "default-maturity-thresholds",
            sql: MIGRATION_6,
        },
        Migration {
            version: 7,
            name: "snapshot-scope-fingerprint",
            sql: MIGRATION_7,
        },
        Migration {
            version: 8,
            name: "analysis-actions",
            sql: MIGRATION_8,
        },
        Migration {
            version: 9,
            name: "rule-versions-and-activations",
            sql: MIGRATION_9,
        },
        Migration {
            version: 10,
            name: "snapshot-integrity-status",
            sql: MIGRATION_10,
        },
        Migration {
            version: 11,
            name: "installed-update-packages",
            sql: MIGRATION_11,
        },
        Migration {
            version: 12,
            name: "soul-catalog-details-and-mechanics",
            sql: MIGRATION_12,
        },
        Migration {
            version: 13,
            name: "catalog-package-validation-metadata",
            sql: MIGRATION_13,
        },
        Migration {
            version: 14,
            name: "soul-catalog-special-category",
            sql: MIGRATION_14,
        },
        Migration {
            version: 15,
            name: "imported-rule-versions-editable",
            sql: MIGRATION_15,
        },
        Migration {
            version: 16,
            name: "separate-imported-rule-identities",
            sql: MIGRATION_16,
        },
        Migration {
            version: 17,
            name: "global-active-score-standard",
            sql: MIGRATION_17,
        },
        Migration {
            version: 18,
            name: "versioned-shikigami-catalog",
            sql: MIGRATION_18,
        },
        Migration {
            version: 19,
            name: "shikigami-ur-rarity",
            sql: MIGRATION_19,
        },
        Migration {
            version: 20,
            name: "analysis-score-columns",
            sql: MIGRATION_20,
        },
        Migration {
            version: 21,
            name: "backfill-unmatched-standard-score",
            sql: MIGRATION_21,
        },
        Migration {
            version: 22,
            name: "soul-radar-cache",
            sql: MIGRATION_22,
        },
        Migration {
            version: 23,
            name: "cbg-acquisition-source",
            sql: MIGRATION_23,
        },
        Migration {
            version: 24,
            name: "owned-shikigami",
            sql: MIGRATION_24,
        },
        Migration {
            version: 25,
            name: "owned-shikigami-per-profile",
            sql: MIGRATION_25,
        },
        Migration {
            version: 26,
            name: "head-tail-cache",
            sql: MIGRATION_26,
        },
        Migration {
            version: 27,
            name: "head-tail-all-candidates",
            sql: MIGRATION_27,
        },
        Migration {
            version: 28,
            name: "shikigami-material-rarity",
            sql: MIGRATION_28,
        },
        Migration {
            version: 29,
            name: "shikigami-material-rarity-zh",
            sql: MIGRATION_29,
        },
    ]
}

/// 迁移 1：应用元数据表。
const MIGRATION_1: &str = "CREATE TABLE IF NOT EXISTS app_meta (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;";

/// 迁移 2：游戏档案与成熟度阈值。
const MIGRATION_2: &str = "
CREATE TABLE IF NOT EXISTS game_profile (
    id TEXT PRIMARY KEY NOT NULL,
    display_name TEXT NOT NULL,
    source_identity TEXT,
    server_label TEXT,
    maturity_mode TEXT NOT NULL DEFAULT 'auto' CHECK (maturity_mode IN ('auto','manual')),
    maturity_level TEXT CHECK (maturity_level IN ('starter','growth','formed','deep')),
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    archived_at TEXT
) STRICT;

CREATE TABLE IF NOT EXISTS maturity_threshold (
    scope_type TEXT NOT NULL CHECK (scope_type IN ('system','profile')),
    profile_id TEXT NOT NULL,
    level TEXT NOT NULL CHECK (level IN ('starter','growth','formed','deep')),
    min_count INTEGER NOT NULL,
    PRIMARY KEY (scope_type, profile_id, level)
) STRICT;
";

/// 迁移 3：原始对象、采集事件、快照与库存投影。
const MIGRATION_3: &str = "
CREATE TABLE IF NOT EXISTS raw_object (
    sha256 TEXT PRIMARY KEY NOT NULL,
    relative_path TEXT NOT NULL UNIQUE,
    compression TEXT NOT NULL DEFAULT 'zstd',
    media_type TEXT NOT NULL,
    raw_size INTEGER NOT NULL,
    stored_size INTEGER NOT NULL,
    verified_at TEXT,
    created_at TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS snapshot (
    id TEXT PRIMARY KEY NOT NULL,
    profile_id TEXT NOT NULL REFERENCES game_profile(id) ON DELETE CASCADE,
    raw_sha256 TEXT NOT NULL REFERENCES raw_object(sha256),
    content_fingerprint TEXT,
    source_kind TEXT NOT NULL,
    completeness TEXT NOT NULL CHECK (completeness IN ('complete','partial','unknown')),
    scope_json TEXT,
    captured_at TEXT,
    game_version TEXT,
    adapter_version TEXT,
    parser_version TEXT NOT NULL,
    catalog_version TEXT NOT NULL,
    parent_snapshot_id TEXT REFERENCES snapshot(id),
    forced INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_snapshot_profile_captured ON snapshot(profile_id, captured_at DESC);
CREATE INDEX IF NOT EXISTS idx_snapshot_fingerprint ON snapshot(content_fingerprint);

CREATE TABLE IF NOT EXISTS acquisition_event (
    id TEXT PRIMARY KEY NOT NULL,
    profile_id TEXT NOT NULL REFERENCES game_profile(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('native','importer','mumu','desktop')),
    source_format TEXT NOT NULL,
    raw_sha256 TEXT REFERENCES raw_object(sha256),
    captured_at TEXT,
    received_at TEXT NOT NULL,
    game_version TEXT,
    adapter_version TEXT,
    parser_version TEXT NOT NULL,
    completeness TEXT NOT NULL CHECK (completeness IN ('complete','partial','unknown')),
    scope_json TEXT,
    result_kind TEXT NOT NULL CHECK (result_kind IN ('new_snapshot','unchanged','forced','error_free')),
    snapshot_id TEXT REFERENCES snapshot(id)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_acquisition_event_profile ON acquisition_event(profile_id, received_at DESC);

CREATE TABLE IF NOT EXISTS snapshot_soul (
    snapshot_id TEXT NOT NULL REFERENCES snapshot(id) ON DELETE CASCADE,
    internal_id TEXT NOT NULL,
    source_stable_id TEXT,
    identity_quality TEXT NOT NULL CHECK (identity_quality IN ('stable','derived','missing')),
    set_id TEXT NOT NULL,
    slot INTEGER NOT NULL CHECK (slot BETWEEN 1 AND 6),
    quality INTEGER NOT NULL CHECK (quality BETWEEN 1 AND 6),
    level INTEGER NOT NULL CHECK (level BETWEEN 0 AND 15),
    main_attr_type TEXT NOT NULL,
    main_attr_value REAL NOT NULL,
    initial_substat_count INTEGER,
    locked_in_source INTEGER,
    equipped_state TEXT,
    source_json TEXT,
    PRIMARY KEY (snapshot_id, internal_id)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_snapshot_soul_set ON snapshot_soul(snapshot_id, set_id, slot, level);
CREATE INDEX IF NOT EXISTS idx_snapshot_soul_stable ON snapshot_soul(source_stable_id);

CREATE TABLE IF NOT EXISTS soul_attribute (
    snapshot_id TEXT NOT NULL,
    soul_internal_id TEXT NOT NULL,
    attribute_index INTEGER NOT NULL,
    attribute_type TEXT NOT NULL,
    value REAL NOT NULL,
    enhancement_count INTEGER,
    count_provenance TEXT NOT NULL CHECK (count_provenance IN ('source','estimated','user','unknown')),
    fixed_attribute INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (snapshot_id, soul_internal_id, attribute_index),
    FOREIGN KEY (snapshot_id, soul_internal_id) REFERENCES snapshot_soul(snapshot_id, internal_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX IF NOT EXISTS idx_soul_attribute_lookup ON soul_attribute(snapshot_id, attribute_type, value);

CREATE TABLE IF NOT EXISTS inventory_item (
    profile_id TEXT NOT NULL REFERENCES game_profile(id) ON DELETE CASCADE,
    soul_key TEXT NOT NULL,
    snapshot_id TEXT NOT NULL REFERENCES snapshot(id),
    soul_internal_id TEXT NOT NULL,
    first_seen_snapshot_id TEXT NOT NULL REFERENCES snapshot(id),
    last_seen_snapshot_id TEXT NOT NULL REFERENCES snapshot(id),
    presence_state TEXT NOT NULL CHECK (presence_state IN ('present','unconfirmed','removed')),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (profile_id, soul_key)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_inventory_presence ON inventory_item(profile_id, presence_state);
";

/// 迁移 4：御魂目录与分组、常用度。
const MIGRATION_4: &str = "
CREATE TABLE IF NOT EXISTS catalog_package (
    version TEXT PRIMARY KEY NOT NULL,
    installed_at TEXT NOT NULL,
    source TEXT,
    sha256 TEXT,
    signature_text TEXT,
    compatible_game_version TEXT,
    min_app_version TEXT,
    active INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE IF NOT EXISTS soul_set (
    catalog_version TEXT NOT NULL,
    set_id TEXT NOT NULL,
    name TEXT NOT NULL,
    rarity_scope TEXT,
    two_piece_effect_json TEXT,
    four_piece_effect_json TEXT,
    source_refs_json TEXT,
    effective_from TEXT,
    effective_to TEXT,
    PRIMARY KEY (catalog_version, set_id)
) STRICT;

CREATE TABLE IF NOT EXISTS yuhun_group (
    id TEXT PRIMARY KEY NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('system','custom')),
    name TEXT NOT NULL,
    description TEXT,
    parent_id TEXT REFERENCES yuhun_group(id),
    revision INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE TABLE IF NOT EXISTS yuhun_group_member (
    group_id TEXT NOT NULL REFERENCES yuhun_group(id) ON DELETE CASCADE,
    set_id TEXT NOT NULL,
    catalog_version TEXT NOT NULL,
    origin TEXT NOT NULL DEFAULT 'system',
    user_override INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (group_id, set_id, catalog_version)
) STRICT;

CREATE TABLE IF NOT EXISTS set_commonness_default (
    catalog_version TEXT NOT NULL,
    set_id TEXT NOT NULL,
    scenario TEXT NOT NULL CHECK (scenario IN ('PVE','PVP')),
    value TEXT NOT NULL CHECK (value IN ('common','uncommon')),
    PRIMARY KEY (catalog_version, set_id, scenario)
) STRICT;

CREATE TABLE IF NOT EXISTS profile_commonness_override (
    profile_id TEXT NOT NULL REFERENCES game_profile(id) ON DELETE CASCADE,
    set_id TEXT NOT NULL,
    scenario TEXT NOT NULL CHECK (scenario IN ('PVE','PVP')),
    value TEXT NOT NULL CHECK (value IN ('common','uncommon')),
    PRIMARY KEY (profile_id, set_id, scenario)
) STRICT;
";

/// 迁移 5：备份记录表。
const MIGRATION_5: &str = "CREATE TABLE IF NOT EXISTS backup_record (
    id TEXT PRIMARY KEY NOT NULL,
    path TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    file_count INTEGER NOT NULL DEFAULT 0,
    total_size INTEGER NOT NULL DEFAULT 0,
    reason TEXT NOT NULL,
    checksum TEXT,
    created_at TEXT NOT NULL,
    restored_at TEXT,
    verified INTEGER NOT NULL DEFAULT 0
) STRICT;";

/// 迁移 6：写入全局默认库存成熟度边界；使用 OR IGNORE 保留用户已有覆盖。
const MIGRATION_6: &str =
    "INSERT OR IGNORE INTO maturity_threshold (scope_type, profile_id, level, min_count) VALUES
    ('system', '', 'starter', 0),
    ('system', '', 'growth', 500),
    ('system', '', 'formed', 1500),
    ('system', '', 'deep', 3000);";

/// 迁移 7：把范围指纹独立保存，避免仅比较规范御魂内容而误判筛选范围相同。
const MIGRATION_7: &str = "ALTER TABLE snapshot ADD COLUMN scope_fingerprint TEXT;
CREATE INDEX IF NOT EXISTS idx_snapshot_scope_fingerprint ON snapshot(scope_fingerprint);";

/// 迁移 8：分析待办、独立用户决定、决定历史以及强化/清理行动批次。
const MIGRATION_8: &str = "
CREATE TABLE IF NOT EXISTS analysis_todo (
    id TEXT PRIMARY KEY NOT NULL,
    profile_id TEXT NOT NULL REFERENCES game_profile(id) ON DELETE CASCADE,
    soul_key TEXT NOT NULL,
    snapshot_id TEXT NOT NULL REFERENCES snapshot(id) ON DELETE CASCADE,
    soul_internal_id TEXT NOT NULL,
    set_id TEXT NOT NULL,
    slot INTEGER NOT NULL CHECK (slot BETWEEN 1 AND 6),
    quality INTEGER NOT NULL CHECK (quality BETWEEN 1 AND 6),
    level INTEGER NOT NULL CHECK (level BETWEEN 0 AND 15),
    main_attribute TEXT NOT NULL,
    main_value REAL NOT NULL,
    category TEXT NOT NULL CHECK (category IN ('new_embryo','continue','await_review','stop','cleanup','conflict','uncertain','changed')),
    recommendation TEXT NOT NULL CHECK (recommendation IN ('keep','observe','stop','recycle')),
    reason_summary TEXT NOT NULL,
    evidence_level TEXT NOT NULL CHECK (evidence_level IN ('official','open_source','author','community','inferred','unverified','draft')),
    data_quality TEXT NOT NULL CHECK (data_quality IN ('complete','unknown','partial')),
    presence_state TEXT NOT NULL CHECK (presence_state IN ('present','unconfirmed','removed')),
    is_new INTEGER NOT NULL DEFAULT 0,
    is_changed INTEGER NOT NULL DEFAULT 0,
    detail_json TEXT NOT NULL,
    explanation_hash TEXT NOT NULL,
    generated_at TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    UNIQUE (profile_id, soul_key)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_analysis_todo_profile_category
    ON analysis_todo(profile_id, category, generated_at DESC);
CREATE INDEX IF NOT EXISTS idx_analysis_todo_profile_action
    ON analysis_todo(profile_id, recommendation, presence_state);
CREATE INDEX IF NOT EXISTS idx_analysis_todo_soul
    ON analysis_todo(profile_id, soul_key);

CREATE TABLE IF NOT EXISTS user_decision (
    profile_id TEXT NOT NULL REFERENCES game_profile(id) ON DELETE CASCADE,
    soul_key TEXT NOT NULL,
    decision TEXT NOT NULL CHECK (decision IN ('keep','observe','plan_strengthen','plan_recycle','ignore')),
    note TEXT,
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (profile_id, soul_key)
) STRICT;

CREATE TABLE IF NOT EXISTS user_decision_history (
    id TEXT PRIMARY KEY NOT NULL,
    operation_id TEXT NOT NULL,
    profile_id TEXT NOT NULL REFERENCES game_profile(id) ON DELETE CASCADE,
    soul_key TEXT NOT NULL,
    previous_decision TEXT,
    next_decision TEXT,
    note TEXT,
    created_at TEXT NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_user_decision_history_profile
    ON user_decision_history(profile_id, soul_key, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_user_decision_history_operation
    ON user_decision_history(operation_id);

CREATE TABLE IF NOT EXISTS action_batch (
    id TEXT PRIMARY KEY NOT NULL,
    profile_id TEXT NOT NULL REFERENCES game_profile(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('strengthen','cleanup')),
    status TEXT NOT NULL CHECK (status IN ('draft','active','completed','cancelled')),
    target_level INTEGER CHECK (target_level IS NULL OR target_level IN (3,6,9,12,15)),
    snapshot_id TEXT REFERENCES snapshot(id),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT
) STRICT;

CREATE INDEX IF NOT EXISTS idx_action_batch_profile
    ON action_batch(profile_id, created_at DESC);

CREATE TABLE IF NOT EXISTS action_batch_item (
    batch_id TEXT NOT NULL REFERENCES action_batch(id) ON DELETE CASCADE,
    soul_key TEXT NOT NULL,
    group_key TEXT NOT NULL,
    set_id TEXT NOT NULL,
    slot INTEGER NOT NULL CHECK (slot BETWEEN 1 AND 6),
    main_attribute TEXT NOT NULL,
    level INTEGER NOT NULL CHECK (level BETWEEN 0 AND 15),
    status TEXT NOT NULL CHECK (status IN ('pending','completed','skipped','not_found')),
    sort_order INTEGER NOT NULL,
    note TEXT,
    completed_at TEXT,
    PRIMARY KEY (batch_id, soul_key)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_action_batch_item_group
    ON action_batch_item(batch_id, group_key, sort_order);
";

/// 迁移 9：规则版本正文与档案级启用状态，隔离系统只读版本和用户可编辑版本。
const MIGRATION_9: &str = "
CREATE TABLE IF NOT EXISTS rule_preset (
    id TEXT PRIMARY KEY NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('builtin','community','personal','imported','user')),
    author TEXT,
    title TEXT NOT NULL,
    source_text TEXT,
    source_refs_json TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS rule_version (
    id TEXT PRIMARY KEY NOT NULL,
    preset_id TEXT NOT NULL REFERENCES rule_preset(id) ON DELETE CASCADE,
    version TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    title TEXT NOT NULL,
    author TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('draft','verified','deprecated')),
    origin TEXT NOT NULL CHECK (origin IN ('builtin','imported','user')),
    canonical_json TEXT NOT NULL,
    canonical_sha256 TEXT NOT NULL,
    parent_version_id TEXT REFERENCES rule_version(id),
    read_only INTEGER NOT NULL DEFAULT 1 CHECK (read_only IN (0,1)),
    created_at TEXT NOT NULL,
    UNIQUE (preset_id, version, canonical_sha256)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_rule_version_preset ON rule_version(preset_id, version DESC);
CREATE INDEX IF NOT EXISTS idx_rule_version_origin ON rule_version(origin, created_at DESC);

CREATE TABLE IF NOT EXISTS profile_rule_activation (
    profile_id TEXT NOT NULL REFERENCES game_profile(id) ON DELETE CASCADE,
    rule_version_id TEXT NOT NULL REFERENCES rule_version(id) ON DELETE CASCADE,
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
    position INTEGER NOT NULL DEFAULT 0,
    note TEXT,
    enabled_at TEXT NOT NULL,
    PRIMARY KEY (profile_id, rule_version_id)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_profile_rule_activation_enabled
    ON profile_rule_activation(profile_id, enabled, position);
";

/// 迁移 10：记录原始对象校验结果，使损坏对象对应的快照在历史页中持续可见。
const MIGRATION_10: &str = "
CREATE TABLE IF NOT EXISTS snapshot_integrity (
    snapshot_id TEXT PRIMARY KEY NOT NULL REFERENCES snapshot(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('healthy','missing','corrupt')),
    message TEXT,
    checked_at TEXT NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_snapshot_integrity_status
    ON snapshot_integrity(status, checked_at DESC);
";

/// 迁移 11：记录签名更新安装历史与活动版本，支持用户查看差异和主动回退。
const MIGRATION_11: &str = "
CREATE TABLE IF NOT EXISTS installed_package (
    id TEXT PRIMARY KEY NOT NULL,
    package_type TEXT NOT NULL CHECK (package_type IN ('application','catalog','rules','adapter')),
    version TEXT NOT NULL,
    sha256 TEXT,
    signature_fingerprint TEXT,
    active INTEGER NOT NULL DEFAULT 0 CHECK (active IN (0,1)),
    source TEXT,
    content_path TEXT,
    installed_at TEXT NOT NULL,
    UNIQUE (package_type, version, sha256)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_installed_package_type_active
    ON installed_package(package_type, active, installed_at DESC);
";

/// 迁移 12：补齐离线图标、套装分类和随目录版本保存的强化机械规则。
const MIGRATION_12: &str = "
ALTER TABLE soul_set ADD COLUMN icon_asset_id INTEGER;
ALTER TABLE soul_set ADD COLUMN category TEXT;
ALTER TABLE soul_set ADD COLUMN enhancement_rule_json TEXT;
";

/// 迁移 13：保存目录包解析器、校验结果和静态资源覆盖率，支持离线升级协议。
const MIGRATION_13: &str = "
ALTER TABLE catalog_package ADD COLUMN parser_version TEXT;
ALTER TABLE catalog_package ADD COLUMN validation_status TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE catalog_package ADD COLUMN validation_errors_json TEXT;
ALTER TABLE catalog_package ADD COLUMN icon_coverage INTEGER NOT NULL DEFAULT 0;
ALTER TABLE catalog_package ADD COLUMN mechanics_coverage INTEGER NOT NULL DEFAULT 0;
";

/// 迁移 14：区分标准二件套属性与首领、星痕类别。
const MIGRATION_14: &str = "
ALTER TABLE soul_set ADD COLUMN special_category TEXT;
";

/// 迁移 15：用户导入的规则归用户所有，历史导入版本统一解除只读限制。
const MIGRATION_15: &str = "
UPDATE rule_version
SET read_only = 0
WHERE origin = 'imported';
";

/// 迁移 16：历史导入版本改用独立预设身份，避免与系统内置版本发生唯一键冲突。
const MIGRATION_16: &str = "
INSERT INTO rule_preset (id, origin, author, title, created_at)
SELECT version.id, 'imported', version.author, version.title, version.created_at
FROM rule_version AS version
WHERE version.origin = 'imported'
  AND version.preset_id <> version.id
  AND NOT EXISTS (
      SELECT 1 FROM rule_preset AS preset WHERE preset.id = version.id
  );

UPDATE rule_version
SET preset_id = id
WHERE origin = 'imported'
  AND preset_id <> id;
";

/// 迁移 17：评分标准改为全局单选启用，不再绑定游戏档案。
const MIGRATION_17: &str = "
CREATE TABLE IF NOT EXISTS active_score_standard (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    rule_version_id TEXT NOT NULL REFERENCES rule_version(id) ON DELETE CASCADE,
    enabled_at TEXT NOT NULL,
    note TEXT
) STRICT;
";

/// 迁移 18：为每个目录版本保存式神面板、技能、技能投入和就业场景 JSON。
/// 旧目录包保留默认 0 覆盖率，读取时仍能明确显示“未核验”，不会伪造式神数据。
const MIGRATION_18: &str = "
ALTER TABLE catalog_package ADD COLUMN shikigami_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE catalog_package ADD COLUMN panel_coverage INTEGER NOT NULL DEFAULT 0;
ALTER TABLE catalog_package ADD COLUMN skill_coverage INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS shikigami (
    catalog_version TEXT NOT NULL,
    shikigami_id TEXT NOT NULL,
    name TEXT NOT NULL,
    rarity TEXT NOT NULL CHECK (rarity IN ('SP','SSR','SR','R','N')),
    icon_asset_id TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    panel_level INTEGER NOT NULL CHECK (panel_level = 40),
    awakened INTEGER NOT NULL CHECK (awakened IN (0,1)),
    panel_json TEXT NOT NULL,
    skills_json TEXT NOT NULL,
    skill_investment_json TEXT NOT NULL,
    employment_scenes_json TEXT NOT NULL,
    game_version TEXT NOT NULL,
    source_refs_json TEXT NOT NULL,
    PRIMARY KEY (catalog_version, shikigami_id)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_shikigami_catalog_rarity
    ON shikigami(catalog_version, rarity, sort_order, name);
";

/// 迁移 19：重建式神表以纳入 UR；SQLite 无法直接修改已有 CHECK 约束。
const MIGRATION_19: &str = "
ALTER TABLE shikigami RENAME TO shikigami_v18;
DROP INDEX IF EXISTS idx_shikigami_catalog_rarity;

CREATE TABLE shikigami (
    catalog_version TEXT NOT NULL,
    shikigami_id TEXT NOT NULL,
    name TEXT NOT NULL,
    rarity TEXT NOT NULL CHECK (rarity IN ('UR','SP','SSR','SR','R','N')),
    icon_asset_id TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    panel_level INTEGER NOT NULL CHECK (panel_level = 40),
    awakened INTEGER NOT NULL CHECK (awakened IN (0,1)),
    panel_json TEXT NOT NULL,
    skills_json TEXT NOT NULL,
    skill_investment_json TEXT NOT NULL,
    employment_scenes_json TEXT NOT NULL,
    game_version TEXT NOT NULL,
    source_refs_json TEXT NOT NULL,
    PRIMARY KEY (catalog_version, shikigami_id)
) STRICT;

INSERT INTO shikigami (
    catalog_version, shikigami_id, name, rarity, icon_asset_id, sort_order,
    panel_level, awakened, panel_json, skills_json, skill_investment_json,
    employment_scenes_json, game_version, source_refs_json
)
SELECT
    catalog_version, shikigami_id, name, rarity, icon_asset_id, sort_order,
    panel_level, awakened, panel_json, skills_json, skill_investment_json,
    employment_scenes_json, game_version, source_refs_json
FROM shikigami_v18;

DROP TABLE shikigami_v18;

CREATE INDEX idx_shikigami_catalog_rarity
    ON shikigami(catalog_version, rarity, sort_order, name);
";

/// 迁移 20：把标准评分和综合评分从解释 JSON 提升为可查询的分析字段。
/// 标准评分用于当前列表筛选；综合评分先保存结果，页面展示仍按产品约定暂缓。
const MIGRATION_20: &str = "
ALTER TABLE analysis_todo ADD COLUMN standard_score REAL;
ALTER TABLE analysis_todo ADD COLUMN composite_score REAL;
UPDATE analysis_todo
SET standard_score = json_extract(detail_json, '$.standardScore.score')
WHERE json_extract(detail_json, '$.standardScore.score') IS NOT NULL;
UPDATE analysis_todo
SET composite_score = (
    SELECT MAX(json_extract(value, '$.score'))
    FROM json_each(analysis_todo.detail_json, '$.uses')
    WHERE json_extract(value, '$.selectorMatched') = 1
);
CREATE INDEX IF NOT EXISTS idx_analysis_todo_standard_score
    ON analysis_todo(profile_id, standard_score);
";

/// 迁移 21：把旧版“已重算但未命中评分卡片”的空结果补成可追踪的 0 分。
/// 这样覆盖率只反映是否完成计算，不会把“规则未命中”误报为“尚未计算”。
const MIGRATION_21: &str = "
UPDATE analysis_todo
SET standard_score = 0
WHERE standard_score IS NULL;
UPDATE analysis_todo
SET detail_json = json_set(
    detail_json,
    '$.standardScore',
    json_object(
        'score', standard_score,
        'matchedRuleId', 'unmatched',
        'matchedRuleName', '未匹配评分规则卡片',
        'formula', '未匹配评分规则卡片，评分为 0 分'
    )
)
WHERE standard_score = 0
  AND (json_type(detail_json, '$.standardScore') IS NULL
       OR json_type(detail_json, '$.standardScore') = 'null');
";

/// 迁移 22：保存御魂雷达的聚合结果，进入页面时只读取缓存，不重新扫描整份御魂事实。
/// 缓存按档案隔离，点位表使用复合主键保证同一套装、号位和指标只有一个最高值。
const MIGRATION_22: &str = "
CREATE TABLE IF NOT EXISTS soul_radar_cache (
    profile_id TEXT PRIMARY KEY NOT NULL REFERENCES game_profile(id) ON DELETE CASCADE,
    calculated_at TEXT NOT NULL,
    inventory_revision TEXT NOT NULL,
    inventory_count INTEGER NOT NULL CHECK (inventory_count >= 0)
) STRICT;

CREATE TABLE IF NOT EXISTS soul_radar_point (
    profile_id TEXT NOT NULL REFERENCES soul_radar_cache(profile_id) ON DELETE CASCADE,
    set_id TEXT NOT NULL,
    slot INTEGER NOT NULL CHECK (slot BETWEEN 1 AND 6),
    metric_type TEXT NOT NULL,
    value REAL NOT NULL,
    main_attr_type TEXT,
    PRIMARY KEY (profile_id, set_id, slot, metric_type)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_soul_radar_point_profile_set
    ON soul_radar_point(profile_id, set_id, slot);
";

/// 迁移 23：为采集事件增加藏宝阁来源；通过重建表兼容已经存在的 SQLite CHECK 约束。
/// 历史事件逐列搬迁，新增来源只影响后续事件，不改变已有快照和库存事实。
const MIGRATION_23: &str = "
CREATE TABLE acquisition_event_v23 (
    id TEXT PRIMARY KEY NOT NULL,
    profile_id TEXT NOT NULL REFERENCES game_profile(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('native','importer','mumu','desktop','cbg')),
    source_format TEXT NOT NULL,
    raw_sha256 TEXT REFERENCES raw_object(sha256),
    captured_at TEXT,
    received_at TEXT NOT NULL,
    game_version TEXT,
    adapter_version TEXT,
    parser_version TEXT NOT NULL,
    completeness TEXT NOT NULL CHECK (completeness IN ('complete','partial','unknown')),
    scope_json TEXT,
    result_kind TEXT NOT NULL CHECK (result_kind IN ('new_snapshot','unchanged','forced','error_free')),
    snapshot_id TEXT REFERENCES snapshot(id)
) STRICT;

INSERT INTO acquisition_event_v23 (
    id, profile_id, source_kind, source_format, raw_sha256, captured_at,
    received_at, game_version, adapter_version, parser_version, completeness,
    scope_json, result_kind, snapshot_id
)
SELECT
    id, profile_id, source_kind, source_format, raw_sha256, captured_at,
    received_at, game_version, adapter_version, parser_version, completeness,
    scope_json, result_kind, snapshot_id
FROM acquisition_event;

DROP INDEX IF EXISTS idx_acquisition_event_profile;
DROP TABLE acquisition_event;
ALTER TABLE acquisition_event_v23 RENAME TO acquisition_event;
CREATE INDEX idx_acquisition_event_profile ON acquisition_event(profile_id, received_at DESC);
";

/// 迁移 24：持久化玩家持有的式神实例。
/// 当前数据覆盖导入时整表替换；只读快照性质的镜像，供数据库侧查询与后续分析，
/// 页面展示仍以 current-data.json 为单一事实源。
const MIGRATION_24: &str = "
CREATE TABLE IF NOT EXISTS owned_shikigami (
    instance_id TEXT PRIMARY KEY NOT NULL,
    shikigami_id TEXT NOT NULL,
    star INTEGER NOT NULL,
    level INTEGER,
    exp INTEGER,
    locked INTEGER,
    awakened INTEGER,
    skin_id INTEGER,
    skills_json TEXT NOT NULL DEFAULT '[]',
    selected_skill_ids_json TEXT NOT NULL DEFAULT '[]',
    source_kind TEXT NOT NULL,
    imported_at TEXT NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_owned_shikigami_shikigami_id
    ON owned_shikigami(shikigami_id, star, level);
";

/// 迁移 25：持有式神镜像按角色档案隔离。
/// 角色切换后每个角色的式神镜像独立维护；旧版单份镜像在升级时整体丢弃
/// （多角色功能明确从空开始，不迁移旧“默认数据”）。
const MIGRATION_25: &str = "
DROP TABLE owned_shikigami;

CREATE TABLE owned_shikigami (
    profile_id TEXT NOT NULL REFERENCES game_profile(id) ON DELETE CASCADE,
    instance_id TEXT NOT NULL,
    shikigami_id TEXT NOT NULL,
    star INTEGER NOT NULL,
    level INTEGER,
    exp INTEGER,
    locked INTEGER,
    awakened INTEGER,
    skin_id INTEGER,
    skills_json TEXT NOT NULL DEFAULT '[]',
    selected_skill_ids_json TEXT NOT NULL DEFAULT '[]',
    source_kind TEXT NOT NULL,
    imported_at TEXT NOT NULL,
    PRIMARY KEY (profile_id, instance_id)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_owned_shikigami_shikigami_id
    ON owned_shikigami(profile_id, shikigami_id, star, level);
";

/// 迁移 26：保存头尾分析的候选卡片及其真实副属性，按角色和库存版本隔离。
/// 初版结果表每个位置只允许一枚卡片；迁移 27 会为结果增加候选序号以支持完整列表。
const MIGRATION_26: &str = "
CREATE TABLE IF NOT EXISTS head_tail_cache (
    profile_id TEXT PRIMARY KEY NOT NULL REFERENCES game_profile(id) ON DELETE CASCADE,
    calculated_at TEXT NOT NULL,
    inventory_revision TEXT NOT NULL,
    inventory_count INTEGER NOT NULL CHECK (inventory_count >= 0),
    head_candidate_count INTEGER NOT NULL CHECK (head_candidate_count >= 0),
    tail_candidate_count INTEGER NOT NULL CHECK (tail_candidate_count >= 0)
) STRICT;

CREATE TABLE IF NOT EXISTS head_tail_result (
    profile_id TEXT NOT NULL REFERENCES head_tail_cache(profile_id) ON DELETE CASCADE,
    position TEXT NOT NULL CHECK (position IN ('head', 'tail')),
    soul_key TEXT NOT NULL,
    set_id TEXT NOT NULL,
    slot INTEGER NOT NULL CHECK (slot IN (2, 4)),
    quality INTEGER NOT NULL CHECK (quality >= 0),
    level INTEGER NOT NULL CHECK (level >= 0),
    main_attr_type TEXT NOT NULL,
    main_attr_value REAL NOT NULL,
    speed REAL NOT NULL,
    speed_rolls INTEGER NOT NULL CHECK (speed_rolls = 5),
    attributes_json TEXT NOT NULL,
    PRIMARY KEY (profile_id, position)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_head_tail_result_profile
    ON head_tail_result(profile_id, position);
";

/// 迁移 27：允许同一档案和位置保存全部头尾候选，并保留旧缓存为序号 0 的兼容数据。
const MIGRATION_27: &str = "
CREATE TABLE head_tail_result_v27 (
    profile_id TEXT NOT NULL REFERENCES head_tail_cache(profile_id) ON DELETE CASCADE,
    position TEXT NOT NULL CHECK (position IN ('head', 'tail')),
    candidate_rank INTEGER NOT NULL CHECK (candidate_rank >= 0),
    soul_key TEXT NOT NULL,
    set_id TEXT NOT NULL,
    slot INTEGER NOT NULL CHECK (slot IN (2, 4)),
    quality INTEGER NOT NULL CHECK (quality >= 0),
    level INTEGER NOT NULL CHECK (level >= 0),
    main_attr_type TEXT NOT NULL,
    main_attr_value REAL NOT NULL,
    speed REAL NOT NULL,
    speed_rolls INTEGER NOT NULL CHECK (speed_rolls = 5),
    attributes_json TEXT NOT NULL,
    PRIMARY KEY (profile_id, position, candidate_rank)
) STRICT;

INSERT INTO head_tail_result_v27 (
    profile_id, position, candidate_rank, soul_key, set_id, slot, quality, level,
    main_attr_type, main_attr_value, speed, speed_rolls, attributes_json
)
SELECT
    profile_id, position, 0, soul_key, set_id, slot, quality, level,
    main_attr_type, main_attr_value, speed, speed_rolls, attributes_json
FROM head_tail_result;

DROP INDEX IF EXISTS idx_head_tail_result_profile;
DROP TABLE head_tail_result;
ALTER TABLE head_tail_result_v27 RENAME TO head_tail_result;

CREATE INDEX idx_head_tail_result_profile
    ON head_tail_result(profile_id, position, candidate_rank);
";

/// 迁移 28：重建式神表以纳入素材稀有度；素材允许使用 0 级占位面板，其他条目仍严格要求 40 级。
const MIGRATION_28: &str = "
CREATE TABLE shikigami_v28 (
    catalog_version TEXT NOT NULL,
    shikigami_id TEXT NOT NULL,
    name TEXT NOT NULL,
    rarity TEXT NOT NULL CHECK (rarity IN ('UR','SP','SSR','SR','R','N','MATERIAL')),
    icon_asset_id TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    panel_level INTEGER NOT NULL CHECK (
        panel_level = 40
        OR (rarity = 'MATERIAL' AND panel_level = 0)
    ),
    awakened INTEGER NOT NULL CHECK (awakened IN (0,1)),
    panel_json TEXT NOT NULL,
    skills_json TEXT NOT NULL,
    skill_investment_json TEXT NOT NULL,
    employment_scenes_json TEXT NOT NULL,
    game_version TEXT NOT NULL,
    source_refs_json TEXT NOT NULL,
    PRIMARY KEY (catalog_version, shikigami_id)
) STRICT;

INSERT INTO shikigami_v28 (
    catalog_version, shikigami_id, name, rarity, icon_asset_id, sort_order,
    panel_level, awakened, panel_json, skills_json, skill_investment_json,
    employment_scenes_json, game_version, source_refs_json
)
SELECT
    catalog_version, shikigami_id, name, rarity, icon_asset_id, sort_order,
    panel_level, awakened, panel_json, skills_json, skill_investment_json,
    employment_scenes_json, game_version, source_refs_json
FROM shikigami;

DROP INDEX IF EXISTS idx_shikigami_catalog_rarity;
DROP TABLE shikigami;
ALTER TABLE shikigami_v28 RENAME TO shikigami;

CREATE INDEX idx_shikigami_catalog_rarity
    ON shikigami(catalog_version, rarity, sort_order, name);
";

/// 迁移 29：把旧的 MATERIAL 值统一为“素材”，并保留旧值兼容外部目录回滚。
const MIGRATION_29: &str = "
CREATE TABLE shikigami_v29 (
    catalog_version TEXT NOT NULL,
    shikigami_id TEXT NOT NULL,
    name TEXT NOT NULL,
    rarity TEXT NOT NULL CHECK (rarity IN ('UR','SP','SSR','SR','R','N','素材','MATERIAL')),
    icon_asset_id TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    panel_level INTEGER NOT NULL CHECK (
        panel_level = 40
        OR (rarity IN ('素材','MATERIAL') AND panel_level = 0)
    ),
    awakened INTEGER NOT NULL CHECK (awakened IN (0,1)),
    panel_json TEXT NOT NULL,
    skills_json TEXT NOT NULL,
    skill_investment_json TEXT NOT NULL,
    employment_scenes_json TEXT NOT NULL,
    game_version TEXT NOT NULL,
    source_refs_json TEXT NOT NULL,
    PRIMARY KEY (catalog_version, shikigami_id)
) STRICT;

INSERT INTO shikigami_v29 (
    catalog_version, shikigami_id, name, rarity, icon_asset_id, sort_order,
    panel_level, awakened, panel_json, skills_json, skill_investment_json,
    employment_scenes_json, game_version, source_refs_json
)
SELECT
    catalog_version, shikigami_id, name,
    CASE WHEN rarity = 'MATERIAL' THEN '素材' ELSE rarity END,
    icon_asset_id, sort_order, panel_level, awakened, panel_json, skills_json,
    skill_investment_json, employment_scenes_json, game_version, source_refs_json
FROM shikigami;

DROP INDEX IF EXISTS idx_shikigami_catalog_rarity;
DROP TABLE shikigami;
ALTER TABLE shikigami_v29 RENAME TO shikigami;

CREATE INDEX idx_shikigami_catalog_rarity
    ON shikigami(catalog_version, rarity, sort_order, name);
";

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_memory() -> Connection {
        let conn = Connection::open_in_memory().expect("创建内存数据库");
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .expect("配置连接");
        conn
    }

    #[test]
    fn 全新数据库迁移到最新版本() {
        let mut conn = open_memory();
        let runner = MigrationRunner::builtin();
        let version = runner.run(&mut conn).expect("迁移应成功");
        assert_eq!(version, 29, "应迁移到最新版本 29");

        let count: i64 = conn
            .query_row("SELECT count(*) FROM schema_migration", [], |row| {
                row.get(0)
            })
            .expect("查询迁移记录");
        assert_eq!(count, 29, "应记录 29 次迁移");
    }

    #[test]
    fn 重复迁移不产生新记录() {
        let mut conn = open_memory();
        let runner = MigrationRunner::builtin();
        runner.run(&mut conn).expect("首次迁移");
        runner.run(&mut conn).expect("重复迁移");

        let count: i64 = conn
            .query_row("SELECT count(*) FROM schema_migration", [], |row| {
                row.get(0)
            })
            .expect("查询迁移记录");
        assert_eq!(count, 29, "重复迁移不应增加记录");

        let table_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'acquisition_event'",
                [],
                |row| row.get(0),
            )
            .expect("读取采集事件表定义");
        assert!(table_sql.contains("'cbg'"), "采集事件应允许藏宝阁来源");
    }

    /// 回归约束：历史导入版本位于 rule_version 表中，迁移后必须解除只读状态。
    #[test]
    fn 用户导入规则迁移后应允许编辑() {
        let conn = open_memory();
        conn.execute_batch(MIGRATION_9).expect("创建规则版本表");
        conn.execute_batch(
            "
            INSERT INTO rule_preset (
                id, origin, author, title, source_text, source_refs_json, created_at
            ) VALUES (
                'preset:test-imported', 'imported', 'tester', '测试导入', NULL, '[]', '2026-08-11T00:00:00Z'
            );
            INSERT INTO rule_version (
                id, preset_id, version, schema_version, title, author, status, origin,
                canonical_json, canonical_sha256, parent_version_id, read_only, created_at
            ) VALUES (
                'version:test-imported', 'preset:test-imported', '0.1.0', 1, '测试导入',
                'tester', 'draft', 'imported', '{}', 'test-hash', NULL, 1,
                '2026-08-11T00:00:00Z'
            );
            ",
        )
        .expect("创建历史导入版本");

        // 迁移只更新导入版本，系统内置版本仍由原有只读约束保护。
        conn.execute_batch(MIGRATION_15)
            .expect("解除历史导入版本只读状态");
        let read_only: i64 = conn
            .query_row(
                "SELECT read_only FROM rule_version WHERE id = 'version:test-imported'",
                [],
                |row| row.get(0),
            )
            .expect("查询导入版本只读状态");
        assert_eq!(read_only, 0, "历史导入版本迁移后应可编辑");
    }

    /// 回归约束：旧版导入记录必须迁移到自己的预设身份，不能继续复用内置预设 ID。
    #[test]
    fn 历史导入规则迁移后应隔离预设身份() {
        let conn = open_memory();
        conn.execute_batch(MIGRATION_9).expect("创建规则版本表");
        conn.execute_batch(
            "
            INSERT INTO rule_preset (
                id, origin, author, title, source_text, source_refs_json, created_at
            ) VALUES (
                'builtin.owner.default-manual', 'builtin', 'system', '默认标准', NULL, '[]', '2026-08-11T00:00:00Z'
            );
            INSERT INTO rule_version (
                id, preset_id, version, schema_version, title, author, status, origin,
                canonical_json, canonical_sha256, parent_version_id, read_only, created_at
            ) VALUES (
                'imported:legacy-default:hash', 'builtin.owner.default-manual', '0.2.0', 1,
                '默认标准', 'system', 'draft', 'imported', '{}', 'legacy-hash', NULL, 0,
                '2026-08-11T00:00:00Z'
            );
            ",
        )
        .expect("创建旧版导入版本");

        // 先补齐被外键引用的独立预设，再切换规则版本的归属身份。
        conn.execute_batch(MIGRATION_16)
            .expect("隔离历史导入预设身份");
        let (preset_id, preset_exists): (String, i64) = conn
            .query_row(
                "
                SELECT version.preset_id,
                       (SELECT count(*) FROM rule_preset WHERE id = version.id)
                FROM rule_version AS version
                WHERE version.id = 'imported:legacy-default:hash'
                ",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("查询历史导入预设身份");
        assert_eq!(preset_id, "imported:legacy-default:hash");
        assert_eq!(preset_exists, 1, "历史导入版本应拥有独立预设记录");
    }

    #[test]
    fn 校验和漂移被检测() {
        let mut conn = open_memory();
        let runner = MigrationRunner::builtin();
        runner.run(&mut conn).expect("首次迁移");

        // 篡改校验和
        conn.execute(
            "UPDATE schema_migration SET checksum = 'fake' WHERE version = 1",
            [],
        )
        .expect("篡改校验和");

        let result = runner.run(&mut conn);
        assert!(result.is_err(), "校验和漂移应被检测到");
    }

    /// 回归约束：旧版应用遇到由新版追加的更高迁移号时，
    /// 必须保持启动以完成自动更新，不得在窗口创建前直接退出。
    #[test]
    fn 更高的未知迁移不应阻断旧版启动() {
        let mut conn = open_memory();
        let old_runner = MigrationRunner::new_with_migrations(vec![Migration {
            version: 1,
            name: "old-version",
            sql: "CREATE TABLE old_version_data (id INTEGER PRIMARY KEY);",
        }]);
        old_runner.run(&mut conn).expect("初始化旧版数据库");

        // 模拟新版已应用只追加索引的迁移，随后用户回到旧版启动。
        conn.execute_batch(
            "CREATE INDEX idx_old_version_data_id ON old_version_data(id);
             INSERT INTO schema_migration
                (version, checksum, started_at, finished_at, result, applied_by)
             VALUES
                (2, 'future-checksum', '2026-08-24T00:00:00Z',
                 '2026-08-24T00:00:01Z', 'applied', 'future-version');",
        )
        .expect("写入更高版本迁移夹具");

        assert_eq!(old_runner.run(&mut conn).expect("旧版应保持可启动"), 2);
    }

    /// 迁移列表中间版本缺失仍是真正的历史漂移，不能借“兼容高版本”静默放行。
    #[test]
    fn 当前支持范围内的未知迁移仍应拒绝() {
        let mut conn = open_memory();
        let runner = MigrationRunner::new_with_migrations(vec![
            Migration {
                version: 1,
                name: "first-known",
                sql: "CREATE TABLE first_known (id INTEGER PRIMARY KEY);",
            },
            Migration {
                version: 3,
                name: "third-known",
                sql: "CREATE TABLE third_known (id INTEGER PRIMARY KEY);",
            },
        ]);
        runner.run(&mut conn).expect("初始化已知迁移");
        conn.execute(
            "INSERT INTO schema_migration
                (version, checksum, started_at, finished_at, result, applied_by)
             VALUES
                (2, 'unknown-history', '2026-08-24T00:00:00Z',
                 '2026-08-24T00:00:01Z', 'applied', 'drifted-version')",
            [],
        )
        .expect("写入历史漂移夹具");

        assert!(runner.run(&mut conn).is_err(), "未知历史迁移应继续拒绝");
    }

    #[test]
    fn 迁移失败时回滚但不影响已有迁移() {
        let mut conn = open_memory();
        let runner = MigrationRunner::builtin();
        runner.run(&mut conn).expect("首次迁移");

        // 创建一个在迁移 3 会失败的场景——但迁移 3 已经成功，所以这不会触发。
        // 使用一个包含非法 SQL 的测试迁移来验证回滚行为。
        let bad_migration = Migration {
            version: 99,
            name: "bad-test",
            sql: "CREATE TABLE test_bad (id INTEGER); this_is_not_valid_sql;",
        };
        let bad_runner = MigrationRunner::new_with_migrations(vec![bad_migration]);
        let result = bad_runner.run(&mut conn);
        assert!(result.is_err(), "非法迁移应失败");

        // 验证迁移 99 未被记录
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM schema_migration WHERE version = 99",
                [],
                |row| row.get(0),
            )
            .expect("查询");
        assert_eq!(count, 0, "失败迁移不应被记录");
    }

    /// 后续迁移失败时，前置迁移和迁移记录必须整体回滚，避免半升级数据库。
    #[test]
    fn 批量迁移失败时整体回滚() {
        let mut conn = open_memory();
        let runner = MigrationRunner::new_with_migrations(vec![
            Migration {
                version: 1,
                name: "first-test",
                sql: "CREATE TABLE first_test (id INTEGER);",
            },
            Migration {
                version: 2,
                name: "second-test-bad",
                sql: "CREATE TABLE second_test (id INTEGER); INVALID SQL;",
            },
        ]);

        assert!(runner.run(&mut conn).is_err(), "第二个迁移应失败");
        let first_exists: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'first_test'",
                [],
                |row| row.get(0),
            )
            .expect("查询前置迁移表");
        assert_eq!(first_exists, 0, "前置迁移也应随本次升级回滚");

        let migration_count: i64 = conn
            .query_row("SELECT count(*) FROM schema_migration", [], |row| {
                row.get(0)
            })
            .expect("查询迁移记录");
        assert_eq!(migration_count, 0, "失败升级不应留下已应用记录");
    }

    #[test]
    fn 所有迁移创建了预期表() {
        let mut conn = open_memory();
        let runner = MigrationRunner::builtin();
        runner.run(&mut conn).expect("迁移");

        let expected_tables = [
            "schema_migration",
            "app_meta",
            "game_profile",
            "maturity_threshold",
            "raw_object",
            "snapshot",
            "acquisition_event",
            "snapshot_soul",
            "soul_attribute",
            "inventory_item",
            "catalog_package",
            "soul_set",
            "shikigami",
            "yuhun_group",
            "yuhun_group_member",
            "set_commonness_default",
            "profile_commonness_override",
            "backup_record",
            "snapshot_integrity",
            "installed_package",
            "active_score_standard",
            "soul_radar_cache",
            "soul_radar_point",
            "head_tail_cache",
            "head_tail_result",
        ];

        for table in &expected_tables {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    rusqlite::params![table],
                    |row| row.get(0),
                )
                .expect("查询表");
            assert_eq!(count, 1, "表 {table} 应存在");
        }
    }

    #[test]
    fn 式神目录表允许素材稀有度和无培养面板() {
        let mut conn = open_memory();
        MigrationRunner::builtin().run(&mut conn).expect("迁移");

        // 素材式神没有等级与技能面板，但必须能够通过数据库约束写入目录，供图鉴和式神录共用。
        conn.execute(
            "INSERT INTO shikigami (
                catalog_version, shikigami_id, name, rarity, icon_asset_id, sort_order,
                panel_level, awakened, panel_json, skills_json, skill_investment_json,
                employment_scenes_json, game_version, source_refs_json
             ) VALUES (?1, ?2, ?3, '素材', NULL, ?4, 0, 0, '{}', '[]', '{}', '[]', ?5, '[]')",
            rusqlite::params!["test-material", "411", "御行达摩", 1000, "2026.08"],
        )
        .expect("素材式神应能写入目录");
    }

    #[test]
    fn 旧头尾结果迁移后保留为零号候选() {
        let conn = open_memory();
        conn.execute_batch("CREATE TABLE game_profile (id TEXT PRIMARY KEY NOT NULL) STRICT;")
            .expect("创建最小档案表");
        conn.execute_batch(MIGRATION_26).expect("创建旧头尾表");
        conn.execute("INSERT INTO game_profile (id) VALUES ('profile-a')", [])
            .expect("写入档案");
        conn.execute(
            "INSERT INTO head_tail_cache (
                profile_id, calculated_at, inventory_revision, inventory_count,
                head_candidate_count, tail_candidate_count
             ) VALUES ('profile-a', '2026-08-20T00:00:00Z', 'revision-1', 1, 1, 0)",
            [],
        )
        .expect("写入旧头尾缓存");
        conn.execute(
            "INSERT INTO head_tail_result (
                profile_id, position, soul_key, set_id, slot, quality, level,
                main_attr_type, main_attr_value, speed, speed_rolls, attributes_json
             ) VALUES ('profile-a', 'head', 'head-1', 'set-speed', 2, 6, 15,
                       'speed', 57.0, 16.76, 5, '[]')",
            [],
        )
        .expect("写入旧头尾卡片");

        conn.execute_batch(MIGRATION_27).expect("升级头尾候选表");

        let candidate_rank: i64 = conn
            .query_row(
                "SELECT candidate_rank FROM head_tail_result WHERE profile_id = 'profile-a'",
                [],
                |row| row.get(0),
            )
            .expect("读取迁移后的候选序号");
        assert_eq!(candidate_rank, 0);
    }
}
