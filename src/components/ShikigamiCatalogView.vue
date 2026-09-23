<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import type { CatalogStatus, Shikigami, ShikigamiRarity } from "../api/contracts";
import { desktopApi } from "../api/client";
import {
  CURRENT_BUILTIN_SHIKIGAMI_COUNT,
  filterShikigami,
  getDefaultShikigami,
  getRarityLabel,
  groupShikigamiByRarity,
  isMaterialShikigamiRarity,
} from "../shikigamiCatalog";
import { getShikigamiWikiUrl } from "../shikigamiWiki";

// 图鉴筛选与目录排序共用完整稀有度集合，素材放在可培养稀有度之后。
const rarityOptions: readonly ShikigamiRarity[] = [
  "UR",
  "SP",
  "SSR",
  "SR",
  "R",
  "N",
  "素材",
];

const entries = ref<Shikigami[]>([]);
const status = ref<CatalogStatus | null>(null);
const selectedId = ref<string | null>(null);
const search = ref("");
const rarity = ref<ShikigamiRarity | "ALL">("ALL");
const loading = ref(true);
const errorMessage = ref<string | null>(null);

/** 调用 Tauri opener 将图鉴中的 Wiki 交给系统默认浏览器，避免 WebView 内打开失败。 */
async function openWiki(entry: Shikigami): Promise<void> {
  await desktopApi.openExternalUrl(getShikigamiWikiUrl(entry));
}

/** 当筛选条件变化时重新计算结果，详情默认跟随首条但不覆盖用户仍可见的选择。 */
const filteredEntries = computed(() =>
  filterShikigami(entries.value, search.value, "ALL", rarity.value),
);
const groups = computed(() => groupShikigamiByRarity(filteredEntries.value));
const selected = computed(
  () =>
    filteredEntries.value.find((entry) => entry.shikigamiId === selectedId.value) ??
    null,
);

/** 保证空列表、筛选无结果或当前项被隐藏时，键盘和详情区仍有确定状态。 */
watch(filteredEntries, (next) => {
  if (next.length === 0) {
    selectedId.value = null;
  } else if (!next.some((entry) => entry.shikigamiId === selectedId.value)) {
    selectedId.value = getDefaultShikigami(next)?.shikigamiId ?? null;
  }
});

/** 读取活动目录；只有首次启动或目录损坏时才安装内置种子，不再把版本写死在前端。 */
async function loadCatalog(): Promise<void> {
  loading.value = true;
  errorMessage.value = null;
  try {
    let current = await desktopApi.getCatalogStatus();
    if (
      !current ||
      current.validationStatus !== "verified" ||
      current.shikigamiCount < CURRENT_BUILTIN_SHIKIGAMI_COUNT ||
      current.panelCoverage === 0 ||
      current.skillCoverage === 0
    ) {
      current = await desktopApi.installBuiltinCatalog();
    }
    const list = await desktopApi.listShikigami(current.version);
    status.value = current;
    entries.value = list;
    selectedId.value = getDefaultShikigami(list)?.shikigamiId ?? null;
  } catch (error) {
    errorMessage.value =
      error instanceof Error ? error.message : "式神目录读取失败，请检查本地目录包。";
  } finally {
    loading.value = false;
  }
}

/** 返回随前端一起打包的本地头像地址，不依赖运行时远程图片。 */
function avatarPath(entry: Shikigami): string {
  return `${import.meta.env.BASE_URL}shikigami-avatars/${entry.shikigamiId}.webp`;
}

/** 头像文件损坏或缺失时隐藏图片，让头像容器保留稀有度底色且不叠加文字。 */
function hideBrokenAvatar(event: Event): void {
  const image = event.currentTarget;
  if (image instanceof HTMLImageElement) image.hidden = true;
}

/** 将中文素材稀有度映射为稳定的 CSS 类名，避免直接把中文值拼入样式选择器。 */
function rarityClass(rarity: string): string {
  return isMaterialShikigamiRarity(rarity) ? "material" : rarity.toLowerCase();
}

/** 面板数值保留必要精度，避免把官方小数误显示成科学计数法。 */
function numberText(value: number): string {
  return Number.isInteger(value)
    ? String(value)
    : value.toFixed(2).replace(/0+$/, "").replace(/\.$/, "");
}

/** 官方暴击率使用 0–1 比例保存，展示为百分比；暴伤加上基础 100%。 */
function percentText(value: number, includeBase = false): string {
  const normalized = includeBase ? 1 + value : value;
  return `${(normalized * 100).toFixed(1).replace(/\.0$/, "")}%`;
}

onMounted(loadCatalog);
</script>

<template>
  <main class="page-main shikigami-page" aria-labelledby="shikigami-title">
    <header class="page-header shikigami-header">
      <div>
        <p class="eyebrow">BASIC DATA / SHIKIGAMI</p>
        <h1 id="shikigami-title">式神图鉴</h1>
        <p class="page-lede">离线式神目录 · 满级属性面板</p>
      </div>
      <div class="shikigami-header__meta" aria-live="polite">
        <span class="catalog-chip"
          >{{ entries.length }} /
          {{ status?.shikigamiCount ?? entries.length }} 条</span
        >
        <span v-if="status" class="catalog-chip catalog-chip--quiet">{{
          status.version
        }}</span>
        <span
          v-if="status?.validationStatus === 'partial'"
          class="catalog-chip catalog-chip--warning"
          >含待核验项</span
        >
      </div>
    </header>

    <div v-if="loading" class="catalog-state" role="status">
      <span class="state-orb" aria-hidden="true">载</span>
      <div>
        <strong>正在打开本地目录</strong>
        <p>首次打开会校验并安装离线资料包。</p>
      </div>
    </div>
    <div
      v-else-if="errorMessage"
      class="catalog-state catalog-state--error"
      role="alert"
    >
      <span class="state-orb" aria-hidden="true">!</span>
      <div>
        <strong>图鉴暂时不可用</strong>
        <p>{{ errorMessage }}</p>
        <button class="text-button" type="button" @click="loadCatalog">重新读取</button>
      </div>
    </div>
    <div v-else class="shikigami-layout">
      <aside class="shikigami-index" aria-label="式神目录筛选">
        <div class="index-toolbar">
          <label class="search-field">
            <span class="sr-only">搜索式神名称</span>
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <circle cx="10.5" cy="10.5" r="6.5" />
              <path d="m16 16 5 5" />
            </svg>
            <input v-model="search" type="search" placeholder="搜索式神名称" />
          </label>
          <div class="rarity-row" aria-label="稀有度筛选">
            <button
              class="rarity-filter"
              :class="{ 'rarity-filter--active': rarity === 'ALL' }"
              type="button"
              @click="rarity = 'ALL'"
            >
              全部
            </button>
            <button
              v-for="item in rarityOptions"
              :key="item"
              class="rarity-filter"
              :class="[
                `rarity-filter--${rarityClass(item)}`,
                { 'rarity-filter--active': rarity === item },
              ]"
              type="button"
              @click="rarity = item"
            >
              {{ item }}
            </button>
          </div>
        </div>

        <div v-if="filteredEntries.length === 0" class="index-empty" role="status">
          没有匹配的式神<br /><small>试试清空搜索或切换稀有度。</small>
        </div>
        <div v-else class="index-groups">
          <section
            v-for="group in groups"
            v-show="group.entries.length > 0"
            :key="group.rarity"
            class="index-group"
          >
            <div class="index-group__heading">
              <span
                :class="['rarity-badge', `rarity-badge--${rarityClass(group.rarity)}`]"
                >{{ group.label }}</span
              ><span>{{ group.entries.length }}</span>
            </div>
            <div class="index-list" role="listbox" :aria-label="`${group.label}式神`">
              <button
                v-for="entry in group.entries"
                :key="entry.shikigamiId"
                class="index-entry"
                :class="{ 'index-entry--selected': selectedId === entry.shikigamiId }"
                type="button"
                role="option"
                :aria-selected="selectedId === entry.shikigamiId"
                @click="selectedId = entry.shikigamiId"
              >
                <!-- 列表项只保留头像和名称，避免额外文本节点破坏图鉴的紧凑纵向节奏。 -->
                <span
                  :class="[
                    'entry-avatar',
                    `entry-avatar--${rarityClass(entry.rarity)}`,
                  ]"
                  :title="`${entry.name}头像 · ${entry.iconAssetId ?? '离线印记'}`"
                  aria-hidden="true"
                >
                  <img
                    class="avatar-image"
                    :src="avatarPath(entry)"
                    alt=""
                    @error="hideBrokenAvatar"
                  />
                </span>
                <span class="index-entry__name">{{ entry.name }}</span>
              </button>
            </div>
          </section>
        </div>
      </aside>

      <section v-if="selected" class="shikigami-detail" aria-live="polite">
        <header class="detail-hero">
          <div
            :class="['hero-avatar', `hero-avatar--${rarityClass(selected.rarity)}`]"
            :title="`${selected.name}头像 · ${selected.iconAssetId ?? '离线印记'}`"
            aria-hidden="true"
          >
            <img
              class="avatar-image"
              :src="avatarPath(selected)"
              alt=""
              @error="hideBrokenAvatar"
            />
          </div>
          <div class="detail-hero__title">
            <div class="detail-kicker">
              <span
                :class="[
                  'rarity-badge',
                  `rarity-badge--${rarityClass(selected.rarity)}`,
                ]"
                >{{ getRarityLabel(selected.rarity) }}</span
              ><span>{{ selected.shikigamiId }}</span>
            </div>
            <h2>{{ selected.name }}</h2>
            <p v-if="isMaterialShikigamiRarity(selected.rarity)">
              素材式神 · 式神仓库只记录持有数量
            </p>
            <p v-else>满级 40 级属性面板</p>
            <a
              class="detail-wiki-link"
              :href="getShikigamiWikiUrl(selected)"
              rel="noopener noreferrer"
              :aria-label="`查看${selected.name} Wiki`"
              @click.prevent="openWiki(selected)"
            >
              查看wiki
            </a>
          </div>
          <div class="detail-hero__seal" aria-label="离线资料">离线<br />资料</div>
        </header>

        <div
          v-if="isMaterialShikigamiRarity(selected.rarity)"
          class="detail-grid material-detail-grid"
        >
          <section
            class="detail-card material-detail-card"
            aria-labelledby="material-title"
          >
            <div class="card-heading">
              <div>
                <span class="section-index">01</span>
                <h3 id="material-title">素材说明</h3>
              </div>
              <span class="card-caption">无培养面板</span>
            </div>
            <div class="material-detail-card__body">
              <div class="material-detail-card__mark" aria-hidden="true">素材</div>
              <div>
                <strong>{{ selected.name }}</strong>
                <p>
                  这是式神仓库中的素材式神，游戏按「分组 +
                  数量」保存，不记录等级、御魂和技能。
                </p>
              </div>
            </div>
          </section>
        </div>
        <div v-else class="detail-grid">
          <section class="detail-card panel-card" aria-labelledby="panel-title">
            <div class="card-heading">
              <div>
                <span class="section-index">01</span>
                <h3 id="panel-title">满级面板</h3>
              </div>
              <span class="card-caption">{{ selected.panelLevel }} 级基础值</span>
            </div>
            <div class="panel-grid">
              <div>
                <span
                  >攻击 <b>{{ selected.panel.grades.attack }}</b></span
                ><strong>{{ numberText(selected.panel.attack) }}</strong>
              </div>
              <div>
                <span
                  >生命 <b>{{ selected.panel.grades.hp }}</b></span
                ><strong>{{ numberText(selected.panel.hp) }}</strong>
              </div>
              <div>
                <span
                  >防御 <b>{{ selected.panel.grades.defense }}</b></span
                ><strong>{{ numberText(selected.panel.defense) }}</strong>
              </div>
              <div>
                <span
                  >速度 <b>{{ selected.panel.grades.speed }}</b></span
                ><strong>{{ numberText(selected.panel.speed) }}</strong>
              </div>
              <div>
                <span
                  >暴击 <b>{{ selected.panel.grades.critRate }}</b></span
                ><strong>{{ percentText(selected.panel.critRate) }}</strong>
              </div>
              <div>
                <span>暴伤</span
                ><strong>{{ percentText(selected.panel.critDamage, true) }}</strong>
              </div>
            </div>
            <p class="card-note">不含御魂、被动转换、队伍增益与实战面板计算。</p>
          </section>
        </div>

        <footer class="detail-source">
          <span>数据版本：{{ status?.version ?? selected.catalogVersion }}</span
          ><span>游戏版本：{{ selected.gameVersion }}</span
          ><span
            >来源：{{ selected.evidence.map((item) => item.source).join("、") }}</span
          ><span
            v-if="isMaterialShikigamiRarity(selected.rarity)"
            class="verification-note"
            >素材条目已纳入目录；不参与面板与技能覆盖统计</span
          ><span v-else class="verification-note">当前条目已完成结构核验</span>
        </footer>
      </section>
      <div v-else class="catalog-state catalog-state--empty" role="status">
        <span class="state-orb" aria-hidden="true">阅</span>
        <div>
          <strong>没有可展示的式神</strong>
          <p>当前筛选条件下没有结果。</p>
        </div>
      </div>
    </div>
  </main>
</template>

<style scoped>
.shikigami-page {
  max-width: 1440px;
  /* 图鉴直接挂载在应用壳网格中，沿用式神录的页内留白，避免标题和卡片贴住窗口边缘。 */
  padding: 34px clamp(24px, 4vw, 64px) 28px;
}
.shikigami-header {
  align-items: flex-end;
}
.shikigami-header__meta {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  justify-content: flex-end;
}
.catalog-chip {
  border: 1px solid var(--line);
  color: var(--accent-deep);
  background: rgba(255, 255, 255, 0.58);
  padding: 7px 11px;
  border-radius: 999px;
  font-size: 11px;
  letter-spacing: 0.08em;
}
.catalog-chip--quiet {
  color: var(--ink-soft);
  border-color: var(--line-soft);
}
.catalog-chip--warning {
  color: var(--cinnabar);
  border-color: rgba(164, 70, 53, 0.25);
  background: rgba(164, 70, 53, 0.06);
}
.shikigami-layout {
  display: grid;
  grid-template-columns: minmax(250px, 300px) minmax(0, 1fr);
  gap: 20px;
  align-items: start;
}
.shikigami-index,
.detail-card,
.detail-hero {
  background: rgba(255, 255, 255, 0.62);
  border: 1px solid var(--line-soft);
  box-shadow: 0 12px 30px rgba(61, 53, 44, 0.05);
}
.shikigami-index {
  position: sticky;
  top: 18px;
  max-height: calc(100vh - 130px);
  overflow: auto;
  border-radius: 14px;
}
.index-toolbar {
  padding: 14px;
  border-bottom: 1px solid var(--line-soft);
}
.search-field {
  display: flex;
  align-items: center;
  gap: 8px;
  border: 1px solid var(--line);
  background: var(--paper);
  padding: 8px 10px;
  border-radius: 8px;
}
.search-field svg {
  width: 16px;
  height: 16px;
  fill: none;
  stroke: var(--ink-soft);
  stroke-width: 1.6;
}
.search-field input {
  width: 100%;
  border: 0;
  outline: 0;
  background: transparent;
  color: var(--ink);
  font: inherit;
  font-size: 12px;
}
.rarity-row {
  display: flex;
  gap: 5px;
  margin-top: 10px;
  flex-wrap: wrap;
}
.rarity-filter {
  border: 1px solid transparent;
  background: transparent;
  color: var(--ink-soft);
  cursor: pointer;
  border-radius: 999px;
  padding: 5px 9px;
  font: inherit;
  font-size: 11px;
}
.rarity-filter:hover,
.rarity-filter--active {
  color: var(--accent-deep);
  background: rgba(31, 119, 103, 0.08);
  border-color: rgba(31, 119, 103, 0.2);
}
.rarity-filter {
  font-weight: 700;
  letter-spacing: 0.04em;
}
.rarity-filter--sp {
  color: #8b4d73;
}
.rarity-filter--ur {
  color: #8a4f98;
}
.rarity-filter--ssr {
  color: #9f6f28;
}
.rarity-filter--sr {
  color: #487c6d;
}
.rarity-filter--r {
  color: #6480a3;
}
.rarity-filter--n {
  color: #73706b;
}
.rarity-filter--material {
  color: #8a5a35;
}
.index-groups {
  padding: 8px 10px 14px;
}
.index-group {
  margin-top: 8px;
}
.index-group__heading {
  display: flex;
  justify-content: space-between;
  align-items: center;
  color: var(--ink-faint);
  font-size: 11px;
  padding: 5px 4px;
}
.rarity-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 34px;
  padding: 3px 7px;
  border-radius: 5px;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.08em;
  color: white;
}
.rarity-badge--sp {
  background: #87506f;
}
.rarity-badge--ur {
  background: linear-gradient(135deg, #8c5bb0, #4c3774);
}
.rarity-badge--ssr {
  background: #a3752f;
}
.rarity-badge--sr {
  background: #4e8777;
}
.rarity-badge--r {
  background: #6e8fb4;
}
.rarity-badge--n,
.rarity-badge--unknown {
  background: #79756d;
}
.rarity-badge--material {
  background: linear-gradient(135deg, #b87945, #765033);
}
.index-list {
  display: grid;
  gap: 2px;
}
.index-entry {
  display: flex;
  gap: 8px;
  align-items: center;
  width: 100%;
  border: 1px solid transparent;
  background: transparent;
  padding: 6px;
  border-radius: 8px;
  text-align: left;
  cursor: pointer;
  color: var(--ink);
  font: inherit;
}
.index-entry:hover,
.index-entry--selected {
  background: rgba(31, 119, 103, 0.08);
  border-color: rgba(31, 119, 103, 0.18);
}
.index-entry--selected .index-entry__name {
  color: var(--accent-deep);
  font-weight: 700;
}
.entry-avatar {
  position: relative;
  display: grid;
  flex: 0 0 26px;
  place-items: center;
  width: 26px;
  height: 26px;
  border-radius: 50%;
  color: white;
  font-family: "Noto Serif SC", serif;
  font-size: 12px;
  overflow: hidden;
  background: transparent;
}
.avatar-image {
  position: absolute;
  inset: 0;
  z-index: 1;
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.index-entry__name {
  flex: 1;
  font-size: 12px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.index-entry__count {
  color: var(--ink-faint);
  font-size: 10px;
}
.index-empty {
  padding: 30px 18px;
  color: var(--ink-soft);
  text-align: center;
  line-height: 1.8;
}
.index-empty small {
  color: var(--ink-faint);
}
.shikigami-detail {
  min-width: 0;
  display: grid;
  gap: 16px;
}
.detail-hero {
  display: flex;
  align-items: center;
  gap: 15px;
  padding: 19px 22px;
  border-radius: 14px;
}
.hero-avatar {
  position: relative;
  display: grid;
  flex: 0 0 76px;
  place-items: center;
  width: 76px;
  height: 76px;
  border-radius: 14px;
  color: white;
  font-family: "Noto Serif SC", serif;
  font-size: 36px;
  overflow: hidden;
  background: transparent;
}
.detail-hero__title {
  flex: 1;
  min-width: 0;
}
.detail-kicker {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--ink-faint);
  font-size: 11px;
}
.detail-hero h2 {
  margin: 7px 0 3px;
  color: var(--ink);
  font-family: "Noto Serif SC", serif;
  font-size: clamp(24px, 3vw, 34px);
  font-weight: 700;
}
.detail-hero p {
  margin: 0;
  color: var(--ink-soft);
  font-size: 12px;
}

/* Wiki 链接紧跟图鉴摘要，沿用式神录的轻量胶囊样式并保留键盘焦点反馈。 */
.detail-wiki-link {
  display: inline-flex;
  width: fit-content;
  margin-top: 9px;
  padding: 4px 9px;
  border: 1px solid rgba(31, 119, 103, 0.24);
  border-radius: 999px;
  color: var(--accent-deep);
  background: rgba(31, 119, 103, 0.07);
  font-size: 11px;
  text-decoration: none;
  transition:
    color 160ms ease,
    background 160ms ease,
    border-color 160ms ease;
}

.detail-wiki-link:hover,
.detail-wiki-link:focus-visible {
  border-color: rgba(31, 119, 103, 0.42);
  color: white;
  background: var(--accent-deep);
}

.detail-hero__seal {
  flex: 0 0 48px;
  border: 1px solid var(--cinnabar);
  color: var(--cinnabar);
  padding: 7px 3px;
  text-align: center;
  font-family: "Noto Serif SC", serif;
  font-size: 11px;
  line-height: 1.35;
  transform: rotate(3deg);
}
.detail-grid {
  display: grid;
  /* 当前详情区只有一张面板卡，必须占满可用宽度，避免被遗留的双列模板压缩。 */
  grid-template-columns: minmax(0, 1fr);
  gap: 16px;
}
.material-detail-grid {
  grid-template-columns: minmax(0, 1fr);
}
.material-detail-card__body {
  display: flex;
  gap: 18px;
  align-items: center;
  padding: 12px 2px 2px;
}
.material-detail-card__mark {
  display: grid;
  flex: 0 0 82px;
  width: 82px;
  height: 82px;
  place-items: center;
  border: 1px solid rgba(138, 90, 53, 0.28);
  border-radius: 18px;
  color: #765033;
  background: linear-gradient(
    145deg,
    rgba(184, 121, 69, 0.18),
    rgba(118, 80, 51, 0.08)
  );
  font-family: "Noto Serif SC", serif;
  font-size: 18px;
  font-weight: 700;
}
.material-detail-card__body strong {
  color: var(--ink);
  font-family: "Noto Serif SC", serif;
  font-size: 20px;
}
.material-detail-card__body p {
  max-width: 600px;
  margin: 7px 0 0;
  color: var(--ink-soft);
  font-size: 12px;
  line-height: 1.8;
}
.detail-card {
  border-radius: 14px;
  padding: 18px 20px;
}
.card-heading {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 12px;
  margin-bottom: 15px;
}
.card-heading > div {
  display: flex;
  align-items: center;
  gap: 9px;
}
.section-index {
  color: var(--cinnabar);
  font-family: ui-monospace, monospace;
  font-size: 11px;
}
.card-heading h3 {
  margin: 0;
  color: var(--ink);
  font-family: "Noto Serif SC", serif;
  font-size: 17px;
}
.card-caption {
  color: var(--ink-faint);
  font-size: 11px;
}
.panel-grid {
  display: grid;
  /* 指标值和评级标签有固定的最低可读宽度，窄窗口时自动换列而不是互相覆盖。 */
  grid-template-columns: repeat(auto-fit, minmax(64px, 1fr));
  gap: 7px;
}
.panel-grid > div {
  padding: 11px 8px;
  border: 1px solid var(--line-soft);
  background: rgba(252, 249, 241, 0.62);
  border-radius: 8px;
}
.panel-grid span {
  display: block;
  color: var(--ink-faint);
  font-size: 11px;
}
.panel-grid span b {
  display: inline-grid;
  min-width: 24px;
  height: 18px;
  margin-left: 4px;
  padding: 0 3px;
  place-items: center;
  border: 1px solid rgba(164, 70, 53, 0.28);
  border-radius: 9px;
  color: var(--cinnabar);
  font-family: ui-monospace, monospace;
  font-size: 10px;
}
.panel-grid strong {
  display: block;
  margin-top: 8px;
  color: var(--ink);
  font-size: 15px;
}
.card-note {
  margin: 13px 0 0;
  color: var(--ink-faint);
  font-size: 11px;
  line-height: 1.65;
}
.detail-source {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 15px;
  color: var(--ink-faint);
  font-size: 10px;
  line-height: 1.5;
}
.verification-note {
  color: var(--accent-deep);
}
.catalog-state {
  display: flex;
  justify-content: center;
  align-items: center;
  gap: 14px;
  min-height: 240px;
  border: 1px dashed var(--line);
  border-radius: 14px;
  color: var(--ink);
}
.catalog-state strong {
  font-family: "Noto Serif SC", serif;
}
.catalog-state p {
  margin: 5px 0 0;
  color: var(--ink-soft);
  font-size: 12px;
}
.catalog-state--error {
  border-color: rgba(164, 70, 53, 0.35);
}
.state-orb {
  display: grid;
  place-items: center;
  width: 44px;
  height: 44px;
  border: 1px solid var(--cinnabar);
  border-radius: 50%;
  color: var(--cinnabar);
  font-family: "Noto Serif SC", serif;
}
.text-button {
  margin-top: 10px;
  padding: 0;
  border: 0;
  background: none;
  color: var(--accent-deep);
  cursor: pointer;
  font: inherit;
  font-size: 12px;
}
.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}
@media (max-width: 980px) {
  .shikigami-layout {
    grid-template-columns: minmax(220px, 260px) minmax(0, 1fr);
    gap: 12px;
  }
  .detail-grid {
    grid-template-columns: 1fr;
  }
  .panel-grid {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
}
@media (max-width: 720px) {
  .shikigami-layout {
    display: block;
  }
  .shikigami-index {
    position: static;
    max-height: 380px;
    margin-bottom: 12px;
  }
  .shikigami-header {
    display: block;
  }
  .shikigami-header__meta {
    justify-content: flex-start;
    margin-top: 12px;
  }
  .detail-hero {
    padding: 15px;
  }
  .hero-avatar {
    flex-basis: 58px;
    width: 58px;
    height: 58px;
    font-size: 28px;
  }
  .detail-hero__seal {
    display: none;
  }
  .detail-card {
    padding: 15px;
  }
}
@media (prefers-reduced-motion: reduce) {
  .index-entry,
  .rarity-filter {
    transition: none;
  }
}
</style>
