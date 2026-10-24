//! Windows 上录视频（MP4）：系统自带的 Media Foundation，不用带 FFmpeg。
//!
//! - `video`：写 MP4（H.264 / H.265 交给系统的编码器，有显卡编码就用显卡）、剪辑 / 变速 / 混音导出
//! - `audio`：WASAPI 采声音（系统声音是默认播放设备的回环，麦克风是默认录音设备）
//! - `camera`：摄像头画面（叠在录屏角落）
//!
//! 和 macOS 那边（`macos/video.rs`、`macos/media.rs`）对外是同一套接口。

pub mod audio;
pub mod camera;
pub mod stretch;
pub mod video;

use std::sync::OnceLock;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Media::MediaFoundation::{MFStartup, MFSTARTUP_FULL, MF_VERSION};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::error::{AppError, AppResult};
use crate::platform::types::{MediaAccess, MediaKind};

pub use camera::Camera;
pub use video::{export as export_video, VideoWriter};

/// 本线程进多线程套间（MF 的对象都是自由线程的），整个进程启动一次 MF
pub(crate) fn init() -> AppResult<()> {
    // SAFETY: 已经初始化过（S_FALSE）、套间不同（RPC_E_CHANGED_MODE）都不影响后面用 MF
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    STARTED
        // SAFETY: 进程里只调一次，不配对 MFShutdown（进程退出时系统收拾）
        .get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(|e| e.message()))
        .clone()
        .map_err(|e| AppError::msg(format!("系统的媒体组件用不了：{e}")))
}

/// 两个 32 位数并成 MF 属性里的一个 64 位数（宽高、帧率、像素比都这么存）
pub(crate) fn pack(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

pub(crate) fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

pub(crate) fn mf_err(what: &str, e: windows::core::Error) -> AppError {
    AppError::msg(format!("{what}：{}", e.message()))
}

/// MF 的对象是自由线程的（多线程套间里随便哪个线程都能用），但 windows-rs 没给它们标 Send。
/// 只用在有锁保护、或者明确只在一个线程上用的地方
pub(crate) struct Mt<T>(pub T);
// SAFETY: 见上
unsafe impl<T> Send for Mt<T> {}
unsafe impl<T> Sync for Mt<T> {}

// ───────────────────────── 摄像头 / 麦克风的权限 ─────────────────────────
//
// Windows 的「隐私和安全性」里可以整个关掉摄像头、麦克风，或者只不让桌面应用用。桌面应用没有
// 弹框问用户这一步：关着就是没权限（打开设置页让用户自己开），开着就能用。

const CONSENT: &str =
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore";

fn consent(root: HKEY, sub: &str) -> Option<String> {
    let mut buf = [0u16; 32];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: 缓冲区和长度对得上；读不到返回错误
    unsafe {
        RegGetValueW(
            root,
            &HSTRING::from(sub),
            &HSTRING::from("Value"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
        .ok()
        .ok()?;
    }
    let n = (len as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

fn store_key(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Camera => "webcam",
        MediaKind::Microphone => "microphone",
    }
}

pub fn media_access(kind: MediaKind) -> MediaAccess {
    let base = format!(r"{CONSENT}\{}", store_key(kind));
    let denied =
        |root: HKEY, sub: &str| consent(root, sub).is_some_and(|v| v.eq_ignore_ascii_case("Deny"));
    if denied(HKEY_LOCAL_MACHINE, &base)
        || denied(HKEY_CURRENT_USER, &base)
        || denied(HKEY_CURRENT_USER, &format!(r"{base}\NonPackaged"))
    {
        MediaAccess::Denied
    } else {
        MediaAccess::Granted
    }
}

/// 没权限时打开系统设置里对应的那一页（桌面应用没有授权弹框），返回现在的状态
pub fn request_media_access(kind: MediaKind) -> MediaAccess {
    let access = media_access(kind);
    if access == MediaAccess::Denied {
        let page = match kind {
            MediaKind::Camera => "ms-settings:privacy-webcam",
            MediaKind::Microphone => "ms-settings:privacy-microphone",
        };
        // SAFETY: 打开一个系统设置的链接
        unsafe {
            ShellExecuteW(
                None,
                &HSTRING::from("open"),
                &HSTRING::from(page),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        }
    }
    access
}
