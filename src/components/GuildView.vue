<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import type { GuildMember, GuildOverview } from "../api/contracts";
import CharacterArchiveSwitcher from "./CharacterArchiveSwitcher.vue";
import {
  activeCharacterId,
  characterArchives,
  onActiveCharacterChanged,
} from "../state/activeCharacter";

// 浏览器静态预览没有 Tauri 事件桥，保持静默空态，避免 invoke 报错刷屏。
const isTauriRuntime = computed(
  () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window,
);

// 职务排序：会长 → 副会长 → 普通成员；未知编码排在最后。
const DUTY_ORDER: Record<number, number> = { 1: 0, 2: 1 };
type GuildSortKey =
  | "dutyCode"
  | "name"
  | "level"
  | "joinedAt"
  | "weeklyFeats"
  | "totalFeats"
  | "historyDonate"
  | "offlineAt";
type SortDirection = "asc" | "desc";

// 数字列默认从高到低，姓名和日期默认从前到后，第一次点击即可得到常用的查看顺序。
const DEFAULT_SORT_DIRECTION: Record<GuildSortKey, SortDirection> = {
  dutyCode: "asc",
  name: "asc",
  level: "desc",
  joinedAt: "asc",
  weeklyFeats: "desc",
  totalFeats: "desc",
  historyDonate: "desc",
  offlineAt: "asc",
};

const guild = ref<GuildOverview | null>(null);
const loadError = ref("");
const loaded = ref(false);
// 名称搜索与周贡上限只影响当前页面展示，不修改导入快照中的原始成员数据。
const memberSearch = ref("");
const weeklyLimitInput = ref("");
const sortKey = ref<GuildSortKey>("dutyCode");
const sortDirection = ref<SortDirection>("asc");

// 将用户输入的周贡上限收敛为非负整数；空值或非法输入代表不启用筛选。
const weeklyLimit = computed<number | null>(() => {
  const input = weeklyLimitInput.value.trim();
  if (!input) return null;
  const value = Number(input);
  return Number.isFinite(value) && value >= 0 ? Math.floor(value) : null;
});

// 先做名称/周贡筛选，再交给统一排序逻辑，保证表头排序不会绕过当前筛选条件。
const filteredMembers = computed(() => {
  const keyword = memberSearch.value.trim().toLocaleLowerCase("zh-CN");
  const limit = weeklyLimit.value;
  return (guild.value?.members ?? []).filter((member) => {
    const nameMatched =
      !keyword || member.name.toLocaleLowerCase("zh-CN").includes(keyword);
    const weeklyMatched = limit === null || member.weeklyFeats < limit;
    return nameMatched && weeklyMatched;
  });
});

const members = computed(() => {
  const list = [...filteredMembers.value];
  list.sort((a, b) => {
    const comparison = compareMembers(a, b, sortKey.value);
    if (comparison !== 0) {
      return sortDirection.value === "asc" ? comparison : -comparison;
    }
    return a.name.localeCompare(b.name, "zh-CN");
  });
  return list;
});

// 当前筛选条件的简短回显，让“列表变少”有明确原因。
const activeFilterDescription = computed(() => {
  const conditions: string[] = [];
  if (memberSearch.value.trim())
    conditions.push(`名称含“${memberSearch.value.trim()}”`);
  if (weeklyLimit.value !== null) conditions.push(`周贡低于 ${weeklyLimit.value}`);
  return conditions.length > 0 ? conditions.join(" · ") : "显示全部成员";
});

/** 按表头字段比较成员；相同字段按成员名称稳定排序。 */
function compareMembers(a: GuildMember, b: GuildMember, key: GuildSortKey): number {
  if (key === "name") return a.name.localeCompare(b.name, "zh-CN");
  if (key === "dutyCode") {
    return (DUTY_ORDER[a.dutyCode] ?? 9) - (DUTY_ORDER[b.dutyCode] ?? 9);
  }
  const aValue = a[key];
  const bValue = b[key];
  if (aValue === null) return bValue === null ? 0 : 1;
  if (bValue === null) return -1;
  return Number(aValue) - Number(bValue);
}

/** 点击同一表头切换升降序，切换到新字段时采用该字段的默认方向。 */
function sortMembersBy(key: GuildSortKey): void {
  if (sortKey.value === key) {
    sortDirection.value = sortDirection.value === "asc" ? "desc" : "asc";
    return;
  }
  sortKey.value = key;
  sortDirection.value = DEFAULT_SORT_DIRECTION[key];
}

/** 为当前排序列提供无障碍属性；未排序列明确标记为 none。 */
function sortAria(key: GuildSortKey): "ascending" | "descending" | "none" {
  if (sortKey.value !== key) return "none";
  return sortDirection.value === "asc" ? "ascending" : "descending";
}

function sortIndicator(key: GuildSortKey): string {
  if (sortKey.value !== key) return "↕";
  return sortDirection.value === "asc" ? "↑" : "↓";
}

/** 清除成员查询条件，但保留用户当前的排序选择，避免重复定位时列表跳回原顺序。 */
function clearMemberFilters(): void {
  memberSearch.value = "";
  weeklyLimitInput.value = "";
}

// 寮与角色必然同区，而寮快照本身不带区服名（内存探针读不到），因此取角色档案的区服名。
const serverLabel = computed(
  () =>
    characterArchives.value.find((entry) => entry.profileId === activeCharacterId.value)
      ?.serverLabel ?? null,
);

// 当前角色使用激活档案中的昵称；档案列表异步加载完成后，名册会自动重新判断印章归属。
const currentCharacterName = computed(
  () =>
    characterArchives.value
      .find((entry) => entry.profileId === activeCharacterId.value)
      ?.displayName.trim() ?? "",
);

/** 玩法进度条目：寮突破 / 狭间 / 寮战赛季，三项均来自快照 extra。 */
const progressItems = computed(() => {
  const current = guild.value;
  if (!current) return [];
  return [
    { label: "寮突破", value: `第 ${current.maxGveDifficulty} 层` },
    {
      label: "狭间",
      value: `难度 ${current.creviceDifficulty}（已通关 ${current.creviceDefeatedDifficulty}）`,
    },
    { label: "寮战赛季", value: `第 ${current.pvpSeason} 季` },
  ];
});

/** 职务徽章的配色：会长为朱红、副会长为赭黄、普通成员为灰。 */
function dutyTone(dutyCode: number): string {
  switch (dutyCode) {
    case 1:
      return "leader";
    case 2:
      return "vice";
    default:
      return "member";
  }
}

/** 仅给当前激活角色对应的名册成员显示“本大人”印章；空昵称不参与匹配。 */
function isCurrentCharacter(memberName: string): boolean {
  const currentName = currentCharacterName.value;
  return currentName.length > 0 && memberName.trim() === currentName;
}

function formatDate(value: string | number): string {
  if (!value) return "—";
  if (typeof value === "string") return value;
  const date = new Date(value * 1000);
  if (Number.isNaN(date.getTime())) return "—";
  return date.toLocaleDateString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });
}

function formatCount(value: number): string {
  return value.toLocaleString("zh-CN");
}

function formatFunds(value: number): string {
  return value.toLocaleString("zh-CN", { maximumFractionDigits: 0 });
}

/** 排名展示：-1 是游戏侧的未上榜哨兵值，0 视为数据缺失。 */
function formatRank(value: number): string {
  if (value < 0) return "未上榜";
  if (value === 0) return "—";
  return `第 ${formatCount(value)} 名`;
}

/** 转义 Excel HTML 单元格内容，避免成员名称或宣言中的特殊字符破坏导出表格。 */
function escapeExcelCell(value: string | number): string {
  return String(value).replace(
    /[&<>"']/g,
    (character) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        character
      ] ?? character,
  );
}

/** 生成本地日期文件名，避免 UTC 日期在晚间导出时跨日。 */
function exportDateLabel(): string {
  const now = new Date();
  const year = now.getFullYear();
  const month = String(now.getMonth() + 1).padStart(2, "0");
  const day = String(now.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

/**
 * 导出当前寮概况和筛选后的成员名册；使用 Excel 可打开的 HTML 表格格式，避免引入额外运行时依赖。
 * 导出只在本机生成 Blob 并交给浏览器下载，不会上传寮成员信息。
 */
function exportGuildExcel(): void {
  const current = guild.value;
  if (!current) return;

  const summaryRows: Array<[string, string | number]> = [
    ["寮名称", current.name],
    ["寮等级", current.level],
    ["建寮日期", formatDate(current.createDate)],
    ["会长", current.leaderName || "—"],
    ["服务器", serverLabel.value || "—"],
    ["寮宣言", current.declare || "—"],
    ["活跃成员 / 总成员", `${current.activeMemberCount} / ${current.memberCount}`],
    ["寮资金", current.funds],
    ["寮勋", current.activeScore],
    ["建设度", current.construction],
    ["活跃排名", formatRank(current.activeRank)],
    ["寮战积分", current.pvpScore],
    ["寮战排名", formatRank(current.pvpRank)],
    ["勋章", current.insigniaCount],
    ["寮突破", `第 ${current.maxGveDifficulty} 层`],
    [
      "狭间",
      `难度 ${current.creviceDifficulty}（已通关 ${current.creviceDefeatedDifficulty}）`,
    ],
    ["寮战赛季", `第 ${current.pvpSeason} 季`],
  ];
  const memberHeaders = [
    "成员名称",
    "身份",
    "等级",
    "加入日期",
    "本周功绩",
    "总功绩",
    "累计捐献",
    "最后在线",
  ];
  const memberRows = members.value.map((member) => [
    member.name || "—",
    member.dutyLabel,
    member.level,
    formatDate(member.joinedDate),
    member.weeklyFeats,
    member.totalFeats,
    member.historyDonate,
    member.offlineAt === 0 ? "在线中" : member.offlineAtLabel || "—",
  ]);
  const renderRow = (cells: Array<string | number>, header = false): string => {
    const tag = header ? "th" : "td";
    const renderedCells = cells
      .map((cell) => `<${tag}>${escapeExcelCell(cell)}</${tag}>`)
      .join("");
    return `<tr>${renderedCells}</tr>`;
  };
  const summaryTable = summaryRows.map((row) => renderRow(row)).join("");
  const memberHeader = renderRow(memberHeaders, true);
  const memberBody = memberRows.map((row) => renderRow(row)).join("");
  const workbook = `<!DOCTYPE html>
<html xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel">
  <head>
    <meta charset="UTF-8" />
    <meta http-equiv="content-type" content="application/vnd.ms-excel; charset=UTF-8" />
    <style>
      body { font-family: Arial, "Microsoft YaHei", sans-serif; }
      h1 { color: #a64b3f; font-size: 18pt; }
      h2 { color: #344b75; font-size: 14pt; margin-top: 18px; }
      table { border-collapse: collapse; margin-bottom: 12px; }
      th, td { border: 1px solid #d9d0c4; padding: 6px 10px; white-space: nowrap; }
      th { background: #f8f3e9; font-weight: bold; }
    </style>
  </head>
  <body>
    <h1>${escapeExcelCell(current.name)} · 寮数据</h1>
    <table><tbody>${summaryTable}</tbody></table>
    <h2>成员名册（${members.value.length} 人）</h2>
    <table><thead>${memberHeader}</thead><tbody>${memberBody}</tbody></table>
  </body>
</html>`;
  const fileName = `${current.name.trim().replace(/[\\/:*?"<>|]/g, "_") || "寮数据"}-寮数据-${exportDateLabel()}.xls`;
  const blob = new Blob(["\ufeff", workbook], {
    type: "application/vnd.ms-excel;charset=utf-8",
  });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  anchor.click();
  URL.revokeObjectURL(url);
}

function openDesktopReading(): void {
  window.dispatchEvent(new Event("open-desktop-reading"));
}

async function loadGuild(): Promise<void> {
  if (!isTauriRuntime.value) {
    loaded.value = true;
    return;
  }
  try {
    guild.value = await desktopApi.getCurrentGuild();
  } catch (error) {
    loadError.value =
      typeof error === "object" && error !== null && "message" in error
        ? String(error.message)
        : "读取寮数据失败";
  } finally {
    loaded.value = true;
  }
}

onMounted(() => {
  void loadGuild();
  stopActiveCharacterChanged = onActiveCharacterChanged(() => {
    guild.value = null;
    loadError.value = "";
    loaded.value = false;
    void loadGuild();
  });
});

let stopActiveCharacterChanged: (() => void) | null = null;
onBeforeUnmount(() => {
  stopActiveCharacterChanged?.();
  stopActiveCharacterChanged = null;
});
</script>

<template>
  <main class="page-main guild-page" aria-labelledby="guild-title">
    <header class="guild-header">
      <div>
        <p class="eyebrow">ASSETS / GUILD LEDGER</p>
        <h1 id="guild-title">寮管理</h1>
      </div>
      <div class="guild-header__meta">
        <CharacterArchiveSwitcher />
        <span class="guild-readonly"><i aria-hidden="true"></i>只读档案</span>
        <span v-if="guild" class="guild-current-name">{{ guild.name }}</span>
      </div>
    </header>

    <p v-if="loadError" class="guild-load-error" role="alert">{{ loadError }}</p>

    <template v-if="guild">
      <section class="guild-summary" aria-label="寮概况">
        <div class="guild-summary__seal" aria-hidden="true">
          <span>{{ guild.level }}</span>
          <small>级</small>
        </div>
        <div class="guild-summary__body">
          <h2>{{ guild.name }}</h2>
          <p class="guild-summary__sub">
            建寮于 {{ formatDate(guild.createDate) }}
            <span v-if="guild.leaderName"> · 会长 {{ guild.leaderName }}</span>
            <span v-if="serverLabel"> · {{ serverLabel }}</span>
          </p>
          <p v-if="guild.declare" class="guild-summary__declare">{{ guild.declare }}</p>
        </div>
        <dl class="guild-summary__stats">
          <div>
            <dt>成员</dt>
            <dd>
              {{ guild.activeMemberCount }} / {{ guild.memberCount }}
              <small>活跃 / 总数</small>
            </dd>
          </div>
          <div>
            <dt>寮资金</dt>
            <dd>{{ formatFunds(guild.funds) }}</dd>
          </div>
          <div>
            <dt>寮勋</dt>
            <dd>{{ formatFunds(guild.activeScore) }}</dd>
          </div>
          <div>
            <dt>建设度</dt>
            <dd>{{ formatCount(guild.construction) }}</dd>
          </div>
          <div>
            <dt>活跃排名</dt>
            <dd>{{ formatRank(guild.activeRank) }}</dd>
          </div>
          <div>
            <dt>寮战积分</dt>
            <dd>{{ formatCount(guild.pvpScore) }}</dd>
          </div>
          <div>
            <dt>寮战排名</dt>
            <dd>{{ formatRank(guild.pvpRank) }}</dd>
          </div>
          <div>
            <dt>勋章</dt>
            <dd>{{ formatCount(guild.insigniaCount) }}</dd>
          </div>
        </dl>
      </section>

      <dl class="guild-progress">
        <div v-for="item in progressItems" :key="item.label">
          <dt>{{ item.label }}</dt>
          <dd>{{ item.value }}</dd>
        </div>
      </dl>

      <section class="guild-members" aria-label="寮成员名册">
        <div class="guild-members__head">
          <h3>成员名册</h3>
          <span class="guild-members__count"
            >{{ members.length }} / {{ guild.members.length }} 名</span
          >
        </div>
        <div class="guild-members__overview">
          <div class="guild-member-insight">
            <span>当前筛选</span>
            <strong>{{ members.length }} 人</strong>
            <small>{{ activeFilterDescription }}</small>
          </div>
        </div>
        <div class="guild-members__toolbar" aria-label="成员筛选工具">
          <label class="guild-search-field">
            <span>搜索寮友</span>
            <input
              v-model="memberSearch"
              type="search"
              placeholder="输入成员名称"
              aria-label="搜索寮友名称"
            />
          </label>
          <label class="guild-weekly-filter">
            <span>周贡低于</span>
            <input
              v-model="weeklyLimitInput"
              type="number"
              min="0"
              inputmode="numeric"
              placeholder="不限"
              aria-label="周贡低于多少"
            />
          </label>
          <button
            v-if="memberSearch || weeklyLimitInput"
            class="guild-filter-clear"
            type="button"
            @click="clearMemberFilters"
          >
            清除筛选
          </button>
          <button
            class="guild-export-button"
            type="button"
            title="导出当前筛选后的寮概况和成员名册"
            @click="exportGuildExcel"
          >
            导出 Excel
          </button>
        </div>
        <div class="guild-table-wrap">
          <table class="guild-table">
            <thead>
              <tr>
                <th scope="col" :aria-sort="sortAria('name')">
                  <button
                    class="guild-sort-button"
                    type="button"
                    @click="sortMembersBy('name')"
                  >
                    成员名称 <span aria-hidden="true">{{ sortIndicator("name") }}</span>
                  </button>
                </th>
                <th scope="col" :aria-sort="sortAria('dutyCode')">
                  <button
                    class="guild-sort-button"
                    type="button"
                    @click="sortMembersBy('dutyCode')"
                  >
                    身份 <span aria-hidden="true">{{ sortIndicator("dutyCode") }}</span>
                  </button>
                </th>
                <th scope="col" :aria-sort="sortAria('level')">
                  <button
                    class="guild-sort-button"
                    type="button"
                    @click="sortMembersBy('level')"
                  >
                    等级 <span aria-hidden="true">{{ sortIndicator("level") }}</span>
                  </button>
                </th>
                <th scope="col" :aria-sort="sortAria('joinedAt')">
                  <button
                    class="guild-sort-button"
                    type="button"
                    @click="sortMembersBy('joinedAt')"
                  >
                    加入日期
                    <span aria-hidden="true">{{ sortIndicator("joinedAt") }}</span>
                  </button>
                </th>
                <th scope="col" :aria-sort="sortAria('weeklyFeats')">
                  <button
                    class="guild-sort-button"
                    type="button"
                    @click="sortMembersBy('weeklyFeats')"
                  >
                    本周功绩
                    <span aria-hidden="true">{{ sortIndicator("weeklyFeats") }}</span>
                  </button>
                </th>
                <th scope="col" :aria-sort="sortAria('totalFeats')">
                  <button
                    class="guild-sort-button"
                    type="button"
                    @click="sortMembersBy('totalFeats')"
                  >
                    总功绩
                    <span aria-hidden="true">{{ sortIndicator("totalFeats") }}</span>
                  </button>
                </th>
                <th scope="col" :aria-sort="sortAria('historyDonate')">
                  <button
                    class="guild-sort-button"
                    type="button"
                    @click="sortMembersBy('historyDonate')"
                  >
                    累计捐献
                    <span aria-hidden="true">{{ sortIndicator("historyDonate") }}</span>
                  </button>
                </th>
                <th scope="col" :aria-sort="sortAria('offlineAt')">
                  <button
                    class="guild-sort-button"
                    type="button"
                    @click="sortMembersBy('offlineAt')"
                  >
                    最后在线
                    <span aria-hidden="true">{{ sortIndicator("offlineAt") }}</span>
                  </button>
                </th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="member in members"
                :key="member.id"
                :class="{
                  'guild-table__row--current': isCurrentCharacter(member.name),
                }"
              >
                <td class="guild-table__name">
                  <span>{{ member.name || "—" }}</span>
                  <small
                    v-if="isCurrentCharacter(member.name)"
                    class="guild-current-stamp"
                    >本大人</small
                  >
                </td>
                <td>
                  <span
                    class="guild-duty"
                    :class="`guild-duty--${dutyTone(member.dutyCode)}`"
                    >{{ member.dutyLabel }}</span
                  >
                </td>
                <td>{{ member.level }}</td>
                <td>{{ formatDate(member.joinedDate) }}</td>
                <td class="guild-table__num">{{ formatCount(member.weeklyFeats) }}</td>
                <td class="guild-table__num">{{ formatCount(member.totalFeats) }}</td>
                <td class="guild-table__num">
                  {{ formatCount(member.historyDonate) }}
                </td>
                <td>
                  <span v-if="member.offlineAt === 0" class="guild-online"
                    ><i aria-hidden="true"></i>在线中</span
                  >
                  <span v-else>{{ member.offlineAtLabel || "—" }}</span>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>
    </template>

    <section v-else-if="loaded" class="guild-empty" aria-label="暂无寮数据">
      <p class="guild-empty__mark" aria-hidden="true">寮</p>
      <h2>还没有寮数据</h2>
      <p>
        寮成员名册只存在于 Windows 桌面版的内存读取结果里。macOS 版不读取阴阳师客户端，藏宝阁公开商品也不包含寮数据。
      </p>
      <button class="guild-empty__action" type="button" @click="openDesktopReading">
        前往藏宝阁导入
      </button>
    </section>

    <footer class="guild-footnote">
      <span
        ><b>读取边界</b>
        只展示寮概况与成员功绩等聚合数据，不把任何成员信息发送到远程服务。</span
      >
      <span class="guild-footnote__stamp">LEDGER / G-01</span>
    </footer>
  </main>
</template>

<style scoped>
/* 寮管理页沿用名册视觉：朱红印章色与青绿强调，表格承担成员明细。 */
.guild-page {
  --guild-red: #a64b3f;
  --guild-gold: #b07a3e;
  --guild-indigo: #344b75;
  position: relative;
  max-width: 1480px;
  min-height: 100vh;
  padding: 38px clamp(24px, 4.3vw, 72px) 104px;
  background:
    radial-gradient(circle at 83% 3%, rgb(255 255 255 / 78%), transparent 28%),
    var(--fog);
}

.guild-header {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 28px;
}

.guild-header h1 {
  margin: 7px 0 8px;
}

.guild-header__meta {
  display: flex;
  align-items: center;
  gap: 12px;
  padding-bottom: 9px;
  color: var(--ink-soft);
  font-family: Consolas, monospace;
  font-size: 10px;
  letter-spacing: 0.07em;
}

.guild-readonly {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  color: var(--jade);
}

.guild-readonly i {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: currentColor;
  box-shadow: 0 0 0 4px rgb(82 121 111 / 14%);
}

.guild-current-name {
  max-width: 300px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.guild-load-error {
  margin: 0 0 14px;
  padding: 10px 14px;
  border: 1px solid rgb(200 80 70 / 45%);
  background: rgb(200 80 70 / 8%);
  color: var(--guild-red);
  font-size: 12px;
}

/* 寮概况条：等级印章 + 寮名 + 三项统计。 */
.guild-summary {
  display: flex;
  align-items: center;
  gap: 22px;
  margin-top: 32px;
  padding: 20px 24px;
  border: 1px solid var(--line);
  background: rgb(255 250 240 / 78%);
  box-shadow: 0 16px 40px rgb(73 55 39 / 7%);
}

.guild-summary__seal {
  display: grid;
  width: 62px;
  height: 62px;
  flex: 0 0 auto;
  place-items: center;
  border: 1px solid var(--guild-red);
  border-radius: 50%;
  color: var(--guild-red);
  font-family: STKaiti, KaiTi, serif;
  line-height: 1;
  box-shadow: inset 0 0 0 3px rgb(166 75 63 / 10%);
}

.guild-summary__seal span {
  font-size: 24px;
}

.guild-summary__seal small {
  margin-top: -12px;
  font-size: 10px;
}

.guild-summary__body h2 {
  margin: 0;
  font-family: STKaiti, KaiTi, serif;
  font-size: 24px;
  letter-spacing: -0.02em;
}

.guild-summary__sub {
  margin: 6px 0 0;
  color: var(--ink-soft);
  font-size: 12px;
}

.guild-summary__declare {
  max-width: 420px;
  margin: 5px 0 0;
  color: var(--ink-soft);
  font-family: STKaiti, KaiTi, serif;
  font-size: 12px;
}

.guild-summary__stats {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(118px, 1fr));
  gap: 1px;
  margin: 0 0 0 auto;
  flex: 1 1 520px;
  border: 1px solid var(--line-soft);
  background: var(--line-soft);
}

.guild-summary__stats div {
  padding: 10px 16px;
  background: rgb(255 255 255 / 58%);
}

.guild-summary__stats dt {
  color: var(--ink-soft);
  font-size: 10px;
  letter-spacing: 0.06em;
}

.guild-summary__stats dd {
  margin: 4px 0 0;
  font-family: Consolas, monospace;
  font-size: 15px;
}

.guild-summary__stats dd small {
  color: var(--ink-soft);
  font-size: 10px;
  font-weight: normal;
}

/* 玩法进度一行：寮突破 / 狭间 / 寮战赛季。 */
.guild-progress {
  display: flex;
  flex-wrap: wrap;
  gap: 8px 30px;
  margin: 12px 0 0;
  padding: 10px 18px;
  border: 1px dashed var(--line);
}

.guild-progress div {
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.guild-progress dt {
  color: var(--ink-soft);
  font-size: 10px;
  letter-spacing: 0.06em;
}

.guild-progress dd {
  margin: 0;
  color: var(--ink);
  font-family: Consolas, monospace;
  font-size: 12px;
}

/* 成员名册表格。 */
.guild-members {
  margin-top: 24px;
}

.guild-members__head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  margin-bottom: 10px;
}

.guild-members__head h3 {
  margin: 0;
  font-family: STKaiti, KaiTi, serif;
  font-size: 19px;
}

.guild-members__count {
  color: var(--guild-red);
  font-family: Consolas, monospace;
  font-size: 11px;
}

/* 筛选状态卡保持轻量，避免压过成员表。 */
.guild-members__overview {
  display: grid;
  grid-template-columns: 1fr;
  gap: 1px;
  margin-bottom: 10px;
  border: 1px solid var(--line-soft);
  background: var(--line-soft);
}

.guild-member-insight {
  display: grid;
  grid-template-columns: auto auto 1fr;
  align-items: baseline;
  gap: 10px;
  padding: 9px 13px;
  background: rgb(255 255 255 / 58%);
}

.guild-member-insight span,
.guild-member-insight small {
  color: var(--ink-soft);
  font-size: 10px;
}

.guild-member-insight strong {
  color: var(--ink);
  font-family: Consolas, monospace;
  font-size: 14px;
  font-weight: 600;
  white-space: nowrap;
}

.guild-member-insight small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 搜索与周贡输入放在表格上方，条件变化即时作用于本地快照。 */
.guild-members__toolbar {
  display: flex;
  align-items: flex-end;
  flex-wrap: wrap;
  gap: 10px;
  margin-bottom: 10px;
}

.guild-search-field,
.guild-weekly-filter {
  display: grid;
  gap: 5px;
  color: var(--ink-soft);
  font-size: 10px;
}

.guild-search-field {
  min-width: min(280px, 100%);
  flex: 1 1 280px;
}

.guild-weekly-filter {
  flex: 0 1 150px;
}

.guild-search-field input,
.guild-weekly-filter input {
  min-height: 31px;
  padding: 6px 9px;
  border: 1px solid var(--line);
  background: rgb(255 250 240 / 78%);
  color: var(--ink);
  font: inherit;
  outline: none;
}

.guild-search-field input:focus,
.guild-weekly-filter input:focus {
  border-color: var(--guild-red);
  box-shadow: 0 0 0 2px rgb(166 75 63 / 12%);
}

.guild-filter-clear {
  min-height: 31px;
  padding: 6px 11px;
  border: 1px solid var(--line);
  color: var(--ink-soft);
  background: transparent;
  cursor: pointer;
  font-size: 10px;
}

.guild-filter-clear:hover {
  border-color: var(--guild-red);
  color: var(--guild-red);
}

/* 导出按钮沿用筛选工具的紧凑尺寸，同时用朱红色标出主要动作。 */
.guild-export-button {
  min-height: 31px;
  padding: 6px 12px;
  border: 1px solid var(--guild-red);
  color: var(--guild-red);
  background: rgb(255 250 240 / 78%);
  cursor: pointer;
  font-size: 10px;
}

.guild-export-button:hover,
.guild-export-button:focus-visible {
  background: rgb(166 75 63 / 9%);
}

.guild-table-wrap {
  overflow-x: auto;
  border: 1px solid var(--line);
  background: rgb(255 250 240 / 78%);
}

.guild-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
}

.guild-table th {
  padding: 9px 14px;
  border-bottom: 1px solid var(--line);
  background: rgb(248 243 233 / 80%);
  color: var(--ink-soft);
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-align: left;
  white-space: nowrap;
}

.guild-sort-button {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 0;
  border: 0;
  color: inherit;
  background: transparent;
  cursor: pointer;
  font: inherit;
  letter-spacing: inherit;
  text-align: left;
}

.guild-sort-button:hover,
.guild-sort-button:focus-visible {
  color: var(--guild-red);
}

.guild-sort-button span {
  color: var(--guild-red);
  font-size: 12px;
  line-height: 1;
}

.guild-table td {
  padding: 10px 14px;
  border-bottom: 1px solid var(--line-soft);
  color: var(--ink);
  white-space: nowrap;
}

.guild-table tbody tr:last-child td {
  border-bottom: none;
}

.guild-table tbody tr:hover {
  background: rgb(166 75 63 / 4%);
}

/* 当前角色行使用轻微暖金底色，既能被注意到，又不喧宾夺主。 */
.guild-table tbody tr.guild-table__row--current {
  background: rgb(176 122 62 / 8%);
}

.guild-table tbody tr.guild-table__row--current:hover {
  background: rgb(176 122 62 / 12%);
}

.guild-table__name {
  font-family: STKaiti, KaiTi, serif;
  font-size: 14px;
}

/* 当前角色印章：使用小字号朱红边框，跟随成员名显示且不挤占其他列。 */
.guild-current-stamp {
  display: inline-block;
  margin-left: 7px;
  padding: 1px 4px;
  border: 1px solid var(--guild-red);
  color: var(--guild-red);
  font-family: inherit;
  font-size: 10px;
  font-style: normal;
  line-height: 1.2;
  letter-spacing: 0.04em;
  vertical-align: middle;
}

.guild-table__num {
  font-family: Consolas, monospace;
}

/* 在线徽标：离线时刻为 0 表示该成员此刻在线。 */
.guild-online {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--jade);
}

.guild-online i {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
  box-shadow: 0 0 0 3px rgb(82 121 111 / 14%);
}

/* 职务徽章：会长朱红、副会长赭黄、普通成员灰。 */
.guild-duty {
  display: inline-block;
  padding: 2px 8px;
  border: 1px solid currentColor;
  font-size: 10px;
  letter-spacing: 0.05em;
}

.guild-duty--leader {
  color: var(--guild-red);
  background: rgb(166 75 63 / 6%);
}

.guild-duty--vice {
  color: var(--guild-gold);
  background: rgb(176 122 62 / 7%);
}

.guild-duty--member {
  color: var(--ink-soft);
}

/* 空态与页脚。 */
.guild-empty {
  max-width: 560px;
  margin: 40px auto 0;
  padding: 44px 34px 38px;
  border: 1px dashed var(--line);
  background: rgb(255 250 240 / 55%);
  text-align: center;
}

.guild-empty__mark {
  margin: 0 0 10px;
  color: var(--guild-red);
  font-family: STKaiti, KaiTi, serif;
  font-size: 44px;
  line-height: 1;
}

.guild-empty h2 {
  margin: 0;
  font-family: STKaiti, KaiTi, serif;
  font-size: 22px;
}

.guild-empty p:not(.guild-empty__mark) {
  margin: 12px auto 0;
  color: var(--ink-soft);
  font-size: 12px;
  line-height: 1.8;
}

.guild-empty__action {
  margin-top: 20px;
  padding: 9px 18px;
  border: 1px solid var(--guild-red);
  color: var(--guild-red);
  background: rgb(255 250 240 / 70%);
  cursor: pointer;
  font-size: 12px;
  transition: background 0.15s ease;
}

.guild-empty__action:hover {
  background: rgb(166 75 63 / 9%);
}

.guild-footnote {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
  margin-top: 26px;
  padding-top: 13px;
  border-top: 1px dashed var(--line);
  color: var(--ink-soft);
  font-size: 11px;
  line-height: 1.5;
}

.guild-footnote b {
  color: var(--ink);
}

.guild-footnote__stamp {
  flex: 0 0 auto;
  color: var(--guild-red);
  font-family: Consolas, monospace;
  font-size: 10px;
  letter-spacing: 0.1em;
}

@media (max-width: 720px) {
  .guild-page {
    padding: 27px 18px 96px;
  }

  .guild-header {
    align-items: flex-start;
    flex-direction: column;
  }

  .guild-header__meta {
    padding-bottom: 0;
  }

  .guild-summary {
    align-items: flex-start;
    flex-wrap: wrap;
  }

  .guild-summary__stats {
    width: 100%;
    grid-template-columns: repeat(auto-fit, minmax(96px, 1fr));
    flex: 1 1 100%;
    margin: 4px 0 0;
  }

  .guild-summary__stats div {
    padding: 8px 12px;
  }

  .guild-members__overview {
    grid-template-columns: 1fr;
  }

  .guild-member-insight {
    grid-template-columns: auto auto;
  }

  .guild-member-insight small {
    grid-column: 1 / -1;
  }

  .guild-footnote {
    align-items: flex-start;
    flex-direction: column;
    gap: 8px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .guild-empty__action {
    transition: none;
  }
}
</style>
