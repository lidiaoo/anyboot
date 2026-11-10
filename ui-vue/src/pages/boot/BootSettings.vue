<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import { useBootAdvStore, type LoaderId } from "@/stores/bootAdv";
import { loaderMeta } from "@/pages/boot/themes/loaders";
import { simRows, refindVariableRows, grubRows } from "@/pages/boot/bootRows";
import type { EntryData } from "@/components/boot/types";
import ThemeHost from "@/pages/boot/themes/index.vue";
import { THEME_LABELS, THEME_LAYOUT_MODES, type LayoutMode } from "@/pages/boot/themes/registry";

import { call } from "@/api/tauri";
import { setStartupScanWithAdmin, setHelperReminderSnooze, setLinuxExt4ScanMode, isTauri, linuxPrivStatus, linuxPrivInstall, linuxPrivUninstall, type LinuxPrivStatus } from "@/api/tauri";
import { useToastStore } from "@/stores/toast";
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

const setListOpen = ref(false);
/** 旧版 moveLoader（boot-adv.js）：order 内上/下移一位 */
function moveLoader(id: LoaderId, delta: number) {
  const order = [...adv.state.order];
  const i = order.indexOf(id);
  const j = i + delta;
  if (i < 0 || j < 0 || j >= order.length) return;
  order.splice(j, 0, ...order.splice(i, 1));
  adv.setOrder(order);
}

/** 引导器默认项（旧版 def = l.nextBoot || entries.find(def)） */
function loaderDefault(id: string): string {
  if (id === "refind") {
    const hit = (ui.status?.variables ?? []).find((v) => v.active);
    const lb = (ui.refindSim?.entries ?? []).find((e) => e.lastBoot);
    return (lb?.title || hit?.displayName || "未知").toString();
  }
  if (id === "grub") {
    const g = settings.grubMode === "manual" ? ui.grub : ui.linuxGrub;
    return (g?.nextEntry || g?.savedEntry || "未知").toString();
  }
  const ch = (ui.channels ?? []).find((c) => c.id === id);
  // 旧版 channelEntries：currentDefault 可能是 id 或标题，命中 channel.entries 取标题
  const cur = String(ch?.currentDefault || "");
  const list = ((ch?.entries ?? []) as { id?: string; title?: string }[]);
  const hit = cur
    ? list.find((e) => cur === e.id || cur === e.title || cur.includes(e.title || "\u0000"))
    : undefined;
  return (hit?.title || cur || (ch?.detected ? "未选择" : "未知")).toString();
}
/** 引导器图标里的两个字母（旧版 ldr 图标文字） */
function loaderMono(id: string): string {
  return loaderMeta(id).mono;
}
/** 引导器权限类型（file/nvram）——旧版免提权/需提权判定 */
function loaderPerm(id: string): string {
  return loaderMeta(id).perm;
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

/** Linux 免密组件（polkit/sudo 助手）状态：仅 Linux 有，其它平台恒 null */
const privStatus = ref<LinuxPrivStatus | null>(null);
const privBusy = ref(false);
const privBusyLabel = ref("");
async function refreshPrivStatus(): Promise<void> {
  // 注意：**不要**判 isTauri() 或 isLinux() ——
  // · 网页/手机端走 HTTP 桥也能读 linux_priv_status；判 isTauri() 会永远 return、
  //   privStatus 恒 null、整张卡不渲染（用户报「免密设置没显示」的根因）。
  // · 后端在非 Linux 上返回 platform=false，由模板用 privStatus.platform 判，
  //   不依赖前端自己推平台（更可靠）。
  // 读状态是只读操作，任何端都该能看；只有「安装/移除」按钮按 isDesktop 禁用。
  try {
    privStatus.value = await linuxPrivStatus();
  } catch {
    /* 读不到就不显示细节，不阻塞页面 */
  }
}
/** Linux 免密状态一句话 + 是否需要「安装」按钮 */
const privStateText = computed(() => {
  const st = privStatus.value;
  if (!st) return "";
  if (st.state === "ready") return "已开启";
  if (st.state === "outdated") return `已安装（旧版本 ${st.version || "?"}，当前 ${st.currentVersion}）—— 建议重装`;
  if (st.state === "stale") return "已安装但组件文件缺失，建议重装";
  if (st.state === "absent") return "未开启";
  return st.detail || "未知";
});
const privInstalled = computed(
  () => privStatus.value?.state === "ready",
);
async function toggleLinuxPriv(): Promise<void> {
  if (privBusy.value) return;
  const install = !privInstalled.value;
  privBusy.value = true;
  privBusyLabel.value = install ? "安装中…（可能弹出系统授权框）" : "移除中…";
  try {
    privStatus.value = install
      ? await linuxPrivInstall(null)
      : await linuxPrivUninstall(null);
    toast.show(install ? "Linux 免密组件已开启" : "Linux 免密组件已移除");
  } catch (e) {
    toast.show("操作未完成：" + e + "（如取消了系统授权框可重试）", "bad");
  } finally {
    privBusy.value = false;
    privBusyLabel.value = "";
  }
}
/** Linux 分区扫描策略（仅 Linux 显示） */
const LINUX_EXT4_MODES = [
  { key: "raw", label: "仅裸读（默认）", hint: "三端一致：一律裸读自研 ext4 驱动，RECOVER（上次未干净关机）能看见" },
  { key: "prefer_mount", label: "优先挂载", hint: "已挂载分区先走挂载点直读（快）；仅当该分区真遇到 RECOVER 时再回落裸读补全提示" },
  { key: "mount_only", label: "仅挂载", hint: "已挂载分区只直读（最快）；RECOVER 提示不会出现（上次没干净关机看不出）" },
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
  if (until > now + 3600 * 24 * 365 * 40) return "已永久取消提醒";
  const d = Math.ceil((until - now) / 86400);
  return `已取消提醒（约 ${d} 天）`;
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
      ? (h.installed ? "重装中…（会弹系统授权框，请输入密码）" : "开启中…（会弹系统授权框，请输入密码）")
      : "关闭中…";
  try {
    await call("boot_helper", { action });
    // 失败已由下次快照带回；成功/失败都给明确结果反馈
    if (action === "install") {
      toast.show("免密提权已开启");
    } else {
      toast.show("免密提权已关闭");
    }
  } catch (e) {
    // 用户取消系统授权 / 安装失败：明确告诉用户，而不是静默
    toast.show("免密提权操作未完成：" + e + "（如取消了系统授权框可重试）", "bad");
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
  void refreshPrivStatus();
  varsDirInput.value = ui.status?.varsDir ?? "";
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
  default: "默认",
  wide: "加宽",
  lr: "左右布局",
};

/** 引导器 ESP 展示值（旧版 l.esp）：refind/grub 固定，三通道取 channel.detail */
/** 引导器 ESP 副标题（与 BootApp/旧版适配器一致：按模式零回落） */
function loaderEsp(id: string): string {
  if (id === "refind") return settings.refindMode === "manual" ? "vars/PreviousBoot 快照" : "ESP · refind.conf";
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
  const cur = adv.state.side[id] === "r" ? "r" : "l";
  // 旧版语义：没开左右布局时点「左/右」= 先自动切到左右布局（否则分了列也没效果）
  if (adv.effectiveLayout !== "lr" && adv.supportsSplit) adv.setLayout("lr");
  adv.setSide(id, cur === "r" ? "l" : "r");
}

function toggleVisible(id: LoaderId) {
  adv.setVisible(id, !adv.state.visible[id]);
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

const previewLoaders = computed(() =>
  adv.state.order
    .filter((id) => adv.state.visible[id])
    .map((id) => {
      const perm = loaderMeta(id).perm;
      const defTitle = loaderDefault(id);
      const count = (previewRows.value[id] ?? []).length;
      const entries: { t?: string; def?: boolean }[] = [];
      if (defTitle && defTitle !== "未知") entries.push({ t: defTitle, def: true });
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
    }),
);
/** 预演面板用的真实行（与系统引导页同一份行源） */
const previewRows = computed<Record<string, EntryData[]>>(() => {
  const out: Record<string, EntryData[]> = {};
  for (const id of adv.state.order.filter((x) => adv.state.visible[x])) {
    if (id === "refind") {
      out[id] = settings.refindMode === "manual"
        ? refindVariableRows(ui.status?.variables)
        : simRows(ui.refindSim);
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
    <!-- Linux 免密组件（polkit/sudo 助手）：仅 Linux 有。装一次后，
         GRUB 自动读取 / 挂载 ESP / 免密扫描都能无感（不再每次弹密码）。 -->
    <section v-if="privStatus && privStatus.platform" class="card helper-card" id="linux-priv-card">
      <div class="helper-card-row">
        <span class="helper-label">免密组件（Linux · polkit/sudo 助手）</span>
        <span
          class="helper-state"
          :data-state="privBusy ? 'off' : (privStatus.state === 'ready' ? 'on' : (privStatus.state === 'outdated' || privStatus.state === 'stale') ? 'stale' : 'off')"
        >{{ privBusy ? "处理中…" : privStateText }}</span>
        <span class="grow" />
        <button type="button" class="btn sm" :disabled="privBusy || !isDesktop" @click="toggleLinuxPriv">
          <span v-if="privBusy" class="btn-spinner" aria-hidden="true"></span>
          {{ privBusy ? privBusyLabel : (privInstalled ? "移除" : (privStatus.state === 'outdated' || privStatus.state === 'stale' ? "重装 ⤴" : "开启 ⤴")) }}
        </button>
      </div>
      <small v-if="!isDesktop" class="helper-hint" style="display:block; color: var(--warn, #d97706)">
        免密组件仅电脑端(本机)可管理，移动端不支持开启/移除。
      </small>
      <small class="helper-hint">{{ privStatus.detail }}</small>
      <small class="helper-hint" style="display:block; margin-top:4px; color: var(--text-faint)">
        开启后「启动扫描 / GRUB 自动读取 / 只读挂载 ESP」都用免密助手，不再每次弹密码。
        需要一次系统授权（{{ privStatus.hasPkexec ? "polkit 图形框" : (privStatus.hasSudo ? "sudo 密码" : "管理员权限") }}）；
        授权条目写到 <code>{{ privStatus.sudoersPath }}</code>，可随时移除。
      </small>
    </section>
    <!-- 免密提权（root 助手）本体：仅 macOS 支持（helper.supported）。
         注意：下面「启动时使用管理员扫描 / Linux 分区扫描策略 / 启动免密提醒」三项
         和助手无关，必须放**这个 v-if 外面**，否则 Linux/Windows 上 helper.supported=false
         会把整块连同这些设置一起隐藏（用户报「settings-page 整个没显示」的根因）。 -->
    <section v-if="ui.state?.helper?.supported" class="card helper-card" id="helper-card">
      <div class="helper-card-row">
        <span class="helper-label">免密提权（root 助手）</span>
        <span class="helper-state" :data-state="helperBusy ? 'off' : (ui.state.helper.installed ? (ui.state.helper.stale ? 'stale' : 'on') : 'off')">
          {{ helperBusy ? "处理中…" : (ui.state.helper.installed ? (ui.state.helper.stale ? "已开启（旧构建，建议重装）" : "已开启") : "未开启") }}
        </span>
        <span class="grow" />
        <button type="button" class="btn sm" :disabled="helperBusy || !isDesktop" @click="toggleBootHelper">
          <span v-if="helperBusy" class="btn-spinner" aria-hidden="true"></span>
          {{ helperBusy ? helperBusyLabel : (ui.state.helper.installed && !ui.state.helper.stale ? "关闭" : ui.state.helper.installed ? "重装 ⤴" : "开启 ⤴") }}
        </button>
      </div>
      <small v-if="!isDesktop" class="helper-hint" style="display:block; color: var(--warn, #d97706)">
        免密提权助手仅在电脑端(本机)可管理，移动端不支持开启/关闭。
      </small>
      <small class="helper-hint">{{ ui.state.helper.text }}</small>
      <small class="helper-hint" style="display:block; margin-top:4px; color: var(--text-faint)">
        点「开启/重装」会弹出系统管理员授权框，输入密码后约需几秒完成；完成后状态会变为「已开启」。
      </small>
    </section>
    <!-- 扫描相关设置：所有平台都显示（与免密助手解耦，不受 helper.supported 影响） -->
    <section class="card helper-card" id="scan-settings-card">
      <!-- 启动时使用管理员扫描（默认开）：开启后启动主动扫所有引导器 -->
      <div class="helper-card-row">
        <span class="helper-label">启动时使用管理员扫描</span>
        <span class="grow" style="flex: 1"></span>
        <label style="display: flex; align-items: center; gap: 6px; cursor: pointer">
          <input
            type="checkbox"
            :disabled="!isDesktop"
            :checked="ui.settings?.startupScanWithAdmin !== false"
            @change="toggleStartupAdminScan"
          />
          <span style="font-size: 12px">{{ ui.settings?.startupScanWithAdmin !== false ? "已开启" : "已关闭" }}</span>
        </label>
      </div>
      <small class="helper-hint">
        开启后启动时自动扫描所有引导器（GRUB 自动 + rEFInd 内核）。未开启免密助手时会提示开启，
        也可在此取消提醒。
      </small>
      <!-- Linux 分区扫描策略（所有平台都显示）：仅挂载 / 优先挂载 / 仅裸读。
           注：该策略只在 Linux 上真正影响扫描（macOS/Windows 一律裸读，选哪档行为一致），
           但按用户要求三端都显示这个选项，避免「找不到」的困惑。 -->
      <div class="helper-card-row" style="margin-top: 10px; border-top: 1px dashed var(--line); padding-top: 10px">
        <span class="helper-label">Linux 分区扫描策略</span>
        <span v-if="!isLinux" class="hint" style="margin-left:8px; font-size:12px; color: var(--text-faint)">（仅 Linux 生效；当前平台一律裸读）</span>
      </div>
      <div class="seg" style="margin-top: 6px; display: flex; gap: 6px; flex-wrap: wrap">
        <button
          v-for="m in LINUX_EXT4_MODES"
          :key="m.key"
          type="button"
          :class="['btn', 'sm', linuxExt4Mode === m.key ? '' : 'ghost']"
          :title="m.hint"
          @click="pickLinuxExt4Mode(m.key)"
        >
          {{ m.label }}
        </button>
      </div>
      <small class="helper-hint" style="display:block; margin-top:4px; color: var(--text-faint)">
        {{ linuxExt4Hint }}
      </small>
      <small v-if="linuxExt4Mode === 'mount_only'" class="helper-hint" style="display:block; margin-top:2px; color: var(--warn, #d97706)">
        注意：仅挂载模式下，若分区上次没干净关机（RECOVER），这个提示不会出现。
      </small>
      <!-- 免密助手提醒的取消提醒设置 -->
      <div class="helper-card-row" style="margin-top: 8px; border-top: 1px dashed var(--line); padding-top: 10px">
        <span class="helper-label">启动免密提醒</span>
        <span class="grow" style="flex: 1"></span>
        <template v-if="snoozeLabel">
          <span style="font-size: 12px; color: var(--text-faint); margin-right: 8px">{{ snoozeLabel }}</span>
          <button type="button" class="btn sm ghost" @click="clearSnooze">恢复提醒</button>
        </template>
        <template v-else>
          <span style="font-size: 12px; color: var(--text-faint); margin-right: 8px">取消提醒：</span>
          <button type="button" class="btn sm ghost" @click="snoozeHelperReminder(1)">1天</button>
          <button type="button" class="btn sm ghost" @click="snoozeHelperReminder(7)">7天</button>
          <button type="button" class="btn sm ghost" @click="snoozeHelperReminder(30)">30天</button>
          <button type="button" class="btn sm ghost" @click="snoozeHelperReminder(0)">永久</button>
        </template>
      </div>
    </section>
    <!-- 旧版 renderAdvTab：div.k > div.k-bar（行式：k-row × N + k-tabs + sortbar + setlist） -->
    <div class="k">
      <div class="k-bar">
        <!-- row1：启用引导器（.k-tabs 芯片行 + 重置） -->
        <div class="k-tabs">
          <button
            v-for="id in adv.state.order"
            :key="id"
            type="button"
            :class="['k-tab', { 'is-on': adv.state.visible[id] }]"
            title="点击 开/关 展示该引导器"
            @click="toggleVisible(id)"
          >
            <span>{{ loaderMeta(id).name }}</span>
            <span
              class="k-rg"
              title="点击挪到另一列（没开左右布局时，点它自动切过去）"
              @click.stop="moveSide(id)"
            >{{ adv.state.side[id] === 'r' ? '右' : '左' }}</span>
          </button>
          <span class="grow" />
          <button type="button" class="btn xs ghost" title="回到默认编排" @click="adv.resetOrder()">重置</button>
        </div>
        <!-- row2：版式主题 -->
        <div class="k-row">
          <span class="k-lbl">版式主题</span>
          <div class="seg">
            <button
              v-for="item in THEME_LABELS"
              :key="item.key"
              type="button"
              :class="{ 'is-on': adv.effectiveTheme === item.key }"
              :title="item.key === 'K' ? '不套用 A–J 版式：预演区用本页的引导器卡片（区域编排：启用/左右分栏/排序），仅此页有这种形态' : '引导菜单整体用方案 ' + item.key + ' 的版式；选好后列内排序照常生效'"
              @click="adv.setTheme(item.key)"
            >
              {{ item.label }}
            </button>
          </div>
          <span class="hint">主题决定每张卡怎么画；B / G / J 是加宽版面，选中即把窗口加宽到 932。</span>
        </div>
        <!-- row3：布局 -->
        <div class="k-row">
          <span class="k-lbl">布局</span>
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
          <span v-if="layouts.length > 1" class="hint">默认 = 单列竖排（460）；加宽 = 窗口 932、列表仍单列；左右布局 = 两列各自滚动，点标签上的「左/右」分列、拖分隔条定占比（中间圆钮或双击回正中）——仅「不套用主题」和 A / C；B / G / J 只有加宽，I 没有左右布局（置灰的档位选择仍会记着，换回支持的主题自动生效）。</span>
          <span v-else class="hint">这个主题只有「{{ layoutLabels[layouts[0] ?? "default"] }}」一种布局</span>
        </div>
        <!-- row4：菜单样式 -->
        <div class="k-row">
          <span class="k-lbl">菜单样式</span>
          <div class="seg">
            <button type="button" :class="{ 'is-on': adv.state.style === 'full' }" title="完整：路径 + 徽章 + 全部动作；紧凑：行距减半、隐去路径；极简：只剩图标 + 名称 + 默认高亮" @click="adv.setStyle('full')">完整</button>
            <button type="button" :class="{ 'is-on': adv.state.style === 'compact' }" title="完整：路径 + 徽章 + 全部动作；紧凑：行距减半、隐去路径；极简：只剩图标 + 名称 + 默认高亮" @click="adv.setStyle('compact')">紧凑</button>
            <button type="button" :class="{ 'is-on': adv.state.style === 'min' }" title="完整：路径 + 徽章 + 全部动作；紧凑：行距减半、隐去路径；极简：只剩图标 + 名称 + 默认高亮" @click="adv.setStyle('min')">极简</button>
          </div>
          <span class="hint">三档对任何版式主题都生效；行内动作按钮在紧凑/极简下悬停才出现。</span>
        </div>
        <!-- row4b：下次启动 -->
        <div class="k-row">
          <span class="k-lbl">下次启动</span>
          <div class="seg">
            <button type="button" :class="{ 'is-on': adv.state.nextBoot === 'off' }" title="不显示：整块从界面移除（不是留个空壳），下面引导列表照常可用 —— 出厂默认就是这一档" @click="adv.setNextBoot('off')">不显示</button>
            <button type="button" :class="{ 'is-on': adv.state.nextBoot === 'fold' }" @click="adv.setNextBoot('fold')">折叠框</button>
            <button type="button" :class="{ 'is-on': adv.state.nextBoot === 'open' }" @click="adv.setNextBoot('open')">展开</button>
          </div>
          <span class="hint">「系统引导」页顶部那块：不显示 = 整块移除、不占任何高度（默认）；折叠框 = 默认收起，标题旁小按钮切换；展开 = 打开就看到全部条目，同样可收起。</span>
        </div>
        <!-- row5：排序条（设置动作收在设置区，预演区保持纯观看） -->
        <div class="k-row">
          <span class="k-sortwrap" style="display: contents">
            <span class="k-sortbar">
              <span class="k-sort-lbl">上下排序</span>
              <span v-for="(id, i) in adv.state.order" :key="id" class="k-sortchip">
                <span class="k-sort-nm">{{ loaderMeta(id).name }}</span>
                <button type="button" class="k-zop" title="上移一位" :disabled="i === 0" @click="moveLoader(id, -1)">↑</button>
                <button type="button" class="k-zop" title="下移一位" :disabled="i === adv.state.order.length - 1" @click="moveLoader(id, 1)">↓</button>
              </span>
            </span>
          </span>
          <span class="grow" />
          <span class="hint">预演与「系统引导」页同一份真实数据（只读示意）· 界面效果以「系统引导」页实际为准</span>
        </div>
        <!-- 引导器设置（k-setlist 折叠条） -->
        <button
          type="button"
          class="k-setbar"
          :class="{ 'is-on': setListOpen }"
          title="展开/收起 引导器设置（ESP 路径 / 权限 / 默认项 / 脚本 / 免密模式）"
          @click="setListOpen = !setListOpen"
        >
          <span class="gear" aria-hidden="true">⚙</span>
          <span class="k-setbar-t">引导器设置</span>
          <span class="hint">ESP 路径 · 权限 · 默认项 · 脚本 · 免密模式</span>
          <span class="grow" />
          <span class="chevron car" :class="{ open: setListOpen }">⌄</span>
        </button>
        <div class="k-setlist" :class="{ 'is-on': setListOpen }" :hidden="!setListOpen">
          <template v-if="adv.state.order.some((x) => adv.state.visible[x])">
            <div v-for="id in adv.state.order.filter((x) => adv.state.visible[x])" :key="id" class="k-set-row">
              <span class="ldr" aria-hidden="true">{{ loaderMono(id) }}</span>
              <span class="k-set-nm">{{ loaderMeta(id).name }}</span>
              <span class="chip">ESP · {{ loaderEsp(id) || '未知' }}</span>
              <span v-if="loaderPerm(id) === 'nvram'" class="chip elev">需提权</span>
              <span v-else class="chip ok">免提权</span>
              <span class="chip">默认项 · {{ loaderDefault(id) }}</span>
              <span class="chip">脚本 · {{ id === 'grub' ? '09_anyboot' : '不适用' }}</span>
              <span class="chip">免密 · {{ id === 'opencore' ? '未开启' : '不适用' }}</span>
              <span class="grow" />
              <button type="button" class="btn xs ghost" :title="'打开 ' + loaderMeta(id).name + ' 详细设置'">⚙</button>
            </div>
          </template>
          <div v-else class="k-set-row">没有启用的引导器 —— 点上方标签开启后再设置</div>
        </div>
      </div>
    </div>
    <!-- 预演面板（旧版：bootThemeStage → .k-stage + lg-side 选中项；真实区块由 ThemeHost slot 承接） -->
    <section class="card k-prev" ref="kPrevEl">
      <header class="k-prev-head">
        <h2>预演 · {{ adv.effectiveLayout === 'lr' ? '左右' : '单列' }}</h2>
        <small>{{ adv.state.order.filter((x) => adv.state.visible[x]).length }} 个引导器</small>
      </header>
      <!-- K 不套用主题：旧版 k-zone-cards + kLoaderCard 自绘卡片 -->
      <div v-if="adv.effectiveTheme === 'K'" class="k-zone-cards">
        <div v-for="loader in previewLoaders" :key="loader.id" class="k-card">
          <div class="k-card-head">
            <span class="ldr" aria-hidden="true">{{ loaderMono(loader.id) }}</span>
            <span class="nm">{{ loader.name }}</span>
            <span class="grow" />
            <span class="hint">共 {{ (previewRows[loader.id] ?? []).length }} 条</span>
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
            <div v-if="!(previewRows[loader.id] ?? []).length" class="hint">没有条目</div>
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
