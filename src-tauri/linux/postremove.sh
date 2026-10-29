#!/bin/sh
# AnyBoot 卸载钩子：把「免密提权」组件一起撤掉。
#
# 只有两样东西要删（这就是选 sudoers 而不是常驻服务的原因 —— 没有 daemon 要停、
# 没有 socket 要把服务重新拉起来、没有系统组要清）：
#   * /etc/sudoers.d/anyboot   —— 授权文件（删了就等于撤销免密）
#   * /usr/lib/anyboot/        —— 助手副本（本程序的一份拷贝，root 拥有）
# 外加一份安装记录 /var/lib/anyboot/priv.json。
#
# ⚠️ **升级时不能删**：deb 的 $1 是 "upgrade <ver>"，rpm 的 $1 是 1。升级后授权
#    如果被删掉，用户会莫名其妙地又开始被要密码 —— 这种「更新一下功能就没了」
#    比不装还糟。
case "$1" in
  remove|purge|0)
    rm -f /etc/sudoers.d/anyboot
    rm -rf /usr/lib/anyboot
    rm -f /var/lib/anyboot/priv.json
    # 只在空的时候删掉，别碰用户自己的东西
    rmdir /var/lib/anyboot 2>/dev/null || true
    ;;
  *)
    # upgrade / failed-upgrade / 1：授权保持不动
    ;;
esac

exit 0
