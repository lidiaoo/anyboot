// ═══════════════════════════════════════════════════════════════════════
// boot-adv.js —— 引导高级设置 + 版式主题（一比一还原设计稿
// docs/boot-menu-layouts.html：类名、DOM 结构、样式全部用设计稿原名原样）
//
// 职责边界：
//   ① 状态模型 visible/order/side/layout/ratio/style/theme + localStorage 持久化
//   ② 数据适配层：五类引导器 ← 后端快照统一成设计稿 LOADERS 形状
//   ③ 「引导高级设置」页签 = 设计稿 viewK（k-bar 设置行 + k-stage 预演）
//   ④ 「系统引导」页（#tab-boot）布置：K = order/左右分栏；
//      A/B/C/G/I/J = 设计稿 viewA…J 同构外壳 + 真实区块嵌入（动作照常可用）
//
// 铁律（项目备忘）：
//   - Rust→JS 字段一律 camelCase
//   - 持久化键 anyboot.boot-adv（v1 前端 localStorage）
//   - 通道卡（.boot-channel）由 boot-list-view.js 每拍重建：搬出挂载点的卡
//     可能陈旧，teardown 时「mount 里没有同 id 卡才搬回，否则删」
// ═══════════════════════════════════════════════════════════════════════

(function () {
  "use strict";

  // ── 引导器种类与元数据（mono/color/perm 与设计稿 LOADERS 对齐）────────
  const LOADER_IDS = ["refind", "grub", "sdboot", "opencore", "clover"];
  const LOADER_META = {
    refind: { name: "rEFInd", mono: "rE", color: "#14b8a6", perm: "file" },
    grub: { name: "GRUB", mono: "GR", color: "#f59e0b", perm: "file" },
    sdboot: { name: "systemd-boot", mono: "sd", color: "#38bdf8", perm: "file" },
    opencore: { name: "OpenCore", mono: "OC", color: "#6366f1", perm: "nvram" },
    clover: { name: "Clover", mono: "CL", color: "#a78bfa", perm: "file" },
  };
  const OS_OF_ICON = { linux: "lin", windows: "win", macos: "mac", menu: "tool" };
  const OS_TAG = { mac: "MAC", win: "WIN", lin: "LNX", tool: "TLS", fw: "FW" };

  // ── 版式主题 ──────────────────────────────────────────────────────────
  // 各主题可用的布局档位（2026-10-30 用户定案：「B / G / J 必须只能以加宽的
  // 窗口布局」）——B 左栏通道 / G 命令面板 / J 三栏本身就要吃满两栏标称宽，
  // 窗口窄回 460 时它们只剩一条缝，所以这三个主题**只有加宽**这一档。
  // 首项 = 该主题的基准档（存的档位不在表里时按它渲染）。
  const THEME_LAYOUT_MODES = {
    K: ["default", "wide", "lr"],
    A: ["default", "wide", "lr"],
    C: ["default", "wide", "lr"],
    B: ["wide"],
    G: ["wide"],
    I: ["default", "wide"],
    J: ["wide"],
  };
  // 支持左右布局 = 档位表里有 lr（B/G/I/J 只有单列，保持旧语义）
  const THEME_H = Object.fromEntries(
    Object.entries(THEME_LAYOUT_MODES).map(([k, v]) => [k, v.includes("lr")]),
  );
  const THEMES = [
    ["K", "不套用主题"],
    ["A", "A 卡片流"],
    ["B", "B 左栏通道"],
    ["C", "C 顶部固定"],
    ["G", "G 命令面板"],
    ["I", "I Bento"],
    ["J", "J 三栏"],
  ];
  const THEME_RENDERERS = {}; // 预演渲染器（设置页）：theme -> fn(loaders) -> DOM
  const THEME_ARRANGERS = {}; // 真实页布置器（#tab-boot）

  // ── 访客主题（远程查看）───────────────────────────────────────────────
  // hub 的电脑页 iframe / 手机 WebView 看某台电脑时，加载的是**那台电脑**
  // 自己的页面 —— 跨来源，localStorage 各存各的桶，主题偏好互不相通（看
  // 一台就要重选一次，还让人以为「改了他的主题」）。查看方把「我应用的
  // 主题」经 URL（abTheme=<字母>）带过来：页面照它渲染，但**不读也不写**
  // 本地桶里的主题 —— 「不改掉他的主题，用我的主题看」。主窗口自己打开
  // 时没有这个参数，行为不变。
  const GUEST_THEME = (() => {
    try {
      const val = new URLSearchParams(window.location.search).get("abTheme") || "";
      return THEME_LAYOUT_MODES[val] ? val : "";
    } catch (error) {
      void error;
      return "";
    }
  })();
  const isGuestView = () => GUEST_THEME !== "";
  /// 实际渲染的主题：访客跟随查看方（URL 参数），本机用自己保存的
  function effectiveTheme() {
    return GUEST_THEME || advState.theme;
  }

  // ── 状态模型 + 持久化 ─────────────────────────────────────────────────
  const STORAGE_KEY = "anyboot.boot-adv";
  const DEFAULT_STATE = {
    visible: { refind: true, grub: true, sdboot: true, opencore: true, clover: true },
    order: LOADER_IDS.slice(),
    side: {},
    layout: "default",
    ratio: 0.5,
    style: "full",
    // 出厂默认 = G 命令面板（2026-10-31 定案：电脑与移动端一致，都是 G）。
    // 只影响没存过偏好的新环境；已保存过主题的老用户保持自己的选择。
    theme: "G",
  };

  let advState = loadState();

  function loadState() {
    const base = JSON.parse(JSON.stringify(DEFAULT_STATE));
    try {
      const raw = localStorage.getItem(STORAGE_KEY);
      if (!raw) return base;
      const saved = JSON.parse(raw);
      if (!saved || typeof saved !== "object") return base;
      for (const key of ["visible", "order", "side", "layout", "ratio", "style", "theme"]) {
        if (saved[key] !== undefined) base[key] = saved[key];
      }
    } catch (error) {
      void error;
    }
    return base;
  }

  function saveState() {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(advState));
    } catch (error) {
      void error;
    }
  }

  function resetState() {
    advState = JSON.parse(JSON.stringify(DEFAULT_STATE));
    saveState();
  }

  function normState(presentIds) {
    const ids = presentIds && presentIds.length ? presentIds : LOADER_IDS;
    advState.order = Array.isArray(advState.order)
      ? advState.order.filter((id) => ids.includes(id))
      : [];
    for (const id of ids) {
      if (!advState.order.includes(id)) advState.order.push(id);
    }
    advState.side = advState.side && typeof advState.side === "object" ? advState.side : {};
    for (const id of advState.order) {
      if (advState.side[id] !== 2) advState.side[id] = 1;
    }
    if (!["default", "wide", "lr"].includes(advState.layout)) advState.layout = "default";
    // 主题不支持左右布局时**不要**把用户的选择改掉（原先这里直接改回 default，
    // 于是「选了左右布局 → 一拍快照就变回默认」，看起来就是设置没保存）。
    // 支不支持在渲染时用 effectiveLayout() 判，用户切回支持的主题还能拿回 lr。
    if (!["full", "compact", "min"].includes(advState.style)) advState.style = "full";
    if (advState.ratio < 0.15) advState.ratio = 0.15;
    if (advState.ratio > 0.85) advState.ratio = 0.85;
  }

  /// 当前主题允许的布局档位（首项 = 基准档）
  function layoutModes(theme) {
    return THEME_LAYOUT_MODES[theme || effectiveTheme()] || ["default", "wide"];
  }

  /// 实际生效的布局：存的档位当前主题没有时，按该主题的**基准档**渲染
  /// （B/G/J 基准 = 加宽，其余基准 = 默认）。选择本身保留在 advState.layout
  /// 里，切回支持的主题能拿回原档位。
  function effectiveLayout() {
    const modes = layoutModes();
    return modes.includes(advState.layout) ? advState.layout : modes[0];
  }
  const layoutSupported = () => layoutModes().includes("lr");
  /// 该主题是否「只吃加宽」（B/G/J）：设置里「默认 / 左右布局」置灰说明
  const layoutWideOnly = () => {
    const modes = layoutModes();
    return modes.length === 1 && modes[0] === "wide";
  };

  // ── 数据适配层：快照 → 五类引导器统一形状 ─────────────────────────────
  let lastLoaders = [];

  function adaptChannels(channels) {
    const byId = {};
    for (const c of channels || []) byId[c.id] = c;
    return byId;
  }

  // 预演条目的 kind → 图标类别（`MenuEntry.kind`：macos/windows/generic/linux/tool）
  const OS_OF_KIND = { windows: "win", macos: "mac", linux: "lin", generic: "tool", tool: "tool", unknown: "tool" };

  // icon/kind 认不出时按标题关键词兜底（泛型 EFI 二进制的路径里大多带着
  // 系统名：\ArchLinux\…、Windows Boot Manager、com.apple.recovery…）。
  // kind 只有 windows/macos/linux 三档算「真系统」，generic/tool/unknown 都
  // 放行给 osByTitle —— 泛型条目标题写着 Windows 时就该挂 Windows 图标。
  function osByTitle(title) {
    const s = String(title || "").toLowerCase();
    if (s.includes("windows")) return "win";
    if (s.includes("mac") || s.includes("apple")) return "mac";
    if (s.includes("linux")) return "lin";
    return "";
  }
  const osOfKind = (kind) => (kind === "windows" || kind === "macos" || kind === "linux")
    ? OS_OF_KIND[kind]
    : "";

  function channelEntries(channel) {
    const out = [];
    // 选中项判定必须跟列表同一份规则（boot-list-view.js 的 isCurrentDefault）：
    // currentDefault 有时是条目 id（Boot0080）、有时是标题，只比 title 会让
    // 「下次启动」那几行永远显示「未选中条目」。
    const cur = channel.currentDefault || "";
    const hit = (spec, title) => Boolean(cur) && (cur === spec || cur === title
      || String(cur).includes(title || "\u0000"));
    for (const e of channel.entries || []) {
      out.push({
        id: e.id || e.title,
        t: e.title || "(未命名)",
        detail: e.detail || "",
        os: OS_OF_ICON[e.icon] || osByTitle(e.title) || "tool",
        aux: Boolean(e.requiresElevation) === false && e.icon === "menu",
        def: hit(e.id, e.title),
        src: "",
      });
    }
    // OpenCore：真正的菜单项在 menuPreview 里（entries 只有「写入固件启动项」这类
    // 修复动作）。托盘那块下拉就是这么算的（menu_model::channel_switch_rows），
    // 这里必须同源，否则「下次启动」会漏掉 OpenCore 那一行。
    const preview = channel.menuPreview;
    for (const e of (preview && preview.entries) || []) {
      const spec = e && e.spec ? String(e.spec) : "";
      // spec 为空 = 不能设默认（协议驱动项 / 工具项），列进来点也点不动
      if (!spec || !e.title) continue;
      out.push({
        id: spec,
        t: e.title,
        detail: e.note || "",
        os: osOfKind(e.kind) || osByTitle(e.title) || "tool",
        aux: Boolean(e.auxiliary),
        def: Boolean(e.default) || hit(spec, e.title),
        src: e.source || "",
      });
    }
    // 默认项可能压根没有切换值域：OpenCore 的 macOS 走 APFS Preboot，没法定
    // `opencore-default:<卷>\x01<路径>`（spec=None）。它不在上面那批里，但主窗
    // 列表里它是绿框选中 —— 主题列表也得能看见它，否则「当前默认」在主题视图
    // 里凭空消失。只补这一条：其它 spec 为空的（工具项 / OC 自己的 Bootstrap
    // 入口）点了没反应，列出来只会让人以为坏了。
    if (!out.some((e) => e.def)) {
      const row = ((preview && preview.entries) || []).find((e) => e && e.default && e.title);
      if (row) {
        out.push({
          id: row.spec || row.title,
          t: row.title,
          detail: "",
          os: osOfKind(row.kind) || osByTitle(row.title) || "tool",
          aux: Boolean(row.auxiliary),
          // 已在默认位：主题渲染器对 def 行只画徽标、不给「设为默认」按钮
          def: true,
          fixed: true,
          src: row.source || "",
        });
      }
    }
    return out;
  }

  // 引导器的读取方式（自动 / 手动）在后端设置里，托盘与这里同源 —— 它决定
  // 「下次启动」那一行取哪份数据的选中项，不按它走两边就会各说各的。
  function loaderMode(state, loader) {
    const settings = (state && state.settings) || {};
    const raw = loader === "grub" ? settings.grubMode : settings.refindMode;
    return raw === "manual" ? "manual" : "auto";
  }

  function refindLoader(state) {
    const meta = LOADER_META.refind;
    const mode = loaderMode(state, "refind");
    const entries = [];
    if (mode === "manual") {
      // 手动处理链路 = vars 快照那三行系统（点它写的是快照，不是 PreviousBoot），
      // 所以 id 用 system 名 —— 与自动扫描链路的标题原文不是同一套值域。
      for (const item of (state.status && state.status.variables) || []) {
        const name = (item.displayName || "").trim();
        if (!item.exists || !name) continue;
        entries.push({
          id: item.system,
          t: name,
          detail: item.active ? "已选择" : (item.sourceName || ""),
          os: osOfKind(item.system) || osByTitle(item.displayName) || "tool",
          aux: false,
          def: Boolean(item.active),
          src: item.sourceName || "",
          system: item.system,
        });
      }
      return {
        id: "refind", ...meta,
        detected: entries.length > 0,
        esp: "vars/PreviousBoot 快照",
        healthText: (state.status && state.status.error)
          || (entries.length ? `手动处理：${entries.length} 个系统` : "还没生成快照：切一次系统就有"),
        entries,
        nextBoot: entries.find((e) => e.def) || null,
      };
    }
    const sim = state && state.refindSim;
    for (const e of (sim && sim.entries) || []) {
      const children = e.children || [];
      entries.push({
        id: e.title,
        t: (e.title || "").trim(),
        detail: e.detail || (children.length ? `＋ ${children.length} 项子菜单` : ""),
        os: OS_OF_ICON[e.icon] || "tool",
        aux: children.length > 0,
        // rEFInd 的「选中项」是 vars/PreviousBoot 命中的那一条（列表里挂
        // 「下次启动」徽标用的就是 lastBoot），不是 default_selection。
        def: Boolean(e.lastBoot),
        src: e.source || "",
      });
    }
    const detected = Boolean(sim && entries.length);
    return {
      id: "refind", ...meta,
      detected,
      esp: "ESP · refind.conf",
      healthText: (sim && sim.summary) || "还没扫描：变量目录就绪后自动复算",
      entries,
      nextBoot: entries.find((e) => e.def) || null,
    };
  }

  function grubLoader(state) {
    const meta = LOADER_META.grub;
    const mode = loaderMode(state, "grub");
    const esp = (state && state.grub) || {};
    const linux = (state && state.linuxGrub) || {};
    // 自动读取 = 直接读 Linux 分区那份 grub.cfg / grubenv（主窗「自动读取」面板
    // 显示的就是它）；手动解析 = ESP 上那份。选中项必须跟着同一份走。
    const view = mode === "manual" ? esp : linux;
    const active = mode === "manual"
      ? (esp.nextEntry || esp.savedEntry || "")
      : (linux.nextEntry || linux.savedEntry || "");
    const entries = [];
    // 选中项命中可能在**组内子项**上：grubenv 里的 next/saved 经常是复合
    // selector（`<组 id>><子项 selector>`，os-prober 的条目整串都是），而
    // 上面列表只铺顶层条目 —— 在这里先把子项跳掉的话，选中项永远命不中，
    // 「下次启动」就一直是「未选中条目」，可下面卡片里明明有置顶的选中行
    // （真实 GRUB 卡的置顶是**全量** entries 里找的，两边必须同一口径）。
    const groupNames = new Map(((view && view.groups) || []).map((g) => [g.id, g.name]));
    let childDef = null;
    for (const e of (view && view.entries) || []) {
      const isDef = Boolean(active) && e.selector === active;
      if (e.submenu) {
        if (isDef) {
          const gname = groupNames.get(e.submenu) || e.submenu;
          childDef = {
            id: e.selector,
            // 与真实卡片置顶行同一规则：组名 › 子项名（脱离了组看不出归属）
            t: `${gname} › ${e.name || "(未命名)"}`,
            detail: e.kernels && e.kernels[0] ? `内核 ${e.kernels[0]}` : e.class || "",
            os: /windows/i.test(e.name || "") ? "win"
          : /mac|apple/i.test(e.name || "") ? "mac"
          : "lin",
            aux: true,
            def: true,
            src: e.class || "",
          };
        }
        continue;
      }
      const detail = e.kernels && e.kernels[0] ? `内核 ${e.kernels[0]}`
        : e.chainloader ? `转交 ${e.chainloader}`
        : e.class || "";
      entries.push({
        id: e.selector,
        t: e.name || "(未命名)",
        detail,
        os: /windows/i.test(e.name || "") ? "win"
          : /mac|apple/i.test(e.name || "") ? "mac"
          : "lin",
        aux: Boolean(e.submenu) || (Number(e.variants) || 1) > 1,
        def: isDef,
        src: e.class || "",
      });
    }
    const target = view && view.target;
    // 手动那份（ESP 的 GrubView）没有 `target`，只有 cfgPath/efiDir —— 按
    // 「有没有条目」判就够，别再要求一个它不存在的字段（否则手动模式整块消失）。
    // 与 ui/boot-adv.js 同口径（2026-10-31）：严格按模式、零回落 —— 自动模式
    // 只认「自动读取」那份，没扫到就是空列表 / 计数 0，绝不拿 ESP 那份垫底。
    const detected = entries.length > 0;
    return {
      id: "grub", ...meta,
      detected,
      // 副标题也按模式各说各的：手动报 ESP 的 cfg 路径，自动报分区上的相对路径
      esp: mode === "manual"
        ? (esp.cfgPath || "grub.cfg")
        : ((target && target.cfgRel) || ""),
      healthText: mode === "manual"
        ? (entries.length
          ? `ESP · ${entries.length} 个启动项`
          : "还没解析：点「重新解析」读 ESP 上的 grub.cfg")
        : (entries.length
          ? ((target && `${target.dev || ""} ${entries.length} 个启动项`.trim())
            || `${entries.length} 个启动项`)
          : "还没读过：点「重新读取」自动定位 Linux 分区"),
      entries,
      // 选中项在组内子项上时不在 entries 里（上面只铺顶层），用 childDef 补
      nextBoot: entries.find((e) => e.def) || childDef,
    };
  }

  // 「下次启动」那行显示的默认项 —— 与「列表里哪条能点」不是一回事。
  // OpenCore 的默认项可能没有切换值域（macOS 的引导文件在 APFS Preboot 卷里，
  // OC 不产 `opencore-default:…`），它进不了 channelEntries，但确实是下次会启动
  // 的那一条；只按可切换集合判默认，那一行永远是「未选中条目」。
  function channelNextBoot(channel, entries) {
    const hit = entries.find((e) => e.def);
    if (hit) return hit;
    const preview = channel.menuPreview;
    const row = ((preview && preview.entries) || []).find((e) => e && e.default && e.title);
    if (!row) return null;
    return {
      id: row.spec || row.title,
      t: row.title,
      // 不拿 note 当副标题：OC 的 note 是整句解释（「引导文件在 APFS Preboot
      // 卷里……」），塞进「下次启动」那一行会撑爆版式。
      detail: "",
      os: osOfKind(row.kind) || osByTitle(row.title) || "tool",
      aux: true,
      def: true,
      src: row.source || "",
    };
  }

  function channelLoader(channel) {
    const meta = LOADER_META[channel.id] || { name: channel.label || channel.id, mono: channel.id.slice(0, 2), color: "#888", perm: "file" };
    const entries = channelEntries(channel);
    return {
      id: channel.id, ...meta,
      detected: Boolean(channel.detected),
      esp: channel.detail || "",
      healthText: (channel.health || []).map((h) => h.text).join(" · ") || channel.detail || "",
      entries,
      nextBoot: channelNextBoot(channel, entries),
    };
  }

  function onSnapshot(state) {
    const channels = adaptChannels(state && state.channels);
    const loaders = [
      refindLoader(state),
      grubLoader(state),
      channelLoader(channels.sdboot),
      channelLoader(channels.opencore),
      channelLoader(channels.clover),
    ];
    lastLoaders = loaders;
    normState(loaders.map((l) => l.id));
    applyAll();
  }

  function enabledLoaders() {
    return advState.order
      .map((id) => lastLoaders.find((l) => l.id === id))
      .filter((l) => l && advState.visible[l.id]);
  }

  // ═══════════════════════════════════════════════════════════════════
  // 设计稿原子（docs/boot-menu-layouts.html 第 1/3 节原样移植）
  // ═══════════════════════════════════════════════════════════════════
  const NS = "http://www.w3.org/2000/svg";
  const ICONS = {
    search: '<circle cx="7.2" cy="7.2" r="4.6"/><path d="M10.6 10.6 14 14"/>',
    gear: '<circle cx="8" cy="8" r="2.4"/><path d="M8 1.6v1.8M8 12.6v1.8M1.6 8h1.8M12.6 8h1.8M3.5 3.5l1.3 1.3M11.2 11.2l1.3 1.3M12.5 3.5l-1.3 1.3M4.8 11.2l-1.3 1.3"/>',
    pin: '<path d="M6.2 2.2h3.6l-.7 3.4 2.4 2.6H4.5l2.4-2.6-.7-3.4Z"/><path d="M8 8.2v5.2"/>',
    grip: '<circle cx="6" cy="4" r="1"/><circle cx="10" cy="4" r="1"/><circle cx="6" cy="8" r="1"/><circle cx="10" cy="8" r="1"/><circle cx="6" cy="12" r="1"/><circle cx="10" cy="12" r="1"/>',
    chevron: '<path d="M4 6.2 8 10l4-3.8"/>',
    more: '<circle cx="3.5" cy="8" r="1.1"/><circle cx="8" cy="8" r="1.1"/><circle cx="12.5" cy="8" r="1.1"/>',
    check: '<path d="M3.2 8.4 6.4 11.6 12.8 4.8"/>',
    warn: '<path d="M8 2.6 14 13H2L8 2.6Z"/><path d="M8 6.4v3.2M8 11.4v.1"/>',
    refresh: '<path d="M13.4 8a5.4 5.4 0 1 1-1.6-3.8"/><path d="M13.6 2.4v2.8h-2.8"/>',
    elev: '<circle cx="5.6" cy="5.6" r="2.4"/><path d="M7.4 7.4 13 13"/><path d="M11 13h2v-2"/>',
    plus: '<path d="M8 3.2v9.6M3.2 8h9.6"/>',
    arrow: '<path d="M3.5 8h9M9 4.5 12.5 8 9 11.5"/>',
    x: '<path d="M4 4l8 8M12 4l-8 8"/>',
    power: '<path d="M8 2.4v5.2"/><path d="M12 4.6a5.6 5.6 0 1 1-8 0"/>',
    sliders: '<path d="M3 5h10M3 11h10"/><circle cx="6" cy="5" r="1.6"/><circle cx="10" cy="11" r="1.6"/>',
    folder: '<path d="M2.4 4.6h4l1.2 1.6h6v6.4H11.2Z"/>',
    shield: '<path d="M8 2.2 13 4v4.2c0 3-2.1 4.8-5 5.8-2.9-1-5-2.8-5-5.8V4l5-1.8Z"/>',
    clock: '<circle cx="8" cy="8" r="5.6"/><path d="M8 5v3.2l2.2 1.4"/>',
    info: '<circle cx="8" cy="8" r="5.6"/><path d="M8 7.2v4M8 4.9v.1"/>',
    sort: '<path d="M5 3v10M5 3 2.8 5.2M5 3l2.2 2.2M11 13V3M11 13l2.2-2.2M11 13 8.8 10.8"/>',
    eye: '<path d="M1.8 8S4.2 3.8 8 3.8 14.2 8 14.2 8 11.8 12.2 8 12.2 1.8 8 1.8 8Z"/><circle cx="8" cy="8" r="1.8"/>',
    doc: '<path d="M3.6 2.4h5.6l3.2 3.2v8H3.6Z"/><path d="M9.2 2.4v3.2h3.2"/>',
    bolt: '<path d="M9 2 3.8 9.2h3.4L7 14l5.2-7.2H8.8L9 2Z"/>',
  };

  function n(tag, cls, txt) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (txt != null) e.textContent = txt;
    return e;
  }
  function icon(name, cls) {
    const s = document.createElementNS(NS, "svg");
    s.setAttribute("viewBox", "0 0 16 16");
    s.setAttribute("class", "i " + (cls || ""));
    s.innerHTML = ICONS[name] || "";
    return s;
  }
  function ldr(l, size) {
    const e = n("span", "ldr" + (size ? " " + size : ""), l.mono);
    e.style.setProperty("--c", l.color);
    return e;
  }
  function chip(kind, txt, ic) {
    const e = n("span", "chip" + (kind ? " " + kind : ""));
    if (ic) e.append(icon(ic, "sm"));
    e.append(document.createTextNode(txt));
    return e;
  }
  function badge(kind, txt) {
    return n("span", "badge" + (kind ? " " + kind : ""), txt);
  }
  /// 条目行的系统字章（.row-ico）：win/mac/lin 画系统 logo，其余退回字章文本。
  /// logo 数据在 grub-list-view.js 的 OS_LOGO_SVG（本文件可能单独加载，要兜底）。
  function osIco(os) {
    const e = n("span", "row-ico" + (os ? " " + os : ""));
    const logo = (typeof OS_LOGO_SVG !== "undefined") && OS_LOGO_SVG[os];
    if (logo) {
      if (OS_TAG[os]) e.title = OS_TAG[os];
      const s = document.createElementNS(NS, "svg");
      s.setAttribute("viewBox", logo.vb);
      s.innerHTML = logo.d;
      e.append(s);
    } else {
      e.textContent = OS_TAG[os] || "?";
    }
    return e;
  }
  function dot(state) { return n("i", "dot " + (state || "ok")); }
  function btn(txt, cls, ic) {
    const b = n("button", cls || "btn sm");
    b.type = "button";
    if (ic) b.append(icon(ic, "sm"));
    if (txt) b.append(document.createTextNode(txt));
    return b;
  }
  function icoBtn(name, title, cls) {
    const b = btn("", cls || "btn icon ghost sm", name);
    b.title = title || "";
    b.setAttribute("aria-label", title || name);
    return b;
  }
  /// 分隔条正中那颗「回正中间」按钮（2026-10-30 用户要求：分隔位置要能一键
  /// 重置到宽度正中间）。双击分隔条也能复位，但没人会去猜，所以给个明确入口。
  /// 拖动是靠 pointerdown 起手的 —— 按钮上必须掐掉，否则按下就变成拖拽。
  function splitResetButton(reset) {
    const b = n("button", "k-split-reset");
    b.type = "button";
    b.title = "点击把分隔线重置到正中间（50%）";
    b.setAttribute("aria-label", "重置到正中间");
    b.append(icon("refresh", "sm"));
    b.addEventListener("pointerdown", (ev) => ev.stopPropagation());
    b.addEventListener("dblclick", (ev) => ev.stopPropagation());
    b.addEventListener("click", (ev) => {
      ev.stopPropagation();
      reset();
    });
    return b;
  }
  // 状态：设计稿语义 state warn/off/ok ← 适配层（perm nvram = 需提权）
  function stateOf(l) {
    if (!l.detected) return "off";
    return l.perm === "nvram" ? "warn" : "ok";
  }
  function stateChip(l) {
    if (stateOf(l) === "warn") return chip("warn", "已检测 · 需提权", "elev");
    if (stateOf(l) === "off") return chip("off", "未检测到");
    return chip("ok", "已检测 · 免提权");
  }
  function loaderMeta(l) { return `${l.esp || "ESP 未知"} · ${l.entries.length} 个条目`; }

  /** 条目行：设计稿 entryRow 原样（预演只读，动作按钮为静态样张） */
  function entryRow(e, l, opt) {
    opt = opt || {};
    const row = n("div", "row" + (e.def && !opt.noDefStyle ? " is-def" : ""));
    row.append(osIco(e.os));
    const main = n("span", "row-main");
    main.append(n("b", null, e.t));
    main.append(n("small", null, e.detail || e.src || ""));
    row.append(main);
    if (e.def) row.append(badge("def", "当前默认"));
    if (e.aux) row.append(badge(null, "辅助"));
    const acts = n("span", "row-acts");
    if (e.def) acts.append(btn("已在默认位", "btn xs ghost is-disabled"));
    else acts.append(btn("设为默认", "btn xs", "elev"));
    acts.append(btn("下次启动", "btn xs"));
    if (opt.more !== false) acts.append(icoBtn("more", "更多操作（路径 / 来源 / 复制）"));
    row.append(acts);
    return row;
  }

  /** 引导器级设置面板（.panel-in/.set 原子，数据来自适配层） */
  function settingsPanel(l, cols) {
    const box = n("div", "panel-in");
    if (cols) box.style.gridTemplateColumns = `repeat(auto-fit,minmax(${cols}px,1fr))`;
    const def = l.nextBoot || l.entries.find((e) => e.def) || l.entries[0];
    const rows = [
      ["ESP 路径", l.esp || "未知", "改"],
      ["权限", l.perm === "nvram" ? "写 NVRAM 需提权" : "免提权", l.perm === "nvram" ? "提升" : "说明"],
      ["默认项", (def && def.t) || "未知", "改"],
      ["内置脚本", l.id === "grub" ? "09_anyboot" : "不适用", "编辑"],
      ["免密模式", l.id === "opencore" ? "未开启 · 每次弹一次授权" : "不适用", "开启"],
    ];
    for (const s of rows) {
      const row = n("div", "set");
      const t = n("div", "set-t");
      t.append(n("b", null, s[0]), n("small", null, s[1]));
      row.append(t);
      if (s[2] === "开启" && l.id === "opencore") row.append(n("span", "switch"));
      else row.append(btn(s[2], "btn xs ghost"));
      box.append(row);
    }
    return box;
  }
  /** 可折叠设置条（aria-expanded + 面板），返回 {bar, panel} */
  function settingsAccordion(l, label) {
    const bar = n("button", "acc");
    bar.type = "button";
    bar.setAttribute("aria-expanded", "false");
    const g = n("span", "grow");
    g.append(n("span", null, label || "引导器设置"));
    g.append(n("small", null, "ESP 路径、权限、默认项、脚本"));
    bar.append(icon("gear", "sm"), g, icon("chevron", "sm car"));
    const panel = settingsPanel(l);
    panel.hidden = true;
    bar.addEventListener("click", () => {
      const open = bar.getAttribute("aria-expanded") === "true";
      bar.setAttribute("aria-expanded", String(!open));
      panel.hidden = open;
    });
    return { bar, panel };
  }

  // ═══════════════════════════════════════════════════════════════════
  // K 预演（设计稿 viewK 原样；数据从适配层来）
  // ═══════════════════════════════════════════════════════════════════
  const byId = () => {
    const m = {};
    lastLoaders.forEach((l) => { m[l.id] = l; });
    return m;
  };

  // 预演条目：每家取前 5 条示意（与设计稿一致；真机条目多的滚动发生在「系统引导」页）
  function kEntriesFor(l) { return l.entries.slice(0, 5); }

  function kLoaderCard(l) {
    const card = n("div", "k-card");
    const head = n("div", "k-card-head");
    head.append(ldr(l, "sm"), n("span", "nm", l.name), stateChip(l), n("span", "grow"));
    head.append(n("span", "hint", "共 " + l.entries.length + " 条"));
    head.append(btn("下次启动", "btn xs"));
    card.append(head);
    const scroll = n("div", "k-entries");
    const list = n("div", "entries");
    list.style.padding = "2px 0 0";
    const list0 = kEntriesFor(l);
    if (!list0.length) list.append(n("div", "hint", l.detected ? "没有条目" : "未检测到：装好后重新扫描"));
    list0.forEach((e) => list.append(entryRow(e, l, { more: false })));
    scroll.append(list);
    card.append(scroll);
    return card;
  }

  let setOpen = false; // 引导器设置折叠条展开态（内存态）

  function sortStrip(list, lbl, ids) {
    const sbar = n("div", "k-sortbar");
    sbar.append(n("span", "k-sort-lbl", lbl || "上下排序"));
    const m = byId();
    list.forEach((id, i) => {
      const c = n("span", "k-sortchip");
      c.append(n("span", "k-sort-nm", (m[id] || { name: id }).name));
      const up = zop("↑", "上移一位", () => moveLoader(id, -1));
      const dn = zop("↓", "下移一位", () => moveLoader(id, 1));
      up.disabled = i === 0;
      dn.disabled = i === list.length - 1;
      c.append(up, dn);
      sbar.append(c);
    });
    return sbar;
  }

  function moveLoader(id, delta) {
    const order = advState.order;
    const i = order.indexOf(id);
    const j = i + delta;
    if (i < 0 || j < 0 || j >= order.length) return;
    [order[i], order[j]] = [order[j], order[i]];
    saveState();
    applyAll();
  }

  function zop(txt, title, fn) {
    const b = n("button", "k-zop", txt);
    b.type = "button";
    b.title = title;
    b.addEventListener("click", fn);
    return b;
  }

  // 容器断点：设计稿用 ResizeObserver 往容器挂 .vw-900/.vw-760/.vw-520
  function applyVwClasses(el, width) {
    el.classList.toggle("vw-g", width >= 900);
    el.classList.toggle("vw-900", width <= 900);
    el.classList.toggle("vw-760", width <= 760);
    el.classList.toggle("vw-520", width <= 520);
  }
  const vwObserved = new WeakSet();
  function observeVw(el) {
    if (!el || vwObserved.has(el) || typeof ResizeObserver === "undefined") return;
    vwObserved.add(el);
    const ro = new ResizeObserver((entries) => {
      for (const en of entries) applyVwClasses(el, en.contentRect.width);
    });
    ro.observe(el);
    applyVwClasses(el, el.getBoundingClientRect().width);
  }

  function renderAdvTab() {
    const root = document.getElementById("boot-adv-mount");
    if (!root) return;
    normState(LOADER_IDS);
    const m = byId();
    // G 主题搜索框：重建前记住值与焦点，重建后恢复（焦点恢复必须在挂进文档之后）
    const prevInput = root.querySelector(".lg-search input");
    const prevG = {
      value: prevInput ? prevInput.value : "",
      focus: Boolean(prevInput && document.activeElement === prevInput),
    };

    const wrap = n("div", "k");
    const bar = n("div", "k-bar");

    // row1：启用引导器 —— .k-tabs 文字标签行（设计稿同形）
    const row1 = n("div", "k-tabs");
    const tabs = {};
    const lrOn = effectiveLayout() === "lr";
    for (const id of LOADER_IDS) {
      const l = m[id] || { ...LOADER_META[id], entries: [], detected: false };
      const c = n("button", "k-tab");
      c.type = "button";
      c.title = "点击 开/关 展示该引导器";
      c.append(ldr(l, "sm"), n("span", null, l.name));
      const rg = n("span", "k-rg", advState.side[id] === 2 ? "右" : "左");
      rg.title = "点击挪到另一列（没开左右布局时，点它自动切过去）";
      rg.addEventListener("click", (e) => {
        e.stopPropagation();
        // 没开左右布局时点它 = 先切到左右布局再分配。原先这颗按钮在非 lr 下
        // 是 display:none，点的位置落到标签本身 → 把该引导器关掉了，用户看到
        // 的就是「点了没反应 / 我的设置没保存」（真实踩过：数据里 grub 被关）。
        if (effectiveLayout() !== "lr" && layoutSupported()) {
          advState.layout = "lr";
          layoutWidthSent = null; // 切布局要落宽：强制发一次
        }
        advState.side[id] = advState.side[id] === 2 ? 1 : 2;
        saveState();
        applyAll();
      });
      c.append(rg);
      c.addEventListener("click", () => {
        advState.visible[id] = !advState.visible[id];
        saveState();
        applyAll();
      });
      tabs[id] = c;
      row1.append(c);
    }
    row1.append(n("span", "grow"));
    const reset = btn("重置", "btn xs ghost", "refresh");
    reset.title = "回到默认编排";
    reset.addEventListener("click", () => {
      resetState();
      applyAll();
    });
    row1.append(reset);
    bar.append(row1);

    // row2：版式主题（独立一行 —— 七个档位按钮各自成行，不再和布局/样式挤一条）
    const rowTheme = n("div", "k-row");
    rowTheme.append(n("span", "k-lbl", "版式主题"));
    const themeSeg = n("div", "seg");
    for (const [val, txt] of THEMES) {
      const b = n("button", "", txt);
      b.type = "button";
      b.dataset.val = val;
      b.title = val === "K"
        ? "不套用 A–J 版式：预演区用本页的引导器卡片（区域编排：启用/左右分栏/排序），仅此页有这种形态"
        : "引导菜单整体用方案 " + val + " 的版式；选好后列内排序照常生效"
          + (THEME_LAYOUT_MODES[val] && THEME_LAYOUT_MODES[val].join(",") === "wide"
            ? "（B / G / J 是加宽版面：窗口自动加宽到 932，布局只有「加宽」）"
            : "");
      b.addEventListener("click", () => { advState.theme = val; saveState(); applyAll(); });
      // 访客模式：主题由查看方（URL abTheme）决定，本页改了也不会生效——
      // 置灰说明，别让人点了以为「没保存」
      if (isGuestView()) {
        b.disabled = true;
      }
      themeSeg.append(b);
    }
    rowTheme.append(themeSeg,
      n("span", "hint", isGuestView()
        ? "主题跟随查看方（远程管理窗 / 手机 App）的选择显示，不改动这台电脑保存的主题。"
        : "主题只决定每张卡怎么画；B / G / J 是加宽版面，选中即把窗口加宽到 932。"));
    bar.append(rowTheme);

    // row3：布局（独立一行）
    const rowLayout = n("div", "k-row");
    rowLayout.append(n("span", "k-lbl", "布局"));
    const layoutSeg = n("div", "seg");
    const modes = layoutModes();
    for (const [val, txt] of [["default", "默认"], ["wide", "加宽"], ["lr", "左右布局"]]) {
      const b = n("button", "", txt);
      b.type = "button";
      b.dataset.val = val;
      if (!modes.includes(val)) {
        // 不摆假的：当前主题吃不到这一档就置灰说明，别让用户点了以为「没保存」
        b.disabled = true;
        b.title = layoutWideOnly()
          ? "主题 " + effectiveTheme() + " 是加宽版面（B / G / J 只能用「加宽」）：窗口固定按 932 落宽。先前选的档位仍会记着，换回「不套用主题」/ A / C 自动恢复"
          : "当前主题（" + effectiveTheme() + "）不支持左右布局；换「不套用主题」或 A / C 再选。已选的左右布局会记着，换回来自动生效";
      } else {
        b.title = val === "default"
          ? "默认：全部启用的引导器单列竖排（窗口 460 标称宽），用「上下排序」调先后"
          : val === "wide"
            ? "加宽：窗口加宽到两栏标称宽（932），引导列表仍是单列竖排（原 GRUB「加宽窗口展示」并入这里）"
            : "左右布局：窗口加宽 + 左右两列各自滚动，各列自定义放哪些引导器（仅「不套用主题」和 A / C 支持）";
      }
      b.addEventListener("click", () => {
        if (b.disabled) return;
        advState.layout = val;
        saveState();
        // 用户明确点了布局 = 明确要这个档位的标准宽度：判等不算数，强制发一次
        // （否则「上次就是默认 / 刚切回来」时窗口不动 —— 用户报的默认不缩回）
        layoutWidthSent = null;
        applyAll();
      });
      layoutSeg.append(b);
    }
    rowLayout.append(layoutSeg,
      n("span", "hint", "默认 = 单列竖排（460）；加宽 = 窗口 932、列表仍单列；左右布局 = 两列各自滚动，点标签上的「左/右」分列、拖分隔条定占比（中间圆钮或双击回正中）——仅「不套用主题」和 A / C；B / G / J 只有加宽，I 没有左右布局（置灰的档位选择仍会记着，换回支持的主题自动生效）。"));
    bar.append(rowLayout);

    // row4：菜单样式（独立一行）
    const rowStyle = n("div", "k-row");
    rowStyle.append(n("span", "k-lbl", "菜单样式"));
    const styleSeg = n("div", "seg");
    for (const [val, txt] of [["full", "完整"], ["compact", "紧凑"], ["min", "极简"]]) {
      const b = n("button", "", txt);
      b.type = "button";
      b.title = "完整：路径 + 徽章 + 全部动作；紧凑：行距减半、隐去路径；极简：只剩图标 + 名称 + 默认高亮";
      b.addEventListener("click", () => { advState.style = val; saveState(); applyAll(); });
      styleSeg.append(b);
    }
    rowStyle.append(styleSeg,
      n("span", "hint", "三档对任何版式主题都生效；行内动作按钮在紧凑/极简下悬停才出现。"));
    bar.append(rowStyle);

    // row5：排序条（设置动作收在设置区，预演区保持纯观看）
    const rowSort = n("div", "k-row");
    const sortWrap = n("span", "k-sortwrap");
    sortWrap.style.display = "contents";
    rowSort.append(sortWrap, n("span", "grow"),
      n("span", "hint", "预演与「系统引导」页同一份真实数据（只读示意）· 界面效果以「系统引导」页实际为准"));
    bar.append(rowSort);

    // 引导器设置折叠条
    const setBar = n("button", "k-setbar" + (setOpen ? " is-on" : ""));
    setBar.type = "button";
    setBar.title = "展开/收起 引导器设置（ESP 路径 / 权限 / 默认项 / 脚本 / 免密模式）";
    setBar.append(icon("gear", "sm"), n("span", "k-setbar-t", "引导器设置"),
      n("span", "hint", "ESP 路径 · 权限 · 默认项 · 脚本 · 免密模式"), n("span", "grow"),
      icon("chevron", "sm car"));
    setBar.addEventListener("click", () => { setOpen = !setOpen; applyAll(); });
    const setList = n("div", "k-setlist" + (setOpen ? " is-on" : ""));
    bar.append(setBar, setList);

    // stage
    const stage = n("div", "k-stage");
    const en = advState.order.map((id) => m[id]).filter((l) => l && advState.visible[l.id]);
    const splitH = effectiveLayout() === "lr";

    // row1…row4 的即时状态刷新（与设计稿 paint 一致）
    for (const id of LOADER_IDS) {
      tabs[id].classList.toggle("is-on", Boolean(advState.visible[id]));
      tabs[id].classList.toggle("is-off", !advState.visible[id]);
      const rg = tabs[id].querySelector(".k-rg");
      rg.textContent = advState.side[id] === 2 ? "右" : "左";
      // 主题不支持左右布局（B/G/I/J）时整颗不摆 —— 点了也没有任何可见效果，
      // 只会被当成「设置没保存」。其它主题常显（藏起来会让点击落到标签上、
      // 把引导器关掉，见上面的点击处理）。
      rg.style.display = THEME_H[effectiveTheme()] ? "" : "none";
      rg.style.opacity = lrOn ? "" : "0.55";
    }
    themeSeg.querySelectorAll("button").forEach((b) => b.classList.toggle("is-on", b.dataset.val === effectiveTheme()));
    // 布局档位按主题能力置灰（不隐藏：隐藏会让人以为「选项丢了」）；
    // 当前值若是不支持的档位，effectiveLayout() 已回落该主题的基准档
    layoutSeg.querySelectorAll("button").forEach((b) => {
      const on = modes.includes(b.dataset.val);
      b.style.display = "";
      b.disabled = !on;
      b.classList.toggle("is-on", b.dataset.val === effectiveLayout());
    });
    styleSeg.querySelectorAll("button").forEach((b, i) => {
      b.classList.toggle("is-on", ["full", "compact", "min"][i] === advState.style);
    });
    stage.classList.toggle("h", splitH);

    // 排序条：单列一组；左右布局按列各一组
    sortWrap.replaceChildren();
    if (splitH) {
      const L = en.filter((l) => advState.side[l.id] !== 2).map((l) => l.id);
      const R = en.filter((l) => advState.side[l.id] === 2).map((l) => l.id);
      if (L.length) sortWrap.append(sortStrip(L, "左列排序"));
      if (R.length) sortWrap.append(sortStrip(R, "右列排序"));
    } else if (en.length) {
      sortWrap.append(sortStrip(en.map((l) => l.id), "上下排序"));
    }
    // 引导器设置折叠条内容
    if (setOpen) {
      if (!en.length) {
        setList.append(n("div", "k-set-row", "没有启用的引导器 —— 点上方标签开启后再设置"));
      } else en.forEach((l) => {
        const r = n("div", "k-set-row");
        const def = l.nextBoot || l.entries.find((e) => e.def) || l.entries[0];
        r.append(ldr(l, "sm"), n("span", "k-set-nm", l.name),
          chip("off", "ESP · " + (l.esp || "未知")),
          l.perm === "nvram" ? chip("elev", "需提权") : chip("ok", "免提权"),
          chip("off", "默认项 · " + ((def && def.t) || "未知")),
          chip("off", "脚本 · " + (l.id === "grub" ? "09_anyboot" : "不适用")),
          chip("off", "免密 · " + (l.id === "opencore" ? "未开启" : "不适用")),
          n("span", "grow"),
          icoBtn("gear", "打开 " + l.name + " 详细设置"));
        setList.append(r);
      });
    }

    // 舞台：主题内容走 k-theme-fit；K 卡片走 k-zone-cards（设计稿 fillBody 同构）
    function fillBody(rbody, list, colName) {
      rbody.replaceChildren();
      if (!list.length) {
        rbody.classList.add("is-empty");
        const hint = n("div", "k-empty-hint");
        hint.append(icon("eye", "lg"), n("span", null, colName ? colName + "列是空的 —— 点标签上的「左/右」把引导器分进来" : "还没有启用的引导器 —— 点上方标签开启"));
        rbody.append(hint);
        return;
      }
      rbody.classList.remove("is-empty");
      rbody.classList.toggle("is-theme", effectiveTheme() !== "K");
      if (effectiveTheme() === "K") {
        const content = n("div", "k-zone-cards");
        list.forEach((l) => content.append(kLoaderCard(l)));
        rbody.append(content);
      } else {
        const renderer = THEME_RENDERERS[effectiveTheme()];
        const fit = n("div", "k-theme-fit");
        fit.append(renderer ? renderer(list) : n("div", "empty", n("b", null, `主题 ${effectiveTheme()} 还没实装`)));
        rbody.append(fit);
      }
    }

    function makeRegion(list, title, index) {
      const region = n("section", "k-region");
      if (splitH) {
        region.style.flex = "none";
        region.style.width = (index === 0 ? advState.ratio : 1 - advState.ratio) * 100 + "%";
      }
      const rhd = n("div", "k-rhd");
      rhd.append(n("b", null, title), n("span", "grow"));
      if (index === 0 && splitH) {
        rhd.append(zop("⇄", "交换左右两列", () => {
          for (const l of lastLoaders) advState.side[l.id] = advState.side[l.id] === 2 ? 1 : 2;
          saveState();
          applyAll();
        }));
      }
      rhd.append(n("span", "cnt hint", (splitH ? Math.round((index === 0 ? advState.ratio : 1 - advState.ratio) * 100) + "% · " : "") + list.length + " 个引导器"));
      region.append(rhd);
      const rbody = n("div", "k-rbody v");
      region.append(rbody);
      fillBody(rbody, list, splitH ? (index === 0 ? "左" : "右") : "");
      return region;
    }

    function makeSplit(firstRegion) {
      const split = n("div", "k-split");
      split.title = "拖动调整左右两列的大小（双击恢复对半分）";
      split.addEventListener("pointerdown", (e) => {
        e.preventDefault();
        split.classList.add("is-drag");
        split.setPointerCapture(e.pointerId);
        const onMove = (ev) => {
          const rect = stage.getBoundingClientRect();
          advState.ratio = Math.min(0.85, Math.max(0.15, (ev.clientX - rect.left) / rect.width));
          firstRegion.style.width = (advState.ratio * 100).toFixed(1) + "%";
        };
        const onUp = () => {
          split.classList.remove("is-drag");
          split.removeEventListener("pointermove", onMove);
          split.removeEventListener("pointerup", onUp);
          saveState();
          applyAll();
        };
        split.addEventListener("pointermove", onMove);
        split.addEventListener("pointerup", onUp);
      });
      split.addEventListener("dblclick", () => { advState.ratio = 0.5; saveState(); applyAll(); });
      split.append(splitResetButton(() => { advState.ratio = 0.5; saveState(); applyAll(); }));
      return split;
    }

    if (splitH) {
      const cols = [en.filter((l) => advState.side[l.id] !== 2), en.filter((l) => advState.side[l.id] === 2)];
      const r0 = makeRegion(cols[0], "左列", 0);
      stage.append(r0, makeSplit(r0), makeRegion(cols[1], "右列", 1));
    } else {
      stage.append(makeRegion(en, "预演 · 单列", 0));
    }

    wrap.append(bar, stage);
    root.replaceChildren(wrap);
    observeVw(wrap);
    // G 主题：恢复搜索词 + 焦点（必须在挂进文档之后）
    if (prevG.value) {
      const inp = wrap.querySelector(".lg-search input");
      if (inp) {
        inp.value = prevG.value;
        inp.dispatchEvent(new Event("input"));
        if (prevG.focus) {
          inp.focus();
          try { inp.setSelectionRange(inp.value.length, inp.value.length); } catch (e) { void e; }
        }
      }
    }
  }

  // ═══════════════════════════════════════════════════════════════════
  // 版式主题渲染器（设计稿 viewA/B/C/G/I/J 原样；数据从适配层来）
  // 预演喂设计稿自己的卡片/条目；真实页喂真实区块（bootBuild）。
  // ═══════════════════════════════════════════════════════════════════
  const themeSel = {}; // 各主题的当前选中引导器（内存态）

  // ── A：卡片流 + 顶部通道索引（设计稿 cardA/viewA 原样）─────────────────
  function cardA(l) {
    const card = n("section", "card la-card");
    const head = n("div", "card-head");
    head.append(ldr(l));
    const title = n("div", "card-title");
    title.append(n("b", null, l.name), n("small", null, loaderMeta(l)));
    head.append(title);
    head.append(stateChip(l));
    const acts = n("div", "row-acts");
    acts.style.opacity = "1";
    acts.append(icoBtn("more", "更多：隐藏通道 / 重新识别 / 复制路径"));
    head.append(acts);
    card.append(head);

    const entries = n("div", "entries");
    l.entries.forEach((e) => entries.append(entryRow(e, l)));
    card.append(entries);

    const acc = settingsAccordion(l);
    card.append(acc.bar, acc.panel);

    const foot = n("div", "la-foot");
    foot.append(icon("info", "sm"), n("span", null, l.healthText || ""));
    card.append(foot);
    return card;
  }
  THEME_RENDERERS.A = (lds, build, opts) => {
    opts = opts || {};
    const buildCard = build || cardA;
    const wrap = n("div", "la");
    const idx = n("div", "la-index");
    for (const l of lds) {
      const c = n("button", "la-chip" + (l.id === (lds[0] && lds[0].id) ? " is-on" : ""));
      c.type = "button";
      c.append(dot(stateOf(l)), n("span", "nm", l.name), n("em", null, String(l.entries.length)));
      c.addEventListener("click", () => {
        idx.querySelectorAll(".la-chip").forEach((x) => x.classList.remove("is-on"));
        c.classList.add("is-on");
        const target = items.get(l.id);
        if (target) {
          target.scrollIntoView({ behavior: "smooth", block: "start" });
          target.classList.remove("ba-flash");
          void target.offsetWidth;
          target.classList.add("ba-flash");
        }
      });
      idx.append(c);
    }
    const spacer = n("span");
    spacer.style.flex = "1";
    idx.append(spacer, icoBtn("sort", "拖拽排序 / 编排显示"));
    wrap.append(idx);

    const list = n("div", "la-list");
    const items = new Map();
    lds.forEach((l) => {
      const content = buildCard(l);
      if (content) { items.set(l.id, content); list.append(content); }
    });
    wrap.append(list);
    return wrap;
  };

  // ── B：左栏通道 + 右栏详情（设计稿 viewB 原样）─────────────────────────
  THEME_RENDERERS.B = (lds, build, opts) => {
    opts = opts || {};
    const buildPane = build || ((l) => {
      const frag = n("div");
      const block = n("div", "lb-block");
      const bh = n("div", "lb-block-head");
      bh.append(n("b", null, "启动项"), n("span", null, String(l.entries.length)), n("span", "grow"));
      block.append(bh);
      const body = n("div", "lb-block-body");
      l.entries.forEach((e) => body.append(entryRow(e, l)));
      block.append(body);
      frag.append(block);
      const setBlock = n("div", "lb-block");
      const sh = n("button", "lb-block-head");
      sh.type = "button";
      sh.setAttribute("aria-expanded", "false");
      sh.append(n("b", null, "引导器设置"), n("span", null, "ESP、权限、默认项、脚本"), n("span", "grow"), icon("chevron", "sm car"));
      const sb = n("div", "lb-block-body");
      sb.style.padding = "0";
      sb.hidden = true;
      sb.append(settingsPanel(l));
      sh.addEventListener("click", () => {
        const open = sh.getAttribute("aria-expanded") === "true";
        sh.setAttribute("aria-expanded", String(!open));
        sb.hidden = open;
      });
      setBlock.append(sh, sb);
      frag.append(setBlock);
      return frag;
    });
    const wrap = n("div", "lb");
    const nav = n("aside", "lb-nav");
    const navHead = n("div", "lb-nav-head");
    navHead.append(n("span", "grow", "引导器 " + lds.length), icoBtn("plus", "添加 / 重新识别通道"));
    nav.append(navHead);

    const list = n("div", "lb-list");
    // 左右布局下每个渲染器只拿到**自己那一列**的引导器：记忆里的 active
    // 可能不在这一列（themeSel 是全局的），直接用会取到 undefined →
    // 读 .entries 当场抛错，整页布置中断（「左右布局点了没变化」的一个真凶）。
    let active = (opts.active && lds.some((x) => x.id === opts.active))
      ? opts.active
      : (lds[0] && lds[0].id);
    const mainName = n("h2");
    const mainMeta = n("p");
    const scroll = n("div", "lb-scroll");
    const panes = new Map();

    function paint(id) {
      active = id;
      const i = lds.findIndex((x) => x.id === id);
      const l = lds[i];
      if (!l) return; // 记忆里的 id 不在这批（左右布局分列）时别硬读
      [...list.children].forEach((c, k) => c.classList.toggle("is-on", lds[k] && lds[k].id === id));
      mainName.replaceChildren(ldr(l, "md"), document.createTextNode(l.name), stateChip(l));
      mainMeta.textContent = loaderMeta(l);
      panes.forEach((pane, pid) => pane.classList.toggle("is-shown", pid === id));
      if (opts.onSelect) opts.onSelect(id);
    }

    lds.forEach((l) => {
      const item = n("button", "lb-item");
      item.type = "button";
      item.append(icon("grip", "sm lb-grip"), ldr(l, "sm"));
      const nm = n("span", "nm");
      nm.append(n("b", null, l.name), n("small", null, `${l.entries.length} 个条目 · ${l.perm === "nvram" ? "需提权" : "免提权"}`));
      item.append(nm, dot(stateOf(l)));
      item.addEventListener("click", () => paint(l.id));
      list.append(item);
      const pane = buildPane(l);
      if (pane) {
        pane.classList.add("ba-theme-pane");
        panes.set(l.id, pane);
      }
    });
    nav.append(list);
    const foot = n("div", "lb-nav-foot");
    const reorderBtn = btn("编排与排序", "btn sm ghost", "sliders");
    foot.append(reorderBtn, btn("", "btn sm ghost", "plus"));
    nav.append(foot);

    const main = n("section", "lb-main");
    const head = n("div", "lb-main-head");
    const tt = n("div", "tt");
    tt.append(mainName, mainMeta);
    head.append(tt, btn("下次启动", "btn sm", "power"), btn("", "btn sm ghost", "more"));
    main.append(head, scroll);
    wrap.append(nav, main);
    scroll.append(...panes.values());
    paint(active);
    return wrap;
  };

  // ── C：顶部固定标签页（设计稿 viewC 原样）──────────────────────────────
  THEME_RENDERERS.C = (lds, build, opts) => {
    opts = opts || {};
    const buildPane = build || ((l) => {
      const frag = n("div");
      const list = n("div", "entries");
      l.entries.forEach((e) => list.append(entryRow(e, l)));
      frag.append(list);
      const health = n("div", "la-foot");
      health.style.borderTop = "1px solid var(--line)";
      health.style.marginTop = "8px";
      health.style.background = "transparent";
      health.append(icon("info", "sm"), n("span", null, l.healthText || ""));
      frag.append(health);
      return frag;
    });
    const wrap = n("div", "lc");
    const tabs = n("div", "lc-tabs");
    const bodyWrap = n("div", "lc-body");
    const toolbar = n("div", "lc-toolbar");
    // 左右布局下每个渲染器只拿到**自己那一列**的引导器：记忆里的 active
    // 可能不在这一列（themeSel 是全局的），直接用会取到 undefined →
    // 读 .entries 当场抛错，整页布置中断（「左右布局点了没变化」的一个真凶）。
    let active = (opts.active && lds.some((x) => x.id === opts.active))
      ? opts.active
      : (lds[0] && lds[0].id);
    const panes = new Map();

    const tabEls = lds.map((l) => {
      const t = n("button", "lc-tab");
      t.type = "button";
      t.append(dot(stateOf(l)), n("span", null, l.name), n("em", null, String(l.entries.length)));
      t.addEventListener("click", () => paint(l.id));
      tabs.append(t);
      return t;
    });
    const add = icoBtn("sliders", "编排：固定 / 排序 / 溢出顺序", "btn icon ghost sm lc-tabadd");
    const moreT = icoBtn("more", "溢出标签", "btn icon ghost sm lc-tabadd");
    tabs.append(add, moreT);

    const meta = n("div", "lc-meta");
    const density = n("div", "seg");
    [["full", "舒适"], ["compact", "紧凑"]].forEach(([val, txt]) => {
      const b = n("button", advState.style === val ? "is-on" : "", txt);
      b.type = "button";
      b.addEventListener("click", () => { advState.style = val; saveState(); applyAll(); });
      density.append(b);
    });
    const spacer = n("span");
    spacer.className = "grow";
    toolbar.append(meta, spacer, n("span", "mut", "密度"), density,
      icoBtn("search", "搜索条目"), icoBtn("gear", "引导器设置"));

    function paint(id) {
      active = id;
      const i = lds.findIndex((x) => x.id === id);
      const l = lds[i];
      if (!l) return; // 记忆里的 id 不在这批（左右布局分列）时别硬读
      tabEls.forEach((t, k) => t.classList.toggle("is-on", lds[k] && lds[k].id === id));
      const def = l.nextBoot || l.entries[0];
      meta.replaceChildren(icon("clock", "sm"), n("span", null, "当前默认 "), n("b", null, (def && def.t) || "未知"), n("span", "mut", "· " + (l.esp || "")));
      panes.forEach((pane, pid) => pane.classList.toggle("is-shown", pid === id));
      if (opts.onSelect) opts.onSelect(id);
    }

    lds.forEach((l) => {
      const pane = buildPane(l);
      if (pane) {
        pane.classList.add("ba-theme-pane");
        panes.set(l.id, pane);
      }
    });

    const settingsBar = n("div", "lc-settings");
    const accBar = n("button", "acc");
    const accPanel = n("div", "panel-in");
    accPanel.hidden = true;
    accBar.type = "button";
    accBar.setAttribute("aria-expanded", "false");
    const g = n("span", "grow");
    g.append(n("span", null, "引导器设置"), n("small", null, "  ESP 路径 / 权限 / 默认项 / 脚本 / 免密"));
    accBar.append(icon("gear", "sm"), g, icon("chevron", "sm car"));
    accBar.addEventListener("click", () => {
      const open = accBar.getAttribute("aria-expanded") === "true";
      accBar.setAttribute("aria-expanded", String(!open));
      accPanel.hidden = open;
      accPanel.replaceChildren();
      if (!open) {
        const l = lds.find((x) => x.id === active) || lds[0];
        if (l) {
          const p = settingsPanel(l, 210);
          p.style.borderTop = "1px solid var(--line)";
          accPanel.append(...p.childNodes);
        }
      }
    });
    settingsBar.append(accBar, accPanel);
    // 真实页（build 传入）不要再挂这条：搬进来的每张卡自己已经有一条
    // 「引导器设置」（splitLoaderSection 的 .ba-setbar，展开是真实控件），
    // 这里再挂一条就成了同一个标题上下两次，而且这条展开的只是设计稿示意
    // 面板（settingsPanel 里的「改/提升/编辑」都是死按钮）。
    wrap.append(tabs, toolbar, bodyWrap);
    if (!build) wrap.append(settingsBar);
    bodyWrap.append(...panes.values());
    paint(active);
    return wrap;
  };

  // ── G：命令面板（设计稿 viewG 原样；真实页模式过滤真实条目行）──────────
  const G_ROW_SEL = ".boot-entry, .oc-menu-row, .grub-entry, .row";
  // 选中项在快照重排（5s 一拍，整个主题 DOM 重建）之后要能找回来：只记
  // 「哪个引导器 + 哪一行」，重建后按同样的键再定位一次。
  let gSelMemo = null;
  THEME_RENDERERS.G = (lds, build, opts) => {
    opts = opts || {};
    const wrap = n("div", "lg");
    const main = n("div", "lg-main");
    const top = n("div", "lg-top");

    const searchBox = n("div", "lg-search");
    searchBox.append(icon("search", "lg"));
    const input = n("input");
    input.placeholder = "搜索系统、引导器或动作…";
    input.value = "";
    searchBox.append(input, n("span", "kbd", "⌘K"));
    top.append(searchBox);

    const filters = n("div", "lg-filters");
    const fdefs = [{ id: "all", label: "全部 " + lds.reduce((s, l) => s + l.entries.length, 0) }]
      .concat(lds.map((l) => ({ id: l.id, label: l.name + " " + l.entries.length })));
    let filter = "all";
    const fchips = fdefs.map((f) => {
      const c = n("button", "lg-fchip" + (f.id === "all" ? " is-on" : ""));
      c.type = "button";
      c.textContent = f.label;
      c.addEventListener("click", () => {
        filter = f.id;
        // 真实页模式走 _gPaint（本作用域看不到块内那两个函数，见下方的说明）；
        // 预演模式用同作用域的 paintResults（函数声明，已提升）
        if (wrap._gPaint) wrap._gPaint();
        else paintResults();
      });
      filters.append(c);
      return c;
    });
    top.append(filters);
    main.append(top);

    if (build) {
      // 真实页模式：**1:1 设计稿 viewG** —— 真实区块只当「分组」，区块自己的
      // 标题行由这里换成 .lg-group（字章 + 名称 · N 项）；右侧 .lg-side 是
      // 选中项预览（路径 / 状态 / 两个动作），底部 .lg-foot 是快捷键提示。
      // 条目行仍是真实行（监听都在），只把样式压成设计稿 .lg-item（见样式表）。
      const results = n("div", "lg-results");
      const side = n("aside", "lg-side");
      const cards = new Map(); // loader id -> { group, card, loader, count }
      let selected = null; // { l, card, row }

      // 可显示的条目：排除分组标题行、排除藏在 [hidden] 面板里的另一标签页
      // （GRUB 的「手动解析」、rEFInd 的「手动处理」），否则数字和列表对不上
      const topRows = (card) =>
        [...card.querySelectorAll(G_ROW_SEL)]
          .filter((r) => !r.classList.contains("is-group") && !r.closest("[hidden]"));

      const rowTitle = (row) => {
        const nm = row.querySelector(".grub-entry-name, .boot-entry-name");
        if (nm) return (nm.textContent || "").trim();
        const b = row.querySelector(".oc-menu-head b");
        if (b) return (b.textContent || "").trim();
        return (row.textContent || "").replace(/\s+/g, " ").trim().slice(0, 90);
      };
      const rowSub = (row) => {
        const p = row.querySelector(".oc-menu-path");
        if (p) return (p.textContent || "").trim();
        // GRUB / rEFInd 行没有独立副标题节点：title 的第一行通常是「卷标 · 路径」
        const t = (row.getAttribute("title") || "")
          .split("\n").map((x) => x.trim()).filter(Boolean);
        return t.length > 1 ? t[0] : "";
      };
      const rowIsDef = (row) =>
        row.classList.contains("active") ||
        row.classList.contains("is-def") ||
        row.classList.contains("current");

      for (const l of lds) {
        const c = build(l);
        if (!c) continue;
        c.classList.add("ba-g-card");
        const count = topRows(c).length;
        const g = n("div", "lg-group");
        g.append(ldr(l, "sm"), n("span", null, l.name + " · " + count + " 项"));
        // 这一组一条都读不到（GRUB 的自动读取还没跑过等）：明说一句，
        // 否则用户只看到标题下一片空白，以为主题又坏了
        if (!count) g.append(n("span", "lg-hint", "暂无条目 · 展开下面的「引导器设置」读取"));
        // 设计稿的每行是两行（名称 + mono 路径）。真实行只有名称 —— 有真路径
        // 的（rEFInd 行 title 的首行）就往名称里补一条副标题，GRUB 的 menuentry
        // 本来就没有路径，不硬编。
        for (const row of topRows(c)) {
          const sub = rowSub(row);
          if (!sub || row.querySelector(".oc-menu-path")) continue;
          const nm = row.querySelector(".grub-entry-name, .boot-entry-name");
          if (!nm || nm.querySelector(".ba-g-sub")) continue;
          nm.append(n("small", "ba-g-sub", sub));
        }
        results.append(g, c);
        cards.set(l.id, { group: g, card: c, loader: l, count });
      }

      // 芯片上的数字改成**真实行数**（lds 里的 entries 在真实页可能是空的：
      // GRUB 的条目来自 ESP 上的 grub.cfg，跟 Loaders 结构里的 entries 不同源，
      // 不改的话会出现「GRUB 0」但下面列着几十行）
      let total = 0;
      for (const [, entry] of cards) total += entry.count;
      fchips.forEach((c, i) => {
        const id = fdefs[i].id;
        if (id === "all") c.textContent = "全部 " + total;
        else if (cards.has(id)) c.textContent = cards.get(id).loader.name + " " + cards.get(id).count;
      });

      function visibleRows(card) {
        return [...card.querySelectorAll(G_ROW_SEL)]
          .filter((r) => !r.classList.contains("ba-g-hide")
            && !r.classList.contains("is-group")
            && !r.closest("[hidden]"));
      }

      function markSelected(row) {
        for (const [, entry] of cards) {
          for (const r of entry.card.querySelectorAll(G_ROW_SEL)) r.classList.remove("is-active");
        }
        if (row) row.classList.add("is-active");
      }

      function select(l, card, row, keepScroll) {
        selected = { l, card, row };
        gSelMemo = { id: l.id, title: rowTitle(row) };
        markSelected(row);
        paintSide();
        if (!keepScroll) row.scrollIntoView({ block: "nearest" });
      }

      function paintSide() {
        side.replaceChildren();
        if (!selected) {
          const e = n("div", "empty");
          e.append(n("b", null, "未选中"), n("span", null, "点左侧任意条目，这里显示它的路径与动作"));
          side.append(e);
          return;
        }
        const { l, row } = selected;
        side.append(n("h4", null, "选中项"));
        const card = n("div", "lg-sel");
        const bh = n("div", "lg-sel-head");
        const ico = row.querySelector(".row-ico");
        if (ico) {
          // 整颗克隆（含系统 logo 的 <svg>，textContent 已经是空的了）
          bh.append(ico.cloneNode(true));
        } else {
          bh.append(ldr(l, "sm"));
        }
        bh.append(n("b", null, rowTitle(row)));
        card.append(bh);
        const bd = n("div", "lg-sel-body");
        [["引导器", l.name], ["路径", rowSub(row) || "—"], ["状态", rowIsDef(row) ? "当前默认" : "可用"]]
          .forEach(([k, v]) => {
            const f = n("div", "lg-fact");
            f.append(n("b", null, k), n("span", null, String(v)));
            bd.append(f);
          });
        card.append(bd);
        side.append(card);

        // 动作：都是**代理真实行自己的动作**（不另写一套动作逻辑）
        // 用户 2026-10-30：OC 行撤掉行尾箭头后，「点这一行」就是「设为默认」，
        // 所以 OC 行上的「下次启动」按钮没有对应动作（别让它变成第二个设默认）。
        const isOcRow = row.classList.contains("oc-menu-row");
        const acts = n("div", "toolbar");
        const go = btn("下次启动", "btn sm primary", "power");
        if (isOcRow) {
          go.disabled = true;
          go.title = "OC 预演行没有「下次启动」这个动作：点这一行 = 设为默认";
        } else {
          go.addEventListener("click", () => row.click());
        }
        const def = btn("设为默认", "btn sm", "elev");
        const setBtn = row.querySelector(
          ".oc-menu-set, button[title*='设为默认'], button[aria-label*='设为默认']"
        );
        if (setBtn) def.addEventListener("click", () => setBtn.click());
        else if (isOcRow && row.classList.contains("is-clickable")) {
          def.addEventListener("click", () => row.click());
        } else if (isOcRow) {
          def.disabled = true;
          def.title = "这一行就是当前默认项";
        } else {
          def.disabled = true;
          def.title = "这一行没有「设为默认」的动作：点条目本身就是把它设为下次启动";
        }
        acts.append(go, def);
        side.append(acts);
        side.append(n("div", "hint",
          "点条目 = 直接执行该引导器自己的动作（下次启动 / 进子菜单）；↑↓ 选行，↵ 执行，esc 清空搜索。"));

        const sibs = [...selected.card.querySelectorAll(G_ROW_SEL)]
          .filter((r) => r !== row && !r.classList.contains("is-group"))
          .slice(0, 6);
        if (sibs.length) {
          side.append(n("h4", null, "同引导器"));
          for (const r of sibs) {
            const b = n("button", "lg-item");
            b.type = "button";
            const rico = r.querySelector(".row-ico");
            if (rico) b.append(rico.cloneNode(true));
            b.append(n("span", "t"));
            b.lastChild.append(n("b", null, rowTitle(r)));
            b.title = rowSub(r) || "";
            b.addEventListener("click", () => select(selected.l, selected.card, r));
            side.append(b);
          }
        }
      }

      function paintFilter() {
        fchips.forEach((c, i) => c.classList.toggle("is-on", fdefs[i].id === filter));
      }
      function paintResults() {
        const q = input.value.trim().toLowerCase();
        for (const [id, entry] of cards) {
          const show = filter === "all" || filter === id;
          entry.group.style.display = show ? "" : "none";
          entry.card.style.display = show ? "" : "none";
          if (!show) continue;
          let hits = 0;
          for (const row of entry.card.querySelectorAll(G_ROW_SEL)) {
            const ok = !q || (row.textContent || "").toLowerCase().includes(q);
            row.classList.toggle("ba-g-hide", !ok);
            if (ok) hits++;
          }
          if (q && !hits) {
            entry.group.style.display = "none";
            entry.card.style.display = "none";
          }
        }
        // 选中项被筛掉时清空右侧（否则预览的是看不见的行）
        if (selected && selected.row.classList.contains("ba-g-hide")) {
          selected = null;
          markSelected(null);
          paintSide();
        }
      }

      // 点真实行：捕获阶段记录选中（真实行的监听可能 stopPropagation）
      results.addEventListener("click", (ev) => {
        const row = ev.target.closest && ev.target.closest(G_ROW_SEL);
        if (!row || !results.contains(row)) return;
        for (const [, entry] of cards) {
          if (entry.card.contains(row)) { select(entry.loader, entry.card, row, true); return; }
        }
      }, true);

      input.addEventListener("input", () => { selected = null; markSelected(null); paintResults(); paintSide(); });
      input.addEventListener("keydown", (e) => {
        if (e.key === "Escape") {
          input.value = "";
          selected = null;
          markSelected(null);
          paintResults();
          paintSide();
          return;
        }
        if (e.key !== "ArrowDown" && e.key !== "ArrowUp" && e.key !== "Enter") return;
        const pool = [];
        for (const [, entry] of cards) {
          if (entry.card.style.display === "none") continue;
          pool.push(...visibleRows(entry.card).map((row) => ({ entry, row })));
        }
        if (!pool.length) return;
        e.preventDefault();
        if (e.key === "Enter") {
          (selected ? selected.row : pool[0].row).click();
          return;
        }
        const at = selected ? pool.findIndex((x) => x.row === selected.row) : -1;
        const next = e.key === "ArrowDown"
          ? Math.min(pool.length - 1, at + 1)
          : Math.max(0, (at < 0 ? 0 : at) - 1);
        select(pool[next].entry.loader, pool[next].entry.card, pool[next].row);
      });

      main.append(results);

      const foot = n("div", "lg-foot");
      [["↑ ↓", "移动"], ["↵", "下次启动"], ["esc", "清空搜索"], ["⌘K", "聚焦搜索"]]
        .forEach((x) => {
          const s = n("span");
          s.append(n("span", "kbd", x[0]), document.createTextNode(x[1]));
          foot.append(s);
        });
      main.append(foot);

      wrap.append(main, side);
      // 高度不再用 JS 量着算：真实页现在有 fill 链（#tab-boot.ba-fill 一路
      // flex 到主题容器），预演区也有 max-height + flex，.lg 作为 flex 项本来
      // 就吃满剩余高度，内部 .lg-results 自己滚。原先那套「量顶边 + innerHeight
      // 算一个固定 px」在 fill 之后反而多出 340px 的下限，把面板顶出容器 60px。
      // 底部写着 ⌘K 聚焦搜索，就得真的能用（Ctrl+K 同效，Windows 上没有 ⌘）
      const onCmdK = (e) => {
        if (!wrap.isConnected) { window.removeEventListener("keydown", onCmdK); return; }
        if ((e.metaKey || e.ctrlKey) && (e.key === "k" || e.key === "K")) {
          e.preventDefault();
          input.focus();
          input.select();
        }
      };
      window.addEventListener("keydown", onCmdK);
      // 筛选芯片的 click 处理挂在**外面**的共同作用域（fchips 建得比这里早），
      // 那里看不见本块里的 paintFilter / paintResults —— 原先点芯片直接
      // 「paintFilter is not defined」，G 主题的引导器筛选完全点不动。
      // 统一由 _gPaint 回调进来。
      wrap._gPaint = () => { paintFilter(); paintResults(); };
      // 首拍必须刷一次：cards 刚被搬进来时可能还带着上个主题的 display:none
      paintFilter();
      paintResults();
      // 恢复上次选中（5s 一拍整块重建后不至于每次跳回第一条）／没有就选第一条：
      // 设计稿默认 selected=0，右侧永远有一份预览，不是空窗。
      let restored = false;
      if (gSelMemo) {
        const entry = cards.get(gSelMemo.id);
        const row = entry && visibleRows(entry.card).find((r) => rowTitle(r) === gSelMemo.title);
        if (row) { select(entry.loader, entry.card, row, true); restored = true; }
      }
      if (!restored) {
        for (const [, entry] of cards) {
          const rows = visibleRows(entry.card);
          if (rows.length) { select(entry.loader, entry.card, rows[0], true); break; }
        }
      }
      if (!selected) paintSide();
      return wrap;
    }

    // 预演模式：设计稿完整的命令面板（分组结果 + 右侧详情）
    const results = n("div", "lg-results");
    const side = n("aside", "lg-side");
    let selected = 0;

    function items() {
      const q = input.value.trim().toLowerCase();
      const out = [];
      lds.forEach((l) => {
        if (filter !== "all" && l.id !== filter) return;
        l.entries.forEach((e) => {
          if (q && (e.t + " " + l.name + " " + (e.detail || "")).toLowerCase().indexOf(q) < 0) return;
          out.push({ l, e });
        });
      });
      return out;
    }

    function paintSide(it) {
      side.replaceChildren();
      if (!it) {
        const se = n("div", "empty");
        se.append(n("b", null, "未选中"), n("span", null, "左侧没有可预览的条目"));
        side.append(se);
        return;
      }
      const h = n("h4", null, "选中项");
      side.append(h);
      const card = n("div", "lb-block");
      const bh = n("div", "lb-block-head");
      bh.append(ldr(it.l, "sm"), n("b", null, it.e.t));
      card.append(bh);
      const bd = n("div", "lb-block-body");
      bd.style.padding = "6px 10px";
      [["引导器", it.l.name], ["来源", it.e.src || "—"], ["路径", it.e.detail || "—"], ["状态", it.e.def ? "当前默认" : "可用"]].forEach((x) => {
        const f = n("div", "set");
        const t = n("div", "set-t");
        t.append(n("b", null, x[0]), n("small", null, String(x[1])));
        f.append(t);
        bd.append(f);
      });
      card.append(bd);
      side.append(card);
      const acts = n("div", "toolbar");
      acts.append(btn("下次启动", "btn sm primary", "power"), btn("设为默认", "btn sm", "elev"));
      side.append(acts);
      side.append(n("div", "hint", "回车 = 下次启动；⌘/Ctrl + 回车 = 设为默认（需要提权时会先弹说明）。"));
      side.append(n("h4", null, "同引导器"));
      it.l.entries.filter((e) => e !== it.e).forEach((e) => {
        const b = n("button", "lg-item");
        b.type = "button";
        b.append(osIco(e.os), n("span", "t"));
        b.lastChild.append(n("b", null, e.t));
        if (e.def) b.append(badge("def", "默认"));
        side.append(b);
      });
    }

    function paintResults() {
      fchips.forEach((c, i) => c.classList.toggle("is-on", fdefs[i].id === filter));
      const list = items();
      results.replaceChildren();
      let lastLoader = null;
      list.forEach((it, i) => {
        if (it.l !== lastLoader) {
          const g = n("div", "lg-group");
          g.append(ldr(it.l, "sm"), document.createTextNode(it.l.name + " · " + it.l.entries.length + " 项"));
          results.append(g);
          lastLoader = it.l;
        }
        const b = n("button", "lg-item" + (i === selected ? " is-active" : ""));
        b.type = "button";
        b.append(osIco(it.e.os));
        const t = n("span", "t");
        t.append(n("b", null, it.e.t), n("small", null, it.e.detail || ""));
        b.append(t);
        if (it.e.def) b.append(badge("def", "默认"));
        b.append(n("span", "kbd", "↵"));
        b.addEventListener("click", () => { selected = i; paintResults(); });
        results.append(b);
      });
      if (!list.length) {
        const e = n("div", "empty");
        e.append(n("b", null, "没有匹配项"), n("span", null, "试试「windows」「arch」或清掉筛选"));
        results.append(e);
        side.replaceChildren();
        return;
      }
      paintSide(list[selected] || null);
    }
    input.addEventListener("input", () => { selected = 0; paintResults(); });
    main.append(results);

    const foot = n("div", "lg-foot");
    [["↑ ↓", "移动"], ["↵", "下次启动"], ["⌘↵", "设为默认"], ["⌘,", "设置"], ["esc", "清空筛选"]].forEach((x) => {
      const s = n("span");
      s.append(n("span", "kbd", x[0]), document.createTextNode(x[1]));
      foot.append(s);
    });
    main.append(foot);
    wrap.append(main, side);
    paintResults();
    return wrap;
  };

  // ── I：Bento 磁贴 + 底部抽屉（设计稿 viewI 原样）───────────────────────
  let sheetEsc = null;
  let sheetLid = null;
  let sheetPage = "编排";
  let iSheetOpen = null; // I 渲染器的 open()（快照重排后凭 sheetLid 重开）
  let iSheetClose = null; // I 渲染器的 close()（重建后清掉旧 sheet 的 Esc 监听）
  function closeSheet() {
    if (iSheetClose) { iSheetClose(); return; }
    document.querySelectorAll(".li-sheet").forEach((s) => s.classList.remove("is-on"));
    if (sheetEsc) { document.removeEventListener("keydown", sheetEsc); sheetEsc = null; }
    sheetLid = null;
  }
  THEME_RENDERERS.I = (lds, build, opts) => {
    opts = opts || {};
    const wrap = n("div", "li");
    const grid = n("div", "li-grid");

    const first = lds[0];
    const firstDef = first && (first.entries.find((e) => e.def) || first.entries[0]);
    const hero = n("div", "li-hero");
    const htop = n("div");
    htop.append(n("div", "k", "下次启动 · 一次性"));
    htop.append(n("div", "v", (firstDef && firstDef.t) || "—"));
    htop.append(n("div", "s", first ? `${first.name} → ${first.esp || "ESP"}` : "没有启用的引导器"));
    const acts = n("div", "acts");
    acts.append(btn("取消一次性启动", "btn sm"), btn("换成默认", "btn sm ghost"));
    htop.append(acts);
    hero.append(htop);
    const hfoot = n("div", "li-tile");
    hfoot.style.background = "transparent";
    hfoot.style.border = "0";
    hfoot.style.padding = "8px 0 0";
    hfoot.append(n("div", "s", "BootNext 用后自动失效；不改固件顺序"));
    hero.append(hfoot);
    grid.append(hero);

    lds.forEach((l) => {
      const tile = n("div", "li-tile" + (build ? " is-clickable" : ""));
      // 真实页：整块磁贴都能点开抽屉里那个引导器（原先只有右下角那枚小箭头有
      // 监听，「点了没反应」等于 I 主题看不到任何一个引导器的内容）。
      // 设置页的小预演不接这个监听 —— 那是缩略图，点开抽屉会盖住预览卡。
      if (build) {
        tile.addEventListener("click", () => {
          if (wrap._iBuild) wrap._iBuild(l); // 先把该引导器区块搬进抽屉
          open("引导器", l.id);
        });
      }
      const top = n("div", "top");
      top.append(ldr(l, "sm"), n("span", "nm", l.name), dot(stateOf(l)));
      tile.append(top);
      const big = n("div", "big", String(l.entries.length));
      big.append(n("small", null, "项"));
      tile.append(big);
      const d = l.nextBoot || l.entries[0];
      tile.append(n("div", "s", "默认 · " + ((d && d.t) || "—")));
      const foot = n("div", "foot");
      foot.append(chip(l.perm === "nvram" ? "elev" : "off", l.perm === "nvram" ? "需提权" : "免提权"));
      foot.append(n("span", "grow"));
      const go = icoBtn("arrow", "查看 " + l.name + "（打开底部抽屉并定位）");
      go.addEventListener("click", (ev) => {
        ev.stopPropagation(); // 磁贴自己已经能点开，别重复触发
        if (wrap._iBuild) wrap._iBuild(l); // 真实页：先把该引导器区块搬进抽屉
        open("引导器", l.id);
      });
      foot.append(go);
      tile.append(foot);
      grid.append(tile);
    });
    wrap.append(grid);

    const bar = n("div", "li-sheetbar");
    bar.append(icon("sliders", "sm"), n("span", "grow", "设置与编排：固定 / 排序 / 收起通道 / ESP 路径 / 免密模式"));
    bar.append(chip("off", lds.length + " 个引导器"));
    const openBtn = btn("打开抽屉", "btn sm", "chevron");
    bar.append(openBtn);
    wrap.append(bar);

    // 底部抽屉：遮罩 + 半高面板（编排 / 引导器 两页签 + 真实区块区）
    const sheet = n("div", "li-sheet");
    const scrim = n("div", "li-scrim");
    const panel = n("div", "li-panel");
    const phd = n("div", "li-panel-hd");
    phd.append(icon("sliders", "sm"), n("b", null, "设置与编排"));
    const seg = n("div", "seg");
    const bd = n("div", "li-panel-bd");
    const realSec = n("div", "ba-i-realsec");
    // 预演模式没有真实区块，这块不占位；真实页正是靠它把引导器内容搬进抽屉，
    // 再 display:none 就等于「抽屉打开了、里面空空如也」（I 主题看不到任何引导器）
    if (!build) realSec.style.display = "none";
    const PAGES = {
      "编排": () => {
        lds.forEach((l) => {
          const r = n("div", "li-prow");
          r.append(n("span", "grab", "⠿"), ldr(l, "sm"), n("span", "nm", l.name), n("span", "grow"),
            chip("off", l.entries.length + " 条"));
          bd.append(r);
        });
      },
      "引导器": () => {
        lds.forEach((l) => {
          const r = n("div", "li-prow");
          r.dataset.lid = l.id;
          r.append(ldr(l, "sm"), n("span", "nm", l.name), n("span", "grow"),
            chip("off", l.esp || "ESP 未知"),
            chip(l.perm === "nvram" ? "elev" : "ok", l.perm === "nvram" ? "需提权" : "免密"));
          bd.append(r);
        });
        if (realSec.childNodes.length) {
          const sep = n("div", "sep");
          sep.style.margin = "6px 0";
          bd.append(sep, realSec);
        }
      },
    };
    let cur = sheetPage;
    function paintPanel() {
      seg.querySelectorAll("button").forEach((b) => b.classList.toggle("is-on", b.textContent === cur));
      sheetPage = cur;
      bd.replaceChildren();
      PAGES[cur]();
    }
    Object.keys(PAGES).forEach((k) => {
      const b = n("button", "", k);
      b.type = "button";
      b.addEventListener("click", () => { cur = k; paintPanel(); });
      seg.append(b);
    });
    phd.append(seg, n("span", "grow"));
    const x = icoBtn("x", "关闭抽屉");
    phd.append(x);
    panel.append(phd, bd);
    sheet.append(scrim, panel);
    wrap.append(sheet);

    let esc = null;
    const close = () => {
      sheet.classList.remove("is-on");
      if (esc) { document.removeEventListener("keydown", esc); esc = null; }
      iSheetClose = null;
      sheetLid = null;
    };
    const open = (page, lid) => {
      if (page) cur = page;
      sheet.classList.add("is-on");
      if (!esc) esc = (e) => { if (e.key === "Escape") close(); };
      document.addEventListener("keydown", esc);
      iSheetClose = close;
      paintPanel();
      if (lid) {
        const row = bd.querySelector('.li-prow[data-lid="' + lid + '"]');
        if (row) {
          row.classList.remove("is-flash");
          void row.offsetWidth;
          row.classList.add("is-flash");
          row.scrollIntoView({ block: "center", behavior: "smooth" });
        }
      }
    };
    openBtn.addEventListener("click", () => open());
    scrim.addEventListener("click", close);
    x.addEventListener("click", close);
    iSheetOpen = open;

    // 真实页：区块先收进隐藏 stash（磁贴点开才搬进抽屉，关闭归还）——
    // 不收的话磁贴后面还平铺着真实区块。stash 挂在 .li 内（文档内、仅隐藏），
    // teardown 沿 .ba-theme-root 查询照样能搬回。
    if (build) {
      const stash = n("div", "ba-i-stash");
      stash.style.display = "none";
      const secById = new Map();
      for (const l of lds) {
        const sec = build(l);
        if (sec) { secById.set(l.id, sec); stash.append(sec); }
      }
      wrap._iBuild = (l) => {
        realSec.replaceChildren();
        const sec = secById.get(l.id);
        if (sec) realSec.append(sec);
      };
      wrap._iOpen = open;
      wrap.append(stash);
    }
    return wrap;
  };

  // ── J：三栏工作台（设计稿 viewJ 原样）──────────────────────────────────
  THEME_RENDERERS.J = (lds, build, opts) => {
    opts = opts || {};
    const buildPane = build || ((l) => {
      const frag = n("div");
      const list = n("div", "entries");
      list.style.padding = "0";
      l.entries.forEach((e) => list.append(entryRow(e, l)));
      frag.append(list);
      const note = n("div", "la-foot");
      note.style.marginTop = "10px";
      note.style.borderRadius = "8px";
      note.style.border = "1px solid var(--line)";
      note.append(icon("info", "sm"), n("span", null, l.healthText || ""));
      frag.append(note);
      return frag;
    });
    const wrap = n("div", "lj");
    const c1 = n("aside", "lj-c1");
    c1.append(n("div", "hd", "通道"));
    const nav = n("div", "lj-nav");
    // 左右布局下每个渲染器只拿到**自己那一列**的引导器：记忆里的 active
    // 可能不在这一列（themeSel 是全局的），直接用会取到 undefined →
    // 读 .entries 当场抛错，整页布置中断（「左右布局点了没变化」的一个真凶）。
    let active = (opts.active && lds.some((x) => x.id === opts.active))
      ? opts.active
      : (lds[0] && lds[0].id);
    const c2 = n("section", "lj-c2");
    const head = n("div", "hd");
    const bd = n("div", "bd");
    c2.append(head, bd);
    const c3 = n("aside", "lj-c3");
    const c3hd = n("div", "hd");
    c3hd.append(icon("gear", "sm"), n("b", null, "引导器设置"), n("span", "grow"));
    const fold = icoBtn("chevron", "收起设置栏（只留图标条）");
    c3hd.append(fold);
    const c3bd = n("div", "bd");
    c3.append(c3hd, c3bd);
    const panes = new Map();

    function paint(id) {
      active = id;
      const l = lds.find((x) => x.id === id);
      if (!l) return;
      [...nav.children].forEach((c) => c.classList.toggle("is-on", c.dataset.lid === id));
      head.replaceChildren(ldr(l, "sm"), n("b", null, l.name), stateChip(l), n("span", "grow"));
      head.lastChild.style.marginLeft = "auto";
      head.append(btn("下次启动", "btn xs", "power"), icoBtn("more", "更多"));
      panes.forEach((pane, pid) => pane.classList.toggle("is-shown", pid === id));

      // 上一份停靠的设置先搬回它自己的卡：直接 replaceChildren 会把 body 连
      // 同里面的真控件一起摘出文档 —— teardown 的 restore 再也找不到它们
      //（isConnected 为假），用户填过的 ESP 路径 / 脚本就此消失。
      const docked = c3bd.querySelector(":scope > .ba-set-body.is-docked");
      if (docked) {
        const owner = docked.dataset.owner ? document.getElementById(docked.dataset.owner) : null;
        if (owner) {
          owner.appendChild(docked);
          const ob = owner.querySelector(":scope > .ba-setbar");
          if (ob) ob.style.display = "";
        } else {
          docked.remove();
        }
      }
      c3bd.replaceChildren();
      // 右栏就是「引导器设置」本体：把当前引导器卡里那份**真实**设置（ESP
      // 分区 / 分区明细 / 脚本 / 免密…）整块搬过来。原先这里另画一份设计稿
      // 示意面板（改/刷新/提升/编辑全是死按钮），而卡里还挂着一条同名的
      // 「引导器设置」折叠条 —— 同一个标题两次、其中一份点不动（2026-10-30
      // 用户定案：重复的要合并）。
      const sec = sectionFor(l.id);
      const bar = sec ? sec.querySelector(":scope > .ba-setbar") : null;
      const body = sec ? sec.querySelector(":scope > .ba-set-body") : null;
      if (bar) bar.style.display = "none";
      if (body) {
        body.classList.add("is-docked");
        body.dataset.owner = sec.id;
        c3bd.append(body);
      } else {
        const empty = n("div", "lj-setempty");
        empty.append(n("b", null, "这个引导器没有可调设置"),
          n("small", null, l.entries.length
            ? "在中间列表里点一条，即为下次启动"
            : "还没读到菜单条目 —— 先在上方页面里扫描 / 读取"));
        c3bd.append(empty);
      }
      if (opts.onSelect) opts.onSelect(id);
    }

    lds.forEach((l) => {
      const b = n("button", "lj-navi");
      b.type = "button";
      b.dataset.lid = l.id;
      b.append(ldr(l, "sm"), n("span", "nm", l.name), dot(stateOf(l)));
      b.addEventListener("click", () => paint(l.id));
      nav.append(b);
      const pane = buildPane(l);
      if (pane) {
        pane.classList.add("ba-theme-pane");
        panes.set(l.id, pane);
      }
    });
    c1.append(nav);
    fold.addEventListener("click", () => c3.classList.toggle("folded"));
    bd.append(...panes.values());
    wrap.append(c1, c2, c3);
    paint(active);
    return wrap;
  };

  // ═══════════════════════════════════════════════════════════════════
  // 系统引导页（#tab-boot）布置
  // ═══════════════════════════════════════════════════════════════════
  const bootTab = () => document.getElementById("tab-boot");
  let bootCanonical = null;

  function captureCanonical() {
    const t = bootTab();
    if (t && !bootCanonical) bootCanonical = Array.from(t.children);
  }

  function sectionFor(id) {
    if (id === "refind") return document.getElementById("refind");
    if (id === "grub") return document.getElementById("grub");
    return document.getElementById("channel-" + id);
  }

  // 主题切换留下的「版面类」必须清干净：B / C / I / J 这些分页主题会把真实区块
  // 当成自己的 pane，挂上 .ba-theme-pane（非 .is-shown 一律 display:none!important），
  // G 会挂 .ba-g-card / 给条目行挂 .ba-g-hide。这些类不清的话，用户从 B 点过某个
  // 引导器再换别的主题，**只有那一个区块看得见**（其余全被 :not(.is-shown) 吃掉）——
  // 「明明全选了，系统引导只显示一个」就是这么来的。内联 display 同理（K 隐藏用）。
  const LAYOUT_CLASSES = [
    "ba-theme-pane", "is-shown", "ba-g-card", "ba-g-empty", "is-active",
  ];

  function resetSectionChrome() {
    const t = bootTab();
    if (!t) return;
    for (const id of LOADER_IDS) {
      const s = sectionFor(id);
      if (!s) continue;
      s.classList.remove(...LAYOUT_CLASSES);
      s.style.display = "";
    }
    for (const el of t.querySelectorAll(".ba-g-hide, .ba-g-empty")) {
      el.classList.remove("ba-g-hide", "ba-g-empty");
    }
    // G 主题往条目行里塞的副标题（名称 + mono 路径）也要拔掉，否则切到
    // 别的主题后行里会多出一行路径文字
    t.querySelectorAll(".ba-g-sub").forEach((el) => el.remove());
  }

  function teardownBootArrangement() {
    const t = bootTab();
    if (!t) return;
    // 设置项搬移必须先还原：它们被搬进注入容器 .ba-set-body，而下面按
    // [data-ba-injected] 删容器时会把还在里面的节点一起带走（真控件就没了）
    restoreSetMoves();
    // lr 左右分栏：两栏容器是注入节点，先把里面的区块倒回 #tab-boot 顶层，
    // 否则下面按 [data-ba-injected] 移除容器时会把区块连着一起带走
    for (const col of Array.from(t.querySelectorAll(".ba-lr-cols > .ba-col"))) {
      for (const node of Array.from(col.children)) t.appendChild(node);
    }
    delete document.body.dataset.baLr;
    for (const root of Array.from(t.querySelectorAll(".ba-theme-root"))) {
      for (const s of Array.from(root.querySelectorAll("#refind, #grub, .boot-channel"))) {
        t.appendChild(s);
      }
      root.remove();
    }
    const mountEl = document.getElementById("boot-channels-mount");
    if (mountEl) {
      mountEl.classList.remove("ba-contents");
      for (const card of Array.from(t.querySelectorAll(".boot-channel"))) {
        if (card.parentElement === mountEl) continue;
        const fresh = card.id && mountEl.querySelector("#" + (window.CSS && CSS.escape ? CSS.escape(card.id) : card.id));
        if (fresh) card.remove();
        else mountEl.appendChild(card);
      }
    }
    for (const node of Array.from(t.querySelectorAll("[data-ba-injected]"))) node.remove();
    // 设置项分离态还原：拆掉 ba-split 标记，设置子块回到原位（data-ba-set 一并清掉）
    for (const sec of Array.from(t.querySelectorAll(".ba-split"))) {
      sec.classList.remove("ba-split", "ba-set-open");
      sec.querySelectorAll("[data-ba-set]").forEach((el) => { delete el.dataset.baSet; });
    }
    t.classList.remove("ba-arranged", "ba-lr", "ba-theme", "ba-fill");
    delete document.body.dataset.baFill;
    t.style.gridTemplateColumns = "";
    for (const node of Array.from(t.children)) {
      node.style.order = "";
      node.style.display = "";
      node.style.gridColumn = "";
      node.style.gridRow = "";
      if (node.dataset) delete node.dataset.baCol;
    }
    if (bootCanonical) for (const node of bootCanonical) t.appendChild(node);
    resetSectionChrome();
  }

  // ── 引导器设置项 ⇄ 引导列表 分离（设计稿「设置三级收起」第二级）────────
  // 设计稿铁律：引导页是「纯菜单观感」——每张卡只留标题行 + 引导列表；
  // 设置项（ESP 表单 / 扫描按钮 / 变量目录 / 脚本编辑器…）收进卡片底部一条
  // 「引导器设置」acc 折叠条，点开就地展开（不跳页、不弹窗）。展开的内容
  // **一律落在引导列表下方**（2026-10-30 用户定案，见 splitLoaderSection）。
  const SET_SPLIT = {
    // 读取方式页签（自动扫描 / 手动处理、自动读取 / 手动解析）**不进设置**：
    // 2026-10-30 用户定案「列表最先，其次是自动/手动切换按钮，再后面才是其他
    // 设置」—— 它是高频切换，收起来就等于没了。
    refind: [
      ".sim-scan-head", "#sim-scan-summary", "#sim-scan-issues",
      ".directory",
    ],
    grub: [
      "#grub-esp .grub-esp-head", "#grub-esp .grub-row",
      // 分区明细（arch · UUID · cfg · env 那三行）：2026-10-30 用户要求
      // 一并收进「引导器设置」—— 它原先挂在列表框头，env 行里两个超长
      // 一次性/默认条目名会把列表顶下去一百多像素。
      "#grub-auto-target",
      ".grub-auto-head", "#grub-auto-summary", "#grub-auto-issues",
      "#grub-auto-picker", "#grub-auto-foot", ".grub-panel-note",
      "#grub-install-actions", "#grub-log-box", ".grub-script", "#grub-advice",
      "#grub-format-row", "#grub-hint", "#grub-error",
    ],
    channel: [".oc-menu-issues", ".oc-menu-elevate", ".channel-helper"],
  };
  const SET_HINT = {
    refind: "重新模拟 · 扫描摘要 · 变量目录",
    grub: "ESP 分区 · 分区明细 · 同步 · 脚本",
    channel: "健康检查 · 免密模式",
  };

  // 各卡「引导器设置」的展开态按 section id 记忆：快照每 5s 重排会重建折叠条，
  // 不能把用户展开着的设置吹回去（与 G 搜索框焦点 / I 抽屉重开同一类坑）。
  const setExpandById = new Map();

  // 展开的内容必须落在**引导列表下方**（2026-10-30 用户定案）。
  // 设置项在原 DOM 里两头都有：扫描头 / 摘要 / 读取方式页签在列表**上方**，
  // 分区明细 / 脚本 / 须知在下方，而且埋在 panel、card 这些不同层级里 ——
  // 只靠 CSS order 跨不了嵌套容器，所以真搬：符合条件的节点统一搬进 section
  // 末尾的 .ba-set-body（折叠条之后），收起时整块不显示、展开时原样铺开。
  // 搬动记原父与原后邻，teardown 时按记录**倒序**搬回（倒序才能让同一父下的
  // 兄弟节点各归各位）；节点若已被快照重建（isConnected 为假）就跳过。
  const setMoves = []; // { el, parent, next, sec }

  function restoreSetMoves(scope) {
    for (let i = setMoves.length - 1; i >= 0; i--) {
      const m = setMoves[i];
      if (scope && m.sec !== scope) continue;
      setMoves.splice(i, 1);
      if (!m.el || !m.el.isConnected || !m.parent) continue;
      if (m.el.parentElement === m.parent) continue;
      const at = m.next && m.next.parentElement === m.parent ? m.next : null;
      m.parent.insertBefore(m.el, at);
    }
  }

  function splitLoaderSection(sec, id) {
    if (!sec) return;
    const sels = SET_SPLIT[id] || SET_SPLIT.channel;
    const marked = [];
    sec.querySelectorAll(sels.join(",")).forEach((el) => {
      el.dataset.baSet = "1";
      marked.push(el);
    });
    const count = marked.length;
    let bar = sec.querySelector(":scope > .ba-setbar");
    let body = sec.querySelector(":scope > .ba-set-body");
    if (!count) {
      restoreSetMoves(sec);
      if (bar) bar.remove();
      if (body) body.remove();
      sec.classList.remove("ba-split", "ba-set-open");
      return;
    }
    sec.classList.add("ba-split");
    const open = Boolean(setExpandById.get(sec.id));
    sec.classList.toggle("ba-set-open", open);
    if (!bar) {
      bar = n("button", "acc ba-setbar");
      bar.type = "button";
      bar.dataset.baInjected = "1";
      const g = n("span", "grow");
      g.append(n("span", null, "引导器设置"),
        n("small", null, (SET_HINT[id] || SET_HINT.channel) + " · " + count + " 项"));
      bar.append(icon("gear", "sm"), g, icon("chevron", "sm car"));
      bar.addEventListener("click", () => {
        const now = bar.getAttribute("aria-expanded") !== "true";
        bar.setAttribute("aria-expanded", String(now));
        sec.classList.toggle("ba-set-open", now);
        setExpandById.set(sec.id, now);
      });
    }
    bar.setAttribute("aria-expanded", String(open));
    if (!body) {
      body = n("div", "ba-set-body");
      body.dataset.baInjected = "1";
    }
    // 版面顺序固定为「引导列表 … → 折叠条 → 设置面板」：折叠条与面板永远压在
    // section 末尾，跟设置项原先埋在列表上方还是下方无关。
    if (bar.parentElement !== sec || body.parentElement !== sec
      || sec.lastElementChild !== body || body.previousElementSibling !== bar) {
      sec.appendChild(bar);
      sec.appendChild(body);
    }
    for (const el of marked) {
      if (el.parentElement === body) continue;
      setMoves.push({ el, parent: el.parentElement, next: el.nextSibling, sec });
      body.appendChild(el);
    }
  }

  // 真实页的区块构建器：直接返回既有区块（动作监听都在，搬动不丢）
  const bootBuild = (l) => {
    const s = sectionFor(l.id);
    if (s) {
      s.classList.add("ba-theme-sec");
      splitLoaderSection(s, l.id);
    }
    return s;
  };

  function themeRoot(t) {
    t.classList.add("ba-arranged", "ba-theme");
    for (const hr of t.querySelectorAll("hr.boot-divider")) hr.style.display = "none";
    const root = n("div", "ba-theme-root");
    const nb = document.getElementById("next-boot");
    const priv = document.getElementById("privCard");
    const collapsed = document.getElementById("boot-channel-collapsed");
    if (nb) nb.style.order = "0";
    if (collapsed) collapsed.style.order = "2";
    if (priv) priv.style.order = "2";
    t.appendChild(root);
    return root;
  }

  // 布置用的启用清单：真实页还要求区块真的存在
  function placedLoaders(needSection) {
    return enabledLoaders().filter((l) => !needSection || sectionFor(l.id));
  }

  function applyStyleDensity() {
    const st = advState.style;
    if (st === "full") delete document.body.dataset.bootStyle;
    else document.body.dataset.bootStyle = st;
  }

  // 「下次启动」：每行 = 一个引导器 + 它当前选中的条目（2026-10-30 用户定案）。
  // 只列「引导高级设置」里勾选启用的引导器 —— 没勾选的不出现；还没探测到的
  // 也不占行（一行「未检测到」对「下次会启动什么」没有信息量）。一行都凑不齐
  // （数据还没来 / 全没启用）就整块隐藏，退回原来的单行大字。
  function paintNextBootRows() {
    const box = document.getElementById("next-boot-rows");
    const sec = document.getElementById("next-boot");
    if (!box || !sec) return;
    box.replaceChildren();
    for (const l of enabledLoaders()) {
      // 探测到了但一条都没读到（例如 OpenCore 只给了 channel 名字、没有菜单
      // 条目）时占一行「未选中条目」纯属噪音 —— 直接不列。
      if (!l.detected || (!l.entries.length && !l.nextBoot)) continue;
      // 各 loader 的 entries 都带 def 标记（rEFInd 的 lastBoot / GRUB 的
      // nextEntry·savedEntry / 通道的 currentDefault 或预演 default），选中项
      // 由 loader 的 nextBoot 给出（通道的默认项可能不在可点集合里）。
      const def = l.nextBoot || l.entries.find((e) => e.def) || null;
      const row = n("div", "nb-row");
      row.append(ldr(l, "sm"), n("b", "nb-tag", l.name));
      const val = n("span", "nb-entry" + (def ? "" : " is-empty"), def ? def.t : "未选中条目");
      val.title = def
        ? (def.detail ? def.t + " · " + def.detail : def.t)
        : "在下面的引导列表里点一条，设为下次启动";
      row.append(val);
      if (def && def.detail) row.append(n("small", "nb-detail", def.detail));
      box.append(row);
    }
    const show = box.childElementCount > 0;
    box.hidden = !show;
    sec.classList.toggle("has-rows", show);
  }

  // 真实页主题布置：设计稿 viewK boot 模式同构 —— k-region（虚线）+ k-rbody +
  // k-theme-fit（主题容器包真实区块）；lr 时左右两个 region + k-split。
  function bootThemeStage(list) {
    const t = bootTab();
    const root = themeRoot(t);
    const stage = n("div", "k-stage");
    const splitH = effectiveLayout() === "lr";
    stage.classList.toggle("h", splitH);

    function fillRegion(rbody, cols) {
      rbody.classList.add("is-theme");
      // 空列（左右布局下这一列没分到引导器）：渲染器拿到空清单会取 lds[0]
      // 为 undefined，随后读 .entries 抛错 —— 直接给占位说明就好
      if (!cols.length) {
        const empty = n("div", "empty");
        empty.append(n("b", null, "这一列是空的"), n("span", null, "去「引导高级设置」点标签上的「左/右」分配"));
        rbody.append(empty);
        return;
      }
      const renderer = THEME_RENDERERS[effectiveTheme()];
      const fit = n("div", "k-theme-fit");
      fit.append(renderer
        ? renderer(cols, bootBuild, {
          active: themeSel[effectiveTheme()],
          onSelect: (id) => { themeSel[effectiveTheme()] = id; },
        })
        : n("div", "empty", n("b", null, `主题 ${effectiveTheme()} 还没实装`)));
      rbody.append(fit);
    }

    function makeSplit(firstRegion) {
      const split = n("div", "k-split");
      split.dataset.baInjected = "1";
      split.title = "拖动调整左右两列的大小（双击恢复对半分）";
      split.addEventListener("pointerdown", (e) => {
        e.preventDefault();
        split.classList.add("is-drag");
        split.setPointerCapture(e.pointerId);
        const onMove = (ev) => {
          const rect = stage.getBoundingClientRect();
          advState.ratio = Math.min(0.85, Math.max(0.15, (ev.clientX - rect.left) / rect.width));
          firstRegion.style.width = (advState.ratio * 100).toFixed(1) + "%";
        };
        const onUp = () => {
          split.classList.remove("is-drag");
          split.removeEventListener("pointermove", onMove);
          split.removeEventListener("pointerup", onUp);
          saveState();
          bootAdvApplyToBootPage();
        };
        split.addEventListener("pointermove", onMove);
        split.addEventListener("pointerup", onUp);
      });
      split.addEventListener("dblclick", () => { advState.ratio = 0.5; saveState(); bootAdvApplyToBootPage(); });
      split.append(splitResetButton(() => { advState.ratio = 0.5; saveState(); bootAdvApplyToBootPage(); }));
      return split;
    }

    if (splitH) {
      const L = list.filter((l) => advState.side[l.id] !== 2);
      const R = list.filter((l) => advState.side[l.id] === 2);
      const mk = (cols, ratio) => {
        const region = n("section", "k-region");
        region.style.flex = "none";
        region.style.width = ratio * 100 + "%";
        const rbody = n("div", "k-rbody v");
        region.append(rbody);
        fillRegion(rbody, cols);
        return region;
      };
      const r0 = mk(L, advState.ratio);
      stage.append(r0, makeSplit(r0), mk(R, 1 - advState.ratio));
      if (!L.length || !R.length) {
        const empty = n("div", "empty");
        empty.dataset.baInjected = "1";
        empty.append(n("b", null, "有一列是空的"), n("span", null, "去「引导高级设置」点标签上的「左/右」分配"));
        stage.append(empty);
      }
    } else {
      const region = n("section", "k-region");
      const rbody = n("div", "k-rbody v");
      region.append(rbody);
      fillRegion(rbody, list);
      stage.append(region);
    }
    root.append(stage);
    observeVw(root);
  }

  // K 布置：出厂样貌一个 class 都不加；其余 order 排 + lr 两栏。
  // 加宽（wide）不改页面布置 —— 它只负责窗口落宽，页面保持出厂单列样貌。
  function applyKArrangement() {
    const t = bootTab();
    const st = advState;
    const pristine = st.theme === "K" && (effectiveLayout() === "default" || effectiveLayout() === "wide")
      && LOADER_IDS.every((id) => st.visible[id])
      && st.order.join(",") === LOADER_IDS.join(",");
    if (pristine) return;
    t.classList.add("ba-arranged");
    const nb = document.getElementById("next-boot");
    const priv = document.getElementById("privCard");
    const collapsed = document.getElementById("boot-channel-collapsed");
    if (nb) nb.style.order = "0";
    if (collapsed) collapsed.style.order = "90";
    if (priv) priv.style.order = "100";
    for (const hr of t.querySelectorAll("hr.boot-divider")) hr.style.display = "none";
    const mountEl = document.getElementById("boot-channels-mount");
    if (mountEl) mountEl.classList.add("ba-contents");
    const lr = effectiveLayout() === "lr";
    let colL = null, colR = null;
    if (lr) {
      // GRUB 独立区域同款形态（该功能已移除、布局能力由这里承接，2026-10-30
      // 用户定案）：两栏各包进自己的滚动容器、等宽、中缝 12px 放拖动条；
      // 页头/页签/下次启动/免密卡/折叠条仍留在整页层。
      t.classList.add("ba-lr");
      document.body.dataset.baLr = "1";
      colL = n("div", "ba-col");
      colR = n("div", "ba-col");
      const cols = n("div", "ba-lr-cols");
      cols.dataset.baInjected = "1";
      cols.style.order = "50";
      const split = n("div", "k-split");
      split.title = "拖动调整左右两列（双击恢复对半分）";
      const applyRatio = () => {
        cols.style.gridTemplateColumns =
          `minmax(0, calc((100% - 12px) * ${st.ratio})) 12px`
          + ` minmax(0, calc((100% - 12px) * ${1 - st.ratio}))`;
      };
      split.addEventListener("pointerdown", (ev) => {
        ev.preventDefault();
        split.setPointerCapture(ev.pointerId);
        split.classList.add("is-drag");
        const rect = cols.getBoundingClientRect();
        const onMove = (m) => {
          st.ratio = Math.min(0.85, Math.max(0.15, (m.clientX - rect.left) / (rect.width - 12)));
          applyRatio();
        };
        const onUp = () => {
          split.classList.remove("is-drag");
          split.removeEventListener("pointermove", onMove);
          split.removeEventListener("pointerup", onUp);
          saveState();
          applyAll();
        };
        split.addEventListener("pointermove", onMove);
        split.addEventListener("pointerup", onUp);
      });
      split.addEventListener("dblclick", () => {
        st.ratio = 0.5;
        saveState();
        applyAll();
      });
      // 一键回正中间（点它不会误起拖动）
      split.append(splitResetButton(() => {
        st.ratio = 0.5;
        applyRatio();
        saveState();
      }));
      cols.append(colL, split, colR);
      applyRatio();
      t.appendChild(cols);
    }
    let anyL = false, anyR = false;
    st.order.forEach((id, i) => {
      const s = sectionFor(id);
      if (!s) return;
      if (!st.visible[id]) { s.style.display = "none"; return; }
      s.style.display = "";
      s.style.order = String(i + 1);
      splitLoaderSection(s, id);
      if (lr) {
        (st.side[id] === 2 ? colR : colL).appendChild(s);
        if (st.side[id] === 2) anyR = true; else anyL = true;
      }
    });
    if (lr) {
      const empty = n("div", "empty");
      empty.dataset.baInjected = "1";
      empty.append(n("b", null, "有一列是空的"), n("span", null, "去「引导高级设置」点标签上的「左/右」分配"));
      if (!anyL) colL.append(empty);
      else if (!anyR) colR.append(empty);
    }
    observeVw(t);
  }

  // ── 布局窗口落宽（2026-10-30 用户定案：加宽 / 左右布局先扩大窗口宽度）────
  // wide / lr = 窗口至少 932（两栏标称 460+12+460，后端「窄了才加宽」）；
  // default = 切回默认时回内嵌标称宽 460。
  //
  // 两个踩过的坑：
  // ① Tauri v2 的 IPC 在 `__TAURI__.core.invoke`，**不是** `__TAURI__.invoke` ——
  //    原先判 `!tauri.invoke` 就 return，set_boot_layout 一次都没发出去（trace
  //    里完全没有这条命令），表现就是「选什么布局窗口都不动、点默认也不缩回」。
  // ② layoutWidthSent 初值必须取**已存布局**：写死 "default" 时，用户上次存的是
  //    lr、这次点「默认」判等成立 → 又静默不发，窗口永远停在 932。启动首拍
  //    仍按已存布局判等（等于自己）→ 不发，尊重记忆值 / 用户手拖的宽度。
  // 浏览器 / 手机 / hub 走内置 HTTP 的 /api/ui-command（与 app.js 同一形状），
  // 远端也能改本机窗口落宽；两条通道都没有才 no-op。
  let layoutWidthSent = effectiveLayout();
  let lastAppliedLayout = null; // 只用于 trace 诊断：上次真正生效的布局档位
  let widenTries = 0; // 加宽档「窗口还窄」时的补发计数（上限 3，防每拍重发）

  function layoutInvoke(command, args) {
    const host =
      location.protocol === "tauri:" || location.hostname === "tauri.localhost";
    const tauri = window.__TAURI__;
    if (host && tauri && tauri.core && tauri.core.invoke) {
      return tauri.core.invoke(command, args);
    }
    if (!/^https?:$/.test(location.protocol)) return Promise.resolve();
    let token = "";
    const hit = document.cookie.match(/(?:^|;\s*)anyboot_token=([^;]*)/);
    if (hit) {
      try {
        token = decodeURIComponent(hit[1]);
      } catch {
        token = hit[1];
      }
    }
    if (!token) return Promise.resolve();
    return fetch("/api/ui-command", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Auth-Token": token },
      body: JSON.stringify({ cmd: command, args }),
    }).then((res) => res.json().catch(() => ({})));
  }

  function ensureLayoutWidth(layout) {
    // 加宽 / 左右布局都要求窗口 ≥ 两栏标称宽（932）。后端是「窄了才加宽」，
    // 已经够宽时它什么也不做（不会把用户手拖的大窗口压回去），所以窄着就补
    // 一发 —— B / G / J 只能加宽，这条是它们唯一的窗口保证，不能只赌
    // 「档位变了才发」（上次关窗时窗口被拖窄、重开后档位没变就永远窄着）。
    // 只在真实窗口里做「还窄就补发」：远端（手机/浏览器打开本机 UI）的
    // innerWidth 是**看的那块屏**，拿它判本机窗口会一直判成窄、反复去改主机窗口。
    const inTauri = location.protocol === "tauri:" || location.hostname === "tauri.localhost";
    const narrow = inTauri && layout !== "default" && window.innerWidth > 0 && window.innerWidth < 900;
    if (layoutWidthSent === layout && !(narrow && widenTries < 3)) return;
    if (narrow) widenTries += 1; else widenTries = 0;
    layoutWidthSent = layout;
    layoutInvoke("set_boot_layout", { mode: layout }).catch(() => {});
  }

  // 诊断：桌面端前端异常不落 trace（unhandled rejection 只在 DevTools 里看得到），
  // 「选了主题/点了左右，系统引导页还是依旧」这类问题否则没有任何取证途径。
  // 借既有的 sol_log_client_error 回传，失败静默（诊断不能反过来搞坏渲染）。
  function reportClientError(scope, error) {
    try {
      const tauri = window.__TAURI__;
      const invoke = tauri && tauri.core && tauri.core.invoke;
      if (!invoke) return;
      invoke("sol_log_client_error", {
        kind: "boot-adv",
        detail: scope + ": " + ((error && (error.stack || error.message)) || String(error)),
      }).catch(() => {});
    } catch (inner) {
      void inner;
    }
  }

  // 总入口：先拆旧布置，再按当前状态布置
  function bootAdvApplyToBootPage() {
    captureCanonical();
    const reopenLid = sheetLid;
    const reopenPage = sheetPage;
    teardownBootArrangement();
    if (!bootTab()) return;
    applyStyleDensity();
    // 关掉的引导器在任何主题下都得藏起来：非 K 主题原先没人管这件事，关掉的卡
    // 会平铺在主题容器下面（K 由 applyKArrangement 自己处理，这里先统一压一遍）
    for (const id of LOADER_IDS) {
      const s = sectionFor(id);
      if (s && !advState.visible[id]) s.style.display = "none";
    }
    closeSheet();
    sheetLid = null;
    iSheetOpen = null;
    if (effectiveTheme() === "K") {
      applyKArrangement();
    } else {
      const list = placedLoaders(true);
      if (list.length) bootThemeStage(list);
    }
    // 主题态（非 K）：整页切成「一屏内滚」—— #tab-boot 变 flex 列、主题容器
    // 吃掉剩余高度，滚动交给各主题自己的列表（.lb-scroll / .lg-results /
    // .lj-c2 .bd / .li-grid / .lc-body / .la-list）。设计稿的 .win-body 就是
    // 「固定高 + 内部滚」，而 app 原先让主题随内容长高、整页滚 —— 于是同一
    // 主题在 932 和 1180 下高度各不相同，短主题（G）底下留一大片空。
    {
      const t = bootTab();
      const fill = effectiveTheme() !== "K";
      if (t) t.classList.toggle("ba-fill", fill);
      if (fill) document.body.dataset.baFill = "1";
      else delete document.body.dataset.baFill;
    }
    // 加宽 / 左右布局开着时窗口加宽到两栏标称；切回默认时收回
    ensureLayoutWidth(effectiveLayout());
    // 「下次启动」多行态与主题无关，所有主题（含 K 出厂态）都要刷 —— 它跟
    // 「哪个引导器被启用 / 各自选中了哪条」绑定，那两份数据都在本模块手里。
    try {
      paintNextBootRows();
    } catch (error) {
      reportClientError("paintNextBootRows", error);
    }
    // 诊断：布局档位一变就往 trace 落一行（含实际生效值与真实 DOM 证据）。
    // 「选了左右布局、系统引导没变化」这类问题前端异常不进 trace，只能靠这行
    // 区分是「没布置」还是「布置了但看不见」。
    const applied = effectiveLayout();
    if (applied !== lastAppliedLayout) {
      lastAppliedLayout = applied;
      const t = bootTab();
      reportClientError("layout", "saved=" + advState.layout + " effective=" + applied
        + " theme=" + effectiveTheme() + " tabClass=" + (t ? t.className : "-")
        + " cols=" + (t ? t.querySelectorAll(".ba-lr-cols").length : -1)
        + " colKids=" + (t && t.querySelector(".ba-col") ? t.querySelector(".ba-col").children.length : -1));
    }
    // I 主题：抽屉开着的话重排后凭 sheetLid 重开（快照每 5s 一拍，不能吹掉）
    if (reopenLid && effectiveTheme() === "I" && iSheetOpen) {
      const l = placedLoaders(true).find((x) => x.id === reopenLid);
      const rootEl = bootTab().querySelector(".ba-theme-root .li");
      if (l && rootEl && rootEl._iBuild) {
        rootEl._iBuild(l);
        sheetLid = reopenLid;
        iSheetOpen(reopenPage === "引导器" ? "引导器" : "编排", reopenLid);
      }
    }
  }

  function applyAll() {
    // 渲染层抛异常 = 整页停在上一拍（「选了主题还是依旧」的真凶就是这种静默
    // 中断）。异常本身照旧往外抛（该看见的还得看见），但先回传一份给 trace。
    //
    // 两个对象必须**各自**兜住：设置页（renderAdvTab）里任何一个渲染异常，
    // 都不能把「系统引导」页的布置一起带走 —— 那会表现成「点了没保存、页面
    // 也没变化」，而用户看不出是设置页先炸的。
    let firstError = null;
    try {
      renderAdvTab();
    } catch (error) {
      firstError = error;
      reportClientError("renderAdvTab", error);
    }
    try {
      bootAdvApplyToBootPage();
    } catch (error) {
      if (!firstError) firstError = error;
      reportClientError("bootAdvApplyToBootPage", error);
    }
    if (firstError) throw firstError;
  }

  // ── 导出 ─────────────────────────────────────────────────────────────
  window.BootAdv = {
    get state() { return advState; },
    get loaders() { return lastLoaders; },
    enabledLoaders,
    normState,
    resetState,
    saveState,
    THEME_H,
    THEME_LAYOUT_MODES,
    layoutModes,
    layoutWideOnly,
    effectiveLayout,
    effectiveTheme,
    isGuestView,
    THEMES,
    THEME_RENDERERS,
    LOADER_IDS,
    LOADER_META,
    OS_TAG,
    onSnapshot,
    bootAdvApplyToBootPage,
    closeSheet,
  };
})();
