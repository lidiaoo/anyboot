#!/usr/bin/env bash
# 把 release 产物部署到 %LOCALAPPDATA%\AnyBoot（便携部署位），并整棵带上资源目录。
#
# 为什么必须带上 binaries/：内置 Tailscale 引擎是 sidecar（bundle.resources 里声明的
# binaries/anyboot-tsnet.exe）。只拷 anyboot.exe 的话 resource_dir()（= exe 所在目录）
# 下没有 binaries/，点「启用内置引擎」就会报
#   内置 Tailscale 引擎缺失（…\AnyBoot\binaries\anyboot-tsnet.exe）
# —— 而 %ProgramFiles%\AnyBoot\binaries\ 里其实有那份，于是错误看起来像文件丢了。
#
# 用法：bash scripts/deploy-win.sh [--restart]
#   先跑 npm run build:win 出包；--restart 会杀掉当前 GUI 实例再拉起（面板实例不动）。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/src-tauri/target/release"
DEST="${LOCALAPPDATA:-$HOME/AppData/Local}/AnyBoot"
STAMP="$(date +%Y%m%d-%H%M)"

if [ ! -f "$SRC/anyboot.exe" ]; then
  echo "✗ 缺 $SRC/anyboot.exe —— 先跑 npm run build:win" >&2
  exit 1
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

if [ "${1:-}" = "--restart" ]; then
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
