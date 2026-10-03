//! 窗口枚举（规格 02 §3.2、07 §4.2–4.3）。
//!
//! - `EnumWindows` 本身就是按 Z 序从上到下给出顶层窗口
//! - 矩形用 `DWMWA_EXTENDED_FRAME_BOUNDS`，**不用** `GetWindowRect`：后者在 Win10+
//!   会多出约 7px 的不可见阴影边框，自动框选会比窗口大一圈
//! - 过滤：不可见、最小化、`WS_EX_TOOLWINDOW`、鼠标穿透的覆盖层、零尺寸、
//!   DWM cloaked（UWP 的隐藏窗口，不过滤会框到不存在的窗口）、本进程自己的窗口

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::Mutex;
use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetAncestor, GetForegroundWindow, GetWindowLongPtrW,
    GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    WindowFromPoint, GA_ROOT, GWL_EXSTYLE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

use super::util::{from_wide, handle_of, hwnd, pcwstr, wide};
use crate::error::AppResult;
use crate::platform::{AppInfo, PhysicalRect, WindowHandle, WindowInfo};

/// 子控件小于这个尺寸就不参与检测，避免框住一根分隔线（规格 02 §3.2.3）。
const MIN_CHILD_SIZE: u32 = 20;

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam 是下面传进来的 &mut Vec<HWND>，枚举期间一直有效。
    let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    list.push(hwnd);
    BOOL(1)
}

pub fn enumerate_top_level() -> AppResult<Vec<WindowInfo>> {
    let mut hwnds: Vec<HWND> = Vec::with_capacity(256);
    // SAFETY: 回调只往 Vec 里 push。
    unsafe { EnumWindows(Some(collect), LPARAM(&mut hwnds as *mut _ as isize))? };

    // SAFETY: 无参数。
    let own_pid = unsafe { GetCurrentProcessId() };
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut out = Vec::new();

    for h in hwnds {
        // SAFETY: 以下都是只读查询；窗口可能在枚举后被销毁，各调用会安全失败。
        unsafe {
            if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
                continue;
            }
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW.0 != 0 || ex & WS_EX_TRANSPARENT.0 != 0 {
                continue;
            }
            if is_cloaked(h) {
                continue;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            if pid == own_pid {
                continue;
            }
            let Some(bounds) = frame_bounds(h) else {
                continue;
            };
            if bounds.is_empty() {
                continue;
            }
            let app_name = names
                .entry(pid)
                .or_insert_with(|| {
                    process_path(pid)
                        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                        .unwrap_or_default()
                })
                .clone();
            out.push(WindowInfo {
                handle: WindowHandle(handle_of(h)),
                bounds,
                z_order: out.len() as u32,
                title: window_text(h),
                app_name,
                process_id: pid,
                children: Vec::new(),
            });
        }
    }
    Ok(out)
}

pub fn enumerate_children(window: WindowHandle) -> AppResult<Vec<PhysicalRect>> {
    let mut hwnds: Vec<HWND> = Vec::new();
    // SAFETY: 同上；EnumChildWindows 递归枚举所有后代。
    unsafe {
        let _ = EnumChildWindows(
            Some(hwnd(window.0)),
            Some(collect),
            LPARAM(&mut hwnds as *mut _ as isize),
        );
    }
    let mut out = Vec::new();
    for h in hwnds {
        // SAFETY: 只读查询。
        unsafe {
            if !IsWindowVisible(h).as_bool() {
                continue;
            }
            let mut r = RECT::default();
            if GetWindowRect(h, &mut r).is_err() {
                continue;
            }
            let rect = rect_of(r);
            if rect.width >= MIN_CHILD_SIZE && rect.height >= MIN_CHILD_SIZE {
                out.push(rect);
            }
        }
    }
    Ok(out)
}

pub fn window_at(x: i32, y: i32) -> Option<WindowHandle> {
    // SAFETY: 纯查询。
    unsafe {
        let h = WindowFromPoint(POINT { x, y });
        if h.is_invalid() {
            return None;
        }
        let root = GetAncestor(h, GA_ROOT);
        let root = if root.is_invalid() { h } else { root };
        Some(WindowHandle(handle_of(root)))
    }
}

pub fn is_own_window(window: WindowHandle) -> bool {
    let mut pid = 0u32;
    // SAFETY: 只读查询。
    unsafe {
        GetWindowThreadProcessId(hwnd(window.0), Some(&mut pid));
        pid != 0 && pid == GetCurrentProcessId()
    }
}

pub fn foreground_window() -> Option<WindowHandle> {
    // SAFETY: 无参数。
    let h = unsafe { GetForegroundWindow() };
    (!h.is_invalid()).then(|| WindowHandle(handle_of(h)))
}

/// 窗口所属应用：可执行文件路径 + 文件描述（"微信"、"Google Chrome"）。
pub fn app_info_of(window: WindowHandle) -> Option<AppInfo> {
    let h = hwnd(window.0);
    let mut pid = 0u32;
    // SAFETY: 只读查询。
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    let mut path = process_path(pid)?;
    // UWP 应用的顶层窗口属于 ApplicationFrameHost，真正的应用在子窗口里
    if path
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("ApplicationFrameHost.exe"))
    {
        if let Some(real) = uwp_child_process(h, pid).and_then(process_path) {
            path = real;
        }
    }
    let name = describe_exe(&path);
    Some(AppInfo {
        name,
        exe_path: Some(path),
    })
}

fn uwp_child_process(h: HWND, host_pid: u32) -> Option<u32> {
    let mut children: Vec<HWND> = Vec::new();
    // SAFETY: 同 enumerate_children。
    unsafe {
        let _ = EnumChildWindows(
            Some(h),
            Some(collect),
            LPARAM(&mut children as *mut _ as isize),
        );
    }
    children.into_iter().find_map(|c| {
        let mut pid = 0u32;
        // SAFETY: 只读查询。
        unsafe { GetWindowThreadProcessId(c, Some(&mut pid)) };
        (pid != 0 && pid != host_pid).then_some(pid)
    })
}

fn describe_exe(path: &Path) -> String {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(name) = cache.lock().get(path) {
        return name.clone();
    }
    let name = file_description(path)
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    cache.lock().insert(path.to_path_buf(), name.clone());
    name
}

/// 读 PE 版本资源里的 FileDescription。
fn file_description(path: &Path) -> Option<String> {
    let wpath = wide(&path.to_string_lossy());
    // SAFETY: 缓冲区大小由 GetFileVersionInfoSizeW 给出；VerQueryValueW 返回的指针
    // 指向 data 内部，data 存活期间有效。
    unsafe {
        let size = GetFileVersionInfoSizeW(pcwstr(&wpath), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(pcwstr(&wpath), None, size, data.as_mut_ptr().cast()).ok()?;

        let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let key = wide("\\VarFileInfo\\Translation");
        let mut lang = "040904b0".to_string();
        if VerQueryValueW(data.as_ptr().cast(), pcwstr(&key), &mut ptr, &mut len).as_bool()
            && len >= 4
        {
            let pair = std::slice::from_raw_parts(ptr as *const u16, 2);
            lang = format!("{:04x}{:04x}", pair[0], pair[1]);
        }
        for candidate in [lang.as_str(), "040904b0", "080404b0", "000004b0"] {
            let key = wide(&format!("\\StringFileInfo\\{candidate}\\FileDescription"));
            if VerQueryValueW(data.as_ptr().cast(), pcwstr(&key), &mut ptr, &mut len).as_bool()
                && len > 0
            {
                let chars = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                let text = from_wide(chars);
                if !text.trim().is_empty() {
                    return Some(text.trim().to_string());
                }
            }
        }
        None
    }
}

pub fn process_path(pid: u32) -> Option<PathBuf> {
    // SAFETY: 句柄在本函数内打开和关闭；缓冲区长度通过 len 传入传出。
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        ok.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(
            &buf[..len as usize],
        )))
    }
}

fn is_cloaked(h: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: 输出缓冲是一个 u32，大小如实传入。
    unsafe {
        DwmGetWindowAttribute(
            h,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
    }
}

fn frame_bounds(h: HWND) -> Option<PhysicalRect> {
    let mut r = RECT::default();
    // SAFETY: 输出缓冲是一个 RECT，大小如实传入。
    let ok = unsafe {
        DwmGetWindowAttribute(
            h,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .is_ok();
    // DWM 查询失败（窗口刚销毁等）时退回 GetWindowRect（规格 07 §4.2）
    if !ok {
        // SAFETY: 同上。
        unsafe { GetWindowRect(h, &mut r).ok()? };
    }
    Some(rect_of(r))
}

fn rect_of(r: RECT) -> PhysicalRect {
    PhysicalRect::new(
        r.left,
        r.top,
        (r.right - r.left).max(0) as u32,
        (r.bottom - r.top).max(0) as u32,
    )
}

fn window_text(h: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: 缓冲区长度由切片给出。
    let len = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn smoke_enumerate() {
        crate::platform::init_process();
        let started = std::time::Instant::now();
        let windows = super::enumerate_top_level().unwrap();
        println!(
            "{} windows in {}ms",
            windows.len(),
            started.elapsed().as_millis()
        );
        for w in windows.iter().take(20) {
            println!("#{} {:?} [{}] {}", w.z_order, w.bounds, w.app_name, w.title);
        }
        if let Some(fg) = super::foreground_window() {
            println!("foreground: {:?}", super::app_info_of(fg));
        }
    }
}
