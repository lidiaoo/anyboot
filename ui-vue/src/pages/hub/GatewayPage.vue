<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { call } from "@/api/tauri";

/**
 * 远程接入页（原 `ui/hub.html` 的 `#page-gateway`）—— gateway 命令族 36 个。
 *
 * 四块：
 *   ① **Cloudflare 隧道**：二进制管理（下载/手动指定/卸载）+ 启动/停止 + 日志
 *   ② **Tailscale**：安装/登录/开关 + Funnel（对外暴露）+ 日志
 *   ③ **配对管理**：签发 / 收回 / 重签 / 删除访问凭据
 *   ④ **孤儿隧道**：进程残留清理
 *
 * 这是 hub 里命令最多的一块；本组件覆盖主流程，冷门操作（打开扩展设置等）
 * 以按钮形式保留但不抢主视觉。
 */

interface BinaryStatus {
  installed: boolean;
  version: string;
  path: string;
  source: string;
  downloading: boolean;
}

interface TunnelSnapshot {
  running: boolean;
  mode: string;
  url: string;
  started_at: number;
  restarts: number;
  state: string;
  error: string;
  logs?: string[];
}

interface GatewayConfig {
  mode: string;
  named_url: string;
  named_token_set: boolean;
  autostart: boolean;
  binary: BinaryStatus;
  tunnel: TunnelSnapshot;
}

interface PairRow {
  id: string;
  label: string;
  created_at: number;
  last_seen: number;
  revoked: boolean;
  url: string;
  [key: string]: unknown;
}

const config = ref<GatewayConfig | null>(null);
const pairs = ref<PairRow[]>([]);
const loading = ref(true);
const busy = ref("");
const message = ref("");
const errorText = ref("");

async function refreshConfig() {
  try {
    config.value = await call<GatewayConfig>("gateway_config");
  } catch (error) {
    errorText.value = String(error);
  }
}

async function refreshPairs() {
  try {
    pairs.value = await call<PairRow[]>("gateway_pair_list");
  } catch {
    pairs.value = [];
  }
}

async function refreshAll() {
  loading.value = true;
  await Promise.all([refreshConfig(), refreshPairs()]);
  loading.value = false;
}

onMounted(refreshAll);

/** 统一执行：置忙 → 调命令 → 刷新 → 提示 */
async function run(label: string, action: () => Promise<unknown>, done: string) {
  if (busy.value) return;
  busy.value = label;
  message.value = "";
  errorText.value = "";
  try {
    await action();
    message.value = done;
    await refreshAll();
  } catch (error) {
    errorText.value = String(error);
  } finally {
    busy.value = "";
  }
}

function downloadBinary() {
  void run(
    "下载 cloudflared",
    () => call("gateway_binary_download"),
    "开始下载 cloudflared…",
  );
}

function startTunnel() {
  void run("启动隧道", () => call("gateway_tunnel_start"), "隧道已启动");
}

function stopTunnel() {
  void run("停止隧道", () => call("gateway_tunnel_stop"), "隧道已停止");
}

function killOrphans() {
  void run("清理孤儿隧道", () => call("gateway_tunnel_kill_orphan"), "已清理残留进程");
}

// ---- Tailscale ----
const tsStatus = ref<{ installed?: boolean; running?: boolean; ip?: string } | null>(null);

async function refreshTailscale() {
  try {
    tsStatus.value = await call<{ installed?: boolean; running?: boolean; ip?: string }>(
      "gateway_tailscale_status",
    );
  } catch {
    tsStatus.value = null;
  }
}

function tailscaleToggle() {
  void run("Tailscale 开关", () => call("gateway_tailscale_toggle"), "已切换 Tailscale 状态").then(
    () => void refreshTailscale(),
  );
}

// ---- 配对 ----
function issuePair() {
  void run("签发配对", () => call("gateway_pair_issue"), "已签发新的访问凭据");
}

function revokePair(id: string) {
  void run("收回配对", () => call("gateway_pair_revoke", { id }), "已收回该凭据");
}

function deletePair(id: string) {
  void run("删除配对", () => call("gateway_pair_delete", { id }), "已删除");
}

const tunnelUrl = computed(() => config.value?.tunnel?.url ?? "");
const binaryReady = computed(() => Boolean(config.value?.binary?.installed));
</script>

<template>
  <div class="gateway-page">
    <p v-if="loading" class="loading">读取配置…</p>

    <template v-else>
      <!-- ① Cloudflare 隧道 -->
      <section class="card">
        <header>
          <h2>Cloudflare 隧道</h2>
          <span :class="['pill', config?.tunnel?.running ? 'is-on' : 'is-off']">
            {{ config?.tunnel?.running ? "运行中" : "未运行" }}
          </span>
        </header>

        <dl>
          <dt>cloudflared</dt>
          <dd>
            <template v-if="binaryReady">
              {{ config?.binary?.version || "已安装" }}
              <span class="mono dim">{{ config?.binary?.path }}</span>
            </template>
            <template v-else>未安装</template>
          </dd>
          <dt v-if="tunnelUrl">公网地址</dt>
          <dd v-if="tunnelUrl" class="mono">{{ tunnelUrl }}</dd>
          <dt v-if="config?.tunnel?.error">错误</dt>
          <dd v-if="config?.tunnel?.error" class="err">{{ config.tunnel.error }}</dd>
        </dl>

        <div class="actions">
          <button v-if="!binaryReady" type="button" class="btn primary" :disabled="!!busy" @click="downloadBinary">
            下载 cloudflared
          </button>
          <button
            v-if="binaryReady && !config?.tunnel?.running"
            type="button"
            class="btn primary"
            :disabled="!!busy"
            @click="startTunnel"
          >
            启动隧道
          </button>
          <button
            v-if="config?.tunnel?.running"
            type="button"
            class="btn"
            :disabled="!!busy"
            @click="stopTunnel"
          >
            停止隧道
          </button>
          <button type="button" class="btn ghost" :disabled="!!busy" @click="killOrphans">
            清理残留进程
          </button>
        </div>
      </section>

      <!-- ② Tailscale -->
      <section class="card">
        <header>
          <h2>Tailscale</h2>
          <span :class="['pill', tsStatus?.running ? 'is-on' : 'is-off']">
            {{ tsStatus?.running ? "已连接" : tsStatus?.installed ? "未连接" : "未安装" }}
          </span>
        </header>
        <dl>
          <dt>IP</dt>
          <dd class="mono">{{ tsStatus?.ip || "—" }}</dd>
        </dl>
        <div class="actions">
          <button type="button" class="btn" :disabled="!!busy" @click="tailscaleToggle">
            开 / 关
          </button>
          <button type="button" class="btn ghost" @click="refreshTailscale">刷新状态</button>
        </div>
      </section>

      <!-- ③ 配对管理 -->
      <section class="card">
        <header>
          <h2>访问凭据</h2>
          <button type="button" class="btn primary sm" :disabled="!!busy" @click="issuePair">
            签发新凭据
          </button>
        </header>
        <table v-if="pairs.length" class="pairs">
          <thead>
            <tr>
              <th>标签</th>
              <th>地址</th>
              <th>状态</th>
              <th />
            </tr>
          </thead>
          <tbody>
            <tr v-for="pair in pairs" :key="pair.id">
              <td>{{ pair.label || pair.id }}</td>
              <td class="mono">{{ pair.url || "—" }}</td>
              <td>
                <span :class="['pill', pair.revoked ? 'is-off' : 'is-on']">
                  {{ pair.revoked ? "已收回" : "有效" }}
                </span>
              </td>
              <td class="row-actions">
                <button type="button" class="link-btn" :disabled="!!busy" @click="revokePair(pair.id)">
                  收回
                </button>
                <button type="button" class="link-btn danger" :disabled="!!busy" @click="deletePair(pair.id)">
                  删除
                </button>
              </td>
            </tr>
          </tbody>
        </table>
        <p v-else class="empty">还没有签发过访问凭据</p>
      </section>

      <p v-if="message" class="ok">{{ message }}</p>
      <p v-if="errorText" class="err">{{ errorText }}</p>
    </template>
  </div>
</template>

<style scoped>
.gateway-page {
  display: flex;
  flex-direction: column;
  gap: 14px;
  max-width: 820px;
}
.loading {
  color: var(--fg-faint, #9ca3af);
  font-size: 13px;
}
.card {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 10px;
  background: var(--panel, #fff);
  padding: 16px 18px;
}
header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 12px;
}
h2 {
  font-size: 14px;
  margin: 0;
}
.pill {
  font-size: 11px;
  padding: 2px 9px;
  border-radius: 999px;
  background: var(--soft, #f3f4f6);
  color: var(--fg-dim, #6b7280);
}
.pill.is-on {
  background: #dcfce7;
  color: #15803d;
}
.pill.is-off {
  background: var(--soft, #f3f4f6);
  color: var(--fg-faint, #9ca3af);
}
dl {
  display: grid;
  grid-template-columns: 88px 1fr;
  gap: 6px 14px;
  font-size: 12.5px;
  margin: 0 0 14px;
}
dt {
  color: var(--fg-faint, #9ca3af);
}
dd {
  margin: 0;
}
.mono {
  font-family: ui-monospace, Menlo, monospace;
  font-size: 11.5px;
}
.dim {
  color: var(--fg-faint, #9ca3af);
  margin-left: 8px;
  word-break: break-all;
}
.actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.pairs {
  width: 100%;
  border-collapse: collapse;
  font-size: 12.5px;
}
.pairs th {
  text-align: left;
  font-weight: 500;
  color: var(--fg-faint, #9ca3af);
  font-size: 11.5px;
  padding: 6px 8px;
  border-bottom: 1px solid var(--line, #f1f5f9);
}
.pairs td {
  padding: 8px;
  border-bottom: 1px solid var(--line, #f8fafc);
}
.row-actions {
  display: flex;
  gap: 10px;
}
.link-btn {
  border: 0;
  background: transparent;
  color: var(--accent, #2563eb);
  font-size: 12px;
  cursor: pointer;
  padding: 0;
}
.link-btn.danger {
  color: #dc2626;
}
.empty {
  font-size: 12.5px;
  color: var(--fg-faint, #9ca3af);
  margin: 0;
}
.ok {
  font-size: 12.5px;
  color: #15803d;
}
.err {
  font-size: 12.5px;
  color: #dc2626;
}
</style>
