import { createApp } from "vue";
import { createPinia } from "pinia";
import BootApp from "@/pages/boot/BootApp.vue";
import "@/styles/main.css";

/**
 * 主窗「系统引导」页入口（Tauri 默认入口 `index.html`）。
 *
 * 样式：**只加载 `main.css`**（= 旧版 `styles.css`）。
 * 不要引入 `base.css` —— 它含 `hub-inline.css`，会把 body 字体改成 14px。
 */
createApp(BootApp).use(createPinia()).mount("#app");
