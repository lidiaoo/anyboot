import { createApp } from "vue";
import { installI18n } from "@/i18n";
import { createPinia } from "pinia";
import Page from "@/pages/files/index.vue";
import "@/styles/win-files.css";

/**
 * hub-files.html 入口（独立窗）。
 *
 * Rust 侧用 `WebviewUrl::App("hub-files.html")` 打开 —— **文件名不可改**。
 */
const app = createApp(Page);
installI18n(app);
app.use(createPinia()).mount("#app");
