//! 截图时框选界面元素（像 Snow Shot）：鼠标停在网页的表格、单元格、按钮、标题上时，预选框就是那个元素。
//!
//! 走辅助功能接口：在鼠标下面那个窗口所属的应用里做命中测试（AXUIElementCopyElementAtPosition），
//! 再顺着父元素一路往上到窗口，每一层的位置和大小就是一个候选框（由小到大）。每个元素的角色、位置、
//! 大小一次跨进程取完（AXUIElementCopyMultipleAttributeValues），一条链十几次往返、几毫秒。
//!
//! 光有这条链的话，鼠标从一个单元格挪到旁边那格时，新的结果回来之前只认得出外面的整张表，预选框会先
//! 跳到整张表再缩回单元格，一闪一闪的。所以另外有 `elements_near`：把这条链上往上几层的子元素（同一行的
//! 其他格、其他行和它们的格）也取回来，遮罩那边先存着，鼠标在附近挪动时直接在本地找，不用等。
//!
//! 浏览器（Chromium 系、Electron）默认不建网页的辅助功能树，第一次问到某个应用时打开它
//! （AXManualAccessibility，Chromium 系再加 AXEnhancedUserInterface，和读屏软件的做法一样），
//! 树建好之前只能问到窗口那一层。没有「辅助功能」权限时什么都不返回，照旧只按整个窗口框选。

use std::collections::HashSet;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2_core_foundation::{CGPoint, CGSize};
use objc2_foundation::{NSArray, NSString};
use parking_lot::Mutex;

use super::ffi::{self, CFTypeRef};
use super::geometry::{self, Screen};
use super::util::{handle_pid, own_pid};
use crate::error::AppResult;
use crate::platform::{PhysicalRect, UiElement, WindowHandle};

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyElementAtPosition(
        application: CFTypeRef,
        x: f32,
        y: f32,
        element: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementSetAttributeValue(
        element: CFTypeRef,
        attribute: CFTypeRef,
        value: CFTypeRef,
    ) -> i32;
    fn AXUIElementCopyMultipleAttributeValues(
        element: CFTypeRef,
        attributes: CFTypeRef,
        options: u32,
        values: *mut CFTypeRef,
    ) -> i32;
    fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut c_void) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(array: CFTypeRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
}

/// 一个 CFArray 里第 `i` 个（不持有）。不是数组、越界、里面是空的都给 None —— 辅助功能给的
/// 子元素数组里偶尔夹着空项，走 NSArray 的接口会拿到一个空的"对象"，释放时整个程序崩掉
unsafe fn array_items(array: CFTypeRef) -> Vec<CFTypeRef> {
    if array.is_null() || ffi::CFGetTypeID(array) != CFArrayGetTypeID() {
        return Vec::new();
    }
    (0..CFArrayGetCount(array).max(0))
        .map(|i| CFArrayGetValueAtIndex(array, i))
        .filter(|v| !v.is_null())
        .collect()
}

const AX_VALUE_CGPOINT: u32 = 1;
const AX_VALUE_CGSIZE: u32 = 2;
/// 往上最多走几层（网页里嵌套很深，到窗口之前十几二十层很常见）
const MAX_DEPTH: usize = 40;
/// 比这还小（物理像素）的元素不要：一个字、一根线框不住东西
const MIN_SIDE: u32 = 8;
/// 附近元素：从最里层往上取几层父元素的子元素；每个父元素最多看多少个孩子；总数和时间上限
const NEAR_LEVELS: usize = 4;
const NEAR_CHILDREN: usize = 300;
const NEAR_GRANDCHILDREN: usize = 60;
const NEAR_MAX: usize = 1500;
const NEAR_BUDGET: Duration = Duration::from_millis(160);

/// Chromium 系浏览器：要另外打开 AXEnhancedUserInterface 才建网页的辅助功能树
const CHROMIUM: &[&str] = &[
    "com.google.Chrome",
    "com.google.Chrome.beta",
    "com.google.Chrome.canary",
    "com.microsoft.edgemac",
    "com.microsoft.edgemac.Beta",
    "com.brave.Browser",
    "company.thebrowser.Browser",
    "com.vivaldi.Vivaldi",
    "com.operasoftware.Opera",
    "org.chromium.Chromium",
];

/// 已经打开过辅助功能树的应用（进程号）
static ENABLED: Mutex<Option<HashSet<i32>>> = Mutex::new(None);

/// 自己持有的一个辅助功能对象，drop 时释放
struct Ax(CFTypeRef);

impl Drop for Ax {
    fn drop(&mut self) {
        // SAFETY: 构造时拿到的是 +1 的引用
        unsafe { ffi::CFRelease(self.0) };
    }
}

fn attr(name: &str) -> Retained<NSString> {
    NSString::from_str(name)
}

fn cf<T>(obj: &T) -> CFTypeRef {
    std::ptr::from_ref(obj).cast()
}

/// 浏览器之类第一次被问到时，让它把网页的辅助功能树建起来
pub(super) fn enable_tree(pid: i32, app: CFTypeRef) {
    {
        let mut guard = ENABLED.lock();
        let set = guard.get_or_insert_with(HashSet::new);
        if !set.insert(pid) {
            return;
        }
    }
    let chromium = super::window_enum::running_app(pid)
        .and_then(|a| a.bundleIdentifier())
        .is_some_and(|id| CHROMIUM.contains(&id.to_string().as_str()));
    // SAFETY: app 是有效的辅助功能元素；属性名和布尔值都是常量。对方不支持就返回错误，忽略
    unsafe {
        let manual = attr("AXManualAccessibility");
        AXUIElementSetAttributeValue(app, cf(&*manual), ffi::kCFBooleanTrue);
        if chromium {
            let enhanced = attr("AXEnhancedUserInterface");
            AXUIElementSetAttributeValue(app, cf(&*enhanced), ffi::kCFBooleanTrue);
        }
    }
    tracing::debug!(pid, chromium, "打开了应用的辅助功能树");
}

/// 读一个元素的属性（拿到的要自己释放）
unsafe fn copy_attr(element: CFTypeRef, name: &str) -> Option<Ax> {
    let name = attr(name);
    let mut value: CFTypeRef = std::ptr::null();
    let err = ffi::AXUIElementCopyAttributeValue(element, cf(&*name), &mut value);
    // 别用 then_some(Ax(value))：它先把 Ax 建出来，条件不成立时立刻 drop —— 对一个空指针 CFRelease，整个程序崩掉
    if err == 0 && !value.is_null() {
        Some(Ax(value))
    } else {
        None
    }
}

/// 一个元素的角色、位置、大小，一次跨进程取完。位置大小取不到时框是空的
unsafe fn read(element: CFTypeRef, screens: &[Screen]) -> Option<(String, PhysicalRect)> {
    let names = NSArray::from_retained_slice(&[attr("AXRole"), attr("AXPosition"), attr("AXSize")]);
    let mut values: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyMultipleAttributeValues(element, cf(&*names), 0, &mut values);
    if err != 0 || values.is_null() {
        return None;
    }
    let values = Ax(values);
    let list = array_items(values.0);
    if list.len() < 3 {
        return None;
    }
    let (role_ref, pos, size) = (list[0], list[1], list[2]);
    let role = if ffi::CFGetTypeID(role_ref) == ffi::CFStringGetTypeID() {
        (*role_ref.cast::<NSString>()).to_string()
    } else {
        String::new()
    };
    let mut p = CGPoint::new(0.0, 0.0);
    let mut s = CGSize::new(0.0, 0.0);
    // 取不到的那项是一个"错误"类型的值，按位置 / 大小读会失败
    if !AXValueGetValue(pos, AX_VALUE_CGPOINT, std::ptr::from_mut(&mut p).cast())
        || !AXValueGetValue(size, AX_VALUE_CGSIZE, std::ptr::from_mut(&mut s).cast())
    {
        return Some((role, PhysicalRect::default()));
    }
    let rect = geometry::rect_to_physical_in(screens, geometry::rect(p.x, p.y, s.width, s.height));
    Some((role, rect))
}

/// 元素的子元素（最多 `cap` 个）
unsafe fn children(element: CFTypeRef, cap: usize) -> Vec<Ax> {
    let Some(list) = copy_attr(element, "AXChildren") else {
        return Vec::new();
    };
    array_items(list.0)
        .into_iter()
        .take(cap)
        // 数组里的不归我们，各自 retain 一下再交给 Ax
        .map(|item| Ax(CFRetain(item)))
        .collect()
}

fn usable(rect: &PhysicalRect) -> bool {
    rect.width >= MIN_SIDE && rect.height >= MIN_SIDE
}

/// 命中测试 + 顺着父元素往上：由里到外的元素（含窗口本身），和它们的框
struct Chain {
    nodes: Vec<(Ax, String, PhysicalRect)>,
    screens: Vec<Screen>,
}

fn hit_chain(window: WindowHandle, x: i32, y: i32) -> Option<Chain> {
    let pid = handle_pid(window.0);
    if pid == own_pid() || pid <= 0 {
        return None;
    }
    // SAFETY: 都是辅助功能的只读查询（打开辅助功能树除外，见 enable_tree）；拿到的对象由 Ax 释放
    unsafe {
        if !ffi::AXIsProcessTrusted() {
            return None;
        }
        let screens = geometry::screens();
        let at = geometry::rect_to_points_in(&screens, PhysicalRect::new(x, y, 1, 1)).origin;
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let app = Ax(app);
        // 对方程序卡住时别跟着卡
        ffi::AXUIElementSetMessagingTimeout(app.0, 0.15);
        enable_tree(pid, app.0);
        let mut element: CFTypeRef = std::ptr::null();
        let err = AXUIElementCopyElementAtPosition(app.0, at.x as f32, at.y as f32, &mut element);
        if err != 0 || element.is_null() {
            return None;
        }
        let mut nodes = Vec::new();
        let mut current = Ax(element);
        for _ in 0..MAX_DEPTH {
            let Some((role, rect)) = read(current.0, &screens) else {
                break;
            };
            if role == "AXApplication" {
                break;
            }
            let is_window = role == "AXWindow";
            let parent = copy_attr(current.0, "AXParent");
            nodes.push((current, role, rect));
            if is_window {
                break;
            }
            let Some(parent) = parent else { break };
            current = parent;
        }
        Some(Chain { nodes, screens })
    }
}

/// `window` 里、屏幕物理坐标 `(x, y)` 下面的界面元素，由里到外（最后一个是窗口本身）。
/// 没权限、对方不支持、点在自己的窗口上时返回空。
pub fn elements_at(window: WindowHandle, x: i32, y: i32) -> AppResult<Vec<UiElement>> {
    let Some(chain) = hit_chain(window, x, y) else {
        return Ok(Vec::new());
    };
    let mut out: Vec<UiElement> = Vec::new();
    for (_, role, rect) in &chain.nodes {
        // 和上一层一样大的不重复给（网页里一层套一层同样大的容器很常见）
        if usable(rect) && rect.contains_point(x, y) && out.last().map(|e| e.rect) != Some(*rect) {
            out.push(UiElement {
                rect: *rect,
                role: role.clone(),
            });
        }
    }
    // 每层都裁到外面那层里面：网页内容比可视区大（滚动区域里的整篇文档）时只留看得见的部分
    let mut clip: Option<PhysicalRect> = None;
    for e in out.iter_mut().rev() {
        if let Some(outer) = clip {
            if let Some(inner) = e.rect.intersect(&outer) {
                e.rect = inner;
            }
        }
        clip = Some(e.rect);
    }
    Ok(out)
}

/// `(x, y)` 附近的元素：这条链上往上几层父元素的子元素（同一行的其他格、其他行和行里的格……），
/// 不保证包含这个点。遮罩那边存起来，鼠标在附近挪动时在本地找。有时间上限，取到多少给多少。
pub fn elements_near(window: WindowHandle, x: i32, y: i32) -> AppResult<Vec<UiElement>> {
    let started = Instant::now();
    let Some(chain) = hit_chain(window, x, y) else {
        return Ok(Vec::new());
    };
    let window_rect = chain
        .nodes
        .iter()
        .find(|(_, role, _)| role == "AXWindow")
        .map(|(_, _, r)| *r);
    let mut out: Vec<UiElement> = Vec::new();
    let mut seen: HashSet<(i32, i32, u32, u32)> = HashSet::new();
    let mut push = |role: String, rect: PhysicalRect, out: &mut Vec<UiElement>| {
        // 只留窗口里看得见的部分
        let Some(rect) = window_rect.map_or(Some(rect), |w| rect.intersect(&w)) else {
            return;
        };
        if usable(&rect) && seen.insert((rect.x, rect.y, rect.width, rect.height)) {
            out.push(UiElement { rect, role });
        }
    };
    let over = |out: &Vec<UiElement>| out.len() >= NEAR_MAX || started.elapsed() > NEAR_BUDGET;
    // 跳过链上最里面那个（它多半是一段文字），从它的父元素开始往上
    'levels: for (level, (parent, role, _)) in
        chain.nodes.iter().skip(1).take(NEAR_LEVELS).enumerate()
    {
        if role == "AXWindow" {
            break;
        }
        // SAFETY: 只读查询，拿到的对象由 Ax 释放
        unsafe {
            for child in children(parent.0, NEAR_CHILDREN) {
                if over(&out) {
                    break 'levels;
                }
                let Some((role, rect)) = read(child.0, &chain.screens) else {
                    continue;
                };
                push(role, rect, &mut out);
                // 再往上的几层（表、列表）：把每个孩子（行）的孩子（格）也取了
                if level >= 1 {
                    for grand in children(child.0, NEAR_GRANDCHILDREN) {
                        if over(&out) {
                            break 'levels;
                        }
                        if let Some((role, rect)) = read(grand.0, &chain.screens) {
                            push(role, rect, &mut out);
                        }
                    }
                }
            }
        }
    }
    tracing::debug!(
        count = out.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "附近的界面元素"
    );
    Ok(out)
}
