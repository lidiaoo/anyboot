// GRUB 启动项列表的渲染逻辑：右侧 GRUB 专属区（.pane-grub）用这一份。
//
// grub.cfg 里的 submenu 是分组、不是可选条目（后端解析器按同一规则跳过它），
// 所以这里**自己把组头补出来**：子条目归到它的父 submenu 下，并统计组内数量。
// 后端同时给了 submenu 的 (id → 标题) 映射，组头就能显示真名而不是一串 id。
// 同名条目已经在后端按 (父 submenu, 标题) 折叠过了：标签里标出变体数。
//
// 列表格式（用户定案 2026-10-12，快照里的 grub.listFormat）：
// * raw（原始）：现版样式 —— 组头 + 条目名，右侧小字标内核/变体；
// * minimal（极简）/ simple（简易）：条目显示**去前缀内核名**
//   （vmlinuz-6.18-x86_64 → 6.18-x86_64；vmlinuz-linux-cachyos-rc → cachyos-rc），
//   简易的组头额外带 ESP 位置（位于… UUID=…）；
// * medium（中等）：条目显示完整内核名（vmlinuz-…），组头带位置；
// * detailed（复杂）：条目完整名称居左、内核居右，组头带位置。
// 组头按关键字（windows / mac / linux）在名字前加系统图标。
"use strict";

/// 按关键字给条目/组名挑系统图标：windows → 🪟，mac → 🍎，linux → 🐧，
/// 都不中就不加图标（不猜）。
function grubOsIcon(text) {
  const lowered = (text || "").toLowerCase();
  if (lowered.includes("windows")) return "🪟 ";
  if (lowered.includes("mac") || lowered.includes("apple")) return "🍎 ";
  if (lowered.includes("linux")) return "🐧 ";
  return "";
}

/// 条目的内核名：优先取 `linux` 行解析出的内核文件名，取不到退回标题原文。
function grubKernelOf(entry) {
  return (entry.kernels && entry.kernels[0]) || entry.name || "";
}

/// 去内核名前缀：先去 `vmlinuz-`，再去 `linux-`
/// （vmlinuz-linux-cachyos-rc → linux-cachyos-rc → cachyos-rc）。
function grubStripKernel(name) {
  return (name || "").replace(/^vmlinuz-/, "").replace(/^linux-/, "");
}

/// 把「当前选中的启动项」渲染进顶部横幅：级联条目显示「父级名 › 子级名」，
/// 最后一级用大字 + 主题色，父级用小一号的灰字。
function renderGrubActive(container, grub) {
  const entries = grub.entries || [];
  const groups = new Map((grub.groups || []).map((group) => [group.id, group.name]));
  // 与列表的高亮规则一致：优先一次性项（nextEntry），其次永久默认（savedEntry）
  const activeSelector = grub.nextEntry || grub.savedEntry || "";
  const active = entries.find((entry) => entry.selector === activeSelector);

  container.replaceChildren();
  if (!active) {
    if (grub.partuuid) {
      const none = document.createElement("small");
      none.className = "grub-active-none";
      none.textContent = "尚未选择启动项";
      container.append(none);
    }
    return;
  }
  if (active.submenu) {
    const parent = document.createElement("span");
    parent.className = "grub-active-parent";
    parent.textContent = `${groups.get(active.submenu) || active.submenu} ›`;
    container.append(parent);
  }
  const leaf = document.createElement("span");
  leaf.className = "grub-active-leaf";
  leaf.textContent = active.name;
  container.append(leaf);
}

function renderGrubEntries({ container, grub, busy, onSwitch, format = "raw" }) {
  const entries = grub.entries || [];
  // 当前生效的条目：优先看一次性项，其次看永久默认（都来自 ESP 上的环境块）
  const activeSelector = grub.nextEntry || grub.savedEntry || "";

  container.replaceChildren();
  if (!entries.length) {
    const empty = document.createElement("small");
    empty.className = "grub-empty";
    empty.textContent = grub.partuuid
      ? "还没有可显示的启动项 —— 先在 Linux 上装好 09_anyboot 并同步一次"
      : "填好 ESP 目录（点「识别」）或分区 UUID，就能列出 GRUB 启动项";
    container.append(empty);
    return;
  }

  if (format === "raw") {
    renderRawEntries({ container, grub, busy, onSwitch, entries, activeSelector });
    return;
  }
  renderFormatEntries({ container, grub, busy, onSwitch, entries, activeSelector, format });
}

/// 原始格式：现版样式，一字未动。
function renderRawEntries({ container, grub, busy, onSwitch, entries, activeSelector }) {
  const groupTitles = new Map(
    (grub.groups || []).map((group) => [group.id, group.name]),
  );
  const childrenOf = new Map();
  for (const entry of entries) {
    if (entry.submenu) {
      if (!childrenOf.has(entry.submenu)) {
        childrenOf.set(entry.submenu, []);
      }
      childrenOf.get(entry.submenu).push(entry);
    }
  }

  const makeRow = (entry, { group, depth, pinned = false }) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "grub-entry";
    row.classList.add(group ? "is-group" : "is-child");
    if (depth >= 2) {
      row.classList.add(`depth-${depth}`);
    }
    if (entry.selector === activeSelector) {
      row.classList.add("active");
    }
    if (pinned) {
      row.classList.add("is-pinned");
    }
    row.disabled = busy;
    row.title = entry.selector || "";

    const name = document.createElement("span");
    name.className = "grub-entry-name";
    // 置顶行脱离了它原来的组，名字里把组名带上，不然看不出它属于哪个 submenu
    const prefix = pinned && entry.submenu
      ? `${groupTitles.get(entry.submenu) || entry.submenu} › `
      : "";
    // 缩进靠 CSS（.is-child / .depth-N），这里只用箭头补强「这是组里的项」
    name.textContent =
      prefix + (group || depth === 0 ? entry.name : `↳ ${entry.name}`);

    const tag = document.createElement("small");
    if (group) {
      tag.className = "grub-entry-count";
      tag.textContent = `${(childrenOf.get(entry.selector) || []).length} 项`;
      // 组头不是 menuentry，写进 grubenv 也切不过去 —— 只作标题，不可点
      row.disabled = true;
    } else {
      const variants = Number(entry.variants) || 1;
      const parts = [];
      if (variants > 1) {
        parts.push(`${variants} 个变体`);
      }
      parts.push(
        entry.chainloader
          ? "转交引导"
          : (entry.kernels && entry.kernels[0]) || entry.class || "",
      );
      tag.className = "grub-entry-tag";
      tag.textContent = parts.filter(Boolean).join(" · ");
    }

    row.append(name, tag);
    if (!group) {
      row.addEventListener("click", () => onSwitch(entry.selector));
    }
    return row;
  };

  // 当前选中的启动项置顶固定：渲染成 .grub-list 下的独立块（CSS sticky 钉在
  // 滚动容器顶部，怎么滚都不动），正式列表里跳过它 —— 同一条目只出现一次。
  // sticky 必须脱离 grid 流：grid 项的 sticky 只能在自己的格子里挪，出不了那一行，
  // 所以真正的 grid 挪进 .grub-list-body，置顶行做 .grub-list（block）的兄弟块。
  const body = document.createElement("div");
  body.className = "grub-list-body";
  // 选中的条目可以是顶层，也可以是组内子项 —— 只按 selector 找，
  // 不排除子项（组头是渲染时合成的标题，selector 是 submenu id，
  // 不会与 menuentry 的 selector 撞上）。
  const activeIndex = entries.findIndex((entry) => entry.selector === activeSelector);
  if (activeIndex >= 0) {
    container.append(makeRow(entries[activeIndex], { group: false, depth: 0, pinned: true }));
  }
  container.append(body);

  // 顶层条目按原顺序铺开；走到第一条子项时，先把它的组头插进去，再补齐同组子项。
  const emittedGroups = new Set();
  entries.forEach((entry, index) => {
    if (index === activeIndex) {
      return;
    }
    if (!entry.submenu) {
      body.append(makeRow(entry, { group: false, depth: 0 }));
      return;
    }
    if (emittedGroups.has(entry.submenu)) {
      return;
    }
    emittedGroups.add(entry.submenu);
    body.append(
      makeRow(
        {
          selector: entry.submenu,
          name: groupTitles.get(entry.submenu) || entry.submenu,
        },
        { group: true, depth: 1 },
      ),
    );
    for (const child of childrenOf.get(entry.submenu) || []) {
      if (entries.indexOf(child) === activeIndex) {
        continue;
      }
      body.append(makeRow(child, { group: false, depth: 1 }));
    }
  });
}

/// 极简 / 简易 / 中等 / 复杂四种格式：同一个分组走法（置顶、组头、子项），
/// 只有「条目文字、右侧小字、组头附加信息」随格式变。
function renderFormatEntries({ container, grub, busy, onSwitch, entries, activeSelector, format }) {
  const groupTitles = new Map(
    (grub.groups || []).map((group) => [group.id, group.name]),
  );
  const childrenOf = new Map();
  for (const entry of entries) {
    if (entry.submenu) {
      if (!childrenOf.has(entry.submenu)) {
        childrenOf.set(entry.submenu, []);
      }
      childrenOf.get(entry.submenu).push(entry);
    }
  }

  // 组头附加信息（简易/中等/复杂）：ESP 挂载点 + 分区 UUID（取不到的段省略）
  const showLocation = format !== "minimal";
  const locationText = (() => {
    if (!showLocation) return "";
    const parts = [];
    if (grub.efiDir) parts.push("位于" + grub.efiDir);
    const uuid =
      grub.partuuid || (grub.esp && grub.esp.partuuid) || (grub.esp && grub.esp.uuid);
    if (uuid) parts.push("UUID=" + uuid);
    return parts.length ? " " + parts.join(" ") : "";
  })();

  // 条目文字随格式变：
  // * 极简/简易：去前缀内核名；中等：完整内核名；复杂：完整名称（菜单标题）
  const rowLabel = (entry) => {
    if (format === "minimal" || format === "simple") {
      return grubStripKernel(grubKernelOf(entry));
    }
    if (format === "medium") {
      return grubKernelOf(entry);
    }
    return entry.name;
  };
  // 右侧小字只在复杂格式出现：内核（没有内核的转交引导/类别照旧）
  const rowTag = (entry) => {
    if (format !== "detailed") return "";
    if (entry.kernels && entry.kernels[0]) return "内核 " + entry.kernels[0];
    return entry.chainloader ? "转交引导" : entry.class || "";
  };

  const makeRow = ({ text, icon, tag, depth, pinned, selector, clickable, title }) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "grub-entry";
    // depth = -1 是组头（is-group），其余按缩进深度给 is-child / depth-N
    row.classList.add(depth < 0 ? "is-group" : "is-child");
    if (depth >= 2) {
      row.classList.add("depth-" + depth);
    }
    if (selector && selector === activeSelector) {
      row.classList.add("active");
    }
    if (pinned) {
      row.classList.add("is-pinned");
    }
    row.disabled = busy || !clickable;
    if (title) {
      row.title = title;
    }

    const name = document.createElement("span");
    name.className = "grub-entry-name";
    const arrow = pinned ? "" : depth < 0 ? "▸ " : "↳ ";
    name.textContent = arrow + (icon || "") + text;
    row.append(name);

    if (tag) {
      const tagElement = document.createElement("small");
      tagElement.className = depth < 0 ? "grub-entry-count" : "grub-entry-tag";
      tagElement.textContent = tag;
      row.append(tagElement);
    }
    if (clickable && selector) {
      row.addEventListener("click", () => onSwitch(selector));
    }
    return row;
  };

  // 组头（不可点）：▸ 图标 组名 居左，右侧「N 项[ 位置]」
  const makeHeader = (submenuId) => {
    const title = groupTitles.get(submenuId) || submenuId;
    const count = (childrenOf.get(submenuId) || []).length;
    return makeRow({
      text: title,
      icon: grubOsIcon(title),
      tag: count + " 项" + locationText,
      depth: -1,
      selector: submenuId,
      clickable: false,
      title: "",
    });
  };

  // 条目行：极简/简易/中等无右侧小字，复杂有「内核 …」
  const makeEntry = (entry, { depth, pinned = false }) => {
    const ownTitle = entry.name || "";
    // 置顶行脱离了它原来的组，名字里把组名带上
    const prefix =
      pinned && entry.submenu
        ? (groupTitles.get(entry.submenu) || entry.submenu) + " › "
        : "";
    const icon = grubOsIcon(prefix || ownTitle);
    return makeRow({
      text: prefix + rowLabel(entry),
      icon,
      tag: rowTag(entry),
      depth,
      pinned,
      selector: entry.selector,
      clickable: true,
      title: entry.selector || "",
    });
  };

  const body = document.createElement("div");
  body.className = "grub-list-body";
  // 选中条目置顶（与原始格式同一规则），正式列表里跳过
  const activeIndex = entries.findIndex((entry) => entry.selector === activeSelector);
  if (activeIndex >= 0) {
    container.append(makeEntry(entries[activeIndex], { depth: 0, pinned: true }));
  }
  container.append(body);

  const emittedGroups = new Set();
  entries.forEach((entry, index) => {
    if (index === activeIndex) {
      return;
    }
    if (!entry.submenu) {
      // 顶层条目：图标按自己的名字挑，深度 0（无 ↳ 前缀）
      body.append(makeEntry(entry, { depth: 0 }));
      return;
    }
    if (emittedGroups.has(entry.submenu)) {
      return;
    }
    emittedGroups.add(entry.submenu);
    body.append(makeHeader(entry.submenu));
    for (const child of childrenOf.get(entry.submenu) || []) {
      if (entries.indexOf(child) === activeIndex) {
        continue;
      }
      body.append(makeEntry(child, { depth: 1 }));
    }
  });
}
