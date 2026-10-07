//! 窗口枚举。
//!
//! `CGWindowListCopyWindowInfo` 按 Z 序从上到下给出屏幕上的窗口，矩形是全局点坐标。
//! 普通应用窗口在第 0 层；菜单栏、程序坞、通知、我们自己的遮罩和面板都在别的层，按层过滤掉。
//! 窗口标题要有屏幕录制权限才拿得到，没有时是空串，不影响框选。

use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

use super::ffi::{self, CGPoint};
use super::geometry;
use super::util::{encode_handle, handle_pid, own_pid};
use crate::error::AppResult;
use crate::platform::{AppInfo, PhysicalRect, WindowHandle, WindowInfo};

/// 比这还小的窗口不参与自动框选（各种应用都有些零碎的小辅助窗口）
const MIN_WINDOW_SIZE: f64 = 24.0;

struct RawWindow {
    id: u32,
    pid: i32,
    layer: i64,
    alpha: f64,
    bounds: ffi::CGRect,
    owner: String,
    title: String,
}

type Dict = NSDictionary<NSString, AnyObject>;

fn number(dict: &Dict, key: &str) -> Option<Retained<NSNumber>> {
    dict.objectForKey(&NSString::from_str(key))?
        .downcast::<NSNumber>()
        .ok()
}

fn string(dict: &Dict, key: &str) -> String {
    dict.objectForKey(&NSString::from_str(key))
        .and_then(|v| v.downcast::<NSString>().ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// 屏幕上的全部窗口，从上到下。
fn on_screen_windows() -> Vec<RawWindow> {
    // SAFETY: 返回的 CFArray 归我们所有（Copy 规则），里面是 CFDictionary；
    // 两者都能当对应的 NS 类型用，交给 Retained 管理释放。
    let list: Option<Retained<NSArray<Dict>>> = unsafe {
        let array = ffi::CGWindowListCopyWindowInfo(
            ffi::kCGWindowListOptionOnScreenOnly | ffi::kCGWindowListExcludeDesktopElements,
            ffi::kCGNullWindowID,
        );
        Retained::from_raw(array.cast_mut().cast())
    };
    let Some(list) = list else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(list.count());
    for dict in list.iter() {
        let Some(id) = number(&dict, "kCGWindowNumber") else {
            continue;
        };
        let bounds = dict
            .objectForKey(&NSString::from_str("kCGWindowBounds"))
            .and_then(|b| b.downcast::<NSDictionary>().ok())
            .map(|b| {
                // SAFETY: kCGWindowBounds 是 {X, Y, Width, Height} → 数字 的字典。
                let b: &Dict = unsafe { &*std::ptr::from_ref(&*b).cast() };
                let f = |k| number(b, k).map(|n| n.doubleValue()).unwrap_or(0.0);
                geometry::rect(f("X"), f("Y"), f("Width"), f("Height"))
            });
        let Some(bounds) = bounds else { continue };
        out.push(RawWindow {
            id: id.unsignedIntValue(),
            pid: number(&dict, "kCGWindowOwnerPID").map_or(0, |n| n.intValue()),
            layer: number(&dict, "kCGWindowLayer").map_or(0, |n| n.longLongValue()),
            alpha: number(&dict, "kCGWindowAlpha").map_or(1.0, |n| n.doubleValue()),
            bounds,
            owner: string(&dict, "kCGWindowOwnerName"),
            title: string(&dict, "kCGWindowName"),
        });
    }
    out
}

pub fn enumerate_top_level() -> AppResult<Vec<WindowInfo>> {
    let screens = geometry::screens();
    let own = own_pid();
    let mut out = Vec::new();
    for w in on_screen_windows() {
        if w.layer != 0 || w.alpha < 0.05 || w.pid == own {
            continue;
        }
        if w.bounds.size.width < MIN_WINDOW_SIZE || w.bounds.size.height < MIN_WINDOW_SIZE {
            continue;
        }
        let bounds = geometry::rect_to_physical_in(&screens, w.bounds);
        if bounds.is_empty() {
            continue;
        }
        out.push(WindowInfo {
            handle: WindowHandle(encode_handle(w.pid, w.id)),
            bounds,
            z_order: out.len() as u32,
            title: w.title,
            app_name: w.owner,
            process_id: w.pid as u32,
            children: Vec::new(),
        });
    }
    Ok(out)
}

/// 窗口里的控件矩形要走辅助功能接口（AXUIElement）逐个跨进程去问，一个浏览器窗口就是几千次
/// 往返，赶不上热键到遮罩出现的时间预算。这一版只按整个窗口框选。
pub fn enumerate_children(_window: WindowHandle) -> AppResult<Vec<PhysicalRect>> {
    Ok(Vec::new())
}

/// 某个屏幕点上最上面的那个普通窗口。我们自己的遮罩、底图、面板都不在第 0 层，天然被跳过；
/// 自己的主窗口、识字窗口在第 0 层，照常算数（长截图的目标可以是它们）。
pub fn window_at(x: i32, y: i32) -> Option<WindowHandle> {
    let screens = geometry::screens();
    let hit = on_screen_windows().into_iter().find(|w| {
        w.layer == 0
            && w.alpha >= 0.05
            && geometry::rect_to_physical_in(&screens, w.bounds).contains_point(x, y)
    })?;
    Some(WindowHandle(encode_handle(hit.pid, hit.id)))
}

pub fn is_own_window(window: WindowHandle) -> bool {
    handle_pid(window.0) == own_pid()
}

/// 当前激活的应用最上面的窗口。应用没有窗口（只剩桌面的访达）时窗口号为 0，
/// 进程号照样带着，之后把焦点还给它时够用。
pub fn foreground_window() -> Option<WindowHandle> {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    let pid = app.processIdentifier();
    let window = on_screen_windows()
        .into_iter()
        .find(|w| w.pid == pid && w.layer == 0)
        .map_or(0, |w| w.id);
    Some(WindowHandle(encode_handle(pid, window)))
}

pub fn running_app(pid: i32) -> Option<Retained<NSRunningApplication>> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
}

/// 窗口所属的应用：显示名（"微信"、"Safari 浏览器"）和 .app 的路径。
pub fn app_info_of(window: WindowHandle) -> Option<AppInfo> {
    let app = running_app(handle_pid(window.0))?;
    let path = app
        .bundleURL()
        .and_then(|url| url.path())
        .map(|p| PathBuf::from(p.to_string()));
    let name = app
        .localizedName()
        .map(|n| n.to_string())
        .filter(|n| !n.trim().is_empty())
        .or_else(|| {
            path.as_ref()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    Some(AppInfo {
        name,
        exe_path: path,
    })
}

/// 鼠标位置（全局点坐标）。
pub fn cursor_point() -> Option<CGPoint> {
    // SAFETY: 空来源的事件带着当前鼠标位置；用完释放。
    unsafe {
        let event = ffi::CGEventCreate(std::ptr::null_mut());
        if event.is_null() {
            return None;
        }
        let p = ffi::CGEventGetLocation(event);
        ffi::CFRelease(event.cast_const());
        Some(p)
    }
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
