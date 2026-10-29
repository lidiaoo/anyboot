# Linux 免密提权（sudoers 方案）

目标：Linux 上写 ESP、写固件变量（`BootNext` / `OCBt*` / `Boot####`）、读裸分区这些
需要 root 的动作，**不要每次都弹密码框**；同时——同样重要——**卸载 AnyBoot 时能一键
撤掉授权，系统里不留后门**。

代码：`src-tauri/src/linux_priv.rs`（模块注释里也写了同一套结论）。

## 落地了哪三样东西

| 路径 | 内容 | 权限 |
|---|---|---|
| `/etc/sudoers.d/anyboot` | 一行：`<装机用户> ALL=(root) NOPASSWD: /usr/lib/anyboot/anyboot-priv` | `0440 root:root` |
| `/usr/lib/anyboot/anyboot-priv` | 本程序的一份副本（就是 helper，同一个二进制，不同 argv） | `0755 root:root` |
| `/var/lib/anyboot/priv.json` | `{user, version, helper, installedAt}` | `0644`（普通用户要读得到，界面才能显示状态） |

没有常驻进程、没有 systemd unit、没有 socket、没有系统用户组 —— 这是刻意选的：
常驻服务的卸载残留有四类（unit ×2 + 组 + 二进制），还有「socket 还在但程序没了 →
每次连接触发启动失败」这种静默故障。文件方案卸载就是 `rm`。

## 为什么授权的是自己的助手，不是 `install` / `mount`

sudoers 只按「命令 + 参数模式」授权。

- 授 `install`：等于授「任意文件复制到任意位置」——`sudo install -m 4755 /tmp/x /usr/bin/su`
  直接就是 root 后门。
- 授 `mount`：等于授「任意挂载」。
- 授 `/usr/lib/anyboot/anyboot-priv`：它能做什么由它自己的参数白名单决定，边界收在
  一个能 review 的地方，而且这个文件是 root 拥有、普通用户不可写（否则等于白授权）。

所以原来走 `pkexec install` 的 ESP 写入改成走助手子命令 `--linux-esp-install`：
助手内部做「读源 → 写目标 → 0644 → 保留 mtime → fsync」。保留 mtime 是必须的，
上游靠 `size + mtime` 判断「已同步」，丢了它每次扫描都会重写一遍。

## 提权链（运行时）

```
① 免密助手        sudo -n /usr/lib/anyboot/anyboot-priv <args>
② pkexec <self>   系统授权框（polkit 桌面才有）
③ sudo -S <self>  界面里输过一次的密码（只在内存里，进程退出即失）
```

- **① 命中就完全静默**，所以远程请求（`remoteAllowGuiAuth=false`）也能做完 ——
  以前远程一遇到提权只能拒绝，这是这套方案对无人值守场景最大的意义。
- 判据是「结果文件写没写」，不是退出码：每条 root 子命令都把结果写进 `--out`。
  有结果 = 操作真的跑了（失败也算跑通，错误在 JSON 里）；没结果 = 授权没拿到。
  没这个区分，「写失败」会误触发一次密码框。
- `allow_gui=false`（远程）时**只走 ①**，②③ 一律不试。
- ②③ 只在用户主动点按钮 / 本机主窗口操作时才会发生。

## 安装 / 卸载

安装（root 侧）：`--linux-priv-install`

1. 从 `SUDO_USER` / `PKEXEC_UID` 取「要授权的人」；**两个都没有 = 有人直接用 root 跑 → 拒绝**
   （授权给 root 没意义，还会留一行看起来生效、实际没人用的规则）。
2. 用户名过白名单（`[A-Za-z0-9._-]`，长度 ≤32，不以 `-` 开头）。带换行/逗号/`!` 的用户名
   能往 sudoers 里塞任意规则 —— 这是**必须**挡住的一步。
3. 复制自身到 `/usr/lib/anyboot/anyboot-priv.new` → `rename`（正在跑的旧助手是 inode 级的，
   替换目录项不会打断它，也不会留半个文件）。
4. sudoers：先写 `/var/lib/anyboot/anyboot.sudoers.tmp` → `visudo -cf` 校验 →
   拷到 `/etc/sudoers.d/anyboot.incoming`（含 `.` 的文件被 sudo 忽略）→ 设 `0440` → `rename`。
   **临时文件和备份绝不能放 `/etc/sudoers.d/`**：那个目录里所有不以 `~` 结尾的文件都会被
   include，`.tmp` / `.bak` 一样会被当规则解析。写坏 sudoers.d = 整机 sudo 不可用，
   所以找不到 `visudo` 时宁可不装。
5. 写 `priv.json`。

卸载（root 侧）：`--linux-priv-uninstall` —— 删 `sudoers` 文件、删助手、删 `priv.json`，
空目录顺手 `rmdir`。顺序无所谓：没有进程要停，没有 socket 会把服务拉起来。

界面上「关于」页那张卡按 `state` 显示：`ready` / `outdated`（版本变了要重装）/
`stale`（只剩一半 = 有残留，例如被手动删过其中一样）/ `absent`。

## 卸载 AnyBoot 时怎么清

| 分发包 | 谁来做 | 说明 |
|---|---|---|
| deb | `postrm`（`bundle.linux.deb.postRemoveScript`） | `$1` 为 `remove`/`purge` 才清；`upgrade` 不动 |
| rpm | `%postun`（`bundle.linux.rpm.postRemoveScript`） | `$1` 为 `0` 才清；`1` 是升级，不动 |
| AppImage | 应用内「关于 → 移除授权」 | 没有 postrm；**先移除再删文件**，否则授权会留在系统里 |
| 手动 | `sudo rm -f /etc/sudoers.d/anyboot && sudo rm -rf /usr/lib/anyboot` | 两行清完 |

两者共用 `src-tauri/linux/postremove.sh`（脚本里 `case "$1"` 同时认 deb 的字符串与 rpm 的数字）。

**升级不删的原因**：授权被删掉，用户会在更新之后莫名其妙又开始被要密码 —— 比不装更糟。

## 边界（这套方案**不**负责的）

- `mount` / `umount` 仍走 `pkexec`（`mount_command`）：授 `mount` 等于授任意挂载，不接受。
- Tailscale / tsnet 的安装、卸载、`operator` 设置仍各自走 `pkexec`：它们操作的是系统包
  与服务，不属于「写引导」这条线，等以后统一收口。
- 多用户机器只授权**装机的那一个用户**。换人要「移除授权 → 用新账号装一次」。

## 安全上要记住的三件事

1. 这套东西一旦装好，**本用户下任何进程**都能让助手做缓冲区里那些白名单动作。所以
   助手必须：参数白名单、路径校验、`O_NOFOLLOW`、清空环境变量、不读 `PATH`。
2. `visudo -cf` 是硬门槛，不是可选步骤。
3. 用户名白名单是硬门槛（见上）。

## 怎么验证

```bash
# 状态（不写任何东西）
jq -r '.state,.user' /var/lib/anyboot/priv.json 2>/dev/null; ls -l /etc/sudoers.d/anyboot

# 装（等价于界面点「安装免密组件」）
sudo ./anyboot --linux-priv-install --out /tmp/priv.json; cat /tmp/priv.json

# 免密是否真生效（不该再问密码）
sudo -n /usr/lib/anyboot/anyboot-priv --linux-priv-install --out /tmp/priv2.json && echo OK

# 删
sudo ./anyboot --linux-priv-uninstall --out /tmp/priv3.json; cat /tmp/priv3.json
ls /etc/sudoers.d/anyboot /usr/lib/anyboot 2>&1   # 都应该 No such file
```

单测（不需要 root）：`cargo test --lib linux_priv` —— 覆盖用户名白名单（注入用例）、
sudoers 文本只有一条规则且无通配符、状态分类（只剩一半必须判 `stale`）。

打包之后**顺手确认钩子真的进了包**（Tauri 各版本对 `postRemoveScript` 的处理方式
略有差异，二进制包一旦漏了这条，卸载就会把授权留在系统里）：

```bash
dpkg-deb -e AnyBoot_*.deb /tmp/deb-ctl && grep -n "sudoers.d/anyboot" /tmp/deb-ctl/postrm
rpm -qp --scripts AnyBoot-*.rpm | grep -n "sudoers.d/anyboot"
```

两条都应打印出 `rm -f /etc/sudoers.d/anyboot`。
