<script setup lang="ts">
import { onMounted, ref, watch , computed } from "vue";
import { t } from "@/i18n";
import { call } from "@/api/tauri";
import { useEscape } from "@/composables/useEscape";

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
  uptimeSecs?: number;
  lanIps?: string[];
  httpPort?: number;
  appVersion?: string;
}

const props = defineProps<{ serverId: string | null; title: string }>();
const emit = defineEmits<{ (e: "close"): void }>();
useEscape(computed(() => Boolean(props.serverId)), () => emit('close'));

const info = ref<SystemInfo | null>(null);
const OS_LABELS: Record<string, string> = { win: "Windows", mac: "macOS", linux: "Linux" };
const osLabel = computed(() => {
  const i = info.value;
  if (!i) return "—";
  const base = OS_LABELS[i.os] || i.os || "—";
  return i.osName ? `${base} (${i.osName})` : base;
});
const ipsText = computed(() => {
  const ips = info.value?.lanIps?.filter(Boolean) ?? [];
  const port = info.value?.httpPort;
  return (ips.length ? ips.join(" · ") : "") + (port ? " · HTTP " + port : " · " + t("sysinfo.httpNotListening"));
});
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

function copyAll() {
  const i = info.value;
  if (!i) return;
  const text = [
    `${props.title}`,
    t("sysinfo.copyOs", { v: i.osName || i.os || "—" }),
    t("sysinfo.copyVersion", { v: i.osVersion || "—" }),
    t("sysinfo.copyKernel", { v: i.kernel || "—" }),
    t("sysinfo.copyArch", { v: i.arch || "—" }),
    t("sysinfo.copyHost", { v: i.hostname || "—" }),
    t("sysinfo.copyCpu", { v: i.cpuModel || "—", n: i.cpuCores || 0 }),
    t("sysinfo.copyMem", { v: fmtMem(i.memTotalMb) }),
    t("sysinfo.copyUptime", { v: fmtUptime(i.uptimeSec) }),
  ].join("\n");
  void navigator.clipboard.writeText(text).catch(() => {});
}

function fmtMem(mb: number): string {
  if (!mb) return "—";
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${mb} MB`;
}

function fmtUptime(sec: number): string {
  if (!sec) return "—";
  const days = Math.floor(sec / 86400);
  const hours = Math.floor((sec % 86400) / 3600);
  const mins = Math.floor((sec % 3600) / 60);
  if (days) return t("sysinfo.daysHours", { d: days, h: hours });
  if (hours) return t("sysinfo.hoursMins", { h: hours, m: mins });
  return t("sysinfo.minsText", { m: mins });
}
</script>

<template>
  <div v-if="props.serverId" id="sysInfoOverlay" class="overlay" @click.self="emit('close')">
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="sysInfoTitle">
      <div class="dialog-head">
        <h3 id="sysInfoTitle">{{ t("sysinfo.title") }}</h3>
        <button type="button" class="icon-btn" id="sysInfoClose" :aria-label="t('common.close')" @click="emit('close')">✕</button>
      </div>
      <div class="dialog-body">
        <p v-if="loading" class="field-hint">{{ t("hub.state.reading") }}</p>
        <p v-else-if="errorText" class="form-error">{{ errorText }}</p>
        <template v-else-if="info">
          <div class="sysinfo-head" id="sysInfoHead">
            <span class="who">{{ info.hostname || props.title || t("sysinfo.local") }}</span>
            <span class="where">{{ ipsText }}</span>
          </div>
          <div class="sysinfo-rows" id="sysInfoRows">
            <div><span>{{ t("sysinfo.os") }}</span><span>{{ osLabel }}</span></div>
            <div><span>{{ t("sysinfo.osVersion") }}</span><span class="mono">{{ info.osVersion || "—" }}</span></div>
            <div><span>{{ t("sysinfo.kernelBuild") }}</span><span class="mono">{{ info.kernel || "—" }}</span></div>
            <div><span>{{ t("sysinfo.arch") }}</span><span class="mono">{{ info.arch || "—" }}</span></div>
            <div><span>{{ t("sysinfo.hostname") }}</span><span class="mono">{{ info.hostname || "—" }}</span></div>
            <div><span>CPU</span><span>{{ info.cpuModel || "—" }}</span></div>
            <div><span>{{ t("sysinfo.cores") }}</span><span>{{ info.cpuCores ? t("sysinfo.logicCores", { n: info.cpuCores }) : "—" }}</span></div>
            <div><span>{{ t("sysinfo.mem") }}</span><span>{{ fmtMem(info.memTotalMb) }}<template v-if="info.memFreeMb">{{ t("sysinfo.memFree", { size: fmtMem(info.memFreeMb) }) }}</template></span></div>
            <div><span>{{ t("sysinfo.uptime") }}</span><span>{{ fmtUptime(info.uptimeSecs ?? info.uptimeSec) }}</span></div>
            <div><span>AnyBoot</span><span>{{ info.appVersion ? "v" + info.appVersion : "—" }}</span></div>
          </div>
        </template>
      </div>
      <div class="dialog-foot">
        <button type="button" class="btn" id="sysInfoCopy" @click="copyAll">{{ $t("common.copy") }}</button>
        <button type="button" class="btn primary" id="sysInfoOk" @click="emit('close')">{{ $t("common.close") }}</button>
      </div>
    </div>
  </div>
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
