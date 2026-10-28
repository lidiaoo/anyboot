// 三引导器通道（systemd-boot / OpenCore / Clover）的卡片渲染。
//
// 数据源：快照里的 `channels` 数组（后端 channels::refresh 产出，每通道一份
// ChannelView：detected / chip / health / entries / currentDefault / lastBoot）。
// 结构与 .grub / .refind 卡片完全同构：grub-title-row + zone-tag 圆标 +
// 条目行（.boot-entry）。检测到的通道渲染完整卡片；没检测到的收进底部折叠条。
//
// 切换值域与后端 `switch_system` 同一套：
//   sdboot:<entryId> / opencore / opencore-register[:short|:full] / clover-oneshot
"use strict";

const BOOT_CHANNEL_ICONS = {
  linux: "🐧",
  windows: "🪟",
  macos: "🍎",
  menu: "⚙️",
};

const BOOT_CHANNEL_TAGS = {
  sdboot: "systemd-boot 区",
  opencore: "OpenCore 区",
  clover: "Clover 区",
};

/// chip 文案：ok/warn/off 三态 + permission（file=免提权 / nvram=需要提权）
function bootChannelChipText(channel) {
  if (channel.chip === "off") return "未检测到";
  const permission =
    channel.permission === "nvram" ? "需要提权" : "免提权";
  return channel.chip === "warn" ? `已检测 · ${permission}` : `已检测 · ${permission}`;
}

// ── 富文本：设计稿的排版规则 ──
// docs/boot-channel-ui-mockup.html 里健康检查条目是
// `· <code>Misc→Boot→ShowPicker = false</code>：OC 不会弹菜单…`
// —— 配置键 / NVRAM 变量名一律走等宽 <code>，不然一整串中文读下来分不清
// 哪一段是键名、哪一段是解释。后端文案用反引号标注这些片段，这里切开渲染。
// 非 code 部分仍用文本节点（不拼 HTML），文案里的任何字符都不会被当成标签。
function appendRichText(container, text) {
  String(text ?? "")
    .split("`")
    .forEach((part, index) => {
      if (!part) return;
      if (index % 2 === 1) {
        const code = document.createElement("code");
        code.textContent = part;
        container.append(code);
      } else {
        container.append(document.createTextNode(part));
      }
    });
}

// ── 预演 OC 菜单 ──
// 数据来自后端 oc_menu::preview（按 OC 自己的规则纯读复算一遍）。这里只管排版：
// 一行一项，标出来源（BootOrder / bless / Preboot / 协议驱动）、类型、默认项。
const OC_MENU_KIND_LABEL = {
  macos: "macOS",
  windows: "Windows",
  generic: "泛型",
  linux: "Linux",
  tool: "工具项",
  unknown: "未知",
};

const OC_MENU_SOURCE_LABEL = {
  bootorder: "固件 BootOrder",
  bless: "bless 扫描",
  preboot: "APFS Preboot",
  protocol: "协议驱动",
};

/// 行首那条来源标签。`bootorder` 具体读的是哪一组变量由后端告诉我们：
/// `RequestBootVarRouting=true` 时 OC 读的是私有命名空间的 `OCBtOrder`，
/// 这时候再标「固件 BootOrder」就是写错了变量名（用户会去 `nvram` 里找一个不存在的东西）。
function ocMenuSourceLabel(source, orderSource) {
  if (source === "bootorder") {
    return orderSource === "private" ? "OC 私有 OCBtOrder" : "固件 BootOrder";
  }
  return OC_MENU_SOURCE_LABEL[source] || source;
}

const OC_MENU_ICONS = {
  macos: "🍎",
  windows: "🪟",
  generic: "⬜",
  linux: "🐧",
  tool: "⚙️",
  unknown: "❓",
};

/// 预演里的一行（卷 + 路径 + 来源 + 备注 + 「设为默认」）
function ocMenuRow(entry, index, { busy, onSwitch, channel, orderSource, defaultSlot } = {}) {
  const row = document.createElement("div");
  row.className = "oc-menu-row";
  row.dataset.kind = entry.kind;

  const idx = document.createElement("span");
  idx.className = "oc-menu-idx";
  idx.textContent = String(index + 1);

  const body = document.createElement("span");
  body.className = "oc-menu-body";

  const head = document.createElement("span");
  head.className = "oc-menu-head";
  const name = document.createElement("b");
  name.textContent = entry.title;
  head.append(name);

  const kind = document.createElement("span");
  kind.className = "oc-menu-kind";
  kind.textContent = `${OC_MENU_ICONS[entry.kind] || "⬜"} ${OC_MENU_KIND_LABEL[entry.kind] || entry.kind}`;
  head.append(kind);

  const source = document.createElement("span");
  source.className = "oc-menu-src";
  source.textContent = ocMenuSourceLabel(entry.source, orderSource);
  head.append(source);

  body.append(head);

  const path = document.createElement("code");
  path.className = "oc-menu-path";
  path.textContent =
    entry.volume && entry.volume !== "—"
      ? `${entry.volume} · ${entry.path}`
      : entry.path;
  body.append(path);

  if (entry.note) {
    const note = document.createElement("small");
    note.className = "oc-menu-note";
    note.textContent = entry.note;
    body.append(note);
  }

  row.append(idx, body);

  // 「设为默认」= OC 菜单里的 Ctrl+Enter。后端已经算好值域（entry.spec），
  // 前端只负责原样发回去 —— 哪些项能设由后端按 OC 的规则判定，不在这里猜。
  //
  // 已经是默认项的那一行**也要占住这个位置**：以前直接不渲染，看着像「这一行选不了」
  // 或「按钮怎么丢了」，而这恰恰是因为它本来就是默认项 —— OCBtOrder 里排第一的那条。
  if (entry.default) {
    const mark = document.createElement("span");
    mark.className = "oc-menu-set is-default";
    mark.textContent = "✓ 已是默认";
    mark.title = `${
      defaultSlot ? `${defaultSlot} ` : "启动顺序表第一条"
    }就是这一项：下次开机会直接进它。再「设为默认」只是把已经在最前的项再挪一次，所以这行不给按钮。`;
    row.append(mark);
  } else if (entry.spec) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "oc-menu-set needs-elev";
    button.textContent = "设为默认 ⤴";
    button.title =
      "等价于在 OpenCore 菜单里对它按 Ctrl+Enter：写 OC 私有命名空间的 OCBt0080 / OCBtOrder，" +
      "不碰被 macOS 挡住的 Boot* 变量；写前会整份备份 NVRAM";
    button.disabled = Boolean(busy);
    button.addEventListener("click", () => {
      if (onSwitch) onSwitch({ spec: entry.spec }, channel);
    });
    row.append(button);
  }
  return row;
}

/// 预演块默认展开：这张列表就是「这台机器上 OC 会看到什么」，收起来等于把
/// 关键信息藏了（用户 2026-10-28 报「怎么被折叠了」）。但「收起」这个动作要
/// 记住 —— 快照每次刷新都会重建 DOM，不存偏好就会被强行展开回去。
const OC_MENU_OPEN_KEY = "anyboot.ocMenuOpen";

function ocMenuOpenByDefault() {
  try {
    const saved = localStorage.getItem(OC_MENU_OPEN_KEY);
    return saved === null ? true : saved === "1";
  } catch {
    return true; // 隐私模式等取不到存储时，按默认展开
  }
}

/// 整块「预演 OC 菜单」（details，默认展开，用户手动收起后记住）
function renderOcMenuPreview(preview, { busy, onSwitch, channel, onPrebootElevate } = {}) {
  if (!preview) return null;
  const box = document.createElement("details");
  box.className = "oc-menu";
  box.open = ocMenuOpenByDefault();
  box.addEventListener("toggle", () => {
    try {
      localStorage.setItem(OC_MENU_OPEN_KEY, box.open ? "1" : "0");
    } catch {
      /* 存不了就算了，下次仍按默认展开 */
    }
  });

  const summary = document.createElement("summary");
  const title = document.createElement("span");
  title.className = "oc-menu-title";
  title.textContent = "预演 OC 菜单";
  const count = document.createElement("span");
  count.className = "oc-menu-count";
  const parts = [`${(preview.entries || []).length} 项`];
  if ((preview.hidden || []).length) {
    parts.push(`${preview.hidden.length} 项被隐藏`);
  }
  count.textContent = parts.join(" · ");
  summary.append(title, count);
  box.append(summary);

  const hint = document.createElement("p");
  hint.className = "oc-menu-hint";
  // 与上面健康检查同一套排版：键名 / 变量名用反引号标出，渲染成 <code>
  appendRichText(
    hint,
    "OC 不遍历目录：它按固定规则找（`BootOrder` + 每个卷的 bless/预定义路径 + 条目协议驱动）。" +
      "预演本身是纯读复算，不写盘、不改 NVRAM。" +
      "「设为默认」才会写：等价于在 OC 菜单对目标按 `Ctrl+Enter`，改的是 OC 私有命名空间的 " +
      "`OCBtOrder` / `OCBt0080`（macOS 不许写 `Boot*`，这一组可以），写前整份备份。",
  );
  box.append(hint);

  if ((preview.entries || []).length) {
    const list = document.createElement("div");
    list.className = "oc-menu-list";
    preview.entries.forEach((entry, index) =>
      list.append(
        ocMenuRow(entry, index, {
          busy,
          onSwitch,
          channel,
          orderSource: preview.orderSource,
          defaultSlot: preview.defaultSlot,
        }),
      ),
    );
    box.append(list);
  } else {
    const empty = document.createElement("p");
    empty.className = "oc-menu-hint";
    empty.textContent =
      "按当前配置算不出任何条目——ScanPolicy 太严、卷没挂载，或 OC 配置有问题。";
    box.append(empty);
  }

  if ((preview.hidden || []).length) {
    const hiddenBox = document.createElement("div");
    hiddenBox.className = "oc-menu-hidden";
    const head = document.createElement("strong");
    head.textContent = `HideAuxiliary=true：下面 ${preview.hidden.length} 项会收进二级菜单，首屏看不到`;
    hiddenBox.append(head);
    for (const entry of preview.hidden) {
      const line = document.createElement("small");
      line.textContent = `· ${entry.title}（${OC_MENU_KIND_LABEL[entry.kind] || entry.kind}）`;
      hiddenBox.append(line);
    }
    box.append(hiddenBox);
  }

  if ((preview.issues || []).length) {
    const issueBox = document.createElement("div");
    issueBox.className = "oc-menu-issues";
    for (const issue of preview.issues) {
      const line = document.createElement("small");
      if (issue.severity === "warn") line.className = "warn";
      line.append(
        document.createTextNode(`${issue.severity === "warn" ? "⚠" : "ⓘ"} `),
      );
      appendRichText(line, issue.text);
      issueBox.append(line);
    }
    box.append(issueBox);
  }

  // Preboot 直读因权限失败且没有缓存 → 给一个「提权重扫」按钮。
  // 弹一次 UAC（或 pkexec），后端用本程序自身跑一次只读扫描、缓存在进程里，
  // 完成后整页重载即可看到 macOS 条目 —— 不需要用户用管理员身份重启 App。
  if (preview.prebootElevatable && onPrebootElevate) {
    const row = document.createElement("div");
    row.className = "oc-menu-elevate";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "channel-helper-action needs-elev";
    button.textContent = "提权重扫 ⤴";
    button.title =
      "弹一次管理员授权，用 AnyBoot 自身只读扫一遍 APFS 容器翻出 macOS 条目" +
      "（不写盘、不装任何东西；结果缓存 30 分钟，之后不再询问）";
    button.disabled = Boolean(busy);
    button.addEventListener("click", () => onPrebootElevate(false));
    row.append(button);
    box.append(row);
  }

  if (preview.summary) {
    const summaryLine = document.createElement("small");
    summaryLine.className = "oc-menu-summary";
    summaryLine.textContent = `推算输入：${preview.summary}　·　准确率上限约 90%——固件到底枚举得到哪几个卷，只有在 UEFI 里才知道`;
    box.append(summaryLine);
  }

  return box;
}

/// 免密模式那一行（只出现在 OpenCore 卡片里 —— 只有它的「设为默认」需要写 NVRAM）。
///
/// 为什么值得占一行：一次「设为默认」要写 `OCBtOrder` + `OCBt0080` 两条变量，
/// 以前每条路径都要一次管理员密码。装上 root 助手后就不弹了，但代价是
/// `/Library` 里多一个守护项 —— 这是个需要用户自己决定要不要的取舍，
/// 所以把「现在是什么状态、开了以后少什么」直接写在卡片里，不藏在设置页。
function renderHelperLine(helper, { busy, onHelper } = {}) {
  if (!helper || !helper.supported) return null;

  const box = document.createElement("div");
  box.className = "channel-helper";
  box.dataset.state = helper.installed ? (helper.stale ? "stale" : "on") : "off";

  const label = document.createElement("span");
  label.className = "channel-helper-label";
  label.textContent = "免密模式";

  const text = document.createElement("small");
  text.className = "channel-helper-text";
  appendRichText(text, helper.text);

  const button = document.createElement("button");
  button.type = "button";
  button.className = "channel-helper-action needs-elev";
  // 三种状态给三个不同的动词：装上去的助手是 App 的副本，App 更新后它就成了旧版本，
  // 这时候最该做的不是「关闭」而是「重装」—— 否则它按旧协议继续工作，谁也看不出来。
  if (helper.installed && helper.stale) {
    button.textContent = "重装 ⤴";
    button.title =
      "把 /Library 里那份旧助手换成当前这个版本（一次管理员密码）；启动项设置不受影响";
  } else if (helper.installed) {
    button.textContent = "关闭";
    button.title =
      "删掉 /Library 里的 root 助手与信箱：之后「设为默认」恢复成每次输入一次密码";
  } else {
    button.textContent = "开启 ⤴";
    button.title =
      "安装时弹一次管理员密码；之后由 root 助手直接写 OCBt*，不再询问。" +
      "助手只在收到请求时被 launchd 唤起，不常驻、不开端口";
  }
  button.disabled = Boolean(busy);
  button.addEventListener("click", () => {
    const action = helper.installed && !helper.stale ? "uninstall" : "install";
    if (onHelper) onHelper(action);
  });

  box.append(label, text, button);
  return box;
}

/// 组一张通道卡片的 DOM（结构与 .grub/.refind 卡同构）
function makeBootChannelCard(
  channel,
  { busy, onSwitch, helper, onHelper, onPrebootElevate },
) {
  const section = document.createElement("section");
  section.className = `refind boot-channel ${channel.id}`;
  section.id = `channel-${channel.id}`;
  section.setAttribute("aria-label", `${channel.label} 启动项`);

  // ── 标题行：zone-tag 圆标 + 标题 + 状态 chip ──
  const titleRow = document.createElement("div");
  titleRow.className = "grub-title-row";

  const tag = document.createElement("span");
  tag.className = `zone-tag zone-${channel.id}`;
  tag.textContent = BOOT_CHANNEL_TAGS[channel.id] || `${channel.label} 区`;
  titleRow.append(tag);

  const heading = document.createElement("h2");
  heading.textContent = `${channel.label} 启动项`;
  titleRow.append(heading);

  const chip = document.createElement("span");
  chip.className = `channel-chip ${channel.chip === "ok" ? "ok" : channel.chip === "warn" ? "warn" : ""}`;
  chip.textContent = bootChannelChipText(channel);
  titleRow.append(chip);
  section.append(titleRow);

  // ── 摘要行（detail）──
  if (channel.detail) {
    const status = document.createElement("p");
    status.className = "channel-pane-status";
    status.textContent = channel.detail;
    section.append(status);
  }

  // ── 健康检查条（warn 黄 / 全绿 ✓）──
  const issues = channel.health || [];
  if (issues.length) {
    // severity 只有 warn 才算"要处理"；info 是"这台机器上为什么只能看不能改"的说明，
    // 用中性样式，不然卡片一看全是黄的、其实没坏。
    const ordered = [
      ...issues.filter((issue) => issue.severity === "warn"),
      ...issues.filter((issue) => issue.severity !== "warn"),
    ];
    const warnCount = ordered.filter((issue) => issue.severity === "warn").length;
    const health = document.createElement("div");
    health.className = warnCount ? "channel-health bad" : "channel-health info";
    const head = document.createElement("strong");
    // 「ℹ」不带变体选择符时多数系统字体只有一个细小的 i，看着像错字；
    // ⓘ 是稳定的圈 i，与 ⚠ / ✓ 同一档视觉重量。
    head.textContent = warnCount ? `⚠ 健康检查：${warnCount} 项建议处理` : "ⓘ 说明";
    health.append(head);
    for (const issue of ordered) {
      const line = document.createElement("span");
      // 「· 」前缀 + 文案；文案里的反引号片段渲染成 <code>（见 appendRichText）
      line.append(document.createTextNode("· "));
      appendRichText(line, issue.text);
      health.append(line);
    }
    section.append(health);
  } else if (channel.detected && channel.id !== "sdboot") {
    const health = document.createElement("div");
    health.className = "channel-health good";
    const head = document.createElement("strong");
    head.textContent = "✓ 健康检查通过";
    health.append(head);
    section.append(health);
  }

  // ── 预演 OC 菜单（只有 OpenCore 通道有）──
  if (channel.id === "opencore" && channel.menuPreview) {
    const preview = renderOcMenuPreview(channel.menuPreview, {
      busy,
      onSwitch,
      channel,
      onPrebootElevate,
    });
    if (preview) section.append(preview);
  }

  // ── 免密模式（只有 OpenCore 需要：它的「设为默认」要写 NVRAM）──
  if (channel.id === "opencore" && helper) {
    const line = renderHelperLine(helper, { busy, onHelper });
    if (line) section.append(line);
  }

  // ── 条目行 ──
  const list = document.createElement("section");
  list.className = "boot-list";
  list.setAttribute("aria-label", `${channel.label} 条目`);
  for (const entry of channel.entries || []) {
    const row = document.createElement("div");
    row.className = "boot-entry";
    if (
      channel.currentDefault &&
      (channel.currentDefault === entry.id ||
        channel.currentDefault.includes(entry.title))
    ) {
      row.classList.add("current");
    }

    const icon = document.createElement("span");
    icon.className = "boot-entry-ico";
    icon.textContent = BOOT_CHANNEL_ICONS[entry.icon] || "⚙️";
    row.append(icon);

    const meta = document.createElement("span");
    meta.className = "boot-entry-meta";
    const name = document.createElement("b");
    name.textContent = entry.title;
    const detail = document.createElement("small");
    detail.textContent = entry.detail;
    meta.append(name, detail);
    row.append(meta);

    // 「上次启动」徽章（记忆回读）：条目 id 与 lastBoot 对上才挂
    if (
      channel.lastBoot &&
      (channel.lastBoot === entry.id || channel.lastBoot.includes(entry.title))
    ) {
      const badge = document.createElement("span");
      badge.className = "boot-badge last";
      badge.textContent = "上次启动";
      row.append(badge);
    }
    if (
      channel.currentDefault &&
      (channel.currentDefault === entry.id ||
        channel.currentDefault.includes(entry.title))
    ) {
      const badge = document.createElement("span");
      badge.className = "boot-badge";
      badge.textContent = "当前默认";
      row.append(badge);
    }

    const button = document.createElement("button");
    button.type = "button";
    const elevated = Boolean(entry.requiresElevation);
    if (elevated) {
      button.classList.add("needs-elev");
    }
    // 「写入固件启动项」不是一次性启动动作，按钮文案要跟语义分开
    // （它要写 NVRAM，属于提权动作 —— 带 ⤴ 才和「下次启动」区分得开）。
    // 「给 ESP 分配盘符」同理：也不是启动动作，说的必须是它真正做的事。
    const isRepair = String(entry.id).endsWith("-register");
    const isMountEsp = String(entry.id).startsWith("mount-esp");
    button.textContent = isMountEsp
      ? elevated
        ? "分配盘符 ⤴"
        : "分配盘符"
      : isRepair
        ? elevated
          ? "写入固件 ⤴"
          : "写入固件"
        : elevated
          ? "下次启动 ⤴"
          : "下次启动";
    button.disabled = busy;
    button.addEventListener("click", () => onSwitch(entry, channel));
    row.append(button);
    list.append(row);
  }
  if ((channel.entries || []).length) {
    section.append(list);
  }
  return section;
}

/// 通道切换的 spec 组装（与后端 parse_switch_spec 的值域一致）。
/// 预演行自带后端算好的完整值域（`entry.spec`，形如 `opencore-default:<卷>\u0001<路径>`）
/// —— 前端不拼这个字符串；其余条目：sdboot → `sdboot:<id>`；
/// opencore 的条目 id 本身就是值域（`opencore` 进菜单 / `opencore-register` 修复自注册）；
/// clover 动作 → `clover-oneshot`（文件路线 clover-default:<volume> 走远程 API）。
function bootChannelSpec(entry, channel) {
  if (entry && entry.spec) return entry.spec;
  if (channel.id === "sdboot") return `sdboot:${entry.id}`;
  if (channel.id === "opencore") return entry.id;
  if (channel.id === "clover") return "clover-oneshot";
  return entry.id;
}

function renderBootChannels(channels, { busy, onSwitch, helper, onHelper, onPrebootElevate }) {
  const mount = document.getElementById("boot-channels-mount");
  const collapsed = document.getElementById("boot-channel-collapsed");
  const collapsedNames = document.getElementById("boot-channel-collapsed-names");
  if (!mount || !collapsed || !collapsedNames) return;

  mount.replaceChildren();
  const detected = (channels || []).filter((channel) => channel.detected);
  // 未检测到、但后端**查得到具体原因**的（目前只有 Windows 的「ESP 没有盘符」）：
  // 照样渲染成卡片。否则用户只能看到折叠条里一行「OpenCore（未检测到）」，
  // 完全不知道该做什么 —— 而这恰恰是唯一靠自己能修好的情况。
  const explained = (channels || []).filter(
    (channel) => !channel.detected && (channel.health || []).length > 0,
  );
  const missing = (channels || []).filter(
    (channel) => !channel.detected && (channel.health || []).length === 0,
  );

  for (const channel of [...detected, ...explained]) {
    mount.append(
      makeBootChannelCard(channel, {
        busy,
        onSwitch,
        helper,
        onHelper,
        onPrebootElevate,
      }),
    );
  }
  // 折叠条只在「确实还有没说的」时出现；全部未检测到也要出现 —— 否则
  // 「一张 explained 卡片 + 两条彻底消失的通道」会让人以为后两个压根不存在。
  collapsed.hidden = missing.length === 0;
  collapsedNames.textContent = missing.map((channel) => channel.label).join("、")
    ? `${missing.map((channel) => channel.label).join("、")}（未检测到）`
    : "";
}

/// 页首「下次启动」摘要条的「上次启动」位：三通道的 lastBoot 挑一个有值的展示。
function renderNextBootLast(channels) {
  const element = document.getElementById("next-boot-last");
  if (!element) return;
  const found = (channels || []).find(
    (channel) => channel.detected && channel.lastBoot,
  );
  if (!found) {
    element.hidden = true;
    return;
  }
  element.hidden = false;
  const strong = element.querySelector("b");
  if (strong) {
    strong.textContent = `${found.label} · ${found.lastBoot}`;
  }
}
