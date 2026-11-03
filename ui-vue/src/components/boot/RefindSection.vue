<script setup lang="ts">
const toast = useToastStore();
import { computed, ref } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import { call } from "@/api/tauri";
import GrubList from "@/components/boot/GrubList.vue";
import type { EntryData } from "@/components/boot/types";
import { useToastStore } from "@/stores/toast";

/**
 * rEFInd 区块（原 `ui/index.html` 的 `section#refind`，125~195 行）。
 *
 * ## 1:1 结构
 * ```
 * section.refind#refind
 *   .grub-title-row > .zone-tag#refind-zone-tag + h2
 *   #refind-panel-auto（自动扫描）
 *     section.sim-scan#sim-scan
 *       .sim-scan-head > h3 + small + button#sim-scan-refresh
 *       p#sim-scan-summary
 *       div#sim-scan-issues
 *       div#sim-scan-list
 *       details.sim-scan-hidden#sim-scan-hidden
 *         summary（「被隐藏的 N 项」）+ #sim-scan-hidden-list
 *   #refind-panel-manual（手动处理）
 *     section#systems
 *   .grub-tabs#refind-tabs > 2 × button.grub-tab[data-refind-tab]
 *   section.directory                  变量目录
 *     label + input#vars-dir + .actions（4 个按钮）
 * ```
 */

const ui = useUiStateStore();
const settings = useSettingsStore();

/** 自动 / 手动面板（旧版 #refind-tabs） */
const activePane = ref<"auto" | "manual">("auto");
const varsDir = ref("");

const sim = computed(() => ui.refindSim);

/** 模拟扫描的条目（旧版 #sim-scan-list） */
interface SimChild {
  title?: string;
  source?: string;
  options?: string;
  note?: string;
}
interface SimEntry {
  title?: string;
  kind?: string;
  hidden?: boolean;
  lastBoot?: boolean;
  children?: SimChild[];
}
/** 旧版 refindSimEntryNodes：children>1 → 组（子项挂 submenu 供 GrubList 分组）；
 *  ≤1 → 普通行（不值得展开）。子行 tag = 来源标签（旧版 REFIND_SIM_CHILD_SOURCE）。 */
const CHILD_SOURCE: Record<string, string> = {
  default: "默认项",
  options: "refind_linux.conf",
  kernel: "折叠内核",
  macos: "macOS 诊断",
  stanza: "submenuentry",
};
const visibleEntries = computed<EntryData[]>(() => {
  const out: EntryData[] = [];
  for (const e of (sim.value?.entries ?? []) as SimEntry[]) {
    if (e.hidden) continue;
    const title = (e.title || "").trim();
    const children = e.children || [];
    if (children.length > 1) {
      for (const c of children) {
        out.push({
          selector: (c.title || "").trim(),
          name: (c.title || "").trim(),
          class: e.kind,
          submenu: title,
          tag: c.source === "default" ? CHILD_SOURCE.default || "" : CHILD_SOURCE[c.source || ""] || "",
        });
      }
    } else {
      out.push({ selector: title, name: title, class: e.kind });
    }
  }
  return out;
});

/** 被隐藏的条目（旧版 #sim-scan-hidden） */
const hiddenEntries = computed<EntryData[]>(() =>
  (sim.value?.hidden ?? []).map((e) => ({
    selector: e.title.trim(),
    name: e.title.trim(),
    class: e.kind,
  })),
);

/** 当前项：lastBoot 命中的那条（与托盘同一口径） */
const activeSelector = computed(() => {
  if (settings.refindMode === "manual") {
    return ui.status?.variables?.find((v) => v.active)?.source ?? "";
  }
  return sim.value?.entries?.find((e) => e.lastBoot)?.title.trim() ?? "";
});

async function refreshSim() {
  // 旧版：refind_sim_scan → renderRefindSim（结果直接渲染，不等全量 refresh）
  try {
    const result = await call("refind_sim_scan");
    if (result) ui.applySnapshot({ ...(ui.state ?? {}), refindSim: result } as never, { force: true });
  } finally {
    void call("refresh").catch(() => {});
  }
}

function openVarsDir() {
  void call("open_vars_dir").catch(() => {});
}

function useDirectory() {
  const dir = varsDir.value.trim();
  void call("set_vars_dir", { dir }).catch(() => {});
}

function scan() {
  void call("refresh").catch(() => {});
}

function unmountTemp() {
  void call("unmount_temporary").catch(() => {});
}

async function selectEntry(selector: string) {
  try {
    if (activePane.value === "manual") {
      await call("switch_system", { system: selector });
    } else {
      await call("refind_sim_switch", { title: selector });
    }
    toast.show("已切换下次启动项");
  } catch (e) {
    toast.show("切换失败：" + e, "bad");
  }
}
</script>

<template>
  <section aria-label="rEFInd 启动项" class="refind" id="refind">
    <div class="grub-title-row">
      <span class="zone-tag" id="refind-zone-tag">rEFInd 区</span>
      <h2>rEFInd 启动项</h2>
    </div>

    <!-- 面板一：自动扫描 -->
    <div
      v-show="activePane === 'auto'"
      class="grub-panel"
      id="refind-panel-auto"
      role="tabpanel"
    >
      <section aria-label="模拟引导项扫描" class="sim-scan" id="sim-scan">
        <div class="sim-scan-head">
          <h3>模拟引导项扫描</h3>
          <small
            >按 rEFInd 规则复算 · 点条目展开第二层（rEFInd 里按 Tab / + / F2 进入）·
            「下次启动」写 vars/PreviousBoot</small
          >
          <button
            type="button"
            id="sim-scan-refresh"
            class="secondary"
            title="重新读一遍 ESP 与 refind.conf，重算菜单"
            @click="refreshSim"
          >
            重新模拟
          </button>
        </div>

        <p class="sim-scan-summary" id="sim-scan-summary">{{ sim?.summary || "" }}</p>

        <div class="sim-scan-issues" id="sim-scan-issues">
          <p v-for="(issue, i) in sim?.issues ?? []" :key="i" class="sim-issue">
            {{ issue.text }}
          </p>
        </div>

        <div class="sim-scan-list" id="sim-scan-list">
          <GrubList
            :entries="visibleEntries"
            :active-selector="activeSelector"
            @select="selectEntry"
          />
        </div>

        <details v-if="hiddenEntries.length" class="sim-scan-hidden" id="sim-scan-hidden">
          <summary>被隐藏的 <b id="sim-scan-hidden-count">{{ hiddenEntries.length }}</b> 项（rEFInd 菜单里看不到）</summary>
          <div class="sim-scan-list" id="sim-scan-hidden-list">
            <GrubList :entries="hiddenEntries" :active-selector="''" />
          </div>
        </details>
      </section>
    </div>

    <!-- 面板二：手动处理 -->
    <div
      v-show="activePane === 'manual'"
      class="grub-panel"
      id="refind-panel-manual"
      role="tabpanel"
    >
      <section aria-label="可启动系统" id="systems">
        <GrubList
          :entries="
            (ui.status?.variables ?? [])
              .filter((v) => v.exists)
              .map((v) => ({ selector: v.source, name: v.displayName, class: v.system }))
          "
          :active-selector="activeSelector"
          @select="selectEntry"
        />
      </section>
    </div>

    <!-- 两个横向标签 -->
    <div class="grub-tabs" id="refind-tabs" role="tablist" aria-label="rEFInd 读取方式">
      <button
        type="button"
        id="refind-tab-auto"
        class="grub-tab"
        :class="{ 'is-active': activePane === 'auto' }"
        role="tab"
        :aria-selected="activePane === 'auto'"
        data-refind-tab="auto"
        @click="activePane = 'auto'"
      >
        自动扫描
      </button>
      <button
        type="button"
        id="refind-tab-manual"
        class="grub-tab"
        :class="{ 'is-active': activePane === 'manual' }"
        role="tab"
        :aria-selected="activePane === 'manual'"
        data-refind-tab="manual"
        @click="activePane = 'manual'"
      >
        手动处理
      </button>
    </div>

    <!-- 变量目录（旧版 section.directory） -->
    <section class="directory">
      <label for="vars-dir">变量目录</label>
      <input
        id="vars-dir"
        v-model="varsDir"
        placeholder="留空则自动扫描 EFI/refind/vars"
        spellcheck="false"
        type="text"
      />
      <div class="actions">
        <button type="button" id="open-vars-dir" class="secondary" @click="openVarsDir">打开变量目录</button>
        <button type="button" id="use-directory" @click="useDirectory">使用此目录</button>
        <button
          v-if="ui.status?.temporaryMount"
          type="button"
          id="unmount-temporary"
          class="secondary"
          @click="unmountTemp"
        >
          卸载临时挂载
        </button>
        <button type="button" id="refresh" class="secondary" @click="scan">扫描</button>
      </div>
    </section>
  </section>
</template>
