#!/usr/bin/env bash
# 打包 + 静默安装（本机开发自用）：装完直接把应用拉起来，用户零操作。
#
#   ./scripts/dev-reinstall.sh all     # 打 APK + 打 mac，然后各自静默安装
#   ./scripts/dev-reinstall.sh apk     # 只打 APK，adb 静默覆盖安装并拉起
#   ./scripts/dev-reinstall.sh mac     # 只打 mac，替换 /Applications 并拉起
#   SKIP_BUILD=1 ./scripts/dev-reinstall.sh mac   # 用现成产物只安装
#
# 设计要点（踩过的坑，别改回去）：
# - adb 不在 PATH 里：优先 $ANDROID_HOME/platform-tools/adb 与 ~/Library/Android/sdk。
# - 没有设备时**不算失败**：打印提示后跳过，避免整套流程中断（手机上没插线是常态）。
# - mac 覆盖安装必须先退出旧进程：单实例锁 + 端口 12300 会挡住新实例。
# - 旧 .app 不删，挪到 ~/Library/Caches/anyboot-install/ 留一次回滚。
# - IDEA/Android Studio 的 JBR 是 gradle 唯一可用 JDK（系统没有 java）。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-all}"
SKIP_BUILD="${SKIP_BUILD:-0}"

ANDROID_DIR="$ROOT/mobile/android"
APK_PATH="$ANDROID_DIR/app/build/outputs/apk/debug/app-debug.apk"
ANDROID_PKG="com.anyboot.remote"
ANDROID_ACTIVITY="$ANDROID_PKG/.MainActivity"

APP_TARGET="/Applications/AnyBoot.app"
APP_EXEC="$(printf '%s' "$APP_TARGET/Contents/MacOS/anyboot")"
BACKUP_DIR="$HOME/Library/Caches/anyboot-install"

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mxx\033[0m %s\n' "$*"; exit 1; }

# ── Android ──────────────────────────────────────────────────────────────────
find_adb() {
  local candidates=(
    "${ANDROID_HOME:-}/platform-tools/adb"
    "${ANDROID_SDK_ROOT:-}/platform-tools/adb"
    "$HOME/Library/Android/sdk/platform-tools/adb"
  )
  local c
  for c in "${candidates[@]}"; do
    [ -x "$c" ] && { printf '%s' "$c"; return 0; }
  done
  command -v adb 2>/dev/null && return 0
  return 1
}

find_java_home() {
  local c
  for c in "$HOME/Library/Java/JavaVirtualMachines"/*/Contents/Home \
           "/Applications/Android Studio.app/Contents/jbr/Contents/Home"; do
    [ -x "$c/bin/java" ] && { printf '%s' "$c"; return 0; }
  done
  return 1
}

find_gradle() {
  command -v gradle 2>/dev/null && return 0
  local c
  for c in "$HOME"/.gradle/wrapper/dists/gradle-*-bin/*/gradle-*/bin/gradle; do
    [ -x "$c" ] && { printf '%s' "$c"; return 0; }
  done
  return 1
}

build_apk() {
  local gradle java_home
  gradle="$(find_gradle)" || die "找不到 gradle（本地 wrapper 缓存也没有）"
  java_home="$(find_java_home)" || die "找不到 JDK（需要 Android Studio 的 jbr）"
  say "构建 APK（$("$java_home/bin/java" -version 2>&1 | head -1)）"
  ( cd "$ANDROID_DIR" && JAVA_HOME="$java_home" "$gradle" :app:assembleDebug --offline -q ) \
    || die "APK 构建失败"
  [ -f "$APK_PATH" ] || die "构建成功但没找到 $APK_PATH"
}

install_apk() {
  local adb
  adb="$(find_adb)" || { warn "找不到 adb，跳过手机安装"; return 1; }
  local count
  count="$("$adb" devices | awk 'NR>1 && $2=="device"' | wc -l | tr -d ' ')"
  if [ "$count" = "0" ]; then
    warn "没有连接的 Android 设备（USB 调试或无线调试都没开），跳过安装"
    warn "连上后单独跑：./scripts/dev-reinstall.sh apk"
    return 1
  fi
  say "adb 覆盖安装（设备 $count 台，保留数据）"
  "$adb" install -r -d "$APK_PATH" || die "adb install 失败"
  # 静默拉起：装完直接能看到界面，不用用户在手机上找图标
  "$adb" shell am start -n "$ANDROID_ACTIVITY" >/dev/null 2>&1 || true
  say "已在手机上启动 $ANDROID_PKG"
}

# ── macOS ────────────────────────────────────────────────────────────────────
built_app() {
  local c
  for c in "$ROOT/src-tauri/target/release/bundle/macos/AnyBoot.app" \
           "$ROOT"/src-tauri/target/*/release/bundle/macos/AnyBoot.app; do
    [ -d "$c" ] && { printf '%s' "$c"; return 0; }
  done
  return 1
}

build_mac() {
  say "构建 macOS 应用（release bundle）"
  ( cd "$ROOT" && env -u CARGO PATH="$HOME/.cargo/bin:$PATH" npm run build 2>&1 | tail -15 ) \
    || die "macOS 构建失败"
  built_app >/dev/null || die "构建结束但没找到 AnyBoot.app"
}

stop_running_app() {
  pgrep -f "$APP_EXEC" >/dev/null 2>&1 || return 0
  say "退出正在运行的 AnyBoot（先温柔 quit，等 3 秒）"
  osascript -e 'tell application "AnyBoot" to quit' >/dev/null 2>&1 || true
  local i
  for i in 1 2 3 4 5 6; do
    pgrep -f "$APP_EXEC" >/dev/null 2>&1 || return 0
    sleep 0.5
  done
  warn "quit 没生效，直接结束进程"
  pkill -f "$APP_EXEC" >/dev/null 2>&1 || true
  for i in 1 2 3 4 5 6; do
    pgrep -f "$APP_EXEC" >/dev/null 2>&1 || return 0
    sleep 0.5
  done
  pgrep -f "$APP_EXEC" >/dev/null 2>&1 && die "旧进程还在，先手动退出再装"
}

install_mac() {
  local src="$1"
  mkdir -p "$BACKUP_DIR"
  stop_running_app
  if [ -d "$APP_TARGET" ]; then
    say "备份旧版本 → $BACKUP_DIR/AnyBoot.app"
    rm -rf "$BACKUP_DIR/AnyBoot.app"
    mv "$APP_TARGET" "$BACKUP_DIR/AnyBoot.app" || die "旧版本挪不走（权限？）"
  fi
  say "安装新版本 → $APP_TARGET"
  ditto "$src" "$APP_TARGET" || die "复制失败"
  xattr -dr com.apple.quarantine "$APP_TARGET" >/dev/null 2>&1 || true
  codesign --force --deep --sign - "$APP_TARGET" >/dev/null 2>&1 || warn "ad-hoc 重签名失败（沿用包内签名）"
  say "启动新版"
  open -a "$APP_TARGET" || die "启动失败"
}

# ── 入口 ─────────────────────────────────────────────────────────────────────
case "$MODE" in
  apk)  [ "$SKIP_BUILD" = "1" ] || build_apk; install_apk ;;
  mac)  [ "$SKIP_BUILD" = "1" ] || build_mac
        install_mac "$(built_app)" ;;
  all)  [ "$SKIP_BUILD" = "1" ] || build_apk; install_apk
        [ "$SKIP_BUILD" = "1" ] || build_mac
        install_mac "$(built_app)" ;;
  *)    die "用法：$0 {all|apk|mac}（可加 SKIP_BUILD=1）" ;;
esac

say "完成"
