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
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayUnaccessData,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationCacheRequest,
    IUIAutomationCondition, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationTextRange, IUIAutomationValuePattern, TextPatternRangeEndpoint_End,
    TextPatternRangeEndpoint_Start, TextUnit_Character, TreeScope_Children,
    UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId, UIA_TextPatternId,
    UIA_ValuePatternId,
};

use crate::error::AppResult;
use crate::platform::{PhysicalRect, TextBounds, UiElement, WindowHandle};

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

/// 文字范围在屏幕上的框（物理像素）：`GetBoundingRectangles` 给的是一串 x, y, w, h（每行一个框）
fn range_rects(range: &IUIAutomationTextRange) -> Vec<PhysicalRect> {
    // SAFETY: 跨进程取一个 SAFEARRAY(double)，读完就销毁。
    unsafe {
        let Ok(psa) = range.GetBoundingRectangles() else {
            return Vec::new();
        };
        if psa.is_null() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let (Ok(lo), Ok(hi)) = (SafeArrayGetLBound(psa, 1), SafeArrayGetUBound(psa, 1)) {
            let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
            if hi >= lo && SafeArrayAccessData(psa, &mut data).is_ok() {
                let vals = std::slice::from_raw_parts(data as *const f64, (hi - lo + 1) as usize);
                for q in vals.chunks_exact(4) {
                    let (w, h) = (q[2].round() as i32, q[3].round() as i32);
                    if w > 0 && h > 0 {
                        out.push(PhysicalRect::new(q[0].round() as i32, q[1].round() as i32, w as u32, h as u32));
                    }
                }
                let _ = SafeArrayUnaccessData(psa);
            }
        }
        let _ = SafeArrayDestroy(psa);
        out
    }
}

fn union(rects: &[PhysicalRect]) -> Option<PhysicalRect> {
    let x0 = rects.iter().map(|r| r.x).min()?;
    let y0 = rects.iter().map(|r| r.y).min()?;
    let x1 = rects.iter().map(|r| r.right()).max()?;
    let y1 = rects.iter().map(|r| r.bottom()).max()?;
    Some(PhysicalRect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// 选区里从头（`forward`）或从尾数起第一个看得见的字的框；最多看 32 个字
fn visible_char(range: &IUIAutomationTextRange, forward: bool) -> Option<PhysicalRect> {
    // SAFETY: UIA 文字范围的查询。
    unsafe {
        let c = range.Clone().ok()?;
        // 收成选区的开头 / 结尾那一点
        if forward {
            c.MoveEndpointByRange(TextPatternRangeEndpoint_End, range, TextPatternRangeEndpoint_Start).ok()?;
        } else {
            c.MoveEndpointByRange(TextPatternRangeEndpoint_Start, range, TextPatternRangeEndpoint_End).ok()?;
        }
        for _ in 0..32 {
            // 往里扩一个字：c 就是那一个字
            let moved = if forward {
                c.MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, 1).ok()?
            } else {
                c.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, -1).ok()?
            };
            if moved == 0 {
                return None;
            }
            // 走出选区了
            let outside = if forward {
                c.CompareEndpoints(TextPatternRangeEndpoint_End, range, TextPatternRangeEndpoint_End).ok()? > 0
            } else {
                c.CompareEndpoints(TextPatternRangeEndpoint_Start, range, TextPatternRangeEndpoint_Start).ok()? < 0
            };
            if outside {
                return None;
            }
            let text = c.GetText(4).map(|b| b.to_string()).unwrap_or_default();
            let real = text.chars().any(|ch| !ch.is_whitespace() && ch != '\u{FFFC}' && !ch.is_control());
            if real {
                if let Some(r) = range_rects(&c).last().copied().filter(|r| r.width >= 2 && r.height >= 4) {
                    return Some(r);
                }
            }
            // 不算数：收回到这个字的另一头，接着看下一个
            if forward {
                c.MoveEndpointByRange(TextPatternRangeEndpoint_Start, &c, TextPatternRangeEndpoint_End).ok()?;
            } else {
                c.MoveEndpointByRange(TextPatternRangeEndpoint_End, &c, TextPatternRangeEndpoint_Start).ok()?;
            }
        }
        None
    }
}

impl Uia {
    /// 从 `el` 往上找支持文字模式（TextPattern）的元素，最多几层
    fn text_pattern(&self, el: IUIAutomationElement) -> Option<IUIAutomationTextPattern> {
        // SAFETY: UIA 查询。
        unsafe {
            let walker = self.auto.ControlViewWalker().ok()?;
            let mut cur = el;
            for _ in 0..8 {
                if let Ok(p) = cur.GetCurrentPattern(UIA_TextPatternId) {
                    if let Ok(tp) = p.cast::<IUIAutomationTextPattern>() {
                        // 有选区才算（网页里点到的可能是没选中东西的外层文档）
                        if tp.GetSelection().and_then(|s| s.Length()).is_ok_and(|n| n > 0) {
                            return Some(tp);
                        }
                    }
                }
                cur = walker.GetParentElement(&cur).ok()?;
            }
            None
        }
    }

    fn selection_bounds(&self, x: i32, y: i32) -> Option<TextBounds> {
        // SAFETY: UIA 查询。
        unsafe {
            // 先问有焦点的元素（编辑框、文档），不行再问鼠标下面的
            let tp = self
                .auto
                .GetFocusedElement()
                .ok()
                .and_then(|el| self.text_pattern(el))
                .or_else(|| {
                    let el = self.auto.ElementFromPoint(POINT { x, y }).ok()?;
                    self.text_pattern(el)
                })?;
            let range = tp.GetSelection().ok()?.GetElement(0).ok()?;
            let lines = range_rects(&range);
            let whole = union(&lines)?;
            // 第一个、最后一个"看得见的字"：从选区开头往后、从结尾往前一个字一个字地看，跳过空白、换行和
            // 嵌在网页里的对象（文本框、图片在文字里是一个占位字符，框常常只有 1×1 或者是整个文本框）。
            // 拖过了头、在空白处松手时，选区结尾往往就是这种东西
            let first = visible_char(&range, true);
            let last = visible_char(&range, false);
            Some(TextBounds { whole, first, last })
        }
    }
}

/// 叫醒鼠标下面那个程序的无障碍树：Chrome、Edge、Electron 程序平时不建，第一次被 UIA 问到时才开始建，
/// 那一次什么也问不到（新版 Chromium 一阵子没人问还会自己关掉）。划词时在文字上按下鼠标就先问一下，
/// 等松开鼠标再问选区时树已经建好了。不等结果；一秒内只叫一次
pub fn warm_up(x: i32, y: i32) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = LAST.lock();
        if last.is_some_and(|t| t.elapsed() < Duration::from_millis(1000)) {
            return;
        }
        *last = Some(Instant::now());
    }
    let job: Job = Box::new(move |uia| {
        let Some(uia) = uia else { return };
        // SAFETY: UIA 查询，结果不要。
        unsafe {
            if let Ok(el) = uia.auto.ElementFromPoint(POINT { x, y }) {
                let _ = el.GetCurrentPattern(UIA_TextPatternId);
            }
            if let Ok(el) = uia.auto.GetFocusedElement() {
                let _ = el.GetCurrentPattern(UIA_TextPatternId);
            }
        }
    });
    let _ = worker().lock().send(job);
}

/// 现在选中的字在屏幕上的位置（划词按钮"跟着文段"用）。`(x, y)` 是松开鼠标的点，有焦点的元素问不到时
/// 从它下面的元素往上找。程序不支持 UIA 的文字模式（不少原生程序、老程序）就是 None
pub fn selection_bounds(x: i32, y: i32) -> Option<TextBounds> {
    let (tx, rx) = mpsc::channel();
    let job: Job = Box::new(move |uia| {
        let _ = tx.send(uia.and_then(|u| u.selection_bounds(x, y)));
    });
    worker().lock().send(job).ok()?;
    // 等不了太久：按钮要跟着手出来
    rx.recv_timeout(Duration::from_millis(250)).ok().flatten()
}

/// 屏幕上这个点下面是不是能打字的地方（输入框、编辑器、网页里的可编辑区域）：拖剪贴板卡片经过时
/// 影子缩小一下，表示松手就贴在这里。DATO OCR 自己的窗口不算。问不到（程序不支持 UIA、卡住）是 None
pub fn editable_at(x: i32, y: i32) -> Option<bool> {
    let (tx, rx) = mpsc::channel();
    let job: Job = Box::new(move |uia| {
        let _ = tx.send(uia.and_then(|u| u.editable_at(x, y)));
    });
    worker().lock().send(job).ok()?;
    rx.recv_timeout(Duration::from_millis(200)).ok().flatten()
}

impl Uia {
    fn editable_at(&self, x: i32, y: i32) -> Option<bool> {
        const EDIT: i32 = 50004;
        const DOCUMENT: i32 = 50030;
        const PANE: i32 = 50033;
        const WINDOW: i32 = 50032;
        // SAFETY: UIA 查询。
        unsafe {
            let el = self.auto.ElementFromPoint(POINT { x, y }).ok()?;
            if el.CurrentProcessId().ok()? as u32 == std::process::id() {
                return Some(false);
            }
            let walker = self.auto.ControlViewWalker().ok()?;
            let mut cur = el;
            // 文字在可编辑区域里面一两层（网页的 contenteditable、富文本编辑器）：往上看几层
            for _ in 0..4 {
                let kind = cur.CurrentControlType().map_or(0, |t| t.0);
                if kind == PANE || kind == WINDOW {
                    return Some(false);
                }
                let read_only = cur
                    .GetCurrentPattern(UIA_ValuePatternId)
                    .ok()
                    .and_then(|p| p.cast::<IUIAutomationValuePattern>().ok())
                    .and_then(|v| v.CurrentIsReadOnly().ok())
                    .map(|b| b.as_bool());
                match (kind, read_only) {
                    (_, Some(true)) => return Some(false),
                    (EDIT, _) => return Some(true),
                    // 可编辑的文档（记事本、Word、网页编辑器）：有值模式且不是只读
                    (_, Some(false)) if kind == DOCUMENT || cur.CurrentIsKeyboardFocusable().is_ok_and(|b| b.as_bool()) => {
                        return Some(true)
                    }
                    (DOCUMENT, None) => return Some(false),
                    _ => {}
                }
                cur = walker.GetParentElement(&cur).ok()?;
            }
            Some(false)
        }
    }
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
