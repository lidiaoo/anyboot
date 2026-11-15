import { createApp } from "vue";
import { installI18n } from "@/i18n";
import { createPinia } from "pinia";
import BootApp from "@/pages/boot/BootApp.vue";
import "@/styles/main.css";

/**
 * 主窗「系统引导」页入口（Tauri 默认入口 `index.html`）。
 *
 * 样式：**只加载 `main.css`**（= 旧版 `styles.css`）。
 * 不要引入 `base.css` —— 它含 `hub-inline.css`，会把 body 字体改成 14px。
 */
const app = createApp(BootApp);
installI18n(app);
app.use(createPinia()).mount("#app");
