//! SQLite 仓库实现。
//!
//! 所有仓库通过 `Arc<Database>` 访问数据库，不直接持有连接。
//! 领域异常通过 `AppError` 返回，不向前端暴露 SQL 细节。

use crate::application::error::AppError;
use crate::domain::actions::{
    ActionBatch, ActionBatchDetail, ActionRepository, AnalysisTodo, AnalysisTodoPage, BatchItem,
    DecisionApplyResult, DecisionChange, DecisionHistoryEntry, DecisionPreview, SoulScoreSummary,
    TodoQuery, is_protected_todo,
};
use crate::domain::{
    AcquisitionEvent, BackupRecord, CatalogPackage, CatalogStatus, CommonnessValue,
    EffectiveCommonness, GameProfile, GroupMember, HeadTailAttribute, HeadTailCache, HeadTailCard,
    ImportCommitRequest, ImportCommitResult, InstalledPackage, InventoryFilterOptions,
    InventoryItem, InventoryPage, InventoryPageQuery, MaturityLevel, MaturityMode,
    MaturityThreshold, NewGameProfile, OwnedShikigami, ProfileCommonness, RawObject,
    RawObjectStats, RuleActivation, RuleVersionRecord, SetCommonness, Shikigami, Snapshot,
    SnapshotSoul, SoulAttribute, SoulRadarCache, SoulRadarPoint, SoulSet,
    StandardScoreContribution, YuhunGroup,
};
use crate::domain::{
    AcquisitionEventRepository, BackupRecordRepository, CatalogRepository, CommonnessRepository,
    HeadTailRepository, ImportRepository, PackageRepository, ProfileRepository,
    RawObjectRepository, RuleRepository, SnapshotRepository, SoulRadarRepository,
};
use crate::infrastructure::database::connection::Database;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use std::collections::HashMap;
use std::sync::Arc;

// ─── 游戏档案仓库 ─────────────────────────────────────────────────────────────

pub struct SqliteProfileRepository {
    db: Arc<Database>,
}

impl SqliteProfileRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl ProfileRepository for SqliteProfileRepository {
    fn create(&self, new: &NewGameProfile, now: &str) -> Result<GameProfile, AppError> {
        let id = uuid::Uuid::new_v4().to_string();
        let display_name = new.display_name.clone();
        let source_identity = new.source_identity.clone();
        let server_label = new.server_label.clone();
        let created = now.to_string();

        self.db.write("create_profile", |conn| {
            conn.execute(
                "INSERT INTO game_profile (id, display_name, source_identity, server_label, \
                 maturity_mode, revision, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, 'auto', 1, ?5, ?5)",
                params![id, display_name, source_identity, server_label, created],
            )
            .map_err(|e| AppError::constraint_violation("game_profile", &e))?;
            Ok(())
        })?;

        self.get(&id)?
            .ok_or_else(|| AppError::internal("档案创建后无法读取"))
    }

    fn list(&self, include_archived: bool) -> Result<Vec<GameProfile>, AppError> {
        self.db.read("list_profiles", |conn| {
            let sql = if include_archived {
                "SELECT id, display_name, source_identity, server_label, \
                 maturity_mode, maturity_level, revision, created_at, updated_at, archived_at \
                 FROM game_profile ORDER BY created_at"
            } else {
                "SELECT id, display_name, source_identity, server_label, \
                 maturity_mode, maturity_level, revision, created_at, updated_at, archived_at \
                 FROM game_profile WHERE archived_at IS NULL ORDER BY created_at"
            };
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| AppError::database("prepare", &e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(GameProfile {
                        id: row.get(0)?,
                        display_name: row.get(1)?,
                        source_identity: row.get(2)?,
                        server_label: row.get(3)?,
                        maturity_mode: MaturityMode::from_str(&row.get::<_, String>(4)?)
                            .unwrap_or(MaturityMode::Auto),
                        maturity_level: row
                            .get::<_, Option<String>>(5)?
                            .and_then(|s| MaturityLevel::from_str(&s)),
                        revision: row.get::<_, u32>(6)?,
                        created_at: row.get(7)?,
                        updated_at: row.get(8)?,
                        archived_at: row.get(9)?,
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            let mut profiles = Vec::new();
            for row in rows {
                profiles.push(row.map_err(|e| AppError::database("read row", &e))?);
            }
            Ok(profiles)
        })
    }

    fn get(&self, id: &str) -> Result<Option<GameProfile>, AppError> {
        let id = id.to_string();
        self.db.read("get_profile", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, display_name, source_identity, server_label, \
                     maturity_mode, maturity_level, revision, created_at, updated_at, archived_at \
                     FROM game_profile WHERE id = ?1",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let mut rows = stmt
                .query_map(params![id], |row| {
                    Ok(GameProfile {
                        id: row.get(0)?,
                        display_name: row.get(1)?,
                        source_identity: row.get(2)?,
                        server_label: row.get(3)?,
                        maturity_mode: MaturityMode::from_str(&row.get::<_, String>(4)?)
                            .unwrap_or(MaturityMode::Auto),
                        maturity_level: row
                            .get::<_, Option<String>>(5)?
                            .and_then(|s| MaturityLevel::from_str(&s)),
                        revision: row.get::<_, u32>(6)?,
                        created_at: row.get(7)?,
                        updated_at: row.get(8)?,
                        archived_at: row.get(9)?,
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            match rows.next() {
                Some(Ok(p)) => Ok(Some(p)),
                Some(Err(e)) => Err(AppError::database("read row", &e)),
                None => Ok(None),
            }
        })
    }

    fn rename(
        &self,
        id: &str,
        new_name: &str,
        expected_revision: u32,
        now: &str,
    ) -> Result<GameProfile, AppError> {
        let id = id.to_string();
        let new_name = new_name.to_string();
        let now = now.to_string();

        self.db.write("rename_profile", |conn| {
            // 检查修订号
            let actual: u32 = conn
                .query_row("SELECT revision FROM game_profile WHERE id = ?1", params![id], |row| {
                    row.get(0)
                })
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        AppError::not_found("game_profile", &id)
                    }
                    other => AppError::database("check revision", &other),
                })?;

            if actual != expected_revision {
                return Err(AppError::conflict("game_profile", &id, expected_revision, actual));
            }

            conn.execute(
                "UPDATE game_profile SET display_name = ?1, revision = revision + 1, updated_at = ?2 WHERE id = ?3",
                params![new_name, now, id],
            )
            .map_err(|e| AppError::database("rename", &e))?;
            Ok(())
        })?;

        self.get(&id)?
            .ok_or_else(|| AppError::internal("档案创建后无法读取"))
    }

    fn set_archived(
        &self,
        id: &str,
        archived: bool,
        expected_revision: u32,
        now: &str,
    ) -> Result<GameProfile, AppError> {
        let id = id.to_string();
        let now = now.to_string();

        self.db.write("archive_profile", |conn| {
            let actual: u32 = conn
                .query_row("SELECT revision FROM game_profile WHERE id = ?1", params![id], |row| {
                    row.get(0)
                })
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        AppError::not_found("game_profile", &id)
                    }
                    other => AppError::database("check revision", &other),
                })?;

            if actual != expected_revision {
                return Err(AppError::conflict("game_profile", &id, expected_revision, actual));
            }

            let archived_at: Option<String> = if archived { Some(now.clone()) } else { None };
            conn.execute(
                "UPDATE game_profile SET archived_at = ?1, revision = revision + 1, updated_at = ?2 WHERE id = ?3",
                params![archived_at, now, id],
            )
            .map_err(|e| AppError::database("archive", &e))?;
            Ok(())
        })?;

        self.get(&id)?
            .ok_or_else(|| AppError::internal("档案创建后无法读取"))
    }

    fn system_thresholds(&self) -> Result<Vec<MaturityThreshold>, AppError> {
        self.db.read("system_thresholds", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT scope_type, profile_id, level, min_count FROM maturity_threshold WHERE scope_type = 'system'",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let rows = stmt
                .query_map([], row_to_threshold)
                .map_err(|e| AppError::database("query", &e))?;
            collect_thresholds(rows)
        })
    }

    fn profile_thresholds(&self, profile_id: &str) -> Result<Vec<MaturityThreshold>, AppError> {
        let pid = profile_id.to_string();
        self.db.read("profile_thresholds", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT scope_type, profile_id, level, min_count FROM maturity_threshold WHERE profile_id = ?1",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let rows = stmt
                .query_map(params![pid], row_to_threshold)
                .map_err(|e| AppError::database("query", &e))?;
            collect_thresholds(rows)
        })
    }

    fn replace_profile_thresholds(
        &self,
        profile_id: &str,
        thresholds: &[MaturityThreshold],
    ) -> Result<(), AppError> {
        let pid = profile_id.to_string();
        let thresholds = thresholds.to_vec();
        self.db.write("replace_thresholds", |conn| {
            conn.execute(
                "DELETE FROM maturity_threshold WHERE scope_type = 'profile' AND profile_id = ?1",
                params![pid],
            )
            .map_err(|e| AppError::database("delete thresholds", &e))?;

            for t in &thresholds {
                conn.execute(
                    "INSERT INTO maturity_threshold (scope_type, profile_id, level, min_count) VALUES ('profile', ?1, ?2, ?3)",
                    params![pid, t.level.as_str(), t.min_count],
                )
                .map_err(|e| AppError::database("insert threshold", &e))?;
            }
            Ok(())
        })
    }
}

fn row_to_threshold(row: &rusqlite::Row) -> rusqlite::Result<MaturityThreshold> {
    Ok(MaturityThreshold {
        scope_type: row.get(0)?,
        profile_id: row.get(1)?,
        level: MaturityLevel::from_str(&row.get::<_, String>(2)?).unwrap_or(MaturityLevel::Starter),
        min_count: row.get(3)?,
    })
}

fn collect_thresholds<I>(rows: I) -> Result<Vec<MaturityThreshold>, AppError>
where
    I: IntoIterator<Item = rusqlite::Result<MaturityThreshold>>,
{
    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|e| AppError::database("read threshold", &e))?);
    }
    Ok(result)
}

// ─── 持有式神仓库 ─────────────────────────────────────────────────────────────
// 玩家持有的式神实例镜像；当前数据覆盖导入时整表替换，页面展示仍以
// current-data.json 为单一事实源，这里供数据库侧查询与后续分析使用。

pub struct SqliteOwnedShikigamiRepository {
    db: Arc<Database>,
}

impl SqliteOwnedShikigamiRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// 替换指定角色档案的式神镜像（事务内先清空该角色再写入）。
    pub fn replace_all(
        &self,
        profile_id: &str,
        records: &[OwnedShikigami],
        source_kind: &str,
        imported_at: &str,
    ) -> Result<(), AppError> {
        self.db.write("replace_owned_shikigami", |conn| {
            conn.execute(
                "DELETE FROM owned_shikigami WHERE profile_id = ?1",
                params![profile_id],
            )
            .map_err(|error| AppError::database("owned_shikigami", &error))?;
            let mut insert = conn
                .prepare(
                    "INSERT INTO owned_shikigami (\
                         profile_id, instance_id, shikigami_id, star, level, exp, locked, \
                         awakened, skin_id, skills_json, selected_skill_ids_json, source_kind, \
                         imported_at\
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                )
                .map_err(|error| AppError::database("owned_shikigami", &error))?;
            for record in records {
                let skills =
                    serde_json::to_string(&record.skills).unwrap_or_else(|_| "[]".to_owned());
                let selected_skill_ids = serde_json::to_string(&record.selected_skill_ids)
                    .unwrap_or_else(|_| "[]".to_owned());
                insert
                    .execute(params![
                        profile_id,
                        record.instance_id,
                        record.shikigami_id,
                        record.star,
                        record.level,
                        record.exp,
                        record.locked,
                        record.awakened,
                        record.skin_id,
                        skills,
                        selected_skill_ids,
                        source_kind,
                        imported_at,
                    ])
                    .map_err(|error| AppError::database("owned_shikigami", &error))?;
            }
            Ok(())
        })
    }

    /// 清空指定角色档案的式神镜像（清空角色数据时调用）。
    pub fn clear(&self, profile_id: &str) -> Result<(), AppError> {
        self.db.write("clear_owned_shikigami", |conn| {
            conn.execute(
                "DELETE FROM owned_shikigami WHERE profile_id = ?1",
                params![profile_id],
            )
            .map_err(|error| AppError::database("owned_shikigami", &error))?;
            Ok(())
        })
    }

    /// 指定角色档案在数据库中的式神实例总数；无镜像时返回 0。
    pub fn count(&self, profile_id: &str) -> Result<u64, AppError> {
        self.db.read("count_owned_shikigami", |conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM owned_shikigami WHERE profile_id = ?1",
                    params![profile_id],
                    |row| row.get(0),
                )
                .map_err(|error| AppError::database("owned_shikigami", &error))?;
            Ok(count.max(0) as u64)
        })
    }
}

// ─── 原始对象仓库 ─────────────────────────────────────────────────────────────

pub struct SqliteRawObjectRepository {
    db: Arc<Database>,
}

impl SqliteRawObjectRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl RawObjectRepository for SqliteRawObjectRepository {
    fn upsert(&self, object: &RawObject) -> Result<(), AppError> {
        let obj = object.clone();
        self.db.write("upsert_raw_object", |conn| {
            conn.execute(
                "INSERT INTO raw_object (sha256, relative_path, compression, media_type, \
                 raw_size, stored_size, verified_at, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT(sha256) DO UPDATE SET
                    relative_path = excluded.relative_path,
                    compression = excluded.compression,
                    media_type = excluded.media_type,
                    raw_size = excluded.raw_size,
                    stored_size = excluded.stored_size,
                    verified_at = excluded.verified_at",
                params![
                    obj.sha256,
                    obj.relative_path,
                    obj.compression,
                    obj.media_type,
                    obj.raw_size,
                    obj.stored_size,
                    obj.verified_at,
                    obj.created_at,
                ],
            )
            .map_err(|e| AppError::constraint_violation("raw_object", &e))?;
            Ok(())
        })
    }

    fn get(&self, sha256: &str) -> Result<Option<RawObject>, AppError> {
        let hash = sha256.to_string();
        self.db.read("get_raw_object", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT sha256, relative_path, compression, media_type, raw_size, stored_size, \
                     verified_at, created_at FROM raw_object WHERE sha256 = ?1",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let mut rows = stmt
                .query_map(params![hash], |row| {
                    Ok(RawObject {
                        sha256: row.get(0)?,
                        relative_path: row.get(1)?,
                        compression: row.get(2)?,
                        media_type: row.get(3)?,
                        raw_size: row.get(4)?,
                        stored_size: row.get(5)?,
                        verified_at: row.get(6)?,
                        created_at: row.get(7)?,
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            match rows.next() {
                Some(Ok(r)) => Ok(Some(r)),
                Some(Err(e)) => Err(AppError::database("read row", &e)),
                None => Ok(None),
            }
        })
    }

    fn stats(&self) -> Result<RawObjectStats, AppError> {
        self.db.read("raw_object_stats", |conn| {
            let (count, raw, stored): (i64, Option<i64>, Option<i64>) = conn
                .query_row(
                    "SELECT count(*), SUM(raw_size), SUM(stored_size) FROM raw_object",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|e| AppError::database("stats", &e))?;
            Ok(RawObjectStats {
                object_count: count as u64,
                raw_bytes: raw.unwrap_or(0) as u64,
                stored_bytes: stored.unwrap_or(0) as u64,
            })
        })
    }
}

// ─── 采集事件仓库 ─────────────────────────────────────────────────────────────

pub struct SqliteAcquisitionEventRepository {
    db: Arc<Database>,
}

impl SqliteAcquisitionEventRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl AcquisitionEventRepository for SqliteAcquisitionEventRepository {
    fn insert(&self, event: &AcquisitionEvent) -> Result<(), AppError> {
        let e = event.clone();
        self.db.write("insert_event", |conn| {
            conn.execute(
                "INSERT INTO acquisition_event (id, profile_id, source_kind, source_format, \
                 raw_sha256, captured_at, received_at, game_version, adapter_version, \
                 parser_version, completeness, scope_json, result_kind, snapshot_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    e.id,
                    e.profile_id,
                    e.source_kind,
                    e.source_format,
                    e.raw_sha256,
                    e.captured_at,
                    e.received_at,
                    e.game_version,
                    e.adapter_version,
                    e.parser_version,
                    e.completeness,
                    e.scope_json,
                    e.result_kind,
                    e.snapshot_id,
                ],
            )
            .map_err(|e| AppError::constraint_violation("acquisition_event", &e))?;
            Ok(())
        })
    }

    fn list_by_profile(
        &self,
        profile_id: &str,
        limit: u32,
    ) -> Result<Vec<AcquisitionEvent>, AppError> {
        let pid = profile_id.to_string();
        self.db.read("list_events", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, profile_id, source_kind, source_format, raw_sha256, captured_at, \
                     received_at, game_version, adapter_version, parser_version, completeness, \
                     scope_json, result_kind, snapshot_id \
                     FROM acquisition_event WHERE profile_id = ?1 ORDER BY received_at DESC LIMIT ?2",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let rows = stmt
                .query_map(params![pid, limit], |row| {
                    Ok(AcquisitionEvent {
                        id: row.get(0)?,
                        profile_id: row.get(1)?,
                        source_kind: row.get(2)?,
                        source_format: row.get(3)?,
                        raw_sha256: row.get(4)?,
                        captured_at: row.get(5)?,
                        received_at: row.get(6)?,
                        game_version: row.get(7)?,
                        adapter_version: row.get(8)?,
                        parser_version: row.get(9)?,
                        completeness: row.get(10)?,
                        scope_json: row.get(11)?,
                        result_kind: row.get(12)?,
                        snapshot_id: row.get(13)?,
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            let mut events = Vec::new();
            for row in rows {
                events.push(row.map_err(|e| AppError::database("read row", &e))?);
            }
            Ok(events)
        })
    }
}

// ─── 快照仓库 ─────────────────────────────────────────────────────────────────

pub struct SqliteSnapshotRepository {
    db: Arc<Database>,
}

impl SqliteSnapshotRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl SnapshotRepository for SqliteSnapshotRepository {
    fn clear_inventory(&self, profile_id: &str) -> Result<(), AppError> {
        let profile_id = profile_id.to_owned();
        self.db.write("clear_inventory", |conn| {
            // 删除顺序遵循外键依赖：先删用户决定、分析结果和库存投影，再删雷达缓存与快照事实。
            for (operation, sql) in [
                (
                    "clear decision history",
                    "DELETE FROM user_decision_history WHERE profile_id = ?1",
                ),
                (
                    "clear decisions",
                    "DELETE FROM user_decision WHERE profile_id = ?1",
                ),
                (
                    "clear action batches",
                    "DELETE FROM action_batch WHERE profile_id = ?1",
                ),
                (
                    "clear analysis todos",
                    "DELETE FROM analysis_todo WHERE profile_id = ?1",
                ),
                (
                    "clear acquisition events",
                    "DELETE FROM acquisition_event WHERE profile_id = ?1",
                ),
                (
                    "clear inventory",
                    "DELETE FROM inventory_item WHERE profile_id = ?1",
                ),
                (
                    "clear soul radar cache",
                    "DELETE FROM soul_radar_cache WHERE profile_id = ?1",
                ),
                (
                    "clear head tail cache",
                    "DELETE FROM head_tail_cache WHERE profile_id = ?1",
                ),
            ] {
                conn.execute(sql, params![profile_id])
                    .map_err(|error| AppError::database(operation, &error))?;
            }

            // 快照链存在 parent_snapshot_id 自引用，先断开链条再整体删除，保证 SQLite 外键检查通过。
            conn.execute(
                "UPDATE snapshot SET parent_snapshot_id = NULL WHERE profile_id = ?1",
                params![profile_id],
            )
            .map_err(|error| AppError::database("clear snapshot links", &error))?;
            conn.execute(
                "DELETE FROM snapshot WHERE profile_id = ?1",
                params![profile_id],
            )
            .map_err(|error| AppError::database("clear snapshots", &error))?;
            Ok(())
        })
    }

    fn insert_snapshot(
        &self,
        snapshot: &Snapshot,
        souls: &[SnapshotSoul],
        attributes: &[SoulAttribute],
    ) -> Result<(), AppError> {
        let s = snapshot.clone();
        let souls = souls.to_vec();
        let attrs = attributes.to_vec();

        self.db.write("insert_snapshot", |conn| {
            conn.execute(
                "INSERT INTO snapshot (id, profile_id, raw_sha256, content_fingerprint, \
                 scope_fingerprint, source_kind, completeness, scope_json, captured_at, game_version, \
                 adapter_version, parser_version, catalog_version, parent_snapshot_id, \
                 forced, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    s.id, s.profile_id, s.raw_sha256, s.content_fingerprint, s.scope_fingerprint,
                    s.source_kind, s.completeness, s.scope_json, s.captured_at, s.game_version,
                    s.adapter_version, s.parser_version, s.catalog_version,
                    s.parent_snapshot_id, s.forced, s.created_at,
                ],
            )
            .map_err(|e| AppError::constraint_violation("snapshot", &e))?;

            for soul in &souls {
                conn.execute(
                    "INSERT INTO snapshot_soul (snapshot_id, internal_id, source_stable_id, \
                     identity_quality, set_id, slot, quality, level, main_attr_type, \
                     main_attr_value, initial_substat_count, locked_in_source, \
                     equipped_state, source_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    params![
                        soul.snapshot_id, soul.internal_id, soul.source_stable_id,
                        soul.identity_quality, soul.set_id, soul.slot, soul.quality,
                        soul.level, soul.main_attr_type, soul.main_attr_value,
                        soul.initial_substat_count, soul.locked_in_source,
                        soul.equipped_state, soul.source_json,
                    ],
                )
                .map_err(|e| AppError::constraint_violation("snapshot_soul", &e))?;
            }

            for attr in &attrs {
                conn.execute(
                    "INSERT INTO soul_attribute (snapshot_id, soul_internal_id, attribute_index, \
                     attribute_type, value, enhancement_count, count_provenance, fixed_attribute) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        attr.snapshot_id, attr.soul_internal_id, attr.attribute_index,
                        attr.attribute_type, attr.value, attr.enhancement_count,
                        attr.count_provenance, attr.fixed_attribute,
                    ],
                )
                .map_err(|e| AppError::constraint_violation("soul_attribute", &e))?;
            }
            Ok(())
        })
    }

    fn get(&self, id: &str) -> Result<Option<Snapshot>, AppError> {
        let sid = id.to_string();
        self.db.read("get_snapshot", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, profile_id, raw_sha256, content_fingerprint, scope_fingerprint, source_kind, \
                     completeness, scope_json, captured_at, game_version, adapter_version, \
                     parser_version, catalog_version, parent_snapshot_id, forced, created_at \
                     FROM snapshot WHERE id = ?1",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let mut rows = stmt
                .query_map(params![sid], |row| {
                    Ok(Snapshot {
                        id: row.get(0)?,
                        profile_id: row.get(1)?,
                        raw_sha256: row.get(2)?,
                        content_fingerprint: row.get(3)?,
                        scope_fingerprint: row.get(4)?,
                        source_kind: row.get(5)?,
                        completeness: row.get(6)?,
                        scope_json: row.get(7)?,
                        captured_at: row.get(8)?,
                        game_version: row.get(9)?,
                        adapter_version: row.get(10)?,
                        parser_version: row.get(11)?,
                        catalog_version: row.get(12)?,
                        parent_snapshot_id: row.get(13)?,
                        forced: row.get::<_, i64>(14)? != 0,
                        created_at: row.get(15)?,
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            match rows.next() {
                Some(Ok(s)) => Ok(Some(s)),
                Some(Err(e)) => Err(AppError::database("read row", &e)),
                None => Ok(None),
            }
        })
    }

    fn list_by_profile(&self, profile_id: &str) -> Result<Vec<Snapshot>, AppError> {
        let pid = profile_id.to_string();
        self.db.read("list_snapshots", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, profile_id, raw_sha256, content_fingerprint, scope_fingerprint, source_kind, \
                     completeness, scope_json, captured_at, game_version, adapter_version, \
                     parser_version, catalog_version, parent_snapshot_id, forced, created_at \
                     FROM snapshot WHERE profile_id = ?1 ORDER BY created_at DESC",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let rows = stmt
                .query_map(params![pid], |row| {
                    Ok(Snapshot {
                        id: row.get(0)?,
                        profile_id: row.get(1)?,
                        raw_sha256: row.get(2)?,
                        content_fingerprint: row.get(3)?,
                        scope_fingerprint: row.get(4)?,
                        source_kind: row.get(5)?,
                        completeness: row.get(6)?,
                        scope_json: row.get(7)?,
                        captured_at: row.get(8)?,
                        game_version: row.get(9)?,
                        adapter_version: row.get(10)?,
                        parser_version: row.get(11)?,
                        catalog_version: row.get(12)?,
                        parent_snapshot_id: row.get(13)?,
                        forced: row.get::<_, i64>(14)? != 0,
                        created_at: row.get(15)?,
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            let mut snapshots = Vec::new();
            for row in rows {
                snapshots.push(row.map_err(|e| AppError::database("read row", &e))?);
            }
            Ok(snapshots)
        })
    }

    fn upsert_inventory(&self, item: &InventoryItem) -> Result<(), AppError> {
        let i = item.clone();
        self.db.write("upsert_inventory", |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO inventory_item (profile_id, soul_key, snapshot_id, \
                 soul_internal_id, first_seen_snapshot_id, last_seen_snapshot_id, \
                 presence_state, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    i.profile_id,
                    i.soul_key,
                    i.snapshot_id,
                    i.soul_internal_id,
                    i.first_seen_snapshot_id,
                    i.last_seen_snapshot_id,
                    i.presence_state,
                    i.updated_at,
                ],
            )
            .map_err(|e| AppError::constraint_violation("inventory_item", &e))?;
            Ok(())
        })
    }

    fn list_inventory(&self, profile_id: &str) -> Result<Vec<InventoryItem>, AppError> {
        let pid = profile_id.to_owned();
        self.db.read("list_inventory", |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT profile_id, soul_key, snapshot_id, soul_internal_id, \
                     first_seen_snapshot_id, last_seen_snapshot_id, presence_state, updated_at \
                     FROM inventory_item WHERE profile_id = ?1 ORDER BY soul_key",
                )
                .map_err(|error| AppError::database("prepare list_inventory", &error))?;
            let rows = statement
                .query_map(params![pid], |row| {
                    Ok(InventoryItem {
                        profile_id: row.get(0)?,
                        soul_key: row.get(1)?,
                        snapshot_id: row.get(2)?,
                        soul_internal_id: row.get(3)?,
                        first_seen_snapshot_id: row.get(4)?,
                        last_seen_snapshot_id: row.get(5)?,
                        presence_state: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                })
                .map_err(|error| AppError::database("query list_inventory", &error))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read list_inventory row", &error))
        })
    }

    fn list_inventory_page(
        &self,
        profile_id: &str,
        query: &InventoryPageQuery,
    ) -> Result<InventoryPage, AppError> {
        // 页面默认请求 10 条，后端仍限制最大页长，避免异常调用重新拖入整份库存。
        const MAX_PAGE_SIZE: u32 = 100;
        let pid = profile_id.to_owned();
        let limit = query.limit.clamp(1, MAX_PAGE_SIZE);
        let offset = query.offset;
        let query = query.clone();

        self.db.read("list_inventory_page", |conn| {
            let mut where_clauses = vec![
                "i.profile_id = ?1".to_owned(),
                "i.presence_state <> 'removed'".to_owned(),
            ];
            let mut values = vec![Value::Text(pid.clone())];
            // 默认按稳定御魂键排序；启用副属性数值筛选后改为按匹配属性值排序。
            let mut attribute_order_sql = "i.soul_key ASC".to_owned();
            // 数值筛选改为先聚合命中的副属性，再与库存连接，避免每条库存记录重复执行相关子查询。
            let mut attribute_join_sql = String::new();

            // 所有筛选参数都绑定为 SQLite 参数，避免把界面输入拼接进 SQL。
            if let Some(set_id) = query.set_id.as_deref() {
                let index = values.len() + 1;
                where_clauses.push(format!("s.set_id = ?{index}"));
                values.push(Value::Text(set_id.to_owned()));
            }
            if let Some(set_category) = query.set_category.as_deref() {
                let index = values.len() + 1;
                // 目录分类与前端使用同一优先级：首领/星痕等特殊分类覆盖普通二件套分类。
                where_clauses.push(format!(
                    "EXISTS (SELECT 1 FROM soul_set catalog_set \
                     INNER JOIN catalog_package catalog_package \
                       ON catalog_package.version = catalog_set.catalog_version \
                      AND catalog_package.active = 1 \
                     WHERE catalog_set.set_id = s.set_id \
                       AND COALESCE(catalog_set.special_category, catalog_set.category) = ?{index})"
                ));
                values.push(Value::Text(set_category.to_owned()));
            }
            if let Some(slot) = query.slot {
                let index = values.len() + 1;
                where_clauses.push(format!("s.slot = ?{index}"));
                values.push(Value::Integer(i64::from(slot)));
            }
            if let Some(quality) = query.quality {
                let index = values.len() + 1;
                where_clauses.push(format!("s.quality = ?{index}"));
                values.push(Value::Integer(i64::from(quality)));
            }
            if let Some(level) = query.level {
                let index = values.len() + 1;
                where_clauses.push(format!("s.level = ?{index}"));
                values.push(Value::Integer(i64::from(level)));
            }
            if let Some(main_attr_type) = query.main_attr_type.as_deref() {
                let index = values.len() + 1;
                where_clauses.push(format!("s.main_attr_type = ?{index}"));
                values.push(Value::Text(main_attr_type.to_owned()));
            }
            if let Some(sub_attr_type) = query.sub_attr_type.as_deref() {
                let index = values.len() + 1;
                where_clauses.push(format!(
                    "EXISTS (SELECT 1 FROM soul_attribute a \
                     WHERE a.snapshot_id = s.snapshot_id \
                       AND a.soul_internal_id = s.internal_id \
                       AND a.fixed_attribute = 0 \
                       AND a.attribute_type = ?{index})"
                ));
                values.push(Value::Text(sub_attr_type.to_owned()));
            }
            if let (Some(attribute_type), Some(attribute_operator), Some(attribute_value)) = (
                query.attribute_type.as_deref(),
                query.attribute_operator.as_deref(),
                query.attribute_value,
            ) {
                // 比较符只允许白名单中的两种 SQL 运算，属性类型和值仍通过参数绑定。
                let (comparison, sort_direction) = match attribute_operator {
                    "gt" => (">", "DESC"),
                    "lt" => ("<", "ASC"),
                    _ => {
                        return Err(AppError::invalid_argument(
                            "attributeOperator",
                            "属性比较方式不受支持",
                        ));
                    }
                };
                if !attribute_value.is_finite() {
                    return Err(AppError::invalid_argument(
                        "attributeValue",
                        "属性数值必须是有限数字",
                    ));
                }
                let attribute_type_index = values.len() + 1;
                values.push(Value::Text(attribute_type.to_owned()));
                let attribute_value_index = values.len() + 1;
                values.push(Value::Real(attribute_value));
                // 数值筛选只检查副属性；先按御魂聚合命中值，再按比较方向排序。
                attribute_join_sql = format!(
                    "INNER JOIN (SELECT value_attribute.snapshot_id, \
                                        value_attribute.soul_internal_id, \
                                        MAX(value_attribute.value) AS attribute_value \
                                   FROM soul_attribute value_attribute \
                                  WHERE value_attribute.fixed_attribute = 0 \
                                    AND value_attribute.attribute_type = ?{attribute_type_index} \
                                    AND value_attribute.value {comparison} ?{attribute_value_index} \
                                  GROUP BY value_attribute.snapshot_id, value_attribute.soul_internal_id) \
                              attribute_match \
                         ON attribute_match.snapshot_id = s.snapshot_id \
                        AND attribute_match.soul_internal_id = s.internal_id"
                );
                attribute_order_sql = format!(
                    "attribute_match.attribute_value {sort_direction}, i.soul_key ASC"
                );
            }
            if let Some(standard_score_min) = query.standard_score_min {
                let index = values.len() + 1;
                // 评分结果保存在分析结果表字段中；未计算或未命中规则卡片的御魂不会通过分数筛选。
                where_clauses.push(format!("score.standard_score > ?{index}"));
                values.push(Value::Real(standard_score_min));
            }
            if let Some(standard_score_max) = query.standard_score_max {
                let index = values.len() + 1;
                // 最高分使用严格小于；与最低分同时填写时由同一 SQL 组合成开区间。
                where_clauses.push(format!("score.standard_score < ?{index}"));
                values.push(Value::Real(standard_score_max));
            }
            if query.auspicious_only {
                // 大吉只认可强化属性，固有属性不计入，和“我的御魂”卡片上的标识保持一致。
                where_clauses.push(
                    "EXISTS (SELECT 1 FROM soul_attribute auspicious_attribute \
                     WHERE auspicious_attribute.snapshot_id = s.snapshot_id \
                       AND auspicious_attribute.soul_internal_id = s.internal_id \
                       AND auspicious_attribute.fixed_attribute = 0 \
                       AND auspicious_attribute.enhancement_count >= 5 \
                       AND auspicious_attribute.attribute_type IN \
                           ('crit_rate', 'crit_damage', 'effect_hit', 'speed', 'effect_resist'))"
                        .to_owned(),
                );
            }
            let where_sql = where_clauses.join(" AND ");

            let inventory_total: u32 = conn
                .query_row(
                    "SELECT COUNT(*) FROM inventory_item \
                     WHERE profile_id = ?1 AND presence_state <> 'removed'",
                    params![pid.as_str()],
                    |row| row.get(0),
                )
                .map_err(|error| AppError::database("count current inventory", &error))?;

            // 来源标志按当前库存整体汇总，而不是按当前分页汇总，避免翻页后标题来源变化。
            let mut source_statement = conn
                .prepare(
                    "SELECT DISTINCT s.source_kind FROM inventory_item i \
                     INNER JOIN snapshot s ON s.id = i.snapshot_id \
                     WHERE i.profile_id = ?1 AND i.presence_state <> 'removed' \
                     ORDER BY s.source_kind",
                )
                .map_err(|error| AppError::database("prepare inventory source kinds", &error))?;
            let source_rows = source_statement
                .query_map(params![pid.as_str()], |row| row.get::<_, String>(0))
                .map_err(|error| AppError::database("query inventory source kinds", &error))?;
            let source_kinds = source_rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read inventory source kind", &error))?;

            // 普通筛选保留精确总数；数值筛选只取前 N 条，不再为高命中条件执行全量 COUNT。
            let counted_total = if attribute_join_sql.is_empty() {
                let total_sql = format!(
                    "SELECT COUNT(*) FROM inventory_item i \
                     INNER JOIN snapshot_soul s ON s.snapshot_id = i.snapshot_id \
                        AND s.internal_id = i.soul_internal_id \
                     {attribute_join_sql} \
                     LEFT JOIN analysis_todo score ON score.profile_id = i.profile_id \
                        AND score.soul_key = i.soul_key \
                     WHERE {where_sql}"
                );
                Some(
                    conn.query_row(&total_sql, params_from_iter(values.clone()), |row| row.get(0))
                        .map_err(|error| AppError::database("count inventory page", &error))?,
                )
            } else {
                None
            };

            let limit_index = values.len() + 1;
            let offset_index = values.len() + 2;
            let select_sql = format!(
                "SELECT i.profile_id, i.soul_key, i.snapshot_id, i.soul_internal_id, \
                        i.first_seen_snapshot_id, i.last_seen_snapshot_id, \
                        i.presence_state, i.updated_at \
                 FROM inventory_item i \
                 INNER JOIN snapshot_soul s ON s.snapshot_id = i.snapshot_id \
                    AND s.internal_id = i.soul_internal_id \
                 {attribute_join_sql} \
                 LEFT JOIN analysis_todo score ON score.profile_id = i.profile_id \
                    AND score.soul_key = i.soul_key \
                 WHERE {where_sql} \
                 ORDER BY {attribute_order_sql} \
                 LIMIT ?{limit_index} OFFSET ?{offset_index}"
            );
            // 数值筛选多取一条只用于判断“加载更多”，随后立即截断，不把额外记录交给前端。
            let fetch_limit = if attribute_join_sql.is_empty() {
                limit
            } else {
                limit.saturating_add(1)
            };
            values.push(Value::Integer(i64::from(fetch_limit)));
            values.push(Value::Integer(i64::from(offset)));
            let mut statement = conn
                .prepare(&select_sql)
                .map_err(|error| AppError::database("prepare list inventory page", &error))?;
            let rows = statement
                .query_map(params_from_iter(values), |row| {
                    Ok(InventoryItem {
                        profile_id: row.get(0)?,
                        soul_key: row.get(1)?,
                        snapshot_id: row.get(2)?,
                        soul_internal_id: row.get(3)?,
                        first_seen_snapshot_id: row.get(4)?,
                        last_seen_snapshot_id: row.get(5)?,
                        presence_state: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                })
                .map_err(|error| AppError::database("query list inventory page", &error))?;
            let mut items = rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read inventory page row", &error))?;
            let has_more = !attribute_join_sql.is_empty() && items.len() > limit as usize;
            if has_more {
                items.truncate(limit as usize);
            }
            let total = counted_total.unwrap_or_else(|| {
                offset.saturating_add(items.len().try_into().unwrap_or(u32::MAX))
            });

            // 筛选选项只返回去重后的基础值，供前端保留完整筛选能力，不加载御魂属性正文。
            let mut option_statement = conn
                .prepare(
                    "SELECT DISTINCT s.set_id, s.slot, s.quality, s.level, s.main_attr_type \
                     FROM inventory_item i \
                     INNER JOIN snapshot_soul s ON s.snapshot_id = i.snapshot_id \
                        AND s.internal_id = i.soul_internal_id \
                     WHERE i.profile_id = ?1 AND i.presence_state <> 'removed' \
                     ORDER BY s.set_id, s.slot, s.quality, s.level, s.main_attr_type",
                )
                .map_err(|error| AppError::database("prepare inventory filter options", &error))?;
            let option_rows = option_statement
                .query_map(params![pid.as_str()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u8>(1)?,
                        row.get::<_, u8>(2)?,
                        row.get::<_, u8>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })
                .map_err(|error| AppError::database("query inventory filter options", &error))?;
            let mut filter_options = InventoryFilterOptions::default();
            for row in option_rows {
                let (set_id, slot, quality, level, main_attr_type) = row
                    .map_err(|error| AppError::database("read inventory filter option", &error))?;
                filter_options.set_ids.push(set_id);
                filter_options.slots.push(slot);
                filter_options.qualities.push(quality);
                filter_options.levels.push(level);
                filter_options.main_attr_types.push(main_attr_type);
            }

            let mut sub_attr_statement = conn
                .prepare(
                    "SELECT DISTINCT a.attribute_type \
                     FROM inventory_item i \
                     INNER JOIN soul_attribute a ON a.snapshot_id = i.snapshot_id \
                        AND a.soul_internal_id = i.soul_internal_id \
                     WHERE i.profile_id = ?1 AND i.presence_state <> 'removed' \
                       AND a.fixed_attribute = 0 \
                     ORDER BY a.attribute_type",
                )
                .map_err(|error| AppError::database("prepare sub attribute options", &error))?;
            let sub_attr_rows = sub_attr_statement
                .query_map(params![pid.as_str()], |row| row.get::<_, String>(0))
                .map_err(|error| AppError::database("query sub attribute options", &error))?;
            for row in sub_attr_rows {
                filter_options.sub_attr_types.push(
                    row.map_err(|error| AppError::database("read sub attribute option", &error))?,
                );
            }

            filter_options.set_ids.sort_unstable();
            filter_options.set_ids.dedup();
            filter_options.slots.sort_unstable();
            filter_options.slots.dedup();
            filter_options.qualities.sort_unstable();
            filter_options.qualities.dedup();
            filter_options.levels.sort_unstable();
            filter_options.levels.dedup();
            filter_options.main_attr_types.sort_unstable();
            filter_options.main_attr_types.dedup();
            filter_options.sub_attr_types.sort_unstable();
            filter_options.sub_attr_types.dedup();

            Ok(InventoryPage {
                items,
                total,
                has_more,
                inventory_total,
                source_kinds,
                limit,
                offset,
                filter_options,
            })
        })
    }

    /// 一次性读取当前档案中仍可能存在的库存及其快照事实，避免组合搜索产生分页或 N+1 查询。
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
    > {
        let pid = profile_id.to_owned();
        self.db.read("list_inventory_facts", |conn| {
            // 只统计未确认数量，不加载它们的快照正文；这样返回事实集合天然不会混入不可靠记录。
            let excluded_unconfirmed_count: u32 = conn
                .query_row(
                    "SELECT COUNT(*) FROM inventory_item WHERE profile_id = ?1 AND presence_state = 'unconfirmed'",
                    params![pid.as_str()],
                    |row| row.get(0),
                )
                .map_err(|error| AppError::database("count unconfirmed inventory", &error))?;
            let expected_present_count: u32 = conn
                .query_row(
                    "SELECT COUNT(*) FROM inventory_item WHERE profile_id = ?1 AND presence_state = 'present'",
                    params![pid.as_str()],
                    |row| row.get(0),
                )
                .map_err(|error| AppError::database("count present inventory", &error))?;

            let mut statement = conn
                .prepare(
                    "SELECT i.profile_id, i.soul_key, i.snapshot_id, i.soul_internal_id, \
                            i.first_seen_snapshot_id, i.last_seen_snapshot_id, i.presence_state, i.updated_at, \
                            s.snapshot_id, s.internal_id, s.source_stable_id, s.identity_quality, s.set_id, \
                            s.slot, s.quality, s.level, s.main_attr_type, s.main_attr_value, \
                            s.initial_substat_count, s.locked_in_source, s.equipped_state, s.source_json, \
                            a.snapshot_id, a.soul_internal_id, a.attribute_index, a.attribute_type, a.value, \
                            a.enhancement_count, a.count_provenance, a.fixed_attribute \
                     FROM inventory_item i \
                     INNER JOIN snapshot_soul s ON s.snapshot_id = i.snapshot_id \
                        AND s.internal_id = i.soul_internal_id \
                     LEFT JOIN soul_attribute a ON a.snapshot_id = s.snapshot_id \
                        AND a.soul_internal_id = s.internal_id \
                     WHERE i.profile_id = ?1 AND i.presence_state = 'present' \
                     ORDER BY i.soul_key, a.attribute_index",
                )
                .map_err(|error| AppError::database("prepare inventory facts", &error))?;
            let mut inventory = Vec::new();
            let mut inventory_indexes = HashMap::<String, usize>::new();
            let mut souls = Vec::new();
            let mut soul_indexes = HashMap::<(String, String), usize>::new();
            let mut attributes = Vec::new();
            let rows = statement
                .query_map(params![pid.as_str()], |row| {
                    let item = InventoryItem {
                        profile_id: row.get(0)?,
                        soul_key: row.get(1)?,
                        snapshot_id: row.get(2)?,
                        soul_internal_id: row.get(3)?,
                        first_seen_snapshot_id: row.get(4)?,
                        last_seen_snapshot_id: row.get(5)?,
                        presence_state: row.get(6)?,
                        updated_at: row.get(7)?,
                    };
                    let soul = SnapshotSoul {
                        snapshot_id: row.get(8)?,
                        internal_id: row.get(9)?,
                        source_stable_id: row.get(10)?,
                        identity_quality: row.get(11)?,
                        set_id: row.get(12)?,
                        slot: row.get(13)?,
                        quality: row.get(14)?,
                        level: row.get(15)?,
                        main_attr_type: row.get(16)?,
                        main_attr_value: row.get(17)?,
                        initial_substat_count: row.get(18)?,
                        locked_in_source: row.get::<_, Option<i64>>(19)?.map(|value| value != 0),
                        equipped_state: row.get(20)?,
                        source_json: row.get(21)?,
                    };
                    let attribute_index = row.get::<_, Option<u8>>(24)?;
                    let attribute = match attribute_index {
                        Some(attribute_index) => Some(SoulAttribute {
                            snapshot_id: row.get(22)?,
                            soul_internal_id: row.get(23)?,
                            attribute_index,
                            attribute_type: row.get(25)?,
                            value: row.get(26)?,
                            enhancement_count: row.get(27)?,
                            count_provenance: row.get(28)?,
                            fixed_attribute: row.get::<_, i64>(29)? != 0,
                        }),
                        None => None,
                    };
                    Ok((item, soul, attribute))
                })
                .map_err(|error| AppError::database("query inventory facts", &error))?;

            for row in rows {
                let (item, soul, attribute) = row
                    .map_err(|error| AppError::database("read inventory fact row", &error))?;
                inventory_indexes.entry(item.soul_key.clone()).or_insert_with(|| {
                    let index = inventory.len();
                    inventory.push(item);
                    index
                });
                let soul_key = (soul.snapshot_id.clone(), soul.internal_id.clone());
                soul_indexes.entry(soul_key).or_insert_with(|| {
                    let index = souls.len();
                    souls.push(soul);
                    index
                });
                if let Some(attribute) = attribute {
                    attributes.push(attribute);
                }
            }

            if inventory.len() as u32 != expected_present_count {
                return Err(AppError::internal(format!(
                    "当前库存缺少快照事实：期望 {} 枚，实际读取 {} 枚",
                    expected_present_count,
                    inventory.len()
                )));
            }
            Ok((inventory, souls, attributes, excluded_unconfirmed_count))
        })
    }
}

// ─── 御魂雷达缓存仓库 ─────────────────────────────────────────────────────────

/// 雷达仓库只保存聚合后的点位；计算入口仍从快照事实读取，页面入口不再加载御魂正文。
pub struct SqliteSoulRadarRepository {
    db: Arc<Database>,
}

impl SqliteSoulRadarRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl SoulRadarRepository for SqliteSoulRadarRepository {
    fn replace(
        &self,
        profile_id: &str,
        calculated_at: &str,
        inventory_revision: &str,
        inventory_count: u32,
        points: &[SoulRadarPoint],
    ) -> Result<(), AppError> {
        let profile_id = profile_id.to_owned();
        let calculated_at = calculated_at.to_owned();
        let inventory_revision = inventory_revision.to_owned();
        let points = points.to_vec();

        self.db.write("replace_soul_radar", |conn| {
            // 先删除父缓存，依靠外键级联清理旧点位，再在同一事务中写入新结果。
            conn.execute(
                "DELETE FROM soul_radar_cache WHERE profile_id = ?1",
                params![profile_id.as_str()],
            )
            .map_err(|error| AppError::database("delete soul radar cache", &error))?;
            conn.execute(
                "INSERT INTO soul_radar_cache (profile_id, calculated_at, inventory_revision, inventory_count) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    profile_id.as_str(),
                    calculated_at.as_str(),
                    inventory_revision.as_str(),
                    inventory_count,
                ],
            )
            .map_err(|error| AppError::constraint_violation("soul_radar_cache", &error))?;

            for point in &points {
                // 点位已经由应用层按“套装 + 号位 + 指标”去重，数据库复合主键负责最后一道约束。
                conn.execute(
                    "INSERT INTO soul_radar_point \
                     (profile_id, set_id, slot, metric_type, value, main_attr_type) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        profile_id.as_str(),
                        point.set_id,
                        point.slot,
                        point.metric_type,
                        point.value,
                        point.main_attr_type,
                    ],
                )
                .map_err(|error| AppError::constraint_violation("soul_radar_point", &error))?;
            }
            Ok(())
        })
    }

    fn get(&self, profile_id: &str) -> Result<Option<SoulRadarCache>, AppError> {
        let profile_id = profile_id.to_owned();
        self.db.read("get_soul_radar_cache", |conn| {
            let metadata = conn
                .query_row(
                    "SELECT calculated_at, inventory_revision, inventory_count \
                     FROM soul_radar_cache WHERE profile_id = ?1",
                    params![profile_id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, u32>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| AppError::database("read soul radar cache", &error))?;
            let Some((calculated_at, inventory_revision, inventory_count)) = metadata else {
                return Ok(None);
            };

            let mut statement = conn
                .prepare(
                    "SELECT set_id, slot, metric_type, value, main_attr_type \
                     FROM soul_radar_point \
                     WHERE profile_id = ?1 \
                     ORDER BY set_id, slot, metric_type",
                )
                .map_err(|error| AppError::database("prepare soul radar points", &error))?;
            let rows = statement
                .query_map(params![profile_id.as_str()], |row| {
                    Ok(SoulRadarPoint {
                        set_id: row.get(0)?,
                        slot: row.get(1)?,
                        metric_type: row.get(2)?,
                        value: row.get(3)?,
                        main_attr_type: row.get(4)?,
                    })
                })
                .map_err(|error| AppError::database("query soul radar points", &error))?;
            let points = rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read soul radar point", &error))?;
            let (current_inventory_count, current_inventory_revision) =
                read_current_inventory_state(conn, &profile_id)?;

            Ok(Some(SoulRadarCache {
                profile_id,
                calculated_at,
                inventory_revision,
                inventory_count,
                current_inventory_revision,
                current_inventory_count,
                points,
            }))
        })
    }

    fn current_inventory_state(&self, profile_id: &str) -> Result<(u32, String), AppError> {
        let profile_id = profile_id.to_owned();
        self.db.read("current_soul_radar_inventory_state", |conn| {
            read_current_inventory_state(conn, &profile_id)
        })
    }
}

// ─── 头尾分析缓存仓库 ─────────────────────────────────────────────────────────

/// 头尾仓库保存全部候选卡片及其副属性 JSON；计算入口仍从快照事实读取，页面入口不加载库存正文。
pub struct SqliteHeadTailRepository {
    db: Arc<Database>,
}

impl SqliteHeadTailRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl HeadTailRepository for SqliteHeadTailRepository {
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
    ) -> Result<(), AppError> {
        let profile_id = profile_id.to_owned();
        let calculated_at = calculated_at.to_owned();
        let inventory_revision = inventory_revision.to_owned();
        let heads = heads.to_owned();
        let tails = tails.to_owned();

        self.db.write("replace_head_tail", |conn| {
            // 父表与子表在同一事务内替换，确保重算失败时旧结果不会被半删半写。
            conn.execute(
                "DELETE FROM head_tail_cache WHERE profile_id = ?1",
                params![profile_id.as_str()],
            )
            .map_err(|error| AppError::database("delete head tail cache", &error))?;
            conn.execute(
                "INSERT INTO head_tail_cache \
                 (profile_id, calculated_at, inventory_revision, inventory_count, \
                  head_candidate_count, tail_candidate_count) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    profile_id.as_str(),
                    calculated_at.as_str(),
                    inventory_revision.as_str(),
                    inventory_count,
                    head_candidate_count,
                    tail_candidate_count,
                ],
            )
            .map_err(|error| AppError::constraint_violation("head_tail_cache", &error))?;

            // 候选序号与后端排序保持一致，旧缓存迁移后也能按同一顺序读取。
            for (position, cards) in [("head", heads.as_slice()), ("tail", tails.as_slice())] {
                for (candidate_rank, card) in cards.iter().enumerate() {
                let attributes_json = serde_json::to_string(&card.attributes)
                    .map_err(|error| AppError::database("encode head tail attributes", &error))?;
                conn.execute(
                    "INSERT INTO head_tail_result \
                     (profile_id, position, candidate_rank, soul_key, set_id, slot, quality, level, \
                      main_attr_type, main_attr_value, speed, speed_rolls, attributes_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    params![
                        profile_id.as_str(),
                        position,
                        candidate_rank as i64,
                        card.soul_key,
                        card.set_id,
                        card.slot,
                        card.quality,
                        card.level,
                        card.main_attr_type,
                        card.main_attr_value,
                        card.speed,
                        card.speed_rolls,
                        attributes_json,
                    ],
                )
                .map_err(|error| AppError::constraint_violation("head_tail_result", &error))?;
                }
            }
            Ok(())
        })
    }

    fn get(&self, profile_id: &str) -> Result<Option<HeadTailCache>, AppError> {
        let profile_id = profile_id.to_owned();
        self.db.read("get_head_tail_cache", |conn| {
            let metadata = conn
                .query_row(
                    "SELECT calculated_at, inventory_revision, inventory_count, \
                            head_candidate_count, tail_candidate_count \
                     FROM head_tail_cache WHERE profile_id = ?1",
                    params![profile_id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, u32>(2)?,
                            row.get::<_, u32>(3)?,
                            row.get::<_, u32>(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| AppError::database("read head tail cache", &error))?;
            let Some((
                calculated_at,
                inventory_revision,
                inventory_count,
                head_candidate_count,
                tail_candidate_count,
            )) = metadata
            else {
                return Ok(None);
            };

            let mut statement = conn
                .prepare(
                    "SELECT position, candidate_rank, soul_key, set_id, slot, quality, level, \
                            main_attr_type, main_attr_value, speed, speed_rolls, attributes_json \
                     FROM head_tail_result \
                     WHERE profile_id = ?1 ORDER BY position, candidate_rank",
                )
                .map_err(|error| AppError::database("prepare head tail results", &error))?;
            let rows = statement
                .query_map(params![profile_id.as_str()], |row| {
                    let attributes_json: String = row.get(11)?;
                    let attributes = serde_json::from_str::<Vec<HeadTailAttribute>>(
                        &attributes_json,
                    )
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            11,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        HeadTailCard {
                            soul_key: row.get(2)?,
                            set_id: row.get(3)?,
                            slot: row.get(4)?,
                            quality: row.get(5)?,
                            level: row.get(6)?,
                            main_attr_type: row.get(7)?,
                            main_attr_value: row.get(8)?,
                            speed: row.get(9)?,
                            speed_rolls: row.get(10)?,
                            attributes,
                        },
                    ))
                })
                .map_err(|error| AppError::database("query head tail results", &error))?;

            let mut heads = Vec::new();
            let mut tails = Vec::new();
            for row in rows {
                let (position, _candidate_rank, card) =
                    row.map_err(|error| AppError::database("read head tail result", &error))?;
                match position.as_str() {
                    "head" => heads.push(card),
                    "tail" => tails.push(card),
                    _ => {
                        return Err(AppError::database(
                            "read head tail result",
                            &std::io::Error::new(std::io::ErrorKind::InvalidData, "未知头尾位置"),
                        ));
                    }
                }
            }

            let (current_inventory_count, current_inventory_revision) =
                read_current_inventory_state(conn, &profile_id)?;
            Ok(Some(HeadTailCache {
                profile_id,
                calculated_at,
                inventory_revision,
                inventory_count,
                current_inventory_revision,
                current_inventory_count,
                heads,
                tails,
                head_candidate_count,
                tail_candidate_count,
            }))
        })
    }

    fn current_inventory_state(&self, profile_id: &str) -> Result<(u32, String), AppError> {
        let profile_id = profile_id.to_owned();
        self.db.read("current_head_tail_inventory_state", |conn| {
            read_current_inventory_state(conn, &profile_id)
        })
    }
}

/// 读取雷达计算所对应的当前库存版本；只返回数量和最新更新时间，不搬运御魂事实。
fn read_current_inventory_state(
    conn: &Connection,
    profile_id: &str,
) -> Result<(u32, String), AppError> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(updated_at), '') \
         FROM inventory_item \
         WHERE profile_id = ?1 AND presence_state = 'present'",
        params![profile_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map_err(|error| AppError::database("read current inventory state", &error))
}

impl ImportRepository for SqliteSnapshotRepository {
    fn commit_import(&self, request: &ImportCommitRequest) -> Result<ImportCommitResult, AppError> {
        let request = request.clone();
        self.db.write("commit_import", |conn| {
            if request.force_snapshot {
                // 当前数据覆盖模式先清理旧快照、库存和分析派生状态，事务失败时由 SQLite 回滚，避免旧结论残留。
                conn.execute(
                    "DELETE FROM user_decision_history WHERE profile_id = ?1",
                    params![request.snapshot.profile_id],
                )
                .map_err(|error| AppError::database("clear decision history", &error))?;
                conn.execute(
                    "DELETE FROM user_decision WHERE profile_id = ?1",
                    params![request.snapshot.profile_id],
                )
                .map_err(|error| AppError::database("clear decisions", &error))?;
                conn.execute(
                    "DELETE FROM action_batch WHERE profile_id = ?1",
                    params![request.snapshot.profile_id],
                )
                .map_err(|error| AppError::database("clear action batches", &error))?;
                conn.execute(
                    "DELETE FROM analysis_todo WHERE profile_id = ?1",
                    params![request.snapshot.profile_id],
                )
                .map_err(|error| AppError::database("clear analysis todos", &error))?;
                conn.execute(
                    "DELETE FROM acquisition_event WHERE profile_id = ?1",
                    params![request.snapshot.profile_id],
                )
                .map_err(|error| AppError::database("clear acquisition events", &error))?;
                conn.execute(
                    "DELETE FROM inventory_item WHERE profile_id = ?1",
                    params![request.snapshot.profile_id],
                )
                .map_err(|error| AppError::database("clear inventory", &error))?;
                conn.execute(
                    "DELETE FROM snapshot WHERE profile_id = ?1",
                    params![request.snapshot.profile_id],
                )
                .map_err(|error| AppError::database("clear snapshots", &error))?;
            }

            // 内容和范围指纹必须同时命中；完整性也属于语义的一部分，避免把局部数据误当完整基线。
            if !request.force_snapshot {
                let duplicate: Option<String> = conn
                    .query_row(
                        "SELECT id FROM snapshot WHERE profile_id = ?1 \
                         AND content_fingerprint = ?2 AND scope_fingerprint = ?3 \
                         AND completeness = ?4 ORDER BY created_at DESC LIMIT 1",
                        params![
                            request.snapshot.profile_id,
                            request.snapshot.content_fingerprint,
                            request.snapshot.scope_fingerprint,
                            request.snapshot.completeness,
                        ],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| AppError::database("find duplicate snapshot", &error))?;

                if let Some(snapshot_id) = duplicate.clone() {
                    // 重复导入仍保留轻量采集事件，但不复制快照和库存行。
                    let mut event = request.event.clone();
                    event.result_kind = "unchanged".to_owned();
                    event.snapshot_id = Some(snapshot_id.clone());
                    insert_event_row(conn, &event)?;
                    return Ok(ImportCommitResult {
                        result_kind: "unchanged".to_owned(),
                        duplicate_of: Some(snapshot_id),
                        ..ImportCommitResult::default()
                    });
                }
            }

            // 当前最新快照作为父节点，形成可追溯的导入链；查询与后续写入在同一事务中完成。
            let parent_snapshot_id: Option<String> = conn
                .query_row(
                    "SELECT id FROM snapshot WHERE profile_id = ?1 ORDER BY created_at DESC LIMIT 1",
                    params![request.snapshot.profile_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| AppError::database("find parent snapshot", &error))?;
            let mut snapshot = request.snapshot.clone();
            snapshot.parent_snapshot_id = parent_snapshot_id;
            snapshot.forced = request.force_snapshot;

            conn.execute(
                "INSERT INTO snapshot (id, profile_id, raw_sha256, content_fingerprint, \
                 scope_fingerprint, source_kind, completeness, scope_json, captured_at, game_version, \
                 adapter_version, parser_version, catalog_version, parent_snapshot_id, forced, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    snapshot.id, snapshot.profile_id, snapshot.raw_sha256,
                    snapshot.content_fingerprint, snapshot.scope_fingerprint, snapshot.source_kind,
                    snapshot.completeness, snapshot.scope_json, snapshot.captured_at,
                    snapshot.game_version, snapshot.adapter_version, snapshot.parser_version,
                    snapshot.catalog_version, snapshot.parent_snapshot_id, snapshot.forced,
                    snapshot.created_at,
                ],
            )
            .map_err(|error| AppError::constraint_violation("snapshot", &error))?;

            for soul in &request.souls {
                conn.execute(
                    "INSERT INTO snapshot_soul (snapshot_id, internal_id, source_stable_id, \
                     identity_quality, set_id, slot, quality, level, main_attr_type, main_attr_value, \
                     initial_substat_count, locked_in_source, equipped_state, source_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    params![
                        soul.snapshot_id, soul.internal_id, soul.source_stable_id,
                        soul.identity_quality, soul.set_id, soul.slot, soul.quality, soul.level,
                        soul.main_attr_type, soul.main_attr_value, soul.initial_substat_count,
                        soul.locked_in_source, soul.equipped_state, soul.source_json,
                    ],
                )
                .map_err(|error| AppError::constraint_violation("snapshot_soul", &error))?;
            }
            for attribute in &request.attributes {
                conn.execute(
                    "INSERT INTO soul_attribute (snapshot_id, soul_internal_id, attribute_index, \
                     attribute_type, value, enhancement_count, count_provenance, fixed_attribute) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        attribute.snapshot_id, attribute.soul_internal_id, attribute.attribute_index,
                        attribute.attribute_type, attribute.value, attribute.enhancement_count,
                        attribute.count_provenance, attribute.fixed_attribute,
                    ],
                )
                .map_err(|error| AppError::constraint_violation("soul_attribute", &error))?;
            }

            // 完整快照才有资格确认“未出现即离开”；局部快照完全不触碰未出现行。
            let mut removed_count = 0u32;
            if snapshot.completeness == "complete" {
                removed_count = conn
                    .execute(
                        "UPDATE inventory_item SET presence_state = 'removed', updated_at = ?1 \
                         WHERE profile_id = ?2 AND presence_state <> 'removed'",
                        params![snapshot.created_at, snapshot.profile_id],
                    )
                    .map_err(|error| AppError::database("mark missing inventory", &error))?
                    as u32;
            }

            let mut added_count = 0u32;
            let mut updated_count = 0u32;
            let mut skipped_unstable_count = 0u32;
            for soul in &request.souls {
                // 局部快照只有稳定来源 ID 能跨快照合并；无稳定 ID 的记录仍保留在独立快照中。
                let soul_key = if let Some(stable_id) = soul.source_stable_id.as_deref()
                    && soul.identity_quality == "stable"
                {
                    stable_id.to_owned()
                } else if snapshot.completeness == "complete" {
                    format!("snapshot:{}:{}", snapshot.id, soul.internal_id)
                } else {
                    skipped_unstable_count += 1;
                    continue;
                };

                let existing: Option<(String, String)> = conn
                    .query_row(
                        "SELECT first_seen_snapshot_id, presence_state FROM inventory_item \
                         WHERE profile_id = ?1 AND soul_key = ?2",
                        params![snapshot.profile_id, soul_key],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(|error| AppError::database("find inventory item", &error))?;
                let first_seen = match existing {
                    Some((first_seen, _)) => {
                        updated_count += 1;
                        first_seen
                    }
                    None => {
                        added_count += 1;
                        snapshot.id.clone()
                    }
                };

                conn.execute(
                    "INSERT INTO inventory_item (profile_id, soul_key, snapshot_id, soul_internal_id, \
                     first_seen_snapshot_id, last_seen_snapshot_id, presence_state, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?3, 'present', ?6) \
                     ON CONFLICT(profile_id, soul_key) DO UPDATE SET \
                       snapshot_id = excluded.snapshot_id, soul_internal_id = excluded.soul_internal_id, \
                       first_seen_snapshot_id = excluded.first_seen_snapshot_id, \
                       last_seen_snapshot_id = excluded.last_seen_snapshot_id, \
                       presence_state = excluded.presence_state, updated_at = excluded.updated_at",
                    params![
                        snapshot.profile_id, soul_key, snapshot.id, soul.internal_id, first_seen,
                        snapshot.created_at,
                    ],
                )
                .map_err(|error| AppError::constraint_violation("inventory_item", &error))?;

            }

            let mut event = request.event.clone();
            event.result_kind = if request.force_snapshot {
                "forced".to_owned()
            } else {
                "new_snapshot".to_owned()
            };
            event.snapshot_id = Some(snapshot.id.clone());
            insert_event_row(conn, &event)?;

            Ok(ImportCommitResult {
                snapshot_id: Some(snapshot.id),
                result_kind: event.result_kind,
                duplicate_of: None,
                added_count,
                updated_count,
                unchanged_count: 0,
                removed_count,
                skipped_unstable_count,
            })
        })
    }

    fn list_snapshot_souls(
        &self,
        snapshot_id: &str,
    ) -> Result<(Vec<SnapshotSoul>, Vec<SoulAttribute>), AppError> {
        let sid = snapshot_id.to_owned();
        self.db.read("list_snapshot_souls", |conn| {
            let mut soul_statement = conn
                .prepare(
                    "SELECT snapshot_id, internal_id, source_stable_id, identity_quality, set_id, \
                     slot, quality, level, main_attr_type, main_attr_value, initial_substat_count, \
                     locked_in_source, equipped_state, source_json FROM snapshot_soul \
                     WHERE snapshot_id = ?1 ORDER BY internal_id",
                )
                .map_err(|error| AppError::database("prepare list_snapshot_souls", &error))?;
            let souls = soul_statement
                .query_map(params![sid], |row| {
                    Ok(SnapshotSoul {
                        snapshot_id: row.get(0)?,
                        internal_id: row.get(1)?,
                        source_stable_id: row.get(2)?,
                        identity_quality: row.get(3)?,
                        set_id: row.get(4)?,
                        slot: row.get(5)?,
                        quality: row.get(6)?,
                        level: row.get(7)?,
                        main_attr_type: row.get(8)?,
                        main_attr_value: row.get(9)?,
                        initial_substat_count: row.get(10)?,
                        locked_in_source: row.get::<_, Option<i64>>(11)?.map(|value| value != 0),
                        equipped_state: row.get(12)?,
                        source_json: row.get(13)?,
                    })
                })
                .map_err(|error| AppError::database("query list_snapshot_souls", &error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read snapshot soul", &error))?;

            let mut attribute_statement = conn
                .prepare(
                    "SELECT snapshot_id, soul_internal_id, attribute_index, attribute_type, value, \
                     enhancement_count, count_provenance, fixed_attribute FROM soul_attribute \
                     WHERE snapshot_id = ?1 ORDER BY soul_internal_id, attribute_index",
                )
                .map_err(|error| AppError::database("prepare list_soul_attributes", &error))?;
            let attributes = attribute_statement
                .query_map(params![sid], |row| {
                    Ok(SoulAttribute {
                        snapshot_id: row.get(0)?,
                        soul_internal_id: row.get(1)?,
                        attribute_index: row.get(2)?,
                        attribute_type: row.get(3)?,
                        value: row.get(4)?,
                        enhancement_count: row.get(5)?,
                        count_provenance: row.get(6)?,
                        fixed_attribute: row.get::<_, i64>(7)? != 0,
                    })
                })
                .map_err(|error| AppError::database("query list_soul_attributes", &error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read soul attribute", &error))?;

            Ok((souls, attributes))
        })
    }

    fn list_snapshot_souls_by_ids(
        &self,
        snapshot_id: &str,
        internal_ids: &[String],
    ) -> Result<(Vec<SnapshotSoul>, Vec<SoulAttribute>), AppError> {
        if internal_ids.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        let sid = snapshot_id.to_owned();
        let ids = internal_ids.to_vec();
        self.db.read("list_snapshot_souls_by_ids", |conn| {
            // 分页最多带入一页的内部 ID，使用绑定参数避免拼接外部数据。
            let placeholders = (2..=ids.len() + 1)
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let mut values = vec![Value::Text(sid.clone())];
            values.extend(ids.iter().cloned().map(Value::Text));

            let soul_sql = format!(
                "SELECT snapshot_id, internal_id, source_stable_id, identity_quality, set_id, \
                 slot, quality, level, main_attr_type, main_attr_value, initial_substat_count, \
                 locked_in_source, equipped_state, source_json FROM snapshot_soul \
                 WHERE snapshot_id = ?1 AND internal_id IN ({placeholders}) ORDER BY internal_id"
            );
            let mut soul_statement = conn.prepare(&soul_sql).map_err(|error| {
                AppError::database("prepare list selected snapshot souls", &error)
            })?;
            let souls = soul_statement
                .query_map(params_from_iter(values.clone()), |row| {
                    Ok(SnapshotSoul {
                        snapshot_id: row.get(0)?,
                        internal_id: row.get(1)?,
                        source_stable_id: row.get(2)?,
                        identity_quality: row.get(3)?,
                        set_id: row.get(4)?,
                        slot: row.get(5)?,
                        quality: row.get(6)?,
                        level: row.get(7)?,
                        main_attr_type: row.get(8)?,
                        main_attr_value: row.get(9)?,
                        initial_substat_count: row.get(10)?,
                        locked_in_source: row.get::<_, Option<i64>>(11)?.map(|value| value != 0),
                        equipped_state: row.get(12)?,
                        source_json: row.get(13)?,
                    })
                })
                .map_err(|error| AppError::database("query selected snapshot souls", &error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read selected snapshot soul", &error))?;

            let attribute_sql = format!(
                "SELECT snapshot_id, soul_internal_id, attribute_index, attribute_type, value, \
                 enhancement_count, count_provenance, fixed_attribute FROM soul_attribute \
                 WHERE snapshot_id = ?1 AND soul_internal_id IN ({placeholders}) \
                 ORDER BY soul_internal_id, attribute_index"
            );
            let mut attribute_statement = conn
                .prepare(&attribute_sql)
                .map_err(|error| AppError::database("prepare selected soul attributes", &error))?;
            let attributes = attribute_statement
                .query_map(params_from_iter(values), |row| {
                    Ok(SoulAttribute {
                        snapshot_id: row.get(0)?,
                        soul_internal_id: row.get(1)?,
                        attribute_index: row.get(2)?,
                        attribute_type: row.get(3)?,
                        value: row.get(4)?,
                        enhancement_count: row.get(5)?,
                        count_provenance: row.get(6)?,
                        fixed_attribute: row.get::<_, i64>(7)? != 0,
                    })
                })
                .map_err(|error| AppError::database("query selected soul attributes", &error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read selected soul attribute", &error))?;

            Ok((souls, attributes))
        })
    }
}

/// 在导入事务内写入事件，避免调用独立仓库再次开启嵌套事务。
fn insert_event_row(
    conn: &mut rusqlite::Connection,
    event: &AcquisitionEvent,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO acquisition_event (id, profile_id, source_kind, source_format, raw_sha256, \
         captured_at, received_at, game_version, adapter_version, parser_version, completeness, \
         scope_json, result_kind, snapshot_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            event.id, event.profile_id, event.source_kind, event.source_format, event.raw_sha256,
            event.captured_at, event.received_at, event.game_version, event.adapter_version,
            event.parser_version, event.completeness, event.scope_json, event.result_kind,
            event.snapshot_id,
        ],
    )
    .map_err(|error| AppError::constraint_violation("acquisition_event", &error))?;
    Ok(())
}

// ─── 目录仓库 ─────────────────────────────────────────────────────────────────

pub struct SqliteCatalogRepository {
    db: Arc<Database>,
}

impl SqliteCatalogRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl CatalogRepository for SqliteCatalogRepository {
    fn install(
        &self,
        package: &CatalogPackage,
        groups: &[YuhunGroup],
        sets: &[SoulSet],
        shikigami: &[Shikigami],
        group_members: &[GroupMember],
        commonness: &[SetCommonness],
    ) -> Result<(), AppError> {
        let pkg = package.clone();
        let groups = groups.to_vec();
        let sets = sets.to_vec();
        let shikigami = shikigami.to_vec();
        let members = group_members.to_vec();
        let common = commonness.to_vec();

        self.db.write("install_catalog", |conn| {
            // 先停用旧版本
            conn.execute("UPDATE catalog_package SET active = 0 WHERE active = 1", [])
                .map_err(|e| AppError::database("deactivate old catalog", &e))?;

            // 插入或更新包记录
            conn.execute(
                "INSERT OR REPLACE INTO catalog_package (version, installed_at, source, sha256, \
                 parser_version, validation_status, validation_errors_json, icon_coverage, \
                 mechanics_coverage, shikigami_count, panel_coverage, skill_coverage, \
                 signature_text, compatible_game_version, min_app_version, active) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 1)",
                params![
                    pkg.version,
                    pkg.installed_at,
                    pkg.source,
                    pkg.sha256,
                    pkg.parser_version,
                    pkg.validation_status,
                    pkg.validation_errors_json,
                    pkg.icon_coverage,
                    pkg.mechanics_coverage,
                    pkg.shikigami_count,
                    pkg.panel_coverage,
                    pkg.skill_coverage,
                    pkg.signature_text,
                    pkg.compatible_game_version,
                    pkg.min_app_version,
                ],
            )
            .map_err(|e| AppError::constraint_violation("catalog_package", &e))?;

            // 写入分组定义（INSERT OR IGNORE 避免重复）
            for g in &groups {
                conn.execute(
                    "INSERT OR IGNORE INTO yuhun_group (id, origin, name, description, parent_id, revision) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![g.id, g.origin, g.name, g.description, g.parent_id, g.revision],
                )
                .map_err(|e| AppError::constraint_violation("yuhun_group", &e))?;
            }

            // 写入套装定义
            for set in &sets {
                conn.execute(
                    "INSERT OR REPLACE INTO soul_set (catalog_version, set_id, name, icon_asset_id, category, special_category, rarity_scope, \
                     two_piece_effect_json, four_piece_effect_json, source_refs_json, \
                     enhancement_rule_json, effective_from, effective_to) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    params![
                        set.catalog_version, set.set_id, set.name, set.icon_asset_id, set.category,
                        set.special_category,
                        set.rarity_scope,
                        set.two_piece_effect_json, set.four_piece_effect_json,
                        set.source_refs_json, set.enhancement_rule_json,
                        set.effective_from, set.effective_to,
                    ],
                )
                .map_err(|e| AppError::constraint_violation("soul_set", &e))?;
            }

            // 写入式神定义；面板、技能、场景和来源作为版本快照，不覆盖旧目录版本。
            for entry in &shikigami {
                conn.execute(
                    "INSERT OR REPLACE INTO shikigami (catalog_version, shikigami_id, name, rarity, icon_asset_id, sort_order, panel_level, awakened, panel_json, skills_json, skill_investment_json, employment_scenes_json, game_version, source_refs_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    params![
                        entry.catalog_version,
                        entry.shikigami_id,
                        entry.name,
                        entry.rarity,
                        entry.icon_asset_id,
                        entry.sort_order,
                        entry.panel_level,
                        entry.awakened,
                        entry.panel_json,
                        entry.skills_json,
                        entry.skill_investment_json,
                        entry.employment_scenes_json,
                        entry.game_version,
                        entry.source_refs_json,
                    ],
                )
                .map_err(|e| AppError::constraint_violation("shikigami", &e))?;
            }

            // 分组关联（先清空旧版本的分组关系，再插入）
            conn.execute(
                "DELETE FROM yuhun_group_member WHERE catalog_version = ?1",
                params![pkg.version],
            )
            .map_err(|e| AppError::database("clear group members", &e))?;

            for m in &members {
                conn.execute(
                    "INSERT OR REPLACE INTO yuhun_group_member (group_id, set_id, catalog_version, origin, user_override) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![m.group_id, m.set_id, m.catalog_version, m.origin, m.user_override],
                )
                .map_err(|e| AppError::constraint_violation("yuhun_group_member", &e))?;
            }

            // 常用度默认值
            conn.execute(
                "DELETE FROM set_commonness_default WHERE catalog_version = ?1",
                params![pkg.version],
            )
            .map_err(|e| AppError::database("clear commonness", &e))?;

            for c in &common {
                conn.execute(
                    "INSERT OR REPLACE INTO set_commonness_default (catalog_version, set_id, scenario, value) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![c.catalog_version, c.set_id, c.scenario, c.value.as_str()],
                )
                .map_err(|e| AppError::constraint_violation("set_commonness_default", &e))?;
            }

            // 更新 app_meta 中的活跃目录版本
            conn.execute(
                "INSERT OR REPLACE INTO app_meta (key, value, updated_at) VALUES ('active_catalog_version', ?1, ?2)",
                params![pkg.version, pkg.installed_at],
            )
            .map_err(|e| AppError::database("update app_meta", &e))?;

            Ok(())
        })
    }

    /// 只切换目录活动版本；目标版本必须已完整写入目录包和对应事实表。
    fn activate_version(&self, catalog_version: &str) -> Result<(), AppError> {
        let version = catalog_version.to_owned();
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        self.db.write("activate_catalog_version", |conn| {
            let exists: i64 = conn
                .query_row(
                    "SELECT count(*) FROM catalog_package WHERE version = ?1",
                    params![version],
                    |row| row.get(0),
                )
                .map_err(|e| AppError::database("check catalog version", &e))?;
            if exists == 0 {
                return Err(AppError::not_found("catalog_version", &version));
            }
            conn.execute("UPDATE catalog_package SET active = 0 WHERE active = 1", [])
                .map_err(|e| AppError::database("deactivate catalog version", &e))?;
            conn.execute(
                "UPDATE catalog_package SET active = 1 WHERE version = ?1",
                params![version],
            )
            .map_err(|e| AppError::database("activate catalog version", &e))?;
            conn.execute(
                "INSERT OR REPLACE INTO app_meta (key, value, updated_at) VALUES ('active_catalog_version', ?1, ?2)",
                params![version, now],
            )
            .map_err(|e| AppError::database("update active catalog pointer", &e))?;
            Ok(())
        })
    }

    fn active_status(&self) -> Result<Option<CatalogStatus>, AppError> {
        self.db.read("active_catalog_status", |conn| {
            let version: Option<String> = conn
                .query_row(
                    "SELECT value FROM app_meta WHERE key = 'active_catalog_version'",
                    [],
                    |row| row.get(0),
                )
                .ok();

            match version {
                Some(v) => {
                    let set_count: i64 = conn
                        .query_row(
                            "SELECT count(*) FROM soul_set WHERE catalog_version = ?1",
                            params![v],
                            |row| row.get(0),
                        )
                        .unwrap_or(0);
                    let group_count: i64 = conn
                        .query_row("SELECT count(*) FROM yuhun_group", [], |row| row.get(0))
                        .unwrap_or(0);
                    let package_meta = conn
                        .query_row(
                            "SELECT source, sha256, parser_version, validation_status, \
                             validation_errors_json, icon_coverage, mechanics_coverage, \
                             shikigami_count, panel_coverage, skill_coverage \
                             FROM catalog_package WHERE version = ?1",
                            params![v],
                            |row| {
                                Ok((
                                    row.get::<_, Option<String>>(0)?,
                                    row.get::<_, Option<String>>(1)?,
                                    row.get::<_, Option<String>>(2)?,
                                    row.get::<_, String>(3)?,
                                    row.get::<_, Option<String>>(4)?,
                                    row.get::<_, i64>(5)?,
                                    row.get::<_, i64>(6)?,
                                    row.get::<_, i64>(7)?,
                                    row.get::<_, i64>(8)?,
                                    row.get::<_, i64>(9)?,
                                ))
                            },
                        )
                        .optional()
                        .map_err(|e| AppError::database("read catalog package metadata", &e))?;
                    let (
                        source,
                        sha256,
                        parser_version,
                        validation_status,
                        validation_errors_json,
                        icon_coverage,
                        mechanics_coverage,
                        shikigami_count,
                        panel_coverage,
                        skill_coverage,
                    ) = package_meta.unwrap_or((
                        None,
                        None,
                        None,
                        "unknown".to_owned(),
                        None,
                        0,
                        0,
                        0,
                        0,
                        0,
                    ));
                    let validation_errors = validation_errors_json
                        .and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok())
                        .unwrap_or_default();
                    Ok(Some(CatalogStatus {
                        version: v,
                        set_count: set_count as u64,
                        group_count: group_count as u64,
                        source,
                        sha256,
                        parser_version,
                        validation_status,
                        validation_errors,
                        icon_coverage: icon_coverage.max(0) as u64,
                        mechanics_coverage: mechanics_coverage.max(0) as u64,
                        shikigami_count: shikigami_count.max(0) as u64,
                        panel_coverage: panel_coverage.max(0) as u64,
                        skill_coverage: skill_coverage.max(0) as u64,
                    }))
                }
                None => Ok(None),
            }
        })
    }

    fn list_sets(&self, catalog_version: &str) -> Result<Vec<SoulSet>, AppError> {
        let cv = catalog_version.to_string();
        self.db.read("list_sets", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT catalog_version, set_id, name, icon_asset_id, category, special_category, rarity_scope, two_piece_effect_json, \
                     four_piece_effect_json, source_refs_json, enhancement_rule_json, effective_from, effective_to \
                     FROM soul_set WHERE catalog_version = ?1 ORDER BY set_id",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let rows = stmt
                .query_map(params![cv], |row| {
                    Ok(SoulSet {
                        catalog_version: row.get(0)?,
                        set_id: row.get(1)?,
                        name: row.get(2)?,
                        icon_asset_id: row.get(3)?,
                        category: row.get(4)?,
                        special_category: row.get(5)?,
                        rarity_scope: row.get(6)?,
                        two_piece_effect_json: row.get(7)?,
                        four_piece_effect_json: row.get(8)?,
                        source_refs_json: row.get(9)?,
                        enhancement_rule_json: row.get(10)?,
                        effective_from: row.get(11)?,
                        effective_to: row.get(12)?,
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            let mut sets = Vec::new();
            for row in rows {
                sets.push(row.map_err(|e| AppError::database("read row", &e))?);
            }
            Ok(sets)
        })
    }

    fn list_shikigami(&self, catalog_version: &str) -> Result<Vec<Shikigami>, AppError> {
        let cv = catalog_version.to_owned();
        self.db.read("list_shikigami", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT catalog_version, shikigami_id, name, rarity, icon_asset_id, sort_order, panel_level, awakened, panel_json, skills_json, skill_investment_json, employment_scenes_json, game_version, source_refs_json \
                     FROM shikigami WHERE catalog_version = ?1 ORDER BY sort_order, shikigami_id",
                )
                .map_err(|e| AppError::database("prepare list shikigami", &e))?;
            let rows = stmt
                .query_map(params![cv], |row| {
                    Ok(Shikigami {
                        catalog_version: row.get(0)?,
                        shikigami_id: row.get(1)?,
                        name: row.get(2)?,
                        rarity: row.get(3)?,
                        icon_asset_id: row.get(4)?,
                        sort_order: row.get(5)?,
                        panel_level: row.get(6)?,
                        awakened: row.get(7)?,
                        panel_json: row.get(8)?,
                        skills_json: row.get(9)?,
                        skill_investment_json: row.get(10)?,
                        employment_scenes_json: row.get(11)?,
                        game_version: row.get(12)?,
                        source_refs_json: row.get(13)?,
                    })
                })
                .map_err(|e| AppError::database("query list shikigami", &e))?;
            let mut result = Vec::new();
            for row in rows {
                result.push(row.map_err(|e| AppError::database("read shikigami row", &e))?);
            }
            Ok(result)
        })
    }

    fn get_set(&self, catalog_version: &str, set_id: &str) -> Result<Option<SoulSet>, AppError> {
        let cv = catalog_version.to_string();
        let sid = set_id.to_string();
        self.db.read("get_set", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT catalog_version, set_id, name, icon_asset_id, category, special_category, rarity_scope, two_piece_effect_json, \
                     four_piece_effect_json, source_refs_json, enhancement_rule_json, effective_from, effective_to \
                     FROM soul_set WHERE catalog_version = ?1 AND set_id = ?2",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let mut rows = stmt
                .query_map(params![cv, sid], |row| {
                    Ok(SoulSet {
                        catalog_version: row.get(0)?,
                        set_id: row.get(1)?,
                        name: row.get(2)?,
                        icon_asset_id: row.get(3)?,
                        category: row.get(4)?,
                        special_category: row.get(5)?,
                        rarity_scope: row.get(6)?,
                        two_piece_effect_json: row.get(7)?,
                        four_piece_effect_json: row.get(8)?,
                        source_refs_json: row.get(9)?,
                        enhancement_rule_json: row.get(10)?,
                        effective_from: row.get(11)?,
                        effective_to: row.get(12)?,
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            match rows.next() {
                Some(Ok(s)) => Ok(Some(s)),
                Some(Err(e)) => Err(AppError::database("read row", &e)),
                None => Ok(None),
            }
        })
    }

    fn list_groups(&self) -> Result<Vec<YuhunGroup>, AppError> {
        self.db.read("list_groups", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, origin, name, description, parent_id, revision FROM yuhun_group ORDER BY id",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(YuhunGroup {
                        id: row.get(0)?,
                        origin: row.get(1)?,
                        name: row.get(2)?,
                        description: row.get(3)?,
                        parent_id: row.get(4)?,
                        revision: row.get(5)?,
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            let mut groups = Vec::new();
            for row in rows {
                groups.push(row.map_err(|e| AppError::database("read row", &e))?);
            }
            Ok(groups)
        })
    }

    fn group_members(&self, group_id: &str) -> Result<Vec<GroupMember>, AppError> {
        let gid = group_id.to_string();
        self.db.read("group_members", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT group_id, set_id, catalog_version, origin, user_override \
                     FROM yuhun_group_member WHERE group_id = ?1",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let rows = stmt
                .query_map(params![gid], |row| {
                    Ok(GroupMember {
                        group_id: row.get(0)?,
                        set_id: row.get(1)?,
                        catalog_version: row.get(2)?,
                        origin: row.get(3)?,
                        user_override: row.get::<_, i64>(4)? != 0,
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            let mut members = Vec::new();
            for row in rows {
                members.push(row.map_err(|e| AppError::database("read row", &e))?);
            }
            Ok(members)
        })
    }
}

// ─── 常用度仓库 ───────────────────────────────────────────────────────────────

pub struct SqliteCommonnessRepository {
    db: Arc<Database>,
}

impl SqliteCommonnessRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl CommonnessRepository for SqliteCommonnessRepository {
    fn defaults(&self, catalog_version: &str) -> Result<Vec<SetCommonness>, AppError> {
        let cv = catalog_version.to_string();
        self.db.read("commonness_defaults", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT catalog_version, set_id, scenario, value FROM set_commonness_default WHERE catalog_version = ?1",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let rows = stmt
                .query_map(params![cv], |row| {
                    Ok(SetCommonness {
                        catalog_version: row.get(0)?,
                        set_id: row.get(1)?,
                        scenario: row.get(2)?,
                        value: CommonnessValue::from_str(&row.get::<_, String>(3)?).unwrap_or(CommonnessValue::Common),
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            let mut result = Vec::new();
            for row in rows {
                result.push(row.map_err(|e| AppError::database("read row", &e))?);
            }
            Ok(result)
        })
    }

    fn overrides(&self, profile_id: &str) -> Result<Vec<ProfileCommonness>, AppError> {
        let pid = profile_id.to_string();
        self.db.read("commonness_overrides", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT profile_id, set_id, scenario, value FROM profile_commonness_override WHERE profile_id = ?1",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let rows = stmt
                .query_map(params![pid], |row| {
                    Ok(ProfileCommonness {
                        profile_id: row.get(0)?,
                        set_id: row.get(1)?,
                        scenario: row.get(2)?,
                        value: CommonnessValue::from_str(&row.get::<_, String>(3)?).unwrap_or(CommonnessValue::Common),
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            let mut result = Vec::new();
            for row in rows {
                result.push(row.map_err(|e| AppError::database("read row", &e))?);
            }
            Ok(result)
        })
    }

    fn effective(
        &self,
        profile_id: &str,
        catalog_version: &str,
    ) -> Result<Vec<EffectiveCommonness>, AppError> {
        let pid = profile_id.to_string();
        let cv = catalog_version.to_string();
        self.db.read("effective_commonness", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT d.set_id, d.scenario, d.value, (o.value IS NOT NULL) as is_override, \
                     COALESCE(o.value, d.value) as effective_value \
                     FROM set_commonness_default d \
                     LEFT JOIN profile_commonness_override o \
                       ON o.set_id = d.set_id AND o.scenario = d.scenario AND o.profile_id = ?1 \
                     WHERE d.catalog_version = ?2 \
                     ORDER BY d.set_id, d.scenario",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let rows = stmt
                .query_map(params![pid, cv], |row| {
                    Ok(EffectiveCommonness {
                        set_id: row.get(0)?,
                        scenario: row.get(1)?,
                        value: CommonnessValue::from_str(&row.get::<_, String>(4)?)
                            .unwrap_or(CommonnessValue::Common),
                        is_override: row.get::<_, i64>(3)? != 0,
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            let mut result = Vec::new();
            for row in rows {
                result.push(row.map_err(|e| AppError::database("read row", &e))?);
            }
            Ok(result)
        })
    }

    fn upsert_override(&self, override_: &ProfileCommonness) -> Result<(), AppError> {
        let o = override_.clone();
        self.db.write("upsert_commonness_override", |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO profile_commonness_override (profile_id, set_id, scenario, value) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![o.profile_id, o.set_id, o.scenario, o.value.as_str()],
            )
            .map_err(|e| AppError::constraint_violation("profile_commonness_override", &e))?;
            Ok(())
        })
    }

    fn delete_override(
        &self,
        profile_id: &str,
        set_id: &str,
        scenario: &str,
    ) -> Result<(), AppError> {
        let pid = profile_id.to_string();
        let sid = set_id.to_string();
        let sc = scenario.to_string();
        self.db.write("delete_commonness_override", |conn| {
            conn.execute(
                "DELETE FROM profile_commonness_override WHERE profile_id = ?1 AND set_id = ?2 AND scenario = ?3",
                params![pid, sid, sc],
            )
            .map_err(|e| AppError::database("delete override", &e))?;
            Ok(())
        })
    }

    fn copy_overrides(&self, from_profile: &str, to_profile: &str) -> Result<usize, AppError> {
        let from = from_profile.to_string();
        let to = to_profile.to_string();
        self.db.write("copy_commonness_overrides", |conn| {
            // 清空目标覆盖
            conn.execute(
                "DELETE FROM profile_commonness_override WHERE profile_id = ?1",
                params![to],
            )
            .map_err(|e| AppError::database("clear target overrides", &e))?;

            // 复制源覆盖
            conn.execute(
                "INSERT INTO profile_commonness_override (profile_id, set_id, scenario, value) \
                 SELECT ?1, set_id, scenario, value FROM profile_commonness_override WHERE profile_id = ?2",
                params![to, from],
            )
            .map_err(|e| AppError::database("copy overrides", &e))?;

            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM profile_commonness_override WHERE profile_id = ?1",
                    params![to],
                    |row| row.get(0),
                )
                .map_err(|e| AppError::database("count copied", &e))?;
            Ok(count as usize)
        })
    }
}

// ─── 备份记录仓库 ─────────────────────────────────────────────────────────────

pub struct SqliteBackupRecordRepository {
    db: Arc<Database>,
}

impl SqliteBackupRecordRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl BackupRecordRepository for SqliteBackupRecordRepository {
    fn insert(&self, record: &BackupRecord) -> Result<(), AppError> {
        let r = record.clone();
        self.db.write("insert_backup_record", |conn| {
            conn.execute(
                "INSERT INTO backup_record (id, path, schema_version, file_count, total_size, \
                 reason, checksum, created_at, restored_at, verified) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    r.id,
                    r.path,
                    r.schema_version,
                    r.file_count,
                    r.total_size,
                    r.reason,
                    r.checksum,
                    r.created_at,
                    r.restored_at,
                    r.verified,
                ],
            )
            .map_err(|e| AppError::constraint_violation("backup_record", &e))?;
            Ok(())
        })
    }

    fn list(&self) -> Result<Vec<BackupRecord>, AppError> {
        self.db.read("list_backup_records", |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, path, schema_version, file_count, total_size, reason, checksum, \
                     created_at, restored_at, verified FROM backup_record ORDER BY created_at DESC",
                )
                .map_err(|e| AppError::database("prepare", &e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(BackupRecord {
                        id: row.get(0)?,
                        path: row.get(1)?,
                        schema_version: row.get(2)?,
                        file_count: row.get(3)?,
                        total_size: row.get(4)?,
                        reason: row.get(5)?,
                        checksum: row.get(6)?,
                        created_at: row.get(7)?,
                        restored_at: row.get(8)?,
                        verified: row.get::<_, i64>(9)? != 0,
                    })
                })
                .map_err(|e| AppError::database("query", &e))?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(|e| AppError::database("read row", &e))?);
            }
            Ok(records)
        })
    }

    /// 文件清理成功后删除备份记录，避免界面继续展示已经失效的恢复入口。
    fn delete(&self, id: &str) -> Result<(), AppError> {
        let record_id = id.to_owned();
        self.db.write("delete_backup_record", |conn| {
            conn.execute(
                "DELETE FROM backup_record WHERE id = ?1",
                params![record_id],
            )
            .map_err(|error| AppError::database("删除备份记录", &error))?;
            Ok(())
        })
    }

    fn mark_restored(&self, id: &str, now: &str) -> Result<(), AppError> {
        let rid = id.to_string();
        let n = now.to_string();
        self.db.write("mark_restored", |conn| {
            conn.execute(
                "UPDATE backup_record SET restored_at = ?1 WHERE id = ?2",
                params![n, rid],
            )
            .map_err(|e| AppError::database("mark restored", &e))?;
            Ok(())
        })
    }
}

// ─── 已安装数据包仓库 ─────────────────────────────────────────────────────────

/// SQLite 中的更新安装记录仓库；活动状态和历史版本在同一写入事务中切换。
pub struct SqlitePackageRepository {
    db: Arc<Database>,
}

impl SqlitePackageRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl PackageRepository for SqlitePackageRepository {
    /// 写入通过验证的数据包并切换同类型活动版本，避免前端看到半安装状态。
    fn install(&self, package: &InstalledPackage) -> Result<(), AppError> {
        let package = package.clone();
        self.db.write("install_update_package", |conn| {
            conn.execute(
                "UPDATE installed_package SET active = 0 WHERE package_type = ?1",
                params![package.package_type],
            )
            .map_err(|error| AppError::database("停用旧数据包", &error))?;
            conn.execute(
                "INSERT INTO installed_package (
                    id, package_type, version, sha256, signature_fingerprint, active,
                    source, content_path, installed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(id) DO UPDATE SET
                    version = excluded.version,
                    sha256 = excluded.sha256,
                    signature_fingerprint = excluded.signature_fingerprint,
                    active = excluded.active,
                    source = excluded.source,
                    content_path = excluded.content_path,
                    installed_at = excluded.installed_at",
                params![
                    package.id,
                    package.package_type,
                    package.version,
                    package.sha256,
                    package.signature_fingerprint,
                    package.active,
                    package.source,
                    package.content_path,
                    package.installed_at,
                ],
            )
            .map_err(|error| AppError::constraint_violation("installed_package", &error))?;
            Ok(())
        })
    }

    /// 查询完整安装历史，供更新中心展示来源、签名身份和回退目标。
    fn list(&self) -> Result<Vec<InstalledPackage>, AppError> {
        self.db.read("list_installed_packages", |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT id, package_type, version, sha256, signature_fingerprint, active,
                            source, content_path, installed_at
                     FROM installed_package
                     ORDER BY installed_at DESC",
                )
                .map_err(|error| AppError::database("准备数据包历史查询", &error))?;
            let rows = statement
                .query_map([], |row| {
                    Ok(InstalledPackage {
                        id: row.get(0)?,
                        package_type: row.get(1)?,
                        version: row.get(2)?,
                        sha256: row.get(3)?,
                        signature_fingerprint: row.get(4)?,
                        active: row.get::<_, i64>(5)? != 0,
                        source: row.get(6)?,
                        content_path: row.get(7)?,
                        installed_at: row.get(8)?,
                    })
                })
                .map_err(|error| AppError::database("查询数据包历史", &error))?;
            rows.map(|row| row.map_err(|error| AppError::database("解析数据包历史", &error)))
                .collect()
        })
    }

    /// 只允许应用层传入已存在的历史版本；SQL 条件保证不会误激活其他包类型。
    fn activate(&self, package_type: &str, version: &str) -> Result<(), AppError> {
        let package_type = package_type.to_owned();
        let version = version.to_owned();
        self.db.write("activate_installed_package", |conn| {
            let updated = conn
                .execute(
                    "UPDATE installed_package
                     SET active = CASE WHEN version = ?2 THEN 1 ELSE 0 END
                     WHERE package_type = ?1",
                    params![package_type, version],
                )
                .map_err(|error| AppError::database("切换活动数据包", &error))?;
            if updated == 0 {
                return Err(AppError::not_found("installed_package", &version));
            }
            let active: i64 = conn
                .query_row(
                    "SELECT count(*) FROM installed_package
                     WHERE package_type = ?1 AND version = ?2",
                    params![package_type, version],
                    |row| row.get(0),
                )
                .map_err(|error| AppError::database("确认活动数据包", &error))?;
            if active == 0 {
                return Err(AppError::not_found("installed_package", &version));
            }
            Ok(())
        })
    }
}

// ─── 分析待办、用户决定与行动批次仓库 ─────────────────────────────────────────

/// 分析动作仓库；所有批量决定和批次状态变化均复用同一写入队列。
pub struct SqliteActionRepository {
    db: Arc<Database>,
}

impl SqliteActionRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl ActionRepository for SqliteActionRepository {
    fn upsert_todos(&self, todos: &[AnalysisTodo]) -> Result<(), AppError> {
        let todos = todos.to_vec();
        self.db.write("upsert_analysis_todos", |conn| {
            for todo in &todos {
                conn.execute(
                    "INSERT INTO analysis_todo (
                        id, profile_id, soul_key, snapshot_id, soul_internal_id, set_id, slot,
                        quality, level, main_attribute, main_value, standard_score, composite_score,
                        category, recommendation,
                        reason_summary, evidence_level, data_quality, presence_state, is_new,
                        is_changed, detail_json, explanation_hash, generated_at, revision
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                        ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
                    ON CONFLICT(profile_id, soul_key) DO UPDATE SET
                        id = excluded.id,
                        snapshot_id = excluded.snapshot_id,
                        soul_internal_id = excluded.soul_internal_id,
                        set_id = excluded.set_id,
                        slot = excluded.slot,
                        quality = excluded.quality,
                        level = excluded.level,
                        main_attribute = excluded.main_attribute,
                        main_value = excluded.main_value,
                        standard_score = excluded.standard_score,
                        composite_score = excluded.composite_score,
                        category = excluded.category,
                        recommendation = excluded.recommendation,
                        reason_summary = excluded.reason_summary,
                        evidence_level = excluded.evidence_level,
                        data_quality = excluded.data_quality,
                        presence_state = excluded.presence_state,
                        is_new = excluded.is_new,
                        is_changed = excluded.is_changed,
                        detail_json = excluded.detail_json,
                        explanation_hash = excluded.explanation_hash,
                        generated_at = excluded.generated_at,
                        revision = analysis_todo.revision + 1",
                    params![
                        todo.id,
                        todo.profile_id,
                        todo.soul_key,
                        todo.snapshot_id,
                        todo.soul_internal_id,
                        todo.set_id,
                        todo.slot,
                        todo.quality,
                        todo.level,
                        todo.main_attribute,
                        todo.main_value,
                        todo.standard_score,
                        todo.composite_score,
                        todo.category,
                        todo.recommendation,
                        todo.reason_summary,
                        todo.evidence_level,
                        todo.data_quality,
                        todo.presence_state,
                        todo.is_new,
                        todo.is_changed,
                        todo.detail_json,
                        todo.explanation_hash,
                        todo.generated_at,
                        todo.revision,
                    ],
                )
                .map_err(|error| AppError::constraint_violation("analysis_todo", &error))?;
            }
            Ok(())
        })
    }

    fn sync_todo_presence(&self, profile_id: &str) -> Result<(), AppError> {
        let pid = profile_id.to_owned();
        self.db.write("sync_analysis_todo_presence", |conn| {
            conn.execute(
                "UPDATE analysis_todo
                 SET presence_state = COALESCE(
                     (SELECT i.presence_state FROM inventory_item i
                      WHERE i.profile_id = analysis_todo.profile_id AND i.soul_key = analysis_todo.soul_key),
                     presence_state
                 )
                 WHERE profile_id = ?1",
                params![pid],
            )
            .map_err(|error| AppError::database("sync analysis todo presence", &error))?;
            Ok(())
        })
    }

    fn list_todos(
        &self,
        profile_id: &str,
        query: &TodoQuery<'_>,
    ) -> Result<AnalysisTodoPage, AppError> {
        let pid = profile_id.to_owned();
        let category = query.category.map(str::to_owned);
        let search = query.search.map(str::to_owned);
        let use_id = query.use_id.map(str::to_owned);
        let limit = query.limit.clamp(1, 1_000);
        let offset = query.offset;
        self.db.read("list_analysis_todos", |conn| {
            let mut where_sql = String::from("WHERE t.profile_id = ?1 AND t.presence_state <> 'removed'");
            let mut values = vec![Value::Text(pid.clone())];
            if let Some(category) = &category {
                where_sql.push_str(" AND t.category = ?2");
                values.push(Value::Text(category.clone()));
            }
            if let Some(search) = &search {
                let index = values.len() + 1;
                where_sql.push_str(&format!(
                    " AND (t.soul_key LIKE ?{index} OR t.set_id LIKE ?{index} OR \
                     t.main_attribute LIKE ?{index} OR t.detail_json LIKE ?{index})"
                ));
                values.push(Value::Text(format!("%{search}%")));
            }
            if let Some(use_id) = &use_id {
                let index = values.len() + 1;
                // 解释树由 serde_json 生成无空格 JSON；转义 LIKE 元字符后按 useId 字段精确命中。
                let escaped_use_id = escape_like_pattern(use_id);
                where_sql.push_str(&format!(
                    " AND t.detail_json LIKE ?{index} ESCAPE '\\'"
                ));
                values.push(Value::Text(format!("%\"useId\":\"{escaped_use_id}\"%")));
            }

            let total_sql = format!("SELECT count(*) FROM analysis_todo t {where_sql}");
            let total: u32 = conn
                .query_row(&total_sql, params_from_iter(values.clone()), |row| row.get(0))
                .map_err(|error| AppError::database("count analysis todos", &error))?;

            let limit_index = values.len() + 1;
            let offset_index = values.len() + 2;
            let select_sql = format!(
                "SELECT {TODO_SELECT_FIELDS}
                 FROM analysis_todo t
                 LEFT JOIN user_decision d ON d.profile_id = t.profile_id AND d.soul_key = t.soul_key
                 {where_sql}
                 ORDER BY t.is_new DESC, t.is_changed DESC, t.generated_at DESC, t.soul_key
                 LIMIT ?{limit_index} OFFSET ?{offset_index}"
            );
            values.push(Value::Integer(i64::from(limit)));
            values.push(Value::Integer(i64::from(offset)));
            let mut statement = conn
                .prepare(&select_sql)
                .map_err(|error| AppError::database("prepare list analysis todos", &error))?;
            let rows = statement
                .query_map(params_from_iter(values), row_to_analysis_todo)
                .map_err(|error| AppError::database("query list analysis todos", &error))?;
            let items = rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read analysis todo row", &error))?;
            Ok(AnalysisTodoPage {
                items,
                total,
                limit,
                offset,
            })
        })
    }

    /// 为“我的御魂”读取评分摘要，避免列表页加载每枚御魂的完整解释树。
    fn list_score_summaries(&self, profile_id: &str) -> Result<Vec<SoulScoreSummary>, AppError> {
        let pid = profile_id.to_owned();
        self.db.read("list_soul_score_summaries", |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT t.soul_key, t.category, t.recommendation, t.detail_json,
                            t.generated_at, t.standard_score, t.composite_score
                     FROM analysis_todo t
                     WHERE t.profile_id = ?1 AND t.presence_state <> 'removed'
                     ORDER BY t.generated_at DESC, t.soul_key",
                )
                .map_err(|error| AppError::database("prepare list soul score summaries", &error))?;
            let rows = statement
                .query_map(params![pid], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<f64>>(5)?,
                        row.get::<_, Option<f64>>(6)?,
                    ))
                })
                .map_err(|error| AppError::database("query soul score summaries", &error))?;

            rows.map(|row| {
                let (
                    soul_key,
                    category,
                    recommendation,
                    detail_json,
                    generated_at,
                    standard_score,
                    composite_score,
                ) = row.map_err(|error| AppError::database("read soul score summary", &error))?;
                let detail = serde_json::from_str::<serde_json::Value>(&detail_json)
                    .map_err(|error| AppError::database("decode soul score summary", &error))?;
                Ok(score_summary_from_detail(
                    soul_key,
                    category,
                    recommendation,
                    generated_at,
                    standard_score,
                    composite_score,
                    &detail,
                ))
            })
            .collect()
        })
    }

    fn list_score_summaries_by_keys(
        &self,
        profile_id: &str,
        soul_keys: &[String],
    ) -> Result<Vec<SoulScoreSummary>, AppError> {
        if soul_keys.is_empty() {
            return Ok(Vec::new());
        }

        let pid = profile_id.to_owned();
        let keys = soul_keys.to_vec();
        self.db.read("list_soul_score_summaries_by_keys", |conn| {
            // 当前页面最多查询 10 个稳定键，绑定参数只解析当前页对应的评分详情。
            let placeholders = (2..=keys.len() + 1)
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT t.soul_key, t.category, t.recommendation, t.detail_json, t.generated_at, \
                         t.standard_score, t.composite_score \
                 FROM analysis_todo t \
                 WHERE t.profile_id = ?1 AND t.presence_state <> 'removed' \
                   AND t.soul_key IN ({placeholders}) \
                 ORDER BY t.generated_at DESC, t.soul_key"
            );
            let mut values = vec![Value::Text(pid)];
            values.extend(keys.into_iter().map(Value::Text));
            let mut statement = conn.prepare(&sql).map_err(|error| {
                AppError::database("prepare selected soul score summaries", &error)
            })?;
            let rows = statement
                .query_map(params_from_iter(values), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<f64>>(5)?,
                        row.get::<_, Option<f64>>(6)?,
                    ))
                })
                .map_err(|error| {
                    AppError::database("query selected soul score summaries", &error)
                })?;

            rows.map(|row| {
                let (
                    soul_key,
                    category,
                    recommendation,
                    detail_json,
                    generated_at,
                    standard_score,
                    composite_score,
                ) = row.map_err(|error| {
                    AppError::database("read selected soul score summary", &error)
                })?;
                let detail =
                    serde_json::from_str::<serde_json::Value>(&detail_json).map_err(|error| {
                        AppError::database("decode selected soul score summary", &error)
                    })?;
                Ok(score_summary_from_detail(
                    soul_key,
                    category,
                    recommendation,
                    generated_at,
                    standard_score,
                    composite_score,
                    &detail,
                ))
            })
            .collect()
        })
    }

    fn count_score_summaries(&self, profile_id: &str) -> Result<u32, AppError> {
        let pid = profile_id.to_owned();
        self.db.read("count_soul_score_summaries", |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM analysis_todo \
                 WHERE profile_id = ?1 AND presence_state <> 'removed' \
                   AND standard_score IS NOT NULL",
                params![pid],
                |row| row.get(0),
            )
            .map_err(|error| AppError::database("count soul score summaries", &error))
        })
    }

    fn list_decision_history(
        &self,
        profile_id: &str,
        soul_key: &str,
    ) -> Result<Vec<DecisionHistoryEntry>, AppError> {
        let pid = profile_id.to_owned();
        let key = soul_key.to_owned();
        self.db.read("list_decision_history", |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT id, operation_id, profile_id, soul_key, previous_decision,
                            next_decision, note, created_at
                     FROM user_decision_history
                     WHERE profile_id = ?1 AND soul_key = ?2
                     ORDER BY created_at DESC, id DESC",
                )
                .map_err(|error| AppError::database("prepare decision history", &error))?;
            let rows = statement
                .query_map(params![pid, key], |row| {
                    Ok(DecisionHistoryEntry {
                        id: row.get(0)?,
                        operation_id: row.get(1)?,
                        profile_id: row.get(2)?,
                        soul_key: row.get(3)?,
                        previous_decision: row.get(4)?,
                        next_decision: row.get(5)?,
                        note: row.get(6)?,
                        created_at: row.get(7)?,
                    })
                })
                .map_err(|error| AppError::database("query decision history", &error))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read decision history", &error))
        })
    }

    fn preview_decisions(
        &self,
        profile_id: &str,
        changes: &[DecisionChange],
        respect_protection: bool,
    ) -> Result<DecisionPreview, AppError> {
        let pid = profile_id.to_owned();
        let changes = changes.to_vec();
        self.db.read("preview_decisions", |conn| {
            let mut preview = DecisionPreview {
                selected_count: changes.len() as u32,
                ..DecisionPreview::default()
            };
            for change in &changes {
                let Some(todo) = read_analysis_todo(conn, &pid, &change.soul_key)? else {
                    preview.conflict_count += 1;
                    continue;
                };
                classify_decision_change(&todo, change, respect_protection, &mut preview);
            }
            Ok(preview)
        })
    }

    fn apply_decisions(
        &self,
        profile_id: &str,
        operation_id: &str,
        changes: &[DecisionChange],
        respect_protection: bool,
    ) -> Result<DecisionApplyResult, AppError> {
        let pid = profile_id.to_owned();
        let operation = operation_id.to_owned();
        let changes = changes.to_vec();
        let now = crate::application::services::AppServices::now_iso();
        self.db.write("apply_decisions", |conn| {
            let mut result = DecisionApplyResult {
                operation_id: operation.clone(),
                added_count: 0,
                overwritten_count: 0,
                skipped_count: 0,
                protected_count: 0,
                conflict_count: 0,
            };
            for change in &changes {
                let Some(todo) = read_analysis_todo(conn, &pid, &change.soul_key)? else {
                    result.conflict_count += 1;
                    continue;
                };
                let current: Option<String> = conn
                    .query_row(
                        "SELECT decision FROM user_decision WHERE profile_id = ?1 AND soul_key = ?2",
                        params![pid, change.soul_key],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| AppError::database("read current decision", &error))?;
                let should_protect = respect_protection
                    && change.decision.as_deref() == Some("plan_recycle")
                    && (is_protected_todo(&todo)
                        || matches!(current.as_deref(), Some("keep" | "observe")));
                if should_protect {
                    result.protected_count += 1;
                    continue;
                }
                if current == change.decision {
                    result.skipped_count += 1;
                    continue;
                }
                if current.is_some() {
                    result.overwritten_count += 1;
                } else {
                    result.added_count += 1;
                }
                conn.execute(
                    "INSERT INTO user_decision_history (
                        id, operation_id, profile_id, soul_key, previous_decision,
                        next_decision, note, created_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        operation,
                        pid,
                        change.soul_key,
                        current,
                        change.decision,
                        change.note,
                        now,
                    ],
                )
                .map_err(|error| AppError::constraint_violation("user_decision_history", &error))?;
                match &change.decision {
                    Some(decision) => {
                        conn.execute(
                            "INSERT INTO user_decision (
                                profile_id, soul_key, decision, note, revision, created_at, updated_at
                            ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)
                            ON CONFLICT(profile_id, soul_key) DO UPDATE SET
                                decision = excluded.decision,
                                note = excluded.note,
                                revision = user_decision.revision + 1,
                                updated_at = excluded.updated_at",
                            params![pid, change.soul_key, decision, change.note, now],
                        )
                        .map_err(|error| AppError::constraint_violation("user_decision", &error))?;
                    }
                    None => {
                        conn.execute(
                            "DELETE FROM user_decision WHERE profile_id = ?1 AND soul_key = ?2",
                            params![pid, change.soul_key],
                        )
                        .map_err(|error| AppError::database("clear user decision", &error))?;
                    }
                }
            }
            Ok(result)
        })
    }

    fn undo_decision(&self, operation_id: &str) -> Result<(), AppError> {
        let operation = operation_id.to_owned();
        let now = crate::application::services::AppServices::now_iso();
        self.db.write("undo_decision", |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT profile_id, soul_key, previous_decision, next_decision
                     FROM user_decision_history
                     WHERE operation_id = ?1
                     ORDER BY created_at DESC, id DESC",
                )
                .map_err(|error| AppError::database("prepare undo decision", &error))?;
            let rows = statement
                .query_map(params![operation], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .map_err(|error| AppError::database("query undo decision", &error))?;
            let histories = rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read undo decision", &error))?;
            if histories.is_empty() {
                return Err(AppError::not_found("decision_operation", &operation));
            }
            for (profile_id, soul_key, previous, next) in histories {
                let current: Option<String> = conn
                    .query_row(
                        "SELECT decision FROM user_decision WHERE profile_id = ?1 AND soul_key = ?2",
                        params![profile_id, soul_key],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| AppError::database("read undo current decision", &error))?;
                // 只有当前仍处于本次操作结果时才回滚，避免撤销覆盖之后的新决定。
                if current != next {
                    continue;
                }
                match previous {
                    Some(decision) => {
                        conn.execute(
                            "UPDATE user_decision SET decision = ?1, revision = revision + 1, updated_at = ?2
                             WHERE profile_id = ?3 AND soul_key = ?4",
                            params![decision, now, profile_id, soul_key],
                        )
                        .map_err(|error| AppError::database("restore user decision", &error))?;
                    }
                    None => {
                        conn.execute(
                            "DELETE FROM user_decision WHERE profile_id = ?1 AND soul_key = ?2",
                            params![profile_id, soul_key],
                        )
                        .map_err(|error| AppError::database("remove restored decision", &error))?;
                    }
                }
            }
            Ok(())
        })
    }

    fn create_batch(&self, batch: &ActionBatch, items: &[BatchItem]) -> Result<(), AppError> {
        let batch = batch.clone();
        let items = items.to_vec();
        self.db.write("create_action_batch", |conn| {
            conn.execute(
                "INSERT INTO action_batch (
                    id, profile_id, kind, status, target_level, snapshot_id,
                    created_at, updated_at, completed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    batch.id,
                    batch.profile_id,
                    batch.kind,
                    batch.status,
                    batch.target_level,
                    batch.snapshot_id,
                    batch.created_at,
                    batch.updated_at,
                    batch.completed_at,
                ],
            )
            .map_err(|error| AppError::constraint_violation("action_batch", &error))?;
            for item in &items {
                conn.execute(
                    "INSERT INTO action_batch_item (
                        batch_id, soul_key, group_key, set_id, slot, main_attribute, level,
                        status, sort_order, note, completed_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        item.batch_id,
                        item.soul_key,
                        item.group_key,
                        item.set_id,
                        item.slot,
                        item.main_attribute,
                        item.level,
                        item.status,
                        item.sort_order,
                        item.note,
                        item.completed_at,
                    ],
                )
                .map_err(|error| AppError::constraint_violation("action_batch_item", &error))?;
            }
            Ok(())
        })
    }

    fn list_batches(&self, profile_id: &str) -> Result<Vec<ActionBatch>, AppError> {
        let pid = profile_id.to_owned();
        self.db.read("list_action_batches", |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT id FROM action_batch WHERE profile_id = ?1 ORDER BY created_at DESC",
                )
                .map_err(|error| AppError::database("prepare list action batches", &error))?;
            let ids = statement
                .query_map(params![pid], |row| row.get::<_, String>(0))
                .map_err(|error| AppError::database("query action batch ids", &error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read action batch ids", &error))?;
            ids.into_iter()
                .map(|id| {
                    read_batch_detail(conn, profile_id, &id)?
                        .ok_or_else(|| AppError::not_found("action_batch", &id))
                        .map(|detail| detail.batch)
                })
                .collect()
        })
    }

    fn get_batch(
        &self,
        profile_id: &str,
        batch_id: &str,
    ) -> Result<Option<ActionBatchDetail>, AppError> {
        let pid = profile_id.to_owned();
        let bid = batch_id.to_owned();
        self.db.read("get_action_batch", |conn| {
            read_batch_detail(conn, &pid, &bid)
        })
    }

    fn update_batch_item(
        &self,
        profile_id: &str,
        batch_id: &str,
        soul_key: &str,
        status: &str,
        note: Option<&str>,
    ) -> Result<ActionBatchDetail, AppError> {
        if !["pending", "completed", "skipped", "not_found"].contains(&status) {
            return Err(AppError::invalid_argument("status", "批次条目状态不正确"));
        }
        let pid = profile_id.to_owned();
        let bid = batch_id.to_owned();
        let key = soul_key.to_owned();
        let status = status.to_owned();
        let note = note.map(str::to_owned);
        let now = crate::application::services::AppServices::now_iso();
        self.db.write("update_action_batch_item", |conn| {
            let changed = conn
                .execute(
                    "UPDATE action_batch_item SET status = ?1, note = ?2,
                            completed_at = CASE WHEN ?1 = 'pending' THEN NULL ELSE ?3 END
                     WHERE batch_id = ?4 AND soul_key = ?5
                       AND EXISTS (SELECT 1 FROM action_batch WHERE id = ?4 AND profile_id = ?6)",
                    params![status, note, now, bid, key, pid],
                )
                .map_err(|error| AppError::database("update action batch item", &error))?;
            if changed == 0 {
                return Err(AppError::not_found("action_batch_item", &key));
            }
            conn.execute(
                "UPDATE action_batch SET
                    status = CASE WHEN EXISTS (
                        SELECT 1 FROM action_batch_item WHERE batch_id = ?1 AND status = 'pending'
                    ) THEN 'active' ELSE 'completed' END,
                    updated_at = ?2,
                    completed_at = CASE WHEN EXISTS (
                        SELECT 1 FROM action_batch_item WHERE batch_id = ?1 AND status = 'pending'
                    ) THEN NULL ELSE ?2 END
                 WHERE id = ?1 AND profile_id = ?3",
                params![bid, now, pid],
            )
            .map_err(|error| AppError::database("refresh action batch status", &error))?;
            Ok(())
        })?;
        self.get_batch(profile_id, batch_id)?
            .ok_or_else(|| AppError::not_found("action_batch", batch_id))
    }
}

// ─── 规则版本仓库 ─────────────────────────────────────────────────────────────

/// SQLite 规则仓库；规则版本正文和启用关系使用独立写入边界保存。
pub struct SqliteRuleRepository {
    db: Arc<Database>,
}

impl SqliteRuleRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl RuleRepository for SqliteRuleRepository {
    fn list_versions(&self) -> Result<Vec<RuleVersionRecord>, AppError> {
        self.db.read("list_rule_versions", |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT id, preset_id, version, schema_version, title, author, status,
                            origin, canonical_json, canonical_sha256, parent_version_id,
                            read_only, created_at
                     FROM rule_version ORDER BY created_at DESC, id DESC",
                )
                .map_err(|error| AppError::database("prepare rule versions", &error))?;
            let rows = statement
                .query_map([], row_to_rule_version)
                .map_err(|error| AppError::database("query rule versions", &error))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read rule version", &error))
        })
    }

    fn get_version(&self, version_id: &str) -> Result<Option<RuleVersionRecord>, AppError> {
        let version_id = version_id.to_owned();
        self.db.read("get_rule_version", |conn| {
            conn.query_row(
                "SELECT id, preset_id, version, schema_version, title, author, status,
                        origin, canonical_json, canonical_sha256, parent_version_id,
                        read_only, created_at
                 FROM rule_version WHERE id = ?1",
                params![version_id],
                row_to_rule_version,
            )
            .optional()
            .map_err(|error| AppError::database("read rule version", &error))
        })
    }

    fn insert_version(&self, version: &RuleVersionRecord) -> Result<(), AppError> {
        let version = version.clone();
        self.db.write("insert_rule_version", |conn| {
            // 预设元数据与具体版本在同一事务写入，确保版本外键始终可追溯到来源。
            conn.execute(
                "INSERT INTO rule_preset (id, origin, author, title, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET author = excluded.author, title = excluded.title",
                params![
                    &version.preset_id,
                    &version.origin,
                    &version.author,
                    &version.title,
                    &version.created_at,
                ],
            )
            .map_err(|error| AppError::constraint_violation("rule_preset", &error))?;
            conn.execute(
                "INSERT INTO rule_version (
                    id, preset_id, version, schema_version, title, author, status, origin,
                    canonical_json, canonical_sha256, parent_version_id, read_only, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    version.id,
                    version.preset_id,
                    version.version,
                    version.schema_version,
                    version.title,
                    version.author,
                    version.status,
                    version.origin,
                    version.canonical_json,
                    version.canonical_sha256,
                    version.parent_version_id,
                    version.read_only,
                    version.created_at,
                ],
            )
            .map_err(|error| AppError::constraint_violation("rule_version", &error))?;
            Ok(())
        })
    }

    /// 更新评分标准正文和可编辑元数据，保持原有 ID 与预设归属不变。
    fn update_version(&self, version: &RuleVersionRecord) -> Result<(), AppError> {
        let version = version.clone();
        self.db.write("update_rule_version", |conn| {
            // 预设标题和作者与规则正文放在同一事务内更新，避免列表摘要与正文不一致。
            conn.execute(
                "UPDATE rule_preset
                 SET origin = ?2, author = ?3, title = ?4
                 WHERE id = ?1",
                params![
                    &version.preset_id,
                    &version.origin,
                    &version.author,
                    &version.title,
                ],
            )
            .map_err(|error| AppError::constraint_violation("rule_preset", &error))?;
            let changed = conn
                .execute(
                    "UPDATE rule_version
                     SET version = ?2,
                         schema_version = ?3,
                         title = ?4,
                         author = ?5,
                         status = ?6,
                         origin = ?7,
                         canonical_json = ?8,
                         canonical_sha256 = ?9,
                         parent_version_id = ?10,
                         read_only = ?11
                     WHERE id = ?1",
                    params![
                        &version.id,
                        &version.version,
                        version.schema_version,
                        &version.title,
                        &version.author,
                        &version.status,
                        &version.origin,
                        &version.canonical_json,
                        &version.canonical_sha256,
                        &version.parent_version_id,
                        version.read_only,
                    ],
                )
                .map_err(|error| AppError::constraint_violation("rule_version", &error))?;
            if changed == 0 {
                return Err(AppError::not_found("rule_version", &version.id));
            }
            Ok(())
        })
    }

    /// 删除评分标准正文，并在其预设不再被任何规则引用时一并清理预设元数据。
    /// 外键会自动移除全局启用指针，应用层随后负责选择新的当前标准。
    fn delete_version(&self, version_id: &str) -> Result<(), AppError> {
        let version_id = version_id.to_owned();
        self.db.write("delete_rule_version", |conn| {
            let preset_id: Option<String> = conn
                .query_row(
                    "SELECT preset_id FROM rule_version WHERE id = ?1",
                    params![&version_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| AppError::database("read rule version before delete", &error))?;
            if preset_id.is_none() {
                return Err(AppError::not_found("rule_version", &version_id));
            }

            // 历史保存流程可能留下以该标准为父记录的副本；解除历史关联后，用户仍可保留这些副本。
            conn.execute(
                "UPDATE rule_version SET parent_version_id = NULL WHERE parent_version_id = ?1",
                params![&version_id],
            )
            .map_err(|error| AppError::constraint_violation("rule_version", &error))?;
            // 先清掉全局当前指针，再删除正文；即使旧数据库未开启外键级联，也不会留下悬空启用状态。
            conn.execute(
                "DELETE FROM active_score_standard WHERE rule_version_id = ?1",
                params![&version_id],
            )
            .map_err(|error| AppError::constraint_violation("active_score_standard", &error))?;
            conn.execute(
                "DELETE FROM rule_version WHERE id = ?1",
                params![&version_id],
            )
            .map_err(|error| AppError::constraint_violation("rule_version", &error))?;

            // 用户预设通常只包含一条规则；仅在没有其他规则引用时清理，避免误删共享元数据。
            if let Some(preset_id) = preset_id {
                conn.execute(
                    "DELETE FROM rule_preset
                     WHERE id = ?1
                       AND origin <> 'builtin'
                       AND NOT EXISTS (
                           SELECT 1 FROM rule_version WHERE preset_id = rule_preset.id
                       )",
                    params![preset_id],
                )
                .map_err(|error| AppError::constraint_violation("rule_preset", &error))?;
            }
            Ok(())
        })
    }

    fn list_activations(&self, profile_id: &str) -> Result<Vec<RuleActivation>, AppError> {
        let profile_id = profile_id.to_owned();
        self.db.read("list_rule_activations", |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT profile_id, rule_version_id, enabled, position, note, enabled_at
                     FROM profile_rule_activation
                     WHERE profile_id = ?1 ORDER BY position, rule_version_id",
                )
                .map_err(|error| AppError::database("prepare rule activations", &error))?;
            let rows = statement
                .query_map(params![profile_id], |row| {
                    Ok(RuleActivation {
                        profile_id: row.get(0)?,
                        rule_version_id: row.get(1)?,
                        enabled: row.get::<_, i64>(2)? != 0,
                        position: row.get(3)?,
                        note: row.get(4)?,
                        enabled_at: row.get(5)?,
                    })
                })
                .map_err(|error| AppError::database("query rule activations", &error))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read rule activation", &error))
        })
    }

    fn set_activation(&self, activation: &RuleActivation) -> Result<(), AppError> {
        let activation = activation.clone();
        self.db.write("set_rule_activation", |conn| {
            conn.execute(
                "INSERT INTO profile_rule_activation (
                    profile_id, rule_version_id, enabled, position, note, enabled_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(profile_id, rule_version_id) DO UPDATE SET
                    enabled = excluded.enabled,
                    position = excluded.position,
                    note = excluded.note,
                    enabled_at = excluded.enabled_at",
                params![
                    activation.profile_id,
                    activation.rule_version_id,
                    activation.enabled,
                    activation.position,
                    activation.note,
                    activation.enabled_at,
                ],
            )
            .map_err(|error| AppError::constraint_violation("profile_rule_activation", &error))?;
            Ok(())
        })
    }

    fn active_score_standard_id(&self) -> Result<Option<String>, AppError> {
        self.db.read("get_active_score_standard", |conn| {
            conn.query_row(
                "SELECT rule_version_id FROM active_score_standard WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| AppError::database("read active score standard", &error))
        })
    }

    fn set_active_score_standard(
        &self,
        rule_version_id: &str,
        note: Option<&str>,
    ) -> Result<(), AppError> {
        let rule_version_id = rule_version_id.to_owned();
        let note = note.map(str::to_owned);
        let enabled_at = crate::application::services::AppServices::now_iso();
        self.db.write("set_active_score_standard", |conn| {
            // 单例主键确保全局同时只有一个评分标准；切换时先删除旧指针再写入新指针。
            conn.execute("DELETE FROM active_score_standard WHERE singleton = 1", [])
                .map_err(|error| AppError::database("clear active score standard", &error))?;
            conn.execute(
                "INSERT INTO active_score_standard (
                    singleton, rule_version_id, enabled_at, note
                 ) VALUES (1, ?1, ?2, ?3)",
                params![rule_version_id, enabled_at, note],
            )
            .map_err(|error| AppError::constraint_violation("active_score_standard", &error))?;
            Ok(())
        })
    }
}

/// 将规则版本查询行解码为领域实体。
fn row_to_rule_version(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuleVersionRecord> {
    Ok(RuleVersionRecord {
        id: row.get(0)?,
        preset_id: row.get(1)?,
        version: row.get(2)?,
        schema_version: row.get(3)?,
        title: row.get(4)?,
        author: row.get(5)?,
        status: row.get(6)?,
        origin: row.get(7)?,
        canonical_json: row.get(8)?,
        canonical_sha256: row.get(9)?,
        parent_version_id: row.get(10)?,
        read_only: row.get::<_, i64>(11)? != 0,
        created_at: row.get(12)?,
    })
}

/// 待办查询中复用的字段片段；用户决定通过左连接读取，但不属于系统建议本身。
const TODO_SELECT_FIELDS: &str = "
    t.id, t.profile_id, t.soul_key, t.snapshot_id, t.soul_internal_id, t.set_id, t.slot,
    t.quality, t.level, t.main_attribute, t.main_value, t.standard_score, t.composite_score,
    t.category, t.recommendation,
    t.reason_summary, t.evidence_level, t.data_quality, t.presence_state, t.is_new,
    t.is_changed, t.detail_json, t.explanation_hash, t.generated_at, t.revision,
    d.decision, d.note, d.revision";

/// 转义用途 ID 中的 LIKE 元字符，确保下钻按字段值精确匹配。
fn escape_like_pattern(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 把待办查询行解码为领域实体。
fn row_to_analysis_todo(row: &rusqlite::Row<'_>) -> rusqlite::Result<AnalysisTodo> {
    Ok(AnalysisTodo {
        id: row.get(0)?,
        profile_id: row.get(1)?,
        soul_key: row.get(2)?,
        snapshot_id: row.get(3)?,
        soul_internal_id: row.get(4)?,
        set_id: row.get(5)?,
        slot: row.get(6)?,
        quality: row.get(7)?,
        level: row.get(8)?,
        main_attribute: row.get(9)?,
        main_value: row.get(10)?,
        standard_score: row.get(11)?,
        composite_score: row.get(12)?,
        category: row.get(13)?,
        recommendation: row.get(14)?,
        reason_summary: row.get(15)?,
        evidence_level: row.get(16)?,
        data_quality: row.get(17)?,
        presence_state: row.get(18)?,
        is_new: row.get::<_, i64>(19)? != 0,
        is_changed: row.get::<_, i64>(20)? != 0,
        detail_json: row.get(21)?,
        explanation_hash: row.get(22)?,
        generated_at: row.get(23)?,
        revision: row.get(24)?,
        user_decision: row.get(25)?,
        user_decision_note: row.get(26)?,
        decision_revision: row.get(27)?,
    })
}

/// 从持久化解释树中提取列表需要的最佳方案评分；字段缺失时保留“未命中”而不是伪造分数。
fn score_summary_from_detail(
    soul_key: String,
    category: String,
    recommendation: String,
    generated_at: String,
    standard_score: Option<f64>,
    composite_score: Option<f64>,
    detail: &serde_json::Value,
) -> SoulScoreSummary {
    let mut best_score: Option<f64> = None;
    let mut best_use_id = None;
    let mut best_use_title = None;
    let mut effective_growth_count = None;
    let mut scored_use_count = 0;

    let standard_score_rule = detail
        .get("standardScore")
        .and_then(|value| value.get("matchedRuleName"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    // 评分规则说明和分数一起从持久化详情提取，列表无需重新计算或加载完整解释树。
    let standard_score_formula = detail
        .get("standardScore")
        .and_then(|value| value.get("formula"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    // 新版评分结果会保存结构化贡献；旧版数据没有该字段时返回空列表，保持历史结果可读取。
    let standard_score_contributions = detail
        .get("standardScore")
        .and_then(|value| value.get("contributions"))
        .cloned()
        .and_then(|value| serde_json::from_value::<Vec<StandardScoreContribution>>(value).ok())
        .unwrap_or_default();

    if let Some(uses) = detail.get("uses").and_then(serde_json::Value::as_array) {
        for usage in uses {
            if usage
                .get("selectorMatched")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            {
                continue;
            }
            let Some(score) = usage.get("score").and_then(serde_json::Value::as_f64) else {
                continue;
            };
            scored_use_count += 1;
            if best_score.is_none_or(|current| score > current) {
                best_score = Some(score);
                best_use_id = usage
                    .get("useId")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                best_use_title = usage
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                effective_growth_count = usage
                    .get("effectiveGrowthCount")
                    .and_then(serde_json::Value::as_f64);
            }
        }
    }

    let standard = detail.get("scoreStandard");
    SoulScoreSummary {
        soul_key,
        standard_score,
        standard_score_rule,
        standard_score_formula,
        standard_score_contributions,
        composite_score,
        best_score,
        best_use_id,
        best_use_title,
        effective_growth_count,
        scored_use_count,
        recommendation,
        category,
        generated_at,
        standard_id: standard
            .and_then(|value| value.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        standard_version: standard
            .and_then(|value| value.get("version"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        standard_title: standard
            .and_then(|value| value.get("title"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        standard_hash: detail
            .get("rulePreset")
            .and_then(|value| value.get("hash"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    }
}

/// 在当前待办和决定之间计算预览分类；该逻辑与实际事务共用同一规则。
fn classify_decision_change(
    todo: &AnalysisTodo,
    change: &DecisionChange,
    respect_protection: bool,
    preview: &mut DecisionPreview,
) {
    let current = todo.user_decision.as_deref();
    if current == change.decision.as_deref() {
        preview.skipped_count += 1;
        return;
    }
    if respect_protection
        && change.decision.as_deref() == Some("plan_recycle")
        && (is_protected_todo(todo) || matches!(current, Some("keep" | "observe")))
    {
        preview.protected_count += 1;
        return;
    }
    if current.is_some() {
        preview.overwritten_count += 1;
    } else {
        preview.added_count += 1;
    }
}

/// 按主键读取单个待办，并附带当前用户决定。
fn read_analysis_todo(
    conn: &rusqlite::Connection,
    profile_id: &str,
    soul_key: &str,
) -> Result<Option<AnalysisTodo>, AppError> {
    conn.query_row(
        &format!(
            "SELECT {TODO_SELECT_FIELDS}
             FROM analysis_todo t
             LEFT JOIN user_decision d ON d.profile_id = t.profile_id AND d.soul_key = t.soul_key
             WHERE t.profile_id = ?1 AND t.soul_key = ?2"
        ),
        params![profile_id, soul_key],
        row_to_analysis_todo,
    )
    .optional()
    .map_err(|error| AppError::database("read analysis todo", &error))
}

/// 读取批次摘要、分组和条目；调用方已经持有读或写事务边界。
fn read_batch_detail(
    conn: &rusqlite::Connection,
    profile_id: &str,
    batch_id: &str,
) -> Result<Option<ActionBatchDetail>, AppError> {
    let header = conn
        .query_row(
            "SELECT id, profile_id, kind, status, target_level, snapshot_id, created_at,
                    updated_at, completed_at
             FROM action_batch WHERE id = ?1 AND profile_id = ?2",
            params![batch_id, profile_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<u8>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .optional()
        .map_err(|error| AppError::database("read action batch", &error))?;
    let Some((
        id,
        profile_id,
        kind,
        status,
        target_level,
        snapshot_id,
        created_at,
        updated_at,
        completed_at,
    )) = header
    else {
        return Ok(None);
    };
    let mut statement = conn
        .prepare(
            "SELECT batch_id, soul_key, group_key, set_id, slot, main_attribute, level,
                    status, sort_order, note, completed_at
             FROM action_batch_item WHERE batch_id = ?1 ORDER BY sort_order, soul_key",
        )
        .map_err(|error| AppError::database("prepare action batch items", &error))?;
    let items = statement
        .query_map(params![batch_id], |row| {
            Ok(BatchItem {
                batch_id: row.get(0)?,
                soul_key: row.get(1)?,
                group_key: row.get(2)?,
                set_id: row.get(3)?,
                slot: row.get(4)?,
                main_attribute: row.get(5)?,
                level: row.get(6)?,
                status: row.get(7)?,
                sort_order: row.get(8)?,
                note: row.get(9)?,
                completed_at: row.get(10)?,
            })
        })
        .map_err(|error| AppError::database("query action batch items", &error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::database("read action batch item", &error))?;

    let mut groups: Vec<crate::domain::BatchGroup> = Vec::new();
    for item in &items {
        let mut matched = false;
        for group in &mut groups {
            if group.group_key == item.group_key {
                group.item_count += 1;
                matched = true;
                break;
            }
        }
        if !matched {
            groups.push(crate::domain::BatchGroup {
                group_key: item.group_key.clone(),
                set_id: item.set_id.clone(),
                slot: item.slot,
                main_attribute: item.main_attribute.clone(),
                level: item.level,
                item_count: 1,
            });
        }
    }
    let completed_count = items
        .iter()
        .filter(|item| item.status == "completed")
        .count() as u32;
    let skipped_count = items.iter().filter(|item| item.status == "skipped").count() as u32;
    let not_found_count = items
        .iter()
        .filter(|item| item.status == "not_found")
        .count() as u32;
    Ok(Some(ActionBatchDetail {
        batch: ActionBatch {
            id,
            profile_id,
            kind,
            status,
            target_level,
            snapshot_id,
            group_count: groups.len() as u32,
            item_count: items.len() as u32,
            completed_count,
            skipped_count,
            not_found_count,
            created_at,
            updated_at,
            completed_at,
            groups,
        },
        items,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::repositories::SnapshotRepository;
    use crate::domain::OwnedShikigamiSkill;
    use crate::infrastructure::database::migrations::MigrationRunner;

    fn temp_db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("yys-analysis-test-repo")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建测试目录");
        dir.join("test.sqlite3")
    }

    fn open_repo(path: &std::path::Path) -> SqliteOwnedShikigamiRepository {
        let db = Database::open(path).expect("打开数据库");
        db.write("migrate", |conn| MigrationRunner::builtin().run(conn))
            .expect("迁移");
        db.write("seed-profiles", |conn| {
            for profile_id in ["profile-a", "profile-b"] {
                conn.execute(
                    "INSERT OR IGNORE INTO game_profile (id, display_name, created_at, updated_at, revision)
                     VALUES (?1, ?2, '2026-08-19T00:00:00Z', '2026-08-19T00:00:00Z', 1)",
                    rusqlite::params![profile_id, format!("测试角色{profile_id}")],
                )
                .map_err(|e| crate::application::error::AppError::database("seed-profiles", &e))?;
            }
            Ok(())
        })
        .expect("写入测试档案");
        SqliteOwnedShikigamiRepository::new(Arc::new(db))
    }

    fn open_snapshot_repo(path: &std::path::Path) -> SqliteSnapshotRepository {
        let db = Database::open(path).expect("打开数据库");
        db.write("migrate", |conn| MigrationRunner::builtin().run(conn))
            .expect("迁移");
        db.write("seed-profiles", |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO raw_object
                 (sha256, relative_path, compression, media_type, raw_size, stored_size, created_at)
                 VALUES ('hash-attribute-filter', 'test/attribute-filter.json.zst', 'zstd',
                         'application/json', 1, 1, '2026-08-19T00:00:00Z')",
                [],
            )
            .map_err(|e| crate::application::error::AppError::database("seed-raw-object", &e))?;
            for profile_id in ["profile-a", "profile-b"] {
                conn.execute(
                    "INSERT OR IGNORE INTO game_profile (id, display_name, created_at, updated_at, revision)
                     VALUES (?1, ?2, '2026-08-19T00:00:00Z', '2026-08-19T00:00:00Z', 1)",
                    rusqlite::params![profile_id, format!("测试角色{profile_id}")],
                )
                .map_err(|e| crate::application::error::AppError::database("seed-profiles", &e))?;
            }
            Ok(())
        })
        .expect("写入测试档案");
        SqliteSnapshotRepository::new(Arc::new(db))
    }

    #[test]
    fn 库存分页应按主属性和副属性数值比较() {
        let path = temp_db("inventory-attribute-value-filter");
        let repo = open_snapshot_repo(&path);
        let snapshot_id = "snapshot-attribute-filter";
        let souls = vec![
            SnapshotSoul {
                snapshot_id: snapshot_id.to_owned(),
                internal_id: "soul-1".to_owned(),
                source_stable_id: Some("stable-1".to_owned()),
                identity_quality: "stable".to_owned(),
                set_id: "set-a".to_owned(),
                slot: 2,
                quality: 6,
                level: 15,
                main_attr_type: "speed".to_owned(),
                main_attr_value: 12.0,
                initial_substat_count: Some(4),
                locked_in_source: Some(false),
                equipped_state: None,
                source_json: None,
            },
            SnapshotSoul {
                snapshot_id: snapshot_id.to_owned(),
                internal_id: "soul-2".to_owned(),
                source_stable_id: Some("stable-2".to_owned()),
                identity_quality: "stable".to_owned(),
                set_id: "set-b".to_owned(),
                slot: 4,
                quality: 6,
                level: 15,
                main_attr_type: "attack_rate".to_owned(),
                main_attr_value: 0.5,
                initial_substat_count: Some(4),
                locked_in_source: Some(false),
                equipped_state: None,
                source_json: None,
            },
            SnapshotSoul {
                snapshot_id: snapshot_id.to_owned(),
                internal_id: "soul-3".to_owned(),
                source_stable_id: Some("stable-3".to_owned()),
                identity_quality: "stable".to_owned(),
                set_id: "set-c".to_owned(),
                slot: 6,
                quality: 6,
                level: 15,
                main_attr_type: "hp_flat".to_owned(),
                main_attr_value: 1000.0,
                initial_substat_count: Some(4),
                locked_in_source: Some(false),
                equipped_state: None,
                source_json: None,
            },
        ];
        let attributes = vec![
            SoulAttribute {
                snapshot_id: snapshot_id.to_owned(),
                soul_internal_id: "soul-1".to_owned(),
                attribute_index: 0,
                attribute_type: "speed".to_owned(),
                value: 16.5,
                enhancement_count: Some(5),
                count_provenance: "source".to_owned(),
                fixed_attribute: false,
            },
            SoulAttribute {
                snapshot_id: snapshot_id.to_owned(),
                soul_internal_id: "soul-2".to_owned(),
                attribute_index: 0,
                attribute_type: "speed".to_owned(),
                value: 8.5,
                enhancement_count: Some(2),
                count_provenance: "source".to_owned(),
                fixed_attribute: false,
            },
            SoulAttribute {
                snapshot_id: snapshot_id.to_owned(),
                soul_internal_id: "soul-3".to_owned(),
                attribute_index: 0,
                attribute_type: "speed".to_owned(),
                value: 12.0,
                enhancement_count: Some(3),
                count_provenance: "source".to_owned(),
                fixed_attribute: false,
            },
            SoulAttribute {
                snapshot_id: snapshot_id.to_owned(),
                soul_internal_id: "soul-2".to_owned(),
                attribute_index: 1,
                attribute_type: "crit_rate".to_owned(),
                value: 0.1,
                enhancement_count: Some(0),
                count_provenance: "source".to_owned(),
                fixed_attribute: false,
            },
        ];
        repo.insert_snapshot(
            &Snapshot {
                id: snapshot_id.to_owned(),
                profile_id: "profile-a".to_owned(),
                raw_sha256: "hash-attribute-filter".to_owned(),
                content_fingerprint: None,
                scope_fingerprint: None,
                source_kind: "importer".to_owned(),
                completeness: "complete".to_owned(),
                scope_json: None,
                captured_at: Some("2026-08-19T00:00:00Z".to_owned()),
                game_version: None,
                adapter_version: None,
                parser_version: "test".to_owned(),
                catalog_version: "test".to_owned(),
                parent_snapshot_id: None,
                forced: false,
                created_at: "2026-08-19T00:00:00Z".to_owned(),
            },
            &souls,
            &attributes,
        )
        .expect("写入属性筛选测试库存");
        for soul in &souls {
            repo.upsert_inventory(&InventoryItem {
                profile_id: "profile-a".to_owned(),
                soul_key: soul.internal_id.clone(),
                snapshot_id: snapshot_id.to_owned(),
                soul_internal_id: soul.internal_id.clone(),
                first_seen_snapshot_id: snapshot_id.to_owned(),
                last_seen_snapshot_id: snapshot_id.to_owned(),
                presence_state: "present".to_owned(),
                updated_at: "2026-08-19T00:00:00Z".to_owned(),
            })
            .expect("写入属性筛选库存投影");
        }

        let mut query = InventoryPageQuery {
            attribute_type: Some("speed".to_owned()),
            attribute_operator: Some("gt".to_owned()),
            attribute_value: Some(7.0),
            // 数值筛选只返回请求页大小的前 N 条，避免为高命中条件计算全量总数。
            limit: 2,
            ..InventoryPageQuery::default()
        };
        let page = repo
            .list_inventory_page("profile-a", &query)
            .expect("按副属性大于筛选");
        assert_eq!(page.total, 2);
        assert_eq!(page.items.len(), 2);
        assert!(page.has_more);
        assert_eq!(page.items[0].soul_key, "soul-1");
        assert_eq!(page.items[1].soul_key, "soul-3");

        // 加载更多沿用同一排序，从第二批继续读取，不重新计算全量总数。
        query.offset = 2;
        let page = repo
            .list_inventory_page("profile-a", &query)
            .expect("数值筛选应支持加载更多");
        assert_eq!(page.total, 3);
        assert_eq!(page.items.len(), 1);
        assert!(!page.has_more);
        assert_eq!(page.items[0].soul_key, "soul-2");

        query.offset = 0;
        query.attribute_operator = Some("lt".to_owned());
        query.attribute_value = Some(15.0);
        let page = repo
            .list_inventory_page("profile-a", &query)
            .expect("按副属性小于筛选");
        assert_eq!(page.total, 2);
        assert_eq!(page.items[0].soul_key, "soul-2");
        assert_eq!(page.items[1].soul_key, "soul-3");

        query.attribute_type = Some("attack_rate".to_owned());
        query.attribute_operator = Some("gt".to_owned());
        query.attribute_value = Some(0.4);
        let page = repo
            .list_inventory_page("profile-a", &query)
            .expect("主属性不应参与数值筛选");
        assert_eq!(page.total, 0);

        // 标准评分上下限由分析结果表直接筛选；严格边界 20 和 80 都不应进入 (20, 80) 区间。
        let score_rows = souls
            .iter()
            .zip([20.0, 50.0, 80.0])
            .map(|(soul, standard_score)| AnalysisTodo {
                id: format!("todo-{}", soul.internal_id),
                profile_id: "profile-a".to_owned(),
                soul_key: soul.internal_id.clone(),
                snapshot_id: snapshot_id.to_owned(),
                soul_internal_id: soul.internal_id.clone(),
                set_id: soul.set_id.clone(),
                slot: soul.slot,
                quality: soul.quality,
                level: soul.level,
                main_attribute: soul.main_attr_type.clone(),
                main_value: soul.main_attr_value,
                standard_score: Some(standard_score),
                composite_score: None,
                    // 测试数据使用数据库约束允许的有效分类，避免夹具本身干扰区间筛选断言。
                    category: "continue".to_owned(),
                recommendation: "keep".to_owned(),
                reason_summary: "评分区间测试".to_owned(),
                evidence_level: "author".to_owned(),
                data_quality: "complete".to_owned(),
                presence_state: "present".to_owned(),
                is_new: false,
                is_changed: false,
                detail_json: "{}".to_owned(),
                explanation_hash: format!("hash-{}", soul.internal_id),
                generated_at: "2026-08-19T00:00:00Z".to_owned(),
                revision: 1,
                user_decision: None,
                user_decision_note: None,
                decision_revision: None,
            })
            .collect::<Vec<_>>();
        SqliteActionRepository::new(repo.db.clone())
            .upsert_todos(&score_rows)
            .expect("写入评分区间测试结果");
        query.attribute_type = None;
        query.attribute_operator = None;
        query.attribute_value = None;
        query.standard_score_min = Some(20.0);
        query.standard_score_max = Some(80.0);
        let page = repo
            .list_inventory_page("profile-a", &query)
            .expect("按标准评分上下限筛选");
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].soul_key, "soul-2");

        let _ = std::fs::remove_dir_all(&path.parent().expect("测试目录"));
    }

    fn sample() -> Vec<OwnedShikigami> {
        vec![
            OwnedShikigami {
                instance_id: "hero-1".to_owned(),
                shikigami_id: "398".to_owned(),
                star: 6,
                level: Some(40),
                exp: None,
                locked: Some(true),
                awakened: Some(true),
                skin_id: Some(2),
                skills: vec![OwnedShikigamiSkill {
                    skill_id: 3981,
                    level: 5,
                }],
                selected_skill_ids: vec![3981],
            },
            OwnedShikigami {
                instance_id: "hero-2".to_owned(),
                shikigami_id: "412".to_owned(),
                star: 5,
                level: Some(35),
                exp: Some(12345),
                locked: Some(false),
                awakened: Some(false),
                skin_id: None,
                skills: Vec::new(),
                selected_skill_ids: Vec::new(),
            },
        ]
    }

    #[test]
    fn 持有式神应按角色档案替换写入并清空() {
        let path = temp_db("owned-shikigami");
        let repo = open_repo(&path);
        repo.replace_all("profile-a", &sample(), "desktop", "2026-08-19T00:00:00Z")
            .expect("写入式神");
        assert_eq!(repo.count("profile-a").expect("计数"), 2);
        repo.replace_all(
            "profile-a",
            &sample()[..1],
            "desktop",
            "2026-08-19T00:00:00Z",
        )
        .expect("覆盖式神");
        assert_eq!(
            repo.count("profile-a").expect("覆盖后计数"),
            1,
            "替换应清空该角色旧镜像"
        );
        repo.replace_all("profile-b", &sample(), "desktop", "2026-08-19T00:00:00Z")
            .expect("写入另一角色");
        assert_eq!(repo.count("profile-b").expect("另一角色计数"), 2);
        assert_eq!(
            repo.count("profile-a").expect("角色A计数"),
            1,
            "不同角色的镜像应互相隔离"
        );
        repo.clear("profile-a").expect("清空角色A");
        assert_eq!(repo.count("profile-a").expect("清空后计数"), 0);
        assert_eq!(repo.count("profile-b").expect("角色B不受影响"), 2);
        let _ = std::fs::remove_dir_all(&path.parent().expect("测试目录"));
    }

    #[test]
    fn 头尾缓存应持久化卡片和副属性并支持覆盖() {
        let path = temp_db("head-tail-cache");
        let db = Database::open(&path).expect("打开数据库");
        db.write("migrate", |conn| MigrationRunner::builtin().run(conn))
            .expect("迁移");
        db.write("seed-profile", |conn| {
            conn.execute(
                "INSERT INTO game_profile (id, display_name, created_at, updated_at, revision)
                 VALUES ('profile-a', '测试角色', '2026-08-19T00:00:00Z', '2026-08-19T00:00:00Z', 1)",
                [],
            )
            .map_err(|error| crate::application::error::AppError::database("seed-profile", &error))?;
            Ok(())
        })
        .expect("写入测试档案");

        let repo = SqliteHeadTailRepository::new(Arc::new(db));
        let attributes = vec![HeadTailAttribute {
            attribute_type: "speed".to_owned(),
            value: 17.6,
            enhancement_count: Some(5),
            fixed_attribute: false,
        }];
        let head = HeadTailCard {
            soul_key: "head-1".to_owned(),
            set_id: "set-speed".to_owned(),
            slot: 2,
            quality: 6,
            level: 15,
            main_attr_type: "speed".to_owned(),
            main_attr_value: 57.0,
            speed: 17.6,
            speed_rolls: 5,
            attributes: attributes.clone(),
        };
        let head_secondary = HeadTailCard {
            soul_key: "head-2".to_owned(),
            set_id: "set-speed-2".to_owned(),
            slot: 2,
            quality: 6,
            level: 15,
            main_attr_type: "speed".to_owned(),
            main_attr_value: 57.0,
            speed: 16.76,
            speed_rolls: 5,
            attributes: attributes.clone(),
        };
        let tail = HeadTailCard {
            soul_key: "tail-1".to_owned(),
            set_id: "set-effect".to_owned(),
            slot: 4,
            quality: 6,
            level: 15,
            main_attr_type: "effect_hit".to_owned(),
            main_attr_value: 55.0,
            speed: 17.1,
            speed_rolls: 5,
            attributes,
        };
        let tail_secondary = HeadTailCard {
            soul_key: "tail-2".to_owned(),
            set_id: "set-effect-2".to_owned(),
            slot: 4,
            quality: 6,
            level: 15,
            main_attr_type: "effect_resist".to_owned(),
            main_attr_value: 55.0,
            speed: 14.01,
            speed_rolls: 5,
            attributes: vec![HeadTailAttribute {
                attribute_type: "speed".to_owned(),
                value: 14.01,
                enhancement_count: None,
                fixed_attribute: false,
            }],
        };
        let heads = vec![head, head_secondary];
        let tails = vec![tail, tail_secondary];

        // 一次写入同时覆盖父缓存和全部卡片，读取必须还原排序、JSON 副属性及候选数量。
        repo.replace(
            "profile-a",
            "2026-08-19T00:00:00Z",
            "revision-1",
            2,
            &heads,
            &tails,
            2,
            2,
        )
        .expect("写入头尾缓存");
        let cache = repo
            .get("profile-a")
            .expect("读取头尾缓存")
            .expect("头尾缓存存在");
        assert_eq!(cache.inventory_revision, "revision-1");
        assert_eq!(cache.head_candidate_count, 2);
        assert_eq!(cache.tail_candidate_count, 2);
        assert_eq!(cache.heads.len(), 2);
        assert_eq!(cache.heads[0].speed, 17.6);
        assert_eq!(cache.heads[1].speed, 16.76);
        assert_eq!(cache.tails.len(), 2);
        assert_eq!(cache.tails[0].attributes.len(), 1);
        assert_eq!(cache.tails[1].speed, 14.01);

        // 空结果覆盖后，旧卡片必须随父缓存事务一并清除，避免页面读到上一轮结果。
        repo.replace(
            "profile-a",
            "2026-08-19T00:01:00Z",
            "revision-2",
            0,
            &[],
            &[],
            0,
            0,
        )
        .expect("覆盖空头尾缓存");
        let empty_cache = repo
            .get("profile-a")
            .expect("读取覆盖后的缓存")
            .expect("父缓存仍存在");
        assert!(empty_cache.heads.is_empty());
        assert!(empty_cache.tails.is_empty());
        assert_eq!(empty_cache.inventory_revision, "revision-2");

        let _ = std::fs::remove_dir_all(&path.parent().expect("测试目录"));
    }
}
