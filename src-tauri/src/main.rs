#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 带参数 = CLI 子命令（ssh 远程调用用）；不带参数 = 图形界面（托盘常驻）
    let args: Vec<String> = std::env::args().skip(1).collect();
    if anyboot_lib::cli_requested(&args) {
        std::process::exit(anyboot_lib::run_cli(&args));
    }
    anyboot_lib::run();
}
