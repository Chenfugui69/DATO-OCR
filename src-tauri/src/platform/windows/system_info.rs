//! 系统外观探测。
//!
//! 规格 00 §6.5 指定了注册表路径：
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`

use std::ffi::c_void;
use std::mem;

use windows::core::{w, PCWSTR};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

use crate::platform::SystemInfo;

const PERSONALIZE: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");

pub struct WindowsSystemInfo;

impl SystemInfo for WindowsSystemInfo {
    fn is_dark_mode(&self) -> bool {
        // 键不存在时 Windows 的默认行为是浅色。
        read_dword(PERSONALIZE, w!("AppsUseLightTheme")).unwrap_or(1) == 0
    }

    fn is_transparency_enabled(&self) -> bool {
        read_dword(PERSONALIZE, w!("EnableTransparency")).unwrap_or(1) != 0
    }
}

fn read_dword(subkey: PCWSTR, name: PCWSTR) -> Option<u32> {
    let mut value: u32 = 0;
    let mut size = mem::size_of::<u32>() as u32;

    // SAFETY: `subkey` / `name` 是静态的以 NUL 结尾的宽字符串；输出缓冲区是栈上
    // 的 u32，`size` 与之匹配。RegGetValueW 不会写超过 `size` 字节。
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };

    status.is_ok().then_some(value)
}
