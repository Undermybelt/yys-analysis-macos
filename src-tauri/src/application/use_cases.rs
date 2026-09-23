//! 应用层用例：编排领域仓库与基础设施，向命令层提供可组合的业务动作。

use crate::application::error::AppError;
use crate::application::services::AppServices;
use crate::domain::{
    AcquisitionEvent, ActionBatch, ActionBatchDetail, AnalysisTodo, AnalysisTodoPage, BackupRecord,
    BatchGroup, BatchItem, CatalogPackage, CatalogStatus, CatalogValidationResult, CommonnessValue,
    CreateBatchRequest, DecisionApplyResult, DecisionChange, DecisionHistoryEntry, DecisionPreview,
    EffectiveCommonness, EvaluationContext, GameProfile, GroupMember, MaturityLevel,
    MaturityThreshold, NewGameProfile, ProfileCommonness, RawObject, RawObjectStats,
    Recommendation, SetCommonness, Shikigami, SnapshotSoul, SoulAttribute, SoulFacts, SoulSet,
    TodoQuery, UseEvaluation, YuhunGroup, aggregate_use_evaluations, batch_group_key,
    builtin_use_templates, is_protected_todo, load_default_preset, parse_rule_preset,
};
use crate::infrastructure::database::backup::BackupValidation;
use crate::infrastructure::raw_object_store::StoredObject;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 分析待办用例：把不可变快照事实、默认规则评估和用户动作连接起来。
pub struct AnalysisUseCase {
    services: Arc<AppServices>,
}

impl AnalysisUseCase {
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 重新计算档案当前库存，并只更新系统建议，不触碰用户决定和决定历史。
    /// `snapshot_id` 用于导入完成后的增量定位；为空时根据当前库存所引用的快照计算。
    pub fn recalculate(
        &self,
        profile_id: &str,
        snapshot_id: Option<&str>,
    ) -> Result<u32, AppError> {
        self.recalculate_with_progress(profile_id, snapshot_id, |_, _| true)?
            .ok_or_else(|| AppError::internal("分析重算在提交前被取消"))
    }

    /// 执行分析重算并按御魂数量报告进度；返回 None 表示在数据库写入前被取消。
    pub fn recalculate_with_progress<F>(
        &self,
        profile_id: &str,
        snapshot_id: Option<&str>,
        mut report_progress: F,
    ) -> Result<Option<u32>, AppError>
    where
        F: FnMut(u32, u32) -> bool,
    {
        let profile = self
            .services
            .profiles
            .get(profile_id)?
            .ok_or_else(|| AppError::not_found("game_profile", profile_id))?;
        let inventory = self.services.snapshots.list_inventory(profile_id)?;
        let total = inventory
            .iter()
            .filter(|item| item.presence_state != "removed")
            .count() as u32;
        // 先报告总数，让前端可以在真正开始逐枚评分前建立稳定的百分比基线。
        if !report_progress(0, total) {
            return Ok(None);
        }
        let active_catalog = self.services.catalog.active_status()?;
        let (catalog_version, group_map, commonness_map) = if let Some(catalog) = active_catalog {
            let mut group_map: HashMap<String, Vec<String>> = HashMap::new();
            for group in self.services.catalog.list_groups()? {
                for member in self.services.catalog.group_members(&group.id)? {
                    if member.catalog_version == catalog.version {
                        group_map
                            .entry(member.set_id)
                            .or_default()
                            .push(group.id.clone());
                    }
                }
            }
            let commonness_map = self
                .services
                .commonness
                .effective(profile_id, &catalog.version)?
                .into_iter()
                .fold(
                    HashMap::<String, BTreeMap<String, crate::domain::CommonnessValue>>::new(),
                    |mut map, entry| {
                        map.entry(entry.set_id)
                            .or_default()
                            .insert(entry.scenario, entry.value);
                        map
                    },
                );
            (catalog.version, group_map, commonness_map)
        } else {
            ("unresolved".to_owned(), HashMap::new(), HashMap::new())
        };

        // 每个快照只读取一次，避免一万枚库存产生一万次重复查询。
        let mut facts_by_snapshot: HashMap<String, (Vec<SnapshotSoul>, Vec<SoulAttribute>)> =
            HashMap::new();
        for item in &inventory {
            if item.presence_state == "removed" || facts_by_snapshot.contains_key(&item.snapshot_id)
            {
                continue;
            }
            facts_by_snapshot.insert(
                item.snapshot_id.clone(),
                self.services
                    .imports
                    .list_snapshot_souls(&item.snapshot_id)?,
            );
        }

        // 评分标准由规则库的全局启用指针决定；历史数据库缺少指针时回退到内置标准。
        // 这样“我的御魂”点击计算时，使用的规则与“御魂评分标准”页面保持一致。
        // 计算入口也主动修复旧数据库中的内置指针，避免绕过规则库页面时仍按“默认标准”计算。
        let active_score_standard_id =
            crate::application::services::promote_preferred_score_standard_if_needed(
                self.services.rules.as_ref(),
            )?;
        let preset = if let Some(active_id) = active_score_standard_id {
            if let Some(version) = self.services.rules.get_version(&active_id)? {
                parse_rule_preset(
                    version.canonical_json.as_bytes(),
                    crate::domain::ValidationLimits::default(),
                )
                .map_err(|error| AppError::invalid_argument("scoreStandard", error.to_string()))?
            } else {
                load_default_preset()
                    .map_err(|error| AppError::internal(format!("默认规则预设无法加载：{error}")))?
            }
        } else {
            load_default_preset()
                .map_err(|error| AppError::internal(format!("默认规则预设无法加载：{error}")))?
        };
        let engine = crate::domain::RuleEngine::new();
        // 系统基础预设始终参与分析；档案显式启用的个人/导入版本追加参与，停用版本不读取。
        let mut active_rule_presets = Vec::new();
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
            if version.origin == "builtin" {
                continue;
            }
            active_rule_presets.push(
                parse_rule_preset(
                    version.canonical_json.as_bytes(),
                    crate::domain::ValidationLimits::default(),
                )
                .map_err(|error| AppError::invalid_argument("ruleVersion", error.to_string()))?,
            );
        }
        let mut all_facts = Vec::new();
        let mut pending = Vec::new();
        for item in inventory {
            if item.presence_state == "removed" {
                continue;
            }
            let Some((souls, attributes)) = facts_by_snapshot.get(&item.snapshot_id) else {
                continue;
            };
            let Some(soul) = souls
                .iter()
                .find(|soul| soul.internal_id == item.soul_internal_id)
            else {
                continue;
            };
            let commonness = commonness_map
                .get(&soul.set_id)
                .cloned()
                .unwrap_or_default();
            let scenario = Some("PVE".to_owned());
            let facts = SoulFacts::from_snapshot(
                soul,
                attributes,
                group_map.get(&soul.set_id).cloned().unwrap_or_default(),
                commonness,
                scenario,
            );
            all_facts.push(facts.clone());
            pending.push((item, soul.clone(), facts));
        }

        let thresholds = {
            let profile_thresholds = self.services.profiles.profile_thresholds(profile_id)?;
            if profile_thresholds.is_empty() {
                self.services.profiles.system_thresholds()?
            } else {
                profile_thresholds
            }
        };
        let complete_snapshot = snapshot_id
            .and_then(|id| self.services.snapshots.get(id).ok().flatten())
            .is_some_and(|snapshot| snapshot.completeness == "complete");
        let maturity = crate::domain::resolve_maturity(
            profile.maturity_mode,
            profile.maturity_level,
            complete_snapshot,
            &all_facts,
            &thresholds,
        )
        .unwrap_or(MaturityLevel::Growth);

        let templates = builtin_use_templates();
        let mut todos = Vec::with_capacity(pending.len());
        let mut completed = 0_u32;
        for (item, soul, facts) in pending {
            let admission = engine.evaluate_admission(&preset, &facts);
            let standard_score = engine.evaluate_standard_score(&preset, &facts);
            let custom_admissions = active_rule_presets
                .iter()
                .map(|active_preset| engine.evaluate_admission(active_preset, &facts))
                .collect::<Vec<_>>();
            let admitted =
                admission.admitted || custom_admissions.iter().any(|result| result.admitted);
            let mut candidate_uses = admission.candidate_uses.clone();
            for custom_admission in &custom_admissions {
                for candidate_use in &custom_admission.candidate_uses {
                    if !candidate_uses.contains(candidate_use) {
                        candidate_uses.push(candidate_use.clone());
                    }
                }
            }
            let mut context = EvaluationContext::from_soul(&facts);
            // 平将门模板需要显式上下文；没有队伍输入时用 0 表示“未提供”，不伪造玩家面板。
            context.set("hero.panel.initial_attack", json!(0.0));
            context.set("hero.panel.initial_defense", json!(0.0));
            let evaluations = templates
                .iter()
                .filter_map(|template| {
                    engine
                        .evaluate_use(template, &facts, &context, maturity)
                        .ok()
                })
                .collect::<Vec<UseEvaluation>>();
            let enabled_evaluations = evaluations
                .iter()
                .map(|evaluation| {
                    (
                        true,
                        evaluation.recommendation,
                        evaluation.recommendation == Recommendation::Keep
                            || evaluation.recommendation == Recommendation::Observe,
                    )
                })
                .collect::<Vec<_>>();
            let has_keep_or_observe = enabled_evaluations.iter().any(|(_, _, accepted)| *accepted);
            let has_stop_or_recycle = enabled_evaluations.iter().any(|(_, recommendation, _)| {
                matches!(
                    recommendation,
                    Recommendation::Stop | Recommendation::Recycle
                )
            });
            let aggregate = aggregate_use_evaluations(
                evaluations
                    .clone()
                    .into_iter()
                    .map(|evaluation| (true, evaluation))
                    .collect(),
            );
            // 综合评分取命中用途中的最高数字评分；暂不在“我的御魂”展示，但随本次事务一起保存。
            let composite_score = aggregate
                .uses
                .iter()
                .filter(|evaluation| evaluation.selector_matched)
                .map(|evaluation| evaluation.score)
                .max_by(f64::total_cmp);

            let enhancement_counts_known = facts
                .attributes
                .iter()
                .all(|attribute| attribute.enhancement_count.is_some());
            let data_quality = if facts.initial_substat_count.is_some() && enhancement_counts_known
            {
                "complete"
            } else if item.presence_state == "unconfirmed" {
                "partial"
            } else {
                "unknown"
            };
            let is_new = item.first_seen_snapshot_id == item.last_seen_snapshot_id;
            let is_changed =
                !is_new && snapshot_id.is_some_and(|id| item.last_seen_snapshot_id == id);
            let has_conflict = admitted && has_keep_or_observe && has_stop_or_recycle;
            let recommendation = if admitted && soul.level < 15 {
                // 胚子准入优先保证“先观察再决定”，避免占位用途评分过低直接误回收。
                "observe".to_owned()
            } else if admitted && soul.level == 15 {
                if has_keep_or_observe {
                    "keep"
                } else {
                    "recycle"
                }
                .to_owned()
            } else {
                aggregate.recommendation.as_str().to_owned()
            };
            let category = if data_quality != "complete" {
                "uncertain"
            } else if has_conflict {
                "conflict"
            } else if is_new && admitted {
                "new_embryo"
            } else if is_changed {
                "changed"
            } else if recommendation == "observe" && soul.level >= 3 {
                "await_review"
            } else if recommendation == "observe" {
                "continue"
            } else if recommendation == "stop" {
                "stop"
            } else if recommendation == "recycle" {
                "cleanup"
            } else {
                "changed"
            };
            let next_checkpoint = aggregate
                .uses
                .iter()
                .filter_map(|evaluation| evaluation.next_checkpoint)
                .min();
            let reason_summary = if category == "uncertain" {
                "强化次数或快照完整性未知，暂不把系统结论当作最终清理依据".to_owned()
            } else if admitted {
                format!(
                    "启用规则命中 {} 个候选用途{}",
                    candidate_uses.len(),
                    next_checkpoint
                        .map(|level| format!("，建议强化至 +{level} 后复评"))
                        .unwrap_or_default()
                )
            } else {
                "当前启用规则未命中候选用途".to_owned()
            };
            let detail = json!({
                "facts": &facts,
                "admission": &admission,
                "activeRuleAdmissions": &custom_admissions,
                "uses": &aggregate.uses,
                "aggregate": &aggregate,
                "standardScore": &standard_score,
                "catalogVersion": &catalog_version,
                "rulePreset": {
                    "id": preset.preset.id.clone(),
                    "version": preset.preset.version.clone(),
                    "hash": preset.normalized_hash.clone(),
                },
                "scoreStandard": {
                    "id": preset.preset.id.clone(),
                    "version": preset.preset.version.clone(),
                    "title": preset.preset.title.clone(),
                },
            });
            let detail_json = serde_json::to_string(&detail)
                .map_err(|error| AppError::internal(format!("序列化待办解释失败：{error}")))?;
            let explanation_hash = sha256_text(&detail_json);
            todos.push(AnalysisTodo {
                id: format!("{profile_id}:{}", item.soul_key),
                profile_id: profile_id.to_owned(),
                soul_key: item.soul_key,
                snapshot_id: item.snapshot_id,
                soul_internal_id: item.soul_internal_id,
                set_id: soul.set_id,
                slot: soul.slot,
                quality: soul.quality,
                level: soul.level,
                main_attribute: soul.main_attr_type,
                main_value: soul.main_attr_value,
                standard_score: standard_score.as_ref().map(|score| score.score),
                composite_score,
                category: category.to_owned(),
                recommendation,
                reason_summary,
                evidence_level: "draft".to_owned(),
                data_quality: data_quality.to_owned(),
                presence_state: item.presence_state,
                is_new,
                is_changed,
                detail_json,
                explanation_hash,
                generated_at: AppServices::now_iso(),
                revision: 1,
                user_decision: None,
                user_decision_note: None,
                decision_revision: None,
            });
            completed += 1;
            // 取消只发生在当前结果提交前，避免数据库留下半批新旧混合的评分结果。
            if !report_progress(completed, total) {
                return Ok(None);
            }
        }
        // 事实缺失的异常记录不会生成待办，但仍要结束进度，便于前端明确进入完成或失败态。
        if completed < total && !report_progress(total, total) {
            return Ok(None);
        }
        let count = todos.len() as u32;
        self.services.actions.upsert_todos(&todos)?;
        self.services.actions.sync_todo_presence(profile_id)?;
        Ok(Some(count))
    }

    /// 分页读取待办；过滤和排序留在数据库侧，详情 JSON 仅在命中的行返回。
    pub fn list_todos(
        &self,
        profile_id: &str,
        category: Option<&str>,
        search: Option<&str>,
        use_id: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<AnalysisTodoPage, AppError> {
        if let Some(category) = category {
            crate::domain::validate_action_value(
                "category",
                category,
                crate::domain::TODO_CATEGORIES,
            )?;
        }
        self.services.actions.list_todos(
            profile_id,
            &TodoQuery {
                category,
                search,
                use_id,
                limit,
                offset,
            },
        )
    }

    /// 读取当前库存卡片需要的评分摘要，完整解释仍由分析待办详情按需提供。
    pub fn list_score_summaries(
        &self,
        profile_id: &str,
    ) -> Result<Vec<crate::domain::SoulScoreSummary>, AppError> {
        self.services.actions.list_score_summaries(profile_id)
    }

    /// 读取当前页御魂的评分摘要；页面首次打开只解析 10 枚对应的解释树。
    pub fn list_score_summaries_by_keys(
        &self,
        profile_id: &str,
        soul_keys: &[String],
    ) -> Result<Vec<crate::domain::SoulScoreSummary>, AppError> {
        self.services
            .actions
            .list_score_summaries_by_keys(profile_id, soul_keys)
    }

    /// 读取评分结果总数；用于稳定判断“计算评分”是否应该显示为“重新计算评分”。
    pub fn count_score_summaries(&self, profile_id: &str) -> Result<u32, AppError> {
        self.services.actions.count_score_summaries(profile_id)
    }

    /// 生成批量决定预览，并把相同输入转换为仓库可执行的变化集合。
    pub fn preview_decisions(
        &self,
        profile_id: &str,
        soul_keys: &[String],
        decision: &str,
        respect_protection: bool,
    ) -> Result<DecisionPreview, AppError> {
        crate::domain::validate_action_value("decision", decision, crate::domain::DECISION_KINDS)?;
        let changes = soul_keys
            .iter()
            .map(|soul_key| DecisionChange {
                soul_key: soul_key.clone(),
                decision: Some(decision.to_owned()),
                note: None,
            })
            .collect::<Vec<_>>();
        self.services
            .actions
            .preview_decisions(profile_id, &changes, respect_protection)
    }

    /// 应用批量用户决定；每一批只生成一个可撤销 operation ID。
    pub fn apply_decisions(
        &self,
        profile_id: &str,
        changes: Vec<DecisionChange>,
        respect_protection: bool,
    ) -> Result<DecisionApplyResult, AppError> {
        for change in &changes {
            if let Some(decision) = &change.decision {
                crate::domain::validate_action_value(
                    "decision",
                    decision,
                    crate::domain::DECISION_KINDS,
                )?;
            }
            if change.soul_key.trim().is_empty() {
                return Err(AppError::invalid_argument("soulKey", "御魂稳定键不能为空"));
            }
        }
        let operation_id = uuid::Uuid::new_v4().to_string();
        self.services.actions.apply_decisions(
            profile_id,
            &operation_id,
            &changes,
            respect_protection,
        )
    }

    /// 撤销一个决定事务；仓库只回滚仍未被后续决定改写的条目。
    pub fn undo_decision(&self, operation_id: &str) -> Result<(), AppError> {
        self.services.actions.undo_decision(operation_id)
    }

    /// 读取单枚御魂的用户决定历史。
    pub fn list_decision_history(
        &self,
        profile_id: &str,
        soul_key: &str,
    ) -> Result<Vec<DecisionHistoryEntry>, AppError> {
        self.services
            .actions
            .list_decision_history(profile_id, soul_key)
    }

    /// 创建按游戏筛选条件分组的强化或清理批次。
    pub fn create_batch(&self, request: &CreateBatchRequest) -> Result<ActionBatch, AppError> {
        crate::domain::validate_action_value("kind", &request.kind, crate::domain::BATCH_KINDS)?;
        if request.soul_keys.is_empty() {
            return Err(AppError::invalid_argument("soulKeys", "至少选择一枚御魂"));
        }
        if request.kind == "strengthen"
            && !matches!(request.target_level, Some(3 | 6 | 9 | 12 | 15))
        {
            return Err(AppError::invalid_argument(
                "targetLevel",
                "强化批次目标节点必须是 +3、+6、+9、+12 或 +15",
            ));
        }
        let todos = self
            .list_todos(&request.profile_id, None, None, None, 1_000, 0)?
            .items
            .into_iter()
            .map(|todo| (todo.soul_key.clone(), todo))
            .collect::<HashMap<_, _>>();
        let mut selected = request
            .soul_keys
            .iter()
            .filter_map(|key| todos.get(key).cloned())
            .filter(|todo| {
                // 清理批次默认排除保护、冲突、草案证据和不完整数据。
                request.kind != "cleanup" || !is_protected_todo(todo)
            })
            .collect::<Vec<_>>();
        selected.sort_by(|left, right| {
            left.set_id
                .cmp(&right.set_id)
                .then(left.slot.cmp(&right.slot))
                .then(left.main_attribute.cmp(&right.main_attribute))
                .then(left.level.cmp(&right.level))
                .then(left.soul_key.cmp(&right.soul_key))
        });
        if selected.is_empty() {
            return Err(AppError::invalid_argument(
                "soulKeys",
                "没有符合批次保护条件的御魂",
            ));
        }
        let mut groups = Vec::<BatchGroup>::new();
        let mut items = Vec::with_capacity(selected.len());
        let batch_id = uuid::Uuid::new_v4().to_string();
        for (index, todo) in selected.iter().enumerate() {
            let group_key =
                batch_group_key(&todo.set_id, todo.slot, &todo.main_attribute, todo.level);
            if let Some(group) = groups.iter_mut().find(|group| group.group_key == group_key) {
                group.item_count += 1;
            } else {
                groups.push(BatchGroup {
                    group_key: group_key.clone(),
                    set_id: todo.set_id.clone(),
                    slot: todo.slot,
                    main_attribute: todo.main_attribute.clone(),
                    level: todo.level,
                    item_count: 1,
                });
            }
            items.push(BatchItem {
                batch_id: batch_id.clone(),
                soul_key: todo.soul_key.clone(),
                group_key,
                set_id: todo.set_id.clone(),
                slot: todo.slot,
                main_attribute: todo.main_attribute.clone(),
                level: todo.level,
                status: "pending".to_owned(),
                sort_order: index as u32,
                note: None,
                completed_at: None,
            });
        }
        let now = AppServices::now_iso();
        let batch = ActionBatch {
            id: batch_id,
            profile_id: request.profile_id.clone(),
            kind: request.kind.clone(),
            status: "active".to_owned(),
            target_level: request.target_level,
            snapshot_id: selected
                .iter()
                .map(|todo| todo.snapshot_id.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .next(),
            group_count: groups.len() as u32,
            item_count: items.len() as u32,
            completed_count: 0,
            skipped_count: 0,
            not_found_count: 0,
            created_at: now.clone(),
            updated_at: now,
            completed_at: None,
            groups,
        };
        self.services.actions.create_batch(&batch, &items)?;
        Ok(batch)
    }

    /// 列出档案批次。
    pub fn list_batches(&self, profile_id: &str) -> Result<Vec<ActionBatch>, AppError> {
        self.services.actions.list_batches(profile_id)
    }

    /// 读取批次详情。
    pub fn get_batch(
        &self,
        profile_id: &str,
        batch_id: &str,
    ) -> Result<ActionBatchDetail, AppError> {
        self.services
            .actions
            .get_batch(profile_id, batch_id)?
            .ok_or_else(|| AppError::not_found("action_batch", batch_id))
    }

    /// 标记批次条目完成、跳过或找不到，并立即更新批次汇总状态。
    pub fn update_batch_item(
        &self,
        profile_id: &str,
        batch_id: &str,
        soul_key: &str,
        status: &str,
        note: Option<&str>,
    ) -> Result<ActionBatchDetail, AppError> {
        self.services
            .actions
            .update_batch_item(profile_id, batch_id, soul_key, status, note)
    }
}

/// 对详情 JSON 计算稳定哈希，便于发现规则重算前后的解释变化。
fn sha256_text(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

/// 游戏档案用例。
pub struct ProfileUseCase {
    services: Arc<AppServices>,
}

impl ProfileUseCase {
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 创建新档案。
    pub fn create(&self, new: NewGameProfile) -> Result<GameProfile, AppError> {
        if new.display_name.trim().is_empty() {
            return Err(AppError::invalid_argument(
                "displayName",
                "档案名称不能为空",
            ));
        }
        let now = AppServices::now_iso();
        self.services.profiles.create(&new, &now)
    }

    /// 列出档案。
    pub fn list(&self, include_archived: bool) -> Result<Vec<GameProfile>, AppError> {
        self.services.profiles.list(include_archived)
    }

    /// 获取档案。
    pub fn get(&self, id: &str) -> Result<GameProfile, AppError> {
        self.services
            .profiles
            .get(id)?
            .ok_or_else(|| AppError::not_found("game_profile", id))
    }

    /// 重命名档案（乐观并发）。
    pub fn rename(
        &self,
        id: &str,
        new_name: &str,
        expected_revision: u32,
    ) -> Result<GameProfile, AppError> {
        if new_name.trim().is_empty() {
            return Err(AppError::invalid_argument(
                "displayName",
                "档案名称不能为空",
            ));
        }
        let now = AppServices::now_iso();
        self.services
            .profiles
            .rename(id, new_name, expected_revision, &now)
    }

    /// 归档/取消归档档案。
    pub fn set_archived(
        &self,
        id: &str,
        archived: bool,
        expected_revision: u32,
    ) -> Result<GameProfile, AppError> {
        let now = AppServices::now_iso();
        self.services
            .profiles
            .set_archived(id, archived, expected_revision, &now)
    }

    /// 覆盖档案成熟度阈值。
    pub fn set_maturity_thresholds(
        &self,
        profile_id: &str,
        thresholds: &[MaturityThreshold],
    ) -> Result<(), AppError> {
        self.get(profile_id)?;
        self.services
            .profiles
            .replace_profile_thresholds(profile_id, thresholds)
    }

    /// 读取档案及其系统默认的成熟度信息。
    pub fn maturity_info(&self, profile_id: &str) -> Result<MaturityInfo, AppError> {
        self.get(profile_id)?;
        let system = self.services.profiles.system_thresholds()?;
        let profile = self.services.profiles.profile_thresholds(profile_id)?;
        Ok(MaturityInfo { system, profile })
    }
}

/// 成熟度信息：系统默认 + 档案覆盖。
#[derive(Clone, Debug)]
pub struct MaturityInfo {
    pub system: Vec<MaturityThreshold>,
    pub profile: Vec<MaturityThreshold>,
}

/// 御魂目录用例。
pub struct CatalogUseCase {
    services: Arc<AppServices>,
}

/// 当前内置目录版本；内置御魂事实变化时递增，促使已有本地数据库重新安装目录。
pub const CURRENT_BUILTIN_CATALOG_VERSION: &str = "builtin-2026.08.9";
const BUILTIN_CATALOG_SOURCE: &str = "内置御魂与式神目录";
const BUILTIN_CATALOG_PARSER_VERSION: &str = "builtin-catalog-parser/1";

/// 式神目录数据包的紧凑输入结构；技能升级文本在安装前展开为 1–5 级完整效果。
///
/// 内置包和 Gitee 发布包共用这一格式，避免更新链路维护两套资料模型。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinShikigamiPackage {
    version: String,
    game_version: String,
    expected_count: u64,
    default_employment_scenes: Vec<serde_json::Value>,
    default_evidence: Vec<crate::domain::CatalogEvidence>,
    entries: Vec<BuiltinShikigamiEntry>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinShikigamiEntry {
    shikigami_id: String,
    name: String,
    rarity: String,
    sort_order: u32,
    icon_asset_id: Option<String>,
    panel_level: u8,
    awakened: bool,
    panel: serde_json::Value,
    skills: Vec<BuiltinShikigamiSkill>,
    skill_investment: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinShikigamiSkill {
    skill_id: String,
    name: String,
    #[serde(rename = "type")]
    skill_type: String,
    energy_cost: u8,
    base_effect: String,
    upgrades: Vec<Option<String>>,
}

/// 式神增量包只携带发生变化的条目；未变化条目由客户端从基础目录版本复制。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShikigamiPatchPackage {
    format: String,
    protocol_version: u32,
    base_version: String,
    version: String,
    game_version: String,
    expected_count: u64,
    default_employment_scenes: Vec<serde_json::Value>,
    default_evidence: Vec<crate::domain::CatalogEvidence>,
    upserts: Vec<BuiltinShikigamiEntry>,
    deletes: Vec<String>,
}

/// 读取并规范化紧凑式神目录；解析、版本和条目数量不符合约束时拒绝安装。
fn parse_shikigami_package(
    payload: &[u8],
    output_version: &str,
    expected_package_version: &str,
    source_label: &str,
) -> Result<Vec<Shikigami>, AppError> {
    let package: BuiltinShikigamiPackage = serde_json::from_slice(payload).map_err(|error| {
        AppError::invalid_argument(
            "catalog",
            &format!("{source_label}式神目录解析失败：{error}"),
        )
    })?;
    if package.version != expected_package_version || package.expected_count == 0 {
        return Err(AppError::invalid_argument(
            "catalog",
            &format!("{source_label}式神目录版本或声明数量不符合安装协议"),
        ));
    }
    if package.entries.len() as u64 != package.expected_count {
        return Err(AppError::invalid_argument(
            "catalog",
            &format!("{source_label}式神目录实际数量与声明数量不一致"),
        ));
    }

    normalize_shikigami_entries(
        package.entries,
        &package.default_employment_scenes,
        &package.default_evidence,
        &package.game_version,
        output_version,
    )
}

/// 将紧凑输入条目展开为数据库使用的稳定记录；内置包和增量包共享这一实现。
fn normalize_shikigami_entries(
    source_entries: Vec<BuiltinShikigamiEntry>,
    default_employment_scenes: &[serde_json::Value],
    default_evidence: &[crate::domain::CatalogEvidence],
    game_version: &str,
    output_version: &str,
) -> Result<Vec<Shikigami>, AppError> {
    let default_scenes = serde_json::to_string(default_employment_scenes)
        .map_err(|error| AppError::internal(format!("式神场景规范化失败：{error}")))?;
    let mut entries = Vec::with_capacity(source_entries.len());
    for entry in source_entries {
        let mut normalized_skills = Vec::with_capacity(entry.skills.len());
        for skill in entry.skills {
            let mut levels = Vec::with_capacity(5);
            for level in 1..=5_u8 {
                let upgrade = if level > 1 {
                    skill.upgrades.get((level - 2) as usize).cloned().flatten()
                } else {
                    None
                };
                let effect = if level == 1 {
                    skill.base_effect.clone()
                } else {
                    let changes = skill
                        .upgrades
                        .iter()
                        .take((level - 1) as usize)
                        .enumerate()
                        .filter_map(|(index, value)| {
                            value
                                .as_ref()
                                .map(|text| format!("Lv.{}：{}", index + 2, text))
                        })
                        .collect::<Vec<_>>();
                    if changes.is_empty() {
                        format!(
                            "{}\n\n本级未提供独立增量，沿用上一等级效果。",
                            skill.base_effect
                        )
                    } else {
                        format!("{}\n\n{}", skill.base_effect, changes.join("\n"))
                    }
                };
                levels.push(json!({
                    "level": level,
                    "effect": effect,
                    "upgradeEffect": upgrade,
                }));
            }
            normalized_skills.push(json!({
                "skillId": skill.skill_id,
                "name": skill.name,
                "type": skill.skill_type,
                "energyCost": skill.energy_cost,
                "levels": levels,
            }));
        }

        let mut evidence = default_evidence.to_vec();
        for item in &mut evidence {
            if let Some(url) = item.url.as_mut() {
                *url = url.replace("{id}", &entry.shikigami_id);
            }
        }
        let panel_json = serde_json::to_string(&entry.panel)
            .map_err(|error| AppError::internal(format!("式神面板规范化失败：{error}")))?;
        let skills_json = serde_json::to_string(&normalized_skills)
            .map_err(|error| AppError::internal(format!("式神技能规范化失败：{error}")))?;
        let skill_investment_json = serde_json::to_string(&entry.skill_investment)
            .map_err(|error| AppError::internal(format!("技能投入建议规范化失败：{error}")))?;
        let source_refs_json = serde_json::to_string(&evidence)
            .map_err(|error| AppError::internal(format!("式神来源规范化失败：{error}")))?;

        // 新目录统一保存中文“素材”；旧增量包若仍携带 MATERIAL，则在进入数据库前完成归一化。
        let rarity = if entry.rarity == "MATERIAL" {
            "素材".to_owned()
        } else {
            entry.rarity
        };
        entries.push(Shikigami {
            catalog_version: output_version.to_owned(),
            shikigami_id: entry.shikigami_id,
            name: entry.name,
            rarity,
            icon_asset_id: entry.icon_asset_id,
            sort_order: entry.sort_order,
            panel_level: entry.panel_level,
            awakened: entry.awakened,
            panel_json,
            skills_json,
            skill_investment_json,
            employment_scenes_json: default_scenes.clone(),
            game_version: game_version.to_owned(),
            source_refs_json,
        });
    }
    Ok(entries)
}

/// 将基础目录和增量操作合成为新的完整式神目录，失败时不会写入数据库。
fn parse_shikigami_patch(
    payload: &[u8],
    output_version: &str,
    expected_base_version: &str,
    current_entries: Vec<Shikigami>,
) -> Result<Vec<Shikigami>, AppError> {
    let package: ShikigamiPatchPackage = serde_json::from_slice(payload).map_err(|error| {
        AppError::invalid_argument("catalog", format!("外部式神增量包解析失败：{error}"))
    })?;
    if package.format != "yys-shikigami-patch"
        || package.protocol_version != crate::application::updates::SHIKIGAMI_PATCH_PROTOCOL_VERSION
        || package.base_version != expected_base_version
        || package.version != output_version
        || package.expected_count == 0
    {
        return Err(AppError::update_rejected("式神增量包版本或协议不匹配"));
    }

    let mut ids = HashSet::new();
    for id in &package.deletes {
        if id.trim().is_empty() || !ids.insert(id.clone()) {
            return Err(AppError::invalid_argument(
                "catalog",
                "式神增量包包含空的或重复的删除 ID",
            ));
        }
    }
    for entry in &package.upserts {
        if entry.shikigami_id.trim().is_empty() || !ids.insert(entry.shikigami_id.clone()) {
            return Err(AppError::invalid_argument(
                "catalog",
                "式神增量包包含空的或重复的更新 ID",
            ));
        }
    }

    let normalized_upserts = normalize_shikigami_entries(
        package.upserts,
        &package.default_employment_scenes,
        &package.default_evidence,
        &package.game_version,
        output_version,
    )?;
    let mut by_id = current_entries
        .into_iter()
        .map(|entry| (entry.shikigami_id.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    for id in package.deletes {
        if by_id.remove(&id).is_none() {
            return Err(AppError::update_rejected(format!(
                "式神增量包试图删除基础目录中不存在的 ID：{id}"
            )));
        }
    }
    for mut entry in normalized_upserts {
        entry.catalog_version = output_version.to_owned();
        by_id.insert(entry.shikigami_id.clone(), entry);
    }
    if by_id.len() as u64 != package.expected_count {
        return Err(AppError::update_rejected(format!(
            "式神增量包结果数量为 {}，声明数量为 {}",
            by_id.len(),
            package.expected_count
        )));
    }
    let mut entries = by_id.into_values().collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.sort_order
            .cmp(&right.sort_order)
            .then_with(|| left.shikigami_id.cmp(&right.shikigami_id))
    });
    Ok(entries)
}

/// 读取并规范化随应用发布的式神数据包；只做本地解析，不在运行时访问网络。
fn load_builtin_shikigami(version: &str) -> Result<Vec<Shikigami>, AppError> {
    parse_shikigami_package(
        include_str!("../data/shikigami-catalog.json").as_bytes(),
        version,
        "builtin-2026.08.8",
        "内置",
    )
}

/// 构造每个目录版本共用的系统用途分组；用户自定义分组不在更新包中覆盖。
fn builtin_groups() -> Vec<YuhunGroup> {
    crate::domain::BUILTIN_GROUPS
        .iter()
        .map(|(id, name, desc)| YuhunGroup {
            id: id.to_string(),
            origin: "system".to_owned(),
            name: name.to_string(),
            description: desc.map(|d| d.to_string()),
            parent_id: None,
            revision: 1,
        })
        .collect()
}

/// 构造指定目录版本的御魂套装、分组关联和常用度默认值。
/// 外部式神更新暂时复用本地内置御魂事实，避免目录更新破坏御魂分析链路。
fn builtin_catalog_parts(version: &str) -> (Vec<SoulSet>, Vec<GroupMember>, Vec<SetCommonness>) {
    let sets = crate::domain::BUILTIN_SOUL_SETS
        .iter()
        .map(|record| SoulSet {
            catalog_version: version.to_owned(),
            set_id: record.set_id.to_owned(),
            name: record.name.to_owned(),
            icon_asset_id: Some(crate::domain::soul_icon_asset_id(record.code)),
            category: Some(record.category.to_owned()),
            special_category: crate::domain::builtin_special_category(record.name)
                .map(str::to_owned),
            rarity_scope: None,
            two_piece_effect_json: crate::domain::builtin_effect_description(
                record.name,
                record.category,
            )
            .map(|value| json_scalar(&value)),
            four_piece_effect_json: record.four_piece_effect.map(json_scalar),
            enhancement_rule_json: Some(builtin_enhancement_rule_json()),
            source_refs_json: Some(
                serde_json::json!([
                    {
                        "source": "网易官方随机玩法概率公示",
                        "url": "https://yys.163.com/m/news/notice/20170706/25369_665793.html",
                        "evidence": "official"
                    },
                    {
                        "source": "痒痒鼠魔方公开御魂目录与图标",
                        "url": "http://yyshub.top/#/yuhun/list",
                        "evidence": "open_source_reference"
                    }
                ])
                .to_string(),
            ),
            effective_from: None,
            effective_to: None,
        })
        .collect();
    let members = crate::domain::BUILTIN_GROUP_MEMBERS
        .iter()
        .map(|(group_id, set_id)| GroupMember {
            group_id: group_id.to_string(),
            set_id: set_id.to_string(),
            catalog_version: version.to_owned(),
            origin: "system".to_owned(),
            user_override: false,
        })
        .collect();
    let commonness = crate::domain::BUILTIN_PVE_COMMONNESS
        .iter()
        .chain(crate::domain::BUILTIN_PVP_COMMONNESS.iter())
        .enumerate()
        .map(|(index, (set_id, value))| SetCommonness {
            catalog_version: version.to_owned(),
            set_id: set_id.to_string(),
            scenario: if index < crate::domain::BUILTIN_PVE_COMMONNESS.len() {
                "PVE".to_owned()
            } else {
                "PVP".to_owned()
            },
            value: CommonnessValue::from_str(value).unwrap_or(CommonnessValue::Common),
        })
        .collect();
    (sets, members, commonness)
}

impl CatalogUseCase {
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 安装内置示例目录（作为首次安装的种子数据）。
    /// 常用度数据在真实库存校准前标记为 draft。
    pub fn install_builtin(&self) -> Result<CatalogStatus, AppError> {
        // 版本递增用于让已有本地数据库重新安装本次已确认的御魂与式神目录。
        self.install_builtin_version(CURRENT_BUILTIN_CATALOG_VERSION)
    }

    /// 读取目录状态前刷新旧的内置版本；外部目录或用户自定义历史版本不被覆盖。
    pub fn ensure_current_builtin(&self) -> Result<(), AppError> {
        let Some(status) = self.active_status()? else {
            return Ok(());
        };
        let is_builtin = status.source.as_deref() == Some(BUILTIN_CATALOG_SOURCE)
            && status.parser_version.as_deref() == Some(BUILTIN_CATALOG_PARSER_VERSION);
        if is_builtin && status.version != CURRENT_BUILTIN_CATALOG_VERSION {
            // 目录效果保存在数据库中，仅修改源码不会替换已有活动版本；这里通过版本升级重新生成整套内置数据。
            self.install_builtin()?;
        }
        Ok(())
    }

    /// 以指定版本安装内置示例目录；升级测试通过不同版本观察事实层不被改写。
    pub fn install_builtin_version(&self, version: &str) -> Result<CatalogStatus, AppError> {
        let now = AppServices::now_iso();

        let groups = builtin_groups();
        let (sets, members, commonness) = builtin_catalog_parts(version);

        // 式神数据在事务前从应用内置 JSON 展开；任意条目不完整都会阻止新版本接管旧目录。
        let shikigami = load_builtin_shikigami(version)?;

        // 内置包沿用统一目录安装器，保证内置种子与外部 Gitee 包的校验和事务边界一致。
        self.install_catalog_data(
            version,
            now,
            Some(BUILTIN_CATALOG_SOURCE.to_owned()),
            BUILTIN_CATALOG_PARSER_VERSION,
            None,
            None,
            None,
            None,
            groups,
            sets,
            shikigami,
            members,
            commonness,
        )
    }

    /// 安装已通过更新清单校验的外部式神目录包；数据库事务提交前不会停用旧目录。
    pub fn install_external_package(
        &self,
        payload: &[u8],
        version: &str,
        source: &str,
        content_sha256: &str,
        signature_text: &str,
        compatible_game_version: Option<&str>,
        min_app_version: &str,
        delta_kind: Option<&str>,
        base_version: Option<&str>,
        base_content_sha256: Option<&str>,
        result_content_sha256: Option<&str>,
    ) -> Result<CatalogStatus, AppError> {
        // 更新清单的哈希已经在下载边界校验过；这里再次核对，防止未来新增调用方绕过更新用例。
        let actual_sha256 = hex::encode(Sha256::digest(payload));
        if !actual_sha256.eq_ignore_ascii_case(content_sha256) {
            return Err(AppError::update_rejected("外部目录包 SHA-256 与清单不一致"));
        }
        let active = self
            .active_status()?
            .ok_or_else(|| AppError::not_found("catalog_version", "active"))?;
        if let Some(base_version) = base_version
            && active.version != base_version
        {
            return Err(AppError::update_rejected(format!(
                "式神增量包需要基础版本 {base_version}，当前是 {}",
                active.version
            )));
        }
        if let Some(expected_base_sha256) = base_content_sha256
            && active
                .sha256
                .as_deref()
                .map(|actual| actual.eq_ignore_ascii_case(expected_base_sha256))
                != Some(true)
        {
            return Err(AppError::update_rejected(
                "式神增量包基础目录哈希与本地活动目录不一致",
            ));
        }
        let shikigami = if delta_kind == Some("patch") {
            let expected_base_version =
                base_version.ok_or_else(|| AppError::update_rejected("式神增量包缺少基础版本"))?;
            let current_entries = self
                .services
                .catalog
                .list_shikigami(expected_base_version)?;
            parse_shikigami_patch(payload, version, expected_base_version, current_entries)?
        } else if delta_kind.is_none() || delta_kind == Some("full") {
            parse_shikigami_package(payload, version, version, "外部")?
        } else {
            return Err(AppError::update_rejected("不支持的式神目录更新类型"));
        };
        let now = AppServices::now_iso();
        let groups = builtin_groups();
        let (sets, members, commonness) = builtin_catalog_parts(version);

        self.install_catalog_data(
            version,
            now,
            Some(format!("{source}·式神目录更新")),
            "external-catalog-parser/1",
            Some(signature_text.to_owned()),
            compatible_game_version.map(str::to_owned),
            Some(min_app_version.to_owned()),
            result_content_sha256,
            groups,
            sets,
            shikigami,
            members,
            commonness,
        )
    }

    /// 激活已安装的历史目录版本；回退只切换数据库指针，不重新解析远程资料。
    pub fn activate_version(&self, version: &str) -> Result<CatalogStatus, AppError> {
        self.services.catalog.activate_version(version)?;
        self.active_status()?
            .ok_or_else(|| AppError::internal("目录版本激活后应存在活跃版本"))
    }

    /// 统一完成目录校验、哈希摘要构造和数据库事务安装；所有资料源共享这一条深接口。
    #[allow(clippy::too_many_arguments)]
    fn install_catalog_data(
        &self,
        version: &str,
        now: String,
        source: Option<String>,
        parser_version: &str,
        signature_text: Option<String>,
        compatible_game_version: Option<String>,
        min_app_version: Option<String>,
        expected_result_sha256: Option<&str>,
        groups: Vec<YuhunGroup>,
        sets: Vec<SoulSet>,
        shikigami: Vec<Shikigami>,
        members: Vec<GroupMember>,
        commonness: Vec<SetCommonness>,
    ) -> Result<CatalogStatus, AppError> {
        // 新目录必须整体通过静态校验，任何一条损坏资料都会保留旧活动版本。
        let validation = validate_catalog_payload(&sets);
        let shikigami_validation = validate_shikigami_payload(&shikigami);
        if !validation.valid || !shikigami_validation.valid {
            let mut errors = validation.errors;
            errors.extend(shikigami_validation.errors);
            return Err(AppError::invalid_argument(
                "catalog",
                &format!("御魂与式神目录校验失败：{}", errors.join("；")),
            ));
        }

        let canonical_payload = serde_json::to_vec(&json!({
            "version": version,
            "groups": &groups,
            "sets": &sets,
            "shikigami": &shikigami,
            "members": &members,
            "commonness": &commonness,
        }))
        .map_err(|error| AppError::internal(format!("目录包规范化失败：{error}")))?;
        let content_sha256 = hex::encode(Sha256::digest(&canonical_payload));
        if let Some(expected_result_sha256) = expected_result_sha256
            && !content_sha256.eq_ignore_ascii_case(expected_result_sha256)
        {
            return Err(AppError::update_rejected(
                "应用式神增量补丁后的目录哈希与清单不一致",
            ));
        }
        let package = CatalogPackage {
            version: version.to_owned(),
            installed_at: now,
            source,
            sha256: Some(content_sha256),
            parser_version: Some(parser_version.to_owned()),
            // 当前目录以可培养条目的满级基础面板为核验口径；素材条目已按独立类型计入完整性覆盖。
            validation_status: if shikigami_validation.panel_coverage
                == shikigami_validation.shikigami_count
            {
                "verified".to_owned()
            } else {
                "partial".to_owned()
            },
            validation_errors_json: None,
            icon_coverage: validation.icon_coverage,
            mechanics_coverage: validation.mechanics_coverage,
            shikigami_count: shikigami_validation.shikigami_count,
            panel_coverage: shikigami_validation.panel_coverage,
            skill_coverage: shikigami_validation.skill_coverage,
            signature_text,
            compatible_game_version,
            min_app_version,
            active: true,
        };

        self.services.catalog.install(
            &package,
            &groups,
            &sets,
            &shikigami,
            &members,
            &commonness,
        )?;

        self.active_status()?
            .ok_or_else(|| AppError::internal("目录安装后应存在活跃版本"))
    }

    /// 当前活跃目录状态。
    pub fn active_status(&self) -> Result<Option<CatalogStatus>, AppError> {
        self.services.catalog.active_status()
    }

    /// 按版本列出套装。
    pub fn list_sets(&self, catalog_version: &str) -> Result<Vec<SoulSet>, AppError> {
        self.services.catalog.list_sets(catalog_version)
    }

    /// 按目录版本列出式神，详情 JSON 由接口层解码为稳定桌面契约。
    pub fn list_shikigami(&self, catalog_version: &str) -> Result<Vec<Shikigami>, AppError> {
        self.services.catalog.list_shikigami(catalog_version)
    }

    /// 合并后的有效常用度。
    pub fn effective_commonness(
        &self,
        profile_id: &str,
    ) -> Result<Vec<EffectiveCommonness>, AppError> {
        let version = self
            .active_status()?
            .ok_or_else(|| AppError::internal("尚未安装御魂目录"))?
            .version;
        self.services.commonness.effective(profile_id, &version)
    }

    /// 设置档案常用度覆盖。
    pub fn set_commonness_override(
        &self,
        profile_id: &str,
        set_id: &str,
        scenario: &str,
        value: &str,
    ) -> Result<(), AppError> {
        if scenario != "PVE" && scenario != "PVP" {
            return Err(AppError::invalid_argument(
                "scenario",
                "场景必须是 PVE 或 PVP",
            ));
        }
        let value = CommonnessValue::from_str(value).ok_or_else(|| {
            AppError::invalid_argument("value", "常用度必须是 common 或 uncommon")
        })?;
        self.services
            .profiles
            .get(profile_id)?
            .ok_or_else(|| AppError::not_found("game_profile", profile_id))?;

        let catalog_version = self
            .active_status()?
            .ok_or_else(|| AppError::internal("尚未安装御魂目录"))?
            .version;
        self.services
            .catalog
            .get_set(&catalog_version, set_id)?
            .ok_or_else(|| AppError::not_found("soul_set", set_id))?;

        // 只保存相对默认值的覆盖；用户切回默认值时删除覆盖行，保证
        // `isOverride` 准确表达“档案确实偏离了全局设置”。
        let default_value = self
            .services
            .commonness
            .defaults(&catalog_version)?
            .into_iter()
            .find(|entry| entry.set_id == set_id && entry.scenario == scenario)
            .map(|entry| entry.value)
            .ok_or_else(|| AppError::not_found("set_commonness_default", set_id))?;
        if default_value == value {
            self.services
                .commonness
                .delete_override(profile_id, set_id, scenario)
        } else {
            self.services
                .commonness
                .upsert_override(&ProfileCommonness {
                    profile_id: profile_id.to_owned(),
                    set_id: set_id.to_owned(),
                    scenario: scenario.to_owned(),
                    value,
                })
        }
    }

    /// 清除档案常用度覆盖，恢复默认。
    pub fn clear_commonness_override(
        &self,
        profile_id: &str,
        set_id: &str,
        scenario: &str,
    ) -> Result<(), AppError> {
        self.services
            .commonness
            .delete_override(profile_id, set_id, scenario)
    }

    /// 将源档案的常用度覆盖复制到目标档案。
    pub fn copy_commonness(&self, from_profile: &str, to_profile: &str) -> Result<usize, AppError> {
        self.get_profile(from_profile)?;
        self.get_profile(to_profile)?;
        self.services
            .commonness
            .copy_overrides(from_profile, to_profile)
    }

    fn get_profile(&self, id: &str) -> Result<GameProfile, AppError> {
        self.services
            .profiles
            .get(id)?
            .ok_or_else(|| AppError::not_found("game_profile", id))
    }
}

/// 原始对象与采集用例。
pub struct StorageUseCase {
    services: Arc<AppServices>,
}

/// 一次原始对象入库的摘要。
#[derive(Clone, Debug)]
pub struct StoredRawSummary {
    pub raw_object: RawObject,
    pub stored: StoredObject,
    pub is_new: bool,
}

impl StorageUseCase {
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 保存原始字节并写入采集事件；相同内容只保留一份对象。
    pub fn store_raw(
        &self,
        profile_id: &str,
        source_kind: &str,
        source_format: &str,
        media_type: &str,
        bytes: &[u8],
        received_at: &str,
    ) -> Result<StoredRawSummary, AppError> {
        self.services
            .profiles
            .get(profile_id)?
            .ok_or_else(|| AppError::not_found("game_profile", profile_id))?;

        let (sha256, stored) = self.services.raw_object_store.store(bytes, media_type)?;
        let existing = self.services.raw_objects.get(&sha256)?;
        let is_new = existing.is_none();

        let raw_object = if let Some(existing) = existing {
            existing
        } else {
            let created_at = AppServices::now_iso();
            let obj = RawObject {
                sha256: sha256.clone(),
                relative_path:
                    crate::infrastructure::raw_object_store::RawObjectStore::relative_path_for(
                        &sha256,
                    ),
                compression: "zstd".to_owned(),
                media_type: media_type.to_owned(),
                raw_size: stored.raw_size,
                stored_size: stored.stored_size,
                verified_at: Some(created_at.clone()),
                created_at,
            };
            self.services.raw_objects.upsert(&obj)?;
            obj
        };

        // 写入采集事件（即使内容未变化也记录）
        let event = AcquisitionEvent {
            id: uuid::Uuid::new_v4().to_string(),
            profile_id: profile_id.to_owned(),
            source_kind: source_kind.to_owned(),
            source_format: source_format.to_owned(),
            raw_sha256: Some(sha256),
            captured_at: None,
            received_at: received_at.to_owned(),
            game_version: None,
            adapter_version: None,
            parser_version: env!("CARGO_PKG_VERSION").to_owned(),
            completeness: "unknown".to_owned(),
            scope_json: None,
            result_kind: "error_free".to_owned(),
            snapshot_id: None,
        };
        self.services.events.insert(&event)?;

        Ok(StoredRawSummary {
            raw_object,
            stored,
            is_new,
        })
    }

    /// 校验对象哈希完整性。
    pub fn verify_object(&self, sha256: &str) -> Result<(), AppError> {
        match self.services.raw_object_store.verify(sha256)? {
            crate::infrastructure::raw_object_store::VerifyOutcome::Verified => Ok(()),
            crate::infrastructure::raw_object_store::VerifyOutcome::Missing => {
                Err(AppError::internal(format!("原始对象文件缺失：{sha256}")))
            }
        }
    }

    /// 原始对象统计。
    pub fn raw_stats(&self) -> Result<RawObjectStats, AppError> {
        self.services.raw_objects.stats()
    }
}

/// 备份用例。
pub struct BackupUseCase {
    services: Arc<AppServices>,
}

impl BackupUseCase {
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 创建备份并记录。
    pub fn create_backup(&self, reason: &str) -> Result<BackupRecord, AppError> {
        let backup_path = self.services.backup_manager.create_backup()?;
        let validation = self
            .services
            .backup_manager
            .validate(&backup_path, &self.services.runner)?;

        let record = BackupRecord {
            id: uuid::Uuid::new_v4().to_string(),
            path: backup_path.display().to_string(),
            schema_version: validation.schema_version,
            file_count: validation.file_count,
            total_size: validation.total_size,
            reason: reason.to_owned(),
            checksum: validation.checksum,
            created_at: AppServices::now_iso(),
            restored_at: None,
            verified: validation.passed,
        };
        self.services.backup_records.insert(&record)?;
        Ok(record)
    }

    /// 创建并校验新备份后删除更早的同原因备份；清理失败不应阻断已经具备安全备份的更新流程。
    pub fn create_backup_retaining_latest(&self, reason: &str) -> Result<BackupRecord, AppError> {
        let latest = self.create_backup(reason)?;
        if let Err(error) = self.retain_only_backup_for_reason(reason, &latest.id) {
            tracing::warn!(
                backup_id = %latest.id,
                reason,
                error = %error,
                "清理旧自动更新备份失败，将在下次更新时重试"
            );
        }
        Ok(latest)
    }

    /// 列出备份记录。
    pub fn list_backups(&self) -> Result<Vec<BackupRecord>, AppError> {
        self.services.backup_records.list()
    }

    /// 只保留指定原因和 ID 的最新备份，其他原因的手动或回退备份不参与清理。
    fn retain_only_backup_for_reason(&self, reason: &str, keep_id: &str) -> Result<(), AppError> {
        for record in self.services.backup_records.list()? {
            if record.reason != reason || record.id == keep_id {
                continue;
            }
            self.services
                .backup_manager
                .delete_backup(Path::new(&record.path))?;
            self.services.backup_records.delete(&record.id)?;
        }
        Ok(())
    }

    /// 校验备份记录文件。
    pub fn validate_backup(&self, id: &str) -> Result<BackupValidation, AppError> {
        let record = self.find_backup(id)?;
        let path = PathBuf::from(&record.path);
        self.services
            .backup_manager
            .validate(&path, &self.services.runner)
    }

    /// 恢复备份：关闭连接、校验、替换文件、重新打开并迁移。
    pub fn restore_backup(&self, id: &str) -> Result<(), AppError> {
        let record = self.find_backup(id)?;
        let path = PathBuf::from(&record.path);

        // 先校验，失败时保持当前数据库不变
        self.services
            .backup_manager
            .validate(&path, &self.services.runner)?;

        // 先合并 WAL，确保主数据库文件包含全部已提交事实，再关闭连接替换文件。
        self.services.db.checkpoint()?;
        self.services.db.close();
        if let Err(error) = self
            .services
            .backup_manager
            .restore(&path, &self.services.runner)
        {
            let _ = self.services.db.reopen();
            return Err(error);
        }

        // 重新打开并补充迁移
        self.services.db.reopen()?;
        self.services.db.write("migrate_after_restore", |conn| {
            self.services.runner.run(conn)
        })?;

        let now = AppServices::now_iso();
        self.services.backup_records.mark_restored(id, &now)?;
        Ok(())
    }

    fn find_backup(&self, id: &str) -> Result<BackupRecord, AppError> {
        self.services
            .backup_records
            .list()?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| AppError::not_found("backup_record", id))
    }
}

/// 校验内置目录的结构约束；标准套装必须有四件套说明，特殊类别必须有类别规则说明。
/// 图标覆盖率按稳定图标编号去重，强化覆盖率要求节点、概率和高档基准同时存在。
pub fn validate_catalog_payload(sets: &[SoulSet]) -> CatalogValidationResult {
    let expected_count = crate::domain::BUILTIN_SOUL_SETS.len() as u64;
    let mut errors = Vec::new();
    let mut set_ids = HashSet::new();
    let mut icon_ids = HashSet::new();
    let mut icon_coverage = 0_u64;
    let mut mechanics_coverage = 0_u64;

    if sets.len() as u64 != expected_count {
        errors.push(format!(
            "套装数量应为 {expected_count}，实际为 {}",
            sets.len()
        ));
    }

    for set in sets {
        if set.set_id.trim().is_empty() {
            errors.push("存在缺少稳定 ID 的套装".to_owned());
        } else if !set_ids.insert(set.set_id.as_str()) {
            errors.push(format!("稳定 ID 重复：{}", set.set_id));
        }
        if set.name.trim().is_empty() {
            errors.push(format!("套装 {} 缺少名称", set.set_id));
        }
        match set.icon_asset_id {
            Some(icon_id) if icon_ids.insert(icon_id) => icon_coverage += 1,
            Some(icon_id) => errors.push(format!("图标编号重复：{icon_id}")),
            None => errors.push(format!("套装 {} 缺少图标编号", set.set_id)),
        }

        // special_category 与标准四件套字段分离，首领和星痕按各自类别规则校验。
        let is_special = set.special_category.is_some();
        if is_special {
            if !json_scalar_present(&set.two_piece_effect_json) {
                errors.push(format!("特殊类别 {} 缺少类别规则说明", set.set_id));
            }
        } else if !json_scalar_present(&set.four_piece_effect_json) {
            errors.push(format!("标准套装 {} 缺少四件套说明", set.set_id));
        }

        if enhancement_rule_is_complete(&set.enhancement_rule_json) {
            mechanics_coverage += 1;
        } else {
            errors.push(format!("套装 {} 缺少完整强化机制", set.set_id));
        }
    }

    let content_sha256 = serde_json::to_vec(sets)
        .map(|payload| hex::encode(Sha256::digest(payload)))
        .unwrap_or_default();
    CatalogValidationResult {
        valid: errors.is_empty(),
        errors,
        set_count: sets.len() as u64,
        icon_coverage,
        mechanics_coverage,
        shikigami_count: 0,
        panel_coverage: 0,
        skill_coverage: 0,
        content_sha256,
    }
}

/// 校验式神目录的稳定 ID 与稀有度；可培养条目继续校验面板、技能、黑蛋、场景和来源证据，
/// 素材条目只校验目录身份并以数量型资料条目纳入完整覆盖；兼容旧目录中的 MATERIAL 值。
/// 只有完整覆盖 273 个可培养条目与 4 个素材条目时，内置目录才允许替换旧版本。
pub fn validate_shikigami_payload(entries: &[Shikigami]) -> CatalogValidationResult {
    const EXPECTED_COUNT: u64 = 277;
    let mut errors = Vec::new();
    let mut ids = HashSet::new();
    let mut panel_coverage = 0_u64;
    let mut skill_coverage = 0_u64;
    // 目录完整性只要求中文“素材”出现一次；MATERIAL 仅作为旧包输入别名，不应造成第二个稀有度缺失错误。
    let required_rarities = ["UR", "SP", "SSR", "SR", "R", "N", "素材"];
    let accepted_rarities = [
        "UR", "SP", "SSR", "SR", "R", "N", "素材", "MATERIAL",
    ];

    if entries.len() as u64 != EXPECTED_COUNT {
        errors.push(format!(
            "式神数量应为 {EXPECTED_COUNT}，实际为 {}",
            entries.len()
        ));
    }
    for rarity in required_rarities {
        if !entries.iter().any(|entry| entry.rarity == rarity) {
            errors.push(format!("缺少 {rarity} 稀有度式神数据"));
        }
    }

    for entry in entries {
        if entry.shikigami_id.trim().is_empty() {
            errors.push("存在缺少稳定 ID 的式神".to_owned());
        } else if !ids.insert(entry.shikigami_id.as_str()) {
            errors.push(format!("式神稳定 ID 重复：{}", entry.shikigami_id));
        }
        if entry.name.trim().is_empty() {
            errors.push(format!("式神 {} 缺少名称", entry.shikigami_id));
        }
        if !accepted_rarities.contains(&entry.rarity.as_str()) {
            errors.push(format!(
                "式神 {} 稀有度非法：{}",
                entry.shikigami_id, entry.rarity
            ));
        }
        if matches!(entry.rarity.as_str(), "素材" | "MATERIAL") {
            // 素材条目没有等级、面板和技能；先校验稳定身份，再按目录条目完整纳入覆盖率。
            panel_coverage += 1;
            skill_coverage += 1;
            continue;
        }
        // 满级面板不依赖觉醒形态；即使官方接口没有单独的觉醒记录，只要数值字段完整也允许入包。
        if entry.panel_level != 40 {
            errors.push(format!("式神 {} 缺少 40 级基础面板", entry.shikigami_id));
        }
        let panel_ok = serde_json::from_str::<serde_json::Value>(&entry.panel_json)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .map(|panel| {
                let numeric_ok = ["attack", "hp", "defense", "speed", "critRate", "critDamage"]
                    .iter()
                    .all(|key| panel.get(*key).and_then(|value| value.as_f64()).is_some());
                let grades_ok = panel
                    .get("grades")
                    .and_then(|value| value.as_object())
                    .map(|grades| {
                        ["attack", "hp", "defense", "speed", "critRate"]
                            .iter()
                            .all(|key| {
                                matches!(
                                    grades.get(*key).and_then(|value| value.as_str()),
                                    Some("SSS" | "SS" | "S" | "A" | "B" | "C" | "D")
                                )
                            })
                    })
                    .unwrap_or(false);
                numeric_ok && grades_ok
            })
            .unwrap_or(false);
        if panel_ok {
            panel_coverage += 1;
        } else {
            errors.push(format!("式神 {} 面板字段不完整", entry.shikigami_id));
        }

        let skills = serde_json::from_str::<Vec<serde_json::Value>>(&entry.skills_json);
        let skills_ok = skills
            .as_ref()
            .map(|skills| {
                !skills.is_empty()
                    && skills.iter().all(|skill| {
                        let levels = skill.get("levels").and_then(|value| value.as_array());
                        levels
                            .map(|levels| {
                                levels.len() == 5
                                    && levels.iter().enumerate().all(|(index, level)| {
                                        level.get("level").and_then(|value| value.as_u64())
                                            == Some((index + 1) as u64)
                                            && level
                                                .get("effect")
                                                .and_then(|value| value.as_str())
                                                .map(|text| !text.trim().is_empty())
                                                .unwrap_or(false)
                                    })
                            })
                            .unwrap_or(false)
                    })
            })
            .unwrap_or(false);
        let investment_ok = skills
            .as_ref()
            .ok()
            .and_then(|skills| {
                serde_json::from_str::<serde_json::Value>(&entry.skill_investment_json)
                    .ok()
                    .map(|value| (skills, value))
            })
            .map(|(skills, investment)| {
                let targets = investment
                    .get("targetLevels")
                    .and_then(|value| value.as_array());
                let priority = investment
                    .get("priority")
                    .and_then(|value| value.as_array());
                let black_eggs = investment
                    .get("blackEggsFrom111")
                    .and_then(|value| value.as_u64());
                let Some(targets) = targets else { return false };
                let Some(priority) = priority else {
                    return false;
                };
                let Some(black_eggs) = black_eggs else {
                    return false;
                };
                let calculated = targets
                    .iter()
                    .filter_map(|target| target.as_u64())
                    .map(|target| target.saturating_sub(1))
                    .sum::<u64>();
                targets.len() == skills.len()
                    && priority.len() == skills.len()
                    && targets
                        .iter()
                        .all(|target| matches!(target.as_u64(), Some(1..=5)))
                    && calculated == black_eggs
            })
            .unwrap_or(false);
        if skills_ok && investment_ok {
            skill_coverage += 1;
        } else {
            errors.push(format!(
                "式神 {} 技能或技能投入数据不完整",
                entry.shikigami_id
            ));
        }

        let scenes_ok =
            serde_json::from_str::<Vec<serde_json::Value>>(&entry.employment_scenes_json)
                .ok()
                .map(|scenes| {
                    !scenes.is_empty()
                        && scenes.iter().all(|scene| {
                            matches!(
                                scene.get("mode").and_then(|value| value.as_str()),
                                Some("PVE") | Some("PVP")
                            ) && scene
                                .get("scene")
                                .and_then(|value| value.as_str())
                                .map(|text| !text.trim().is_empty())
                                .unwrap_or(false)
                        })
                })
                .unwrap_or(false);
        if !scenes_ok {
            errors.push(format!("式神 {} 缺少合法就业场景", entry.shikigami_id));
        }
        let evidence_ok =
            serde_json::from_str::<Vec<crate::domain::CatalogEvidence>>(&entry.source_refs_json)
                .ok()
                .map(|evidence| {
                    !evidence.is_empty()
                        && evidence.iter().all(|item| {
                            !item.source.trim().is_empty() && !item.evidence.trim().is_empty()
                        })
                })
                .unwrap_or(false);
        if !evidence_ok {
            errors.push(format!("式神 {} 缺少来源证据", entry.shikigami_id));
        }
    }

    let content_sha256 = serde_json::to_vec(entries)
        .map(|payload| hex::encode(Sha256::digest(payload)))
        .unwrap_or_default();
    CatalogValidationResult {
        valid: errors.is_empty(),
        errors,
        set_count: 0,
        icon_coverage: 0,
        mechanics_coverage: 0,
        shikigami_count: entries.len() as u64,
        panel_coverage,
        skill_coverage,
        content_sha256,
    }
}

/// 兼容当前数据库中的 JSON 字符串标量，并拒绝空描述。
fn json_scalar_present(value: &Option<String>) -> bool {
    value
        .as_deref()
        .map(|raw| {
            serde_json::from_str::<String>(raw)
                .map(|text| !text.trim().is_empty())
                .unwrap_or_else(|_| !raw.trim().is_empty())
        })
        .unwrap_or(false)
}

/// 检查强化机制的关键事实，避免把未知概率或缺节点的数据标记为已核验。
fn enhancement_rule_is_complete(value: &Option<String>) -> bool {
    let Some(raw) = value.as_deref() else {
        return false;
    };
    let Ok(rule) = serde_json::from_str::<crate::domain::EnhancementRule>(raw) else {
        return false;
    };
    if rule.checkpoints != [3, 6, 9, 12, 15] {
        return false;
    }
    if (rule.four_substat_mode.per_attribute_probability - 0.25).abs() > f64::EPSILON {
        return false;
    }
    let probabilities = rule
        .incomplete_mode
        .categories
        .iter()
        .map(|category| category.probability)
        .collect::<Vec<_>>();
    if probabilities.len() != 3
        || (probabilities[0] - 0.36).abs() > f64::EPSILON
        || (probabilities[1] - 0.36).abs() > f64::EPSILON
        || (probabilities[2] - 0.28).abs() > f64::EPSILON
    {
        return false;
    }
    !rule.roll_benchmarks.is_empty()
}

/// 生成 JSON 字符串标量。
fn json_scalar(value: &str) -> String {
    serde_json::json!(value).to_string()
}

/// 生成随目录版本保存的强化机械规则；官方只公示类别概率，未披露的单属性概率保持未知。
fn builtin_enhancement_rule_json() -> String {
    serde_json::json!({
        "mechanicsVersion": "official-2017.07-benchmark-v1",
        "checkpoints": [3, 6, 9, 12, 15],
        "fourSubstatMode": {
            "kind": "uniformExisting",
            "perAttributeProbability": 0.25,
            "description": "已有四条副属性时，每条在下一个强化节点被选中的概率均为25%。"
        },
        "incompleteMode": {
            "kind": "addNewSubstat",
            "description": "不足四条副属性时，下一个强化节点新增一条副属性，不提升已有属性。",
            "categories": [
                { "id": "attack", "label": "攻击类", "probability": 0.36, "attributes": ["attack_flat", "attack_rate", "crit_rate", "crit_damage"] },
                { "id": "defense", "label": "防御类", "probability": 0.36, "attributes": ["hp_flat", "defense_flat", "hp_rate", "defense_rate"] },
                { "id": "utility", "label": "功能类", "probability": 0.28, "attributes": ["effect_resist", "effect_hit", "speed"] }
            ]
        },
        "rollBenchmarks": [
            { "attributeId": "hp_flat", "label": "生命", "highValue": 114.0, "unit": "flat" },
            { "attributeId": "defense_flat", "label": "防御", "highValue": 5.0, "unit": "flat" },
            { "attributeId": "attack_flat", "label": "攻击", "highValue": 27.0, "unit": "flat" },
            { "attributeId": "hp_rate", "label": "生命加成", "highValue": 0.03, "unit": "rate" },
            { "attributeId": "defense_rate", "label": "防御加成", "highValue": 0.03, "unit": "rate" },
            { "attributeId": "attack_rate", "label": "攻击加成", "highValue": 0.03, "unit": "rate" },
            { "attributeId": "speed", "label": "速度", "highValue": 3.0, "unit": "flat" },
            { "attributeId": "crit_rate", "label": "暴击", "highValue": 0.03, "unit": "rate" },
            { "attributeId": "crit_damage", "label": "暴击伤害", "highValue": 0.04, "unit": "rate" },
            { "attributeId": "effect_hit", "label": "效果命中", "highValue": 0.04, "unit": "rate" },
            { "attributeId": "effect_resist", "label": "效果抵抗", "highValue": 0.04, "unit": "rate" }
        ],
        "probabilityNote": "官方仅公示不足四条时的属性类别概率，没有公示类别内每个具体属性的独立概率。高档数值用于归一化展示，不代表出现概率。",
        "sourceRefs": [
            "https://yys.163.com/m/news/notice/20170706/25369_665793.html",
            "https://github.com/nguaduot/yys-pick-dwarf/blob/fb3add247c44bd9f852631455c5370fab31df502/pick_dwarf.py"
        ]
    })
    .to_string()
}

/// 供测试与内部使用的辅助函数：构造成熟度阈值。
pub fn make_threshold(level: MaturityLevel, min_count: u64) -> MaturityThreshold {
    MaturityThreshold {
        scope_type: "profile".to_owned(),
        profile_id: String::new(),
        level,
        min_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// 构造与内置包一致的最小套装集合，测试只关注目录校验，不触碰 SQLite。
    fn builtin_sets() -> Vec<SoulSet> {
        crate::domain::BUILTIN_SOUL_SETS
            .iter()
            .map(|record| SoulSet {
                catalog_version: "test".to_owned(),
                set_id: record.set_id.to_owned(),
                name: record.name.to_owned(),
                icon_asset_id: Some(crate::domain::soul_icon_asset_id(record.code)),
                category: Some(record.category.to_owned()),
                special_category: crate::domain::builtin_special_category(record.name)
                    .map(str::to_owned),
                rarity_scope: None,
                two_piece_effect_json: crate::domain::builtin_effect_description(
                    record.name,
                    record.category,
                )
                .map(|value| json_scalar(&value)),
                four_piece_effect_json: record.four_piece_effect.map(json_scalar),
                enhancement_rule_json: Some(builtin_enhancement_rule_json()),
                source_refs_json: None,
                effective_from: None,
                effective_to: None,
            })
            .collect()
    }

    #[test]
    fn 内置目录七十套全部通过校验() {
        let result = validate_catalog_payload(&builtin_sets());
        assert!(result.valid, "校验错误：{:?}", result.errors);
        assert_eq!(result.set_count, 70);
        assert_eq!(result.icon_coverage, 70);
        assert_eq!(result.mechanics_coverage, 70);
        assert!(!result.content_sha256.is_empty());

        // 这批新御魂均为标准四件套，不能被回退成不存在的“单件特殊御魂”类别。
        let ordinary_four_piece_names = [
            "片叶之苇",
            "尘冢",
            "油赤子",
            "夜啼石",
            "夜送犬",
            "雨降",
            "恶楼",
            "贝吹坊",
            "出世螺",
            "火之车",
            "叠叩",
            "应声虫",
            "元兴寺",
            "钓瓶火",
            "无刀取",
            "奉海图",
        ];
        for name in ordinary_four_piece_names {
            let set = builtin_sets()
                .into_iter()
                .find(|set| set.name == name)
                .expect("应存在已确认的普通四件套");
            assert!(set.special_category.is_none(), "{name} 不应有特殊类别");
            assert!(
                set.four_piece_effect_json.is_some(),
                "{name} 应有四件套说明"
            );
        }
    }

    #[test]
    fn 标准套装缺少四件套说明时拒绝() {
        let mut sets = builtin_sets();
        sets.iter_mut()
            .find(|set| set.category.as_deref() == Some("攻击加成"))
            .expect("应存在标准套装")
            .four_piece_effect_json = None;
        let result = validate_catalog_payload(&sets);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|error| error.contains("四件套")));
    }

    #[test]
    fn 首领与星痕类别允许没有四件套但必须有类别说明() {
        let mut sets = builtin_sets();
        for set in &mut sets {
            if matches!(
                set.special_category.as_deref(),
                Some("首领御魂" | "星痕御魂")
            ) {
                assert!(set.four_piece_effect_json.is_none());
                assert!(set.two_piece_effect_json.is_some());
            }
        }
        let valid = validate_catalog_payload(&sets);
        assert!(
            valid.valid,
            "特殊类别不应被误判为缺少四件套：{:?}",
            valid.errors
        );

        sets.iter_mut()
            .find(|set| set.special_category.as_deref() == Some("星痕御魂"))
            .expect("应存在星痕套装")
            .two_piece_effect_json = None;
        let invalid = validate_catalog_payload(&sets);
        assert!(!invalid.valid);
        assert!(
            invalid
                .errors
                .iter()
                .any(|error| error.contains("类别规则"))
        );
    }

    #[test]
    fn 缺少稳定标识图标或强化机制时整体拒绝() {
        let mut sets = builtin_sets();
        sets[0].set_id.clear();
        sets[1].icon_asset_id = None;
        sets[2].enhancement_rule_json = None;
        let result = validate_catalog_payload(&sets);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|error| error.contains("稳定 ID")));
        assert!(result.errors.iter().any(|error| error.contains("图标编号")));
        assert!(result.errors.iter().any(|error| error.contains("强化机制")));
    }

    #[test]
    fn 内置式神目录七种稀有度与属性面板全部通过校验() {
        // 直接读取发布包并走与安装流程相同的展开和校验，防止测试只验证手工拼接的假数据。
        let entries = load_builtin_shikigami("builtin-2026.08.8").expect("内置式神目录应可展开");
        let result = validate_shikigami_payload(&entries);
        assert!(result.valid, "式神目录校验错误：{:?}", result.errors);
        assert_eq!(result.shikigami_count, 277);
        assert_eq!(result.panel_coverage, 277);
        assert_eq!(result.skill_coverage, 277);
        assert!(entries.iter().any(|entry| entry.rarity == "UR"));
        assert!(entries.iter().any(|entry| entry.rarity == "SP"));
        assert!(entries.iter().any(|entry| entry.rarity == "SSR"));
        assert!(entries.iter().any(|entry| entry.rarity == "SR"));
        assert!(entries.iter().any(|entry| entry.rarity == "R"));
        assert!(entries.iter().any(|entry| entry.rarity == "素材"));
        assert!(entries.iter().any(|entry| entry.rarity == "N"));
    }

    #[test]
    fn 式神目录缺少第五级技能效果时拒绝安装() {
        let mut entries =
            load_builtin_shikigami("builtin-2026.08.8").expect("内置式神目录应可展开");
        let skills = serde_json::from_str::<Vec<serde_json::Value>>(&entries[0].skills_json)
            .expect("技能 JSON 应可解析");
        let mut broken_skills = skills;
        broken_skills[0]["levels"] = serde_json::json!([]);
        entries[0].skills_json =
            serde_json::to_string(&broken_skills).expect("技能 JSON 应可序列化");

        let result = validate_shikigami_payload(&entries);
        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.contains("技能或技能投入"))
        );
    }

    /// 自动更新只能淘汰更早的同类备份，用户主动创建的备份必须继续保留。
    #[test]
    fn 自动更新备份只保留最新一份且不影响手动备份() {
        let root = std::env::temp_dir().join(format!(
            "yys-update-backup-retention-{}",
            uuid::Uuid::new_v4()
        ));
        let backups_directory = root.join("backups");
        let services = Arc::new(
            AppServices::initialize(
                root.join("current-analysis.sqlite3"),
                root.join("current-data/raw"),
                root.join("current-data/quarantine"),
                backups_directory,
                root.join("temp"),
            )
            .expect("初始化备份测试服务"),
        );
        let use_case = BackupUseCase::new(services.clone());
        let manual = use_case
            .create_backup("用户手动备份")
            .expect("创建手动备份");
        let previous = use_case
            .create_backup("更新前自动备份")
            .expect("创建旧自动更新备份");
        let latest = use_case
            .create_backup_retaining_latest("更新前自动备份")
            .expect("创建并保留最新自动更新备份");

        let records = use_case.list_backups().expect("读取清理后的备份记录");
        assert_eq!(records.len(), 2, "应只保留手动备份和最新自动更新备份");
        assert!(records.iter().any(|record| record.id == manual.id));
        assert!(records.iter().any(|record| record.id == latest.id));
        assert!(!records.iter().any(|record| record.id == previous.id));
        assert!(Path::new(&manual.path).is_file(), "手动备份文件必须保留");
        assert!(
            Path::new(&latest.path).is_file(),
            "最新自动更新备份必须保留"
        );
        assert!(
            !Path::new(&previous.path).exists(),
            "旧自动更新备份文件必须删除"
        );

        drop(services);
        let _ = std::fs::remove_dir_all(root);
    }
}
