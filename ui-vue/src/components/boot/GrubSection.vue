<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import { call } from "@/api/tauri";
import GrubList from "@/components/boot/GrubList.vue";
import type { EntryData } from "@/components/boot/types";

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

/** 列表格式（旧版 #grub-list-format） */
const settings = useSettingsStore();

function setFormat(format: string) {
  void call("set_grub_list_format", { format }).catch(() => {});
}

function installAnyboot() {
  void call("grub_install", {}).catch(() => {});
}

function uninstallAnyboot() {
  void call("grub_uninstall", {}).catch(() => {});
}

function saveScript() {
  void call("set_grub_script", { text: scriptText.value }).catch(() => {});
}

function resetScript() {
  scriptText.value = "";
  void call("set_grub_script", { text: "" }).catch(() => {});
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

const espDir = computed(() => grubManual.value?.efiDir ?? "");
const partuuid = computed(() => grubManual.value?.partuuid ?? "");

function refreshAuto() {
  void call("linux_grub_scan", { target: null }).catch(() => {});
}

function syncManual() {
  void call("refresh").catch(() => {});
}

function pickEspDir() {
  void call("pick_grub_dir").catch(() => {});
}

function detectEsp() {
  void call("detect_esp", { value: espDir.value }).catch(() => {});
}

function saveEspDir() {
  void call("set_grub_dir", { dir: espDir.value }).catch(() => {});
}

function openGrubDir() {
  void call("open_grub_dir").catch(() => {});
}

function detectPartuuid() {
  void call("detect_esp", { value: espDir.value }).catch(() => {});
}

function savePartuuid() {
  void call("set_grub_partuuid", { partuuid: partuuid.value }).catch(() => {});
}

function toggleMount() {
  void call("set_grub_mount", { keep: !grubManual.value?.keepMounted }).catch(() => {});
}

function switchEntry(selector: string) {
  if (activePane.value === "auto") {
    void call("linux_grub_switch", { selector }).catch(() => {});
  } else {
    void call("switch_grub", { selector }).catch(() => {});
  }
}
</script>

<template>
  <section aria-label="GRUB 启动项" class="grub" id="grub">
    <div class="grub-head">
      <div class="grub-title-row">
        <span class="zone-tag" id="grub-zone-tag">GRUB 区</span>
        <h2>GRUB 启动项</h2>
      </div>
      <small class="grub-pane-status" role="status" />
      <small>{{ grubManual?.syncedAt ? `同步于 ${grubManual.syncedAt}` : "" }}</small>
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
            class="secondary"
            title="按下方选的 ESP 分区重新定位 Linux 分区并读取"
            @click="refreshAuto"
          >
            重新读取
          </button>
        </div>

        <p class="grub-auto-summary">{{ grubAuto?.confidenceNote || "" }}</p>
        <div class="grub-auto-issues">
          <p v-for="(issue, i) in grubAuto?.issues ?? []" :key="i" class="sim-issue">
            {{ issue.text }}
          </p>
        </div>

        <div class="grub-auto-card">
          <div v-if="grubAuto?.target" class="grub-auto-target">
            {{ grubAuto.target }}
          </div>
          <div class="grub-list">
            <GrubList
              :entries="autoEntries"
              :active-selector="activeSelector"
              :group-titles="groupTitles"
              @select="switchEntry"
            />
          </div>
        </div>

        <details v-if="(grubAuto?.candidates?.length ?? 0) > 1" class="grub-auto-picker">
          <summary>换一块分区读</summary>
          <div class="grub-auto-candidates">
            <p v-for="(c, i) in grubAuto?.candidates ?? []" :key="i" class="mono">
              {{ typeof c === "string" ? c : JSON.stringify(c) }}
            </p>
          </div>
        </details>

        <small class="grub-auto-foot">{{ grubAuto?.via || "" }}</small>
      </section>
    </div>

    <!-- 面板二：ESP 手动解析 -->
    <div
      v-show="activePane === 'manual'"
      class="grub-panel"
      id="grub-panel-manual"
      role="tabpanel"
    >
      <p class="grub-panel-note">
        读的是 ESP 上的 <code>/EFI/grub.cfg</code> 与 <code>/EFI/grubenv</code>：在 Linux 上装
        <code>09_anyboot</code> 后，每次 <code>update-grub</code> 会把它同步过来。
      </p>

      <div class="grub-list">
        <GrubList
          :entries="manualEntries"
          :active-selector="activeSelector"
          :group-titles="groupTitles"
          @select="switchEntry"
        />
      </div>

      <!-- 安装 / 卸载 09_anyboot（旧版 #grub-install-actions） -->
      <div class="actions">
        <button type="button" class="secondary" @click="installAnyboot">安装 09_anyboot</button>
        <button type="button" class="secondary" @click="uninstallAnyboot">卸载 09_anyboot</button>
      </div>

      <!-- 执行过程（旧版 #grub-log-box） -->
      <details v-if="grubManual?.scriptLog" class="grub-log">
        <summary>执行过程</summary>
        <pre>{{ grubManual.scriptLog }}</pre>
      </details>

      <!-- 内置脚本编辑（旧版 details.grub-script） -->
      <details class="grub-script">
        <summary>查看 / 修改内置脚本（09_anyboot）</summary>
        <small
          >安装时只把 <code>@@ESP_UUID@@</code> 换成下方「ESP 分区」里填的 fs-uuid，其余按这里的正文来；
          清空后保存 = 恢复内置模板</small
        >
        <textarea v-model="scriptText" rows="8" spellcheck="false" />
        <div class="actions">
          <button type="button" class="secondary" @click="saveScript">保存脚本</button>
          <button type="button" class="secondary" @click="resetScript">恢复内置</button>
        </div>
      </details>

      <!-- 引导配置须知（旧版 .grub-advice，原文照抄） -->
      <small class="grub-advice">
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

      <!-- ESP 配置（旧版 section.directory） -->
      <section class="directory">
        <label for="grub-dir">ESP 目录</label>
        <input
          id="grub-dir"
          :value="espDir"
          placeholder="H:\ 或 /boot/efi 或 /Volumes/ARCH-EFI（分区里任意路径都行）"
          spellcheck="false"
          type="text"
        />
        <div class="actions">
          <button type="button" class="secondary" @click="pickEspDir">选择文件夹…</button>
          <button type="button" class="secondary" @click="detectEsp">识别</button>
          <button type="button" class="secondary" @click="saveEspDir">保存</button>
          <button type="button" class="secondary" @click="openGrubDir">打开文件管理器</button>
        </div>

        <label for="grub-partuuid">分区 UUID</label>
        <input
          id="grub-partuuid"
          :value="partuuid"
          placeholder="F69D7BF9-E8A9-3135-A5CB-4BD0CA7344E3"
          spellcheck="false"
          type="text"
        />
        <div class="actions">
          <button type="button" class="secondary" @click="detectPartuuid">识别</button>
          <button type="button" class="secondary" @click="savePartuuid">保存</button>
          <button type="button" class="secondary" @click="toggleMount">
            {{ grubManual?.keepMounted ? "停止保持挂载" : "保持挂载" }}
          </button>
          <button type="button" @click="syncManual">同步</button>
        </div>
      </section>
    </div>

    <hr aria-hidden="true" class="boot-divider" />

    <!-- 两个横向标签 -->
    <div class="grub-tabs" id="grub-tabs" role="tablist" aria-label="GRUB 读取方式">
      <button
        type="button"
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

    <!-- 列表格式（旧版 #grub-format-row） -->
    <label class="setting" id="grub-format-row">
      <span class="setting-text">
        <strong>列表格式</strong>
        <small>同名启动项的合并与文字详略</small>
      </span>
      <select
        :value="settings.grubListFormat"
        @change="setFormat(($event.target as HTMLSelectElement).value)"
      >
        <option value="minimal">极简</option>
        <option value="simple">简易</option>
        <option value="medium">中等</option>
        <option value="complex">复杂</option>
      </select>
    </label>
  </section>
</template>
