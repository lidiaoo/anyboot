# AnyBoot 远程 · Android 客户端

**它不是 AnyBoot 的安卓移植版**，而是一个 WebView 外壳：加载电脑端 AnyBoot 内置 HTTP 服务
（默认 `12300`）托管的**同一份前端页面**，页面里的 `/api/ui-state`、`/api/ui-command`
都是同源请求。所以：

- 不需要把 Rust 核心交叉编译到 `aarch64-linux-android`，也不需要 NDK；
  服务端永远是 Windows / Linux / macOS 上的那个 AnyBoot。
- 电脑端前端一改，手机端刷新就是新的，不存在两套 UI 走样。
- 手机端只做浏览器不顺手的事：**多服务端同时在线、标签秒切、服务端分组、记住 token、断线重试面板**。

## 多服务端 / 分组

| 能力 | 说明 |
| --- | --- |
| 同时连多个服务端 | 每个标签一个独立 `WebView`，互不影响；菜单里「启动时连接全部服务端」可让它们一起在线 |
| 顶部标签快速切换 | 标签栏横向滚动，点一下就切；已加载过的标签**不重新加载**，秒切 |
| 标签名 = 组名 ＋ 服务端名 | 未分组时只显示服务端名；名字留空则用地址兜底 |
| 分组 | 「分组」用来**区分不同的电脑**：一台电脑多块网卡 / 多次 DHCP 拿到不同 IP 时，把这几条地址填同一个分组 |
| 分组配色 | 同一个分组的标签描边同色（颜色由组名哈希决定，恒定不变），扫一眼就知道哪几个标签是同一台机器 |
| 在线状态点 | 标签左侧小圆点：灰=还没连、橙=加载中、绿=已加载、红=连不上 |
| 增删改 | 标签栏 `＋` 添加；**长按标签**直接编辑；`☰` 或菜单「服务端列表」切换/删除；编辑框里也有「删除」 |
| 1.0 升级 | 旧版单服务端配置（`host`/`token`）会自动迁移成列表里的第一条，用户无感 |

## 目录

```
mobile/android/
├── settings.gradle.kts / build.gradle.kts / gradle.properties
├── local.properties                         # sdk.dir，已被 .gitignore 忽略
└── app/
    ├── build.gradle.kts                     # compileSdk 34 / minSdk 24 / AGP 8.7.3
    └── src/main/
        ├── AndroidManifest.xml              # INTERNET + usesCleartextTraffic
        ├── java/com/anyboot/remote/
        │   ├── MainActivity.java            # 标签栏 + 每标签一个 WebView + 增删改对话框
        │   ├── ServerProfile.java           # 一条服务端记录（分组/名称/地址/token）+ 地址归一化
        │   └── ServerStore.java             # SharedPreferences(JSON) 持久化 + 旧版迁移
        └── res/                             # 图标 + app_name
```

## 构建

本机（Windows + Git Bash）已验证可用的命令：

```bash
export JAVA_HOME=/e/Android/jdk17/jdk-17.0.20.1+1
export GRADLE_USER_HOME='E:\Android\gradle-home'    # 缓存全在 E 盘，不占 C 盘
cd mobile/android
/e/Android/gradle-8.11.1/bin/gradle assembleDebug --no-daemon --console=plain
```

产物：`mobile/android/app/build/outputs/apk/debug/app-debug.apk`

首次构建会从 Google Maven 拉 AGP 及依赖（几百 MB）到 `GRADLE_USER_HOME`。
用 Android Studio 打开 `mobile/android/` 也可以，AGP/Gradle 版本已对齐。

## 安装与使用

```bash
adb install -r mobile/android/app/build/outputs/apk/debug/app-debug.apk
```

1. 电脑端 AnyBoot：**基本设置 ▸ 远程控制** 打开，端口 `12300`。
2. 手机与电脑在同一局域网（同一路由器 / 同一热点）。
3. 打开 App → 首次会要求填一条服务端：分组、服务端名（可留空）、地址
   （`192.168.0.120:12300`，也可以整条带 token 的地址粘进来，token 会自动拆出来）。
4. 想连第二台电脑 / 第二个 IP：点标签栏的 `＋`；同一台电脑的多条地址**填同一个分组**。
5. 长按标签改这一条；`☰` 看列表并切换；菜单里可「重载当前页面 / 全部重载 / 启动时连接全部服务端」。
6. 返回键先退当前页面的站内导航，退不动才退出 App。

## 实现要点（踩过的坑）

- **明文 HTTP 必须显式放行**：`android:usesCleartextTraffic="true"`，
  否则 Android 9+ 直接掐断到 `http://192.168.x.x:12300` 的连接；
  WebView 侧还要 `setMixedContentMode(MIXED_CONTENT_ALWAYS_ALLOW)`。
- **必须开 DOM storage**：前端的 token 存在 cookie、栏目选择存在 localStorage，
  不开 `setDomStorageEnabled(true)` 会表现为「token 永远存不住、每次都要重输」。
- **只有主文档失败才算「连不上」**：`onReceivedError` 里判 `request.isForMainFrame()`，
  否则一个 favicon 404 就会把整个页面替换成错误面板。
- **WebView 懒创建**：只有切到过的标签（或开了「启动时连接全部」）才建 `WebView`，
  十条服务端配置不会在启动时白占十份内存。
- **切标签不重载**：已加载过的页面保留在后台（切回来直接显示），
  只有编辑过地址/token 或手动重载才重新拉页面。
- **`configChanges` 里带 `orientation|screenSize`**：转屏不重建 Activity，
  否则十个标签会一起重新加载。
- 不引 AndroidX / Kotlin：全部用系统 `WebView`、`AlertDialog`，依赖只有 AGP 本身，
  构建体积和失败面都小。
