#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // ⓪⁻ 特权助手服务（Windows）：`--run-service` 由 SCM 拉起并常驻，另外三条是
    //     安装 / 卸载 / 查询。**排在所有分支最前**，理由同 --run-inbox：服务进程
    //     绝不能进 Tauri（开窗口、抢单实例锁、监听端口都会把常驻服务毁掉）。
    //     它存在的意义只有一个：远程 / 无人值守时没有桌面可点 UAC。
    if args.iter().any(|arg| arg == "--run-service") {
        std::process::exit(anyboot_lib::service_run_main());
    }
    if args.iter().any(|arg| arg == "--install-service") {
        std::process::exit(anyboot_lib::service_install_main());
    }
    if args.iter().any(|arg| arg == "--uninstall-service") {
        std::process::exit(anyboot_lib::service_uninstall_main());
    }
    if args.iter().any(|arg| arg == "--service-status") {
        std::process::exit(anyboot_lib::service_status_main());
    }
    // ⓪.1 **通用提权子命令**（Windows）：`--privileged-op <op> --args <json> --out <file>`。
    //      服务不在时，`elevate::run_op` 会用 ShellExecuteExW(runas) 把本程序以这个
    //      形式拉起来，而它内部跑的正是服务端那份 `dispatch` —— 两条提权路共用一份
    //      实现。新增一个可提权操作只需要：dispatch 加一条 match 臂 + 调用方换
    //      `run_op`，不必再加 argv 分支（下面那几条专用分支是历史形状，保留兼容）。
    if let Some(pos) = args.iter().position(|arg| arg == "--privileged-op") {
        let get = |flag: &str| {
            args.iter()
                .position(|arg| arg == flag)
                .and_then(|index| args.get(index + 1))
                .cloned()
        };
        let (Some(op), Some(out)) = (args.get(pos + 1).cloned(), get("--out")) else {
            eprintln!("用法：anyboot --privileged-op <op> --args <json> --out <结果文件>");
            std::process::exit(2);
        };
        let args_json = get("--args").unwrap_or_else(|| "{}".to_string());
        std::process::exit(anyboot_lib::privileged_op_main(&op, &args_json, &out));
    }
    // ⓪ root 助手模式（免密写 NVRAM / 免密读 Linux 分区）：必须排在所有分支最前 ——
    //    装到 /Library 的是本程序的副本，被 launchd 唤起时带 `--run-inbox`，
    //    此时**绝不能**进 Tauri（那会去开窗口、抢单实例锁、监听端口）。
    if args.iter().any(|arg| arg == "--run-inbox") {
        // 非 root 跑没有意义（写 NVRAM、开 /dev/rdisk 必然被拒）：早失败比晚失败好，
        // 否则用户看到的是「助手回了失败」，真相却是「你以自己身份跑了它」。
        let dry_run = std::env::var("ANYBOOT_HELPER_DRY_RUN").as_deref() == Ok("1");
        if !dry_run && anyboot_lib::helper_proto::current_uid() != 0 {
            eprintln!("anyboot --run-inbox 必须以 root 运行（launchd 作业会这么做）。");
            std::process::exit(3);
        }
        std::process::exit(anyboot_lib::helper_proto::run_inbox(
            &anyboot_lib::helper_exec::System,
        ));
    }
    // ⓪.5 提权扫描子命令（Windows/Linux 读 APFS 整盘要管理员，按需自拉起一次，
    //    结果经 JSON 文件回传给主进程 —— 见 channels/apfs_reader.rs 模块注释）。
    //    与 --run-inbox 同理：绝不能进 Tauri（开窗口、抢单实例锁、监听端口全是要命的事）。
    if let Some(pos) = args.iter().position(|arg| arg == "--apfs-scan-privileged") {
        // 调用方只有我们自己（apfs_reader::spawn_*），固定两段式 `--out <path>`
        let out = match args.get(pos + 2) {
            Some(out) => out.clone(),
            None => {
                eprintln!("用法：anyboot --apfs-scan-privileged --out <结果文件路径>");
                std::process::exit(2);
            }
        };
        std::process::exit(anyboot_lib::apfs_privileged_scan_main(&out));
    }
    // ⓪.5b ESP 只读挂载（提权版）：免密助手以 root 执行 mount。启动自动挂载与
    //    卡片按钮共用；参数白名单见 linux_priv::validate_mount_request。
    if args.iter().any(|arg| arg == "--linux-mount-esp") {
        let get = |flag: &str| {
            args.iter()
                .position(|arg| arg == flag)
                .and_then(|index| args.get(index + 1))
                .cloned()
        };
        let (Some(device), Some(mountpoint), Some(out)) =
            (get("--device"), get("--mountpoint"), get("--out"))
        else {
            eprintln!("用法：anyboot --linux-mount-esp --device <dev> --mountpoint <dir> [--fstype <fs>] --out <结果文件>");
            std::process::exit(2);
        };
        let fstype = get("--fstype").unwrap_or_default();
        std::process::exit(anyboot_lib::linux_mount_esp_main(
            &device, &mountpoint, &fstype, &out,
        ));
    }
    // ⓪.6 OC 设默认项（提权版）：主进程检测到没有 NVRAM 权限时自拉起，这里
    //    完成整套 set_default 并把结果写回 --out 指的文件。同样绝不进 Tauri。
    if args.iter().any(|arg| arg == "--oc-set-default") {
        let get = |flag: &str| {
            args.iter()
                .position(|arg| arg == flag)
                .and_then(|index| args.get(index + 1))
                .cloned()
        };
        let (Some(target), Some(out)) = (get("--target"), get("--out")) else {
            eprintln!("用法：anyboot --oc-set-default --target <卷\\u{{1}}路径> [--esp <ESP 根>] --out <结果文件>");
            std::process::exit(2);
        };
        std::process::exit(anyboot_lib::oc_set_default_main(
            &target,
            get("--esp").as_deref(),
            &out,
        ));
    }
    // ⓪.7 NVRAM 变量 dump（提权版）：把 OC 私有（OCBt*）与固件全局（Boot*）
    //    两组启动变量全部 hex 落盘 —— 排障看 OC 到底写了什么槽，也是
    //    「凭真实字节复刻 APFS load option」的样本采集。同样绝不进 Tauri。
    if args.iter().any(|arg| arg == "--nvram-dump-privileged") {
        let out = match args
            .iter()
            .position(|arg| arg == "--out")
            .and_then(|index| args.get(index + 1))
        {
            Some(out) => out.clone(),
            None => {
                eprintln!("用法：anyboot --nvram-dump-privileged --out <结果文件路径>");
                std::process::exit(2);
            }
        };
        std::process::exit(anyboot_lib::nvram_dump_main(&out));
    }
    // ⓪.55 GRUB「自动读取」扫描（提权版）：读 Linux 分区里的 grub.cfg / grubenv。
    //     macOS / Windows 的裸设备只有 root / 管理员能打开，所以这一段由主进程
    //     按需自拉起。与上面几条同理：绝不进 Tauri。
    if args.iter().any(|arg| arg == "--linux-grub-probe") {
        if anyboot_lib::helper_proto::current_uid() != 0 {
            eprintln!("anyboot --linux-grub-probe 必须以 root 运行（读裸分区需要）");
            std::process::exit(3);
        }
        let get = |flag: &str| {
            args.iter()
                .position(|arg| arg == flag)
                .and_then(|index| args.get(index + 1))
                .cloned()
        };
        let Some(out) = get("--out") else {
            eprintln!("用法：anyboot --linux-grub-probe [--esp <ESP 分区 UUID>] --out <结果文件>");
            std::process::exit(2);
        };
        std::process::exit(anyboot_lib::linux_grub_probe_main(
            &out,
            get("--esp").as_deref(),
        ));
    }
    // ⓪.56 GRUB「自动读取」写环境块（提权版）：定点覆盖目标分区的 1024 字节 grubenv。
    if args.iter().any(|arg| arg == "--linux-grub-write") {
        if anyboot_lib::helper_proto::current_uid() != 0 {
            eprintln!("anyboot --linux-grub-write 必须以 root 运行（写裸分区需要）");
            std::process::exit(3);
        }
        let get = |flag: &str| {
            args.iter()
                .position(|arg| arg == flag)
                .and_then(|index| args.get(index + 1))
                .cloned()
        };
        let (Some(dev), Some(rel), Some(payload), Some(out)) = (
            get("--dev"),
            get("--rel"),
            get("--payload"),
            get("--out"),
        ) else {
            eprintln!(
                "用法：anyboot --linux-grub-write --dev <disk0s9> --rel <boot/grub/grubenv> \
                 --payload <1024 字节文件> --out <结果文件>"
            );
            std::process::exit(2);
        };
        std::process::exit(anyboot_lib::linux_grub_write_main(
            &dev, &rel, &payload, &out,
        ));
    }
    // ⓪.57 ESP 文件写入（提权版）：免密助手与 pkexec 兜底**都跑这一条** ——
    //      「走助手时成功、弹授权就报参数错」这类漂移从结构上就不可能出现。
    //      同样绝不进 Tauri。
    if args.iter().any(|arg| arg == "--linux-esp-install") {
        if anyboot_lib::helper_proto::current_uid() != 0 {
            eprintln!("anyboot --linux-esp-install 必须以 root 运行（写 ESP 需要）");
            std::process::exit(3);
        }
        let get = |flag: &str| {
            args.iter()
                .position(|arg| arg == flag)
                .and_then(|index| args.get(index + 1))
                .cloned()
        };
        let (Some(src), Some(dst), Some(out)) = (get("--src"), get("--dst"), get("--out")) else {
            eprintln!("用法：anyboot --linux-esp-install --src <源> --dst <目标> --out <结果文件>");
            std::process::exit(2);
        };
        std::process::exit(anyboot_lib::linux_esp_install_main(&src, &dst, &out));
    }
    // ⓪.58 免密组件（sudoers）的安装 / 卸载：**必须 root**，由 linux_priv 模块用
    //      pkexec / sudo 拉起来。和其余子命令一样绝不进 Tauri —— 这两条只写文件，
    //      进了 Tauri 就会开窗口、抢单实例锁。
    for (flag, label) in [
        ("--linux-priv-install", "安装"),
        ("--linux-priv-uninstall", "移除"),
    ] {
        if !args.iter().any(|arg| arg == flag) {
            continue;
        }
        if anyboot_lib::helper_proto::current_uid() != 0 {
            eprintln!("anyboot {flag} 必须以 root 运行（{label}免密组件要写 /etc 与 /usr/lib）");
            std::process::exit(3);
        }
        let out = match args
            .iter()
            .position(|arg| arg == "--out")
            .and_then(|index| args.get(index + 1))
        {
            Some(out) => out.clone(),
            None => {
                eprintln!("用法：anyboot {flag} --out <结果文件>");
                std::process::exit(2);
            }
        };
        std::process::exit(if flag == "--linux-priv-install" {
            anyboot_lib::linux_priv_install_main(&out)
        } else {
            anyboot_lib::linux_priv_uninstall_main(&out)
        });
    }
    // ① ssh 的 SSH_ASKPASS 把我们当程序调起来（argv 是密码提示语）：
    //    只把密码打到 stdout 就退，既不是 GUI 也不是普通 CLI。必须排在 cli_requested
    //    之前 —— 否则「一个不像命令的参数」会被当成 GUI 启动，ssh 那边只会读到空密码。
    if let Some(code) = anyboot_lib::cli_askpass(&args) {
        std::process::exit(code);
    }
    // ② 带参数 = CLI 子命令（ssh 远程调用用）；不带参数 = 图形界面（托盘常驻）
    if anyboot_lib::cli_requested(&args) {
        std::process::exit(anyboot_lib::run_cli(&args));
    }
    anyboot_lib::run();
}
