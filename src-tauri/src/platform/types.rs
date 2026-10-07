//! 平台层共享的数据类型。这里不得出现任何平台句柄类型（HWND 等一律包成 u64）。

use std::path::PathBuf;

use image::RgbaImage;
use serde::{Deserialize, Serialize};

/// 不透明窗口句柄。Windows 上是 HWND，macOS 上是 进程号 << 32 | CGWindowID。
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WindowHandle(pub u64);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MonitorId(pub u64);

impl std::fmt::Display for MonitorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 物理像素矩形，原点是虚拟桌面左上角（主屏左上角）。副屏在主屏左/上方时坐标为负。
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PhysicalRect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn area(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn intersect(&self, other: &PhysicalRect) -> Option<PhysicalRect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        (r > x && b > y).then(|| PhysicalRect::new(x, y, (r - x) as u32, (b - y) as u32))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    pub id: MonitorId,
    pub name: String,
    pub bounds: PhysicalRect,
    /// 排除任务栏后的区域
    pub work_area: PhysicalRect,
    pub scale_factor: f64,
    pub is_primary: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    pub handle: WindowHandle,
    /// 用 DWM 扩展边框取的真实可见矩形，不含 Win10+ 那圈不可见阴影
    pub bounds: PhysicalRect,
    /// 0 = 最上层
    pub z_order: u32,
    pub title: String,
    pub app_name: String,
    pub process_id: u32,
    /// 子控件矩形（截图时随窗口一起快照，见 `capture::start`）
    #[serde(default)]
    pub children: Vec<PhysicalRect>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// 人类可读的应用名（文件描述，取不到时为可执行文件名）
    pub name: String,
    pub exe_path: Option<PathBuf>,
}

/// 剪贴板一次变化时读到的全部格式。
#[derive(Clone, Debug, Default)]
pub struct ClipboardSnapshot {
    pub text: Option<String>,
    pub html: Option<String>,
    pub rtf: Option<String>,
    pub image: Option<RgbaImage>,
    /// 剪贴板里原样带着的 PNG 字节（如有），保留透明度用
    pub image_png: Option<Vec<u8>>,
    pub files: Vec<PathBuf>,
    /// 应用标记了"不要记录到剪贴板历史"（密码管理器常用）
    pub privacy_flagged: bool,
    pub source: Option<AppInfo>,
    /// 系统剪贴板序列号。自己写入后记下序列号，监听到同一号就跳过（忽略下一次变化）。
    pub sequence: u32,
}

impl ClipboardSnapshot {
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.image.is_none() && self.files.is_empty()
    }
}

pub enum ClipboardPayload {
    Text {
        text: String,
        html: Option<String>,
        rtf: Option<String>,
    },
    Image {
        image: RgbaImage,
        /// 有透明通道的图额外写一份 PNG 格式
        png: Option<Vec<u8>>,
    },
    Files(Vec<PathBuf>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemVisuals {
    pub dark_mode: bool,
    pub transparency_enabled: bool,
    pub power_saver: bool,
    pub reduced_motion: bool,
    /// 系统窗口材质可用（Windows 上是 Mica，Win11 22H2+；macOS 上一直有）
    pub mica_supported: bool,
}

/// 长截图期间全局输入钩子上报的事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    Wheel { delta: i32, x: i32, y: i32 },
    MouseMove { x: i32, y: i32 },
    Key(HookKey),
}

/// 划词监听上报的事件（见 `platform::start_selection_watch`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionEvent {
    /// 刚选了文字：拖选或双击 / 三击。`anchor` 是按下点和松开点的包围盒，
    /// `end` 是松开点；`alt` / `ctrl` 表示选的时候按着这个键
    Selected {
        anchor: PhysicalRect,
        end: (i32, i32),
        alt: bool,
        ctrl: bool,
    },
    /// 点了悬浮翻译按钮（这次点击已被吞掉，原程序的选区还在）
    ButtonClicked,
    /// 在别处按下鼠标 / 滚轮：该收起按钮了
    Dismiss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookKey {
    Enter,
    Escape,
    Backspace,
}

/// 浮层的种类（见 `platform::build_floating`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatingKind {
    /// 截图遮罩：盖住整块屏幕，包括菜单栏和程序坞
    Overlay,
    /// 面板、气泡、贴图、提示
    Panel,
}

/// 各功能的默认热键（global-hotkey 的写法）。每个平台一套，见 `platform::default_hotkeys`。
pub struct DefaultHotkeys {
    pub capture: &'static str,
    pub longshot: &'static str,
    pub ocr: &'static str,
    pub clipboard: &'static str,
    pub translate: &'static str,
    pub instant: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Permission {
    ScreenCapture,
    Accessibility,
}

/// 要用户手动授予的系统权限。`None` = 这个平台没有这项权限、不用管。
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    /// 屏幕录制：没有就截不到别的程序的窗口
    pub screen_capture: Option<bool>,
    /// 辅助功能：模拟粘贴 / 复制、全局拦截按键
    pub accessibility: Option<bool>,
}

/// 系统自带 OCR 的一行结果（物理像素，相对输入图像）。
#[derive(Clone, Debug)]
pub struct SysOcrLine {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[cfg(test)]
mod tests {
    use super::PhysicalRect;

    #[test]
    fn rect_intersection_handles_negative_coords() {
        let a = PhysicalRect::new(-1920, 0, 1920, 1080);
        let b = PhysicalRect::new(-100, 100, 300, 300);
        assert_eq!(
            a.intersect(&b),
            Some(PhysicalRect::new(-100, 100, 100, 300))
        );
        assert_eq!(a.intersect(&PhysicalRect::new(0, 0, 10, 10)), None);
        assert!(a.contains_point(-1, 1079));
        assert!(!a.contains_point(0, 0));
    }
}
