# 约定：「旧版」= git tag **v0.2.5**（基准参考实现）

用户 2026-11-09 最终定案（此前一度说 7bd3ce2，后更正为 tag v0.2.5），**长期有效、以后不要忘记**：

- 本项目里用户说「**旧版**」= **git tag `v0.2.5`**（= commit cb65e8b，解引用 457bd05）。
- 凡"照抄旧版 / 和旧版一样 / 对齐旧版 / 抄旧版逻辑" → 一律以 **v0.2.5** 为准。
- **不要自己发挥、不要加旧版没有的东西**；严格照抄旧版的 DOM 结构 / CSS / 交互逻辑。

## 取旧版源码
```bash
git -C /Users/jianghh/IdeaProjects/anyboot archive v0.2.5 ui | tar -x -C /tmp/old25
# 看 /tmp/old25/ui/hub.html、app.js、styles 等
git show v0.2.5:ui/hub.html | ...
```

## 已踩教训（照抄前先核对 v0.2.5）
- RemoteEditDialog「连接优先级」`rp-row`：v0.2.5 里每行的元素，照 v0.2.5 原文核对后再实现，
  **不要凭印象加东西**（曾自作主张加"展示"开关等）。
