# 新旧界面比对工具

用于验证 `ui-vue/` 与旧版 `ui/`（commit `7bd3ce2`）的 1:1 还原度。

## 前置

```bash
# 1. 从旧 commit 取出旧版前端
mkdir -p /tmp/cmp/old
git archive 7bd3ce2 ui/ | tar -x -C /tmp/cmp/tmp-old
mv /tmp/cmp/tmp-old/ui/* /tmp/cmp/old/

# 2. 构建新版
npm run build:vue     # → ui-vue/dist

# 3. 真实快照（做 mock 数据）
TOKEN=$(python3 -c "import json;print(json.load(open('$HOME/Library/Application Support/com.example.anyboot/settings.json'))['httpToken'])")
curl -s "http://127.0.0.1:12300/api/ui-state?token=$TOKEN" > /tmp/cmp/uistate.json
# 再按 README 里的 python 片段合成 mockdata.json

# 4. 起对比服务
python3 serve.py      # http://127.0.0.1:8770/old/...  /new/...
```

## 验证脚本一览

| 脚本 | 层级 | 作用 |
|---|---|---|
| `render-check.js` | 第 0 层 | 入口是否指向真实实现（不是占位页） |
| `dom-diff.js` | 第 1 层 | 逐元素比对 class / id（只看可见元素） |
| `verify-all.js` | — | 各页签可见元素数 + JS 异常 |
| `smoke.js` | 第 4 层 | 自动点所有按钮，抓未捕获异常 |
| `style-diff.js` | 第 5 层 | 计算样式逐项比对 |
| `dialog-diff.js` | 第 6 层 | 弹层尺寸 / 控件数比对 |
| `theme-render.js` | 第 7 层 | 6 套主题渲染内容 |
| `theme-func.js` | 第 7 层 | 主题交互（搜索/筛选/选中） |
| `func1..4.js` | 第 10 层 | 功能抽查（点了有没有效果） |
| `功能回归.sh` | — | 一次跑完 func1~4 + 主题 |

## 关键经验

1. **DOM 比对必须只看可见元素** —— 旧版用 `hidden` 把 12 个页面都留在 DOM 里，
   新版用 `v-if` 只挂载当前页；不过滤会得出「每页缺 300 个类」的假象。
2. **比对 key 用「父级 tag.class > 自身 tag.class」** —— 只用 class 名会跨元素误配，
   用完整路径又太严（一处差异就全部失配）。
3. **mock 不完整会制造假 bug** —— 例如 `backup_sync_set_enabled` 只读不写时，
   开关切换后立刻被重置，看着像功能缺失。
4. **验证工具本身也会错** —— 先确认工具对，再判定代码错。

## 严格 IPC 校验（第 44 轮起）

mock 会在 `invoke` 上包一层严格校验：前端调用的命令若缺 Rust 必填参数
（来自 `cmd_required.json`，由 `gen-cmd-required.py` 从 Rust 源码生成），
会记入 `window.__STRICT_VIOLATIONS__`。

- `strict-ipc.js`：跑全流程（hub 10 页 + 主窗 4 页签 + 4 独立窗）收集违规
- 注意：校验器对参数名兼容 snake/camel 两种写法，**不能发现 camelCase
  方向性错误**（第 45 轮教训：Tauri 2 前端必须传 camelCase，只有真机
  trace.log 能暴露）。字段名以静态比对（call/commit 键 vs Rust 签名）为准。

## mock 数据约定（第 107 轮起）

- `mockdata.json` 需含：remotes[].machine_os（引导 os-badge）、
  snapshot.lastAction（status-bar 回执）、snapshot.helper（免密模式）
- `mock-tauri.js` 需含：remote_system_info / gateway_pair_issue /
  gateway_tunnel_logs / remote_boot_info（记录 __RBI_ARGS__）等 handler
