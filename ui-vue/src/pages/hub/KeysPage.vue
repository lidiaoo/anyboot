<script setup lang="ts">
import { onMounted, ref } from "vue";
import { call } from "@/api/tauri";

/**
 * 密钥页（原 `ui/hub.html` 的 `#page-keys`，3446 行）。
 *
 * 1:1 结构（就 4 个元素）：
 * ```
 * .act-card
 *   svg（钥匙图标）
 *   p.t  「密钥」
 *   p.s  #keysPageHint（密钥数量提示）
 *   button.btn.primary  #keysBtn「打开密钥管理」（带钥匙图标）
 * ```
 * 真正的密钥管理在独立窗（`hub-keys.html`），这里只是入口。
 */

interface KeyRow {
  id: string;
  name: string;
}

const keys = ref<KeyRow[]>([]);
const loading = ref(true);

onMounted(async () => {
  try {
    keys.value = (await call<KeyRow[]>("hub_keys_list")) ?? [];
  } catch {
    keys.value = [];
  } finally {
    loading.value = false;
  }
});

function openKeys() {
  void call("hub_open_window", { label: "server-keys" }).catch(() => {});
}
</script>

<template>
  <div class="act-card">
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.6"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <circle cx="8" cy="15" r="4" />
      <path d="M10.8 12.2 20 3M17 6l2.5 2.5" />
    </svg>
    <p class="t">密钥</p>
    <p class="s" id="keysPageHint">
      {{ loading ? "正在读取密钥列表…" : keys.length ? `本机已有 ${keys.length} 把 SSH 密钥` : "还没有 SSH 密钥 —— 在密钥管理里生成或导入一把" }}
    </p>
    <button type="button" id="keysBtn" class="btn primary" @click="openKeys">
      <svg
        width="15"
        height="15"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <circle cx="8" cy="15" r="4" />
        <path d="M10.8 12.2 20 3M17 6l2.5 2.5" />
      </svg>
      打开密钥管理
    </button>
  </div>
</template>
