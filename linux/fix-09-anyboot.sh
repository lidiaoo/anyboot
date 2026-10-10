#!/usr/bin/env bash
# =============================================================================
#  AnyBoot · 修复 / 安装 /etc/grub.d/09_anyboot
#
#  作用：把 GRUB 的"环境块"(grubenv) 从 ext4 的 /boot/grub/ 迁到 ESP(FAT32)，
#        并顺手把 grub.cfg 复制一份到 ESP。这样 Windows / macOS 端只需读写
#        ESP 上那一个目录，**不需要 ext4 工具、不需要提权**。
#
#  ── 为什么上一版会报错 ──────────────────────────────────────────────────────
#   /etc/grub.d/ 下的每个文件，grub-mkconfig 的处理方式是：
#         「执行它 → 读它的 stdout → 把输出拼进 grub.cfg」
#   所以这个文件必须是 **shell 脚本**（开头 #!/bin/sh + exec tail -n +3 $0），
#   由 shell 把后面的 GRUB 脚本原样打印出去。
#   如果直接把 GRUB 命令裸写进该文件，grub-mkconfig 会拿 sh 去执行它们，于是：
#        /etc/grub.d/09_anyboot: 行 1: search: 未找到命令
#        /etc/grub.d/09_anyboot: 行 2: 未预期的记号 "(" 附近有语法错误
#   本脚本生成的文件带正确的 shell 包装，并在写完后用 grub-script-check 自检。
#
#  ── 用法 ────────────────────────────────────────────────────────────────────
#        sudo bash fix-09-anyboot.sh                 # 安装 / 覆盖修复
#        sudo ESP_SUBDIR=EFI/arch-efi bash fix-09-anyboot.sh
#        sudo bash fix-09-anyboot.sh --uninstall     # 卸载（恢复用 ext4 grubenv）
# =============================================================================

set -euo pipefail

ESP_SUBDIR="${ESP_SUBDIR:-EFI/arch-efi}"    # ESP 上存放 grub.cfg / grubenv 的子目录
GRUB_CFG="${GRUB_CFG:-/boot/grub/grub.cfg}"
GRUBENV_HOST="${GRUBENV_HOST:-/boot/grub/grubenv}"
SNIPPET="/etc/grub.d/09_anyboot"

BL=$'\033[1;34m'; YE=$'\033[1;33m'; RD=$'\033[1;31m'; NC=$'\033[0m'
log()  { printf '%s[AnyBoot]%s %s\n' "$BL" "$NC" "$*"; }
warn() { printf '%s[AnyBoot]%s %s\n' "$YE" "$NC" "$*" >&2; }
die()  { printf '%s[AnyBoot]%s %s\n' "$RD" "$NC" "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "请用 root 运行： sudo bash $0"

ESP_MNT=""; ESP_DEV=""; ESP_UUID=""
detect_esp() {
  local cand fstype
  for cand in /efi /boot/efi /boot; do
    if mountpoint -q "$cand" 2>/dev/null \
       && [ "$(findmnt -no FSTYPE --target "$cand" 2>/dev/null || echo '')" = "vfat" ]; then
      ESP_MNT="$cand"; ESP_DEV="$(findmnt -no SOURCE --target "$cand")"; return 0
    fi
  done
  while read -r src mnt; do
    [ -n "${mnt:-}" ] || continue
    if [ -d "$mnt/EFI" ]; then ESP_MNT="$mnt"; ESP_DEV="$src"; return 0; fi
  done < <(findmnt -rno SOURCE,TARGET -t vfat 2>/dev/null)
  return 1
}

write_snippet() {
  log "写入生成器：$SNIPPET"
  # ── 注意：heredoc 用引号（'GRUBEOF'）→ 里面所有 $ 都保持字面量，
  #    这样 GRUB 的 ${next_entry} / $0 不会被 shell 就地展开。
  #    需要填入的值（ESP UUID / 路径）用 @@占位符@@，写完再 sed 替换。
  cat > "$SNIPPET" <<'GRUBEOF'
#!/bin/sh
exec tail -n +3 "$0"
# ============================================================================
#  AnyBoot —— 由 fix-09-anyboot.sh 生成，请勿手改。
#  重新生成： sudo bash fix-09-anyboot.sh
#
#  ★ 开头两行 `#!/bin/sh` 与 `exec tail -n +3 "$0"` 是 shell 包装。
#    grub-mkconfig 会「执行」本文件并读取它的 stdout；包装行负责把下面这段
#    GRUB 脚本原样打印出去。删掉它们，grub-mkconfig 就会拿 sh 去跑 GRUB 命令，
#    于是报 `search: 未找到命令` / `未预期的记号 "("`。**不要删！**
#
#  作用：把 GRUB 的"环境块"(grubenv) 从 ext4 的 /boot/grub/ 迁到 ESP(FAT32)：
#          @@ENV_DISPLAY@@
#  这样 Windows / macOS 无需 ext4 支持、无需提权，即可读写"上次/下次启动项"。
#
#  为什么写在 grub.d 而不是直接改 grub.cfg：
#    grub.cfg 是 grub-mkconfig/update-grub 的生成产物，直接改会被下次覆盖；
#    本文件是生成器，update-grub 会把它重新编译进去，因此永久生效。
#
#  排序：09 —— 在 00_header 之后、10_linux 之前。GRUB 在渲染菜单前会把整份
#    grub.cfg 执行一遍，所以这里重设 default、重定义 savedefault 会覆盖
#    00_header 里指向 ext4 的旧逻辑（后写入者胜出）。
#
#  模块依赖：search / load_env / save_env 在内核镜像里已内置（本机原 grub.cfg
#    全程未 insmod 它们）；fat 未内置，必须显式 insmod，否则 search 探测不到 ESP。
# ============================================================================

insmod part_gpt
insmod part_msdos
insmod fat

# 1) 通过卷序列号定位 ESP（GRUB 的 --fs-uuid 用的就是这个，形如 E537-7F31）
if search --no-floppy --fs-uuid --set=anyboot_esp @@ESP_UUID@@ ; then
  set anyboot_env="($anyboot_esp)@@ENV_REL@@"
  export anyboot_esp
  export anyboot_env

  # 2) 从 ESP 上的环境块读取状态（saved_entry / next_entry / 任意自定义键）
  if [ -s "${anyboot_env}" ] ; then
    load_env -f "${anyboot_env}"
  fi

  # 3) 用 ESP 的状态决定 default（覆盖 00_header 基于内建 grubenv 的结论）。
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

# 4) 覆盖 00_header 的 savedefault：把"记住上次选择"写回 ESP。
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
    -e "s#@@ENV_REL@@#$ENV_REL#g" \
    -e "s#@@ENV_DISPLAY@@#$ESP_MNT/$ESP_SUBDIR/grubenv#g" \
    "$SNIPPET"
  chmod 755 "$SNIPPET"

  # 自检 1：shell 包装行必须在
  if ! head -n 2 "$SNIPPET" | grep -q 'exec tail -n +3'; then
    rm -f "$SNIPPET"
    die "$SNIPPET 缺少 shell 包装行，已删除以免破坏引导"
  fi

  # 自检 2：GRUB 语法
  if command -v grub-script-check >/dev/null 2>&1; then
    body="$(mktemp)"
    tail -n +3 "$SNIPPET" > "$body"
    if grub-script-check "$body" >/dev/null 2>&1; then
      log "GRUB 脚本语法自检：通过"
    else
      warn "正文保存在：$body"
      rm -f "$SNIPPET"
      die "$SNIPPET 的 GRUB 语法自检失败，已删除该文件（未触碰 grub.cfg）"
    fi
    rm -f "$body"
  else
    warn "未找到 grub-script-check（属 grub 包），跳过语法自检"
  fi
}

ensure_grubenv() {
  local f="$ESP_MNT/$ESP_SUBDIR/grubenv"
  mkdir -p "$ESP_MNT/$ESP_SUBDIR"
  if [ -f "$f" ]; then
    log "ESP 上已存在 grubenv，保留：$f"
  else
    log "在 ESP 上创建 1024 字节 grubenv：$f"
    if command -v grub-editenv >/dev/null 2>&1; then
      grub-editenv "$f" create
    fi
    if [ ! -s "$f" ]; then
      { printf '# GRUB Environment Block\n'; printf '#%.0s' $(seq 1 1024); } > "$f"
      truncate -s 1024 "$f"
    fi
  fi
  # 从 ext4 的内建 grubenv 迁移现有状态（只在 ESP 里还没有该键时）
  if [ -f "$GRUBENV_HOST" ] && command -v grub-editenv >/dev/null 2>&1; then
    local k v
    for k in saved_entry next_entry; do
      v=$(grub-editenv "$GRUBENV_HOST" list 2>/dev/null | sed -n "s/^$k=//p" | head -1) || v=""
      if [ -n "$v" ]; then
        if ! grub-editenv "$f" list 2>/dev/null | grep -q "^$k="; then
          grub-editenv "$f" set "$k=$v"
          log "迁移 $k=$v （ext4 -> ESP）"
        fi
      fi
    done
  fi
  log "grubenv 大小：$(stat -c '%s 字节' "$f" 2>/dev/null || echo '?')"
}

export_grubcfg() {
  [ -f "$GRUB_CFG" ] || { warn "未找到 $GRUB_CFG，跳过同步"; return 0; }
  local tmp="$ESP_MNT/$ESP_SUBDIR/.grub.cfg.new.$$"
  cp -f "$GRUB_CFG" "$tmp"
  mv -f "$tmp" "$ESP_MNT/$ESP_SUBDIR/grub.cfg"
  sync
  log "已同步 $GRUB_CFG -> $ESP_MNT/$ESP_SUBDIR/grub.cfg"
}

do_uninstall() {
  log "卸载：删除 $SNIPPET"
  rm -f "$SNIPPET"
  if command -v grub-mkconfig >/dev/null 2>&1; then
    grub-mkconfig -o "$GRUB_CFG"
  elif command -v update-grub >/dev/null 2>&1; then
    update-grub
  fi
  warn "之后 GRUB 重新使用 ext4 的 $GRUBENV_HOST（ESP 上的副本保留未删）。"
}

main() {
  detect_esp || die "找不到 EFI 系统分区(ESP)。确认 ESP 已挂载到 /efi 或 /boot/efi。"
  ESP_UUID="$(blkid -s UUID -o value "$ESP_DEV")"
  [ -n "$ESP_UUID" ] || die "无法读取 $ESP_DEV 的 UUID（需要 blkid）"
  ENV_REL="/$ESP_SUBDIR/grubenv"

  log "ESP 挂载点  : $ESP_MNT  ($ESP_DEV)"
  log "ESP fs-uuid : $ESP_UUID   (GRUB 的 search --fs-uuid 用它)"
  log "状态文件    : $ESP_MNT$ENV_REL"

  [ -f "$GRUB_CFG" ] || die "找不到 $GRUB_CFG"

  # 备份
  ts=$(date +%Y%m%d_%H%M%S)
  if [ -f "$SNIPPET" ]; then
    cp -a "$SNIPPET" "$SNIPPET.bak.$ts"
    log "已备份旧文件 -> $SNIPPET.bak.$ts"
  fi
  cp -a "$GRUB_CFG" "$GRUB_CFG.bak.anyboot.$ts"

  write_snippet
  ensure_grubenv

  log "重新生成 grub.cfg ..."
  if command -v grub-mkconfig >/dev/null 2>&1; then
    grub-mkconfig -o "$GRUB_CFG"
  elif command -v update-grub >/dev/null 2>&1; then
    update-grub
  else
    die "找不到 grub-mkconfig / update-grub"
  fi

  # 生成后整体语法自检（这是"重启前"的最后一道闸）
  if command -v grub-script-check >/dev/null 2>&1; then
    if grub-script-check "$GRUB_CFG" >/dev/null 2>&1; then
      log "整份 grub.cfg 语法自检：通过"
    else
      warn "grub.cfg 语法自检未通过，请检查；备份在 $GRUB_CFG.bak.anyboot.$ts"
    fi
  fi

  # 确认 09_anyboot 的内容真的被编译进去了
  if grep -q 'anyboot_esp' "$GRUB_CFG"; then
    log "确认：09_anyboot 已编译进 grub.cfg"
    grep -n 'anyboot' "$GRUB_CFG" | head -20
  else
    warn "警告：grub.cfg 里没找到 anyboot 相关内容，请把 grub-mkconfig 的输出发我"
  fi

  export_grubcfg

  cat <<EOF

${BL}[AnyBoot]${NC} 完成。

  · 生成器 : $SNIPPET   （update-grub 一遍又一遍地跑，这段设置都不会丢）
  · 状态   : $ESP_MNT$ENV_REL   （1024 字节纯文本，Windows/macOS 直接读写）
  · 菜单   : $ESP_MNT/$ESP_SUBDIR/grub.cfg

  · 重启前建议： grub-script-check $GRUB_CFG
  · 恢复原状：   sudo bash $0 --uninstall

EOF
}

if [ "${1:-}" = "--uninstall" ]; then do_uninstall; else main; fi
