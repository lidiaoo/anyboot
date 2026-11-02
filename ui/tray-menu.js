const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

/*
 * 自绘托盘菜单。
 *
 * 后端给的是**纯数据模型**（`src-tauri/src/lib.rs` 的 `build_menu_nodes`）：
 *   { id, label, enabled, kind: item|header|sep|sub, checked?, keepsOpen, children[] }
 * 这里只负责画，以及把点击按 id 送回后端（id 语义与原来的原生菜单完全一致 ——
 * 分发还是同一个 `handle_menu_event`）。
 *
 * 两条产品定则落在数据结构里，不在这个文件里各算一遍：
 *   ① 勾/单选态一律是**标签文字**（后端已经写好 ☑/☐/◉/○），这里不额外画控件。
 *   ② 点「带选择框」的项**不关菜单**：模型给每条标了 `keepsOpen`，关不关窗由后端
 *      （`tray_menu_click` 里查同一个字段）说了算。前端只做按下反馈。
 *
 * 子菜单（固定显示位置 / 固定显示器 / 点击❌时 / 延迟）**在窗内展开**而不是悬停弹出：
 * 展开后一组里「恰好一个 ◉」一眼可见，还能连着点几项都不用重开菜单（这正是 ③ 的收益）。
 * 默认收起，所以菜单高度跟原来的原生菜单一样（列表太长会顶出屏幕）。
 */

const menuElement = document.querySelector("#menu");
const CHEVRON_CLOSED = "▸";
const CHEVRON_OPEN = "▾";

/// 当前内容（后端推来或拉来的那份）
let nodes = [];
/// 展开了哪个子菜单（一次只开一个）：值是 sub 节点的 id
let expandedGroup = null;
/// 上次报给后端的高度：内容没变就不折腾窗口（改尺寸 + 重新摆位有代价，
/// 而且窗口正在被点的时候挪位置会很难受）。
let presentedHeight = null;
/// 后端「本次打开」的序号：变了说明这是**新开的一次**，哪怕高度与上次一样也要重新呈现
/// （不然重开菜单时窗口永远不显示）。见 Rust 侧 `TrayMenuRuntime::seq`。
let lastSeq = null;

function elementWithClass(className, text) {
  const element = document.createElement("div");
  element.className = className;
  if (text !== undefined) element.textContent = text;
  return element;
}

/// 列表类条目的「当前项」标记：模型给的是布尔，画成 ◉/○（与设置项同一套标记）
function checkMark(node) {
  if (node.checked === true) return "◉ ";
  if (node.checked === false) return "○ ";
  return "";
}

function itemRow(node) {
  const row = elementWithClass("row");
  if (!node.enabled) row.classList.add("disabled");
  // keepsOpen 只用来给「点完不关」的项一点按下反馈：动作项点完菜单就没了，
  // 本身就是反馈；留着菜单的项得让用户知道这一下点上了。
  if (node.keepsOpen) row.classList.add("choice");
  // textContent 会保留 \n（CSS white-space: pre-line）—— 顶部「下次启动」是两行
  row.textContent = checkMark(node) + node.label;
  if (node.enabled && node.id) {
    // 用 mousedown 而不是 click：点下去立刻响应，也免得先触发一次焦点抖动
    row.addEventListener("mousedown", (event) => {
      event.preventDefault();
      void invoke("tray_menu_click", { id: node.id });
    });
  }
  return row;
}

function groupRow(node) {
  const open = expandedGroup === node.id;
  const row = elementWithClass("group-row");
  if (open) row.classList.add("open");
  row.textContent = `${open ? CHEVRON_OPEN : CHEVRON_CLOSED} ${node.label}`;
  row.addEventListener("mousedown", (event) => {
    // 展开/收起不是动作，不关菜单
    event.preventDefault();
    expandedGroup = open ? null : node.id;
    render();
  });
  return row;
}

function render() {
  menuElement.textContent = "";
  for (const node of nodes) {
    if (node.kind === "sep") {
      menuElement.appendChild(elementWithClass("sep"));
      continue;
    }
    if (node.kind === "header") {
      menuElement.appendChild(elementWithClass("header", node.label));
      continue;
    }
    if (node.kind === "sub") {
      menuElement.appendChild(groupRow(node));
      if (expandedGroup === node.id) {
        const box = elementWithClass("children");
        for (const child of node.children || []) box.appendChild(itemRow(child));
        menuElement.appendChild(box);
      }
      continue;
    }
    menuElement.appendChild(itemRow(node));
  }
  void presentWindow();
}

/// 收下一整份内容并重画。
function apply(payload) {
  const seq = payload && payload.seq;
  if (seq !== lastSeq) {
    lastSeq = seq;
    // 新开的一次：高度必须重新报一遍（哪怕跟上次一样），否则窗口不会显示
    presentedHeight = null;
  }
  nodes = (payload && payload.nodes) || [];
  applyTheme(payload && payload.theme);
  // 展开的那个组在新内容里没了（GRUB 区切换、显示器拔了…）就自动收起来
  if (expandedGroup && !nodes.some((node) => node.kind === "sub" && node.id === expandedGroup)) {
    expandedGroup = null;
  }
  render();
}

/// 量出内容自然高度（不减去滚动：`scrollHeight` 给的是整块内容）。
/// 隐藏期的 WebView 在 Linux/GTK 上视口是 0x0，但 `scrollHeight` 只取决于内容宽度，
/// 所以这里量得到；万一量不到（0/NaN）就不报，让后端按标称高度兜底。
async function presentWindow() {
  const height = Math.ceil(menuElement.scrollHeight);
  if (!Number.isFinite(height) || height <= 0) return;
  if (height === presentedHeight) return;
  presentedHeight = height;
  try {
    await invoke("tray_menu_present", { height });
  } catch (error) {
    void error;
  }
}

// 窗口被后端改过尺寸后重测一次（显示后视口才是真的，用来自愈隐藏期量不准的情况）
window.addEventListener("resize", () => {
  presentedHeight = null;
  void presentWindow();
});

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    event.preventDefault();
    void invoke("tray_menu_close");
  }
});

// 点了菜单里的**空白**（分隔线、分组标题、条目之间的空隙）→ 收起菜单 —— 与点菜单
// 外面的行为一致（菜单本来就该这样）。点在条目上行不行由条目自己的 mousedown 决定，
// 事件会冒泡到这里，所以用 closest 判断落点。
document.addEventListener("mousedown", (event) => {
  if (event.target.closest(".row, .group-row")) return;
  event.preventDefault();
  void invoke("tray_menu_close");
});

/// 主题：跟随主界面的亮/暗（后端给的是 light / dark，与主界面同一套取值）
function applyTheme(theme) {
  const resolved =
    theme === "light"
      ? "light"
      : theme === "dark"
        ? "dark"
        : window.matchMedia("(prefers-color-scheme: light)").matches
          ? "light"
          : "dark";
  document.documentElement.dataset.theme = resolved;
}

// theme-changed 广播（payload 就是主题原始取值）：菜单开着时主题改了立即跟随
function applyThemeValue(theme) {
  const resolved =
    theme === "light"
      ? "light"
      : theme === "dark"
        ? "dark"
        : window.matchMedia("(prefers-color-scheme: light)").matches
          ? "light"
          : "dark";
  document.documentElement.dataset.theme = resolved;
}

(async () => {
  // 后端每次「开菜单」都会推一份最新内容；菜单开着而状态变了也会再推一次
  await listen("tray-menu-nodes", (event) => {
    apply(event.payload);
  });
  // 主题广播：任何入口改了主题，开着的菜单立即跟随
  await listen("theme-changed", (event) => {
    applyThemeValue(event.payload);
  });
  // 首次打开时窗口可能刚建出来、上面那条事件已经发过去了 —— 主动拉一次兜住
  try {
    apply(await invoke("tray_menu_nodes"));
  } catch (error) {
    void error;
  }
})();
