//! 规则库用例：编排预设校验、版本持久化、档案启用和影响预览。
//!
//! 命令层只传递字节和稳定标识；本模块负责把文件/分享码统一解析为已校验预设，
//! 再以不可变版本写入数据库，避免导入内容绕过 DSL 安全边界。

use super::error::AppError;
use super::services::AppServices;
use crate::domain::{
    RuleActivation, RuleShareError, RuleVersionRecord, ScoreColorThresholds, ValidatedRulePreset,
    ValidationLimits, encode_share_code, export_rule_file, parse_rule_file, parse_rule_preset,
};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

/// 规则版本在规则库页面展示的摘要和当前档案启用状态。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleVersionView {
    pub id: String,
    pub preset_id: String,
    pub version: String,
    pub title: String,
    pub author: String,
    pub status: String,
    pub origin: String,
    pub normalized_hash: String,
    pub rule_count: usize,
    pub read_only: bool,
    pub parent_version_id: Option<String>,
    pub created_at: String,
    pub enabled: bool,
    /// 当前评分标准保存的列表颜色分级，供“我的御魂”按标准显示评分色阶。
    pub score_color_thresholds: ScoreColorThresholds,
}

/// 导入前预览；此对象不代表版本已经保存或启用。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePresetPreview {
    pub source_kind: String,
    pub id: String,
    pub version: String,
    pub title: String,
    pub author: String,
    pub status: String,
    pub normalized_hash: String,
    pub rule_count: usize,
    pub read_only: bool,
    pub warnings: Vec<String>,
}

/// 规则影响预览；样本来自当前档案最近一次分析结果，未分析时会明确提示。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleImpactPreview {
    pub inventory_count: u32,
    pub matching_count: u32,
    pub sample_soul_keys: Vec<String>,
    pub overlap_count: u32,
    pub changed_count: u32,
    pub warnings: Vec<String>,
}

/// 规则库用例。
pub struct RuleUseCase {
    services: Arc<AppServices>,
}

impl RuleUseCase {
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// 列出带档案启用状态的规则记录，供旧的套装配置页面继续使用。
    pub fn list_versions(
        &self,
        profile_id: Option<&str>,
    ) -> Result<Vec<RuleVersionView>, AppError> {
        let builtin_record = builtin_record()?;

        let activations = if let Some(profile_id) = profile_id {
            self.services
                .profiles
                .get(profile_id)?
                .ok_or_else(|| AppError::not_found("game_profile", profile_id))?;
            let mut existing = self.services.rules.list_activations(profile_id)?;
            // 系统基础规则是每个档案的默认启用项；用户规则仍必须显式启用。
            if !existing
                .iter()
                .any(|activation| activation.rule_version_id == builtin_record.id)
            {
                let activation = RuleActivation {
                    profile_id: profile_id.to_owned(),
                    rule_version_id: builtin_record.id.clone(),
                    enabled: true,
                    position: 0,
                    note: Some("系统基础规则默认启用".to_owned()),
                    enabled_at: AppServices::now_iso(),
                };
                self.services.rules.set_activation(&activation)?;
                existing.push(activation);
            }
            existing
        } else {
            Vec::new()
        };
        let versions = self.services.rules.list_versions()?;
        Ok(versions
            .into_iter()
            .map(|version| {
                let enabled = activations
                    .iter()
                    .find(|activation| activation.rule_version_id == version.id)
                    .is_some_and(|activation| activation.enabled);
                view_from_record(version, enabled)
            })
            .collect())
    }

    /// 列出评分标准并读取 SQL 中的全局启用标志；不再要求游戏档案存在。
    pub fn list_score_standards(&self) -> Result<Vec<RuleVersionView>, AppError> {
        // 页面进入时再次修复旧数据库的内置指针，确保用户无需先手动切换就能按指定标准计算。
        let active_id = crate::application::services::promote_preferred_score_standard_if_needed(
            self.services.rules.as_ref(),
        )?;
        let versions = self.services.rules.list_versions()?;
        // 用户标准成为全局当前标准后，内置“默认标准”只作为内部兜底保留，不再出现在评分标准列表。
        // 若用户尚未启用任何自定义标准，仍展示内置项，确保首次使用时页面有可用标准。
        let hide_builtin = active_id.as_ref().is_some_and(|active_id| {
            versions
                .iter()
                .any(|version| &version.id == active_id && version.origin != "builtin")
        });
        Ok(versions
            .into_iter()
            .filter(|version| !(hide_builtin && version.origin == "builtin"))
            .map(|version| {
                let enabled = active_id.as_deref() == Some(version.id.as_str());
                view_from_record(version, enabled)
            })
            .collect())
    }

    /// 预检规则文件或分享码，所有导入入口共享同一套限制和校验。
    pub fn preview(
        &self,
        source_kind: &str,
        payload: &[u8],
    ) -> Result<RulePresetPreview, AppError> {
        let preset = parse_input(source_kind, payload)?;
        let mut warnings = Vec::new();
        if preset.preset.status == crate::domain::RuleStatus::Draft {
            warnings.push("该规则版本仍处于草案状态，启用前请确认来源和证据".to_owned());
        }
        if preset.preset.rules.is_empty() {
            warnings.push("规则数量为 0，只能浏览，不能产生业务结论".to_owned());
        }
        Ok(preview_from_validated(
            source_kind,
            &preset,
            false,
            warnings,
        ))
    }

    /// 导入规则版本；用户导入后拥有可编辑副本，但版本默认不启用，启用必须由单独命令明确完成。
    pub fn import(&self, source_kind: &str, payload: &[u8]) -> Result<RuleVersionView, AppError> {
        let preset = parse_input(source_kind, payload)?;
        let id = format!(
            "imported:{}:{}",
            preset.preset.id,
            &preset.normalized_hash[..16]
        );
        let record = imported_record_from_preset(&preset, id.clone());
        if self.services.rules.get_version(&id)?.is_none() {
            self.services.rules.insert_version(&record)?;
        }
        Ok(view_from_record(record, false))
    }

    /// 将一个评分标准设为全局当前标准；启用状态直接写入 SQL，重启后仍保持不变。
    pub fn set_active_score_standard(&self, rule_version_id: &str) -> Result<(), AppError> {
        let version = self
            .services
            .rules
            .get_version(rule_version_id)?
            .ok_or_else(|| AppError::not_found("rule_version", rule_version_id))?;
        self.services
            .rules
            .set_active_score_standard(&version.id, Some("用户切换评分标准"))
    }

    /// 删除用户拥有的评分标准；若删除的是当前标准，则按偏好用户标准、其他用户标准、内置标准顺序回退。
    /// 内置标准仍保留在数据库中作为无用户标准时的安全兜底，但不再覆盖已选用户标准。
    pub fn delete_score_standard(&self, rule_version_id: &str) -> Result<(), AppError> {
        let version = self
            .services
            .rules
            .get_version(rule_version_id)?
            .ok_or_else(|| AppError::not_found("rule_version", rule_version_id))?;
        if version.origin == "builtin" {
            return Err(AppError::invalid_argument(
                "versionId",
                "系统默认评分标准不能删除；如需修改，请先复制并编辑",
            ));
        }

        let was_active =
            self.services.rules.active_score_standard_id()?.as_deref() == Some(version.id.as_str());
        self.services.rules.delete_version(&version.id)?;

        if was_active {
            // 当前标准删除后优先保留铁血战士胖虎标准，避免删除操作又把评分切回旧默认标准。
            let remaining = self.services.rules.list_versions()?;
            let fallback_id = remaining
                .iter()
                .find(|candidate| {
                    crate::application::services::is_preferred_score_standard(candidate)
                })
                .or_else(|| {
                    remaining
                        .iter()
                        .find(|candidate| candidate.origin != "builtin")
                })
                .or_else(|| {
                    remaining
                        .iter()
                        .find(|candidate| candidate.origin == "builtin")
                })
                .map(|fallback| fallback.id.clone());
            if let Some(fallback_id) = fallback_id {
                self.services.rules.set_active_score_standard(
                    &fallback_id,
                    Some("删除当前标准后自动选择可用标准"),
                )?;
            }
        }
        Ok(())
    }

    /// 从只读版本派生可编辑副本，原版本正文和哈希不会被覆盖。
    pub fn copy(&self, source_id: &str) -> Result<RuleVersionView, AppError> {
        let source = self
            .services
            .rules
            .get_version(source_id)?
            .ok_or_else(|| AppError::not_found("rule_version", source_id))?;
        let mut preset = parse_record(&source)?.preset;
        let copy_uuid = uuid::Uuid::new_v4().to_string();
        let suffix = &copy_uuid[..8];
        preset.version = format!("{}-copy-{suffix}", preset.version);
        preset.title = format!("{}（副本）", preset.title);
        preset.status = crate::domain::RuleStatus::Draft;
        let validated = preset
            .validate()
            .map_err(|error| AppError::invalid_argument("preset", error.to_string()))?;
        let record = record_from_preset(
            &validated,
            format!("user:{}", uuid::Uuid::new_v4()),
            "user",
            false,
            Some(source.id),
        );
        self.services.rules.insert_version(&record)?;
        Ok(view_from_record(record, false))
    }

    /// 保存完整规则正文；选中已有用户标准时更新原记录，新建草稿时才生成新记录。
    pub fn save_editable(
        &self,
        source_kind: &str,
        payload: &[u8],
        parent_version_id: Option<&str>,
    ) -> Result<RuleVersionView, AppError> {
        let preset = parse_input(source_kind, payload)?;

        if let Some(existing_id) = parent_version_id {
            let existing = self
                .services
                .rules
                .get_version(existing_id)?
                .ok_or_else(|| AppError::not_found("rule_version", existing_id))?;
            if existing.read_only || existing.origin == "builtin" {
                return Err(AppError::invalid_argument(
                    "parentVersionId",
                    "系统默认评分标准不可直接修改，请先复制并编辑",
                ));
            }

            // 保留原 ID、预设归属和创建时间，保存动作只替换用户实际编辑的正文。
            let mut record = record_from_preset(
                &preset,
                existing.id.clone(),
                &existing.origin,
                false,
                existing.parent_version_id.clone(),
            );
            record.preset_id = existing.preset_id;
            record.created_at = existing.created_at;
            self.services.rules.update_version(&record)?;
            let enabled = self.services.rules.active_score_standard_id()?.as_deref()
                == Some(record.id.as_str());
            return Ok(view_from_record(record, enabled));
        }

        let record = record_from_preset(
            &preset,
            format!("user:{}", uuid::Uuid::new_v4()),
            "user",
            false,
            parent_version_id.map(str::to_owned),
        );
        self.services.rules.insert_version(&record)?;
        Ok(view_from_record(record, false))
    }

    /// 按档案单独启用或停用规则版本，任何版本内容都不被修改。
    pub fn set_activation(
        &self,
        profile_id: &str,
        rule_version_id: &str,
        enabled: bool,
        position: u32,
        note: Option<String>,
    ) -> Result<(), AppError> {
        self.services
            .profiles
            .get(profile_id)?
            .ok_or_else(|| AppError::not_found("game_profile", profile_id))?;
        let version = self
            .services
            .rules
            .get_version(rule_version_id)?
            .ok_or_else(|| AppError::not_found("rule_version", rule_version_id))?;
        if !enabled && version.origin == "builtin" {
            return Err(AppError::invalid_argument(
                "enabled",
                "系统基础规则始终启用，不能被普通规则停用",
            ));
        }
        self.services.rules.set_activation(&RuleActivation {
            profile_id: profile_id.to_owned(),
            rule_version_id: rule_version_id.to_owned(),
            enabled,
            position,
            note,
            enabled_at: AppServices::now_iso(),
        })
    }

    /// 计算当前分析待办中的规则命中和变化摘要，避免预览把未完成分析伪装成实时全量结果。
    pub fn impact_preview(
        &self,
        profile_id: &str,
        source_kind: &str,
        payload: &[u8],
    ) -> Result<RuleImpactPreview, AppError> {
        let preset = parse_input(source_kind, payload)?;
        let page = crate::domain::ActionRepository::list_todos(
            self.services.actions.as_ref(),
            profile_id,
            &crate::domain::TodoQuery {
                category: None,
                search: None,
                use_id: None,
                limit: 10_000,
                offset: 0,
            },
        )?;
        let default_preset = crate::domain::load_default_preset()
            .map_err(|error| AppError::internal(format!("默认规则预设无法加载：{error}")))?;
        let engine = crate::domain::RuleEngine::new();
        let mut matching_count = 0;
        let mut samples = Vec::new();
        let mut warnings = Vec::new();
        let mut overlap_count = 0;
        let mut changed_count = 0;
        for todo in &page.items {
            let Ok(detail) = serde_json::from_str::<Value>(&todo.detail_json) else {
                warnings.push(format!("{} 的分析详情无法读取", todo.soul_key));
                continue;
            };
            let Some(facts_value) = detail.get("facts") else {
                warnings.push("当前档案尚未生成可用于规则预览的事实缓存".to_owned());
                break;
            };
            let Ok(facts) = serde_json::from_value(facts_value.clone()) else {
                warnings.push(format!("{} 的规则事实格式不兼容", todo.soul_key));
                continue;
            };
            let admission = engine.evaluate_admission(&preset, &facts);
            let default_admission = engine.evaluate_admission(&default_preset, &facts);
            if admission.admitted {
                matching_count += 1;
                if samples.len() < 3 {
                    samples.push(todo.soul_key.clone());
                }
            }
            if admission.admitted && default_admission.admitted {
                overlap_count += 1;
            }
            if admission.admitted != default_admission.admitted {
                changed_count += 1;
            }
        }
        if page.items.is_empty() {
            warnings.push("当前档案没有可用于影响预览的分析待办".to_owned());
        }
        Ok(RuleImpactPreview {
            inventory_count: page.total,
            matching_count,
            sample_soul_keys: samples,
            overlap_count,
            changed_count,
            warnings,
        })
    }

    /// 生成规范规则文件和离线分享码，导出不携带档案、库存或用户决定。
    pub fn export(&self, version_id: &str) -> Result<(String, String), AppError> {
        let version = self
            .services
            .rules
            .get_version(version_id)?
            .ok_or_else(|| AppError::not_found("rule_version", version_id))?;
        let validated = parse_record(&version)?;
        let file = export_rule_file(&validated).map_err(share_error)?;
        let code = encode_share_code(&validated).map_err(share_error)?;
        Ok((file, code))
    }
}

/// 解析记录正文，保证数据库中的正文即使被外部破坏也不会直接进入评估。
fn parse_record(record: &RuleVersionRecord) -> Result<ValidatedRulePreset, AppError> {
    parse_rule_preset(
        record.canonical_json.as_bytes(),
        ValidationLimits::default(),
    )
    .map_err(|error| AppError::invalid_argument("ruleVersion", error.to_string()))
}

/// 根据来源选择文件解析或分享码解析，并把所有错误收敛为字段级命令错误。
fn parse_input(source_kind: &str, payload: &[u8]) -> Result<ValidatedRulePreset, AppError> {
    match source_kind {
        "file" => parse_rule_file(payload, ValidationLimits::default()).map_err(share_error),
        "shareCode" => {
            let code = std::str::from_utf8(payload)
                .map_err(|_| AppError::invalid_argument("payload", "分享码不是 UTF-8 文本"))?;
            crate::domain::decode_share_code(code, ValidationLimits::default()).map_err(share_error)
        }
        _ => Err(AppError::invalid_argument(
            "sourceKind",
            "规则来源必须是 file 或 shareCode",
        )),
    }
}

/// 将领域分享错误转换为稳定的前端参数错误。
fn share_error(error: RuleShareError) -> AppError {
    AppError::invalid_argument("payload", error.to_string())
}

/// 把已校验预设转换为数据库评分标准记录。
fn record_from_preset(
    preset: &ValidatedRulePreset,
    id: String,
    origin: &str,
    read_only: bool,
    parent_version_id: Option<String>,
) -> RuleVersionRecord {
    // 系统内置版本沿用预设稳定 ID；用户导入或复制版本必须使用独立身份，
    // 避免与同一份 JSON 的内置版本撞上数据库唯一约束并导致重启加载失败。
    let preset_id = if origin == "builtin" {
        preset.preset.id.clone()
    } else {
        id.clone()
    };
    RuleVersionRecord {
        id,
        preset_id,
        version: preset.preset.version.clone(),
        schema_version: preset.preset.schema_version,
        title: preset.preset.title.clone(),
        author: preset.preset.author.clone(),
        status: preset.preset.status.as_str().to_owned(),
        origin: origin.to_owned(),
        canonical_json: preset.canonical_json.clone(),
        canonical_sha256: preset.normalized_hash.clone(),
        parent_version_id,
        read_only,
        created_at: AppServices::now_iso(),
    }
}

/// 构造系统内置评分标准记录；服务初始化阶段会把它写入 SQL 数据库。
pub(crate) fn builtin_record() -> Result<RuleVersionRecord, AppError> {
    let builtin = crate::domain::load_default_preset()
        .map_err(|error| AppError::internal(format!("默认规则预设无法加载：{error}")))?;
    Ok(record_from_preset(
        &builtin,
        format!("builtin:{}:{}", builtin.preset.id, builtin.preset.version),
        "builtin",
        true,
        None,
    ))
}

/// 将数据库版本转换为页面摘要。
fn view_from_record(record: RuleVersionRecord, enabled: bool) -> RuleVersionView {
    let parsed = parse_record(&record).ok();
    let rule_count = parsed
        .as_ref()
        .map(|preset| preset.preset.rules.len())
        .unwrap_or_default();
    let score_color_thresholds = parsed
        .map(|preset| preset.preset.score_color_thresholds)
        .unwrap_or_default();
    RuleVersionView {
        id: record.id,
        preset_id: record.preset_id,
        version: record.version,
        title: record.title,
        author: record.author,
        status: record.status,
        origin: record.origin,
        normalized_hash: record.canonical_sha256,
        rule_count,
        read_only: record.read_only,
        parent_version_id: record.parent_version_id,
        created_at: record.created_at,
        enabled,
        score_color_thresholds,
    }
}

/// 生成导入预览对象，明确说明导入后默认不启用但允许编辑。
fn preview_from_validated(
    source_kind: &str,
    preset: &ValidatedRulePreset,
    read_only: bool,
    warnings: Vec<String>,
) -> RulePresetPreview {
    RulePresetPreview {
        source_kind: source_kind.to_owned(),
        id: preset.preset.id.clone(),
        version: preset.preset.version.clone(),
        title: preset.preset.title.clone(),
        author: preset.preset.author.clone(),
        status: preset.preset.status.as_str().to_owned(),
        normalized_hash: preset.normalized_hash.clone(),
        rule_count: preset.preset.rules.len(),
        read_only,
        warnings,
    }
}

/// 将用户导入的预设标记为自己的可编辑版本；只有系统内置版本保持只读。
fn imported_record_from_preset(preset: &ValidatedRulePreset, id: String) -> RuleVersionRecord {
    record_from_preset(preset, id, "imported", false, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归约束：用户导入的规则必须可编辑，不能被错误标记为只读。
    #[test]
    fn 用户导入规则版本应允许编辑() {
        let preset = crate::domain::load_default_preset().expect("默认规则预设");
        let record = imported_record_from_preset(&preset, "imported:test".to_owned());

        assert_eq!(record.origin, "imported");
        assert!(!record.read_only);
        assert_eq!(record.preset_id, record.id);
    }
}
