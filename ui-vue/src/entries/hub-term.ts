import { createApp } from "vue";
import { createPinia } from "pinia";
import Page from "@/pages/term/index.vue";
import "@/styles/win-term.css";

/**
 * hub-term.html 入口（独立窗）。
 *
 * Rust 侧用 `WebviewUrl::App("hub-term.html")` 打开 —— **文件名不可改**。
 */
createApp(Page).use(createPinia()).mount("#app");
