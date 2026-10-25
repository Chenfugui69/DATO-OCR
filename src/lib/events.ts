// Rust → 前端事件（名称与 src-tauri/src/events.rs 一致）。

import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useEffect, useRef } from 'react';

import type {
  AiContext,
  ClipChanged,
  EditorDoc,
  GifProgress,
  GifStateEvent,
  LongshotProgress,
  LongshotStateEvent,
  OcrJob,
  PaddleDownload,
  PopItem,
  Settings,
  ToastPayload,
  UpdateStatus,
  VisualCapabilities,
} from './types';

export interface EventMap {
  'settings-changed': Settings;
  'visuals-changed': VisualCapabilities;
  'capture-session-start': { sessionId: number };
  'capture-session-end': null;
  'capture-hotkey': 'normal' | 'ocr' | 'longshot' | 'translate';
  /** 遮罩之间互相通知：哪块屏上已经有选区了（null = 没有） */
  'capture-active-monitor': { sessionId: number; monitorId: number | null };
  'longshot-state': LongshotStateEvent;
  'longshot-progress': LongshotProgress;
  'gif-state': GifStateEvent;
  'gif-progress': GifProgress;
  /** 录制时按下了一个键（按键显示用）：⌘C、⏎…… */
  'record-key': string;
  /** 本程序要截图了：开着毛玻璃的面板换成实心底（true）/ 换回来（false），见 `useGlassSnapshot` */
  'glass-snapshot': boolean;
  /** 录制准备开始 / 结束：所有遮罩都不压暗、鼠标穿透（monitorId 是录制框所在的屏） */
  'record-prep': { active: boolean; monitorId: number };
  /** 录制准备时的鼠标（遮罩穿透了收不到）：位置（本屏局部物理坐标，不在这块屏上是 null）、按着的键、这一拍滚了多少 */
  'record-prep-pointer': { at: [number, number] | null; buttons: number; scroll: number };
  /** 自己画的右键菜单：要显示的菜单项（菜单窗口收）/ 选了哪一项（要菜单的窗口收） */
  'popmenu-items': { owner: string; items: PopItem[] };
  'popmenu-select': string;
  'clipboard-changed': ClipChanged;
  'clipboard-panel-show': { style: 'bottom' | 'vertical' };
  /** 大预览窗口要显示哪一条 */
  'clip-preview': number;
  /** 多端同步的状态变了，重新拉一次 */
  'sync-changed': null;
  'update-status': UpdateStatus;
  'library-changed': null;
  'ocr-job': OcrJob;
  'ocr-engine-download': PaddleDownload;
  /** 本地公式识别模型的下载进度 */
  'ocr-latex-download': PaddleDownload;
  'ocr-history-changed': null;
  toast: ToastPayload;
  navigate: string;
  'translate-request': { text: string; reserve?: number; preview?: boolean; input?: boolean };
  'translate-outside': null;
  'translate-preview-closed': null;
  'selection-button-show': { generation: number; dark: boolean | null; backdrop: string | null };
  'ai-context': AiContext;
  'editor-open': EditorDoc;
  /** 剪辑窗口：又录完一段要剪（这段的 id） */
  'record-edit': number;
}

export type EventName = keyof EventMap;

export function on<K extends EventName>(name: K, handler: (payload: EventMap[K]) => void): Promise<UnlistenFn> {
  return listen<EventMap[K]>(name, (e) => handler(e.payload));
}

export function broadcast<K extends EventName>(name: K, payload: EventMap[K]): Promise<void> {
  return emit(name, payload);
}

/** 订阅一个事件，组件卸载时自动退订。handler 可以随渲染变化，不会重复订阅。 */
export function useEvent<K extends EventName>(name: K, handler: (payload: EventMap[K]) => void): void {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    on(name, (p) => ref.current(p))
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [name]);
}
