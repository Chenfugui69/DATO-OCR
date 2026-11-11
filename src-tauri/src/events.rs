//! Rust → 前端事件名。前端对应的类型定义在 `src/lib/events.ts`。

pub const SETTINGS_CHANGED: &str = "settings-changed";
pub const VISUALS_CHANGED: &str = "visuals-changed";

pub const CAPTURE_SESSION_START: &str = "capture-session-start";
pub const CAPTURE_SESSION_END: &str = "capture-session-end";
/// 遮罩打开期间又按了截图类热键（F2/F3）：转发给遮罩，对当前选区执行对应动作
pub const CAPTURE_HOTKEY: &str = "capture-hotkey";

pub const LONGSHOT_PROGRESS: &str = "longshot-progress";
pub const LONGSHOT_LIVE: &str = "longshot-live";
pub const LONGSHOT_STATE: &str = "longshot-state";
pub const GIF_STATE: &str = "gif-state";
pub const GIF_PROGRESS: &str = "gif-progress";
/// 录制时按下了一个键（按键显示用），带显示的写法：⌘C、⏎……
pub const RECORD_KEY: &str = "record-key";
/// 录制准备开始 / 结束（所有遮罩都收：不压暗、穿透）
pub const RECORD_PREP: &str = "record-prep";
/// 录制准备时的鼠标（遮罩穿透了收不到）：位置、按着的键、滚轮，给鼠标效果预览用
pub const RECORD_PREP_POINTER: &str = "record-prep-pointer";
/// 剪辑窗口：有一段新录的要剪（负载是这段的 id）
pub const RECORD_EDIT: &str = "record-edit";

pub const CLIPBOARD_CHANGED: &str = "clipboard-changed";
pub const CLIPBOARD_PANEL_SHOW: &str = "clipboard-panel-show";
/// 大预览窗口要显示哪一条（剪贴板记录 ID）
pub const CLIP_PREVIEW: &str = "clip-preview";
/// 按住 Shift 的悬浮预览：看哪一条（null = 收起）
pub const CLIP_PEEK: &str = "clip-peek";
/// 拖卡片时的影子：拖着的那几条（空数组 = 收起）
pub const CLIP_GHOST: &str = "clip-ghost";
/// 拖卡片时鼠标下面是不是输入框（影子缩小表示松手就贴在这儿）
pub const CLIP_GHOST_TARGET: &str = "clip-ghost-target";
/// 拖卡片时松开了鼠标（发给开始拖的那个窗口，带着松手时在不在那个窗口外面）。macOS 上鼠标拖出窗口、
/// 窗口设成鼠标穿透后，页面收不到移动和松开，靠 Rust 盯着鼠标告诉它
pub const CLIP_GHOST_RELEASED: &str = "clip-ghost-released";
/// 拖卡片时鼠标出了开始拖的那个窗口（面板该让开了）
pub const CLIP_GHOST_LEFT: &str = "clip-ghost-left";
/// 系统原生拖拽卡片开始了（macOS；截好了拖动图像，系统接管了拖动）
#[cfg(target_os = "macos")]
pub const CLIP_NATIVE_DRAG_BEGIN: &str = "clip-native-drag-begin";
/// 系统原生拖拽卡片结束（macOS）：{ accepted, inside }
#[cfg(target_os = "macos")]
pub const CLIP_NATIVE_DRAG_END: &str = "clip-native-drag-end";
/// 多端同步的状态变了（设备上下线、有人申请加入、网盘同步结果…）
pub const SYNC_CHANGED: &str = "sync-changed";
/// 检查更新 / 下载安装的状态变了
pub const UPDATE_STATUS: &str = "update-status";

pub const LIBRARY_CHANGED: &str = "library-changed";
pub const OCR_JOB: &str = "ocr-job";
pub const OCR_HISTORY_CHANGED: &str = "ocr-history-changed";
/// PaddleOCR 引擎下载进度
pub const OCR_ENGINE_DOWNLOAD: &str = "ocr-engine-download";
/// 本地公式识别模型的下载进度（`ocr::latex::DownloadStatus`）
pub const OCR_LATEX_DOWNLOAD: &str = "ocr-latex-download";

pub const TOAST: &str = "toast";
/// 本程序要截图了：开着毛玻璃的面板换成实心底（true）/ 换回来（false），见 `wm::GlassSnapshot`
pub const GLASS_SNAPSHOT: &str = "glass-snapshot";
pub const NAVIGATE: &str = "navigate";
pub const TRANSLATE_REQUEST: &str = "translate-request";
/// 鼠标在翻译面板外面按下了
pub const TRANSLATE_OUTSIDE: &str = "translate-outside";
/// 设置页的面板预览被收起了（面板上点了关闭）：设置页把"预览"开关拨回去
pub const TRANSLATE_PREVIEW_CLOSED: &str = "translate-preview-closed";
/// 划词悬浮按钮要显示了（payload：第几次显示，前端用来重放出场动画）
pub const SELECTION_BUTTON_SHOW: &str = "selection-button-show";
/// AI 窗口换上新的上下文（payload：AiContext）
pub const AI_CONTEXT: &str = "ai-context";
pub const EDITOR_OPEN: &str = "editor-open";
