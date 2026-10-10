#!/usr/bin/env bash
# =============================================================================
#  AnyBoot · Linux 端 GRUB 数据收集脚本
#  在 Arch / Debian / Ubuntu / Fedora 等使用 GRUB 的系统上，以 root 运行一次。
#
#  ── 为什么必须这样做 ────────────────────────────────────────────────────────
#   GRUB 启动用的 /boot/grub/grub.cfg 是 `grub-mkconfig`(=`update-grub`) 的
#   **生成产物**。直接改 grub.cfg，下次 update-grub 就会被整份覆盖。
#   正确做法是改“生成器” —— 也就是 /etc/grub.d/ 里的脚本：
#       · 本脚本安装 /etc/grub.d/09_anyboot
#       · 由它把 GRUB 的“环境块”(grubenv) 从 ext4 的 /boot/grub/ 迁到 ESP(FAT32)
#       · 以后无论跑多少次 update-grub，这套设置都被重新编译进 grub.cfg，不会丢
#
#  ── 本脚本做的四件事 ────────────────────────────────────────────────────────
#   1) 安装 /etc/grub.d/09_anyboot        —— grubenv 外置到 ESP（一次安装，永久生效）
#   2) 安装 /usr/local/sbin/anyboot-grub-export + systemd path 单元
#                                          —— grub.cfg 一被重新生成，就自动复制到 ESP
#   3) 在 ESP 上准备 grubenv（1024 字节）并从 ext4 迁移现有 saved_entry/next_entry
#   4) 备份 + 立即执行一次 grub-mkconfig 并同步
#
#  ── 收集完成后，Windows / macOS 需要什么 ────────────────────────────────────
#   只读 ESP 上这一个目录即可（纯 FAT32，**不需要 ext4 工具、不需要提权**）：
#       <ESP>/<SUBDIR>/grub.cfg            菜单全文（含每个条目的 entry-id）
#       <ESP>/<SUBDIR>/grubenv             状态（saved_entry / next_entry）
#       <ESP>/<SUBDIR>/anyboot-grub.json   解析好的机器可读清单
#
#  ── 用法 ────────────────────────────────────────────────────────────────────
#       sudo ./anyboot-grub-setup.sh                     # 安装 / 重新安装
#       sudo ./anyboot-grub-setup.sh --esp-subdir EFI/arch-efi
#       sudo ./anyboot-grub-setup.sh --uninstall         # 卸载（恢复用 ext4 grubenv）
# =============================================================================

set -euo pipefail

# ---------------------------------------------------------------- 可配置项 ---
ESP_SUBDIR="${ESP_SUBDIR:-EFI/arch-efi}"   # ESP 上存放 grub.cfg/grubenv 的子目录
GRUB_CFG="${GRUB_CFG:-/boot/grub/grub.cfg}"
GRUBENV_HOST="${GRUBENV_HOST:-/boot/grub/grubenv}"
GRUBD_DIR="/etc/grub.d"
SNIPPET="$GRUBD_DIR/09_anyboot"
EXPORT_BIN="/usr/local/sbin/anyboot-grub-export"
AWK_LIB="/usr/local/lib/anyboot/grub-manifest.awk"
UNIT_SVC="/etc/systemd/system/anyboot-grub-export.service"
UNIT_PATH="/etc/systemd/system/anyboot-grub-export.path"
BACKUP_ROOT="/var/backups/anyboot"

BL=$'\033[1;34m'; YE=$'\033[1;33m'; RD=$'\033[1;31m'; NC=$'\033[0m'
log()  { printf '%s[AnyBoot]%s %s\n' "$BL" "$NC" "$*"; }
warn() { printf '%s[AnyBoot]%s %s\n' "$YE" "$NC" "$*" >&2; }
die()  { printf '%s[AnyBoot]%s %s\n' "$RD" "$NC" "$*" >&2; exit 1; }

usage() {
  sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
}

# ------------------------------------------------------------------- 参数 ----
UNINSTALL=0
while [ $# -gt 0 ]; do
  case "$1" in
    --uninstall)  UNINSTALL=1; shift ;;
    --esp-subdir) [ $# -ge 2 ] || die "--esp-subdir 缺少参数"; ESP_SUBDIR="$2"; shift 2 ;;
    -h|--help)    usage; exit 0 ;;
    *)            die "未知参数: $1（用 --help 查看用法）" ;;
  esac
done

[ "$(id -u)" -eq 0 ] || die "请用 root 运行：sudo $0 $*"
command -v blkid   >/dev/null || die "缺少 blkid（util-linux）"
command -v findmnt >/dev/null || die "缺少 findmnt（util-linux）"

ESP_MNT=""; ESP_DEV=""; ESP_UUID=""; ESP_DIR=""; GRUBENV_REL=""

# --------------------------------------------------------------- 探测 ESP ----
detect_esp() {
  local cand fstype src mnt
  # 1) 常见挂载点优先：/efi、/boot/efi、/boot（Arch 常把 ESP 直接挂 /boot）
  for cand in /efi /boot/efi /boot; do
    if mountpoint -q "$cand" 2>/dev/null; then
      fstype=$(findmnt -no FSTYPE --target "$cand" 2>/dev/null || echo "")
      if [ "$fstype" = "vfat" ]; then
        ESP_MNT="$cand"
        ESP_DEV=$(findmnt -no SOURCE --target "$cand")
        ESP_UUID=$(blkid -s UUID -o value "$ESP_DEV" 2>/dev/null || true)
        [ -n "$ESP_UUID" ] && return 0
      fi
    fi
  done
  # 2) 兜底：扫描所有 vfat 挂载点里带 EFI/ 目录的
  while read -r src mnt; do
    [ -n "${mnt:-}" ] || continue
    if [ -d "$mnt/EFI" ]; then
      ESP_MNT="$mnt"; ESP_DEV="$src"
      ESP_UUID=$(blkid -s UUID -o value "$src" 2>/dev/null || true)
      [ -n "$ESP_UUID" ] && return 0
    fi
  done < <(findmnt -rno SOURCE,TARGET -t vfat 2>/dev/null)
  return 1
}

# ------------------------------------------------------- 安装 grub.d 生成器 ---
install_snippet() {
  log "安装 grub.d 生成器：$SNIPPET"
  cat > "$SNIPPET" <<'GRUBEOF'
#!/bin/sh
exec tail -n +3 $0
# ============================================================================
#  AnyBoot —— 由 anyboot-grub-setup.sh 生成，请勿手改。
#  重新生成： sudo anyboot-grub-setup.sh
#
#  ★ 开头两行 `#!/bin/sh` 与 `exec tail -n +3 $0` 是 shell 包装。
#    grub-mkconfig 会「执行」本文件并读取它的 stdout；包装行负责把下面这段
#    GRUB 脚本原样打印出去。删掉它们，grub-mkconfig 就会拿 sh 去跑 GRUB 命令，
#    于是报 `search: 未找到命令` / `未预期的记号 "("。**不要删！**
#
#  作用：把 GRUB 的“环境块”(grubenv) 从 ext4 的 /boot/grub/ 迁到 ESP(FAT32)：
#          @@ENV_DISPLAY@@
#  这样 Windows / macOS 无需 ext4 支持、无需提权，即可读写“上次/下次启动项”。
#
#  为什么写在 grub.d 而不是直接改 grub.cfg：
#    grub.cfg 是 grub-mkconfig/update-grub 的生成产物，直接改会被下次覆盖；
#    本文件是生成器，update-grub 会把它重新编译进去，因此永久生效。
#
#  排序：09 —— 在 00_header 之后、10_linux 之前。
#    因此这里重设 default、重定义 savedefault 会覆盖 00_header 中指向 ext4 的
#    旧逻辑（GRUB 在渲染菜单前会执行完整份 grub.cfg，最后写入的状态胜出）。
# ============================================================================

insmod part_gpt
insmod part_msdos
insmod fat

if search --no-floppy --fs-uuid --set=anyboot_esp @@ESP_UUID@@ ; then
  set anyboot_env="($anyboot_esp)@@ENV_REL@@"
  export anyboot_esp
  export anyboot_env

  # 1) 从 ESP 上的环境块读取状态（saved_entry / next_entry / 任意自定义键）
  if [ -s "${anyboot_env}" ] ; then
    load_env -f "${anyboot_env}"
  fi

  # 2) 用 ESP 的状态决定 default（覆盖 00_header 基于内建 grubenv 的结论）。
  #    若 00_header 已取到一次性项（Linux 侧 grub-reboot / grub-editenv），
  #    它会置 boot_once=true —— 此时不再覆盖，尊重那一次的选择。
  if [ -z "${boot_once}" ] ; then
    if [ "${next_entry}" ] ; then
      set default="${next_entry}"
      set next_entry=
      save_env -f "${anyboot_env}" next_entry
      set boot_once=true
    elif [ "${saved_entry}" ] ; then
      set default="${saved_entry}"
    fi
  fi
fi

# 3) 覆盖 00_header 的 savedefault：把“记住上次选择”写回 ESP。
#    定义在最外层（不嵌在 if 里），ESP 若找不到会自动降级回内建 grubenv。
function savedefault {
  if [ -z "${boot_once}" ] ; then
    saved_entry="${chosen}"
    if [ "${anyboot_env}" ] ; then
      save_env -f "${anyboot_env}" saved_entry
    else
      save_env saved_entry
    fi
  fi
}
GRUBEOF
  sed -i \
    -e "s#@@ESP_UUID@@#$ESP_UUID#g" \
    -e "s#@@ENV_REL@@#$GRUBENV_REL#g" \
    -e "s#@@ENV_DISPLAY@@#$ESP_DIR/grubenv#g" \
    "$SNIPPET"
  chmod 755 "$SNIPPET"

  # --- 自检 1：shell 包装行必须在。不在这行，grub-mkconfig 会拿 sh 去跑 GRUB 命令 ---
  if ! head -n 2 "$SNIPPET" | grep -q 'exec tail -n +3'; then
    rm -f "$SNIPPET"
    die "$SNIPPET 缺少 shell 包装行（#!/bin/sh + exec tail -n +3 \$0），已删除以免破坏引导"
  fi

  # --- 自检 2：用 grub-script-check 验证正文是合法 GRUB 脚本 ---
  if command -v grub-script-check >/dev/null 2>&1; then
    local _body
    _body="$(mktemp /tmp/anyboot-grubcheck.XXXXXX)"
    tail -n +3 "$SNIPPET" > "$_body"
    if grub-script-check "$_body" >/dev/null 2>&1; then
      log "GRUB 脚本语法自检：通过"
    else
      warn "被检查的正文：$_body"
      rm -f "$SNIPPET"
      die "$SNIPPET 的 GRUB 语法自检失败，已中止且删除该文件（未触碰 grub.cfg）"
    fi
    rm -f "$_body"
  else
    warn "未找到 grub-script-check（属 grub 包），跳过语法自检"
  fi
}

# ------------------------------------------- 安装同步器（grub.cfg -> ESP） ---
install_export() {
  log "安装同步器：$EXPORT_BIN"
  install -d -m 755 "$(dirname "$AWK_LIB")"

  cat > "$AWK_LIB" <<'AWKEOF'
# AnyBoot · grub.cfg -> anyboot-grub.json
# 用法: awk -v gens=<ISO8601> -v src=<path> -f grub-manifest.awk <grub.cfg>
function jesc(s) { gsub(/\\/, "\\\\", s); gsub(/"/, "\\\"", s); gsub(/\t/, "\\t", s); return s }

function first_quoted(s,   i, j, q, r, k) {
  i = index(s, "'"); j = index(s, "\"")
  if (i == 0 && j == 0) return ""
  if (j == 0)      q = "'"
  else if (i == 0) q = "\""
  else if (i < j)  q = "'"
  else             q = "\""
  i = index(s, q); r = substr(s, i + 1); k = index(r, q)
  if (k == 0) return ""
  return substr(r, 1, k - 1)
}

function last_index(s, m,   start, p, found) {
  start = 1; found = 0
  while (start <= length(s)) {
    p = index(substr(s, start), m)
    if (p == 0) break
    found = start + p - 1
    start = found + 1
  }
  return found
}

# 取 marker 之后的下一个引号串。用“最后一次出现”的 marker：
# os-prober 偶尔会把 "$menuentry_id_option" 文本写进菜单名里，此时真正的 id 在行尾。
function after_marker(s, m,   p) {
  p = last_index(s, m)
  if (p == 0) return ""
  return first_quoted(substr(s, p))
}

function extract_id(s) {
  if (index(s, "$menuentry_id_option")) return after_marker(s, "$menuentry_id_option")
  if (index(s, "--id"))                 return after_marker(s, "--id")
  if (index(s, "--unrestricted"))       return after_marker(s, "--unrestricted")
  return ""
}

function indent_of(s,   n, t) {
  n = 0; t = s
  while (substr(t, 1, 1) == "\t") { n++; t = substr(t, 2) }
  return n
}

BEGIN { nE = 0; nS = 0; cur_sub = ""; cur_sub_id = "" }

{
  line = $0
  ind  = indent_of(line)
  if (line ~ /^[ \t]*submenu[ \t]/) {
    nm = first_quoted(line); id = extract_id(line)
    nS++; s_name[nS] = nm; s_id[nS] = id; s_level[nS] = ind
    cur_sub = nm; cur_sub_id = id
    next
  }
  if (line ~ /^[ \t]*menuentry[ \t]/) {
    nm = first_quoted(line); id = extract_id(line)
    nE++; e_name[nE] = nm; e_id[nE] = id
    if (ind > 0) { e_sub[nE] = cur_sub; e_subid[nE] = cur_sub_id }
    else         { e_sub[nE] = "";       e_subid[nE] = "" }
    next
  }
}

END {
  print "{"
  print "  \"format\": \"anyboot-grub/1\","
  printf "  \"source\": \"%s\",\n", jesc(src)
  printf "  \"generated\": \"%s\",\n", jesc(gens)
  print "  \"entries\": ["
  for (i = 1; i <= nE; i++)
    printf "    {\"name\": \"%s\", \"id\": \"%s\", \"submenu\": \"%s\", \"submenu_id\": \"%s\"}%s\n", \
      jesc(e_name[i]), jesc(e_id[i]), jesc(e_sub[i]), jesc(e_subid[i]), (i < nE ? "," : "")
  print "  ],"
  print "  \"submenus\": ["
  for (i = 1; i <= nS; i++)
    printf "    {\"name\": \"%s\", \"id\": \"%s\", \"level\": %d}%s\n", \
      jesc(s_name[i]), jesc(s_id[i]), s_level[i], (i < nS ? "," : "")
  print "  ]"
  print "}"
}
AWKEOF
  chmod 644 "$AWK_LIB"

  cat > "$EXPORT_BIN" <<EXPORTEOF
#!/usr/bin/env bash
# AnyBoot —— 由 anyboot-grub-setup.sh 生成，请勿手改。
# 把 $GRUB_CFG 同步到 ESP，并生成机器可读清单 anyboot-grub.json
set -euo pipefail
ESP_MNT="$ESP_MNT"
SUBDIR="$ESP_SUBDIR"
SRC="$GRUB_CFG"
AWK_LIB="$AWK_LIB"
DST="\$ESP_MNT/\$SUBDIR"

if ! mountpoint -q "\$ESP_MNT" 2>/dev/null; then
  mount "\$ESP_MNT" 2>/dev/null || true
fi
if [ ! -d "\$ESP_MNT" ]; then
  echo "anyboot-grub-export: ESP 未挂载: \$ESP_MNT" >&2
  exit 1
fi
mkdir -p "\$DST"
if [ ! -f "\$SRC" ]; then
  echo "anyboot-grub-export: 未找到 \$SRC" >&2
  exit 1
fi

# 原子替换，避免 GRUB/AnyBoot 读到半截文件
tmp="\$DST/.grub.cfg.new.\$\$"
cp -f "\$SRC" "\$tmp"
mv -f "\$tmp" "\$DST/grub.cfg"

ts=\$(date -u +%Y-%m-%dT%H:%M:%SZ)
mtmp="\$DST/.anyboot-grub.json.new.\$\$"
if awk -v gens="\$ts" -v src="\$SRC" -f "\$AWK_LIB" "\$SRC" > "\$mtmp" 2>/dev/null; then
  mv -f "\$mtmp" "\$DST/anyboot-grub.json"
else
  rm -f "\$mtmp"
fi
sync
echo "anyboot-grub-export: 已同步 \$SRC -> \$DST/{grub.cfg,anyboot-grub.json}"
EXPORTEOF
  chmod 755 "$EXPORT_BIN"
}

# ------------------------------------------------------- systemd 自动同步 ----
install_units() {
  if ! command -v systemctl >/dev/null 2>&1; then
    warn "未检测到 systemd，跳过自动同步（可手动运行 $EXPORT_BIN，或在 update-grub 后跑一次）"
    return 0
  fi
  log "安装 systemd 自动同步单元"
  cat > "$UNIT_SVC" <<EOF
[Unit]
Description=AnyBoot: 把 $GRUB_CFG 同步到 ESP
After=local-fs.target

[Service]
Type=oneshot
ExecStart=$EXPORT_BIN
EOF
  cat > "$UNIT_PATH" <<EOF
[Unit]
Description=AnyBoot: 监视 $GRUB_CFG 的变化

[Path]
PathChanged=$GRUB_CFG
Unit=$(basename "$UNIT_SVC")

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  if systemctl enable --now "$(basename "$UNIT_PATH")" >/dev/null 2>&1; then
    log "自动同步已启用（$(basename "$UNIT_PATH")）"
  else
    warn "启用 path 单元失败；可手动：systemctl enable --now $(basename "$UNIT_PATH")"
  fi
}

# ------------------------------------------------------ ESP 上的 grubenv ----
ensure_grubenv() {
  local f="$ESP_DIR/grubenv"
  mkdir -p "$ESP_DIR"
  if [ -f "$f" ]; then
    log "ESP 上已存在 grubenv，保留：$f"
  else
    log "在 ESP 上创建 1024 字节 grubenv：$f"
    if command -v grub-editenv >/dev/null 2>&1; then
      grub-editenv "$f" create
    else
      { printf '# GRUB Environment Block\n'; printf '#%.0s' $(seq 1 1024); } > "$f"
      truncate -s 1024 "$f"
    fi
  fi

  # 从 ext4 旧 grubenv 迁移 saved_entry / next_entry（仅当 ESP 里还没有该键）
  if [ -f "$GRUBENV_HOST" ] && command -v grub-editenv >/dev/null 2>&1; then
    local k v
    for k in saved_entry next_entry; do
      v=$(grub-editenv "$GRUBENV_HOST" list 2>/dev/null | sed -n "s/^$k=//p" | head -1 || true)
      if [ -n "$v" ] && ! grub-editenv "$f" list 2>/dev/null | grep -q "^$k="; then
        grub-editenv "$f" set "$k=$v"
        log "迁移 $k=$v （ext4 -> ESP）"
      fi
    done
  fi
  log "grubenv 大小：$(stat -c '%s 字节' "$f" 2>/dev/null || echo '?')"
}

# ------------------------------------------------------------------ 备份 ----
do_backup() {
  local ts dir
  ts=$(date +%Y%m%d_%H%M%S)
  dir="$BACKUP_ROOT/$ts"
  mkdir -p "$dir"
  if [ -f "$GRUB_CFG" ]; then cp -a "$GRUB_CFG" "$dir/grub.cfg"; log "备份 $GRUB_CFG -> $dir/"; fi
  if [ -f "$GRUBENV_HOST" ]; then cp -a "$GRUBENV_HOST" "$dir/grubenv"; log "备份 $GRUBENV_HOST -> $dir/"; fi
  if [ -f "$ESP_DIR/grubenv" ]; then cp -a "$ESP_DIR/grubenv" "$dir/esp-grubenv"; fi
  printf '%s' "$dir" > /tmp/.anyboot_last_backup
}

# --------------------------------------------------------------- 重新生成 ----
regen() {
  if command -v grub-mkconfig >/dev/null 2>&1; then
    grub-mkconfig -o "$GRUB_CFG"
  elif command -v update-grub >/dev/null 2>&1; then
    update-grub
  else
    die "找不到 grub-mkconfig / update-grub"
  fi
}

# ------------------------------------------------------------------ 卸载 ----
do_uninstall() {
  log "卸载 AnyBoot 的 Linux 端配置"
  systemctl disable --now anyboot-grub-export.path >/dev/null 2>&1 || true
  rm -f "$UNIT_PATH" "$UNIT_SVC" "$EXPORT_BIN" "$SNIPPET"
  rm -rf "$(dirname "$AWK_LIB")"
  systemctl daemon-reload >/dev/null 2>&1 || true
  log "已删除 grub.d 生成器 / 同步器 / systemd 单元"
  log "重新生成一份干净的 grub.cfg ..."
  regen
  log "完成。ESP 上的 grub.cfg / grubenv / anyboot-grub.json 保留未删（如需删除请手动）。"
  warn "注意：卸载后 GRUB 将重新使用 ext4 的 $GRUBENV_HOST。"
}

# ------------------------------------------------------------------ 主流程 ---
main() {
  detect_esp || die "找不到 EFI 系统分区(ESP)。请确认 ESP 已挂载到 /efi 或 /boot/efi（或 /boot）。"
  ESP_DIR="$ESP_MNT/$ESP_SUBDIR"
  GRUBENV_REL="/$ESP_SUBDIR/grubenv"

  log "ESP 设备    : $ESP_DEV"
  log "ESP 挂载点  : $ESP_MNT"
  log "ESP fs-uuid : $ESP_UUID   (GRUB 的 search --fs-uuid 用它)"
  log "目标目录    : $ESP_DIR"

  [ -f "$GRUB_CFG" ] || die "找不到 $GRUB_CFG —— 这台机器的 GRUB 配置似乎不在默认位置"

  do_backup
  install_snippet
  install_export
  install_units
  ensure_grubenv

  log "重新生成 grub.cfg（让 09_anyboot 生效）..."
  regen

  log "首次同步到 ESP ..."
  "$EXPORT_BIN"

  cat <<EOF

$(printf '%s[AnyBoot]%s' "$BL" "$NC") 安装完成。

  · 生成器   : $SNIPPET
  · 同步器   : $EXPORT_BIN  （systemd: $(basename "$UNIT_PATH") 监听 $GRUB_CFG）
  · ESP 数据 : $ESP_DIR/
        grub.cfg            ← 菜单全文（含 entry-id）
        grubenv             ← 状态（saved_entry / next_entry）
        anyboot-grub.json   ← 机器可读清单

  · 以后无论怎样跑 update-grub / grub-mkconfig，这两样都会保持：
      - grubenv 落在 ESP（由 09_anyboot 保证）
      - ESP 上的 grub.cfg / anyboot-grub.json 自动刷新（由 path 单元保证）

  · 恢复原状： sudo $0 --uninstall

EOF
  warn "改动 grub 有风险。已备份到 $(cat /tmp/.anyboot_last_backup 2>/dev/null || echo "$BACKUP_ROOT")/"
}

if [ "$UNINSTALL" -eq 1 ]; then
  do_uninstall
else
  main
fi
