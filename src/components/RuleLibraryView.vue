<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import type {
  RuleExport,
  RulePresetPreview,
  RuleSourceKind,
  RuleVersion,
  SoulSet,
} from "../api/contracts";
import { normalizeCommandError } from "../api/errors";
import {
  ATTRIBUTE_LABELS,
  ATTRIBUTE_OPTIONS,
  MAIN_ATTRIBUTES_BY_SLOT,
  buildManualPreset,
  createManualRuleCard,
  parseManualRuleCards,
  parseScoreColorThresholds,
  stripCanonicalHash,
  validateScoreColorThresholds,
  validateManualCards,
  type ManualPresetMeta,
  type ManualRuleCardDraft,
  type RuleScenario,
  type ScoreColorThresholdsDraft,
} from "../rulePresetBuilder";

// 图标与御魂档案共用同一份离线资源，套装选择器不发起网络请求。
const iconModules = import.meta.glob("../assets/soul-icons/*.png", {
  eager: true,
  import: "default",
  query: "?url",
});

// 规则库页面只维护当前编辑草稿；正文和全局启用状态都交给 Rust 持久化。
const sets = ref<SoulSet[]>([]);
const versions = ref<RuleVersion[]>([]);
const cards = ref<ManualRuleCardDraft[]>([]);
// 折叠状态只属于当前编辑页面，不写入规则正文；没有本地记录时默认全部展开。
const collapsedCardIds = ref<Set<string>>(new Set());
const selectedVersionId = ref<string | null>(null);
const selectedPreview = ref<RulePresetPreview | null>(null);
const exportResult = ref<RuleExport | null>(null);
const editorDocument = ref<Record<string, unknown>>({});
const editorPayload = ref<number[]>([]);
const editorParentId = ref<string | undefined>();
const editorTitle = ref("");
const editorAuthor = ref("本机用户");
const editorDescription = ref("");
// 评分标准本身保存列表颜色分级，御魂列表读取启用标准后按此配置展示。
const scoreColorThresholds = ref<ScoreColorThresholdsDraft>({
  jadeScore: 60,
  goldScore: 70,
  rainbowScore: 75,
});
const editorMode = ref<"simple" | "advanced">("simple");
const advancedJson = ref("");
const shareCode = ref("");
const sourceKind = ref<RuleSourceKind>("file");
const fileInput = ref<HTMLInputElement | null>(null);
const activeSetPickerId = ref<string | null>(null);
const setSearch = ref("");
const setCategory = ref("全部");
const error = ref("");
const notice = ref("");
const loading = ref(false);
const busy = ref(false);

const selectedVersion = computed(
  () =>
    versions.value.find((version) => version.id === selectedVersionId.value) ?? null,
);
const enabledCount = computed(
  () => versions.value.filter((version) => version.enabled).length,
);
const canEdit = computed(
  () => !selectedVersion.value || !selectedVersion.value.readOnly,
);
// 一级分类优先使用特殊效果分类，首领/星痕套装不会再次混入普通二件套分类。
const setCategories = computed(() => [
  "全部",
  ...new Set(
    sets.value
      .map((set) => set.specialCategory ?? set.category)
      .filter((category): category is string => Boolean(category)),
  ),
]);
const visibleSets = computed(() => {
  const keyword = setSearch.value.trim().toLocaleLowerCase("zh-CN");
  return sets.value.filter((set) => {
    const category = set.specialCategory ?? set.category;
    const categoryMatched =
      setCategory.value === "全部" || category === setCategory.value;
    const keywordMatched =
      !keyword || set.name.toLocaleLowerCase("zh-CN").includes(keyword);
    return categoryMatched && keywordMatched;
  });
});
const cardCountLabel = computed(() => `${cards.value.length} 条手动规则`);
const scenarioOptions: RuleScenario[] = ["all", "pve", "pvp"];

const COLLAPSE_STORAGE_KEY = "yys-analysis.rule-card-collapse.v1";

/** 读取本机界面偏好；异常或旧格式数据按空对象处理，不影响规则正文加载。 */
function readCollapsePreferences(): Record<string, string[]> {
  if (typeof window === "undefined") return {};
  try {
    const raw = window.localStorage.getItem(COLLAPSE_STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    return Object.fromEntries(
      Object.entries(parsed).map(([key, value]) => [
        key,
        Array.isArray(value)
          ? value.filter((item): item is string => typeof item === "string")
          : [],
      ]),
    );
  } catch {
    return {};
  }
}

/** 当前标准使用独立偏好键，避免不同评分标准的卡片 ID 互相污染折叠状态。 */
function collapsePreferenceId(parsed?: Record<string, unknown>): string {
  const documentId =
    parsed && typeof parsed.id === "string" && parsed.id.trim()
      ? parsed.id
      : "new-standard";
  return selectedVersionId.value ?? documentId;
}

/** 保存当前标准的折叠卡片 ID；界面偏好不进入规则 JSON，也不会影响分享码。 */
function saveCollapsePreferences(): void {
  if (typeof window === "undefined") return;
  try {
    const preferences = readCollapsePreferences();
    preferences[collapsePreferenceId(editorDocument.value)] = [
      ...collapsedCardIds.value,
    ];
    window.localStorage.setItem(COLLAPSE_STORAGE_KEY, JSON.stringify(preferences));
  } catch {
    // 本机存储不可用时仅放弃记忆折叠状态，不阻断规则编辑和保存。
  }
}

/** 加载当前标准上次的折叠状态，并过滤已经不存在的规则卡片。 */
function restoreCollapsePreferences(parsed: Record<string, unknown>): void {
  const saved = readCollapsePreferences()[collapsePreferenceId(parsed)] ?? [];
  const validIds = new Set(cards.value.map((card) => card.id));
  collapsedCardIds.value = new Set(saved.filter((id) => validIds.has(id)));
}

function statusLabel(status: RuleVersion["status"]): string {
  return { draft: "草案", verified: "已核验", deprecated: "已废弃" }[status];
}

function originLabel(origin: RuleVersion["origin"]): string {
  return { builtin: "系统内置", imported: "离线导入", user: "我的规则" }[origin];
}

function setError(caught: unknown): void {
  const commandError = normalizeCommandError(caught);
  error.value = `${commandError.code} · ${commandError.message}`;
}

function attributeLabel(attribute: string): string {
  return ATTRIBUTE_LABELS[attribute] ?? attribute;
}

function scenarioLabel(scenario: RuleScenario): string {
  return { all: "全部场景", pve: "PVE", pvp: "PVP" }[scenario];
}

function soulIcon(set: SoulSet): string | null {
  if (!set.iconAssetId) return null;
  return iconModules[`../assets/soul-icons/${set.iconAssetId}.png`] ?? null;
}

function setName(setId: string): string {
  return sets.value.find((set) => set.setId === setId)?.name ?? setId;
}

/** 返回已选套装的目录对象，已选标签与候选面板共用同一份图标数据。 */
function selectedSet(setId: string): SoulSet | undefined {
  return sets.value.find((set) => set.setId === setId);
}

function setPickerLabel(card: ManualRuleCardDraft): string {
  if (!card.setIds.length) return "全部套装";
  return `已选择 ${card.setIds.length} 套`;
}

function mainAttributesForSlot(slot: number): string[] {
  return MAIN_ATTRIBUTES_BY_SLOT[slot] ?? [];
}

function hasMainAttribute(
  card: ManualRuleCardDraft,
  slot: number,
  attribute: string,
): boolean {
  return card.slotMainPairs.some(
    (pair) => pair.slot === slot && pair.mainAttribute === attribute,
  );
}

function toggleSet(card: ManualRuleCardDraft, setId: string): void {
  if (card.setIds.includes(setId)) {
    card.setIds = card.setIds.filter((item) => item !== setId);
  } else {
    card.setIds = [...card.setIds, setId];
  }
}

/** 批量加入当前搜索结果，避免玩家逐个点选一整类套装。 */
function selectVisibleSets(card: ManualRuleCardDraft): void {
  const ids = new Set(card.setIds);
  visibleSets.value.forEach((set) => ids.add(set.setId));
  card.setIds = [...ids];
}

function clearSets(card: ManualRuleCardDraft): void {
  card.setIds = [];
}

/** 选择号位时默认展开该号位全部合法主属性，之后可手动取消不需要的属性。 */
function toggleSlot(card: ManualRuleCardDraft, slot: number): void {
  const selected = card.slots.includes(slot);
  if (selected) {
    card.slots = card.slots.filter((item) => item !== slot);
    card.slotMainPairs = card.slotMainPairs.filter((pair) => pair.slot !== slot);
    return;
  }
  card.slots = [...card.slots, slot].sort((left, right) => left - right);
  card.slotMainPairs = [
    ...card.slotMainPairs,
    ...mainAttributesForSlot(slot).map((mainAttribute) => ({ slot, mainAttribute })),
  ];
}

function toggleMainAttribute(
  card: ManualRuleCardDraft,
  slot: number,
  mainAttribute: string,
): void {
  const selected = hasMainAttribute(card, slot, mainAttribute);
  if (selected) {
    card.slotMainPairs = card.slotMainPairs.filter(
      (pair) => !(pair.slot === slot && pair.mainAttribute === mainAttribute),
    );
    if (!card.slotMainPairs.some((pair) => pair.slot === slot)) {
      card.slots = card.slots.filter((item) => item !== slot);
    }
    return;
  }
  card.slotMainPairs = [...card.slotMainPairs, { slot, mainAttribute }];
  if (!card.slots.includes(slot)) {
    card.slots = [...card.slots, slot].sort((left, right) => left - right);
  }
}

/** 核心属性和一般属性互斥，点击一侧时自动从另一侧移除。 */
function toggleSubstat(
  card: ManualRuleCardDraft,
  group: "core" | "general",
  attribute: string,
): void {
  const target = group === "core" ? card.coreSubstats : card.generalSubstats;
  const other = group === "core" ? card.generalSubstats : card.coreSubstats;
  if (target.includes(attribute)) {
    if (group === "core") {
      card.coreSubstats = target.filter((item) => item !== attribute);
    } else {
      card.generalSubstats = target.filter((item) => item !== attribute);
    }
    return;
  }
  if (group === "core") {
    card.coreSubstats = [...target, attribute];
    card.generalSubstats = other.filter((item) => item !== attribute);
  } else {
    card.generalSubstats = [...target, attribute];
    card.coreSubstats = other.filter((item) => item !== attribute);
  }
}

function toggleInitialSubstatCount(card: ManualRuleCardDraft, count: number): void {
  card.initialSubstatCounts = card.initialSubstatCounts.includes(count)
    ? card.initialSubstatCounts.filter((item) => item !== count)
    : [...card.initialSubstatCounts, count].sort((left, right) => left - right);
}

function addRuleCard(): void {
  cards.value = [...cards.value, createManualRuleCard(cards.value.length + 1)];
}

function removeRuleCard(cardId: string): void {
  if (cards.value.length <= 1) return;
  cards.value = cards.value.filter((card) => card.id !== cardId);
  const nextCollapsed = new Set(collapsedCardIds.value);
  nextCollapsed.delete(cardId);
  collapsedCardIds.value = nextCollapsed;
  saveCollapsePreferences();
  if (activeSetPickerId.value === cardId) activeSetPickerId.value = null;
}

/** 切换单张规则卡片的展开状态；状态变更使用新 Set 保证 Vue 能追踪更新。 */
function toggleCardCollapsed(cardId: string): void {
  const nextCollapsed = new Set(collapsedCardIds.value);
  if (nextCollapsed.has(cardId)) {
    nextCollapsed.delete(cardId);
  } else {
    nextCollapsed.add(cardId);
  }
  collapsedCardIds.value = nextCollapsed;
  saveCollapsePreferences();
}

/** 批量折叠全部规则，适合标准包含较多规则时快速浏览。 */
function collapseAllCards(): void {
  collapsedCardIds.value = new Set(cards.value.map((card) => card.id));
  saveCollapsePreferences();
}

/** 批量展开全部规则，恢复完整编辑视图。 */
function expandAllCards(): void {
  collapsedCardIds.value = new Set();
  saveCollapsePreferences();
}

function isCardCollapsed(cardId: string): boolean {
  return collapsedCardIds.value.has(cardId);
}

function setScore(
  card: ManualRuleCardDraft,
  field: keyof ManualRuleCardDraft["scoring"],
  value: string,
): void {
  const numberValue = Number(value);
  if (Number.isFinite(numberValue)) card.scoring[field] = numberValue;
}

/** 统一读取数字输入框，模板不直接书写 TypeScript 类型断言。 */
function inputValue(event: Event): string {
  return (event.target as HTMLInputElement).value;
}

function setAdvancedJson(parsed: Record<string, unknown>): void {
  // 高级编辑也不能继续携带导出时的旧哈希，否则修改任意字段都会被后端拒绝。
  advancedJson.value = JSON.stringify(stripCanonicalHash(parsed), null, 2);
}

/** 从已导出的规则正文填充全部手动规则卡片，不再只读取首条规则。 */
function loadEditorDocument(parsed: Record<string, unknown>): void {
  const editableDocument = stripCanonicalHash(parsed);
  editorDocument.value = editableDocument;
  editorTitle.value =
    typeof editableDocument.title === "string" ? editableDocument.title : "";
  editorAuthor.value =
    typeof editableDocument.author === "string" ? editableDocument.author : "本机用户";
  editorDescription.value =
    typeof editableDocument.rawDescription === "string"
      ? editableDocument.rawDescription
      : "";
  scoreColorThresholds.value = parseScoreColorThresholds(editableDocument);
  cards.value = parseManualRuleCards(editableDocument);
  restoreCollapsePreferences(editableDocument);
  setAdvancedJson(editableDocument);
}

async function loadCatalog(): Promise<void> {
  let catalog = await desktopApi.getCatalogStatus();
  if (
    !catalog ||
    catalog.validationStatus !== "verified" ||
    catalog.setCount < 70 ||
    catalog.iconCoverage < 70
  ) {
    catalog = await desktopApi.installBuiltinCatalog();
  }
  sets.value = await desktopApi.listSoulSets(catalog.version);
}

async function loadVersions(): Promise<void> {
  loading.value = true;
  error.value = "";
  try {
    // 规则库只加载已持久化的标准；用户标准启用后，系统内置项仅作内部兜底，不再展示。
    versions.value = await desktopApi.listScoreStandards();
    // 默认打开当前全局启用的标准，避免按最近保存顺序误选其他标准造成“计算标准不一致”的错觉。
    selectedVersionId.value =
      versions.value.find((version) => version.enabled)?.id ?? versions.value[0]?.id ?? null;
    if (selectedVersionId.value) {
      await loadVersionForEditor(selectedVersionId.value);
    } else {
      startNewStandard();
    }
  } catch (caught) {
    setError(caught);
  } finally {
    loading.value = false;
  }
}

/** 加载评分标准正文；启用状态由数据库中的全局单选指针提供。 */
async function loadVersionForEditor(versionId: string): Promise<void> {
  try {
    const exported = await desktopApi.exportRuleVersion(versionId);
    const parsed = JSON.parse(exported.fileContent) as Record<string, unknown>;
    exportResult.value = exported;
    editorPayload.value = Array.from(new TextEncoder().encode(exported.fileContent));
    editorParentId.value = versionId;
    selectedPreview.value = null;
    shareCode.value = exported.shareCode;
    loadEditorDocument(parsed);
  } catch (caught) {
    setError(caught);
  }
}

function selectVersion(versionId: string): void {
  selectedVersionId.value = versionId;
  void loadVersionForEditor(versionId);
}

/** 创建本地空白草稿；数据库 ID 由 Rust 保存时生成。 */
function startNewStandard(): void {
  const blank = buildManualPreset(
    {},
    {
      id: `manual.score.${Date.now()}`,
      version: "0.1.0",
      title: "新建评分标准",
      author: "本机用户",
      description: "",
    },
    [createManualRuleCard(1)],
  );
  selectedVersionId.value = null;
  editorParentId.value = undefined;
  selectedPreview.value = null;
  exportResult.value = null;
  shareCode.value = "";
  loadEditorDocument(blank);
}

/** 读取本地规则文件并立即写入评分标准，避免用户只看到预览却没有真正保存。 */
async function readFile(file: File): Promise<void> {
  sourceKind.value = "file";
  const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
  editorPayload.value = bytes;
  busy.value = true;
  error.value = "";
  try {
    selectedPreview.value = await desktopApi.previewRulePreset("file", bytes);
    await confirmImport();
  } catch (caught) {
    setError(caught);
  } finally {
    busy.value = false;
  }
}

function handleFileChange(event: Event): void {
  const file = (event.target as HTMLInputElement).files?.[0];
  if (file) void readFile(file);
}

/** 对分享码先预览再导入，清理复制时可能出现的换行但不改动规则载荷。 */
async function previewShareCode(): Promise<void> {
  sourceKind.value = "shareCode";
  const bytes = Array.from(new TextEncoder().encode(shareCode.value.trim()));
  editorPayload.value = bytes;
  busy.value = true;
  error.value = "";
  try {
    selectedPreview.value = await desktopApi.previewRulePreset("shareCode", bytes);
    notice.value = "分享码校验通过：确认后才会加入规则库。";
  } catch (caught) {
    setError(caught);
  } finally {
    busy.value = false;
  }
}

async function confirmImport(): Promise<void> {
  if (!editorPayload.value.length || !selectedPreview.value) return;
  busy.value = true;
  try {
    const imported = await desktopApi.importRulePreset(
      sourceKind.value,
      editorPayload.value,
    );
    versions.value = [
      imported,
      ...versions.value.filter((version) => version.id !== imported.id),
    ];
    selectedVersionId.value = imported.id;
    notice.value = "规则已导入规则库，可直接编辑；当前尚未启用。";
    await loadVersionForEditor(imported.id);
  } catch (caught) {
    setError(caught);
  } finally {
    busy.value = false;
  }
}

/** 导入或内置标准通过复制生成个人标准，原始来源和校验哈希保持不变。 */
async function copySelected(): Promise<void> {
  if (!selectedVersion.value) return;
  busy.value = true;
  try {
    const copied = await desktopApi.copyRuleVersion(selectedVersion.value.id);
    versions.value = [copied, ...versions.value];
    selectedVersionId.value = copied.id;
    notice.value = "已创建可编辑副本；现在可以手动选择套装和属性。";
    await loadVersionForEditor(copied.id);
  } catch (caught) {
    setError(caught);
  } finally {
    busy.value = false;
  }
}

/** 将当前简单模式卡片转换成完整规则文件，保存前由 Rust 再次执行结构校验。 */
async function saveEditor(): Promise<void> {
  if (!canEdit.value) return;
  busy.value = true;
  error.value = "";
  try {
    let payload: number[];
    if (editorMode.value === "advanced") {
      const parsed = JSON.parse(advancedJson.value) as Record<string, unknown>;
      payload = Array.from(
        new TextEncoder().encode(JSON.stringify(stripCanonicalHash(parsed))),
      );
    } else {
      const validationError = validateManualCards(cards.value);
      if (validationError) {
        error.value = validationError;
        return;
      }
      const colorThresholdError = validateScoreColorThresholds(
        scoreColorThresholds.value,
      );
      if (colorThresholdError) {
        error.value = colorThresholdError;
        return;
      }
      const base = editorDocument.value;
      const meta: ManualPresetMeta = {
        id:
          typeof base.id === "string" && base.id.trim()
            ? base.id
            : `manual.score.${Date.now()}`,
        version: `${typeof base.version === "string" ? base.version : "0.1.0"}-manual`,
        title: editorTitle.value,
        author: editorAuthor.value,
        description: editorDescription.value,
      };
      const parsed = buildManualPreset(
        base,
        meta,
        cards.value,
        scoreColorThresholds.value,
      );
      payload = Array.from(new TextEncoder().encode(JSON.stringify(parsed)));
    }
    const saved = await desktopApi.saveRuleVersion(
      "file",
      payload,
      editorParentId.value,
    );
    versions.value = [saved, ...versions.value.filter((item) => item.id !== saved.id)];
    selectedVersionId.value = saved.id;
    notice.value = "评分标准已保存；是否使用由当前启用标志决定。";
    await loadVersionForEditor(saved.id);
  } catch (caught) {
    if (caught instanceof SyntaxError) {
      error.value = "高级模式中的规则正文不是合法 JSON";
    } else {
      setError(caught);
    }
  } finally {
    busy.value = false;
  }
}

/** 只预览当前草稿，不写入规则库，也不会切换当前评分标准。 */
async function previewCurrentDraft(): Promise<void> {
  if (!canEdit.value) return;
  const validationError = validateManualCards(cards.value);
  if (validationError) {
    error.value = validationError;
    return;
  }
  const colorThresholdError = validateScoreColorThresholds(scoreColorThresholds.value);
  if (colorThresholdError) {
    error.value = colorThresholdError;
    return;
  }
  busy.value = true;
  error.value = "";
  try {
    const base = editorDocument.value;
    const payloadObject = buildManualPreset(
      base,
      {
        id: typeof base.id === "string" ? base.id : `manual.score.${Date.now()}`,
        version: typeof base.version === "string" ? base.version : "0.1.0",
        title: editorTitle.value,
        author: editorAuthor.value,
        description: editorDescription.value,
      },
      cards.value,
      scoreColorThresholds.value,
    );
    const bytes = Array.from(new TextEncoder().encode(JSON.stringify(payloadObject)));
    editorPayload.value = bytes;
    selectedPreview.value = await desktopApi.previewRulePreset("file", bytes);
    notice.value = "当前手动配置已生成预览，尚未保存。";
  } catch (caught) {
    setError(caught);
  } finally {
    busy.value = false;
  }
}

async function toggleEnabled(version: RuleVersion): Promise<void> {
  busy.value = true;
  try {
    await desktopApi.setActiveScoreStandard(version.id);
    versions.value = versions.value.map((item) => ({
      ...item,
      enabled: item.id === version.id,
    }));
    notice.value = "评分标准已启用。";
  } catch (caught) {
    setError(caught);
  } finally {
    busy.value = false;
  }
}

/** 删除用户评分标准；若删除的是当前标准，后端会优先切换到其他用户标准。 */
async function deleteSelected(): Promise<void> {
  const version = selectedVersion.value;
  if (!version || version.origin === "builtin") return;
  if (
    typeof window !== "undefined" &&
    !window.confirm(`确认删除“${version.title}”？删除后无法恢复。`)
  ) {
    return;
  }

  busy.value = true;
  error.value = "";
  try {
    await desktopApi.deleteScoreStandard(version.id);
    // 删除后重新读取全局启用标志，确保列表和编辑区显示同一份 SQL 状态。
    versions.value = await desktopApi.listScoreStandards();
    selectedVersionId.value =
      versions.value.find((item) => item.enabled)?.id ?? versions.value[0]?.id ?? null;
    if (selectedVersionId.value) {
      await loadVersionForEditor(selectedVersionId.value);
    } else {
      startNewStandard();
    }
    notice.value = "评分标准已删除。";
  } catch (caught) {
    setError(caught);
  } finally {
    busy.value = false;
  }
}

async function exportSelected(): Promise<void> {
  if (!selectedVersion.value) return;
  try {
    exportResult.value = await desktopApi.exportRuleVersion(selectedVersion.value.id);
    shareCode.value = exportResult.value.shareCode;
    downloadText(
      `${selectedVersion.value.presetId}-${selectedVersion.value.version}.yysrule.json`,
      exportResult.value.fileContent,
    );
    notice.value = "已生成规则文件和离线分享码；正文不包含具体御魂 ID。";
  } catch (caught) {
    setError(caught);
  }
}

/** 使用浏览器下载已由 Rust 校验过的正文，避免 WebView 直接访问任意路径。 */
function downloadText(fileName: string, content: string): void {
  const url = URL.createObjectURL(
    new Blob([content], { type: "application/json;charset=utf-8" }),
  );
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  anchor.click();
  URL.revokeObjectURL(url);
}

async function loadPage(): Promise<void> {
  loading.value = true;
  try {
    await loadCatalog();
    await loadVersions();
  } catch (caught) {
    setError(caught);
  } finally {
    loading.value = false;
  }
}

onMounted(() => void loadPage());
</script>

<template>
  <main id="rule-library" class="workspace rule-library">
    <header class="page-header">
      <div>
        <p class="eyebrow">SOUL SCORE / 07</p>
        <h1>御魂评分标准</h1>
        <p class="lede">
          手动选择套装、号位和属性；一张标准可以包含多条规则，分享时只携带配置条件。
        </p>
      </div>
      <div class="page-header__actions">
        <div class="health-badge">
          <span class="health-badge__pulse" aria-hidden="true"></span>
          <span>{{ enabledCount }} 个标准正在使用</span>
        </div>

        <div class="rule-toolbar" aria-label="评分标准工具栏">
          <button
            class="button button--primary"
            type="button"
            :disabled="busy"
            title="新建一份空白评分标准"
            @click="startNewStandard"
          >
            新建
          </button>
          <input
            ref="fileInput"
            type="file"
            accept=".yysrule.json,.json"
            hidden
            @change="handleFileChange"
          />
          <button
            class="button button--quiet"
            type="button"
            :disabled="busy"
            title="导入规则文件"
            @click="fileInput?.click()"
          >
            导入
          </button>
          <button
            class="button button--quiet"
            type="button"
            :disabled="busy"
            title="校验分享码"
            @click="previewShareCode"
          >
            校验
          </button>
          <span class="toolbar-note"
            >{{ sets.length }} 套离线目录 · {{ cardCountLabel }}</span
          >
        </div>
      </div>
    </header>

    <section v-if="error" class="error-panel" role="alert">
      <div>
        <p class="error-panel__title">评分标准操作未完成</p>
        <p>{{ error }}</p>
      </div>
      <button class="button button--quiet" type="button" @click="loadVersions">
        重试
      </button>
    </section>
    <p v-if="notice" class="success-panel" role="status">{{ notice }}</p>

    <section class="rule-layout">
      <div class="rule-list-card">
        <div class="card-heading">
          <div>
            <p class="section-kicker">RULE LIBRARY</p>
            <h2>评分标准</h2>
          </div>
          <span class="status-chip">{{ versions.length }} 份标准</span>
        </div>
        <div v-if="loading" class="placeholder-card">正在读取评分标准…</div>
        <button
          v-for="version in versions"
          :key="version.id"
          class="rule-version-row"
          :class="{ 'rule-version-row--active': version.id === selectedVersionId }"
          type="button"
          @click="selectVersion(version.id)"
        >
          <span class="rule-version-icon" aria-hidden="true">{{
            version.origin === "builtin"
              ? "基"
              : version.origin === "imported"
                ? "导"
                : "我"
          }}</span>
          <span class="rule-version-copy">
            <strong>{{ version.title }}</strong>
            <small>{{ originLabel(version.origin) }}</small>
            <small class="mono-text"
              >{{ version.normalizedHash.slice(0, 12) }}… ·
              {{ version.ruleCount }} 条规则</small
            >
          </span>
          <span class="rule-version-state">
            <span
              class="status-chip"
              :class="{ 'status-chip--safe': version.enabled }"
              >{{ version.enabled ? "已启用" : statusLabel(version.status) }}</span
            >
            <span v-if="version.readOnly" class="lock-label">只读</span>
          </span>
        </button>
        <div v-if="!versions.length && !loading" class="empty-state">
          <strong>还没有评分标准</strong>
          <span>点击右上角“新建”开始配置。</span>
        </div>
      </div>

      <div class="rule-editor-card">
        <div class="card-heading">
          <div>
            <p class="section-kicker">MANUAL BUILDER</p>
            <h2>{{ selectedVersion?.title ?? (editorTitle || "新建评分标准") }}</h2>
          </div>
          <div class="editor-header-actions">
            <div class="editor-tabs">
              <button
                type="button"
                :class="{ active: editorMode === 'simple' }"
                @click="editorMode = 'simple'"
              >
                简单
              </button>
              <button
                type="button"
                :class="{ active: editorMode === 'advanced' }"
                @click="editorMode = 'advanced'"
              >
                高级
              </button>
            </div>
            <div class="editor-actions editor-actions--top">
              <button
                class="button button--primary"
                type="button"
                :disabled="busy || !canEdit"
                title="保存当前评分标准"
                @click="saveEditor"
              >
                保存
              </button>
              <button
                class="button button--quiet"
                type="button"
                :disabled="busy || !selectedVersion"
                title="复制当前标准并创建可编辑副本"
                @click="copySelected"
              >
                复制
              </button>
              <button
                v-if="editorMode === 'simple'"
                class="button button--quiet"
                type="button"
                :disabled="busy || !canEdit"
                title="只检查当前未保存配置，不会保存或启用"
                @click="previewCurrentDraft"
              >
                预览
              </button>
              <button
                class="button button--quiet"
                type="button"
                :disabled="busy || !selectedVersion"
                title="导出规则文件和分享码"
                @click="exportSelected"
              >
                导出
              </button>
              <button
                v-if="selectedVersion"
                class="button button--primary editor-action--activate"
                type="button"
                :disabled="busy || selectedVersion.enabled"
                @click="toggleEnabled(selectedVersion)"
              >
                {{ selectedVersion.enabled ? "已启用" : "启用" }}
              </button>
              <button
                v-if="selectedVersion"
                class="button button--danger"
                type="button"
                :disabled="busy || selectedVersion.origin === 'builtin'"
                :title="
                  selectedVersion.origin === 'builtin'
                    ? '系统默认评分标准不能删除'
                    : '删除这份评分标准'
                "
                @click="deleteSelected"
              >
                删除
              </button>
            </div>
          </div>
        </div>

        <div v-if="selectedPreview" class="preview-banner">
          <span class="preview-banner__mark">✓</span>
          <div>
            <strong>规则预览 · {{ selectedPreview.title }}</strong>
            <span
              >{{ selectedPreview.author }} · {{ selectedPreview.ruleCount }} 条规则 ·
              {{ selectedPreview.status }}</span
            >
          </div>
          <button
            v-if="sourceKind !== 'file'"
            class="button button--primary"
            type="button"
            :disabled="busy"
            @click="confirmImport"
          >
            导入
          </button>
        </div>

        <div v-if="editorMode === 'simple'" class="editor-form">
          <div class="editor-meta-grid">
            <label class="field-label"
              >标准名称<input
                v-model="editorTitle"
                class="text-input"
                :disabled="!canEdit || busy"
                placeholder="例如：我的默认标准"
            /></label>
            <label class="field-label"
              >作者<input
                v-model="editorAuthor"
                class="text-input"
                :disabled="!canEdit || busy"
                placeholder="本机用户"
            /></label>
          </div>
          <label class="field-label"
            >说明<textarea
              v-model="editorDescription"
              class="text-input text-area"
              :disabled="!canEdit || busy"
              placeholder="记录这套评分标准适用的场景和取舍"
            ></textarea>
          </label>

          <section class="score-color-settings" aria-label="评分光效分级配置">
            <div class="score-color-settings__heading">
              <div>
                <strong>评分光效分级</strong>
                <span>控制“御魂分析”的评分光效层级，不改变评分结果。</span>
              </div>
              <span class="score-color-settings__preview" aria-hidden="true">
                <i
                  class="score-color-settings__swatch score-color-settings__swatch--jade"
                ></i>
                <i
                  class="score-color-settings__swatch score-color-settings__swatch--gold"
                ></i>
                <i
                  class="score-color-settings__swatch score-color-settings__swatch--rainbow"
                ></i>
              </span>
            </div>
            <div class="score-color-settings__grid">
              <label class="field-label">
                青玉色 ≥
                <input
                  v-model.number="scoreColorThresholds.jadeScore"
                  class="text-input"
                  type="number"
                  min="0"
                  max="100"
                  step="1"
                  :disabled="!canEdit || busy"
                />
              </label>
              <label class="field-label">
                金橙色 ≥
                <input
                  v-model.number="scoreColorThresholds.goldScore"
                  class="text-input"
                  type="number"
                  min="0"
                  max="100"
                  step="1"
                  :disabled="!canEdit || busy"
                />
              </label>
              <label class="field-label">
                彩虹色 ≥
                <input
                  v-model.number="scoreColorThresholds.rainbowScore"
                  class="text-input"
                  type="number"
                  min="0"
                  max="100"
                  step="1"
                  :disabled="!canEdit || busy"
                />
              </label>
            </div>
          </section>

          <div class="manual-builder-heading">
            <div>
              <div class="editor-section-label">手动规则卡片</div>
              <p>每张卡片是一条独立规则；不会自动分组，也不会绑定具体御魂。</p>
            </div>
            <div class="manual-builder-heading__actions">
              <span class="status-chip">{{ cardCountLabel }}</span>
              <button
                class="button button--quiet button--small"
                type="button"
                @click="collapseAllCards"
              >
                全部折叠
              </button>
              <button
                class="button button--quiet button--small"
                type="button"
                @click="expandAllCards"
              >
                全部展开
              </button>
            </div>
          </div>

          <article
            v-for="(card, index) in cards"
            :key="card.id"
            class="manual-rule-card"
            :class="{ 'manual-rule-card--collapsed': isCardCollapsed(card.id) }"
          >
            <header class="manual-rule-card__header">
              <div class="manual-rule-card__title-block">
                <span class="manual-rule-card__index"
                  >RULE {{ String(index + 1).padStart(2, "0") }}</span
                >
                <input
                  v-model="card.name"
                  class="manual-rule-card__title"
                  :disabled="!canEdit || busy"
                  aria-label="规则名称"
                />
              </div>
              <div class="manual-rule-card__header-actions">
                <button
                  class="button button--quiet button--small manual-rule-card__collapse"
                  type="button"
                  :aria-expanded="!isCardCollapsed(card.id)"
                  @click="toggleCardCollapsed(card.id)"
                >
                  <span aria-hidden="true">{{
                    isCardCollapsed(card.id) ? "＋" : "－"
                  }}</span>
                  {{ isCardCollapsed(card.id) ? "展开" : "折叠" }}
                </button>
                <button
                  v-if="cards.length > 1"
                  class="button button--quiet button--small"
                  type="button"
                  :disabled="!canEdit || busy"
                  @click="removeRuleCard(card.id)"
                >
                  删除
                </button>
              </div>
            </header>

            <div v-if="isCardCollapsed(card.id)" class="manual-rule-card__summary">
              <span>{{ setPickerLabel(card) }}</span>
              <span>{{
                card.slots.length ? card.slots.join("、") + "号位" : "未选号位"
              }}</span>
              <span>{{ card.coreSubstats.length }} 个核心属性</span>
              <span>{{ card.generalSubstats.length }} 个一般属性</span>
              <span>{{ scenarioLabel(card.scenario) }}</span>
            </div>

            <div v-if="!isCardCollapsed(card.id)" class="manual-rule-card__body">
              <section class="manual-rule-section">
                <div class="manual-rule-label-row">
                  <div>
                    <strong>适用御魂套装</strong><span>{{ setPickerLabel(card) }}</span>
                  </div>
                  <button
                    class="text-button"
                    type="button"
                    :disabled="!canEdit || busy"
                    @click="
                      activeSetPickerId = activeSetPickerId === card.id ? null : card.id
                    "
                  >
                    选择
                  </button>
                </div>
                <div class="selection-chips">
                  <span v-if="!card.setIds.length" class="selection-empty"
                    >未限定套装 · 适用于全部套装</span
                  >
                  <button
                    v-for="setId in card.setIds"
                    :key="setId"
                    class="selection-chip selection-chip--set"
                    type="button"
                    :disabled="!canEdit || busy"
                    @click="toggleSet(card, setId)"
                  >
                    <span class="selection-chip__set-content">
                      <span class="set-option__icon set-option__icon--chip">
                        <img
                          v-if="selectedSet(setId) && soulIcon(selectedSet(setId)!)"
                          :src="soulIcon(selectedSet(setId)!)!"
                          :alt="`${setName(setId)}图标`"
                        />
                        <span v-else>{{ setName(setId).slice(0, 1) }}</span>
                      </span>
                      <span>{{ setName(setId) }}</span> </span
                    ><span aria-hidden="true">×</span>
                  </button>
                </div>
                <div v-if="activeSetPickerId === card.id" class="set-picker">
                  <div class="set-picker__toolbar">
                    <select
                      v-model="setCategory"
                      class="text-input"
                      aria-label="套装效果"
                    >
                      <option
                        v-for="category in setCategories"
                        :key="category"
                        :value="category"
                      >
                        {{ category }}
                      </option>
                    </select>
                    <input
                      v-model="setSearch"
                      class="text-input"
                      placeholder="搜索具体御魂"
                      aria-label="搜索具体御魂"
                    />
                    <button
                      class="text-button"
                      type="button"
                      @click="selectVisibleSets(card)"
                    >
                      全选
                    </button>
                    <button class="text-button" type="button" @click="clearSets(card)">
                      清空
                    </button>
                  </div>
                  <div class="set-picker__grid">
                    <button
                      v-for="set in visibleSets"
                      :key="set.setId"
                      class="set-option"
                      :class="{
                        'set-option--selected': card.setIds.includes(set.setId),
                      }"
                      type="button"
                      :aria-pressed="card.setIds.includes(set.setId)"
                      @click="toggleSet(card, set.setId)"
                    >
                      <span class="set-option__icon">
                        <img
                          v-if="soulIcon(set)"
                          :src="soulIcon(set)!"
                          :alt="`${set.name}图标`"
                        />
                        <span v-else>{{ set.name.slice(0, 1) }}</span>
                      </span>
                      <span>{{ set.name }}</span>
                      <span
                        v-if="card.setIds.includes(set.setId)"
                        class="set-option__check"
                        >✓</span
                      >
                    </button>
                  </div>
                  <p v-if="!visibleSets.length" class="inline-hint">
                    没有匹配的套装，请清空搜索或分类条件。
                  </p>
                </div>
              </section>

              <section class="manual-rule-section">
                <div class="manual-rule-label-row">
                  <div>
                    <strong>适用号位</strong><span>点击号位后手动配置对应主属性</span>
                  </div>
                </div>
                <div class="toggle-row">
                  <button
                    v-for="slot in [1, 2, 3, 4, 5, 6]"
                    :key="slot"
                    class="toggle-pill"
                    :class="{ 'toggle-pill--active': card.slots.includes(slot) }"
                    type="button"
                    :disabled="!canEdit || busy"
                    @click="toggleSlot(card, slot)"
                  >
                    {{ slot }}号
                  </button>
                </div>
                <div class="main-attribute-matrix">
                  <div
                    v-for="slot in card.slots"
                    :key="slot"
                    class="main-attribute-row"
                  >
                    <strong>{{ slot }}号位主属性</strong>
                    <div class="selection-chips">
                      <button
                        v-for="attribute in mainAttributesForSlot(slot)"
                        :key="attribute"
                        class="selection-chip"
                        :class="{
                          'selection-chip--active': hasMainAttribute(
                            card,
                            slot,
                            attribute,
                          ),
                        }"
                        type="button"
                        :disabled="!canEdit || busy"
                        @click="toggleMainAttribute(card, slot, attribute)"
                      >
                        {{ attributeLabel(attribute) }}
                      </button>
                    </div>
                  </div>
                </div>
              </section>

              <section class="manual-rule-section">
                <div class="manual-rule-label-row">
                  <div>
                    <strong>初始副属性条数</strong
                    ><span>用于胚子筛选；不选表示不限制</span>
                  </div>
                </div>
                <div class="toggle-row">
                  <button
                    v-for="count in [2, 3, 4]"
                    :key="count"
                    class="toggle-pill"
                    :class="{
                      'toggle-pill--active': card.initialSubstatCounts.includes(count),
                    }"
                    type="button"
                    :disabled="!canEdit || busy"
                    @click="toggleInitialSubstatCount(card, count)"
                  >
                    {{ count }} 条
                  </button>
                </div>
              </section>

              <section class="manual-rule-section">
                <div class="manual-rule-label-row">
                  <div>
                    <strong>有效副属性</strong
                    ><span>核心属性分值更高，一般属性用于过渡保留</span>
                  </div>
                </div>
                <div class="attribute-group">
                  <span class="attribute-group__label attribute-group__label--core"
                    >核心有效属性</span
                  >
                  <div class="selection-chips">
                    <button
                      v-for="attribute in ATTRIBUTE_OPTIONS"
                      :key="`core-${attribute.id}`"
                      class="selection-chip"
                      :class="{
                        'selection-chip--active selection-chip--core':
                          card.coreSubstats.includes(attribute.id),
                      }"
                      type="button"
                      :disabled="!canEdit || busy"
                      @click="toggleSubstat(card, 'core', attribute.id)"
                    >
                      {{ attribute.label }}
                    </button>
                  </div>
                </div>
                <div class="attribute-group">
                  <span class="attribute-group__label attribute-group__label--general"
                    >一般有效属性</span
                  >
                  <div class="selection-chips">
                    <button
                      v-for="attribute in ATTRIBUTE_OPTIONS"
                      :key="`general-${attribute.id}`"
                      class="selection-chip"
                      :class="{
                        'selection-chip--active selection-chip--general':
                          card.generalSubstats.includes(attribute.id),
                      }"
                      type="button"
                      :disabled="!canEdit || busy"
                      @click="toggleSubstat(card, 'general', attribute.id)"
                    >
                      {{ attribute.label }}
                    </button>
                  </div>
                </div>
              </section>

              <section class="manual-rule-section">
                <div class="manual-rule-label-row">
                  <div>
                    <strong>使用场景</strong
                    ><span>场景只作为规则条件，不改变套装属性</span>
                  </div>
                </div>
                <div class="toggle-row">
                  <button
                    v-for="scenario in scenarioOptions"
                    :key="scenario"
                    class="toggle-pill"
                    :class="{ 'toggle-pill--active': card.scenario === scenario }"
                    type="button"
                    :disabled="!canEdit || busy"
                    @click="card.scenario = scenario"
                  >
                    {{ scenarioLabel(scenario) }}
                  </button>
                </div>
              </section>

              <section class="manual-rule-section manual-rule-section--score">
                <div class="manual-rule-label-row">
                  <div>
                    <strong>评分权重与阈值</strong
                    ><span>评分范围 0–100；阈值需满足：弃置 ≤ 观察 ≤ 保留</span>
                  </div>
                </div>
                <div class="score-grid">
                  <label class="field-label"
                    >核心属性分/条<input
                      class="text-input"
                      type="number"
                      min="0"
                      step="1"
                      :disabled="!canEdit || busy"
                      :value="card.scoring.coreAttributeWeight"
                      @input="
                        setScore(card, 'coreAttributeWeight', inputValue($event))
                      "
                  /></label>
                  <label class="field-label"
                    >一般属性分/条<input
                      class="text-input"
                      type="number"
                      min="0"
                      step="1"
                      :disabled="!canEdit || busy"
                      :value="card.scoring.generalAttributeWeight"
                      @input="
                        setScore(card, 'generalAttributeWeight', inputValue($event))
                      "
                  /></label>
                  <label class="field-label"
                    >强化命中核心<input
                      class="text-input"
                      type="number"
                      min="0"
                      step="1"
                      :disabled="!canEdit || busy"
                      :value="card.scoring.enhancementCoreWeight"
                      @input="
                        setScore(card, 'enhancementCoreWeight', inputValue($event))
                      "
                  /></label>
                  <label class="field-label"
                    >强化命中一般<input
                      class="text-input"
                      type="number"
                      min="0"
                      step="1"
                      :disabled="!canEdit || busy"
                      :value="card.scoring.enhancementGeneralWeight"
                      @input="
                        setScore(card, 'enhancementGeneralWeight', inputValue($event))
                      "
                  /></label>
                  <label class="field-label"
                    >保留 ≥<input
                      class="text-input"
                      type="number"
                      min="0"
                      max="100"
                      step="1"
                      :disabled="!canEdit || busy"
                      :value="card.scoring.keepScore"
                      @input="setScore(card, 'keepScore', inputValue($event))"
                  /></label>
                  <label class="field-label"
                    >观察 ≥<input
                      class="text-input"
                      type="number"
                      min="0"
                      max="100"
                      step="1"
                      :disabled="!canEdit || busy"
                      :value="card.scoring.observeScore"
                      @input="setScore(card, 'observeScore', inputValue($event))"
                  /></label>
                  <label class="field-label"
                    >弃置
                    <input
                      class="text-input"
                      type="number"
                      min="0"
                      max="100"
                      step="1"
                      :disabled="!canEdit || busy"
                      :value="card.scoring.discardScore"
                      @input="setScore(card, 'discardScore', inputValue($event))"
                  /></label>
                </div>
              </section>

              <footer class="manual-rule-card__footer">
                <span
                  >已选套装：{{
                    card.setIds.length
                      ? card.setIds.map(setName).join("、")
                      : "全部套装"
                  }}</span
                >
                <span>已选号位：{{ card.slots.join("、") || "未选择" }}</span>
                <span>场景：{{ scenarioLabel(card.scenario) }}</span>
              </footer>
            </div>
          </article>

          <button
            class="add-rule-card"
            type="button"
            :disabled="!canEdit || busy"
            @click="addRuleCard"
          >
            ＋ 添加规则
          </button>

          <div class="editor-facts">
            <span
              >状态：{{
                selectedVersion ? statusLabel(selectedVersion.status) : "本地草稿"
              }}</span
            >
            <span>正文：Rust 校验</span>
          </div>
        </div>

        <div v-else class="advanced-panel">
          <div class="advanced-tree">
            <div class="tree-node tree-node--root">
              <strong>声明式规则正文</strong
              ><span>高级模式可查看和编辑分享文件，保存时仍由 Rust 校验</span>
            </div>
            <label class="field-label advanced-expression-label"
              >规则 JSON<textarea
                v-model="advancedJson"
                class="text-input text-area expression-editor"
                :disabled="!canEdit || busy"
              ></textarea>
            </label>
          </div>
          <p class="advanced-note">
            高级模式面向需要精细调整规则文件的用户；普通配置请返回简单模式使用套装图标和属性按钮。
          </p>
        </div>
      </div>
    </section>

    <section class="share-panel">
      <div>
        <p class="section-kicker">OFFLINE SHARE</p>
        <h2>离线分享码</h2>
        <p>分享码只包含手动规则、评分权重和阈值，不包含库存或具体御魂 ID。</p>
      </div>
      <textarea
        v-model="shareCode"
        class="share-code-input"
        placeholder="YYSR1.…"
        aria-label="规则离线分享码"
      ></textarea>
      <button
        class="button button--quiet"
        type="button"
        :disabled="!shareCode || busy"
        @click="previewShareCode"
      >
        预览
      </button>
      <div v-if="exportResult" class="export-hint">
        文件正文已生成，可复制后保存为 `.yysrule.json`。
      </div>
    </section>
  </main>
</template>
