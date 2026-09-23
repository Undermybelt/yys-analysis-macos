//! Issue 08 的历史追溯、分析中心、导出和原始对象校验用例。
//!
//! 用例统一按 profile_id 读取数据，再把快照、规则、练度和解释树组合成用户可观察结果。
//! 指标只消费同一份库存投影与分析待办，避免前端以另一套近似算法造成计数不一致。

use crate::application::error::AppError;
use crate::application::services::AppServices;
use crate::domain::{
    AnalysisCenter, AnalysisMetricRow, AnalysisTodo, AnalysisTrace, AttributeDistributionMetric,
    CleanupYield, DataQualitySummary, EnhancementFunnelStage, ExportPayload, HistoryOverview,
    ObjectIntegrityReport, ProfileObjectVerification, RuleTraceVersion, SetStructureMetric,
    TodoQuery, UsageCoverageMetric,
};
use crate::infrastructure::history_repository::current_inventory_snapshot_id;
use crate::infrastructure::raw_object_store::VerifyOutcome;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// 历史分析用例，负责跨仓库编排但不向前端暴露数据库对象。
pub struct HistoryAnalysisUseCase {
    services: Arc<AppServices>,
}

impl HistoryAnalysisUseCase {
    /// 创建历史分析用例。
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 返回快照时间线、采集事件和当前基线，保留局部基线不能确认删除的风险。
    pub fn history_overview(
        &self,
        profile_id: &str,
        event_limit: u32,
    ) -> Result<HistoryOverview, AppError> {
        self.ensure_profile(profile_id)?;
        let current_snapshot_id = current_inventory_snapshot_id(&self.services.db, profile_id)?;
        let snapshots = self
            .services
            .history
            .list_snapshot_history(profile_id, current_snapshot_id.as_deref())?;
        let acquisition_events = self
            .services
            .events
            .list_by_profile(profile_id, event_limit.clamp(1, 1_000))?;
        let current_completeness = snapshots
            .iter()
            .find(|entry| entry.is_current_baseline)
            .map(|entry| entry.snapshot.completeness.clone());

        Ok(HistoryOverview {
            profile_id: profile_id.to_owned(),
            current_baseline_snapshot_id: current_snapshot_id,
            can_confirm_removals: current_completeness.as_deref() == Some("complete"),
            current_completeness,
            snapshots,
            acquisition_events,
        })
    }

    /// 计算库存结构、用途覆盖、强化漏斗、清理收益和数据质量指标。
    pub fn analysis_center(&self, profile_id: &str) -> Result<AnalysisCenter, AppError> {
        self.ensure_profile(profile_id)?;
        let current_snapshot_id = current_inventory_snapshot_id(&self.services.db, profile_id)?;
        let inventory = self
            .services
            .history
            .list_inventory_analysis_rows(profile_id)?;
        let analysis_rows = self
            .services
            .history
            .list_analysis_metric_rows(profile_id)?;
        let history = self
            .services
            .history
            .list_snapshot_history(profile_id, current_snapshot_id.as_deref())?;

        let present_count = inventory
            .iter()
            .filter(|row| row.presence_state == "present")
            .count() as u32;
        let unconfirmed_count = inventory
            .iter()
            .filter(|row| row.presence_state == "unconfirmed")
            .count() as u32;
        let removed_count = inventory
            .iter()
            .filter(|row| row.presence_state == "removed")
            .count() as u32;

        let current_completeness = current_snapshot_id.as_ref().and_then(|snapshot_id| {
            history
                .iter()
                .find(|entry| entry.snapshot.id == *snapshot_id)
                .map(|entry| entry.snapshot.completeness.as_str())
        });

        let mut attribute_counts = BTreeMap::<String, u32>::new();
        let mut set_counts = BTreeMap::<String, u32>::new();
        for row in &inventory {
            if row.presence_state == "removed" {
                continue;
            }
            *set_counts.entry(row.set_id.clone()).or_default() += 1;
            for attribute_type in &row.attribute_types {
                *attribute_counts.entry(attribute_type.clone()).or_default() += 1;
            }
        }

        let commonness = self.commonness_map(profile_id)?;
        let set_structure = set_counts
            .into_iter()
            .map(|(set_id, count)| SetStructureMetric {
                pve_common: commonness
                    .get(&(set_id.clone(), "PVE".to_owned()))
                    .copied()
                    .unwrap_or(false),
                pvp_common: commonness
                    .get(&(set_id.clone(), "PVP".to_owned()))
                    .copied()
                    .unwrap_or(false),
                set_id,
                count,
            })
            .collect();

        let usage_coverage = build_usage_coverage(&analysis_rows);
        let enhancement_funnel = build_enhancement_funnel(&analysis_rows);
        let cleanup_yield = build_cleanup_yield(&analysis_rows);
        let current_created_at = current_snapshot_id.as_ref().and_then(|snapshot_id| {
            history
                .iter()
                .find(|entry| entry.snapshot.id == *snapshot_id)
                .map(|entry| entry.snapshot.created_at.clone())
        });
        let affected_snapshot_count = history
            .iter()
            .filter(|entry| entry.integrity_status != "healthy")
            .count() as u32;
        let data_quality = DataQualitySummary {
            current_baseline_complete: current_completeness == Some("complete"),
            unknown_enhancement_count: analysis_rows
                .iter()
                .filter(|row| row.data_quality == "unknown")
                .count() as u32,
            draft_rule_count: analysis_rows
                .iter()
                .filter(|row| row.evidence_level == "draft")
                .count() as u32,
            stale_analysis_count: analysis_rows
                .iter()
                .filter(|row| {
                    current_created_at
                        .as_deref()
                        .is_some_and(|created_at| row.generated_at.as_str() < created_at)
                })
                .count() as u32,
            affected_snapshot_count,
        };

        let attribute_distribution = attribute_counts
            .into_iter()
            .map(|(attribute_type, count)| AttributeDistributionMetric {
                attribute_type,
                count,
            })
            .collect();

        Ok(AnalysisCenter {
            profile_id: profile_id.to_owned(),
            current_baseline_snapshot_id: current_snapshot_id,
            inventory_count: present_count + unconfirmed_count,
            present_count,
            unconfirmed_count,
            removed_count,
            usage_coverage,
            attribute_distribution,
            set_structure,
            enhancement_funnel,
            cleanup_yield,
            data_quality,
        })
    }

    /// 读取单枚御魂的完整追溯链，供详情抽屉和审计导出使用。
    pub fn analysis_trace(
        &self,
        profile_id: &str,
        soul_key: &str,
    ) -> Result<AnalysisTrace, AppError> {
        let profile = self.ensure_profile(profile_id)?;
        let page = self.services.actions.list_todos(
            profile_id,
            &TodoQuery {
                category: None,
                search: Some(soul_key),
                use_id: None,
                limit: 1_000,
                offset: 0,
            },
        )?;
        let todo = page
            .items
            .into_iter()
            .find(|todo| todo.soul_key == soul_key)
            .ok_or_else(|| AppError::not_found("analysis_todo", soul_key))?;
        let snapshot = self
            .services
            .snapshots
            .get(&todo.snapshot_id)?
            .ok_or_else(|| AppError::not_found("snapshot", &todo.snapshot_id))?;
        let detail = serde_json::from_str::<Value>(&todo.detail_json).unwrap_or(Value::Null);
        let mut rule_versions = Vec::new();
        if let Some(rule_preset) = detail.get("rulePreset") {
            if let (Some(id), Some(version), Some(hash)) = (
                json_string(rule_preset, "id"),
                json_string(rule_preset, "version"),
                json_string(rule_preset, "hash"),
            ) {
                rule_versions.push(RuleTraceVersion {
                    id,
                    version,
                    title: "默认内置规则".to_owned(),
                    normalized_hash: hash,
                    enabled: true,
                });
            }
        }
        for activation in self.services.rules.list_activations(profile_id)? {
            if !activation.enabled {
                continue;
            }
            let Some(version) = self
                .services
                .rules
                .get_version(&activation.rule_version_id)?
            else {
                continue;
            };
            if rule_versions.iter().any(|item| item.id == version.id) {
                continue;
            }
            rule_versions.push(RuleTraceVersion {
                id: version.id,
                version: version.version,
                title: version.title,
                normalized_hash: version.canonical_sha256,
                enabled: true,
            });
        }

        let history = self
            .services
            .history
            .list_snapshot_history(profile_id, None)?;
        let integrity = history
            .iter()
            .find(|entry| entry.snapshot.id == snapshot.id)
            .map(|entry| {
                (
                    entry.integrity_status.clone(),
                    entry.integrity_message.clone(),
                )
            })
            .unwrap_or_else(|| ("healthy".to_owned(), None));
        let maturity_level = profile
            .maturity_level
            .map(|level| level.as_str().to_owned())
            .or_else(|| {
                detail
                    .get("uses")
                    .and_then(Value::as_array)
                    .and_then(|uses| uses.first())
                    .and_then(|use_item| json_string(use_item, "maturity"))
            });

        Ok(AnalysisTrace {
            profile_id: profile_id.to_owned(),
            soul_key: soul_key.to_owned(),
            todo,
            raw_sha256: snapshot.raw_sha256.clone(),
            catalog_version: snapshot.catalog_version.clone(),
            snapshot,
            maturity_level,
            rule_versions,
            integrity_status: integrity.0,
            integrity_message: integrity.1,
        })
    }

    /// 校验档案引用的全部原始对象；校验失败的对象由存储层隔离并回写快照状态。
    pub fn verify_profile_objects(
        &self,
        profile_id: &str,
    ) -> Result<ProfileObjectVerification, AppError> {
        self.ensure_profile(profile_id)?;
        let references = self
            .services
            .history
            .list_profile_raw_references(profile_id)?;
        let mut reports = Vec::with_capacity(references.len());
        for reference in references {
            let (status, message, suggestion) =
                match self.services.raw_object_store.verify(&reference.sha256) {
                    Ok(VerifyOutcome::Verified) => {
                        ("healthy".to_owned(), None, "对象完整，无需修复".to_owned())
                    }
                    Ok(VerifyOutcome::Missing) => (
                        "missing".to_owned(),
                        Some("内容寻址对象文件缺失".to_owned()),
                        "从已验证备份恢复原始对象，或重新导入对应快照".to_owned(),
                    ),
                    Err(error) => (
                        "corrupt".to_owned(),
                        Some(error.message.clone()),
                        "对象已隔离；请从已验证备份恢复，或重新导入对应快照".to_owned(),
                    ),
                };
            for snapshot_id in &reference.snapshot_ids {
                self.services.history.record_snapshot_integrity(
                    snapshot_id,
                    &status,
                    message.as_deref(),
                    &AppServices::now_iso(),
                )?;
            }
            reports.push(ObjectIntegrityReport {
                sha256: reference.sha256,
                status,
                message,
                affected_snapshot_ids: reference.snapshot_ids,
                repair_suggestion: suggestion,
            });
        }
        let checked_count = reports.len() as u32;
        let healthy_count = reports
            .iter()
            .filter(|report| report.status == "healthy")
            .count() as u32;
        let missing_count = reports
            .iter()
            .filter(|report| report.status == "missing")
            .count() as u32;
        let corrupt_count = reports
            .iter()
            .filter(|report| report.status == "corrupt")
            .count() as u32;
        Ok(ProfileObjectVerification {
            profile_id: profile_id.to_owned(),
            checked_count,
            healthy_count,
            missing_count,
            corrupt_count,
            reports,
        })
    }

    /// 导出单一档案、单个快照或当前筛选结果；所有查询都显式绑定 profile_id。
    pub fn export_data(
        &self,
        profile_id: &str,
        kind: &str,
        snapshot_id: Option<&str>,
        category: Option<&str>,
        search: Option<&str>,
    ) -> Result<ExportPayload, AppError> {
        let profile = self.ensure_profile(profile_id)?;
        // 所有导出类型共用同一套分类校验，避免 profile 导出因非法筛选值而静默返回空结果。
        if let Some(category) = category {
            crate::domain::validate_action_value(
                "category",
                category,
                crate::domain::TODO_CATEGORIES,
            )?;
        }
        match kind {
            "profile" => {
                let overview = self.history_overview(profile_id, 1_000)?;
                let inventory = self.services.snapshots.list_inventory(profile_id)?;
                let todos = self.list_all_todos(profile_id, category, search)?;
                let content = serde_json::to_string_pretty(&json!({
                    "format": "yys-analysis-profile-export",
                    "version": 1,
                    "profile": profile,
                    "history": overview,
                    "inventory": inventory,
                    "analysis": todos,
                }))
                .map_err(|error| AppError::internal(format!("序列化档案导出失败：{error}")))?;
                Ok(ExportPayload {
                    file_name: format!("yys-analysis-{}-profile.json", profile_id),
                    media_type: "application/json".to_owned(),
                    content,
                    profile_id: profile_id.to_owned(),
                    snapshot_id: None,
                    record_count: (overview.snapshots.len()
                        + overview.acquisition_events.len()
                        + todos.len()) as u32,
                })
            }
            "snapshot" => {
                let snapshot_id = snapshot_id.ok_or_else(|| {
                    AppError::invalid_argument("snapshotId", "导出快照时必须提供 snapshotId")
                })?;
                let snapshot = self
                    .services
                    .snapshots
                    .get(snapshot_id)?
                    .ok_or_else(|| AppError::not_found("snapshot", snapshot_id))?;
                if snapshot.profile_id != profile_id {
                    return Err(AppError::not_found("snapshot", snapshot_id));
                }
                let (souls, attributes) = self.services.imports.list_snapshot_souls(snapshot_id)?;
                let content = serde_json::to_string_pretty(&json!({
                    "format": "yys-analysis-snapshot-export",
                    "version": 1,
                    "profileId": profile_id,
                    "snapshot": snapshot,
                    "souls": souls,
                    "attributes": attributes,
                }))
                .map_err(|error| AppError::internal(format!("序列化快照导出失败：{error}")))?;
                Ok(ExportPayload {
                    file_name: format!("yys-analysis-{}-snapshot-{}.json", profile_id, snapshot_id),
                    media_type: "application/json".to_owned(),
                    content,
                    profile_id: profile_id.to_owned(),
                    snapshot_id: Some(snapshot_id.to_owned()),
                    record_count: souls.len() as u32,
                })
            }
            "analysis" => {
                let todos = self.list_all_todos(profile_id, category, search)?;
                let content = serde_json::to_string_pretty(&json!({
                    "format": "yys-analysis-filtered-analysis-export",
                    "version": 1,
                    "profileId": profile_id,
                    "filter": { "category": category, "search": search },
                    "analysis": todos,
                }))
                .map_err(|error| AppError::internal(format!("序列化分析导出失败：{error}")))?;
                Ok(ExportPayload {
                    file_name: format!("yys-analysis-{}-analysis.json", profile_id),
                    media_type: "application/json".to_owned(),
                    content,
                    profile_id: profile_id.to_owned(),
                    snapshot_id: None,
                    record_count: todos.len() as u32,
                })
            }
            _ => Err(AppError::invalid_argument(
                "kind",
                "导出类型必须是 profile、snapshot 或 analysis",
            )),
        }
    }

    /// 校验档案存在，所有公开用例统一复用这一边界。
    fn ensure_profile(&self, profile_id: &str) -> Result<crate::domain::GameProfile, AppError> {
        self.services
            .profiles
            .get(profile_id)?
            .ok_or_else(|| AppError::not_found("game_profile", profile_id))
    }

    /// 读取当前激活目录下的 PVE/PVP 常用度，用于套装结构标签。
    fn commonness_map(
        &self,
        profile_id: &str,
    ) -> Result<BTreeMap<(String, String), bool>, AppError> {
        let Some(catalog) = self.services.catalog.active_status()? else {
            return Ok(BTreeMap::new());
        };
        Ok(self
            .services
            .commonness
            .effective(profile_id, &catalog.version)
            .unwrap_or_default()
            .into_iter()
            .map(|entry| {
                (
                    (entry.set_id, entry.scenario),
                    entry.value.as_str() == "common",
                )
            })
            .collect())
    }

    /// 按分页批量读取完整待办，避免 1000 枚以上库存的导出静默截断。
    fn list_all_todos(
        &self,
        profile_id: &str,
        category: Option<&str>,
        search: Option<&str>,
    ) -> Result<Vec<AnalysisTodo>, AppError> {
        let mut offset = 0u32;
        let mut result = Vec::new();
        loop {
            let page = self.services.actions.list_todos(
                profile_id,
                &TodoQuery {
                    category,
                    search,
                    use_id: None,
                    limit: 1_000,
                    offset,
                },
            )?;
            let loaded = page.items.len() as u32;
            result.extend(page.items);
            if loaded == 0 || offset + loaded >= page.total {
                break;
            }
            offset += loaded;
        }
        Ok(result)
    }
}

/// 从待办解释树提取用途覆盖，解析失败的旧版本数据不会阻断其他指标。
fn build_usage_coverage(rows: &[AnalysisMetricRow]) -> Vec<UsageCoverageMetric> {
    let mut coverage = BTreeMap::<String, (String, u32, u32, u32, Option<String>)>::new();
    for row in rows.iter().filter(|row| row.presence_state != "removed") {
        let Some(uses) = serde_json::from_str::<Value>(&row.detail_json)
            .ok()
            .and_then(|detail| detail.get("uses").cloned())
            .and_then(|uses| uses.as_array().cloned())
        else {
            continue;
        };
        for use_item in uses {
            let Some(use_id) = json_string(&use_item, "useId") else {
                continue;
            };
            let title = json_string(&use_item, "title").unwrap_or_else(|| use_id.clone());
            let selector_matched = use_item
                .get("selectorMatched")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let recommendation = json_string(&use_item, "recommendation").unwrap_or_default();
            let entry =
                coverage
                    .entry(use_id)
                    .or_insert((title, 0, 0, 0, Some(row.category.clone())));
            // 一个用途可能同时出现在多个行动分类中，此时下钻只使用精确 use_id。
            if entry.4.as_deref() != Some(row.category.as_str()) {
                entry.4 = None;
            }
            if selector_matched && recommendation == "keep" {
                entry.1 += 1;
            } else if selector_matched && recommendation == "observe" {
                entry.2 += 1;
            } else {
                entry.3 += 1;
            }
        }
    }
    coverage
        .into_iter()
        .map(
            |(use_id, (title, usable_count, observing_count, gap_count, drill_down_category))| {
                let drill_down_search = use_id.clone();
                UsageCoverageMetric {
                    use_id,
                    title,
                    usable_count,
                    observing_count,
                    gap_count,
                    drill_down_category,
                    drill_down_search,
                }
            },
        )
        .collect()
}

/// 由系统建议状态生成可解释的强化漏斗阶段。
fn build_enhancement_funnel(rows: &[AnalysisMetricRow]) -> Vec<EnhancementFunnelStage> {
    let active = rows
        .iter()
        .filter(|row| row.presence_state != "removed")
        .collect::<Vec<_>>();
    let admitted = active
        .iter()
        .filter(|row| {
            serde_json::from_str::<Value>(&row.detail_json)
                .ok()
                .and_then(|detail| {
                    detail
                        .get("admission")
                        .and_then(|value| value.get("admitted"))
                        .and_then(Value::as_bool)
                })
                .unwrap_or(false)
        })
        .count() as u32;
    let observing = active
        .iter()
        .filter(|row| row.recommendation == "observe")
        .count() as u32;
    let finished = active
        .iter()
        .filter(|row| row.level == 15 && row.recommendation == "keep")
        .count() as u32;
    vec![
        EnhancementFunnelStage {
            stage: "admitted".to_owned(),
            count: admitted,
            drill_down_category: Some("new_embryo".to_owned()),
        },
        EnhancementFunnelStage {
            stage: "observe".to_owned(),
            count: observing,
            drill_down_category: Some("continue".to_owned()),
        },
        EnhancementFunnelStage {
            stage: "finished".to_owned(),
            count: finished,
            drill_down_category: Some("changed".to_owned()),
        },
    ]
}

/// 计算清理候选、局部快照未确认项和完整快照已确认移除项。
fn build_cleanup_yield(rows: &[AnalysisMetricRow]) -> CleanupYield {
    let cleanup = rows
        .iter()
        .filter(|row| row.recommendation == "recycle")
        .collect::<Vec<_>>();
    CleanupYield {
        candidate_count: cleanup.len() as u32,
        confirmed_removed_count: cleanup
            .iter()
            .filter(|row| row.presence_state == "removed")
            .count() as u32,
        unconfirmed_count: cleanup
            .iter()
            .filter(|row| row.presence_state == "unconfirmed")
            .count() as u32,
        pending_count: cleanup
            .iter()
            .filter(|row| row.presence_state == "present")
            .count() as u32,
    }
}

/// 从 JSON 对象读取字符串字段，避免把不可信解释文本当作代码执行。
fn json_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造指标测试行，只填充与纯统计函数相关的字段。
    fn metric_row(
        category: &str,
        recommendation: &str,
        presence_state: &str,
        detail_json: &str,
    ) -> AnalysisMetricRow {
        AnalysisMetricRow {
            soul_key: format!("soul-{category}-{recommendation}"),
            snapshot_id: "snapshot-test".to_owned(),
            category: category.to_owned(),
            recommendation: recommendation.to_owned(),
            data_quality: "complete".to_owned(),
            evidence_level: "verified".to_owned(),
            level: 15,
            presence_state: presence_state.to_owned(),
            generated_at: "2026-08-10T00:00:00Z".to_owned(),
            detail_json: detail_json.to_owned(),
        }
    }

    /// 用途同时落在多个待办分类时，统计保留精确 use_id 而不套用首条分类。
    #[test]
    fn 用途覆盖下钻不会因混合分类漏项() {
        let detail = r#"{"uses":[{"useId":"use-crit","title":"暴击输出","selectorMatched":true,"recommendation":"keep"}]}"#;
        let rows = vec![
            metric_row("new_embryo", "keep", "present", detail),
            metric_row("continue", "observe", "present", detail),
        ];

        let metrics = build_usage_coverage(&rows);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].usable_count, 2);
        assert_eq!(metrics[0].drill_down_category, None);
        assert_eq!(metrics[0].drill_down_search, "use-crit");
    }

    /// 清理收益应把局部快照中的未确认项与完整快照中的已确认移除项分开。
    #[test]
    fn 清理收益区分确认状态() {
        let detail = r#"{"uses":[]}"#;
        let rows = vec![
            metric_row("cleanup", "recycle", "removed", detail),
            metric_row("cleanup", "recycle", "unconfirmed", detail),
            metric_row("cleanup", "recycle", "present", detail),
        ];

        let yield_ = build_cleanup_yield(&rows);
        assert_eq!(yield_.candidate_count, 3);
        assert_eq!(yield_.confirmed_removed_count, 1);
        assert_eq!(yield_.unconfirmed_count, 1);
        assert_eq!(yield_.pending_count, 1);
    }
}
