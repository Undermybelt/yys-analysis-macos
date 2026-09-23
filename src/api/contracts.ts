/** 角色平台；公开数据未标明时保持为空，不按区服猜测。 */
export type GamePlatform = "android" | "ios";

/** 结界卡条目；字段顺序与游戏快照一致。 */
export type RealmCardEntry = [
  id: string | number,
  itemId: string | number,
  totalTime: string | number,
  attrs: unknown,
];

/** 前后端稳定错误码；界面不得依赖 Rust 内部错误文本做分支。 */
export type ErrorCode =
  | "INVALID_ARGUMENT"
  | "PATH_UNAVAILABLE"
  | "TASK_NOT_FOUND"
  | "IO_ERROR"
  | "EVENT_EMIT_FAILED"
  | "INTERNAL_ERROR"
  | "DATABASE_ERROR"
  | "NOT_FOUND"
  | "CONFLICT"
  | "CHECKSUM_MISMATCH"
  | "MIGRATION_FAILED"
  | "BACKUP_FAILED"
  | "CONSTRAINT_VIOLATION"
  | "MUMU_UNAVAILABLE"
  | "DESKTOP_UNAVAILABLE"
  | "ADAPTER_REJECTED"
  | "READ_SESSION_CONFLICT"
  | "UPDATE_REJECTED"
  | "UPDATE_SOURCE_UNAVAILABLE"
  | "RESOURCE_LIMIT_EXCEEDED";

/** 所有 Tauri 命令失败时返回的统一错误形状。 */
export interface CommandErrorPayload {
  code: ErrorCode;
  message: string;
  details: Record<string, unknown> | null;
  retryable: boolean;
}

/** 后台命令接收后立即返回的任务凭据。 */
export interface TaskAccepted {
  taskId: string;
}

/** 本机 ADB 中正在运行阴阳师、可供玩家选择的 MuMu 模拟器实例。 */
export interface CharacterArchive {
  /** 稳定的角色身份键；单个角色删除时用于定位档案。 */
  identityKey: string;
  accountId: string;
  /** 该角色绑定的数据档案 ID；切换角色时直接使用该 ID。 */
  profileId: string | null;
  /** 角色昵称；身份缺失时后端使用“未命名角色”友好标签。 */
  displayName: string;
  /** 角色平台；旧档案没有该字段时为 null。 */
  platform: GamePlatform | null;
  /** 游戏内玩家 ID（UID）；快照未携带时为 null。 */
  playerId: string | null;
  /** 区服名；快照未携带时为 null。 */
  serverLabel: string | null;
  /** 最后一次成功导入的时间。 */
  lastUpdatedAt: string | null;
  /** 是否与当前导入快照是同一角色。 */
  isCurrent: boolean;
  readable: boolean;
  warning: string | null;
  /** 六星御魂数。 */
  soulCount: number;
  /** 式神数。 */
  shikigamiCount: number;
  /** 档案来源：cbg（藏宝阁）/ native（本地导入）等。 */
  sourceKind: string;
  /** 角色来源展示名：藏宝阁 / 本地导入等。 */
  sourceLabel: string;
}

/** 角色档案扫描结果；只返回本机已导入的档案摘要，不关联游戏目录。 */
export interface CharacterArchiveScanResult {
  characters: CharacterArchive[];
  message: string;
}

/** 角色档案导出结果；Rust 已把 JSON 与 ZIP 写入系统下载目录，前端只展示实际路径。 */
export interface CharacterArchiveExportResult {
  characterName: string;
  jsonPath: string;
  zipPath: string;
  recordCount: number;
}

/** 角色档案导入结果；不把大段御魂正文放回 WebView，只返回本次覆盖摘要。 */
export interface CharacterArchiveImportResult {
  characterName: string;
  profileId: string;
  soulCount: number;
  shikigamiCount: number;
}

/** 切换当前激活角色的请求；档案 ID 必须是已导入角色绑定的数据档案。 */
export interface SetActiveCharacterRequest {
  profileId: string;
}

/**
 * 寮成员条目；来自内存读取快照的寮成员数组，只读展示。
 *
 * 游戏服务器不下发捐献次数、最后登录时间和领取次数，因此这些字段不在契约内。
 */
export interface GuildMember {
  id: string;
  name: string;
  /** 游戏内职务编码；未知编码保留原始值。 */
  dutyCode: number;
  /** 职务展示名：会长 / 副会长 / 普通成员。 */
  dutyLabel: string;
  level: number;
  /** 加入时间（Unix 秒）；缺失为 0。 */
  joinedAt: number;
  /** 加入日期（YYYY-MM-DD，本地时区）；缺失为空串。 */
  joinedDate: string;
  /** 本周功绩。 */
  weeklyFeats: number;
  /** 总功绩。 */
  totalFeats: number;
  /** 累计捐献。 */
  historyDonate: number;
  /** 离线时间（Unix 秒）；为 0 表示该成员当前在线。 */
  offlineAt: number;
  /** 离线时刻（YYYY-MM-DD HH:mm，本地时区）；当前在线时为空串。 */
  offlineAtLabel: string;
  pvpScore: number;
}

/** 寮管理页数据：寮概况与全部成员；当前快照无寮数据时为 null。 */
export interface GuildOverview {
  id: string;
  name: string;
  level: number;
  /** 寮资金。 */
  funds: number;
  activeMemberCount: number;
  memberCount: number;
  serverId: number;
  createTime: number;
  /** 创建日期（YYYY-MM-DD，本地时区）。 */
  createDate: string;
  badge: number;
  pvpScore: number;
  /** 现任会长名；无会长成员时回退到建寮者名。 */
  leaderName: string;
  /** 寮宣言；未设置为空串。 */
  declare: string;
  /** 活跃排名；-1 表示未上榜。 */
  activeRank: number;
  /** 寮战排名；-1 表示未上榜。 */
  pvpRank: number;
  /** 活跃度。 */
  activeScore: number;
  /** 建设度。 */
  construction: number;
  /** 勋章总数。 */
  insigniaCount: number;
  /** 寮战赛季序号。 */
  pvpSeason: number;
  /** 寮突破已达最高难度。 */
  maxGveDifficulty: number;
  /** 狭间当前难度。 */
  creviceDifficulty: number;
  /** 狭间已通关难度。 */
  creviceDefeatedDifficulty: number;
  members: GuildMember[];
}

/** 当前导入快照摘要。 */
export interface CurrentDataSummary {
  exists: boolean;
  file_name: string | null;
  source_kind: string | null;
  completeness: string | null;
  imported_at: string | null;
  soul_count: number;
  shikigami_count: number;
  /** 非 0 的资源/道具项数。 */
  item_count: number;
  /** 结界卡张数。 */
  realm_card_count: number;
  /** 寮名称；未读取到寮数据时为 null。 */
  guild_name: string | null;
  /** 寮成员数；未读取到寮数据时为 0。 */
  guild_member_count: number;
  /** 玩家昵称；快照未携带时为 null。 */
  account_name: string | null;
  /** 区服名；快照未携带时为 null。 */
  server_name: string | null;
  /** 玩家短 ID（UID）；快照未携带时为 null。 */
  short_id: string | null;
  /** 玩家平台；公开数据未给出时为 null。 */
  platform: GamePlatform | null;
}

/** 藏宝阁公开商品预检结果。 */
export interface CbgReadPreview {
  sourceUrl: string;
  accountName: string | null;
  serverName: string | null;
  /** 仅在藏宝阁公开详情明确给出时返回；缺失时保持未知，不根据区服猜测。 */
  platform: GamePlatform | null;
  inventoryCount: number;
  retainedCount: number;
  sixStarCount: number;
  lowerStarCount: number;
  shikigamiCount: number;
  sampleSouls: CbgSoulPreview[];
  warnings: string[];
  message: string;
}

/** 藏宝阁预览中的单枚御魂；用于确认公开字段到“我的御魂”的映射。 */
export interface CbgSoulPreview {
  id: string;
  setName: string;
  slot: number;
  quality: number;
  level: number;
  mainAttribute: string;
  mainValue: string;
  subAttributes: string[];
}

/** 后台任务状态同时覆盖正常完成、取消和失败终态。 */
export type TaskStatus = "running" | "completed" | "cancelled" | "failed";

/** Rust 通过全局事件发送的统一任务进度。 */
export interface TaskProgress {
  taskId: string;
  phase: string;
  completed: number;
  total: number;
  message: string;
  status: TaskStatus;
  error: CommandErrorPayload | null;
  /** 业务任务完成时携带的结构化摘要；没有摘要的任务保持为空。 */
  result: Record<string, unknown> | null;
}

// ─── 签名更新 ─────────────────────────────────────────────────────────────────

/** 更新源固定为 Gitee，只控制更新请求的协议字段，不改变 Rust 内置签名信任根。 */
export type UpdateSourceMode = "gitee";
/** 构建产物绑定的更新通道；正式版和测试版不能互相选择。 */
export type UpdateChannel = "stable" | "test";
/** 应用、御魂目录、分析规则和读取适配器分别维护活动版本。 */
export type UpdatePackageType = "application" | "catalog" | "rules" | "adapter";

export interface UpdateMirrorSummary {
  source: "gitee";
  status: "valid" | "current" | "rejected" | "unavailable" | "unconfigured";
  version: string | null;
  contentSha256: string | null;
  signerKeyId: string | null;
  message: string | null;
}

/** 更新检查结果只把已校验候选的摘要交给界面，载荷仍留在 Rust 暂存目录。 */
export interface UpdateCandidateSummary {
  source: "gitee";
  channel: UpdateChannel;
  packageType: UpdatePackageType;
  packageId: string;
  version: string;
  contentSha256: string;
  contentSize: number;
  signerKeyId: string;
  signatureFingerprint: string;
  releaseNotes: string;
  stagedUpdateId: string;
}

/** 点击更新后的流式下载状态；总字节数来自 HTTPS 响应或签名清单。 */
export interface UpdateProgress {
  stagedUpdateId: string;
  phase: "downloading" | "verifying" | "installing";
  completed: number;
  total: number;
  message: string;
}

export interface UpdateCheckResult {
  sourceMode: UpdateSourceMode;
  channel: UpdateChannel;
  packageType: UpdatePackageType;
  currentVersion: string;
  selected: UpdateCandidateSummary | null;
  mirrors: UpdateMirrorSummary[];
}

export interface UpdateInstallResult {
  packageType: UpdatePackageType;
  channel: UpdateChannel;
  version: string;
  state: "deferred" | "pending_restart" | "restarting" | "activated" | "rolled_back";
  backupId: string | null;
  contentSha256: string;
  message: string;
}

export interface InstalledPackage {
  id: string;
  packageType: UpdatePackageType;
  version: string;
  sha256: string | null;
  signatureFingerprint: string | null;
  active: boolean;
  /** 历史版本可能保留旧镜像标记；新安装和新更新只会写入 gitee。 */
  source: "github" | "gitee" | null;
  contentPath: string | null;
  installedAt: string;
}

// ─── 数据层 DTO ────────────────────────────────────────────────────────────────

/** 游戏档案。 */
export interface GameProfile {
  id: string;
  displayName: string;
  sourceIdentity: string | null;
  serverLabel: string | null;
  maturityMode: string;
  maturityLevel: string | null;
  revision: number;
  createdAt: string;
  updatedAt: string;
  archivedAt: string | null;
}

/** 创建档案返回。 */
export interface ProfileCreated {
  profile: GameProfile;
}

/** 创建档案请求。 */
export interface CreateProfileRequest {
  displayName: string;
  sourceIdentity?: string | null;
  serverLabel?: string | null;
}

/** 重命名档案请求。 */
export interface RenameProfileRequest {
  profileId: string;
  newName: string;
  expectedRevision: number;
}

/** 归档请求。 */
export interface ArchiveProfileRequest {
  profileId: string;
  archived: boolean;
  expectedRevision: number;
}

/** 目录状态。 */
export interface CatalogEvidence {
  source: string;
  url: string | null;
  evidence: string;
}

/** 内置目录安装前后的可审计校验摘要。 */
export interface CatalogValidationResult {
  valid: boolean;
  errors: string[];
  setCount: number;
  shikigamiCount: number;
  iconCoverage: number;
  mechanicsCoverage: number;
  panelCoverage: number;
  skillCoverage: number;
  contentSha256: string;
}

/** 版本化目录包元数据，后续手动更新源沿用这一协议。 */
export interface CatalogPackage {
  version: string;
  installedAt: string;
  source: string | null;
  sha256: string | null;
  parserVersion: string | null;
  validationStatus: string;
}

export interface CatalogStatus {
  version: string;
  setCount: number;
  shikigamiCount: number;
  groupCount: number;
  source: string | null;
  sha256: string | null;
  parserVersion: string | null;
  validationStatus: string;
  validationErrors: string[];
  iconCoverage: number;
  mechanicsCoverage: number;
  panelCoverage: number;
  skillCoverage: number;
}

/** 御魂套装 DTO。 */
export interface SoulEnhancementCategoryProbability {
  id: string;
  label: string;
  probability: number;
  attributes: string[];
}

/** 六星副属性高档归一基准；该值不是出现概率。 */
export interface SoulRollBenchmark {
  attributeId: string;
  label: string;
  highValue: number;
  unit: "flat" | "rate";
}

/** 随目录版本保存的强化机械规则。 */
export interface SoulEnhancementRule {
  mechanicsVersion: string;
  checkpoints: number[];
  fourSubstatMode: {
    kind: "uniformExisting";
    perAttributeProbability: number;
    description: string;
  };
  incompleteMode: {
    kind: "addNewSubstat";
    description: string;
    categories: SoulEnhancementCategoryProbability[];
  };
  rollBenchmarks: SoulRollBenchmark[];
  probabilityNote: string;
  sourceRefs: string[];
}

/** 对外稳定名称；SoulEnhancementRule 保留以兼容已有页面代码。 */
export type EnhancementRule = SoulEnhancementRule;

/** 一键模拟强化请求；阈值只影响本批次烟花提示，seed 用于复现娱乐结果。 */
export interface SimulateEnhancementRequest {
  threshold: number;
  seed?: number;
}

/** 模拟强化后的一条副属性；最终值与参考文档区间同时返回，强化次数表达模拟命中次数。 */
export interface SimulationAttribute {
  attributeType: string;
  attributeLabel: string;
  value: number;
  valueMin: number;
  valueMax: number;
  enhancementCount: number;
}

/** 模拟强化在 +3/+6/+9/+12/+15 的单次命中轨迹。 */
export interface SimulationRoll {
  checkpoint: number;
  attributeType: string;
  attributeLabel: string;
  valueDelta: number;
  outcome: "existingAttribute" | "newAttribute";
  probabilityBasis: string;
}

/** 单枚六星未强化御魂模拟到 +15 的结果。 */
export interface SimulatedSoulResult {
  soulKey: string;
  setId: string;
  setName: string;
  slot: number;
  quality: number;
  level: number;
  mainAttrType: string;
  mainAttrValue: number;
  initialSubstatCount: number | null;
  finalAttributes: SimulationAttribute[];
  rolls: SimulationRoll[];
  standardScore: number;
  standardScoreRule: string;
  standardScoreFormula: string;
  firework: boolean;
}

/** 本批次模拟使用的概率和成本版本及证据。 */
export interface SimulationCostSummary {
  costVersion: string;
  mechanicsVersion: string;
  valueModelVersion: string;
  expPerSoul: number;
  goldPerSoul: number;
  probabilityNote: string;
  valueModelNote: string;
  valueModelSource: string;
  costNote: string;
  probabilitySource: string;
  costSource: string;
}

/** 一键模拟强化的完整批次结果；结果只存在于当前页面状态。 */
export interface SimulateEnhancementResult {
  seed: number;
  threshold: number;
  eligibleCount: number;
  totalRolls: number;
  totalExp: number;
  totalGold: number;
  entertainmentNote: string;
  cost: SimulationCostSummary;
  results: SimulatedSoulResult[];
}

// ─── 神奇海螺目标面板 ─────────────────────────────────────────────────────────

/** 神奇海螺目标；填写的每一项都表示最低值，百分比字段在接口中使用小数。 */
export interface MiracleTarget {
  attack?: number | null;
  hp?: number | null;
  defense?: number | null;
  speed?: number | null;
  critRate?: number | null;
  critDamage?: number | null;
  effectHit?: number | null;
  effectResist?: number | null;
}

/** 一行套装配方；散件使用 __scattered__，分类通配使用 __unrestricted__ 并携带 category。 */
export interface MiracleRecipeLine {
  setId: string;
  count: number;
  /** 具体套装未选择时的一级分类约束；具体套装行可不传。 */
  category?: string;
}

/** 神奇海螺计算请求。 */
export interface MiracleConchRequest {
  shikigamiId: string;
  target: MiracleTarget;
  /** 是否只纳入六星 +15 御魂；未传旧请求时由后端默认开启。 */
  onlyMaxLevel: boolean;
  /** 组合指标列表；每项都必须达标，空数组时沿用八项面板最低值模式。 */
  metrics: MiracleMetric[];
  /** 2/4/6 号位主属性限制；未填写的位置不限制主属性。 */
  mainAttributes: MiracleMainAttributeConstraint[];
  recipe: MiracleRecipeLine[];
}

/** 神奇海螺的一项可编辑指标；公式由指标预设提供，限制属性支持最低值和可选最高值。 */
export interface MiracleMetric {
  presetId: string;
  metricType: "product" | "minimum";
  label: string;
  operands: string[];
  threshold: number;
  percentage: boolean;
  constraints: MiracleMetricConstraint[];
}

/** 组合指标中的面板属性区间，例如暴击 100%～120%、速度 152～180。 */
export interface MiracleMetricConstraint {
  attribute: string;
  minimum: number;
  /** 未填写时不限制上限；使用小数表示百分比属性。 */
  maximum?: number | null;
}

/** 指定号位必须使用的主属性；slot 只允许 2、4、6。 */
export interface MiracleMainAttributeConstraint {
  slot: 2 | 4 | 6;
  mainAttrType: string;
}

/** 八项静态面板；暴击、暴伤、命中和抵抗均为小数。 */
export interface MiraclePanel {
  attack: number;
  hp: number;
  defense: number;
  speed: number;
  critRate: number;
  critDamage: number;
  effectHit: number;
  effectResist: number;
}

/** 目标缺口；未设置目标的属性通常为零。 */
export type MiraclePanelGap = MiraclePanel;

/** 结果中的御魂副属性。 */
export interface MiracleAttributeView {
  /** 属性在原始御魂中的稳定顺序；用于保持“我的御魂”的展示顺序。 */
  attributeIndex: number;
  attributeType: string;
  value: number;
  enhancementCount: number | null;
  countProvenance: string;
  fixedAttribute: boolean;
}

/** 结果中的单枚御魂摘要。 */
export interface MiracleSoulView {
  soulKey: string;
  setId: string;
  slot: number;
  quality: number;
  level: number;
  mainAttrType: string;
  mainAttrValue: number;
  equippedState: string | null;
  initialSubstatCount: number | null;
  attributes: MiracleAttributeView[];
}

/** 已激活但未计入静态面板的套装效果，或已计入的二件套面板加成。 */
export interface MiracleSetEffectNote {
  setId: string;
  setName: string;
  pieceCount: number;
  kind: string;
  effect: string;
  countedInPanel: boolean;
}

/** 一套六件组合及其可解释结果。 */
export interface MiracleSolution {
  souls: MiracleSoulView[];
  setCounts: Record<string, number>;
  panel: MiraclePanel;
  gap: MiraclePanelGap;
  metAttributes: string[];
  unmetAttributes: string[];
  metric: MiracleMetricResult | null;
  metrics: MiracleMetricResult[];
  countedSetEffects: MiracleSetEffectNote[];
  uncountedSetEffects: MiracleSetEffectNote[];
  approximate: boolean;
}

/** 方案实际指标值及其目标缺口。 */
export interface MiracleMetricResult {
  presetId: string;
  label: string;
  expression: string;
  actual: number;
  threshold: number;
  gap: number;
  met: boolean;
  percentage: boolean;
  constraints: MiracleMetricConstraintResult[];
}

/** 组合指标中一项属性限制的实际值与缺口。 */
export interface MiracleMetricConstraintResult {
  attribute: string;
  actual: number;
  minimum: number;
  /** 未设置上限时为 null。 */
  maximum?: number | null;
  gap: number;
  met: boolean;
}

/** 最佳组合中各号位对目标缺口的影响。 */
export interface MiracleSlotGap {
  slot: number;
  impactScore: number;
  missingAttributes: string[];
  reason: string;
}

/** 期望强化后的副属性展示项；前端据此高亮变化属性和预计强化手数。 */
export interface MiracleExpectedAttribute {
  attributeIndex: number;
  attributeType: string;
  currentValue: number;
  expectedValue: number;
  expectedAddedValue: number;
  /** 该属性在典型强化轨迹中命中的整数手数；三腿新增属性包含必然新增的第一手。 */
  expectedEnhancementCount: number;
  fixedAttribute: boolean;
  isNewAttribute: boolean;
  /** 新增属性的归一化概率；已有属性为 null。 */
  probability: number | null;
}

/** 可继续强化的御魂胚子及理论上界。 */
export interface MiracleEnhancementCandidate {
  /** 当前方案中被替换的原始御魂，用于和推荐胚子并排对照。 */
  originalSoul: MiracleSoulView;
  soul: MiracleSoulView;
  slot: number;
  currentGapScore: number;
  theoreticalGapScore: number;
  possibleImprovement: number;
  /** 用于计算“强化前 → 理论上限”的面板收益，避免前端猜测候选替换后的当前值。 */
  currentPanel: MiraclePanel;
  currentGap: MiraclePanelGap;
  /** 理论强化上限对应的面板缺口，用于明确展示预期还能补多少。 */
  theoreticalGap: MiraclePanelGap;
  theoreticalPanel: MiraclePanel;
  /** 与理论面板同一条最有利强化路径上的副属性明细，供强化卡片逐条展示。 */
  theoreticalAttributes: MiracleExpectedAttribute[];
  /** 按新增属性类别概率和后续四腿均匀强化得到的平均收益；不是单次随机结果。 */
  expectedImprovement: number;
  expectedGap: MiraclePanelGap;
  expectedPanel: MiraclePanel;
  expectedAttributes: MiracleExpectedAttribute[];
  currentMetric: MiracleMetricResult | null;
  theoreticalMetric: MiracleMetricResult | null;
  expectedMetric: MiracleMetricResult | null;
  currentMetrics: MiracleMetricResult[];
  theoreticalMetrics: MiracleMetricResult[];
  expectedMetrics: MiracleMetricResult[];
  /** 后端稳定排序给出的名次依据，避免页面只能看到无法复核的顺序。 */
  rankExplanation: string;
  reason: string;
}

/** 库存不足时的购买方向，不绑定商店或掉落来源。 */
export interface MiraclePurchaseSuggestion {
  slot: number;
  setDirection: string;
  mainAttribute: string;
  prioritySubAttributes: string[];
  missingAttribute: string;
  priorityScore: number;
  reason: string;
}

/** 神奇海螺完整计算结果。 */
export interface MiracleConchResult {
  status: "matched" | "closest" | "no_candidate";
  searchComplete: boolean;
  candidateCount: number;
  excludedUnconfirmedCount: number;
  excludedNonSixStarCount: number;
  solutions: MiracleSolution[];
  slotGaps: MiracleSlotGap[];
  enhancementCandidates: MiracleEnhancementCandidate[];
  purchaseSuggestions: MiraclePurchaseSuggestion[];
  qualityNotes: string[];
}

/** 御魂套装 DTO；图标编号对应前端打包的离线图片。 */
export interface SoulSet {
  catalogVersion: string;
  setId: string;
  name: string;
  iconAssetId: number | null;
  category: string | null;
  specialCategory: string | null;
  twoPieceEffect: string | null;
  fourPieceEffect: string | null;
  enhancementRule: SoulEnhancementRule | null;
  evidence: CatalogEvidence[];
}

/** 式神稀有度；UR、SP 与 SSR 分开，保持图鉴筛选和排序与游戏资料一致。 */
/** 素材是没有培养面板的目录条目，单独作为最后一档展示。 */
export type ShikigamiRarity = "UR" | "SP" | "SSR" | "SR" | "R" | "N" | "素材";

/** 面板评级；官方资料可能出现 D/C/B/A/S/SS/SSS 七档基础属性等级。 */
export type ShikigamiPanelGrade = "SSS" | "SS" | "S" | "A" | "B" | "C" | "D";

/** 满级式神面板评级；暴伤没有独立的官方七档评级字段。 */
export interface ShikigamiPanelGrades {
  attack: ShikigamiPanelGrade;
  hp: ShikigamiPanelGrade;
  defense: ShikigamiPanelGrade;
  speed: ShikigamiPanelGrade;
  critRate: ShikigamiPanelGrade;
}

/** 式神基础面板；数值固定为满级 40 级，不包含御魂和队伍增益。 */
export interface ShikigamiPanel {
  attack: number;
  hp: number;
  defense: number;
  speed: number;
  critRate: number;
  critDamage: number;
  grades: ShikigamiPanelGrades;
}

/** 单个技能的单级效果；level=1 保存基础效果，后续等级保存该级新增效果。 */
export interface ShikigamiSkillLevel {
  level: number;
  effect: string;
  upgradeEffect: string | null;
}

/** 式神技能资料，技能顺序与游戏内顺序一致。 */
export interface ShikigamiSkill {
  skillId: string;
  name: string;
  type: "普攻" | "主动" | "被动" | "特殊";
  energyCost: number;
  levels: ShikigamiSkillLevel[];
}

/** 推荐就业场景；标签用于筛选，说明用于解释推荐原因。 */
export interface ShikigamiEmploymentScene {
  mode: "PVE" | "PVP";
  scene: string;
  recommendation: string;
}

/** 从 111 起算的技能投入建议；黑蛋数量由目标等级差计算得到。 */
export interface ShikigamiSkillInvestment {
  targetLevels: number[];
  priority: number[];
  blackEggsFrom111: number;
  note: string;
}

/** 式神目录条目；它是只读基础资料，不代表玩家已拥有或已培养的式神。 */
export interface Shikigami {
  catalogVersion: string;
  shikigamiId: string;
  name: string;
  rarity: ShikigamiRarity;
  iconAssetId: string | null;
  panelLevel: number;
  awakened: boolean;
  panel: ShikigamiPanel;
  skills: ShikigamiSkill[];
  skillInvestment: ShikigamiSkillInvestment;
  employmentScenes: ShikigamiEmploymentScene[];
  gameVersion: string;
  evidence: CatalogEvidence[];
}

/** 玩家持有的一只式神；与静态 Shikigami 目录分开，避免把“拥有”和“资料”混为一谈。 */
export interface MyShikigami {
  instanceId: string;
  shikigamiId: string;
  star: number;
  level: number | null;
  exp: number | null;
  locked: boolean | null;
  awakened: boolean | null;
  skinId: number | null;
  skills: Array<{
    skillId: number;
    level: number;
  }>;
  selectedSkillIds: number[];
}

/**
 * 式神仓库中的一组素材式神（达摩、素材怪等）。
 *
 * 游戏把这类式神压缩成「分组 → 数量」存储，没有逐只实例，因此没有等级、
 * 御魂和技能。游戏式神录顶部的总数包含这部分。
 */
export interface ShikigamiBagGroup {
  shikigamiId: string;
  groupKey: string;
  count: number;
}

/** 当前账号的目标式神碎片；只包含 SSR、SP、UR 和御行达摩。 */
export interface ShikigamiShardEntry {
  shikigamiId: string;
  shardCount: number;
  maxShardCount: number;
}

/** 当前账号的一条传记进度；式神录按 index 展示传记一、二、三。 */
export interface ShikigamiBiographyProgress {
  index: number;
  activityId: string;
  current: number;
  required: number;
  remaining: number;
  completed: boolean;
}

/** 传记二所对应的公开规则类型；unknown 表示没有可靠的外部条件，不能换算黑蛋。 */
export type ShikigamiUnlockProgressKind =
  | "skillUpgrade"
  | "level"
  | "battleWin"
  | "teamBattle"
  | "shardCollection"
  | "clanPrayer"
  | "awakening"
  | "other"
  | "unknown";

/** 当前账号按式神保存的全部传记解锁进度；旧快照没有对应条目时不会返回记录。 */
export interface ShikigamiStoryProgress {
  shikigamiId: string;
  activityId: string;
  current: number;
  required: number;
  remaining: number;
  unlocked: boolean;
  progressKind: ShikigamiUnlockProgressKind;
  /** 公开规则表中的传记二条件；规则未收录或目标不一致时为空。 */
  unlockCondition: string | null;
  /** 规则来源标识，界面用来提示来源时效。 */
  unlockRuleSource: string | null;
  /** 未解锁时完成传记二可获得的碎片数；已解锁时为空。 */
  unlockableShardCount: number | null;
  biographies: ShikigamiBiographyProgress[];
}

/** 跨角色碎片查询中的单个游戏档案；不可用档案不能被解释为“未拥有”。 */
export interface ShikigamiShardLookupCharacter {
  identityKey: string;
  profileId: string | null;
  displayName: string;
  platform: GamePlatform | null;
  playerId: string | null;
  serverLabel: string | null;
  lastUpdatedAt: string | null;
  dataAvailable: boolean;
  warning: string | null;
  ownedCount: number;
  shardCount: number;
  maxShardCount: number;
  /** 是否已完成传记二并解锁碎片；旧档案没有该字段时为 null。 */
  shardUnlocked: boolean | null;
  /** 传记二对应的公开条件类型；unknown 时黑蛋剩余数必须保持未知。 */
  shardUnlockProgressKind: ShikigamiUnlockProgressKind | null;
  /** 公开规则表中的传记二条件，例如“升至40级”。 */
  shardUnlockCondition: string | null;
  /** 规则来源标识；为空时表示没有匹配到可靠规则。 */
  shardUnlockRuleSource: string | null;
  shardUnlockCurrent: number | null;
  shardUnlockRequired: number | null;
  shardUnlockRemaining: number | null;
  /** 未解锁时完成传记二可获得的碎片数，已解锁或旧档案为 null。 */
  unlockableShardCount: number | null;
}

/** 一个目标式神在全部本机游戏档案中的只读查询结果。 */
export interface ShikigamiShardLookupResult {
  shikigamiId: string;
  characters: ShikigamiShardLookupCharacter[];
}

/** 我的御魂中的单条副属性；字段来自导入快照的规范事实。 */
export interface MySoulAttribute {
  attributeIndex: number;
  attributeType: string;
  value: number;
  enhancementCount: number | null;
  countProvenance: string;
  fixedAttribute: boolean;
}

/** 当前导入库存中的单枚御魂；setId 与御魂目录共用稳定编号。 */
export interface MySoul {
  soulKey: string;
  snapshotId: string;
  soulInternalId: string;
  sourceStableId: string | null;
  identityQuality: string;
  setId: string;
  slot: number;
  quality: number;
  level: number;
  mainAttrType: string;
  mainAttrValue: number;
  initialSubstatCount: number | null;
  lockedInSource: boolean | null;
  equippedState: string | null;
  presenceState: string;
  attributes: MySoulAttribute[];
}

/** “我的御魂”分页筛选选项；后端只返回去重后的稳定值，不返回额外御魂正文。 */
export interface MySoulFilterOptions {
  setIds: string[];
  slots: number[];
  qualities: number[];
  levels: number[];
  mainAttrTypes: string[];
  subAttrTypes: string[];
}

/** “我的御魂”分页结果；数值筛选支持通过 hasMore 追加下一批御魂。 */
export interface MySoulPage {
  /** 当前页实际所属的游戏档案；评分任务必须复用该 ID。 */
  profileId: string | null;
  items: MySoul[];
  total: number;
  /** 副属性数值筛选还有下一批结果时为 true。 */
  hasMore: boolean;
  inventoryTotal: number;
  /** 当前库存包含的读取来源；标题旁显示 MuMu/桌面版标志。 */
  sourceKinds: string[];
  limit: number;
  offset: number;
  filterOptions: MySoulFilterOptions;
  scoreCount: number;
}

/** 御魂雷达中某个套装、号位和指标的最高副属性；二四六号位附带主属性。 */
export interface SoulRadarPoint {
  setId: string;
  slot: number;
  metricType: string;
  value: number;
  mainAttrType: string | null;
}

/** 已保存的雷达结果及当前库存版本；版本不一致时页面提示重新计算。 */
export interface SoulRadarCache {
  profileId: string;
  calculatedAt: string;
  inventoryRevision: string;
  inventoryCount: number;
  currentInventoryRevision: string;
  currentInventoryCount: number;
  points: SoulRadarPoint[];
}

/** 头尾卡片中的副属性事实；比例属性仍按后端小数保存。 */
export interface HeadTailAttribute {
  attributeType: string;
  value: number;
  enhancementCount: number | null;
  fixedAttribute: boolean;
}

/** 单枚头尾候选御魂；只有明确记录速度强化 5 次才入选，并按速度倒序返回。 */
export interface HeadTailCard {
  soulKey: string;
  setId: string;
  slot: number;
  quality: number;
  level: number;
  mainAttrType: string;
  mainAttrValue: number;
  speed: number;
  speedRolls: number;
  attributes: HeadTailAttribute[];
}

/** 已保存的头尾分析结果；库存版本不一致时页面要求用户重新计算。 */
export interface HeadTailCache {
  profileId: string;
  calculatedAt: string;
  inventoryRevision: string;
  inventoryCount: number;
  currentInventoryRevision: string;
  currentInventoryCount: number;
  heads: HeadTailCard[];
  tails: HeadTailCard[];
  headCandidateCount: number;
  tailCandidateCount: number;
}

/** “我的御魂”列表使用的轻量评分摘要；完整解释在分析待办中按需查看。 */
export interface StandardScoreContribution {
  attributeType: string;
  attributeName: string;
  kind: "core" | "general";
  baseWeight: number;
  enhancementCount: number;
  enhancementWeight: number;
  contribution: number;
}

/** “我的御魂”列表使用的轻量评分摘要；完整解释在分析待办中按需查看。 */
export interface MySoulScore {
  soulKey: string;
  standardScore: number | null;
  standardScoreRule: string | null;
  standardScoreFormula: string | null;
  standardScoreContributions: StandardScoreContribution[];
  compositeScore: number | null;
  bestScore: number | null;
  bestUseId: string | null;
  bestUseTitle: string | null;
  effectiveGrowthCount: number | null;
  scoredUseCount: number;
  recommendation: string;
  category: string;
  generatedAt: string;
  standardId: string | null;
  standardVersion: string | null;
  standardTitle: string | null;
  /** 实际参与评分的规范规则正文哈希，用于识别保存标准后的旧结果。 */
  standardHash: string | null;
}

/** 成品成长质量的固定档位边界；maxScore 为空表示极高档位没有上限。 */
export interface GrowthQualityTierBoundary {
  label: string;
  minScore: number;
  maxScore: number | null;
}

/** 成品成长质量中的单条可强化副属性；固定属性不会从后端返回。 */
export interface GrowthQualityAttribute {
  attributeType: string;
  attributeLabel: string;
  /** 原始精确值；比例属性按 0～1 保存。 */
  value: number;
  enhancementCount: number | null;
  handCount: number | null;
  singleRollQuality: number | null;
}

/** 单枚六星 +15 御魂的成长质量结果。 */
export interface GrowthQualitySoul {
  soulKey: string;
  setId: string;
  slot: number;
  quality: number;
  level: number;
  mainAttrType: string;
  mainAttrValue: number;
  attributes: GrowthQualityAttribute[];
  totalGrowthScore: number | null;
  tier: string | null;
  scoreNote: string | null;
}

/** 当前档案的六星 +15 成品成长质量报告。 */
export interface GrowthQualityReport {
  modelVersion: string;
  evidenceStatus: string;
  modelNote: string;
  handCount: number;
  tierBoundaries: GrowthQualityTierBoundary[];
  candidateCount: number;
  scoreableCount: number;
  unknownDataCount: number;
  /** 局部快照中未确认、未纳入当前库存分析的御魂数量。 */
  excludedUnconfirmedCount: number;
  items: GrowthQualitySoul[];
}

/** 胚子同类 +15 对照项；value 保留后端比较使用的精确小数。 */
export interface EmbryoComparisonItem {
  soulKey: string;
  value: number;
}

/** 三腿新增类别下的具体属性可能分支；未知值使用 null，不填充为零。 */
export interface EmbryoPossibleAttributePath {
  attributeType: string;
  attributeLabel: string;
  theoreticalUpper: number | null;
  usualExpected: number | null;
  hitProbability: number | null;
  comparisonValue: number | null;
  comparisonSource: "inventory" | "average-threshold" | "unavailable";
  comparisonItems: EmbryoComparisonItem[];
  comparisonCandidateCount: number;
  theoreticalExcess: number | null;
  usualExcess: number | null;
  conclusion: "stop" | "high-risk" | "worth-it" | null;
  dataNote: string;
}

/** 六星 +0 三腿或四腿胚子的一条独立副属性强化路径。 */
export interface EmbryoDecisionPath {
  pathId: string;
  soulKey: string;
  setId: string;
  slot: number;
  quality: number;
  level: number;
  mainAttrType: string;
  legCount: number;
  pathKind: "existing-attribute" | "new-category";
  attributeIndex: number | null;
  attributeType: string;
  attributeLabel: string;
  initialValue: number | null;
  theoreticalUpper: number | null;
  usualExpected: number | null;
  /** 已有属性路径保存完整命中概率；新增类别路径由 categoryProbability 表示。 */
  hitProbability: number | null;
  comparisonValue: number | null;
  comparisonSource: "inventory" | "average-threshold" | "category-branches";
  comparisonItems: EmbryoComparisonItem[];
  comparisonCandidateCount: number;
  theoreticalExcess: number | null;
  usualExcess: number | null;
  conclusion: "stop" | "high-risk" | "worth-it";
  possibleAttributes: EmbryoPossibleAttributePath[];
  categoryProbability: number | null;
  dataNote: string;
}

/** 当前档案所选腿数的六星 +0 胚子强化决策报告。 */
export interface EmbryoDecisionReport {
  modelVersion: string;
  evidenceStatus: string;
  modelNote: string;
  remainingHandCount: number;
  selectedLegCounts: number[];
  candidateCount: number;
  pathCount: number;
  excludedUnconfirmedCount: number;
  paths: EmbryoDecisionPath[];
}

/** 胚子分析的腿数选择；界面默认只勾选四腿。 */
export interface EmbryoDecisionRequest {
  includeThreeLeg: boolean;
  includeFourLeg: boolean;
}

/** 有效常用度。 */
export interface EffectiveCommonness {
  setId: string;
  scenario: string;
  value: string;
  isOverride: boolean;
}

/** 常用度覆盖请求。 */
export interface CommonnessOverrideRequest {
  profileId: string;
  setId: string;
  scenario: string;
  value: string;
}

/** 复制常用度请求。 */
export interface CopyCommonnessRequest {
  fromProfileId: string;
  toProfileId: string;
}

/** 存储原始对象请求。 */
export interface StoreRawRequest {
  profileId: string;
  sourceKind: string;
  sourceFormat: string;
  mediaType: string;
  payload: number[];
  receivedAt: string;
}

/** 存储原始对象返回。 */
export interface StoredRawObject {
  sha256: string;
  rawSize: number;
  storedSize: number;
  isNew: boolean;
}

/** 备份记录。 */
export interface BackupRecord {
  id: string;
  path: string;
  schemaVersion: number;
  fileCount: number;
  totalSize: number;
  reason: string;
  checksum: string | null;
  createdAt: string;
  restoredAt: string | null;
  verified: boolean;
}

/** 创建备份请求。 */
export interface CreateBackupRequest {
  reason: string;
}

// ─── 规则引擎 ─────────────────────────────────────────────────────────────────

/** 默认规则预设的稳定摘要；哈希用于追溯分析输入版本。 */
export interface RulePresetSummary {
  id: string;
  version: string;
  title: string;
  author: string;
  status: "draft" | "verified" | "deprecated";
  ruleCount: number;
  normalizedHash: string;
}

/** 规则引擎内置用途模板的证据状态。 */
export interface BuiltinUseTemplate {
  id: string;
  title: string;
  status: "draft" | "verified" | "deprecated";
  evidenceLevel:
    "official" | "open_source" | "author" | "community" | "inferred" | "unverified";
}

// ─── 规则编辑、版本与离线分享 ─────────────────────────────────────────────────

/** 规则版本摘要；正文只在用户打开编辑器或导出时按需读取。 */
export interface RuleVersion {
  id: string;
  presetId: string;
  version: string;
  title: string;
  author: string;
  status: "draft" | "verified" | "deprecated";
  origin: "builtin" | "imported" | "user";
  normalizedHash: string;
  ruleCount: number;
  readOnly: boolean;
  parentVersionId: string | null;
  createdAt: string;
  enabled: boolean;
  scoreColorThresholds: ScoreColorThresholds;
}

/** 评分标准控制“我的御魂”列表的颜色分级。 */
export interface ScoreColorThresholds {
  jadeScore: number;
  goldScore: number;
  rainbowScore: number;
}

/** 规则导入来源；两种来源最终走同一份 Rust 校验器。 */
export type RuleSourceKind = "file" | "shareCode";

/** 导入前规则元数据与风险提示。 */
export interface RulePresetPreview {
  sourceKind: RuleSourceKind;
  id: string;
  version: string;
  title: string;
  author: string;
  status: "draft" | "verified" | "deprecated";
  normalizedHash: string;
  ruleCount: number;
  readOnly: boolean;
  warnings: string[];
}

/** 规则对当前档案分析缓存的命中摘要。 */
export interface RuleImpactPreview {
  inventoryCount: number;
  matchingCount: number;
  sampleSoulKeys: string[];
  overlapCount: number;
  changedCount: number;
  warnings: string[];
}

/** 规则导出结果。 */
export interface RuleExport {
  fileContent: string;
  shareCode: string;
}

/** 分析中心的用途覆盖指标。 */
export interface UsageCoverageMetric {
  useId: string;
  title: string;
  usableCount: number;
  observingCount: number;
  gapCount: number;
  drillDownCategory: string | null;
  drillDownSearch: string;
}

export interface AttributeDistributionMetric {
  attributeType: string;
  count: number;
}

export interface SetStructureMetric {
  setId: string;
  count: number;
  pveCommon: boolean;
  pvpCommon: boolean;
}

export interface EnhancementFunnelStage {
  stage: string;
  count: number;
  drillDownCategory: string | null;
}

export interface CleanupYield {
  candidateCount: number;
  confirmedRemovedCount: number;
  unconfirmedCount: number;
  pendingCount: number;
}

export interface DataQualitySummary {
  currentBaselineComplete: boolean;
  unknownEnhancementCount: number;
  draftRuleCount: number;
  staleAnalysisCount: number;
  affectedSnapshotCount: number;
}

/** 分析中心总览；图表点击可使用 drillDownCategory 回到待办筛选。 */
export interface AnalysisCenter {
  profileId: string;
  currentBaselineSnapshotId: string | null;
  inventoryCount: number;
  presentCount: number;
  unconfirmedCount: number;
  removedCount: number;
  usageCoverage: UsageCoverageMetric[];
  attributeDistribution: AttributeDistributionMetric[];
  setStructure: SetStructureMetric[];
  enhancementFunnel: EnhancementFunnelStage[];
  cleanupYield: CleanupYield;
  dataQuality: DataQualitySummary;
}

/** 后端生成的隔离导出载荷；前端只负责触发浏览器下载。 */
export interface ExportPayload {
  fileName: string;
  mediaType: string;
  content: string;
  profileId: string;
  snapshotId: string | null;
  recordCount: number;
}

// ─── 分析待办、用户决定与行动批次 ─────────────────────────────────────────────

/** 分析待办：系统建议和用户决定分栏保存，详情包含逐用途解释树。 */
export interface AnalysisTodo {
  id: string;
  profileId: string;
  soulKey: string;
  snapshotId: string;
  soulInternalId: string;
  setId: string;
  slot: number;
  quality: number;
  level: number;
  mainAttribute: string;
  mainValue: number;
  standardScore: number | null;
  compositeScore: number | null;
  category: string;
  recommendation: "keep" | "observe" | "stop" | "recycle";
  reasonSummary: string;
  evidenceLevel: string;
  dataQuality: string;
  presenceState: string;
  isNew: boolean;
  isChanged: boolean;
  detail: Record<string, unknown> | null;
  explanationHash: string;
  generatedAt: string;
  revision: number;
  userDecision: string | null;
  userDecisionNote: string | null;
  decisionRevision: number | null;
}

/** 待办服务端分页结果。 */
export interface AnalysisTodoPage {
  items: AnalysisTodo[];
  total: number;
  limit: number;
  offset: number;
}

/** 重新计算分析请求。 */
export interface RecalculateAnalysisRequest {
  profileId: string;
  snapshotId?: string;
}

/** 待办筛选请求。 */
export interface ListAnalysisTodosRequest {
  profileId: string;
  category?: string;
  search?: string;
  useId?: string;
  limit?: number;
  offset?: number;
}

/** 用户决定变化；decision 为空表示清除我的决定。 */
export interface DecisionChange {
  soulKey: string;
  decision: string | null;
  note?: string;
}

/** 批量决定预览统计。 */
export interface DecisionPreview {
  selectedCount: number;
  addedCount: number;
  overwrittenCount: number;
  skippedCount: number;
  protectedCount: number;
  conflictCount: number;
}

/** 批量决定事务返回值。 */
export interface DecisionApplyResult extends DecisionPreview {
  operationId: string;
}

/** 用户决定历史。 */
export interface DecisionHistoryEntry {
  id: string;
  operationId: string;
  profileId: string;
  soulKey: string;
  previousDecision: string | null;
  nextDecision: string | null;
  note: string | null;
  createdAt: string;
}

/** 行动批次中的筛选分组。 */
export interface BatchGroup {
  groupKey: string;
  setId: string;
  slot: number;
  mainAttribute: string;
  level: number;
  itemCount: number;
}

/** 行动批次摘要。 */
export interface ActionBatch {
  id: string;
  profileId: string;
  kind: "strengthen" | "cleanup";
  status: string;
  targetLevel: number | null;
  snapshotId: string | null;
  groupCount: number;
  itemCount: number;
  completedCount: number;
  skippedCount: number;
  notFoundCount: number;
  createdAt: string;
  updatedAt: string;
  completedAt: string | null;
  groups: BatchGroup[];
}

/** 行动批次条目。 */
export interface BatchItem {
  batchId: string;
  soulKey: string;
  groupKey: string;
  setId: string;
  slot: number;
  mainAttribute: string;
  level: number;
  status: string;
  sortOrder: number;
  note: string | null;
  completedAt: string | null;
}

/** 行动批次详情。 */
export interface ActionBatchDetail {
  batch: ActionBatch;
  items: BatchItem[];
}

/** 创建行动批次。 */
export interface CreateActionBatchRequest {
  profileId: string;
  kind: "strengthen" | "cleanup";
  targetLevel?: number;
  soulKeys: string[];
}
