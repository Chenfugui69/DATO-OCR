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

#[derive(Default)]
pub struct PinRegistry {
    pins: Mutex<HashMap<String, PinEntry>>,
}

impl PinRegistry {
    pub fn image(&self, label: &str) -> Option<Arc<RgbaImage>> {
        self.pins.lock().get(label).map(|p| p.image.clone())
    }

    pub fn labels(&self) -> Vec<String> {
        self.pins.lock().keys().cloned().collect()
    }
}

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

/// `at`：图片左上角的屏幕物理坐标（即原选区位置），贴图出现在原地。
pub fn create(
    app: &AppHandle,
    image: Arc<RgbaImage>,
    at: (i32, i32),
    scale: f64,
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
                let _ = platform::place_window(
                    &window,
                    at.0 - margin_px,
                    at.1 - margin_px,
                    w + 2 * margin_px as u32,
                    h + 2 * margin_px as u32,
                );
                let _ = window.show();
                let _ = platform::take_focus(&window);
            }
            Err(err) => {
                tracing::error!("创建贴图窗口失败：{err}");
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

fn remove(app: &AppHandle, label: &str) {
    let st = state(app);
    st.pins.pins.lock().remove(label);
    st.images.remove(label);
}

pub fn close(app: &AppHandle, label: &str) {
    remove(app, label);
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.destroy();
    }
}

pub fn close_all(app: &AppHandle) {
    for label in state(app).pins.labels() {
        close(app, &label);
    }
}
