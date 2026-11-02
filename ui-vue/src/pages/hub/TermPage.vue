<script setup lang="ts">
const toast = useToastStore();
import { computed, onMounted, ref } from "vue";
import { useServersStore } from "@/stores/servers";
import { call, isTauri } from "@/api/tauri";
import { useToastStore } from "@/stores/toast";

/**
 * 终端页（原 `ui/hub.html` 的 `#page-term`，3355 行）。
 *
 * 1:1 结构（旧版就 4 个元素）：
 * ```
 * .act-card
 *   svg（终端图标）
 *   p.t     「终端」
 *   p.s     #termPageHint 提示（随是否选中 SSH 服务器变化）
 *   button.btn.primary  #pageTermBtn「打开终端」（没有服务器时 disabled）
 * ```
 */

const store = useServersStore();
const sshServers = computed(() => store.servers.filter((s) => s.protocol === "ssh"));
const selectedId = ref("");

onMounted(async () => {
  if (!store.loaded) await store.load();
  selectedId.value = sshServers.value[0]?.id ?? "";
});

const hint = computed(() =>
  sshServers.value.length
    ? `当前选中：${store.displayAddr(sshServers.value.find((s) => s.id === selectedId.value)!)}。点下方按钮打开终端（PTY + 系统 ssh，独立窗口）。`
    : "在「服务器」页选中一台 SSH 服务器后打开终端（PTY + 系统 ssh，独立窗口）。",
);

/** 按钮文案（旧版：有选中时「打开终端 · 名称」） */
const termBtnText = computed(() => {
  const s = sshServers.value.find((x) => x.id === selectedId.value);
  return s ? `打开终端 · ${s.name || "(未命名)"}` : "打开终端";
});

function openTerminal() {
  if (!isTauri()) {
    toast.show("终端需要应用环境", "bad");
    return;
  }
  if (!selectedId.value) {
    toast.show("先在「服务器」页选中一台 SSH 服务器", "bad");
    return;
  }
  // 旧版用的是专门的 hub_open_term_window（带 serverId 直接连）
  void call("hub_open_term_window", { serverId: selectedId.value }).catch((e) => toast.show("打开终端失败：" + e, "bad"));
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
      <rect x="2" y="4" width="20" height="14" rx="2" />
      <path d="M6 9l3 3-3 3" />
    </svg>
    <p class="t">终端</p>
    <p class="s" id="termPageHint">{{ hint }}</p>
    <button type="button" id="pageTermBtn" class="btn primary" :disabled="!selectedId" @click="openTerminal">
      {{ termBtnText }}
    </button>
  </div>
</template>
