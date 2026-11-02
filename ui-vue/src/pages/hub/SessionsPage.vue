<script setup lang="ts">
const toast = useToastStore();
import { onMounted, ref } from "vue";
import { call } from "@/api/tauri";
import { useToastStore } from "@/stores/toast";

/**
 * 会话页（原 `ui/hub.html` 的 `#page-sessions`，3365 行 / 渲染逻辑 6810–6900）。
 *
 * ## 1:1 结构（三段）
 * ```
 * .page-head > h2「会话」 + span.sub
 * #sessionList.session-list.scroll-thin
 *   ① 「进行中的终端会话」.session-sec
 *      .session-row > .dot.ok + .s-main(名称 + 主机) + 「聚焦」+「断开」
 *   ② 「历史 · 本窗口打开过」.session-sec
 *      .session-row > .dot + .s-main(名称 + 主机) + 时间 + 「重连」+「移除」
 *   ③ 「运行中的隧道」.session-sec
 *      .session-row > .dot.ok + .s-main(名称 + 转发说明)
 *   空态：.act-card > p.t「暂无会话」+ p.s 说明
 * ```
 */

interface TermSession {
  id: string;
  server_name?: string;
  host_label?: string;
}

interface TermLogRow {
  name: string;
  host: string;
  at: number;
  serverId?: string;
}

interface TunnelRow {
  id: string;
  name?: string;
  kind?: string;
  local_port?: number;
  target_host?: string;
  target_port?: number;
  running?: boolean;
}

const sessions = ref<TermSession[]>([]);
const termLog = ref<TermLogRow[]>([]);
const tunnels = ref<TunnelRow[]>([]);
const loading = ref(true);

const runningTunnels = ref<TunnelRow[]>([]);

async function refresh() {
  loading.value = true;
  try {
    sessions.value = (await call<TermSession[]>("hub_term_sessions")) ?? [];
  } catch {
    sessions.value = [];
  }
  try {
    const list = (await call<TunnelRow[]>("hub_list_tunnels", {})) ?? [];
    tunnels.value = list;
    runningTunnels.value = list.filter((t) => t.running);
  } catch {
    tunnels.value = [];
    runningTunnels.value = [];
  }
  try {
    // 历史记录：旧版是前端 `termLog` 数组（本窗口打开过的终端），
    // 新版用 localStorage 镜像，key 与旧版一致
    const raw = localStorage.getItem("anyboot.termLog");
    termLog.value = raw ? (JSON.parse(raw) as TermLogRow[]) : [];
  } catch {
    termLog.value = [];
  }
  loading.value = false;
}

onMounted(refresh);

function fmtTime(ts?: number): string {
  if (!ts) return "";
  return new Date(ts).toLocaleTimeString("zh-CN", { hour12: false });
}

function tunnelSub(t: TunnelRow): string {
  if (t.kind === "dynamic") return `127.0.0.1:${t.local_port} → SOCKS5`;
  return `127.0.0.1:${t.local_port} → ${t.target_host || "?"}:${t.target_port || "?"}`;
}

function focusTerminal() {
  void call("hub_focus_window", { label: "server-term" }).catch(() => {});
}

function killSession(id: string) {
  void call("hub_term_close", { session_id: id }).then(refresh).catch((e) => toast.show("断开失败：" + e, "bad"));
}

function reconnect(row: TermLogRow) {
  if (!row.serverId) return;
  void call("hub_open_term_window", { server_id: row.serverId }).catch((e) => toast.show("重连失败：" + e, "bad"));
}

function removeLog(index: number) {
  termLog.value = termLog.value.filter((_, i) => i !== index);
  try {
    localStorage.setItem("anyboot.termLog", JSON.stringify(termLog.value));
  } catch {
    /* 忽略 */
  }
}

const isEmpty = () =>
  !sessions.value.length && !termLog.value.length && !runningTunnels.value.length;
</script>

<template>
  <div class="page-head">
    <h2>会话</h2>
    <span class="sub">本窗口打开的终端 · 运行中的隧道</span>
  </div>

  <div class="session-list scroll-thin" id="sessionList">
    <p v-if="loading" class="field-hint">读取中…</p>

    <!-- 空态 -->
    <div v-else-if="isEmpty()" class="act-card" style="margin: 8px 0">
      <p class="t">暂无会话</p>
      <p class="s">在「服务器」页打开终端、或启动隧道后，这里会显示本窗口的真实会话记录。</p>
    </div>

    <template v-else>
      <!-- ① 进行中的终端会话 -->
      <template v-if="sessions.length">
        <p class="session-sec">进行中的终端会话</p>
        <div v-for="m in sessions" :key="m.id" class="session-row">
          <span class="dot ok" />
          <div class="s-main">
            <div>{{ m.server_name || "(未命名)" }}</div>
            <div class="s-sub">{{ m.host_label || "" }}</div>
          </div>
          <button type="button" class="btn" @click="focusTerminal">聚焦</button>
          <button type="button" class="btn" @click="killSession(m.id)">断开</button>
        </div>
      </template>

      <!-- ② 历史 · 本窗口打开过 -->
      <template v-if="termLog.length">
        <p class="session-sec">历史 · 本窗口打开过</p>
        <div v-for="(t, i) in termLog" :key="i" class="session-row">
          <span class="dot" />
          <div class="s-main">
            <div>{{ t.name }}</div>
            <div class="s-sub">{{ t.host }}</div>
          </div>
          <span class="mono" style="font-size: 11px; color: rgb(var(--fg-faint))">{{ fmtTime(t.at) }}</span>
          <button v-if="t.serverId" type="button" class="btn" @click="reconnect(t)">重连</button>
          <button type="button" class="btn" @click="removeLog(i)">移除</button>
        </div>
      </template>

      <!-- ③ 运行中的隧道 -->
      <template v-if="runningTunnels.length">
        <p class="session-sec">运行中的隧道</p>
        <div v-for="t in runningTunnels" :key="t.id" class="session-row">
          <span class="dot ok" />
          <div class="s-main">
            <div>{{ t.name || "(未命名)" }}</div>
            <div class="s-sub">{{ tunnelSub(t) }}</div>
          </div>
        </div>
      </template>
    </template>
  </div>
</template>
