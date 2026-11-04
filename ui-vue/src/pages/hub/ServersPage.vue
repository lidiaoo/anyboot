<script setup lang="ts">
const toast = useToastStore();
import { computed, onMounted, ref } from "vue";
import { useServersStore, type HubServer } from "@/stores/servers";
import ServerEditDialog from "@/components/hub/ServerEditDialog.vue";
import TunnelDialog from "@/components/hub/TunnelDialog.vue";
import LatencyTrend from "@/components/hub/LatencyTrend.vue";
import { useToastStore } from "@/stores/toast";
import { isTauri } from "@/api/tauri";
import { useEscape } from "@/composables/useEscape";

/**
 * 服务器页（原 `ui/hub.html` 的 `#page-servers`）。
 *
 * ```
 * .toolbar   搜索 + 协议筛选（全部/SSH/Telnet）+ 添加
 * .cols
 *   aside    服务器列表（点选 → 右侧详情）
 *   main     详情：地址 / 认证 / 跳板机 / 终端设置 / 测试连接 / 打开终端
 * ```
 *
 * 这是 SSH/SFTP 服务器管理，与「电脑列表」（AnyBoot 电脑）是两套独立档案。
 */

const store = useServersStore();

const query = ref("");
const protocol = ref<"" | "ssh" | "telnet">("");
const selectedId = ref<string>("");
const dialogOpen = ref(false);
/** 编辑哪一台；null = 新增 */
const editing = ref<HubServer | null>(null);

function openAdd() {
  editing.value = null;
  dialogOpen.value = true;
}

function openEdit(server: HubServer) {
  editing.value = server;
  dialogOpen.value = true;
}

async function onSave(payload: Record<string, unknown>) {
  try {
    await store.save(payload);
    dialogOpen.value = false;
    // 新增后选中新那台
    const saved = store.servers.find((s) => s.name === payload.name);
    if (saved) selectedId.value = saved.id;
  } catch (error) {
    window.alert(`保存失败：${error}`);
  }
}

/** 探测状态（旧版 probes[id].state） */
function testState(id: string): string {
  const result = store.testResults.get(id);
  if (!result) return "";
  return String(result).includes("失败") || String(result).includes("错误") ? "bad" : "ok";
}

/** 探测耗时（旧版 probes[id].ms） */
function testMs(id: string): number | null {
  const result = store.testResults.get(id);
  if (!result) return null;
  const match = String(result).match(/(\d+)\s*ms/);
  return match ? Number(match[1]) : null;
}

/** 耗时颜色档（旧版 msClass()） */
function msClass(ms: number): string {
  if (ms < 80) return "ok";
  if (ms < 300) return "warn";
  return "bad";
}

/** 状态文案（旧版 stateLabel()） */
function stateLabel(id: string): string {
  const result = store.testResults.get(id);
  if (!result) return "未测";
  return String(result);
}





/** 状态胶囊（旧版 stateLabel，hub.html:5908） */
const statePillText = computed(() => {
  const result = store.testResults.get(selectedId.value);
  if (!result) return "未测";
  return String(result).includes("失败") || String(result).includes("错误") ? "离线" : "在线";
});
const statePillClass = computed(() => {
  const result = store.testResults.get(selectedId.value);
  if (!result) return "dim";
  return String(result).includes("失败") || String(result).includes("错误") ? "bad" : "ok";
});

/** 端口（旧版 `server.port || (protocol === 'telnet' ? 23 : 22)`） */
function portOf(server: HubServer): number {
  return server.port || (server.protocol === "telnet" ? 23 : 22);
}

/** 经由（旧版 renderDetail 的「经由」格） */
function jumpLabel(server: HubServer): string {
  if (!server.jump_id) return "直连";
  const jump = store.servers.find((s) => s.id === server.jump_id);
  return jump ? `跳板机 · ${jump.name || store.displayAddr(jump)}` : "跳板机（已删除，保存后回落直连）";
}

/** 延迟文案（旧版 msText） */
const msText = computed(() => {
  const ms = testMs(selectedId.value);
  return ms === null ? "—" : `${ms} ms`;
});

/** 测试回执行（旧版 #detailTestLine） */
const testLine = computed(() => {
  const result = store.testResults.get(selectedId.value);
  return result ? String(result) : "";
});

/** 隧道列表（旧版 #tunnelBlock 内容） */
interface TunnelRow {
  id: string;
  name: string;
  kind: string;
  localPort: number;
  targetHost: string;
  targetPort: number;
  running: boolean;
}
const tunnels = ref<TunnelRow[]>([]);

async function refreshTunnels() {
  try {
    const { call } = await import("@/api/tauri");
    tunnels.value = ((await call("hub_list_tunnels", {})) as TunnelRow[]) ?? [];
  } catch {
    tunnels.value = [];
  }
}

const tunnelOpen = ref(false);
const editingTunnel = ref<{ id?: string; name: string; kind: string; localPort: number; targetHost: string; targetPort: number } | null>(null);
/** 新建隧道（旧版 tunnelAddBtn → 打开 TunnelDialog） */
function openTunnel(id: string) {
  void id;
  editingTunnel.value = null;
  tunnelOpen.value = true;
}
/** 编辑已有隧道 */
function openTunnelEdit(tn: { id?: string; name: string; kind: string; localPort: number; targetHost: string; targetPort: number }) {
  editingTunnel.value = { ...tn };
  tunnelOpen.value = true;
}
/** 保存隧道 */
async function saveTunnel(payload: Record<string, unknown>) {
  const { call } = await import("@/api/tauri");
  try {
    await call("hub_save_tunnel", {
      tunnel: {
        id: String(payload.id ?? ""),
        server_id: String(payload.serverId ?? selectedId.value),
        name: String(payload.name ?? ""),
        kind: String(payload.kind ?? "local"),
        local_port: Number(payload.localPort ?? 0),
        target_host: String(payload.targetHost ?? ""),
        target_port: Number(payload.targetPort ?? 0),
      },
    });
    tunnelOpen.value = false;
    await refreshTunnels();
  } catch (e) {
    void e;
  }
}

async function toggleTunnel(id: string) {
  const row = tunnels.value.find((x) => x.id === id);
  const { call } = await import("@/api/tauri");
  try {
    await call(row?.running ? "hub_tunnel_stop" : "hub_tunnel_start", { tunnelId: id });
    await refreshTunnels();
  } catch {
    /* 忽略 */
  }
}

async function deleteTunnel(id: string) {
  const { call } = await import("@/api/tauri");
  try {
    await call("hub_delete_tunnel", { id });
    await refreshTunnels();
  } catch {
    /* 忽略 */
  }
}

/** 浏览文件（旧版 #filesBtn → hub_open_files_window） */
function openFiles(server: HubServer) {
  if (!isTauri()) { toast.show("文件浏览需要应用环境", "bad"); return; }
  if (server.protocol !== "ssh") return;
  void import("@/api/tauri").then(({ call }) =>
    call("hub_open_files_window", { serverId: server.id }).catch((e) => toast.show("打开文件浏览失败：" + e, "bad")),
  );
}

function openTerminal(server: HubServer) {
  if (!isTauri()) { toast.show("终端需要应用环境", "bad"); return; }
  void import("@/api/tauri").then(({ call }) =>
    call("hub_open_term_window", { serverId: server.id }).catch((e) => toast.show("打开终端失败：" + e, "bad")),
  );
}

/** 删除确认（旧版用 #deleteOverlay 弹层，不是 window.confirm） */
const deleteTarget = ref<HubServer | null>(null);
useEscape(computed(() => Boolean(deleteTarget.value)), () => { deleteTarget.value = null });

function onDelete(server: HubServer) {
  deleteTarget.value = server;
}

async function confirmDelete() {
  const server = deleteTarget.value;
  deleteTarget.value = null;
  if (!server) return;
  try {
    await store.remove(server.id);
    if (selectedId.value === server.id) selectedId.value = store.servers[0]?.id ?? "";
  } catch (error) {
    window.alert(`删除失败：${error}`);
  }
}

const rows = computed(() => {
  const keyword = query.value.trim().toLowerCase();
  return store.servers.filter((server) => {
    if (protocol.value && server.protocol !== protocol.value) return false;
    if (!keyword) return true;
    return [server.name, server.host, ...(server.addresses ?? [])]
      .join(" ")
      .toLowerCase()
      .includes(keyword);
  });
});

const selected = computed<HubServer | null>(
  () => store.servers.find((server) => server.id === selectedId.value) ?? null,
);

/** 底栏时钟（旧版 #clock） */
const clock = ref("--:--:--");
function tickClock() {
  clock.value = new Date().toLocaleTimeString("zh-CN", { hour12: false });
}
onMounted(() => { tickClock(); window.setInterval(tickClock, 1000); });

onMounted(() => {
  // 旧版 selectedId 初始为 null（hub.html:5140），**不自动选中第一台** ——
  // 进入页面应显示「选择左侧服务器」空态。只有用户点击、或新增/编辑保存后
  // 才会选中（旧版 5951 / 6657 两处）。
  void store.load();
});

const latencyRef = ref<InstanceType<typeof LatencyTrend> | null>(null);

async function testOne(server: HubServer) {
  const started = performance.now();
  await store.test(server);
  // 从结果文字里抽出「通 · N ms」，抽不到就用本地耗时
  const text = store.testResults.get(server.id) ?? "";
  const matched = text.match(/(\d+)\s*ms/);
  latencyRef.value?.push(matched ? Number(matched[1]) : Math.round(performance.now() - started));
}
</script>

<template>
  <div class="toolbar">
    <div class="search-wrap">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
        <circle cx="11" cy="11" r="7" />
        <path d="M20 20l-3.5-3.5" />
      </svg>
      <input
        id="searchBox"
        v-model="query"
        type="search"
        placeholder="搜索名称 / 地址"
        aria-label="搜索服务器"
      />
    </div>

    <div class="segmented" id="protoTabs" role="tablist" aria-label="协议筛选">
      <button role="tab" :aria-selected="protocol === ''" @click="protocol = ''">
        全部 <span class="mono" id="cntAll">{{ store.byProtocol.all }}</span>
      </button>
      <button role="tab" :aria-selected="protocol === 'ssh'" @click="protocol = 'ssh'">
        SSH <span class="mono" id="cntSsh">{{ store.byProtocol.ssh }}</span>
      </button>
      <button role="tab" :aria-selected="protocol === 'telnet'" @click="protocol = 'telnet'">
        Telnet <span class="mono" id="cntTel">{{ store.byProtocol.telnet }}</span>
      </button>
    </div>

    <div class="toolbar-right">
      <span class="summary" id="summary">{{ rows.length }} / {{ store.servers.length }} 台</span>
      <button type="button" id="addBtn" class="btn primary" @click="openAdd">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" aria-hidden="true">
          <path d="M12 5v14M5 12h14" />
        </svg>
        添加服务器
      </button>
    </div>
  </div>

  <div class="cols">
    <aside aria-label="服务器列表">
      <div class="aside-head">
        <h2>服务器</h2>
        <button type="button" id="refreshBtn" class="link-btn" aria-label="测试全部连接" @click="store.testAll()">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
            <path d="M21 12a9 9 0 1 1-2.6-6.4M21 3v6h-6" />
          </svg>
          刷新
        </button>
      </div>
      <div class="list scroll-thin" id="serverList" role="listbox" aria-label="服务器">
        <div
          v-for="server in rows"
          :key="server.id"
          role="option"
          :aria-selected="server.id === selectedId"
        >
          <!-- 旧版 renderServers() 的行结构：.server-row > .s-icon + .s-main + .s-right -->
          <button
            type="button"
            :class="['server-row', { active: server.id === selectedId }]"
            @click="selectedId = server.id"
          >
            <span class="s-icon">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <rect x="2" y="3" width="20" height="7" rx="2" />
                <rect x="2" y="14" width="20" height="7" rx="2" />
                <path d="M6 6.5h.01M6 17.5h.01" />
              </svg>
            </span>
            <span class="s-main">
              <span class="s-line">
                <span class="s-name">{{ server.name || "(未命名)" }}</span>
                <span class="s-proto">{{ server.protocol === "telnet" ? "TEL" : "SSH" }}</span>
              </span>
              <span class="s-host">
                <span class="dot" :class="testState(server.id)" />
                <span class="txt">{{ store.displayAddr(server) }}</span>
              </span>
            </span>
            <span class="s-right">
              <span v-if="testMs(server.id) !== null" class="s-ms" :class="msClass(testMs(server.id)!)">
                {{ testMs(server.id) }}ms
              </span>
              <span v-else class="s-ms ms-none">—</span>
              <span class="s-state">{{ stateLabel(server.id) }}</span>
            </span>
          </button>
        </div>
        <p v-if="!rows.length" class="empty">没有匹配的服务器</p>
      </div>
    </aside>

    <main>
      <div id="detail" class="scroll-thin">
        <!-- 空态（旧版 renderDetail 的 placeholder 分支） -->
        <div v-if="!selected" class="placeholder">
          <div class="card-hint">
            <div class="big">🖥️</div>
            <h2>选择左侧服务器</h2>
            <p>查看详情、测试连通性，或点「打开终端」直接进 SSH。</p>
          </div>
        </div>

        <!-- 详情（旧版 renderDetail，hub.html:6087） -->
        <template v-else>
          <!-- .d-head：图标 + 标题行（名称 / 状态胶囊 / 协议徽章 / 地址 / 备注）+ 动作组 -->
          <div class="d-head">
            <span class="d-icon">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <rect x="2" y="3" width="20" height="7" rx="2" />
                <rect x="2" y="14" width="20" height="7" rx="2" />
                <path d="M6 6.5h.01M6 17.5h.01" />
              </svg>
            </span>
            <div class="d-title">
              <div class="row">
                <h3>{{ selected.name || "(未命名)" }}</h3>
                <span :class="['pill', statePillClass]"><span class="pdot" />{{ statePillText }}</span>
                <span class="badge">{{ selected.protocol === "ssh" ? "SSH" : "TELNET" }}</span>
              </div>
              <p class="host">{{ store.displayAddr(selected) }}</p>
              <p v-if="selected.notes" class="notes">{{ selected.notes }}</p>
            </div>
            <div class="d-actions">
              <button
                type="button"
                id="connectBtn"
                class="btn primary"
                :disabled="selected.protocol !== 'ssh'"
                :title="selected.protocol === 'ssh' ? '打开终端' : 'Telnet 终端在后续版本开通'"
                @click="openTerminal(selected)"
              >
                <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <rect x="2" y="4" width="20" height="14" rx="2" />
                  <path d="M6 9l3 3-3 3" />
                </svg>
                打开终端
              </button>
              <button
                type="button"
                id="filesBtn"
                class="btn"
                :disabled="selected.protocol !== 'ssh'"
                :title="selected.protocol === 'ssh' ? '浏览文件' : 'Telnet 不提供文件浏览'"
                @click="openFiles(selected)"
              >
                <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
                </svg>
                浏览文件
              </button>
              <button type="button" id="editBtn" class="icon-btn boxed" aria-label="编辑服务器" title="编辑" @click="openEdit(selected)">
                <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <path d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z" />
                </svg>
              </button>
              <button type="button" id="delBtn" class="icon-btn boxed danger" aria-label="删除服务器" title="删除" @click="onDelete(selected)">
                <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <path d="M3 6h18M8 6V4h8v2M6 6l1 15h10l1-15M10 11v6M14 11v6" />
                </svg>
              </button>
            </div>
          </div>

          <!-- .info-grid：5 格（用户 / 认证方式 / 端口 / 经由 / 延迟） -->
          <div class="info-grid" role="group" aria-label="连接信息">
            <div>
              <p class="k">用户</p>
              <p class="v mono">{{ selected.username || "—" }}</p>
            </div>
            <div>
              <p class="k">认证方式</p>
              <p class="v">
                <template v-if="selected.key_id">密钥 · <span class="mono">{{ selected.key_id }}</span></template>
                <template v-else-if="selected.password_enc">密码 · 已存（免输）</template>
                <template v-else>系统 ssh · 终端内交互</template>
              </p>
            </div>
            <div>
              <p class="k">端口</p>
              <p class="v mono">{{ portOf(selected) }}</p>
            </div>
            <div>
              <p class="k">经由</p>
              <p class="v">{{ jumpLabel(selected) }}</p>
            </div>
            <div>
              <p class="k">延迟</p>
              <p class="v mono">{{ msText }}</p>
            </div>
          </div>

          <!-- .d-section 1：连通性 -->
          <div class="d-section">
            <div class="sec-head">
              <h4>连通性</h4>
              <button type="button" id="testBtn" class="btn small" @click="testOne(selected)">测试连接</button>
            </div>
            <p class="test-line" id="detailTestLine">{{ testLine }}</p>
          </div>

          <!-- .d-section 2：延迟趋势 -->
          <div class="d-section">
            <LatencyTrend ref="latencyRef" />
          </div>

          <!-- .d-section 3：隧道（旧版 #tunnelBlock） -->
          <div class="d-section" id="tunnelBlock">
            <div class="sec-head">
              <h4>SSH 隧道</h4>
              <button type="button" class="btn small" id="tunnelAddBtn" @click="openTunnel(selected.id)">新建隧道</button>
            </div>
            <div v-if="!tunnels.length" class="tunnel-empty">
              还没有隧道。建一条：把本机端口转发到服务器的内网（本地转发）或变成 SOCKS5 代理（动态转发）。<br />认证复用终端会话 —— 起隧道前先在该服务器上开一次终端。
            </div>
            <div v-else class="tunnel-list">
              <div v-for="tn in tunnels" :key="tn.id" :class="['tunnel-row', { off: !tn.running }]">
                <span :class="['dot', { ok: tn.running }]" />
                <div class="t-main">
                  <div class="t-name">
                    <span class="nm">{{ tn.name || "(未命名)" }}</span>
                    <span :class="['tunnel-kind', { dyn: tn.kind === 'dynamic' }]">{{ tn.kind === "dynamic" ? "SOCKS5" : "转发" }}</span>
                  </div>
                  <div class="t-sub">
                    {{ tn.kind === "dynamic" ? "本机 127.0.0.1:" + tn.localPort + " → SOCKS5 动态代理" : "本机 127.0.0.1:" + tn.localPort + " → " + (tn.targetHost || "?") + ":" + (tn.targetPort || "?") }}
                  </div>
                </div>
                <div class="t-acts">
                  <button v-if="tn.running" type="button" class="btn small" @click="toggleTunnel(tn.id)">停止</button>
                  <button v-else type="button" class="btn small primary" @click="toggleTunnel(tn.id)">启动</button>
                  <button type="button" class="btn small" @click="openTunnelEdit(tn)">编辑</button>
                  <button type="button" class="btn small danger" @click="deleteTunnel(tn.id)">删除</button>
                </div>
              </div>
            </div>
          </div>
        </template>
      </div>
    </main>
  </div>

  <!-- 底栏（旧版 .footer） -->
  <footer>
    <span class="dot" />
    <span id="footText">终端 / 文件 / 密钥均在独立窗口</span>
    <span class="sep">·</span>
    <span id="footCount">{{ store.servers.length }} 台服务器</span>
    <span class="clock" id="clock">{{ clock }}</span>
  </footer>

  <ServerEditDialog
    :open="dialogOpen"
    :server="editing"
    @save="onSave"
    @close="dialogOpen = false"
  />
    <TunnelDialog
      :open="tunnelOpen"
      :tunnel="editingTunnel"
      :server="selected ? ({ id: selected.id, name: selected.name, addresses: [], token: '' } as any) : null"
      @save="saveTunnel"
      @close="tunnelOpen = false; refreshTunnels()"
    />

  <!-- 删除确认（旧版 #deleteOverlay） -->
  <div v-if="deleteTarget" id="deleteOverlay" class="overlay" @click.self="deleteTarget = null">
    <div class="dialog" role="dialog" aria-modal="true" aria-label="删除服务器">
      <div class="dialog-head">
        <h3>删除服务器</h3>
        <button type="button" class="icon-btn" id="deleteX" aria-label="关闭" @click="deleteTarget = null">✕</button>
      </div>
      <div class="dialog-body">
        <p class="d-text">确定删除「<b id="deleteName">{{ deleteTarget?.name ?? "" }}</b>」吗？此操作不可恢复。</p>
      </div>
      <div class="dialog-foot">
        <span class="spacer" />
        <button type="button" class="btn ghost" id="deleteCancel" @click="deleteTarget = null">取消</button>
        <button type="button" class="btn danger" id="deleteConfirm" @click="confirmDelete">删除</button>
      </div>
    </div>
  </div>
</template>

<!--
  样式**不写在这里**：`.toolbar` / `.cols` / `aside` / `#detail` / `.list` /
  `.aside-head` 全部由 `styles/vendor/hub-inline.css` 定义（从旧 hub.html
  内联样式块原样提取）。自造一套会覆盖掉原样式。
-->
