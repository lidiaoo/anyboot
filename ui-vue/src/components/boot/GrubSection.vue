<script setup lang="ts">
/** hideList=true 时只渲染设置部分 */
const props = defineProps<{ hideList?: boolean }>();
const toast = useToastStore();
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import { call, setLoaderMode } from "@/api/tauri";
import GrubList from "@/components/boot/GrubList.vue";
import type { EntryData } from "@/components/boot/types";
import { useToastStore } from "@/stores/toast";
const { t } = useI18n();

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
const settings = useSettingsStore();

/** 自动 / 手动面板。**由后端 Settings.grubMode 驱动**（v0.2.5 同口径）：
 *  切 tab 必须 set_loader_mode 落盘，否则后端还是 auto、列表/托盘判定不跟着变。 */
const activePane = ref<"auto" | "manual">(
  settings.grubMode === "manual" ? "manual" : "auto",
);
watch(
  () => settings.grubMode,
  (mode) => {
    const want = mode === "manual" ? "manual" : "auto";
    if (activePane.value !== want) activePane.value = want;
  },
);
async function setGrubTab(name: "auto" | "manual"): Promise<void> {
  const changed = activePane.value !== name;
  activePane.value = name;
  if (changed) {
    try {
      await setLoaderMode("grub", name);
    } catch (e) {
      ui.setBootNotice(t("grub.switchFail", { msg: String(e) }), true);
    }
  }
}

/** 09_anyboot 脚本正文（旧版 #grub-script） */
const scriptText = ref("");
/** GRUB 错误提示（旧版 #grub-error） */
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

/** 自动读取命中的 Linux 分区：可读文本（对象 → 「label（dev）」，别直接渲染 [object Object]） */
const autoTargetText = computed(() => {
  const t = (grubAuto.value as any)?.target;
  if (!t) return "";
  if (typeof t === "string") return t;
  const label = t.label || t.dev || "";
  const dev = t.dev || "";
  return label && dev && label !== dev ? `${label}（${dev}）` : label || dev;
});
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
  ui.setBootNotice(t("grub.reading"));
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
    // 统一文案：扫描不到条目只提示「GRUB」扫描完成，但没有识别到条目（不标红、不带长诊断）。
    if (okN > 0) {
      ui.setBootNotice(t("grub.scanDone", { name: "GRUB", n: okN }));
    } else if (lastErr) {
      ui.setBootNotice(t("grub.scanFail", { name: "GRUB", msg: String(lastErr) }), true);
    } else {
      ui.setBootNotice(t("grub.scanEmpty", { name: "GRUB" }));
    }
    void call("refresh").catch(() => {});
  } catch (e) {
    ui.setBootNotice(t("grub.rereadFail", { msg: String(e) }), true);
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
  } catch (e) {
    toast.show(t("grub.switchFailToast", { msg: String(e) }), "bad");
  }
}
</script>

<template>
  <section :aria-label="t('grub.sectionTitle', { name: 'GRUB' })" class="grub" id="grub">
    <div v-if="!props.hideList" class="grub-head">
      <div class="grub-title-row">
        <span class="zone-tag" id="grub-zone-tag">{{ t("grub.zone", { name: "GRUB" }) }}</span>
        <h2>{{ t("grub.sectionTitle", { name: "GRUB" }) }}</h2>
      </div>
      <small class="grub-pane-status" id="grub-status" role="status" />
      <small id="grub-meta">{{ grubManual?.syncedAt ? t("grub.syncedAt", { t: grubManual.syncedAt }) : "" }}</small>
    </div>

    <!-- 两个横向标签 -->
    <div class="grub-tabs" id="grub-tabs" role="tablist" :aria-label="t('grub.tabsAria', { name: 'GRUB' })">
      <button
        type="button"
        id="grub-tab-auto"
        class="grub-tab"
        :class="{ 'is-active': activePane === 'auto' }"
        role="tab"
        :aria-selected="activePane === 'auto'"
        data-grub-tab="auto"
        @click="setGrubTab('auto')"
      >
        {{ t("grub.tabAuto") }}
      </button>
      <button
        type="button"
        id="grub-tab-manual"
        class="grub-tab"
        :class="{ 'is-active': activePane === 'manual' }"
        role="tab"
        :aria-selected="activePane === 'manual'"
        data-grub-tab="manual"
        @click="setGrubTab('manual')"
      >
        {{ t("grub.tabManual") }}
      </button>
    </div>

    <!-- 面板一：自动读取 Linux 分区 -->
    <div
      v-show="activePane === 'auto'"
      class="grub-panel"
      id="grub-panel-auto"
      role="tabpanel"
    >
      <section :aria-label="t('grub.autoAria')" class="grub-auto" id="grub-auto">
        <div class="grub-auto-head">
          <h3>{{ t("grub.autoTitle") }} <small>{{ t("grub.autoTitleSub") }}</small></h3>
          <button
            type="button"
            id="grub-auto-refresh"
            class="secondary"
            :disabled="autoBusy"
            :title="t('grub.reprobeTitle')"
            @click="refreshAuto"
          >
            {{ autoBusy ? t("grub.reading") : t("grub.reread") }}
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
          <div v-if="autoTargetText" class="grub-auto-target" id="grub-auto-target">
            {{ autoTargetText }}
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
        {{ t("grub.espReadHint") }}
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
          <strong>{{ t("grub.notFoundTitle", { name: "GRUB" }) }}</strong>
          <small>{{ t("grub.notFoundHint") }}</small>
          <div class="actions">
            <button type="button" class="secondary" id="grub-empty-pick" @click="pickEspDir">{{ t("grub.pickDir") }}</button>
          </div>
        </div>
      </div>

      <!-- 安装 / 卸载 09_anyboot（旧版 #grub-install-actions） -->
      <div class="actions" id="grub-install-actions">
        <button type="button" class="secondary" id="grub-install" @click="installAnyboot">{{ t("grub.install") }}</button>
        <button type="button" class="secondary" id="grub-uninstall" @click="uninstallAnyboot">{{ t("grub.uninstall") }}</button>
      </div>

      <!-- 执行过程（旧版 #grub-log-box） -->
      <details v-if="grubManual?.scriptLog" id="grub-log-box" class="grub-log">
        <summary id="grub-log-summary">{{ t("grub.logSummary") }}</summary>
        <pre id="grub-log">{{ grubManual.scriptLog }}</pre>
      </details>

      <!-- 内置脚本编辑（旧版 details.grub-script） -->
      <details class="grub-script">
        <summary>{{ t("grub.scriptSummary") }}</summary>
        <small
          >{{ t("grub.scriptHint") }}</small
        >
        <textarea id="grub-script" v-model="scriptText" rows="8" spellcheck="false" />
        <div class="actions">
          <button type="button" class="secondary" id="grub-script-save" @click="saveScript">{{ t("grub.scriptSave") }}</button>
          <button type="button" class="secondary" id="grub-script-reset" @click="resetScript">{{ t("grub.scriptReset") }}</button>
        </div>
      </details>

      <!-- 引导配置须知（旧版 .grub-advice，原文照抄） -->
      <small class="grub-advice" id="grub-advice">
        <strong>{{ t("grub.noticeTitle") }}</strong>
        <ul>
          <li>
            {{ t("grub.notice1") }}
          </li>
          <li>
            {{ t("grub.notice2") }}
          </li>
          <li>
            {{ t("grub.notice3") }}
          </li>
        </ul>
      </small>

    </div>

      <!-- ESP 共享表单已剥离为 GrubEsp.vue（LoaderSettings grub 分支独立渲染） -->



    <hr aria-hidden="true" class="boot-divider" id="boot-divider" />



    <!-- 列表格式（旧版 #grub-format-row） -->
    <div class="grub-row" id="grub-format-row">
      <label for="grub-list-format">{{ t("grub.listFormat") }}</label>
      <select
        id="grub-list-format"
        :value="settings.grubListFormat"
        @change="setFormat(($event.target as HTMLSelectElement).value)"
      >
        <option value="raw">{{ t("hub.picker.fmt.raw") }}</option>
        <option value="minimal">{{ t("hub.picker.fmt.minimal") }}</option>
        <option value="simple">{{ t("hub.picker.fmt.simple") }}</option>
        <option value="medium">{{ t("hub.picker.fmt.medium") }}</option>
        <option value="detailed">{{ t("hub.picker.fmt.detailed") }}</option>
      </select>
    </div>
  </section>
</template>
