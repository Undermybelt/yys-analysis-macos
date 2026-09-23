import { createApp } from "vue";
import App from "./App.vue";
import "./styles.css";

// 应用入口只负责创建 Vue 实例；具体业务页面将在后续按功能逐步挂载。
createApp(App).mount("#app");
