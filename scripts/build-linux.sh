#!/usr/bin/env bash
# Linux 三包构建：deb / rpm / appimage
#
# 为什么不用一条 `tauri build --bundles deb,rpm,appimage`：
#
# 1) **appimagetool 拒绝多架构** —— resources 里同时带 amd64 与 arm64 的 tsnet
#    sidecar，缺 ARCH 时它直接退出非 0，而 tauri 只把上层文案报成
#    `failed to run linuxdeploy`。**这句不等于 linuxdeploy 炸了，别顺着它查**；
#    用 `env -u ARCH appimagetool AnyBoot.AppDir /tmp/x.AppImage` 一句就能复现
#    `More than one architectures were found`。
#
# 2) **tauri 的退出码不可信** —— linuxdeploy 收尾返回非 0、但 AppImage 其实已经
#    完整写出来了（实测能跑、--version 正常）。反过来也成立（Windows 上已知
#    「退出码 1 但 bundles 完成」）。所以成败**以产物为准**，不看退出码。
#
# 3) 失败重试有意义：linuxdeploy 每次都要从 GitHub 现下 runtime，且复用上一次
#    残留的 AppDir 时可能报 strip `No such file`；重跑一次通常就过。
#
# 4) **不要在这里 rm -rf AppDir** —— 清了反而更容易撞上上面第 3 条的半成品状态，
#    而且本仓库所在的沙箱里 `rm` 会被删除垫片接管（异步删），可能把正在打包的
#    文件删掉，症状正是 strip `No such file`。
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

: "${ARCH:=$(uname -m)}"
export ARCH

TAURI=(npx tauri build)
BUNDLE_DIR="src-tauri/target/release/bundle"
APPIMAGE_DIR="$BUNDLE_DIR/appimage"

echo "== [1/3] deb + rpm（ARCH=$ARCH）"
if ! "${TAURI[@]}" --bundles deb,rpm; then
  echo "!! deb/rpm 打包失败，终止（加 --verbose 看细节）" >&2
  exit 1
fi

echo "== [2/3] appimage（最多 3 次）"
app=""
for attempt in 1 2 3; do
  # 记下构建前的产物时间戳：只有「比它新」才算这次真的产出了
  before=$(stat -c %Y "$APPIMAGE_DIR"/*.AppImage 2>/dev/null | head -1)
  before=${before:-0}

  if "${TAURI[@]}" --bundles appimage; then
    :
  else
    echo "-- 第 $attempt 次退出码非 0，检查产物"
  fi

  candidate=$(ls "$APPIMAGE_DIR"/*.AppImage 2>/dev/null | head -1)
  if [ -n "$candidate" ]; then
    after=$(stat -c %Y "$candidate")
    if [ "$after" -gt "$before" ]; then
      app=$candidate
      echo "-- 产物已更新（$(date -d "@$after" '+%H:%M:%S')），按成功处理"
      break
    fi
    echo "-- 产物还是旧的（未更新），不算成功"
  fi
  [ "$attempt" -lt 3 ] && sleep 3
done
[ -n "$app" ] || { echo "!! appimage 三次都没产出新包（用 --verbose 看 linuxdeploy 原始报错）" >&2; exit 1; }

echo "== [3/3] 自检"
status=0
check_file() {
  local path=$1 label=$2 min_mb=$3
  if [ ! -f "$path" ]; then
    echo "!! 缺产物：$label ($path)" >&2
    status=1
    return
  fi
  local mb=$(( $(stat -c %s "$path") / 1024 / 1024 ))
  if [ "$mb" -lt "$min_mb" ]; then
    echo "!! $label 只有 ${mb}MB，低于下限 ${min_mb}MB，可能是残包" >&2
    status=1
  else
    printf '   %-8s %4sMB  %s\n' "$label" "$mb" "$(md5sum "$path" | cut -d' ' -f1)"
  fi
}

check_file "$(ls "$BUNDLE_DIR"/deb/*.deb 2>/dev/null | head -1)" deb 5
check_file "$(ls "$BUNDLE_DIR"/rpm/*.rpm 2>/dev/null | head -1)" rpm 5
check_file "$app" appimage 20

# AppImage 冒烟：本机常常没有 /dev/fuse，用解包直跑
if [ -f "$app" ]; then
  out=$(APPIMAGE_EXTRACT_AND_RUN=1 timeout 120 "$app" --version 2>&1 | tail -1)
  case "$out" in
    AnyBoot*) printf '   冒烟     %s\n' "$out" ;;
    *) echo "!! AppImage 冒烟异常：$out" >&2; status=1 ;;
  esac
fi

[ "$status" = 0 ] && echo "== 完成：deb / rpm / appimage 三包就绪"
exit "$status"
