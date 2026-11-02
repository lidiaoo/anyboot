import { createApp } from "vue";
import { createPinia } from "pinia";
import Page from "@/pages/files/index.vue";
import "@/styles/base.css";

/**
 * hub-files.html 入口（独立窗）。
 *
 * Rust 侧用 `WebviewUrl::App("hub-files.html")` 打开 —— **文件名不可改**。
 */
createApp(Page).use(createPinia()).mount("#app");
