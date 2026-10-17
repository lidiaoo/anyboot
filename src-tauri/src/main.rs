#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
