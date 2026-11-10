#!/usr/bin/env bash
# 测试用快速重载：只构建前端 + gen + 编译 release 二进制 + 重启，不打包 deb/rpm/AppImage。
# 打包（deb/rpm/AppImage）只在发交付版时用 `npm run build:linux`。
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

echo "== [1/3] 前端构建 + gen ui_assets"
if command -v bun >/dev/null 2>&1; then
  bun run node_modules/vite/bin/vite.js build --config ui-vue/vite.config.ts || exit 1
else
  npm run build:vue || exit 1
fi
python3 scripts/gen-ui-assets.py || exit 1

echo "== [2/3] 编译 release 二进制（不打包）"
( cd src-tauri && NO_STRIP=1 cargo build --release --bin anyboot ) || exit 1

echo "== [3/3] 重启"
pkill -x anyboot 2>/dev/null
P=$(ss -tlnp 2>/dev/null | grep 12300 | grep -oP 'pid=\K[0-9]+')
[ -n "$P" ] && kill -9 "$P" 2>/dev/null
sleep 1
( cd src-tauri/target/release && DISPLAY=:0 setsid ./anyboot > /tmp/anyboot-dev.log 2>&1 < /dev/null & )
sleep 4
if ss -tlnp 2>/dev/null | grep -q 12300; then
  echo "== 就绪：http://127.0.0.1:12300  pid=$(pgrep -x anyboot)"
else
  echo "!! 没起来，看 /tmp/anyboot-dev.log"; tail -5 /tmp/anyboot-dev.log
fi
