import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { resolve } from "node:path";

/**
 * Vue 迁移的多入口构建。
 *
 * **入口文件名必须与 Rust 侧 `WebviewUrl::App("xxx.html")` 里写死的名字一致** ——
 * 窗口是运行时创建的（tauri.conf.json 的 windows 是空数组），改文件名就得改 Rust：
 *   hub.html       → lib.rs:1042
 *   hub-keys.html  → lib.rs:1073
 *   hub-backup.html→ lib.rs:1125
 *   hub-files.html → hub_sftp.rs:1160
 *   hub-term.html  → hub_term.rs:123
 *   index.html     → 主窗「系统引导」页（Tauri 默认入口）
 *
 * 迁移完成后这些名字**保持不变**，只是从 `ui/` 换成 `ui-vue/dist/`。
 */
export default defineConfig({
  // 以 ui-vue/ 为根：npm script 从仓库根调用时，产物才会稳定落到 ui-vue/dist
  root: __dirname,
  plugins: [vue()],
  resolve: {
    alias: {
      "@": resolve(__dirname, "src"),
    },
  },
  // 相对路径：Tauri 在 tauri://localhost 下按目录服务资源
  base: "./",
  build: {
    outDir: "dist",
    // outDir 相对 root（= ui-vue/）
    emptyOutDir: true,
    // Tauri 用的 WebView 现代（WKWebView / WebView2 / WebKitGTK），可以放心用新语法
    target: "es2021",
    sourcemap: false,
    rollupOptions: {
      input: {
        index: resolve(__dirname, "index.html"),
        hub: resolve(__dirname, "hub.html"),
        "hub-term": resolve(__dirname, "hub-term.html"),
        "hub-files": resolve(__dirname, "hub-files.html"),
        "hub-backup": resolve(__dirname, "hub-backup.html"),
        "hub-keys": resolve(__dirname, "hub-keys.html"),
      },
      output: {
        // 入口保持原名（hub.js / hub-term.js …），chunk 与静态资源分开，
        // 方便排查「哪个页面的包出了问题」
        // 入口也带 content hash：固定名（assets/index.js）会被浏览器 / WebView 永久
        // 缓存，更新后用户仍在跑旧 bundle（真实踩坑：改了界面网页却一直显示旧版）。
        // HTML 由 vite 生成、自动引用新名；Rust 侧 ui_assets 表跟着 gen 脚本走。
        entryFileNames: "assets/[name]-[hash].js",
        chunkFileNames: "assets/chunks/[name]-[hash].js",
        assetFileNames: "assets/[name]-[hash][extname]",
      },
    },
  },
  server: {
    port: 5173,
    strictPort: true,
  },
});
