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
    /// 原样的 JPEG 字节（手机照片），存原图比转 PNG 小得多
    pub image_jpeg: Option<Vec<u8>>,
    pub files: Vec<PathBuf>,
    /// 应用标记了"不要记录到剪贴板历史"（密码管理器常用）
    pub privacy_flagged: bool,
    /// 从别的苹果设备经通用剪贴板过来的（macOS）。`source` 这时是本机碰巧在前台的应用，不能当来源
    pub from_other_device: bool,
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

/// 录屏时的键盘和滚轮（见 `platform::start_record_input`）：按键显示、滚动箭头用
#[derive(Clone, Debug, PartialEq)]
pub enum RecordInput {
    /// 按下了一个键，显示用的写法（带修饰键）：⌘C、⇧A、⏎、Space……
    Key(String),
    /// 滚轮：往下滚为正（像素或行，只看方向）
    Scroll(f64),
}

/// 划词监听上报的事件（见 `platform::start_selection_watch`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionEvent {
    /// 刚选了文字：拖选或双击 / 三击。`anchor` 是按下点和松开点的包围盒，
    /// `end` 是松开点；`alt` / `ctrl` 表示选的时候按着这个键；
    /// `text` 是选中的字（系统肯告诉我们时才有，用来判断该不该弹按钮；None = 不知道）
    Selected {
        anchor: PhysicalRect,
        end: (i32, i32),
        alt: bool,
        ctrl: bool,
        text: Option<String>,
    },
    /// 点了悬浮翻译按钮（这次点击已被吞掉，原程序的选区还在）
    ButtonClicked,
    /// 在别处按下鼠标 / 滚轮：该收起按钮了
    Dismiss,
    /// 鼠标在某处按下了（任何键）。翻译面板靠它判断"点了面板外面"：面板没拿到键盘焦点时等不到失焦
    PointerDown,
}

/// 窗口在哪、它所在那块屏的可用区域（都是逻辑像素，和 `set_bounds_logical` 同一套坐标）。
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowPlacement {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub work_x: f64,
    pub work_y: f64,
    pub work_width: f64,
    pub work_height: f64,
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
    FullDiskAccess,
}

/// 截图框选用的一个界面元素（辅助功能树里的）：屏幕物理坐标的框和它的角色（AXCell、AXButton……）
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiElement {
    pub rect: PhysicalRect,
    pub role: String,
}

/// 朗读的选项（设置里的语速、音调、各语言的声音）
#[derive(Clone, Debug, Default)]
pub struct SpeakOptions {
    pub rate: f64,
    pub pitch: f64,
    /// 语言（zh、en、zh-TW……）→ 声音标识
    pub voices: std::collections::BTreeMap<String, String>,
}

impl SpeakOptions {
    /// 某个语言标签（如 zh-CN）设置里选的声音：先找完整标签（繁体中文 zh-TW 单独设），再找语言部分
    pub fn voice_for(&self, tag: &str) -> Option<&str> {
        let base = tag.split('-').next().unwrap_or(tag);
        self.voices
            .get(tag)
            .or_else(|| (tag != "zh-TW").then(|| self.voices.get(base)).flatten())
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }
}

/// 系统里的一个朗读声音
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Voice {
    pub id: String,
    pub name: String,
    /// BCP-47 语言标签，如 zh-CN
    pub lang: String,
    /// 1 普通 / 2 增强 / 3 高品质
    pub quality: u8,
}

/// 录视频时要用的设备
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind {
    Microphone,
    Camera,
}

/// 麦克风 / 摄像头的授权状态
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaAccess {
    Granted,
    Denied,
    /// 还没问过用户（第一次用时系统会弹框）
    NotDetermined,
    /// 这个平台不支持
    Unsupported,
}

/// 录视频时除了画面还录什么、画面怎么压
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoOptions {
    pub system_audio: bool,
    pub microphone: bool,
    pub encoding: VideoEncoding,
}

/// 视频画面怎么压：录制和剪辑导出用同一套（导出不用系统"最高质量"预设，那个码率很高，剪一刀文件反而变大）
#[derive(Clone, Copy, Debug)]
pub struct VideoEncoding {
    /// H.265（HEVC）：同样清晰文件小三四成；老设备、部分网页和聊天软件放不了
    pub hevc: bool,
    /// 画质 10–100：决定码率，越低文件越小
    pub quality: u32,
    pub fps: u32,
}

impl Default for VideoEncoding {
    fn default() -> Self {
        Self {
            hevc: false,
            quality: 50,
            fps: 30,
        }
    }
}

impl VideoEncoding {
    /// 平均码率（bit/s）。屏幕内容大块不动、只录变了的帧，每像素每帧给得很少也清楚：
    /// 画质 50 ≈ 0.045 bit/像素/帧（1080p30 约 2.8 Mbps），100 ≈ 0.14；帧率超过 30 的部分只按四成算；
    /// H.265 再打六五折
    pub fn bitrate(&self, width: u32, height: u32) -> u32 {
        let q = f64::from(self.quality.clamp(10, 100)) / 100.0;
        let bpp = 0.012 + 0.13 * q * q;
        let fps = f64::from(self.fps.clamp(1, 30)) + f64::from(self.fps.saturating_sub(30)) * 0.4;
        let mut bits = f64::from(width) * f64::from(height) * fps * bpp;
        if self.hevc {
            bits *= 0.65;
        }
        bits.clamp(300_000.0, 40_000_000.0) as u32
    }
}

/// 录好的视频剪一下：只要 `[start_ms, end_ms)` 这一段，按 `speed` 倍速
#[derive(Clone, Copy, Debug)]
pub struct VideoEdit {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speed: f64,
}

/// 要用户手动授予的系统权限。`None` = 这个平台没有这项权限、不用管。
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    /// 屏幕录制：没有就截不到别的程序的窗口
    pub screen_capture: Option<bool>,
    /// 辅助功能：模拟粘贴 / 复制、全局拦截按键
    pub accessibility: Option<bool>,
    /// 完全磁盘访问：读「信息」里 iPhone 转发来的短信（短信验证码）
    pub full_disk_access: Option<bool>,
}

/// 系统短信应用里收到的一条消息（macOS：iPhone 转发到「信息」的短信 / iMessage）。
#[derive(Clone, Debug)]
pub struct SmsMessage {
    pub text: String,
    /// 发件人（号码或邮箱），可能没有
    pub sender: Option<String>,
    /// 收到的时间（Unix 毫秒）
    pub at_ms: i64,
    /// 短信应用本身，用作剪贴板历史里的来源图标
    pub app_path: Option<std::path::PathBuf>,
}

/// 识别出来的一个二维码 / 条码。位置是物理像素，相对输入图像。
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannedCode {
    /// 码的种类：QR、EAN-13、Code 128…
    pub kind: String,
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
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
