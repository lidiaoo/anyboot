<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import { useBootAdvStore, type LoaderId } from "@/stores/bootAdv";
import ThemeHost from "@/pages/boot/themes/index.vue";
import BootList from "@/components/boot/BootList.vue";
import BootSettings from "./BootSettings.vue";
import { loaderMeta } from "@/pages/boot/themes/loaders";
import type { LoaderInfo } from "@/pages/boot/themes/types";

/**
 * 引导页（原 `ui/index.html` + `ui/app.js` + `ui/boot-adv.js` 的「系统引导」）。
 *
 * ## 结构变化
 *
 * 旧实现：`bootBuild(l)` 把真实的 `#refind` / `#grub` / `#channel-xxx` 节点
 * 搬进主题容器。**节点只有一份**，所以切主题只能靠搬。
 *
 * 这里：主题组件拿到的是 `LoaderInfo[]` **数据**，自己决定怎么渲染；
 * 条目数据由 `BootList` 按 loader id 从 store 取。切主题 = 换组件，数据不动。
 *
 * ## K（不套用主题）
 *
 * 不渲染任何主题组件，退回「每个引导器一张独立卡片」的原生排布。
 */

const ui = useUiStateStore();
const settings = useSettingsStore();
const adv = useBootAdvStore();

onMounted(async () => {
  await ui.start();
  // 验收信息写进 trace（macOS 无法 attach devtools）
  window.setTimeout(() => {
    const line =
      `[vue-boot] ready=${ui.ready ? 1 : 0} rev=${ui.revision} ` +
      `theme=${adv.effectiveTheme} layout=${adv.effectiveLayout} ` +
      `enabled=[${adv.enabledLoaders.join(",")}] ` +
      `refindMode=${settings.refindMode} refindSim=${ui.refindSim?.entries?.length ?? 0} ` +
      `grubMode=${settings.grubMode} grubEntries=${(settings.grubMode === "manual" ? ui.grub : (ui.linuxGrub ?? ui.grub))?.entries?.length ?? 0} ` +
      `channels=${ui.channels.length}`;
    void import("@/api/tauri")
      .then(({ isTauri, call }) =>
        isTauri()
          ? call<void>("hub_report_error", { source: "vue-boot", message: line, stack: "" })
          : undefined,
      )
      .catch(() => {});
  }, 3000);
});

/** 把 store 数据整理成主题需要的 LoaderInfo[] */
const loaders = computed<LoaderInfo[]>(() => {
  const list: LoaderInfo[] = [];
  for (const id of adv.enabledLoaders) {
    const meta = loaderMeta(id);
    const channel = ui.channels.find((c) => c.id === id);
    const detected = id === "refind" || id === "grub" ? true : Boolean(channel?.detected);
    const entryCount = countEntries(id);
    list.push({
      id,
      name: meta.name,
      state: !detected ? "off" : meta.perm === "nvram" ? "warn" : "ok",
      entries: new Array(entryCount).fill(0),
      esp: id === "grub" ? ui.grub?.efiDir : undefined,
      perm: meta.perm,
      detected,
    });
  }
  return list;
});

/** 条目数（与 BootList 同口径的数据源） */
function countEntries(id: LoaderId | string): number {
  if (id === "refind") {
    return settings.refindMode === "manual"
      ? (ui.status?.variables ?? []).filter((v) => v.exists).length
      : (ui.refindSim?.entries ?? []).filter((e) => !e.hidden).length;
  }
  if (id === "grub") {
    const source = settings.grubMode === "manual" ? ui.grub : (ui.linuxGrub ?? ui.grub);
    return source?.entries?.length ?? 0;
  }
  const ch = ui.channels.find((c) => c.id === id);
  if (!ch) return 0;
  return (ch.menuPreview?.entries ?? ch.entries ?? []).length;
}

const activeId = computed(() => adv.enabledLoaders[0] ?? "");

function onArrange() {
  // 编排面板尚未迁移，先把用户带到「引导高级设置」
  void import("@/api/tauri").then(({ call }) => call("open_config_dir").catch(() => {}));
}

function onSelect(selector: string) {
  void import("@/api/tauri").then(({ call }) => call("switch_grub", { selector }).catch(() => {}));
}

/** 显示设置面板（原「引导高级设置」页） */
const settingsOpen = ref(false);

const themeOptions = [
  { key: "G", label: "G 命令面板" },
  { key: "A", label: "A 卡片流" },
  { key: "B", label: "B 左栏通道" },
  { key: "C", label: "C 顶部固定" },
  { key: "I", label: "I Bento" },
  { key: "J", label: "J 三栏" },
  { key: "K", label: "不套用主题" },
] as const;
</script>

<template>
  <div class="boot-page" :class="[`ba-theme-${adv.effectiveTheme}`, `ba-layout-${adv.effectiveLayout}`]">
    <!-- 主题切换条（阶段 2 临时 UI；正式版由「引导高级设置」承担） -->
    <div class="theme-bar">
      <span class="lbl">主题</span>
      <button
        v-for="opt in themeOptions"
        :key="opt.key"
        type="button"
        :class="['tbtn', { 'is-on': adv.effectiveTheme === opt.key }]"
        @click="adv.setTheme(opt.key)"
      >
        {{ opt.label }}
      </button>
      <span class="grow" />
      <span class="rev">rev {{ ui.revision }}</span>
      <button type="button" class="tbtn" :class="{ 'is-on': settingsOpen }" @click="settingsOpen = !settingsOpen">
        引导高级设置
      </button>
    </div>

    <BootSettings v-if="settingsOpen" class="settings-host" />

    <ThemeHost
      :theme="adv.effectiveTheme"
      :layout="adv.effectiveLayout"
      :loaders="loaders"
      :active-id="activeId"
      :on-select="onSelect"
      :on-arrange="onArrange"
    >
      <!-- K：不套用主题的原生排布 -->
      <div class="k-native">
        <section v-for="loader in loaders" :key="loader.id" class="k-card">
          <h3>{{ loader.name }} <small>{{ loader.entries.length }} 个条目</small></h3>
          <BootList :loader="loader.id" />
        </section>
      </div>
    </ThemeHost>

    <p v-if="!ui.ready" class="waiting">等待快照…</p>
  </div>
</template>

<style scoped>
.boot-page {
  padding: 14px 16px 24px;
  min-height: 100%;
}
.settings-host {
  margin-bottom: 18px;
}
.theme-bar {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  margin-bottom: 14px;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--line, #e5e7eb);
}
.lbl {
  font-size: 11.5px;
  color: var(--fg-faint, #9ca3af);
  margin-right: 4px;
}
.tbtn {
  font-size: 11.5px;
  padding: 4px 10px;
  border-radius: 999px;
  border: 1px solid var(--line, #d1d5db);
  background: var(--panel, #fff);
  color: var(--fg-dim, #6b7280);
  cursor: pointer;
}
.tbtn.is-on {
  background: var(--accent, #2563eb);
  border-color: var(--accent, #2563eb);
  color: #fff;
}
.grow {
  flex: 1;
}
.rev {
  font-size: 11px;
  color: var(--fg-faint, #9ca3af);
  font-family: ui-monospace, Menlo, monospace;
}
.k-native {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.k-card {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 10px;
  padding: 10px 12px;
  background: var(--panel, #fff);
}
.k-card h3 {
  font-size: 13px;
  margin: 0 0 8px;
  color: var(--fg-dim, #6b7280);
}
.k-card h3 small {
  font-weight: normal;
  color: var(--fg-faint, #9ca3af);
  margin-left: 6px;
}
.waiting {
  color: var(--fg-faint, #9ca3af);
  font-size: 13px;
}
</style>
