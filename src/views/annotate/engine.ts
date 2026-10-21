// 标注引擎：截图遮罩和图片编辑器共用。
//
// 刻意写成命令式类而不是 React state：拖拽绘制时每个 pointermove 都要重画，走 React
// 重渲染达不到 60fps（规格 02 §4 最后一条）。React 只订阅"版本号"来刷新撤销按钮之类。
//
// 两块画布都是**图像物理像素分辨率**：已完成的标注一块、正在画的那一笔一块（画完合并）。
//
// 画完的标注可以再选中：拖动挪位置，拖控制点改大小（矩形、椭圆、矩形马赛克、文字块是八个点，
// 箭头是两头，文字是四个角、按比例缩放字号），Delete 删掉，选中时改颜色 / 粗细直接作用在它身上。
// 撤销按"操作"回退（加、挪、改、删都算一步），每步前存一份快照。

import {
  constrainAngle,
  constrainSquare,
  fromPoints,
  handlePoint,
  HANDLES,
  intersect,
  resizeByHandle,
  union,
  type Handle,
  type Point,
  type Rect,
} from '@/views/capture/geometry';

import { DEFAULT_FILL_OPACITY, newId, type Annotation, type TextStyle, type Tool, type ToolOptions, type Watermark } from './model';
import {
  annotationBounds,
  drawAnnotation,
  drawSpotlights,
  drawWatermark,
  LABEL_REVEAL_MS,
  measureText,
  mosaicCell,
  placeMagnifier,
  type Backdrop,
} from './render';
import { instantiate, listTemplates } from './templates';

type Spotlight = Extract<Annotation, { kind: 'spotlight' }>;
const isSpotlight = (a: Annotation): a is Spotlight => a.kind === 'spotlight';
/** 有矩形范围、能拖八个控制点改大小的标注 */
const isBoxed = (a: Annotation): a is Extract<Annotation, { rect: Rect }> =>
  a.kind === 'rect' || a.kind === 'ellipse' || a.kind === 'mosaicRect' || a.kind === 'label' || a.kind === 'highlightRect' || a.kind === 'spotlight';

export interface TextDraft {
  at: Point;
  content: string;
  color: string;
  fontSize: number;
  bold: boolean;
  style?: TextStyle;
  /** 重新编辑已有文字时，原来那条的 id */
  editing?: string;
}

/** 选中标注上的控制点：矩形类八个方向，箭头两头，角度标注还有顶点 */
export type AnnotationHandle = Handle | 'from' | 'to' | 'vertex';

export interface SelectionInfo {
  id: string;
  kind: Annotation['kind'];
  /** 虚线框（箭头没有） */
  outline: Rect | null;
  handles: { id: AnnotationHandle; at: Point }[];
}

const MAX_HISTORY = 100;
/** 多久之内对同一个标注的连续改样式算一步撤销 */
const RESTYLE_MERGE_MS = 800;
/** 文字只有四个角能拖：字是按比例缩放的，拉单边没有意义 */
const TEXT_HANDLES: Handle[] = ['nw', 'ne', 'se', 'sw'];

/** 拖文字的某个角：按拖出来的框和原框的比例缩放字号，对角保持不动。 */
function scaledText(o: Extract<Annotation, { kind: 'text' }>, handle: Handle, p: Point): Annotation {
  const b = annotationBounds(o);
  const r = resizeByHandle(b, handle, p);
  const ratio = Math.max(0.2, Math.min(10, (r.width / b.width + r.height / b.height) / 2));
  const fontSize = Math.max(6, o.fontSize * ratio);
  const m = measureText({ content: o.content, fontSize, bold: o.bold });
  // annotationBounds 比文字本身四周各宽 2 像素
  const w = m.width + 4;
  const h = m.height + 4;
  const left = handle.includes('w') ? b.x + b.width - w : b.x;
  const top = handle.includes('n') ? b.y + b.height - h : b.y;
  return { ...o, fontSize, at: { x: left + 2, y: top + 2 } };
}

function distToSegment(p: Point, a: Point, b: Point): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len2 = dx * dx + dy * dy;
  const t = len2 ? Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2)) : 0;
  return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

const inRect = (r: Rect, p: Point, pad = 0) => p.x >= r.x - pad && p.x <= r.x + r.width + pad && p.y >= r.y - pad && p.y <= r.y + r.height + pad;

/** 平移一个标注（返回新对象，不改原来的）。 */
function translated(a: Annotation, dx: number, dy: number): Annotation {
  const mv = (p: Point) => ({ x: p.x + dx, y: p.y + dy });
  const mr = (r: Rect) => ({ ...r, x: r.x + dx, y: r.y + dy });
  switch (a.kind) {
    case 'rect':
    case 'ellipse':
    case 'mosaicRect':
    case 'label':
    case 'highlightRect':
    case 'spotlight':
      return { ...a, rect: mr(a.rect) };
    case 'arrow':
      return { ...a, from: mv(a.from), to: mv(a.to) };
    case 'pen':
    case 'highlight':
    case 'mosaic':
      return { ...a, points: a.points.map(mv) };
    case 'text':
    case 'counter':
      return { ...a, at: mv(a.at) };
    case 'magnifier':
      return { ...a, source: mr(a.source), center: mv(a.center) };
    case 'distance':
      return { ...a, from: mv(a.from), to: mv(a.to) };
    case 'angle':
      return { ...a, vertex: mv(a.vertex), a: mv(a.a), b: mv(a.b) };
  }
}

/** 哪个工具的选项对应这个标注（选中时二级工具条显示它的选项） */
export function toolOf(a: Annotation): Tool | null {
  switch (a.kind) {
    case 'rect':
    case 'ellipse':
    case 'arrow':
    case 'pen':
    case 'text':
    case 'counter':
      return a.kind;
    case 'highlight':
    case 'highlightRect':
    case 'spotlight':
    case 'magnifier':
      return 'highlight';
    case 'mosaicRect':
      return 'mosaic';
    case 'distance':
    case 'angle':
      return 'measure';
    default:
      return null;
  }
}

export class AnnotationEngine {
  annotations: Annotation[] = [];
  draft: Annotation | null = null;
  text: TextDraft | null = null;
  clip: Rect | null = null;
  backdrop: Backdrop = { pixels: null, bitmap: null };
  /** 物理像素 / CSS 像素，用来把工具选项换成物理尺寸 */
  scale = 1;
  /**
   * 画布左上角对应的图像坐标。截图遮罩的画布覆盖整块屏，原点恒为 (0,0)；
   * 编辑器里超长图片只给可见区域建画布，滚动时移动原点。
   */
  origin: Point = { x: 0, y: 0 };
  version = 0;
  /** 选中的标注 id */
  selected: string | null = null;
  /** 上一次改选中标注样式的时间（合并撤销用，见 `applyOptions`） */
  private lastRestyle: { id: string; at: number } | null = null;

  private committed: HTMLCanvasElement | null = null;
  private drafting: HTMLCanvasElement | null = null;
  private listeners = new Set<() => void>();
  private start: Point | null = null;
  private lastDraftBounds: Rect | null = null;
  /** 已提交的那层画布上现在有没有东西（没有就不用清） */
  private committedDirty = false;
  private history: Annotation[][] = [];
  private transform: { handle: AnnotationHandle | 'move'; at: Point; orig: Annotation; moved: boolean } | null = null;
  /** 水印（整张图平铺），null = 不画。不进撤销历史：改设置一样随时开关 */
  watermark: Watermark | null = null;
  /** 角度标注画完第一条边、松开了鼠标：在等第二条边（移鼠标定方向，再点一下完成） */
  private anglePending = false;
  /** 橡皮擦正按着：拖过的标注都擦掉，整次拖动算一步撤销 */
  private erasing: { checkpointed: boolean; tolerance: number } | null = null;

  attach(committed: HTMLCanvasElement | null, drafting: HTMLCanvasElement | null) {
    this.committed = committed;
    this.drafting = drafting;
    this.renderCommitted();
  }

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private changed() {
    this.version += 1;
    for (const fn of this.listeners) fn();
  }

  reset() {
    this.stopReveal();
    this.lockedGroups.clear();
    this.annotations = [];
    this.draft = null;
    this.text = null;
    this.start = null;
    this.selected = null;
    this.transform = null;
    this.erasing = null;
    this.anglePending = false;
    this.watermark = null;
    this.history = [];
    this.clearCanvas(this.committed);
    this.committedEmpty = true;
    this.clearCanvas(this.drafting);
    this.committedDirty = false;
    this.changed();
  }

  setClip(clip: Rect | null) {
    this.clip = clip;
    // 拖选区时每帧都会走到这里。没有标注时画布本来就是空的，不用再整块清一遍
    // （4K 画布清一次就要几毫秒，还得把整张画布重新上传给合成器）
    if (this.annotations.length === 0 && !this.watermark && this.committedEmpty) return;
    this.renderCommitted();
  }

  setBackdrop(b: Backdrop) {
    this.backdrop = b;
    this.renderCommitted();
  }

  // ───────────────────────── 历史 ─────────────────────────

  /** 改动之前调一次：存一份快照，撤销时回到这里。 */
  private checkpoint() {
    this.history.push(structuredClone(this.annotations));
    if (this.history.length > MAX_HISTORY) this.history.shift();
  }

  get canUndo(): boolean {
    return this.history.length > 0;
  }

  undo() {
    const prev = this.history.pop();
    if (!prev) return;
    this.annotations = prev;
    if (this.selected && !this.find(this.selected)) this.selected = null;
    this.renderCommitted();
    this.changed();
  }

  // ───────────────────────── 绘制 ─────────────────────────

  begin(tool: Exclude<Tool, 'text'>, p: Point, opts: ToolOptions) {
    // 角度标注的第二次点击：定下第二条边，完成
    if (this.anglePending && this.draft?.kind === 'angle') {
      this.anglePending = false;
      this.draft.b = p;
      this.end();
      return;
    }
    const s = this.scale;
    this.start = p;
    this.selected = null;
    if (tool === 'template') {
      // 模板：整组插进来，左上角放在点的位置
      const t = listTemplates().find((x) => x.id === opts.template.selected);
      this.start = null;
      if (t) this.addAll(instantiate(t, p, s));
      return;
    }
    if (tool === 'watermark') {
      this.start = null;
      return;
    }
    if (tool === 'eraser') {
      this.erasing = { checkpointed: false, tolerance: 6 * s };
      this.eraseAt(p);
      this.changed();
      return;
    }
    switch (tool) {
      case 'rect':
      case 'ellipse': {
        const o = opts[tool];
        this.draft = { kind: tool, id: newId(), rect: fromPoints(p, p), color: o.color, lineWidth: o.lineWidth * s, filled: o.filled, fillOpacity: o.fillOpacity, dash: o.dash };
        break;
      }
      case 'arrow': {
        const o = opts.arrow;
        this.draft = { kind: 'arrow', id: newId(), from: p, to: p, color: o.color, lineWidth: o.lineWidth * s, style: o.style, dash: o.dash };
        break;
      }
      case 'pen':
        this.draft = { kind: 'pen', id: newId(), points: [p], color: opts.pen.color, lineWidth: opts.pen.lineWidth * s };
        break;
      case 'highlight': {
        const o = opts.highlight;
        this.draft =
          o.shape === 'brush'
            ? { kind: 'highlight', id: newId(), points: [p], color: o.color, lineWidth: o.lineWidth * s }
            : o.shape === 'rect'
              ? { kind: 'highlightRect', id: newId(), rect: fromPoints(p, p), color: o.color }
              : o.shape === 'magnifier'
                ? { kind: 'magnifier', id: newId(), source: fromPoints(p, p), ...placeMagnifier(fromPoints(p, p), this.clip), color: o.color }
                : { kind: 'spotlight', id: newId(), rect: fromPoints(p, p), dim: o.dim };
        break;
      }
      case 'measure': {
        const o = opts.measure;
        this.draft =
          o.shape === 'distance'
            ? { kind: 'distance', id: newId(), from: p, to: p, color: o.color, lineWidth: o.lineWidth * s, unit: o.unit, scale: s }
            : { kind: 'angle', id: newId(), vertex: p, a: p, b: p, color: o.color, lineWidth: o.lineWidth * s };
        break;
      }
      case 'counter': {
        const o = opts.counter;
        // 接着现有最大的编号往下数：删掉中间某个不影响后面新加的
        const n = this.annotations.reduce((m, a) => (a.kind === 'counter' ? Math.max(m, a.n) : m), 0) + 1;
        this.draft = { kind: 'counter', id: newId(), at: p, n, numbering: o.numbering, color: o.color, size: o.size * s };
        break;
      }
      case 'mosaic': {
        const o = opts.mosaic;
        this.draft =
          o.shape === 'rect'
            ? { kind: 'mosaicRect', id: newId(), rect: fromPoints(p, p), cell: mosaicCell(o.brushSize * s), mode: o.mode }
            : { kind: 'mosaic', id: newId(), points: [p], brushSize: o.brushSize * s, mode: o.mode };
        break;
      }
    }
    this.renderDraft();
    this.changed();
  }

  update(p: Point, shift: boolean) {
    if (this.erasing) {
      this.eraseAt(p);
      return;
    }
    const d = this.draft;
    const start = this.start;
    if (!d || !start) return;
    switch (d.kind) {
      case 'rect':
      case 'ellipse':
      case 'mosaicRect':
      case 'highlightRect':
      case 'spotlight':
        d.rect = fromPoints(start, shift ? constrainSquare(start, p) : p);
        break;
      case 'counter':
        // 按下之后拖一拖再松手：编号跟着挪，方便摆准
        d.at = p;
        break;
      case 'distance':
        d.to = shift ? constrainAngle(start, p) : p;
        break;
      case 'angle':
        // 先拖第一条边；松手后移鼠标定第二条边
        if (this.anglePending) d.b = shift ? constrainAngle(d.vertex, p) : p;
        else {
          d.a = shift ? constrainAngle(start, p) : p;
          d.b = d.a;
        }
        break;
      case 'magnifier': {
        // 框要放大的那块；圆跟着摆在旁边
        d.source = fromPoints(start, shift ? constrainSquare(start, p) : p);
        const placed = placeMagnifier(d.source, this.clip);
        d.center = placed.center;
        d.radius = placed.radius;
        break;
      }
      case 'arrow':
        d.to = shift ? constrainAngle(start, p) : p;
        break;
      case 'pen':
      case 'highlight':
      case 'mosaic': {
        const last = d.points[d.points.length - 1];
        // 太密的采样点没有意义，还会让贝塞尔平滑变抖
        if (!last || Math.hypot(p.x - last.x, p.y - last.y) >= 1.5 * this.scale) d.points.push(p);
        break;
      }
      default:
        break;
    }
    this.renderDraft();
  }

  /** 画完一笔。太小的（误触）丢掉。 */
  end() {
    if (this.erasing) {
      this.erasing = null;
      this.start = null;
      this.changed();
      return;
    }
    const d = this.draft;
    // 角度的第一条边拖完了：留着草稿，接着等第二条边
    if (d?.kind === 'angle' && !this.anglePending && d.b === d.a) {
      if (Math.hypot(d.a.x - d.vertex.x, d.a.y - d.vertex.y) < 4 * this.scale) {
        this.cancelDraft();
        return;
      }
      this.anglePending = true;
      this.changed();
      return;
    }
    this.anglePending = false;
    this.draft = null;
    this.start = null;
    this.clearCanvas(this.drafting);
    this.lastDraftBounds = null;
    if (!d) return;
    const tiny =
      ((d.kind === 'rect' || d.kind === 'ellipse' || d.kind === 'mosaicRect' || d.kind === 'highlightRect' || d.kind === 'spotlight') &&
        (d.rect.width < 3 || d.rect.height < 3)) ||
      (d.kind === 'magnifier' && (d.source.width < 3 || d.source.height < 3)) ||
      ((d.kind === 'arrow' || d.kind === 'distance') && Math.hypot(d.to.x - d.from.x, d.to.y - d.from.y) < 4);
    if (tiny) {
      // 拖聚光灯时草稿是画在已提交那层上的，丢掉的话要把那层重画回去
      if (d.kind === 'spotlight') this.renderCommitted();
      this.changed();
      return;
    }
    this.checkpoint();
    this.annotations.push(d);
    this.renderCommitted();
    this.changed();
  }

  get drawing(): boolean {
    return this.draft !== null || this.erasing !== null;
  }

  /** 正在等角度标注的第二条边（松着鼠标也要跟着动） */
  get waitingSecondClick(): boolean {
    return this.anglePending;
  }

  /** 扔掉正在画的那一笔（Esc） */
  cancelDraft() {
    this.draft = null;
    this.start = null;
    this.anglePending = false;
    this.clearCanvas(this.drafting);
    this.lastDraftBounds = null;
    this.changed();
  }

  /** 开关 / 修改水印 */
  setWatermark(w: Watermark | null) {
    if (JSON.stringify(w) === JSON.stringify(this.watermark)) return;
    this.watermark = w;
    this.committedDirty = true;
    this.renderCommitted();
    this.changed();
  }

  /** 橡皮擦：擦掉这一点碰到的标注（锁住的译文块除外） */
  private eraseAt(p: Point) {
    const er = this.erasing;
    if (!er) return;
    const doomed = this.annotations.filter((a) => this.erasable(a, p, er.tolerance));
    if (!doomed.length) return;
    if (!er.checkpointed) {
      this.checkpoint();
      er.checkpointed = true;
    }
    const ids = new Set(doomed.map((a) => a.id));
    this.annotations = this.annotations.filter((a) => !ids.has(a.id));
    if (this.selected && ids.has(this.selected)) this.selected = null;
    this.renderCommitted();
    this.changed();
  }

  private erasable(a: Annotation, p: Point, tol: number): boolean {
    // 手绘马赛克平时点不中（是一片涂抹），橡皮擦要能擦：碰到任意一个笔刷点就算
    if (a.kind === 'mosaic') return a.points.some((q) => Math.hypot(p.x - q.x, p.y - q.y) <= a.brushSize / 2 + tol);
    // 空心框只认边，擦的时候框里面也算
    if (a.kind === 'rect' || a.kind === 'ellipse' || a.kind === 'spotlight') return inRect(a.rect, p, tol);
    return this.hits(a, p, tol);
  }

  // ───────────────────────── 图层顺序 ─────────────────────────

  /** 选中的标注往上 / 往下挪一层，或者直接到最上 / 最下。返回是否真的动了。 */
  reorderSelected(where: 'up' | 'down' | 'top' | 'bottom'): boolean {
    const id = this.selected;
    const i = this.annotations.findIndex((a) => a.id === id);
    if (i < 0) return false;
    const last = this.annotations.length - 1;
    const j = where === 'up' ? Math.min(last, i + 1) : where === 'down' ? Math.max(0, i - 1) : where === 'top' ? last : 0;
    if (j === i) return false;
    this.checkpoint();
    const [a] = this.annotations.splice(i, 1);
    this.annotations.splice(j, 0, a!);
    this.renderCommitted();
    this.changed();
    return true;
  }

  /** 一次加一批（截图原位翻译的译文块），算一步撤销。 */
  addAll(list: Annotation[]) {
    if (!list.length) return;
    this.checkpoint();
    this.annotations.push(...list);
    this.renderCommitted();
    this.changed();
  }

  /** 去掉某一组（比如全部译文块），算一步撤销。 */
  removeGroup(group: string) {
    if (!this.annotations.some((a) => a.kind === 'label' && a.group === group)) return;
    this.checkpoint();
    this.annotations = this.annotations.filter((a) => !(a.kind === 'label' && a.group === group));
    if (this.selected && !this.find(this.selected)) this.selected = null;
    this.renderCommitted();
    this.changed();
  }

  /** 某一组整体显示 / 隐藏（看原文），不算撤销步骤。 */
  setGroupHidden(group: string, hidden: boolean) {
    for (const a of this.annotations) if (a.kind === 'label' && a.group === group) a.hidden = hidden;
    if (hidden) this.selected = null;
    this.renderCommitted();
    this.changed();
  }

  hasGroup(group: string): boolean {
    return this.annotations.some((a) => a.kind === 'label' && a.group === group);
  }

  /** 锁住的组点不中、拖不动（译文默认锁着，鼠标在上面是选字；打开"移动"才解开） */
  private lockedGroups = new Set<string>();

  setGroupLocked(group: string, locked: boolean) {
    if (locked) this.lockedGroups.add(group);
    else this.lockedGroups.delete(group);
    const sel = this.selectedAnnotation;
    if (locked && sel?.kind === 'label' && sel.group === group) {
      this.selected = null;
      this.transform = null;
      this.changed();
    }
  }

  /** 译文显现：组里每块按 delayOf 给的毫秒数错开，底色先铺满，字再由虚到实浮上来（见 render.ts drawLabel） */
  private reveal: { start: number; delays: Map<string, number>; raf: number } | null = null;

  revealGroup(group: string, delayOf: (r: Rect) => number) {
    this.stopReveal();
    const delays = new Map<string, number>();
    for (const a of this.annotations) if (a.kind === 'label' && a.group === group) delays.set(a.id, delayOf(a.rect));
    if (delays.size === 0) return;
    const end = Math.max(...delays.values()) + LABEL_REVEAL_MS;
    const reveal = { start: performance.now(), delays, raf: 0 };
    const tick = () => {
      this.renderCommitted();
      if (performance.now() - reveal.start < end) reveal.raf = requestAnimationFrame(tick);
      else {
        this.reveal = null;
        this.renderCommitted();
      }
    };
    this.reveal = reveal;
    tick();
  }

  private stopReveal() {
    if (this.reveal) cancelAnimationFrame(this.reveal.raf);
    this.reveal = null;
  }

  private revealProgress(a: Annotation): number {
    const delay = this.reveal?.delays.get(a.id);
    if (delay === undefined || !this.reveal) return 1;
    return Math.min(1, Math.max(0, (performance.now() - this.reveal.start - delay) / LABEL_REVEAL_MS));
  }

  // ───────────────────────── 选中 / 挪动 / 改大小 ─────────────────────────

  find(id: string): Annotation | undefined {
    return this.annotations.find((a) => a.id === id);
  }

  get selectedAnnotation(): Annotation | null {
    return this.selected ? (this.find(this.selected) ?? null) : null;
  }

  /** 点中了哪个标注（从上往下找）。手绘马赛克是一片涂抹，不参与。 */
  hit(p: Point, tolerance: number): Annotation | null {
    for (let i = this.annotations.length - 1; i >= 0; i -= 1) {
      const a = this.annotations[i]!;
      if (this.hits(a, p, tolerance)) return a;
    }
    return null;
  }

  private hits(a: Annotation, p: Point, tol: number): boolean {
    switch (a.kind) {
      case 'rect': {
        const r = a.rect;
        if (!inRect(r, p, tol)) return false;
        if (a.filled) return true;
        // 空心框只认边：框里面还能接着画别的
        const inset = a.lineWidth + tol;
        return !(p.x > r.x + inset && p.x < r.x + r.width - inset && p.y > r.y + inset && p.y < r.y + r.height - inset);
      }
      case 'ellipse': {
        const r = a.rect;
        const rx = r.width / 2;
        const ry = r.height / 2;
        if (rx < 1 || ry < 1) return false;
        const d = Math.hypot((p.x - r.x - rx) / rx, (p.y - r.y - ry) / ry);
        if (a.filled) return d <= 1 + tol / Math.min(rx, ry);
        return Math.abs(d - 1) * Math.min(rx, ry) <= a.lineWidth / 2 + tol;
      }
      case 'arrow':
        return distToSegment(p, a.from, a.to) <= (a.style === 'thick' ? a.lineWidth * 1.6 : a.lineWidth) + tol;
      case 'highlightRect':
        return inRect(a.rect, p, tol);
      case 'spotlight': {
        // 聚光灯只认边：框里正是要标注的地方，点进去要能接着画
        const r = a.rect;
        const t = tol + 2 * this.scale;
        return inRect(r, p, t) && !(p.x > r.x + t && p.x < r.x + r.width - t && p.y > r.y + t && p.y < r.y + r.height - t);
      }
      case 'counter':
        return Math.hypot(p.x - a.at.x, p.y - a.at.y) <= a.size / 2 + tol;
      case 'magnifier':
        return Math.hypot(p.x - a.center.x, p.y - a.center.y) <= a.radius + tol || inRect(a.source, p, tol);
      case 'distance':
        return distToSegment(p, a.from, a.to) <= a.lineWidth * 2 + tol;
      case 'angle':
        return distToSegment(p, a.vertex, a.a) <= a.lineWidth * 2 + tol || distToSegment(p, a.vertex, a.b) <= a.lineWidth * 2 + tol;
      case 'highlight':
      case 'pen': {
        const lim = a.lineWidth / 2 + tol;
        if (a.points.length === 1) return Math.hypot(p.x - a.points[0]!.x, p.y - a.points[0]!.y) <= lim;
        for (let i = 1; i < a.points.length; i += 1) if (distToSegment(p, a.points[i - 1]!, a.points[i]!) <= lim) return true;
        return false;
      }
      case 'text':
        return inRect(annotationBounds(a), p, tol);
      case 'mosaicRect':
        return inRect(a.rect, p, tol);
      case 'label':
        return !a.hidden && !(a.group && this.lockedGroups.has(a.group)) && inRect(a.rect, p, tol);
      case 'mosaic':
        return false;
    }
  }

  select(id: string | null) {
    if (this.selected === id) return;
    this.selected = id;
    this.changed();
  }

  /** 选中标注的虚线框和控制点（图像物理像素）。 */
  selectionInfo(): SelectionInfo | null {
    const a = this.selectedAnnotation;
    if (!a) return null;
    switch (a.kind) {
      case 'rect':
      case 'ellipse':
      case 'mosaicRect':
      case 'label':
      case 'highlightRect':
      case 'spotlight':
        return { id: a.id, kind: a.kind, outline: a.rect, handles: HANDLES.map((h) => ({ id: h, at: handlePoint(a.rect, h) })) };
      case 'arrow':
        return {
          id: a.id,
          kind: a.kind,
          outline: null,
          handles: [
            { id: 'from', at: a.from },
            { id: 'to', at: a.to },
          ],
        };
      case 'text': {
        const b = annotationBounds(a);
        return { id: a.id, kind: a.kind, outline: b, handles: TEXT_HANDLES.map((h) => ({ id: h, at: handlePoint(b, h) })) };
      }
      case 'distance':
        return {
          id: a.id,
          kind: a.kind,
          outline: null,
          handles: [
            { id: 'from', at: a.from },
            { id: 'to', at: a.to },
          ],
        };
      case 'angle':
        return {
          id: a.id,
          kind: a.kind,
          outline: null,
          handles: [
            { id: 'vertex', at: a.vertex },
            { id: 'from', at: a.a },
            { id: 'to', at: a.b },
          ],
        };
      case 'magnifier':
        // 拖圆心单独挪圆，拖圆边改大小（也就是改放大倍数）；拖别处整体挪
        return {
          id: a.id,
          kind: a.kind,
          outline: a.source,
          handles: [
            { id: 'from', at: a.center },
            { id: 'to', at: { x: a.center.x + a.radius, y: a.center.y } },
          ],
        };
      default:
        return { id: a.id, kind: a.kind, outline: annotationBounds(a), handles: [] };
    }
  }

  /** 点中了选中标注的哪个控制点。 */
  hitHandle(p: Point, radius: number): AnnotationHandle | null {
    const info = this.selectionInfo();
    if (!info) return null;
    for (const h of info.handles) if (Math.hypot(p.x - h.at.x, p.y - h.at.y) <= radius) return h.id;
    return null;
  }

  /** 按下：开始挪（'move'）或拖某个控制点。 */
  beginTransform(p: Point, handle: AnnotationHandle | 'move') {
    const a = this.selectedAnnotation;
    if (!a) return;
    this.transform = { handle, at: p, orig: structuredClone(a), moved: false };
  }

  get transforming(): boolean {
    return this.transform !== null;
  }

  updateTransform(p: Point, shift: boolean) {
    const tf = this.transform;
    if (!tf) return;
    const dx = p.x - tf.at.x;
    const dy = p.y - tf.at.y;
    // 抖一两个像素不算挪（单击文字要进编辑，不能因为手抖变成拖动）
    if (!tf.moved && Math.hypot(dx, dy) < 3 * this.scale) return;
    if (!tf.moved) {
      tf.moved = true;
      this.checkpoint();
    }
    const o = tf.orig;
    let next: Annotation = o;
    if (tf.handle === 'move') {
      next = translated(o, dx, dy);
    } else if (o.kind === 'distance' && (tf.handle === 'from' || tf.handle === 'to')) {
      next = tf.handle === 'from' ? { ...o, from: shift ? constrainAngle(o.to, p) : p } : { ...o, to: shift ? constrainAngle(o.from, p) : p };
    } else if (o.kind === 'angle' && (tf.handle === 'from' || tf.handle === 'to' || tf.handle === 'vertex')) {
      next = tf.handle === 'vertex' ? { ...o, vertex: p } : tf.handle === 'from' ? { ...o, a: p } : { ...o, b: p };
    } else if (o.kind === 'magnifier' && (tf.handle === 'from' || tf.handle === 'to')) {
      next = tf.handle === 'from' ? { ...o, center: p } : { ...o, radius: Math.max(12 * this.scale, Math.hypot(p.x - o.center.x, p.y - o.center.y)) };
    } else if (o.kind === 'arrow' && (tf.handle === 'from' || tf.handle === 'to')) {
      next = tf.handle === 'from' ? { ...o, from: shift ? constrainAngle(o.to, p) : p } : { ...o, to: shift ? constrainAngle(o.from, p) : p };
    } else if (isBoxed(o) && tf.handle !== 'from' && tf.handle !== 'to' && tf.handle !== 'vertex') {
      next = { ...o, rect: resizeByHandle(o.rect, tf.handle, p) };
    } else if (o.kind === 'text' && tf.handle !== 'from' && tf.handle !== 'to' && tf.handle !== 'vertex') {
      next = scaledText(o, tf.handle, p);
    }
    this.replace(next);
  }

  /** 松手。返回是否真的挪 / 改了（没动 = 单击）。 */
  endTransform(): boolean {
    const moved = this.transform?.moved ?? false;
    this.transform = null;
    this.changed();
    return moved;
  }

  private replace(next: Annotation) {
    const i = this.annotations.findIndex((a) => a.id === next.id);
    if (i < 0) return;
    this.annotations[i] = next;
    this.renderCommitted();
    this.changed();
  }

  deleteSelected(): boolean {
    const id = this.selected;
    if (!id || !this.find(id)) return false;
    this.checkpoint();
    this.annotations = this.annotations.filter((a) => a.id !== id);
    this.selected = null;
    this.renderCommitted();
    this.changed();
    return true;
  }

  /** 选中时改了二级工具条上的选项：作用到它身上（CSS 像素 → 物理像素）。 */
  applyOptions(opts: ToolOptions) {
    const a = this.selectedAnnotation;
    if (!a) return;
    const s = this.scale;
    let next: Annotation = a;
    switch (a.kind) {
      case 'rect':
      case 'ellipse': {
        const o = opts[a.kind];
        next = { ...a, color: o.color, lineWidth: o.lineWidth * s, filled: o.filled, fillOpacity: o.fillOpacity, dash: o.dash };
        break;
      }
      case 'arrow':
        next = { ...a, color: opts.arrow.color, lineWidth: opts.arrow.lineWidth * s, style: opts.arrow.style, dash: opts.arrow.dash };
        break;
      case 'pen':
        next = { ...a, color: opts.pen.color, lineWidth: opts.pen.lineWidth * s };
        break;
      case 'highlight':
        next = { ...a, color: opts.highlight.color, lineWidth: opts.highlight.lineWidth * s };
        break;
      case 'highlightRect':
        next = { ...a, color: opts.highlight.color };
        break;
      case 'spotlight':
        next = { ...a, dim: opts.highlight.dim };
        break;
      case 'magnifier':
        next = { ...a, color: opts.highlight.color };
        break;
      case 'distance':
        next = { ...a, color: opts.measure.color, lineWidth: opts.measure.lineWidth * s, unit: opts.measure.unit };
        break;
      case 'angle':
        next = { ...a, color: opts.measure.color, lineWidth: opts.measure.lineWidth * s };
        break;
      case 'counter':
        next = { ...a, color: opts.counter.color, size: opts.counter.size * s, numbering: opts.counter.numbering };
        break;
      case 'text':
        next = { ...a, color: opts.text.color, fontSize: opts.text.fontSize * s, bold: opts.text.bold, style: opts.text.style };
        break;
      case 'mosaicRect':
        next = { ...a, mode: opts.mosaic.mode, cell: mosaicCell(opts.mosaic.brushSize * s) };
        break;
      default:
        return;
    }
    if (JSON.stringify(next) === JSON.stringify(a)) return;
    // 拖透明度滑块会连着改很多次：同一个标注一小会儿之内的连续修改只记一步撤销
    const now = performance.now();
    if (this.lastRestyle?.id !== a.id || now - this.lastRestyle.at > RESTYLE_MERGE_MS) this.checkpoint();
    this.lastRestyle = { id: a.id, at: now };
    this.replace(next);
  }

  /** 选中标注的选项（物理像素 → CSS 像素），用来让二级工具条显示它当前的样子。 */
  optionsOf(a: Annotation, base: ToolOptions): ToolOptions {
    const s = this.scale;
    const near = (v: number) => Math.round((v / s) * 2) / 2;
    switch (a.kind) {
      case 'rect':
      case 'ellipse':
        return {
          ...base,
          [a.kind]: { color: a.color, lineWidth: near(a.lineWidth), filled: a.filled, fillOpacity: a.fillOpacity ?? DEFAULT_FILL_OPACITY, dash: a.dash ?? 'solid' },
        };
      case 'arrow':
        return { ...base, arrow: { color: a.color, lineWidth: near(a.lineWidth), style: a.style, dash: a.dash ?? 'solid' } };
      case 'pen':
        return { ...base, pen: { color: a.color, lineWidth: near(a.lineWidth) } };
      case 'highlight':
        return { ...base, highlight: { ...base.highlight, shape: 'brush', color: a.color, lineWidth: near(a.lineWidth) } };
      case 'highlightRect':
        return { ...base, highlight: { ...base.highlight, shape: 'rect', color: a.color } };
      case 'spotlight':
        return { ...base, highlight: { ...base.highlight, shape: 'spotlight', dim: a.dim } };
      case 'magnifier':
        return { ...base, highlight: { ...base.highlight, shape: 'magnifier', color: a.color } };
      case 'distance':
        return { ...base, measure: { shape: 'distance', color: a.color, lineWidth: near(a.lineWidth), unit: a.unit } };
      case 'angle':
        return { ...base, measure: { ...base.measure, shape: 'angle', color: a.color, lineWidth: near(a.lineWidth) } };
      case 'counter':
        return { ...base, counter: { color: a.color, size: near(a.size), numbering: a.numbering } };
      case 'text':
        return { ...base, text: { color: a.color, fontSize: near(a.fontSize), bold: a.bold, style: a.style ?? 'plain' } };
      case 'mosaicRect':
        return { ...base, mosaic: { ...base.mosaic, mode: a.mode, shape: 'rect' } };
      default:
        return base;
    }
  }

  // ───────────────────────── 文字 ─────────────────────────

  /** 点中已有文字 → 返回它（用于重新编辑） */
  hitText(p: Point): Extract<Annotation, { kind: 'text' }> | null {
    for (let i = this.annotations.length - 1; i >= 0; i -= 1) {
      const a = this.annotations[i]!;
      if (a.kind !== 'text') continue;
      if (inRect(annotationBounds(a), p)) return a;
    }
    return null;
  }

  /** 重新编辑某条文字：先从画布上拿掉，输入框盖在原位。 */
  editText(id: string) {
    const a = this.find(id);
    if (!a || a.kind !== 'text') return;
    this.checkpoint();
    this.annotations = this.annotations.filter((x) => x.id !== id);
    this.selected = null;
    this.text = { at: a.at, content: a.content, color: a.color, fontSize: a.fontSize, bold: a.bold, style: a.style, editing: a.id };
    this.renderCommitted();
    this.changed();
  }

  startText(p: Point, opts: ToolOptions) {
    const existing = this.hitText(p);
    if (existing) {
      this.editText(existing.id);
      return;
    }
    const o = opts.text;
    // 点击位置作为第一行文字的垂直中心，体验上更像"在这里打字"
    const size = o.fontSize * this.scale;
    this.selected = null;
    this.text = { at: { x: p.x, y: p.y - (size * 1.4) / 2 }, content: '', color: o.color, fontSize: size, bold: o.bold, style: o.style };
    this.changed();
  }

  updateText(content: string) {
    if (this.text) {
      this.text.content = content;
      this.changed();
    }
  }

  /** 提交文字。返回是否真的加了内容。 */
  commitText(): boolean {
    const t = this.text;
    this.text = null;
    if (t && t.content.trim()) {
      // 重新编辑的那次在 editText 时已经存过快照
      if (!t.editing) this.checkpoint();
      this.annotations.push({ kind: 'text', id: t.editing ?? newId(), at: t.at, content: t.content.replace(/\s+$/, ''), color: t.color, fontSize: t.fontSize, bold: t.bold, style: t.style });
      this.renderCommitted();
      this.changed();
      return true;
    }
    this.changed();
    return false;
  }

  textSize(t: TextDraft) {
    return measureText({ content: t.content || ' ', fontSize: t.fontSize, bold: t.bold });
  }

  // ───────────────────────── 渲染 ─────────────────────────

  private clearCanvas(c: HTMLCanvasElement | null) {
    const ctx = c?.getContext('2d');
    if (!c || !ctx) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, c.width, c.height);
  }

  private withClip(ctx: CanvasRenderingContext2D, fn: () => void) {
    ctx.save();
    if (this.clip) {
      ctx.beginPath();
      ctx.rect(this.clip.x, this.clip.y, this.clip.width, this.clip.height);
      ctx.clip();
    }
    fn();
    ctx.restore();
  }

  setOrigin(origin: Point) {
    this.origin = origin;
    this.renderCommitted();
  }

  /** 已提交画布上现在什么都没画（setClip 据此跳过重画） */
  private committedEmpty = true;

  /** `extra`：正在拖的聚光灯草稿（它要和已有的聚光灯一起挖洞，只能画在这一层） */
  renderCommitted(extra?: Annotation) {
    const c = this.committed;
    const ctx = c?.getContext('2d');
    if (!c || !ctx) return;
    const list = extra ? [...this.annotations, extra] : this.annotations;
    // 拖选区时每动一下都会走到这里（裁剪区变了）。画布和屏幕一样大，哪怕只是清空一遍，
    // 浏览器也得把整块画布重新提交给合成器；上面本来就是空的、也没有要画的，就什么都别做
    const empty = list.length === 0 && !this.watermark;
    if (empty && !this.committedDirty) return;
    this.committedDirty = !empty;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, c.width, c.height);
    this.committedEmpty = empty;
    ctx.setTransform(1, 0, 0, 1, -this.origin.x, -this.origin.y);
    this.withClip(ctx, () => this.drawAll(ctx, list, true));
  }

  /** 先统一画聚光灯的压暗（见 drawSpotlights），再按顺序画其余标注 */
  private drawAll(ctx: CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D, list: Annotation[], animate: boolean) {
    const spots = list.filter(isSpotlight);
    if (spots.length && this.clip) drawSpotlights(ctx, spots, this.clip);
    for (const a of list) drawAnnotation(ctx, a, this.backdrop, animate && this.reveal ? this.revealProgress(a) : 1);
    // 水印压在所有标注上面
    if (this.watermark && this.clip) drawWatermark(ctx, this.watermark, this.clip);
  }

  private renderDraft() {
    const c = this.drafting;
    const ctx = c?.getContext('2d');
    const d = this.draft;
    if (!c || !ctx || !d) return;
    if (d.kind === 'spotlight') {
      this.renderCommitted(d);
      return;
    }
    // 只清上一帧画过的那块，4K 画布整块清每帧也要好几毫秒
    const b = annotationBounds(d);
    const dirty = this.lastDraftBounds ? union(this.lastDraftBounds, b) : b;
    ctx.setTransform(1, 0, 0, 1, -this.origin.x, -this.origin.y);
    ctx.clearRect(dirty.x - 2, dirty.y - 2, dirty.width + 4, dirty.height + 4);
    this.lastDraftBounds = b;
    this.withClip(ctx, () => drawAnnotation(ctx, d, this.backdrop));
  }

  private get visible(): Annotation[] {
    return this.annotations.filter((a) => !(a.kind === 'label' && a.hidden));
  }

  /** 所有标注（∩ 裁剪区）的包围盒；没有标注返回 null。 */
  contentBounds(): Rect | null {
    let acc: Rect | null = null;
    for (const a of this.visible) {
      // 聚光灯压暗的是整个选区
      const b = a.kind === 'spotlight' && this.clip ? this.clip : annotationBounds(a);
      acc = acc ? union(acc, b) : b;
    }
    if (this.watermark && this.clip) acc = acc ? union(acc, this.clip) : this.clip;
    if (!acc) return null;
    const rounded = { x: Math.floor(acc.x), y: Math.floor(acc.y), width: Math.ceil(acc.width) + 1, height: Math.ceil(acc.height) + 1 };
    return this.clip ? intersect(rounded, this.clip) : rounded;
  }

  /** 导出标注层（透明背景 PNG），只导出有内容的那一块。 */
  async exportLayer(): Promise<{ png: Uint8Array; at: [number, number] } | null> {
    const b = this.contentBounds();
    if (!b) return null;
    const canvas = new OffscreenCanvas(b.width, b.height);
    const ctx = canvas.getContext('2d');
    if (!ctx) return null;
    ctx.translate(-b.x, -b.y);
    if (this.clip) {
      ctx.beginPath();
      ctx.rect(this.clip.x, this.clip.y, this.clip.width, this.clip.height);
      ctx.clip();
    }
    this.drawAll(ctx, this.visible, false);
    const blob = await canvas.convertToBlob({ type: 'image/png' });
    return { png: new Uint8Array(await blob.arrayBuffer()), at: [b.x, b.y] };
  }
}
