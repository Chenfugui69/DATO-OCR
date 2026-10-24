// 截图遮罩（规格 02）。逐条对齐微信截图的手感。
//
// 这个窗口是透明的：冻结的屏幕画面在它正下方的原生底图窗口里，这里只画压暗层、选区、
// 标注、放大镜和工具条。选区是**真镂空**，洞里看到的就是原始像素。

import clsx from 'clsx';
import './capture.css';
import '@/views/annotate/annotate.css';

import { Copy, Eye, Languages, Lock, Move, X } from 'lucide-react';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
import { useTranslation } from 'react-i18next';

import i18n from '@/i18n';
import { broadcast, on } from '@/lib/events';
import { formatColor, readableOn } from '@/lib/format';
import { CONTENT_INTERVAL, useElementSize, useThrottled } from '@/lib/hooks';
import { capture, gif, reportError, system } from '@/lib/ipc';
import { useSettingsStore } from '@/lib/settings';
import { isMac, shotUrl, windowLabel } from '@/lib/platform';
import { DEFAULT_GROUPS, MID_ACTIONS, TAIL_ACTIONS, type CaptureIntent, type CaptureTable, type FinishAction, type FrameStyle, type TailAction } from '@/lib/types';
import { notify } from '@/ui/overlays';
import { AnnotationEngine, toolOf } from '@/views/annotate/engine';
import { BRUSH_RANGE, PEN_RANGE, watermarkOf, type Tool } from '@/views/annotate/model';
import { bmpSource, type PixelSource } from '@/views/annotate/pixels';
import { canPick, handleCursor, SelectionOverlay, toolCursor } from '@/views/annotate/SelectionOverlay';
import { TextEditor, useEngineVersion } from '@/views/annotate/TextEditor';
import { ACTION_ICONS, GroupBar, SubToolbar, TOOL_KEYS, Toolbar, useGroupAnchor, useToolAnchor, type ActionIcon, type ActionId, type ToolbarGroup } from '@/views/annotate/Toolbar';
import { buildLabels, lineColors, TRANSLATION_GROUP } from '@/views/annotate/translateLayer';

import {
  HANDLES,
  HANDLE_CURSORS,
  area,
  bottom,
  clampMove,
  clampRect,
  contains,
  fromPoints,
  handlePoint,
  hitHandle,
  intersect,
  placeSizeHint,
  placeToolbar,
  resizeByHandle,
  resizeWithRatio,
  right,
  snapEdges,
  snapLines,
  snapMove,
  type Handle,
  type Point,
  type Rect,
  type SnapLines,
} from './geometry';
import { CodeBadges, CodeResults } from './CodeResults';
import { candidatesAt, ElementCache, rectKey, thresholds, type ElementLevel, type UiElement } from './elements';
import { aiTableLayout, FormulaInPlace, mergeAiTable, TableInPlace } from './InPlaceResult';
import { CameraFrame, Countdown, KeyHud, MoveHandle, RecordHints, RecordToolbar, sendGlyphs } from './RecordUI';
import { rememberSelection, SelectionPanel } from './SelectionPanel';
import { saveTemplate } from '@/views/annotate/templates';
import { GifUI } from './GifUI';
import { LongshotUI } from './LongshotUI';
import { Magnifier } from './Magnifier';
import { get, initialOverlay, set, useOverlay, type CaptureStructured, type LoadingLine } from './store';
import { TranslationText } from './TranslationText';

const MONITOR_ID = Number(windowLabel().replace('capture-', ''));
const DRAG_THRESHOLD_CSS = 4;

// ───────────────────────── 会话级（非 React）状态 ─────────────────────────

const engine = new AnnotationEngine();
let pixels: PixelSource | null = null;
let bitmap: ImageBitmap | null = null;
let windowsLocal: { handle: number; rect: Rect }[] = [];
const childCache = new Map<number, Rect[]>();
let childTimer: number | undefined;
let childPending: number | null = null;
/**
 * 界面元素（macOS 辅助功能，像 Snow Shot）：鼠标下面那个窗口里的元素由里到外的框（表格单元格 → 整张表 → … → 窗口）。
 * `level` = 用户用滚轮往外挑了几层；鼠标挪到别的元素上就回到最里面那层。
 */
const elementCache = new ElementCache();
let elementLevel = 0;
/** 当前最里层候选的框（换了就把层级回到最里面） */
let elementInner = '';
let elementSeq = 0;
let elementTimer: number | undefined;
let elementLast = 0;
/** 上次去问时鼠标所在的最里层元素：鼠标还在它里面就不用再问 */
let elementAsked: { handle: number; rect: Rect | null; at: number } | null = null;
/** 正在问、本地又只找得到比刚才大得多的框时，先停在刚才那个框上（最多这么久），不往外跳 */
let elementHoldUntil = 0;
/** 滚轮切层级：触控板一次滑动会来一串小的滚动事件，攒够这么多才算一格 */
let wheelAccum = 0;
const WHEEL_STEP = 40;
/** 问元素的最短间隔（毫秒）：每次都是十几次跨进程往返，鼠标快速划过时没必要每一下都问 */
const ELEMENT_INTERVAL = 40;
const ELEMENT_HOLD = 140;

/** lasso：按着 Option / Alt 拖 = 自由画选区（`points` 是一路画下来的轨迹） */
let press: { at: Point; hover: Rect | null; lasso: boolean; points: Point[] } | null = null;
let drag: { kind: 'move' | 'resize'; handle?: Handle; at: Point; orig: Rect; shape?: Point[] | null } | null = null;

/** 一串点的外接矩形 */
function boundsOf(points: Point[]): Rect {
  const xs = points.map((q) => q.x);
  const ys = points.map((q) => q.y);
  const x = Math.floor(Math.min(...xs));
  const y = Math.floor(Math.min(...ys));
  return { x, y, width: Math.max(1, Math.ceil(Math.max(...xs)) - x), height: Math.max(1, Math.ceil(Math.max(...ys)) - y) };
}
let lines: SnapLines = { xs: [], ys: [] };
let activeElsewhere: number | null = null;
/** 正在打字时按了右键：已经在按下时提交了文字，紧跟着的 contextmenu 不能再当"退出截图" */
let swallowMenu = false;

const scale = () => get().session?.monitor.scaleFactor ?? window.devicePixelRatio;

function screenRect(): Rect {
  const m = get().session?.monitor;
  return { x: 0, y: 0, width: m?.bounds.width ?? 1, height: m?.bounds.height ?? 1 };
}

function toPx(e: { clientX: number; clientY: number }): Point {
  const s = scale();
  const scr = screenRect();
  return {
    x: Math.max(0, Math.min(scr.width - 1, Math.floor(e.clientX * s))),
    y: Math.max(0, Math.min(scr.height - 1, Math.floor(e.clientY * s))),
  };
}

function snapThreshold(e: { altKey: boolean }): number {
  // 按住 Alt 临时禁用吸附（规格 02 §3.5.10）
  return e.altKey ? 0 : (get().session?.settings.snapThreshold ?? 8);
}

// ───────────────────────── 窗口检测 ─────────────────────────

function scheduleChildren(handle: number) {
  if (childPending === handle || childCache.has(handle)) return;
  window.clearTimeout(childTimer);
  childPending = handle;
  // 子控件懒枚举：鼠标在某窗口上停 80ms 才查它的子控件（规格 02 §7）
  childTimer = window.setTimeout(() => {
    const origin = get().session?.monitor.bounds;
    capture
      .windowChildren(handle)
      .then((rects) => {
        if (!origin) return;
        const local = rects
          .map((r) => intersect({ x: r.x - origin.x, y: r.y - origin.y, width: r.width, height: r.height }, screenRect()))
          .filter((r): r is Rect => r !== null);
        childCache.set(handle, local);
        childPending = null;
        const { cursor, phase } = get();
        if (cursor && phase === 'detect') set({ hover: hoverAt(cursor) });
      })
      .catch((err) => {
        childCache.set(handle, []);
        childPending = null;
        reportError('capture-window-children', err);
      });
  }, 80);
}

/**
 * 鼠标底下要高亮的区域。截图识字 / 翻译不自动框窗口（返回 null）：整屏均匀压暗，等用户自己拖 ——
 * 识字要的是一小段文字，自动框出来的整个窗口几乎从来不是想要的范围。
 */
function hoverAt(p: Point, query = false): Rect | null {
  return isTextIntent(get().session?.intent) ? null : detect(p, query);
}

/** 鼠标下面可选的元素框（比窗口小的），由小到大 */
function elementCandidates(hit: { handle: number; rect: Rect }, p: Point): Rect[] {
  const s = get().session;
  const level: ElementLevel = s?.settings.elementLevel === 'block' ? 'block' : 'element';
  return candidatesAt(elementCache.items(hit.handle), p, { ...thresholds(level, s?.monitor.scaleFactor ?? 1), maxArea: area(hit.rect) * 0.97 });
}

/** 节流着去问鼠标下面的界面元素；鼠标还在上次问到的最里层元素里面（而且问过没多久）就不用再问 */
function scheduleElements(hit: { handle: number; rect: Rect }, p: Point) {
  // 鼠标还在上次问到的最里层元素里、而且那是个小元素（一格、一个按钮）：里面不会再有别的，不用再问。
  // 最里层是个大块（整个列表、整页）时照样问 —— 比如刚开始问到的是网页还没建好树时的整块
  const asked = elementAsked;
  const small = asked?.rect && area(asked.rect) < area(hit.rect) * 0.2;
  if (asked && small && asked.handle === hit.handle && asked.rect && contains(asked.rect, p) && performance.now() - asked.at < 1500) return;
  window.clearTimeout(elementTimer);
  const wait = ELEMENT_INTERVAL - (performance.now() - elementLast);
  const run = () => {
    elementLast = performance.now();
    const cursor = get().cursor;
    if (cursor && get().phase === 'detect') void queryElements(hit.handle, cursor);
  };
  elementHoldUntil = performance.now() + Math.max(0, wait) + ELEMENT_HOLD;
  if (wait <= 0) run();
  else elementTimer = window.setTimeout(run, wait);
}

async function queryElements(handle: number, p: Point) {
  const id = ++elementSeq;
  const origin = get().session?.monitor.bounds;
  if (!origin) return;
  const toLocal = (list: { rect: Rect; role: string }[]) =>
    list
      .map((e) => ({ role: e.role, rect: intersect({ x: e.rect.x - origin.x, y: e.rect.y - origin.y, width: e.rect.width, height: e.rect.height }, screenRect()) }))
      .filter((e): e is UiElement => e.rect !== null);
  const gx = origin.x + Math.round(p.x);
  const gy = origin.y + Math.round(p.y);
  try {
    const chain = toLocal(await capture.elementsAt(handle, gx, gy));
    elementCache.add(handle, chain);
    elementAsked = { handle, rect: chain.find((e) => e.rect.width >= 8 && e.rect.height >= 8)?.rect ?? null, at: performance.now() };
    if (id === elementSeq) {
      // 结果回来时鼠标已经挪走了（问的是刚才那个点）：先别放开"停在原框上"，接着问现在这个点
      const now = get().cursor;
      const moved = !!now && Math.hypot(now.x - p.x, now.y - p.y) > 2;
      elementHoldUntil = moved ? performance.now() + ELEMENT_HOLD : 0;
      refreshHover();
      if (moved && now && get().phase === 'detect') {
        elementLast = performance.now();
        void queryElements(handle, now);
      }
    }
    // 附近的元素（同一行的其他格、其他行……）后台取回来存着；同一块地方只取一次
    const anchor = chain[1]?.rect ?? chain[0]?.rect;
    if (anchor && elementCache.claimNear(handle, anchor)) {
      const near = toLocal(await capture.elementsNear(handle, gx, gy));
      elementCache.add(handle, near);
      refreshHover();
    }
  } catch (err) {
    reportError('capture-elements', err);
  }
}

/** 缓存变了：按鼠标现在的位置重挑预选框 */
function refreshHover() {
  const { cursor, phase } = get();
  if (cursor && phase === 'detect') set({ hover: hoverAt(cursor) });
}

/** 检测态里滚轮：在鼠标下面的元素层级之间切换（往上滚 = 外面一层，往下滚 = 里面一层） */
function stepElementLevel(deltaY: number) {
  const { cursor, session } = get();
  if (!cursor || !session?.settings.detectChildWindows) return;
  const hit = windowsLocal.find((w) => contains(w.rect, cursor));
  if (!hit) return;
  // 比窗口小的那些元素，再加上窗口本身（最外面一层）
  const levels = elementCandidates(hit, cursor).length + 1;
  wheelAccum += deltaY;
  if (Math.abs(wheelAccum) < WHEEL_STEP) return;
  const dir = wheelAccum < 0 ? 1 : -1;
  wheelAccum = 0;
  const next = Math.max(0, Math.min(levels - 1, elementLevel + dir));
  if (next === elementLevel) return;
  elementLevel = next;
  set({ hover: hoverAt(cursor) });
}

/** 命中判定：按 Z 序从上到下取第一个包含鼠标的窗口；再细分到鼠标下面的界面元素 / 子控件。
 *  `query`：鼠标真的动了（不是别的原因重算），可以去问新的界面元素 */
function detect(p: Point, query = false): Rect {
  const s = get().session;
  const screen = screenRect();
  if (!s?.settings.detectWindows) return screen;
  const hit = windowsLocal.find((w) => contains(w.rect, p));
  if (!hit) return screen;
  // 界面元素（macOS 的辅助功能、Windows 的 UI Automation）；Windows 上问不到元素的窗口（没有 UI Automation 的
  // 老程序）退回按子窗口框
  const cands = s.settings.detectChildWindows ? elementCandidates(hit, p) : [];
  if (s.settings.detectChildWindows && query) scheduleElements(hit, p);
  if (s.settings.detectChildWindows && (isMac || cands.length > 0)) {
    // 换到另一个元素上了：层级回到最里面
    const inner = cands[0] ? rectKey(cands[0]) : '';
    if (inner !== elementInner) {
      elementInner = inner;
      elementLevel = 0;
    }
    const pick = cands[Math.min(elementLevel, cands.length)] ?? hit.rect;
    const prev = get().hover;
    if (elementLevel === 0 && prev && area(pick) > area(prev) * 2.5 && contains(hit.rect, { x: prev.x, y: prev.y })) {
      // 正在问新位置、本地只找得到比刚才大得多的框（多半是整张表、整页）：先停在刚才的框上，别闪一下外框
      if (performance.now() < elementHoldUntil) return prev;
      // 刚出了一个小框、还贴着它的边（格子之间的缝、表头和第一行之间那几像素）：也先不跳到大框
      const gap = Math.max(prev.x - p.x, p.x - (prev.x + prev.width), prev.y - p.y, p.y - (prev.y + prev.height), 0);
      if (gap <= 8 * (s.monitor.scaleFactor || 1)) return prev;
    }
    return pick;
  }
  if (s.settings.detectChildWindows) {
    const children = childCache.get(hit.handle);
    if (children) {
      const best = children.filter((c) => contains(c, p) && area(c) >= 400 && area(c) < area(hit.rect)).sort((a, b) => area(a) - area(b))[0];
      if (best) return best;
    } else {
      scheduleChildren(hit.handle);
    }
  }
  return hit.rect;
}

// ───────────────────────── 会话生命周期 ─────────────────────────

async function startSession() {
  let info;
  try {
    info = await capture.sessionInfo(MONITOR_ID);
  } catch (err) {
    reportError('capture-overlay', err);
    return;
  }
  if (!info) return;
  if (get().session?.sessionId === info.sessionId) return;

  engine.reset();
  // 水印每次截图都从关着开始（工具的其他选项照旧记着）
  const opts = get().options;
  if (opts.watermark.enabled) set({ options: { ...opts, watermark: { ...opts.watermark, enabled: false } } });
  pixels = null;
  bitmap?.close();
  bitmap = null;
  engine.scale = info.monitor.scaleFactor;
  engine.setBackdrop({ pixels: null, bitmap: null });
  childCache.clear();
  elementCache.clear();
  elementLevel = 0;
  elementInner = '';
  elementAsked = null;
  elementSeq++;
  activeElsewhere = null;
  press = null;
  drag = null;
  const origin = info.monitor.bounds;
  const screen = { x: 0, y: 0, width: origin.width, height: origin.height };
  const toLocal = (r: { x: number; y: number; width: number; height: number }) =>
    intersect({ x: r.x - origin.x, y: r.y - origin.y, width: r.width, height: r.height }, screen);
  windowsLocal = info.windows
    .map((w) => ({ handle: w.handle, rect: toLocal(w.bounds) }))
    .filter((w): w is { handle: number; rect: Rect } => w.rect !== null);
  // 子控件用 Rust 在遮罩出现前拍的快照：浏览器被遮住后会把网页内容区藏起来，事后再查就查不到了
  if (info.settings.detectChildWindows) {
    for (const w of info.windows) {
      childCache.set(
        w.handle,
        w.children.map(toLocal).filter((r): r is Rect => r !== null),
      );
    }
  }
  lines = snapLines(
    windowsLocal.map((w) => w.rect),
    screen,
  );

  const cursor = info.cursor ? { x: info.cursor[0], y: info.cursor[1] } : null;
  // 先把状态同步提交到 DOM，再通知 Rust 上屏 —— 否则可能先亮一帧上次会话的残留画面
  flushSync(() => {
    set({
      ...initialOverlay(),
      session: info,
      phase: 'detect',
      cursor,
      colorFormat: info.settings.colorFormat,
    });
  });
  if (cursor) set({ hover: hoverAt(cursor, true) });

  try {
    await capture.overlayReady(info.sessionId, MONITOR_ID);
  } catch (err) {
    reportError('capture-overlay', err);
    // 会话在 Rust 侧，前端失败不收回的话之后每次热键都会被"已在截图中"挡掉
    void capture.cancel(info.sessionId);
    return;
  }
  void loadPixels(info.sessionId, info.imageUrlId);
  if (info.settings.autoScanCodes !== false && info.intent !== 'longshot') void autoScanCodes(info.sessionId);
}

/** 截图一开始就在后台把整屏的二维码 / 条码找出来（遮罩已经上屏，不耽误出图） */
async function autoScanCodes(sessionId: number) {
  try {
    const found = await capture.scanCodes(sessionId, MONITOR_ID, screenRect());
    if (get().session?.sessionId === sessionId && found.length) set({ autoCodes: found });
  } catch (err) {
    reportError('capture-auto-codes', err);
  }
}

/** 原始像素异步流入（放大镜/取色/马赛克用）。失败只降级不中断：画面已经在屏上了。 */
async function loadPixels(sessionId: number, imageId: string) {
  try {
    const res = await fetch(shotUrl(imageId));
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const buf = await res.arrayBuffer();
    if (get().session?.sessionId !== sessionId) return;
    pixels = bmpSource(buf);
    engine.setBackdrop({ pixels, bitmap: null });
    set({ pixelsReady: !!pixels });
    const bmp = await createImageBitmap(new Blob([buf], { type: 'image/bmp' }), { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
    if (get().session?.sessionId !== sessionId) {
      bmp.close();
      return;
    }
    bitmap = bmp;
    engine.setBackdrop({ pixels, bitmap });
    set({ bitmapReady: true });
  } catch (err) {
    reportError('capture-overlay-pixels', err);
  }
}

function endSession() {
  pendingMove = null;
  if (moveFrame) cancelAnimationFrame(moveFrame);
  moveFrame = 0;
  pixels = null;
  bitmap?.close();
  bitmap = null;
  engine.reset();
  window.clearTimeout(childTimer);
  window.clearTimeout(elementTimer);
  elementCache.clear();
  set({ ...initialOverlay() });
}

function cancel() {
  const s = get().session;
  if (s) void capture.cancel(s.sessionId);
}

/** `shape`：自由选区的轮廓。不传 = 普通矩形选区（输入宽高、套比例、Ctrl+A 之类都会变回矩形） */
function setSelection(sel: Rect | null, phase = get().phase, shape: Point[] | null = null) {
  // 选区一动，二维码的框和结果就对不上了，收起来
  // 识别表格 / 公式的卡片也一样（正在识别的，结果回来时发现对不上就丢掉）
  set(get().codes || get().structured ? { selection: sel, phase, shape, codes: null, structured: null } : { selection: sel, phase, shape });
  engine.setClip(sel);
}

const isTextIntent = (intent: CaptureIntent | undefined) => intent === 'ocr' || intent === 'translate';

function enterEditing(sel: Rect, shape: Point[] | null = null) {
  setSelection(sel, 'editing', shape);
  const s = get().session;
  if (!s) return;
  void broadcast('capture-active-monitor', { sessionId: s.sessionId, monitorId: MONITOR_ID });
  // 截图识字：框好就直接出结果（设置里可关，关了就和普通截图一样先进编辑态）；
  // 截图翻译：就在选区里原位显示译文
  if (s.intent === 'translate' && s.settings.ocrInstant) void translateInPlace();
  else if (s.intent === 'ocr' && s.settings.ocrInstant) void finish('ocr');
}

function clearSelection() {
  engine.reset();
  set({ selection: null, phase: 'detect', tool: null, hover: get().cursor ? hoverAt(get().cursor!) : null });
  const s = get().session;
  if (s) void broadcast('capture-active-monitor', { sessionId: s.sessionId, monitorId: null });
}

function intentAction(intent: CaptureIntent): FinishAction {
  return intent === 'normal' ? 'copy' : intent;
}

async function finish(action: FinishAction | 'cancel') {
  const { session, selection, busy } = get();
  if (!session || busy) return;
  if (action === 'cancel') {
    cancel();
    return;
  }
  if (!selection) return;
  if (engine.text) engine.commitText();
  rememberSelection(MONITOR_ID, selection);
  set({ busy: true, sizePanel: false });
  try {
    const layer = await engine.exportLayer();
    await capture.finish(
      {
        sessionId: session.sessionId,
        monitorId: MONITOR_ID,
        rect: selection,
        action,
        annotationAt: layer?.at ?? null,
        shape: get().shape?.map((q): [number, number] => [q.x, q.y]) ?? null,
      },
      layer?.png ?? new Uint8Array(),
    );
  } catch (err) {
    notify.error(err);
    reportError('capture-finish', err);
  } finally {
    set({ busy: false });
  }
}

function onAction(id: ActionId) {
  const s = get().session;
  if (!s) return;
  if (id === 'done') void finish(s.intent === 'translate' ? 'copy' : intentAction(s.intent));
  else if (id === 'translate') void translateInPlace();
  else if (id === 'qr') void scanCodes();
  else if (id === 'table' || id === 'formula') void recognizeStructure(id);
  else if (id === 'undo') engine.undo();
  else if (id === 'record') enterRecordPrep();
  else void finish(id === 'cancel' ? 'cancel' : id === 'save' ? 'save' : id);
}

/** 点了「录制」：先进录制准备（工具条换成录制的、下面一条录制栏），点开始才录 */
function enterRecordPrep() {
  if (engine.text) engine.commitText();
  engine.select(null);
  set({ recordPrep: true, tool: null, toolbarGroup: null, codes: null, structured: null, sizePanel: false, recordPanel: null });
}

let countdownTimer: number | undefined;

function exitRecordPrep() {
  window.clearInterval(countdownTimer);
  set({ recordPrep: false, countdown: null, recordPanel: null });
}

/** 开始录：设置里有倒数就先倒数（选区正中一个大数字），再开始 */
function startRecording() {
  const st = get();
  if (!st.recordPrep || st.countdown != null) return;
  const settings = useSettingsStore.getState().settings;
  // 播放时间用的字形先给 Rust（它画进动图时用）
  if (settings?.record.progress === 'time') sendGlyphs();
  const go = () => {
    window.clearInterval(countdownTimer);
    set({ countdown: null, recordPanel: null });
    void finish('record');
  };
  const n = settings?.record.countdown ?? 0;
  if (n <= 0) return go();
  set({ countdown: n, recordPanel: null });
  countdownTimer = window.setInterval(() => {
    const left = (get().countdown ?? 1) - 1;
    if (left <= 0) go();
    else set({ countdown: left });
  }, 1000);
}

/** 把现在的标注存成模板（模板工具里的"存当前标注"） */
function saveCurrentTemplate(): boolean {
  const t = saveTemplate(engine.annotations, engine.scale);
  if (!t) {
    notify.info(i18n.t('tools.templateNothing'));
    return false;
  }
  set({ options: { ...get().options, template: { selected: t.id } } });
  notify.success(i18n.t('tools.templateSaved'));
  return true;
}

/** 识别选区里的二维码 / 条码，结果显示在选区旁边的卡片里 */
async function scanCodes() {
  const { session, selection, scanning } = get();
  if (!session || !selection || scanning) return;
  set({ scanning: true, codes: null });
  try {
    const codes = await capture.scanCodes(session.sessionId, MONITOR_ID, selection);
    if (get().session?.sessionId !== session.sessionId) return;
    if (!codes.length) notify.info(i18n.t('capture.noCode'));
    else set({ codes });
  } catch (err) {
    notify.error(err);
  } finally {
    set({ scanning: false });
  }
}

/**
 * 选区识别成表格 / 公式，原位盖在选区上（像截图翻译）：表格默认本地识别（几百毫秒、不联网），本地没认出表格时
 * 自动换 AI 再认；公式只有 AI。认的时候选区上扫一道光（和截图翻译同一套），不弹卡片。
 * 原位的表格再点「AI 重新识别」：AI 认出的行列对得上就只换字，样式和位置不变；对不上就按原来的范围画一张表。
 * 认不出、出错只弹一条提示，选区和别的标注都不动。
 */
async function recognizeStructure(kind: 'table' | 'formula', wanted?: 'local' | 'ai') {
  const { session, selection, structured } = get();
  if (!session || !selection || structured?.status === 'running') return;
  if (engine.text) engine.commitText();
  // 和原位翻译盖在同一块地方：先把译文去掉
  if (engine.hasGroup(TRANSLATION_GROUP)) {
    engine.removeGroup(TRANSLATION_GROUP);
    set({ showOriginal: false, moveLabels: false, translatedBlocks: [] });
  }
  const how = kind === 'formula' ? 'ai' : (wanted ?? 'local');
  // 原位的结果上再用 AI 认：结果留着（半透明），认完替换
  const prev = structured?.kind === kind && structured.status === 'done' ? structured : undefined;
  set({
    structured: { kind, engine: how, status: 'running', table: prev?.table, text: prev?.text, region: prev?.region },
    codes: null,
    sizePanel: false,
    toolbarGroup: null,
    trLoading: { rect: selection, lines: [], done: false },
  });
  const stale = () => get().session?.sessionId !== session.sessionId || get().structured?.kind !== kind || get().selection !== selection;
  const settle = (next: CaptureStructured | null) => {
    if (stale()) return;
    const loading = get().trLoading;
    set({ structured: next, trLoading: loading ? { ...loading, done: true } : null });
  };
  const message = (err: unknown) => (err instanceof Error ? err.message : String(err));
  /** 认失败了：结果收掉（或留着原来的），弹一条提示。先看是不是已经过时（收掉之后就判断不了了） */
  const fail = (err: unknown, keep: CaptureStructured | null) => {
    if (stale()) return;
    settle(keep);
    notify.error(err);
  };
  const region = () => capture.regionStyle(session.sessionId, MONITOR_ID, selection).catch(() => undefined);
  /** AI 认表格，套回原位（对得上就只换字，对不上就按原来的范围画一张） */
  const askAi = async (base?: CaptureTable) => {
    const [html, colors] = await Promise.all([capture.structure(session.sessionId, MONITOR_ID, selection, 'table', 'ai'), base ? undefined : region()]);
    const table = (base && mergeAiTable(base, html)) ?? aiTableLayout(html, selection, colors, base);
    if (!table) throw new Error(i18n.t('capture.structured.notFound'));
    return { html, table };
  };
  try {
    if (kind === 'table' && how === 'local') {
      let table: CaptureTable;
      try {
        table = await capture.table(session.sessionId, MONITOR_ID, selection);
      } catch (local) {
        if (stale()) return;
        // 本地没认出（没有格子线、字太乱）：换 AI 认，光接着扫
        try {
          const r = await askAi();
          settle({ kind, engine: 'ai', status: 'done', text: r.html, table: r.table });
        } catch (ai) {
          // AI 也说图里没有表格：就说没认出表格；AI 出错了（超时、没配好）才把原因带上
          const said = message(ai);
          const none = !said || said === message(local) || /没有认出表格|no table/i.test(said);
          fail(new Error(none ? i18n.t('capture.structured.notFound') : i18n.t('capture.structured.bothFailed', { message: said })), null);
        }
        return;
      }
      settle({ kind, engine: how, status: 'done', table });
    } else if (kind === 'table') {
      const r = await askAi(prev?.table);
      settle({ kind, engine: how, status: 'done', text: r.html, table: r.table });
    } else {
      const [text, style] = await Promise.all([
        capture.structure(session.sessionId, MONITOR_ID, selection, kind, how),
        prev?.region ? prev.region : capture.regionStyle(session.sessionId, MONITOR_ID, selection),
      ]);
      settle({ kind, engine: how, status: 'done', text, region: style });
    }
  } catch (err) {
    // 重新认失败：留着原来的结果
    fail(err, prev ?? null);
  }
}

/** 识别结果原位盖在选区上；正在认、还没有结果时只扫光（加载层），不显示别的 */
function StructuredResultLayer({ state, selection, s, viewport }: { state: CaptureStructured; selection: Rect; s: number; viewport: { width: number; height: number } }) {
  const close = () => set({ structured: null });
  const busy = state.status === 'running';
  if (state.kind === 'table' && state.table) {
    return (
      <TableInPlace
        data={state.table}
        selection={selection}
        s={s}
        viewport={viewport}
        busy={busy}
        onAi={() => void recognizeStructure('table', 'ai')}
        onClose={close}
        onDone={cancel}
      />
    );
  }
  if (state.kind === 'formula' && state.region && state.text) {
    return (
      <FormulaInPlace
        text={state.text}
        region={state.region}
        selection={selection}
        s={s}
        viewport={viewport}
        busy={busy}
        onAi={() => void recognizeStructure('formula')}
        onClose={close}
        onDone={cancel}
      />
    );
  }
  return null;
}

/** 译文从上往下依次显现：选区顶上的一行马上开始，最底下的一行晚这么多毫秒 */
const REVEAL_SPREAD = 360;
const revealDelay = (y: number, sel: Rect) => Math.round(Math.min(1, Math.max(0, (y - sel.y) / Math.max(1, sel.height))) * REVEAL_SPREAD);

/** 截图原位翻译：识别选区里的字、翻译，在原位置铺底色写上译文（变成可撤销、可导出的标注）。
 *  等待时：先整块选区上一道光扫过；识别完，要翻的每一行换成和原图同色的占位条，光从条上扫过；
 *  翻完：占位条从上往下化开，译文一行行由虚到实浮上来（TranslateLoading + engine.revealGroup）。 */
async function translateInPlace() {
  const { session, selection, translating } = get();
  if (!session || !selection || translating) return;
  if (engine.text) engine.commitText();
  // 和原位的表格 / 公式盖在同一块地方：先去掉
  set({ translating: true, structured: null, trLoading: { rect: selection, lines: [], done: false } });
  try {
    const r = await capture.translateRegion({ sessionId: session.sessionId, monitorId: MONITOR_ID, rect: selection }, (rows) => {
      if (get().session?.sessionId !== session.sessionId || !get().translating) return;
      const lines = rows.map(([x, y, width, height]): LoadingLine => {
        const c = pixels ? lineColors(pixels, { x, y, width, height }) : null;
        return { x, y, width, height, bg: c?.bg ?? null, fg: c?.fg ?? null, light: c?.light ?? true };
      });
      set({ trLoading: { rect: selection, lines, done: false } });
    });
    if (get().session?.sessionId !== session.sessionId) return;
    engine.removeGroup(TRANSLATION_GROUP);
    engine.addAll(buildLabels(r.blocks, pixels, scale(), selection));
    // 默认锁着：鼠标在译文上是选字，打开"移动"才能拖
    engine.setGroupLocked(TRANSLATION_GROUP, true);
    engine.revealGroup(TRANSLATION_GROUP, (rc) => revealDelay(rc.y, selection));
    set({ showOriginal: false, moveLabels: false, translatedBlocks: r.blocks, translatedText: r.blocks.map((b) => b.text).join('\n') });
  } catch (err) {
    notify.error(err);
  } finally {
    // 同一次更新里转入收尾：分两步的话加载层会先消失一帧再出现，所有动画从头再放一遍
    const cur = get().trLoading;
    set({ translating: false, trLoading: cur ? { ...cur, done: true } : null });
  }
}

/** 把鼠标下的颜色贴到屏幕上（一张小卡片，能看 HEX / RGB / HSL、复制），截图随之结束 */
function pinColor() {
  const { cursor, session } = get();
  if (!cursor || !pixels || !session) return;
  const c = pixels.get(cursor.x, cursor.y);
  if (!c) return;
  void capture.pinColor(session.sessionId, MONITOR_ID, Math.round(cursor.x), Math.round(cursor.y), [c[0], c[1], c[2]]).catch(notify.error);
}

function copyColor() {
  const { cursor, colorFormat } = get();
  if (!cursor || !pixels) return;
  const c = pixels.get(cursor.x, cursor.y);
  if (!c) return;
  const text = formatColor(c, colorFormat);
  capture
    .writeText(text)
    .then(() => notify.success(text))
    .catch(notify.error);
}

// ───────────────────────── 指针 ─────────────────────────

/** 遮罩在屏幕上、却没有截图会话（不该出现的状态）：让后端把它收起来，别挡着用户 */
function strayOverlay() {
  void capture.strayOverlay().catch(() => undefined);
}

function onPointerDown(e: React.PointerEvent) {
  // 录制栏的弹层开着：点别处先收起它
  if (get().recordPanel) set({ recordPanel: null });
  flushMove();
  const st = get();
  if (!st.session) return strayOverlay();
  if (st.busy) return;
  // 选区面板、按钮组的二级条开着时点别处：先收起（它们自己的点击不会冒泡到这里）
  if (st.sizePanel || st.toolbarGroup) set({ sizePanel: false, toolbarGroup: null });
  if (e.button === 2) {
    // 打字时右键和左键一样：提交文字，不退出截图（防止打完字误触右键全没了）
    if (engine.text) {
      engine.commitText();
      swallowMenu = true;
    }
    return;
  }
  if (e.button !== 0) return;
  const p = toPx(e);

  if (st.phase === 'passive' && activeElsewhere !== null) {
    // 另一块屏上已经有选区了：把焦点还给它，键盘操作才有效
    // 不用 setFocus()：它会激活整个应用，主窗口也跟着跑到最前面
    void system.takeFocus(`capture-${activeElsewhere}`);
    return;
  }
  (e.target as Element).setPointerCapture?.(e.pointerId);

  if (st.phase === 'detect') {
    press = { at: p, hover: st.hover, lasso: e.altKey, points: [p] };
    set({ phase: 'pressing' });
    return;
  }
  if (st.phase !== 'editing' || !st.selection) return;

  if (engine.text) {
    // 点别处 = 提交正在输入的文字（不要顺手又开一个新的输入框）
    engine.commitText();
    return;
  }
  const sel = st.selection;
  const handleRadius = 7 * scale();
  // 先看是不是点在已画好的标注上：拖控制点改大小、拖本体挪位置
  const ah = engine.hitHandle(p, handleRadius);
  if (ah) {
    engine.beginTransform(p, ah);
    return;
  }
  if (canPick(st.tool, st.options) && contains(sel, p)) {
    // 单击已有标注（包括文字）= 选中，可拖动、拖控制点缩放；双击文字才进入编辑
    const hit = engine.hit(p, 4 * scale());
    if (hit) {
      engine.select(hit.id);
      engine.beginTransform(p, 'move');
      return;
    }
  }
  engine.select(null);
  if (st.tool) {
    if (!contains(sel, p)) return;
    if (st.tool === 'text') engine.startText(p, st.options);
    else engine.begin(st.tool, p, st.options);
    return;
  }
  const h = hitHandle(sel, p, handleRadius);
  // 自由选区只能整体挪，不能拖边改大小（轮廓是画出来的，拉伸没有意义）
  if (h && !st.shape) drag = { kind: 'resize', handle: h, at: p, orig: sel };
  else if (contains(sel, p)) drag = { kind: 'move', at: p, orig: sel, shape: st.shape };
}

/** 遮罩鼠标穿透、露出真实桌面的那几个状态（长截图、GIF 录制） */
const isLive = (phase: string) => phase === 'longshot' || phase === 'longshot-other' || phase === 'gif' || phase === 'gif-other';

/** 处理鼠标移动要用到的那几个字段（合并时只留最后一个事件的） */
interface MoveInput {
  clientX: number;
  clientY: number;
  shiftKey: boolean;
  altKey: boolean;
}

let pendingMove: MoveInput | null = null;
let moveFrame = 0;

/**
 * 鼠标移动合并到每帧处理一次。高回报率鼠标一秒能来 500–1000 个移动事件，每个都重算选区、重画整个
 * 遮罩的话，低配机的主线程跟不上，拖起来一顿一顿的。画笔 / 手绘马赛克要每一个点（不然线条变成折线），不合并。
 */
function onPointerMove(e: React.PointerEvent) {
  const input = { clientX: e.clientX, clientY: e.clientY, shiftKey: e.shiftKey, altKey: e.altKey };
  if (get().phase === 'editing' && engine.drawing) {
    flushMove();
    handleMove(input);
    return;
  }
  pendingMove = input;
  if (!moveFrame) moveFrame = requestAnimationFrame(flushMove);
}

/** 把还没处理的那次移动立刻处理掉（按下、松开前调用，保证用的是最新位置）。 */
function flushMove() {
  if (moveFrame) {
    cancelAnimationFrame(moveFrame);
    moveFrame = 0;
  }
  const m = pendingMove;
  pendingMove = null;
  if (m) handleMove(m);
}

function handleMove(e: MoveInput) {
  const st = get();
  if (!st.session || st.phase === 'idle' || isLive(st.phase)) return;
  const p = toPx(e);
  set({ cursor: p });

  switch (st.phase) {
    case 'detect':
      set({ hover: hoverAt(p, true) });
      return;
    case 'pressing': {
      if (!press) return;
      const moved = Math.hypot(p.x - press.at.x, p.y - press.at.y) / scale();
      if (moved < DRAG_THRESHOLD_CSS) return;
      if (press.lasso) {
        press.points.push(p);
        setSelection(boundsOf(press.points), 'selecting', [...press.points]);
      } else setSelection(fromPoints(press.at, p), 'selecting');
      return;
    }
    case 'selecting': {
      if (!press) return;
      if (press.lasso) {
        // 自由画：点太密没用，隔一两个像素记一个
        const last = press.points[press.points.length - 1]!;
        if (Math.hypot(p.x - last.x, p.y - last.y) >= 2 * scale()) press.points.push(p);
        setSelection(clampRect(boundsOf(press.points), screenRect()), 'selecting', [...press.points]);
        return;
      }
      const raw = fromPoints(press.at, p);
      const sel = snapEdges(raw, { l: p.x <= press.at.x, r: p.x > press.at.x, t: p.y <= press.at.y, b: p.y > press.at.y }, lines, snapThreshold(e));
      setSelection(clampRect(sel, screenRect()));
      return;
    }
    case 'editing': {
      if (engine.drawing) {
        engine.update(p, e.shiftKey);
        return;
      }
      if (engine.transforming) {
        engine.updateTransform(p, e.shiftKey);
        return;
      }
      if (drag?.kind === 'move') {
        const moved = { ...drag.orig, x: drag.orig.x + p.x - drag.at.x, y: drag.orig.y + p.y - drag.at.y };
        const next = clampMove(snapMove(moved, lines, snapThreshold(e)), screenRect());
        // 自由选区：轮廓跟着外接矩形一起平移
        const dx = next.x - drag.orig.x;
        const dy = next.y - drag.orig.y;
        setSelection(next, undefined, drag.shape?.map((q) => ({ x: q.x + dx, y: q.y + dy })) ?? null);
        return;
      }
      if (drag?.kind === 'resize' && drag.handle) {
        // 锁了宽高比：按比例缩放，不吸附（吸附会把比例拉歪）
        const ratio = get().ratio;
        if (ratio) {
          setSelection(clampRect(resizeWithRatio(drag.orig, drag.handle, p, ratio), screenRect()));
          return;
        }
        const raw = resizeByHandle(drag.orig, drag.handle, p);
        const o = drag.orig;
        // 只吸附真正在动的那几条边（拖过对边翻转后，动的边可能换了一侧）
        const sel = snapEdges(
          raw,
          { l: raw.x !== o.x, r: right(raw) !== right(o), t: raw.y !== o.y, b: bottom(raw) !== bottom(o) },
          lines,
          snapThreshold(e),
        );
        setSelection(clampRect(sel, screenRect()));
        return;
      }
      // 悬停时更新光标形状。'tool' = 用当前工具自己的指针（画笔是跟粗细一样大的圆圈）
      const sel = st.selection;
      let cursor = 'default';
      const ah = engine.hitHandle(p, 7 * scale());
      if (ah) cursor = handleCursor(ah);
      else if (sel && contains(sel, p) && canPick(st.tool, st.options) && engine.hit(p, 4 * scale())) cursor = 'move';
      else if (st.tool) cursor = sel && contains(sel, p) ? 'tool' : 'default';
      else if (sel) {
        const h = hitHandle(sel, p, 7 * scale());
        cursor = h ? HANDLE_CURSORS[h] : contains(sel, p) ? 'move' : 'default';
      }
      if (cursor !== st.cursorStyle) set({ cursorStyle: cursor });
      return;
    }
    default:
      return;
  }
}

function onPointerUp() {
  flushMove();
  const st = get();
  if (st.phase === 'pressing') {
    // 单击（移动 < 4px）= 采纳当前高亮区域。截图识字没有高亮区域，单击不算数，接着等用户拖
    const target = press?.hover ?? (isTextIntent(st.session?.intent) ? null : screenRect());
    press = null;
    if (target) enterEditing(target);
    else set({ phase: 'detect' });
    return;
  }
  if (st.phase === 'selecting' && st.selection) {
    const lasso = press?.lasso ? press.points : null;
    press = null;
    // 自由画的轮廓太小（几乎没拖）就当没画，回到等待框选
    if (lasso && (lasso.length < 3 || st.selection.width < 8 || st.selection.height < 8)) {
      setSelection(null, 'detect');
      return;
    }
    enterEditing(st.selection, lasso);
    return;
  }
  if (engine.drawing) engine.end();
  if (engine.transforming) engine.endTransform();
  drag = null;
}

/** 画笔 / 手绘马赛克时滚轮调粗细，指针的圆圈跟着变 */
function onWheel(e: React.WheelEvent) {
  const st = get();
  if (st.phase === 'detect') {
    stepElementLevel(e.deltaY);
    return;
  }
  if (st.phase !== 'editing' || !st.tool) return;
  const o = st.options;
  const dir = e.deltaY < 0 ? 1 : -1;
  const clamp = (v: number, [lo, hi]: [number, number]) => Math.max(lo, Math.min(hi, v));
  if (st.tool === 'pen') set({ options: { ...o, pen: { ...o.pen, lineWidth: clamp(o.pen.lineWidth + dir, PEN_RANGE) } } });
  else if (st.tool === 'mosaic' && o.mosaic.shape === 'brush') {
    set({ options: { ...o, mosaic: { ...o.mosaic, brushSize: clamp(o.mosaic.brushSize + dir * 4, BRUSH_RANGE) } } });
  }
}

function onDoubleClick(e: React.MouseEvent) {
  const st = get();
  if (st.phase !== 'editing' || !st.selection || !st.session) return;
  // 双击文字 = 改字
  const hit = engine.hit(toPx(e), 4 * scale());
  if (hit?.kind === 'text') {
    engine.editText(hit.id);
    return;
  }
  if (!st.tool && !hit && contains(st.selection, toPx(e))) void finish(st.session.intent === 'translate' ? 'copy' : intentAction(st.session.intent));
}

function onContextMenu(e: React.MouseEvent) {
  e.preventDefault();
  const st = get();
  if (!st.session) return;
  if (swallowMenu) {
    swallowMenu = false;
    return;
  }
  // 在输入框里点右键：什么都不做，接着打字
  if ((e.target as Element).closest?.('.an-text-input')) return;
  if (engine.text) {
    engine.commitText();
    return;
  }
  // 默认右键什么都不做；设置里可改成"直接退出"（微信）或"先取消选区、再右键才退出"（Snipaste）
  const mode = st.session.settings.rightClick;
  if (mode === 'none') return;
  if (mode === 'cancelSelection' && st.selection) clearSelection();
  else cancel();
}

// ───────────────────────── 键盘 ─────────────────────────

function nudge(dx: number, dy: number, resize: boolean) {
  const sel = get().selection;
  if (!sel) return;
  const next = resize
    ? clampRect({ ...sel, width: Math.max(1, sel.width + dx), height: Math.max(1, sel.height + dy) }, screenRect())
    : clampMove({ ...sel, x: sel.x + dx, y: sel.y + dy }, screenRect());
  setSelection(next);
}

function onKeyDown(e: KeyboardEvent) {
  const st = get();
  if (!st.session) return strayOverlay();
  if (st.phase === 'idle' || isLive(st.phase)) return;
  if (engine.text) return; // 文字输入框自己处理
  // 输入框里打字（选区面板、水印文字）：字母不能当成工具快捷键，回车也不是完成截图
  if ((e.target as Element | null)?.closest?.('input, textarea, [contenteditable="true"], .cap-panel')) return;
  // 角度标注画了一条边、在等第二条：Esc 只放弃这一笔
  if (e.key === 'Escape' && engine.waitingSecondClick) {
    e.preventDefault();
    engine.cancelDraft();
    return;
  }
  const key = e.key;
  const ctrl = e.ctrlKey || e.metaKey;

  if (key === 'Escape') {
    e.preventDefault();
    // 录制准备：先收起弹层，再停掉倒数 / 退回普通截图
    if (st.recordPrep) {
      if (st.recordPanel) set({ recordPanel: null });
      else exitRecordPrep();
      return;
    }
    // 先收起二维码结果 / 选区面板，再取消选中的标注，再按一次才退出截图
    if (st.codes || st.sizePanel || st.structured || st.toolbarGroup) set({ codes: null, sizePanel: false, structured: null, toolbarGroup: null });
    else if (engine.selected) engine.select(null);
    else cancel();
    return;
  }
  if (st.recordPrep) {
    // 录制准备里：回车 / ⌘G 开始录，别的键（画图工具的快捷键）都不管
    if (key === 'Enter' || (ctrl && key.toLowerCase() === 'g')) {
      e.preventDefault();
      startRecording();
    }
    return;
  }
  if (key === 'Enter') {
    e.preventDefault();
    if (st.phase === 'detect' && st.hover) enterEditing(st.hover);
    if (get().selection) void finish(st.session.intent === 'translate' ? 'copy' : intentAction(st.session.intent));
    return;
  }
  if ((key === 'Delete' || key === 'Backspace') && st.phase === 'editing' && engine.selected) {
    e.preventDefault();
    engine.deleteSelected();
    return;
  }
  if (ctrl && key.toLowerCase() === 'a') {
    e.preventDefault();
    enterEditing(screenRect());
    return;
  }
  if ((st.phase === 'detect' || st.phase === 'selecting' || st.phase === 'pressing') && key.toLowerCase() === 'c' && !ctrl) {
    e.preventDefault();
    if (e.shiftKey) {
      const order = ['hex', 'rgb', 'hsl'] as const;
      set({ colorFormat: order[(order.indexOf(st.colorFormat) + 1) % order.length]! });
    } else copyColor();
    return;
  }
  if ((st.phase === 'detect' || st.phase === 'selecting' || st.phase === 'pressing') && key.toLowerCase() === 'p' && !ctrl) {
    e.preventDefault();
    pinColor();
    return;
  }
  if (st.phase !== 'editing') return;

  if (ctrl && key.toLowerCase() === 'z') {
    e.preventDefault();
    engine.undo();
    return;
  }
  // 选中的标注调图层：⌘] 上移、⌘[ 下移，加 Shift 直接到最上 / 最下
  if (ctrl && engine.selected && (e.code === 'BracketRight' || e.code === 'BracketLeft')) {
    e.preventDefault();
    const up = e.code === 'BracketRight';
    engine.reorderSelected(e.shiftKey ? (up ? 'top' : 'bottom') : up ? 'up' : 'down');
    return;
  }
  if (ctrl && key.toLowerCase() === 's') return void (e.preventDefault(), finish('save'));
  if (ctrl && key.toLowerCase() === 'p') return void (e.preventDefault(), finish('pin'));
  if (ctrl && key.toLowerCase() === 'g') return void (e.preventDefault(), enterRecordPrep());
  if (ctrl && key.toLowerCase() === 't') return void (e.preventDefault(), translateInPlace());
  if (ctrl && key.toLowerCase() === 'c') {
    e.preventDefault();
    // 在译文 / 原文上选了字：复制选中的字，不是完成截图
    const picked = selectedTranslationText();
    if (picked) {
      void capture
        .writeText(picked)
        .then(() => notify.success(i18n.t('capture.textCopied')))
        .catch(notify.error);
      return;
    }
    return void finish('copy');
  }

  const arrows: Record<string, [number, number]> = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
  const dir = arrows[key];
  if (dir) {
    e.preventDefault();
    const step = e.shiftKey ? 10 : 1;
    nudge(dir[0] * step, dir[1] * step, ctrl);
    return;
  }
  if (!ctrl && !e.altKey) {
    const tool = (Object.keys(TOOL_KEYS) as Tool[]).find((t) => TOOL_KEYS[t] === key.toUpperCase());
    if (tool) {
      e.preventDefault();
      if (tool === 'mosaic' && !st.pixelsReady) return;
      engine.select(null);
      set({ tool: st.tool === tool ? null : tool, cursorStyle: 'tool' });
    }
  }
}

// ───────────────────────── 渲染 ─────────────────────────

// 拖选区时这里每一帧都在变，所以遮罩和选区框都拆成小块、只靠 transform 摆位：
//
// - 压暗遮罩：洞的上下左右各一块纯色，都是一小块独立合成层用 scale 拉到位，拖动时一个像素都
//   不用重画。最早是一整块全屏元素配 clip-path 挖洞，每动一下全屏（4K 屏就是 800 多万像素）重画一遍。
//   四块直接给宽高也不行：WebKit 里图层一变大小就整块重新光栅化。
// - 选区框：四条细边（圆角时再加四个角）。一个带 outline / border 的元素范围就是整个选区，
//   每动一下要把选区那么大的一块重新光栅化 —— 实测是拖动时最大的一项开销，选区越大越卡。
//
// Chromium（Windows）扛得住原来的写法；WebKit（macOS）在 4K / 5K、高刷新率的屏上会掉帧。
// 坐标都是物理像素换算过来的，正好落在设备像素上，四块拼缝处不会有亮线。

/**
 * 拉伸用的底块边长（CSS 像素，和 capture.css 里 .cap-mask / .cap-edge--fill 的宽高一致）。
 * 不能是 1：150% 缩放下 1 个 CSS 像素是 1.5 个物理像素，底块被对齐成 2×2，拉伸之后整块大三分之一，
 * 遮罩和框线全部错位。100 在 125% / 150% / 175% 下都是整数个物理像素。
 */
const STRETCH_BASE = 100;
/** 把底块拉成 w×h（CSS 像素）的 scale 写法 */
const stretch = (w: number, h: number) => `scale(${Math.max(0, w) / STRETCH_BASE}, ${Math.max(0, h) / STRETCH_BASE})`;

/** 压暗遮罩。`hole` 是不压暗的那块（选区或悬停高亮的窗口），本屏局部物理坐标。 */
function Mask({
  hole,
  s,
  radius,
  opacity,
  viewport,
}: {
  hole: Rect | null;
  s: number;
  radius: number;
  opacity: number;
  viewport: { width: number; height: number };
}) {
  // 完全不压暗（截图识字默认如此）就没什么可画的
  if (opacity <= 0) return null;
  const color = `rgba(0,0,0,${opacity})`;
  const { width: w, height: h } = viewport;
  const piece = (key: string, x: number, y: number, pw: number, ph: number) => (
    <div key={key} className="cap-mask" style={{ background: color, transform: `translate(${x}px, ${y}px) ${stretch(pw, ph)}` }} />
  );
  if (!hole) return piece('all', 0, 0, w, h);
  const x1 = hole.x / s;
  const y1 = hole.y / s;
  const x2 = right(hole) / s;
  const y2 = bottom(hole) / s;
  // 圆角框配圆角洞，不然四个角会露出一小块没压暗的直角
  const r = Math.min(radius, (x2 - x1) / 2, (y2 - y1) / 2);
  const corner = (key: string, x: number, y: number, at: string) => (
    <div
      key={key}
      className="cap-mask-corner"
      style={{
        width: r,
        height: r,
        transform: `translate(${x}px, ${y}px)`,
        background: `radial-gradient(circle at ${at}, transparent ${r - 0.5}px, ${color} ${r + 0.5}px)`,
      }}
    />
  );
  return (
    <>
      {piece('t', 0, 0, w, y1)}
      {piece('b', 0, y2, w, h - y2)}
      {piece('l', 0, y1, x1, y2 - y1)}
      {piece('r', x2, y1, w - x2, y2 - y1)}
      {r > 0 && (
        <>
          {corner('tl', x1, y1, '100% 100%')}
          {corner('tr', x2 - r, y1, '0 100%')}
          {corner('br', x2 - r, y2 - r, '0 0')}
          {corner('bl', x1, y2 - r, '100% 0')}
        </>
      )}
    </>
  );
}

/**
 * 自由选区：外接矩形里、轮廓外面的部分也压暗（矩形外面已经由 Mask 压暗了），再沿轮廓描一圈框色。
 * 用一个盖住外接矩形的 SVG：矩形加轮廓按奇偶规则填充，正好填的是"矩形减轮廓"。
 */
function ShapeOutline({ shape, rect, s, opacity }: { shape: Point[]; rect: Rect; s: number; opacity: number }) {
  const ox = rect.x;
  const oy = rect.y;
  const pts = shape.map((q) => `${((q.x - ox) / s).toFixed(1)},${((q.y - oy) / s).toFixed(1)}`).join(' L');
  const w = rect.width / s;
  const h = rect.height / s;
  return (
    <svg className="cap-shape" style={{ left: ox / s, top: oy / s, width: w, height: h }} viewBox={`0 0 ${w} ${h}`}>
      {opacity > 0 && <path d={`M0,0 H${w} V${h} H0 Z M${pts} Z`} fill={`rgba(0,0,0,${opacity})`} fillRule="evenodd" />}
      <path d={`M${pts} Z`} className="cap-shape__line" />
    </svg>
  );
}

/** 框外面那圈暗边的宽度（CSS 像素）。2x 屏上正好一个物理像素 */
const HALO = 0.5;

/**
 * 选区框 / 悬停高亮框。`inside`：框画在矩形内侧（悬停高亮）；否则画在外侧，不遮住选区边缘的真实像素。
 * 颜色、线型来自 .cap-root 上的 --cap-frame-* 变量，这里只管几何。
 *
 * 每条边、每个角都是两层：下面一条略宽的半透明暗边，上面才是线 —— 白框压在白色内容上也看得清。
 * 实线的边和压暗遮罩一样，是一小块合成层用 scale 拉到位，拖动时不重画；角块大小固定，只挪位置。
 * 只有虚线 / 点线没法拉伸，那几条边用真实尺寸的 border 画，每帧要重画细细的一条。
 *
 * 试过又放弃的写法（WebKit 上都慢，量过）：
 * - 一个带 outline 的元素：范围是整个选区，每帧重新光栅化这么大一块
 * - 每小块各带一个 filter: drop-shadow 当暗边：八个滤镜的开销和上面那种差不多
 * - 真实尺寸的小块各自 will-change：每帧给八个尺寸在变的图层重新分配缓冲，反而更慢
 */
function FrameBox({
  rect,
  s,
  width,
  radius,
  solid,
  corners,
  inside,
}: {
  rect: Rect;
  s: number;
  width: number;
  radius: number;
  solid: boolean;
  /** 只画四个角（截图识字默认的样子）：每个角是一段圆弧带两条短臂，四条边不画 */
  corners?: boolean;
  inside?: boolean;
}) {
  const x = rect.x / s;
  const y = rect.y / s;
  const w = rect.width / s;
  const h = rect.height / s;
  const t = width;
  // 框的外沿
  const ox = inside ? x : x - t;
  const oy = inside ? y : y - t;
  const ow = inside ? w : w + 2 * t;
  const oh = inside ? h : h + 2 * t;
  const r = Math.min(radius, w / 2, h / 2);
  // 外沿的圆角半径；直角框没有角块，横边一直画到外角，竖边夹在两条横边之间
  const ro = r > 0 ? (inside ? r : r + t) : 0;
  const len = (v: number) => Math.max(0, v);
  // 四角样式：角块 = 圆弧 + 两条短臂，边长 L。选区很小时收短，四个角不碰到一起
  const arm = ro > 0 ? Math.max(10, Math.min(28, ro * 0.9)) : 16;
  const L = Math.max(t, Math.min(ro + arm, ow / 2 - 3, oh / 2 - 3));

  /** 四角样式的一圈：只有四个角块。暗边比线四周各多出 g。 */
  const bracketRing = (kind: 'halo' | 'line', g: number) => {
    const size = L + 2 * g;
    const corner = (name: string, px: number, py: number) => (
      <span
        key={name}
        className={`cap-corner cap-corner--bracket cap-corner--${name} cap-corner--${kind}`}
        style={{ width: size, height: size, transform: `translate(${px}px, ${py}px)` }}
      />
    );
    return (
      <div className="cap-ring" style={{ '--cap-edge': `${t + g}px`, '--cap-corner-r': `${Math.min(ro > 0 ? ro + g : 0, size)}px` } as React.CSSProperties}>
        {corner('tl', ox - g, oy - g)}
        {corner('tr', ox + ow - L - g, oy - g)}
        {corner('br', ox + ow - L - g, oy + oh - L - g)}
        {corner('bl', ox - g, oy + oh - L - g)}
      </div>
    );
  };
  if (corners) {
    return (
      <>
        {bracketRing('halo', HALO)}
        {bracketRing('line', 0)}
      </>
    );
  }

  /** 一圈框的八块。`g` / `gi`：比线本身向外 / 向里多出多少（暗边才多出来，线本身是 0）。 */
  const ring = (kind: 'halo' | 'line', g: number, gi: number) => {
    const tt = t + g + gi;
    const rr = ro > 0 ? ro + g : 0;
    // 直角时横边要盖住外角，所以两头各多出 g；圆角时横边只到角块为止
    const hx = ro > 0 ? ox + ro : ox - g;
    const hLen = len(ro > 0 ? ow - 2 * ro : ow + 2 * g);
    const vy = ro > 0 ? oy + ro : oy + t + gi;
    const vLen = len(ro > 0 ? oh - 2 * ro : oh - 2 * (t + gi));
    const filled = kind === 'halo' || solid;
    const edge = (key: string, dir: 'h' | 'v', px: number, py: number, length: number) =>
      filled ? (
        <span
          key={key}
          className={`cap-edge cap-edge--fill cap-edge--${kind}`}
          style={{ transform: `translate(${px}px, ${py}px) ${dir === 'h' ? stretch(length, tt) : stretch(tt, length)}` }}
        />
      ) : (
        <span
          key={key}
          className={`cap-edge cap-edge--${dir}`}
          style={{ [dir === 'h' ? 'width' : 'height']: length, transform: `translate(${px}px, ${py}px)` }}
        />
      );
    const corner = (name: string, px: number, py: number) => (
      <span key={name} className={`cap-corner cap-corner--${name} cap-corner--${kind}`} style={{ width: rr, height: rr, transform: `translate(${px}px, ${py}px)` }} />
    );
    return (
      <div className="cap-ring" style={{ '--cap-edge': `${tt}px` } as React.CSSProperties}>
        {edge('t', 'h', hx, oy - g, hLen)}
        {edge('b', 'h', hx, oy + oh - t - gi, hLen)}
        {edge('l', 'v', ox - g, vy, vLen)}
        {edge('r', 'v', ox + ow - t - gi, vy, vLen)}
        {rr > 0 && [corner('tl', ox - g, oy - g), corner('tr', ox + ow - ro, oy - g), corner('br', ox + ow - ro, oy + oh - ro), corner('bl', ox - g, oy + oh - ro)]}
      </div>
    );
  };
  return (
    <>
      {/* 画在选区外侧的框，暗边只往外多出一圈：选区贴着屏幕边时线在屏幕外，里边要是也有暗边，就只剩一条黑线 */}
      {ring('halo', HALO, inside ? HALO : 0)}
      {ring('line', 0, 0)}
    </>
  );
}

/** 选区框样式 → CSS 变量（选区框、悬停框、拖柄共用）。 */
function frameVars(f: FrameStyle | undefined): React.CSSProperties {
  const style = f ?? { color: 'accent', width: 3, style: 'solid', radius: 0, handles: 'bar' };
  return {
    '--cap-frame-color': style.color === 'accent' ? 'var(--cn-accent)' : style.color,
    // 完成按钮用框的颜色当底色，上面的勾按底色深浅选黑或白
    '--cap-frame-fg': style.color === 'accent' || readableOn(style.color) === 'white' ? '#fff' : 'rgba(0,0,0,0.85)',
    '--cap-frame-width': `${style.width}px`,
    // 四角样式的角块也是实线画的
    '--cap-frame-style': style.style === 'corners' ? 'solid' : style.style,
    '--cap-frame-radius': `${style.radius}px`,
  } as React.CSSProperties;
}

function SizeHint({
  rect,
  s,
  viewport,
  interactive,
}: {
  rect: Rect;
  s: number;
  viewport: { width: number; height: number };
  /** 编辑态：点它打开选区面板 */
  interactive?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const size = useElementSize(ref, { width: 90, height: 22 });
  const panel = useOverlay((x) => x.sizePanel);
  const ratio = useOverlay((x) => x.ratio);
  const session = useOverlay((x) => x.session);
  // 位置每帧跟着选区走；数字隔一会儿才换（每换一次都要重画文字）
  const text = useThrottled(`${rect.width} × ${rect.height}`, CONTENT_INTERVAL);
  const pos = placeSizeHint({ x: rect.x / s, y: rect.y / s, width: rect.width / s, height: rect.height / s }, size, viewport);
  // 面板放在标签下面；离屏幕底边太近就放上面
  const below = pos.y + 140 < viewport.height;
  return (
    <div
      ref={ref}
      className={clsx('cap-size cn-numeric', interactive && 'cap-size--interactive')}
      style={{ transform: `translate(${pos.x}px, ${pos.y}px)` }}
      onPointerDown={interactive ? (e) => e.stopPropagation() : undefined}
      onClick={interactive ? () => set({ sizePanel: !get().sizePanel }) : undefined}
      title={interactive ? i18n.t('capture.panel.open') : undefined}
    >
      {ratio !== null && interactive && <Lock size={9} strokeWidth={2} className="cap-size__lock" aria-hidden />}
      {text}
      {interactive && panel && session && (
        <div className={clsx('cap-panel-anchor', !below && 'cap-panel-anchor--above')}>
          <SelectionPanel
            selection={rect}
            bounds={screenRect()}
            monitor={MONITOR_ID}
            ratio={ratio}
            onChange={(r) => setSelection(r)}
            onRatio={(r) => set({ ratio: r })}
          />
        </div>
      )}
    </div>
  );
}

function Handles({ rect, s, radius, dots }: { rect: Rect; s: number; radius: number; dots: boolean }) {
  // 圆角框的四个角点沿 45° 收到圆弧上，不悬在框外
  const inset = Math.min(radius, rect.width / s / 2, rect.height / s / 2) * (1 - Math.SQRT1_2);
  return (
    <>
      {HANDLES.map((h) => {
        const p = handlePoint(rect, h);
        const dx = h.includes('w') ? inset : h.includes('e') ? -inset : 0;
        const dy = h.includes('n') ? inset : h.includes('s') ? -inset : 0;
        const corner = h.length === 2;
        // 框线是 outline，画在选区外面一圈；拖柄要落在线的正中，往外挪半个线宽
        const sx = h.includes('w') ? -1 : h.includes('e') ? 1 : 0;
        const sy = h.includes('n') ? -1 : h.includes('s') ? 1 : 0;
        // 设置里选了小圆点：四条边中间也画成和角上一样的圆点
        const kind = corner || dots ? 'corner' : sy !== 0 ? 'h' : 'v';
        return (
          <span
            key={h}
            className={`cap-handle cap-handle--${kind}`}
            style={
              {
                left: p.x / s + (corner ? dx : 0),
                top: p.y / s + (corner ? dy : 0),
                cursor: HANDLE_CURSORS[h],
                '--sx': sx,
                '--sy': sy,
              } as React.CSSProperties
            }
          />
        );
      })}
    </>
  );
}

/** 原位翻译后浮在选区右上角的小条（左上角是尺寸提示）：译文 / 原文切换、复制译文、去掉译文 */
function TranslationBar({ css }: { css: { x: number; y: number; width: number } }) {
  const { t } = useTranslation();
  useEngineVersion(engine);
  const showOriginal = useOverlay((x) => x.showOriginal);
  const moveLabels = useOverlay((x) => x.moveLabels);
  const text = useOverlay((x) => x.translatedText);
  const ref = useRef<HTMLDivElement>(null);
  const size = useElementSize(ref, { width: 220, height: 34 });
  if (!engine.hasGroup(TRANSLATION_GROUP)) return null;
  const toggle = () => {
    engine.setGroupHidden(TRANSLATION_GROUP, !showOriginal);
    set({ showOriginal: !showOriginal });
  };
  return (
    <div
      ref={ref}
      className="cap-trbar cn-glass"
      style={{ transform: `translate(${Math.max(0, css.x + css.width - size.width)}px, ${Math.max(0, css.y - size.height - 6)}px)` }}
      onPointerDown={(e) => e.stopPropagation()}
    >
      <button type="button" className={showOriginal ? '' : 'is-on'} onClick={() => showOriginal && toggle()}>
        <Languages size={14} strokeWidth={1.75} />
        {t('capture.translated')}
      </button>
      <button type="button" className={showOriginal ? 'is-on' : ''} onClick={() => !showOriginal && toggle()}>
        <Eye size={14} strokeWidth={1.75} />
        {t('capture.original')}
      </button>
      {/* 默认鼠标在字上是选字；打开这个才能拖动译文挪位置（看原文时没有可挪的） */}
      <button
        type="button"
        className={moveLabels && !showOriginal ? 'is-on' : ''}
        title={t('capture.moveLabels')}
        aria-pressed={moveLabels}
        disabled={showOriginal}
        onClick={() => {
          engine.setGroupLocked(TRANSLATION_GROUP, moveLabels);
          set({ moveLabels: !moveLabels });
        }}
      >
        <Move size={14} strokeWidth={1.75} />
      </button>
      <span className="cap-trbar__sep" />
      <button
        type="button"
        title={t('capture.copyTranslation')}
        onClick={() =>
          void capture
            .writeText(text)
            .then(() => notify.success(t('capture.translationCopied')))
            .catch(notify.error)
        }
      >
        <Copy size={14} strokeWidth={1.75} />
      </button>
      <button
        type="button"
        title={t('capture.removeTranslation')}
        onClick={() => {
          engine.removeGroup(TRANSLATION_GROUP);
          set({ showOriginal: false, moveLabels: false, translatedBlocks: [] });
        }}
      >
        <X size={14} strokeWidth={1.75} />
      </button>
    </div>
  );
}

/** 原位翻译后叠在选区上的可选文字（见 TranslationText）。翻译中、打开了"移动"时不显示 */
function TranslationTextLayer({ s }: { s: number }) {
  useEngineVersion(engine);
  const showOriginal = useOverlay((x) => x.showOriginal);
  const moveLabels = useOverlay((x) => x.moveLabels);
  const translating = useOverlay((x) => x.translating);
  const blocks = useOverlay((x) => x.translatedBlocks);
  if (translating || !engine.hasGroup(TRANSLATION_GROUP) || (moveLabels && !showOriginal)) return null;
  return <TranslationText engine={engine} blocks={blocks} showOriginal={showOriginal} s={s} />;
}

/** 翻译中的加载动画，翻完后再留一会儿做收尾（占位条化开，和画布上译文的显现对齐） */
const LOADING_OUTRO_MS = REVEAL_SPREAD + 640;

function TranslateLoading({ s }: { s: number }) {
  const { t } = useTranslation();
  const loading = useOverlay((x) => x.trLoading);
  useEffect(() => {
    if (!loading?.done) return;
    const timer = window.setTimeout(() => get().trLoading === loading && set({ trLoading: null }), LOADING_OUTRO_MS);
    return () => window.clearTimeout(timer);
  }, [loading]);
  if (!loading) return null;
  const outro = loading.done;
  const sel = loading.rect;
  const sx = sel.x / s;
  const sw = sel.width / s;
  // 扫过去的那道光的宽度：选区宽的一半，太窄的选区也至少这么宽
  const band = Math.max(140, sw * 0.5);
  return (
    <div
      className={clsx('cap-ld', outro && 'cap-ld--out', loading.lines.length > 0 && 'cap-ld--lines')}
      style={{ '--sx': `${sx}px`, '--sw': `${sw}px`, '--band': `${band}px` } as React.CSSProperties}
      role="status"
      aria-label={t('capture.translating')}
    >
      <div className="cap-ld__veil" style={{ left: sx, top: sel.y / s, width: sw, height: sel.height / s, '--bx': `${sx}px` } as React.CSSProperties} />
      {loading.lines.map((l, i) => {
        const x = l.x / s;
        const h = l.height / s;
        // 底色块比字框大一圈才盖得住字的边缘；占位条比字矮一点、两头圆
        const grow = Math.max(2, h * 0.14);
        const bar = Math.max(5, Math.min(h * 0.62, 22));
        return (
          <div
            key={i}
            className="cap-ld__line"
            style={
              {
                left: x - grow,
                top: l.y / s - grow,
                width: l.width / s + grow * 2,
                height: h + grow * 2,
                background: l.bg ?? undefined,
                '--bx': `${x}px`,
                '--d': `${revealDelay(l.y, sel)}ms`,
                '--bar': `${bar}px`,
                '--tone': l.bg && l.fg ? `color-mix(in srgb, ${l.fg} 14%, ${l.bg})` : 'rgba(128, 128, 128, 0.28)',
                '--sheen': l.light || !l.bg || !l.fg ? 'rgba(255, 255, 255, 0.78)' : `color-mix(in srgb, ${l.fg} 30%, ${l.bg})`,
              } as React.CSSProperties
            }
          >
            <span className="cap-ld__bar" />
          </div>
        );
      })}
    </div>
  );
}

/** 当前在译文 / 原文上选中的字（没选、选的不是这层里的字返回空） */
function selectedTranslationText(): string {
  const sel = window.getSelection();
  const text = sel?.toString().trim() ?? '';
  if (!text || !sel?.anchorNode) return '';
  const el = sel.anchorNode instanceof Element ? sel.anchorNode : sel.anchorNode.parentElement;
  return el?.closest('.cap-trtext') ? text : '';
}

/** 选区左上角留给尺寸提示的宽度（CSS 像素），够放下 "3840 × 2160" */
const SIZE_HINT_ROOM = 96;

/** 表格相关的界面元素角色（辅助功能）：选区框住的正好是表格时直接认为有表格 */
const TABLE_ROLES = new Set(['AXTable', 'AXGrid', 'AXOutline']);

/**
 * 「识别」组的提示：选区里有自动识别出来的二维码 → 组按钮换成二维码图标；看起来是表格 → 表格图标。
 * 两样都有时二维码优先（码小、一眼就能用上）。
 */
function useRecognizeHints(rect: Rect): { icon?: ActionIcon; hints: string[] } {
  const autoCodes = useOverlay((x) => x.autoCodes);
  const tableHint = useOverlay((x) => x.tableHint);
  return useMemo(() => {
    const qr = autoCodes.some((c) => contains(rect, { x: c.x + c.width / 2, y: c.y + c.height / 2 }));
    const hints = [...(qr ? ['qr'] : []), ...(tableHint ? ['table'] : [])];
    return { icon: qr ? ACTION_ICONS.qr : tableHint ? ACTION_ICONS.table : undefined, hints };
  }, [autoCodes, tableHint, rect]);
}

/** 选区停下来之后看看里面有没有表格：先看框住的界面元素是不是表格，再让后端按格子线判断 */
function useTableDetection() {
  const selection = useOverlay((x) => x.selection);
  const phase = useOverlay((x) => x.phase);
  const session = useOverlay((x) => x.session);
  useEffect(() => {
    if (phase !== 'editing' || !selection || !session) {
      if (get().tableHint) set({ tableHint: false });
      return;
    }
    const hit = windowsLocal.find((w) => contains(w.rect, { x: selection.x + 1, y: selection.y + 1 }));
    const ax =
      !!hit &&
      elementCache
        .items(hit.handle)
        .some((e) => TABLE_ROLES.has(e.role) && intersectArea(e.rect, selection) >= 0.6 * Math.min(area(e.rect), area(selection)));
    if (ax) {
      set({ tableHint: true });
      return;
    }
    let stale = false;
    const timer = window.setTimeout(() => {
      capture
        .detectTable(session.sessionId, MONITOR_ID, selection)
        .then((yes) => !stale && get().tableHint !== yes && set({ tableHint: yes }))
        .catch(() => {});
    }, 220);
    return () => {
      stale = true;
      window.clearTimeout(timer);
    };
  }, [selection, phase, session]);
}

function intersectArea(a: Rect, b: Rect): number {
  const r = intersect(a, b);
  return r ? area(r) : 0;
}

function Toolbars({
  rect,
  s,
  viewport,
  tools,
  mid,
  order,
  hidden,
  frosted,
  groups,
}: {
  rect: Rect;
  s: number;
  viewport: { width: number; height: number };
  /** 磨砂玻璃样式：工具条底下垫一块系统的毛玻璃（模糊冻结的画面） */
  frosted?: boolean;
  /** 三组按钮各自的顺序（设置里可调）：绘制工具、识字翻译那一组、最右边那一组；可以有按钮组 */
  tools?: string[];
  mid?: string[];
  order?: TailAction[];
  /** 设置里藏起来的按钮 */
  hidden?: string[];
  /** 按钮组 */
  groups?: ToolbarGroup[];
}) {
  const { t } = useTranslation();
  const version = useEngineVersion(engine);
  const tool = useOverlay((x) => x.tool);
  const translating = useOverlay((x) => x.translating);
  const scanning = useOverlay((x) => x.scanning);
  const options = useOverlay((x) => x.options);
  const pixelsReady = useOverlay((x) => x.pixelsReady);
  const bitmapReady = useOverlay((x) => x.bitmapReady);
  const intent = useOverlay((x) => x.session?.intent ?? 'normal');
  const hasStructured = useOverlay((x) => !!x.structured);
  const barRef = useRef<HTMLDivElement>(null);
  const subRef = useRef<HTMLDivElement>(null);
  const bar = useElementSize(barRef, { width: 540, height: 40 });
  const picked0 = engine.selectedAnnotation;
  const hasSub = !!(tool ?? (picked0 ? toolOf(picked0) : null));
  const sub = useElementSize(subRef, { width: 300, height: 40 }, [hasSub]);
  const intentRef = useRef<HTMLDivElement>(null);
  const intentSize = useElementSize(intentRef, { width: 170, height: 26 });
  const openGroup = useOverlay((x) => x.toolbarGroup);
  const groupRef = useRef<HTMLDivElement>(null);
  const groupSize = useElementSize(groupRef, { width: 240, height: 40 }, [openGroup]);
  const recognize = useRecognizeHints(rect);

  const css = { x: rect.x / s, y: rect.y / s, width: rect.width / s, height: rect.height / s };
  const pos = placeToolbar(css, bar, viewport);
  const above = !pos.inside && pos.y < css.y;

  // 磨砂玻璃：毛玻璃是原生的一层，挪它要过一趟进程间调用，跟不上工具条每一帧的位置。
  // 所以工具条在动的时候（拖选区、调大小）先撤掉、用普通样式，停稳了再垫上。
  const [backed, setBacked] = useState(false);
  const backedRef = useRef(false);
  useEffect(() => {
    if (!frosted) return;
    if (backedRef.current) {
      backedRef.current = false;
      setBacked(false);
      void capture.toolbarGlass(null, 0).catch(() => {});
    }
    let stale = false;
    const timer = window.setTimeout(() => {
      const area: [number, number, number, number] = [pos.x * s, pos.y * s, bar.width * s, bar.height * s];
      capture.toolbarGlass(area, Math.round(12 * s)).then(
        () => {
          if (stale) return;
          backedRef.current = true;
          setBacked(true);
        },
        () => {},
      );
    }, 120);
    return () => {
      stale = true;
      window.clearTimeout(timer);
    };
  }, [frosted, pos.x, pos.y, bar.width, bar.height, s]);
  // 工具条收起（截完了、取消了、换成长截图）时撤掉
  useEffect(
    () => () => {
      if (backedRef.current) void capture.toolbarGlass(null, 0).catch(() => {});
      backedRef.current = false;
    },
    [],
  );
  let subY = above ? pos.y - 4 - sub.height : pos.y + bar.height + 4;
  if (subY + sub.height > viewport.height || subY < 0) subY = above ? pos.y + bar.height + 4 : pos.y - 4 - sub.height;
  // 按钮组展开的二级条：和工具的二级条一样放在工具条外侧，对准组按钮
  const group = openGroup ? groups?.find((g) => g.id === openGroup) : undefined;
  const groupAnchor = useGroupAnchor(barRef, group?.id ?? null);
  let groupY = above ? pos.y - 4 - groupSize.height : pos.y + bar.height + 4;
  if (groupY + groupSize.height > viewport.height || groupY < 0) groupY = above ? pos.y + bar.height + 4 : pos.y - 4 - groupSize.height;
  const groupX = Math.max(4, Math.min(pos.x + (groupAnchor ?? bar.width / 2) - groupSize.width / 2, viewport.width - groupSize.width - 4));

  // 长截图入口只在这块屏的选区足够大时有意义；模糊需要位图到位
  const blurPending = tool === 'mosaic' && options.mosaic.mode === 'blur' && !bitmapReady;
  // 选中了已有标注：二级工具条显示它的样式，改了直接作用到它身上
  const picked = engine.selectedAnnotation;
  const pickedTool = picked ? toolOf(picked) : null;
  const subTool = pickedTool ?? tool;
  // 二级条的中心对准对应的工具按钮，贴到屏幕边时再往里收
  const anchor = useToolAnchor(barRef, subTool);
  const subX = Math.max(4, Math.min(pos.x + (anchor ?? bar.width / 2) - sub.width / 2, viewport.width - sub.width - 4));
  const subOptions = picked && pickedTool ? engine.optionsOf(picked, options) : options;

  // 拖选区框、调大小时工具条只是跟着换位置，里面几十个按钮一个都没变。位置直接写到元素上（下面的副作用），
  // 工具条本身只在内容真的变了（换工具、撤销栈、按钮顺序、能不能点）时才重画 —— 165Hz 的屏上每帧只有 6ms，
  // 每动一下都把整条工具条重画一遍是拖动时最大的一笔开销。
  const barClass = clsx(pos.inside && 'cap-toolbar--inside', frosted && backed && 'an-toolbar--backed');
  const canUndo = engine.canUndo;
  const pickTool = (next: Tool | null) => {
    if (engine.text) engine.commitText();
    engine.select(null);
    set({ tool: next, cursorStyle: 'tool', toolbarGroup: null });
  };
  const groupIcons = recognize.icon ? { recognize: recognize.icon } : undefined;
  const groupHints = recognize.hints.length ? { recognize: true } : undefined;
  const toolbar = useMemo(
    () => (
      <Toolbar
        ref={barRef}
        className={barClass}
        tool={tool}
        onTool={pickTool}
        groups={groups}
        openGroup={openGroup}
        onGroup={(id) => set({ toolbarGroup: id })}
        groupIcons={groupIcons}
        groupHints={groupHints}
        canUndo={canUndo}
        onUndo={() => engine.undo()}
        tools={tools}
        hidden={hidden}
        actions={[mid?.length ? mid : MID_ACTIONS, order?.length ? order : TAIL_ACTIONS]}
        onAction={onAction}
        disabledTools={{ mosaic: !pixelsReady }}
        disabledActions={{ translate: translating, qr: scanning }}
      />
    ),
    // version：撤销栈、选中的标注变了工具条上的状态可能跟着变
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [barClass, tool, canUndo, tools, mid, order, groups, openGroup, recognize.icon, recognize.hints.length, pixelsReady, translating, version],
  );
  // 位置：重画之后、上屏之前写上去（重画不会动这个内联样式，但第一次出现时要有）
  useLayoutEffect(() => {
    if (barRef.current) barRef.current.style.transform = `translate(${pos.x}px, ${pos.y}px)`;
  });
  return (
    <>
      {toolbar}
      {group && (
        <GroupBar
          ref={groupRef}
          style={{ transform: `translate(${groupX}px, ${groupY}px)` }}
          group={group}
          tool={tool}
          hidden={hidden}
          hints={recognize.hints}
          disabledActions={{ translate: translating, qr: scanning }}
          disabledTools={{ mosaic: !pixelsReady }}
          onTool={pickTool}
          onAction={onAction}
          onClose={() => set({ toolbarGroup: null })}
        />
      )}
      {subTool && !group && (
        <SubToolbar
          ref={subRef}
          style={{ transform: `translate(${subX}px, ${subY}px)` }}
          tool={subTool}
          options={subOptions}
          onChange={(o) => {
            if (picked && pickedTool) engine.applyOptions(o);
            set({ options: o });
          }}
          onSaveTemplate={saveCurrentTemplate}
        />
      )}
      <TranslationBar css={css} />
      <TranslateLoading s={s} />
      {intent !== 'normal' && !engine.hasGroup(TRANSLATION_GROUP) && !hasStructured && (
        // 选区左上角是尺寸提示，这条提示靠右上角放；选区太窄就排在尺寸提示右边，不压在它上面
        <div
          ref={intentRef}
          className="cap-intent cn-glass-thin"
          style={{
            transform: `translate(${Math.max(0, Math.min(Math.max(css.x + SIZE_HINT_ROOM, css.x + css.width - intentSize.width), viewport.width - intentSize.width))}px, ${Math.max(0, css.y - 6 - intentSize.height)}px)`,
          }}
        >
          {t(`capture.intent.${intent}`)}
        </div>
      )}
      {blurPending && <div className="cap-intent cn-glass-thin">{t('capture.preparing')}</div>}
    </>
  );
}

export default function CaptureView() {
  const committed = useRef<HTMLCanvasElement>(null);
  const drafting = useRef<HTMLCanvasElement>(null);
  const session = useOverlay((x) => x.session);
  const phase = useOverlay((x) => x.phase);
  const selection = useOverlay((x) => x.selection);
  const codes = useOverlay((x) => x.codes);
  const structured = useOverlay((x) => x.structured);
  const recordPrep = useOverlay((x) => x.recordPrep);
  const autoCodes = useOverlay((x) => x.autoCodes);
  const shape = useOverlay((x) => x.shape);
  useTableDetection();
  const hover = useOverlay((x) => x.hover);
  const cursorStyle = useOverlay((x) => x.cursorStyle);
  const tool = useOverlay((x) => x.tool);
  const options = useOverlay((x) => x.options);
  // 水印跟着选项走（开关、文字、字号…），画在标注层上一起导出
  useEffect(() => {
    engine.setWatermark(session ? watermarkOf(options.watermark, engine.scale) : null);
  }, [options.watermark, session]);
  const [windowSize, setViewport] = useState({ width: window.innerWidth, height: window.innerHeight });
  // 不截图时 macOS 上遮罩窗口缩成 1×1 点省内存，铺回整屏后的 resize 事件可能比第一帧晚到，
  // 会话期间按这块屏本身的大小排版
  const viewport = useMemo(
    () =>
      session
        ? { width: session.monitor.bounds.width / session.monitor.scaleFactor, height: session.monitor.bounds.height / session.monitor.scaleFactor }
        : windowSize,
    [session, windowSize],
  );

  useEffect(() => {
    engine.attach(committed.current, drafting.current);
  }, [session?.sessionId]);

  useEffect(() => {
    const unsubs: Promise<() => void>[] = [
      on('capture-session-start', () => void startSession()),
      on('capture-session-end', endSession),
      on('capture-active-monitor', (p) => {
        if (p.sessionId !== get().session?.sessionId || p.monitorId === MONITOR_ID) return;
        activeElsewhere = p.monitorId;
        const phase = get().phase;
        if (p.monitorId !== null && (phase === 'detect' || phase === 'pressing')) set({ phase: 'passive', hover: null });
        if (p.monitorId === null && phase === 'passive') set({ phase: 'detect' });
      }),
      on('longshot-state', (p) => {
        if (!p.active) return;
        if (p.monitorId === MONITOR_ID) {
          set({ phase: 'longshot', selection: p.rect, tool: null, longshot: null });
          engine.reset();
        } else set({ phase: 'longshot-other' });
      }),
      on('longshot-progress', (p) => set({ longshot: p })),
      on('gif-state', (p) => {
        if (!p.active) return;
        if (p.monitorId === MONITOR_ID) {
          set({ phase: 'gif', selection: p.rect, tool: null, gif: null, recordPrep: false, countdown: null, recordPanel: null });
          engine.reset();
        } else set({ phase: 'gif-other' });
      }),
      on('gif-progress', (p) => set({ gif: p })),
      on('capture-hotkey', (intent) => {
        // 录 GIF 时再按一次截图热键 = 录完
        if (get().phase === 'gif') {
          void gif.finish();
          return;
        }
        // 遮罩打开时按 F2/F3：对当前选区直接长截图/识字（全局热键先被系统截走了，由 Rust 转发过来）
        if (intent !== 'normal' && get().phase === 'editing') void finish(intent);
      }),
    ];
    // 页面加载晚于热键时补上会话
    void startSession();
    const onResize = () => setViewport({ width: window.innerWidth, height: window.innerHeight });
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('resize', onResize);
    return () => {
      for (const u of unsubs) void u.then((fn) => fn());
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('resize', onResize);
    };
  }, []);

  const s = session?.monitor.scaleFactor ?? window.devicePixelRatio;
  const opacity =
    phase === 'longshot'
      ? 0.6
      : phase === 'gif'
        ? 0.35
      : isTextIntent(session?.intent)
        ? (session?.settings.ocrMaskOpacity ?? 0.2)
        : (session?.settings.maskOpacity ?? 0.4);
  const hole =
    phase === 'detect' || phase === 'pressing'
      ? hover
      : phase === 'selecting' || phase === 'editing' || phase === 'longshot' || phase === 'gif'
        ? selection
        : null;
  const showMask = phase !== 'longshot-other' && phase !== 'gif-other';
  const frame = isTextIntent(session?.intent) ? session?.settings.ocrFrame : session?.settings.frame;
  const radius = phase === 'longshot' || phase === 'gif' ? 0 : (frame?.radius ?? 0);
  const frameWidth = frame?.width ?? 3;
  const frameSolid = (frame?.style ?? 'solid') === 'solid';
  const frameCorners = frame?.style === 'corners';
  const cfg = session?.settings;
  const pw = session?.monitor.bounds.width ?? 1;
  const ph = session?.monitor.bounds.height ?? 1;

  return (
    <div
      className="cap-root"
      // 自动化测试读预选框用（几何，物理像素）
      data-hover={(phase === 'detect' || phase === 'pressing') && hover ? `${hover.x},${hover.y},${hover.width},${hover.height}` : undefined}
      data-phase={phase}
      style={{
        // 只有框选时是十字；编辑时按工具：画笔、手绘马赛克是圆圈，文字是 I 形，挪动标注是四向箭头
        cursor: phase === 'editing' ? (cursorStyle === 'tool' ? toolCursor(tool, options) : cursorStyle) : phase === 'passive' ? 'default' : 'crosshair',
        ...frameVars(frame),
      }}
      onWheel={onWheel}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onDoubleClick={onDoubleClick}
      onContextMenu={onContextMenu}
    >
      {showMask && <Mask hole={hole} s={s} radius={radius} opacity={opacity} viewport={viewport} />}
      <canvas ref={committed} className="cap-canvas" width={pw} height={ph} />
      <canvas ref={drafting} className="cap-canvas" width={pw} height={ph} />

      {(phase === 'detect' || phase === 'pressing') && hover && (
        <>
          <FrameBox rect={hover} s={s} width={frameWidth + 0.5} radius={radius} solid={frameSolid} corners={frameCorners} inside />
          <SizeHint rect={hover} s={s} viewport={viewport} />
        </>
      )}
      {(phase === 'selecting' || phase === 'editing') && selection && (
        <>
          {shape ? (
            <ShapeOutline shape={shape} rect={selection} s={s} opacity={opacity} />
          ) : (
            <FrameBox rect={selection} s={s} width={frameWidth} radius={radius} solid={frameSolid} corners={frameCorners} />
          )}
          {/* 拖柄从开始拖选区就跟着显示，不是松手才冒出来。四角样式的框本身就是四个角，不再叠拖柄 */}
          {!frameCorners && !engine.drawing && !shape && <Handles rect={selection} s={s} radius={radius} dots={frame?.handles === 'dot'} />}
          <SizeHint rect={selection} s={s} viewport={viewport} interactive={phase === 'editing'} />
        </>
      )}
      {phase === 'editing' && selection && (
        <>
          <TranslationTextLayer s={s} />
          <div className="cap-text-layer">
            <SelectionOverlay engine={engine} displayScale={s} />
            <TextEditor engine={engine} displayScale={s} />
          </div>
          {recordPrep ? (
            <>
              <RecordToolbar rect={selection} s={s} viewport={viewport} live={false} onStart={startRecording} onExit={exitRecordPrep} />
              <MoveHandle rect={selection} s={s} viewport={viewport} live={false} />
              <RecordHints rect={selection} s={s} />
              <CameraFrame rect={selection} s={s} />
              <Countdown rect={selection} s={s} />
            </>
          ) : (
            <Toolbars
              rect={selection}
              s={s}
              viewport={viewport}
              tools={cfg?.toolbarTools}
              hidden={cfg?.toolbarHidden}
              mid={cfg?.toolbarActions}
              order={cfg?.toolbarOrder}
              frosted={cfg?.toolbarStyle === 'frosted'}
              groups={cfg?.toolbarGroups ?? DEFAULT_GROUPS}
            />
          )}
        </>
      )}
      {autoCodes.length > 0 && !codes && (phase === 'detect' || (phase === 'editing' && !tool)) && (
        <CodeBadges codes={autoCodes} s={s} viewport={viewport} onDone={cancel} />
      )}
      {phase === 'editing' && selection && codes && (
        <CodeResults codes={codes} selection={selection} s={s} viewport={viewport} onClose={() => set({ codes: null })} onDone={cancel} />
      )}
      {phase === 'editing' && selection && structured && <StructuredResultLayer state={structured} selection={selection} s={s} viewport={viewport} />}
      {phase === 'longshot' && selection && <LongshotUI rect={selection} s={s} viewport={viewport} />}
      {phase === 'gif' && selection && (
        <>
          <GifUI rect={selection} s={s} />
          <RecordToolbar rect={selection} s={s} viewport={viewport} live onStart={() => {}} onExit={() => void gif.cancel()} />
          <MoveHandle rect={selection} s={s} viewport={viewport} live />
          <KeyHud rect={selection} s={s} />
        </>
      )}
      <MagnifierLayer viewport={viewport} />
    </div>
  );
}

/**
 * 放大镜单独订阅鼠标位置：鼠标每动一下只重渲染它自己，不带着整个遮罩一起。
 */
function MagnifierLayer({ viewport }: { viewport: { width: number; height: number } }) {
  const session = useOverlay((x) => x.session);
  const phase = useOverlay((x) => x.phase);
  const cursor = useOverlay((x) => x.cursor);
  const pixelsReady = useOverlay((x) => x.pixelsReady);
  const colorFormat = useOverlay((x) => x.colorFormat);
  const shown = phase === 'detect' || phase === 'pressing' || phase === 'selecting';
  if (!shown || !cursor || !pixelsReady || !pixels || !session || session.settings.showMagnifier === false) return null;
  return (
    <Magnifier
      pixels={pixels}
      cursor={cursor}
      origin={{ x: session.monitor.bounds.x, y: session.monitor.bounds.y }}
      scale={session.monitor.scaleFactor}
      viewport={viewport}
      format={colorFormat}
      levelHint={phase === 'detect' && hasElementLevels(cursor)}
    />
  );
}

/** 鼠标下面除了窗口还有可选的界面元素（滚轮能切层级） */
function hasElementLevels(p: Point): boolean {
  const hit = windowsLocal.find((w) => contains(w.rect, p));
  return !!hit && elementCandidates(hit, p).length > 0;
}
