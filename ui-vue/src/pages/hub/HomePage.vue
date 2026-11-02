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
/** 电源悬浮球展开态 */
const fabOpen = ref(false);
/** 电源帮助弹层（旧版 #fabHelpOverlay） */
const fabHelpOpen = ref(false);

/** 帮助正文（旧版 #fabHelpText，原文照抄） */
const FAB_HELP_TEXT =
  "唤醒 = 发魔术包让电脑开机 —— 需要配好 MAC 并在主板开启「网络唤醒 / Wake on LAN」。" +
  "睡眠 = 电脑挂起到内存，网页服务立刻停掉，会显示离线，要叫醒请点「唤醒」。" +
  "关机 = 电脑立刻关机，网页服务随之中断，关机后只能用「唤醒」开机或到机器旁按电源键。" +
  "重启 = 电脑立刻重启，网页会话断开，重启完成后会自动恢复在线。" +
  "没配电源路由时走这台电脑自己的 /api/power（需要能连上它）；配了路由则发局域网 sol 包。";
/** 标签 ⋮ 菜单 */
const chipMenuOpen = ref(false);

/** 电源动作（旧版 #pcFabWrap 的四个 chip） */
const POWER_ACTIONS = [
  { act: "wake", label: "唤醒" },
  { act: "sleep", label: "睡眠" },
  { act: "shutdown", label: "关机" },
  { act: "reboot", label: "重启" },
] as const;

function sendPower(action: string) {
  void import("@/api/tauri").then(({ call }) =>
    call("remote_power", { serverId: activeId.value, action }).catch(() => {}),
  );
}

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
      <div class="pc-frame-host">
        <iframe
          v-if="currentUrl"
          :key="`${activeId}-${reloadSeed}`"
          :src="currentUrl"
          :title="currentName"
          class="pc-frame"
          @error="onFrameError"
        />
        <div v-else class="pc-empty">
          <div>
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
              stroke-linecap="round"
              aria-hidden="true"
            >
              <rect x="3" y="4" width="18" height="16" rx="2" />
              <path d="M3 9h18M8 4v5" />
            </svg>
            <p><b>{{ failMessage || "选择上方标签查看对应电脑的页面" }}</b></p>
            <p>
              本机 = 这台电脑的主界面；其余标签 = 已添加的 AnyBoot 电脑。点右上角 ⋮
              可添加电脑、重载页面等；想看全部电脑的状态请进「电脑列表」。
            </p>
          </div>
        </div>
      </div>

      <!-- 重载悬浮按钮（旧版 #pcRefreshFab） -->
      <button
        type="button"
        class="pc-refresh-fab"
        title="重载当前页面（重新探测全部地址，谁在线用谁）"
        aria-label="重载当前页面"
        @click="reload"
      >
        <svg
          viewBox="0 0 24 24"
          width="17"
          height="17"
          fill="none"
          stroke="currentColor"
          stroke-width="1.9"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M21 12a9 9 0 1 1-2.6-6.4" />
          <path d="M21 3v6h-6" />
        </svg>
      </button>

      <!-- 电源悬浮球（旧版 #pcFabWrap） -->
      <div class="pc-fab" :hidden="!fabOpen">
        <span class="pc-fab-note" title="电源操作说明">电源走局域网 · 需 sol</span>
        <button type="button" class="pc-fab-help" title="电源操作说明" @click="fabHelpOpen = true">
          ?
        </button>
        <button
          v-for="p in POWER_ACTIONS"
          :key="p.act"
          type="button"
          class="pc-fab-chip"
          :data-pact="p.act"
          @click="sendPower(p.act)"
        >
          {{ p.label }}<span class="hint" />
        </button>
      </div>
      <button
        type="button"
        class="pc-fab-ball"
        :aria-expanded="fabOpen"
        aria-label="电源操作"
        @click="fabOpen = !fabOpen"
      >
        ⏻
      </button>
    </div>

    <!-- 电源 · 睡眠与唤醒 帮助弹层（旧版 #fabHelpOverlay） -->
    <div v-if="fabHelpOpen" class="overlay" @click.self="fabHelpOpen = false">
      <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="fabHelpTitle">
        <div class="dialog-head">
          <h3 id="fabHelpTitle">电源 · 睡眠与唤醒</h3>
          <button type="button" class="icon-btn" aria-label="关闭" @click="fabHelpOpen = false">✕</button>
        </div>
        <div class="dialog-body">
          <p class="fab-help-text">{{ FAB_HELP_TEXT }}</p>
        </div>
        <div class="dialog-foot">
          <span class="spacer" />
          <button type="button" class="btn" @click="fabHelpOpen = false">知道了</button>
        </div>
      </div>
    </div>

    <!-- 标签 ⋮ 菜单（旧版 #chipMenu） -->
    <div v-if="chipMenuOpen" class="pc-menu" role="menu" aria-label="标签菜单">
      <button role="menuitem" @click="chipMenuOpen = false">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 20h9" /><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z" /></svg>
        编辑这个服务端
      </button>
      <button role="menuitem" @click="chipMenuOpen = false">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="13" rx="2" /><path d="M8 21h8M12 17v4" /></svg>
        系统信息（系统 / CPU / 内存）
      </button>
      <button role="menuitem" @click="chipMenuOpen = false; reload()">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 12a9 9 0 1 1-2.6-6.4" /><path d="M21 3v6h-6" /></svg>
        重载此页（重新探测地址）
      </button>
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
