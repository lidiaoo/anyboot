# AnyBoot 0.2.12（小版本）

## 引导器统一
- 所有引导器统一为独立通道：rEFInd / RefindPlus / GRUB / systemd-boot / OpenCore / Clover，三端同一套按钮/下拉 UI。
- 后端 `BootOption` 增加 `child` 字段，一级/二级层级三端一致（一级不缩进、二级缩进+竖线）。
- 组头统一：浅底 + accent 蓝字 + 图标（无竖条）；置顶行统一：绿底 + 「下次启动」徽标 + 一级›二级名称 + 图标。
- 移除 GRUB 专属「条目文字格式」入口（PC 下拉/系统引导页/⋮ 菜单 + Android 下拉）。
- 修复 PC 置顶行 sticky 被覆盖、iOS 置顶行随滚动、iOS 卡片回显缺父组前缀、iOS 列表当前项多余徽标、GRUB/OpenCore 数据渲染与可选性。

## 构建
- macOS: AnyBoot_0.2.12_x64.dmg
- Android: AnyBoot_0.2.12_debug.apk / AnyBoot_0.2.12_release.apk（debug keystore 签名）
- iOS: 模拟器构建通过（AnyBootRemote-iOS-simulator.app）
