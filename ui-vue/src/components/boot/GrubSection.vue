<script setup lang="ts">
/** hideList=true 时只渲染设置部分 */
const props = defineProps<{ hideList?: boolean }>();
const toast = useToastStore();
import { computed, ref, watch } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import { call } from "@/api/tauri";
import GrubList from "@/components/boot/GrubList.vue";
import type { EntryData } from "@/components/boot/types";
import { useToastStore } from "@/stores/toast";

/**
 * GRUB 区块（原 `ui/index.html` 的 `section#grub`，197~290 行）。
 *
 * ## 1:1 结构
 * ```
 * section.grub#grub
 *   .grub-head > .grub-title-row(.zone-tag + h2) + small#grub-status + small#grub-meta
 *   #grub-panel-auto（自动读取 Linux 分区）
 *     section.grub-auto#grub-auto
 *       .grub-auto-head > h3 + button#grub-auto-refresh
 *       p.grub-auto-summary
 *       .grub-auto-issues
 *       .grub-auto-card > #grub-auto-target + #grub-auto-list
 *       details.grub-auto-picker > summary + .grub-auto-candidates
 *       small.grub-auto-foot
 *   #grub-panel-manual（ESP 手动解析）
 *     p.grub-panel-note
 *     #grub-list
 *     #grub-install-actions（安装/卸载 09_anyboot）
 *     details#grub-log-box（执行日志）
 *     #grub-script（09_anyboot 正文编辑）
 *     section.directory（ESP 配置：目录 / 分区 UUID / 挂载）
 *   .grub-tabs#grub-tabs > 2 × button.grub-tab
 *   #grub-format-row（列表格式选择）
 * ```
 */

const ui = useUiStateStore();

const activePane = ref<"auto" | "manual">("auto");

/** 09_anyboot 脚本正文（旧版 #grub-script） */
const scriptText = ref("");
/** GRUB 错误提示（旧版 #grub-error） */
/** 候选分区（旧版 #grub-auto-candidates；从 grubAuto 取） */
const autoCandidates = computed<string[]>(() => {
  const list = (grubAuto.value as any)?.candidates;
  return Array.isArray(list) ? list.map((c: any) => (typeof c === "string" ? c : c?.path ?? String(c))) : [];
});
function pickAutoCandidate(path: string) {
  void import("@/api/tauri").then(({ call }) => call("set_grub_dir", { dir: path }).catch(() => {}));
}

/** 列表格式（旧版 #grub-list-format） */
const settings = useSettingsStore();

function setFormat(format: string) {
  void call("set_grub_list_format", { format }).catch(() => {});
}

function installAnyboot() {
  void call("install_grub_script", {}).catch(() => {});
}

function uninstallAnyboot() {
  void call("uninstall_grub_script", {}).catch(() => {});
}

function saveScript() {
  void call("set_grub_script", { script: scriptText.value }).catch(() => {});
}

function resetScript() {
  scriptText.value = "";
  void call("set_grub_script", { script: "" }).catch(() => {});
}

const grubAuto = computed(() => ui.linuxGrub);

// 脚本正文：优先用户改过的，否则用快照里的当前生效值
watch(
  () => ui.state?.grubScript,
  (text) => {
    if (!scriptText.value && text) scriptText.value = text;
  },
  { immediate: true },
);
const grubManual = computed(() => ui.grub);

/** 自动读取的条目 */
const autoEntries = computed<EntryData[]>(() =>
  (grubAuto.value?.entries ?? []).map((e) => ({
    selector: e.selector,
    name: e.name,
    class: e.class,
    kernels: e.kernels,
    variants: e.variants,
    chainloader: e.chainloader,
    submenu: e.submenu,
  })),
);

/** 手动解析的条目 */
const manualEntries = computed<EntryData[]>(() =>
  (grubManual.value?.entries ?? []).map((e) => ({
    selector: e.selector,
    name: e.name,
    class: e.class,
    kernels: e.kernels,
    variants: e.variants,
    chainloader: e.chainloader,
    submenu: e.submenu,
  })),
);

/** 组标题映射 */
const groupTitles = computed(() => {
  const map: Record<string, string> = {};
  const source = activePane.value === "auto" ? grubAuto.value : grubManual.value;
  for (const g of source?.groups ?? []) map[g.id ?? ""] = g.name ?? "";
  return map;
});

const activeSelector = computed(() => {
  const source = activePane.value === "auto" ? grubAuto.value : grubManual.value;
  return source?.nextEntry || source?.savedEntry || "";
});


const autoBusy = ref(false);
async function refreshAuto() {
  if (autoBusy.value) return;
  autoBusy.value = true;
  ui.setBootNotice("正在读取…");
  try {
    // 「重新读取」：循环重试直到读到条目或达到上限（期间按钮保持“读取中…”）。
    // 不带 auto → Elevation::Allow（首轮可能弹一次授权）。
    const MAX_TRIES = 1; // T8: 扫描已快，不再连打多次（失败一次即报错）
    let lastErr = "";
    let okN = 0;
    for (let i = 0; i < MAX_TRIES; i++) {
      const view = await call<{ error?: string | null; entries?: unknown[] } | null>(
        "linux_grub_scan",
        { target: null },
      ).catch((e) => ({ error: String(e) } as { error: string; entries?: unknown[] }));
      const n = view?.entries?.length ?? 0;
      if (view?.error) lastErr = view.error;
      if (n > 0) { okN = n; break; }
      if (i < MAX_TRIES - 1) await new Promise((r) => setTimeout(r, 1500));
    }
    if (okN > 0) {
      ui.setBootNotice(`重新读取成功：识别到 ${okN} 个启动项`);
    } else if (lastErr) {
      ui.setBootNotice("重新读取失败：" + lastErr, true);
    } else {
      ui.setBootNotice("重新读取完成，但没有识别到启动项（检查分区/免密授权）", true);
    }
    void call("refresh").catch(() => {});
  } catch (e) {
    ui.setBootNotice("重新读取失败：" + e, true);
  } finally {
    autoBusy.value = false;
  }
}









function pickEspDir() {
  void call("pick_grub_dir").catch(() => {});
}
async function switchEntry(selector: string) {
  try {
    if (activePane.value === "auto") {
      await call("linux_grub_switch", { selector });
    } else {
      await call("switch_grub", { selector });
    }
    toast.show("已切换下次启动项");
  } catch (e) {
    toast.show("切换失败：" + e, "bad");
  }
}
</script>

<template>
  <section aria-label="GRUB 启动项" class="grub" id="grub">
    <div v-if="!props.hideList" class="grub-head">
      <div class="grub-title-row">
        <span class="zone-tag" id="grub-zone-tag">GRUB 区</span>
        <h2>GRUB 启动项</h2>
      </div>
      <small class="grub-pane-status" id="grub-status" role="status" />
      <small id="grub-meta">{{ grubManual?.syncedAt ? `同步于 ${grubManual.syncedAt}` : "" }}</small>
    </div>

    <!-- 两个横向标签 -->
    <div class="grub-tabs" id="grub-tabs" role="tablist" aria-label="GRUB 读取方式">
      <button
        type="button"
        id="grub-tab-auto"
        class="grub-tab"
        :class="{ 'is-active': activePane === 'auto' }"
        role="tab"
        :aria-selected="activePane === 'auto'"
        data-grub-tab="auto"
        @click="activePane = 'auto'"
      >
        自动读取
      </button>
      <button
        type="button"
        id="grub-tab-manual"
        class="grub-tab"
        :class="{ 'is-active': activePane === 'manual' }"
        role="tab"
        :aria-selected="activePane === 'manual'"
        data-grub-tab="manual"
        @click="activePane = 'manual'"
      >
        手动解析
      </button>
    </div>

    <!-- 面板一：自动读取 Linux 分区 -->
    <div
      v-show="activePane === 'auto'"
      class="grub-panel"
      id="grub-panel-auto"
      role="tabpanel"
    >
      <section aria-label="自动读取（Linux 分区自己的 GRUB）" class="grub-auto" id="grub-auto">
        <div class="grub-auto-head">
          <h3>直接读 Linux 分区自己的 grub.cfg <small>不用装 09_anyboot</small></h3>
          <button
            type="button"
            id="grub-auto-refresh"
            class="secondary"
            :disabled="autoBusy"
            title="按下方选的 ESP 分区重新定位 Linux 分区并读取"
            @click="refreshAuto"
          >
            {{ autoBusy ? "读取中…" : "重新读取" }}
          </button>
        </div>

        <!-- 扫描摘要 + 问题：收进「重新读取」下方小滚动区（默认字号，细滚动条） -->
        <div class="grub-auto-note">
          <p class="grub-auto-summary" id="grub-auto-summary">{{ grubAuto?.confidenceNote || "" }}</p>
          <div class="grub-auto-issues" id="grub-auto-issues">
            <p v-for="(issue, i) in grubAuto?.issues ?? []" :key="i" class="sim-issue">
              {{ issue.text }}
            </p>
          </div>
        </div>

        <div class="grub-auto-card" id="grub-auto-card">
          <div v-if="grubAuto?.target" class="grub-auto-target" id="grub-auto-target">
            {{ grubAuto.target }}
          </div>
          <div class="grub-list" id="grub-auto-list">
            <GrubList
              v-if="!props.hideList"
              :entries="autoEntries"
              :active-selector="activeSelector"
              :group-titles="groupTitles"
              :format="settings.grubListFormat"
              @select="switchEntry"
            />
          </div>
        </div>

        <details v-if="(grubAuto?.candidates?.length ?? 0) > 1" class="grub-auto-picker" id="grub-auto-picker">
          <summary>换一块分区读</summary>
          <div class="grub-auto-candidates">
            <p v-for="(c, i) in grubAuto?.candidates ?? []" :key="i" class="mono">
              {{ typeof c === "string" ? c : JSON.stringify(c) }}
            </p>
          </div>
        </details>

        <!-- 候选分区（旧版 #grub-auto-picker） -->
        <details v-if="autoCandidates.length" id="grub-auto-picker" class="grub-auto-picker">
          <summary id="grub-auto-picker-summary">换一块分区读</summary>
          <div id="grub-auto-candidates" class="grub-auto-candidates">
            <button v-for="c in autoCandidates" :key="c" type="button" class="secondary" @click="pickAutoCandidate(c)">{{ c }}</button>
          </div>
        </details>
        <small class="grub-auto-foot" id="grub-auto-foot">{{ grubAuto?.via || "" }}</small>
      </section>
    </div>

    <!-- 面板二：ESP 手动解析 -->
    <div
      v-show="activePane === 'manual'"
      class="grub-panel"
      id="grub-panel-manual"
      role="tabpanel"
    >
      <p class="grub-panel-note" id="grub-hint">
        读的是 ESP 上的 <code>/EFI/grub.cfg</code> 与 <code>/EFI/grubenv</code>：在 Linux 上装
        <code>09_anyboot</code> 后，每次 <code>update-grub</code> 会把它同步过来。
      </p>

      <div class="grub-list" id="grub-list">
        <GrubList
          v-if="!props.hideList"
          :entries="manualEntries"
          :active-selector="activeSelector"
          :group-titles="groupTitles"
          :format="settings.grubListFormat"
          @select="switchEntry"
        />
        <!-- 空状态（旧版 #grub-empty），无条目时显示 -->
        <div v-if="!manualEntries.length" id="grub-empty" class="grub-empty-state">
          <strong>还没识别到 GRUB 启动信息</strong>
          <small>选 ESP 分区里的文件夹去识别，或直接填下面的 fs-uuid —— 两种方式填一个就行，另一个会自动对上</small>
          <div class="actions">
            <button type="button" class="secondary" id="grub-empty-pick" @click="pickEspDir">选择文件夹识别…</button>
          </div>
        </div>
      </div>

      <!-- 安装 / 卸载 09_anyboot（旧版 #grub-install-actions） -->
      <div class="actions" id="grub-install-actions">
        <button type="button" class="secondary" id="grub-install" @click="installAnyboot">安装 09_anyboot</button>
        <button type="button" class="secondary" id="grub-uninstall" @click="uninstallAnyboot">卸载 09_anyboot</button>
      </div>

      <!-- 执行过程（旧版 #grub-log-box） -->
      <details v-if="grubManual?.scriptLog" id="grub-log-box" class="grub-log">
        <summary id="grub-log-summary">执行过程</summary>
        <pre id="grub-log">{{ grubManual.scriptLog }}</pre>
      </details>

      <!-- 内置脚本编辑（旧版 details.grub-script） -->
      <details class="grub-script">
        <summary>查看 / 修改内置脚本（09_anyboot）</summary>
        <small
          >安装时只把 <code>@@ESP_UUID@@</code> 换成下方「ESP 分区」里填的 fs-uuid，其余按这里的正文来；
          清空后保存 = 恢复内置模板</small
        >
        <textarea id="grub-script" v-model="scriptText" rows="8" spellcheck="false" />
        <div class="actions">
          <button type="button" class="secondary" id="grub-script-save" @click="saveScript">保存脚本</button>
          <button type="button" class="secondary" id="grub-script-reset" @click="resetScript">恢复内置</button>
        </div>
      </details>

      <!-- 引导配置须知（旧版 .grub-advice，原文照抄） -->
      <small class="grub-advice" id="grub-advice">
        <strong>引导配置须知</strong>
        <ul>
          <li>
            Linux 必须开启 <strong>os-prober</strong>：在 <code>/etc/default/grub</code> 里加
            <code>GRUB_DISABLE_OS_PROBER=false</code> 后执行 update-grub，其他系统的启动项才会进
            grub.cfg；装了多个 Linux 发行版时，只需让<strong>一个</strong>发行版管理 EFI（它的 GRUB +
            os-prober 收集其余系统）。
          </li>
          <li>
            Windows 的 EFI 分区，GPT 标志必须是 <code>boot, efi</code>（gparted 里给该分区设 boot
            标志），否则 os-prober 不识别它。
          </li>
          <li>
            <strong>EFI 第一启动项</strong>：用 rEFInd 就把 rEFInd 的 EFI 设为第一启动项；
            不用 rEFInd 就把 GRUB 的 EFI 设为第一启动项。
          </li>
        </ul>
      </small>

    </div>

      <!-- ESP 共享表单已剥离为 GrubEsp.vue（LoaderSettings grub 分支独立渲染） -->



    <hr aria-hidden="true" class="boot-divider" id="boot-divider" />


    <!-- 列表格式（旧版 #grub-format-row） -->
    <div class="grub-row" id="grub-format-row">
      <label for="grub-list-format">列表格式</label>
      <select
        id="grub-list-format"
        :value="settings.grubListFormat"
        @change="setFormat(($event.target as HTMLSelectElement).value)"
      >
        <option value="raw">原始</option>
        <option value="minimal">极简</option>
        <option value="simple">简易</option>
        <option value="medium">中等</option>
        <option value="detailed">复杂</option>
      </select>
    </div>
  </section>
</template>
