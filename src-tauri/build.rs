fn main() {
    // 给自研命令显式生成 ACL 权限（默认 app 命令应自动放行，但部分窗口/构建下
    // 新命令会被 “not allowed by ACL” 拒绝；显式声明后 capability 引用即可）。
    let attributes = tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&["boot_instance_switch"]),
    );
    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
