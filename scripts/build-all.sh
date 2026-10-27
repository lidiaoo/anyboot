#!/usr/bin/env bash
# build-all.sh —— 本机（macOS）一键编译打包所有"本机可产"的端。
#
# 能产：
#   1. Android APK      mobile/android/app/build/outputs/apk/debug/app-debug.apk
#   2. macOS 桌面 .app/.dmg（本机架构；--intel/--universal 可选）
#   3. iOS 模拟器构建（默认）或 --device 真机 archive（需本机签名证书）
#
# 产不了（单机无交叉编译）→ 用 .github/workflows/build-all.yml 在 CI 出：
#   Windows NSIS/MSI、Linux deb/rpm/AppImage、签名 IPA。
#
# 用法：
#   bash scripts/build-all.sh                  # 三端全做（macOS 按本机架构）
#   bash scripts/build-all.sh --skip-ios       # 跳过 iOS
#   bash scripts/build-all.sh --skip-android --skip-desktop
#   bash scripts/build-all.sh --mac-universal  # 桌面出 universal（Intel+ARM 合包）
#   bash scripts/build-all.sh --ios-device     # iOS 真机 archive（需证书）
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SKIP_ANDROID=0; SKIP_DESKTOP=0; SKIP_IOS=0
MAC_TARGET=""; IOS_DEVICE=0
for arg in "$@"; do
  case "$arg" in
    --skip-android) SKIP_ANDROID=1 ;;
    --skip-desktop) SKIP_DESKTOP=1 ;;
    --skip-ios)     SKIP_IOS=1 ;;
    --mac-intel)    MAC_TARGET="intel" ;;
    --mac-universal) MAC_TARGET="universal" ;;
    --ios-device)   IOS_DEVICE=1 ;;
    *) echo "未知参数: $arg" >&2; exit 2 ;;
  esac
done

log()  { printf '\n\033[1;36m== %s ==\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31m✗ %s\033[0m\n' "$*" >&2; exit 1; }
ok()   { printf '\033[1;32m✓ %s\033[0m\n' "$*"; }

# ---------- 1. Android ----------
if [ "$SKIP_ANDROID" = 0 ]; then
  log "Android APK"
  GRADLE_BIN="$(command -v gradle || true)"
  [ -z "$GRADLE_BIN" ] && GRADLE_BIN="$(ls -d "$HOME"/.gradle/wrapper/dists/gradle-*-bin/*/gradle-*/bin/gradle 2>/dev/null | sort -V | tail -1)"
  [ -n "$GRADLE_BIN" ] || fail "找不到 gradle（装一个或先跑过 Android Studio）"
  JBR="$(ls -d "$HOME"/Library/Java/JavaVirtualMachines/jbr-*/Contents/Home 2>/dev/null | sort -V | tail -1)"
  [ -n "$JBR" ] || JBR="$("/usr/libexec/java_home" 2>/dev/null || true)"
  [ -n "$JBR" ] || fail "找不到 JDK（装 Android Studio 的 JBR 或任意 JDK 17+）"
  ( cd "$ROOT/mobile/android" && JAVA_HOME="$JBR" "$GRADLE_BIN" assembleDebug --console=plain -q ) \
    || fail "Android 构建失败"
  APK="$ROOT/mobile/android/app/build/outputs/apk/debug/app-debug.apk"
  [ -f "$APK" ] || fail "APK 未产出"
  ok "Android: $APK ($(du -h "$APK" | cut -f1))"
else
  log "跳过 Android"
fi

# ---------- 2. macOS 桌面（Tauri） ----------
if [ "$SKIP_DESKTOP" = 0 ]; then
  log "macOS 桌面（Tauri，本机已带全部平台 sidecar 二进制）"
  [ "$(uname)" = Darwin ] || fail "桌面打包只在 macOS 上做（Windows/Linux 走 CI）"
  case "$MAC_TARGET" in
    intel)     SCRIPT="build:mac-intel" ;;
    universal) SCRIPT="build:mac-universal" ;;
    *)         SCRIPT="build:mac-arm"; [ "$(uname -m)" = x86_64 ] && SCRIPT="build:mac-intel" ;;
  esac
  ( cd "$ROOT" && npm run $SCRIPT ) || fail "桌面构建失败（npm run ${SCRIPT}）"
  for dir in "$ROOT"/src-tauri/target/*/release/bundle; do
    find "$dir" -name '*.dmg' -o -name '*.app' 2>/dev/null | while read -r f; do ok "桌面: $f"; done
  done
else
  log "跳过桌面"
fi

# ---------- 3. iOS ----------
if [ "$SKIP_IOS" = 0 ]; then
  log "iOS"
  XCG="$(command -v xcodegen || echo "$HOME/bin/xcodegen")"
  [ -x "$XCG" ] || fail "找不到 xcodegen（brew install xcodegen）"
  ( cd "$ROOT/mobile/ios" && "$XCG" generate -q ) || fail "xcodegen 生成工程失败"
  SCHEME="$(xcodebuild -list -project "$ROOT/mobile/ios/AnyBootRemote.xcodeproj" 2>/dev/null \
            | awk '/Targets:/{f=1;next} /Configurations:/{f=0} f{print $1; exit}')"
  [ -n "$SCHEME" ] || SCHEME="AnyBootRemote"
  if [ "$IOS_DEVICE" = 1 ]; then
    ( cd "$ROOT/mobile/ios" && xcodebuild archive -project AnyBootRemote.xcodeproj \
        -scheme "$SCHEME" -archivePath build/AnyBootRemote.xcarchive ) \
      || fail "iOS 真机 archive 失败（需要签名证书 + 描述文件）"
    ok "iOS archive: $ROOT/mobile/ios/build/AnyBootRemote.xcarchive（Xcode → Distribute App 出 IPA）"
  else
    # 目标模拟器：优先 iPhone 16（CI 默认），本机没有就取第一台可用 iPhone 的名字
    DEST="platform=iOS Simulator,name=iPhone 16,OS=latest"
    if ! xcrun simctl list devices available | grep -q "iPhone 16 (" ; then
      SIM_NAME="$(xcrun simctl list devices available \
        | sed -n 's/^ *\(iPhone[^()]*\) *([A-F0-9-]*) (.*/\1/p' | head -1 | xargs)"
      [ -n "$SIM_NAME" ] && DEST="platform=iOS Simulator,name=$SIM_NAME,OS=latest"
    fi
    ( cd "$ROOT/mobile/ios" && xcodebuild build -project AnyBootRemote.xcodeproj \
        -scheme "$SCHEME" -destination "$DEST" -quiet ) \
      || fail "iOS 模拟器构建失败"
    ok "iOS 模拟器构建通过（${DEST}）"
  fi
else
  log "跳过 iOS"
fi

log "全部完成"
