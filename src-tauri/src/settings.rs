//! 配置文件 `settings.json`（规格 07 §8）。
//!
//! - 人类可读，用户可手改；**不含任何密钥**（密钥在数据库 `secrets` 表，DPAPI 加密）
//! - 文件不存在或解析失败 → 用默认值并重写，不崩溃
//! - 缺字段用默认值补；未知字段原样保留（`extra`），向前兼容
//! - 写入走"临时文件 → fsync → 原子重命名"，防止断电写坏

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::AppResult;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub version: u32,
    pub general: GeneralSettings,
    pub appearance: AppearanceSettings,
    pub hotkeys: HotkeySettings,
    pub capture: CaptureSettings,
    pub longshot: LongshotSettings,
    pub ocr: OcrSettings,
    pub translate: TranslateSettings,
    pub clipboard: ClipboardSettings,
    pub sync: SyncSettings,
    pub update: UpdateSettings,
    pub network: NetworkSettings,
    pub ai: AiSettings,
    pub mcp: McpSettings,
    pub record: RecordSettings,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 录制视频（MP4）和 GIF。GIF 的帧率、录不录鼠标还在 `capture` 里（老设置），设置页一起放在"录制"下面。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RecordSettings {
    /// 视频分辨率上限：original = 按屏幕的物理像素（最长边最多 3840）| 1080p | 720p（装进 1920×1080 / 1280×720）
    pub resolution: String,
    /// 视频画质 10–100：决定码率，越低文件越小（见 `platform::VideoEncoding::bitrate`）
    pub quality: u32,
    /// 视频编码器：h264（哪里都能放）| hevc（H.265，文件小三四成，老设备放不了）
    pub codec: String,
    /// 系统声音和麦克风各存一条音轨（后期好分开调）。关着时录完混成一条（大多数播放器只放第一条）
    pub separate_audio_tracks: bool,
    /// GIF 分辨率上限：original（最长边最多 1600）| 1080p | 720p | 480p
    pub gif_resolution: String,
    /// GIF 循环播放（关掉 = 放一遍停在最后一帧）
    pub gif_loop: bool,
    /// 视频帧率：15 / 24 / 30 / 60
    pub fps: u32,
    /// 录系统声音（电脑里正在放的声音）
    pub system_audio: bool,
    /// 录麦克风
    pub microphone: bool,
    /// 摄像头画面叠在角落里（圆形）
    pub camera: bool,
    /// 摄像头画面放在哪个角：top-left | top-right | bottom-left | bottom-right
    pub camera_position: String,
    /// 摄像头画面的直径占录制区域短边的比例：small | medium | large
    pub camera_size: String,
    /// 视频最长录多少秒
    pub max_seconds: u32,
    /// 录完先打开剪辑窗口（掐头去尾、改速度），再保存。GIF 和视频都管
    pub edit_before_save: bool,
    /// 录成什么：mp4 | gif | apng（截图工具条点「录制」后，录制栏最左边选）
    pub format: String,
    /// 鼠标轨迹的颜色 #rrggbb；空 = 不画轨迹
    pub trail_color: String,
    /// 点击时那一圈涟漪的颜色 #rrggbb；空 = 不画
    pub click_color: String,
    /// 指针底下垫一个高亮圆
    pub mouse_highlight: bool,
    /// 按着鼠标键时在指针旁标出是哪个键
    pub mouse_buttons: bool,
    /// 滚动时在指针旁画上下箭头
    pub mouse_scroll: bool,
    /// 录制时把按下的键显示在画面下面的角落
    pub keystrokes: bool,
    /// 动图（GIF / APNG）底下画进度条或播放时间：off | bar | time
    pub progress: String,
    /// 点开始后倒数几秒再录（0 = 马上录）
    pub countdown: u32,
    /// 摄像头画面左上角在选区里的位置（占选区宽高的比例）；None = 选区左下角
    pub camera_at: Option<[f64; 2]>,
    /// 摄像头画面的形状：circle | square | rounded
    pub camera_shape: String,
    /// 摄像头画面的大小（逻辑像素，正方形的边长 / 圆的直径）
    pub camera_px: u32,
    /// 效果的细节（鼠标轨迹多长、按键显示多大、高亮多大……）
    pub effects: RecordEffects,
    /// 录制时工具条自动藏起来，鼠标移到它那儿再出来
    pub auto_hide_toolbar: bool,
    /// 导出完成后提示一下
    pub notify_done: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 录制效果的细节（截图工具条「录制」→ 录制栏的「效果设置」）
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RecordEffects {
    /// 鼠标轨迹留多久（毫秒）
    pub trail_ms: u32,
    /// 按键显示的高度（逻辑像素）
    pub keyboard_px: u32,
    /// 按键显示的底色 / 字色（#rrggbb 或 #rrggbbaa）
    pub keyboard_bg: String,
    pub keyboard_fg: String,
    /// 指针高亮圆的半径（逻辑像素）和颜色（#rrggbbaa）
    pub highlight_radius: u32,
    pub highlight_color: String,
    /// 进度条 / 播放时间的颜色
    pub progress_color: String,
}

impl Default for RecordEffects {
    fn default() -> Self {
        Self {
            trail_ms: 500,
            keyboard_px: 44,
            keyboard_bg: "#000000cc".into(),
            keyboard_fg: "#ffffff".into(),
            highlight_radius: 40,
            highlight_color: "#ffff0080".into(),
            progress_color: "#1677ff".into(),
        }
    }
}

/// `#rrggbb` / `#rrggbbaa` → RGBA；写错了用 `fallback`
pub fn parse_color(s: &str, fallback: [u8; 4]) -> [u8; 4] {
    let h = s.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => match (byte(0), byte(2), byte(4)) {
            (Some(r), Some(g), Some(b)) => [r, g, b, 255],
            _ => fallback,
        },
        8 => match (byte(0), byte(2), byte(4), byte(6)) {
            (Some(r), Some(g), Some(b), Some(a)) => [r, g, b, a],
            _ => fallback,
        },
        _ => fallback,
    }
}

impl Default for RecordSettings {
    fn default() -> Self {
        Self {
            resolution: "1080p".into(),
            quality: 50,
            codec: "h264".into(),
            separate_audio_tracks: false,
            gif_resolution: "720p".into(),
            gif_loop: true,
            fps: 30,
            system_audio: false,
            microphone: false,
            camera: false,
            camera_position: "bottom-right".into(),
            camera_size: "medium".into(),
            max_seconds: 300,
            edit_before_save: false,
            format: "mp4".into(),
            trail_color: String::new(),
            click_color: "#ff3b30".into(),
            mouse_highlight: false,
            mouse_buttons: false,
            mouse_scroll: false,
            keystrokes: false,
            progress: "off".into(),
            countdown: 0,
            camera_at: None,
            camera_shape: "circle".into(),
            camera_px: 150,
            effects: RecordEffects::default(),
            auto_hide_toolbar: false,
            notify_done: true,
            extra: Map::new(),
        }
    }
}

impl RecordSettings {
    fn sanitize(&mut self) {
        if !matches!(self.resolution.as_str(), "original" | "1080p" | "720p") {
            self.resolution = "1080p".into();
        }
        self.quality = self.quality.clamp(10, 100);
        if !matches!(self.codec.as_str(), "h264" | "hevc") {
            self.codec = "h264".into();
        }
        if !matches!(
            self.gif_resolution.as_str(),
            "original" | "1080p" | "720p" | "480p"
        ) {
            self.gif_resolution = "720p".into();
        }
        if !matches!(self.fps, 15 | 24 | 30 | 60) {
            self.fps = 30;
        }
        if !matches!(
            self.camera_position.as_str(),
            "top-left" | "top-right" | "bottom-left" | "bottom-right"
        ) {
            self.camera_position = "bottom-right".into();
        }
        if !matches!(self.camera_size.as_str(), "small" | "medium" | "large") {
            self.camera_size = "medium".into();
        }
        self.max_seconds = self.max_seconds.clamp(10, 3600);
        if !matches!(self.format.as_str(), "mp4" | "gif" | "apng") {
            self.format = "mp4".into();
        }
        if !matches!(self.progress.as_str(), "off" | "bar" | "time") {
            self.progress = "off".into();
        }
        if !matches!(self.camera_shape.as_str(), "circle" | "square" | "rounded") {
            self.camera_shape = "circle".into();
        }
        self.countdown = self.countdown.min(10);
        self.camera_px = self.camera_px.clamp(60, 480);
        self.camera_at = self
            .camera_at
            .map(|[x, y]| [x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)]);
        let e = &mut self.effects;
        e.trail_ms = e.trail_ms.clamp(100, 3000);
        e.keyboard_px = e.keyboard_px.clamp(20, 120);
        e.highlight_radius = e.highlight_radius.clamp(10, 120);
    }
}

/// 给 AI 工具用的 MCP 服务（Claude、Cursor 之类接上后能搜剪贴板、整理分组、查识字记录）。只在本机。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct McpSettings {
    pub enabled: bool,
    pub port: u16,
    /// 允许 AI 往剪贴板里加内容、建分组、把内容放进分组（关掉就只能查）
    pub allow_write: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 47390,
            allow_write: true,
            extra: Map::new(),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            general: Default::default(),
            appearance: Default::default(),
            hotkeys: Default::default(),
            capture: Default::default(),
            longshot: Default::default(),
            ocr: Default::default(),
            translate: Default::default(),
            clipboard: Default::default(),
            sync: Default::default(),
            update: Default::default(),
            network: Default::default(),
            ai: Default::default(),
            mcp: Default::default(),
            record: Default::default(),
            extra: Map::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct NetworkSettings {
    /// system = 跟随系统代理（含 HTTP_PROXY 等环境变量）| none = 直连 | custom = 用下面的地址
    pub proxy_mode: String,
    /// http://127.0.0.1:7890 或 socks5://127.0.0.1:7891；不写协议按 http
    pub proxy_url: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            proxy_mode: "system".into(),
            proxy_url: String::new(),
            extra: Map::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct GeneralSettings {
    pub language: String,
    pub auto_start: bool,
    pub close_to_tray: bool,
    /// 完全离线：禁用翻译等所有联网功能
    pub offline_mode: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            language: "system".into(),
            auto_start: false,
            close_to_tray: true,
            offline_mode: false,
            extra: Map::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AppearanceSettings {
    /// system | light | dark
    pub theme: String,
    /// auto | on | off
    pub glass_effect: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            glass_effect: "auto".into(),
            extra: Map::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct HotkeySettings {
    pub capture: String,
    pub longshot: String,
    pub ocr: String,
    pub clipboard: String,
    pub translate: String,
    /// 瞬间截屏：按下即抓整个桌面（带鼠标指针）
    pub instant: String,
    /// 打开 AI 对话窗口（默认不设）
    pub ai: String,
    /// 把剪贴板里的图贴到屏幕上（默认不设）
    pub pin_clipboard: String,
    /// 输入翻译：弹出翻译面板，自己打字翻（默认不设）
    pub input_translate: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        // 每个平台一套（Windows：F1 / F2 / F3；macOS：⌥1 / ⌥2 / ⌥3）
        let keys = crate::platform::default_hotkeys();
        Self {
            capture: keys.capture.into(),
            longshot: keys.longshot.into(),
            ocr: keys.ocr.into(),
            clipboard: keys.clipboard.into(),
            translate: keys.translate.into(),
            instant: keys.instant.into(),
            ai: String::new(),
            pin_clipboard: String::new(),
            input_translate: String::new(),
            extra: Map::new(),
        }
    }
}

const DEFAULT_FILE_NAME: &str = "DATO OCR_{yyyy}{MM}{dd}_{HH}{mm}{ss}";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CaptureSettings {
    /// 普通截图的屏幕压暗（0 = 不压暗）
    pub mask_opacity: f64,
    /// 截图识字（F3）的屏幕压暗，默认不压暗：要看清字
    pub ocr_mask_opacity: f64,
    /// 截图识字：框好就直接出识字结果，不用再按 Enter
    pub ocr_instant: bool,
    /// 瞬间截屏之后：select = 照常框选 | copy = 直接复制整屏 | save = 直接保存整屏
    pub instant_action: String,
    /// 瞬间截屏把鼠标指针也画进去
    pub instant_cursor: bool,
    /// GIF 录制的帧率（默认 10：GIF 帧率高了文件涨得快，屏幕操作 10 帧足够顺）
    pub gif_fps: u32,
    /// GIF 录制把鼠标指针也录进去
    pub gif_cursor: bool,
    /// 普通截图的选区框样式
    pub frame: FrameStyle,
    /// 截图识字（F3）的选区框样式，默认白色圆角
    pub ocr_frame: FrameStyle,
    pub show_magnifier: bool,
    /// hex | rgb | hsl
    pub color_format: String,
    pub detect_windows: bool,
    pub detect_child_windows: bool,
    /// 窗口里的元素框到多细：block = 只框大块（表格、面板、工具栏）| element = 连单元格、按钮都框
    pub element_level: String,
    /// 截图时自动找出画面里的二维码 / 条码，码中间放个小图标，鼠标移上去看内容
    pub auto_scan_codes: bool,
    /// 识别出的表格 / 公式盖在原处之后，原图怎么看：side = 直接摆在选区左边（放不下就右边）| toggle = 上方小条上点按钮切换
    pub inplace_original: String,
    pub snap_threshold: u32,
    /// none（默认，右键什么都不做）| exit | cancelSelection
    pub right_click: String,
    /// 截图工具条三组按钮各自从左到右的顺序（设置里拖动图标调整）：绘制工具
    pub toolbar_tools: Vec<String>,
    /// 识字、翻译、问 AI、长截图、GIF 那一组
    pub toolbar_actions: Vec<String>,
    /// 最右边那一组
    pub toolbar_order: Vec<String>,
    /// 工具条上隐藏的按钮（设置里点图标切换；完成、取消不能藏）
    pub toolbar_hidden: Vec<String>,
    /// 按钮组：几个按钮合成工具条上的一个，点一下弹出二级工具条。组在上面的顺序里写作 `group:组名`，
    /// 组里的按钮不再单独出现在顺序里。默认有一个「识别」组：二维码、表格、公式
    pub toolbar_groups: Vec<ToolbarGroup>,
    /// 截图工具条的样子：glass = 半透明的玻璃条（默认）| frosted = 磨砂玻璃（像剪贴板面板，模糊底下的画面）
    /// | flat = 扁平（不透明、不模糊，低配机器用）
    pub toolbar_style: String,
    /// copy | copyAndSave
    pub finish_action: String,
    pub save_to_library: bool,
    /// None = 图片\DATO OCR
    pub save_directory: Option<String>,
    pub file_name_template: String,
    /// png | jpg
    pub image_format: String,
    pub jpg_quality: u8,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            mask_opacity: 0.4,
            ocr_mask_opacity: 0.2,
            ocr_instant: true,
            instant_action: "select".into(),
            instant_cursor: true,
            gif_fps: 10,
            gif_cursor: true,
            frame: FrameStyle::default(),
            ocr_frame: FrameStyle {
                color: "#FFFFFF".into(),
                width: 3.0,
                style: "corners".into(),
                radius: 24,
                ..FrameStyle::default()
            },
            show_magnifier: true,
            color_format: "hex".into(),
            detect_windows: true,
            detect_child_windows: true,
            element_level: "element".into(),
            auto_scan_codes: true,
            inplace_original: "side".into(),
            snap_threshold: 8,
            right_click: "none".into(),
            toolbar_tools: owned(&TOOLBAR_TOOLS),
            toolbar_actions: owned(&DEFAULT_ACTIONS),
            toolbar_groups: default_groups(),
            toolbar_order: owned(&TOOLBAR_ORDER),
            toolbar_hidden: Vec::new(),
            toolbar_style: "flat".into(),
            finish_action: "copy".into(),
            save_to_library: true,
            save_directory: None,
            file_name_template: DEFAULT_FILE_NAME.into(),
            image_format: "png".into(),
            jpg_quality: 92,
            extra: Map::new(),
        }
    }
}

/// 截图工具条三组按钮的默认顺序：绘制工具、识字翻译那一组、最右边那一组。
pub const TOOLBAR_TOOLS: [&str; 12] = [
    "rect",
    "ellipse",
    "arrow",
    "pen",
    "highlight",
    "counter",
    "mosaic",
    "text",
    "measure",
    "watermark",
    "template",
    "eraser",
];
pub const TOOLBAR_ACTIONS: [&str; 8] = [
    "ocr",
    "translate",
    "qr",
    "table",
    "formula",
    "ai",
    "longshot",
    "record",
];
pub const TOOLBAR_ORDER: [&str; 5] = ["undo", "save", "pin", "cancel", "done"];
/// 新装时中间那一组的顺序（二维码、表格、公式在「识别」组里）
const DEFAULT_ACTIONS: [&str; 6] = [
    "ocr",
    "translate",
    "group:recognize",
    "ai",
    "longshot",
    "record",
];
/// 顺序里按钮组的写法：`group:组名`
pub const GROUP_PREFIX: &str = "group:";

/// 工具条上的一个按钮组
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolbarGroup {
    pub id: String,
    /// 用户起的名字；空 = 按组名用内置的叫法（recognize → 识别）
    pub name: String,
    pub items: Vec<String>,
}

fn default_groups() -> Vec<ToolbarGroup> {
    vec![ToolbarGroup {
        id: "recognize".into(),
        name: String::new(),
        items: owned(&["qr", "table", "formula"]),
    }]
}

/// 按钮组理顺：组名不重复、组里只放绘制工具或中间那组的按钮（同一类）、一个按钮只在一个组里，
/// 空组去掉。然后两组顺序里：组里的按钮拿掉，组按 `group:组名` 出现，缺的补上。
fn fix_toolbar(c: &mut CaptureSettings) {
    let mut taken: Vec<String> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    c.toolbar_groups.retain_mut(|g| {
        g.id = g.id.trim().to_string();
        if g.id.is_empty() || g.id.contains(':') || ids.contains(&g.id) {
            return false;
        }
        let tools = g
            .items
            .first()
            .is_some_and(|i| TOOLBAR_TOOLS.contains(&i.as_str()));
        let category: &[&str] = if tools {
            &TOOLBAR_TOOLS
        } else {
            &TOOLBAR_ACTIONS
        };
        g.items.retain(|i| {
            let ok = category.contains(&i.as_str()) && !taken.contains(i);
            if ok {
                taken.push(i.clone());
            }
            ok
        });
        if g.items.is_empty() {
            return false;
        }
        ids.push(g.id.clone());
        true
    });
    // 完整的顺序：按内置顺序走，碰到组里的按钮就换成那个组（组排在它第一个按钮原来的位置）
    let canonical = |all: &[&str]| {
        let mut out: Vec<String> = Vec::new();
        for id in all {
            let item = match c
                .toolbar_groups
                .iter()
                .find(|g| g.items.iter().any(|i| i == id))
            {
                Some(g) => format!("{GROUP_PREFIX}{}", g.id),
                None => (*id).to_string(),
            };
            if !out.contains(&item) {
                out.push(item);
            }
        }
        out
    };
    let tools = canonical(&TOOLBAR_TOOLS);
    let actions = canonical(&TOOLBAR_ACTIONS);
    let tool_refs: Vec<&str> = tools.iter().map(String::as_str).collect();
    let action_refs: Vec<&str> = actions.iter().map(String::as_str).collect();
    fix_order(&mut c.toolbar_tools, &tool_refs);
    fix_order(&mut c.toolbar_actions, &action_refs);
    fix_order(&mut c.toolbar_order, &TOOLBAR_ORDER);
    c.toolbar_hidden.retain(|id| {
        (tools.contains(id)
            || actions.contains(id)
            || taken.contains(id)
            || TOOLBAR_ORDER.contains(&id.as_str()))
            && id != "done"
            && id != "cancel"
    });
    c.toolbar_hidden.dedup();
}

/// 一次性：二维码、表格、公式合成「识别」组，放在原来二维码的位置
fn migrate_recognize_group(c: &mut CaptureSettings) {
    if c.extra
        .insert("toolbarGroupsV1".into(), Value::Bool(true))
        .is_some()
        || !c.toolbar_groups.is_empty()
    {
        return;
    }
    let items = ["qr", "table", "formula"];
    let pos = c
        .toolbar_actions
        .iter()
        .position(|i| items.contains(&i.as_str()))
        .unwrap_or(c.toolbar_actions.len());
    c.toolbar_actions.retain(|i| !items.contains(&i.as_str()));
    c.toolbar_actions.insert(
        pos.min(c.toolbar_actions.len()),
        format!("{GROUP_PREFIX}recognize"),
    );
    c.toolbar_groups = default_groups();
}

/// 老版本工具条上「录制 GIF」「录制视频」是两个按钮，现在合成一个「录制」（格式在录制栏里选）：
/// 顺序里、按钮组里第一个出现的换成「录制」，另一个去掉；两个都藏起来了才藏「录制」
fn migrate_record_action(c: &mut CaptureSettings) {
    let old = |id: &String| id == "gif" || id == "video";
    let mut placed = false;
    let mut swap = |list: &mut Vec<String>| {
        let mut out = Vec::with_capacity(list.len());
        for id in list.drain(..) {
            if old(&id) {
                if !placed {
                    placed = true;
                    out.push("record".into());
                }
            } else {
                out.push(id);
            }
        }
        *list = out;
    };
    swap(&mut c.toolbar_actions);
    swap(&mut c.toolbar_order);
    for g in &mut c.toolbar_groups {
        swap(&mut g.items);
    }
    let both_hidden = ["gif", "video"]
        .iter()
        .all(|id| c.toolbar_hidden.iter().any(|h| h == id));
    c.toolbar_hidden.retain(|h| !old(h));
    if both_hidden && !c.toolbar_hidden.iter().any(|h| h == "record") {
        c.toolbar_hidden.push("record".into());
    }
}

fn owned(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| (*s).into()).collect()
}

/// 顺序里每个按钮正好出现一次：不认识的、重复的去掉，缺的（新版本加的按钮）插到默认顺序里排在它
/// 后面、用户这边也有的那个按钮前面，后面没有就放最后。用户自己排过的顺序保留，不因为加了新按钮就整个恢复默认。
fn fix_order(order: &mut Vec<String>, all: &[&str]) {
    let mut kept: Vec<String> = Vec::with_capacity(all.len());
    for id in order.iter() {
        if all.contains(&id.as_str()) && !kept.contains(id) {
            kept.push(id.clone());
        }
    }
    for (i, id) in all.iter().enumerate().rev() {
        if kept.iter().any(|k| k == id) {
            continue;
        }
        let before = all[i + 1..]
            .iter()
            .find_map(|next| kept.iter().position(|k| k == next));
        kept.insert(before.unwrap_or(kept.len()), (*id).to_string());
    }
    *order = kept;
}

/// 截图选区框的样式。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct FrameStyle {
    /// `accent` = 跟随系统主题色，否则为 #RRGGBB
    pub color: String,
    /// 线宽（逻辑像素）
    pub width: f64,
    /// solid | dashed | dotted | corners（只画四个角）
    pub style: String,
    /// 圆角（逻辑像素）。只影响框的样子，截下来的图仍是矩形
    pub radius: u32,
    /// 四条边中间的拖柄：bar = 小圆条，dot = 小圆点（四个角一直是圆点）
    pub handles: String,
}

impl Default for FrameStyle {
    fn default() -> Self {
        Self {
            color: "accent".into(),
            width: 3.0,
            style: "solid".into(),
            radius: 0,
            handles: "bar".into(),
        }
    }
}

impl FrameStyle {
    fn sanitize(&mut self) {
        self.width = self.width.clamp(1.0, 6.0);
        self.radius = self.radius.min(32);
        if !matches!(
            self.style.as_str(),
            "solid" | "dashed" | "dotted" | "corners"
        ) {
            self.style = "solid".into();
        }
        if !matches!(self.handles.as_str(), "bar" | "dot") {
            self.handles = "bar".into();
        }
        let hex = self.color.strip_prefix('#').unwrap_or("");
        let valid_hex = hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit());
        if self.color != "accent" && !valid_hex {
            self.color = "accent".into();
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LongshotSettings {
    pub scroll_debounce_ms: u64,
    pub max_height: u32,
    pub detect_fixed_header: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for LongshotSettings {
    fn default() -> Self {
        Self {
            scroll_debounce_ms: 160,
            max_height: 32_000,
            detect_fixed_header: true,
            extra: Map::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct OcrSettings {
    /// rapid | paddle | system
    pub engine: String,
    /// 0 = 永不回收
    pub idle_timeout_minutes: u32,
    pub keep_line_breaks: bool,
    /// 识别完自动把文字复制到剪贴板
    pub auto_copy: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for OcrSettings {
    fn default() -> Self {
        Self {
            engine: "rapid".into(),
            idle_timeout_minutes: 5,
            keep_line_breaks: false,
            auto_copy: true,
            extra: Map::new(),
        }
    }
}

/// AI 对话（OpenAI 兼容接口 / Anthropic 接口）。密钥不在这里，在数据库 secrets 表（DPAPI 加密）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AiSettings {
    pub providers: Vec<AiProvider>,
    /// 默认模型，格式 `服务商id/模型名`；空 = 第一个可用的
    pub default_model: String,
    pub system_prompt: String,
    pub temperature: f64,
    /// 0 = 用服务商的默认值
    pub max_tokens: u32,
    /// 思考深度：auto = 不指定（模型自己的默认）| low | medium | high | max
    pub thinking: String,
    /// 快捷提问。`{text}` 换成上下文文字（选中的字 / 识字结果）
    pub quick_prompts: Vec<QuickPrompt>,
    pub panel: AiPanelStyle,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AiProvider {
    pub id: String,
    pub name: String,
    /// openai = OpenAI 兼容（OpenAI / DeepSeek / 通义 / Kimi / 智谱 / Ollama …）| anthropic
    pub kind: String,
    pub base_url: String,
    /// 用户从"检测模型"结果里挑出来要用的模型
    pub models: Vec<String>,
    pub enabled: bool,
}

impl Default for AiProvider {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            kind: "openai".into(),
            base_url: String::new(),
            models: Vec::new(),
            enabled: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct QuickPrompt {
    pub id: String,
    pub label: String,
    pub prompt: String,
}

/// 对话面板外观。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AiPanelStyle {
    pub font_size: u32,
    /// 独立窗口 / 划词面板展开后的大小（逻辑像素）
    pub width: u32,
    pub height: u32,
    /// bubble = 聊天气泡 | plain = 文档式（不加气泡，适合长回答）
    pub layout: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for AiPanelStyle {
    fn default() -> Self {
        Self {
            font_size: 14,
            width: 520,
            height: 620,
            layout: "bubble".into(),
            extra: Map::new(),
        }
    }
}

/// 系统提示词和快捷提问的默认内容。它们存在设置里、用户可以改；没改过的会跟着界面语言换
/// （见 `AiSettings::follow_language`）。
fn ai_defaults(english: bool) -> (String, Vec<QuickPrompt>) {
    let prompt = |id: &str, label: &str, prompt: &str| QuickPrompt {
        id: id.into(),
        label: label.into(),
        prompt: prompt.into(),
    };
    if english {
        (
            "You are the assistant inside DATO OCR. Answer concisely and accurately, in English by default; if the user's content is in another language or they ask for one, follow that.".into(),
            vec![
                prompt("explain", "Explain", "Explain the following:\n\n{text}"),
                prompt("summary", "Summarize", "Summarize the key points of the following in a few sentences:\n\n{text}"),
                prompt("translate", "Translate", "Translate the following into English (if it is already in English, translate it into Chinese). Give only the translation:\n\n{text}"),
                prompt("polish", "Polish", "Polish the following text. Keep its meaning and language, and make it read more naturally:\n\n{text}"),
                prompt("reply", "Reply for me", "Someone sent me this message. Write a suitable reply for me:\n\n{text}"),
            ],
        )
    } else {
        (
            "你是 DATO OCR 里的助手。回答简洁、准确，默认用简体中文；用户给的内容是什么语言、要求用什么语言，就照做。".into(),
            vec![
                prompt("explain", "解释", "解释下面这段内容：\n\n{text}"),
                prompt("summary", "总结", "用几句话总结下面这段内容的要点：\n\n{text}"),
                prompt("translate", "翻译", "把下面这段内容翻译成中文（如果已经是中文就翻译成英文），只给译文：\n\n{text}"),
                prompt("polish", "润色", "润色下面这段文字，保持原意和语言，让它更通顺自然：\n\n{text}"),
                prompt("reply", "帮我回复", "这是别人发给我的消息，帮我写一条得体的回复：\n\n{text}"),
            ],
        )
    }
}

/// 没选预设时新建的服务商叫什么（和前端 settings.ai.customProvider 一致）
fn custom_provider_name(english: bool) -> &'static str {
    if english {
        "Custom service"
    } else {
        "自定义服务"
    }
}

impl Default for AiSettings {
    fn default() -> Self {
        let (system_prompt, quick_prompts) = ai_defaults(crate::i18n::english());
        Self {
            providers: Vec::new(),
            default_model: String::new(),
            system_prompt,
            temperature: 0.7,
            max_tokens: 0,
            thinking: "auto".into(),
            quick_prompts,
            panel: Default::default(),
            extra: Map::new(),
        }
    }
}

impl AiSettings {
    /// 界面语言变了：还是另一种语言的默认内容的（用户没改过），换成这种语言的默认内容。
    /// 不然英文界面下系统提示词还写着"默认用简体中文"，AI 就一直回中文。用户改过的一个字都不动。
    pub fn follow_language(&mut self, english: bool) {
        let (other_system, other_prompts) = ai_defaults(!english);
        let (system, prompts) = ai_defaults(english);
        if self.system_prompt == other_system {
            self.system_prompt = system;
        }
        for (theirs, ours) in other_prompts.iter().zip(&prompts) {
            if let Some(p) = self.quick_prompts.iter_mut().find(|p| p.id == theirs.id) {
                if p.label == theirs.label {
                    p.label = ours.label.clone();
                }
                if p.prompt == theirs.prompt {
                    p.prompt = ours.prompt.clone();
                }
            }
        }
        for provider in &mut self.providers {
            if provider.name == custom_provider_name(!english) {
                provider.name = custom_provider_name(english).into();
            }
        }
    }
}

/// AI 思考深度的几档，从浅到深。
pub const THINKING_LEVELS: [&str; 5] = ["auto", "low", "medium", "high", "max"];

/// 全部翻译源 id。内置免费源在前，自填密钥的在后。
pub const TRANSLATE_PROVIDERS: [&str; 12] = [
    "bing",
    "transmart",
    "youdao",
    "google",
    "deepl",
    "openai",
    "baidu",
    "caiyun",
    "niutrans",
    "azure",
    "tencentcloud",
    "volcengine",
];

/// 用户自己添加的翻译（内置里没有的）。密钥不在这里，加密存在密钥表里。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CustomTranslator {
    /// `custom-` 开头的唯一标识，在 `providers` 的顺序表里用的也是它
    pub id: String,
    pub name: String,
    /// ai = OpenAI 兼容的模型接口 | deeplx = DeepLX 这类自建接口 | http = 自己描述的任意 HTTP 接口
    pub kind: String,
    /// ai：接口地址（到 /v1 为止）；deeplx、http：完整的请求地址
    pub url: String,
    /// ai：模型名
    pub model: String,
    /// ai：自己的翻译提示词（`{to}` `{from}` 换成语言名），空 = 用内置的
    pub prompt: String,
    /// http：请求头，一行一个 `名字: 值`
    pub headers: String,
    /// http：请求体模板（`{text}` `{from}` `{to}` `{key}`），空 = 发 GET
    pub body: String,
    /// http：译文在返回的 JSON 里的路径（`data.translations.0.text`），空 = 整个返回内容就是译文
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEntry {
    pub id: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TranslateSettings {
    /// auto = 中文 ↔ 英文自动；否则为目标语言代码
    pub target_language: String,
    /// 全部翻译源及顺序。排在最前面的可用源就是默认源，失败时依次往后降级
    pub providers: Vec<ProviderEntry>,
    /// 翻译面板同时显示所有已启用源的结果；关掉只显示默认源
    pub show_all_providers: bool,
    /// 截图原位翻译用哪个翻译源：auto = 按上面的顺序；指定的源失败时照样按顺序降级
    pub region_provider: String,
    pub custom_base_url: String,
    pub custom_model: String,
    /// 用户自己添加的翻译
    pub custom: Vec<CustomTranslator>,
    pub selection: SelectionTranslateSettings,
    /// 划词翻译面板的外观
    pub popup: PopupStyle,
    /// 朗读（翻译面板上的小喇叭）
    pub speech: SpeechSettings,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 朗读：语速、音调、每种语言用哪个声音
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SpeechSettings {
    /// 语速倍数：1 = 正常，0.5 ~ 2
    pub rate: f64,
    /// 音调倍数：1 = 正常，0.5 ~ 2
    pub pitch: f64,
    /// 语言代码（zh、en、ja……）→ 声音的标识；没设的语言用系统默认的声音
    pub voices: std::collections::BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for SpeechSettings {
    fn default() -> Self {
        Self {
            rate: 1.0,
            pitch: 1.0,
            voices: Default::default(),
            extra: Map::new(),
        }
    }
}

/// 划词翻译面板外观。尺寸是逻辑像素；用户拖拽改了大小会写回这里。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PopupStyle {
    pub width: u32,
    /// 0 = 自动（按同时显示的翻译源个数）
    pub height: u32,
    pub font_size: u32,
    pub opacity: f64,
    pub radius: u32,
    /// 面板顶部显示原文
    pub show_source: bool,
    /// 问 AI 时对话区怎么出来：drawer = 从面板底部往上弹出的抽屉 | side = 在右侧展开
    pub ai_layout: String,
    /// 抽屉的高度（逻辑像素）
    pub drawer_height: u32,
    /// 毛玻璃：窗口背后的内容模糊透出来（系统材质，圆角由系统决定）
    pub blur: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for PopupStyle {
    fn default() -> Self {
        Self {
            width: 380,
            height: 0,
            font_size: 15,
            // 毛玻璃默认开着：色调要淡，背后的模糊才透得出来
            opacity: 0.45,
            radius: 24,
            show_source: false,
            ai_layout: "drawer".into(),
            drawer_height: 380,
            blur: true,
            extra: Map::new(),
        }
    }
}

impl Default for TranslateSettings {
    fn default() -> Self {
        Self {
            target_language: "auto".into(),
            // 默认顺序：有道、必应、谷歌、腾讯；要自己填密钥的两个排在后面、默认不开
            providers: ["youdao", "bing", "google", "transmart", "deepl", "openai"]
                .iter()
                .map(|id| ProviderEntry {
                    id: (*id).into(),
                    enabled: !matches!(*id, "deepl" | "openai"),
                })
                .collect(),
            show_all_providers: true,
            region_provider: "auto".into(),
            custom_base_url: "https://api.openai.com/v1".into(),
            custom_model: "gpt-4o-mini".into(),
            custom: Vec::new(),
            selection: Default::default(),
            popup: Default::default(),
            speech: Default::default(),
            extra: Map::new(),
        }
    }
}

/// 邮箱验证码：用 IMAP 连邮箱，新邮件里认出验证码就自动复制（Windows、macOS 都一样）。
/// 密码（各家叫"授权码""应用专用密码"）不在这里，加密存在密钥表里，也不参与多端同步：每台设备各连各的。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct MailCodeSettings {
    pub enabled: bool,
    /// qq | 163 | gmail | icloud | proton | custom。预设的几家服务器地址是定死的（`otp::mail::endpoint`）
    pub provider: String,
    /// 邮箱地址，也是登录名
    pub address: String,
    /// 服务器地址，只有 custom 用
    pub host: String,
    /// 端口：custom 和 proton（Proton Mail Bridge 的本机端口可以改）用，0 = 默认
    pub port: u16,
    /// tls | starttls，只有 custom 用
    pub security: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for MailCodeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "qq".into(),
            address: String::new(),
            host: String::new(),
            port: 0,
            security: "tls".into(),
            extra: Map::new(),
        }
    }
}

/// 划词翻译（规格 04 §8.2 的扩展）。快捷键 Ctrl+Alt+T 始终可用。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SelectionTranslateSettings {
    /// 选中文字后在选区旁边显示一个小翻译按钮
    pub show_button: bool,
    /// bottomRight | topRight | bottomLeft | topLeft
    pub button_position: String,
    /// 划词按钮的样子：liquid = 液态玻璃（通透）| frosted = 磨砂玻璃（更实，密集文字上更好认）|
    /// solid = 纯色（没有玻璃效果，弹出最快、最省资源）| custom = 自定义（液态玻璃的各项自己调，见 `glass`）
    pub button_style: String,
    /// 自定义玻璃的各项（样式选"自定义"时用）
    pub glass: GlassTuning,
    /// 纯色圆点的配色（样式选"纯色"时用）
    pub solid: SolidDot,
    /// 选中的是系统语言的文字（中文系统里选中文）时不弹按钮
    pub skip_system_language: bool,
    /// 选中这些语言的文字时不弹按钮（语言代码：zh、en、ja……）
    pub skip_languages: Vec<String>,
    /// 划词按钮圆点的直径（逻辑像素）
    pub button_size: u32,
    /// none | alt | ctrl：按住它选文字，松开鼠标直接弹翻译面板
    pub modifier: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for SelectionTranslateSettings {
    fn default() -> Self {
        Self {
            show_button: true,
            button_position: "bottomRight".into(),
            button_style: "liquid".into(),
            glass: GlassTuning::default(),
            solid: SolidDot::default(),
            skip_system_language: false,
            skip_languages: Vec::new(),
            button_size: 28,
            modifier: "none".into(),
            extra: Map::new(),
        }
    }
}

/// 划词按钮的自定义玻璃：都是 0 ~ 100 的滑块，默认值就是"液态玻璃"的样子（页面 `liquidLens.ts` 换算成折射参数）
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct GlassTuning {
    /// 色差：边缘彩边有多明显（0 = 没有彩边）
    pub dispersion: u32,
    /// 模糊：透过玻璃看到的画面糊多少（0 = 清楚）
    pub blur: u32,
    /// 折射：边缘把画面压弯得多厉害
    pub refraction: u32,
    /// 放大：中间放大多少
    pub magnify: u32,
    /// 图标底下那团雾：越浓图标越清楚，玻璃越不通透
    pub veil: u32,
    /// 玻璃上那层淡色的浓淡
    pub tint: u32,
    /// 高光的强弱
    pub highlight: u32,
}

impl Default for GlassTuning {
    fn default() -> Self {
        Self {
            dispersion: 35,
            blur: 0,
            refraction: 48,
            magnify: 20,
            veil: 88,
            tint: 10,
            highlight: 70,
        }
    }
}

impl GlassTuning {
    fn sanitize(&mut self) {
        for v in [
            &mut self.dispersion,
            &mut self.blur,
            &mut self.refraction,
            &mut self.magnify,
            &mut self.veil,
            &mut self.tint,
            &mut self.highlight,
        ] {
            *v = (*v).min(100);
        }
    }
}

/// 纯色划词圆点的配色。颜色是 #RRGGBB，空 = 跟着深浅色主题（浅色白底、深色深灰底，图标是强调色）
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SolidDot {
    /// 圆点的底色
    pub background: String,
    /// 圆点里图标的颜色
    pub icon: String,
    /// 边框颜色
    pub border: String,
    /// 边框粗细（逻辑像素，0 ~ 4；0 = 没有边框）
    pub border_width: f64,
    /// 阴影浓淡（0 ~ 100；0 = 没有阴影）
    pub shadow: u32,
}

impl Default for SolidDot {
    fn default() -> Self {
        Self {
            background: String::new(),
            icon: String::new(),
            border: String::new(),
            border_width: 0.5,
            shadow: 50,
        }
    }
}

impl SolidDot {
    fn sanitize(&mut self) {
        for c in [&mut self.background, &mut self.icon, &mut self.border] {
            let ok = c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
            if !ok {
                c.clear();
            }
        }
        self.border_width = if self.border_width.is_finite() { self.border_width.clamp(0.0, 4.0) } else { 0.5 };
        self.shadow = self.shadow.min(100);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ClipboardSettings {
    pub enabled: bool,
    /// bottom | vertical
    pub panel_style: String,
    /// 面板毛玻璃
    pub panel_blur: bool,
    /// 底部样式贴边：直角、紧贴屏幕底部（不贴边是四周留空的悬浮圆角卡片）
    pub panel_docked: bool,
    /// 底部样式贴着屏幕最底下、盖在程序坞上面（macOS；关掉是停在程序坞上方）
    pub panel_over_dock: bool,
    /// 面板弹出 / 收起的动画
    pub panel_animation: bool,
    /// 面板里点一下就粘贴（默认要双击，单击只是选中）
    pub single_click_paste: bool,
    pub max_text_mb: u32,
    pub max_image_mb: u32,
    /// 尊重应用的"不要记录"标记（密码管理器）。默认关闭 = 全量记录（铁律 6）
    pub respect_privacy_flag: bool,
    pub use_blacklist: bool,
    /// 黑名单：可执行文件名（小写），如 `keepass.exe`
    pub blacklist: Vec<String>,
    /// 0 = 永久保留
    pub retention_days: u32,
    /// 0 = 不限条数
    pub retention_max_items: u32,
    /// 读「信息」里 iPhone 转发来的短信，认出验证码自动复制（macOS，要完全磁盘访问权限）
    pub sms_codes: bool,
    /// 剪贴板里的验证码打码显示（鼠标放到眼睛图标上才显示）
    pub otp_mask: bool,
    /// 验证码在剪贴板历史里留多久（秒），0 = 不删。置顶、收藏的不删
    pub otp_expire_secs: u32,
    /// 连邮箱读新邮件里的验证码（见 `otp::mail`）
    pub mail_codes: MailCodeSettings,
    /// 智能推荐交给 AI 模型挑（会把当前窗口认出的文字和最近剪贴板的摘要发给设置里的 AI 服务商）。
    /// 关掉 = 只在本机按词打分
    pub smart_ai: bool,
    /// 智能推荐用哪个模型（"服务商 id/模型"），空 = 跟「AI 对话」的默认模型。挑个快的小模型推荐会快很多
    pub smart_model: String,
    /// 智能推荐水珠的颜色：warm | ocean | pearl | mint | violet（面板光晕跟着它），默认海蓝
    pub orb_color: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 多端同步（规格 09）。账号密码、同步密码不在这里（在数据库 `secrets` 表）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncSettings {
    /// 本机在其他设备上显示的名字；空 = 计算机名
    pub device_name: String,
    /// 局域网同步：本机开一个服务，其他设备输入本机的设备码、本机同意后加入
    pub lan_enabled: bool,
    pub lan_port: u16,
    /// 允许手机浏览器 / 快捷指令连接（不用装 App）
    pub web_enabled: bool,
    /// WebDAV 网盘同步（坚果云，或者用 Alist 等挂成 WebDAV 的 115 之类）
    pub webdav_enabled: bool,
    pub webdav_url: String,
    pub webdav_user: String,
    /// 网盘上放同步数据的文件夹
    pub webdav_folder: String,
    /// 多久查一次网盘（秒）。坚果云免费账号半小时限 600 次请求，别太勤
    pub webdav_interval: u32,
    /// 图片也同步
    pub send_images: bool,
    /// 单张图片上限（MB），超过的不同步
    pub max_image_mb: u32,
    /// 收到其他设备刚复制的内容，直接放进本机剪贴板（Ctrl+V 就能粘）
    pub auto_write: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 检查更新。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateSettings {
    /// 自动检查更新（启动后一次，之后每 6 小时）
    pub auto_check: bool,
    /// cn = 国内（Gitee）| global = 国外（GitHub）；连不上会自动试另一个
    pub channel: String,
    /// 用户点了"不显示更新提示"的版本：这个版本不再提示，有更新的再提示
    pub skipped_version: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_check: true,
            channel: "cn".into(),
            skipped_version: String::new(),
            extra: Map::new(),
        }
    }
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            device_name: String::new(),
            lan_enabled: false,
            lan_port: 47380,
            web_enabled: true,
            webdav_enabled: false,
            webdav_url: "https://dav.jianguoyun.com/dav/".into(),
            webdav_user: String::new(),
            webdav_folder: "DATO-OCR".into(),
            webdav_interval: 15,
            send_images: true,
            max_image_mb: 10,
            auto_write: true,
            extra: Map::new(),
        }
    }
}

impl Default for ClipboardSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            panel_style: "bottom".into(),
            panel_blur: false,
            panel_docked: false,
            panel_over_dock: false,
            panel_animation: true,
            single_click_paste: false,
            max_text_mb: 5,
            max_image_mb: 30,
            respect_privacy_flag: false,
            use_blacklist: false,
            blacklist: Vec::new(),
            retention_days: 0,
            retention_max_items: 0,
            sms_codes: false,
            otp_mask: true,
            otp_expire_secs: 60,
            mail_codes: MailCodeSettings::default(),
            smart_ai: false,
            smart_model: String::new(),
            orb_color: "ocean".into(),
            extra: Map::new(),
        }
    }
}

impl Settings {
    /// 读配置。任何失败都退回默认值并尽量重写文件，绝不让应用起不来。
    pub fn load(path: &Path) -> Self {
        let parsed = std::fs::read_to_string(path).ok().and_then(|text| {
            match serde_json::from_str::<Settings>(&text) {
                Ok(settings) => Some(settings),
                Err(err) => {
                    tracing::warn!("settings.json 解析失败，使用默认值：{err}");
                    // 保留一份坏文件供排查
                    let _ = std::fs::copy(path, path.with_extension("json.broken"));
                    None
                }
            }
        });
        let settings = parsed.unwrap_or_default().sanitized();
        if let Err(err) = settings.save(path) {
            tracing::warn!("写入 settings.json 失败：{err}");
        }
        settings
    }

    pub fn save(&self, path: &Path) -> AppResult<()> {
        let text = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// 把越界值拉回合法范围（用户手改配置时可能写出任何东西）。
    pub fn sanitized(mut self) -> Self {
        self.record.sanitize();
        let sp = &mut self.translate.speech;
        sp.rate = if sp.rate.is_finite() {
            sp.rate.clamp(0.5, 2.0)
        } else {
            1.0
        };
        sp.pitch = if sp.pitch.is_finite() {
            sp.pitch.clamp(0.5, 2.0)
        } else {
            1.0
        };
        let c = &mut self.capture;
        // 品牌改名：还是旧默认值的跟着换成新名字，用户自己改过的不动
        if matches!(
            c.file_name_template.as_str(),
            "CHENOCR_{yyyy}{MM}{dd}_{HH}{mm}{ss}" | "DATO COR_{yyyy}{MM}{dd}_{HH}{mm}{ss}"
        ) {
            c.file_name_template = DEFAULT_FILE_NAME.into();
        }
        c.mask_opacity = c.mask_opacity.clamp(0.0, 0.9);
        c.gif_fps = c.gif_fps.clamp(5, 30);
        c.ocr_mask_opacity = c.ocr_mask_opacity.clamp(0.0, 0.9);
        if !matches!(c.instant_action.as_str(), "select" | "copy" | "save") {
            c.instant_action = "select".into();
        }
        if !matches!(c.inplace_original.as_str(), "side" | "toggle") {
            c.inplace_original = "side".into();
        }
        if !matches!(c.element_level.as_str(), "block" | "element") {
            c.element_level = "element".into();
        }
        if !matches!(c.right_click.as_str(), "none" | "exit" | "cancelSelection") {
            c.right_click = "none".into();
        }
        // 识字框改成"四角"样式之前存下来的设置：还是以前那个白色实线圆角框的，换成新样式（只换一次，
        // 之后用户自己改回实线就不再动）
        if c.extra
            .insert("ocrFrameCorners".into(), Value::Bool(true))
            .is_none()
            && c.ocr_frame.style == "solid"
            && c.ocr_frame.radius == 12
            && c.ocr_frame.color.eq_ignore_ascii_case("#FFFFFF")
        {
            c.ocr_frame.style = "corners".into();
            c.ocr_frame.radius = 24;
        }
        c.frame.sanitize();
        c.ocr_frame.sanitize();
        migrate_recognize_group(c);
        migrate_record_action(c);
        fix_toolbar(c);
        if !matches!(c.toolbar_style.as_str(), "glass" | "frosted" | "flat") {
            c.toolbar_style = "flat".into();
        }
        c.snap_threshold = c.snap_threshold.min(32);
        c.jpg_quality = c.jpg_quality.clamp(40, 100);
        if !matches!(c.color_format.as_str(), "hex" | "rgb" | "hsl") {
            c.color_format = "hex".into();
        }
        let l = &mut self.longshot;
        l.scroll_debounce_ms = l.scroll_debounce_ms.clamp(80, 600);
        l.max_height = l.max_height.clamp(2_000, 32_000);
        migrate_translate_providers(&mut self.translate);
        let sel = &mut self.translate.selection;
        if !matches!(
            sel.button_position.as_str(),
            "bottomRight" | "topRight" | "bottomLeft" | "topLeft"
        ) {
            sel.button_position = "bottomRight".into();
        }
        sel.glass.sanitize();
        sel.solid.sanitize();
        if !matches!(
            sel.button_style.as_str(),
            "liquid" | "frosted" | "solid" | "custom"
        ) {
            sel.button_style = "liquid".into();
        }
        sel.button_size = sel.button_size.clamp(20, 48);
        let mail = &mut self.clipboard.mail_codes;
        if !matches!(
            mail.provider.as_str(),
            "qq" | "163" | "gmail" | "icloud" | "proton" | "custom"
        ) {
            mail.provider = "qq".into();
        }
        if !matches!(mail.security.as_str(), "tls" | "starttls") {
            mail.security = "tls".into();
        }
        mail.address = mail.address.trim().to_string();
        mail.host = mail.host.trim().to_string();
        if !matches!(sel.modifier.as_str(), "none" | "alt" | "ctrl") {
            sel.modifier = "none".into();
        }
        let pop = &mut self.translate.popup;
        pop.width = pop.width.clamp(320, 1000);
        if pop.height != 0 {
            pop.height = pop.height.clamp(200, 1200);
        }
        pop.font_size = pop.font_size.clamp(12, 22);
        pop.opacity = pop.opacity.clamp(0.3, 1.0);
        pop.radius = pop.radius.min(24);
        if !matches!(pop.ai_layout.as_str(), "drawer" | "side") {
            pop.ai_layout = "drawer".into();
        }
        pop.drawer_height = pop.drawer_height.clamp(200, 1000);
        let ai = &mut self.ai;
        for old in ["你是 CHENOCR 里的助手。", "你是 DATO COR 里的助手。"] {
            if let Some(rest) = ai.system_prompt.strip_prefix(old) {
                ai.system_prompt = format!("你是 DATO OCR 里的助手。{rest}");
            }
        }
        ai.temperature = ai.temperature.clamp(0.0, 2.0);
        ai.max_tokens = ai.max_tokens.min(200_000);
        if !THINKING_LEVELS.contains(&ai.thinking.as_str()) {
            ai.thinking = "auto".into();
        }
        ai.providers.retain(|p| !p.id.trim().is_empty());
        for p in &mut ai.providers {
            if !matches!(p.kind.as_str(), "openai" | "anthropic") {
                p.kind = "openai".into();
            }
            p.base_url = p.base_url.trim().trim_end_matches('/').to_string();
            p.models.retain(|m| !m.trim().is_empty());
            p.models.dedup();
        }
        let panel = &mut ai.panel;
        panel.font_size = panel.font_size.clamp(11, 22);
        panel.width = panel.width.clamp(360, 1400);
        panel.height = panel.height.clamp(360, 1400);
        if !matches!(panel.layout.as_str(), "bubble" | "plain") {
            panel.layout = "bubble".into();
        }
        let net = &mut self.network;
        if !matches!(net.proxy_mode.as_str(), "system" | "none" | "custom") {
            net.proxy_mode = "system".into();
        }
        net.proxy_url = net.proxy_url.trim().to_string();
        let cb = &mut self.clipboard;
        cb.max_text_mb = cb.max_text_mb.clamp(1, 50);
        cb.max_image_mb = cb.max_image_mb.clamp(1, 200);
        for item in &mut cb.blacklist {
            *item = item.trim().to_lowercase();
        }
        cb.blacklist.retain(|s| !s.is_empty());
        let sy = &mut self.sync;
        sy.device_name = sy.device_name.trim().chars().take(40).collect();
        if sy.lan_port < 1024 {
            sy.lan_port = 47380;
        }
        sy.webdav_url = sy.webdav_url.trim().to_string();
        sy.webdav_user = sy.webdav_user.trim().to_string();
        sy.webdav_folder = sy
            .webdav_folder
            .trim()
            .trim_matches(['/', '\\'])
            .to_string();
        if sy.webdav_folder.is_empty() {
            sy.webdav_folder = "DATO-OCR".into();
        }
        sy.webdav_interval = sy.webdav_interval.clamp(5, 300);
        sy.max_image_mb = sy.max_image_mb.clamp(1, 50);
        if !matches!(self.update.channel.as_str(), "cn" | "global") {
            self.update.channel = "cn".into();
        }
        self
    }
}

/// 旧版用 `freeProviders`（启用的免费源）+ `customKind`（一个自填源）两个字段，
/// 新版合并成一个带顺序和开关的 `providers` 列表。顺手补齐缺的源、去掉不认识的。
fn migrate_translate_providers(tr: &mut TranslateSettings) {
    let old_free = tr.extra.remove("freeProviders");
    let old_custom = tr.extra.remove("customKind");
    tr.extra.remove("provider");
    if old_free.is_some() || old_custom.is_some() {
        let mut list = Vec::new();
        if let Some(custom) = old_custom.as_ref().and_then(Value::as_str) {
            if matches!(custom, "deepl" | "openai") {
                // 旧版里自填源总是优先
                list.push(ProviderEntry {
                    id: custom.into(),
                    enabled: true,
                });
            }
        }
        let free: Vec<String> = old_free
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        for id in free {
            // 早期默认源 microsoft（Edge 翻译接口）已下线，换成必应网页版
            let id = if id == "microsoft" { "bing".into() } else { id };
            list.push(ProviderEntry { id, enabled: true });
        }
        tr.providers = list;
    }
    // 自己添加的：标识、类型不对的丢掉，名字空的补一个
    let mut custom_ids: Vec<String> = Vec::new();
    tr.custom.retain_mut(|c| {
        c.id = c.id.trim().to_string();
        let ok = c.id.starts_with("custom-")
            && !custom_ids.contains(&c.id)
            && matches!(c.kind.as_str(), "ai" | "deeplx" | "http");
        if ok {
            if c.name.trim().is_empty() {
                c.name = c.kind.to_uppercase();
            }
            custom_ids.push(c.id.clone());
        }
        ok
    });
    let mut seen: Vec<String> = Vec::new();
    tr.providers.retain(|p| {
        let known = TRANSLATE_PROVIDERS.contains(&p.id.as_str()) || custom_ids.contains(&p.id);
        let keep = known && !seen.contains(&p.id);
        seen.push(p.id.clone());
        keep
    });
    // 刚添加的自定义翻译：排到最后，直接打开
    for id in &custom_ids {
        if !tr.providers.iter().any(|p| &p.id == id) {
            tr.providers.push(ProviderEntry {
                id: id.clone(),
                enabled: true,
            });
        }
    }
    for id in TRANSLATE_PROVIDERS {
        if tr.providers.iter().any(|p| p.id == id) {
            continue;
        }
        if id == "youdao" {
            // 后来加的免费源：老用户也直接打开，排在腾讯翻译后面（没有就排最后）
            let at = tr
                .providers
                .iter()
                .position(|p| p.id == "transmart")
                .map_or(tr.providers.len(), |i| i + 1);
            tr.providers.insert(
                at,
                ProviderEntry {
                    id: id.into(),
                    enabled: true,
                },
            );
            continue;
        }
        tr.providers.push(ProviderEntry {
            id: id.into(),
            enabled: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix_order_keeps_custom_order_and_adds_new_buttons() {
        // 老版本只有 6 个工具、用户把文字挪到了最前面；新版本加了荧光笔、序号、橡皮擦
        let mut order: Vec<String> = [
            "text", "rect", "ellipse", "arrow", "pen", "mosaic", "bogus", "rect",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        fix_order(&mut order, &TOOLBAR_TOOLS);
        assert_eq!(
            order,
            [
                "text",
                "rect",
                "ellipse",
                "arrow",
                "pen",
                "highlight",
                "counter",
                "mosaic",
                "measure",
                "watermark",
                "template",
                "eraser"
            ]
        );
    }

    #[test]
    fn old_toolbar_gets_a_recognize_group_where_qr_was() {
        let mut c = CaptureSettings {
            toolbar_actions: owned(&["ocr", "qr", "translate", "table", "formula", "ai"]),
            toolbar_groups: Vec::new(),
            ..Default::default()
        };
        c.extra.clear();
        migrate_recognize_group(&mut c);
        fix_toolbar(&mut c);
        assert_eq!(
            c.toolbar_actions,
            [
                "ocr",
                "group:recognize",
                "translate",
                "ai",
                "longshot",
                "record"
            ]
        );
        assert_eq!(c.toolbar_groups[0].items, ["qr", "table", "formula"]);
        // 只迁移一次：用户之后把组解散了就不再建
        c.toolbar_groups.clear();
        migrate_recognize_group(&mut c);
        fix_toolbar(&mut c);
        assert!(c.toolbar_actions.contains(&"qr".to_string()));
        assert!(!c
            .toolbar_actions
            .iter()
            .any(|i| i.starts_with(GROUP_PREFIX)));
    }

    #[test]
    fn gif_and_video_buttons_become_one_record_button() {
        let mut c = CaptureSettings {
            toolbar_actions: owned(&["ocr", "video", "ai", "gif"]),
            toolbar_hidden: owned(&["gif"]),
            ..Default::default()
        };
        migrate_record_action(&mut c);
        assert_eq!(c.toolbar_actions, ["ocr", "record", "ai"]);
        // 只藏了一个：录制按钮照样显示
        assert!(c.toolbar_hidden.is_empty());
        let mut both = CaptureSettings {
            toolbar_actions: owned(&["gif", "video"]),
            toolbar_hidden: owned(&["gif", "video"]),
            ..Default::default()
        };
        migrate_record_action(&mut both);
        assert_eq!(both.toolbar_actions, ["record"]);
        assert_eq!(both.toolbar_hidden, ["record"]);
    }

    #[test]
    fn groups_keep_one_category_and_items_leave_the_order() {
        let mut c = CaptureSettings::default();
        c.toolbar_groups.push(ToolbarGroup {
            id: "shapes".into(),
            name: "形状".into(),
            items: owned(&["rect", "ellipse", "ocr", "rect"]),
        });
        fix_toolbar(&mut c);
        let shapes = c.toolbar_groups.iter().find(|g| g.id == "shapes").unwrap();
        assert_eq!(shapes.items, ["rect", "ellipse"]);
        assert!(!c.toolbar_tools.contains(&"rect".to_string()));
        assert!(c.toolbar_tools.contains(&"group:shapes".to_string()));
        assert!(c.toolbar_actions.contains(&"ocr".to_string()));
    }

    #[test]
    fn untouched_ai_defaults_follow_the_language() {
        let mut ai = AiSettings::default();
        ai.quick_prompts[1].prompt = "我自己写的总结提示词 {text}".into();
        ai.providers.push(AiProvider {
            name: "自定义服务".into(),
            ..Default::default()
        });
        ai.follow_language(true);
        assert!(ai.system_prompt.starts_with("You are the assistant"));
        assert_eq!(ai.quick_prompts[0].label, "Explain");
        assert_eq!(ai.quick_prompts[1].label, "Summarize");
        // 用户改过的不动
        assert_eq!(ai.quick_prompts[1].prompt, "我自己写的总结提示词 {text}");
        assert_eq!(ai.providers[0].name, "Custom service");
        ai.follow_language(false);
        assert!(ai.system_prompt.starts_with("你是 DATO OCR 里的助手"));
        assert_eq!(ai.quick_prompts[0].prompt, "解释下面这段内容：\n\n{text}");
        assert_eq!(ai.providers[0].name, "自定义服务");
    }

    #[test]
    fn missing_fields_take_defaults_and_unknown_fields_survive() {
        let text = r#"{"capture":{"maskOpacity":0.3,"futureFlag":true},"newSection":{"a":1}}"#;
        let settings: Settings = serde_json::from_str(text).unwrap();
        assert_eq!(settings.capture.mask_opacity, 0.3);
        assert_eq!(settings.capture.snap_threshold, 8);
        assert_eq!(
            settings.hotkeys.capture,
            crate::platform::default_hotkeys().capture
        );
        assert_eq!(
            settings.capture.extra.get("futureFlag"),
            Some(&Value::Bool(true))
        );
        let back = serde_json::to_value(&settings).unwrap();
        assert_eq!(back["newSection"]["a"], 1);
        assert_eq!(back["capture"]["futureFlag"], true);
    }

    #[test]
    fn sanitize_clamps_values() {
        let mut s = Settings::default();
        s.capture.mask_opacity = 5.0;
        s.capture.color_format = "cmyk".into();
        s.clipboard.blacklist = vec![" KeePass.EXE ".into(), "".into()];
        let s = s.sanitized();
        assert_eq!(s.capture.mask_opacity, 0.9);
        assert_eq!(s.capture.color_format, "hex");
        assert_eq!(s.clipboard.blacklist, vec!["keepass.exe".to_string()]);
    }

    fn ids(s: &Settings) -> Vec<(&str, bool)> {
        s.translate
            .providers
            .iter()
            .map(|p| (p.id.as_str(), p.enabled))
            .collect()
    }

    #[test]
    fn old_translate_fields_migrate_to_ordered_list() {
        let text = r#"{"translate":{"freeProviders":["microsoft","google"],"customKind":"deepl"}}"#;
        let s: Settings = serde_json::from_str(text).unwrap();
        let s = s.sanitized();
        assert_eq!(
            ids(&s),
            vec![
                ("deepl", true),
                ("bing", true),
                ("google", true),
                ("transmart", false),
                ("youdao", true),
                ("openai", false),
                // 后来加的要自己填密钥的源：补在最后，默认关着
                ("baidu", false),
                ("caiyun", false),
                ("niutrans", false),
                ("azure", false),
                ("tencentcloud", false),
                ("volcengine", false)
            ]
        );
        assert!(!s.translate.extra.contains_key("freeProviders"));
        assert!(!s.translate.extra.contains_key("customKind"));
    }

    /// 自己添加的翻译：进顺序表（排最后、打开）；标识、类型不对的丢掉；删掉之后顺序表里的也跟着没
    #[test]
    fn custom_translators_join_the_order() {
        let text = r#"{"translate":{"custom":[
            {"id":"custom-a1","name":"我的模型","kind":"ai","url":"http://localhost:11434/v1","model":"qwen"},
            {"id":"bad","name":"x","kind":"ai"},
            {"id":"custom-b2","name":"","kind":"deeplx","url":"https://x/translate"},
            {"id":"custom-c3","name":"y","kind":"ftp"}
        ],"providers":[{"id":"custom-gone","enabled":true},{"id":"google","enabled":true}]}}"#;
        let s: Settings = serde_json::from_str(text).unwrap();
        let s = s.sanitized();
        let custom: Vec<&str> = s.translate.custom.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(custom, vec!["custom-a1", "custom-b2"]);
        assert_eq!(s.translate.custom[1].name, "DEEPLX");
        let all = ids(&s);
        assert_eq!(all[0], ("google", true));
        assert!(!all.iter().any(|(id, _)| *id == "custom-gone"));
        assert!(all.contains(&("custom-a1", true)) && all.contains(&("custom-b2", true)));
    }

    #[test]
    fn brand_rename_updates_untouched_defaults_only() {
        let old = r#"{"capture":{"fileNameTemplate":"CHENOCR_{yyyy}{MM}{dd}_{HH}{mm}{ss}"},"ai":{"systemPrompt":"你是 CHENOCR 里的助手。别的话"}}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        let s = s.sanitized();
        assert_eq!(s.capture.file_name_template, DEFAULT_FILE_NAME);
        assert_eq!(s.ai.system_prompt, "你是 DATO OCR 里的助手。别的话");
        let custom = r#"{"capture":{"fileNameTemplate":"CHENOCR-{yyyy}"}}"#;
        let s: Settings = serde_json::from_str(custom).unwrap();
        assert_eq!(s.sanitized().capture.file_name_template, "CHENOCR-{yyyy}");

        // 第二次改名：DATO COR 时期的默认值也跟着换
        let cor = r#"{"capture":{"fileNameTemplate":"DATO COR_{yyyy}{MM}{dd}_{HH}{mm}{ss}"},"ai":{"systemPrompt":"你是 DATO COR 里的助手。回答简洁"}}"#;
        let s: Settings = serde_json::from_str(cor).unwrap();
        let s = s.sanitized();
        assert_eq!(
            s.capture.file_name_template,
            "DATO OCR_{yyyy}{MM}{dd}_{HH}{mm}{ss}"
        );
        assert_eq!(s.ai.system_prompt, "你是 DATO OCR 里的助手。回答简洁");
    }

    #[test]
    fn provider_list_is_completed_and_deduplicated() {
        let text = r#"{"translate":{"providers":[{"id":"google","enabled":true},{"id":"google","enabled":false},{"id":"nope","enabled":true}]}}"#;
        let s: Settings = serde_json::from_str(text).unwrap();
        let s = s.sanitized();
        assert_eq!(ids(&s)[0], ("google", true));
        assert_eq!(s.translate.providers.len(), TRANSLATE_PROVIDERS.len());
    }
}
