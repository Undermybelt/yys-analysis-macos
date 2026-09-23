<script setup lang="ts">
import { defineAsyncComponent } from "vue";

// 启动入口只负责尽快挂载加载态；完整应用壳通过动态 import 延后转换，避免首屏等待全部导航与页面依赖。
const AppShell = defineAsyncComponent(() => import("./AppShell.vue"));
</script>

<template>
  <Suspense>
    <AppShell />
    <template #fallback>
      <main class="app-bootstrap-loading" aria-live="polite">
        <span class="app-bootstrap-loading__mark" aria-hidden="true">YYS</span>
        <span>正在载入本地工作台…</span>
      </main>
    </template>
  </Suspense>
</template>

<style>
/* 启动壳使用独立的最小样式，确保完整应用壳尚未转换时也不会出现白屏。 */
.app-bootstrap-loading {
  display: grid;
  min-height: 100vh;
  place-content: center;
  gap: 12px;
  color: #344b75;
  background: #f8f1e5;
  font:
    12px/1.5 "Microsoft YaHei",
    sans-serif;
  letter-spacing: 0.08em;
  text-align: center;
}

.app-bootstrap-loading__mark {
  color: #a64b3f;
  font-size: 24px;
  font-weight: 700;
  letter-spacing: 0.16em;
}
</style>
