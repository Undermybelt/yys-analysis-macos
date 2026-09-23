<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { desktopApi } from "../api/client";
import type {
  ActionBatch,
  AnalysisTodo,
  BatchItem,
  DecisionApplyResult,
  DecisionPreview,
  GameProfile,
} from "../api/contracts";
import { normalizeCommandError } from "../api/errors";
import {
  activeProfileIfListed,
  onActiveCharacterChanged,
} from "../state/activeCharacter";

const props = defineProps<{
  /** 分析中心下钻传入的后端稳定分类；为空时展示全部待办。 */
  initialCategory?: string;
  /** 用途覆盖下钻传入用途 ID，服务端在解释树中做精确搜索。 */
  initialSearch?: string;
  /** 用途覆盖下钻传入精确用途 ID，不与用户可见的全文搜索混用。 */
  initialUseId?: string;
}>();

/** 待办页固定的业务分类；顺序与产品的行动优先级一致。 */
const categories = [
  { value: "", label: "全部待办" },
  { value: "new_embryo", label: "新胚子" },
  { value: "continue", label: "继续强化" },
  { value: "await_review", label: "节点待复评" },
  { value: "stop", label: "建议停止" },
  { value: "cleanup", label: "可清理" },
  { value: "conflict", label: "用途冲突" },
  { value: "uncertain", label: "数据待确认" },
  { value: "changed", label: "新增/变化" },
];

const profiles = ref<GameProfile[]>([]);
const selectedProfileId = ref<string | null>(null);
const todos = ref<AnalysisTodo[]>([]);
const total = ref(0);
const batches = ref<ActionBatch[]>([]);
const activeBatchId = ref<string | null>(null);
const activeBatchItems = ref<BatchItem[]>([]);
const selectedKeys = ref<string[]>([]);
const selectedTodo = ref<AnalysisTodo | null>(null);
const category = ref(props.initialCategory ?? "");
const search = ref(props.initialSearch ?? "");
const useId = ref(props.initialUseId ?? "");
const batchDecision = ref("plan_strengthen");
const targetLevel = ref(3);
const preview = ref<DecisionPreview | null>(null);
const lastApply = ref<DecisionApplyResult | null>(null);
const error = ref("");
const loading = ref(false);
const busy = ref(false);

const selectedCount = computed(() => selectedKeys.value.length);
const selectedAll = computed(
  () => todos.value.length > 0 && selectedCount.value === todos.value.length,
);
const selectedProfile = computed(
  () =>
    profiles.value.find((profile) => profile.id === selectedProfileId.value) ?? null,
);

function recommendationLabel(value: AnalysisTodo["recommendation"]): string {
  return {
    keep: "保留",
    observe: "继续观察",
    stop: "停止投入",
    recycle: "可清理",
  }[value];
}

function categoryLabel(value: string): string {
  return categories.find((item) => item.value === value)?.label ?? value;
}

function dataQualityLabel(value: string): string {
  return value === "complete"
    ? "数据完整"
    : value === "partial"
      ? "局部快照"
      : "强化次数未知";
}

function detailUses(todo: AnalysisTodo): Array<Record<string, unknown>> {
  const uses = todo.detail?.uses;
  return Array.isArray(uses) ? (uses as Array<Record<string, unknown>>) : [];
}

async function run<T>(operation: () => Promise<T>): Promise<T | null> {
  try {
    return await operation();
  } catch (caught) {
    const commandError = normalizeCommandError(caught);
    error.value = `${commandError.code} · ${commandError.message}`;
    return null;
  }
}

/** 读取当前档案的服务端分页待办和已缓存批次。 */
async function loadTodos(): Promise<void> {
  if (!selectedProfileId.value) return;
  loading.value = true;
  error.value = "";
  const page = await run(() =>
    desktopApi.listAnalysisTodos({
      profileId: selectedProfileId.value!,
      category: category.value || undefined,
      search: search.value.trim() || undefined,
      useId: useId.value || undefined,
      limit: 100,
      offset: 0,
    }),
  );
  if (page) {
    todos.value = page.items;
    total.value = page.total;
    selectedKeys.value = selectedKeys.value.filter((key) =>
      page.items.some((todo) => todo.soulKey === key),
    );
  }
  const nextBatches = await run(() =>
    desktopApi.listActionBatches(selectedProfileId.value!),
  );
  if (nextBatches) batches.value = nextBatches;
  loading.value = false;
}

/** 首次打开时若已有库存但尚未生成派生结果，补一次分析后进入待办；跟随当前激活角色。 */
async function loadPage(): Promise<void> {
  const nextProfiles = await run(() => desktopApi.listProfiles());
  if (!nextProfiles) return;
  profiles.value = nextProfiles;
  const following = activeProfileIfListed(profiles.value);
  if (following) selectedProfileId.value = following;
  selectedProfileId.value ??= profiles.value[0]?.id ?? null;
  category.value = props.initialCategory ?? "";
  search.value = props.initialSearch ?? "";
  useId.value = props.initialUseId ?? "";
  if (!selectedProfileId.value) return;
  const current = await run(() =>
    desktopApi.listAnalysisTodos({ profileId: selectedProfileId.value!, limit: 1 }),
  );
  if (current?.total === 0) {
    await run(() =>
      desktopApi.recalculateAnalysis({ profileId: selectedProfileId.value! }),
    );
  }
  await loadTodos();
}

/** 图表下钻切换回待办时，只更新筛选条件并复用原有服务端分页查询。 */
watch(
  () => [props.initialCategory, props.initialSearch, props.initialUseId],
  ([nextCategory, nextSearch, nextUseId]) => {
    category.value = nextCategory ?? "";
    search.value = nextSearch ?? "";
    useId.value = nextUseId ?? "";
    if (selectedProfileId.value) void loadTodos();
  },
);

function toggleAll(): void {
  selectedKeys.value = selectedAll.value ? [] : todos.value.map((todo) => todo.soulKey);
}

function toggleSelected(todo: AnalysisTodo): void {
  selectedKeys.value = selectedKeys.value.includes(todo.soulKey)
    ? selectedKeys.value.filter((key) => key !== todo.soulKey)
    : [...selectedKeys.value, todo.soulKey];
}

/** 先请求服务端保护/覆盖统计，用户确认后才写入决定。 */
async function previewDecisions(): Promise<void> {
  if (!selectedProfileId.value || selectedCount.value === 0) return;
  const result = await run(() =>
    desktopApi.previewDecisions({
      profileId: selectedProfileId.value!,
      soulKeys: selectedKeys.value,
      decision: batchDecision.value,
      respectProtection: true,
    }),
  );
  if (result) preview.value = result;
}

async function applyDecisions(): Promise<void> {
  if (!selectedProfileId.value || !preview.value) return;
  busy.value = true;
  const result = await run(() =>
    desktopApi.applyDecisions({
      profileId: selectedProfileId.value!,
      changes: selectedKeys.value.map((soulKey) => ({
        soulKey,
        decision: batchDecision.value,
        note: "分析待办批量决定",
      })),
      respectProtection: true,
    }),
  );
  if (result) {
    lastApply.value = result;
    preview.value = null;
    selectedKeys.value = [];
    await loadTodos();
  }
  busy.value = false;
}

/** 只撤销最近一次已确认的决定事务，之后的规则重算和新决定保持不变。 */
async function undoLastDecision(): Promise<void> {
  if (!lastApply.value) return;
  busy.value = true;
  const result = await run(() => desktopApi.undoDecision(lastApply.value!.operationId));
  if (result === null) {
    busy.value = false;
    return;
  }
  lastApply.value = null;
  await loadTodos();
  busy.value = false;
}

/** 单枚决定沿用批量事务协议，保证后续撤销入口一致。 */
async function setDecision(todo: AnalysisTodo, decision: string | null): Promise<void> {
  if (!selectedProfileId.value) return;
  busy.value = true;
  const result = await run(() =>
    desktopApi.applyDecisions({
      profileId: selectedProfileId.value!,
      changes: [{ soulKey: todo.soulKey, decision, note: "详情抽屉决定" }],
      respectProtection: false,
    }),
  );
  if (result) await loadTodos();
  busy.value = false;
}

async function createBatch(kind: "strengthen" | "cleanup"): Promise<void> {
  if (!selectedProfileId.value || selectedCount.value === 0) return;
  busy.value = true;
  const batch = await run(() =>
    desktopApi.createActionBatch({
      profileId: selectedProfileId.value!,
      kind,
      targetLevel: kind === "strengthen" ? targetLevel.value : undefined,
      soulKeys: selectedKeys.value,
    }),
  );
  if (batch) {
    batches.value = [batch, ...batches.value];
    selectedKeys.value = [];
  }
  busy.value = false;
}

async function updateBatchItem(
  batch: ActionBatch,
  soulKey: string,
  status: "completed" | "skipped" | "not_found",
): Promise<void> {
  if (!selectedProfileId.value) return;
  const detail = await run(() =>
    desktopApi.updateActionBatchItem({
      profileId: selectedProfileId.value!,
      batchId: batch.id,
      soulKey,
      status,
    }),
  );
  if (detail) {
    batches.value = batches.value.map((item) =>
      item.id === detail.batch.id ? detail.batch : item,
    );
  }
}

/** 展开一个批次的逐枚清单，用户可以记录完成、跳过或找不到。 */
async function openBatch(batch: ActionBatch): Promise<void> {
  if (!selectedProfileId.value) return;
  const detail = await run(() =>
    desktopApi.getActionBatch(selectedProfileId.value!, batch.id),
  );
  if (detail) {
    activeBatchId.value = batch.id;
    activeBatchItems.value = detail.items;
  }
}

onMounted(() => {
  void loadPage();
  stopActiveCharacterChanged = onActiveCharacterChanged((profileId) => {
    if (!profileId || !profiles.value.some((profile) => profile.id === profileId))
      return;
    selectedProfileId.value = profileId;
    void loadPage();
  });
});

let stopActiveCharacterChanged: (() => void) | null = null;
onBeforeUnmount(() => {
  stopActiveCharacterChanged?.();
  stopActiveCharacterChanged = null;
});
</script>

<template>
  <!-- eslint-disable vue/html-indent, vue/html-closing-bracket-newline -->
  <main class="workspace">
    <header class="page-header">
      <div>
        <p class="eyebrow">ACTION / 05</p>
        <h1>分析待办</h1>
        <p class="lede">把规则结论整理成下一步可执行的强化、复评和清理动作。</p>
      </div>
      <div class="health-badge" :class="{ 'health-badge--error': error }">
        <span class="health-badge__pulse" aria-hidden="true"></span>
        <span>{{
          error ? "待办需要处理" : (selectedProfile?.displayName ?? "未选择档案")
        }}</span>
      </div>
    </header>

    <section v-if="error" class="error-panel" role="alert">
      <div>
        <p class="error-panel__title">待办操作失败</p>
        <p>{{ error }}</p>
      </div>
      <button class="button button--quiet" type="button" @click="loadPage">
        重新加载
      </button>
    </section>

    <section v-if="!profiles.length && !loading" class="empty-action-card">
      <p class="section-kicker">还没有当前数据</p>
      <h2>先读取一份御魂数据</h2>
      <p>分析待办只读取本地当前数据，不会连接或修改游戏。</p>
    </section>

    <template v-else>
      <section class="inbox-toolbar">
        <label>
          游戏档案
          <select v-model="selectedProfileId" @change="loadTodos">
            <option v-for="profile in profiles" :key="profile.id" :value="profile.id">
              {{ profile.displayName }}
            </option>
          </select>
        </label>
        <label>
          队列
          <select v-model="category" @change="loadTodos">
            <option v-for="item in categories" :key="item.value" :value="item.value">
              {{ item.label }}
            </option>
          </select>
        </label>
        <label class="inbox-search">
          搜索
          <input
            v-model="search"
            class="text-input"
            placeholder="套装、主属性或御魂键"
            @keyup.enter="loadTodos"
          />
        </label>
        <button
          class="button button--quiet"
          type="button"
          :disabled="loading"
          @click="loadTodos"
        >
          刷新分析
        </button>
      </section>

      <section class="inbox-summary">
        <span
          ><strong>{{ total }}</strong> 条待办</span
        >
        <span
          ><strong>{{ selectedCount }}</strong> 条已选</span
        >
        <span v-if="lastApply"
          >本次决定：新增 {{ lastApply.addedCount }}，覆盖
          {{ lastApply.overwrittenCount }}，保护 {{ lastApply.protectedCount }}</span
        >
      </section>

      <section class="inbox-layout">
        <article class="runtime-card inbox-list-card">
          <div class="card-heading">
            <div>
              <p class="section-kicker">分类队列</p>
              <h2>下一步动作</h2>
            </div>
            <label class="select-all-label">
              <input type="checkbox" :checked="selectedAll" @change="toggleAll" />
              全选本页
            </label>
          </div>

          <div v-if="loading" class="placeholder-card">正在读取本地待办…</div>
          <div v-else-if="!todos.length" class="empty-note">当前筛选没有待办。</div>
          <div v-else class="todo-list">
            <button
              v-for="todo in todos"
              :key="todo.id"
              class="todo-row"
              :class="{ 'todo-row--selected': selectedKeys.includes(todo.soulKey) }"
              type="button"
              @click="selectedTodo = todo"
            >
              <input
                type="checkbox"
                :checked="selectedKeys.includes(todo.soulKey)"
                :aria-label="`选择 ${todo.soulKey}`"
                @click.stop
                @change="toggleSelected(todo)"
              />
              <span class="todo-row__main">
                <strong
                  >{{ todo.setId }} · {{ todo.slot }}号 · +{{ todo.level }}</strong
                >
                <small>{{ todo.mainAttribute }} · {{ todo.reasonSummary }}</small>
              </span>
              <span class="todo-row__badges">
                <em
                  :class="`recommendation-chip recommendation-chip--${todo.recommendation}`"
                >
                  {{ recommendationLabel(todo.recommendation) }}
                </em>
                <em v-if="todo.isNew" class="status-chip status-chip--safe">新增</em>
                <em v-else-if="todo.isChanged" class="status-chip">变化</em>
              </span>
            </button>
          </div>
        </article>

        <aside class="runtime-card action-card">
          <div class="card-heading">
            <div>
              <p class="section-kicker">批量动作</p>
              <h2>一次预览再确认</h2>
            </div>
          </div>
          <label>
            我的决定
            <select v-model="batchDecision">
              <option value="plan_strengthen">计划强化</option>
              <option value="keep">锁定保留</option>
              <option value="observe">锁定观察</option>
              <option value="plan_recycle">准备清理</option>
              <option value="ignore">忽略建议</option>
            </select>
          </label>
          <label v-if="batchDecision === 'plan_strengthen'">
            强化目标节点
            <select v-model.number="targetLevel">
              <option :value="3">+3</option>
              <option :value="6">+6</option>
              <option :value="9">+9</option>
              <option :value="12">+12</option>
              <option :value="15">+15</option>
            </select>
          </label>
          <div class="action-stack">
            <button
              class="button button--primary"
              type="button"
              :disabled="busy || !selectedCount"
              @click="previewDecisions"
            >
              预览 {{ selectedCount }} 条决定
            </button>
            <button
              class="button button--quiet"
              type="button"
              :disabled="busy || !selectedCount"
              @click="createBatch('strengthen')"
            >
              创建强化批次
            </button>
            <button
              class="button button--quiet"
              type="button"
              :disabled="busy || !selectedCount"
              @click="createBatch('cleanup')"
            >
              创建清理批次
            </button>
          </div>
          <div v-if="preview" class="preview-panel">
            <strong>批量预览</strong>
            <p>
              新增 {{ preview.addedCount }} · 覆盖 {{ preview.overwrittenCount }} · 跳过
              {{ preview.skippedCount }}
            </p>
            <p>保护 {{ preview.protectedCount }} · 冲突 {{ preview.conflictCount }}</p>
            <button
              class="button button--primary"
              type="button"
              :disabled="busy"
              @click="applyDecisions"
            >
              确认写入并可撤销
            </button>
          </div>
          <button
            v-if="lastApply"
            class="button button--quiet"
            type="button"
            :disabled="busy"
            @click="undoLastDecision"
          >
            撤销上次决定
          </button>
        </aside>
      </section>

      <section v-if="batches.length" class="runtime-card batch-card">
        <div class="card-heading">
          <div>
            <p class="section-kicker">行动批次</p>
            <h2>游戏内执行进度</h2>
          </div>
          <span class="status-chip">{{ batches.length }} 个批次</span>
        </div>
        <div v-for="batch in batches" :key="batch.id" class="batch-row">
          <div>
            <strong
              >{{ batch.kind === "strengthen" ? "强化" : "清理" }}批次 ·
              {{ batch.itemCount }} 枚</strong
            >
            <p>
              {{ batch.groupCount }} 组筛选 · 已完成 {{ batch.completedCount }} · 跳过
              {{ batch.skippedCount }} · 找不到 {{ batch.notFoundCount }}
            </p>
          </div>
          <div class="batch-groups">
            <span v-for="group in batch.groups" :key="group.groupKey">
              {{ group.setId }} · {{ group.slot }}号 · +{{ group.level }}（{{
                group.itemCount
              }}）
            </span>
          </div>
          <div v-if="activeBatchId === batch.id" class="batch-item-list">
            <div
              v-for="item in activeBatchItems"
              :key="item.soulKey"
              class="batch-item-row"
            >
              <span>{{ item.soulKey }} · {{ item.status }}</span>
              <span class="batch-item-actions">
                <button
                  class="button button--quiet"
                  type="button"
                  @click="updateBatchItem(batch, item.soulKey, 'completed')"
                >
                  完成
                </button>
                <button
                  class="button button--quiet"
                  type="button"
                  @click="updateBatchItem(batch, item.soulKey, 'skipped')"
                >
                  跳过
                </button>
                <button
                  class="button button--quiet"
                  type="button"
                  @click="updateBatchItem(batch, item.soulKey, 'not_found')"
                >
                  找不到
                </button>
              </span>
            </div>
          </div>
          <button
            class="button button--quiet batch-open-button"
            type="button"
            @click="openBatch(batch)"
          >
            {{ activeBatchId === batch.id ? "刷新逐枚状态" : "打开执行清单" }}
          </button>
        </div>
        <p class="batch-note">
          批次状态只记录你在游戏内的执行声明；御魂是否消失仍需后续完整快照确认。
        </p>
      </section>
    </template>

    <div v-if="selectedTodo" class="drawer-backdrop" @click="selectedTodo = null">
      <aside class="detail-drawer" aria-label="御魂待办详情" @click.stop>
        <div class="card-heading">
          <div>
            <p class="section-kicker">待办详情</p>
            <h2>
              {{ selectedTodo.setId }} · {{ selectedTodo.slot }}号 · +{{
                selectedTodo.level
              }}
            </h2>
          </div>
          <button
            class="button button--quiet"
            type="button"
            @click="selectedTodo = null"
          >
            关闭
          </button>
        </div>
        <div class="detail-conclusion">
          <span
            :class="`recommendation-chip recommendation-chip--${selectedTodo.recommendation}`"
          >
            {{ recommendationLabel(selectedTodo.recommendation) }}
          </span>
          <span>{{ categoryLabel(selectedTodo.category) }}</span>
          <span>{{ dataQualityLabel(selectedTodo.dataQuality) }}</span>
        </div>
        <p class="detail-reason">{{ selectedTodo.reasonSummary }}</p>
        <div class="detail-actions">
          <button
            class="button button--quiet"
            type="button"
            :disabled="busy"
            @click="setDecision(selectedTodo, 'keep')"
          >
            我的决定：保留
          </button>
          <button
            class="button button--quiet"
            type="button"
            :disabled="busy"
            @click="setDecision(selectedTodo, 'observe')"
          >
            我的决定：观察
          </button>
          <button
            class="button button--quiet"
            type="button"
            :disabled="busy"
            @click="setDecision(selectedTodo, 'plan_recycle')"
          >
            我的决定：清理
          </button>
          <button
            class="button button--quiet"
            type="button"
            :disabled="busy || !selectedTodo.userDecision"
            @click="setDecision(selectedTodo, null)"
          >
            清除我的决定
          </button>
        </div>
        <p class="section-kicker detail-section-title">逐用途结果</p>
        <div v-if="detailUses(selectedTodo).length" class="use-result-list">
          <div
            v-for="use in detailUses(selectedTodo)"
            :key="String(use.useId)"
            class="use-result-row"
          >
            <strong>{{ use.title }}</strong>
            <span>{{ use.recommendation }} · {{ use.score }} 分</span>
            <small
              >有效成长 {{ use.effectiveGrowthCount ?? "未知" }} · 歪点
              {{ use.crookedPoints ?? "未知" }} · 剩余上限
              {{ use.remainingUpperBound ?? "未知" }}</small
            >
          </div>
        </div>
        <p v-else class="empty-note">暂无可展示的用途结果。</p>
        <p class="section-kicker detail-section-title">数据说明</p>
        <p class="detail-meta">
          规则证据：{{ selectedTodo.evidenceLevel }} · 解释哈希：{{
            selectedTodo.explanationHash
          }}
        </p>
      </aside>
    </div>

    <footer class="page-footer">
      <span>系统建议 ≠ 我的决定 ≠ 游戏内状态</span>
      <span aria-hidden="true">·</span>
      <span>所有批次只写入本地记录</span>
    </footer>
  </main>
  <!-- eslint-enable vue/html-indent, vue/html-closing-bracket-newline -->
</template>
