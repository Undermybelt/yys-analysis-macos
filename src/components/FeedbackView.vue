<script setup lang="ts">
import { onBeforeUnmount, ref } from "vue";

// 反馈邮箱集中维护，Bug 与优化建议按钮只预填主题，不自动发送邮件。
const supportEmail = "xx1271328330@163.com";
const bugFeedbackMailto = `mailto:${supportEmail}?subject=${encodeURIComponent("【Bug反馈】")}`;
const ideaFeedbackMailto = `mailto:${supportEmail}?subject=${encodeURIComponent("【优化建议】")}`;
const copyEmailLabel = ref("复制作者邮箱");
let copyEmailResetTimer: number | undefined;

/** 复制邮箱并在按钮上反馈结果；剪贴板不可用时提示用户直接复制页面上的邮箱。 */
async function copySupportEmail(): Promise<void> {
  copyEmailLabel.value = "复制中…";
  try {
    if (!navigator.clipboard) throw new Error("clipboard-unavailable");
    await navigator.clipboard.writeText(supportEmail);
    copyEmailLabel.value = "已复制作者邮箱";
  } catch {
    copyEmailLabel.value = "复制失败，请手动复制";
  }

  if (copyEmailResetTimer !== undefined) {
    window.clearTimeout(copyEmailResetTimer);
  }
  copyEmailResetTimer = window.setTimeout(() => {
    copyEmailLabel.value = "复制作者邮箱";
    copyEmailResetTimer = undefined;
  }, 2000);
}

onBeforeUnmount(() => {
  if (copyEmailResetTimer !== undefined) {
    window.clearTimeout(copyEmailResetTimer);
  }
});
</script>

<template>
  <main id="feedback" class="workspace feedback-workspace">
    <header class="page-header">
      <div>
        <p class="eyebrow">FEEDBACK / 10</p>
        <h1>问题反馈</h1>
        <p class="lede">
          遇到 Bug，请提供问题截图和复现步骤；有优化建议，直接发邮件告诉我你的想法。
        </p>
      </div>
      <span class="status-chip status-chip--safe">邮件沟通 · 信息越完整越快</span>
    </header>

    <section class="runtime-card feedback-guide" aria-labelledby="feedback-guide-title">
      <div class="feedback-guide__header">
        <div>
          <p class="section-kicker">问题反馈</p>
          <h2 id="feedback-guide-title">把问题和想法告诉我</h2>
          <p>遇到问题请尽量提供可复现信息；有优化方向也欢迎直接来信。</p>
        </div>
        <a class="contact-link feedback-guide__email" :href="`mailto:${supportEmail}`">
          {{ supportEmail }}
        </a>
      </div>

      <div class="feedback-guide__grid">
        <article class="feedback-path feedback-path--bug">
          <div class="feedback-path__index" aria-hidden="true">01</div>
          <div>
            <p class="section-kicker">BUG REPORT</p>
            <h3>遇到 Bug</h3>
            <p>请把数据、截图和问题现象一起发来，方便定位。</p>
            <ol class="feedback-steps">
              <li>
                <span>1</span>
                <div>
                  <strong>描述问题现象</strong>
                  <small>写清楚操作步骤、预期结果和实际结果。</small>
                </div>
              </li>
              <li>
                <span>2</span>
                <div>
                  <strong>附上 Bug 截图</strong>
                  <small>截下出现问题的页面，能说明问题即可。</small>
                </div>
              </li>
            </ol>
            <div class="feedback-path__actions">
              <a
                class="button button--quiet feedback-path__action"
                :href="bugFeedbackMailto"
              >
                发送 Bug 反馈
              </a>
              <button
                class="button button--quiet feedback-path__action"
                type="button"
                @click="copySupportEmail"
              >
                {{ copyEmailLabel }}
              </button>
            </div>
          </div>
        </article>

        <article class="feedback-path feedback-path--idea">
          <div class="feedback-path__index" aria-hidden="true">02</div>
          <div>
            <p class="section-kicker">IDEA &amp; FEEDBACK</p>
            <h3>有优化建议</h3>
            <p>不用准备数据，直接发邮件说说你的想法和使用感受。</p>
            <div class="feedback-path__note">
              <strong>可以写这些</strong>
              <span>你遇到了什么不便、希望怎么改，以及这个改动会怎样帮助你。</span>
            </div>
            <div class="feedback-path__actions">
              <a
                class="button button--quiet feedback-path__action"
                :href="ideaFeedbackMailto"
              >
                发送优化建议
              </a>
            </div>
          </div>
        </article>
      </div>
    </section>
  </main>
</template>
