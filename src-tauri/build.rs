fn main() {
    // macOS 在 exFAT / FAT / 网络盘上会给带扩展属性的文件生成 `._名字` 伴生文件。
    // tauri-build 把 capabilities/ 下的每个文件都当权限配置解析，读到它们就报
    // "stream did not contain valid UTF-8"；前端产物目录里的也会被原样打进程序。构建前清掉。
    #[cfg(target_os = "macos")]
    for dir in ["capabilities", "../dist"] {
        remove_apple_double(std::path::Path::new(dir));
    }
    tauri_build::build()
}

#[cfg(target_os = "macos")]
fn remove_apple_double(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            remove_apple_double(&path);
        } else if entry.file_name().to_string_lossy().starts_with("._") {
            let _ = std::fs::remove_file(&path);
        }
    }
}
