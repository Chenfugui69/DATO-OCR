// 所有 Rust command 的类型安全封装。组件里禁止直接调 invoke（规格 01 §3）。

import { Channel, invoke } from '@tauri-apps/api/core';

import type { AiContext, AppInfo, CaptureIntent, ChatEvent, ChatRequest, ClipDetail, ClipGroup, ClipPage, ClipQuery, ClipStats, EditorAction, EditorDoc, EngineStatus, FinishAction, HotkeyStatus, OcrJob, OcrPage, PhysicalRect, PinInfo, ProviderInfo, RegionTranslation, SecretsStatus, SessionInfo, Settings, ShotPage, ShotQuery, TranslateRequest, TranslateResult, VisualCapabilities } from './types';

/** Rust 侧 AppError 的结构化形态。 */
export class AppError extends Error {
  constructor(
    public code: string,
    message: string,
  ) {
    super(message);
  }
}

function normalize(err: unknown): AppError {
  if (err instanceof AppError) return err;
  if (err && typeof err === 'object' && 'message' in err) {
    const e = err as { code?: string; message: string };
    return new AppError(e.code ?? 'error', e.message);
  }
  return new AppError('error', String(err));
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (err) {
    throw normalize(err);
  }
}

/** 二进制请求：元数据走 x-meta 头（纯 ASCII JSON），正文是原始字节。 */
async function callRaw<T>(cmd: string, meta: unknown, body: Uint8Array): Promise<T> {
  try {
    return await invoke<T>(cmd, body, { headers: { 'x-meta': JSON.stringify(meta) } });
  } catch (err) {
    throw normalize(err);
  }
}

/** 前端异常写进 Rust 日志（隐藏窗口里的异常界面上看不见）。永不抛出。 */
export function reportError(scope: string, error: unknown): void {
  const message = error instanceof Error ? `${error.message}\n${error.stack ?? ''}` : String(error);
  invoke('report_error', { scope, message }).catch(() => undefined);
}

// ───────────────────────── 截图 ─────────────────────────

export const capture = {
  start: (intent: CaptureIntent) => call<void>('capture_start', { intent }),
  sessionInfo: (monitorId: number) => call<SessionInfo | null>('capture_session_info', { monitorId }),
  overlayReady: (sessionId: number, monitorId: number) =>
    call<void>('capture_overlay_ready', { sessionId, monitorId }),
  windowChildren: (handle: number) => call<PhysicalRect[]>('capture_window_children', { handle }),
  finish: (
    meta: {
      sessionId: number;
      monitorId: number;
      rect: PhysicalRect;
      action: FinishAction;
      annotationAt: [number, number] | null;
    },
    annotationPng: Uint8Array,
  ) => callRaw<void>('capture_finish', meta, annotationPng),
  cancel: (sessionId: number) => call<void>('capture_cancel', { sessionId }),
  /** 截图原位翻译：识别选区文字、整批翻译，返回每段的位置和译文 */
  translateRegion: (request: { sessionId: number; monitorId: number; rect: PhysicalRect }) =>
    call<RegionTranslation>('capture_translate', { request }),
  writeText: (text: string) => call<void>('clipboard_write_text', { text }),
};

export const longshot = {
  setRegions: (regions: PhysicalRect[]) => call<void>('longshot_set_regions', { regions }),
  finish: () => call<void>('longshot_finish'),
  abort: () => call<void>('longshot_abort'),
  undo: () => call<void>('longshot_undo'),
};

export const pin = {
  info: (label: string) => call<PinInfo>('pin_info', { label }),
  close: (label: string) => call<void>('pin_close', { label }),
  closeAll: () => call<void>('pin_close_all'),
  copy: (label: string) => call<void>('pin_copy', { label }),
  save: (label: string) => call<void>('pin_save', { label }),
  ocr: (label: string, translate: boolean) => call<void>('pin_ocr', { label, translate }),
};

export const editor = {
  current: () => call<EditorDoc | null>('editor_current'),
  finish: (meta: { docId: string; action: EditorAction; annotationAt: [number, number] | null }, png: Uint8Array) =>
    callRaw<void>('editor_finish', meta, png),
  close: () => call<void>('editor_close'),
  openShot: (id: number) => call<void>('editor_open_shot', { id }),
};

// ───────────────────────── 剪贴板 ─────────────────────────

export const clipboard = {
  query: (query: ClipQuery) => call<ClipPage>('clipboard_query', { query }),
  get: (id: number) => call<ClipDetail>('clipboard_get', { id }),
  stats: () => call<ClipStats>('clipboard_stats'),
  paste: (id: number, plain = false) => call<void>('clipboard_paste', { id, plain }),
  copy: (id: number, plain = false) => call<void>('clipboard_copy', { id, plain }),
  remove: (ids: number[]) => call<void>('clipboard_delete', { ids }),
  setPinned: (id: number, value: boolean) => call<void>('clipboard_set_pinned', { id, value }),
  setFavorite: (id: number, value: boolean) => call<void>('clipboard_set_favorite', { id, value }),
  setNote: (id: number, note: string | null) => call<void>('clipboard_set_note', { id, note }),
  setGroup: (ids: number[], groupId: number | null) => call<void>('clipboard_set_group', { ids, groupId }),
  groups: () => call<ClipGroup[]>('clipboard_groups'),
  createGroup: (name: string) => call<number>('clipboard_create_group', { name }),
  renameGroup: (id: number, name: string) => call<void>('clipboard_rename_group', { id, name }),
  deleteGroup: (id: number) => call<void>('clipboard_delete_group', { id }),
  clear: (scope: 'unpinned' | 'everything') => call<number>('clipboard_clear', { scope }),
  hidePanel: () => call<void>('clipboard_panel_hide'),
  open: (id: number) => call<void>('clipboard_open', { id }),
};

// ───────────────────────── 截图库 / 识字 ─────────────────────────

export const library = {
  query: (query: ShotQuery) => call<ShotPage>('library_query', { query }),
  remove: (ids: number[]) => call<void>('library_delete', { ids }),
  setFavorite: (id: number, value: boolean) => call<void>('library_set_favorite', { id, value }),
  setNote: (id: number, note: string | null) => call<void>('library_set_note', { id, note }),
  copy: (id: number) => call<void>('library_copy', { id }),
  pin: (id: number) => call<void>('library_pin', { id }),
  saveAs: (id: number) => call<void>('library_save_as', { id }),
  reveal: (id: number) => call<void>('library_reveal', { id }),
  ocr: (id: number, translate = false) => call<void>('library_ocr', { id, translate }),
};

export const ocr = {
  currentJob: () => call<OcrJob | null>('ocr_current_job'),
  rerun: (jobId: string, engine: string | null, upscale: boolean) =>
    call<void>('ocr_rerun', { jobId, engine, upscale }),
  saveText: (recordId: number, text: string) => call<void>('ocr_save_text', { recordId, text }),
  status: () => call<EngineStatus>('ocr_status'),
  history: (query: { keyword?: string; offset: number; limit: number }) => call<OcrPage>('ocr_history', { query }),
  openRecord: (id: number) => call<void>('ocr_open_record', { id }),
  deleteRecord: (id: number) => call<void>('ocr_delete_record', { id }),
  fromClip: (id: number, translate = false) => call<void>('ocr_from_clip', { id, translate }),
};

export const translate = {
  text: (request: TranslateRequest) => call<TranslateResult>('translate_text', { request }),
  providers: () => call<ProviderInfo[]>('translate_providers'),
  detect: (text: string) => call<string>('translate_detect', { text }),
  popupText: () => call<string | null>('translate_popup_text'),
};

// ───────────────────────── 系统 ─────────────────────────

export const ai = {
  models: (providerId: string) => call<string[]>('ai_models', { providerId }),
  /** 开始一次回答；onEvent 收到一段段的回答，直到 done / error */
  chat: (request: ChatRequest, onEvent: (e: ChatEvent) => void) => {
    const channel = new Channel<ChatEvent>();
    channel.onmessage = onEvent;
    return call<void>('ai_chat', { request, onEvent: channel });
  },
  cancel: (id: string) => call<void>('ai_cancel', { id }),
  keys: () => call<Record<string, string>>('ai_keys'),
  setKey: (providerId: string, key: string | null) => call<void>('ai_set_key', { providerId, key }),
  open: (context: Omit<AiContext, 'seq'>) => call<void>('ai_open', { context: { ...context, seq: 0 } }),
  context: () => call<AiContext | null>('ai_context'),
};

export const system = {
  settings: () => call<Settings>('settings_get'),
  setSettings: (settings: Settings) => call<Settings>('settings_set', { settings }),
  resetSettings: () => call<Settings>('settings_reset'),
  visuals: () => call<VisualCapabilities>('visuals_get'),
  hotkeys: () => call<HotkeyStatus[]>('hotkeys_status'),
  suspendHotkeys: () => call<void>('hotkeys_suspend'),
  resumeHotkeys: () => call<HotkeyStatus[]>('hotkeys_resume'),
  validateHotkey: (accelerator: string) => call<void>('hotkey_validate', { accelerator }),
  secrets: () => call<SecretsStatus>('secrets_status'),
  setSecret: (key: 'deepl' | 'openai', value: string | null) => call<void>('secret_set', { key, value }),
  dataDir: () => call<string>('data_dir'),
  appInfo: () => call<AppInfo>('app_info'),
  openFolder: (which: 'data' | 'logs' | 'save') => call<void>('open_data_folder', { which }),
  openUrl: (url: string) => call<void>('open_url', { url }),
  pickDirectory: () => call<string | null>('pick_directory'),
  showMain: (page?: string) => call<void>('show_main', { page: page ?? null }),
  quit: () => call<void>('quit_app'),
  hideToast: () => call<void>('toast_hide'),
};
