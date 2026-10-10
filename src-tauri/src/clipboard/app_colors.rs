//! 剪贴板卡片标题栏的颜色：常见应用用固定的颜色（参照 Paste：照片是红色、备忘录是黄色、Xcode 是蓝色……），
//! 图标是好几种颜色拼起来的（照片的花瓣、Chrome）按图标算主色会算偏，所以单独列出来。
//! 其余的按图标主色算（`imaging::dominant_color`）。

/// 应用文件名（小写，macOS 是 `xxx.app`，Windows 是 `xxx.exe`）→ 颜色
const KNOWN: &[(&str, &str)] = &[
    // 苹果自带
    ("photos.app", "#f5414b"),
    ("notes.app", "#f7c600"),
    ("messages.app", "#34c759"),
    ("facetime.app", "#34c759"),
    ("maps.app", "#34c759"),
    ("safari.app", "#0a84ff"),
    ("mail.app", "#0a84ff"),
    ("finder.app", "#1e8cf0"),
    ("xcode.app", "#1d7cf2"),
    ("music.app", "#fa3e55"),
    ("podcasts.app", "#9a50e0"),
    ("calendar.app", "#f43f3a"),
    ("dictionary.app", "#e8323c"),
    ("reminders.app", "#0a84ff"),
    ("pages.app", "#f7861b"),
    ("numbers.app", "#2fb550"),
    ("keynote.app", "#1d8cf8"),
    ("preview.app", "#3d8ef0"),
    ("terminal.app", "#1c1c1e"),
    ("passwords.app", "#1c1c1e"),
    ("textedit.app", "#8e8e93"),
    ("system settings.app", "#8e8e93"),
    // 常用的第三方
    ("wechat.app", "#07c160"),
    ("wechat.exe", "#07c160"),
    ("weixin.exe", "#07c160"),
    ("qq.app", "#12b7f5"),
    ("qq.exe", "#12b7f5"),
    ("dingtalk.app", "#1677ff"),
    ("dingtalk.exe", "#1677ff"),
    ("lark.app", "#3370ff"),
    ("feishu.app", "#3370ff"),
    ("feishu.exe", "#3370ff"),
    ("visual studio code.app", "#007acc"),
    ("code.exe", "#007acc"),
    ("cursor.app", "#1c1c1e"),
    ("cursor.exe", "#1c1c1e"),
    ("google chrome.app", "#1a73e8"),
    ("chrome.exe", "#1a73e8"),
    ("microsoft edge.app", "#0c7bd8"),
    ("msedge.exe", "#0c7bd8"),
    ("firefox.app", "#ff7139"),
    ("firefox.exe", "#ff7139"),
    ("microsoft word.app", "#2b579a"),
    ("winword.exe", "#2b579a"),
    ("microsoft excel.app", "#217346"),
    ("excel.exe", "#217346"),
    ("microsoft powerpoint.app", "#d24726"),
    ("powerpnt.exe", "#d24726"),
    ("claude.app", "#d97757"),
    ("claude.exe", "#d97757"),
    ("notion.app", "#1c1c1e"),
    ("notion.exe", "#1c1c1e"),
    ("figma.app", "#1c1c1e"),
    ("figma.exe", "#1c1c1e"),
    ("slack.app", "#4a154b"),
    ("slack.exe", "#4a154b"),
    ("explorer.exe", "#f0b429"),
    ("windowsterminal.exe", "#1c1c1e"),
];

/// 应用路径 → 固定颜色（没有返回 None，调用方按图标算）
pub fn known(app_path: &str) -> Option<&'static str> {
    let name = app_path
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()?
        .to_lowercase();
    KNOWN.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

#[cfg(test)]
mod tests {
    use super::known;

    #[test]
    fn matches_by_file_name() {
        assert_eq!(known("/System/Applications/Photos.app"), Some("#f5414b"));
        assert_eq!(known("/System/Applications/Photos.app/"), Some("#f5414b"));
        assert_eq!(
            known(r"C:\Program Files\Tencent\WeChat\WeChat.exe"),
            Some("#07c160")
        );
        assert_eq!(known("/Applications/Unknown Thing.app"), None);
    }
}
