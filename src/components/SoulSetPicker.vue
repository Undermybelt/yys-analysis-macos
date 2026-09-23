<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import type { SoulSet } from "../api/contracts";

/** 选择器只依赖套装的展示字段，允许调用方传入目录套装或带“未匹配”占位项。 */
export type SoulSetOption = Pick<
  SoulSet,
  "setId" | "name" | "iconAssetId" | "category" | "specialCategory"
>;

const props = withDefaults(
  defineProps<{
    modelValue: string;
    options: SoulSetOption[];
    emptyLabel?: string;
    emptyValue?: string;
    disabled?: boolean;
    ariaLabel?: string;
    /** 外层已经完成分类筛选时关闭内部分类栏，避免同一行出现两套分类控件。 */
    showCategories?: boolean;
  }>(),
  {
    emptyLabel: "全部御魂",
    emptyValue: "",
    disabled: false,
    ariaLabel: "选择御魂",
    showCategories: true,
  },
);

const emit = defineEmits<{
  "update:modelValue": [value: string];
}>();

const iconModules = import.meta.glob("../assets/soul-icons/*.png", {
  eager: true,
  import: "default",
  query: "?url",
});
const root = ref<HTMLElement | null>(null);
const open = ref(false);
const activeCategory = ref("all");

const selectedOption = computed(
  () => props.options.find((option) => option.setId === props.modelValue) ?? null,
);

/** 分类优先于具体套装，减少长列表扫描；特殊类别优先显示首领/星痕等实际效果分类。 */
const categories = computed(() => {
  const values = new Set<string>();
  for (const option of props.options) {
    const category = option.specialCategory ?? option.category;
    if (category) values.add(category);
  }
  return Array.from(values)
    .sort((left, right) => left.localeCompare(right, "zh-CN"))
    .map((category) => ({ key: category, label: category }));
});

const visibleOptions = computed(() =>
  props.options.filter((option) => {
    // 神奇海螺等页面已经在外层完成一级分类，此时直接显示传入的具体套装。
    if (!props.showCategories) return true;
    // 两级选择器未点分类前不展示具体御魂，避免打开菜单就加载超长列表。
    if (activeCategory.value === "none") return false;
    return (option.specialCategory ?? option.category) === activeCategory.value;
  }),
);

/** 从统一离线资源表返回套装图标；缺图时由模板显示文字首字。 */
function soulIcon(option: SoulSetOption): string | null {
  if (!option.iconAssetId) return null;
  return iconModules[`../assets/soul-icons/${option.iconAssetId}.png`] ?? null;
}

/** 提交选项并关闭菜单；选择行为只修改外部 v-model，不触碰业务查询逻辑。 */
function selectOption(value: string): void {
  emit("update:modelValue", value);
  open.value = false;
}

/** 打开面板时优先定位到当前御魂所属分类，用户仍可切回全部效果。 */
function toggleOpen(): void {
  if (open.value) {
    open.value = false;
    return;
  }
  activeCategory.value = !props.showCategories
    ? "all"
    : selectedOption.value
      ? selectedOption.value.specialCategory ?? selectedOption.value.category ?? "none"
      : "none";
  open.value = true;
}

/** 点击控件外部时关闭面板，避免菜单持续覆盖页面内容。 */
function handlePointerDown(event: PointerEvent): void {
  if (root.value && !root.value.contains(event.target as Node)) {
    open.value = false;
  }
}

/** 键盘 Escape 与鼠标点击拥有相同的关闭语义，保证选择器可被键盘正常退出。 */
function handleKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape") open.value = false;
}

onMounted(() => {
  document.addEventListener("pointerdown", handlePointerDown);
  document.addEventListener("keydown", handleKeydown);
});

onBeforeUnmount(() => {
  document.removeEventListener("pointerdown", handlePointerDown);
  document.removeEventListener("keydown", handleKeydown);
});
</script>

<template>
  <div ref="root" class="soul-set-picker">
    <button
      type="button"
      class="soul-set-picker__trigger"
      :disabled="disabled"
      :aria-label="ariaLabel"
      :aria-expanded="open"
      aria-haspopup="listbox"
      @click="toggleOpen"
    >
      <span v-if="selectedOption" class="soul-set-picker__selected">
        <img
          v-if="soulIcon(selectedOption)"
          :src="soulIcon(selectedOption) ?? undefined"
          :alt="`${selectedOption.name}图标`"
        />
        <span v-else class="soul-set-picker__fallback" aria-hidden="true">{{ selectedOption.name.slice(0, 1) }}</span>
        <span class="soul-set-picker__label">{{ selectedOption.name }}</span>
      </span>
      <span v-else class="soul-set-picker__label">{{ emptyLabel }}</span>
      <span class="soul-set-picker__chevron" aria-hidden="true">⌄</span>
    </button>

    <div v-if="open" class="soul-set-picker__menu" role="listbox" :aria-label="ariaLabel">
      <div v-if="showCategories" class="soul-set-picker__categories" aria-label="选择套装效果分类">
        <span class="soul-set-picker__category-title">先选套装效果</span>
        <button
          v-for="category in categories"
          :key="category.key"
          type="button"
          class="soul-set-picker__category"
          :class="{ 'soul-set-picker__category--active': activeCategory === category.key }"
          @click="activeCategory = category.key"
        >
          {{ category.label }}
        </button>
      </div>
      <button
        type="button"
        class="soul-set-picker__option"
        :class="{ 'soul-set-picker__option--selected': modelValue === emptyValue }"
        role="option"
        :aria-selected="modelValue === emptyValue"
        @click="selectOption(emptyValue)"
      >
        <span class="soul-set-picker__label">{{ emptyLabel }}</span>
      </button>
      <button
        v-for="option in visibleOptions"
        :key="option.setId"
        type="button"
        class="soul-set-picker__option"
        :class="{ 'soul-set-picker__option--selected': modelValue === option.setId }"
        role="option"
        :aria-selected="modelValue === option.setId"
        @click="selectOption(option.setId)"
      >
        <img v-if="soulIcon(option)" :src="soulIcon(option) ?? undefined" :alt="`${option.name}图标`" />
        <span v-else class="soul-set-picker__fallback" aria-hidden="true">{{ option.name.slice(0, 1) }}</span>
        <span class="soul-set-picker__label">{{ option.name }}</span>
        <span v-if="modelValue === option.setId" class="soul-set-picker__check" aria-hidden="true">✓</span>
      </button>
      <p v-if="showCategories && activeCategory === 'none'" class="soul-set-picker__empty">请选择套装效果</p>
      <p v-else-if="!visibleOptions.length" class="soul-set-picker__empty">该效果分类暂无可选御魂</p>
    </div>
  </div>
</template>

<style scoped>
.soul-set-picker { position: relative; min-width: 0; width: 100%; }
.soul-set-picker__trigger { display: flex; width: 100%; min-width: 0; height: 34px; align-items: center; gap: 7px; padding: 0 9px; border: 1px solid var(--line, #dce1e8); border-radius: 6px; color: var(--ink, #34414d); background: #fff; font: inherit; text-align: left; cursor: pointer; }
.soul-set-picker__trigger:hover:not(:disabled), .soul-set-picker__trigger[aria-expanded="true"] { border-color: var(--accent-deep, #4799ed); box-shadow: 0 0 0 2px rgb(90 171 255 / 12%); }
.soul-set-picker__trigger:focus-visible { border-color: var(--accent-deep, #4799ed); outline: 0; box-shadow: 0 0 0 3px rgb(90 171 255 / 18%); }
.soul-set-picker__trigger:disabled { cursor: not-allowed; opacity: 0.55; }
.soul-set-picker__selected { display: flex; min-width: 0; align-items: center; gap: 7px; }
.soul-set-picker__trigger img, .soul-set-picker__option img { width: 22px; height: 22px; flex: 0 0 22px; object-fit: contain; }
.soul-set-picker__fallback { display: inline-grid; width: 22px; height: 22px; flex: 0 0 22px; place-items: center; border-radius: 50%; color: #fff; background: #9ca9b5; font-size: 11px; font-weight: 700; }
.soul-set-picker__label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.soul-set-picker__chevron { margin-left: auto; color: #9ba5af; font-size: 17px; line-height: 1; transform: translateY(-2px); }
.soul-set-picker__menu { position: absolute; top: calc(100% + 6px); right: 0; left: 0; z-index: 50; max-height: 300px; overflow-y: auto; padding: 5px; border: 1px solid var(--line, #dce1e8); border-radius: 8px; background: #fff; box-shadow: 0 12px 28px rgb(46 62 78 / 16%); }
.soul-set-picker__categories { display: flex; flex-wrap: wrap; gap: 4px; margin: 1px 1px 5px; padding-bottom: 6px; border-bottom: 1px solid #edf0f3; }
.soul-set-picker__category-title { width: 100%; padding: 2px 7px 1px; color: var(--ink-soft, #84909b); font-size: 11px; }
.soul-set-picker__category { padding: 5px 7px; border: 0; border-radius: 5px; color: var(--ink-soft, #71808c); background: transparent; font-size: 11px; cursor: pointer; }
.soul-set-picker__category:hover, .soul-set-picker__category--active { color: var(--accent-deep, #2f78bb); background: #eff7ff; }
.soul-set-picker__option { display: flex; width: 100%; min-height: 34px; align-items: center; gap: 7px; padding: 5px 7px; border: 0; border-radius: 5px; color: var(--ink-soft, #596773); background: transparent; font: inherit; text-align: left; cursor: pointer; }
.soul-set-picker__option:hover, .soul-set-picker__option--selected { color: var(--accent-deep, #2f78bb); background: #eff7ff; }
.soul-set-picker__check { margin-left: auto; color: #4d9ee9; font-weight: 800; }
.soul-set-picker__empty { margin: 8px; color: var(--ink-soft, #84909b); font-size: 12px; text-align: center; }
</style>
