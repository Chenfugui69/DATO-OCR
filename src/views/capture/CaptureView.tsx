// 截图遮罩（规格 02）。逐条对齐微信截图的手感。
//
// 这个窗口是透明的：冻结的屏幕画面在它正下方的原生底图窗口里，这里只画压暗层、选区、
// 标注、放大镜和工具条。选区是**真镂空**，洞里看到的就是原始像素。

import './capture.css';
import '@/views/annotate/annotate.css';

import { Window } from '@tauri-apps/api/window';
import { Copy, Eye, Languages, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
import { useTranslation } from 'react-i18next';

import { broadcast, on } from '@/lib/events';
import { formatColor, readableOn } from '@/lib/format';
import { CONTENT_INTERVAL, useElementSize, useThrottled } from '@/lib/hooks';
import { capture, gif, reportError } from '@/lib/ipc';
import { shotUrl, windowLabel } from '@/lib/platform';
import type { CaptureIntent, FinishAction, FrameStyle } from '@/lib/types';
import { Spinner } from '@/ui/controls';
import { notify } from '@/ui/overlays';
import { AnnotationEngine, toolOf } from '@/views/annotate/engine';
import { BRUSH_RANGE, PEN_RANGE, type Tool } from '@/views/annotate/model';
import { bmpSource, type PixelSource } from '@/views/annotate/pixels';
import { canPick, handleCursor, SelectionOverlay, toolCursor } from '@/views/annotate/SelectionOverlay';
import { TextEditor, useEngineVersion } from '@/views/annotate/TextEditor';
import { SubToolbar, TOOL_KEYS, Toolbar, useToolAnchor, type ActionId } from '@/views/annotate/Toolbar';
import { buildLabels, TRANSLATION_GROUP } from '@/views/annotate/translateLayer';

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
  right,
  snapEdges,
  snapLines,
  snapMove,
  type Handle,
  type Point,
  type Rect,
  type SnapLines,
} from './geometry';
import { GifUI } from './GifUI';
import { LongshotUI } from './LongshotUI';
import { Magnifier } from './Magnifier';
import { get, initialOverlay, set, useOverlay } from './store';

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
let press: { at: Point; hover: Rect | null } | null = null;
let drag: { kind: 'move' | 'resize'; handle?: Handle; at: Point; orig: Rect } | null = null;
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
        if (cursor && phase === 'detect') set({ hover: detect(cursor) });
      })
      .catch((err) => {
        childCache.set(handle, []);
        childPending = null;
        reportError('capture-window-children', err);
      });
  }, 80);
}

/** 命中判定：按 Z 序从上到下取第一个包含鼠标的窗口；再细分到面积最小的子控件。 */
function detect(p: Point): Rect {
  const s = get().session;
  const screen = screenRect();
  if (!s?.settings.detectWindows) return screen;
  const hit = windowsLocal.find((w) => contains(w.rect, p));
  if (!hit) return screen;
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
  pixels = null;
  bitmap?.close();
  bitmap = null;
  engine.scale = info.monitor.scaleFactor;
  engine.setBackdrop({ pixels: null, bitmap: null });
  childCache.clear();
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
  if (cursor) set({ hover: detect(cursor) });

  try {
    await capture.overlayReady(info.sessionId, MONITOR_ID);
  } catch (err) {
    reportError('capture-overlay', err);
    // 会话在 Rust 侧，前端失败不收回的话之后每次热键都会被"已在截图中"挡掉
    void capture.cancel(info.sessionId);
    return;
  }
  void loadPixels(info.sessionId, info.imageUrlId);
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
  set({ ...initialOverlay() });
}

function cancel() {
  const s = get().session;
  if (s) void capture.cancel(s.sessionId);
}

function setSelection(sel: Rect | null, phase = get().phase) {
  set({ selection: sel, phase });
  engine.setClip(sel);
}

const isTextIntent = (intent: CaptureIntent | undefined) => intent === 'ocr' || intent === 'translate';

function enterEditing(sel: Rect) {
  setSelection(sel, 'editing');
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
  set({ selection: null, phase: 'detect', tool: null, hover: get().cursor ? detect(get().cursor!) : null });
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
  set({ busy: true });
  try {
    const layer = await engine.exportLayer();
    await capture.finish(
      { sessionId: session.sessionId, monitorId: MONITOR_ID, rect: selection, action, annotationAt: layer?.at ?? null },
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
  else void finish(id === 'cancel' ? 'cancel' : id === 'save' ? 'save' : id);
}

/** 截图原位翻译：识别选区里的字、翻译，在原位置铺底色写上译文（变成可撤销、可导出的标注）。 */
async function translateInPlace() {
  const { session, selection, translating } = get();
  if (!session || !selection || translating) return;
  if (engine.text) engine.commitText();
  set({ translating: true });
  try {
    const r = await capture.translateRegion({ sessionId: session.sessionId, monitorId: MONITOR_ID, rect: selection });
    if (get().session?.sessionId !== session.sessionId) return;
    engine.removeGroup(TRANSLATION_GROUP);
    engine.addAll(buildLabels(r.blocks, pixels, scale(), selection));
    set({ showOriginal: false, translatedText: r.blocks.map((b) => b.text).join('\n') });
  } catch (err) {
    notify.error(err);
  } finally {
    set({ translating: false });
  }
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

function onPointerDown(e: React.PointerEvent) {
  flushMove();
  const st = get();
  if (!st.session || st.busy) return;
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
    void Window.getByLabel(`capture-${activeElsewhere}`).then((w) => w?.setFocus());
    return;
  }
  (e.target as Element).setPointerCapture?.(e.pointerId);

  if (st.phase === 'detect') {
    press = { at: p, hover: st.hover };
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
  if (h) drag = { kind: 'resize', handle: h, at: p, orig: sel };
  else if (contains(sel, p)) drag = { kind: 'move', at: p, orig: sel };
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
      set({ hover: detect(p) });
      return;
    case 'pressing': {
      if (!press) return;
      const moved = Math.hypot(p.x - press.at.x, p.y - press.at.y) / scale();
      if (moved >= DRAG_THRESHOLD_CSS) setSelection(fromPoints(press.at, p), 'selecting');
      return;
    }
    case 'selecting': {
      if (!press) return;
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
        setSelection(clampMove(snapMove(moved, lines, snapThreshold(e)), screenRect()));
        return;
      }
      if (drag?.kind === 'resize' && drag.handle) {
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
    // 单击（移动 < 4px）= 采纳当前高亮区域
    const target = press?.hover ?? screenRect();
    press = null;
    enterEditing(target);
    return;
  }
  if (st.phase === 'selecting' && st.selection) {
    press = null;
    enterEditing(st.selection);
    return;
  }
  if (engine.drawing) engine.end();
  if (engine.transforming) engine.endTransform();
  drag = null;
}

/** 画笔 / 手绘马赛克时滚轮调粗细，指针的圆圈跟着变 */
function onWheel(e: React.WheelEvent) {
  const st = get();
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
  if (!st.session || st.phase === 'idle' || isLive(st.phase)) return;
  if (engine.text) return; // 文字输入框自己处理
  const key = e.key;
  const ctrl = e.ctrlKey || e.metaKey;

  if (key === 'Escape') {
    e.preventDefault();
    // 有选中的标注先取消选中，再按一次才退出截图
    if (engine.selected) engine.select(null);
    else cancel();
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
  if (st.phase !== 'editing') return;

  if (ctrl && key.toLowerCase() === 'z') {
    e.preventDefault();
    engine.undo();
    return;
  }
  if (ctrl && key.toLowerCase() === 's') return void (e.preventDefault(), finish('save'));
  if (ctrl && key.toLowerCase() === 'p') return void (e.preventDefault(), finish('pin'));
  if (ctrl && key.toLowerCase() === 'g') return void (e.preventDefault(), finish('gif'));
  if (ctrl && key.toLowerCase() === 't') return void (e.preventDefault(), translateInPlace());
  if (ctrl && key.toLowerCase() === 'c') return void (e.preventDefault(), finish('copy'));

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
// - 压暗遮罩：洞的上下左右各一块纯色，都是 1×1 的独立合成层用 scale 拉到位，拖动时一个像素都
//   不用重画。最早是一整块全屏元素配 clip-path 挖洞，每动一下全屏（4K 屏就是 800 多万像素）重画一遍。
//   四块直接给宽高也不行：WebKit 里图层一变大小就整块重新光栅化。
// - 选区框：四条细边（圆角时再加四个角）。一个带 outline / border 的元素范围就是整个选区，
//   每动一下要把选区那么大的一块重新光栅化 —— 实测是拖动时最大的一项开销，选区越大越卡。
//
// Chromium（Windows）扛得住原来的写法；WebKit（macOS）在 4K / 5K、高刷新率的屏上会掉帧。
// 坐标都是物理像素换算过来的，正好落在设备像素上，四块拼缝处不会有亮线。

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
    <div key={key} className="cap-mask" style={{ background: color, transform: `translate(${x}px, ${y}px) scale(${Math.max(0, pw)}, ${Math.max(0, ph)})` }} />
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

/** 框外面那圈暗边的宽度（CSS 像素）。2x 屏上正好一个物理像素 */
const HALO = 0.5;

/**
 * 选区框 / 悬停高亮框。`inside`：框画在矩形内侧（悬停高亮）；否则画在外侧，不遮住选区边缘的真实像素。
 * 颜色、线型来自 .cap-root 上的 --cap-frame-* 变量，这里只管几何。
 *
 * 每条边、每个角都是两层：下面一条略宽的半透明暗边，上面才是线 —— 白框压在白色内容上也看得清。
 * 实线的边和压暗遮罩一样，是 1×1 的合成层用 scale 拉到位，拖动时不重画；角块大小固定，只挪位置。
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
  inside,
}: {
  rect: Rect;
  s: number;
  width: number;
  radius: number;
  solid: boolean;
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

  /** 一圈框的八块。`g` / `gi`：比线本身向外 / 向里多出多少（暗边才多出来，线本身是 0）。 */
  const ring = (kind: 'halo' | 'line', g: number, gi: number) => {
    const tt = t + g + gi;
    const rr = ro > 0 ? ro + g : 0;
    // 直角时横边要盖住外角，所以两头各多出 g；圆角时横边只到角块为止
    const hx = ro > 0 ? ox + ro : ox - g;
    const hLen = len(ro > 0 ? ow - 2 * ro : ow + 2 * g);
    const vy = ro > 0 ? oy + ro : oy + t + gi;
    const vLen = len(ro > 0 ? oh - 2 * ro : oh - 2 * (t + gi));
    const stretch = kind === 'halo' || solid;
    const edge = (key: string, dir: 'h' | 'v', px: number, py: number, length: number) =>
      stretch ? (
        <span
          key={key}
          className={`cap-edge cap-edge--fill cap-edge--${kind}`}
          style={{ transform: `translate(${px}px, ${py}px) scale(${dir === 'h' ? length : tt}, ${dir === 'h' ? tt : length})` }}
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
  const style = f ?? { color: 'accent', width: 1.5, style: 'solid', radius: 0 };
  return {
    '--cap-frame-color': style.color === 'accent' ? 'var(--cn-accent)' : style.color,
    // 完成按钮用框的颜色当底色，上面的勾按底色深浅选黑或白
    '--cap-frame-fg': style.color === 'accent' || readableOn(style.color) === 'white' ? '#fff' : 'rgba(0,0,0,0.85)',
    '--cap-frame-width': `${style.width}px`,
    '--cap-frame-style': style.style,
    '--cap-frame-radius': `${style.radius}px`,
  } as React.CSSProperties;
}

function SizeHint({ rect, s, viewport }: { rect: Rect; s: number; viewport: { width: number; height: number } }) {
  const ref = useRef<HTMLDivElement>(null);
  const size = useElementSize(ref, { width: 90, height: 22 });
  // 位置每帧跟着选区走；数字隔一会儿才换（每换一次都要重画文字）
  const text = useThrottled(`${rect.width} × ${rect.height}`, CONTENT_INTERVAL);
  const pos = placeSizeHint({ x: rect.x / s, y: rect.y / s, width: rect.width / s, height: rect.height / s }, size, viewport);
  return (
    <div ref={ref} className="cap-size cn-numeric" style={{ transform: `translate(${pos.x}px, ${pos.y}px)` }}>
      {text}
    </div>
  );
}

function Handles({ rect, s, radius }: { rect: Rect; s: number; radius: number }) {
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
        const kind = corner ? 'corner' : sy !== 0 ? 'h' : 'v';
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
          set({ showOriginal: false });
        }}
      >
        <X size={14} strokeWidth={1.75} />
      </button>
    </div>
  );
}

function Toolbars({ rect, s, viewport }: { rect: Rect; s: number; viewport: { width: number; height: number } }) {
  const { t } = useTranslation();
  useEngineVersion(engine);
  const tool = useOverlay((x) => x.tool);
  const translating = useOverlay((x) => x.translating);
  const options = useOverlay((x) => x.options);
  const pixelsReady = useOverlay((x) => x.pixelsReady);
  const bitmapReady = useOverlay((x) => x.bitmapReady);
  const intent = useOverlay((x) => x.session?.intent ?? 'normal');
  const barRef = useRef<HTMLDivElement>(null);
  const subRef = useRef<HTMLDivElement>(null);
  const bar = useElementSize(barRef, { width: 540, height: 40 });
  const picked0 = engine.selectedAnnotation;
  const hasSub = !!(tool ?? (picked0 ? toolOf(picked0) : null));
  const sub = useElementSize(subRef, { width: 300, height: 40 }, [hasSub]);

  const css = { x: rect.x / s, y: rect.y / s, width: rect.width / s, height: rect.height / s };
  const pos = placeToolbar(css, bar, viewport);
  const above = !pos.inside && pos.y < css.y;
  let subY = above ? pos.y - 4 - sub.height : pos.y + bar.height + 4;
  if (subY + sub.height > viewport.height || subY < 0) subY = above ? pos.y + bar.height + 4 : pos.y - 4 - sub.height;

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
  return (
    <>
      <Toolbar
        ref={barRef}
        className={pos.inside ? 'cap-toolbar--inside' : undefined}
        style={{ transform: `translate(${pos.x}px, ${pos.y}px)` }}
        tool={tool}
        onTool={(next) => {
          if (engine.text) engine.commitText();
          engine.select(null);
          set({ tool: next, cursorStyle: 'tool' });
        }}
        canUndo={engine.canUndo}
        onUndo={() => engine.undo()}
        actions={[
          ['ocr', 'translate', 'ai', 'longshot', 'gif', 'pin'],
          ['save', 'cancel', 'done'],
        ]}
        onAction={onAction}
        disabledTools={{ mosaic: !pixelsReady }}
        disabledActions={{ translate: translating }}
      />
      {subTool && (
        <SubToolbar
          ref={subRef}
          style={{ transform: `translate(${subX}px, ${subY}px)` }}
          tool={subTool}
          options={subOptions}
          onChange={(o) => {
            if (picked && pickedTool) engine.applyOptions(o);
            set({ options: o });
          }}
        />
      )}
      <TranslationBar css={css} />
      {translating && (
        <div className="cap-busy cn-glass" style={{ transform: `translate(${css.x + css.width / 2}px, ${css.y + css.height / 2}px)` }}>
          <Spinner size={16} />
          {t('capture.translating')}
        </div>
      )}
      {intent !== 'normal' && !engine.hasGroup(TRANSLATION_GROUP) && (
        <div className="cap-intent cn-glass-thin" style={{ transform: `translate(${css.x}px, ${Math.max(0, css.y - 32)}px)` }}>
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
  const hover = useOverlay((x) => x.hover);
  const cursorStyle = useOverlay((x) => x.cursorStyle);
  const tool = useOverlay((x) => x.tool);
  const options = useOverlay((x) => x.options);
  const [viewport, setViewport] = useState({ width: window.innerWidth, height: window.innerHeight });

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
          set({ phase: 'gif', selection: p.rect, tool: null, gif: null });
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
        ? (session?.settings.ocrMaskOpacity ?? 0)
        : (session?.settings.maskOpacity ?? 0.45);
  const hole =
    phase === 'detect' || phase === 'pressing'
      ? hover
      : phase === 'selecting' || phase === 'editing' || phase === 'longshot' || phase === 'gif'
        ? selection
        : null;
  const showMask = phase !== 'longshot-other' && phase !== 'gif-other';
  const frame = isTextIntent(session?.intent) ? session?.settings.ocrFrame : session?.settings.frame;
  const radius = phase === 'longshot' || phase === 'gif' ? 0 : (frame?.radius ?? 0);
  const frameWidth = frame?.width ?? 1.5;
  const frameSolid = (frame?.style ?? 'solid') === 'solid';
  const pw = session?.monitor.bounds.width ?? 1;
  const ph = session?.monitor.bounds.height ?? 1;

  return (
    <div
      className="cap-root"
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
          <FrameBox rect={hover} s={s} width={frameWidth + 0.5} radius={radius} solid={frameSolid} inside />
          <SizeHint rect={hover} s={s} viewport={viewport} />
        </>
      )}
      {(phase === 'selecting' || phase === 'editing') && selection && (
        <>
          <FrameBox rect={selection} s={s} width={frameWidth} radius={radius} solid={frameSolid} />
          <SizeHint rect={selection} s={s} viewport={viewport} />
        </>
      )}
      {phase === 'editing' && selection && (
        <>
          {!engine.drawing && <Handles rect={selection} s={s} radius={radius} />}
          <div className="cap-text-layer">
            <SelectionOverlay engine={engine} displayScale={s} />
            <TextEditor engine={engine} displayScale={s} />
          </div>
          <Toolbars rect={selection} s={s} viewport={viewport} />
        </>
      )}
      {phase === 'longshot' && selection && <LongshotUI rect={selection} s={s} viewport={viewport} />}
      {phase === 'gif' && selection && <GifUI rect={selection} s={s} viewport={viewport} />}
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
    />
  );
}
