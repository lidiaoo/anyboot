<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useServersStore, type HubServer } from "@/stores/servers";
import ServerEditDialog from "@/components/hub/ServerEditDialog.vue";
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

async function onDelete(server: HubServer) {
  if (!window.confirm(`确定删除「${server.name}」？`)) return;
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
  <div class="servers-page">
    <div class="toolbar">
      <div class="search-wrap">
        <span aria-hidden="true">🔍</span>
        <input v-model="query" type="search" placeholder="搜索名称 / 地址" aria-label="搜索服务器" />
      </div>
      <div class="segmented" role="tablist" aria-label="协议筛选">
        <button role="tab" :aria-selected="protocol === ''" @click="protocol = ''">
          全部 <span class="mono">{{ store.byProtocol.all }}</span>
        </button>
        <button role="tab" :aria-selected="protocol === 'ssh'" @click="protocol = 'ssh'">
          SSH <span class="mono">{{ store.byProtocol.ssh }}</span>
        </button>
        <button role="tab" :aria-selected="protocol === 'telnet'" @click="protocol = 'telnet'">
          Telnet <span class="mono">{{ store.byProtocol.telnet }}</span>
        </button>
      </div>
      <div class="toolbar-right">
        <span class="summary">{{ rows.length }} / {{ store.servers.length }} 台</span>
        <button type="button" class="btn primary" @click="openAdd">＋ 添加服务器</button>
      </div>
    </div>

    <div class="cols">
      <aside>
        <div class="aside-head">
          <h2>服务器</h2>
          <button type="button" class="link-btn" @click="store.testAll()">刷新</button>
        </div>
        <div class="list">
          <button
            v-for="server in rows"
            :key="server.id"
            type="button"
            :class="['srv-item', { 'is-on': server.id === selectedId }]"
            @click="selectedId = server.id"
          >
            <div class="srv-name">{{ server.name }}</div>
            <div class="srv-meta">
              <span class="proto">{{ server.protocol.toUpperCase() }}</span>
              <span class="addr">{{ store.displayAddr(server) }}</span>
            </div>
            <div v-if="store.testResults.get(server.id)" class="srv-test">
              {{ store.testResults.get(server.id) }}
            </div>
          </button>
          <p v-if="!rows.length" class="empty">没有匹配的服务器</p>
        </div>
      </aside>

      <main>
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
          <div class="actions">
            <button type="button" class="btn" @click="testOne(selected)">测试连接</button>
            <button type="button" class="btn" @click="openEdit(selected)">编辑</button>
            <button type="button" class="btn ghost danger" @click="onDelete(selected)">删除</button>
          </div>
        </div>
        <div v-else class="placeholder">
          <div class="big">🖥️</div>
          <h2>选择左侧服务器</h2>
          <p>查看详情、测试连通性，或点「打开终端」直接进 SSH。</p>
        </div>
      </main>
    </div>

    <ServerEditDialog
      :open="dialogOpen"
      :server="editing"
      @save="onSave"
      @close="dialogOpen = false"
    />
  </div>
</template>

<style scoped>
.servers-page {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: 14px 16px;
  gap: 12px;
}
.toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.search-wrap {
  display: flex;
  align-items: center;
  gap: 6px;
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 8px;
  padding: 5px 10px;
  background: var(--panel, #fff);
}
.search-wrap input {
  border: 0;
  outline: 0;
  font-size: 13px;
  width: 200px;
  background: transparent;
}
.segmented {
  display: flex;
  gap: 2px;
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 8px;
  padding: 2px;
  background: var(--soft, #f9fafb);
}
.segmented button {
  border: 0;
  background: transparent;
  font-size: 12.5px;
  padding: 4px 10px;
  border-radius: 6px;
  cursor: pointer;
  color: var(--fg-dim, #4b5563);
}
.segmented button[aria-selected="true"] {
  background: var(--panel, #fff);
  color: var(--fg, #111827);
  font-weight: 600;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.06);
}
.mono {
  font-family: ui-monospace, Menlo, monospace;
  font-size: 11px;
}
.toolbar-right {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 10px;
}
.summary {
  font-size: 12px;
  color: var(--fg-faint, #9ca3af);
}
.cols {
  flex: 1;
  display: grid;
  grid-template-columns: 260px 1fr;
  gap: 12px;
  min-height: 0;
}
aside {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 10px;
  background: var(--panel, #fff);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.aside-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  border-bottom: 1px solid var(--line, #f1f5f9);
}
.aside-head h2 {
  font-size: 12px;
  margin: 0;
  color: var(--fg-dim, #6b7280);
  text-transform: uppercase;
  letter-spacing: 0.06em;
}
.list {
  flex: 1;
  overflow-y: auto;
  padding: 6px;
}
.srv-item {
  display: block;
  width: 100%;
  text-align: left;
  border: 0;
  background: transparent;
  border-radius: 8px;
  padding: 8px 10px;
  cursor: pointer;
}
.srv-item:hover {
  background: var(--soft, #f3f4f6);
}
.srv-item.is-on {
  background: var(--accent-soft, #eff6ff);
}
.srv-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--fg, #111827);
}
.srv-meta {
  display: flex;
  gap: 8px;
  font-size: 11px;
  color: var(--fg-faint, #9ca3af);
  margin-top: 2px;
}
.srv-test {
  font-size: 11px;
  color: #16a34a;
  margin-top: 2px;
}
.empty {
  padding: 18px;
  text-align: center;
  color: var(--fg-faint, #9ca3af);
  font-size: 12px;
}
main {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 10px;
  background: var(--panel, #fff);
  overflow-y: auto;
}
.detail {
  padding: 18px 20px;
}
.detail h2 {
  font-size: 16px;
  margin: 0 0 14px;
}
.detail dl {
  display: grid;
  grid-template-columns: 90px 1fr;
  gap: 8px 14px;
  font-size: 13px;
  margin: 0 0 18px;
}
.detail dt {
  color: var(--fg-faint, #9ca3af);
}
.detail dd {
  margin: 0;
}
.actions {
  display: flex;
  gap: 8px;
}
.btn.danger {
  color: #dc2626;
}
.placeholder {
  padding: 48px 20px;
  text-align: center;
  color: var(--fg-faint, #9ca3af);
}
.placeholder .big {
  font-size: 30px;
  margin-bottom: 8px;
}
.placeholder h2 {
  font-size: 15px;
  color: var(--fg-dim, #6b7280);
  margin: 0 0 6px;
}
.placeholder p {
  font-size: 12.5px;
  margin: 0;
}
</style>
