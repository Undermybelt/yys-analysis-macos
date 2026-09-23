import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import type {
  TaskAccepted,
  TaskProgress,
  GameProfile,
  ProfileCreated,
  CreateProfileRequest,
  RenameProfileRequest,
  ArchiveProfileRequest,
  CatalogStatus,
  SoulSet,
  Shikigami,
  MyShikigami,
  ShikigamiBagGroup,
  ShikigamiShardEntry,
  ShikigamiStoryProgress,
  ShikigamiShardLookupResult,
  RealmCardEntry,
  CurrentDataSummary,
  EmbryoDecisionRequest,
  MySoulPage,
  MySoulScore,
  GrowthQualityReport,
  EmbryoDecisionReport,
  SoulRadarCache,
  HeadTailCache,
  SimulateEnhancementRequest,
  SimulateEnhancementResult,
  MiracleConchRequest,
  MiracleConchResult,
  EffectiveCommonness,
  CommonnessOverrideRequest,
  CopyCommonnessRequest,
  StoreRawRequest,
  StoredRawObject,
  BackupRecord,
  CreateBackupRequest,
  AnalysisCenter,
  ExportPayload,
  RulePresetSummary,
  BuiltinUseTemplate,
  RuleVersion,
  RuleSourceKind,
  RulePresetPreview,
  RuleImpactPreview,
  RuleExport,
  AnalysisTodoPage,
  ListAnalysisTodosRequest,
  RecalculateAnalysisRequest,
  DecisionPreview,
  DecisionApplyResult,
  DecisionChange,
  DecisionHistoryEntry,
  CreateActionBatchRequest,
  ActionBatch,
  ActionBatchDetail,
  UpdateSourceMode,
  UpdateChannel,
  UpdatePackageType,
  UpdateCheckResult,
  UpdateProgress,
  UpdateInstallResult,
  InstalledPackage,
  CharacterArchiveScanResult,
  CharacterArchiveExportResult,
  SetActiveCharacterRequest,
  GuildOverview,
  CbgReadPreview,
} from "./contracts";
import { normalizeCommandError } from "./errors";

/** 所有后台任务共用同一事件名，消费者再按 taskId 隔离自己的状态。 */
export const TASK_PROGRESS_EVENT = "task://progress";
/** 应用更新下载事件；只由更新弹窗消费，不进入通用任务中心。 */
export const UPDATE_PROGRESS_EVENT = "update://progress";

/** 表现层可调用的最小本地能力集合，不暴露文件系统、数据库或 shell。 */
export interface DesktopApi {
  // 外部网页由 Tauri opener 交给系统默认浏览器，不在应用 WebView 内创建新窗口。
  openExternalUrl(url: string): Promise<void>;
  cancelBackgroundTask(taskId: string): Promise<void>;
  onTaskProgress(listener: (progress: TaskProgress) => void): Promise<UnlistenFn>;
  onUpdateProgress(listener: (progress: UpdateProgress) => void): Promise<UnlistenFn>;

  // 角色档案页：列出导入写入的角色档案摘要；清空和单个删除都会同步清理绑定的本机数据。
  listCharacterArchives(): Promise<CharacterArchiveScanResult>;
  // 角色档案交换文件：后端直接写入下载目录，导入只接收用户选择的 JSON 字节。
  exportCharacterArchive(profileId: string): Promise<CharacterArchiveExportResult>;
  importCharacterArchive(request: {
    fileName: string;
    payload: number[];
  }): Promise<TaskAccepted>;
  // 单个角色删除：后端会同步清理该角色的数据档案、库存正文和分析派生结果。
  deleteCharacterArchive(identityKey: string): Promise<void>;
  clearCharacterArchives(): Promise<number>;
  // 切换当前激活角色；之后所有页面读取都跟随该角色的数据。
  setActiveCharacter(request: SetActiveCharacterRequest): Promise<void>;
  // 藏宝阁公开商品读取：先预检，再确认覆盖当前御魂与式神数据。
  inspectCbgRead(request: { sourceUrl: string }): Promise<CbgReadPreview>;
  startCbgImportTask(request: { sourceUrl: string }): Promise<TaskAccepted>;

  clearCurrentInventory(): Promise<void>;

  getAnalysisCenter(profileId: string): Promise<AnalysisCenter>;
  exportData(request: {
    profileId: string;
    kind: "profile" | "snapshot" | "analysis";
    snapshotId?: string;
    category?: string;
    search?: string;
  }): Promise<ExportPayload>;

  // 游戏档案
  createProfile(request: CreateProfileRequest): Promise<ProfileCreated>;
  listProfiles(includeArchived?: boolean): Promise<GameProfile[]>;
  renameProfile(request: RenameProfileRequest): Promise<GameProfile>;
  archiveProfile(request: ArchiveProfileRequest): Promise<GameProfile>;

  // 目录
  installBuiltinCatalog(): Promise<CatalogStatus>;
  getCatalogStatus(): Promise<CatalogStatus | null>;
  listSoulSets(catalogVersion: string): Promise<SoulSet[]>;
  listShikigami(catalogVersion: string): Promise<Shikigami[]>;
  listMyShikigami(): Promise<MyShikigami[]>;
  // 式神仓库中的素材式神分组；游戏只保存分组数量，没有逐只实例。
  listShikigamiBag(): Promise<ShikigamiBagGroup[]>;
  // 当前导入的目标式神碎片；只返回 SSR、SP、UR 与御行达摩。
  listShikigamiShards(): Promise<ShikigamiShardEntry[]>;
  // 当前导入账号按式神归一化的全部传记进度；没有新字段时返回空列表。
  listShikigamiStoryProgress(): Promise<ShikigamiStoryProgress[]>;
  // 跨全部游戏档案查询一个目标式神；调用不会改变当前激活角色。
  lookupShikigamiShards(shikigamiId: string): Promise<ShikigamiShardLookupResult>;
  // 当前导入的资源/道具数量（21 项键值对象）、结界卡、式神碎片与数据摘要；无当前数据时返回空值。
  getCurrentItems(): Promise<Record<string, number>>;
  getCurrentRealmCards(): Promise<RealmCardEntry[]>;
  getCurrentDataSummary(): Promise<CurrentDataSummary>;
  // 寮管理页：读取当前导入快照中的寮概况与成员列表；无寮数据时返回 null。
  getCurrentGuild(): Promise<GuildOverview | null>;
  listMySouls(request?: {
    offset?: number;
    limit?: number;
    setId?: string;
    setCategory?: string;
    slot?: number;
    quality?: number;
    level?: number;
    mainAttrType?: string;
    subAttrType?: string;
    attributeType?: string;
    attributeOperator?: "gt" | "lt";
    attributeValue?: number;
    standardScoreMin?: number;
    standardScoreMax?: number;
    auspiciousOnly?: boolean;
  }): Promise<MySoulPage>;
  listMySoulScores(soulKeys?: string[]): Promise<MySoulScore[]>;
  analyzePlus15GrowthQuality(): Promise<GrowthQualityReport>;
  analyzeFourLegEmbryoDecision(): Promise<EmbryoDecisionReport>;
  analyzeEmbryoDecision(request: EmbryoDecisionRequest): Promise<EmbryoDecisionReport>;
  getSoulRadarCache(): Promise<SoulRadarCache | null>;
  getHeadTailCache(): Promise<HeadTailCache | null>;

  // 神奇海螺页面配置由 Rust 写入应用配置目录，跨 WebView/开发进程重启保留。
  getMiracleConchConfiguration(): Promise<string | null>;
  setMiracleConchConfiguration(configuration: string): Promise<void>;
  calculateSoulRadar(): Promise<SoulRadarCache>;
  calculateHeadTail(): Promise<HeadTailCache>;
  simulateEnhancement(
    request: SimulateEnhancementRequest,
  ): Promise<SimulateEnhancementResult>;
  calculateMiracleConch(request: MiracleConchRequest): Promise<MiracleConchResult>;
  getEffectiveCommonness(profileId: string): Promise<EffectiveCommonness[]>;
  setCommonnessOverride(request: CommonnessOverrideRequest): Promise<void>;
  copyCommonness(request: CopyCommonnessRequest): Promise<number>;

  // 原始对象
  storeRawObject(request: StoreRawRequest): Promise<StoredRawObject>;
  verifyRawObject(sha256: string): Promise<void>;

  // 备份
  createBackup(request: CreateBackupRequest): Promise<BackupRecord>;
  listBackups(): Promise<BackupRecord[]>;
  validateBackup(backupId: string): Promise<void>;
  restoreBackup(backupId: string): Promise<void>;

  // 规则引擎
  getDefaultRulePreset(): Promise<RulePresetSummary>;
  listBuiltinUseTemplates(): Promise<BuiltinUseTemplate[]>;
  listRuleVersions(profileId?: string): Promise<RuleVersion[]>;
  listScoreStandards(): Promise<RuleVersion[]>;
  previewRulePreset(
    sourceKind: RuleSourceKind,
    payload: number[],
  ): Promise<RulePresetPreview>;
  importRulePreset(sourceKind: RuleSourceKind, payload: number[]): Promise<RuleVersion>;
  copyRuleVersion(versionId: string): Promise<RuleVersion>;
  saveRuleVersion(
    sourceKind: RuleSourceKind,
    payload: number[],
    parentVersionId?: string,
  ): Promise<RuleVersion>;
  exportRuleVersion(versionId: string): Promise<RuleExport>;
  setRuleActivation(request: {
    profileId: string;
    ruleVersionId: string;
    enabled: boolean;
    position?: number;
    note?: string;
  }): Promise<void>;
  setActiveScoreStandard(versionId: string): Promise<void>;
  deleteScoreStandard(versionId: string): Promise<void>;
  previewRuleImpact(
    profileId: string,
    sourceKind: RuleSourceKind,
    payload: number[],
  ): Promise<RuleImpactPreview>;
  startRuleRecalculationTask(request: {
    profileId: string;
    snapshotId?: string;
  }): Promise<TaskAccepted>;

  // 分析待办、用户决定与行动批次
  recalculateAnalysis(request: RecalculateAnalysisRequest): Promise<number>;
  listAnalysisTodos(request: ListAnalysisTodosRequest): Promise<AnalysisTodoPage>;
  previewDecisions(request: {
    profileId: string;
    soulKeys: string[];
    decision: string;
    respectProtection?: boolean;
  }): Promise<DecisionPreview>;
  applyDecisions(request: {
    profileId: string;
    changes: DecisionChange[];
    respectProtection?: boolean;
  }): Promise<DecisionApplyResult>;
  undoDecision(operationId: string): Promise<void>;
  listDecisionHistory(
    profileId: string,
    soulKey: string,
  ): Promise<DecisionHistoryEntry[]>;
  createActionBatch(request: CreateActionBatchRequest): Promise<ActionBatch>;
  listActionBatches(profileId: string): Promise<ActionBatch[]>;
  getActionBatch(profileId: string, batchId: string): Promise<ActionBatchDetail>;
  updateActionBatchItem(request: {
    profileId: string;
    batchId: string;
    soulKey: string;
    status: string;
    note?: string;
  }): Promise<ActionBatchDetail>;

  // 签名更新
  checkUpdates(request: {
    sourceMode: UpdateSourceMode;
    channel: UpdateChannel;
    packageType: UpdatePackageType;
    gameVersion?: string;
    /** 启动检查只保存清单；手动页面不传时保持下载并暂存的兼容行为。 */
    stagePayload?: boolean;
  }): Promise<UpdateCheckResult>;
  installUpdate(request: {
    stagedUpdateId: string;
    autoRestart?: boolean;
    gameVersion?: string;
  }): Promise<UpdateInstallResult>;
  listInstalledPackages(): Promise<InstalledPackage[]>;
  rollbackUpdate(request: {
    packageType: UpdatePackageType;
    version: string;
    gameVersion?: string;
  }): Promise<UpdateInstallResult>;
}

/** 生产环境的类型化 Tauri 命令客户端。 */
export const desktopApi: DesktopApi = {
  async openExternalUrl(url) {
    try {
      await openUrl(url);
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async cancelBackgroundTask(taskId) {
    try {
      await invoke<void>("cancel_background_task", { taskId });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async onTaskProgress(listener) {
    return listen<TaskProgress>(TASK_PROGRESS_EVENT, (event) => {
      listener(event.payload);
    });
  },

  async onUpdateProgress(listener) {
    return listen<UpdateProgress>(UPDATE_PROGRESS_EVENT, (event) => {
      listener(event.payload);
    });
  },

  async listCharacterArchives() {
    try {
      return await invoke<CharacterArchiveScanResult>("list_character_archives");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async exportCharacterArchive(profileId) {
    try {
      return await invoke<CharacterArchiveExportResult>("export_character_archive", {
        request: { profileId },
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async importCharacterArchive(request) {
    try {
      return await invoke<TaskAccepted>("import_character_archive", {
        request,
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async deleteCharacterArchive(identityKey) {
    try {
      await invoke<void>("delete_character_archive", {
        request: { identityKey },
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async clearCharacterArchives() {
    try {
      return await invoke<number>("clear_character_archives");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async setActiveCharacter(request: SetActiveCharacterRequest) {
    try {
      return await invoke<void>("set_active_character", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async inspectCbgRead(request) {
    try {
      return await invoke<CbgReadPreview>("inspect_cbg_read", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async startCbgImportTask(request) {
    try {
      return await invoke<TaskAccepted>("start_cbg_import_task", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 清空按钮只调用本地事务命令；评分、雷达和历史派生结果由 Rust 一并处理。
  async clearCurrentInventory() {
    try {
      await invoke<void>("clear_current_inventory");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async getAnalysisCenter(profileId) {
    try {
      return await invoke<AnalysisCenter>("get_analysis_center", {
        request: { profileId },
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async exportData(request) {
    try {
      return await invoke<ExportPayload>("export_data", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // ─── 游戏档案 ──────────────────────────────────────────────────────────────

  async createProfile(request) {
    try {
      return await invoke<ProfileCreated>("create_profile", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async listProfiles(includeArchived) {
    try {
      return await invoke<GameProfile[]>("list_profiles", {
        includeArchived: includeArchived ?? false,
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async renameProfile(request) {
    try {
      return await invoke<GameProfile>("rename_profile", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async archiveProfile(request) {
    try {
      return await invoke<GameProfile>("archive_profile", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // ─── 目录 ──────────────────────────────────────────────────────────────────

  async installBuiltinCatalog() {
    try {
      return await invoke<CatalogStatus>("install_builtin_catalog");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async getCatalogStatus() {
    try {
      return await invoke<CatalogStatus | null>("get_catalog_status");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async listSoulSets(catalogVersion) {
    try {
      return await invoke<SoulSet[]>("list_soul_sets", { catalogVersion });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 式神图鉴只读取已安装目录版本，离线时不触发任何远程请求。
  async listShikigami(catalogVersion) {
    try {
      return await invoke<Shikigami[]>("list_shikigami", { catalogVersion });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 我的式神只读取当前导入文件的归一化列表；前端不直接接触 heroes 原始缓存。
  async listMyShikigami() {
    try {
      return await invoke<MyShikigami[]>("list_my_shikigami");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async listShikigamiBag() {
    try {
      return await invoke<ShikigamiBagGroup[]>("list_shikigami_bag");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async listShikigamiShards() {
    try {
      return await invoke<ShikigamiShardEntry[]>("list_shikigami_shards");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async listShikigamiStoryProgress() {
    try {
      return await invoke<ShikigamiStoryProgress[]>("list_shikigami_story_progress");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async lookupShikigamiShards(shikigamiId) {
    try {
      return await invoke<ShikigamiShardLookupResult>(
        "lookup_shikigami_shards_across_archives",
        { request: { shikigamiId } },
      );
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async getCurrentItems() {
    try {
      return await invoke<Record<string, number>>("get_current_items");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async getCurrentRealmCards() {
    try {
      return await invoke<RealmCardEntry[]>("get_current_realm_cards");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async getCurrentDataSummary() {
    try {
      return await invoke<CurrentDataSummary>("get_current_data_summary");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async getCurrentGuild() {
    try {
      return await invoke<GuildOverview | null>("get_current_guild");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 我的御魂只读取后端当前页，前端不直接解析原始 JSON 或访问数据库。
  async listMySouls(request) {
    try {
      return await invoke<MySoulPage>("list_my_souls", { request: request ?? null });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 列表只读取当前页评分摘要；逐用途解释继续由分析待办按需提供。
  async listMySoulScores(soulKeys) {
    try {
      return await invoke<MySoulScore[]>("list_my_soul_scores", {
        request: soulKeys ? { soulKeys } : null,
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 成品成长质量只读取当前档案的六星 +15 快照事实，不改变库存或评分结果。
  async analyzePlus15GrowthQuality() {
    try {
      return await invoke<GrowthQualityReport>("analyze_plus15_growth_quality");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 胚子决策由用户手动触发，只读取当前档案的六星 +0 四腿事实，不改变库存或评分结果。
  async analyzeFourLegEmbryoDecision() {
    try {
      return await invoke<EmbryoDecisionReport>("analyze_four_leg_embryo_decision");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 胚子腿数由用户显式选择；后端仍会再次按选择过滤，避免未勾选腿数参与计算。
  async analyzeEmbryoDecision(request: EmbryoDecisionRequest) {
    try {
      return await invoke<EmbryoDecisionReport>("analyze_embryo_decision", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 雷达页面进入时只读取已持久化的聚合结果，不把全部御魂正文搬到前端。
  async getSoulRadarCache() {
    try {
      return await invoke<SoulRadarCache | null>("get_soul_radar_cache");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 用户点击计算后才触发全量御魂聚合，并由后端把结果写入缓存表。
  async calculateSoulRadar() {
    try {
      return await invoke<SoulRadarCache>("calculate_soul_radar");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 头尾页面进入时只读取已持久化结果，不把全部御魂正文搬到前端。
  async getHeadTailCache() {
    try {
      return await invoke<HeadTailCache | null>("get_head_tail_cache");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 用户点击计算后才触发头尾筛选，并由后端将最佳结果写入 SQLite 缓存。
  async calculateHeadTail() {
    try {
      return await invoke<HeadTailCache>("calculate_head_tail");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 模拟强化只读取后端当前库存并返回内存结果，不写入真实御魂或分析待办。
  async simulateEnhancement(request) {
    try {
      return await invoke<SimulateEnhancementResult>("simulate_enhancement", {
        request,
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 神奇海螺只在内存中计算当前库存的目标配装，不写入库存、评分或行动批次。
  async calculateMiracleConch(request) {
    try {
      return await invoke<MiracleConchResult>("calculate_miracle_conch", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // 神奇海螺配置不进入御魂库存数据库，只保存可恢复的页面表单 JSON。
  async getMiracleConchConfiguration() {
    try {
      return await invoke<string | null>("get_miracle_conch_configuration");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async setMiracleConchConfiguration(configuration) {
    try {
      await invoke<void>("set_miracle_conch_configuration", { configuration });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async getEffectiveCommonness(profileId) {
    try {
      return await invoke<EffectiveCommonness[]>("get_effective_commonness", {
        profileId,
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async setCommonnessOverride(request) {
    try {
      return await invoke<void>("set_commonness_override", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async copyCommonness(request) {
    try {
      return await invoke<number>("copy_commonness", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // ─── 原始对象 ──────────────────────────────────────────────────────────────

  async storeRawObject(request) {
    try {
      return await invoke<StoredRawObject>("store_raw_object", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async verifyRawObject(sha256) {
    try {
      return await invoke<void>("verify_raw_object", { sha256 });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // ─── 备份 ──────────────────────────────────────────────────────────────────

  async createBackup(request) {
    try {
      return await invoke<BackupRecord>("create_backup", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async listBackups() {
    try {
      return await invoke<BackupRecord[]>("list_backups");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async validateBackup(backupId) {
    try {
      return await invoke<void>("validate_backup", { backupId });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async restoreBackup(backupId) {
    try {
      return await invoke<void>("restore_backup", { backupId });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // ─── 规则引擎 ──────────────────────────────────────────────────────────────

  async getDefaultRulePreset() {
    try {
      return await invoke<RulePresetSummary>("get_default_rule_preset");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async listBuiltinUseTemplates() {
    try {
      return await invoke<BuiltinUseTemplate[]>("list_builtin_use_templates");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async listRuleVersions(profileId) {
    try {
      return await invoke<RuleVersion[]>("list_rule_versions", {
        request: { profileId: profileId ?? null },
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async listScoreStandards() {
    try {
      return await invoke<RuleVersion[]>("list_score_standards");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async previewRulePreset(sourceKind, payload) {
    try {
      return await invoke<RulePresetPreview>("preview_rule_preset", {
        request: { sourceKind, payload },
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async importRulePreset(sourceKind, payload) {
    try {
      return await invoke<RuleVersion>("import_rule_preset", {
        request: { sourceKind, payload },
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async copyRuleVersion(versionId) {
    try {
      return await invoke<RuleVersion>("copy_rule_version", { versionId });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async saveRuleVersion(sourceKind, payload, parentVersionId) {
    try {
      return await invoke<RuleVersion>("save_rule_version", {
        request: { sourceKind, payload, parentVersionId },
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async exportRuleVersion(versionId) {
    try {
      return await invoke<RuleExport>("export_rule_version", { versionId });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async setRuleActivation(request) {
    try {
      await invoke<void>("set_rule_activation", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async setActiveScoreStandard(versionId) {
    try {
      await invoke<void>("set_active_score_standard", { versionId });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async deleteScoreStandard(versionId) {
    try {
      await invoke<void>("delete_score_standard", { versionId });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async previewRuleImpact(profileId, sourceKind, payload) {
    try {
      return await invoke<RuleImpactPreview>("preview_rule_impact", {
        request: { profileId, sourceKind, payload },
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async startRuleRecalculationTask(request) {
    try {
      return await invoke<TaskAccepted>("start_rule_recalculation_task", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // ─── 分析待办、用户决定与行动批次 ────────────────────────────────────────

  async recalculateAnalysis(request) {
    try {
      return await invoke<number>("recalculate_analysis", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async listAnalysisTodos(request) {
    try {
      return await invoke<AnalysisTodoPage>("list_analysis_todos", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async previewDecisions(request) {
    try {
      return await invoke<DecisionPreview>("preview_decisions", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async applyDecisions(request) {
    try {
      return await invoke<DecisionApplyResult>("apply_decisions", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async undoDecision(operationId) {
    try {
      await invoke<void>("undo_decision", { request: { operationId } });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async listDecisionHistory(profileId, soulKey) {
    try {
      return await invoke<DecisionHistoryEntry[]>("list_decision_history", {
        request: { profileId, soulKey },
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async createActionBatch(request) {
    try {
      return await invoke<ActionBatch>("create_action_batch", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async listActionBatches(profileId) {
    try {
      return await invoke<ActionBatch[]>("list_action_batches", { profileId });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async getActionBatch(profileId, batchId) {
    try {
      return await invoke<ActionBatchDetail>("get_action_batch", {
        profileId,
        batchId,
      });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async updateActionBatchItem(request) {
    try {
      return await invoke<ActionBatchDetail>("update_action_batch_item", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  // ─── 签名更新 ──────────────────────────────────────────────────────────────

  async checkUpdates(request) {
    try {
      return await invoke<UpdateCheckResult>("check_updates", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async installUpdate(request) {
    try {
      return await invoke<UpdateInstallResult>("install_update", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async listInstalledPackages() {
    try {
      return await invoke<InstalledPackage[]>("list_installed_packages");
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },

  async rollbackUpdate(request) {
    try {
      return await invoke<UpdateInstallResult>("rollback_update", { request });
    } catch (error) {
      throw normalizeCommandError(error);
    }
  },
};
