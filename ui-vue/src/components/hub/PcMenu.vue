<script setup lang="ts">
const toast = useToastStore();
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useSettingsStore } from "@/stores/settings";
import { useToastStore } from "@/stores/toast";
import { t } from "@/i18n";
import { useEscape } from "@/composables/useEscape";

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
useEscape(open, () => { open.value = false; });
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
      ? t("menu.themeToastFollow")
      : (guestTheme.value === "K" ? t("menu.themeToastNone") : t("menu.themeToastScheme", { x: guestTheme.value })),
  );
}

/** 列表打开方式（旧版 localStorage["cqOpenMode"]） */
const openMode = ref((() => {
  try { return localStorage.getItem("cqOpenMode") === "goto" ? "goto" : "inline"; } catch { return "inline"; }
})());
/** 探测地址方式（旧版 data-reprobe：写偏好 + toast） */
/** 离线判定阈值（旧版 data-offretry：写偏好 + 清失败计数 + toast） */
function pickOffretry(n: number) {
  settings.setOfflineRetryCount(n);
  // 旧版 cqFailCount.clear()：改完立即按新阈值判定，不受旧计数影响
  void import("@/stores/remote").then(({ useRemoteStore }) => useRemoteStore().clearFailCounts());
  open.value = false;
  toast.show(t("menu.tipFailThreshold", { n }));
}

function pickReprobe(always: boolean) {
  settings.setReprobeAlways(always);
  open.value = false;
  // 旧版 toast（复刻原文案）
  toast.show(
    always
      ? t("menu.tipReprobe")
      : t("menu.tipReuse"),
  );
}

function setOpenMode(v: string) {
  openMode.value = v === "goto" ? "goto" : "inline";
  try { localStorage.setItem("cqOpenMode", openMode.value); } catch { /* 忽略 */ }
  open.value = false;
  // 旧版 toast（复刻原文案）
  toast.show(
    openMode.value === "goto"
      ? t("menu.tipGoto")
      : t("menu.tipInline"),
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
  const input = window.prompt(t("menu.promptFailReload"), String(settings.failReloadSec));
  if (input === null) return;
  const parsed = Number.parseInt(input, 10);
  if (!Number.isFinite(parsed) || parsed < 0) return;
  settings.setFailReloadSec(parsed);
  open.value = false;
}

function pickRefresh(sec: number) {
  settings.setDataRefreshSec(sec);
  open.value = false;
  // 旧版 toast（复刻原文案）
  toast.show(sec > 0 ? t("menu.toastRefreshOn", { sec }) : t("menu.toastRefreshOff"));
}

function pickCustom() {
  const input = window.prompt(t("menu.promptRefresh"), String(settings.dataRefreshSec));
  if (input === null) return;
  const parsed = Number.parseInt(input, 10);
  if (!Number.isFinite(parsed) || parsed < 0) {
    toast.show(t("menu.toastInvalid"), "bad");
    return;
  }
  settings.setDataRefreshSec(parsed);
  open.value = false;
  toast.show(parsed > 0 ? t("menu.toastRefreshOn", { sec: parsed }) : t("menu.toastRefreshOff"));
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
      :aria-label="$t('menu.more')"
      aria-haspopup="menu"
      :aria-expanded="open"
      @click.stop="toggle"
    >
      ⋮
    </button>

    <div v-if="open" id="pcMenu" class="menu" role="menu" @click.stop>
      <button role="menuitem" type="button" data-act="add" @click="menuAct('add')">{{ $t("menu.add") }}</button>
      <button role="menuitem" type="button" data-act="edit" @click="menuAct('edit')">{{ $t("menu.edit") }}</button>
      <button role="menuitem" type="button" data-act="list" @click="menuAct('list')">{{ $t("menu.list") }}</button>
      <button role="menuitem" type="button" data-act="reloadCurrent" @click="menuAct('reloadCurrent')">{{ $t("menu.reloadCurrent") }}</button>
      <button role="menuitem" type="button" data-act="reloadAll" @click="menuAct('reloadAll')">{{ $t("menu.reloadAll") }}</button>
      <button role="menuitem" type="button" id="pcConnectAllItem" :class="{ 'is-on': connectAll }" @click="toggleConnectAll">
        <span id="pcConnectAllText">{{ $t("menu.connectAll") }}</span>
      </button>
      <button role="menuitem" type="button" data-act="transport" disabled :title="$t('menu.transportTip')">{{ $t("menu.transport") }}</button>
      <div class="divider" />

      <div class="caption">{{ $t("menu.capGuest") }}</div>
      <button role="menuitem" type="button" data-guesttheme="" @click="setGuestTheme('')">{{ guestTheme === '' ? '✓ ' : '' }}{{ $t("menu.themeFollow") }}</button>
      <button role="menuitem" type="button" data-guesttheme="K" @click="setGuestTheme('K')">{{ guestTheme === 'K' ? '✓ ' : '' }}{{ $t("menu.themeNone") }}</button>
      <button role="menuitem" type="button" data-guesttheme="A" @click="setGuestTheme('A')">{{ guestTheme === 'A' ? '✓ ' : '' }}{{ $t("menu.schemeA") }}</button>
      <button role="menuitem" type="button" data-guesttheme="B" @click="setGuestTheme('B')">{{ guestTheme === 'B' ? '✓ ' : '' }}{{ $t("menu.schemeB") }}</button>
      <button role="menuitem" type="button" data-guesttheme="C" @click="setGuestTheme('C')">{{ guestTheme === 'C' ? '✓ ' : '' }}{{ $t("menu.schemeC") }}</button>
      <button role="menuitem" type="button" data-guesttheme="G" @click="setGuestTheme('G')">{{ guestTheme === 'G' ? '✓ ' : '' }}{{ $t("menu.schemeG") }}</button>
      <button role="menuitem" type="button" data-guesttheme="I" @click="setGuestTheme('I')">{{ guestTheme === 'I' ? '✓ ' : '' }}{{ $t("menu.schemeI") }}</button>
      <button role="menuitem" type="button" data-guesttheme="J" @click="setGuestTheme('J')">{{ guestTheme === 'J' ? '✓ ' : '' }}{{ $t("menu.schemeJ") }}</button>
      <div class="divider" />

      <div class="caption">{{ $t("menu.capRefresh") }}</div>
      <button role="menuitem" type="button" data-refresh="2" :class="{ 'is-on': settings.dataRefreshSec === 2 }" @click="pickRefresh(2)">{{ $t("menu.everyNDefault", { n: 2 }) }}</button>
      <button role="menuitem" type="button" data-refresh="5" :class="{ 'is-on': settings.dataRefreshSec === 5 }" @click="pickRefresh(5)">{{ $t("menu.everyN", { n: 5 }) }}</button>
      <button role="menuitem" type="button" data-refresh="10" :class="{ 'is-on': settings.dataRefreshSec === 10 }" @click="pickRefresh(10)">{{ $t("menu.everyN", { n: 10 }) }}</button>
      <button role="menuitem" type="button" data-refresh="30" :class="{ 'is-on': settings.dataRefreshSec === 30 }" @click="pickRefresh(30)">{{ $t("menu.everyN", { n: 30 }) }}</button>
      <button role="menuitem" type="button" data-refresh="0" :class="{ 'is-on': settings.dataRefreshSec === 0 }" @click="pickRefresh(0)">{{ $t("menu.offRefresh") }}</button>
      <button role="menuitem" type="button" :class="{ 'is-on': customActive }" @click="pickCustom">{{ $t("menu.custom") }}</button>
      <button role="menuitem" type="button" data-reprobe="reuse" :class="{ 'is-on': !settings.reprobeAlways }" @click="pickReprobe(false)">{{ settings.reprobeAlways ? '' : '✓ ' }}{{ $t("menu.reuseAddr") }}</button>
      <button role="menuitem" type="button" data-reprobe="always" :class="{ 'is-on': settings.reprobeAlways }" @click="pickReprobe(true)">{{ settings.reprobeAlways ? '✓ ' : '' }}{{ $t("menu.alwaysProbe") }}</button>

      <div class="divider" />
      <button
        role="menuitem"
        type="button"
        :class="{ 'is-on': !settings.reprobeAlways }"
        @click="settings.setReprobeAlways(false)"
      >
        {{ $t("menu.reuseAddrLong") }}
      </button>
      <button
        role="menuitem"
        type="button"
        :class="{ 'is-on': settings.reprobeAlways }"
        @click="settings.setReprobeAlways(true)"
      >
        {{ $t("menu.alwaysProbeLong") }}
      </button>

      <div class="divider" />
      <div class="caption">{{ $t("menu.capOffRetry") }}</div>
      <button role="menuitem" type="button" data-offretry="1" :class="{ 'is-on': settings.offlineRetryCount === 1 }" @click="pickOffretry(1)">{{ $t("menu.times1Most") }}</button>
      <button role="menuitem" type="button" data-offretry="2" :class="{ 'is-on': settings.offlineRetryCount === 2 }" @click="pickOffretry(2)">{{ $t("menu.timesN", { n: 2 }) }}</button>
      <button role="menuitem" type="button" data-offretry="3" :class="{ 'is-on': settings.offlineRetryCount === 3 }" @click="pickOffretry(3)">{{ $t("menu.timesNDefault", { n: 3 }) }}</button>
      <button role="menuitem" type="button" data-offretry="5" :class="{ 'is-on': settings.offlineRetryCount === 5 }" @click="pickOffretry(5)">{{ $t("menu.timesN", { n: 5 }) }}</button>

      <div class="divider" />
      <div class="caption">{{ $t("menu.capFailReload") }}</div>
      <button role="menuitem" type="button" data-reload="5" :class="{ 'is-on': settings.failReloadSec === 5 }" @click="settings.setFailReloadSec(5)">{{ $t("menu.everyNDefault", { n: 5 }) }}</button>
      <button role="menuitem" type="button" data-reload="10" :class="{ 'is-on': settings.failReloadSec === 10 }" @click="settings.setFailReloadSec(10)">{{ $t("menu.everyN", { n: 10 }) }}</button>
      <button role="menuitem" type="button" data-reload="30" :class="{ 'is-on': settings.failReloadSec === 30 }" @click="settings.setFailReloadSec(30)">{{ $t("menu.everyN", { n: 30 }) }}</button>
      <button role="menuitem" type="button" data-reload="60" :class="{ 'is-on': settings.failReloadSec === 60 }" @click="settings.setFailReloadSec(60)">{{ $t("menu.everyN", { n: 60 }) }}</button>
      <button role="menuitem" type="button" data-reload="0" :class="{ 'is-on': settings.failReloadSec === 0 }" @click="settings.setFailReloadSec(0)">{{ $t("menu.never") }}</button>
      <button role="menuitem" type="button" data-reload="custom" @click="customReload">{{ $t("menu.custom") }}</button>

      <div class="divider" />
      <div class="caption">{{ $t("menu.capOpenMode") }}</div>
      <button role="menuitem" type="button" data-openmode="inline" :class="{ 'is-on': openMode === 'inline' }" @click="setOpenMode('inline')">{{ $t("menu.openInline") }}</button>
      <button role="menuitem" type="button" data-openmode="goto" :class="{ 'is-on': openMode === 'goto' }" @click="setOpenMode('goto')">{{ $t("menu.openGoto") }}</button>

      <div class="divider" />
      <div class="caption">{{ $t("menu.capGrubFmt") }}</div>
      <button role="menuitem" type="button" data-grubfmt="minimal" :class="{ 'is-on': grubFmt === 'minimal' }" @click="setGrubFmt('minimal')">{{ $t("menu.grubMinimal") }}</button>
      <button role="menuitem" type="button" data-grubfmt="simple" :class="{ 'is-on': grubFmt === 'simple' }" @click="setGrubFmt('simple')">{{ $t("menu.grubSimple") }}</button>
      <button role="menuitem" type="button" data-grubfmt="medium" :class="{ 'is-on': grubFmt === 'medium' }" @click="setGrubFmt('medium')">{{ $t("menu.grubMedium") }}</button>
      <button role="menuitem" type="button" data-grubfmt="raw" :class="{ 'is-on': grubFmt === 'raw' }" @click="setGrubFmt('raw')">{{ $t("menu.grubRaw") }}</button>
      <button role="menuitem" type="button" data-grubfmt="detailed" :class="{ 'is-on': grubFmt === 'detailed' }" @click="setGrubFmt('detailed')">{{ $t("menu.grubDetailed") }}</button>

      <div class="divider" />
      <div class="caption">{{ $t("menu.capOrder") }}</div>
      <button role="menuitem" type="button" data-order="custom" :class="{ 'is-on': order === 'custom' }" @click="setOrderMode('custom')">{{ $t("menu.orderCustom") }}</button>
      <button role="menuitem" type="button" data-order="name" :class="{ 'is-on': order === 'name' }" @click="setOrderMode('name')">{{ $t("menu.orderName") }}</button>
      <button role="menuitem" type="button" data-order="recent" :class="{ 'is-on': order === 'recent' }" @click="setOrderMode('recent')">{{ $t("menu.orderRecent") }}</button>
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
