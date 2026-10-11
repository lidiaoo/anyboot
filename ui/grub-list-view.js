// GRUB 启动项列表的渲染逻辑：主窗口内嵌列表与独立跟随窗口共用这一份。
//
// grub.cfg 里的 submenu 是分组、不是可选条目（后端解析器按同一规则跳过它），
// 所以这里**自己把组头补出来**：子条目归到它的父 submenu 下，并统计组内数量。
// 后端同时给了 submenu 的 (id → 标题) 映射，组头就能显示真名而不是一串 id。
// 同名条目已经在后端按 (父 submenu, 标题) 折叠过了：标签里标出变体数。
"use strict";

/// 把「当前选中的启动项」渲染进顶部横幅：级联条目显示「父级名 › 子级名」，
/// 最后一级用大字 + 主题色，父级用小一号的灰字 —— 三种展示方式共用。
function renderGrubActive(container, grub) {
  const entries = grub.entries || [];
  const groups = new Map((grub.groups || []).map((group) => [group.id, group.name]));
  // 与列表的高亮规则一致：优先一次性项（nextEntry），其次永久默认（savedEntry）
  const activeSelector = grub.nextEntry || grub.savedEntry || "";
  const active = entries.find((entry) => entry.selector === activeSelector);

  container.replaceChildren();
  if (!active) {
    if (grub.uuid) {
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

function renderGrubEntries({ container, grub, busy, onSwitch }) {
  const entries = grub.entries || [];
  // 当前生效的条目：优先看一次性项，其次看永久默认（都来自 ESP 上的环境块）
  const activeSelector = grub.nextEntry || grub.savedEntry || "";

  container.replaceChildren();
  if (!entries.length) {
    const empty = document.createElement("small");
    empty.className = "grub-empty";
    empty.textContent = grub.uuid
      ? "还没有可显示的启动项 —— 先在 Linux 上装好 09_anyboot 并同步一次"
      : "填好 ESP 目录（点「识别」）或 fs-uuid，就能列出 GRUB 启动项";
    container.append(empty);
    return;
  }

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

  const makeRow = (entry, { group, depth }) => {
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
    row.disabled = busy;
    row.title = entry.selector || "";

    const name = document.createElement("span");
    name.className = "grub-entry-name";
    // 缩进靠 CSS（.is-child / .depth-N），这里只用箭头补强「这是组里的项」
    name.textContent = group || depth === 0 ? entry.name : `↳ ${entry.name}`;

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

  // 顶层条目按原顺序铺开；走到第一条子项时，先把它的组头插进去，再补齐同组子项。
  const emittedGroups = new Set();
  for (const entry of entries) {
    if (!entry.submenu) {
      container.append(makeRow(entry, { group: false, depth: 0 }));
      continue;
    }
    if (emittedGroups.has(entry.submenu)) {
      continue;
    }
    emittedGroups.add(entry.submenu);
    container.append(
      makeRow(
        {
          selector: entry.submenu,
          name: groupTitles.get(entry.submenu) || entry.submenu,
        },
        { group: true, depth: 1 },
      ),
    );
    for (const child of childrenOf.get(entry.submenu) || []) {
      container.append(makeRow(child, { group: false, depth: 1 }));
    }
  }
}
