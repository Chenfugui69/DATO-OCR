//! 从可执行文件提取应用图标（剪贴板来源应用显示用，规格 05 §1.6）。

use std::path::Path;

use image::RgbaImage;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetIconInfo, PrivateExtractIconsW, HICON, ICONINFO,
};

use super::util::{pcwstr, wide};

const ICON_SIZE: i32 = 48;

pub fn extract(exe: &Path) -> Option<RgbaImage> {
    let icon = private_extract(exe).or_else(|| shell_icon(exe))?;
    let image = icon_to_rgba(icon);
    // SAFETY: 图标由上面的 API 创建，归我们所有。
    unsafe {
        let _ = DestroyIcon(icon);
    }
    image
}

fn private_extract(exe: &Path) -> Option<HICON> {
    let mut path = [0u16; 260];
    let encoded: Vec<u16> = exe.to_string_lossy().encode_utf16().collect();
    if encoded.len() >= path.len() {
        return None;
    }
    path[..encoded.len()].copy_from_slice(&encoded);
    let mut icons = [HICON::default(); 1];
    // SAFETY: 缓冲区大小固定 260；输出一个图标句柄。
    let n =
        unsafe { PrivateExtractIconsW(&path, 0, ICON_SIZE, ICON_SIZE, Some(&mut icons), None, 0) };
    (n > 0 && n != u32::MAX && !icons[0].is_invalid()).then_some(icons[0])
}

fn shell_icon(exe: &Path) -> Option<HICON> {
    let path = wide(&exe.to_string_lossy());
    let mut info = SHFILEINFOW::default();
    // SAFETY: 输出结构体大小如实传入。
    let ok = unsafe {
        SHGetFileInfoW(
            pcwstr(&path),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    (ok != 0 && !info.hIcon.is_invalid()).then_some(info.hIcon)
}

fn icon_to_rgba(icon: HICON) -> Option<RgbaImage> {
    let mut info = ICONINFO::default();
    // SAFETY: GetIconInfo 创建两张位图，下面负责删除。
    unsafe { GetIconInfo(icon, &mut info).ok()? };
    let result = read_bitmap(info.hbmColor, info.hbmMask);
    // SAFETY: 位图归我们所有。
    unsafe {
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(info.hbmColor.into());
        }
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(info.hbmMask.into());
        }
    }
    result
}

fn bitmap_pixels(bitmap: HBITMAP, width: i32, height: i32) -> Option<Vec<u8>> {
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut buf = vec![0u8; (width * height * 4) as usize];
    // SAFETY: 缓冲大小与 bmi 描述一致；DC 在本函数内创建和销毁。
    unsafe {
        let dc = CreateCompatibleDC(None);
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(buf.as_mut_ptr().cast()),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        let _ = DeleteDC(dc);
        (lines == height).then_some(buf)
    }
}

fn read_bitmap(color: HBITMAP, mask: HBITMAP) -> Option<RgbaImage> {
    if color.is_invalid() {
        return None;
    }
    let mut bm = BITMAP::default();
    // SAFETY: 输出到栈上的 BITMAP，大小如实传入。
    let got = unsafe {
        GetObjectW(
            color.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut _ as *mut _),
        )
    };
    if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight);
    let mut px = bitmap_pixels(color, w, h)?;
    let has_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
    if !has_alpha {
        // 老式图标没有 alpha，用掩码位图补：掩码为黑（0）的地方不透明
        let mask_px = if mask.is_invalid() {
            None
        } else {
            bitmap_pixels(mask, w, h)
        };
        for (i, p) in px.chunks_exact_mut(4).enumerate() {
            let transparent = mask_px.as_ref().is_some_and(|m| m[i * 4] != 0);
            p[3] = if transparent { 0 } else { 255 };
        }
    }
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2); // BGRA → RGBA
    }
    RgbaImage::from_raw(w as u32, h as u32, px)
}
