import { createApp } from "vue";
import { createPinia } from "pinia";
import Page from "@/pages/keys/index.vue";
import "@/styles/base.css";

/**
 * hub-keys.html 入口（独立窗）。
 *
 * Rust 侧用 `WebviewUrl::App("hub-keys.html")` 打开 —— **文件名不可改**。
 */
createApp(Page).use(createPinia()).mount("#app");
