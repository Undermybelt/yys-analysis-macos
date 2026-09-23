<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import type { AnalysisCenter, GameProfile } from "../api/contracts";
import { normalizeCommandError } from "../api/errors";
import {
  activeProfileIfListed,
  onActiveCharacterChanged,
} from "../state/activeCharacter";

const profiles = ref<GameProfile[]>([]);
const selectedProfileId = ref<string | null>(null);
const center = ref<AnalysisCenter | null>(null);
const error = ref("");
const loading = ref(false);
const exporting = ref(false);

const selectedProfile = computed(
  () =>
    profiles.value.find((profile) => profile.id === selectedProfileId.value) ?? null,
);
const maxAttributeCount = computed(() =>
  Math.max(
    ...(center.value?.attributeDistribution.map((item) => item.count) ?? [1]),
    1,
  ),
);

/** 统一显示后端稳定错误，页面不依赖 Rust 内部错误文本做分支。 */
async function run<T>(operation: () => Promise<T>): Promise<T | null> {
  try {
    return await operation();
  } catch (caught) {
    const commandError = normalizeCommandError(caught);
    error.value = `${commandError.code} · ${commandError.message}`;
    return null;
  }
}

/** 读取档案并加载同一档案的分析中心指标；首次进入跟随当前激活角色。 */
async function loadPage(): Promise<void> {
  const nextProfiles = await run(() => desktopApi.listProfiles());
  if (!nextProfiles) return;
  profiles.value = nextProfiles;
  const following = activeProfileIfListed(profiles.value);
  if (following) selectedProfileId.value = following;
  selectedProfileId.value ??= profiles.value[0]?.id ?? null;
  await loadCenter();
}

async function loadCenter(): Promise<void> {
  if (!selectedProfileId.value) return;
  loading.value = true;
  error.value = "";
  center.value = await run(() =>
    desktopApi.getAnalysisCenter(selectedProfileId.value!),
  );
  loading.value = false;
}

/** 图表点击只传递后端提供的分类，待办页会用同一筛选条件再次查询。 */
function drillDown(category: string | null, search?: string, useId?: string): void {
  if (!category && !search && !useId) return;
  window.dispatchEvent(
    new CustomEvent("open-analysis-filter", {
      detail: { category, search, useId },
    }),
  );
}

function stageLabel(stage: string): string {
  return (
    {
      admitted: "准入",
      observe: "观察节点",
      finished: "满级成品",
    }[stage] ?? stage
  );
}

function attributeLabel(attribute: string): string {
  return attribute.replaceAll("_", " ");
}

/** 让用户下载后端按档案隔离生成的 JSON，不在浏览器端拼接其他档案数据。 */
async function exportProfile(): Promise<void> {
  if (!selectedProfileId.value) return;
  exporting.value = true;
  const payload = await run(() =>
    desktopApi.exportData({ profileId: selectedProfileId.value!, kind: "profile" }),
  );
  if (payload) {
    const blob = new Blob([payload.content], { type: payload.mediaType });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = payload.fileName;
    anchor.click();
    URL.revokeObjectURL(url);
  }
  exporting.value = false;
}

onMounted(() => {
  void loadPage();
  stopActiveCharacterChanged = onActiveCharacterChanged((profileId) => {
    if (!profileId || !profiles.value.some((profile) => profile.id === profileId))
      return;
    selectedProfileId.value = profileId;
    void loadCenter();
  });
});

let stopActiveCharacterChanged: (() => void) | null = null;
onBeforeUnmount(() => {
  stopActiveCharacterChanged?.();
  stopActiveCharacterChanged = null;
});
</script>

<template>
  <main class="workspace">
    <header class="page-header">
      <div>
        <p class="eyebrow">ANALYSIS / 08</p>
        <h1>分析中心</h1>
        <p class="lede">看清库存能做什么、缺什么，以及哪些结论仍需要更完整的数据。</p>
      </div>
      <div class="header-actions">
        <label class="compact-field">
          <span>游戏档案</span>
          <select v-model="selectedProfileId" @change="loadCenter">
            <option v-for="profile in profiles" :key="profile.id" :value="profile.id">
              {{ profile.displayName }}
            </option>
          </select>
        </label>
        <button
          class="button button--quiet"
          type="button"
          :disabled="exporting"
          @click="exportProfile"
        >
          {{ exporting ? "准备导出…" : "导出档案" }}
        </button>
      </div>
    </header>

    <section v-if="error" class="error-panel" role="alert">
      <div>
        <p class="error-panel__title">分析中心暂时不可用</p>
        <p>{{ error }}</p>
      </div>
      <button class="button button--quiet" type="button" @click="loadCenter">
        重试
      </button>
    </section>

    <section v-else-if="!selectedProfile" class="empty-state">
      <p class="section-kicker">尚未创建档案</p>
      <h2>先创建一个游戏档案</h2>
      <p>不同档案的库存、快照和分析结果始终隔离。</p>
    </section>

    <template v-else-if="center">
      <section class="analysis-summary-grid" aria-label="库存摘要">
        <article class="analysis-metric-card analysis-metric-card--accent">
          <span>当前库存</span>
          <strong>{{ center.inventoryCount }}</strong>
          <small
            >在场 {{ center.presentCount }} · 未确认
            {{ center.unconfirmedCount }}</small
          >
        </article>
        <article class="analysis-metric-card">
          <span>用途缺口</span>
          <strong>{{
            center.usageCoverage.filter((item) => item.usableCount === 0).length
          }}</strong>
          <small>按启用用途方案统计</small>
        </article>
        <article class="analysis-metric-card">
          <span>清理候选</span>
          <strong>{{ center.cleanupYield.candidateCount }}</strong>
          <small
            >待处理 {{ center.cleanupYield.pendingCount }} · 未确认
            {{ center.cleanupYield.unconfirmedCount }}</small
          >
        </article>
        <article class="analysis-metric-card">
          <span>数据质量</span>
          <strong>{{
            center.dataQuality.currentBaselineComplete ? "完整" : "局部"
          }}</strong>
          <small
            >未知强化 {{ center.dataQuality.unknownEnhancementCount }} · 过期
            {{ center.dataQuality.staleAnalysisCount }}</small
          >
        </article>
      </section>

      <section class="analysis-panel-grid">
        <article class="surface-card">
          <div class="card-heading">
            <div>
              <p class="section-kicker">用途覆盖</p>
              <h2>库存能做什么</h2>
            </div>
            <span class="status-chip">同一档案</span>
          </div>
          <div v-if="center.usageCoverage.length" class="analysis-list">
            <button
              v-for="item in center.usageCoverage"
              :key="item.useId"
              class="analysis-list-row analysis-list-row--button"
              type="button"
              @click="
                drillDown(item.drillDownCategory, item.drillDownSearch, item.useId)
              "
            >
              <span
                ><strong>{{ item.title }}</strong
                ><small>{{ item.useId }}</small></span
              >
              <span class="analysis-list-row__numbers">
                <b>{{ item.usableCount }}</b> 可用 <b>{{ item.observingCount }}</b> 观察
                <b class="text-muted">{{ item.gapCount }}</b> 缺口
              </span>
            </button>
          </div>
          <p v-else class="empty-note">尚未生成分析待办，请先导入库存并刷新分析。</p>
        </article>

        <article class="surface-card">
          <div class="card-heading">
            <div>
              <p class="section-kicker">强化漏斗</p>
              <h2>从准入到成品</h2>
            </div>
          </div>
          <div class="funnel-list">
            <button
              v-for="stage in center.enhancementFunnel"
              :key="stage.stage"
              class="funnel-row funnel-row--button"
              type="button"
              @click="drillDown(stage.drillDownCategory)"
            >
              <span>{{ stageLabel(stage.stage) }}</span>
              <strong>{{ stage.count }}</strong>
            </button>
          </div>
          <p class="panel-note">
            强化漏斗只表达当前建议数量，不代表成功概率或游戏内已执行。
          </p>
        </article>

        <article class="surface-card">
          <div class="card-heading">
            <div>
              <p class="section-kicker">属性分布</p>
              <h2>副属性结构</h2>
            </div>
          </div>
          <div v-if="center.attributeDistribution.length" class="bar-list">
            <div
              v-for="item in center.attributeDistribution"
              :key="item.attributeType"
              class="bar-row"
            >
              <span>{{ attributeLabel(item.attributeType) }}</span>
              <div class="bar-track">
                <i :style="{ width: `${(item.count / maxAttributeCount) * 100}%` }"></i>
              </div>
              <strong>{{ item.count }}</strong>
            </div>
          </div>
          <p v-else class="empty-note">暂无副属性分布。</p>
        </article>

        <article class="surface-card">
          <div class="card-heading">
            <div>
              <p class="section-kicker">数据质量</p>
              <h2>结论可信度</h2>
            </div>
          </div>
          <dl class="quality-list">
            <div>
              <dt>当前基线</dt>
              <dd>
                {{
                  center.dataQuality.currentBaselineComplete
                    ? "完整，可确认移除"
                    : "局部，不能确认移除"
                }}
              </dd>
            </div>
            <div>
              <dt>未知强化次数</dt>
              <dd>{{ center.dataQuality.unknownEnhancementCount }}</dd>
            </div>
            <div>
              <dt>草案规则命中</dt>
              <dd>{{ center.dataQuality.draftRuleCount }}</dd>
            </div>
            <div>
              <dt>过期分析</dt>
              <dd>{{ center.dataQuality.staleAnalysisCount }}</dd>
            </div>
            <div>
              <dt>受影响快照</dt>
              <dd>{{ center.dataQuality.affectedSnapshotCount }}</dd>
            </div>
          </dl>
        </article>
      </section>

      <section class="surface-card">
        <div class="card-heading">
          <div>
            <p class="section-kicker">套装结构</p>
            <h2>PVE / PVP 常用度</h2>
          </div>
          <span class="status-chip">{{ center.setStructure.length }} 套</span>
        </div>
        <div v-if="center.setStructure.length" class="set-table-wrap">
          <table class="data-table set-table">
            <thead>
              <tr>
                <th>套装</th>
                <th>库存</th>
                <th>PVE</th>
                <th>PVP</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="item in center.setStructure" :key="item.setId">
                <td>{{ item.setId }}</td>
                <td>{{ item.count }}</td>
                <td>
                  <span
                    class="status-chip"
                    :class="item.pveCommon ? 'status-chip--safe' : ''"
                    >{{ item.pveCommon ? "常用" : "少用" }}</span
                  >
                </td>
                <td>
                  <span
                    class="status-chip"
                    :class="item.pvpCommon ? 'status-chip--safe' : ''"
                    >{{ item.pvpCommon ? "常用" : "少用" }}</span
                  >
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <p v-else class="empty-note">暂无套装结构。</p>
      </section>
    </template>

    <p v-else class="loading-state">
      {{ loading ? "正在读取分析指标…" : "暂无分析指标" }}
    </p>
  </main>
</template>
