<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import type { CatalogStatus, SoulSet } from "../api/contracts";
import { normalizeCommandError } from "../api/errors";

// 图鉴随应用离线打包，键名由 Vite 保留相对路径，数据库只保存稳定的数字图标编号。
const iconModules = import.meta.glob("../assets/soul-icons/*.png", {
  eager: true,
  import: "default",
  query: "?url",
});

// 当前完整目录应覆盖导入器能够识别的 70 个套装；旧目录会自动升级。
const EXPECTED_CATALOG_SET_COUNT = 70;
const catalogStatus = ref<CatalogStatus | null>(null);
const sets = ref<SoulSet[]>([]);
const selectedSetId = ref<string | null>(null);
const search = ref("");
const categoryFilter = ref("全部");
const loading = ref(true);
const error = ref("");
const notice = ref("");
const selectedSet = computed(
  () => sets.value.find((set) => set.setId === selectedSetId.value) ?? null,
);
const categories = computed(() => [
  "全部",
  ...new Set(
    sets.value
      .flatMap((set) => [set.specialCategory, set.category])
      .filter(Boolean) as string[],
  ),
]);
const filteredSets = computed(() => {
  const keyword = search.value.trim().toLocaleLowerCase("zh-CN");
  return sets.value.filter((set) => {
    const categoryMatched =
      categoryFilter.value === "全部" ||
      set.category === categoryFilter.value ||
      set.specialCategory === categoryFilter.value;
    const keywordMatched =
      !keyword ||
      set.name.toLocaleLowerCase("zh-CN").includes(keyword) ||
      set.category?.toLocaleLowerCase("zh-CN").includes(keyword) ||
      set.specialCategory?.toLocaleLowerCase("zh-CN").includes(keyword);
    return categoryMatched && keywordMatched;
  });
});
const isSpecialCategory = computed(() => Boolean(selectedSet.value?.specialCategory));

// 页面头部只展示目录规模；版本、哈希和校验状态仍由后台保留，不把实现元数据放到主视线。
const catalogHealthLabel = computed(() => {
  return catalogStatus.value ? `${catalogStatus.value.setCount} 套目录` : "本地目录";
});

/** 将命令错误收敛为用户可见的稳定编码与说明。 */
function setError(caught: unknown): void {
  const commandError = normalizeCommandError(caught);
  error.value = `${commandError.code} · ${commandError.message}`;
}

/** 根据套装编号返回离线图标；缺图时页面使用文字印章，不发起网络请求。 */
function soulIcon(set: SoulSet): string | null {
  if (!set.iconAssetId) return null;
  return iconModules[`../assets/soul-icons/${set.iconAssetId}.png`] ?? null;
}

/** 选择套装并把键盘焦点所在的详情与规则配置切换到同一对象。 */
function selectSet(setId: string): void {
  selectedSetId.value = setId;
}

/** 切换套装分类后选中当前筛选结果的第一项；没有匹配项时清空详情选择。 */
function selectCategory(category: string): void {
  categoryFilter.value = category;
  selectedSetId.value = filteredSets.value[0]?.setId ?? null;
}

/** 首次进入图鉴时自动安装或升级内置目录，保证导入器认识的套装都可被选择。 */
async function loadPage(): Promise<void> {
  loading.value = true;
  error.value = "";
  try {
    let catalog = await desktopApi.getCatalogStatus();
    if (
      !catalog ||
      catalog.setCount < EXPECTED_CATALOG_SET_COUNT ||
      catalog.iconCoverage < EXPECTED_CATALOG_SET_COUNT ||
      catalog.mechanicsCoverage < EXPECTED_CATALOG_SET_COUNT ||
      !["verified", "partial"].includes(catalog.validationStatus)
    ) {
      catalog = await desktopApi.installBuiltinCatalog();
      notice.value = `本地图鉴已更新到 ${catalog.setCount} 个套装。`;
    }
    catalogStatus.value = catalog;
    sets.value = await desktopApi.listSoulSets(catalog.version);
    selectedSetId.value =
      sets.value.find((set) => set.name === "破势")?.setId ??
      sets.value[0]?.setId ??
      null;
  } catch (caught) {
    setError(caught);
  } finally {
    loading.value = false;
  }
}

/*
 * 规则编辑属于后续“规则库”功能；首期只读展示御魂目录与强化机制，避免在目录页面
 * 直接写入玩家档案规则。保留这段备份逻辑作为后续恢复规则库时的迁移参考。
// 构造完整的声明式预设；具体套装通过 setIds 写入 Rust 选择器并参与实际分析。
function buildRulePayload(): number[] {
  const set = selectedSet.value;
  if (!set || !selectedPresetId.value) return [];
  const now = new Date();
  const version = `${now.getFullYear()}.${now.getMonth() + 1}.${now.getDate()}-${now.getTime()}`;
  const targetCount = Math.min(
    Math.max(form.minimumTargetCount, 1),
    form.targetAttributes.length,
  );
  const preset = {
    format: "yys-rule-preset",
    schemaVersion: 1,
    id: selectedPresetId.value,
    version,
    title: `${set.name}专属强化规则`,
    author: "本机用户",
    locale: "zh-CN",
    gameVersionRange: "*",
    status: "draft",
    sourceRefs: [],
    sourceText: ["由御魂档案的套装强化配置生成"],
    rawDescription: `只作用于${set.name}；满足条件时进入强化观察。`,
    assumptions: ["续投节点沿用当前档案练度与系统用途模板"],
    defaults: { quality: [6], level: [0] },
    rules: [
      {
        id: `${selectedPresetId.value}.admission`,
        stage: "admission",
        rawLabel: `${set.name}套装专属准入`,
        name: `${set.name}胚子准入`,
        status: "draft",
        evidenceLevel: "author",
        sourceRefs: [],
        selector: {
          quality: [6],
          level: [0],
          setIds: [set.setId],
          slots: [...form.slots].sort(),
          mainAttributes: form.mainAttributes,
          initialSubstatCounts: [...form.initialSubstatCounts].sort(),
          requiredSubstats: {
            allOf: [],
            anyOf: [],
            atLeast: {
              count: targetCount,
              from: form.targetAttributes,
            },
          },
        },
        candidateUses: [form.candidateUse.trim() || `${set.name}强化观察`],
      },
    ],
  };
  return Array.from(new TextEncoder().encode(JSON.stringify(preset)));
}

// 预览当前表单在当前档案的命中数量，不保存、不启用也不重算。
async function previewRule(): Promise<void> {
  if (!selectedProfileId.value || !form.targetAttributes.length) return;
  busy.value = true;
  error.value = "";
  try {
    impact.value = await desktopApi.previewRuleImpact(
      selectedProfileId.value,
      "file",
      buildRulePayload(),
    );
    notice.value = "预览完成，尚未保存规则。";
  } catch (caught) {
    setError(caught);
  } finally {
    busy.value = false;
  }
}

// 保存新版本、停用该套装旧版本并启用新版本，随后启动当前档案的分析重算。
async function saveAndEnableRule(): Promise<void> {
  if (!selectedProfileId.value || !selectedSet.value) return;
  if (!form.targetAttributes.length || !form.initialSubstatCounts.length) {
    error.value = "请至少选择一种目标副属性和一种初始腿数。";
    return;
  }
  busy.value = true;
  error.value = "";
  try {
    const oldVersions = [...selectedRuleVersions.value];
    const saved = await desktopApi.saveRuleVersion(
      "file",
      buildRulePayload(),
      currentRuleVersion.value?.id,
    );
    // 同一档案同一套装只启用最新版本，旧版本继续保留用于追溯与回退。
    for (const version of oldVersions) {
      if (!version.enabled) continue;
      await desktopApi.setRuleActivation({
        profileId: selectedProfileId.value,
        ruleVersionId: version.id,
        enabled: false,
      });
    }
    await desktopApi.setRuleActivation({
      profileId: selectedProfileId.value,
      ruleVersionId: saved.id,
      enabled: true,
      note: `${selectedSet.value.name}图鉴配置`,
    });
    await desktopApi.startRuleRecalculationTask({
      profileId: selectedProfileId.value,
    });
    notice.value = `${selectedSet.value.name}规则已保存并对当前档案启用，分析正在刷新。`;
    await loadVersions();
  } catch (caught) {
    setError(caught);
  } finally {
    busy.value = false;
  }
}
*/

onMounted(() => void loadPage());
</script>

<template>
  <main class="workspace soul-catalog">
    <header class="page-header soul-catalog__header">
      <div>
        <p class="eyebrow">SOUL ARCHIVE / 01</p>
        <h1>御魂档案</h1>
        <p class="lede">
          这里保存的是平安志的御魂目录与强化机制，不是玩家库存；所有图标和说明均可离线查阅。
        </p>
      </div>
      <div class="health-badge" :class="{ 'health-badge--error': error }">
        <span class="health-badge__pulse" aria-hidden="true"></span>
        <span>{{ catalogHealthLabel }}</span>
      </div>
    </header>

    <section v-if="error" class="error-panel" role="alert">
      <div>
        <p class="error-panel__title">图鉴操作未完成</p>
        <p>{{ error }}</p>
      </div>
      <button class="button button--quiet" type="button" @click="loadPage">
        重新加载
      </button>
    </section>
    <p v-if="notice" class="success-panel" role="status">{{ notice }}</p>

    <section v-if="loading" class="placeholder-card">正在安装并读取本地图鉴…</section>

    <section v-else class="soul-archive-layout">
      <!-- 目录索引置于详情上方，先完成筛选与定位，再阅读当前套装档案。 -->
      <aside class="soul-index" aria-label="御魂套装索引">
        <div class="soul-index__tools">
          <label class="catalog-search">
            <span>搜索套装</span>
            <input v-model="search" class="text-input" placeholder="名称或二件套属性" />
          </label>
          <div class="catalog-category-strip" aria-label="按套装属性筛选">
            <button
              v-for="category in categories"
              :key="category"
              type="button"
              :class="{ active: categoryFilter === category }"
              @click="selectCategory(category)"
            >
              {{ category }}
            </button>
          </div>
        </div>
        <div class="soul-index__list">
          <button
            v-for="set in filteredSets"
            :key="set.setId"
            class="soul-index-row"
            :class="{ 'soul-index-row--active': set.setId === selectedSetId }"
            type="button"
            @click="selectSet(set.setId)"
          >
            <span class="soul-icon soul-icon--small">
              <img
                v-if="soulIcon(set)"
                :src="soulIcon(set)!"
                :alt="`${set.name}图标`"
              />
              <span v-else>{{ set.name.slice(0, 1) }}</span>
            </span>
            <span class="soul-index-row__copy">
              <strong>{{ set.name }}</strong>
              <small>{{ set.specialCategory ?? set.category ?? "未分类" }}</small>
            </span>
          </button>
          <p v-if="!filteredSets.length" class="empty-note">没有匹配的御魂套装。</p>
        </div>
      </aside>

      <article v-if="selectedSet" class="soul-scroll">
        <div class="soul-scroll__identity">
          <span class="soul-icon soul-icon--hero">
            <img
              v-if="soulIcon(selectedSet)"
              :src="soulIcon(selectedSet)!"
              :alt="`${selectedSet.name}图标`"
            />
            <span v-else>{{ selectedSet.name.slice(0, 1) }}</span>
          </span>
          <div>
            <p class="section-kicker">SET · {{ selectedSet.iconAssetId ?? "—" }}</p>
            <h2>{{ selectedSet.name }}</h2>
            <span class="soul-category">
              {{ selectedSet.specialCategory ?? selectedSet.category ?? "未分类" }}
            </span>
          </div>
        </div>

        <section class="effect-block">
          <div class="effect-block__number">贰</div>
          <div>
            <p class="section-kicker">
              {{ isSpecialCategory ? "二件套特殊效果" : "二件套属性" }}
            </p>
            <strong>{{
              selectedSet.twoPieceEffect ?? "未核验：缺少二件套说明"
            }}</strong>
          </div>
        </section>
        <section v-if="!isSpecialCategory" class="effect-block effect-block--four">
          <div class="effect-block__number">肆</div>
          <div>
            <p class="section-kicker">四件套效果</p>
            <strong v-if="selectedSet.fourPieceEffect">{{
              selectedSet.fourPieceEffect
            }}</strong>
            <strong v-else class="unverified-copy">未核验：缺少四件套说明</strong>
            <small v-if="!selectedSet.fourPieceEffect"
              >标准套装资料不完整，目录包应拒绝安装。</small
            >
          </div>
        </section>
      </article>
    </section>
  </main>
</template>

<style scoped>
.soul-catalog {
  --archive-paper: #f7f2e8;
  --archive-line: rgb(139 71 36 / 20%);
}

.soul-catalog__header {
  margin-bottom: 24px;
}

/* 页面阅读顺序固定为“先筛选目录、后阅读详情”，让详情在桌面端占满可用宽度。 */
.soul-archive-layout {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: 18px;
  align-items: start;
}

.soul-index,
.soul-scroll,
.catalog-notes {
  border: 1px solid rgb(23 36 63 / 10%);
  background: rgb(248 250 249 / 94%);
  box-shadow: var(--shadow);
}

.soul-index {
  overflow: hidden;
}

.soul-index__tools {
  display: grid;
  grid-template-columns: minmax(240px, 0.72fr) minmax(0, 1.28fr);
  align-items: end;
  gap: 18px;
  padding: 18px;
  border-bottom: 1px solid var(--line-soft);
}

.catalog-search {
  display: grid;
  gap: 7px;
  color: var(--ink-soft);
  font-size: 11px;
  font-weight: 700;
}

.catalog-category-strip {
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
  align-content: center;
  max-height: 72px;
  overflow: auto;
}

.catalog-category-strip button,
.choice-row button,
.attribute-choice-grid button {
  border: 1px solid var(--line);
  border-radius: 4px;
  color: var(--ink-soft);
  background: transparent;
  cursor: pointer;
}

.catalog-category-strip button {
  padding: 5px 7px;
  font-size: 10px;
}

.catalog-category-strip button.active,
.choice-row button.active,
.attribute-choice-grid button.active {
  border-color: rgb(182 105 58 / 50%);
  color: var(--accent-deep);
  background: rgb(182 105 58 / 10%);
}

/* 70 套目录使用受限高度的多列网格，保证顶部索引紧凑且仍可完整滚动访问。 */
.soul-index__list {
  display: grid;
  grid-template-columns: repeat(7, minmax(0, 1fr));
  max-height: 236px;
  gap: 6px;
  overflow-y: auto;
  padding: 8px;
}

.soul-index-row {
  position: relative;
  display: flex;
  width: 100%;
  min-width: 0;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border: 0;
  border-radius: 6px;
  color: var(--ink);
  background: transparent;
  cursor: pointer;
  text-align: left;
}

.soul-index-row:hover {
  background: rgb(23 36 63 / 5%);
}

.soul-index-row--active {
  background: rgb(201 70 61 / 9%) !important;
  box-shadow: inset 0 3px 0 var(--cinnabar);
}

.soul-icon {
  position: relative;
  display: grid;
  flex: 0 0 auto;
  place-items: center;
  overflow: hidden;
  border: 1px solid rgb(139 71 36 / 28%);
  border-radius: 50% 50% 44% 44%;
  color: var(--accent-deep);
  background: radial-gradient(circle at 50% 35%, #fff 0 28%, transparent 29%), #e8d9c5;
  font-family: STKaiti, KaiTi, serif;
}

.soul-icon::after {
  position: absolute;
  inset: 4px;
  border: 1px solid rgb(255 255 255 / 58%);
  border-radius: inherit;
  content: "";
  pointer-events: none;
}

.soul-icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.soul-icon--small {
  width: 42px;
  height: 42px;
  font-size: 18px;
}

.soul-icon--hero {
  width: 92px;
  height: 92px;
  box-shadow: 0 14px 28px rgb(23 36 63 / 16%);
  font-size: 34px;
}

.soul-index-row__copy {
  display: grid;
  min-width: 0;
  gap: 3px;
}

.soul-index-row__copy strong {
  overflow: hidden;
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.soul-index-row__copy small {
  color: var(--ink-soft);
  font-size: 10px;
}

.rule-dot {
  width: 7px;
  height: 7px;
  margin-left: auto;
  border-radius: 50%;
  background: var(--jade);
  box-shadow: 0 0 0 4px rgb(41 120 105 / 10%);
}

.soul-scroll {
  position: relative;
  overflow: hidden;
  padding: 30px;
  background:
    linear-gradient(
      90deg,
      transparent 0 18px,
      rgb(182 105 58 / 7%) 19px,
      transparent 20px
    ),
    var(--archive-paper);
}

.soul-scroll::after {
  position: absolute;
  top: 0;
  right: 18px;
  width: 1px;
  height: 100%;
  background: rgb(182 105 58 / 9%);
  content: "";
}

.soul-scroll__identity {
  display: flex;
  align-items: center;
  gap: 20px;
  padding-bottom: 26px;
  border-bottom: 1px solid var(--archive-line);
}

.soul-scroll__identity h2 {
  margin-top: 4px;
  font-family: STKaiti, KaiTi, serif;
  font-size: 34px;
}

.soul-category {
  display: inline-block;
  margin-top: 8px;
  padding: 4px 8px;
  border: 1px solid var(--archive-line);
  color: var(--accent-deep);
  font-size: 11px;
}

.effect-block {
  display: grid;
  grid-template-columns: 44px 1fr;
  gap: 14px;
  padding: 24px 0;
  border-bottom: 1px solid var(--archive-line);
}

.effect-block__number {
  display: grid;
  width: 40px;
  height: 40px;
  place-items: center;
  border: 1px solid rgb(201 70 61 / 36%);
  color: var(--cinnabar);
  font-family: STKaiti, KaiTi, serif;
  font-size: 20px;
  transform: rotate(-2deg);
}

.effect-block strong {
  display: block;
  margin-top: 8px;
  color: var(--ink);
  font-size: 14px;
  line-height: 1.75;
}

.effect-block small {
  display: block;
  margin-top: 7px;
  color: var(--ink-soft);
  line-height: 1.6;
}

.unverified-copy {
  color: var(--accent-deep) !important;
}

.mechanics-board {
  padding-top: 28px;
}

.checkpoint-rail {
  position: relative;
  display: grid;
  grid-template-columns: repeat(5, 1fr);
  gap: 5px;
  margin: 18px 0 22px;
}

.checkpoint-rail::before {
  position: absolute;
  top: 50%;
  right: 4%;
  left: 4%;
  height: 1px;
  background: rgb(182 105 58 / 32%);
  content: "";
}

.checkpoint-rail span {
  position: relative;
  z-index: 1;
  justify-self: center;
  padding: 5px 7px;
  border: 1px solid rgb(182 105 58 / 28%);
  border-radius: 999px;
  color: var(--accent-deep);
  background: var(--archive-paper);
  font:
    700 10px Consolas,
    monospace;
}

.probability-switch {
  display: grid;
  grid-template-columns: 1fr 1fr 1.25fr;
  gap: 10px;
  align-items: end;
}

.probability-switch label {
  display: grid;
  gap: 6px;
  color: var(--ink-soft);
  font-size: 11px;
  font-weight: 700;
}

.probability-result {
  display: grid;
  min-height: 66px;
  place-items: center;
  border: 1px solid rgb(41 120 105 / 22%);
  color: var(--jade);
  background: rgb(41 120 105 / 6%);
  text-align: center;
}

.probability-result span {
  font-size: 10px;
}

.probability-result strong {
  font:
    700 24px Consolas,
    monospace;
}

.uniform-lines,
.category-probabilities {
  display: grid;
  gap: 7px;
  margin-top: 12px;
}

.uniform-lines {
  grid-template-columns: repeat(4, 1fr);
}

.uniform-lines span {
  padding: 8px 4px;
  border: 1px solid var(--archive-line);
  color: var(--ink-soft);
  font-size: 10px;
  text-align: center;
}

.uniform-lines span.active {
  border-color: rgb(41 120 105 / 32%);
  color: var(--jade);
  background: rgb(41 120 105 / 7%);
}

.category-probabilities {
  grid-template-columns: repeat(3, 1fr);
}

.category-probabilities div {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  padding: 10px;
  border-bottom: 2px solid var(--accent);
  background: rgb(255 255 255 / 45%);
}

.category-probabilities span {
  color: var(--ink-soft);
  font-size: 10px;
}

.category-probabilities strong {
  font:
    700 17px Consolas,
    monospace;
}

.mechanics-note,
.roll-benchmarks p {
  color: var(--ink-soft);
  font-size: 11px;
  line-height: 1.65;
}

.mechanics-note {
  margin: 14px 0 0;
}

.roll-benchmarks {
  margin-top: 16px;
  border-top: 1px solid var(--archive-line);
  padding-top: 14px;
}

.roll-benchmarks summary,
.advanced-rule-fields summary {
  color: var(--accent-deep);
  cursor: pointer;
  font-size: 12px;
  font-weight: 700;
}

.roll-benchmarks__grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 6px;
  margin-top: 12px;
}

.roll-benchmarks__grid div {
  display: flex;
  justify-content: space-between;
  gap: 8px;
  padding: 7px 8px;
  background: rgb(255 255 255 / 45%);
  font-size: 10px;
}

.set-rule-desk {
  position: sticky;
  top: 18px;
  padding: 24px;
}

.catalog-notes {
  position: sticky;
  top: 18px;
  padding: 24px;
}

.catalog-notes__heading {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 20px;
}

.catalog-notes__heading h2 {
  font-size: 18px;
}

.catalog-facts {
  display: grid;
  gap: 11px;
  margin: 0;
}

.catalog-facts div {
  display: grid;
  grid-template-columns: 70px minmax(0, 1fr);
  gap: 12px;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--line-soft);
}

.catalog-facts dt {
  color: var(--ink-soft);
  font-size: 10px;
}

.catalog-facts dd {
  overflow: hidden;
  margin: 0;
  color: var(--ink);
  font-family: "Cascadia Mono", Consolas, monospace;
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.catalog-note-block {
  margin-top: 18px;
  padding: 12px;
  border-left: 3px solid var(--jade);
  background: rgb(83 123 111 / 8%);
}

.catalog-note-block span {
  color: var(--accent-deep);
  font-size: 10px;
  font-weight: 700;
}

.catalog-note-block p {
  margin: 6px 0 0;
  color: var(--ink-soft);
  font-size: 11px;
  line-height: 1.6;
}

.catalog-note-footnote {
  margin: 18px 0 0;
  color: var(--ink-soft);
  font-size: 10px;
  line-height: 1.65;
}

.set-rule-desk__heading {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 20px;
}

.set-rule-desk__heading h2 {
  font-size: 18px;
}

.rule-choice-group {
  margin: 18px 0 0;
  padding: 0;
  border: 0;
}

.rule-choice-group legend {
  color: var(--ink);
  font-size: 12px;
  font-weight: 700;
}

.rule-choice-group p {
  margin: 5px 0 9px;
  color: var(--ink-soft);
  font-size: 10px;
  line-height: 1.5;
}

.choice-row,
.attribute-choice-grid {
  display: grid;
  gap: 6px;
}

.choice-row {
  grid-template-columns: repeat(3, 1fr);
}

.choice-row--slots {
  grid-template-columns: repeat(6, 1fr);
}

.choice-row button,
.attribute-choice-grid button {
  min-height: 32px;
  padding: 5px;
  font-size: 10px;
}

.attribute-choice-grid {
  grid-template-columns: repeat(3, 1fr);
}

.compact-number {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 10px;
  color: var(--ink-soft);
  font-size: 11px;
}

.compact-number .text-input {
  width: 62px;
}

.advanced-rule-fields {
  margin-top: 18px;
  padding: 14px 0;
  border-top: 1px solid var(--line-soft);
  border-bottom: 1px solid var(--line-soft);
}

.bound-rule-summary {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 7px 12px;
  margin-top: 18px;
  padding: 12px;
  border-left: 3px solid var(--jade);
  background: rgb(41 120 105 / 6%);
  font-size: 10px;
}

.bound-rule-summary span {
  color: var(--ink-soft);
}

.rule-impact-compact {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 8px;
  margin-top: 12px;
}

.rule-impact-compact div {
  display: grid;
  padding: 10px;
  border: 1px solid var(--line-soft);
}

.rule-impact-compact strong {
  font:
    700 20px Consolas,
    monospace;
}

.rule-impact-compact span,
.rule-impact-compact p {
  color: var(--ink-soft);
  font-size: 10px;
}

.rule-impact-compact p {
  grid-column: 1 / -1;
  margin: 0;
}

.set-rule-actions {
  display: grid;
  grid-template-columns: 0.8fr 1.2fr;
  gap: 8px;
  margin-top: 18px;
}

.set-rule-footnote {
  margin: 10px 0 0;
  color: var(--ink-soft);
  font-size: 10px;
  line-height: 1.55;
}

@media (max-width: 1280px) {
  .soul-index__tools {
    grid-template-columns: minmax(220px, 0.8fr) minmax(0, 1.2fr);
  }

  .soul-index__list {
    grid-template-columns: repeat(5, minmax(0, 1fr));
  }
}

@media (max-width: 820px) {
  /* 窄屏将搜索、分类和索引逐层收拢，避免详情内容产生横向溢出。 */
  .soul-archive-layout {
    grid-template-columns: 1fr;
    gap: 14px;
  }

  .soul-index__tools {
    grid-template-columns: 1fr;
    gap: 12px;
  }

  .catalog-category-strip {
    max-height: 80px;
  }

  .soul-index__list {
    grid-template-columns: repeat(3, minmax(0, 1fr));
    max-height: 300px;
  }

  .probability-switch {
    grid-template-columns: 1fr;
  }

  .soul-scroll {
    padding: 22px;
  }
}

@media (max-width: 560px) {
  .soul-index__list {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .soul-scroll__identity {
    align-items: flex-start;
    flex-direction: column;
    gap: 14px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .soul-index-row,
  .catalog-category-strip button,
  .choice-row button,
  .attribute-choice-grid button {
    transition: none;
  }
}

.evidence-card {
  margin-top: 18px;
  padding: 18px;
  border-top: 1px solid var(--line-soft);
  background: rgb(255 250 240 / 76%);
}

.evidence-card .card-heading {
  margin-bottom: 14px;
}

.evidence-list {
  display: grid;
  gap: 9px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.evidence-list li {
  display: grid;
  grid-template-columns: 62px minmax(0, 1fr);
  gap: 10px;
  align-items: center;
  color: var(--ink-soft);
  font-size: 11px;
}

.evidence-list li span {
  color: var(--cinnabar);
  font-size: 10px;
}

.evidence-list li strong {
  overflow: hidden;
  color: var(--ink);
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.hash-line {
  display: block;
  overflow: hidden;
  margin-top: 14px;
  color: var(--ink-soft);
  font-family: "Cascadia Mono", Consolas, monospace;
  font-size: 10px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
