<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useSettingsStore } from "@/stores/settings";

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

function label(sec: number): string {
  if (sec === 0) return "关闭数据刷新";
  return `每 ${sec} 秒${sec === 2 ? "（默认）" : ""}`;
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

    <div v-if="open" class="menu" role="menu" @click.stop>
      <div class="caption">数据刷新</div>
      <button
        v-for="sec in refreshOptions"
        :key="sec"
        role="menuitem"
        type="button"
        :class="{ 'is-on': settings.dataRefreshSec === sec }"
        @click="pickRefresh(sec)"
      >
        {{ label(sec) }}
      </button>
      <button role="menuitem" type="button" :class="{ 'is-on': customActive }" @click="pickCustom">
        自定义间隔…
      </button>

      <div class="divider" />
      <div class="caption">刷新时是否重新探测地址</div>
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
      <button
        v-for="n in [1, 2, 3, 5]"
        :key="n"
        role="menuitem"
        type="button"
        :class="{ 'is-on': settings.offlineRetryCount === n }"
        @click="settings.setOfflineRetryCount(n)"
      >
        {{ n }} 次{{ n === 3 ? "（默认）" : "" }}
      </button>

      <div class="divider" />
      <div class="caption">连不上时自动重载</div>
      <button
        v-for="sec in [5, 10, 30, 60, 0]"
        :key="`fail-${sec}`"
        role="menuitem"
        type="button"
        :class="{ 'is-on': settings.failReloadSec === sec }"
        @click="settings.setFailReloadSec(sec)"
      >
        {{ sec === 0 ? "永不（关闭自动重载）" : `每 ${sec} 秒${sec === 5 ? "（默认）" : ""}` }}
      </button>
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
