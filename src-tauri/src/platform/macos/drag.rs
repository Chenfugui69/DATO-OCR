//! 剪贴板卡片的系统原生拖拽（参照 Paste）。
//!
//! 以前是自己开一个小窗口当"影子"，后台线程每 8ms 按鼠标位置挪一次，比鼠标慢 1~2 帧；松手后再模拟点一下、
//! 发 ⌘V。原生拖拽的拖动图像由系统画在光标下面，跟手；松手由目标程序自己接（文字插在松手的地方、
//! 图片 / 文件按它自己的方式收），不碰系统剪贴板。
//!
//! 流程：页面发现拖起来了 → 这里先把卡片截一张小图（WKWebView 截图，异步）→ 用截图当拖动图像、
//! 每条记录一个 NSDraggingItem，从网页视图上开一个拖拽会话。会话的移动 / 结束经 [`DragEvents`] 回调出去。

use std::cell::RefCell;
use std::path::PathBuf;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSApplication, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession,
    NSDraggingSource, NSEvent, NSEventModifierFlags, NSEventType, NSImage, NSPasteboardItem,
    NSPasteboardTypeFileURL, NSPasteboardTypeHTML, NSPasteboardTypePNG, NSPasteboardTypeRTF,
    NSPasteboardTypeString, NSPasteboardTypeURL, NSView,
};
use objc2_foundation::{
    NSArray, NSData, NSError, NSNumber, NSObject, NSObjectProtocol, NSPoint, NSProcessInfo, NSRect,
    NSSize, NSString, NSURL,
};
use tauri::WebviewWindow;

use super::util::{ns_window, on_main};
use crate::error::{AppError, AppResult};

/// 拖出去的一条记录
pub enum DragEntry {
    /// 文字类（文字、链接、颜色……）：纯文字，有的话带上 HTML / RTF（目标程序按自己认得的格式收），链接再带一份 URL
    Text {
        text: String,
        html: Option<String>,
        rtf: Option<String>,
        url: Option<String>,
    },
    /// 图片：一个文件（访达、聊天程序按文件收）+ PNG 数据（备忘录、邮件这类直接插图）
    Image { file: PathBuf, png: Option<Vec<u8>> },
    /// 文件
    File(PathBuf),
}

/// 拖拽会话的回调
pub struct DragEvents {
    /// 会话开始了（截图都截好了，系统接管了拖动）
    pub began: Box<dyn Fn() + Send>,
    /// 鼠标移动：现在在不在开始拖的那个窗口里
    pub moved: Box<dyn Fn(bool) + Send>,
    /// 松手 / 取消：有没有程序收下（Esc、松在不收的地方 = false）、松手时在不在那个窗口里、
    /// 松手的位置（那个窗口里的点，左上角原点，和页面的 CSS 像素一致）
    pub ended: Box<dyn Fn(bool, bool, (f64, f64)) + Send>,
}

/// 拖动图像相对卡片的缩放（Paste 的拖动图像大约是卡片的一半大）
const IMAGE_SCALE: f64 = 0.55;

struct SourceIvars {
    events: DragEvents,
    /// 开始拖的那个窗口的位置（屏幕坐标，左下角原点）
    frame: NSRect,
    /// 开始拖的网页视图：结束时补它一个松开
    view: Retained<NSView>,
}

define_class!(
    /// 拖拽源：告诉系统允许"拷贝"，把移动、结束转给 [`DragEvents`]
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DatoClipDragSource"]
    #[ivars = SourceIvars]
    struct DragSource;

    unsafe impl NSObjectProtocol for DragSource {}

    unsafe impl NSDraggingSource for DragSource {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation_mask(&self, _session: &NSDraggingSession, _context: NSDraggingContext) -> NSDragOperation {
            // 只拷贝：卡片拖出去，历史里那条还在。拖回面板里（分组标签）由页面按松手的位置自己处理
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingSession:movedToPoint:))]
        fn moved(&self, _session: &NSDraggingSession, point: NSPoint) {
            (self.ivars().events.moved)(self.inside(point));
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn ended(&self, _session: &NSDraggingSession, point: NSPoint, operation: NSDragOperation) {
            self.release_mouse(point);
            (self.ivars().events.ended)(operation != NSDragOperation::None, self.inside(point), self.local(point));
            // 会话结束：放掉自己（下一轮主循环再放，别在自己的方法里把自己释放掉）
            super::util::on_main_async(|_| {
                ACTIVE.with(|a| a.borrow_mut().take());
            });
        }
    }
);

thread_local! {
    /// 正在进行的拖拽的源（会话不一定持有它，自己留着，结束时放掉）
    static ACTIVE: RefCell<Option<Retained<DragSource>>> = const { RefCell::new(None) };
}

impl DragSource {
    fn new(mtm: MainThreadMarker, events: DragEvents, frame: NSRect, view: Retained<NSView>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SourceIvars { events, frame, view });
        // SAFETY: NSObject 的 init
        unsafe { msg_send![super(this), init] }
    }

    /// 拖拽会话吃掉了这次的鼠标松开，网页一直以为鼠标还按着，下一次按下就不发 pointerdown（拖一次好一次坏）。
    /// 结束时补网页视图一个左键松开（位置放在窗口外面，不点到页面上的东西）
    fn release_mouse(&self, screen: NSPoint) {
        let view = &self.ivars().view;
        let Some(window) = view.window() else { return };
        let f = self.ivars().frame;
        let at = NSPoint::new(screen.x - f.origin.x, -10_000.0);
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::LeftMouseUp,
            at,
            NSEventModifierFlags::empty(),
            NSProcessInfo::processInfo().systemUptime(),
            window.windowNumber(),
            None,
            0,
            1,
            0.0,
        );
        if let Some(event) = event {
            view.mouseUp(&event);
        }
    }

    fn local(&self, p: NSPoint) -> (f64, f64) {
        let f = self.ivars().frame;
        (p.x - f.origin.x, f.origin.y + f.size.height - p.y)
    }

    fn inside(&self, p: NSPoint) -> bool {
        let f = self.ivars().frame;
        p.x >= f.origin.x && p.x < f.origin.x + f.size.width && p.y >= f.origin.y && p.y < f.origin.y + f.size.height
    }
}

/// 窗口里的网页视图（WKWebView）：内容视图下面能截图的那个
fn find_webview(view: &NSView) -> Option<Retained<NSView>> {
    let snap = sel!(takeSnapshotWithConfiguration:completionHandler:);
    for sub in view.subviews().iter() {
        if sub.respondsToSelector(snap) {
            return Some(sub);
        }
        if let Some(found) = find_webview(&sub) {
            return Some(found);
        }
    }
    None
}

fn pasteboard_item(entry: &DragEntry) -> Retained<NSPasteboardItem> {
    let item = NSPasteboardItem::new();
    let set = |value: &str, kind: &NSString| {
        item.setString_forType(&NSString::from_str(value), kind);
    };
    let file_url = |path: &PathBuf| {
        NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
            .absoluteString()
            .map(|s| s.to_string())
    };
    // SAFETY: 这几个是 AppKit 导出的常量
    unsafe {
        match entry {
            DragEntry::Text { text, html, rtf, url } => {
                set(text, NSPasteboardTypeString);
                if let Some(html) = html {
                    set(html, NSPasteboardTypeHTML);
                }
                if let Some(rtf) = rtf {
                    item.setData_forType(&NSData::with_bytes(rtf.as_bytes()), NSPasteboardTypeRTF);
                }
                if let Some(url) = url {
                    set(url, NSPasteboardTypeURL);
                }
            }
            DragEntry::Image { file, png } => {
                if let Some(u) = file_url(file) {
                    set(&u, NSPasteboardTypeFileURL);
                }
                if let Some(png) = png {
                    item.setData_forType(&NSData::with_bytes(png), NSPasteboardTypePNG);
                }
            }
            DragEntry::File(path) => {
                if let Some(u) = file_url(path) {
                    set(&u, NSPasteboardTypeFileURL);
                }
            }
        }
    }
    item
}

/// 拖着的一张卡片：它带出去的记录（"文件"卡片可能是好几个文件）和它在页面里的位置（CSS 像素 = 点；
/// 滚出视野、没画出来的是 None，拖动图像里就没有它，系统在角上的数字里还算它）
pub struct DragCard {
    pub entries: Vec<DragEntry>,
    pub rect: Option<(f64, f64, f64, f64)>,
}

/// 拖动图像里最多画几张卡片（叠成一摞，再多也看不见）
const MAX_IMAGES: usize = 5;

/// 开始拖：`cards` 按粘贴的顺序，`grabbed` 是手上那张（在 `cards` 里的下标），`grab` 是鼠标按在它上面的点。
/// 先把卡片截图（并行），都截到了再开会话；鼠标这时已经松开了就不拖了（返回前不等截图）。
pub fn start(window: &WebviewWindow, cards: Vec<DragCard>, grabbed: usize, grab: (f64, f64), events: DragEvents) -> AppResult<()> {
    if cards.iter().all(|c| c.entries.is_empty()) {
        return Err(AppError::msg("没有要拖的记录"));
    }
    let grabbed = grabbed.min(cards.len() - 1);
    let window = window.clone();
    let payload = std::sync::Mutex::new(Some((cards, events)));
    on_main(move |mtm| -> AppResult<()> {
        let ns = ns_window(&window, mtm).ok_or_else(|| AppError::msg("窗口不在了"))?;
        let content = ns.contentView().ok_or_else(|| AppError::msg("窗口没有内容视图"))?;
        let webview = find_webview(&content).ok_or_else(|| AppError::msg("找不到网页视图"))?;
        let (cards, events) = payload.lock().ok().and_then(|mut p| p.take()).ok_or_else(|| AppError::msg("重复开始"))?;
        let cfg_class = AnyClass::get(c"WKSnapshotConfiguration").ok_or_else(|| AppError::msg("没有 WebKit 截图"))?;

        // 截哪几张：手上那张先，再按顺序补到 MAX_IMAGES 张
        let mut wanted: Vec<usize> = Vec::new();
        for i in std::iter::once(grabbed).chain(0..cards.len()) {
            if wanted.len() < MAX_IMAGES && !wanted.contains(&i) && cards[i].rect.is_some() {
                wanted.push(i);
            }
        }
        let state = std::rc::Rc::new(RefCell::new(Pending {
            images: (0..cards.len()).map(|_| None).collect(),
            left: wanted.len(),
            rest: Some((cards, events)),
        }));
        let finish = {
            let state = state.clone();
            let (view, ns_win) = (webview.clone(), ns.clone());
            move || {
                let mut st = state.borrow_mut();
                let Some((cards, events)) = st.rest.take() else { return };
                let images = std::mem::take(&mut st.images);
                drop(st);
                // SAFETY: 截图回调、这里都在主线程
                let mtm = unsafe { MainThreadMarker::new_unchecked() };
                // 截图要几十毫秒，这期间鼠标已经松了：不拖了，当成取消
                if NSEvent::pressedMouseButtons() & 1 == 0 {
                    (events.ended)(false, true, (-1.0, -1.0));
                    return;
                }
                begin(mtm, &ns_win, &view, cards, images, grabbed, grab, events);
            }
        };
        let finish = std::rc::Rc::new(finish);
        if wanted.is_empty() {
            finish();
            return Ok(());
        }
        for i in wanted {
            let Some((x, y, w, h)) = state.borrow().rest.as_ref().and_then(|(c, _)| c[i].rect) else { continue };
            // WKSnapshotConfiguration 的 rect 是网页视图的坐标（和 CSS 像素一致）
            // SAFETY: WKSnapshotConfiguration 的公开接口
            let cfg: Retained<AnyObject> = unsafe { msg_send![cfg_class, new] };
            let width = NSNumber::new_f64(w * IMAGE_SCALE);
            unsafe {
                let _: () = msg_send![&*cfg, setRect: NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))];
                let _: () = msg_send![&*cfg, setAfterScreenUpdates: false];
                let _: () = msg_send![&*cfg, setSnapshotWidth: &*width];
            }
            let (state, finish) = (state.clone(), finish.clone());
            let block = RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
                // SAFETY: 截图回调给的是 +0 的 NSImage（可能为空）
                let image: Option<Retained<NSImage>> = unsafe { Retained::retain(image) };
                let done = {
                    let mut st = state.borrow_mut();
                    if let Some(slot) = st.images.get_mut(i) {
                        *slot = image;
                    }
                    st.left = st.left.saturating_sub(1);
                    st.left == 0
                };
                if done {
                    finish();
                }
            });
            // SAFETY: WKWebView 的公开接口，回调签名 (NSImage *, NSError *)
            unsafe {
                let _: () = msg_send![&*webview, takeSnapshotWithConfiguration: &*cfg, completionHandler: &*block];
            }
        }
        Ok(())
    })
}

struct Pending {
    images: Vec<Option<Retained<NSImage>>>,
    left: usize,
    rest: Option<(Vec<DragCard>, DragEvents)>,
}

#[allow(clippy::too_many_arguments)]
fn begin(
    mtm: MainThreadMarker,
    window: &objc2_app_kit::NSWindow,
    view: &NSView,
    cards: Vec<DragCard>,
    images: Vec<Option<Retained<NSImage>>>,
    grabbed: usize,
    grab: (f64, f64),
    events: DragEvents,
) {
    // 拖动图像的大小按手上那张算；按"按住的那个点还在鼠标下"摆（缩小后的同一个比例位置）
    let (w, h) = cards[grabbed].rect.map_or((200.0, 200.0), |r| (r.2, r.3));
    let size = NSSize::new(w * IMAGE_SCALE, h * IMAGE_SCALE);
    let mouse = view.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
    let fx = (grab.0 / w.max(1.0)).clamp(0.0, 1.0);
    let fy = (grab.1 / h.max(1.0)).clamp(0.0, 1.0);
    let origin_y = if view.isFlipped() {
        mouse.y - size.height * fy
    } else {
        mouse.y - size.height * (1.0 - fy)
    };
    let frame = NSRect::new(NSPoint::new(mouse.x - size.width * fx, origin_y), size);
    let multi = cards.len() > 1;

    // 手上那张排在最前：系统叠成一摞时第一条在最上面（Paste 也是手上那张在上面）。放下时也是这个顺序
    let order: Vec<usize> = std::iter::once(grabbed).chain((0..cards.len()).filter(|&i| i != grabbed)).collect();
    let mut items: Vec<Retained<NSDraggingItem>> = Vec::new();
    for &i in &order {
        for (k, entry) in cards[i].entries.iter().enumerate() {
            let pb = pasteboard_item(entry);
            let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), ProtocolObject::from_ref(&*pb));
            // 每张卡片的第一条带它自己的截图；"文件"卡片里其余几个文件、没截到图的不画（系统角标里还算数）
            let image = if k == 0 { images.get(i).and_then(|im| im.as_deref()) } else { None };
            let contents: Option<&AnyObject> = image.map(|im| im.as_ref());
            // SAFETY: contents 是 NSImage 或空
            unsafe { item.setDraggingFrame_contents(frame, contents) };
            items.push(item);
        }
    }
    let items = NSArray::from_retained_slice(&items);
    // 开会话要一个鼠标事件：当前事件是左键按下 / 拖动就用它，否则按现在的位置造一个拖动事件
    let app = NSApplication::sharedApplication(mtm);
    let event = app
        .currentEvent()
        .filter(|e| matches!(e.r#type(), NSEventType::LeftMouseDown | NSEventType::LeftMouseDragged))
        .or_else(|| {
            NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                NSEventType::LeftMouseDragged,
                window.mouseLocationOutsideOfEventStream(),
                NSEventModifierFlags::empty(),
                NSProcessInfo::processInfo().systemUptime(),
                window.windowNumber(),
                None,
                0,
                1,
                1.0,
            )
        });
    let Some(event) = event else {
        (events.ended)(false, true, (-1.0, -1.0));
        return;
    };
    let source = DragSource::new(mtm, events, window.frame(), view.retain());
    let busy = ACTIVE.with(|a| a.borrow().is_some());
    // 系统不让开（比如上一次拖拽还没结束）时返回空：生成的绑定遇到空会 panic，自己发消息拿 Option
    // SAFETY: NSView 的公开接口，参数类型和签名一致
    let session: Option<Retained<NSDraggingSession>> = unsafe {
        msg_send![view, beginDraggingSessionWithItems: &*items, event: &*event, source: ProtocolObject::<dyn NSDraggingSource>::from_ref(&*source)]
    };
    let Some(session) = session else {
        tracing::warn!(busy, "系统没让开始拖拽");
        (source.ivars().events.ended)(false, true, (-1.0, -1.0));
        return;
    };
    // 没人收（Esc、松在桌面上不收的地方）：图像飞回卡片原处，像 Paste
    session.setAnimatesToStartingPositionsOnCancelOrFail(true);
    // 多张：系统把它们歪着堆成一摞、放在鼠标右下方，角上标个数（Paste 也是这样）
    if multi {
        session.setDraggingFormation(objc2_app_kit::NSDraggingFormation::Pile);
    }
    (source.ivars().events.began)();
    ACTIVE.with(|a| *a.borrow_mut() = Some(source));
}
