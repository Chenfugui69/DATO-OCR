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
  Settings,
  ToastPayload,
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
  'clipboard-changed': ClipChanged;
  'clipboard-panel-show': { style: 'bottom' | 'vertical' };
  'library-changed': null;
  'ocr-job': OcrJob;
  'ocr-history-changed': null;
  toast: ToastPayload;
  navigate: string;
  'translate-request': { text: string };
  'selection-button-show': number;
  'ai-context': AiContext;
  'editor-open': EditorDoc;
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
