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

const OC_MENU_ICONS = {
  macos: "🍎",
  windows: "🪟",
  generic: "⬜",
  linux: "🐧",
  tool: "⚙️",
  unknown: "❓",
};

/// 预演里的一行（卷 + 路径 + 来源 + 备注 + 「设为默认」）
function ocMenuRow(entry, index, { busy, onSwitch, channel } = {}) {
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
  source.textContent = OC_MENU_SOURCE_LABEL[entry.source] || entry.source;
  head.append(source);

  if (entry.default) {
    const badge = document.createElement("span");
    badge.className = "oc-menu-default";
    badge.textContent = "OC 默认项";
    head.append(badge);
  }
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
  if (entry.spec && !entry.default) {
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

/// 整块「预演 OC 菜单」（details，默认折叠，不抢条目行的注意力）
function renderOcMenuPreview(preview, { busy, onSwitch, channel } = {}) {
  if (!preview) return null;
  const box = document.createElement("details");
  box.className = "oc-menu";

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
  hint.textContent =
    "OC 不遍历目录：它按固定规则找（BootOrder + 每个卷的 bless/预定义路径 + 条目协议驱动）。" +
    "这里是按同一套规则纯读复算，不写盘、不改 NVRAM。" +
    "「设为默认」等价于进 OC 菜单对目标按 Ctrl+Enter —— 改的是 OC 私有命名空间的 " +
    "OCBtOrder / OCBt0080（macOS 不许写 Boot*，但这一组可以），写前整份备份。";
  box.append(hint);

  if ((preview.entries || []).length) {
    const list = document.createElement("div");
    list.className = "oc-menu-list";
    preview.entries.forEach((entry, index) =>
      list.append(ocMenuRow(entry, index, { busy, onSwitch, channel })),
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
      line.textContent = `${issue.severity === "warn" ? "⚠" : "ℹ"} ${issue.text}`;
      issueBox.append(line);
    }
    box.append(issueBox);
  }

  if (preview.summary) {
    const summaryLine = document.createElement("small");
    summaryLine.className = "oc-menu-summary";
    summaryLine.textContent = `推算输入：${preview.summary}　·　准确率上限约 90%——固件到底枚举得到哪几个卷，只有在 UEFI 里才知道`;
    box.append(summaryLine);
  }

  return box;
}

/// 组一张通道卡片的 DOM（结构与 .grub/.refind 卡同构）
function makeBootChannelCard(channel, { busy, onSwitch }) {
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
    head.textContent = warnCount ? `⚠ 健康检查：${warnCount} 项建议处理` : "ℹ 说明";
    health.append(head);
    for (const issue of ordered) {
      const line = document.createElement("span");
      line.textContent = `· ${issue.text}`;
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
    });
    if (preview) section.append(preview);
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
    // （它要写 NVRAM，属于提权动作 —— 带 ⤴ 才和「下次启动」区分得开）
    const isRepair = String(entry.id).endsWith("-register");
    button.textContent = isRepair
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

function renderBootChannels(channels, { busy, onSwitch }) {
  const mount = document.getElementById("boot-channels-mount");
  const collapsed = document.getElementById("boot-channel-collapsed");
  const collapsedNames = document.getElementById("boot-channel-collapsed-names");
  if (!mount || !collapsed || !collapsedNames) return;

  mount.replaceChildren();
  const detected = (channels || []).filter((channel) => channel.detected);
  const missing = (channels || []).filter((channel) => !channel.detected);

  for (const channel of detected) {
    mount.append(makeBootChannelCard(channel, { busy, onSwitch }));
  }
  collapsed.hidden = missing.length === 0 || detected.length === 0;
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
