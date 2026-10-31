# 构建细则（三端 + 产物核验）

> 索引在 `MEMORY.md`「构建」段。这里只放命令与坑，定案类结论（⛔ 不要主动出包）留在 MEMORY.md。

## Windows（2026-10-28 定案：不往 J 盘写任何东西）
- 仓库 `E:\Common\Project\IdeaProjects\anyboot`，产物用默认 `src-tauri/target`（已 gitignore）。
- 只把临时目录指到项目内：`$env:TMP=$env:TEMP='E:\...\anyboot\.build\tmp'`——WiX/NSIS 要几 GB，落 C: 报 os error 112。
- `npm run build` 出 msi+nsis；**管道退出码常为 1，见「Finished 2 bundles」即成功**。
- 部署：`taskkill /IM anyboot.exe /F` → 备份 `AnyBoot-bak-YYYYMMDD<x>.exe` → 复制 exe。
- 12300 落保留段 bind 报 10013 → `netsh int ipv4 set dynamicport tcp start=49152 num=16384` 后重启 winnat。

## macOS（本机是 Hackintosh x86_64，无 arm64）
- `npm run build:mac-intel`。
- `bundle_dmg.sh` 在本机必失败（要 GUI 自动化摆窗口）→ 卸残留挂载 + 删 `rw.*.dmg` + `hdiutil create -ov -format UDZO` 补包。配方见 `2026-10-26.md`。
- Windows 版在这里编不出来（无 MSVC）。

## Linux
- 系统 `/usr/bin/rustc` 已坏（要 libLLVM.so.23.1）→ 用自装工具链 `/home/disk1/{cargo,rustup}-home`。
- release 偶发 SIGABRT：`CARGO_BUILD_JOBS=2`（jammy 链单加 `dangerouslyDisableSandbox` 不够，2026-10-29 复现）。
- **仓库目录跑不了 check/test**（指纹扫描读 .git exclude 报 EACCES）→ rsync `src-tauri` + 同级 `ui/` 到 `/home/disk1/anyboot-check` 再跑；`TMPDIR` 要指到大盘（/tmp 是 10M tmpfs）。
- 交叉 check `--target x86_64-pc-windows-gnu` 本机做不了：cc-rs 要 `x86_64-w64-mingw32-gcc`，无 mingw-w64。
- 细则见 `2026-10-25.md`。

## Android
- 无 gradlew、无全局 gradle → 用 `~/.gradle/wrapper/dists/gradle-8.14.3-bin/*/gradle-8.14.3/bin/gradle`。
- 必须给 JDK17（`/usr/local/software/jdk/jdk-17`）+ `ANDROID_HOME`，并**在命令行覆盖 `-Duser.home`**（`gradle.properties` 里写着 Linux 的 `/home/jianghh`，不覆盖连 debug keystore 都生不出）。
- 产物核验：逐 dex 搜符号（本工程 4 个 dex，`RemotePair` 在 classes3.dex），别拿体积当判据。

## 产物新旧核验
- **唯一可靠判据 = 拆包 grep 二进制串**（deb：`ar x` + 解 data.tar；AppImage：`--appimage-extract usr/bin/anyboot`）。
- 构建完立刻 md5sum 会读到**上轮产物的旧视图缓存**（md5/mtime 全是旧值，隔一分钟才刷新），别拿它当判据。

## 其它
- 内置 tsnet sidecar 一律跑 `scripts/build-sidecar.sh`，详见 `MEMORY-detail-tailscale.md`。
