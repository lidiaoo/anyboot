<script setup lang="ts">
import { computed } from "vue";
import { useServersStore } from "@/stores/servers";
import { call } from "@/api/tauri";

/**
 * 终端页（原 `ui/hub.html` 的 `#page-term`）。
 *
 * 它本身不渲染终端 —— 终端是**独立窗口**（`hub-term.html`，PTY + 系统 ssh + xterm）。
 * 这里只负责：选中一台 SSH 服务器后把终端窗开起来。
 */

const store = useServersStore();
const sshServers = computed(() => store.servers.filter((s) => s.protocol === "ssh"));

async function openTerminal(serverId: string) {
  try {
    await call("hub_open_window", { label: "server-term", serverId });
  } catch {
    /* 打开失败不阻塞页面 */
  }
}
</script>

<template>
  <div class="act-page">
    <div class="act-card">
      <div class="big" aria-hidden="true">▮</div>
      <p class="t">终端</p>
      <p class="s">在「服务器」页选中一台 SSH 服务器后打开终端（PTY + 系统 ssh，独立窗口）。</p>

      <div v-if="sshServers.length" class="server-picks">
        <button
          v-for="server in sshServers"
          :key="server.id"
          type="button"
          class="btn primary"
          @click="openTerminal(server.id)"
        >
          打开「{{ server.name }}」的终端
        </button>
      </div>
      <button v-else type="button" class="btn primary" disabled>打开终端</button>
    </div>
  </div>
</template>

<style scoped>
.act-page {
  display: grid;
  place-items: center;
  min-height: 60vh;
  padding: 20px;
}
.act-card {
  text-align: center;
  max-width: 420px;
}
.big {
  font-size: 30px;
  color: var(--fg-faint, #9ca3af);
  margin-bottom: 10px;
}
.t {
  font-size: 15px;
  font-weight: 600;
  margin: 0 0 6px;
}
.s {
  font-size: 12.5px;
  color: var(--fg-faint, #9ca3af);
  margin: 0 0 16px;
  line-height: 1.7;
}
.server-picks {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
</style>
