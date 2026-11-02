<script setup lang="ts">
const toast = useToastStore();
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useSettingsStore } from "@/stores/settings";
import { useToastStore } from "@/stores/toast";

/**
 * ⋮ 菜单（原 `ui/hub.html` 的 `#cqMore` / `#pcMenu`）。
 *
 * 三个设置分组（2026-11-02 用户定案）：
 *   ① **数据刷新**：间隔 2/5/10/30/关 + 自定义（默认 2 秒）
 *   ② **刷新时是否重新探测地址**：复用上次地址（省电、地址稳）/ 每次都重探
 *   ③ **连续失败多少次才判离线**：弱网容忍（默认 3 次）
 *
 * ②③ 与「连不上时自动重载」是**三件独立的事**，别混：
 *   · 数据刷新   = 正常时多久刷一次
 *   · 重探地址   = 刷新时要不要重新选路
 *   · 失败重试   = 连不上之后要不要重试
 */

const settings = useSettingsStore();
const open = ref(false);
const rootRef = ref<HTMLElement | null>(null);

const refreshOptions = [2, 5, 10, 30, 0];

/** 引导页主题（旧版 localStorage["anyboot.hub.boot-theme"]） */
const guestTheme = ref((() => {
  try { return localStorage.getItem("anyboot.hub.boot-theme") || ""; } catch { return ""; }
})());
function setGuestTheme(v: string) {
  guestTheme.value = /^[A-K]$/.test(v) ? v : "";
  try { localStorage.setItem("anyboot.hub.boot-theme", guestTheme.value); } catch { /* 忽略 */ }
  open.value = false;
  // 旧版 toast 提示（复刻原文案）
  toast.show(
    guestTheme.value === ""
      ? "引导页主题：跟随电脑 —— 看哪台就用哪台自己保存的主题"
      : "引导页主题：" + (guestTheme.value === "K" ? "不套用主题" : "方案 " + guestTheme.value) + "（重载页面后生效，不改电脑自己保存的主题）",
  );
}

/** 列表打开方式（旧版 localStorage["cqOpenMode"]） */
const openMode = ref((() => {
  try { return localStorage.getItem("cqOpenMode") === "goto" ? "goto" : "inline"; } catch { return "inline"; }
})());
function setOpenMode(v: string) {
  openMode.value = v === "goto" ? "goto" : "inline";
  try { localStorage.setItem("cqOpenMode", openMode.value); } catch { /* 忽略 */ }
  open.value = false;
  // 旧版 toast（复刻原文案）
  toast.show(
    openMode.value === "goto"
      ? "列表里「打开页面」会跳到「电脑」页并选中那台"
      : "列表里「打开页面」用内嵌弹窗查看",
  );
}

/** GRUB 条目文字格式（旧版 set_grub_list_format） */
const grubFmt = ref("simple");
function setGrubFmt(fmt: string) {
  void import("@/api/tauri").then(({ call }) =>
    call("set_grub_list_format", { format: fmt }).then(() => { grubFmt.value = fmt; open.value = false; }).catch((e) => { toast.show("切换格式失败：" + e, "bad"); }),
  );
}

/** 排序方式（旧版 anyboot.orderMode） */
const order = ref((() => {
  try { const v = localStorage.getItem("anyboot.orderMode"); return v === "name" || v === "recent" ? v : "custom"; } catch { return "custom"; }
})());
function setOrderMode(v: string) {
  order.value = v;
  try { localStorage.setItem("anyboot.orderMode", v); } catch { /* 忽略 */ }
  open.value = false;
}

const emit = defineEmits<{ (e: "menu-action", act: string): void }>();

/** 顶部操作项（add/edit/list/reloadCurrent/reloadAll）—— 交给父组件处理 */
function menuAct(act: string) {
  open.value = false;
  emit("menu-action", act);
}
/** 启动时连接全部服务端（旧版 #pcConnectAllItem → remote_set_connect_all） */
const connectAll = ref(false);
function toggleConnectAll() {
  connectAll.value = !connectAll.value;
  void import("@/api/tauri").then(({ call }) =>
    call("remote_set_connect_all", { on: connectAll.value }).catch(() => {}),
  );
}


function customReload() {
  const input = window.prompt("连不上时每隔多少秒自动重载？（填 0 = 永不）", String(settings.failReloadSec));
  if (input === null) return;
  const parsed = Number.parseInt(input, 10);
  if (!Number.isFinite(parsed) || parsed < 0) return;
  settings.setFailReloadSec(parsed);
  open.value = false;
}

function pickRefresh(sec: number) {
  settings.setDataRefreshSec(sec);
}

function pickCustom() {
  const input = window.prompt("数据刷新间隔（秒，0 = 关闭）", String(settings.dataRefreshSec));
  if (input === null) return;
  const parsed = Number.parseInt(input, 10);
  if (!Number.isFinite(parsed) || parsed < 0) {
    window.alert("要是不小于 0 的整数秒");
    return;
  }
  settings.setDataRefreshSec(parsed);
}

function toggle() {
  open.value = !open.value;
}

/** 点外面关闭 */
function onDocClick(event: MouseEvent) {
  if (!open.value) return;
  if (rootRef.value && !rootRef.value.contains(event.target as Node)) open.value = false;
}

onMounted(() => document.addEventListener("click", onDocClick));
onUnmounted(() => document.removeEventListener("click", onDocClick));

const customActive = computed(() => !refreshOptions.includes(settings.dataRefreshSec));
</script>

<template>
  <div ref="rootRef" class="pc-menu-root">
    <button
      type="button"
      class="icon-btn"
      aria-label="更多"
      aria-haspopup="menu"
      :aria-expanded="open"
      @click.stop="toggle"
    >
      ⋮
    </button>

    <div v-if="open" id="pcMenu" class="menu" role="menu" @click.stop>
      <button role="menuitem" type="button" data-act="add" @click="menuAct('add')">添加电脑</button>
      <button role="menuitem" type="button" data-act="edit" @click="menuAct('edit')">编辑当前服务端</button>
      <button role="menuitem" type="button" data-act="list" @click="menuAct('list')">服务端列表</button>
      <button role="menuitem" type="button" data-act="reloadCurrent" @click="menuAct('reloadCurrent')">重载当前电脑</button>
      <button role="menuitem" type="button" data-act="reloadAll" @click="menuAct('reloadAll')">全部重载</button>
      <button role="menuitem" type="button" id="pcConnectAllItem" :class="{ 'is-on': connectAll }" @click="toggleConnectAll">
        <span id="pcConnectAllText">启动时连接全部服务端</span>
      </button>
      <button role="menuitem" type="button" data-act="transport" disabled title="PC 端经系统浏览器内核直连服务端页面，无需选择传输方式（手机端才需要）">传输方式（直连）</button>
      <div class="divider" />

      <div class="caption">引导页主题（看别人时 · 本机偏好）</div>
      <button role="menuitem" type="button" data-guesttheme="" @click="setGuestTheme('')">{{ guestTheme === '' ? '✓ ' : '' }}跟随电脑（不带回退）</button>
      <button role="menuitem" type="button" data-guesttheme="K" @click="setGuestTheme('K')">{{ guestTheme === 'K' ? '✓ ' : '' }}不套用主题</button>
      <button role="menuitem" type="button" data-guesttheme="A" @click="setGuestTheme('A')">{{ guestTheme === 'A' ? '✓ ' : '' }}方案 A（卡片流）</button>
      <button role="menuitem" type="button" data-guesttheme="B" @click="setGuestTheme('B')">{{ guestTheme === 'B' ? '✓ ' : '' }}方案 B（左栏通道）</button>
      <button role="menuitem" type="button" data-guesttheme="C" @click="setGuestTheme('C')">{{ guestTheme === 'C' ? '✓ ' : '' }}方案 C（顶部固定）</button>
      <button role="menuitem" type="button" data-guesttheme="G" @click="setGuestTheme('G')">{{ guestTheme === 'G' ? '✓ ' : '' }}方案 G（命令面板）</button>
      <button role="menuitem" type="button" data-guesttheme="I" @click="setGuestTheme('I')">{{ guestTheme === 'I' ? '✓ ' : '' }}方案 I（Bento）</button>
      <button role="menuitem" type="button" data-guesttheme="J" @click="setGuestTheme('J')">{{ guestTheme === 'J' ? '✓ ' : '' }}方案 J（三栏）</button>
      <div class="divider" />

      <div class="caption">数据刷新（PC 端列表）</div>
      <button role="menuitem" type="button" data-refresh="2" :class="{ 'is-on': settings.dataRefreshSec === 2 }" @click="pickRefresh(2)">每 2 秒（默认）</button>
      <button role="menuitem" type="button" data-refresh="5" :class="{ 'is-on': settings.dataRefreshSec === 5 }" @click="pickRefresh(5)">每 5 秒</button>
      <button role="menuitem" type="button" data-refresh="10" :class="{ 'is-on': settings.dataRefreshSec === 10 }" @click="pickRefresh(10)">每 10 秒</button>
      <button role="menuitem" type="button" data-refresh="30" :class="{ 'is-on': settings.dataRefreshSec === 30 }" @click="pickRefresh(30)">每 30 秒</button>
      <button role="menuitem" type="button" data-refresh="0" :class="{ 'is-on': settings.dataRefreshSec === 0 }" @click="pickRefresh(0)">关闭数据刷新</button>
      <button role="menuitem" type="button" :class="{ 'is-on': customActive }" @click="pickCustom">自定义间隔…</button>
      <button role="menuitem" type="button" data-reprobe="reuse" :class="{ 'is-on': !settings.reprobeAlways }" @click="settings.setReprobeAlways(false)">✓ 复用上次地址（省电、地址稳）</button>
      <button role="menuitem" type="button" data-reprobe="always" :class="{ 'is-on': settings.reprobeAlways }" @click="settings.setReprobeAlways(true)">每次重新探测地址（换 IP 抓得最快）</button>

      <div class="divider" />
      <button
        role="menuitem"
        type="button"
        :class="{ 'is-on': !settings.reprobeAlways }"
        @click="settings.setReprobeAlways(false)"
      >
        复用上次成功的地址（省电、地址稳）
      </button>
      <button
        role="menuitem"
        type="button"
        :class="{ 'is-on': settings.reprobeAlways }"
        @click="settings.setReprobeAlways(true)"
      >
        每次刷新都重新探测（换 IP 抓得最快）
      </button>

      <div class="divider" />
      <div class="caption">连续失败多少次才判离线（弱网容忍）</div>
      <button role="menuitem" type="button" data-offretry="1" :class="{ 'is-on': settings.offlineRetryCount === 1 }" @click="settings.setOfflineRetryCount(1)">1 次（最灵敏）</button>
      <button role="menuitem" type="button" data-offretry="2" :class="{ 'is-on': settings.offlineRetryCount === 2 }" @click="settings.setOfflineRetryCount(2)">2 次</button>
      <button role="menuitem" type="button" data-offretry="3" :class="{ 'is-on': settings.offlineRetryCount === 3 }" @click="settings.setOfflineRetryCount(3)">3 次（默认）</button>
      <button role="menuitem" type="button" data-offretry="5" :class="{ 'is-on': settings.offlineRetryCount === 5 }" @click="settings.setOfflineRetryCount(5)">5 次</button>

      <div class="divider" />
      <div class="caption">连不上时自动重载（全局）</div>
      <button role="menuitem" type="button" data-reload="5" :class="{ 'is-on': settings.failReloadSec === 5 }" @click="settings.setFailReloadSec(5)">每 5 秒（默认）</button>
      <button role="menuitem" type="button" data-reload="10" :class="{ 'is-on': settings.failReloadSec === 10 }" @click="settings.setFailReloadSec(10)">每 10 秒</button>
      <button role="menuitem" type="button" data-reload="30" :class="{ 'is-on': settings.failReloadSec === 30 }" @click="settings.setFailReloadSec(30)">每 30 秒</button>
      <button role="menuitem" type="button" data-reload="60" :class="{ 'is-on': settings.failReloadSec === 60 }" @click="settings.setFailReloadSec(60)">每 60 秒</button>
      <button role="menuitem" type="button" data-reload="0" :class="{ 'is-on': settings.failReloadSec === 0 }" @click="settings.setFailReloadSec(0)">永不（关闭自动重载）</button>
      <button role="menuitem" type="button" data-reload="custom" @click="customReload">自定义间隔…</button>

      <div class="divider" />
      <div class="caption">列表里「打开页面」的方式</div>
      <button role="menuitem" type="button" data-openmode="inline" :class="{ 'is-on': openMode === 'inline' }" @click="setOpenMode('inline')">内嵌弹窗查看</button>
      <button role="menuitem" type="button" data-openmode="goto" :class="{ 'is-on': openMode === 'goto' }" @click="setOpenMode('goto')">跳到「电脑」页</button>

      <div class="divider" />
      <div class="caption">GRUB 条目文字（全局显示格式）</div>
      <button role="menuitem" type="button" data-grubfmt="minimal" :class="{ 'is-on': grubFmt === 'minimal' }" @click="setGrubFmt('minimal')">极简（内核短名）</button>
      <button role="menuitem" type="button" data-grubfmt="simple" :class="{ 'is-on': grubFmt === 'simple' }" @click="setGrubFmt('simple')">简易（内核短名 · 带位置）</button>
      <button role="menuitem" type="button" data-grubfmt="medium" :class="{ 'is-on': grubFmt === 'medium' }" @click="setGrubFmt('medium')">中等（完整内核名）</button>
      <button role="menuitem" type="button" data-grubfmt="raw" :class="{ 'is-on': grubFmt === 'raw' }" @click="setGrubFmt('raw')">原始（菜单标题）</button>
      <button role="menuitem" type="button" data-grubfmt="detailed" :class="{ 'is-on': grubFmt === 'detailed' }" @click="setGrubFmt('detailed')">复杂（标题 + 内核）</button>

      <div class="divider" />
      <div class="caption">排序方式</div>
      <button role="menuitem" type="button" data-order="custom" :class="{ 'is-on': order === 'custom' }" @click="setOrderMode('custom')">自定义（我用 ↑ ↓ 排的）</button>
      <button role="menuitem" type="button" data-order="name" :class="{ 'is-on': order === 'name' }" @click="setOrderMode('name')">名称 A → Z</button>
      <button role="menuitem" type="button" data-order="recent" :class="{ 'is-on': order === 'recent' }" @click="setOrderMode('recent')">最近连接</button>
    </div>
  </div>
</template>

<style scoped>
.pc-menu-root {
  position: relative;
  display: inline-block;
}
.icon-btn {
  border: 0;
  background: transparent;
  font-size: 16px;
  line-height: 1;
  padding: 6px 8px;
  border-radius: 7px;
  cursor: pointer;
  color: var(--fg-dim, #4b5563);
}
.icon-btn:hover {
  background: var(--soft, #f3f4f6);
}
.menu {
  position: absolute;
  right: 0;
  top: calc(100% + 6px);
  min-width: 268px;
  background: var(--panel, #fff);
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 10px;
  box-shadow: 0 12px 32px rgba(15, 23, 42, 0.14);
  padding: 6px;
  z-index: 70;
  max-height: 70vh;
  overflow-y: auto;
}
.caption {
  font-size: 10.5px;
  color: var(--fg-faint, #9ca3af);
  text-transform: uppercase;
  letter-spacing: 0.06em;
  padding: 7px 10px 4px;
}
.menu button[role="menuitem"] {
  display: block;
  width: 100%;
  text-align: left;
  border: 0;
  background: transparent;
  font-size: 12.5px;
  padding: 6px 10px;
  border-radius: 6px;
  cursor: pointer;
  color: var(--fg, #111827);
}
.menu button[role="menuitem"]:hover {
  background: var(--soft, #f3f4f6);
}
.menu button.is-on {
  color: var(--accent, #2563eb);
  font-weight: 600;
}
.menu button.is-on::before {
  content: "✓ ";
}
.divider {
  height: 1px;
  background: var(--line, #f1f5f9);
  margin: 4px 6px;
}
</style>
