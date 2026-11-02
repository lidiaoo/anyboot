import { createApp } from "vue";
import { createPinia } from "pinia";
import BootApp from "@/pages/boot/BootApp.vue";
import "@/styles/base.css";

/**
 * 主窗「系统引导」页入口（Tauri 默认入口 `index.html`）。
 */
createApp(BootApp).use(createPinia()).mount("#app");
