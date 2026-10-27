#!/usr/bin/env bash
# build-sidecar.sh —— 交叉编译内置 Tailscale 引擎 sidecar（全部 5 个目标）。
#
# 产物（src-tauri/binaries/，由 tauri bundle.resources 分平台打包）：
#   anyboot-tsnet.exe                  Windows（在 Windows 本机或 GOOS=windows 构建）
#   anyboot-tsnet-linux-amd64          Linux x86_64
#   anyboot-tsnet-linux-arm64          Linux aarch64（树莓派 / ARM 服务器）
#   anyboot-tsnet-darwin-amd64         macOS Intel
#   anyboot-tsnet-darwin-arm64         macOS Apple Silicon
#
# 依赖：真实 Go 工具链（go.mod 指定的版本，gccgo 编不了 tailscale.com/tsnet）。
#   默认依次找：$GO_BIN → $HOME/.workbuddy/binaries/go/go/bin/go → PATH 里的 go。
#   （本机旧路径 /home/jianghh/.workbuddy/... 已废除写死，改为 $HOME 相对，
#    否则 CI/他人机器上 mkdir /home/jianghh 直接 Permission denied。）
#
# 坑（2026-10-25 实测）：
#   - /tmp 是 10MB tmpfs，Go 编译器 $WORK 写不下 → 必须 TMPDIR 指到大盘。
#   - 国内网络 → GOPROXY=goproxy.cn。
#   - 一律 CGO_ENABLED=0：tsnet 纯 Go，静态链接免运行时依赖，且才能跨编 darwin。
set -euo pipefail

SIDECAR_DIR="$(cd "$(dirname "$0")/../src-tauri/sidecar-tsnet" && pwd)"
OUT_DIR="$SIDECAR_DIR/../binaries"

GO_BIN="${GO_BIN:-$HOME/.workbuddy/binaries/go/go/bin/go}"
if [ ! -x "$GO_BIN" ]; then
  GO_BIN="$(command -v go || true)"
fi
if [ -z "${GO_BIN:-}" ]; then
  echo "错误：找不到 Go 工具链（默认 \$HOME/.workbuddy/binaries/go/go/bin/go 或 PATH 里的 go）" >&2
  exit 1
fi

# 代理：CI（GitHub Actions 等）走官方源，本机国内网络默认 goproxy.cn；均可 env 覆盖
if [ -z "${GOPROXY:-}" ]; then
  if [ -n "${CI:-}" ]; then
    export GOPROXY="https://proxy.golang.org,direct"
  else
    export GOPROXY="https://goproxy.cn,direct"
  fi
fi
GO_WORK="$HOME/.cache/anyboot-sidecar-go"
export GOCACHE="${GOCACHE:-$GO_WORK/gocache}"
export GOPATH="${GOPATH:-$GO_WORK/gopath}"
export TMPDIR="${TMPDIR:-$GO_WORK/tmp}"
mkdir -p "$GOCACHE" "$GOPATH" "$TMPDIR"

cd "$SIDECAR_DIR"
echo "== go test（原生目标） =="
"$GO_BIN" test ./...

LDFLAGS="-s -w"
build() { # build <goos> <goarch> <outname>
  echo "== build $1/$2 -> $3 =="
  CGO_ENABLED=0 GOOS="$1" GOARCH="$2" "$GO_BIN" build -trimpath -ldflags "$LDFLAGS" \
    -o "$OUT_DIR/$3" .
}

build linux amd64  anyboot-tsnet-linux-amd64
build linux arm64  anyboot-tsnet-linux-arm64
build darwin amd64 anyboot-tsnet-darwin-amd64
build darwin arm64 anyboot-tsnet-darwin-arm64

# Windows：在 Linux/mac 上同样能交叉编译；在 Windows 本机跑本脚本则用原生 go build
if [ "$(uname -s)" = "Windows" ] || uname -s | grep -qi mingw; then
  build windows amd64 anyboot-tsnet.exe
else
  CGO_ENABLED=0 GOOS=windows GOARCH=amd64 "$GO_BIN" build -trimpath -ldflags "$LDFLAGS" \
    -o "$OUT_DIR/anyboot-tsnet.exe" .
fi

echo "== 产物 =="
ls -la "$OUT_DIR"
