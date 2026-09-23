/// <reference types="vite/client" />

declare module "*.vue" {
  import type { DefineComponent } from "vue";

  // 为 ESLint 的类型感知解析补充单文件组件模块类型。
  const component: DefineComponent;
  export default component;
}

// 让 Vite 打包随新版桌面版提供的 ICO 图标时，TypeScript 能识别其 URL 模块类型。
declare module "*.ico" {
  const src: string;
  export default src;
}
