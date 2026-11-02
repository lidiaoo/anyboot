<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { call } from "@/api/tauri";

/**
 * SOL 远程唤醒页（原 `ui/index.html` 的 `#tab-sol`，835 行起）。
 *
 * ## 1:1 结构
 * ```
 * section.settings#sol-section
 *   p.sol-desc                    说明
 *   label.setting#sol-state-row   服务状态 + .remote-inline（刷新/特权/重启）
 *   label.setting#sol-http-enabled-row
 *   label.setting#sol-http-listen-row
 *   ...
 * ```
 */

interface SolStatus {
  installed?: boolean;
  running?: boolean;
  port?: number;
  projectPath?: string;
  version?: string;
  [key: string]: unknown;
}

const status = ref<SolStatus | null>(null);
const loading = ref(true);
const errorText = ref("");

const stateText = computed(() => {
  if (loading.value) return "正在读取…";
  if (!status.value?.installed) return "未安装";
  return status.value.running ? "运行中" : "已停止";
});

async function refresh() {
  loading.value = true;
  errorText.value = "";
  try {
    status.value = await call<SolStatus>("sol_status");
  } catch (error) {
    errorText.value = String(error);
    status.value = null;
  } finally {
    loading.value = false;
  }
}

onMounted(refresh);

function checkPrivileged() {
  void call("sol_status_privileged").catch(() => {});
}

function restartService() {
  void call("sol_project_open").catch(() => {});
}
</script>

<template>
  <section class="settings" id="sol-section">
    <h2>SOL 远程唤醒</h2>
    <p class="sol-desc">
      用服务器管理卡（BMC / IPMI）的 Serial-over-LAN 通道远程开机 —— 需要目标机的 BMC
      支持 SOL 且已配好网络。
    </p>

    <label class="setting" id="sol-state-row">
      <span class="setting-text">
        <strong>服务状态</strong>
        <small>SOL 辅助服务是否就绪</small>
        <small class="setting-state" id="sol-state-text">{{ stateText }}</small>
      </span>
      <span class="remote-inline">
        <button type="button" class="secondary" @click="refresh">刷新</button>
        <button type="button" class="secondary" @click="checkPrivileged">检查权限</button>
        <button type="button" class="secondary" @click="restartService">重启服务</button>
      </span>
    </label>

    <label class="setting" id="sol-http-enabled-row">
      <span class="setting-text">
        <strong>通过 HTTP 启用</strong>
        <small>允许手机 / 浏览器经本机 HTTP 服务发起 SOL 唤醒</small>
        <small class="setting-state" id="sol-http-enabled-state">{{ status?.running ? "已启用" : "未启用" }}</small>
      </span>
      <input type="checkbox" id="sol-http-enabled" :checked="Boolean(status?.running)" disabled />
    </label>

    <label class="setting" id="sol-http-listen-row">
      <span class="setting-text">
        <strong>监听地址</strong>
        <small>SOL 服务监听的地址与端口</small>
      </span>
      <span class="setting-state mono">{{ status?.port ? `:${status.port}` : "—" }}</span>
    </label>

    <p v-if="errorText" class="field-hint" style="color: rgb(var(--bad))">{{ errorText }}</p>
  </section>
</template>
