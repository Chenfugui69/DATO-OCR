//! 系统外观信息：深浅色、透明效果开关、省电模式、减少动画（规格 06 §3.3）。

use std::path::PathBuf;

use windows::core::w;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Shell::{FOLDERID_Pictures, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

use crate::platform::SystemVisuals;

fn reg_dword(
    root: windows::Win32::System::Registry::HKEY,
    key: windows::core::PCWSTR,
    value: windows::core::PCWSTR,
) -> Option<u32> {
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: 输出缓冲是一个 u32，大小如实传入。
    let status = unsafe {
        RegGetValueW(
            root,
            key,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    status.is_ok().then_some(data)
}

fn windows_build() -> u32 {
    let mut buf = [0u16; 32];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: 输出缓冲大小如实传入。
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return 0;
    }
    super::util::from_wide(&buf).parse().unwrap_or(0)
}

pub fn visuals() -> SystemVisuals {
    const PERSONALIZE: windows::core::PCWSTR =
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let dark_mode = reg_dword(HKEY_CURRENT_USER, PERSONALIZE, w!("AppsUseLightTheme")) == Some(0);
    let transparency_enabled =
        reg_dword(HKEY_CURRENT_USER, PERSONALIZE, w!("EnableTransparency")) != Some(0);

    let mut power = SYSTEM_POWER_STATUS::default();
    // SAFETY: 输出到栈上的结构体。
    let power_saver =
        unsafe { GetSystemPowerStatus(&mut power) }.is_ok() && power.SystemStatusFlag == 1;

    let mut animations: i32 = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION 输出一个 BOOL。
    let _ = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut animations as *mut i32 as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };

    SystemVisuals {
        dark_mode,
        transparency_enabled,
        power_saver,
        reduced_motion: animations == 0,
        mica_supported: windows_build() >= 22000,
    }
}

pub fn pictures_dir() -> Option<PathBuf> {
    // SAFETY: 返回的字符串由 CoTaskMemFree 释放。
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const core::ffi::c_void));
        text.map(PathBuf::from)
    }
}
