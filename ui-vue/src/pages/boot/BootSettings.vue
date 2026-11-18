<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import { useBootAdvStore, type LoaderId } from "@/stores/bootAdv";
import { useBootInstancesStore } from "@/stores/bootInstances";
import { loaderMeta } from "@/pages/boot/themes/loaders";
import { simRows, refindVariableRows, grubRows } from "@/pages/boot/bootRows";
import type { EntryData } from "@/components/boot/types";
import ThemeHost from "@/pages/boot/themes/index.vue";
import type { LoaderInfo } from "@/pages/boot/themes/types";
import { THEME_LABELS, THEME_LAYOUT_MODES, type LayoutMode } from "@/pages/boot/themes/registry";

import { call } from "@/api/tauri";
import { setStartupScanWithAdmin, setHelperReminderSnooze, setLinuxExt4ScanMode, isTauri, linuxPrivInstall, linuxPrivUninstall, type LinuxPrivStatus } from "@/api/tauri";
import { useToastStore } from "@/stores/toast";
const { t } = useI18n();
/**
 * 引导高级设置面板（原 `ui/boot-adv.js` 的设置页部分 + `index.html` 的设置区）。
 *
 * 三块：
 *   ① **版式主题**：K / A / B / C / G / I / J + 布局档位（部分主题只有加宽）
 *   ② **启用与排序**：勾选哪些引导器、上下移动
 *   ③ **应用设置**：vars 目录、开机自启、收进托盘、关闭行为、临时挂载
 *
 * 主题与排序存在 localStorage（`stores/bootAdv`），应用设置走后端命令。
 */

const ui = useUiStateStore();
const settings = useSettingsStore();
const adv = useBootAdvStore();
/** 多实例：引导高级设置也按实例平铺（老端无实例时回退固定 6 类） */
const bootInstances = ref<any[]>([]);
const instancePrefs = useBootInstancesStore();
function instanceIdOf(item: any): string { return String(item?.instanceId || item?.kind || ""); }
function instanceOf(id: string): any { return bootInstances.value.find((item) => instanceIdOf(item) === id); }
function baseKindOf(id: string): string { return String(instanceOf(id)?.kind || id); }
function metaOf(id: string): any { return loaderMeta(baseKindOf(id) as LoaderId); }
function displayNameOf(id: string): string {
  const inst = instanceOf(id);
  return String(inst?.title || metaOf(id).name);
}
const orderIds = computed<string[]>(() => bootInstances.value.length ? instancePrefs.state.order : adv.state.order);
function isVisible(id: string): boolean { return bootInstances.value.length ? instancePrefs.isVisible(id) : adv.state.visible[id as LoaderId] !== false; }
function sideOf(id: string): string { return bootInstances.value.length ? instancePrefs.sideOf(id) : (adv.state.side[id] === "r" ? "r" : "l"); }
function resetOrder(): void {
  if (bootInstances.value.length) instancePrefs.reset(bootInstances.value);
  else adv.resetOrder();
}

/** 旧版 moveLoader（boot-adv.js）：order 内上/下移一位 */
function moveLoader(id: string, delta: number) {
  const order = [...orderIds.value];
  const i = order.indexOf(id);
  const j = i + delta;
  if (i < 0 || j < 0 || j >= order.length) return;
  order.splice(j, 0, ...order.splice(i, 1));
  if (bootInstances.value.length) instancePrefs.move(id, delta);
  else adv.setOrder(order as LoaderId[]);
}

/** 引导器默认项（旧版 def = l.nextBoot || entries.find(def)） */
function loaderDefault(id: string): string {
  if (id === "refind") {
    const hit = (ui.status?.variables ?? []).find((v) => v.active);
    const lb = (ui.refindSim?.entries ?? []).find((e) => e.lastBoot);
    return (lb?.title || hit?.displayName || t("boot.unknown")).toString();
  }
  if (id === "refindplus") {
    // 与 refind 同口径，看 refindplus 通道的独立槽位
    const hit = (ui.refindplusVariables ?? []).find((v) => v.active);
    const lb = (ui.refindplusSim?.entries ?? []).find((e) => e.lastBoot);
    return (lb?.title || hit?.displayName || t("boot.unknown")).toString();
  }
  if (id === "grub") {
    const g = settings.grubMode === "manual" ? ui.grub : ui.linuxGrub;
    return (g?.nextEntry || g?.savedEntry || t("boot.unknown")).toString();
  }
  const ch = (ui.channels ?? []).find((c) => c.id === id);
  // 旧版 channelEntries：currentDefault 可能是 id 或标题，命中 channel.entries 取标题
  const cur = String(ch?.currentDefault || "");
  const list = ((ch?.entries ?? []) as { id?: string; title?: string }[]);
  const hit = cur
    ? list.find((e) => cur === e.id || cur === e.title || cur.includes(e.title || "\u0000"))
    : undefined;
  return (hit?.title || cur || (ch?.detected ? t("hub.state.notSelected") : t("boot.unknown"))).toString();
}
/** 引导器图标里的两个字母（旧版 ldr 图标文字） */
function loaderMono(id: string): string {
  return metaOf(id).mono;
}
/** 免密提权总入口：切换 root 助手（boot_helper install/uninstall） */
const helperBusy = ref(false);
/** 是否桌面端(Tauri)：免密提权助手的管理只在桌面本机；移动端禁用交互 */
const isDesktop = isTauri();
const toast = useToastStore();
/** 助手操作的阶段文案（给按钮/状态当"进行中"反馈，让用户知道在等什么） */
const helperBusyLabel = ref("");
/** 启动时使用管理员扫描（默认开） */
async function toggleStartupAdminScan(): Promise<void> {
  const cur = ui.settings?.startupScanWithAdmin !== false;
  await setStartupScanWithAdmin(!cur).catch(() => {});
}
/** 当前平台（后端 SettingsView.os 给出，可靠；不回退 navigator） */
const currentOs = computed(() => String(ui.settings?.os ?? ""));
const isLinux = computed(() => currentOs.value === "linux");

/** Linux 免密组件（polkit/sudo 助手）状态：**从 UI 快照读**（响应式）。
 *  放快照而不是进页面时请求一次 —— 后者装完 / 切标签都不会刷新
 *  （用户报「点开启后数据不刷新、切标签也没用」）。 */
const privStatus = computed<LinuxPrivStatus | null>(() => {
  const lp = ui.linuxPriv as LinuxPrivStatus | null | undefined;
  return lp ?? null;
});
const privBusy = ref(false);

/** 进行中的动作：install / uninstall，用来决定按钮显示「开启中…」「关闭中…」 */
const privBusyAction = ref<"install" | "uninstall" | "">("");
/** Linux 免密状态一句话 + 是否需要「安装」按钮 */
const privStateText = computed(() => {
  const st = privStatus.value;
  if (!st) return "";
  if (st.state === "ready") return t("bs.helperReady");
  if (st.state === "outdated") return t("bs.helperOutdated", { v: st.version || "?", c: st.currentVersion });
  if (st.state === "stale") return t("bs.helperStale");
  if (st.state === "absent") return t("bs.helperAbsent");
  return st.detail || t("boot.unknown");
});
const privInstalled = computed(
  () => privStatus.value?.state === "ready",
);
/**
 * 开启 / 关闭 Linux 免密组件。严格的三态：
 *   点下 → 立即「开启中…」/「关闭中…」（按钮转圈、禁用）
 *   → 等后端真正返回成功
 *   → 主动拉到新快照确认 state 变了，才显示「已开启 / 未开启」
 * 失败（取消授权等）→ 回到原状态 + 错误提示，绝不停在中间态。
 */
async function toggleLinuxPriv(): Promise<void> {
  if (privBusy.value) return;
  const install = !privInstalled.value;
  privBusy.value = true;
  privBusyAction.value = install ? "install" : "uninstall";
  try {
    if (install) {
      await linuxPrivInstall(null);
    } else {
      await linuxPrivUninstall(null);
    }
    // 后端已在内部 publish_ui，但推送可能有延迟；主动拉一次快照，
    // 确保按钮「立刻」从「开启中」切到「已开启」（不等推送那一拍）。
    await ui.pollOnce().catch(() => {});
    toast.show(install ? t("bs.linuxHelperOn") : t("bs.linuxHelperOff"));
  } catch (e) {
    toast.show(t("bs.linuxHelperFail", { msg: String(e) }), "bad");
  } finally {
    privBusy.value = false;
    privBusyAction.value = "";
  }
}
/** Linux 分区扫描策略（仅 Linux 显示） */
const LINUX_EXT4_MODES = [
  { key: "raw", label: "bs.readMode.raw", hint: "bs.readMode.rawHint" },
  { key: "prefer_mount", label: "bs.readMode.preferMount", hint: "bs.readMode.preferMountHint" },
  { key: "mount_only", label: "bs.readMode.mountOnly", hint: "bs.readMode.mountOnlyHint" },
] as const;
const linuxExt4Mode = computed(() => String(ui.settings?.linuxExt4ScanMode ?? "raw"));
const linuxExt4Hint = computed(
  () => LINUX_EXT4_MODES.find((m) => m.key === linuxExt4Mode.value)?.hint ?? "",
);
async function pickLinuxExt4Mode(mode: "raw" | "prefer_mount" | "mount_only"): Promise<void> {
  await setLinuxExt4ScanMode(mode).catch(() => {});
}
/** 「免密助手提醒」的取消提醒到期时间文案 */
const snoozeLabel = computed(() => {
  const until = Number(ui.settings?.helperReminderSnoozeUntil ?? 0);
  const now = Math.floor(Date.now() / 1000);
  if (!until || until <= now) return "";
  if (until > now + 3600 * 24 * 365 * 40) return t("bs.snoozeForever");
  const d = Math.ceil((until - now) / 86400);
  return t("bs.snoozeDays", { d });
});
async function snoozeHelperReminder(days: number): Promise<void> {
  const until = days === 0 ? 4_102_444_800 : Math.floor(Date.now() / 1000) + days * 86400;
  await setHelperReminderSnooze(until).catch(() => {});
}
async function clearSnooze(): Promise<void> {
  await setHelperReminderSnooze(0).catch(() => {});
}
async function toggleBootHelper(): Promise<void> {
  if (helperBusy.value) return;
  const h = ui.state?.helper;
  if (!h) return;
  const action = h.installed && !h.stale ? "uninstall" : "install";
  helperBusy.value = true;
  helperBusyLabel.value =
    action === "install"
      ? (h.installed ? t("bs.privReinstall") : t("bs.privEnabling"))
      : t("bs.disabling");
  try {
    await call("boot_helper", { action });
    // 失败已由下次快照带回；成功/失败都给明确结果反馈
    if (action === "install") {
      toast.show(t("bs.privOn"));
    } else {
      toast.show(t("bs.privOff"));
    }
  } catch (e) {
    // 用户取消系统授权 / 安装失败：明确告诉用户，而不是静默
    toast.show(t("bs.privFail", { msg: String(e) }), "bad");
  } finally {
    helperBusy.value = false;
    helperBusyLabel.value = "";
    // 后端 boot_helper 成功后会 publish_ui（推送新 helper 状态）；这里再主动拉一次快照兜底，
    // 确保状态标签/按钮立刻回显（之前只 get_settings 不更新 helper 快照 → 点完不回显）。
    void ui.pollOnce().catch(() => {});
  }
}
const message = ref("");
const errorText = ref("");

const varsDirInput = ref("");

onMounted(() => {
  varsDirInput.value = ui.status?.varsDir ?? "";
  const applyInstances = (list: any[]) => {
    if (Array.isArray(list) && list.length) {
      bootInstances.value = list;
      instancePrefs.setInstances(list);
    }
  };
  call<any[]>("boot_channels_all").then(applyInstances).catch(() => {
    // 浏览器/网页端没有 Tauri IPC：走 HTTP（同源，带 URL token）
    try {
      const token = new URLSearchParams(location.search).get("token") || "";
      void fetch(`/api/boot-channels?token=${encodeURIComponent(token)}`)
        .then((response) => response.json())
        .then(applyInstances)
        .catch(() => {});
    } catch { /* 忽略 */ }
  });
});

// 版式主题/布局切换时按档位设定窗口宽度（默认=460 / 加宽=932 / 左右=932）。
// 旧版 ensureLayoutWidth 在系统引导页做，这里高级设置页也要做，否则切布局窗口不变。
function ensureLayoutWidth(): void {
  const inTauri = location.protocol === "tauri:" || location.hostname === "tauri.localhost";
  if (!inTauri) return;
  const layout = adv.effectiveLayout;
  void call("set_boot_layout", { mode: layout }).catch(() => {});
}
watch([() => adv.effectiveLayout, () => adv.effectiveTheme], () => ensureLayoutWidth());

/** 当前主题可用的布局档位 */
const layouts = computed<LayoutMode[]>(
  () => THEME_LAYOUT_MODES[adv.effectiveTheme] ?? ["default"],
);

const layoutLabels: Record<LayoutMode, string> = {
  default: "bs.layout.default",
  wide: "bs.layout.wide",
  lr: "bs.layout.lr",
};

/** 引导器 ESP 展示值（旧版 l.esp）：refind/grub 固定，三通道取 channel.detail */
/** 引导器 ESP 副标题（与 BootApp/旧版适配器一致：按模式零回落） */
function loaderEsp(id: string): string {
  if (id === "refind") return settings.refindMode === "manual" ? t("boot.srcSnapshot") : "ESP · refind.conf";
  if (id === "refindplus") return settings.refindplusMode === "manual" ? t("boot.srcSnapshot") : (ui.refindplusSim?.installDir || "ESP · config.conf");
  if (id === "grub") {
    if (settings.grubMode === "manual") return ui.grub?.cfgPath || "grub.cfg";
    const t = (ui.linuxGrub as unknown as { target?: { cfgRel?: string } } | null)?.target;
    return t?.cfgRel || "";
  }
  const ch = (ui.channels || []).find((c) => c.id === id);
  return ch?.detail || "";
}

/** 切引导器左右列（旧版 k-rg） */
function moveSide(id: string) {
  if (bootInstances.value.length) {
    if (adv.effectiveLayout !== "lr" && adv.supportsSplit) adv.setLayout("lr");
    instancePrefs.toggleSide(id);
    return;
  }
  const cur = adv.state.side[id] === "r" ? "r" : "l";
  if (adv.effectiveLayout !== "lr" && adv.supportsSplit) adv.setLayout("lr");
  adv.setSide(id, cur === "r" ? "l" : "r");
}

function toggleVisible(id: string) {
  if (bootInstances.value.length) {
    instancePrefs.toggleVisible(id);
    return;
  }
  adv.setVisible(id as LoaderId, !adv.state.visible[id as LoaderId]);
}



/** 预演面板用的真实引导器（ESP/权限/默认项），供各布局预览卡片显示设置 */
/** 引导器状态点(绿/黄/灰)——与「系统引导」页 BootApp.loaderState 同一套真实健康判定。
 *  之前预演用 `perm==='nvram'?warn:ok` 假逻辑 → 小绿点全不对。 */
function previewLoaderState(id: string): "ok" | "warn" | "off" {
  const hasWarn = (issues?: { severity?: string }[] | null) =>
    !!issues && issues.some((i) => (i.severity || "").toLowerCase() === "warn");
  if (id === "refind") {
    const sim = ui.refindSim;
    if (!sim) return "off";
    if (sim.entries?.length === 0 && sim.issues?.length === 0) return "warn";
    return hasWarn(sim.issues) ? "warn" : "ok";
  }
  if (id === "refindplus") {
    // 与 refind 同口径，看 refindplus 通道自己的模拟扫描
    const sim = ui.refindplusSim;
    if (!sim) return "off";
    if (sim.entries?.length === 0 && sim.issues?.length === 0) return "warn";
    return hasWarn(sim.issues) ? "warn" : "ok";
  }
  if (id === "grub") {
    const g = ui.linuxGrub;
    if (!g) return "off";
    if (g.error) return "warn";
    if (!(g.entries?.length) && hasWarn(g.issues)) return "warn";
    return hasWarn(g.issues) ? "warn" : "ok";
  }
  const ch = ui.channels.find((c) => c.id === id);
  if (!ch || !ch.detected) return "off";
  const chip = String((ch as { chip?: string }).chip || "");
  if (chip === "warn") return "warn";
  if (chip === "off") return "off";
  return "ok";
}

const previewLoaders = computed<LoaderInfo[]>(() => {
  if (bootInstances.value.length) {
    return orderIds.value.filter(isVisible).map((id) => {
      const inst = instanceOf(id);
      const opts = (inst?.options ?? []).filter((o: any) => !o.header && String(o.value ?? "").trim());
      const current = String(inst?.current ?? "").trim();
      const hit = opts.find((o: any) => String(o.value ?? "") === current)
        ?? opts.find((o: any) => current && String(o.label ?? "").includes(current));
      const entries: { t?: string; def?: boolean }[] = [];
      if (hit || current) entries.push({ t: String(hit?.label ?? current), def: true });
      while (entries.length < opts.length) entries.push({ t: "", def: false });
      return {
        id,
        name: String(inst?.title || metaOf(id).name),
        state: (inst?.available ? (opts.length ? "ok" : "warn") : "off") as "ok" | "warn" | "off",
        entries,
        esp: String(inst?.partition || inst?.installDir || ""),
        perm: "file",
        detected: true,
        nextBoot: entries.find((e) => e.def) ?? null,
      };
    });
  }
  return adv.state.order
    .filter((id) => adv.state.visible[id])
    .map((id) => {
      const perm = loaderMeta(id).perm;
      const defTitle = loaderDefault(id);
      const count = (previewRows.value[id] ?? []).length;
      const entries: { t?: string; def?: boolean }[] = [];
      if (defTitle && defTitle !== t("boot.unknown")) entries.push({ t: defTitle, def: true });
      while (entries.length < count) entries.push({ t: "", def: false });
      return {
        id,
        name: loaderMeta(id).name,
        state: previewLoaderState(id),
        entries,
        esp: loaderEsp(id),
        perm,
        detected: true,
        nextBoot: entries.find((e) => e.def) ?? null,
      };
    });
});
/** 预演面板用的真实行（与系统引导页同一份行源） */
const previewRows = computed<Record<string, EntryData[]>>(() => {
  const out: Record<string, EntryData[]> = {};
  if (bootInstances.value.length) {
    for (const id of orderIds.value.filter(isVisible)) {
      const inst = instanceOf(id);
      out[id] = ((inst?.options ?? []) as any[])
        .filter((o) => !o.header)
        .map((o) => ({
          selector: String(o.value ?? ""),
          name: String(o.label ?? o.value ?? ""),
          class: String(o.class ?? ""),
        })) as EntryData[];
    }
    return out;
  }
  for (const id of adv.state.order.filter((x) => adv.state.visible[x])) {
    if (id === "refind") {
      out[id] = settings.refindMode === "manual"
        ? refindVariableRows(ui.status?.variables)
        : simRows(ui.refindSim);
    } else if (id === "refindplus") {
      // 与 refind 同口径，看 refindplus 通道的独立槽位
      out[id] = settings.refindplusMode === "manual"
        ? refindVariableRows(ui.refindplusVariables)
        : simRows(ui.refindplusSim);
    } else if (id === "grub") {
      out[id] = grubRows({
        grubMode: settings.grubMode,
        linuxGrubEntries: (ui.linuxGrub?.entries ?? []) as never,
        grubEntries: (ui.grub?.entries ?? []) as never,
        variables: ui.status?.variables,
      });
    } else {
      // 与系统引导页同口径：OpenCore 的真实菜单在 menuPreview.entries（entries 只有动作），
      // 漏了它会导致预演里 OC 的统计数/行数不对（用户：统计引导列表的数量不对）
      const ch = ui.channels.find((c) => c.id === id);
      const source = (ch?.menuPreview?.entries?.length ? ch.menuPreview.entries : ch?.entries ?? []) as {
        spec?: string; title?: string; id?: string;
      }[];
      out[id] = source.map((row) => ({
        selector: row.spec || row.title || row.id || "",
        name: row.title || row.id || "",
      }) as EntryData);
    }
  }
  return out;
});

/** 旧版 applyVwClasses：按容器宽度挂 .vw-900/.vw-760/.vw-520 断点类（屏幕适配） */
const rootEl = ref<HTMLElement | null>(null);
const kPrevEl = ref<HTMLElement | null>(null);
let vwRo: ResizeObserver | null = null;
function applyVwClasses(el: HTMLElement, width: number): void {
  el.classList.toggle("vw-g", width >= 900);
  el.classList.toggle("vw-900", width <= 900);
  el.classList.toggle("vw-760", width <= 760);
  el.classList.toggle("vw-520", width <= 520);
}
onMounted(() => {
  const el = rootEl.value;
  if (!el || typeof ResizeObserver === "undefined") return;
  vwRo = new ResizeObserver((entries) => {
    for (const en of entries) {
      applyVwClasses(el, en.contentRect.width);
      // 预演卡片按自身宽度挂断点类（宽窗口里预演区仍可能很窄 → 需要移动端堆叠）
      if (kPrevEl.value) applyVwClasses(kPrevEl.value, kPrevEl.value.getBoundingClientRect().width);
    }
  });
  vwRo.observe(el);
  applyVwClasses(el, el.getBoundingClientRect().width);
  if (kPrevEl.value) {
    vwRo.observe(kPrevEl.value);
    applyVwClasses(kPrevEl.value, kPrevEl.value.getBoundingClientRect().width);
  }
});
onUnmounted(() => { vwRo?.disconnect(); vwRo = null; });
</script>

<template>
  <div class="settings-page" ref="rootEl">
    <!-- 免密提权 + 扫描设置：合成**同一张卡片**。
         免密提权本体分平台：Linux=polkit/sudo 组件，macOS=root 助手（二者同平台只显示其一）；
         其下「启动时使用管理员扫描」「启动免密提醒」与助手无关，始终显示。 -->
    <section class="card helper-card" id="scan-settings-card">
      <!-- 免密提权（Linux · polkit/sudo 助手） -->
      <template v-if="privStatus && privStatus.platform">
        <div class="helper-card-row">
          <span class="helper-label">{{ t("bs.privLabelLinux") }}</span>
          <span
            class="helper-state"
            :data-state="privBusy ? 'off' : (privStatus.state === 'ready' ? 'on' : (privStatus.state === 'outdated' || privStatus.state === 'stale') ? 'stale' : 'off')"
          >{{ privBusy ? t("bs.processing") : privStateText }}</span>
          <span class="grow" />
          <button type="button" class="btn sm" :disabled="privBusy || !isDesktop" @click="toggleLinuxPriv">
            <span v-if="privBusy" class="btn-spinner" aria-hidden="true"></span>
            {{ privBusy
                ? (privBusyAction === "uninstall" ? t("bs.disabling") : (privStatus.state === 'outdated' || privStatus.state === 'stale' ? t("bs.privReinstall") : t("bs.privEnabling")))
                : (privInstalled ? t("bs.close") : (privStatus.state === 'outdated' || privStatus.state === 'stale' ? t("bs.reinstall") : t("bs.enable"))) }}
          </button>
        </div>
        <small v-if="!isDesktop" class="helper-hint" style="display:block; color: var(--warn, #d97706)">
          {{ t("bs.privMobileNote") }}
        </small>
        <small class="helper-hint">{{ privStatus.detail }}</small>
        <small class="helper-hint" style="display:block; margin-top:4px; color: var(--text-faint)">
          {{ t("bs.privBody1") }}
          {{ t("bs.privBody2", { method: privStatus.hasPkexec ? t("bs.privMethodPkexec") : (privStatus.hasSudo ? t("bs.privMethodSudo") : t("bs.privMethodAdmin")), path: privStatus.sudoersPath }) }}
        </small>
      </template>
      <!-- 免密提权（root 助手）本体：仅 macOS 支持（helper.supported） -->
      <template v-else-if="ui.state?.helper?.supported">
        <div class="helper-card-row">
          <span class="helper-label">{{ t("bs.privLabelRoot") }}</span>
          <span class="helper-state" :data-state="helperBusy ? 'off' : (ui.state.helper.installed ? (ui.state.helper.stale ? 'stale' : 'on') : 'off')">
            {{ helperBusy ? t("bs.processing") : (ui.state.helper.installed ? (ui.state.helper.stale ? t("bs.helperStaleOld") : t("bs.helperReady")) : t("bs.helperAbsent")) }}
          </span>
          <span class="grow" />
          <button type="button" class="btn sm" :disabled="helperBusy || !isDesktop" @click="toggleBootHelper">
            <span v-if="helperBusy" class="btn-spinner" aria-hidden="true"></span>
            {{ helperBusy ? helperBusyLabel : (ui.state.helper.installed && !ui.state.helper.stale ? t("bs.close") : ui.state.helper.installed ? t("bs.reinstall") : t("bs.enable")) }}
          </button>
        </div>
        <small v-if="!isDesktop" class="helper-hint" style="display:block; color: var(--warn, #d97706)">
          {{ t("bs.helperMobileNote") }}
        </small>
        <small class="helper-hint">{{ ui.state.helper.text }}</small>
        <small class="helper-hint" style="display:block; margin-top:4px; color: var(--text-faint)">
          {{ t("bs.helperBody2") }}
        </small>
      </template>
      <!-- 启动时使用管理员扫描（默认开） -->
      <div class="helper-card-row" style="margin-top: 14px; border-top: 1px dashed var(--line); padding-top: 14px">
        <span class="helper-label">{{ t("bs.startupAdminLabel") }}</span>
        <span class="grow" style="flex: 1"></span>
        <label style="display: flex; align-items: center; gap: 6px; cursor: pointer">
          <input
            type="checkbox"
            :disabled="!isDesktop"
            :checked="ui.settings?.startupScanWithAdmin !== false"
            @change="toggleStartupAdminScan"
          />
          <span>{{ ui.settings?.startupScanWithAdmin !== false ? t("bs.on") : t("bs.off") }}</span>
        </label>
      </div>
      <small class="helper-hint">
        {{ t("bs.startupAdminNote2") }}
      </small>
      <!-- 启动免密提醒（取消提醒设置） -->
      <div class="helper-card-row" style="margin-top: 12px; border-top: 1px dashed var(--line); padding-top: 14px">
        <span class="helper-label">{{ t("bs.snoozeLabel") }}</span>
        <span class="grow" style="flex: 1"></span>
        <template v-if="snoozeLabel">
          <span style="color: var(--text-faint); margin-right: 8px">{{ snoozeLabel }}</span>
          <button type="button" class="btn sm ghost" @click="clearSnooze">{{ t("bs.restoreReminder") }}</button>
        </template>
        <template v-else>
          <span style="color: var(--text-faint); margin-right: 8px">{{ t("bs.cancelReminder") }}</span>
          <button type="button" class="btn sm ghost" @click="snoozeHelperReminder(1)">{{ t("bs.daysN", { n: 1 }) }}</button>
          <button type="button" class="btn sm ghost" @click="snoozeHelperReminder(7)">{{ t("bs.daysN", { n: 7 }) }}</button>
          <button type="button" class="btn sm ghost" @click="snoozeHelperReminder(30)">{{ t("bs.daysN", { n: 30 }) }}</button>
          <button type="button" class="btn sm ghost" @click="snoozeHelperReminder(0)">{{ t("helper.forever") }}</button>
        </template>
      </div>
    </section>
    <!-- Linux 分区扫描策略：**单独一张卡片**（仅 Linux 显示） -->
    <section v-if="isLinux" class="card helper-card" id="linux-scan-card">
      <div class="helper-card-row">
        <span class="helper-label">{{ t("bs.linuxScanPolicy") }}</span>
      </div>
      <div class="seg" role="group" :aria-label="t('bs.linuxScanPolicy')" style="margin-top: 6px">
        <button
          v-for="m in LINUX_EXT4_MODES"
          :key="m.key"
          type="button"
          :class="{ 'is-on': linuxExt4Mode === m.key }"
          :aria-pressed="linuxExt4Mode === m.key"
          :title="m.hint"
          @click="pickLinuxExt4Mode(m.key)"
        >
          {{ m.label }}
        </button>
      </div>
      <small class="helper-hint" style="display:block; margin-top:4px;">
        {{ linuxExt4Hint }}
      </small>
      <small v-if="linuxExt4Mode === 'mount_only'" class="helper-hint" style="display:block; margin-top:2px; color: var(--warn, #d97706)">
        {{ t("bs.mountOnlyNote") }}
      </small>
    </section>
    <!-- 旧版 renderAdvTab：div.k > div.k-bar（行式：k-row × N + k-tabs + sortbar + setlist） -->
    <div class="k">
      <div class="k-bar">
        <!-- row1：启用引导器（.k-tabs 芯片行 + 重置） -->
        <div class="k-tabs">
          <button
            v-for="id in orderIds"
            :key="id"
            type="button"
            :class="['k-tab', { 'is-on': isVisible(id) }]"
            :title="t('bs.toggleShowTitle')"
            @click="toggleVisible(id)"
          >
            <span>{{ displayNameOf(id) }}</span>
            <span
              class="k-rg"
              :title="t('bs.moveColTitle')"
              @click.stop="moveSide(id)"
            >{{ sideOf(id) === 'r' ? t('bs.right') : t('bs.left') }}</span>
          </button>
          <span class="grow" />
          <button type="button" class="btn xs ghost" :title="t('bs.reset')" @click="resetOrder">{{ t("bs.reset") }}</button>
        </div>
        <!-- row2：版式主题 -->
        <div class="k-row">
          <span class="k-lbl">{{ t("bs.layoutTheme") }}</span>
          <div class="seg">
            <button
              v-for="item in THEME_LABELS"
              :key="item.key"
              type="button"
              :class="{ 'is-on': adv.effectiveTheme === item.key }"
              :title="item.key === 'K' ? t('bs.themeKTitle') : t('bs.themeTitle', { k: item.key })"
              @click="adv.setTheme(item.key)"
            >
              {{ t(item.label) }}
            </button>
          </div>
          <span class="hint">{{ t("bs.themeHint") }}</span>
        </div>
        <!-- row3：布局 -->
        <div class="k-row">
          <span class="k-lbl">{{ t("bs.layoutLabel") }}</span>
          <div class="seg">
            <button
              v-for="mode in layouts"
              :key="mode"
              type="button"
              :class="['theme-btn', { 'is-on': adv.effectiveLayout === mode }]"
              @click="adv.setLayout(mode)"
            >
              {{ layoutLabels[mode] }}
            </button>
          </div>
          <span v-if="layouts.length > 1" class="hint">{{ t("bs.layoutHint") }}</span>
          <span v-else class="hint">{{ t("bs.layoutOnlyOne", { x: layoutLabels[layouts[0] ?? "default"] }) }}</span>
        </div>
        <!-- row4：菜单样式 -->
        <div class="k-row">
          <span class="k-lbl">{{ t("bs.menuStyle") }}</span>
          <div class="seg">
            <button type="button" :class="{ 'is-on': adv.state.style === 'full' }" :title="t('bs.styleTitle')" @click="adv.setStyle('full')">{{ t("bs.styleFull") }}</button>
            <button type="button" :class="{ 'is-on': adv.state.style === 'compact' }" :title="t('bs.styleTitle')" @click="adv.setStyle('compact')">{{ t("bs.styleCompact") }}</button>
            <button type="button" :class="{ 'is-on': adv.state.style === 'min' }" :title="t('bs.styleTitle')" @click="adv.setStyle('min')">{{ t("bs.styleMin") }}</button>
          </div>
          <span class="hint">{{ t("bs.styleHint") }}</span>
        </div>
        <!-- row4b：下次启动 -->
        <div class="k-row">
          <span class="k-lbl">{{ t("bs.nextBootSec") }}</span>
          <div class="seg">
            <button type="button" :class="{ 'is-on': adv.state.nextBoot === 'off' }" :title="t('bs.nbOffTitle')" @click="adv.setNextBoot('off')">{{ t("bs.nbOff") }}</button>
            <button type="button" :class="{ 'is-on': adv.state.nextBoot === 'fold' }" @click="adv.setNextBoot('fold')">{{ t("bs.nbFold") }}</button>
            <button type="button" :class="{ 'is-on': adv.state.nextBoot === 'open' }" @click="adv.setNextBoot('open')">{{ t("bs.nbOpen") }}</button>
          </div>
          <span class="hint">{{ t("bs.nextBootHint") }}</span>
        </div>
        <!-- row5：排序条（设置动作收在设置区，预演区保持纯观看） -->
        <div class="k-row">
          <span class="k-sortwrap" style="display: contents">
            <span class="k-sortbar">
              <span class="k-sort-lbl">{{ t("bs.orderLabel") }}</span>
              <span v-for="(id, i) in orderIds" :key="id" class="k-sortchip">
                <span class="k-sort-nm">{{ displayNameOf(id) }}</span>
                <button type="button" class="k-zop" :title="t('bs.moveUpOne')" :disabled="i === 0" @click="moveLoader(id, -1)">↑</button>
                <button type="button" class="k-zop" :title="t('bs.moveDownOne')" :disabled="i === orderIds.length - 1" @click="moveLoader(id, 1)">↓</button>
              </span>
            </span>
          </span>
          <span class="grow" />
          <span class="hint">{{ t("bs.previewHint") }}</span>
        </div>

      </div>
    </div>
    <!-- 预演面板（旧版：bootThemeStage → .k-stage + lg-side 选中项；真实区块由 ThemeHost slot 承接） -->
    <section class="card k-prev" ref="kPrevEl">
      <header class="k-prev-head">
        <h2>{{ t('bs.previewTitle', { layout: adv.effectiveLayout === 'lr' ? t('bs.previewLr') : t('bs.previewSingle') }) }}</h2>
        <small>{{ t('bs.loaderCount', { n: orderIds.filter((x) => isVisible(x)).length }) }}</small>
      </header>
      <!-- K 不套用主题：旧版 k-zone-cards + kLoaderCard 自绘卡片 -->
      <div v-if="adv.effectiveTheme === 'K'" class="k-zone-cards">
        <div v-for="loader in previewLoaders" :key="loader.id" class="k-card">
          <div class="k-card-head">
            <span class="ldr" aria-hidden="true">{{ loaderMono(loader.id) }}</span>
            <span class="nm">{{ loader.name }}</span>
            <span class="grow" />
            <span class="hint">{{ t("bs.totalRows", { n: (previewRows[loader.id] ?? []).length }) }}</span>
          </div>
          <div class="k-entries">
            <div
              v-for="(row, i) in previewRows[loader.id] ?? []"
              :key="i"
              class="grub-entry"
              :class="{ 'is-pinned': i === 0 }"
            >
              <span class="grub-entry-name">{{ row.name }}</span>
            </div>
            <div v-if="!(previewRows[loader.id] ?? []).length" class="hint">{{ t("bs.noEntries") }}</div>
          </div>
        </div>
      </div>
      <ThemeHost
        v-else
        :rows="previewRows"
        :theme="adv.effectiveTheme"
        :layout="adv.effectiveLayout"
        :loaders="previewLoaders"
        :side="adv.state?.side"
      />
    </section>
    <p v-if="message" class="ok">{{ message }}</p>
    <p v-if="errorText" class="err">{{ errorText }}</p>
  </div>
</template>


<style scoped>
.settings-page {
  display: flex;
  flex-direction: column;
  gap: 14px;
  /* 旧版无限宽（boot-adv-mount 全宽） */
  max-width: none;
}
.card {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 10px;
  background: var(--panel, #fff);
  padding: 16px 18px;
}
header {
  margin-bottom: 12px;
}
h2 {
  font-size: 14px;
  margin: 0;
}
.themes,
.layouts {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  align-items: center;
  margin-bottom: 8px;
}
.theme-btn {
  border: 1px solid var(--line, #d1d5db);
  background: var(--panel, #fff);
  border-radius: 999px;
  padding: 5px 13px;
  font-size: 12px;
  cursor: pointer;
  color: var(--fg-dim, #4b5563);
}
.theme-btn.is-on {
  background: var(--accent, #2563eb);
  border-color: var(--accent, #2563eb);
  color: #fff;
}
.lbl {
  font-size: 11.5px;
  color: var(--fg-faint, #9ca3af);
  margin-right: 4px;
}
.loader-list {
  list-style: none;
  padding: 0;
  margin: 0 0 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.loader-list li {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 10px;
  border: 1px solid var(--line, #f1f5f9);
  border-radius: 8px;
  font-size: 13px;
}
.spacer {
  flex: 1;
}
.row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 10px;
  font-size: 12.5px;
}
.row label {
  width: 110px;
  color: var(--fg-faint, #9ca3af);
  flex: none;
}
.row input[type="text"],
.row input:not([type]) {
  flex: 1;
}
.mono {
  font-family: ui-monospace, Menlo, monospace;
  font-size: 11.5px;
  flex: 1;
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 7px;
  padding: 6px 10px;
}
.hint {
  font-size: 11.5px;
  color: var(--fg-faint, #9ca3af);
  margin: 0;
}
.ok {
  font-size: 12.5px;
  color: #15803d;
  margin: 0;
}
.err {
  font-size: 12.5px;
  color: #dc2626;
  margin: 0;
}
</style>
