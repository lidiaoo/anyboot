# 约定：「旧版」= git commit 7bd3ce2（基准参考实现）

用户 2026-11-09 定案，**长期有效、以后不要忘记**：

- 本项目里用户说「**旧版**」= git commit **7bd3ce2**（Vue3 迁移之前的原始实现）。
- 凡"照抄旧版 / 和旧版一样 / 对齐旧版 / 抄逻辑" → 一律以 **7bd3ce2** 为准。
- **不要自己发挥、不要加旧版没有的东西**；严格照抄旧版的 DOM 结构 / CSS / 交互逻辑。

## 取旧版源码
```bash
git -C /Users/jianghh/IdeaProjects/anyboot archive 7bd3ce2 ui | tar -x -C /tmp/oldui
# 看 /tmp/oldui/ui/hub.html、app.js、styles 等
git show 7bd3ce2:ui/hub.html | ...
```

## 已踩教训（照抄前先核对）
- RemoteEditDialog「连接优先级」`rp-row`：7bd3ce2 里每行只有
  【拖拽柄 ☰ + 通道名 + 探测超时输入 + ↑↓ + 启用开关】**没有"展示"开关**。
  曾自作主张在每行加第二颗"展示"switch —— 错误，应删掉。逐通道「是否展示」按 7bd3ce2 的真实位置/形态还原。
