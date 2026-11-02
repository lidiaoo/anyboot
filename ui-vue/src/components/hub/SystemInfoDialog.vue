<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import { call } from "@/api/tauri";

/**
 * 系统信息弹层（原 `ui/hub.html` 的 `openSysInfo()`）。
 *
 * 点电脑名旁的系统徽标打开，显示那台机的 `remote_system_info`：
 * 系统族 / 版本 / 内核 / 架构 / 主机名 / CPU / 内存 / 开机时长。
 */

interface SystemInfo {
  os: string;
  osName: string;
  osVersion: string;
  kernel: string;
  arch: string;
  hostname: string;
  cpuModel: string;
  cpuCores: number;
  memTotalMb: number;
  memFreeMb: number;
  uptimeSec: number;
}

const props = defineProps<{ serverId: string | null; title: string }>();
const emit = defineEmits<{ (e: "close"): void }>();

const info = ref<SystemInfo | null>(null);
const loading = ref(false);
const errorText = ref("");

watch(
  () => props.serverId,
  async (id) => {
    info.value = null;
    errorText.value = "";
    if (!id) return;
    loading.value = true;
    try {
      const result = await call<Record<string, unknown>>("remote_system_info", { serverId: id });
      // 后端用 camelCase 序列化，这里直接透传
      info.value = result as unknown as SystemInfo;
    } catch (error) {
      errorText.value = String(error);
    } finally {
      loading.value = false;
    }
  },
  { immediate: true },
);

onMounted(() => {
  /* watch immediate 已负责首次加载 */
});

function fmtMem(mb: number): string {
  if (!mb) return "—";
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${mb} MB`;
}

function fmtUptime(sec: number): string {
  if (!sec) return "—";
  const days = Math.floor(sec / 86400);
  const hours = Math.floor((sec % 86400) / 3600);
  const mins = Math.floor((sec % 3600) / 60);
  if (days) return `${days} 天 ${hours} 小时`;
  if (hours) return `${hours} 小时 ${mins} 分`;
  return `${mins} 分`;
}
</script>

<template>
  <Teleport to="body">
    <div v-if="serverId" class="backdrop" @click.self="emit('close')">
      <div class="dialog" role="dialog" aria-modal="true">
        <header>
          <h2>{{ title }} · 系统信息</h2>
          <button type="button" class="btn icon ghost sm" @click="emit('close')">✕</button>
        </header>

        <div class="body">
          <p v-if="loading" class="hint">读取中…</p>
          <p v-else-if="errorText" class="err">{{ errorText }}</p>
          <dl v-else-if="info">
            <dt>系统</dt>
            <dd>{{ info.osName || info.os || "—" }}</dd>
            <dt>版本</dt>
            <dd class="mono">{{ info.osVersion || "—" }}</dd>
            <dt>内核</dt>
            <dd class="mono">{{ info.kernel || "—" }}</dd>
            <dt>架构</dt>
            <dd class="mono">{{ info.arch || "—" }}</dd>
            <dt>主机名</dt>
            <dd class="mono">{{ info.hostname || "—" }}</dd>
            <dt>处理器</dt>
            <dd>{{ info.cpuModel || "—" }}<span v-if="info.cpuCores"> · {{ info.cpuCores }} 核</span></dd>
            <dt>内存</dt>
            <dd>
              {{ fmtMem(info.memTotalMb) }}
              <span v-if="info.memFreeMb" class="dim">（可用 {{ fmtMem(info.memFreeMb) }}）</span>
            </dd>
            <dt>运行时长</dt>
            <dd>{{ fmtUptime(info.uptimeSec) }}</dd>
          </dl>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.backdrop {
  position: fixed;
  inset: 0;
  background: rgba(15, 23, 42, 0.35);
  display: grid;
  place-items: center;
  z-index: 80;
  padding: 20px;
}
.dialog {
  width: min(460px, 100%);
  background: var(--panel, #fff);
  border-radius: 12px;
  overflow: hidden;
  box-shadow: 0 20px 50px rgba(15, 23, 42, 0.25);
}
header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 13px 18px;
  border-bottom: 1px solid var(--line, #e5e7eb);
}
h2 {
  font-size: 14px;
  margin: 0;
}
.body {
  padding: 16px 18px;
}
dl {
  display: grid;
  grid-template-columns: 78px 1fr;
  gap: 8px 14px;
  font-size: 12.5px;
  margin: 0;
}
dt {
  color: var(--fg-faint, #9ca3af);
}
dd {
  margin: 0;
  overflow-wrap: anywhere;
}
.mono {
  font-family: ui-monospace, Menlo, monospace;
  font-size: 11.5px;
}
.dim {
  color: var(--fg-faint, #9ca3af);
}
.hint {
  font-size: 12.5px;
  color: var(--fg-faint, #9ca3af);
  margin: 0;
}
.err {
  font-size: 12.5px;
  color: #dc2626;
  margin: 0;
}
</style>
