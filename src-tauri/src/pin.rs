//! 贴图：把截图钉在屏幕最上层（规格 02 §6，参照 Snipaste）。
//!
//! 每张贴图一个窗口 `pin-{id}`。窗口四周留一圈透明边距画阴影，所以窗口尺寸 =
//! 图片尺寸 + 2 × 边距。图片本身放内存仓库，经 `shot:` 协议显示。
//! 贴图窗口**默认不排除抓屏**：用户可能就是想把贴图截进下一张图里。

use std::collections::HashMap;
use std::sync::Arc;

use image::RgbaImage;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};
use crate::platform::{self, FloatingKind};
use crate::state::state;
use crate::wm;

/// 阴影边距（逻辑像素）
pub const MARGIN: f64 = 12.0;
pub const LABEL_PREFIX: &str = "pin-";

pub struct PinEntry {
    pub image: Arc<RgbaImage>,
    pub scale: f64,
}

/// 贴图窗口画好第一帧时要做的事（截图遮罩等到这时才收）
pub type OnReady = Box<dyn FnOnce() + Send>;

#[derive(Default)]
pub struct PinRegistry {
    pins: Mutex<HashMap<String, PinEntry>>,
    /// 贴图里识别出来的文字（第一次要的时候才识别，见 `text`）
    texts: Mutex<HashMap<String, Arc<Vec<PinText>>>>,
    /// 最近关掉的几张贴图（新的在后面），"恢复刚关闭的贴图"用
    closed: Mutex<Vec<ClosedPin>>,
    /// 还没显示出来的贴图：页面把图画好之后才显示窗口（见 `ready`）；带着开始建的时间，记耗时用
    waiting: Mutex<HashMap<String, (std::time::Instant, Option<OnReady>)>>,
}

impl PinRegistry {
    pub fn image(&self, label: &str) -> Option<Arc<RgbaImage>> {
        self.pins.lock().get(label).map(|p| p.image.clone())
    }

    pub fn labels(&self) -> Vec<String> {
        self.pins.lock().keys().cloned().collect()
    }
}

/// 关掉的贴图：图和它原来在屏幕上的位置（图片左上角，物理像素）
struct ClosedPin {
    image: Arc<RgbaImage>,
    scale: f64,
    at: (i32, i32),
}

/// 关掉的贴图留几张（只在内存里，退出就没了）
const CLOSED_KEEP: usize = 5;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinInfo {
    pub image_id: String,
    /// 图片物理像素尺寸
    pub width: u32,
    pub height: u32,
    /// 创建时所在屏的缩放
    pub scale: f64,
    pub margin: f64,
}

/// 贴图里的一段文字：识别出来的四边形取外接框，图片物理像素坐标。
/// 页面按这些框在图上铺一层透明文字，鼠标就能直接选中、复制。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinText {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 页面最多等这么久；到点还没画好也把窗口显示出来
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

/// `at`：图片左上角的屏幕物理坐标（即原选区位置），贴图出现在原地。
///
/// 窗口建好先藏着，等页面把图画出来（`ready`）才显示：新建一个网页窗口要两三百毫秒，
/// 先显示的话是一个空窗口，图过一会儿才冒出来。`on_ready` 在窗口显示的那一刻调用 ——
/// 从截图工具条点"贴图"时，遮罩就是等到这时才收的，冻结的画面直接换成贴图，中间不闪。
pub fn create(
    app: &AppHandle,
    image: Arc<RgbaImage>,
    at: (i32, i32),
    scale: f64,
    on_ready: Option<OnReady>,
) -> AppResult<String> {
    let st = state(app);
    let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    let label = format!("{LABEL_PREFIX}{id}");
    let scale = if scale > 0.0 { scale } else { 1.0 };
    st.images.put(label.clone(), image.clone());
    st.pins.pins.lock().insert(
        label.clone(),
        PinEntry {
            image: image.clone(),
            scale,
        },
    );

    let started = std::time::Instant::now();
    st.pins
        .waiting
        .lock()
        .insert(label.clone(), (started, on_ready));
    let timer_app = app.clone();
    let timer_label = label.clone();
    std::thread::spawn(move || {
        std::thread::sleep(READY_TIMEOUT);
        show_when_ready(&timer_app, &timer_label, true);
    });

    let (w, h) = image.dimensions();
    let margin_px = (MARGIN * scale).round() as i32;
    let ui_app = app.clone();
    let ui_label = label.clone();
    app.run_on_main_thread(move || {
        let pinned = wm::builder(&ui_app, &ui_label)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .shadow(false)
            .focused(true)
            .position(
                f64::from(at.0) / scale - MARGIN,
                f64::from(at.1) / scale - MARGIN,
            )
            .inner_size(
                f64::from(w) / scale + MARGIN * 2.0,
                f64::from(h) / scale + MARGIN * 2.0,
            );
        match platform::build_floating(pinned, FloatingKind::Panel) {
            Ok(window) => {
                tracing::debug!(build_ms = started.elapsed().as_millis() as u64, "贴图窗口建好");
                let _ = platform::place_window(
                    &window,
                    at.0 - margin_px,
                    at.1 - margin_px,
                    w + 2 * margin_px as u32,
                    h + 2 * margin_px as u32,
                );
            }
            Err(err) => {
                tracing::error!("创建贴图窗口失败：{err}");
                ready(&ui_app, &ui_label);
                remove(&ui_app, &ui_label);
            }
        }
    })?;
    Ok(label)
}

pub fn info(app: &AppHandle, label: &str) -> AppResult<PinInfo> {
    let st = state(app);
    let pins = st.pins.pins.lock();
    let entry = pins
        .get(label)
        .ok_or_else(|| AppError::NotFound("贴图".into()))?;
    Ok(PinInfo {
        image_id: label.to_string(),
        width: entry.image.width(),
        height: entry.image.height(),
        scale: entry.scale,
        margin: MARGIN,
    })
}

/// 贴图页面把图画好了（或者等超时了）：显示窗口，做 `on_ready`。每张贴图只生效一次。
pub fn ready(app: &AppHandle, label: &str) {
    show_when_ready(app, label, false);
}

fn show_when_ready(app: &AppHandle, label: &str, timed_out: bool) {
    let Some((started, on_ready)) = state(app).pins.waiting.lock().remove(label) else {
        return;
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    if timed_out {
        tracing::warn!(elapsed_ms, "贴图页面迟迟没画好，先把窗口显示出来");
    } else {
        tracing::info!(elapsed_ms, "贴图可见");
    }
    let ui_app = app.clone();
    let label = label.to_string();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = ui_app.get_webview_window(&label) {
            let _ = window.show();
            let _ = platform::take_focus(&window);
        }
        if let Some(f) = on_ready {
            f();
        }
    });
}

/// 贴图里的文字。第一次要时识别（鼠标移到贴图上时页面来要），之后用缓存。
pub fn text(app: &AppHandle, label: &str) -> AppResult<Arc<Vec<PinText>>> {
    let st = state(app);
    if let Some(cached) = st.pins.texts.lock().get(label) {
        return Ok(cached.clone());
    }
    let image = st
        .pins
        .image(label)
        .ok_or_else(|| AppError::NotFound("贴图".into()))?;
    let result = crate::ocr::recognize(app, &image, None, false)?;
    let texts: Vec<PinText> = result
        .blocks
        .into_iter()
        .filter(|b| !b.text.trim().is_empty())
        .map(|b| {
            let xs = b.quad.map(|p| p[0]);
            let ys = b.quad.map(|p| p[1]);
            let x = xs.iter().copied().fold(f64::INFINITY, f64::min);
            let y = ys.iter().copied().fold(f64::INFINITY, f64::min);
            PinText {
                text: b.text,
                x,
                y,
                width: xs.iter().copied().fold(f64::NEG_INFINITY, f64::max) - x,
                height: ys.iter().copied().fold(f64::NEG_INFINITY, f64::max) - y,
            }
        })
        .collect();
    let texts = Arc::new(texts);
    // 识别的这段时间里贴图可能已经关了，关了就不留
    if st.pins.pins.lock().contains_key(label) {
        st.pins.texts.lock().insert(label.to_string(), texts.clone());
    }
    Ok(texts)
}

fn remove(app: &AppHandle, label: &str) {
    let st = state(app);
    st.pins.pins.lock().remove(label);
    st.pins.texts.lock().remove(label);
    st.images.remove(label);
}

pub fn close(app: &AppHandle, label: &str) {
    remember_closed(app, label);
    remove(app, label);
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.destroy();
    }
}

/// 关之前记下图和位置，"恢复刚关闭的贴图"时原样放回去
fn remember_closed(app: &AppHandle, label: &str) {
    let st = state(app);
    let Some((image, scale)) = st
        .pins
        .pins
        .lock()
        .get(label)
        .map(|p| (p.image.clone(), p.scale))
    else {
        return;
    };
    let margin_px = (MARGIN * scale).round() as i32;
    let at = app
        .get_webview_window(label)
        .and_then(|w| w.outer_position().ok())
        .map_or((0, 0), |p| (p.x + margin_px, p.y + margin_px));
    let mut closed = st.pins.closed.lock();
    closed.push(ClosedPin { image, scale, at });
    let extra = closed.len().saturating_sub(CLOSED_KEEP);
    closed.drain(..extra);
}

/// 恢复最近关掉的一张贴图。没有可恢复的返回 false。
pub fn restore_last(app: &AppHandle) -> AppResult<bool> {
    let Some(last) = state(app).pins.closed.lock().pop() else {
        return Ok(false);
    };
    create(app, last.image, last.at, last.scale, None)?;
    Ok(true)
}

/// 把剪贴板里的图（剪贴板历史里最新的一条）贴到鼠标所在的位置。
pub fn pin_clipboard(app: &AppHandle) -> AppResult<()> {
    let st = state(app);
    let latest = st.db.with(|c| crate::storage::clipboard::latest_kind(c))?;
    let Some((id, kind)) = latest else {
        return Err(AppError::msg("剪贴板里没有图片"));
    };
    if kind != "image" {
        return Err(AppError::msg("剪贴板里没有图片"));
    }
    pin_clip_item(app, id)
}

/// 把剪贴板历史里的一张图贴到鼠标所在的位置。
pub fn pin_clip_item(app: &AppHandle, id: i64) -> AppResult<()> {
    let crate::platform::ClipboardPayload::Image { image, .. } =
        crate::clipboard::payload_of(app, id, false)?
    else {
        return Err(AppError::msg("剪贴板里没有图片"));
    };
    let monitor = wm::monitor_under_cursor();
    let scale = monitor.as_ref().map_or(1.0, |m| m.scale_factor);
    let (cx, cy) = platform::cursor_position().unwrap_or((0, 0));
    let (w, h) = (image.width() as i32, image.height() as i32);
    // 以鼠标为中心摆，尽量别出屏幕
    let mut at = (cx - w / 2, cy - h / 2);
    if let Some(m) = monitor {
        let b = m.work_area;
        at.0 = at.0.min(b.x + b.width as i32 - w).max(b.x);
        at.1 = at.1.min(b.y + b.height as i32 - h).max(b.y);
    }
    create(app, Arc::new(image), at, scale, None)?;
    Ok(())
}

/// 旋转 / 翻转贴图。直接改存着的那张图：复制、保存、识字、选字拿到的都是转过的。
pub fn transform(app: &AppHandle, label: &str, op: &str) -> AppResult<PinInfo> {
    use image::imageops;
    let st = state(app);
    {
        let mut pins = st.pins.pins.lock();
        let entry = pins
            .get_mut(label)
            .ok_or_else(|| AppError::NotFound("贴图".into()))?;
        let next = match op {
            "rotateCw" => imageops::rotate90(&*entry.image),
            "rotateCcw" => imageops::rotate270(&*entry.image),
            "flipH" => imageops::flip_horizontal(&*entry.image),
            "flipV" => imageops::flip_vertical(&*entry.image),
            _ => return Err(AppError::msg("不认识的变换")),
        };
        entry.image = Arc::new(next);
        st.images.put(label.to_string(), entry.image.clone());
    }
    // 识别出来的字位置不对了，下次重新识别
    st.pins.texts.lock().remove(label);
    info(app, label)
}

/// 把贴图送进编辑器画标注（编辑完可以再贴、复制或保存）。
pub fn edit(app: &AppHandle, label: &str) -> AppResult<()> {
    let image = state(app)
        .pins
        .image(label)
        .ok_or_else(|| AppError::NotFound("贴图".into()))?;
    crate::editor::open(app, (*image).clone(), None)?;
    Ok(())
}

/// 所有贴图都恢复接收鼠标（从托盘菜单解除"鼠标穿透"）。
pub fn release_click_through(app: &AppHandle) {
    for (label, window) in app.webview_windows() {
        if label.starts_with(LABEL_PREFIX) {
            let _ = window.set_ignore_cursor_events(false);
        }
    }
}

pub fn close_all(app: &AppHandle) {
    for label in state(app).pins.labels() {
        close(app, &label);
    }
    // 颜色贴图不在登记表里（没有图），直接关窗口
    for (label, window) in app.webview_windows() {
        if label.starts_with(COLOR_PREFIX) {
            let _ = window.close();
        }
    }
}

/// 颜色贴图的窗口标签前缀；后面跟 6 位十六进制颜色和一个随机后缀，页面从标签里读颜色
pub const COLOR_PREFIX: &str = "pin-color-";
/// 颜色贴图收起时的大小（逻辑像素，含阴影的留边）
const COLOR_SIZE: (f64, f64) = (200.0, 60.0);

/// 颜色贴图（截图时取色，按 P）：屏幕上一张小卡片，色块 + HEX，点开看 RGB / HSL、复制。
/// `at`：卡片左上角的屏幕物理坐标（鼠标位置）。
pub fn create_color(app: &AppHandle, rgb: [u8; 3], at: (i32, i32), scale: f64) -> AppResult<String> {
    let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    let label = format!(
        "{COLOR_PREFIX}{:02x}{:02x}{:02x}-{id}",
        rgb[0], rgb[1], rgb[2]
    );
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let ui_app = app.clone();
    let ui_label = label.clone();
    app.run_on_main_thread(move || {
        let builder = wm::builder(&ui_app, &ui_label)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .shadow(false)
            .focused(false)
            .position(f64::from(at.0) / scale, f64::from(at.1) / scale)
            .inner_size(COLOR_SIZE.0, COLOR_SIZE.1);
        match platform::build_floating(builder, FloatingKind::Panel) {
            Ok(window) => {
                let _ = platform::place_window(
                    &window,
                    at.0,
                    at.1,
                    (COLOR_SIZE.0 * scale).round() as u32,
                    (COLOR_SIZE.1 * scale).round() as u32,
                );
                // 卡片背景是透明的，页面还没画出来时显示也只是一块透明，不用等
                if let Err(err) = platform::show_without_activate(&window) {
                    tracing::warn!("显示颜色贴图失败：{err}");
                }
            }
            Err(err) => tracing::error!("创建颜色贴图失败：{err}"),
        }
    })?;
    Ok(label)
}
