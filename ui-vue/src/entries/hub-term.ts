import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "@/App.vue";
import "@/styles/base.css";

/**
 * 入口：hub-term
 *
 * 阶段 0 只验证三件事：
 *   ① Vite 多入口能出产物且文件名保持原名
 *   ② Tauri 能在 tauri://localhost 下加载它
 *   ③ IPC（import { invoke } from "@tauri-apps/api/core"）能通
 * 真实页面在阶段 2 / 3 / 4 替换这个 App.vue。
 */
createApp(App).use(createPinia()).mount("#app");
