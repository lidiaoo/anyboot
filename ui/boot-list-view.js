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
/// sdboot 条目 → `sdboot:<id>`；opencore 的条目 id 本身就是值域
/// （`opencore` 进菜单 / `opencore-register` 修复自注册配置）；
/// clover 动作 → `clover-oneshot`（文件路线 clover-default:<volume> 走远程 API）。
function bootChannelSpec(entry, channel) {
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
