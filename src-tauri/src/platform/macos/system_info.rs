//! 系统外观信息：深浅色、透明效果开关、低电量模式、减少动态效果（规格 06 §3.3）。

use std::path::PathBuf;

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSProcessInfo, NSString, NSUserDefaults};

use crate::platform::SystemVisuals;

pub fn visuals() -> SystemVisuals {
    // 浅色时这个键不存在，深色时是 "Dark"（"自动"模式下跟着当前实际外观变）
    let dark_mode = NSUserDefaults::standardUserDefaults()
        .stringForKey(&NSString::from_str("AppleInterfaceStyle"))
        .is_some_and(|style| style.to_string().eq_ignore_ascii_case("dark"));
    let workspace = NSWorkspace::sharedWorkspace();
    SystemVisuals {
        dark_mode,
        transparency_enabled: !workspace.accessibilityDisplayShouldReduceTransparency(),
        power_saver: NSProcessInfo::processInfo().isLowPowerModeEnabled(),
        reduced_motion: workspace.accessibilityDisplayShouldReduceMotion(),
        // 这个字段的含义是"系统窗口材质可用"；macOS 上一直有
        mica_supported: true,
    }
}

pub fn pictures_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Pictures"))
}

/// 改名前的旧登录项。Mac 版是改名之后才有的，没有旧的要清。
pub fn remove_autostart_entry(_name: &str) -> bool {
    false
}
