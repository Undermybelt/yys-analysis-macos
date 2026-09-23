import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// Tauri 在开发期需要固定端口，以便壳层稳定连接前端服务。
const DEV_SERVER_PORT = 1420;

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: DEV_SERVER_PORT,
    strictPort: true,
    // Rust 构建产物、打包资源和诊断临时目录不参与前端热更新；排除这些大目录可避免 chokidar 启动时递归扫描数万文件。
    watch: {
      ignored: [
        "**/target/**",
        "**/res/**",
        "**/release/**",
        "**/.backup/**",
        "**/.scratch/**",
      ],
    },
  },
  // 入口页面已按功能懒加载；关闭自动依赖发现，避免 Vite 启动时递归扫描全部业务页面。
  optimizeDeps: {
    noDiscovery: true,
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: "es2022",
    // 关闭小图片内联，避免 Tauri CSP 拦截 data: 图片，确保所有御魂图标都作为本地资源打包。
    assetsInlineLimit: 0,
    // Vite 8 默认使用内置 Oxc 压缩；调试包关闭压缩以保留可读堆栈。
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG),
  },
});
