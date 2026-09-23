<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { taskScheduler, type ScheduledTask } from "../tasks/taskScheduler";

const open = ref(false);
const tasks = ref<ScheduledTask[]>([]);
let stopListening: (() => void) | null = null;

/** 任务中心只展示后台任务；前台按钮的进度仍留在对应页面，避免入口职责混淆。 */
const backgroundTasks = computed(() =>
  tasks.value.filter(
    (task) => task.kind.startsWith("backend:") || task.metadata.mode === "background",
  ),
);
const runningTasks = computed(() =>
  backgroundTasks.value.filter((task) => task.status === "running"),
);
const visibleTasks = computed(() => backgroundTasks.value.slice(0, 8));

/** 把无总数任务显示为阶段提示；有总数时统一限制在 0—100%。 */
function progressPercent(task: ScheduledTask): number | null {
  if (!task.total) return null;
  return Math.min(100, Math.max(0, Math.round((task.completed / task.total) * 100)));
}

/** 有总数任务显示为阶段进度；无总数任务只显示阶段提示。 */
function taskCountLabel(task: ScheduledTask): string {
  if (!task.total) return "";
  return `${task.completed} / ${task.total}`;
}

/** 任务中心只把取消动作交给调度器，具体 Worker 或 Rust 命令由任务自己注册。 */
function cancelTask(task: ScheduledTask): void {
  taskScheduler.cancel(task.id);
}

/** 格式化任务日志时间，保留时分秒便于排查长任务卡在哪个阶段。 */
function formatLogTime(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? "--:--:--"
    : date.toLocaleTimeString("zh-CN", { hour12: false });
}

onMounted(() => {
  stopListening = taskScheduler.subscribe((snapshot) => {
    tasks.value = snapshot;
  });
});

onBeforeUnmount(() => {
  stopListening?.();
  stopListening = null;
});
</script>

<template>
  <div class="task-center" :class="{ 'task-center--open': open }">
    <button
      class="task-center__launcher"
      type="button"
      :aria-expanded="open"
      aria-controls="task-center-panel"
      aria-label="打开任务中心"
      @click="open = !open"
    >
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path d="M7 4h10v16H7zM9.5 8h5M9.5 12h5M9.5 16h3" />
      </svg>
      <span v-if="runningTasks.length" class="task-center__badge">
        {{ runningTasks.length > 9 ? "9+" : runningTasks.length }}
      </span>
    </button>

    <section
      v-if="open"
      id="task-center-panel"
      class="task-center__panel"
      aria-label="任务中心"
    >
      <header class="task-center__header">
        <div>
          <p class="task-center__eyebrow">LOCAL TASKS</p>
          <h2>任务中心</h2>
        </div>
        <button
          type="button"
          class="task-center__close"
          aria-label="关闭任务中心"
          @click="open = false"
        >
          ×
        </button>
      </header>

      <p v-if="!visibleTasks.length" class="task-center__empty">
        暂无进行中或最近完成的任务。
      </p>
      <div v-else class="task-center__list">
        <article v-for="task in visibleTasks" :key="task.id" class="task-center__task">
          <div class="task-center__task-heading">
            <div>
              <strong>{{ task.title }}</strong>
              <small>{{
                task.status === "running"
                  ? "进行中"
                  : task.status === "completed"
                    ? "已完成"
                    : task.status === "cancelled"
                      ? "已取消"
                      : "失败"
              }}</small>
            </div>
            <button
              v-if="task.status === 'running' && task.cancellable"
              type="button"
              class="task-center__cancel"
              @click="cancelTask(task)"
            >
              取消
            </button>
          </div>
          <p class="task-center__message">{{ task.message }}</p>
          <div
            v-if="task.status === 'running'"
            class="task-center__progress"
            role="progressbar"
            :aria-valuenow="progressPercent(task) ?? undefined"
            aria-valuemin="0"
            :aria-valuemax="progressPercent(task) === null ? undefined : 100"
          >
            <span
              :style="{
                width:
                  progressPercent(task) === null
                    ? undefined
                    : `${progressPercent(task)}%`,
              }"
            ></span>
          </div>
          <div v-if="taskCountLabel(task)" class="task-center__count">
            {{ taskCountLabel(task) }}
          </div>
          <ul v-if="task.logs.length" class="task-center__logs">
            <li
              v-for="log in task.logs.slice(-4)"
              :key="`${log.at}-${log.message}`"
              :class="`task-center__log--${log.level}`"
            >
              <time>{{ formatLogTime(log.at) }}</time
              >{{ log.message }}
            </li>
          </ul>
        </article>
      </div>
    </section>
  </div>
</template>

<style scoped>
/* 任务入口固定在应用左下角，页面切换时仍由应用壳保留。 */
.task-center {
  position: fixed;
  z-index: 80;
  left: 16px;
  bottom: 16px;
}

.task-center__launcher {
  position: relative;
  display: grid;
  width: 42px;
  height: 42px;
  place-items: center;
  border: 1px solid rgb(255 255 255 / 34%);
  border-radius: 50%;
  color: #fffaf0;
  background: #7d4b38;
  box-shadow: 0 8px 20px rgb(52 45 40 / 24%);
  cursor: pointer;
}

.task-center__launcher svg {
  width: 21px;
  fill: none;
  stroke: currentColor;
  stroke-linecap: round;
  stroke-linejoin: round;
  stroke-width: 1.7;
}

.task-center__badge {
  position: absolute;
  top: -5px;
  right: -5px;
  min-width: 18px;
  padding: 2px 4px;
  border-radius: 9px;
  color: #fff;
  background: #b3433e;
  font-size: 10px;
  line-height: 14px;
  text-align: center;
}

.task-center__panel {
  position: absolute;
  left: 0;
  bottom: 54px;
  width: min(400px, calc(100vw - 32px));
  max-height: min(650px, calc(100vh - 92px));
  overflow: auto;
  padding: 18px;
  border: 1px solid #d9c9b2;
  border-radius: 10px;
  color: #342d28;
  background: #fffaf0;
  box-shadow: 0 18px 46px rgb(52 45 40 / 22%);
}

.task-center__header,
.task-center__task-heading {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}

.task-center__header {
  margin-bottom: 14px;
  padding-bottom: 12px;
  border-bottom: 1px solid #eadfce;
}

.task-center__eyebrow {
  margin: 0 0 4px;
  color: #ae7653;
  font-size: 9px;
  font-weight: 800;
  letter-spacing: 0.16em;
}

.task-center h2 {
  margin: 0;
  font-size: 18px;
}

.task-center__close,
.task-center__cancel {
  border: 0;
  color: #7d4b38;
  background: transparent;
  cursor: pointer;
}

.task-center__close {
  font-size: 22px;
  line-height: 1;
}

.task-center__cancel {
  padding: 3px 8px;
  border: 1px solid #d9b9a7;
  border-radius: 4px;
  font-size: 11px;
}

.task-center__empty,
.task-center__message,
.task-center__count {
  margin: 0;
  color: #75695d;
  font-size: 12px;
  line-height: 1.55;
}

.task-center__list {
  display: grid;
  gap: 10px;
}

.task-center__task {
  padding: 12px;
  border: 1px solid #eadfce;
  border-radius: 7px;
  background: rgb(255 255 255 / 58%);
}

.task-center__task-heading strong {
  display: block;
  font-size: 13px;
}

.task-center__task-heading small {
  display: block;
  margin-top: 3px;
  color: #ae7653;
  font-size: 10px;
}

.task-center__message {
  margin-top: 8px;
}

.task-center__progress {
  height: 5px;
  margin-top: 9px;
  overflow: hidden;
  border-radius: 4px;
  background: #eadfce;
}

.task-center__progress span {
  display: block;
  width: 35%;
  height: 100%;
  background: #ae7653;
  animation: task-progress-pulse 1.4s ease-in-out infinite alternate;
}

.task-center__count {
  margin-top: 4px;
  font-size: 10px;
  text-align: right;
}

.task-center__logs {
  display: grid;
  gap: 3px;
  margin: 9px 0 0;
  padding: 7px 0 0;
  border-top: 1px dashed #eadfce;
  list-style: none;
  color: #8a7d70;
  font-size: 10px;
  line-height: 1.4;
}

.task-center__logs time {
  margin-right: 6px;
  color: #b0a294;
  font-variant-numeric: tabular-nums;
}

.task-center__log--error {
  color: #b3433e;
}

.task-center__log--success {
  color: #537b6f;
}

@keyframes task-progress-pulse {
  from {
    opacity: 0.58;
  }
  to {
    opacity: 1;
  }
}

@media (max-width: 650px) {
  .task-center {
    left: 12px;
    bottom: 12px;
  }

  .task-center__panel {
    left: -1px;
    width: calc(100vw - 24px);
  }
}
</style>
