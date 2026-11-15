import { createApp } from "vue";
import { installI18n } from "@/i18n";
import { createPinia } from "pinia";
import HubApp from "@/pages/hub/HubApp.vue";
import "@/styles/base.css";

/**
 * hub 入口（管理界面）。
 *
 * Rust 侧 `lib.rs:1042` 用 `WebviewUrl::App("hub.html")` 打开它 ——
 * **文件名不可改**。
 */
const app = createApp(HubApp);
installI18n(app);
app.use(createPinia()).mount("#app");
