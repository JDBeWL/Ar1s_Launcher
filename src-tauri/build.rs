fn main() {
    // include_str! 的文件不会触发 build script 重跑，必须显式声明，
    // 否则改了 app.manifest 而嵌入 exe 的清单不更新
    println!("cargo:rerun-if-changed=windows/app.manifest");
    // 自定义 Windows 清单启用 longPathAware（见 windows/app.manifest）
    // 该文件必须保持纯 ASCII：无 BOM 的清单按系统 ANSI 代码页解析，
    // 出现中文等非 ASCII 字符会把嵌入 exe 的清单变成非法 XML，启动报 os error 14001
    let windows =
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("windows/app.manifest"));
    let attrs = tauri_build::Attributes::new().windows_attributes(windows);
    tauri_build::try_build(attrs).expect("failed to run tauri-build");
}
