//! 截图时框选界面元素（像 Snow Shot）：鼠标停在网页的表格、单元格、按钮、标题上时，预选框就是那个元素。
//!
//! 走 UI Automation。截图时遮罩盖在整块屏上，按点问"这里是什么"（ElementFromPoint）只会问到遮罩自己，
//! 所以从鼠标下面那个窗口的元素开始往下找：每一层取回全部子元素和它们的框（一次跨进程调用，用缓存请求
//! 把框和类型一起带回来），挑包含鼠标的最小的那个接着往下，直到没有更里面的。走过的这条链就是由里到外的
//! 候选框；每层顺带取回的兄弟元素（同一行的其他格、其他行）就是 `elements_near` 要的"附近的元素"。
//!
//! 浏览器（Chromium 系、Electron）第一次被问到时才建网页的无障碍树，那之前只问得到窗口的几层。
//! UIA 的调用都放在一个专用的 COM 线程（MTA）上，别的线程排队等，等太久就当没有。

use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use windows::core::Interface;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationCacheRequest,
    IUIAutomationCondition, IUIAutomationElement, TreeScope_Children,
    UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId,
};

use crate::error::AppResult;
use crate::platform::{PhysicalRect, UiElement, WindowHandle};

/// 比这小的不要（线、点）
const MIN_SIDE: u32 = 4;
/// 往下最多找几层（网页一般十几到二十几层）
const MAX_DEPTH: usize = 48;
/// 一层最多看多少个子元素（长列表、大表格）
const MAX_CHILDREN: i32 = 600;
/// 找一条链最多花多久
const AT_BUDGET: Duration = Duration::from_millis(250);
/// "附近的元素"：链上最里面几层的兄弟，再往外几层时连兄弟的子元素也取
const NEAR_LEVELS: usize = 4;
const NEAR_GRANDCHILDREN: i32 = 60;
const NEAR_MAX: usize = 1500;
const NEAR_BUDGET: Duration = Duration::from_millis(160);
/// 等 UIA 线程最多多久（对方程序卡住时别把截图也卡住）
const WAIT: Duration = Duration::from_millis(900);

struct Uia {
    auto: IUIAutomation,
    cache: IUIAutomationCacheRequest,
    cond: IUIAutomationCondition,
}

type Job = Box<dyn FnOnce(Option<&Uia>) + Send>;

fn worker() -> &'static Mutex<Sender<Job>> {
    static TX: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let _ = std::thread::Builder::new()
            .name("ui-automation".into())
            .spawn(move || {
                let uia = init()
                    .map_err(|e| tracing::warn!("UI Automation 用不了：{e}"))
                    .ok();
                for job in rx {
                    job(uia.as_ref());
                }
            });
        Mutex::new(tx)
    })
}

fn init() -> windows::core::Result<Uia> {
    // SAFETY: 本线程专门跑 UIA，进入多线程套间后一直不退出。
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let auto: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
        // 对方程序没响应时别等默认的几十秒
        if let Ok(a2) = auto.cast::<IUIAutomation2>() {
            let _ = a2.SetConnectionTimeout(600);
            let _ = a2.SetTransactionTimeout(600);
        }
        let cache = auto.CreateCacheRequest()?;
        cache.AddProperty(UIA_BoundingRectanglePropertyId)?;
        cache.AddProperty(UIA_ControlTypePropertyId)?;
        let cond = auto.ControlViewCondition()?;
        Ok(Uia { auto, cache, cond })
    }
}

/// 在 UIA 线程上跑，等结果；UIA 用不了、等太久都给空
fn run<T: Send + Default + 'static>(f: impl FnOnce(&Uia) -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    let job: Job = Box::new(move |uia| {
        let _ = tx.send(uia.map(f).unwrap_or_default());
    });
    if worker().lock().send(job).is_err() {
        return T::default();
    }
    rx.recv_timeout(WAIT).unwrap_or_default()
}

/// 一个元素：框（屏幕物理像素）和类型
#[derive(Clone)]
struct Node {
    el: IUIAutomationElement,
    rect: PhysicalRect,
    role: &'static str,
}

fn to_rect(r: RECT) -> Option<PhysicalRect> {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    (w >= MIN_SIDE as i32 && h >= MIN_SIDE as i32)
        .then(|| PhysicalRect::new(r.left, r.top, w as u32, h as u32))
}

fn node(el: IUIAutomationElement) -> Option<Node> {
    // SAFETY: 取缓存里的属性，不跨进程。
    let (rect, kind) = unsafe {
        (
            el.CachedBoundingRectangle().ok()?,
            el.CachedControlType().ok()?,
        )
    };
    Some(Node {
        rect: to_rect(rect)?,
        role: role_name(kind.0),
        el,
    })
}

impl Uia {
    /// 一个元素的子元素（带框和类型），最多 `max` 个
    fn children(&self, el: &IUIAutomationElement, max: i32) -> Vec<Node> {
        // SAFETY: 一次跨进程调用取回全部子元素和缓存的属性。
        let Ok(list) =
            (unsafe { el.FindAllBuildCache(TreeScope_Children, &self.cond, &self.cache) })
        else {
            return Vec::new();
        };
        // SAFETY: 读数组。
        let n = unsafe { list.Length() }.unwrap_or(0).min(max);
        (0..n)
            .filter_map(|i| unsafe { list.GetElement(i) }.ok())
            .filter_map(node)
            .collect()
    }

    /// 从窗口往下找到鼠标下面最里面的元素。返回由外到里的链（第一个是窗口），和每一层的全部子元素
    fn descend(
        &self,
        window: WindowHandle,
        x: i32,
        y: i32,
        budget: Duration,
    ) -> (Vec<Node>, Vec<Vec<Node>>) {
        let started = Instant::now();
        let hwnd = HWND(window.0 as usize as *mut _);
        // SAFETY: 句柄只是拿来问 UIA；窗口没了会返回错误。
        let Some(root) = (unsafe { self.auto.ElementFromHandleBuildCache(hwnd, &self.cache) })
            .ok()
            .and_then(node)
        else {
            return (Vec::new(), Vec::new());
        };
        let mut kids = self.children(&root.el, MAX_CHILDREN);
        let mut chain = vec![root];
        let mut levels = Vec::new();
        while chain.len() < MAX_DEPTH && started.elapsed() < budget {
            // 包含鼠标的，由小到大
            let mut hits: Vec<&Node> = kids.iter().filter(|k| k.rect.contains_point(x, y)).collect();
            hits.sort_by_key(|k| k.rect.area());
            // 叠在一起的取小的（浮层、单元格里的按钮），但盖满整块的空壳不算：浏览器在网页上面叠着好几层
            // 一样大的空窗格（绘制用的、遮罩用的），真正的网页内容在旁边那个更大一点的里面。
            // 所以挑第一个"里面还有东西在鼠标下面"的；都没有就取最小的（它就是最里面了）
            let mut picked: Option<(Node, Vec<Node>)> = None;
            for h in hits.iter().take(6) {
                let inner = self.children(&h.el, MAX_CHILDREN);
                let deeper = inner.iter().any(|k| k.rect.contains_point(x, y));
                if deeper {
                    picked = Some(((*h).clone(), inner));
                    break;
                }
                if picked.is_none() {
                    picked = Some(((*h).clone(), inner));
                }
            }
            levels.push(std::mem::take(&mut kids));
            let Some((next, inner)) = picked else { break };
            // 和外面一样大的套娃也往下走（网页里很常见），只是不重复给
            chain.push(next);
            kids = inner;
        }
        (chain, levels)
    }
}

/// `window` 里、屏幕物理坐标 `(x, y)` 下面的界面元素，由里到外（最后一个是窗口本身）。
/// UIA 用不了、点在自己的窗口上时返回空。
pub fn elements_at(window: WindowHandle, x: i32, y: i32) -> AppResult<Vec<UiElement>> {
    if super::window_enum::is_own_window(window) {
        return Ok(Vec::new());
    }
    Ok(run(move |uia| {
        let (chain, _) = uia.descend(window, x, y, AT_BUDGET);
        let mut out: Vec<UiElement> = Vec::new();
        for n in chain.iter().rev() {
            if n.rect.contains_point(x, y) && out.last().map(|e| e.rect) != Some(n.rect) {
                out.push(UiElement {
                    rect: n.rect,
                    role: n.role.to_string(),
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
        out
    }))
}

/// `(x, y)` 附近的元素：链上最里面几层的兄弟（同一行的其他格、其他行），再往外几层时连兄弟的子元素
/// （行里的格）也取。不保证包含这个点。遮罩那边存起来，鼠标在附近挪动时在本地找。有时间上限。
pub fn elements_near(window: WindowHandle, x: i32, y: i32) -> AppResult<Vec<UiElement>> {
    if super::window_enum::is_own_window(window) {
        return Ok(Vec::new());
    }
    Ok(run(move |uia| {
        let started = Instant::now();
        let (chain, levels) = uia.descend(window, x, y, AT_BUDGET);
        let Some(window_rect) = chain.first().map(|n| n.rect) else {
            return Vec::new();
        };
        let mut out: Vec<UiElement> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut push = |n: &Node, out: &mut Vec<UiElement>| {
            if let Some(rect) = n.rect.intersect(&window_rect) {
                if rect.width >= MIN_SIDE
                    && rect.height >= MIN_SIDE
                    && seen.insert((rect.x, rect.y, rect.width, rect.height))
                {
                    out.push(UiElement {
                        rect,
                        role: n.role.to_string(),
                    });
                }
            }
        };
        // levels[i] 是 chain[i] 的子元素；从最里面那层往外
        'levels: for (depth, kids) in levels.iter().rev().take(NEAR_LEVELS).enumerate() {
            for k in kids {
                if out.len() >= NEAR_MAX || started.elapsed() > NEAR_BUDGET {
                    break 'levels;
                }
                push(k, &mut out);
                // 再往外的几层（表、列表）：把每个孩子（行）的孩子（格）也取了
                if depth >= 1 && !k.rect.contains_point(x, y) {
                    for g in uia.children(&k.el, NEAR_GRANDCHILDREN) {
                        push(&g, &mut out);
                    }
                }
            }
        }
        tracing::debug!(
            count = out.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "附近的界面元素"
        );
        out
    }))
}

/// UIA 的控件类型 → 名字（页面按名字挑：文字片段、滚动条这类不当预选框）
fn role_name(id: i32) -> &'static str {
    match id {
        50000 => "Button",
        50001 => "Calendar",
        50002 => "CheckBox",
        50003 => "ComboBox",
        50004 => "Edit",
        50005 => "Hyperlink",
        50006 => "Image",
        50007 => "ListItem",
        50008 => "List",
        50009 => "Menu",
        50010 => "MenuBar",
        50011 => "MenuItem",
        50012 => "ProgressBar",
        50013 => "RadioButton",
        50014 => "ScrollBar",
        50015 => "Slider",
        50016 => "Spinner",
        50017 => "StatusBar",
        50018 => "Tab",
        50019 => "TabItem",
        50020 => "Text",
        50021 => "ToolBar",
        50022 => "ToolTip",
        50023 => "Tree",
        50024 => "TreeItem",
        50026 => "Group",
        50027 => "Thumb",
        50028 => "DataGrid",
        50029 => "DataItem",
        50030 => "Document",
        50031 => "SplitButton",
        50032 => "Window",
        50033 => "Pane",
        50034 => "Header",
        50035 => "HeaderItem",
        50036 => "Table",
        50037 => "TitleBar",
        50038 => "Separator",
        _ => "Custom",
    }
}
