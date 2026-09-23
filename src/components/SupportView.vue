<script setup lang="ts">
// 捐赠人按支付渠道分组维护；昵称中的脱敏符号必须原样保留，避免公开未授权的完整身份信息。
const donorChannels = [
  {
    name: "微信",
    donors: [
      "*始",
      "Lan兰",
      "S*r",
      "E*e",
      "* `",
      "**宇",
      "J*s",
      "*C",
      "* 始",
      "*之",
    ],
  },
  {
    name: "支付宝",
    donors: ["**嘉", "**勇"],
  },
];

// 列表右上角展示所有渠道的捐赠人数，避免新增渠道后只统计单个分组。
const donorCount = donorChannels.reduce(
  (total, channel) => total + channel.donors.length,
  0,
);

// 建议和反馈贡献者按普通昵称与 NGA 公开链接分组；未提供 UID 的新增昵称仅展示文字。
const feedbackContributors = [
  "辰家老许",
  "Li,兰",
  "腿杰",
  "junkDog",
  "luwei87402667",
  "累了我只想摸鱼",
  "少时之约-转折出发",
  "小红书5DDCC0F4",
];
const ngaContributors = [
  { name: "Forever丶Angel彡", uid: "60587121" },
  { name: "nabbit", uid: "63110632" },
  { name: "天宫米", uid: "41531816" },
  { name: "月出皎兮心悄悄", uid: "43258513" },
  { name: "fluxxu", uid: "41171522" },
  { name: "11xxxxx11", uid: "65110311" },
  { name: "面包永不面壁", uid: "63859739" },
];
const additionalNgaContributors = ["芝士瓜子", "BobPan", "脩殒问问", "にゃおす"];
</script>

<template>
  <main id="support" class="workspace support-workspace">
    <header class="page-header support-header">
      <div>
        <p class="eyebrow">SUPPORT / 11</p>
        <h1>支持我</h1>
        <p class="lede">你的捐赠让我有动力继续开发。</p>
      </div>
      <div class="support-header__seal" aria-hidden="true">谢</div>
    </header>

    <section class="support-intro" aria-labelledby="support-coffee-title">
      <div>
        <p class="section-kicker">请我喝杯咖啡</p>
        <h2 id="support-coffee-title">让平安志继续保持好用</h2>
        <p>
          如果这个工具帮你更快整理了御魂，欢迎用一杯咖啡表达支持。每一份心意，都会成为继续维护和开发的动力。
        </p>
      </div>
      <span class="support-intro__mark" aria-hidden="true">☕</span>
    </section>

    <section class="support-section" aria-labelledby="support-codes-title">
      <div class="support-section__heading">
        <div>
          <p class="section-kicker">MY QR CODES</p>
          <h2 id="support-codes-title">我的收款码</h2>
        </div>
        <p>打开支付宝或微信扫一扫即可支持。</p>
      </div>

      <div class="support-code-grid">
        <article class="support-code-card">
          <div class="support-code-card__label">
            <span class="support-code-card__badge support-code-card__badge--alipay"
              >支</span
            >
            <div>
              <h3>支付宝</h3>
              <p>推荐使用支付宝</p>
            </div>
          </div>
          <img
            src="/donation-alipay.jpg"
            alt="支付宝收款码"
            class="support-code-card__image support-code-card__image--alipay"
          />
        </article>

        <article class="support-code-card">
          <div class="support-code-card__label">
            <span class="support-code-card__badge support-code-card__badge--wechat"
              >微</span
            >
            <div>
              <h3>微信支付</h3>
              <p>推荐使用微信支付</p>
            </div>
          </div>
          <img
            src="/donation-wechat.jpg"
            alt="微信支付收款码"
            class="support-code-card__image support-code-card__image--wechat"
          />
        </article>
      </div>
    </section>

    <section class="support-lower-grid">
      <article class="support-panel" aria-labelledby="support-donors-title">
        <div class="support-panel__heading">
          <div>
            <p class="section-kicker">THANK YOU</p>
            <h2 id="support-donors-title">捐赠人列表</h2>
          </div>
          <span class="support-panel__count">{{ donorCount }} 位</span>
        </div>
        <div v-if="donorCount" class="support-donor-channels">
          <div
            v-for="channel in donorChannels"
            :key="channel.name"
            class="support-donor-channel"
          >
            <div class="support-donor-channel__heading">
              <span class="support-donor-channel__name">{{ channel.name }}渠道</span>
              <span class="support-donor-channel__count"
                >{{ channel.donors.length }} 位</span
              >
            </div>
            <div class="support-donor-list">
              <span v-for="donor in channel.donors" :key="donor">{{ donor }}</span>
            </div>
          </div>
        </div>
        <p v-if="donorCount" class="support-donor-note">
          备注：部分捐赠人用的 emoji 符号我打不出来 😅，没法记录，请见谅。
        </p>
        <p v-else class="support-empty">
          感谢每一位支持者。捐赠人名单将在获得公开许可并补充姓名后展示。
        </p>
      </article>

      <article
        class="support-panel support-feedback"
        aria-labelledby="support-feedback-title"
      >
        <div>
          <p class="section-kicker">THANK YOU</p>
          <h2 id="support-feedback-title">感谢以下大佬提供的建议和反馈</h2>
        </div>
        <div class="support-feedback__contributors">
          <p>
            式神录数据支持：
            <a href="https://wiki.biligame.com/" target="_blank" rel="noreferrer"
              >wiki.biligame.com</a
            >
          </p>
          <p>
            传记数据：
            <a
              href="https://yys.huijiwiki.com/wiki/%E9%A6%96%E9%A1%B5"
              target="_blank"
              rel="noreferrer"
              >灰机wiki</a
            >
          </p>
          <p>
            其他渠道：
            <span
              v-for="(contributor, index) in feedbackContributors"
              :key="contributor"
              >{{ contributor }}{{ index < feedbackContributors.length - 1 ? "，" : "。" }}</span
            >
          </p>
          <p>
            NGA：
            <span
              v-for="(contributor, index) in ngaContributors"
              :key="contributor.uid"
              class="support-feedback__nga-contributor"
            >
              <a
                :href="`https://bbs.nga.cn/nuke.php?func=ucp&uid=${contributor.uid}`"
                target="_blank"
                rel="noreferrer"
                >{{ contributor.name }}</a
              >{{
                index < ngaContributors.length - 1 || additionalNgaContributors.length
                  ? "，"
                  : "。"
              }}
            </span>
            <span
              v-for="(contributor, index) in additionalNgaContributors"
              :key="contributor"
              class="support-feedback__nga-contributor"
            >
              {{ contributor }}{{ index < additionalNgaContributors.length - 1 ? "，" : "。" }}
            </span>
          </p>
        </div>
        <p class="support-feedback__note">部分人员未统计到请见谅。</p>
      </article>
    </section>
  </main>
</template>
