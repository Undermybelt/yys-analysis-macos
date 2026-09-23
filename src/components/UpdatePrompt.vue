<script setup lang="ts">
import { computed } from "vue";
import type { UpdateCandidateSummary, UpdateChannel, UpdateProgress } from "../api/contracts";
import UpdateBackupNotice from "./UpdateBackupNotice.vue";

const props = defineProps<{
  candidate: UpdateCandidateSummary;
  currentVersion: string;
  channel: UpdateChannel;
  installing: boolean;
  progress: UpdateProgress | null;
  errorMessage: string;
}>();

const emit = defineEmits<{
  update: [];
  dismiss: [];
}>();

// 弹窗只展示后端已校验并暂存的候选；安装动作由父级统一防抖并交给更新助手执行。
const progressPercent = computed(() => {
  if (!props.progress || props.progress.total <= 0) return 0;
  return Math.min(100, Math.round((props.progress.completed / props.progress.total) * 100));
});
</script>

<template>
  <div class="startup-update-dialog" role="presentation">
    <div class="startup-update-dialog__scrim"></div>
    <section
      class="startup-update-dialog__panel"
      role="dialog"
      aria-modal="true"
      aria-labelledby="startup-update-title"
    >
      <header class="startup-update-dialog__header">
        <div>
          <p class="section-kicker">
            {{ props.channel === "test" ? "测试更新" : "发现新版本" }}
          </p>
          <h2 id="startup-update-title">平安志 {{ props.candidate.version }}</h2>
        </div>
        <span class="status-chip status-chip--safe">{{
          props.channel === "test" ? "test 通道" : "stable 通道"
        }}</span>
      </header>

      <p class="startup-update-dialog__summary">
        来源：Gitee Release · 当前版本 {{ props.currentVersion }}，清单签名已校验；点击更新后下载并校验安装包。
      </p>

      <article class="startup-update-dialog__notes">
        <h3>更新内容</h3>
        <p>{{ props.candidate.releaseNotes || "本次更新没有附加说明。" }}</p>
      </article>

      <UpdateBackupNotice />

      <section v-if="props.installing" class="startup-update-dialog__progress" aria-live="polite">
        <div class="startup-update-dialog__progress-heading">
          <strong>{{ props.progress?.message || "正在准备更新" }}</strong>
          <span>{{ progressPercent }}%</span>
        </div>
        <div class="startup-update-dialog__progress-track" role="progressbar" :aria-valuenow="progressPercent" aria-valuemin="0" aria-valuemax="100">
          <span :style="{ width: `${progressPercent}%` }"></span>
        </div>
        <p v-if="props.progress?.total" class="startup-update-dialog__progress-detail">
          已处理 {{ (props.progress.completed / 1024 / 1024).toFixed(1) }} MB /
          {{ (props.progress.total / 1024 / 1024).toFixed(1) }} MB
        </p>
      </section>

      <p v-if="props.errorMessage" class="inline-error" role="alert">
        {{ props.errorMessage }}
      </p>

      <footer class="startup-update-dialog__actions">
        <button
          class="button button--quiet"
          type="button"
          :disabled="props.installing"
          @click="emit('dismiss')"
        >
          稍后提醒
        </button>
        <button
          class="button button--primary"
          type="button"
          :disabled="props.installing"
          @click="emit('update')"
        >
          {{ props.installing ? "正在更新…" : "立即更新" }}
        </button>
      </footer>
    </section>
  </div>
</template>
