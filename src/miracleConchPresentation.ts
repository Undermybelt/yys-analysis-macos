import type { ShikigamiPanel } from "./api/contracts";

/** 神奇海螺目标对象区的八项基础面板展示项。 */
export interface MiracleBasePanelItem {
  key: string;
  label: string;
  value: number;
  percent: boolean;
}

/** 目标输入使用玩家看到的数值；百分比字段转换为 0–100 的可编辑文本。 */
export type MiracleTargetInput = Record<
  "attack" | "hp" | "defense" | "speed" | "critRate" | "critDamage" | "effectHit" | "effectResist",
  string
>;

/**
 * 统一生成基础面板摘要，避免页面只拼接前四项属性。
 * 式神目录没有记录效果命中和效果抵抗，目标配装首版按静态默认值 0 展示；
 * 暴伤则把目录保存的额外值转换成包含基础 100% 的面板值。
 */
export function getMiracleBasePanelItems(panel: ShikigamiPanel): MiracleBasePanelItem[] {
  return [
    { key: "attack", label: "攻击", value: panel.attack, percent: false },
    { key: "hp", label: "生命", value: panel.hp, percent: false },
    { key: "defense", label: "防御", value: panel.defense, percent: false },
    { key: "speed", label: "速度", value: panel.speed, percent: false },
    { key: "critRate", label: "暴击", value: panel.critRate, percent: true },
    { key: "critDamage", label: "暴伤", value: 1 + panel.critDamage, percent: true },
    { key: "effectHit", label: "效果命中", value: 0, percent: true },
    { key: "effectResist", label: "效果抵抗", value: 0, percent: true },
  ];
}

/**
 * 将当前式神基础面板自动带入目标面板。
 * 目标仍是最低值，用户可以继续修改或清空任一字段；命中与抵抗没有目录基础值，固定从 0 开始。
 */
export function getMiracleTargetInputFromPanel(panel: ShikigamiPanel): MiracleTargetInput {
  const items = getMiracleBasePanelItems(panel);
  return Object.fromEntries(
    items.map((item) => [item.key, item.percent ? String(item.value * 100) : String(item.value)]),
  ) as MiracleTargetInput;
}

/** 把属性区间的可选上限转换为后端数字；空输入表示不限制。 */
export function parseOptionalMetricMaximum(value: unknown): number | null {
  // Vue 数字输入框会自动产出 number；本地恢复的旧配置则可能仍是 string，两种口径都必须保留。
  if (typeof value === "number") return value;
  const maximumText = typeof value === "string" ? value.trim() : "";
  return maximumText ? Number(maximumText) : null;
}
