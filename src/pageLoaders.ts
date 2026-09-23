import type { Component } from "vue";

/** 延迟页面的稳定键；应用壳只保存键，不在首屏直接引用业务组件。 */
export type DeferredPageKey =
  | "analysisInbox"
  | "analysisCenter"
  | "soulCatalog"
  | "shikigamiCatalog"
  | "myShikigami"
  | "shardQuery"
  | "gameAssets"
  | "mySouls"
  | "ruleLibrary"
  | "guild"
  | "cbgRead"
  | "update"
  | "feedback"
  | "simulation"
  | "miracleConch"
  | "support";

type PageModule = { default: Component };

/**
 * 页面模块加载表只在用户离开首屏后才被动态加载。
 * 每个 loader 保持独立 import 边界，Vite 能将对应页面及其重型依赖拆成单独 chunk。
 */
export const deferredPageLoaders: Record<DeferredPageKey, () => Promise<PageModule>> = {
  analysisInbox: () => import("./components/AnalysisInboxView.vue"),
  analysisCenter: () => import("./components/AnalysisCenterView.vue"),
  soulCatalog: () => import("./components/SoulCatalogView.vue"),
  shikigamiCatalog: () => import("./components/ShikigamiCatalogView.vue"),
  myShikigami: () => import("./components/MyShikigamiView.vue"),
  shardQuery: () => import("./components/ShikigamiShardQueryView.vue"),
  gameAssets: () => import("./components/GameAssetsView.vue"),
  mySouls: () => import("./components/MySoulsView.vue"),
  ruleLibrary: () => import("./components/RuleLibraryView.vue"),
  guild: () => import("./components/GuildView.vue"),
  cbgRead: () => import("./components/CbgReadView.vue"),
  update: () => import("./components/UpdateView.vue"),
  feedback: () => import("./components/FeedbackView.vue"),
  simulation: () => import("./components/SimulationView.vue"),
  miracleConch: () => import("./components/MiracleConchView.vue"),
  support: () => import("./components/SupportView.vue"),
};
