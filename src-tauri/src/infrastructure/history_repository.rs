//! 历史与分析中心的 SQLite 读取实现。
//!
//! 查询按档案 ID 强制隔离；所有统计使用同一份当前库存投影和分析待办，
//! 这样前端图表与下钻列表不会因为各自重算而出现跨档案或计数漂移。

use crate::application::error::AppError;
use crate::domain::{
    AnalysisMetricRow, HistoryAnalysisRepository, InventoryAnalysisRow, ProfileRawReference,
    Snapshot, SnapshotHistoryItem,
};
use crate::infrastructure::database::connection::Database;
use rusqlite::{OptionalExtension, params};
use std::collections::BTreeMap;
use std::sync::Arc;

/// SQLite 历史分析仓库。
pub struct SqliteHistoryAnalysisRepository {
    db: Arc<Database>,
}

impl SqliteHistoryAnalysisRepository {
    /// 创建与应用服务共享连接池的历史分析仓库。
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl HistoryAnalysisRepository for SqliteHistoryAnalysisRepository {
    fn list_snapshot_history(
        &self,
        profile_id: &str,
        current_snapshot_id: Option<&str>,
    ) -> Result<Vec<SnapshotHistoryItem>, AppError> {
        let profile_id = profile_id.to_owned();
        let current_snapshot_id = current_snapshot_id.map(str::to_owned);
        self.db.read("list_snapshot_history", |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT s.id, s.profile_id, s.raw_sha256, s.content_fingerprint,
                            s.scope_fingerprint, s.source_kind, s.completeness, s.scope_json,
                            s.captured_at, s.game_version, s.adapter_version, s.parser_version,
                            s.catalog_version, s.parent_snapshot_id, s.forced, s.created_at,
                            (SELECT count(*) FROM snapshot_soul ss WHERE ss.snapshot_id = s.id),
                            (SELECT count(*) FROM inventory_item i
                             WHERE i.profile_id = s.profile_id AND i.snapshot_id = s.id),
                            COALESCE(si.status, 'healthy'), si.message
                     FROM snapshot s
                     LEFT JOIN snapshot_integrity si ON si.snapshot_id = s.id
                     WHERE s.profile_id = ?1
                     ORDER BY s.created_at DESC, s.id DESC",
                )
                .map_err(|error| AppError::database("prepare snapshot history", &error))?;
            let rows = statement
                .query_map(params![profile_id], |row| {
                    let snapshot_id: String = row.get(0)?;
                    Ok(SnapshotHistoryItem {
                        snapshot: Snapshot {
                            id: snapshot_id.clone(),
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
                        },
                        soul_count: row.get(16)?,
                        inventory_reference_count: row.get(17)?,
                        is_current_baseline: current_snapshot_id.as_deref()
                            == Some(snapshot_id.as_str()),
                        integrity_status: row.get(18)?,
                        integrity_message: row.get(19)?,
                    })
                })
                .map_err(|error| AppError::database("query snapshot history", &error))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read snapshot history", &error))
        })
    }

    fn list_inventory_analysis_rows(
        &self,
        profile_id: &str,
    ) -> Result<Vec<InventoryAnalysisRow>, AppError> {
        let profile_id = profile_id.to_owned();
        self.db.read("list_inventory_analysis_rows", |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT i.soul_key, i.snapshot_id, i.soul_internal_id, ss.set_id,
                            ss.slot, ss.level, ss.initial_substat_count,
                            COALESCE((SELECT group_concat(sa.attribute_type, '|')
                                      FROM soul_attribute sa
                                      WHERE sa.snapshot_id = i.snapshot_id
                                        AND sa.soul_internal_id = i.soul_internal_id), ''),
                            i.presence_state, s.created_at, s.completeness
                     FROM inventory_item i
                     JOIN snapshot_soul ss
                       ON ss.snapshot_id = i.snapshot_id AND ss.internal_id = i.soul_internal_id
                     JOIN snapshot s ON s.id = i.snapshot_id
                     WHERE i.profile_id = ?1
                     ORDER BY i.soul_key",
                )
                .map_err(|error| AppError::database("prepare inventory analysis rows", &error))?;
            let rows = statement
                .query_map(params![profile_id], |row| {
                    Ok(InventoryAnalysisRow {
                        soul_key: row.get(0)?,
                        snapshot_id: row.get(1)?,
                        soul_internal_id: row.get(2)?,
                        set_id: row.get(3)?,
                        slot: row.get(4)?,
                        level: row.get(5)?,
                        initial_substat_count: row.get(6)?,
                        attribute_types: row
                            .get::<_, String>(7)?
                            .split('|')
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned)
                            .collect(),
                        presence_state: row.get(8)?,
                        snapshot_created_at: row.get(9)?,
                        snapshot_completeness: row.get(10)?,
                    })
                })
                .map_err(|error| AppError::database("query inventory analysis rows", &error))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read inventory analysis rows", &error))
        })
    }

    fn list_analysis_metric_rows(
        &self,
        profile_id: &str,
    ) -> Result<Vec<AnalysisMetricRow>, AppError> {
        let profile_id = profile_id.to_owned();
        self.db.read("list_analysis_metric_rows", |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT soul_key, snapshot_id, category, recommendation, data_quality,
                            evidence_level, level, presence_state, generated_at, detail_json
                     FROM analysis_todo
                     WHERE profile_id = ?1
                     ORDER BY soul_key",
                )
                .map_err(|error| AppError::database("prepare analysis metric rows", &error))?;
            let rows = statement
                .query_map(params![profile_id], |row| {
                    Ok(AnalysisMetricRow {
                        soul_key: row.get(0)?,
                        snapshot_id: row.get(1)?,
                        category: row.get(2)?,
                        recommendation: row.get(3)?,
                        data_quality: row.get(4)?,
                        evidence_level: row.get(5)?,
                        level: row.get(6)?,
                        presence_state: row.get(7)?,
                        generated_at: row.get(8)?,
                        detail_json: row.get(9)?,
                    })
                })
                .map_err(|error| AppError::database("query analysis metric rows", &error))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::database("read analysis metric rows", &error))
        })
    }

    fn list_profile_raw_references(
        &self,
        profile_id: &str,
    ) -> Result<Vec<ProfileRawReference>, AppError> {
        let profile_id = profile_id.to_owned();
        self.db.read("list_profile_raw_references", |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT s.raw_sha256, s.id
                     FROM snapshot s
                     WHERE s.profile_id = ?1
                     ORDER BY s.raw_sha256, s.created_at, s.id",
                )
                .map_err(|error| AppError::database("prepare profile raw references", &error))?;
            let rows = statement
                .query_map(params![profile_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| AppError::database("query profile raw references", &error))?;
            let mut grouped = BTreeMap::<String, Vec<String>>::new();
            for row in rows {
                let (sha256, snapshot_id) =
                    row.map_err(|error| AppError::database("read profile raw reference", &error))?;
                grouped.entry(sha256).or_default().push(snapshot_id);
            }
            Ok(grouped
                .into_iter()
                .map(|(sha256, snapshot_ids)| ProfileRawReference {
                    sha256,
                    snapshot_ids,
                })
                .collect())
        })
    }

    fn record_snapshot_integrity(
        &self,
        snapshot_id: &str,
        status: &str,
        message: Option<&str>,
        checked_at: &str,
    ) -> Result<(), AppError> {
        if !["healthy", "missing", "corrupt"].contains(&status) {
            return Err(AppError::invalid_argument(
                "status",
                format!("不支持的快照完整性状态：{status}"),
            ));
        }
        let snapshot_id = snapshot_id.to_owned();
        let status = status.to_owned();
        let message = message.map(str::to_owned);
        let checked_at = checked_at.to_owned();
        self.db.write("record_snapshot_integrity", |conn| {
            conn.execute(
                "INSERT INTO snapshot_integrity (snapshot_id, status, message, checked_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(snapshot_id) DO UPDATE SET
                    status = excluded.status,
                    message = excluded.message,
                    checked_at = excluded.checked_at",
                params![snapshot_id, status, message, checked_at],
            )
            .map_err(|error| AppError::constraint_violation("snapshot_integrity", &error))?;
            Ok(())
        })
    }
}

/// 读取档案最新的库存引用快照，局部快照也必须作为当前基线保留其风险提示。
pub fn current_inventory_snapshot_id(
    db: &Database,
    profile_id: &str,
) -> Result<Option<String>, AppError> {
    let profile_id = profile_id.to_owned();
    db.read("current_inventory_snapshot", |conn| {
        conn.query_row(
            "SELECT i.snapshot_id
             FROM inventory_item i
             JOIN snapshot s ON s.id = i.snapshot_id
             WHERE i.profile_id = ?1 AND i.presence_state <> 'removed'
             ORDER BY s.created_at DESC, i.updated_at DESC, i.snapshot_id DESC
             LIMIT 1",
            params![profile_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| AppError::database("read current inventory snapshot", &error))
    })
}
