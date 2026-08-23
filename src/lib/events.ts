/**
 * 所有 Tauri 事件的名字与载荷类型（规格 01 §3.2）。
 *
 * 事件名格式：`域-动作`，kebab-case。
 * 这里只列**已经实现**的事件，其余随各自里程碑补上。
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type CaptureMode = 'normal' | 'longshot' | 'ocr';

export interface CaptureHotkeyPayload {
  mode: CaptureMode;
}

/**
 * 发给单个遮罩窗口，告诉它新会话开始了、去拉自己那块屏的底图。
 *
 * 遮罩窗口是常驻复用的（见 `src-tauri/src/capture/overlay.rs` 的文件头），
 * 所以"该干活了"这件事只能靠事件通知 —— 组件的 mount 只在启动预建时发生一次。
 */
export interface CaptureSessionStartPayload {
  /** 会话时间戳，用于丢弃迟到的上一轮响应。 */
  sessionId: number;
  monitor: number;
}

interface EventMap {
  'capture-hotkey': CaptureHotkeyPayload;
  'capture-session-start': CaptureSessionStartPayload;
  /** 会话结束，遮罩该把底图放掉。窗口不销毁，所以得显式回收。 */
  'capture-session-end': null;
}

export function on<K extends keyof EventMap>(
  event: K,
  handler: (payload: EventMap[K]) => void,
): Promise<UnlistenFn> {
  return listen<EventMap[K]>(event, ({ payload }) => handler(payload));
}
