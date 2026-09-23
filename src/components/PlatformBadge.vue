<script setup lang="ts">
import { computed } from "vue";
import type { GamePlatform } from "../api/contracts";

const props = withDefaults(
  defineProps<{
    /** 稳定的平台枚举；调用方只传入已校验的安卓或 iOS。 */
    platform: GamePlatform;
    /** 紧凑场景使用更小的徽章，仍保留图标和文字。 */
    compact?: boolean;
  }>(),
  { compact: false },
);

// 平台采用项目现有的 jade / indigo 色系，但用不同底色和图形避免只靠文字区分。
const platformLabel = computed(() => (props.platform === "android" ? "安卓" : "iOS"));
const platformAriaLabel = computed(() => `${platformLabel.value}平台`);
</script>

<template>
  <span
    class="platform-badge"
    :class="[
      `platform-badge--${platform}`,
      { 'platform-badge--compact': compact },
    ]"
    :aria-label="platformAriaLabel"
    :title="platformAriaLabel"
  >
    <svg
      v-if="platform === 'android'"
      class="platform-badge__icon"
      viewBox="0 0 24 24"
      aria-hidden="true"
    >
      <path d="M8.1 8.3h7.8v8.1a1.6 1.6 0 0 1-1.6 1.6H9.7a1.6 1.6 0 0 1-1.6-1.6V8.3Z" />
      <path d="m9.2 7.8-1.3-2.1m6.9 2.1 1.3-2.1M6.3 9.1v6.2m11.4-6.2v6.2M9.4 18v2.1m5.2-2.1v2.1" />
      <circle cx="10.5" cy="11.2" r=".7" />
      <circle cx="13.5" cy="11.2" r=".7" />
    </svg>
    <svg
      v-else
      class="platform-badge__icon platform-badge__icon--apple"
      viewBox="0 0 24 24"
      aria-hidden="true"
    >
      <path d="M16.6 12.6c0-1.8 1.5-2.7 1.6-2.8-.9-1.3-2.2-1.5-2.7-1.5-1.1-.1-2.1.7-2.7.7-.6 0-1.5-.7-2.4-.7-1.2 0-2.4.7-3 1.8-1.3 2.2-.3 5.5.9 7.3.6.9 1.3 1.8 2.3 1.8s1.4-.6 2.6-.6 1.5.6 2.6.6 1.7-.9 2.3-1.7c.7-1 1-2 1.1-2.1-.1 0-2.6-1-2.6-2.8Z" />
      <path d="M14.8 6.9c.5-.7.9-1.7.8-2.7-.9 0-2 .6-2.6 1.3-.5.6-.9 1.6-.8 2.5 1 .1 2-.4 2.6-1.1Z" />
    </svg>
    <span>{{ platformLabel }}</span>
  </span>
</template>

<style scoped>
.platform-badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-height: 20px;
  padding: 3px 8px 3px 5px;
  border: 1px solid;
  border-radius: 999px;
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.04em;
  line-height: 1;
  white-space: nowrap;
}

.platform-badge--android {
  border-color: rgb(42 143 89 / 45%);
  background: rgb(42 143 89 / 11%);
  color: #247b50;
}

.platform-badge--ios {
  border-color: rgb(67 78 116 / 48%);
  background: rgb(67 78 116 / 11%);
  color: #414d78;
}

.platform-badge--compact {
  min-height: 18px;
  gap: 3px;
  padding: 2px 6px 2px 4px;
  font-size: 9px;
}

.platform-badge__icon {
  width: 13px;
  height: 13px;
  flex: 0 0 auto;
  fill: currentColor;
  stroke: currentColor;
  stroke-linecap: round;
  stroke-linejoin: round;
  stroke-width: 1.65;
}

.platform-badge__icon--apple {
  stroke: none;
}
</style>
