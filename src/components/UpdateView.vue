<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import { normalizeCommandError } from "../api/errors";
import packageJson from "../../package.json";
import type {
  InstalledPackage,
  CatalogStatus,
  UpdateCheckResult,
  UpdateProgress,
  UpdateInstallResult,
  UpdatePackageType,
  UpdateSourceMode,
  UpdateChannel,
} from "../api/contracts";
import { APP_UPDATE_CHANNEL } from "../updateChannel";
import UpdateBackupNotice from "./UpdateBackupNotice.vue";

const props = withDefaults(
  defineProps<{
    /** 测试菜单只允许检查应用包，避免测试人员误操作数据包通道。 */
    testOnly?: boolean;
  }>(),
  { testOnly: false },
);

// 版本发布统一走维护者的 Gitee Release，避免用户在已停用的渠道之间做选择。
const sourceMode = ref<UpdateSourceMode>("gitee");
const channel = ref<UpdateChannel>(APP_UPDATE_CHANNEL);
// 测试菜单固定应用包；正式帮助页仍保留目录、规则和适配器的手动更新选择。
const packageType = ref<UpdatePackageType>("application");
const result = ref<UpdateCheckResult | null>(null);
const installed = ref<InstalledPackage[]>([]);
const catalogStatus = ref<CatalogStatus | null>(null);
const errorMessage = ref("");
const statusMessage = ref("");
const isChecking = ref(false);
const isInstalling = ref(false);
const updateProgress = ref<UpdateProgress | null>(null);
let stopUpdateProgress: (() => void) | null = null;
const updateProgressPercent = computed(() => {
  if (!updateProgress.value || updateProgress.value.total <= 0) return 0;
  return Math.min(
    100,
    Math.round((updateProgress.value.completed / updateProgress.value.total) * 100),
  );
});

// 软件信息集中维护，帮助页与构建版本保持同步，避免手工填写版本号造成误导。
const appVersion = packageJson.version;
const supportEmail = "xx1271328330@163.com";
const repositoryUrl = "https://gitee.com/tigerdonotcry/yys-analysis";

const packageLabels: Record<UpdatePackageType, string> = {
  application: "应用",
  catalog: "御魂与式神目录",
  rules: "分析规则",
  adapter: "读取适配器",
};

const activePackage = computed(() =>
  installed.value.find((item) => item.packageType === packageType.value && item.active),
);

interface DisplayInstalledPackage extends InstalledPackage {
  /** 前端补出的运行中应用版本，用于区分尚未写入更新历史的当前版本。 */
  isRuntimeVersion?: boolean;
}

/** 内置目录首次安装可能还没有通用更新历史，页面此时仍应展示真实活动版本。 */
const currentPackageVersion = computed(() => {
  if (activePackage.value) return activePackage.value.version;
  if (packageType.value === "catalog") return catalogStatus.value?.version ?? "未安装";
  return packageType.value === "application" ? appVersion : "未安装";
});

/** 应用首次启动时还没有独立更新记录；补充运行版本后，版本历史仍能显示真实当前版本。 */
const historyItems = computed<DisplayInstalledPackage[]>(() => {
  const hasActiveApplication = installed.value.some(
    (item) => item.packageType === "application" && item.active,
  );
  if (hasActiveApplication) return installed.value;

  return [
    {
      // 该标识只用于 Vue 列表稳定渲染，不会写入后台安装记录。
      id: "runtime-application-version",
      packageType: "application",
      version: appVersion,
      sha256: null,
      signatureFingerprint: null,
      active: true,
      source: null,
      contentPath: null,
      installedAt: "当前运行",
      isRuntimeVersion: true,
    },
    ...installed.value,
  ];
});

const selected = computed(() => result.value?.selected ?? null);

/** 首次打开只读取安装历史，不自动联网，避免页面挂载产生外部副作用。 */
async function loadInstalled(): Promise<void> {
  try {
    const [packages, currentCatalog] = await Promise.all([
      desktopApi.listInstalledPackages(),
      desktopApi.getCatalogStatus(),
    ]);
    installed.value = packages;
    catalogStatus.value = currentCatalog;
  } catch (error) {
    errorMessage.value = formatError(error);
  }
}

/** 用户明确点击检查后才访问 Gitee；应用包正文延迟到安装按钮点击后下载。 */
async function checkUpdates(): Promise<void> {
  isChecking.value = true;
  errorMessage.value = "";
  updateProgress.value = null;
  statusMessage.value = "正在检查镜像并验证发布清单…";
  result.value = null;
  try {
    result.value = await desktopApi.checkUpdates({
      sourceMode: sourceMode.value,
      channel: channel.value,
      packageType: packageType.value,
      stagePayload: false,
    });
    statusMessage.value = result.value.selected
      ? `发现 ${result.value.selected.version}，清单已校验，点击安装后开始下载。`
      : `当前已是最新${channel.value === "test" ? "测试" : "稳定"}版本。`;
  } catch (error) {
    errorMessage.value = formatError(error);
    statusMessage.value = "检查未完成。";
  } finally {
    isChecking.value = false;
  }
}

/** 安装动作再次由 Rust 校验暂存清单；活动读取或后台任务运行时会返回延迟状态。 */
async function installUpdate(): Promise<void> {
  if (!selected.value) return;
  isInstalling.value = true;
  errorMessage.value = "";
  updateProgress.value = null;
  try {
    const installResult = await desktopApi.installUpdate({
      stagedUpdateId: selected.value.stagedUpdateId,
      autoRestart: packageType.value === "application",
    });
    handleInstallResult(installResult);
    await loadInstalled();
  } catch (error) {
    errorMessage.value = formatError(error);
  } finally {
    isInstalling.value = false;
  }
}

/** 历史回退要求用户逐次点击确认，界面不提供自动降级开关。 */
async function rollback(item: InstalledPackage): Promise<void> {
  if (
    item.active ||
    !window.confirm(
      `确定回退 ${packageLabels[item.packageType]} 到 ${item.version} 吗？`,
    )
  ) {
    return;
  }
  errorMessage.value = "";
  try {
    const installResult = await desktopApi.rollbackUpdate({
      packageType: item.packageType,
      version: item.version,
    });
    handleInstallResult(installResult);
    await loadInstalled();
  } catch (error) {
    errorMessage.value = formatError(error);
  }
}

function handleInstallResult(installResult: UpdateInstallResult): void {
  statusMessage.value = installResult.message;
  if (installResult.state === "deferred") {
    statusMessage.value = `已延迟：${installResult.message}`;
  }
}

function formatError(error: unknown): string {
  const normalized = normalizeCommandError(error);
  return `${normalized.code} · ${normalized.message}`;
}

onMounted(() => {
  void loadInstalled();
  void desktopApi
    .onUpdateProgress((progress) => {
      if (progress.stagedUpdateId === selected.value?.stagedUpdateId) {
        updateProgress.value = progress;
      }
    })
    .then((unlisten) => {
      stopUpdateProgress = unlisten;
    })
    .catch(() => {
      // 浏览器预览模式没有 Tauri 事件桥，保持页面可用。
    });
});

onBeforeUnmount(() => {
  stopUpdateProgress?.();
  stopUpdateProgress = null;
});
</script>

<template>
  <main id="help" class="workspace">
    <header class="page-header">
      <div>
        <p class="eyebrow">HELP / 09</p>
        <h1>{{ props.testOnly ? "测试更新" : "帮助" }}</h1>
        <p class="lede">
          <template v-if="props.testOnly">
            检查 test 通道应用安装包，验证发布说明、暂存、安装和自动重启链路。
          </template>
          <template v-else>
            查看软件信息、联系作者，或从 Gitee 检查应用、目录、规则和读取适配器的最新版本。
            更新包会在签名身份与 SHA-256 校验通过后才进入暂存。
          </template>
        </p>
      </div>
      <span class="status-chip status-chip--safe"
        >本地优先 · {{ channel === "test" ? "测试通道" : "稳定通道" }}</span
      >
    </header>

    <section class="help-overview-grid" aria-label="软件信息与使用帮助">
      <article class="runtime-card software-info-card">
        <div class="card-heading">
          <div>
            <p class="section-kicker">软件信息</p>
            <h2>平安志</h2>
          </div>
          <span class="status-chip">v{{ appVersion }}</span>
        </div>
        <div class="software-identity">
          <div class="software-identity__mark" aria-hidden="true">志</div>
          <div>
            <strong>阴阳师本地数据工作台</strong>
            <p>帮助你理解御魂资产，获得可解释的处置结论。</p>
          </div>
        </div>
        <dl class="fact-list software-facts">
          <div>
            <dt>作者</dt>
            <dd>铁血战士胖虎</dd>
          </div>
          <div>
            <dt>邮箱</dt>
            <dd>
              <a class="contact-link" :href="`mailto:${supportEmail}`">{{
                supportEmail
              }}</a>
            </dd>
          </div>
          <div>
            <dt>项目地址</dt>
            <dd>
              <a
                class="contact-link"
                :href="repositoryUrl"
                target="_blank"
                rel="noreferrer"
              >
                Gitee / tigerdonotcry/yys-analysis
              </a>
            </dd>
          </div>
        </dl>
      </article>

      <article class="runtime-card help-notes-card">
        <div class="card-heading">
          <div>
            <p class="section-kicker">使用帮助</p>
          <h2>{{ props.testOnly ? "测试更新流程" : "需要更新时" }}</h2>
          </div>
          <span class="status-chip status-chip--safe">可控更新</span>
        </div>
        <ul class="help-list">
          <li>
            <strong>选择更新对象</strong>
            <span>{{
              props.testOnly
                ? "测试菜单只检查应用安装包，不会改变正式通道数据。"
                : "应用、御魂与式神目录、分析规则和读取适配器均从 Gitee Release 获取。"
            }}</span>
          </li>
          <li>
            <strong>点击检查更新</strong>
            <span>{{
              props.testOnly
                ? "点击检查后会下载并校验测试通道清单。"
                : "应用启动会后台检查应用更新；本页按钮用于主动检查目录、规则和适配器。"
            }}</span>
          </li>
          <li>
            <strong>确认后再安装</strong>
            <span>通过签名、哈希和兼容性校验后，才可安装或回退版本。</span>
          </li>
        </ul>
      </article>
    </section>

    <section class="pulse-card update-card" aria-label="检查更新">
      <div class="pulse-card__copy">
        <p class="section-kicker">检查更新</p>
        <h2>{{ packageLabels[packageType] }}</h2>
        <p>当前版本：{{ currentPackageVersion }}</p>
        <div class="update-controls">
          <div class="update-source-note" aria-label="更新源">
            <span>更新源</span>
            <strong>Gitee Release</strong>
          </div>
          <label>
            <span>更新对象</span>
            <select v-if="!props.testOnly" v-model="packageType">
              <option
                v-for="(label, value) in packageLabels"
                :key="value"
                :value="value"
              >
                {{ label }}
              </option>
            </select>
          </label>
          <button
            class="button button--primary"
            type="button"
            :disabled="isChecking"
            @click="checkUpdates"
          >
            {{ isChecking ? "检查中…" : "检查更新" }}
          </button>
        </div>
        <p v-if="statusMessage" class="task-message">{{ statusMessage }}</p>
        <p v-if="errorMessage" class="inline-error" role="alert">{{ errorMessage }}</p>
      </div>
    </section>

    <section
      v-if="result"
      class="diagnostic-grid update-grid"
      aria-label="镜像校验结果"
    >
      <article class="runtime-card">
        <div class="card-heading">
          <div>
            <p class="section-kicker">候选更新</p>
            <h2>{{ selected ? selected.version : "无可用更新" }}</h2>
          </div>
          <button
            v-if="selected"
            class="button button--primary"
            type="button"
            :disabled="isInstalling"
            @click="installUpdate"
          >
            {{
              isInstalling
                ? "安装中…"
                : packageType === "application"
                  ? "更新并重启"
                  : "安装已验证版本"
            }}
          </button>
        </div>
        <UpdateBackupNotice v-if="selected" />
        <section v-if="isInstalling" class="update-download-progress" aria-live="polite">
          <div class="update-download-progress__heading">
            <strong>{{ updateProgress?.message || "正在准备更新" }}</strong>
            <span>{{ updateProgressPercent }}%</span>
          </div>
          <div class="update-download-progress__track" role="progressbar" :aria-valuenow="updateProgressPercent" aria-valuemin="0" aria-valuemax="100">
            <span :style="{ width: `${updateProgressPercent}%` }"></span>
          </div>
        </section>
        <dl v-if="selected" class="fact-list">
          <div>
            <dt>来源</dt>
            <dd>{{ selected.source }}</dd>
          </div>
          <div>
            <dt>签名身份</dt>
            <dd>{{ selected.signerKeyId }}</dd>
          </div>
          <div>
            <dt>签名指纹</dt>
            <dd class="mono-note">{{ selected.signatureFingerprint }}</dd>
          </div>
          <div>
            <dt>SHA-256</dt>
            <dd class="mono-note">{{ selected.contentSha256 }}</dd>
          </div>
          <div>
            <dt>说明</dt>
            <dd>{{ selected.releaseNotes || "无发布说明" }}</dd>
          </div>
        </dl>
      </article>

      <article class="runtime-card">
        <div class="card-heading">
          <div>
            <p class="section-kicker">镜像一致性</p>
            <h2>校验结果</h2>
          </div>
          <span class="status-chip status-chip--safe">不会静默替换</span>
        </div>
        <ul class="backup-list">
          <li v-for="mirror in result.mirrors" :key="mirror.source">
            <div class="backup-row">
              <div>
                <strong>{{ mirror.source }}</strong>
                <span class="backup-row__meta"
                  >{{ mirror.version ?? "—" }} · {{ mirror.status }}</span
                >
              </div>
              <small>{{
                mirror.message ?? mirror.signerKeyId ?? "已通过清单校验"
              }}</small>
            </div>
          </li>
        </ul>
      </article>
    </section>

    <section class="runtime-card update-history-card" aria-label="已安装数据包历史">
      <div class="card-heading">
        <div>
          <p class="section-kicker">版本历史</p>
          <h2>已安装包</h2>
        </div>
        <span class="status-chip">{{ historyItems.length }} 个记录</span>
      </div>
      <ul class="backup-list">
        <li v-for="item in historyItems" :key="item.id">
          <div class="backup-row">
            <div>
              <strong
                >{{ packageLabels[item.packageType] }} · {{ item.version }}</strong
              >
              <span class="backup-row__meta">
                {{
                  item.isRuntimeVersion
                    ? "当前运行版本"
                    : item.active
                      ? "活动版本"
                      : "历史版本"
                }}
                · {{ item.source ?? "本地" }} · {{ item.installedAt }}
              </span>
            </div>
            <button
              v-if="!item.active && !item.isRuntimeVersion"
              class="button button--quiet"
              type="button"
              @click="rollback(item)"
            >
              用户选择回退
            </button>
          </div>
        </li>
      </ul>
    </section>
  </main>
</template>
