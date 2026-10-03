//! 剪贴板：监听、多格式读取、写入、整份备份/恢复（规格 05 §1）。
//!
//! 监听用 `AddClipboardFormatListener` + 专用线程上的 message-only 窗口，不轮询。
//! 收到 `WM_CLIPBOARDUPDATE` 后**同步**取前台窗口作为来源 —— 异步再取的话前台
//! 可能已经切走了（规格 05 §1.6）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use std::time::Duration;

use image::RgbaImage;
use windows::core::w;
use windows::Win32::Foundation::{
    GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, EnumClipboardFormats,
    GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassExW,
    TranslateMessage, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE,
    WNDCLASSEXW,
};

use super::util::{handle_of, hwnd as to_hwnd};
use super::window_enum;
use crate::error::{AppError, AppResult};
use crate::platform::{ClipboardPayload, ClipboardSnapshot};

/// 监听窗口句柄，写剪贴板时用作 owner。
static LISTENER: AtomicU64 = AtomicU64::new(0);
static SENDER: OnceLock<Sender<ClipboardSnapshot>> = OnceLock::new();

/// 单份备份的大小上限，超过就不备份（划词翻译后不恢复，好过卡住）。
const MAX_BACKUP_BYTES: usize = 64 * 1024 * 1024;

struct Formats {
    html: u32,
    rtf: u32,
    png: u32,
    exclude: u32,
    can_include: u32,
    viewer_ignore: u32,
    drop_effect: u32,
}

fn formats() -> &'static Formats {
    static F: OnceLock<Formats> = OnceLock::new();
    // SAFETY: 注册剪贴板格式名是幂等的全局操作。
    F.get_or_init(|| unsafe {
        Formats {
            html: RegisterClipboardFormatW(w!("HTML Format")),
            rtf: RegisterClipboardFormatW(w!("Rich Text Format")),
            png: RegisterClipboardFormatW(w!("PNG")),
            exclude: RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")),
            can_include: RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")),
            viewer_ignore: RegisterClipboardFormatW(w!("Clipboard Viewer Ignore")),
            drop_effect: RegisterClipboardFormatW(w!("Preferred DropEffect")),
        }
    })
}

pub fn sequence_number() -> u32 {
    // SAFETY: 无参数。
    unsafe { GetClipboardSequenceNumber() }
}

// ───────────────────────── 监听 ─────────────────────────

pub fn start_listener(tx: Sender<ClipboardSnapshot>) -> AppResult<()> {
    if SENDER.set(tx).is_err() {
        return Err(AppError::msg("剪贴板监听已经启动"));
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new()
        .name("clipboard-listener".into())
        .spawn(move || listener_thread(ready_tx))?;
    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| AppError::msg("剪贴板监听线程没有响应"))?
        .map_err(AppError::Msg)
}

fn listener_thread(ready: std::sync::mpsc::Sender<Result<(), String>>) {
    let _ = formats();
    // SAFETY: 标准的窗口类注册 + message-only 窗口创建 + 消息循环。
    unsafe {
        let instance = match GetModuleHandleW(None) {
            Ok(m) => m.into(),
            Err(err) => {
                let _ = ready.send(Err(err.to_string()));
                return;
            }
        };
        let class_name = w!("ChenocrClipboardListener");
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(listener_proc),
            hInstance: instance,
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassExW(&class);
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("DATO COR Clipboard"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        ) {
            Ok(h) => h,
            Err(err) => {
                let _ = ready.send(Err(format!("创建剪贴板监听窗口失败：{err}")));
                return;
            }
        };
        if let Err(err) = AddClipboardFormatListener(hwnd) {
            let _ = ready.send(Err(format!("注册剪贴板监听失败：{err}")));
            return;
        }
        LISTENER.store(handle_of(hwnd), Ordering::SeqCst);
        let _ = ready.send(Ok(()));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn listener_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        on_update(hwnd);
        return LRESULT(0);
    }
    // SAFETY: 原样转发。
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn on_update(hwnd: HWND) {
    let Some(tx) = SENDER.get() else { return };
    // 必须先同步取来源，再读内容
    let source = window_enum::foreground_window().and_then(window_enum::app_info_of);
    let sequence = sequence_number();
    match read_snapshot(Some(hwnd)) {
        Ok(mut snapshot) => {
            snapshot.source = source;
            snapshot.sequence = sequence;
            let _ = tx.send(snapshot);
        }
        Err(err) => tracing::debug!("读取剪贴板失败：{err}"),
    }
}

// ───────────────────────── 读取 ─────────────────────────

/// 打开剪贴板。别的进程可能正持有它，重试几次。
fn open(owner: Option<HWND>) -> AppResult<ClipboardGuard> {
    for attempt in 0..12 {
        // SAFETY: 成功后由 ClipboardGuard 负责关闭。
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return Ok(ClipboardGuard);
        }
        std::thread::sleep(Duration::from_millis(10 + attempt * 5));
    }
    Err(AppError::msg("剪贴板被其他程序占用"))
}

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: 只在成功打开后构造。
        let _ = unsafe { CloseClipboard() };
    }
}

fn has(format: u32) -> bool {
    // SAFETY: 纯查询。
    unsafe { IsClipboardFormatAvailable(format) }.is_ok()
}

/// 读出某个 HGLOBAL 格式的全部字节。
fn read_bytes(format: u32) -> Option<Vec<u8>> {
    // SAFETY: 剪贴板已打开；GlobalLock 返回的指针在 Unlock 前有效，长度由 GlobalSize 给出。
    unsafe {
        let handle = GetClipboardData(format).ok()?;
        let hg = HGLOBAL(handle.0);
        let size = GlobalSize(hg);
        let ptr = GlobalLock(hg) as *const u8;
        if ptr.is_null() || size == 0 {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
        let _ = GlobalUnlock(hg);
        Some(bytes)
    }
}

fn read_unicode_text() -> Option<String> {
    let bytes = read_bytes(u32::from(CF_UNICODETEXT.0))?;
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

fn read_files() -> Vec<PathBuf> {
    // SAFETY: 剪贴板已打开；DragQueryFileW 的缓冲区长度由切片给出。
    unsafe {
        let Ok(handle) = GetClipboardData(u32::from(CF_HDROP.0)) else {
            return Vec::new();
        };
        let drop = HDROP(handle.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let mut files = Vec::with_capacity(count as usize);
        for i in 0..count.min(500) {
            let len = DragQueryFileW(drop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(drop, i, Some(&mut buf));
            files.push(PathBuf::from(String::from_utf16_lossy(&buf[..len])));
        }
        files
    }
}

fn read_snapshot(owner: Option<HWND>) -> AppResult<ClipboardSnapshot> {
    let f = formats();
    let mut snap = ClipboardSnapshot::default();
    let mut png: Option<Vec<u8>> = None;
    let mut dib: Option<Vec<u8>> = None;
    {
        let _guard = open(owner)?;
        snap.privacy_flagged = has(f.exclude)
            || has(f.viewer_ignore)
            || (has(f.can_include)
                && read_bytes(f.can_include)
                    .is_some_and(|b| b.len() >= 4 && b[..4] == [0, 0, 0, 0]));

        if has(u32::from(CF_HDROP.0)) {
            snap.files = read_files();
        }
        if has(u32::from(CF_UNICODETEXT.0)) {
            snap.text = read_unicode_text();
        }
        if has(f.html) {
            snap.html = read_bytes(f.html).and_then(|b| html_fragment(&b));
        }
        if has(f.rtf) {
            snap.rtf = read_bytes(f.rtf).map(|b| {
                let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
                String::from_utf8_lossy(&b[..end]).into_owned()
            });
        }
        if snap.files.is_empty() {
            if has(f.png) {
                png = read_bytes(f.png);
            }
            if png.is_none() {
                if has(u32::from(CF_DIBV5.0)) {
                    dib = read_bytes(u32::from(CF_DIBV5.0));
                } else if has(u32::from(CF_DIB.0)) {
                    dib = read_bytes(u32::from(CF_DIB.0));
                }
            }
        }
    }
    // 解码放在关闭剪贴板之后，别让其他程序干等
    if let Some(bytes) = png {
        match image::load_from_memory_with_format(&bytes, image::ImageFormat::Png) {
            Ok(img) => {
                snap.image = Some(img.to_rgba8());
                snap.image_png = Some(bytes);
            }
            Err(err) => tracing::debug!("剪贴板 PNG 解码失败：{err}"),
        }
    }
    if snap.image.is_none() {
        if let Some(bytes) = dib {
            match decode_dib(&bytes) {
                Ok(img) => snap.image = Some(img),
                Err(err) => tracing::debug!("剪贴板位图解码失败：{err}"),
            }
        }
    }
    Ok(snap)
}

pub fn read_text() -> Option<String> {
    let owner = LISTENER.load(Ordering::SeqCst);
    let _guard = open((owner != 0).then(|| to_hwnd(owner))).ok()?;
    read_unicode_text()
}

/// "HTML Format" 是带头部的 UTF-8：按 StartFragment/EndFragment 字节偏移截出片段。
fn html_fragment(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let offset = |key: &str| -> Option<usize> {
        let at = text.find(key)? + key.len();
        text[at..]
            .lines()
            .next()?
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|v| *v >= 0)
            .map(|v| v as usize)
    };
    let (start, end) = (offset("StartFragment:")?, offset("EndFragment:")?);
    if start >= end || end > bytes.len() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&bytes[start..end])
            .trim()
            .to_string(),
    )
}

/// CF_DIB / CF_DIBV5 → RGBA。补一个 BITMAPFILEHEADER 交给 image 的 BMP 解码器。
fn decode_dib(dib: &[u8]) -> AppResult<RgbaImage> {
    if dib.len() < 40 {
        return Err(AppError::msg("位图数据过短"));
    }
    let header_size = u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]) as usize;
    let bit_count = u16::from_le_bytes([dib[14], dib[15]]);
    let compression = u32::from_le_bytes([dib[16], dib[17], dib[18], dib[19]]);
    let clr_used = u32::from_le_bytes([dib[32], dib[33], dib[34], dib[35]]) as usize;
    let palette = if bit_count <= 8 {
        if clr_used == 0 {
            1usize << bit_count
        } else {
            clr_used
        }
    } else {
        clr_used
    };
    // BI_BITFIELDS 且只有 40 字节头时，三个掩码紧跟在头后面
    let masks = if header_size == 40 && compression == 3 {
        12
    } else {
        0
    };
    let pixel_offset = 14 + header_size + masks + palette * 4;

    let mut file = Vec::with_capacity(dib.len() + 14);
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
    file.extend_from_slice(&[0, 0, 0, 0]);
    file.extend_from_slice(&(pixel_offset as u32).to_le_bytes());
    file.extend_from_slice(dib);

    let mut img = image::load_from_memory_with_format(&file, image::ImageFormat::Bmp)?.to_rgba8();
    // 很多程序给的 32 位位图 alpha 全是 0（本意是不透明），统一按不透明处理
    if img.pixels().all(|p| p[3] == 0) {
        for p in img.pixels_mut() {
            p[3] = 255;
        }
    }
    Ok(img)
}

// ───────────────────────── 写入 ─────────────────────────

fn alloc(bytes: &[u8]) -> AppResult<HGLOBAL> {
    // SAFETY: 分配后立即写入，长度一致。
    unsafe {
        let hg = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let ptr = GlobalLock(hg) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(hg));
            return Err(AppError::msg("剪贴板内存锁定失败"));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(hg);
        Ok(hg)
    }
}

fn set(format: u32, bytes: &[u8]) -> AppResult<()> {
    let hg = alloc(bytes)?;
    // SAFETY: 成功后内存归系统所有；失败时由我们释放。
    unsafe {
        if let Err(err) = SetClipboardData(format, Some(HANDLE(hg.0))) {
            let _ = GlobalFree(Some(hg));
            return Err(err.into());
        }
    }
    Ok(())
}

fn utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(|u| u.to_le_bytes())
        .collect()
}

pub fn write(payload: &ClipboardPayload) -> AppResult<()> {
    let f = formats();
    // 编码在打开剪贴板之前做完，持有时间越短越好
    let mut entries: Vec<(u32, Vec<u8>)> = Vec::new();
    match payload {
        ClipboardPayload::Text { text, html, rtf } => {
            entries.push((u32::from(CF_UNICODETEXT.0), utf16z(text)));
            if let Some(html) = html {
                entries.push((f.html, build_html_format(html)));
            }
            if let Some(rtf) = rtf {
                let mut b = rtf.as_bytes().to_vec();
                b.push(0);
                entries.push((f.rtf, b));
            }
        }
        ClipboardPayload::Image { image, png } => {
            entries.push((u32::from(CF_DIB.0), encode_dib(image)));
            if let Some(png) = png {
                entries.push((f.png, png.clone()));
            }
        }
        ClipboardPayload::Files(files) => {
            entries.push((u32::from(CF_HDROP.0), build_hdrop(files)));
            // 告诉资源管理器这是"复制"不是"剪切"
            entries.push((f.drop_effect, 1u32.to_le_bytes().to_vec()));
        }
    }

    let owner = LISTENER.load(Ordering::SeqCst);
    let _guard = open((owner != 0).then(|| to_hwnd(owner)))?;
    // SAFETY: 剪贴板已打开。
    unsafe { EmptyClipboard()? };
    for (format, bytes) in &entries {
        set(*format, bytes)?;
    }
    Ok(())
}

fn build_html_format(fragment: &str) -> Vec<u8> {
    const PREFIX: &str = "<html><body>\r\n<!--StartFragment-->";
    const SUFFIX: &str = "<!--EndFragment-->\r\n</body></html>";
    let header_len = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n".len();
    let start_html = header_len;
    let start_fragment = start_html + PREFIX.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + SUFFIX.len();
    let mut out = format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n{PREFIX}{fragment}{SUFFIX}"
    )
    .into_bytes();
    out.push(0);
    out
}

/// 32bpp BI_RGB 自下而上的 DIB。几乎所有程序（画图、微信、浏览器、Office）都认。
fn encode_dib(image: &RgbaImage) -> Vec<u8> {
    let (w, h) = image.dimensions();
    let mut out = Vec::with_capacity(40 + (w * h * 4) as usize);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(w * h * 4).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    let raw = image.as_raw();
    let stride = (w * 4) as usize;
    for row in (0..h as usize).rev() {
        for px in raw[row * stride..(row + 1) * stride].chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    out
}

fn build_hdrop(files: &[PathBuf]) -> Vec<u8> {
    let header = DROPFILES {
        pFiles: std::mem::size_of::<DROPFILES>() as u32,
        pt: POINT::default(),
        fNC: false.into(),
        fWide: true.into(),
    };
    // SAFETY: DROPFILES 是 POD，按字节拷贝。
    let mut out = unsafe {
        std::slice::from_raw_parts(
            &header as *const DROPFILES as *const u8,
            std::mem::size_of::<DROPFILES>(),
        )
        .to_vec()
    };
    for file in files {
        for u in file
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
        {
            out.extend_from_slice(&u.to_le_bytes());
        }
    }
    out.extend_from_slice(&[0, 0]);
    out
}

// ───────────────────────── 备份 / 恢复 ─────────────────────────

pub type Backup = Vec<(u32, Vec<u8>)>;

/// 这些格式是 GDI 句柄而不是 HGLOBAL，不能按字节拷。系统会从 CF_DIB 重新合成位图。
fn is_gdi_format(format: u32) -> bool {
    matches!(format, 2 | 3 | 9 | 14 | 0x80 | 0x82 | 0x83 | 0x8E)
}

pub fn backup() -> Option<Backup> {
    let owner = LISTENER.load(Ordering::SeqCst);
    let _guard = open((owner != 0).then(|| to_hwnd(owner))).ok()?;
    let mut out = Vec::new();
    let mut total = 0usize;
    let mut format = 0u32;
    loop {
        // SAFETY: 剪贴板已打开。
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            break;
        }
        if is_gdi_format(format) {
            continue;
        }
        if let Some(bytes) = read_bytes(format) {
            total += bytes.len();
            if total > MAX_BACKUP_BYTES {
                return None;
            }
            out.push((format, bytes));
        }
    }
    Some(out)
}

pub fn restore(backup: Backup) {
    let owner = LISTENER.load(Ordering::SeqCst);
    let Ok(_guard) = open((owner != 0).then(|| to_hwnd(owner))) else {
        return;
    };
    // SAFETY: 剪贴板已打开。
    if unsafe { EmptyClipboard() }.is_err() {
        return;
    }
    for (format, bytes) in backup {
        let _ = set(format, &bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_format_roundtrip() {
        let encoded = build_html_format("<b>你好</b> world");
        let fragment = html_fragment(&encoded[..encoded.len() - 1]).unwrap();
        assert_eq!(fragment, "<b>你好</b> world");
    }

    #[test]
    fn dib_roundtrip() {
        let mut img = RgbaImage::new(3, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(2, 1, image::Rgba([0, 0, 255, 255]));
        let decoded = decode_dib(&encode_dib(&img)).unwrap();
        assert_eq!(decoded.dimensions(), (3, 2));
        assert_eq!(decoded.get_pixel(0, 0), &image::Rgba([255, 0, 0, 255]));
        assert_eq!(decoded.get_pixel(2, 1), &image::Rgba([0, 0, 255, 255]));
    }
}
