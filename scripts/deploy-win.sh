#!/usr/bin/env bash
# 部署 release 产物。
#
# 两种落位，差别不只是路径：
#   · 安装目录 %ProgramFiles%\AnyBoot —— 特权服务 AnyBootHelper 只认这一份 exe
#     （service_win::is_installed_copy）。只有它跑起来，需要提权的动作才会走
#     「服务优先、拿不到才弹 UAC」的阶梯，也就是免密。
#   · 便携目录 %LOCALAPPDATA%\AnyBoot —— 服务会回 not_our_exe（→ 客户端判
#     Unavailable），于是**每一个**特权动作（GRUB/OC 选中、读 OCFBt*、APFS 扫描、
#     分配盘符…）都退化成一次 UAC 弹窗。仅用于调试，别当日常部署。
#
# 所以：本机装了安装版时，默认拒绝便携部署，让用户用安装包覆盖安装；
# 确实要便携跑就显式 --portable（并接受提权弹窗）。
#
# 便携部署还必须带上 binaries/：内置 Tailscale 引擎是 bundle.resources 里的
# sidecar，只拷 anyboot.exe 会报「内置 Tailscale 引擎缺失」。
#
# 用法：bash scripts/deploy-win.sh [--portable] [--restart]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/src-tauri/target/release"
INSTALLED="${ProgramFiles:-C:/Program Files}/AnyBoot"
DEST="${LOCALAPPDATA:-$HOME/AppData/Local}/AnyBoot"
STAMP="$(date +%Y%m%d-%H%M)"
# 安装包名带版本号：从 tauri.conf.json 读，别在这里写死（换版本就会指错文件）
VERSION="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$ROOT/src-tauri/tauri.conf.json" | head -1)"
PORTABLE=0
RESTART=0

for arg in "$@"; do
  case "$arg" in
    --portable) PORTABLE=1 ;;
    --restart) RESTART=1 ;;
    *) echo "未知参数：$arg（只认 --portable / --restart）" >&2; exit 1 ;;
  esac
done

if [ ! -f "$SRC/anyboot.exe" ]; then
  echo "✗ 缺 $SRC/anyboot.exe —— 先跑 npm run build:win" >&2
  exit 1
fi

# 装了安装版却往便携目录铺 = 亲手制造「什么都弹 UAC」的现场（已踩过两次）
if [ -f "$INSTALLED/anyboot.exe" ] && [ "$PORTABLE" -eq 0 ]; then
  cat >&2 <<MSG
✗ 本机装有安装版（$INSTALLED），不做便携部署。

  原因：特权服务只接受「安装目录里那份 exe」的请求。便携副本会被服务回
        not_our_exe，于是 GRUB/OpenCore 选中、读固件变量、APFS 扫描这些
        需要提权的动作全部退化成 UAC 弹窗 —— 看起来就像「免密服务没生效」。

  正确做法（一次 UAC，之后一直免密）：
      双击 dist/AnyBoot_${VERSION:-0.2.8}_x64-setup.exe 覆盖安装
      安装器会把 exe 写进 $INSTALLED 并重启 AnyBootHelper。

  确实要便携跑（接受弹窗）：bash scripts/deploy-win.sh --portable --restart
MSG
  exit 2
fi

mkdir -p "$DEST/binaries"

if [ -f "$DEST/anyboot.exe" ]; then
  cp -f "$DEST/anyboot.exe" "$DEST/AnyBoot-bak-$STAMP-deploy.exe"
  echo "· 已备份旧版 → AnyBoot-bak-$STAMP-deploy.exe"
fi

cp -f "$SRC/anyboot.exe" "$DEST/anyboot.exe"
echo "· anyboot.exe → $DEST"

# bundle.resources 的全部落位（当前只有 sidecar；以后加资源也不用改这里）
if [ -d "$SRC/binaries" ]; then
  cp -f "$SRC/binaries/"* "$DEST/binaries/" 2>/dev/null || true
  echo "· binaries/ → $DEST/binaries（$(ls -1 "$DEST/binaries" | wc -l) 个文件）"
else
  echo "! 缺 $SRC/binaries —— 内置 Tailscale 引擎会报「引擎缺失」" >&2
fi

echo "! 便携副本：特权服务不会认这一份，提权动作会弹 UAC" >&2

if [ "$RESTART" -eq 1 ]; then
  # 只杀 Console 会话那个 GUI 实例；Services 会话里的提权服务保留
  pid="$(tasklist //FI "IMAGENAME eq anyboot.exe" //FO CSV 2>/dev/null \
    | awk -F'","' 'NR>1 && $3 ~ /Console/ {gsub(/"/,"",$2); print $2}' | head -1)"
  if [ -n "${pid:-}" ]; then
    taskkill //PID "$pid" //F >/dev/null 2>&1 || true
    sleep 1
    echo "· 已停止 GUI 实例 $pid"
  fi
  ( cd "$DEST" && ./anyboot.exe >/dev/null 2>&1 & )
  echo "· 已重新拉起 $DEST/anyboot.exe"
fi

echo "✓ 部署完成：$DEST"
