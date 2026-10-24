//! APNG（会动的 PNG）：录屏选 APNG 时用。和 GIF 比，颜色是全彩的（GIF 只有 256 色），浏览器、聊天软件也都认。
//!
//! 边录边写：每帧只写和上一帧比变了的那一块（块里没变的像素透明，叠在上一帧上），帧的图像数据用 `png`
//! 压一张小 PNG、取出它的 IDAT 数据装进 fdAT。动画控制块（acTL）里要写总帧数，录的时候还不知道，
//! 先写 0，写完再回到那个位置改掉（连同校验码）。
//!
//! 也能一帧帧读回来（剪辑、加进度条时重新编码用），按 dispose / blend 叠成完整的画面。

use std::fs::File;
use std::io::{BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use image::RgbaImage;

use crate::error::{AppError, AppResult};

fn png_err(err: impl std::fmt::Display) -> AppError {
    AppError::msg(format!("APNG 编码失败：{err}"))
}

/// PNG 块的校验码（CRC-32，多项式 0xEDB88320）
fn crc32(parts: &[&[u8]]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *v = c;
        }
        t
    });
    let mut c = 0xFFFF_FFFFu32;
    for part in parts {
        for &b in *part {
            c = table[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    c ^ 0xFFFF_FFFF
}

fn write_chunk(out: &mut impl Write, kind: &[u8; 4], data: &[u8]) -> std::io::Result<()> {
    out.write_all(&(data.len() as u32).to_be_bytes())?;
    out.write_all(kind)?;
    out.write_all(data)?;
    out.write_all(&crc32(&[kind, data]).to_be_bytes())
}

/// 一块 RGBA 压成 PNG 的图像数据（zlib 流，就是 IDAT 里的那部分）
fn compress(width: u32, height: u32, rgba: &[u8]) -> AppResult<Vec<u8>> {
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().map_err(png_err)?;
        writer.write_image_data(rgba).map_err(png_err)?;
        writer.finish().map_err(png_err)?;
    }
    // 跳过文件头，把所有 IDAT 的数据接起来
    let mut data = Vec::new();
    let mut i = 8;
    while i + 8 <= png.len() {
        let len = u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]) as usize;
        let kind = &png[i + 4..i + 8];
        if kind == b"IDAT" {
            data.extend_from_slice(&png[i + 8..i + 8 + len]);
        }
        i += 12 + len;
    }
    Ok(data)
}

/// 一帧：画布上的位置、大小和像素（RGBA，透明的地方露出上一帧）
pub struct ApngFrame {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct ApngWriter {
    out: BufWriter<File>,
    /// acTL 块的数据在文件里从哪开始（写完回来改帧数）
    actl_at: u64,
    plays: u32,
    seq: u32,
    frames: u32,
}

impl ApngWriter {
    /// `looping`：循环播放（否则播一遍停在最后一帧）
    pub fn create(path: &Path, width: u32, height: u32, looping: bool) -> AppResult<Self> {
        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])?;
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        // 8 位、RGBA、标准压缩 / 过滤、不隔行
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        write_chunk(&mut out, b"IHDR", &ihdr)?;
        // 签名 8 + IHDR 25，acTL 的数据在长度和类型之后
        let actl_at = 8 + 25 + 8;
        let plays: u32 = if looping { 0 } else { 1 };
        let mut actl = [0u8; 8];
        actl[4..].copy_from_slice(&plays.to_be_bytes());
        write_chunk(&mut out, b"acTL", &actl)?;
        Ok(Self {
            out,
            actl_at,
            plays,
            seq: 0,
            frames: 0,
        })
    }

    /// 写一帧，显示 `delay_ms` 毫秒。第一帧必须是整张画布
    pub fn write(&mut self, frame: &ApngFrame, delay_ms: u64) -> AppResult<()> {
        let first = self.frames == 0;
        let mut fctl = Vec::with_capacity(26);
        fctl.extend_from_slice(&self.seq.to_be_bytes());
        for v in [frame.width, frame.height, frame.x, frame.y] {
            fctl.extend_from_slice(&v.to_be_bytes());
        }
        // 延迟按毫秒写（分母 1000），一帧最长 65 秒
        fctl.extend_from_slice(&(delay_ms.clamp(1, 65_535) as u16).to_be_bytes());
        fctl.extend_from_slice(&1000u16.to_be_bytes());
        // 不清掉（留着给下一帧叠）；第一帧直接覆盖，之后的按透明度叠上去
        fctl.push(0);
        fctl.push(if first { 0 } else { 1 });
        write_chunk(&mut self.out, b"fcTL", &fctl)?;
        self.seq += 1;
        let data = compress(frame.width, frame.height, &frame.rgba)?;
        // 一块最多写 1MB，太长的拆开
        for piece in data.chunks(1 << 20) {
            if first {
                write_chunk(&mut self.out, b"IDAT", piece)?;
            } else {
                let mut fdat = Vec::with_capacity(4 + piece.len());
                fdat.extend_from_slice(&self.seq.to_be_bytes());
                fdat.extend_from_slice(piece);
                write_chunk(&mut self.out, b"fdAT", &fdat)?;
                self.seq += 1;
            }
        }
        self.frames += 1;
        Ok(())
    }

    /// 写完：文件尾，再回去把总帧数填进 acTL
    pub fn finish(mut self) -> AppResult<u32> {
        if self.frames == 0 {
            return Err(AppError::msg("没有录到画面"));
        }
        write_chunk(&mut self.out, b"IEND", &[])?;
        let mut actl = [0u8; 8];
        actl[..4].copy_from_slice(&self.frames.to_be_bytes());
        actl[4..].copy_from_slice(&self.plays.to_be_bytes());
        let crc = crc32(&[b"acTL", &actl]);
        self.out.seek(SeekFrom::Start(self.actl_at))?;
        self.out.write_all(&actl)?;
        self.out.write_all(&crc.to_be_bytes())?;
        self.out.flush()?;
        Ok(self.frames)
    }
}

/// 一帧帧读回来：每帧叠好之后的整张画面和它显示多久（毫秒）。`f` 返回 false 就不再读了
pub fn read_frames(path: &Path, mut f: impl FnMut(&RgbaImage, u64) -> bool) -> AppResult<()> {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().map_err(png_err)?;
    let (w, h) = (reader.info().width, reader.info().height);
    let frames = reader
        .info()
        .animation_control
        .map_or(1, |a| a.num_frames)
        .max(1);
    let mut canvas = RgbaImage::new(w, h);
    let mut buf = vec![
        0u8;
        reader
            .output_buffer_size()
            .ok_or_else(|| AppError::msg("APNG 太大"))?
    ];
    for _ in 0..frames {
        let out = match reader.next_frame(&mut buf) {
            Ok(o) => o,
            Err(_) => break,
        };
        let fc = reader.info().frame_control;
        let (fx, fy, fw, fh, delay, blend, dispose) = match fc {
            Some(c) => {
                let den = if c.delay_den == 0 {
                    100
                } else {
                    u64::from(c.delay_den)
                };
                (
                    c.x_offset,
                    c.y_offset,
                    c.width,
                    c.height,
                    u64::from(c.delay_num) * 1000 / den,
                    c.blend_op,
                    c.dispose_op,
                )
            }
            None => (0, 0, w, h, 100, png::BlendOp::Source, png::DisposeOp::None),
        };
        let before = (dispose == png::DisposeOp::Previous).then(|| canvas.clone());
        let line = out.line_size;
        for y in 0..fh.min(h.saturating_sub(fy)) {
            for x in 0..fw.min(w.saturating_sub(fx)) {
                let i = y as usize * line + x as usize * 4;
                let src = [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]];
                let dst = canvas.get_pixel_mut(fx + x, fy + y);
                if blend == png::BlendOp::Source || src[3] == 255 {
                    dst.0 = src;
                } else if src[3] > 0 {
                    let a = u32::from(src[3]);
                    for (d, s) in dst.0.iter_mut().zip(src).take(3) {
                        *d = ((u32::from(s) * a + u32::from(*d) * (255 - a)) / 255) as u8;
                    }
                    dst.0[3] = dst.0[3].max(src[3]);
                }
            }
        }
        if !f(&canvas, delay) {
            break;
        }
        match dispose {
            png::DisposeOp::Background => {
                for y in fy..(fy + fh).min(h) {
                    for x in fx..(fx + fw).min(w) {
                        canvas.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
                    }
                }
            }
            png::DisposeOp::Previous => {
                if let Some(b) = before {
                    canvas = b;
                }
            }
            png::DisposeOp::None => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_an_animation_that_reads_back() {
        let dir = std::env::temp_dir().join(format!("dato-apng-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.png");
        let mut w = ApngWriter::create(&path, 4, 3, true).unwrap();
        w.write(
            &ApngFrame {
                x: 0,
                y: 0,
                width: 4,
                height: 3,
                rgba: [10, 20, 30, 255].repeat(12),
            },
            120,
        )
        .unwrap();
        // 第二帧只改右下角那个像素，别处透明
        w.write(
            &ApngFrame {
                x: 3,
                y: 2,
                width: 1,
                height: 1,
                rgba: vec![200, 0, 0, 255],
            },
            300,
        )
        .unwrap();
        assert_eq!(w.finish().unwrap(), 2);
        let mut got = Vec::new();
        read_frames(&path, |img, delay| {
            got.push((img.get_pixel(0, 0).0, img.get_pixel(3, 2).0, delay));
            true
        })
        .unwrap();
        assert_eq!(
            got,
            vec![
                ([10, 20, 30, 255], [10, 20, 30, 255], 120),
                ([10, 20, 30, 255], [200, 0, 0, 255], 300)
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
