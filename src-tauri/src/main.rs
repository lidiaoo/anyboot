#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // ⓪ root 助手模式（免密写 NVRAM）：必须排在所有分支最前 ——
    //    装到 /Library 的是本程序的副本，被 launchd 唤起时带 `--run-inbox`，
    //    此时**绝不能**进 Tauri（那会去开窗口、抢单实例锁、监听端口）。
    if args.iter().any(|arg| arg == "--run-inbox") {
        // 非 root 跑没有意义（写 NVRAM 必然被拒）：早失败比晚失败好，
        // 否则用户看到的是「助手回了失败」，真相却是「你以自己身份跑了它」。
        // dry-run 是测试用的例外（见 helper_proto::apply）。
        let dry_run = std::env::var("ANYBOOT_HELPER_DRY_RUN").as_deref() == Ok("1");
        if !dry_run && anyboot_lib::helper_proto::current_uid() != 0 {
            eprintln!("anyboot --run-inbox 必须以 root 运行（launchd 作业会这么做）。");
            std::process::exit(3);
        }
        std::process::exit(anyboot_lib::helper_proto::run_inbox());
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
