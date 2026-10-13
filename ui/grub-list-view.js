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
  // 选中条目**只置顶、不从原位删除**（用户要求：原位留着才找得到上下文），
  // 原位行照常带 active 高亮。
  const emittedGroups = new Set();
  entries.forEach((entry) => {
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

  // 组头（不可点）：▸ 图标 组名 居左，右侧「N 项[ 位置]」—— N 按去重后的行数
  const makeHeader = (submenuId, children) => {
    const title = groupTitles.get(submenuId) || submenuId;
    const distinct = new Set(children.map(labelOf)).size;
    return makeRow({
      text: title,
      icon: grubOsIcon(title),
      tag: distinct + " 项" + locationText,
      depth: -1,
      selector: submenuId,
      clickable: false,
      title: "",
    });
  };

  // 条目行：极简/简易/中等无右侧小字（有变体时标「N 个变体」），复杂有「内核 …」
  const makeEntry = (entry, { depth, pinned = false, tag: tagOverride }) => {
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
      tag: tagOverride !== undefined ? tagOverride : rowTag(entry),
      depth,
      pinned,
      selector: entry.selector,
      clickable: true,
      title: entry.selector || "",
    });
  };

  // 相同渲染结果折叠（用户指出：极简/简易/中等按内核名出行，同一内核的
  // fallback / initramfs 变体渲染结果一模一样，不折叠就是一列重复行）。
  // 组内按渲染结果计数：第一个条目代表出面，其余并入「N 个变体」。
  // 选中条目只置顶、原位照常出行（用户要求：原位留着才找得到上下文），
  // 所以计数就是列表里的真实条数。复杂格式的行文字就是菜单标题
  // （后端已按 (组,标题) 折叠），这里照走一遍只是防御。
  const labelOf = rowLabel;
  const collapseScope = (list) => {
    const counts = new Map();
    for (const entry of list) {
      const label = labelOf(entry);
      counts.set(label, (counts.get(label) || 0) + 1);
    }
    return counts;
  };
  const emitEntry = (entry, depth, counts, emitted) => {
    const label = labelOf(entry);
    if (emitted.has(label)) {
      return;
    }
    emitted.add(label);
    const variants = counts.get(label) || 1;
    const tag =
      format === "detailed"
        ? rowTag(entry)
        : variants > 1
          ? `${variants} 个变体`
          : "";
    body.append(makeEntry(entry, { depth, tag }));
  };

  const body = document.createElement("div");
  body.className = "grub-list-body";
  // 选中条目置顶（与原始格式同一规则），正式列表里跳过
  const activeIndex = entries.findIndex((entry) => entry.selector === activeSelector);
  if (activeIndex >= 0) {
    container.append(makeEntry(entries[activeIndex], { depth: 0, pinned: true }));
  }
  container.append(body);

  // 顶层条目共享一套去重状态（同名/同内核不管出现在列表哪个位置都只出一行）；
  // 各组一套独立的。选中条目置顶之外原位照常出行。
  const topLevel = entries.filter((entry) => !entry.submenu);
  const topCounts = collapseScope(topLevel);
  const topEmitted = new Set();

  const emittedGroups = new Set();
  entries.forEach((entry) => {
    if (!entry.submenu) {
      emitEntry(entry, 0, topCounts, topEmitted);
      return;
    }
    if (emittedGroups.has(entry.submenu)) {
      return;
    }
    emittedGroups.add(entry.submenu);
    const children = childrenOf.get(entry.submenu) || [];
    body.append(makeHeader(entry.submenu, children));
    const counts = collapseScope(children);
    const emitted = new Set();
    for (const child of children) {
      emitEntry(child, 1, counts, emitted);
    }
  });
}
