// release 构建下不要弹出控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    chenocr_lib::run()
}
