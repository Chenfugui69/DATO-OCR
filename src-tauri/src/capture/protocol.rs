//! `shot:` 自定义协议 —— 把底图直接喂给 WebView 的图片解码器。
//!
//! # 为什么不走 IPC
//!
//! 最初的做法是 `capture_get_image` command 返回 BMP 字节，前端包成 Blob URL
//! 再交给 `<img>`。4K 单屏实测这一步要 **825–925ms**，另加 190–220ms 在 JS 里
//! 解码 —— 而热键到上屏的总预算是 150ms（规格 00 §6.4）。
//!
//! 慢的原因是那 33MB 被反复搬：Rust 编好的 `Vec<u8>` → IPC 响应体 →
//! JS 的 `ArrayBuffer` → `new Blob([bytes])` 又抄一份 → 解码器再读一遍。
//!
//! 换成自定义协议之后，`<img src="shot://…">` 由 WebView 自己的网络栈取字节、
//! 直接进解码器，中间那几趟 JS 堆上的拷贝全部消失，也不用等 `invoke` 的
//! 往返。前端那边连 `Blob` 都不需要了。
//!
//! # 4K 单屏实测（release，3840x2160 @150%，取热态三轮的稳定值）
//!
//! | 阶段 | BMP 32bpp | BMP 24bpp | PNG Fast/NoFilter |
//! |---|---|---|---|
//! | 字节数 | 33.18MB | 24.88MB | 33.18MB |
//! | Rust 编码 | 8ms | 7ms | 59ms |
//! | 传输到 WebView | 154ms | 134ms | 204ms |
//! | 解码 | 35ms | 28ms | 26ms |
//!
//! 两个结论：
//!
//! 1. **PNG 是死路**。这块屏上摊着照片壁纸，不做滤波时 zlib 一级压缩基本压不动
//!    （33.18MB，比 BMP 还大），却白搭 51ms 编码。
//! 2. **传输不是带宽瓶颈**。砍掉 25% 字节只换回 13% 时间；拿两个字节数拟合出来
//!    约是「74ms 固定开销 + 2.4ms/MB」。固定开销那部分和图多大无关，估计是
//!    Chromium 的 URLLoader 按 64KB 分块读 IStream，25MB 要跑近 400 个来回。
//!
//! 所以这条路的地板大约是 74ms 传输 + 28ms 解码，加上抓屏和 show 就顶到 150ms
//! 预算之外了 —— 无损压缩再怎么折腾都跨不过去。真要达标得让底图根本不进 WebView，
//! 见 `docs/notes/` 里关于原生底图层的记录。
//! # URL 形状
//!
//! `<monitor_id>-<session_id>.<ext>`，例如
//! `shot://localhost/65537-1755940049123.bmp`（Windows 上实际是
//! `http://shot.localhost/...`）。
//!
//! - 会话号是 cache-buster：同一块屏在两次截图之间 URL 必须不同，
//!   否则 WebView 会把上一次的底图从缓存里捞出来。
//! - 扩展名决定编码格式，见 [`super::backdrop`]。

use tauri::http::{Request, Response, StatusCode};
use tauri::{AppHandle, UriSchemeContext, UriSchemeResponder, Wry};

use super::backdrop::Format;
use crate::platform::MonitorId;

pub const SCHEME: &str = "shot";

pub fn handle(
    ctx: UriSchemeContext<'_, Wry>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    let path = request.uri().path().trim_start_matches('/').to_owned();

    // 编 BMP 要搬 33MB，别在事件循环线程上干 —— 那条线程同时也在处理窗口消息。
    std::thread::spawn(move || {
        responder.respond(build(&app, &path));
    });
}

fn build(app: &AppHandle, path: &str) -> Response<Vec<u8>> {
    let started = std::time::Instant::now();

    let Some((monitor, format)) = parse(path) else {
        tracing::warn!(path, "shot 协议收到无法解析的路径");
        return error(StatusCode::BAD_REQUEST);
    };

    match super::image_bytes(app, monitor, format) {
        Ok(bytes) => {
            tracing::info!(
                path,
                bytes = bytes.len(),
                encode_ms = started.elapsed().as_millis(),
                "shot 协议已交付底图"
            );
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", format.mime())
                // 底图只在本次会话里有意义，URL 又带了会话号，没有任何复用价值。
                .header("Cache-Control", "no-store")
                // 页面在 tauri.localhost，这里是 shot.localhost，属于跨源。
                // `<img>` 不校验 CORS，但 `fetch()` 会 —— 少了这个头，
                // 前端只会拿到一句没有细节的 "Failed to fetch"。
                .header("Access-Control-Allow-Origin", "*")
                .body(bytes)
                // builder 只会在 header 名字非法时出错，这里全是字面量。
                .unwrap_or_else(|_| error(StatusCode::INTERNAL_SERVER_ERROR))
        }
        Err(err) => {
            tracing::warn!(path, "shot 协议取底图失败: {err}");
            error(StatusCode::NOT_FOUND)
        }
    }
}

/// 从 `<monitor_id>-<session_id>.<ext>` 里取出显示器号和格式。
fn parse(path: &str) -> Option<(MonitorId, Format)> {
    let (stem, ext) = path.rsplit_once('.')?;
    let monitor = stem.split('-').next()?.parse::<u64>().ok()?;
    Some((MonitorId(monitor), Format::from_extension(ext)?))
}

/// 不用 `Response::builder()`：那条路径的 `body()` 返回 `Result`，只能 expect 掉。
/// 直接构造再改 status 是完全无错的，`status_mut` 不返回 `Result`。
fn error(status: StatusCode) -> Response<Vec<u8>> {
    let mut response = Response::new(Vec::new());
    *response.status_mut() = status;
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析显示器号与格式() {
        assert_eq!(
            parse("65537-1755940049123.bmp"),
            Some((MonitorId(65537), Format::Bmp))
        );
        assert_eq!(
            parse("65537-1755940049123.png"),
            Some((MonitorId(65537), Format::Png))
        );
        // 没带会话号也得能用，方便手动敲 URL 调试。
        assert_eq!(parse("65537.bmp"), Some((MonitorId(65537), Format::Bmp)));

        assert_eq!(parse(""), None);
        assert_eq!(parse("65537"), None, "缺扩展名");
        assert_eq!(parse("abc-1.bmp"), None, "显示器号不是数字");
        assert_eq!(parse("65537-1.jpg"), None, "不支持的格式");
    }
}
