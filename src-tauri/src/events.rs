//! Rust → 前端事件名。前端对应的类型定义在 `src/lib/events.ts`。

pub const SETTINGS_CHANGED: &str = "settings-changed";
pub const VISUALS_CHANGED: &str = "visuals-changed";

pub const CAPTURE_SESSION_START: &str = "capture-session-start";
pub const CAPTURE_SESSION_END: &str = "capture-session-end";
/// 遮罩打开期间又按了截图类热键（F2/F3）：转发给遮罩，对当前选区执行对应动作
pub const CAPTURE_HOTKEY: &str = "capture-hotkey";

pub const LONGSHOT_PROGRESS: &str = "longshot-progress";
pub const LONGSHOT_STATE: &str = "longshot-state";

pub const CLIPBOARD_CHANGED: &str = "clipboard-changed";
pub const CLIPBOARD_PANEL_SHOW: &str = "clipboard-panel-show";

pub const LIBRARY_CHANGED: &str = "library-changed";
pub const OCR_JOB: &str = "ocr-job";
pub const OCR_HISTORY_CHANGED: &str = "ocr-history-changed";

pub const TOAST: &str = "toast";
pub const NAVIGATE: &str = "navigate";
pub const TRANSLATE_REQUEST: &str = "translate-request";
/// 划词悬浮按钮要显示了（payload：第几次显示，前端用来重放出场动画）
pub const SELECTION_BUTTON_SHOW: &str = "selection-button-show";
/// AI 窗口换上新的上下文（payload：AiContext）
pub const AI_CONTEXT: &str = "ai-context";
pub const EDITOR_OPEN: &str = "editor-open";
