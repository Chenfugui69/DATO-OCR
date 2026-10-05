// 与 Rust 侧序列化结构一一对应（serde rename_all = camelCase）。

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
}

export interface WindowInfo {
  handle: number;
  bounds: PhysicalRect;
  zOrder: number;
  title: string;
  appName: string;
  processId: number;
  /** 子控件矩形：截图开始时（遮罩出现之前）快照的 */
  children: PhysicalRect[];
}

// ───────────────────────── 设置 ─────────────────────────

export interface Settings {
  version: number;
  general: {
    language: string;
    autoStart: boolean;
    closeToTray: boolean;
    offlineMode: boolean;
  };
  appearance: {
    theme: 'system' | 'light' | 'dark';
    glassEffect: 'auto' | 'on' | 'off';
  };
  hotkeys: {
    capture: string;
    longshot: string;
    ocr: string;
    clipboard: string;
    translate: string;
    instant: string;
    ai: string;
  };
  capture: CaptureSettings;
  longshot: {
    scrollDebounceMs: number;
    maxHeight: number;
    detectFixedHeader: boolean;
  };
  ocr: {
    engine: 'rapid' | 'system';
    idleTimeoutMinutes: number;
    keepLineBreaks: boolean;
  };
  translate: {
    targetLanguage: string;
    /** 全部翻译源及顺序，排第一的可用源是默认源 */
    providers: { id: ProviderId; enabled: boolean }[];
    showAllProviders: boolean;
    customBaseUrl: string;
    customModel: string;
    popup: PopupStyle;
    selection: {
      showButton: boolean;
      buttonPosition: 'bottomRight' | 'topRight' | 'bottomLeft' | 'topLeft';
      modifier: 'none' | 'alt' | 'ctrl';
    };
  };
  network: {
    proxyMode: 'system' | 'none' | 'custom';
    proxyUrl: string;
  };
  ai: AiSettings;
  clipboard: {
    enabled: boolean;
    panelStyle: 'bottom' | 'vertical';
    maxTextMb: number;
    maxImageMb: number;
    respectPrivacyFlag: boolean;
    useBlacklist: boolean;
    blacklist: string[];
    retentionDays: number;
    retentionMaxItems: number;
  };
}

export interface PopupStyle {
  width: number;
  /** 0 = 自动 */
  height: number;
  fontSize: number;
  opacity: number;
  radius: number;
  showSource: boolean;
  /** 问 AI 时对话区怎么出来 */
  aiLayout: 'drawer' | 'side';
  drawerHeight: number;
  /** 毛玻璃 */
  blur: boolean;
}

export interface FrameStyle {
  /** 'accent' = 跟随主题色，否则 #RRGGBB */
  color: string;
  width: number;
  style: 'solid' | 'dashed' | 'dotted';
  radius: number;
}

export interface CaptureSettings {
  maskOpacity: number;
  ocrMaskOpacity: number;
  ocrInstant: boolean;
  instantAction: 'select' | 'copy' | 'save';
  instantCursor: boolean;
  gifFps: number;
  gifCursor: boolean;
  frame: FrameStyle;
  ocrFrame: FrameStyle;
  showMagnifier: boolean;
  colorFormat: 'hex' | 'rgb' | 'hsl';
  detectWindows: boolean;
  detectChildWindows: boolean;
  snapThreshold: number;
  rightClick: 'exit' | 'cancelSelection';
  finishAction: 'copy' | 'copyAndSave';
  saveToLibrary: boolean;
  saveDirectory: string | null;
  fileNameTemplate: string;
  imageFormat: 'png' | 'jpg';
  jpgQuality: number;
}

export interface VisualCapabilities {
  glass: boolean;
  backdrop: 'mica' | 'none';
  darkMode: boolean;
  reducedMotion: boolean;
  transparencyEnabled: boolean;
  powerSaver: boolean;
}

export type HotkeyAction = 'capture' | 'longshot' | 'ocr' | 'clipboard' | 'translate' | 'instant' | 'ai';

export interface HotkeyStatus {
  action: HotkeyAction;
  accelerator: string;
  ok: boolean;
  error: string | null;
}

export interface SecretsStatus {
  deepl: string | null;
  openai: string | null;
}

export interface EngineStatus {
  rapid: boolean;
  rapidRunning: boolean;
  system: boolean;
  avx: boolean;
}

export interface AppInfo {
  version: string;
  dataDir: string;
  saveDir: string;
  ocr: EngineStatus;
  dataBytes: number;
}

// ───────────────────────── 截图 ─────────────────────────

export type CaptureIntent = 'normal' | 'ocr' | 'longshot' | 'translate';
export type FinishAction = 'copy' | 'save' | 'pin' | 'ocr' | 'translate' | 'longshot' | 'ai' | 'gif';

export interface SessionInfo {
  sessionId: number;
  intent: CaptureIntent;
  monitor: MonitorInfo;
  imageUrlId: string;
  windows: WindowInfo[];
  cursor: [number, number] | null;
  hasFocus: boolean;
  settings: CaptureSettings;
}

/** GIF 录制开始 / 结束（rect 是本屏局部物理坐标） */
export interface GifStateEvent {
  active: boolean;
  monitorId: number;
  rect: PhysicalRect;
}

export interface GifProgress {
  elapsedMs: number;
  maxMs: number;
  frames: number;
}

export interface LongshotStateEvent {
  active: boolean;
  monitorId: number;
  rect: PhysicalRect;
}

export type LongshotStatus = 'first' | 'added' | 'revisited' | 'bottom' | 'failed' | 'toolong' | 'undone';

export interface LongshotProgress {
  frames: number;
  width: number;
  height: number;
  status: LongshotStatus;
  failures: number;
  previewVersion: number;
  seam: number | null;
}

export interface PinInfo {
  imageId: string;
  width: number;
  height: number;
  scale: number;
  margin: number;
}

export interface EditorDoc {
  id: string;
  imageId: string;
  width: number;
  height: number;
  screenshotId: number | null;
}

export type EditorAction = 'copy' | 'save' | 'pin' | 'ocr' | 'translate' | 'ai';

// ───────────────────────── 剪贴板 ─────────────────────────

export type ClipType = 'text' | 'link' | 'color' | 'image' | 'files';

export interface ClipItem {
  id: number;
  type: ClipType;
  preview: string | null;
  hasHtml: boolean;
  filePath: string | null;
  thumbPath: string | null;
  files: string[];
  charCount: number | null;
  sizeBytes: number | null;
  width: number | null;
  height: number | null;
  sourceApp: string | null;
  sourceIcon: string | null;
  truncated: boolean;
  pinned: boolean;
  favorite: boolean;
  note: string | null;
  groupId: number | null;
  createdAt: number;
  lastUsedAt: number;
}

export interface ClipDetail extends ClipItem {
  contentText: string | null;
  contentHtml: string | null;
  contentRtf: string | null;
  sourceAppPath: string | null;
}

export interface ClipQuery {
  keyword?: string;
  kinds?: ClipType[];
  pinnedOnly?: boolean;
  favoriteOnly?: boolean;
  groupId?: number | null;
  cursor?: string | null;
  limit: number;
}

export interface ClipPage {
  items: ClipItem[];
  nextCursor: string | null;
}

export interface ClipStats {
  total: number;
  pinned: number;
  favorite: number;
}

export interface ClipGroup {
  id: number;
  name: string;
  color: string | null;
  count: number;
}

// ───────────────────────── 截图库 ─────────────────────────

export interface Shot {
  id: number;
  filePath: string;
  thumbPath: string | null;
  width: number;
  height: number;
  sizeBytes: number;
  kind: 'normal' | 'longshot';
  sourceApp: string | null;
  ocrText: string | null;
  hasAnnotations: boolean;
  favorite: boolean;
  note: string | null;
  createdAt: number;
}

export interface ShotQuery {
  keyword?: string;
  favoriteOnly?: boolean;
  kind?: string | null;
  cursor?: string | null;
  limit: number;
}

export interface ShotPage {
  items: Shot[];
  nextCursor: string | null;
  total: number;
}

// ───────────────────────── 识字 / 翻译 ─────────────────────────

export interface OcrBlock {
  text: string;
  box: [[number, number], [number, number], [number, number], [number, number]];
  score: number;
}

export interface BoxRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface OcrLine {
  text: string;
  bbox: BoxRect;
  blockIndices: number[];
}

export interface OcrParagraph {
  text: string;
  lines: OcrLine[];
  bbox: BoxRect;
  blockIndices: number[];
}

export interface OcrResult {
  blocks: OcrBlock[];
  paragraphs: OcrParagraph[];
  plainText: string;
  rawText: string;
  engine: string;
  elapsedMs: number;
}

export interface OcrJob {
  id: string;
  status: 'running' | 'done' | 'error';
  imagePath: string;
  width: number;
  height: number;
  translate: boolean;
  recordId: number | null;
  result: OcrResult | null;
  error: string | null;
}

export interface OcrRecordSummary {
  id: number;
  imagePath: string | null;
  width: number | null;
  height: number | null;
  engine: string;
  preview: string;
  charCount: number;
  elapsedMs: number | null;
  createdAt: number;
}

export interface OcrPage {
  items: OcrRecordSummary[];
  total: number;
}

export interface TranslateRequest {
  text: string;
  from?: string | null;
  to?: string | null;
  provider?: string | null;
  /** 不读缓存 */
  fresh?: boolean;
}

export interface TranslateResult {
  text: string;
  from: string;
  to: string;
  provider: string;
  cached: boolean;
}

// ───────────────────────── AI 对话 ─────────────────────────

export interface AiProvider {
  id: string;
  name: string;
  kind: 'openai' | 'anthropic';
  baseUrl: string;
  /** 用户从"检测模型"结果里挑出来要用的 */
  models: string[];
  enabled: boolean;
}

export interface QuickPrompt {
  id: string;
  label: string;
  /** {text} 换成上下文文字 */
  prompt: string;
}

export interface AiSettings {
  providers: AiProvider[];
  /** 服务商id/模型名 */
  defaultModel: string;
  systemPrompt: string;
  temperature: number;
  maxTokens: number;
  thinking: ThinkingLevel;
  quickPrompts: QuickPrompt[];
  panel: { fontSize: number; width: number; height: number; layout: 'bubble' | 'plain' };
}

/** 思考深度：auto = 不指定，用模型自己的默认 */
export type ThinkingLevel = 'auto' | 'low' | 'medium' | 'high' | 'max';
export const THINKING_LEVELS: ThinkingLevel[] = ['auto', 'low', 'medium', 'high', 'max'];

export type ChatImage = { kind: 'store'; id: string } | { kind: 'file'; path: string };

export interface ChatMessage {
  role: 'user' | 'assistant';
  content: string;
  images?: ChatImage[];
}

export interface ChatRequest {
  id: string;
  model?: string | null;
  messages: ChatMessage[];
  thinking?: ThinkingLevel;
}

export type ChatEvent =
  | { type: 'start'; model: string }
  | { type: 'delta'; text: string }
  | { type: 'reasoning'; text: string }
  | { type: 'notice'; message: string }
  | { type: 'done' }
  | { type: 'error'; message: string };

export interface AiContext {
  text?: string | null;
  images: ChatImage[];
  source: 'selection' | 'ocr' | 'capture' | 'free';
  /** 已有的对话（挪到独立窗口时带过去） */
  history?: ChatTurn[];
  seq: number;
}

/** 对话里的一条（AiChat 内部格式，跨窗口原样传递）。 */
export interface ChatTurn {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  sent?: string;
  images?: ChatImage[];
  template?: string;
  model?: string;
  /** 模型的思考过程（服务给了才有） */
  reasoning?: string;
  /** 回答之外的一句提示 */
  notice?: string;
  error?: string;
  streaming?: boolean;
}

export type ProviderId = 'bing' | 'transmart' | 'google' | 'deepl' | 'openai';

export interface ProviderInfo {
  id: ProviderId;
  name: string;
  free: boolean;
  /** 自填源要填了密钥才算可用 */
  configured: boolean;
  enabled: boolean;
}

export interface ToastPayload {
  kind: 'success' | 'error' | 'info';
  message: string;
}

export interface ClipChanged {
  id: number;
  isNew: boolean;
}

/** 截图原位翻译的结果（坐标是本屏局部物理像素） */
export interface RegionTranslation {
  blocks: { x: number; y: number; width: number; height: number; lineHeight: number; source: string; text: string }[];
  provider: string;
  from: string;
  to: string;
}
