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
  // 默认选第一台 SSH 服务器（有就直接可开终端）
  if (!selectedId.value && sshServers.value.length) selectedId.value = sshServers.value[0].id;
});

const hint = computed(() => {
  if (!sshServers.value.length) return "还没有 SSH 服务器。先到「服务器」页添加一台，再回来打开终端。";
  const s = sshServers.value.find((x) => x.id === selectedId.value);
  if (!s) return "选一台 SSH 服务器后点「打开终端」（PTY + 系统 ssh，独立窗口）。";
  return `当前选中：${store.displayAddr(s)}。点下方按钮打开终端（PTY + 系统 ssh，独立窗口）。`;
});

/** 按钮文案（旧版：有选中时「打开终端 · 名称」） */
const termBtnText = computed(() => {
  const s = sshServers.value.find((x) => x.id === selectedId.value);
  return s ? `打开终端 · ${s.name || "(未命名)"}` : "打开终端";
});

function goServers() {
  // 切到左侧「服务器」页（模拟点导航，HubApp 用 currentPage 管切页）
  const btn = document.querySelector<HTMLElement>('.snav[data-page=servers]');
  btn?.click();
}

async function openTerminal() {
  if (!isTauri()) {
    // 网页端：本机 HTTP 已提供 hub-term.html；带 token + serverId 在新标签打开
    if (!selectedId.value) { toast.show("先在下方选一台 SSH 服务器", "bad"); return; }
    try {
      const { getAuthToken } = await import("@/api/http");
      const tok = encodeURIComponent(getAuthToken());
      window.open(`/hub-term.html?token=${tok}&serverId=${encodeURIComponent(selectedId.value)}`, "_blank");
    } catch { /* ignore */ }
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

    <!-- 空状态：还没有任何 SSH 服务器 -->
    <template v-if="!sshServers.length">
      <p class="t">还没有 SSH 服务器</p>
      <p class="s" id="termPageHint">终端需要先添加一台 SSH 服务器。到左侧「服务器」页点「添加服务器」配好地址 / 账号 / 密码或密钥后，再回来打开终端。</p>
      <button type="button" class="btn" @click="goServers">去「服务器」页添加</button>
    </template>

    <!-- 有服务器：选一台 + 打开 -->
    <template v-else>
      <p class="t">终端</p>
      <select
        class="term-server-select"
        aria-label="选择要连接的 SSH 服务器"
        :value="selectedId"
        @change="selectedId = ($event.target as HTMLSelectElement).value"
      >
        <option v-for="s in sshServers" :key="s.id" :value="s.id">
          {{ s.name || "(未命名)" }} · {{ store.displayAddr(s) }}
        </option>
      </select>
      <p class="s" id="termPageHint">{{ hint }}</p>
      <button type="button" id="pageTermBtn" class="btn primary" :disabled="!selectedId" @click="openTerminal">
        {{ termBtnText }}
      </button>
    </template>
  </div>
</template>
