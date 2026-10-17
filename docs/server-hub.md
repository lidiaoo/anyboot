# 远端管理（server-hub）：三端使用与平台差异

主窗口里的「远端管理」是一组独立窗口：**服务器档案**（地址 / 凭据 / 分组）、**终端**（PTY 跑 ssh）、
**文件浏览**（sftp）、**隧道**（本地转发 / SOCKS5）、**跳板机**（ProxyCommand）。

它**不引 SSH 库**，直接复用系统自带的 `ssh` / `sftp` 命令行。所以「能不能用」取决于两件事：
系统有没有 OpenSSH 客户端，以及客户端的**能力差异**。后者才是三端表现不一样的原因。

---

## 一、前置条件

| 平台 | OpenSSH 客户端 | 说明 |
| --- | --- | --- |
| Linux | 自带 | 发行版一般预装 `openssh-client` |
| macOS | 自带 | 系统 `/usr/bin/ssh`（9.x） |
| Windows | 10（1809+）/ 11 自带 | `C:\Windows\System32\OpenSSH\ssh.exe`；缺失时「设置 ▸ 应用 ▸ 可选功能」装 *OpenSSH 客户端* |

在 Windows 上想确认应用实际会用到哪个 ssh：

```powershell
Get-Command ssh | Select-Object Source
# 正常应指向 C:\Windows\System32\OpenSSH\ssh.exe
```

> **别用 Git Bash 里的 `ssh` 判断应用行为**：那里解析到的是 `PortableGit/usr/bin/ssh.exe`（MSYS 版），
> 它支持连接复用，和系统自带的 Win32-OpenSSH **能力不同**。应用从开始菜单启动时走的是系统 PATH。

---

## 二、认证方式（三选一，编辑表单里分段）

| 方式 | 存的字段 | 怎么工作 |
| --- | --- | --- |
| 交互 | 都不填 | 密码 / 首次指纹在终端里手输，凭据不落盘。文件 / 隧道需要非交互认证，**这种方式用不了** |
| 密钥 | `key_id` | `ssh -i <私钥> -o IdentitiesOnly=yes`；带口令的私钥在终端里解锁 |
| 保存密码 | `password_enc` | AES-256-GCM 密文落盘；ssh 要密码时由 **本程序自身**充当 askpass 解密喂入 |

保存密码的实现要点（三端同一套）：

- `SSH_ASKPASS` 指向 **anyboot.exe 自己**，`SSH_ASKPASS_REQUIRE=force`（**环境变量**，不是 `-o` 选项），
  `ANYBOOT_ASKPASS_SERVER=<server_id>` 用环境变量带给子进程；
- ssh 调 askpass 时 `argv[1]` 是提示语（`user@host's password: `），程序解析出 host：
  与目标机对得上就用目标机的密码，与跳板机对得上就用跳板机的密码；
- 明文只在内存里活一瞬，写进 ssh 的 stdout 管道，不落盘、不进命令行。

以前是写 `#!/bin/sh` 脚本 + `chmod 700`，Windows 上跑不了（CreateProcess 不认 shebang），已废弃。

---

## 三、平台能力矩阵

| 能力 | Linux | macOS | Windows | 说明 |
| --- | --- | --- | --- | --- |
| 服务器档案 / 分组 / 加密存密码 | ✅ | ✅ | ✅ | 纯 Rust，无平台依赖 |
| 终端（PTY + ssh） | ✅ | ✅ | ✅ | Windows 走 ConPTY（10 1809+） |
| 保存密码免输 | ✅ | ✅ | ✅ | 三端都走「askpass = 自身 exe」 |
| 文件浏览 / 上传下载 / 断点续传 | ✅ | ✅ | ✅ | 见下「复用」差异 |
| 端口转发 / SOCKS5 | ✅ | ✅ | ✅ | |
| 跳板机（ProxyCommand） | ✅ | ✅ | ✅ | 密码由继承的 askpass 按 host 认领 |
| **连接复用（ControlMaster）** | ✅ | ✅ | ❌ | Win32-OpenSSH 不支持 |

### Windows 没有 ControlMaster，意味着什么

Unix 上终端先建立一条 ControlMaster，sftp / 隧道 / 远端命令都靠 `ControlPath` 复用它 ——
同一台服务器只认证一次。**Win32-OpenSSH 不支持多路复用**（实测：带 `ControlMaster=yes` +
`ControlPath=…` 连接会直接 `getsockname failed: Not a socket`，控制套接字文件根本不落地）。

所以 Windows 上改成**每次操作自己认证**：

- 密钥认证：`-i` + `BatchMode=yes`，快；
- 保存密码：`SSH_ASKPASS_REQUIRE=force` 走 askpass，每次多一次进程启动 + 一次握手
  （本地实测百毫秒级，文件浏览这种批操作一个批次只认证一次）。

副作用：Windows 上每次操作都会在服务器 `auth.log` 里留一条认证记录；Unix 上靠复用则会少很多。

---

## 四、排错

| 现象 | 原因 / 处理 |
| --- | --- |
| `Permission denied (publickey,password,…)` | 密码错 / 公钥没加到服务器 `~/.ssh/authorized_keys`；也可能是用了未解锁的带口令私钥 |
| 提示「这台服务器还没有保存凭据…」 | 认证方式选了「交互」——文件 / 隧道需要非交互认证，改成密钥或保存密码 |
| 提示「无法启动 ssh」 | PATH 里没有 ssh：Windows 装「OpenSSH 客户端」；Linux 装 `openssh-client` |
| 跳板机报「没有保存凭据」 | 跳板机自己也必须是密钥或保存密码，不能只在终端里手输过一次 |
| Windows 上文件浏览很慢 | 没有连接复用，每次重新认证；属预期 |

---

## 五、验证到什么程度

- Windows：`cargo test` 166 项全过；askpass 端到端（真实二进制 + 真实加密配置）验证了
  「目标机密码 / 跳板机密码 / 回落 / CLI 不被劫持」四条；真实 `ssh` / `sftp` 进程确认会消费这套
  环境变量、参数顺序正确（以错误密码得到 `Permission denied` 收尾，不是挂死）。
- **未验证**：Windows 上对真实服务器的完整密码登录 / sftp 列目录 / 隧道转发（本机没有可连的
  测试服务器，Docker 与 PyPI 均被网络拦截）。这部分请在可连的服务器上按上表走一遍。
