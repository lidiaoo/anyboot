<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { call } from "@/api/tauri";
import { useRemoteStore } from "@/stores/remote";

/**
 * 电脑页（原 `ui/hub.html` 的 `#page-home`）。
 *
 * ```
 * .pc-top
 *   .pc-chips   本机 + 每台远程电脑一个标签 + 「＋ 添加」
 *   ⋮
 * .pc-body
 *   iframe      当前标签的整页（本机 = localhost:port，远程 = 那台机的 AnyBoot 页面）
 *   .pc-empty   没选 / 连不上时的说明
 * ```
 *
 * **每台电脑一个 iframe**：懒创建、重载 = 重建（`v-if` 换 key）。
 * 这与手机端「每台电脑一个标签、每个标签一个 WebView」是同一套设计。
 */

const remote = useRemoteStore();

interface LocalEndpoint {
  port: number;
  token: string;
  enabled: boolean;
  listening: boolean;
  addr: string;
  error: string;
}

const localEndpoint = ref<LocalEndpoint | null>(null);
const activeId = ref("local");
/** 重载计数：改变它 → iframe 的 key 变化 → 强制重建 */
const reloadSeed = ref(0);
const failMessage = ref("");

/** 本机 URL */
const localUrl = computed(() => {
  const ep = localEndpoint.value;
  if (!ep?.port) return "";
  return `http://127.0.0.1:${ep.port}/?token=${encodeURIComponent(ep.token)}`;
});

/** 远程电脑 URL（用 `remote_endpoint` 解析出的地址）*/
const remoteUrls = ref(new Map<string, string>());

const currentUrl = computed(() => {
  if (activeId.value === "local") return localUrl.value;
  return remoteUrls.value.get(activeId.value) ?? "";
});

const currentName = computed(() => {
  if (activeId.value === "local") return "本机";
  return remote.servers.find((s) => s.id === activeId.value)?.name ?? "";
});

const chips = computed(() => [
  { id: "local", name: "本机", sub: "默认 · 本地" },
  ...remote.servers.map((s) => ({ id: s.id, name: s.name, sub: "" })),
]);

onMounted(async () => {
  try {
    localEndpoint.value = await call<LocalEndpoint>("local_http_endpoint");
  } catch {
    localEndpoint.value = null;
  }
  await ensureRemoteUrl(activeId.value);
});

/** 解析某台远程电脑的访问地址 */
async function ensureRemoteUrl(id: string): Promise<void> {
  if (id === "local" || remoteUrls.value.has(id)) return;
  try {
    const endpoint = await call<{ host: string; port: number }>("remote_endpoint", {
      serverId: id,
    });
    if (endpoint?.host) {
      const profile = remote.servers.find((s) => s.id === id);
      const token = String(profile?.token ?? "");
      remoteUrls.value.set(
        id,
        `http://${endpoint.host}:${endpoint.port}/?token=${encodeURIComponent(token)}`,
      );
      remoteUrls.value = new Map(remoteUrls.value);
    }
  } catch {
    /* 连不上就留空，界面显示失败提示 */
  }
}

watch(activeId, async (id) => {
  failMessage.value = "";
  await ensureRemoteUrl(id);
});

function selectChip(id: string) {
  activeId.value = id;
}

/** 重载 = 换 key 强制重建 iframe（比改 src 更可靠） */
function reload() {
  failMessage.value = "";
  if (activeId.value !== "local") {
    remoteUrls.value.delete(activeId.value);
    remoteUrls.value = new Map(remoteUrls.value);
    void ensureRemoteUrl(activeId.value).then(() => {
      reloadSeed.value += 1;
    });
  } else {
    reloadSeed.value += 1;
  }
}

function onFrameError() {
  failMessage.value = "这台电脑的页面加载失败 —— 可能已关机、在睡眠，或 HTTP 服务没开";
}
</script>

<template>
  <div class="pc-page">
    <div class="pc-top">
      <div class="pc-chips" role="tablist" aria-label="AnyBoot 电脑标签">
        <button
          v-for="chip in chips"
          :key="chip.id"
          type="button"
          role="tab"
          :class="['pc-chip', { active: activeId === chip.id }]"
          :aria-current="activeId === chip.id"
          @click="selectChip(chip.id)"
        >
          <span class="dot" aria-hidden="true" />
          {{ chip.name }}
          <span v-if="chip.sub" class="mini">{{ chip.sub }}</span>
        </button>
        <button type="button" class="pc-chip add" disabled title="添加电脑弹窗待迁移">
          ＋ 添加
        </button>
      </div>
      <div class="pc-top-actions">
        <button type="button" class="btn ghost sm" @click="reload">重载</button>
      </div>
    </div>

    <div class="pc-body">
      <template v-if="currentUrl">
        <iframe
          :key="`${activeId}-${reloadSeed}`"
          :src="currentUrl"
          :title="currentName"
          class="pc-frame"
          @error="onFrameError"
        />
      </template>
      <div v-else class="pc-empty">
        <div class="big">🖥</div>
        <p>
          <b>{{ failMessage || `「${currentName}」暂时连不上` }}</b>
        </p>
        <p class="hint">
          · 目标电脑的 AnyBoot 是否在跑？<br />
          · 设置里的「HTTP 服务」是否显示<b>正在监听</b>？<br />
          · 点上方「重载」再试一次。
        </p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.pc-page {
  display: flex;
  flex-direction: column;
  height: 100%;
}
.pc-top {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 14px;
  border-bottom: 1px solid var(--line, #e5e7eb);
  background: var(--panel, #fff);
}
.pc-chips {
  display: flex;
  gap: 4px;
  flex-wrap: wrap;
  flex: 1;
  min-width: 0;
}
.pc-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: 1px solid transparent;
  background: transparent;
  border-radius: 7px;
  padding: 5px 10px;
  font-size: 12.5px;
  color: var(--fg-dim, #4b5563);
  cursor: pointer;
}
.pc-chip:hover {
  background: var(--soft, #f3f4f6);
}
.pc-chip.active {
  background: var(--accent-soft, #eff6ff);
  border-color: var(--accent-line, #bfdbfe);
  color: var(--accent, #2563eb);
  font-weight: 600;
}
.pc-chip .dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: #16a34a;
}
.pc-chip .mini {
  font-size: 10px;
  font-weight: 400;
  color: var(--fg-faint, #9ca3af);
}
.pc-chip.add {
  color: var(--fg-faint, #9ca3af);
}
.pc-top-actions {
  flex: none;
}
.pc-body {
  flex: 1;
  min-height: 0;
  position: relative;
}
.pc-frame {
  width: 100%;
  height: 100%;
  border: 0;
  display: block;
  background: #fff;
}
.pc-empty {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: var(--fg-faint, #9ca3af);
  padding: 24px;
}
.pc-empty .big {
  font-size: 34px;
}
.pc-empty p {
  margin: 0;
  font-size: 13px;
  text-align: center;
  line-height: 1.7;
  color: var(--fg-dim, #6b7280);
}
.pc-empty .hint {
  font-size: 12px;
  color: var(--fg-faint, #9ca3af);
}
</style>
