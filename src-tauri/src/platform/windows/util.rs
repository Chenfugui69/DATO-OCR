//! Win32 小工具。

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;

pub fn hwnd(handle: u64) -> HWND {
    HWND(handle as usize as *mut core::ffi::c_void)
}

pub fn handle_of(hwnd: HWND) -> u64 {
    hwnd.0 as usize as u64
}

/// 以 NUL 结尾的 UTF-16 缓冲。调用方要保证返回值活得比用到的 PCWSTR 久。
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn pcwstr(buf: &[u16]) -> PCWSTR {
    PCWSTR(buf.as_ptr())
}

pub fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}
