<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useServersStore, type HubServer } from "@/stores/servers";
import ServerEditDialog from "@/components/hub/ServerEditDialog.vue";
import ConfirmDialog from "@/components/hub/ConfirmDialog.vue";
import LatencyTrend from "@/components/hub/LatencyTrend.vue";

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
  if (!result) return "未测试";
  return String(result);
}

function openTerminal(server: HubServer) {
  void import("@/api/tauri").then(({ call }) =>
    call("hub_open_window", { label: "server-term", serverId: server.id }).catch(() => {}),
  );
}

/** 删除确认（旧版用 #deleteOverlay 弹层，不是 window.confirm） */
const deleteTarget = ref<HubServer | null>(null);

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

onMounted(() => {
  void store.load().then(() => {
    const first = store.servers[0];
    if (first) selectedId.value = first.id;
  });
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
        <div v-if="selected" class="detail">
          <h2>{{ selected.name }}</h2>
          <dl>
            <dt>协议</dt>
            <dd>{{ selected.protocol.toUpperCase() }}</dd>
            <dt>地址</dt>
            <dd class="mono">{{ store.displayAddr(selected) }}</dd>
            <dt>用户名</dt>
            <dd>{{ selected.username || "（交互输入）" }}</dd>
            <dt>认证</dt>
            <dd>
              {{
                selected.key_id
                  ? `密钥 · ${selected.key_id}`
                  : selected.password_enc
                    ? "已保存密码"
                    : "交互输入"
              }}
            </dd>
            <dt>跳板机</dt>
            <dd>{{ selected.jump_id ? `经由 ${selected.jump_id}` : "直连" }}</dd>
            <dt>终端路径</dt>
            <dd class="mono">{{ selected.term_path || "（远端家目录）" }}</dd>
            <dt>终端 shell</dt>
            <dd class="mono">{{ selected.term_shell || "（登录 shell）" }}</dd>
          </dl>

          <LatencyTrend ref="latencyRef" />

          <div class="row-actions">
            <button type="button" class="btn" @click="testOne(selected)">测试连接</button>
            <button type="button" class="btn" @click="openEdit(selected)">编辑</button>
            <button type="button" class="btn ghost danger" @click="onDelete(selected)">删除</button>
            <button type="button" class="btn ghost" @click="openTerminal(selected)">打开终端</button>
          </div>
        </div>

        <div v-else class="placeholder">
          <div class="card-hint">
            <div class="big">🖥️</div>
            <h2>选择左侧服务器</h2>
            <p>查看详情、测试连通性，或点「打开终端」直接进 SSH。</p>
          </div>
        </div>
      </div>
    </main>
  </div>

  <ServerEditDialog
    :open="dialogOpen"
    :server="editing"
    @save="onSave"
    @close="dialogOpen = false"
  />

  <!-- 删除确认（旧版 #deleteOverlay） -->
  <ConfirmDialog
    :open="Boolean(deleteTarget)"
    title="删除服务器"
    :message="`确定删除「${deleteTarget?.name ?? ''}」吗？此操作不可恢复。`"
    confirm-text="删除"
    :danger="true"
    @confirm="confirmDelete"
    @cancel="deleteTarget = null"
  />
</template>

<!--
  样式**不写在这里**：`.toolbar` / `.cols` / `aside` / `#detail` / `.list` /
  `.aside-head` 全部由 `styles/vendor/hub-inline.css` 定义（从旧 hub.html
  内联样式块原样提取）。自造一套会覆盖掉原样式。
-->
