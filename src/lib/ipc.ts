/**
 * 所有 Tauri command 的类型安全封装。
 *
 * 规格 00 §6.2：组件里**禁止**直接 `invoke`，一律走这里。好处是错误处理只有
 * 一个地方、command 名字只写一次、类型和 Rust 侧一一对应。
 */

import { convertFileSrc, invoke } from '@tauri-apps/api/core';

// ── 错误 ───────────────────────────────────────────────────────────────────

/** Rust 侧 `AppError` 的序列化结果（见 src-tauri/src/error.rs）。 */
interface AppErrorPayload {
  code: string;
  message: string;
  detail: string | null;
}

export class IpcError extends Error {
  constructor(
    readonly command: string,
    readonly code: string,
    message: string,
    readonly detail: string | null,
  ) {
    super(message);
    this.name = 'IpcError';
  }
}

function isAppErrorPayload(value: unknown): value is AppErrorPayload {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as AppErrorPayload).code === 'string' &&
    typeof (value as AppErrorPayload).message === 'string'
  );
}

function toIpcError(command: string, raw: unknown): IpcError {
  if (isAppErrorPayload(raw)) {
    return new IpcError(command, raw.code, raw.message, raw.detail);
  }
  // Tauri 自身的错误（权限不足、command 不存在）是纯字符串。
  return new IpcError(command, 'unknown', String(raw), null);
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (raw) {
    const error = toIpcError(command, raw);
    // M6 接入 toast 之后这里改成统一提示；现在至少保证不会静默失败。
    console.error(`[ipc] ${command} 失败: ${error.code} ${error.message}`, error.detail);
    throw error;
  }
}

// ── 截图 ───────────────────────────────────────────────────────────────────

export interface MonitorSnapshot {
  id: number;
  name: string;
  /** 虚拟桌面物理坐标，副屏在左侧/上方时为负。 */
  x: number;
  y: number;
  /** 底图真实尺寸（物理像素）。 */
  width: number;
  height: number;
  scaleFactor: number;
  isPrimary: boolean;
  /** M0 不落盘，像素走 {@link captureImageUrl}。 */
  imagePath: string | null;
}

/** 底图编码格式。由 Rust 侧的 `CHENOCR_BACKDROP_FORMAT` 环境变量决定。 */
export type BackdropFormat = 'bmp' | 'png';

export interface WindowRect {
  handle: number;
  x: number;
  y: number;
  width: number;
  height: number;
  zOrder: number;
  title: string;
  appName: string;
}

export interface CapturePrepareResult {
  monitors: MonitorSnapshot[];
  /** M1 接入窗口枚举后才有内容。 */
  windows: WindowRect[];
  capturedAt: number;
  /** 拼 {@link captureImageUrl} 时要用的格式。 */
  backdropFormat: BackdropFormat;
}

/** 物理像素矩形，原点是虚拟桌面左上角。 */
export interface Bounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type CaptureAction = 'copy' | 'save' | 'pin' | 'ocr' | 'translate' | 'longshot';

export interface CaptureFinishResult {
  action: CaptureAction;
  width: number;
  height: number;
}

export function capturePrepare(): Promise<CapturePrepareResult> {
  return call('capture_prepare');
}

/**
 * 某块屏底图的 URL。
 *
 * 走自定义协议而不是 IPC：4K 底图 33MB，过 `invoke` 拿 ArrayBuffer 再包 Blob
 * 实测 1.0–1.1 秒，而整条链路的预算是 150ms。交给 WebView 自己的网络栈取，
 * 中间那几趟 JS 堆拷贝就都没了。详见 src-tauri/src/capture/protocol.rs。
 *
 * `sessionId` 只是 cache-buster —— 同一块屏两次截图的 URL 必须不一样，
 * 否则 WebView 会把上一次的底图从缓存里捞出来。
 *
 * `format` 必须用 `capturePrepare()` 返回的那个值，不能写死：格式由 Rust 侧的
 * 环境变量决定，猜错了协议处理器会直接 400。
 */
export function captureImageUrl(
  monitorId: number,
  sessionId: number,
  format: BackdropFormat,
): string {
  return convertFileSrc(`${monitorId}-${sessionId}.${format}`, 'shot');
}

/**
 * 通知 Rust「遮罩前端已挂载、监听已注册」。
 *
 * 遮罩窗口是启动时预建的，热键可能比这个 WebView 加载完更早到。Rust 收到这个
 * 信号时如果发现已经有等着的会话，会补发一次 `capture-session-start`。
 */
export function captureOverlayBoot(monitorId: number): Promise<void> {
  return call('capture_overlay_boot', { monitorId });
}

/**
 * 通知 Rust「压暗层已就位」，由它把画面显示出来。
 *
 * 冻结画面在原生底图窗口里，所以这一步**不用等任何像素传输** —— 拿到会话元信息
 * 就可以调。`prepareMs` 只用于日志。
 */
export function captureOverlayReady(monitorId: number, prepareMs?: number): Promise<void> {
  return call('capture_overlay_ready', { monitorId, prepareMs });
}

/** 底图像素送达 WebView 的耗时，只用于日志排查。 */
export interface PixelTimings {
  /** 取字节，纯传输。 */
  fetchMs: number;
  /** 解码。 */
  decodeMs: number;
}

/**
 * 上报「底图像素已到位」，放大镜 / 取色 / 马赛克从此可用。
 *
 * 这条路已经不在"多久能看见画面"的关键路径上了，但它决定了那几个要读背景像素的
 * 功能多久之后能响应，所以照量。
 */
export function captureOverlayPixelsReady(
  monitorId: number,
  timings: PixelTimings,
): Promise<void> {
  return call('capture_overlay_pixels_ready', { monitorId, timings });
}

export function captureFinish(bounds: Bounds, action: CaptureAction): Promise<CaptureFinishResult> {
  return call('capture_finish', { bounds, action });
}

export function captureCancel(): Promise<void> {
  return call('capture_cancel');
}

export function captureTrigger(mode: 'normal' | 'longshot' | 'ocr' = 'normal'): Promise<void> {
  return call('capture_trigger', { mode });
}

// ── 系统 ───────────────────────────────────────────────────────────────────

export interface PhysicalRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface MonitorInfo {
  id: number;
  name: string;
  bounds: PhysicalRect;
  workArea: PhysicalRect;
  scaleFactor: number;
  isPrimary: boolean;
  refreshRate: number | null;
}

export interface SystemDiagnostics {
  appVersion: string;
  dataDir: string;
  darkMode: boolean;
  transparencyEnabled: boolean;
  monitors: MonitorInfo[];
}

export function getSystemDiagnostics(): Promise<SystemDiagnostics> {
  return call('get_system_diagnostics');
}

export function openDataFolder(): Promise<void> {
  return call('open_data_folder');
}

/**
 * 把错误写进 Rust 日志。
 *
 * 遮罩窗口没有可见界面也没有 devtools，异常在里面抛掉就彻底看不见了。
 * 凡是"用户只会感觉到功能没反应"的失败路径，都往这里报一份。
 */
export async function reportError(scope: string, message: string): Promise<void> {
  try {
    await invoke('report_error', { scope, message });
  } catch {
    // 连报错都报不出去就只能放弃了，不值得再套一层
  }
}
