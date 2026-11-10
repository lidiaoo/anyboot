<script setup lang="ts">
/** hideList=true 时只渲染设置部分（供各主题「引导器设置」折叠面板用） */
const props = defineProps<{ hideList?: boolean }>();
const toast = useToastStore();
const settings = useSettingsStore();
import { computed, ref, watch } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import { call, setLoaderMode } from "@/api/tauri";
import GrubList from "@/components/boot/GrubList.vue";
import { simRows, refindVariableRows } from "@/pages/boot/bootRows";
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

/** 自动 / 手动面板（旧版 #refind-tabs）。
 *  **由后端 Settings.refindMode 驱动**（与 v0.2.5 同口径）：切 tab 必须
 *  set_loader_mode 落盘，否则只改前端显示、后端 refindMode 还是 auto →
 *  列表（BootList）/托盘的「自动 vs 手动」判定不跟着变，表现为「我改成手动了
 *  却还显示自动扫描的条目」。
 *  默认自动（用户 2026-10-29 定案「默认进自动」）。 */
const activePane = ref<"auto" | "manual">(
  settings.refindMode === "manual" ? "manual" : "auto",
);
// 后端设置变了（托盘/另一处改过 / 切换落盘后快照回来）→ 对账页签位置
watch(
  () => settings.refindMode,
  (mode) => {
    const want = mode === "manual" ? "manual" : "auto";
    if (activePane.value !== want) activePane.value = want;
  },
);
/** 用户点 tab：先切显示，再把读取方式落盘（后端 refindMode 是权威值）。 */
async function setRefindTab(name: "auto" | "manual"): Promise<void> {
  const changed = activePane.value !== name;
  activePane.value = name;
  if (changed) {
    try {
      await setLoaderMode("refind", name);
    } catch (e) {
      ui.setBootNotice("切换读取方式失败：" + e, true);
    }
  }
}
const varsDir = ref("");

const sim = computed(() => ui.refindSim);

/** 模拟扫描的条目（旧版 #sim-scan-list） */
const visibleEntries = computed<EntryData[]>(() => {
  if (settings.refindMode === "manual") {
    return refindVariableRows(ui.status?.variables);
  }
  return simRows(sim.value);
});
/** 旧版 activeSelector：自动=sim lastBoot title；手动=活动变量 source */
const activeSelector = computed<string>(() => {
  if (settings.refindMode === "manual") {
    return ui.status?.variables?.find((v) => v.active)?.source ?? "";
  }
  // 原文返回（与 selector 原文相等，回显高亮才能命中；rEFInd 标题末尾常带空格）
  return sim.value?.entries?.find((e) => e.lastBoot)?.title ?? "";
});

/** 被隐藏的条目（旧版 #sim-scan-hidden：refindSimTree(hidden, {plain:true})，
 *  同样按 children>1 展开组/子行 —— 旧实测 11 行） */
const hiddenEntries = computed<EntryData[]>(() =>
  simRows({ entries: ((sim.value?.hidden ?? []) as { title?: string; kind?: string; hidden?: boolean; children?: { title?: string; source?: string }[] }[]).map((e) => ({ ...e, hidden: false })) }),
);

const simBusy = ref(false);
async function refreshSim() {
  if (simBusy.value) return;
  simBusy.value = true;
  ui.setBootNotice("正在读取…");
  try {
    // 循环重试直到读到条目或达上限；期间按钮保持“读取中…”
    const MAX_TRIES = 1; // T8: 扫描已快，不再连打多次（失败一次即报错）
    let okN = 0;
    let lastErr = "";
    let last: { entries?: unknown[]; summary?: string | null } | null = null;
    for (let i = 0; i < MAX_TRIES; i++) {
      const result = await call<{ entries?: unknown[]; summary?: string | null } | null>("refind_sim_scan")
        .catch((e) => { lastErr = String(e); return null; });
      if (result) { last = result; ui.applySnapshot({ ...(ui.state ?? {}), refindSim: result } as never, { force: true }); }
      const n = result?.entries?.length ?? 0;
      if (n > 0) { okN = n; break; }
      if (i < MAX_TRIES - 1) await new Promise((r) => setTimeout(r, 1200));
    }
    // 统一文案：扫描不到任何条目时只提示「rEFInd」扫描完成，但没有识别到条目
    // （不再带一长串诊断/error，也不标红 —— 没装/没条目是正常状态）。
    if (okN > 0) ui.setBootNotice(`「rEFInd」扫描完成，识别到 ${okN} 条`);
    else if (lastErr) ui.setBootNotice("「rEFInd」扫描失败：" + lastErr, true);
    else { void last; ui.setBootNotice("「rEFInd」扫描完成，但没有识别到条目"); }
  } catch (e) {
    ui.setBootNotice("重新读取失败：" + e, true);
  } finally {
    simBusy.value = false;
    // **不要在这里再 call("refresh")**：refind_sim_scan 已经把 sim 算好并写回
    // AppState + publish_ui，前端也已 applySnapshot。多调一次 refresh 会重新走
    // scan_vars_directory → 重新枚举挂载卷 / 提权临时挂载找 vars 目录，而刚才
    // refind_sim_scan 用的临时挂载可能已释放，于是这次 refresh 报「已扫描全部
    // 已挂载与临时挂载的 FAT/ESP 分区，但未找到 EFI/refind/vars 目录」——表现为
    // 「点一次正常、再点一次报未找到」交替。删掉它即可。
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
  } catch (e) {
    toast.show("切换失败：" + e, "bad");
  }
}
</script>

<template>
  <section aria-label="rEFInd 启动项" class="refind" id="refind">
    <div v-if="!props.hideList" class="grub-title-row">
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
            :disabled="simBusy"
            title="重新读一遍 ESP 与 refind.conf，重算菜单"
            @click="refreshSim"
          >
            {{ simBusy ? "读取中…" : "重新读取" }}
          </button>
        </div>

        <!-- 扫描摘要 + 问题：收进「重新读取」下方的小滚动区（默认字号，带细滚动条） -->
        <div class="sim-scan-note">
          <p class="sim-scan-summary" id="sim-scan-summary">{{ sim?.summary || "" }}</p>
  
          <div class="sim-scan-issues" id="sim-scan-issues">
            <p v-for="(issue, i) in sim?.issues ?? []" :key="i" class="sim-issue">
              {{ issue.text }}
            </p>
          </div>
        </div>

        <div class="sim-scan-list" id="sim-scan-list">
          <GrubList
            v-if="!props.hideList"
            :entries="visibleEntries"
            :active-selector="activeSelector"
            @select="selectEntry"
          />
        </div>

        <!-- 「被隐藏的 N 项」是**诊断信息**，不属于主列表 —— hide-list（引导器设置
             折叠面板）里也必须能展开看到，否则用户只看到「N 项」却点开是空的
             （用户报「被隐藏的项折叠没显示条目」）。所以这里去掉 v-if="!props.hideList"。 -->
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
          v-if="!props.hideList"
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
        @click="setRefindTab('auto')"
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
        @click="setRefindTab('manual')"
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
