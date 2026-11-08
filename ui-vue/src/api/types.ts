/**
 * 后端 DTO 的 TypeScript 映射。
 *
 * **来源**：`src-tauri/src/state.rs` / `ui_state.rs` / `channels/*` 的
 * `#[derive(Serialize)]` 结构，全部带 `#[serde(rename_all = "camelCase")]`。
 * 字段已对着运行时真实快照（`/api/ui-state`）核对过一遍。
 *
 * **维护约定**：Rust 侧改了 `Serialize` 结构，这里必须同步改；漏改的表现是
 * 界面读不到新字段（TS 不报错、运行时是 undefined）。改完跑 `npm run typecheck`。
 */

// ---------------------------------------------------------------------------
// 基础
// ---------------------------------------------------------------------------

/** 引导器读取方式：自动（按引导器自己规则复算） / 手动（vars 快照或 ESP 配置） */
export type LoaderMode = "auto" | "manual";

/** 图标分类（前端取色点 / 选图标用） */
export type EntryClass = "mac" | "win" | "linux" | "auto" | string;

export interface ActionResult {
  ok: boolean;
  action: string;
  message: string;
}

// ---------------------------------------------------------------------------
// status：rEFInd 的 vars 快照（手动模式的数据源）
// ---------------------------------------------------------------------------

export interface VariableInfo {
  /** 这一项是不是当前「已选择」的 */
  active: boolean;
  displayName: string;
  exists: boolean;
  source: string;
  sourceName: string;
  /** windows / macos / linux / … */
  system: string;
}

export interface AppStatus {
  varsDir: string | null;
  temporaryMount: boolean;
  variables: VariableInfo[];
  message: string;
  error: string | null;
}

// ---------------------------------------------------------------------------
// refindSim：rEFInd「模拟引导项扫描」（自动模式的数据源）
// ---------------------------------------------------------------------------

export interface HealthIssue {
  severity: string;
  text: string;
}

export interface SimChild {
  title: string;
  note: string;
  options: string;
  source: string;
}

export interface SimEntry {
  /** rEFInd 的 `me.Title` 原文（末尾可能带空格，展示时 trim） */
  title: string;
  kind: string;
  source: string;
  volume: string;
  /** 分区内路径（无前导反斜杠） */
  path: string;
  /** 被 HiddenTags / DontScanFiles 挡住 —— 菜单里看不到 */
  hidden: boolean;
  /** 与 vars/PreviousBoot 的标题一致 = 上次启动的那一项 */
  lastBoot: boolean;
  children: SimChild[];
}

export interface SimPreview {
  entries: SimEntry[];
  hidden: SimEntry[];
  issues: HealthIssue[];
  summary: string;
  varsDir: string;
  configPath: string | null;
  scanfor: string;
  /** file（vars 目录） / nvram */
  varMode: string;
  /** vars/PreviousBoot 解出来的标题 */
  previousBoot: string | null;
  /** default_selection 原文 */
  defaultSelection: string;
}

// ---------------------------------------------------------------------------
// grub：ESP 上的 grub.cfg（手动模式）与 Linux 分区那份（自动模式）
// ---------------------------------------------------------------------------

export interface GrubEntry {
  id: string;
  name: string;
  selector: string;
  class: EntryClass;
  kernels: string[];
  variants: number;
  chainloader: boolean;
  submenu: string | null;
}

export interface GrubGroup {
  id: string;
  name: string;
}

export interface EspInfo {
  path?: string;
  uuid?: string;
  fat32?: boolean;
  mounted?: boolean;
  [key: string]: unknown;
}

export interface GrubView {
  partuuid: string;
  dir: string;
  esp: EspInfo | null;
  keepMounted: boolean;
  mountSupported: boolean;
  efiDir: string;
  cfgPath: string;
  envPath: string;
  envExists: boolean;
  installed: boolean;
  entries: GrubEntry[];
  groups: GrubGroup[];
  nextEntry: string;
  savedEntry: string;
  listFormat: string;
  syncedAt: string;
  scriptLog: string;
  hint: string;
  error: string | null;
}

/** Linux 分区自动读取的 grub（结构比 GrubView 多几个扫描元信息） */
export interface LinuxGrubView {
  entries: GrubEntry[];
  groups: GrubGroup[];
  nextEntry: string;
  savedEntry: string;
  envExists: boolean;
  error: string | null;
  cfgRel: string;
  cfgSize: number;
  cfgMtime: string;
  envRel: string;
  envSize: number;
  target: string;
  via: string;
  platform: string;
  scannedAt: string;
  confidence: string;
  confidenceNote: string;
  needsElevation: boolean;
  manualTakesOver: boolean;
  candidates: unknown[];
  issues: HealthIssue[];
}

// ---------------------------------------------------------------------------
// channels：systemd-boot / OpenCore / Clover
// ---------------------------------------------------------------------------

export interface ChannelMenuEntry {
  title: string;
  kind: string;
  volume: string;
  path: string;
  source: string;
  note: string;
  /** 这一行是不是当前默认项 */
  default: boolean;
  /** 辅助行（协议驱动等），不可切 */
  auxiliary: boolean;
  /** 后端算好的完整值域（「切换」命令要用） */
  spec: string;
}

export interface ChannelMenuPreview {
  entries: ChannelMenuEntry[];
  hidden: ChannelMenuEntry[];
  issues: HealthIssue[];
  summary: string;
  defaultSlot: string;
  orderSource: string;
  orderElevatable: boolean;
  prebootElevatable: boolean;
}

export interface ChannelEntry {
  id?: string;
  title?: string;
  kind?: string;
  [key: string]: unknown;
}

export interface ChannelView {
  /** sdboot / opencore / clover */
  id: string;
  label: string;
  detected: boolean;
  /** file = 免提权；nvram = 切换必经 NVRAM 写 */
  permission: string;
  /** ok / warn / off */
  chip: string;
  detail: string | null;
  entries: ChannelEntry[];
  currentDefault: string | null;
  lastBoot: string | null;
  lastBootAt?: string | null;
  error: string | null;
  health: HealthIssue[];
  /** 只有 OpenCore 有：开机菜单预演，动作行从这里出 */
  menuPreview: ChannelMenuPreview | null;
}

// ---------------------------------------------------------------------------
// 其它视图
// ---------------------------------------------------------------------------

export interface PortOccupantView {
  pid?: number;
  name?: string;
  [key: string]: unknown;
}

export interface RequestLogEntry {
  time: string;
  path: string;
  status: string;
  channel: string;
  source: string;
  summary: string;
  message: string;
}

export interface HttpView {
  enabled: boolean;
  bind: string;
  port: number;
  listening: boolean;
  addr: string | null;
  lastError: string | null;
  portOccupant: PortOccupantView | null;
  tokenSet: boolean;
  /** 打码后的 token（前 4 / 后 4 位） */
  tokenHint: string;
  /** 监听状态的一句话（主窗与托盘共用） */
  stateText: string;
  requests: RequestLogEntry[];
}

export interface BusyState {
  power: string | null;
  switching: boolean;
  scanning: boolean;
  unmounting: boolean;
}

export interface NotifyView {
  title?: string;
  body?: string;
  [key: string]: unknown;
}

/** 免密写 NVRAM 的 root 助手状态 */
export interface HelperView {
  installed: boolean;
  stale: boolean;
  supported: boolean;
  text: string;
}

export interface ChannelDelaySettings {
  delaySeconds: number;
  notify: boolean;
}

/** 主窗设置（`SettingsView`；字段极多，只列前端实际用的，其余走索引签名兜底） */
export interface SettingsView {
  refindMode: LoaderMode;
  grubMode: LoaderMode;
  grubListFormat: string;
  grubListView: string;
  grubEfiDir: string;
  grubEfiPartuuid: string;
  grubKeepMounted: boolean;
  grubScriptOverride: string;
  linuxGrubDev: string;
  httpEnabled: boolean;
  httpBind: string;
  httpPort: number;
  httpToken: string;
  theme: string;
  accentColor: string;
  opacityPercent: number;
  closeAction: string;
  startInTray: boolean;
  autostart: boolean;
  autostartState: string;
  autostartHint: string | null;
  hideFromDockTaskbar: boolean;
  mainAlwaysOnTop: boolean;
  mainAutoHide: boolean;
  mainAutoHideEdgeDistance: number;
  mainDockStripColor: string;
  mainDockStripEdgeGap: number;
  mainDockStripHoverMs: number;
  mainDockStripLength: number;
  mainDockStripOpacity: number;
  mainDockStripThickness: number;
  mainFixedMonitor: string;
  mainHotzoneBorder: boolean;
  mainHotzoneBorderColor: string;
  mainHotzoneColor: string;
  mainHotzoneOpacity: number;
  mainHotzoneThickness: number;
  mainMonitorOptions: string[];
  mainWindowLast: number[];
  mainWindowPosition: string;
  windowWidth: number;
  windowHeight: number;
  windowSizeLocked: boolean;
  notifyEnabled: boolean;
  playSound: boolean;
  soundFilePath: string;
  debugTrace: boolean;
  trayMenuStyle: string;
  remoteAllowGuiAuth: boolean;
  channels: { cli: ChannelDelaySettings; http: ChannelDelaySettings };
  gatewayMode: string;
  gatewayAutostart: boolean;
  gatewayNamedUrl: string;
  gatewayNamedToken: string;
  tailscaleAutostart: boolean;
  tailscaleAuthkey: string;
  tsnetAutostart: boolean;
  tsnetAuthkey: string;
  updateAutoCheck: boolean;
  updateMirror: string;
  updateSkipVersion: string;
  machineId: string;
  /** 后端新增字段不破坏前端 */
  [key: string]: unknown;
}

// ---------------------------------------------------------------------------
// 顶层快照
// ---------------------------------------------------------------------------

/** `ui-state-changed` 事件 / `/api/ui-state` 的完整快照 */
export interface UiState {
  revision: number;
  status: AppStatus;
  settings: SettingsView;
  busy: BusyState;
  lastAction: ActionResult | null;
  http: HttpView;
  notify: NotifyView | null;
  grub: GrubView;
  channels: ChannelView[];
  helper: HelperView;
  grubScript: string;
  docked: boolean;
  dockEdge: string | null;
  refindSim: SimPreview | null;
  linuxGrub: LinuxGrubView | null;
}

// ---------------------------------------------------------------------------
// 远程（电脑列表）用的视图：`remote_boot_info` 的返回
// ---------------------------------------------------------------------------

export interface RemoteBootOption {
  value: string;
  label: string;
  /** 图标分类：mac / win / linux / auto */
  class: EntryClass;
  /** 组头行（value 恒空、不可选） */
  header: boolean;
  /** 固定行（value 恒空、不可切） */
  fixed: boolean;
}

export interface RemoteBootGroup {
  kind: string;
  title: string;
  available: boolean;
  current: string;
  next: string;
  saved: string;
  note: string;
  options: RemoteBootOption[];
}

export interface RemoteBootInfo {
  ok: boolean;
  online: boolean;
  addr: string;
  error: string;
  refind: RemoteBootGroup;
  grub: RemoteBootGroup;
  channels: RemoteBootGroup[];
  probe_results?: { address: string; ok: boolean; ms: number; error: string; kind?: string; pair_id?: string; remote?: boolean }[];
  machineOs?: string;
}
