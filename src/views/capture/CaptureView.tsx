// 截图遮罩（规格 02）。逐条对齐微信截图的手感。
//
// 这个窗口是透明的：冻结的屏幕画面在它正下方的原生底图窗口里，这里只画压暗层、选区、
// 标注、放大镜和工具条。选区是**真镂空**，洞里看到的就是原始像素。

import './capture.css';
import '@/views/annotate/annotate.css';

import { Window } from '@tauri-apps/api/window';
import { useEffect, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
import { useTranslation } from 'react-i18next';

import { broadcast, on } from '@/lib/events';
import { formatColor } from '@/lib/format';
import { useElementSize } from '@/lib/hooks';
import { capture, reportError } from '@/lib/ipc';
import { shotUrl, windowLabel } from '@/lib/platform';
import type { CaptureIntent, FinishAction, FrameStyle } from '@/lib/types';
import { notify } from '@/ui/overlays';
import { AnnotationEngine } from '@/views/annotate/engine';
import type { Tool } from '@/views/annotate/model';
import { bmpSource, type PixelSource } from '@/views/annotate/pixels';
import { TextEditor, useEngineVersion } from '@/views/annotate/TextEditor';
import { SubToolbar, TOOL_KEYS, Toolbar, type ActionId } from '@/views/annotate/Toolbar';

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
  // 截图识字：框好就直接出结果（设置里可关，关了就和普通截图一样先进编辑态）
  if (isTextIntent(s.intent) && s.settings.ocrInstant) void finish(intentAction(s.intent));
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
  if (id === 'done') void finish(intentAction(s.intent));
  else void finish(id === 'cancel' ? 'cancel' : id === 'save' ? 'save' : id);
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
  const st = get();
  if (!st.session || st.busy) return;
  if (e.button === 2) return;
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

function onPointerMove(e: React.PointerEvent) {
  const st = get();
  if (!st.session || st.phase === 'idle' || st.phase === 'longshot' || st.phase === 'longshot-other') return;
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
      // 悬停时更新光标形状
      const sel = st.selection;
      let cursor = 'crosshair';
      if (st.tool === 'text') cursor = engine.hitText(p) ? 'text' : sel && contains(sel, p) ? 'text' : 'default';
      else if (st.tool) cursor = sel && contains(sel, p) ? 'crosshair' : 'default';
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
  drag = null;
}

function onDoubleClick(e: React.MouseEvent) {
  const st = get();
  if (st.phase === 'editing' && !st.tool && st.selection && contains(st.selection, toPx(e)) && st.session) {
    void finish(intentAction(st.session.intent));
  }
}

function onContextMenu(e: React.MouseEvent) {
  e.preventDefault();
  const st = get();
  if (!st.session) return;
  if (engine.text) {
    engine.commitText();
    return;
  }
  // 微信行为：右键直接退出；设置里可改成"先取消选区、再右键才退出"（Snipaste）
  if (st.session.settings.rightClick === 'cancelSelection' && st.selection) clearSelection();
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
  if (!st.session || st.phase === 'idle' || st.phase === 'longshot' || st.phase === 'longshot-other') return;
  if (engine.text) return; // 文字输入框自己处理
  const key = e.key;
  const ctrl = e.ctrlKey || e.metaKey;

  if (key === 'Escape') {
    e.preventDefault();
    cancel();
    return;
  }
  if (key === 'Enter') {
    e.preventDefault();
    if (st.phase === 'detect' && st.hover) enterEditing(st.hover);
    if (get().selection) void finish(intentAction(st.session.intent));
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
  if (ctrl && key.toLowerCase() === 't') return void (e.preventDefault(), finish('translate'));
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
      set({ tool: st.tool === tool ? null : tool });
    }
  }
}

// ───────────────────────── 渲染 ─────────────────────────

function holeClip(r: Rect | null, s: number, radius: number, viewport: { width: number; height: number }): string | undefined {
  if (!r) return undefined;
  const x1 = r.x / s;
  const y1 = r.y / s;
  const x2 = right(r) / s;
  const y2 = bottom(r) / s;
  // evenodd：外框一圈 + 内框一圈 = 挖洞。比四个 div 拼四周重绘便宜得多
  const rr = Math.min(radius, (x2 - x1) / 2, (y2 - y1) / 2);
  if (rr <= 0) {
    return `polygon(evenodd, 0 0, 100% 0, 100% 100%, 0 100%, 0 0, ${x1}px ${y1}px, ${x1}px ${y2}px, ${x2}px ${y2}px, ${x2}px ${y1}px, ${x1}px ${y1}px)`;
  }
  // 圆角框配圆角洞，不然四个角会露出一小块没压暗的直角
  const { width: w, height: h } = viewport;
  const arc = (x: number, y: number) => `A ${rr} ${rr} 0 0 1 ${x} ${y}`;
  const hole = `M ${x1 + rr} ${y1} H ${x2 - rr} ${arc(x2, y1 + rr)} V ${y2 - rr} ${arc(x2 - rr, y2)} H ${x1 + rr} ${arc(x1, y2 - rr)} V ${y1 + rr} ${arc(x1 + rr, y1)} Z`;
  return `path(evenodd, 'M 0 0 H ${w} V ${h} H 0 Z ${hole}')`;
}

/** 选区框样式 → CSS 变量（选区框、悬停框、拖柄共用）。 */
function frameVars(f: FrameStyle | undefined): React.CSSProperties {
  const style = f ?? { color: 'accent', width: 1.5, style: 'solid', radius: 0 };
  return {
    '--cap-frame-color': style.color === 'accent' ? 'var(--cn-accent)' : style.color,
    '--cap-frame-width': `${style.width}px`,
    '--cap-frame-style': style.style,
    '--cap-frame-radius': `${style.radius}px`,
  } as React.CSSProperties;
}

function cssRect(r: Rect, s: number): React.CSSProperties {
  return { left: r.x / s, top: r.y / s, width: r.width / s, height: r.height / s };
}

function SizeHint({ rect, s, viewport }: { rect: Rect; s: number; viewport: { width: number; height: number } }) {
  const ref = useRef<HTMLDivElement>(null);
  const size = useElementSize(ref, { width: 90, height: 22 });
  const text = `${rect.width} × ${rect.height}`;
  const pos = placeSizeHint({ x: rect.x / s, y: rect.y / s, width: rect.width / s, height: rect.height / s }, size, viewport);
  return (
    <div ref={ref} className="cap-size cn-glass-thin cn-numeric" style={{ transform: `translate(${pos.x}px, ${pos.y}px)` }}>
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
        return (
          <span
            key={h}
            className="cap-handle"
            style={{ left: p.x / s + (corner ? dx : 0), top: p.y / s + (corner ? dy : 0), cursor: HANDLE_CURSORS[h] }}
          />
        );
      })}
    </>
  );
}

function Toolbars({ rect, s, viewport }: { rect: Rect; s: number; viewport: { width: number; height: number } }) {
  const { t } = useTranslation();
  useEngineVersion(engine);
  const tool = useOverlay((x) => x.tool);
  const options = useOverlay((x) => x.options);
  const pixelsReady = useOverlay((x) => x.pixelsReady);
  const bitmapReady = useOverlay((x) => x.bitmapReady);
  const intent = useOverlay((x) => x.session?.intent ?? 'normal');
  const barRef = useRef<HTMLDivElement>(null);
  const subRef = useRef<HTMLDivElement>(null);
  const bar = useElementSize(barRef, { width: 540, height: 40 });
  const sub = useElementSize(subRef, { width: 300, height: 36 });

  const css = { x: rect.x / s, y: rect.y / s, width: rect.width / s, height: rect.height / s };
  const pos = placeToolbar(css, bar, viewport);
  const above = !pos.inside && pos.y < css.y;
  let subY = above ? pos.y - 4 - sub.height : pos.y + bar.height + 4;
  if (subY + sub.height > viewport.height || subY < 0) subY = above ? pos.y + bar.height + 4 : pos.y - 4 - sub.height;
  const subX = Math.max(0, Math.min(pos.x + bar.width - sub.width, viewport.width - sub.width));

  // 长截图入口只在这块屏的选区足够大时有意义；模糊需要位图到位
  const blurPending = tool === 'mosaic' && options.mosaic.mode === 'blur' && !bitmapReady;
  return (
    <>
      <Toolbar
        ref={barRef}
        className={pos.inside ? 'cap-toolbar--inside' : undefined}
        style={{ transform: `translate(${pos.x}px, ${pos.y}px)` }}
        tool={tool}
        onTool={(next) => {
          if (engine.text) engine.commitText();
          set({ tool: next, cursorStyle: next === 'text' ? 'text' : 'crosshair' });
        }}
        canUndo={engine.canUndo}
        onUndo={() => engine.undo()}
        actions={[
          ['ocr', 'translate', 'ai', 'longshot', 'pin'],
          ['save', 'cancel', 'done'],
        ]}
        onAction={onAction}
        disabledTools={{ mosaic: !pixelsReady }}
      />
      {tool && (
        <SubToolbar
          ref={subRef}
          style={{ transform: `translate(${subX}px, ${subY}px)` }}
          tool={tool}
          options={options}
          onChange={(o) => set({ options: o })}
        />
      )}
      {intent !== 'normal' && (
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
  const cursor = useOverlay((x) => x.cursor);
  const cursorStyle = useOverlay((x) => x.cursorStyle);
  const pixelsReady = useOverlay((x) => x.pixelsReady);
  const colorFormat = useOverlay((x) => x.colorFormat);
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
      on('capture-hotkey', (intent) => {
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
      : isTextIntent(session?.intent)
        ? (session?.settings.ocrMaskOpacity ?? 0)
        : (session?.settings.maskOpacity ?? 0.45);
  const hole =
    phase === 'detect' || phase === 'pressing'
      ? hover
      : phase === 'selecting' || phase === 'editing' || phase === 'longshot'
        ? selection
        : null;
  const showMask = phase !== 'longshot-other';
  const frame = isTextIntent(session?.intent) ? session?.settings.ocrFrame : session?.settings.frame;
  const radius = phase === 'longshot' ? 0 : (frame?.radius ?? 0);
  const showMagnifier =
    !!cursor && pixelsReady && !!pixels && session?.settings.showMagnifier !== false && (phase === 'detect' || phase === 'pressing' || phase === 'selecting');
  const pw = session?.monitor.bounds.width ?? 1;
  const ph = session?.monitor.bounds.height ?? 1;

  return (
    <div
      className="cap-root"
      data-phase={phase}
      style={{ cursor: phase === 'editing' ? cursorStyle : phase === 'passive' ? 'default' : 'crosshair', ...frameVars(frame) }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onDoubleClick={onDoubleClick}
      onContextMenu={onContextMenu}
    >
      {showMask && <div className="cap-mask" style={{ background: `rgba(0,0,0,${opacity})`, clipPath: holeClip(hole, s, radius, viewport) }} />}
      <canvas ref={committed} className="cap-canvas" width={pw} height={ph} />
      <canvas ref={drafting} className="cap-canvas" width={pw} height={ph} />

      {(phase === 'detect' || phase === 'pressing') && hover && (
        <>
          <div className="cap-hover" style={cssRect(hover, s)} />
          <SizeHint rect={hover} s={s} viewport={viewport} />
        </>
      )}
      {(phase === 'selecting' || phase === 'editing') && selection && (
        <>
          <div className="cap-frame" style={cssRect(selection, s)} />
          <SizeHint rect={selection} s={s} viewport={viewport} />
        </>
      )}
      {phase === 'editing' && selection && (
        <>
          {!engine.drawing && <Handles rect={selection} s={s} radius={radius} />}
          <div className="cap-text-layer">
            <TextEditor engine={engine} displayScale={s} />
          </div>
          <Toolbars rect={selection} s={s} viewport={viewport} />
        </>
      )}
      {phase === 'longshot' && selection && <LongshotUI rect={selection} s={s} viewport={viewport} />}
      {showMagnifier && cursor && pixels && session && (
        <Magnifier
          pixels={pixels}
          cursor={cursor}
          origin={{ x: session.monitor.bounds.x, y: session.monitor.bounds.y }}
          scale={s}
          viewport={viewport}
          format={colorFormat}
        />
      )}
    </div>
  );
}
