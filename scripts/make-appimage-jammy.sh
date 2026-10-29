#!/usr/bin/env bash
# 构建「任何机器都能用」的自包含 AppImage。
#
# 原理：
#   库门槛 = 包内最高的 GLIBC_x.y 需求。原包裹的是 CachyOS 库（webkit 2.44），
#   在老系统（Debian 11 等）直接拒载。本脚本改用 Ubuntu 22.04 (jammy, glibc 2.35)
#   的整套库替换，并用 zig 以 -target x86_64-linux-gnu.2.35 重链本体（降到 2.34）。
#   → 全包门槛 ≈ 2.35，任何 2012 年以后的发行版都能跑。
#
# 依赖（缺什么脚本会尽量自动补救；路径全部可用环境变量覆盖，见下文「可移植路径」）：
#   sysroot                           jammy 库闭包（缺了自动跑 scripts/fetch-jammy-sysroot.py）
#   原版 AppImage                     cargo tauri build 的产物（缺解包时自动 --appimage-extract）
#   本体 target/release/anyboot       门槛 >2.35 时自动用 zig 重链
#   appimagetool / zig / glib-compile-schemas / gtk-query-immodules-3.0（PATH 或环境变量指定）
#
# 注意：不要用 grep -ob 'hsqs' 找 runtime 起点——会在 runtime ELF 内部命中假
# 阳性（194183），真偏移必须问包自己：orig-backup.AppImage --appimage-offset。
set -euo pipefail

# ==================== 可移植路径 ====================
# 全部可用环境变量覆盖；默认值按「仓库 + ~/.cache」布局，换机器只需给对应变量。
#   ANYBOOT_PROJ     仓库根目录（默认：脚本所在目录的上一级）
#   AB_SYSROOT       jammy 库闭包目录（fetch-jammy-sysroot.py 的 --dest；缺了自动现跑）
#   AB_ORIG_DIR      原版 AppImage / 解包 AppDir 的工作目录
#   AB_OUT           产物路径
#   AB_ZIG           zig 可执行文件（PATH 里没有时回退 ~/.cache/ab-zig/zig/zig）
#   AB_APPIMAGETOOL  appimagetool（PATH → 原包解包目录 依次探测）
PROJ=${ANYBOOT_PROJ:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}
ORIG_DIR=${AB_ORIG_DIR:-$HOME/.cache/ab-appimage}
SYS=${AB_SYSROOT:-$HOME/.cache/ab-sysroot/root}
BUILD=$ORIG_DIR/jammy-build
APPDIR=$BUILD/AppDir
BIN=$PROJ/src-tauri/target/release/anyboot
OUT=${AB_OUT:-$PROJ/src-tauri/target/release/bundle/appimage/AnyBoot_0.1.0_amd64-jammy.AppImage}

ZIG=${AB_ZIG:-$(command -v zig 2>/dev/null || true)}
if [ -z "$ZIG" ] && [ -x "$HOME/.cache/ab-zig/zig/zig" ]; then ZIG=$HOME/.cache/ab-zig/zig/zig; fi
APPIMAGETOOL=${AB_APPIMAGETOOL:-$(command -v appimagetool 2>/dev/null || ls $ORIG_DIR/squashfs-root/usr/bin/appimagetool 2>/dev/null || true)}

ORIG=$ORIG_DIR/orig-backup.AppImage   # 原版完整包备份（runtime 偏移从它实测）
SQ=$ORIG_DIR/restore/squashfs-root    # 原包解出的 AppDir

# 防线：先删旧产物再打包——文件被占用时（如残留 FUSE 挂载）这里立刻报错，
# 而不是让 mksquashfs 静默失败、旧文件冒充新包通过终验
/usr/bin/rm -f "$OUT"

# 原版素材：优先用已有解包（restore/squashfs-root）+ 原包备份；都没有就从
# cargo tauri 产出的原版 AppImage 现场解。注意不用 grep -ob 'hsqs' 找 runtime
# 起点——会在 runtime ELF 内部命中假阳性，真偏移必须问包自己 --appimage-offset。
if [ -d "$SQ" ] && [ -f "$ORIG" ]; then
  :
elif SRC=$(ls $PROJ/src-tauri/target/release/bundle/appimage/AnyBoot_0.1.0_amd64.AppImage 2>/dev/null); then
  ORIG=$SRC
  echo "==> 现场解原版 AppDir（$SRC）"
  mkdir -p "$ORIG_DIR/restore"
  (cd "$ORIG_DIR/restore" && "$ORIG" --appimage-extract >/dev/null 2>&1) || { echo "✗ 原版解包失败"; exit 1; }
else
  echo "✗ 缺原版素材：需要 $ORIG（+ $SQ），或先跑 cargo tauri build 出原版 AppImage"; exit 1
fi

# jammy 库闭包：没有就现场抓（脚本同目录）
[ -d "$SYS/usr/lib/x86_64-linux-gnu" ] || {
  echo "==> 缺 sysroot，现场抓取（首次约几分钟）"
  python3 "$PROJ/scripts/fetch-jammy-sysroot.py" --elf "$BIN" --dest "$SYS" \
    --cache "$(dirname "$SYS")" || { echo "✗ sysroot 抓取失败"; exit 1; }
}

[ -x "$APPIMAGETOOL" ] || { echo "✗ 缺 appimagetool（PATH 里放一个，或设 AB_APPIMAGETOOL）"; exit 1; }
[ -x "$BIN" ] || { echo "✗ 缺本体 $BIN（先 cargo build --release）"; exit 1; }
command -v glib-compile-schemas >/dev/null || { echo "✗ 缺 glib-compile-schemas"; exit 1; }
command -v gtk-query-immodules-3.0 >/dev/null || { echo "✗ 缺 gtk-query-immodules-3.0"; exit 1; }

# 本体门槛检查：>2.35 就地用 zig 重链（也是交付的一部分；没有 zig 则明确报错）
# 判据写法：MAX 自己排在 2.35 前面（sort -V）才算 ≤2.35。
# 不要写 "sort 后 head -1 = 2.35" —— 那对任何 MAX>2.35 也成立（2.35 恒排最前），等于永远通过。
BINMAX=$(objdump -T "$BIN" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -Vu | tail -1)
if [ -n "$BINMAX" ] && [ "$BINMAX" != "$(printf '%s\n2.35\n' "$BINMAX" | sort -Vu | head -1)" ]; then
  [ -n "$ZIG" ] && [ -x "$ZIG" ] || { echo "✗ 本体门槛 $BINMAX > 2.35，且没有 zig 可重链（设 AB_ZIG）"; exit 1; }
  echo "==> 本体门槛 $BINMAX > 2.35，用 zig 重链"
  mkdir -p "$BUILD"
  printf '#!/bin/sh\nexec "%s" cc -target x86_64-linux-gnu.2.35 "$@"\n' "$ZIG" > "$BUILD/zigcc"
  chmod +x "$BUILD/zigcc"
  (cd "$PROJ/src-tauri" && cargo rustc --release --bin anyboot -- -C linker="$BUILD/zigcc") \
    || { echo "✗ zig 重链失败"; exit 1; }
fi

echo "==> 1/7 以原包 AppDir 为底子"
/usr/bin/rm -rf "$BUILD"; mkdir -p "$BUILD"
cp -a "$SQ" "$APPDIR"

echo "==> 2/7 换本体（门槛以自检 A 实测为准）"
cp "$PROJ/src-tauri/target/release/anyboot" "$APPDIR/usr/bin/anyboot"

echo "==> 3/7 用 jammy 库整包替换 usr/lib（平铺，保持原包 Arch 布局）"
/usr/bin/rm -rf "$APPDIR/usr/lib" "$APPDIR/usr/lib64"
mkdir -p "$APPDIR/usr/lib"
cp -a "$SYS/usr/lib/x86_64-linux-gnu/." "$APPDIR/usr/lib/"
# jammy 部分包（usr-merge 前）把基础库放 /lib/x86_64-linux-gnu，一并平铺合并
[ -d "$SYS/lib/x86_64-linux-gnu" ] && cp -a "$SYS/lib/x86_64-linux-gnu/." "$APPDIR/usr/lib/"
[ -d "$SYS/usr/lib/aspell" ] && cp -a "$SYS/usr/lib/aspell" "$APPDIR/usr/lib/aspell"
# webkit 辅助进程两份布局照抄原包（lib 与 lib64）
mkdir -p "$APPDIR/usr/lib64/webkit2gtk-4.1"
cp -a "$APPDIR/usr/lib/webkit2gtk-4.1/." "$APPDIR/usr/lib64/webkit2gtk-4.1/"

# ⚠️ 上面 `rm -rf usr/lib` 是整棵删的，会把 tauri 的 resources 一起带走：
#    usr/lib/AnyBoot/binaries/anyboot-tsnet-linux-{amd64,arm64}（tauri.linux.conf.json
#    的 bundle.resources）。sidecar 丢了**不会报错**，只会让 tsnet/远程组网静默不可用——
#    所以这里必须补回来，并且后面自检要核。
mkdir -p "$APPDIR/usr/lib/AnyBoot/binaries"
for f in anyboot-tsnet-linux-amd64 anyboot-tsnet-linux-arm64; do
  if [ -f "$PROJ/src-tauri/binaries/$f" ]; then
    cp -f "$PROJ/src-tauri/binaries/$f" "$APPDIR/usr/lib/AnyBoot/binaries/$f"
    chmod 0755 "$APPDIR/usr/lib/AnyBoot/binaries/$f"
  else
    echo "✗ 缺 sidecar 源 $PROJ/src-tauri/binaries/$f（先跑 scripts/build-sidecar.sh）"; exit 1
  fi
done

echo "==> 3.5/7 改写 libwebkit 辅助进程路径（jammy 发布版无 WEBKIT_EXEC_PATH，只能照 tauri#5292 打相对路径）"
python3 - "$APPDIR/usr/lib/libwebkit2gtk-4.1.so.0" <<'PYEOF'
import sys, os
p = sys.argv[1]
orig_size = os.path.getsize(p)
data = open(p, 'rb').read()
old_dir = b"/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1"
new_dir = b"./lib/webkit2gtk-4.1"
assert len(new_dir) < len(old_dir)
def repl(old, new):
    # 必须等长替换：文件短一个字节，后面全部偏移错位 → SEGV
    assert len(new) <= len(old)
    return new + b"\0" * (len(old) - len(new))
old_bundle = old_dir + b"/injected-bundle/"
new_bundle = new_dir + b"/injected-bundle/"
n1 = data.count(old_dir)
data = data.replace(old_bundle, repl(old_bundle, new_bundle))
n2 = data.count(old_dir)
data = data.replace(old_dir, repl(old_dir, new_dir))
assert len(data) == orig_size, "补丁改变了文件长度，中止"
open(p, 'wb').write(data)
print(f"  patched: injected-bundle={n1-n2}, helper-dir={n2}, size unchanged={len(data)==orig_size}")
assert n2 > 0
PYEOF
# 相对路径以 AppDir 为工作目录解析 → 辅助进程放 $APPDIR/lib/ 下
mkdir -p "$APPDIR/lib"
cp -a "$APPDIR/usr/lib/webkit2gtk-4.1" "$APPDIR/lib/webkit2gtk-4.1"

echo "==> 4/7 gsettings schema 编译（glib-networking 的代理解析需要）"
cp -a "$SYS/usr/share/glib-2.0/schemas/." "$APPDIR/usr/share/glib-2.0/schemas/" 2>/dev/null || true
glib-compile-schemas "$APPDIR/usr/share/glib-2.0/schemas"

echo "==> 5/7 生成 gdk-pixbuf loaders.cache（jammy 是外部 loader，不能像 Arch 那样空缓存）"
LOADERS="$APPDIR/usr/lib/gdk-pixbuf-2.0/2.10.0"
LD_LIBRARY_PATH="$APPDIR/usr/lib" \
  "$APPDIR/usr/lib/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders" "$LOADERS/loaders"/libpixbufloader-*.so \
  > "$BUILD/loaders.cache.raw"
sed "s|$LOADERS/loaders/|./|g" "$BUILD/loaders.cache.raw" > "$LOADERS/loaders.cache"
grep -c 'libpixbufloader' "$LOADERS/loaders.cache" >/dev/null

echo "==> 6/7 生成 GTK 输入法模块缓存 immodules.cache"
IM="$APPDIR/usr/lib/gtk-3.0/3.0.0"
mkdir -p "$IM/immodules"
LD_LIBRARY_PATH="$APPDIR/usr/lib" \
  gtk-query-immodules-3.0 "$IM/immodules"/im-*.so > "$IM/immodules.cache" 2>/dev/null \
  || echo "  (警告: immodules.cache 生成失败，仅影响输入法，继续)"
grep -q 'immodules' "$IM/immodules.cache" 2>/dev/null && sed -i "s|$APPDIR/|./|g" "$IM/immodules.cache"
# 相对条目按 CWD（=AppDir，见 7/7 的 cd）解析，故前缀写成 ./usr/lib/...

echo "==> 6.5/7 编译 XInitThreads 垫片（WebKit 多线程 X 并发，见 scripts/xinitthreads_shim.c）"
ZIG_GLOBAL_CACHE_DIR="$BUILD/zig-cache" "$ZIG" cc -shared -fPIC -O2 \
  -target x86_64-linux-gnu.2.35 \
  -o "$APPDIR/usr/lib/xinitthreads.so" \
  "$PROJ/scripts/xinitthreads_shim.c"

echo "==> 7/7 追加 hook：cd 进 AppDir（webkit 相对路径的前提）+ pixbuf loader 目录"
# 注意：linuxdeploy 生成的 AppRun 只 source 这一个 hook 文件，新开文件不会被读
cat >> "$APPDIR/apprun-hooks/linuxdeploy-plugin-gtk.sh" <<'HOOK'

# --- AnyBoot 追加 ---
# libwebkit 里的辅助进程路径已改写为 ./lib/webkit2gtk-4.1（相对路径），
# 必须以 AppDir 为工作目录才能找到 WebKitNetworkProcess / WebKitWebProcess
cd "$APPDIR"
# 相对路径的 pixbuf loader 条目按此目录解析（gdk-pixbuf 官方环境变量）
export GDK_PIXBUF_MODULE_DIR="$APPDIR/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders"
# XInitThreads 必须在任何 Xlib 调用前执行（WebKit 多线程渲染），垫片最先加载
export LD_PRELOAD="$APPDIR/usr/lib/xinitthreads.so${LD_PRELOAD:+:$LD_PRELOAD}"
# gio 模块目录必须指向包内：jammy libgio 按编译期绝对路径扫描，
# 不设就会加载宿主的 /usr/lib/x86_64-linux-gnu/gio/modules（gvfs 按新 glib 编译 → 混载即崩）
export GIO_MODULE_DIR="$APPDIR/usr/lib/gio/modules"
# 渲染保守开关：默认关 DMABUF 渲染器与加速合成（部分机器 EGL 显示创建失败直接 abort）。
# 用户显式设过任一 WEBKIT_* 变量（含设成 0）则尊重原值不插手。
: "${WEBKIT_DISABLE_DMABUF_RENDERER:=1}"
: "${WEBKIT_DISABLE_COMPOSITING_MODE:=1}"
export WEBKIT_DISABLE_DMABUF_RENDERER WEBKIT_DISABLE_COMPOSITING_MODE
HOOK

echo "==> 自检 A：全包 GLIBC 门槛扫描（目标 <= 2.35）"
MAX=$(find "$APPDIR" -type f \( -name '*.so*' -o -name 'anyboot' -o -name 'WebKit*' \) -exec sh -c \
  'objdump -T "$1" 2>/dev/null | grep -o "GLIBC_[0-9.]*"' _ {} \; | sed 's/GLIBC_//' | sort -Vu | tail -1)
echo "  全包最高 GLIBC 需求 = $MAX"
# 正确判据：MAX 自己排在 2.35 之前（含相等）才放行（见上文 BINMAX 处的说明）
[ "$MAX" = "$(printf '%s\n2.35\n' "$MAX" | sort -Vu | head -1)" ] || { echo "  ✗ 超过 2.35"; exit 1; }

echo "==> 自检 A2：tauri resources 在包内（tsnet sidecar 缺了不会报错，只会静默失效）"
for f in anyboot-tsnet-linux-amd64 anyboot-tsnet-linux-arm64; do
  p="$APPDIR/usr/lib/AnyBoot/binaries/$f"
  [ -x "$p" ] || { echo "  ✗ 缺 $p"; exit 1; }
  cmp -s "$p" "$PROJ/src-tauri/binaries/$f" || { echo "  ✗ $f 与源不一致（版本错位）"; exit 1; }
  echo "  ✓ $f  $(stat -c%s "$p") 字节"
done

echo "==> 自检 A3：前端资产确实是本轮构建的"
# 资产是 brotli 压缩后嵌进二进制的，扫二进制不现实；改查 codegen 产物目录。
# 目录跟着**本体二进制**走（BIN 常是指向另一条构建线的符号链接，直接用
# $PROJ/src-tauri/target 会查到上一次的旧资产，得出「不一致」的假警报）。
# 注意：一轮会编出几十个页面资产，hub.html 往往不是 mtime 最新的那个 —— 全量解压搜。
BINREAL=$(readlink -f "$BIN" 2>/dev/null || echo "$BIN")
ASSET_ROOT=$(ls -d "$(dirname "$BINREAL")/build"/anyboot-*/out/tauri-codegen-assets 2>/dev/null | tail -1)
if [ -n "$ASSET_ROOT" ]; then
  node - "$ASSET_ROOT" "$PROJ/ui/hub.html" <<'JSEOF'
const fs = require('fs'), path = require('path'), zlib = require('zlib');
const [root, hub] = process.argv.slice(2);
const want = fs.readFileSync(hub);
const hits = fs.readdirSync(root).filter(f => {
  try {
    const out = zlib.brotliDecompressSync(fs.readFileSync(path.join(root, f)));
    return out.length === want.length && out.equals(want);
  } catch (e) { return false; }
});
if (!hits.length) { console.error('  ✗ 没找到与 ui/hub.html 字节一致的 codegen 资产（codegen 没重编？）'); process.exit(1); }
console.log('  ✓ hub.html 资产字节一致（' + want.length + ' 字节）→ ' + hits[0].slice(0, 12) + '…');
JSEOF
else
  echo "  · 跳过（找不到 $PROJ/src-tauri/target/release/build/anyboot-*/out）"
fi

echo "==> 自检 B：每个 ELF 的依赖都能在包内解出"
MISS=0
while read -r f; do
  if ! LD_LIBRARY_PATH="$APPDIR/usr/lib" ldd "$f" 2>/dev/null | grep -q 'not found'; then continue; fi
  echo "  ✗ $f:"; LD_LIBRARY_PATH="$APPDIR/usr/lib" ldd "$f" 2>/dev/null | grep 'not found' | sed 's/^/    /'
  MISS=1
done < <(find "$APPDIR/usr/bin" "$APPDIR/usr/lib" -type f -exec file {} \; | grep -E 'ELF (64-bit.*dynamically linked|pie)' | cut -d: -f1)
[ "$MISS" = 0 ] || { echo "  ✗ 有未解出依赖"; exit 1; }

echo "==> 打包（runtime = 原包前 N 字节的 ELF，N 由 --appimage-offset 实测；offset 之后是 squashfs 数据）"
RT=$BUILD/runtime-x86_64
OFF=$($ORIG --appimage-offset)   # 问包自己，绝不猜（grep 'hsqs' 会命中 runtime 内部假阳性）
head -c "$OFF" "$ORIG" > "$RT"
chmod +x "$RT"
cd "$BUILD"
set +e
# ARCH=x86_64 必须显式给：tauri 的 resources 里带了 arm64 的 tsnet sidecar，包内同时
# 存在两个架构的二进制，appimagetool 自检会说「More than one architectures were found」
# 然后拒绝打包（不是我们的包真有错，是它按 ELF 猜架构）。runtime 本身也只有 x86_64 一份。
ARCH=x86_64 "$APPIMAGETOOL" --runtime-file "$RT" "$APPDIR" "$OUT"
RC=$?
set -e
# appimagetool 收尾偶发段错误（上游已知），以产物存在、自报偏移且**是本次新建**为准
[ -f "$OUT" ] || { echo "✗ 打包失败 (rc=$RC)"; exit 1; }
[ "$OUT" -nt "$BUILD/AppDir/apprun-hooks" ] || { echo "✗ 产物时间戳早于本次构建（mksquashfs 可能静默失败，检查文件占用）"; exit 1; }

echo "==> 终验"
"$OUT" --appimage-offset >/dev/null 2>&1 || { echo "✗ 产物 --appimage-offset 失败"; exit 1; }
echo "  --appimage-offset 自报: $("$OUT" --appimage-offset 2>/dev/null)"
/usr/bin/rm -rf "$BUILD/extract-check"; mkdir -p "$BUILD/extract-check"; cd "$BUILD/extract-check"
"$OUT" --appimage-extract >/dev/null 2>&1 || { echo "✗ 产物 --appimage-extract 失败"; exit 1; }
N=$(find squashfs-root -type f | wc -l); echo "  --appimage-extract 解出 $N 个文件"
[ "$N" -gt 400 ] || { echo "✗ 解出文件数异常，包可能不完整"; exit 1; }
MAX2=$(find squashfs-root -type f \( -name '*.so*' -o -name 'anyboot' \) -exec sh -c \
  'objdump -T "$1" 2>/dev/null | grep -o "GLIBC_[0-9.]*"' _ {} \; | sed 's/GLIBC_//' | sort -Vu | tail -1)
echo "  产物全包 GLIBC 门槛 = $MAX2"
[ "$MAX2" = "$(printf '%s\n2.35\n' "$MAX2" | sort -Vu | head -1)" ] || { echo "✗ 产物门槛超过 2.35"; exit 1; }
ls -la "$OUT"
echo "✓ 完成: $OUT"
