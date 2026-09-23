<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { desktopApi } from "../api/client";
import {
  activeCharacterId,
  characterArchives,
  onActiveCharacterChanged,
  switchActiveCharacter,
} from "../state/activeCharacter";
import type { CharacterArchive, GamePlatform } from "../api/contracts";
import PlatformBadge from "./PlatformBadge.vue";

/** 全局角色切换下拉；放在各页面右上角，切换后由页面自行监听事件重新加载数据。 */
const switching = ref(false);
const switchError = ref(false);

const currentArchive = computed(
  () =>
    characterArchives.value.find(
      (entry) => entry.profileId === activeCharacterId.value,
    ) ?? null,
);

const currentLabel = computed(() => {
  const archive = currentArchive.value;
  if (!archive) return "未导入角色";
  return archiveLabel(archive);
});

/** 将档案中的稳定平台值翻译成用户可读名称；旧档案缺失时省略平台段。 */
function platformLabel(platform: GamePlatform | null): string {
  if (platform === "android") return "安卓";
  if (platform === "ios") return "iOS";
  return "";
}

/** 统一当前角色提示和下拉选项的身份展示顺序，避免区服/平台信息不一致。 */
function archiveLabel(archive: CharacterArchive): string {
  return [archive.displayName, archive.serverLabel, platformLabel(archive.platform)]
    .filter(Boolean)
    .join(" · ");
}

// 只有当前激活角色不在列表里（尚未导入或档案失效）才保留空占位，避免正常状态下多出一个无效选项。
const showPlaceholder = computed(() => currentArchive.value === null);

async function handleChange(event: Event): Promise<void> {
  const profileId = (event.target as HTMLSelectElement).value;
  if (!profileId || profileId === activeCharacterId.value) return;
  switching.value = true;
  switchError.value = false;
  try {
    await switchActiveCharacter(profileId);
  } catch {
    switchError.value = true;
  } finally {
    switching.value = false;
  }
}

async function refresh(): Promise<void> {
  try {
    const result = await desktopApi.listCharacterArchives();
    characterArchives.value = result.characters;
  } catch {
    // 拉取失败时保留已有列表，切换器不阻塞页面。
  }
}

let stopListening: (() => void) | null = null;
onMounted(() => {
  void refresh();
  stopListening = onActiveCharacterChanged(() => void refresh());
});
onBeforeUnmount(() => {
  stopListening?.();
  stopListening = null;
});
</script>

<template>
  <div class="character-switcher">
    <label class="character-switcher__label" for="character-switcher">当前角色</label>
    <select
      id="character-switcher"
      class="character-switcher__select"
      :value="activeCharacterId ?? ''"
      :disabled="switching"
      :aria-label="`当前角色：${currentLabel}`"
      @change="handleChange"
    >
      <option v-if="showPlaceholder" value="" disabled>未导入角色</option>
      <option
        v-for="archive in characterArchives"
        :key="archive.profileId ?? archive.accountId"
        :value="archive.profileId ?? ''"
        :disabled="!archive.profileId"
      >
        {{ archiveLabel(archive) }}
      </option>
    </select>
    <PlatformBadge
      v-if="currentArchive?.platform"
      :platform="currentArchive.platform"
      compact
    />
    <span v-if="switchError" class="character-switcher__error" role="alert"
      >切换失败</span
    >
  </div>
</template>

<style scoped>
.character-switcher {
  display: inline-flex;
  align-items: center;
  gap: 0.55rem;
  white-space: nowrap;
}

.character-switcher__label {
  color: var(--ink-soft);
  font-size: 0.72rem;
  font-weight: 700;
  letter-spacing: 0.08em;
}

.character-switcher__select {
  min-height: 2.35rem;
  max-width: 15rem;
  padding: 0.35rem 2rem 0.35rem 0.65rem;
  border: 1px solid var(--line);
  border-radius: 0.65rem;
  background: var(--paper);
  color: var(--ink);
  font-size: 0.85rem;
}

.character-switcher__error {
  color: var(--cinnabar);
  font-size: 0.75rem;
}
</style>
