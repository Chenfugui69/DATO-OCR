//! 截图 / 长截图 / 贴图 / 编辑器命令。

use tauri::ipc::Request;
use tauri::AppHandle;

use super::{blocking, raw_request};
use crate::capture::{self, CaptureIntent, FinishMeta, SessionInfo};
use crate::editor::{self, EditorDoc, EditorFinish};
use crate::error::{AppError, AppResult};
use crate::pin::{self, PinInfo};
use crate::platform::{self, ClipboardPayload, MonitorId, PhysicalRect, ScannedCode, WindowHandle};
use crate::state::state;
use crate::{library, longshot, ocr, wm};

#[tauri::command]
pub async fn capture_start(app: AppHandle, intent: CaptureIntent) -> AppResult<()> {
    // 从主窗口按钮触发：主窗口别拍进去，截图结束时 `capture::end_session` 再放回来
    capture::stash_main(&app);
    capture::trigger(&app, intent);
    Ok(())
}

/// 遮罩页面加载完成时调一次：如果热键早于页面加载到达，补上会话。
#[tauri::command]
pub async fn capture_session_info(
    app: AppHandle,
    monitor_id: u64,
) -> AppResult<Option<SessionInfo>> {
    capture::session_info(&app, MonitorId(monitor_id))
}

#[tauri::command]
pub async fn capture_overlay_ready(
    app: AppHandle,
    session_id: u64,
    monitor_id: u64,
) -> AppResult<()> {
    capture::overlay_ready(&app, session_id, MonitorId(monitor_id))
}

#[tauri::command]
pub async fn capture_window_children(handle: u64) -> AppResult<Vec<PhysicalRect>> {
    platform::enumerate_children(WindowHandle(handle))
}

#[tauri::command]
pub async fn capture_finish(app: AppHandle, request: Request<'_>) -> AppResult<()> {
    let (meta, body): (FinishMeta, Vec<u8>) = raw_request(&request)?;
    blocking(move || capture::finish(&app, meta, body)).await
}

/// 截图界面里把选区识别成表格 / 公式（AI 看图），返回模型的回答（表格是 HTML，公式是 LaTeX + MathML）。
#[tauri::command]
pub async fn capture_structure(
    app: AppHandle,
    session_id: u64,
    monitor_id: u64,
    rect: PhysicalRect,
    kind: String,
    engine: Option<String>,
) -> AppResult<String> {
    let image = {
        let app = app.clone();
        blocking(move || {
            let session = state(&app)
                .capture
                .current()
                .filter(|s| s.id == session_id)
                .ok_or_else(|| AppError::msg("截图会话已结束"))?;
            let shot = session
                .monitor(MonitorId(monitor_id))
                .ok_or_else(|| AppError::msg("显示器不存在"))?;
            crate::imaging::crop_opaque(&shot.image, rect)
        })
        .await?
    };
    // 表格默认本地识别（快、不联网）；要 AI 时页面传 engine = ai
    if kind == "table" && engine.as_deref() != Some("ai") {
        return blocking(move || crate::ocr::table_local(&app, &image)).await;
    }
    crate::ocr::structure_image(&app, &image, &kind).await
}

/// 选区里看起来是表格（有格子线）还是公式：截图工具条的「识别」按钮换成对应的图标、亮个小点，点一下直接识别。
/// 返回 "table" / "formula"，都不像返回 None
#[tauri::command]
pub async fn capture_detect_structure(
    app: AppHandle,
    session_id: u64,
    monitor_id: u64,
    rect: PhysicalRect,
) -> AppResult<Option<&'static str>> {
    blocking(move || {
        // 不缩小：一像素的表格线缩一半就淡得认不出了。逐像素比一遍，几百万像素也就几十毫秒
        let image = selection_image(&app, session_id, monitor_id, rect)?;
        detect_structure(&app, &image)
    })
    .await
}

/// 一张图看起来是表格（有格子线）还是公式（截图选区、贴图共用）
fn detect_structure(app: &AppHandle, image: &image::RgbaImage) -> AppResult<Option<&'static str>> {
    if crate::ocr::table::looks_like_table(image) {
        return Ok(Some("table"));
    }
    // 公式要识字才看得出来：只看不大的图（公式一般不大），大图里有分数线才看
    let pixels = image.width() * image.height();
    if pixels > 1_500_000 && crate::ocr::formula::fraction_bars(image) == 0 {
        return Ok(None);
    }
    let result = crate::ocr::recognize(app, image, None, pixels < 400_000)?;
    Ok(crate::ocr::formula::looks_like_formula(image, &result.blocks).then_some("formula"))
}

/// 截图会话里一块屏上的选区原图
fn selection_image(
    app: &AppHandle,
    session_id: u64,
    monitor_id: u64,
    rect: PhysicalRect,
) -> AppResult<image::RgbaImage> {
    let session = state(app)
        .capture
        .current()
        .filter(|s| s.id == session_id)
        .ok_or_else(|| AppError::msg("截图会话已结束"))?;
    let shot = session
        .monitor(MonitorId(monitor_id))
        .ok_or_else(|| AppError::msg("显示器不存在"))?;
    crate::imaging::crop_opaque(&shot.image, rect)
}

/// 原位识别出来的表格：每格的位置和样式，加上底图（字抹掉的选区）和原图在图片仓库里的编号
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureTable {
    pub table: ocr::table::LaidTable,
    pub background: String,
    pub source: String,
}

/// 截图界面里「识别表格」：本地认出表格，原位盖在原来的表格上（像截图翻译那样）。
/// 两张图的编号以 `cap-<会话>-` 开头，截图结束时跟着冻结的画面一起清掉
#[tauri::command]
pub async fn capture_table(
    app: AppHandle,
    session_id: u64,
    monitor_id: u64,
    rect: PhysicalRect,
) -> AppResult<CaptureTable> {
    blocking(move || {
        let image = selection_image(&app, session_id, monitor_id, rect)?;
        laid_table(&app, image, &format!("cap-{session_id}-"))
    })
    .await
}

/// 认出表格、量好每格，底图和原图放进图片仓库（编号以 `prefix` 开头，跟着截图会话 / 贴图一起清掉）
fn laid_table(app: &AppHandle, image: image::RgbaImage, prefix: &str) -> AppResult<CaptureTable> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (table, background) = ocr::table_layout(app, &image)?;
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (bg_id, src_id) = (format!("{prefix}table{n}-bg"), format!("{prefix}table{n}-src"));
    let st = state(app);
    st.images.put(bg_id.clone(), std::sync::Arc::new(background));
    st.images.put(src_id.clone(), std::sync::Arc::new(image));
    Ok(CaptureTable {
        table,
        background: bg_id,
        source: src_id,
    })
}

/// 选区的底色和字色（原位显示公式用），再把选区原图放进图片仓库（左边给用户对照）
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionStyle {
    pub background: String,
    pub color: String,
    pub source: String,
}

#[tauri::command]
pub async fn capture_region_style(
    app: AppHandle,
    session_id: u64,
    monitor_id: u64,
    rect: PhysicalRect,
) -> AppResult<RegionStyle> {
    blocking(move || {
        let image = selection_image(&app, session_id, monitor_id, rect)?;
        Ok(region_style(&app, image, &format!("cap-{session_id}-")))
    })
    .await
}

/// 一块图的底色、字色，原图放进图片仓库（编号以 `prefix` 开头）
fn region_style(app: &AppHandle, image: image::RgbaImage, prefix: &str) -> RegionStyle {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (background, color) = ocr::table::region_colors(&image);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let source = format!("{prefix}region{n}-src");
    state(app).images.put(source.clone(), std::sync::Arc::new(image));
    RegionStyle {
        background,
        color,
        source,
    }
}

/// 截图时把鼠标下的颜色贴到屏幕上（颜色贴图）。`x`、`y` 是这块屏的局部物理坐标。
#[tauri::command]
pub async fn capture_pin_color(
    app: AppHandle,
    session_id: u64,
    monitor_id: u64,
    x: i32,
    y: i32,
    rgb: [u8; 3],
) -> AppResult<()> {
    let session = state(&app)
        .capture
        .current()
        .filter(|s| s.id == session_id)
        .ok_or_else(|| AppError::msg("截图会话已结束"))?;
    let shot = session
        .monitor(MonitorId(monitor_id))
        .ok_or_else(|| AppError::msg("显示器不存在"))?;
    let at = (shot.info.bounds.x + x, shot.info.bounds.y + y);
    pin::create_color(&app, rgb, at, shot.info.scale_factor)?;
    capture::end_session(&app, session_id, true);
    Ok(())
}

/// 遮罩收到了点击 / 按键，页面上却没有截图会话：没在截图（也没在长截图、录制）的话把它收起来。
/// 正常不会发生；万一遮罩被留在了屏幕上，点一下就能自己消失，不用再按一次截图热键。
#[tauri::command]
pub async fn capture_overlay_stray(app: AppHandle, window: tauri::WebviewWindow) -> AppResult<()> {
    let st = state(&app);
    if st.capture.current().is_some() || st.longshot.is_active() || crate::gif_record::is_active() {
        return Ok(());
    }
    tracing::warn!(label = window.label(), "没有截图会话的遮罩还在屏幕上，收起");
    app.run_on_main_thread(move || {
        let _ = window.set_ignore_cursor_events(false);
        platform::set_overlay_capturable(&window, false);
        platform::park_overlay(&window);
        platform::backdrop::release_all();
    })?;
    Ok(())
}

/// 截图框选界面元素：鼠标（屏幕物理坐标）下面的元素，由里到外
#[tauri::command]
pub async fn capture_elements_at(
    handle: u64,
    x: i32,
    y: i32,
) -> AppResult<Vec<platform::UiElement>> {
    blocking(move || platform::elements_at(WindowHandle(handle), x, y)).await
}

/// 鼠标附近的界面元素（同一行的其他格、其他行……），遮罩先存着
#[tauri::command]
pub async fn capture_elements_near(
    handle: u64,
    x: i32,
    y: i32,
) -> AppResult<Vec<platform::UiElement>> {
    blocking(move || platform::elements_near(WindowHandle(handle), x, y)).await
}

/// 识别选区里的二维码 / 条码。位置换成这块屏的局部坐标（和选区一样）。
#[tauri::command]
pub async fn capture_scan_codes(
    app: AppHandle,
    session_id: u64,
    monitor_id: u64,
    rect: PhysicalRect,
) -> AppResult<Vec<ScannedCode>> {
    blocking(move || {
        let session = state(&app)
            .capture
            .current()
            .filter(|s| s.id == session_id)
            .ok_or_else(|| AppError::msg("截图会话已结束"))?;
        let shot = session
            .monitor(MonitorId(monitor_id))
            .ok_or_else(|| AppError::msg("显示器不存在"))?;
        let image = crate::imaging::crop_opaque(&shot.image, rect)?;
        let mut codes = platform::scan_codes(&image)?;
        for c in &mut codes {
            c.x += f64::from(rect.x);
            c.y += f64::from(rect.y);
        }
        Ok(codes)
    })
    .await
}

/// 截图原位翻译：识别选区里的文字并整批翻译，返回每段的位置和译文。
#[tauri::command]
pub async fn capture_translate(
    app: AppHandle,
    request: capture::region_translate::RegionRequest,
    on_lines: tauri::ipc::Channel<Vec<[f64; 4]>>,
) -> AppResult<capture::region_translate::RegionTranslation> {
    capture::region_translate::translate_region(&app, request, Some(on_lines)).await
}

#[tauri::command]
pub async fn capture_cancel(app: AppHandle, session_id: u64) -> AppResult<()> {
    capture::end_session(&app, session_id, true);
    Ok(())
}

/// 读剪贴板里的文字（识别出的表格里粘贴：从 Excel 复制来的是制表符分隔的几行）。没有文字时给空
#[tauri::command]
pub async fn clipboard_read_text() -> AppResult<String> {
    Ok(platform::clipboard_read_text().unwrap_or_default())
}

/// 取色器复制颜色值、识字结果复制等：写一段纯文本到剪贴板（会进历史）。
#[tauri::command]
pub async fn clipboard_write_text(text: String) -> AppResult<()> {
    platform::clipboard_write(&ClipboardPayload::Text {
        text,
        html: None,
        rtf: None,
    })
}

/// 同时写纯文本和 HTML（表格：粘贴到 Excel / Word 时合并单元格、表头都还在）
#[tauri::command]
pub async fn clipboard_write_rich(text: String, html: String) -> AppResult<()> {
    platform::clipboard_write(&ClipboardPayload::Text {
        text,
        html: Some(html),
        rtf: None,
    })
}

// ───────────────────────── 长截图 ─────────────────────────

#[tauri::command]
pub async fn longshot_set_regions(app: AppHandle, regions: Vec<PhysicalRect>) -> AppResult<()> {
    longshot::set_regions(&app, regions);
    Ok(())
}

#[tauri::command]
pub async fn gif_set_regions(regions: Vec<PhysicalRect>) -> AppResult<()> {
    crate::capture::record_prep::set_regions(regions.clone());
    crate::gif_record::set_regions(regions);
    Ok(())
}

/// 进 / 出录制准备：截图框变成录制框时不压暗、露出真实桌面，遮罩除了控制条这些以外鼠标穿透
/// （见 capture/record_prep.rs）。`on = false`：退回截图编辑
#[tauri::command]
pub async fn capture_record_prep(app: AppHandle, session_id: u64, monitor_id: u64, on: bool) -> AppResult<()> {
    if on {
        crate::capture::record_prep::enter(&app, session_id, platform::MonitorId(monitor_id))
    } else {
        crate::capture::record_prep::leave(&app, true);
        Ok(())
    }
}

/// 录制中遮罩能不能被录进去（选区里画了演示用的标注时要能）
#[tauri::command]
pub async fn gif_set_capturable(app: AppHandle, on: bool) -> AppResult<()> {
    crate::gif_record::set_capturable(&app, on);
    Ok(())
}

/// 录制中拖了选区右下角的把手：录制区域挪到 `rect`（本屏局部物理坐标）
#[tauri::command]
pub async fn gif_move_to(rect: PhysicalRect) -> AppResult<()> {
    crate::gif_record::move_to(rect);
    Ok(())
}

/// 页面画好的一排字形（`0123456789:/` 这些，PNG 的 base64）：动图底下画播放时间时用。
/// `chars` 里每个字在图里的横向位置 `xs`、宽度 `widths`
#[tauri::command]
pub async fn record_glyphs(
    png: String,
    chars: String,
    xs: Vec<u32>,
    widths: Vec<u32>,
) -> AppResult<()> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png.trim_start_matches("data:image/png;base64,"))
        .map_err(|e| AppError::msg(format!("字形图不对：{e}")))?;
    let atlas = crate::imaging::decode(&bytes)?;
    let chars = chars
        .chars()
        .zip(xs)
        .zip(widths)
        .map(|((c, x), w)| (c, x, w))
        .collect();
    crate::record_fx::set_glyphs(crate::record_fx::Glyphs { atlas, chars });
    Ok(())
}

#[tauri::command]
pub async fn gif_finish() -> AppResult<()> {
    crate::gif_record::finish();
    Ok(())
}

#[tauri::command]
pub async fn gif_cancel() -> AppResult<()> {
    crate::gif_record::cancel();
    Ok(())
}

#[tauri::command]
pub async fn longshot_finish(app: AppHandle) -> AppResult<()> {
    blocking(move || {
        longshot::finish(&app);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn longshot_abort(app: AppHandle) -> AppResult<()> {
    longshot::abort(&app);
    Ok(())
}

#[tauri::command]
pub async fn longshot_undo(app: AppHandle) -> AppResult<()> {
    longshot::undo(&app);
    Ok(())
}

// ───────────────────────── 贴图 ─────────────────────────

#[tauri::command]
pub async fn pin_info(app: AppHandle, label: String) -> AppResult<PinInfo> {
    pin::info(&app, &label)
}

/// 贴图页面把图画好了，可以显示窗口了。
#[tauri::command]
pub async fn pin_ready(app: AppHandle, label: String) {
    pin::ready(&app, &label);
}

#[tauri::command]
pub async fn pin_close(app: AppHandle, label: String) -> AppResult<()> {
    let ui = app.clone();
    app.run_on_main_thread(move || pin::close(&ui, &label))?;
    Ok(())
}

#[tauri::command]
pub async fn pin_close_all(app: AppHandle) -> AppResult<()> {
    let ui = app.clone();
    app.run_on_main_thread(move || pin::close_all(&ui))?;
    Ok(())
}

fn pin_image(app: &AppHandle, label: &str) -> AppResult<image::RgbaImage> {
    state(app)
        .pins
        .image(label)
        .map(|img| (*img).clone())
        .ok_or_else(|| AppError::NotFound("贴图".into()))
}

#[tauri::command]
pub async fn pin_copy(app: AppHandle, label: String) -> AppResult<()> {
    blocking(move || {
        let image = pin_image(&app, &label)?;
        platform::clipboard_write(&ClipboardPayload::Image { image, png: None })?;
        wm::toast(&app, "success", "已复制到剪贴板");
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn pin_save(app: AppHandle, label: String) -> AppResult<()> {
    blocking(move || {
        let image = pin_image(&app, &label)?;
        let settings = state(&app).settings.read().capture.clone();
        if let Some(path) = capture::ask_save_path(&app, &settings)? {
            capture::write_image_file(&image, &path, &settings)?;
            wm::toast(&app, "success", "已保存");
        }
        Ok(())
    })
    .await
}

/// 旋转 / 翻转贴图（op：rotateCw、rotateCcw、flipH、flipV），返回新的尺寸
#[tauri::command]
pub async fn pin_transform(app: AppHandle, label: String, op: String) -> AppResult<PinInfo> {
    blocking(move || pin::transform(&app, &label, &op)).await
}

/// 恢复最近关掉的一张贴图，没有可恢复的返回 false
#[tauri::command]
pub async fn pin_restore_last(app: AppHandle) -> AppResult<bool> {
    blocking(move || pin::restore_last(&app)).await
}

/// 把贴图送进编辑器画标注
#[tauri::command]
pub async fn pin_edit(app: AppHandle, label: String) -> AppResult<()> {
    blocking(move || pin::edit(&app, &label)).await
}

/// 贴图里的二维码 / 条码
#[tauri::command]
pub async fn pin_scan_codes(app: AppHandle, label: String) -> AppResult<Vec<ScannedCode>> {
    blocking(move || platform::scan_codes(&pin_image(&app, &label)?)).await
}

/// 贴图看起来是表格还是公式（贴图右下角的小提示）
#[tauri::command]
pub async fn pin_detect_structure(app: AppHandle, label: String) -> AppResult<Option<&'static str>> {
    blocking(move || detect_structure(&app, &pin_image(&app, &label)?)).await
}

/// 贴图里的表格：认出来、量好每格，像截图里一样原位盖在图上能改（图片编号跟着贴图一起清）
#[tauri::command]
pub async fn pin_table(app: AppHandle, label: String) -> AppResult<CaptureTable> {
    blocking(move || {
        let image = pin_image(&app, &label)?;
        laid_table(&app, image, &format!("{label}-"))
    })
    .await
}

/// 贴图用 AI 认表格 / 公式（表格是 HTML，公式是 LaTeX + MathML）
#[tauri::command]
pub async fn pin_structure(app: AppHandle, label: String, kind: String) -> AppResult<String> {
    let image = {
        let app = app.clone();
        let label = label.clone();
        blocking(move || pin_image(&app, &label)).await?
    };
    crate::ocr::structure_image(&app, &image, &kind).await
}

/// 贴图的底色、字色（公式原位显示时铺底、上字）
#[tauri::command]
pub async fn pin_region_style(app: AppHandle, label: String) -> AppResult<RegionStyle> {
    blocking(move || {
        let image = pin_image(&app, &label)?;
        Ok(region_style(&app, image, &format!("{label}-")))
    })
    .await
}

/// 贴图里的文字和位置（鼠标直接选字用）
#[tauri::command]
pub async fn pin_text(app: AppHandle, label: String) -> AppResult<Vec<pin::PinText>> {
    blocking(move || pin::text(&app, &label).map(|t| (*t).clone())).await
}

#[tauri::command]
pub async fn pin_ocr(app: AppHandle, label: String, translate: bool) -> AppResult<()> {
    blocking(move || {
        let image = pin_image(&app, &label)?;
        ocr::open_job(&app, image, None, translate)
    })
    .await
}

// ───────────────────────── 编辑器 ─────────────────────────

#[tauri::command]
pub async fn editor_current() -> AppResult<Option<EditorDoc>> {
    Ok(editor::current())
}

#[tauri::command]
pub async fn editor_finish(app: AppHandle, request: Request<'_>) -> AppResult<()> {
    let (meta, body): (EditorFinish, Vec<u8>) = raw_request(&request)?;
    blocking(move || editor::finish(&app, meta, body)).await
}

#[tauri::command]
pub async fn editor_close(app: AppHandle) -> AppResult<()> {
    let ui = app.clone();
    app.run_on_main_thread(move || editor::close(&ui))?;
    Ok(())
}

#[tauri::command]
pub async fn editor_open_shot(app: AppHandle, id: i64) -> AppResult<()> {
    blocking(move || {
        let image = library::load_image(&app, id)?;
        editor::open(&app, image, Some(id)).map(drop)
    })
    .await
}

// ───────────────────────── 录完剪辑 ─────────────────────────

/// 剪辑窗口要的信息：待剪辑的那段录制（时长、尺寸、缩略图、临时文件）
#[tauri::command]
pub async fn record_edit_info() -> AppResult<crate::record_edit::EditInfo> {
    crate::record_edit::info()
}

/// 按剪辑保存：`start_ms`..`end_ms` 一段、`speed` 倍速（导出要一会儿，放后台线程）
#[tauri::command]
pub async fn record_edit_save(
    app: AppHandle,
    id: u64,
    start_ms: u64,
    end_ms: u64,
    speed: f64,
) -> AppResult<()> {
    blocking(move || {
        crate::record_edit::save(
            &app,
            id,
            Some(platform::VideoEdit {
                start_ms,
                end_ms,
                speed,
            }),
        )
    })
    .await
}

#[tauri::command]
pub async fn record_edit_discard(app: AppHandle, id: u64) -> AppResult<()> {
    crate::record_edit::discard(&app, id);
    Ok(())
}

/// 麦克风 / 摄像头的授权状态
#[tauri::command]
pub async fn media_access(kind: platform::MediaKind) -> AppResult<platform::MediaAccess> {
    Ok(platform::media_access(kind))
}

/// 还没问过就弹系统的授权框，等用户点完
#[tauri::command]
pub async fn media_request(kind: platform::MediaKind) -> AppResult<platform::MediaAccess> {
    blocking(move || Ok(platform::request_media_access(kind))).await
}

// ───────────────────────── 自己画的右键菜单（贴图用） ─────────────────────────

/// 弹出菜单：`owner` 是要菜单的窗口，选了哪项发 `popmenu-select` 给它
#[tauri::command]
pub async fn popmenu_open(app: AppHandle, owner: String, items: serde_json::Value) -> AppResult<()> {
    crate::popmenu::open(&app, owner, items)
}

/// 菜单页面来取要显示的菜单项
#[tauri::command]
pub async fn popmenu_items() -> AppResult<Option<crate::popmenu::Pending>> {
    Ok(crate::popmenu::items())
}

/// 菜单页面量好了自己多大（CSS 像素）：摆到鼠标那里显示
#[tauri::command]
pub async fn popmenu_place(app: AppHandle, width: f64, height: f64) -> AppResult<()> {
    crate::popmenu::place(&app, width, height)
}

/// 选了一项
#[tauri::command]
pub async fn popmenu_pick(app: AppHandle, id: String) -> AppResult<()> {
    crate::popmenu::pick(&app, id);
    Ok(())
}

/// 不选了（点别处、Esc）
#[tauri::command]
pub async fn popmenu_close(app: AppHandle) -> AppResult<()> {
    crate::popmenu::hide(&app);
    Ok(())
}
