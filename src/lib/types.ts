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
    pinClipboard: string;
    inputTranslate: string;
  };
  capture: CaptureSettings;
  longshot: {
    scrollDebounceMs: number;
    maxHeight: number;
    detectFixedHeader: boolean;
  };
  ocr: {
    engine: 'rapid' | 'paddle' | 'system';
    idleTimeoutMinutes: number;
    keepLineBreaks: boolean;
    /** 识别完自动复制到剪贴板 */
    autoCopy: boolean;
  };
  translate: {
    targetLanguage: string;
    /** 全部翻译源及顺序，排第一的可用源是默认源 */
    providers: { id: ProviderId; enabled: boolean }[];
    /** 用户自己添加的翻译（内置里没有的） */
    custom: CustomTranslator[];
    showAllProviders: boolean;
    /** 截图原位翻译用的翻译源：auto = 按顺序 */
    regionProvider: string;
    customBaseUrl: string;
    customModel: string;
    popup: PopupStyle;
    speech: { rate: number; pitch: number; voices: Record<string, string> };
    selection: {
      showButton: boolean;
      buttonPosition: 'bottomRight' | 'topRight' | 'bottomLeft' | 'topLeft';
      /** 划词按钮的样子 */
      buttonStyle: 'liquid' | 'frosted' | 'solid' | 'custom';
      /** 自定义玻璃（样式选"自定义"时用） */
      glass: GlassTuning;
      /** 纯色圆点的配色（样式选"纯色"时用） */
      solid: SolidDot;
      /** 选中系统语言的文字时不弹按钮 */
      skipSystemLanguage: boolean;
      /** 选中这些语言的文字时不弹按钮 */
      skipLanguages: string[];
      /** 划词按钮圆点的直径（逻辑像素） */
      buttonSize: number;
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
    /** 面板毛玻璃 */
    panelBlur: boolean;
    /** 底部样式贴边：直角、紧贴屏幕底部 */
    panelDocked: boolean;
    /** 底部样式盖在程序坞上面、贴着屏幕最底下（macOS） */
    panelOverDock: boolean;
    /** 面板弹出 / 收起的动画 */
    panelAnimation: boolean;
    /** 面板里点一下就粘贴（默认双击粘贴） */
    singleClickPaste: boolean;
    maxTextMb: number;
    maxImageMb: number;
    respectPrivacyFlag: boolean;
    useBlacklist: boolean;
    blacklist: string[];
    retentionDays: number;
    retentionMaxItems: number;
    /** 读「信息」里 iPhone 转发来的短信，认出验证码自动复制（macOS） */
    smsCodes: boolean;
    /** 剪贴板里的验证码打码显示（鼠标放到眼睛图标上才显示） */
    otpMask: boolean;
    /** 验证码在剪贴板历史里留多久（秒），0 = 不删 */
    otpExpireSecs: number;
    /** 邮箱验证码：用 IMAP 连邮箱读新邮件里的验证码（密码不在这里，另存在加密的密钥表里） */
    mailCodes: MailCodeSettings;
    /** 智能推荐交给 AI 模型挑（关掉 = 只在本机按词打分） */
    smartAi: boolean;
    /** 智能推荐用的模型（"服务商 id/模型"），空 = 跟 AI 对话的默认模型 */
    smartModel: string;
    /** 智能推荐水珠的颜色：warm | ocean | pearl | mint | violet */
    orbColor: string;
  };
  sync: SyncSettings;
  /** 给 AI 工具用的 MCP 服务 */
  mcp: { enabled: boolean; port: number; allowWrite: boolean };
  record: RecordSettings;
  update: {
    /** 自动检查更新（启动后一次，之后每 6 小时） */
    autoCheck: boolean;
    /** cn = 国内（Gitee），global = 国外（GitHub） */
    channel: 'cn' | 'global';
    /** 点了"不显示更新提示"的版本 */
    skippedVersion: string;
  };
}

export interface UpdateChanges {
  added: string[];
  fixed: string[];
  improved: string[];
}

export interface UpdateInfo {
  version: string;
  date: string | null;
  changes: UpdateChanges;
  changesEn: UpdateChanges | null;
  notes: string | null;
  size: number | null;
}

export interface UpdateStatus {
  current: string;
  checking: boolean;
  lastCheck: number | null;
  error: string | null;
  available: UpdateInfo | null;
  stage: 'downloading' | 'verifying' | 'installing' | 'ready' | null;
  progress: number | null;
}

/** 多端同步（规格 09）。密码不在这里（在 Rust 那边加密保存）。 */
export interface SyncSettings {
  /** 空 = 计算机名 */
  deviceName: string;
  lanEnabled: boolean;
  lanPort: number;
  webEnabled: boolean;
  webdavEnabled: boolean;
  webdavUrl: string;
  webdavUser: string;
  webdavFolder: string;
  webdavInterval: number;
  sendImages: boolean;
  maxImageMb: number;
  autoWrite: boolean;
}

export interface SyncPeer {
  deviceId: string;
  name: string;
  platform: string;
  kind: 'desktop' | 'web';
  role: 'member' | 'host';
  address: string | null;
  code: string | null;
  lastSeen: number | null;
  createdAt: number;
  online: boolean;
}

export interface SyncPairRequest {
  id: string;
  name: string;
  platform: string;
  kind: 'desktop' | 'web';
  /** 电脑之间配对的 4 位核对数字 */
  sas: string | null;
  address: string;
}

export interface SyncStatus {
  deviceId: string;
  deviceName: string;
  defaultName: string;
  platform: string;
  code: string;
  lanMode: 'off' | 'host' | 'member';
  lanError: string | null;
  port: number | null;
  urls: string[];
  members: SyncPeer[];
  host: SyncPeer | null;
  hostStatus: { online: boolean; address: string | null; error: string | null } | null;
  pending: SyncPairRequest[];
  join: { phase: '' | 'idle' | 'searching' | 'waiting' | 'done' | 'error'; sas: string | null; hostName: string | null; error: string | null };
  webdav: { configured: boolean; running: boolean; lastSync: number | null; error: string | null };
}

export interface DavDevice {
  deviceId: string;
  name: string;
  platform: string;
  lastSeen: number;
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

/** 截图工具条最右边那一组里的按钮 */
export type TailAction = 'undo' | 'save' | 'pin' | 'cancel' | 'done';
export const TAIL_ACTIONS: TailAction[] = ['undo', 'save', 'pin', 'cancel', 'done'];
/** 中间那一组 */
export type MidAction = 'ocr' | 'translate' | 'qr' | 'table' | 'formula' | 'ai' | 'longshot' | 'gif' | 'video';
/** 中间那一组新装时的顺序（二维码、表格、公式在「识别」组里） */
export const MID_ACTIONS: string[] = ['ocr', 'translate', 'group:recognize', 'ai', 'longshot', 'record'];
/** 新装时的按钮组 */
export const DEFAULT_GROUPS = [{ id: 'recognize', name: '', items: ['qr', 'table', 'formula'] }];
/** 绘制工具 */
export type ToolId = 'rect' | 'ellipse' | 'arrow' | 'pen' | 'highlight' | 'counter' | 'mosaic' | 'text' | 'measure' | 'watermark' | 'template' | 'eraser';
export const TOOL_ORDER: ToolId[] = ['rect', 'ellipse', 'arrow', 'pen', 'highlight', 'counter', 'mosaic', 'text', 'measure', 'watermark', 'template', 'eraser'];

export interface FrameStyle {
  /** 'accent' = 跟随主题色，否则 #RRGGBB */
  color: string;
  width: number;
  /** corners = 只画四个角 */
  style: 'solid' | 'dashed' | 'dotted' | 'corners';
  radius: number;
  /** 四条边中间的拖柄：bar = 小圆条，halfBar = 半圆条（只露出框外的一半），dot = 小圆点（四个角一直是圆点） */
  handles: HandleStyle;
}

export type HandleStyle = 'bar' | 'halfBar' | 'dot';

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
  /** 窗口里的元素框到多细：block = 只框大块 | element = 连单元格、按钮都框 */
  elementLevel: 'block' | 'element';
  /** 截图时自动识别画面里的二维码 / 条码 */
  autoScanCodes: boolean;
  /** 识别出的表格 / 公式盖在原处之后原图怎么看：side = 摆在选区左边 | toggle = 小条上点按钮切换 */
  /** 录屏（录制准备）时选区框的拖柄，和截图的分开设 */
  recordHandles: HandleStyle;
  inplaceOriginal: 'side' | 'toggle';
  snapThreshold: number;
  rightClick: 'none' | 'exit' | 'cancelSelection';
  /** 截图工具条三组按钮各自的顺序：绘制工具、识字翻译那一组、最右边那一组 */
  toolbarTools: string[];
  /** 工具条上隐藏的按钮（设置里点图标切换） */
  toolbarHidden?: string[];
  /** 可以有按钮组（`group:组名`） */
  toolbarActions: string[];
  toolbarOrder: TailAction[];
  /** 按钮组：几个按钮合成工具条上的一个，点一下弹出二级工具条 */
  toolbarGroups?: { id: string; name: string; items: string[] }[];
  /** 截图工具条的样子：glass = 半透明玻璃条 | frosted = 磨砂玻璃 | flat = 扁平（不透明、不模糊） */
  toolbarStyle: 'glass' | 'frosted' | 'flat';
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

export type HotkeyAction = 'capture' | 'longshot' | 'ocr' | 'clipboard' | 'translate' | 'instant' | 'ai' | 'pinClipboard' | 'inputTranslate';

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
  /** PaddleOCR 已下载 */
  paddle: boolean;
  paddleDownload: PaddleDownload;
  /** 本地公式识别：这台电脑能不能用、下载了没有、要下多少字节、下载进度 */
  latexSupported: boolean;
  latex: boolean;
  latexSize: number;
  latexDownload: PaddleDownload;
}

/** PaddleOCR 引擎下载进度 */
export interface PaddleDownload {
  stage: 'downloading' | 'extracting' | null;
  progress: number | null;
  error: string | null;
}

export interface Permissions {
  /** 屏幕录制：没有就截不到别的程序的窗口 */
  screenCapture: boolean | null;
  /** 辅助功能：模拟粘贴 / 复制、全局拦截按键 */
  accessibility: boolean | null;
  /** 完全磁盘访问：读「信息」里的短信（短信验证码） */
  fullDiskAccess: boolean | null;
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
export type FinishAction = 'copy' | 'save' | 'pin' | 'ocr' | 'translate' | 'longshot' | 'ai' | 'gif' | 'video' | 'record';

export interface SessionInfo {
  sessionId: number;
  intent: CaptureIntent;
  monitor: MonitorInfo;
  imageUrlId: string;
  windows: WindowInfo[];
  cursor: [number, number] | null;
  hasFocus: boolean;
  /** 录制中开的截图（录 DATO 自己的操作）：工具条上不放录制、长截图 */
  recording: boolean;
  settings: CaptureSettings;
}

/** GIF 录制开始 / 结束（rect 是本屏局部物理坐标） */
export interface GifStateEvent {
  active: boolean;
  monitorId: number;
  rect: PhysicalRect;
  /** 录完回到录制准备（active = false 时）：录制框留在 rect */
  prep: boolean;
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

/** 识别出来的一个二维码 / 条码，位置按物理像素 */
export interface ScannedCode {
  kind: string;
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

/** 贴图里识别出来的一段文字，外接框按图片物理像素 */
export interface PinText {
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
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

export type ClipType = 'text' | 'link' | 'color' | 'image' | 'files' | 'otp';

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
  /** 从别的设备同步来的 */
  remote: boolean;
  pinned: boolean;
  favorite: boolean;
  note: string | null;
  groupId: number | null;
  createdAt: number;
  lastUsedAt: number;
  /** 验证码（type = otp）所在的短信原文 */
  originText: string | null;
  /** 来自哪种设备：iphone | ipad | android | mac | macbook | pc | laptop | apple（通用剪贴板） */
  deviceKind: string | null;
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
  kind: 'normal' | 'longshot' | 'gif';
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
  /** 识别完已经自动复制到剪贴板 */
  copied: boolean;
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
  /** 识别表格 / 公式（AI 看图）用的模型，`服务商id/模型名`；空 = 跟默认模型一样 */
  visionModel: string;
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

/** 翻译源的标识：内置的是固定的几个名字，用户自己添加的以 custom- 开头 */
export type ProviderId = string;

/** 用户自己添加的翻译。密钥不在这里（加密存在后端） */
export interface CustomTranslator {
  id: string;
  name: string;
  /** ai = OpenAI 兼容的模型接口 | deeplx = DeepLX 这类自建接口 | http = 自己描述的任意 HTTP 接口 */
  kind: 'ai' | 'deeplx' | 'http';
  url: string;
  model: string;
  prompt: string;
  headers: string;
  body: string;
  path: string;
}

/** 翻译源要用户填的一项（密钥、应用 ID、区域……） */
export interface ProviderField {
  key: string;
  label: string;
  secret: boolean;
  required: boolean;
  hint: string;
  set: boolean;
  /** 密钥类的是打码后的样子，其余的是原样 */
  value: string | null;
}

export interface ProviderInfo {
  id: ProviderId;
  name: string;
  free: boolean;
  /** 用户自己添加的；kind 是它的类型 */
  custom: boolean;
  kind: CustomTranslator['kind'] | null;
  /** 自填源要把必填的几项填了才算可用 */
  configured: boolean;
  enabled: boolean;
  /** 要用户填的几项和现在填没填 */
  fields: ProviderField[];
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
  /** bold：原文是粗体（标题、表头），译文也用粗体 */
  blocks: { x: number; y: number; width: number; height: number; lineHeight: number; source: string; text: string; bold: boolean; fontSize: number }[];
  provider: string;
  from: string;
  to: string;
}

/** MCP 服务的状态（设置 → AI 工具） */
export interface McpStatus {
  running: boolean;
  url: string;
  token: string;
  error: string | null;
  /** 命令行桥接要用的程序路径 */
  command: string;
}

/** 剪贴板智能推荐的结果 */
export interface SmartSuggestion {
  /** 推荐的条目，最可能的在前；可能是空的 */
  items: ClipItem[];
  /** 模型给的一句理由（本机打分没有） */
  reason: string | null;
  /** 是 AI 模型挑的（false = 本机按词打分） */
  byAi: boolean;
  /** 看的是哪个应用 */
  app: string | null;
}

/** 邮箱验证码连哪个邮箱 */
export interface MailCodeSettings {
  enabled: boolean;
  /** qq | 163 | gmail | icloud | proton | custom */
  provider: string;
  address: string;
  /** 服务器地址，只有 custom 用 */
  host: string;
  /** 端口，custom 和 proton 用；0 = 默认 */
  port: number;
  /** tls | starttls，只有 custom 用 */
  security: string;
}

/** 邮箱验证码的连接状态 */
export type MailCodesStatus = (
  | { state: 'off' | 'incomplete' | 'offline' | 'connecting' | 'connected' }
  | { state: 'error'; message: string }
) & {
  /** 存没存密码（密码本身读不回来） */
  hasPassword: boolean;
};

/** 划词按钮的自定义玻璃：都是 0 ~ 100，默认值就是"液态玻璃"的样子 */
/** 纯色划词圆点的配色：颜色 #RRGGBB，空 = 跟着深浅色主题 */
export interface SolidDot {
  background: string;
  icon: string;
  border: string;
  /** 边框粗细（0 ~ 4） */
  borderWidth: number;
  /** 阴影浓淡（0 ~ 100） */
  shadow: number;
}

export interface GlassTuning {
  dispersion: number;
  blur: number;
  refraction: number;
  magnify: number;
  veil: number;
  tint: number;
  highlight: number;
}

/** 截图界面里原位显示的表格的一格（坐标是选区图的像素） */
export interface LaidCell {
  row: number;
  col: number;
  rowSpan: number;
  colSpan: number;
  header: boolean;
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
  /** 字所在的范围 [x, y, 宽, 高]；空格子没有 */
  textBox: [number, number, number, number] | null;
  /** textBox 是按字的墨迹量的（不是的话是识字框，比字大一圈） */
  byInk?: boolean;
  /** 笔画宽 ÷ 字号：粗体的按它选半粗还是全粗 */
  stroke?: number | null;
  /** 换过字的格子（AI 重新认过）：原图里那段字（墨迹框量的是它） */
  inkText?: string;
  /** 斜体 */
  italic?: boolean;
  /** 等宽字体（代码、终端里的表） */
  mono?: boolean;
  lines: number;
  /** 字号（像素）：按墨迹高换算 */
  fontPx: number;
  /** 墨迹中心比行框中心低多少（字号的倍数） */
  centerShift: number;
  color: string;
  background: string;
  bold: boolean;
  align: 'left' | 'center' | 'right';
}

/** 一块在选区图里的范围（物理像素） */
export interface RegionRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** 原位识别出来的一张表。格子的坐标都是这张表所在那块（`rect`）里的物理像素 */
export interface CaptureTable {
  table: { width: number; height: number; rows: number; cols: number; cells: LaidCell[] };
  /** 字抹掉的底图（图片仓库里的编号，用 shotUrl 取，和 `rect` 一样大）；自己画的表没有底图 */
  background: string;
  /** 这张表在选区图里的范围 */
  rect: RegionRect;
  /** 自己画的表（AI 认出的行列和原位的格子对不上时）：铺在哪、线用什么颜色 */
  plain?: { x: number; y: number; width: number; height: number; line: string };
}

/** 选区里认出来的几张表（几张表、表和公式在一起时分开认），加上选区原图（摆在旁边对照） */
export interface CaptureTables {
  tables: CaptureTable[];
  source: string;
}

/** 选区里像公式的几块（表格不算）：范围、底色、字色，加上选区原图 */
export interface FormulaRegions {
  regions: { rect: RegionRect; background: string; color: string }[];
  source: string;
}

/** 选区的底色、字色和原图（原位显示公式用） */
export interface RegionStyle {
  background: string;
  color: string;
  source: string;
}

/** 录制视频（MP4）和 GIF 的设置；GIF 的帧率、录不录鼠标在 capture 里 */
export interface RecordSettings {
  resolution: 'original' | '1080p' | '720p';
  /** 视频画质 10–100，决定码率 */
  quality: number;
  codec: 'h264' | 'hevc';
  /** 系统声音、麦克风各存一条音轨 */
  separateAudioTracks: boolean;
  gifResolution: 'original' | '1080p' | '720p' | '480p';
  gifLoop: boolean;
  fps: 15 | 24 | 30 | 60;
  systemAudio: boolean;
  microphone: boolean;
  camera: boolean;
  cameraPosition: 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right';
  cameraSize: 'small' | 'medium' | 'large';
  maxSeconds: number;
  editBeforeSave: boolean;
  /** 录成什么（录制栏最左边选） */
  format: RecordFormat;
  /** 鼠标轨迹 / 点击涟漪的颜色 #rrggbb；空 = 不画 */
  trailColor: string;
  clickColor: string;
  mouseHighlight: boolean;
  mouseButtons: boolean;
  mouseScroll: boolean;
  /** 录制时把按下的键显示在画面角落 */
  keystrokes: boolean;
  /** 动图底下画进度条 / 播放时间 */
  progress: 'off' | 'bar' | 'time';
  /** 点开始后倒数几秒 */
  countdown: number;
  /** 摄像头画面左上角在选区里的位置（占选区宽高的比例）；null = 左下角 */
  cameraAt: [number, number] | null;
  cameraShape: 'circle' | 'square' | 'rounded';
  /** 摄像头画面大小（逻辑像素） */
  cameraPx: number;
  effects: RecordEffects;
  autoHideToolbar: boolean;
  notifyDone: boolean;
}

export type RecordFormat = 'mp4' | 'gif' | 'apng';

/** 录制效果的细节（录制栏「效果设置」） */
export interface RecordEffects {
  trailMs: number;
  keyboardPx: number;
  keyboardBg: string;
  keyboardFg: string;
  highlightRadius: number;
  highlightColor: string;
  progressColor: string;
}

export type MediaKind = 'microphone' | 'camera';
export type MediaAccess = 'granted' | 'denied' | 'notDetermined' | 'unsupported';

/** 录完待剪辑的一段（GIF 或视频） */
export interface RecordEditInfo {
  id: number;
  format: 'gif' | 'mp4';
  durationMs: number;
  width: number;
  height: number;
  /** 临时文件路径（视频用 asset 协议播放） */
  path: string;
  thumbs: { ms: number; url: string }[];
}

/** 自己画的右键菜单的一项（见 popmenu.rs）：图标是 lucide 图标的名字 */
export type PopItem =
  | { kind: 'item'; id: string; label: string; icon?: string; shortcut?: string; checked?: boolean; disabled?: boolean; danger?: boolean }
  | { kind: 'separator' }
  /** 一排图标按钮（旋转、翻转这种一组小操作） */
  | { kind: 'row'; label: string; items: { id: string; icon: string; label: string }[] };
