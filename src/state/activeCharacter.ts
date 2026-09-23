import { ref } from "vue";
import { desktopApi, type DesktopApi } from "../api/client";

// 激活角色类型从档案查询 API 推导，保持页面状态与后端返回结构同步且不直接加载完整契约文件。
type CharacterArchive = Awaited<
  ReturnType<DesktopApi["listCharacterArchives"]>
>["characters"][number];

/** 角色切换后由各页面监听并重新拉取数据的窗口事件名。 */
export const ACTIVE_CHARACTER_CHANGED_EVENT = "active-character-changed";

/** 应用重启后记住上次激活的角色档案 ID；不存在或失效时自动忽略。 */
const STORAGE_KEY = "yys-active-character-id";

/** 当前激活角色的数据档案 ID；null 表示尚未导入任何角色。 */
export const activeCharacterId = ref<string | null>(null);

/** 本机已导入的全部角色档案；由切换器和角色档案页各自拉取后共享。 */
export const characterArchives = ref<CharacterArchive[]>([]);

/** 组件挂载时一次性同步：优先沿用后端激活角色，其次恢复上次记住的角色。 */
export async function syncActiveCharacter(): Promise<string | null> {
  try {
    const result = await desktopApi.listCharacterArchives();
    characterArchives.value = result.characters;
    const remembered = localStorage.getItem(STORAGE_KEY);
    const existing = characterArchives.value.find(
      (entry) => entry.profileId === remembered,
    );
    const target =
      characterArchives.value.find((entry) => entry.isCurrent) ??
      existing ??
      characterArchives.value[0] ??
      null;
    const previous = activeCharacterId.value;
    if (target?.profileId) {
      // 应用重启后 Rust 端的激活档案状态会重新初始化；恢复本地记忆角色前必须同步后端，
      // 否则“我的御魂”查询会因没有 active profile 直接返回空列表。
      if (!target.isCurrent) {
        await desktopApi.setActiveCharacter({ profileId: target.profileId });
      }
      activeCharacterId.value = target.profileId;
      localStorage.setItem(STORAGE_KEY, target.profileId);
    } else {
      activeCharacterId.value = null;
      localStorage.removeItem(STORAGE_KEY);
    }
    // 导入完成后后端会静默切换激活角色；这里把变化广播给各页面重新加载。
    if (previous !== activeCharacterId.value) {
      window.dispatchEvent(
        new CustomEvent(ACTIVE_CHARACTER_CHANGED_EVENT, {
          detail: { profileId: activeCharacterId.value },
        }),
      );
    }
    return activeCharacterId.value;
  } catch {
    activeCharacterId.value = null;
    return null;
  }
}

/** 切换当前激活角色：先通知后端，成功后广播事件让所有页面重新加载。 */
export async function switchActiveCharacter(profileId: string): Promise<void> {
  await desktopApi.setActiveCharacter({ profileId });
  activeCharacterId.value = profileId;
  localStorage.setItem(STORAGE_KEY, profileId);
  window.dispatchEvent(
    new CustomEvent(ACTIVE_CHARACTER_CHANGED_EVENT, {
      detail: { profileId },
    }),
  );
}

/** 清空角色档案后同步本地激活状态，避免残留上次角色 ID。 */
export function resetActiveCharacter(): void {
  activeCharacterId.value = null;
  localStorage.removeItem(STORAGE_KEY);
  window.dispatchEvent(
    new CustomEvent(ACTIVE_CHARACTER_CHANGED_EVENT, {
      detail: { profileId: null },
    }),
  );
}

/** 订阅角色切换事件；返回取消监听函数，组件卸载时必须调用。 */
export function onActiveCharacterChanged(
  listener: (profileId: string | null) => void,
): () => void {
  const handler = (event: Event): void => {
    const detail = (event as CustomEvent<{ profileId: string | null }>).detail;
    listener(detail?.profileId ?? null);
  };
  window.addEventListener(ACTIVE_CHARACTER_CHANGED_EVENT, handler);
  return () => window.removeEventListener(ACTIVE_CHARACTER_CHANGED_EVENT, handler);
}

/**
 * 分析页等保留手动档案下拉的页面：若当前激活角色存在于档案列表则返回其 ID，
 * 否则返回 null（保持页面现有选择）。
 */
export function activeProfileIfListed(
  profiles: readonly { id: string }[],
): string | null {
  const active = activeCharacterId.value;
  if (active && profiles.some((profile) => profile.id === active)) return active;
  return null;
}
