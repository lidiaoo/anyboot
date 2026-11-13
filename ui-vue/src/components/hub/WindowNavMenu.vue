<script setup lang="ts">
/**
 * 独立窗（终端/文件/密钥/备份）统一的「回 hub 侧边栏导航」：
 * 一个 ⋮ 按钮，点开弹出与主 hub 相同的全局导航；选某项 → 回到 hub 并定位到该页。
 * - web：localStorage 记目标页 + location.href 跳 /hub.html?token=（HubApp 启动读 ab-hub-page 定位）
 * - Tauri：同样先写 localStorage，再 hub_open_window 唤/建 hub 窗口（其启动读同一键定位）
 */
import { ref, onMounted, onUnmounted } from "vue";

const NAV = [
  { id: "home", label: "电脑" },
  { id: "computers", label: "电脑列表" },
  { id: "servers", label: "服务器" },
  { id: "gateway", label: "远程接入" },
  { id: "term", label: "终端" },
  { id: "sessions", label: "会话" },
  { id: "transfers", label: "传输" },
  { id: "files", label: "文件" },
  { id: "keys", label: "密钥" },
  { id: "backup", label: "备份" },
  { id: "termset", label: "终端设置" },
  { id: "about", label: "关于" },
] as const;

const open = ref(false);
const rootEl = ref<HTMLElement | null>(null);

function toggle() {
  open.value = !open.value;
}

async function go(id: string) {
  open.value = false;
  try { localStorage.setItem("ab-hub-page", id); } catch { /* 忽略 */ }
  const { isTauri } = await import("@/api/tauri");
  if (!isTauri()) {
    const { getAuthToken } = await import("@/api/http");
    window.location.href = "/hub.html?token=" + encodeURIComponent(getAuthToken());
    return;
  }
  const { call } = await import("@/api/tauri");
  void call("hub_open_window", {}).catch(() => {});
}

function onDocClick(e: MouseEvent) {
  if (rootEl.value && !rootEl.value.contains(e.target as Node)) open.value = false;
}
onMounted(() => document.addEventListener("click", onDocClick, true));
onUnmounted(() => document.removeEventListener("click", onDocClick, true));
</script>

<template>
  <div class="wnm" ref="rootEl">
    <button
      type="button"
      class="wnm-btn"
      aria-label="导航"
      title="导航"
      :aria-expanded="open"
      @click.stop="toggle"
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
        <circle cx="12" cy="5" r="1.7" />
        <circle cx="12" cy="12" r="1.7" />
        <circle cx="12" cy="19" r="1.7" />
      </svg>
    </button>
    <div v-if="open" class="wnm-menu" role="menu" @click.stop>
      <button
        v-for="item in NAV"
        :key="item.id"
        type="button"
        role="menuitem"
        @click="go(item.id)"
      >
        {{ item.label }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.wnm { position: relative; }
.wnm-btn {
  display: inline-flex; align-items: center; justify-content: center;
  width: 28px; height: 26px; padding: 0; border: 0; border-radius: 7px;
  background: transparent; color: inherit; cursor: pointer;
}
.wnm-btn:hover { background: rgb(var(--elevate, 128 140 160) / .55); }
.wnm-menu {
  position: absolute; top: calc(100% + 6px); right: 0; z-index: 200;
  min-width: 148px; max-height: 70vh; overflow-y: auto;
  padding: 4px; border-radius: 10px;
  background: rgb(var(--card, 255 255 255)); border: 1px solid rgb(var(--line, 226 232 240));
  box-shadow: 0 10px 30px rgba(0, 0, 0, .22);
  display: flex; flex-direction: column;
}
.wnm-menu button {
  text-align: left; padding: 8px 10px; border: 0; border-radius: 7px;
  background: transparent; color: inherit; font-size: 13px; cursor: pointer;
}
.wnm-menu button:hover { background: rgb(var(--elevate, 128 140 160) / .55); }
</style>
