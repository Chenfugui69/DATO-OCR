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
    pub network: NetworkSettings,
    pub ai: AiSettings,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
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
            network: Default::default(),
            ai: Default::default(),
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
            language: "zh-CN".into(),
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
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            capture: "F1".into(),
            longshot: "F2".into(),
            ocr: "F3".into(),
            clipboard: "Alt+V".into(),
            translate: "Ctrl+Alt+T".into(),
            instant: "Shift+F1".into(),
            ai: String::new(),
            extra: Map::new(),
        }
    }
}

const DEFAULT_FILE_NAME: &str = "DATO COR_{yyyy}{MM}{dd}_{HH}{mm}{ss}";

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
    /// 普通截图的选区框样式
    pub frame: FrameStyle,
    /// 截图识字（F3）的选区框样式，默认白色圆角
    pub ocr_frame: FrameStyle,
    pub show_magnifier: bool,
    /// hex | rgb | hsl
    pub color_format: String,
    pub detect_windows: bool,
    pub detect_child_windows: bool,
    pub snap_threshold: u32,
    /// exit | cancelSelection
    pub right_click: String,
    /// copy | copyAndSave
    pub finish_action: String,
    pub save_to_library: bool,
    /// None = 图片\DATO COR
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
            mask_opacity: 0.45,
            ocr_mask_opacity: 0.0,
            ocr_instant: true,
            instant_action: "select".into(),
            instant_cursor: true,
            frame: FrameStyle::default(),
            ocr_frame: FrameStyle {
                color: "#FFFFFF".into(),
                width: 2.0,
                style: "solid".into(),
                radius: 12,
            },
            show_magnifier: true,
            color_format: "hex".into(),
            detect_windows: true,
            detect_child_windows: true,
            snap_threshold: 8,
            right_click: "exit".into(),
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

/// 截图选区框的样式。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct FrameStyle {
    /// `accent` = 跟随系统主题色，否则为 #RRGGBB
    pub color: String,
    /// 线宽（逻辑像素）
    pub width: f64,
    /// solid | dashed | dotted
    pub style: String,
    /// 圆角（逻辑像素）。只影响框的样子，截下来的图仍是矩形
    pub radius: u32,
}

impl Default for FrameStyle {
    fn default() -> Self {
        Self {
            color: "accent".into(),
            width: 1.5,
            style: "solid".into(),
            radius: 0,
        }
    }
}

impl FrameStyle {
    fn sanitize(&mut self) {
        self.width = self.width.clamp(1.0, 6.0);
        self.radius = self.radius.min(32);
        if !matches!(self.style.as_str(), "solid" | "dashed" | "dotted") {
            self.style = "solid".into();
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
    /// rapid | system
    pub engine: String,
    /// 0 = 永不回收
    pub idle_timeout_minutes: u32,
    pub keep_line_breaks: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for OcrSettings {
    fn default() -> Self {
        Self {
            engine: "rapid".into(),
            idle_timeout_minutes: 5,
            keep_line_breaks: false,
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

impl Default for AiSettings {
    fn default() -> Self {
        let prompt = |id: &str, label: &str, prompt: &str| QuickPrompt {
            id: id.into(),
            label: label.into(),
            prompt: prompt.into(),
        };
        Self {
            providers: Vec::new(),
            default_model: String::new(),
            system_prompt: "你是 DATO COR 里的助手。回答简洁、准确，默认用简体中文；用户给的内容是什么语言、要求用什么语言，就照做。".into(),
            temperature: 0.7,
            max_tokens: 0,
            quick_prompts: vec![
                prompt("explain", "解释", "解释下面这段内容：\n\n{text}"),
                prompt("summary", "总结", "用几句话总结下面这段内容的要点：\n\n{text}"),
                prompt("translate", "翻译", "把下面这段内容翻译成中文（如果已经是中文就翻译成英文），只给译文：\n\n{text}"),
                prompt("polish", "润色", "润色下面这段文字，保持原意和语言，让它更通顺自然：\n\n{text}"),
                prompt("reply", "帮我回复", "这是别人发给我的消息，帮我写一条得体的回复：\n\n{text}"),
            ],
            panel: Default::default(),
            extra: Map::new(),
        }
    }
}

/// 全部翻译源 id。内置免费源在前，自填密钥的在后。
pub const TRANSLATE_PROVIDERS: [&str; 5] = ["bing", "transmart", "google", "deepl", "openai"];

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
    pub custom_base_url: String,
    pub custom_model: String,
    pub selection: SelectionTranslateSettings,
    /// 划词翻译面板的外观
    pub popup: PopupStyle,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
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
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for PopupStyle {
    fn default() -> Self {
        Self {
            width: 460,
            height: 0,
            font_size: 15,
            opacity: 1.0,
            radius: 14,
            show_source: false,
            extra: Map::new(),
        }
    }
}

impl Default for TranslateSettings {
    fn default() -> Self {
        Self {
            target_language: "auto".into(),
            providers: TRANSLATE_PROVIDERS
                .iter()
                .map(|id| ProviderEntry {
                    id: (*id).into(),
                    enabled: matches!(*id, "bing" | "transmart" | "google"),
                })
                .collect(),
            show_all_providers: true,
            custom_base_url: "https://api.openai.com/v1".into(),
            custom_model: "gpt-4o-mini".into(),
            selection: Default::default(),
            popup: Default::default(),
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
            modifier: "none".into(),
            extra: Map::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ClipboardSettings {
    pub enabled: bool,
    /// bottom | vertical
    pub panel_style: String,
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
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for ClipboardSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            panel_style: "bottom".into(),
            max_text_mb: 5,
            max_image_mb: 30,
            respect_privacy_flag: false,
            use_blacklist: false,
            blacklist: Vec::new(),
            retention_days: 0,
            retention_max_items: 0,
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
        let c = &mut self.capture;
        // 品牌改名：还是旧默认值的跟着换成新名字，用户自己改过的不动
        if c.file_name_template == "CHENOCR_{yyyy}{MM}{dd}_{HH}{mm}{ss}" {
            c.file_name_template = DEFAULT_FILE_NAME.into();
        }
        c.mask_opacity = c.mask_opacity.clamp(0.0, 0.9);
        c.ocr_mask_opacity = c.ocr_mask_opacity.clamp(0.0, 0.9);
        if !matches!(c.instant_action.as_str(), "select" | "copy" | "save") {
            c.instant_action = "select".into();
        }
        c.frame.sanitize();
        c.ocr_frame.sanitize();
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
        if !matches!(sel.modifier.as_str(), "none" | "alt" | "ctrl") {
            sel.modifier = "none".into();
        }
        let pop = &mut self.translate.popup;
        pop.width = pop.width.clamp(320, 1000);
        if pop.height != 0 {
            pop.height = pop.height.clamp(200, 1200);
        }
        pop.font_size = pop.font_size.clamp(12, 22);
        pop.opacity = pop.opacity.clamp(0.6, 1.0);
        pop.radius = pop.radius.min(24);
        let ai = &mut self.ai;
        if ai.system_prompt.starts_with("你是 CHENOCR 里的助手。") {
            ai.system_prompt = ai.system_prompt.replacen("CHENOCR", "DATO COR", 1);
        }
        ai.temperature = ai.temperature.clamp(0.0, 2.0);
        ai.max_tokens = ai.max_tokens.min(200_000);
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
    let mut seen: Vec<String> = Vec::new();
    tr.providers.retain(|p| {
        let keep = TRANSLATE_PROVIDERS.contains(&p.id.as_str()) && !seen.contains(&p.id);
        seen.push(p.id.clone());
        keep
    });
    for id in TRANSLATE_PROVIDERS {
        if !tr.providers.iter().any(|p| p.id == id) {
            tr.providers.push(ProviderEntry {
                id: id.into(),
                enabled: false,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults_and_unknown_fields_survive() {
        let text = r#"{"capture":{"maskOpacity":0.3,"futureFlag":true},"newSection":{"a":1}}"#;
        let settings: Settings = serde_json::from_str(text).unwrap();
        assert_eq!(settings.capture.mask_opacity, 0.3);
        assert_eq!(settings.capture.snap_threshold, 8);
        assert_eq!(settings.hotkeys.capture, "F1");
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
                ("openai", false)
            ]
        );
        assert!(!s.translate.extra.contains_key("freeProviders"));
        assert!(!s.translate.extra.contains_key("customKind"));
    }

    #[test]
    fn brand_rename_updates_untouched_defaults_only() {
        let old = r#"{"capture":{"fileNameTemplate":"CHENOCR_{yyyy}{MM}{dd}_{HH}{mm}{ss}"},"ai":{"systemPrompt":"你是 CHENOCR 里的助手。别的话"}}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        let s = s.sanitized();
        assert_eq!(s.capture.file_name_template, DEFAULT_FILE_NAME);
        assert_eq!(s.ai.system_prompt, "你是 DATO COR 里的助手。别的话");
        let custom = r#"{"capture":{"fileNameTemplate":"CHENOCR-{yyyy}"}}"#;
        let s: Settings = serde_json::from_str(custom).unwrap();
        assert_eq!(s.sanitized().capture.file_name_template, "CHENOCR-{yyyy}");
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
