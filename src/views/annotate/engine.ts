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

import { newId, type Annotation, type Tool, type ToolOptions } from './model';
import { annotationBounds, drawAnnotation, measureText, mosaicCell, type Backdrop } from './render';

export interface TextDraft {
  at: Point;
  content: string;
  color: string;
  fontSize: number;
  bold: boolean;
  /** 重新编辑已有文字时，原来那条的 id */
  editing?: string;
}

/** 选中标注上的控制点：矩形类八个方向，箭头两头 */
export type AnnotationHandle = Handle | 'from' | 'to';

export interface SelectionInfo {
  id: string;
  kind: Annotation['kind'];
  /** 虚线框（箭头没有） */
  outline: Rect | null;
  handles: { id: AnnotationHandle; at: Point }[];
}

const MAX_HISTORY = 100;
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
      return { ...a, rect: mr(a.rect) };
    case 'arrow':
      return { ...a, from: mv(a.from), to: mv(a.to) };
    case 'pen':
    case 'mosaic':
      return { ...a, points: a.points.map(mv) };
    case 'text':
      return { ...a, at: mv(a.at) };
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
      return a.kind;
    case 'mosaicRect':
      return 'mosaic';
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

  private committed: HTMLCanvasElement | null = null;
  private drafting: HTMLCanvasElement | null = null;
  private listeners = new Set<() => void>();
  private start: Point | null = null;
  private lastDraftBounds: Rect | null = null;
  /** 已提交的那层画布上现在有没有东西（没有就不用清） */
  private committedDirty = false;
  private history: Annotation[][] = [];
  private transform: { handle: AnnotationHandle | 'move'; at: Point; orig: Annotation; moved: boolean } | null = null;

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
    this.annotations = [];
    this.draft = null;
    this.text = null;
    this.start = null;
    this.selected = null;
    this.transform = null;
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
    if (this.annotations.length === 0 && this.committedEmpty) return;
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
    const s = this.scale;
    this.start = p;
    this.selected = null;
    switch (tool) {
      case 'rect':
      case 'ellipse': {
        const o = opts[tool];
        this.draft = { kind: tool, id: newId(), rect: fromPoints(p, p), color: o.color, lineWidth: o.lineWidth * s, filled: o.filled };
        break;
      }
      case 'arrow': {
        const o = opts.arrow;
        this.draft = { kind: 'arrow', id: newId(), from: p, to: p, color: o.color, lineWidth: o.lineWidth * s, style: o.style };
        break;
      }
      case 'pen':
        this.draft = { kind: 'pen', id: newId(), points: [p], color: opts.pen.color, lineWidth: opts.pen.lineWidth * s };
        break;
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
    const d = this.draft;
    const start = this.start;
    if (!d || !start) return;
    switch (d.kind) {
      case 'rect':
      case 'ellipse':
      case 'mosaicRect':
        d.rect = fromPoints(start, shift ? constrainSquare(start, p) : p);
        break;
      case 'arrow':
        d.to = shift ? constrainAngle(start, p) : p;
        break;
      case 'pen':
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
    const d = this.draft;
    this.draft = null;
    this.start = null;
    this.clearCanvas(this.drafting);
    this.lastDraftBounds = null;
    if (!d) return;
    const tiny =
      ((d.kind === 'rect' || d.kind === 'ellipse' || d.kind === 'mosaicRect') && (d.rect.width < 3 || d.rect.height < 3)) ||
      (d.kind === 'arrow' && Math.hypot(d.to.x - d.from.x, d.to.y - d.from.y) < 4);
    if (tiny) {
      this.changed();
      return;
    }
    this.checkpoint();
    this.annotations.push(d);
    this.renderCommitted();
    this.changed();
  }

  get drawing(): boolean {
    return this.draft !== null;
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
        return !a.hidden && inRect(a.rect, p, tol);
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
    } else if (o.kind === 'arrow' && (tf.handle === 'from' || tf.handle === 'to')) {
      next = tf.handle === 'from' ? { ...o, from: shift ? constrainAngle(o.to, p) : p } : { ...o, to: shift ? constrainAngle(o.from, p) : p };
    } else if ((o.kind === 'rect' || o.kind === 'ellipse' || o.kind === 'mosaicRect' || o.kind === 'label') && tf.handle !== 'from' && tf.handle !== 'to') {
      next = { ...o, rect: resizeByHandle(o.rect, tf.handle, p) };
    } else if (o.kind === 'text' && tf.handle !== 'from' && tf.handle !== 'to') {
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
        next = { ...a, color: o.color, lineWidth: o.lineWidth * s, filled: o.filled };
        break;
      }
      case 'arrow':
        next = { ...a, color: opts.arrow.color, lineWidth: opts.arrow.lineWidth * s, style: opts.arrow.style };
        break;
      case 'pen':
        next = { ...a, color: opts.pen.color, lineWidth: opts.pen.lineWidth * s };
        break;
      case 'text':
        next = { ...a, color: opts.text.color, fontSize: opts.text.fontSize * s, bold: opts.text.bold };
        break;
      case 'mosaicRect':
        next = { ...a, mode: opts.mosaic.mode, cell: mosaicCell(opts.mosaic.brushSize * s) };
        break;
      default:
        return;
    }
    if (JSON.stringify(next) === JSON.stringify(a)) return;
    this.checkpoint();
    this.replace(next);
  }

  /** 选中标注的选项（物理像素 → CSS 像素），用来让二级工具条显示它当前的样子。 */
  optionsOf(a: Annotation, base: ToolOptions): ToolOptions {
    const s = this.scale;
    const near = (v: number) => Math.round((v / s) * 2) / 2;
    switch (a.kind) {
      case 'rect':
      case 'ellipse':
        return { ...base, [a.kind]: { color: a.color, lineWidth: near(a.lineWidth), filled: a.filled } };
      case 'arrow':
        return { ...base, arrow: { color: a.color, lineWidth: near(a.lineWidth), style: a.style } };
      case 'pen':
        return { ...base, pen: { color: a.color, lineWidth: near(a.lineWidth) } };
      case 'text':
        return { ...base, text: { color: a.color, fontSize: near(a.fontSize), bold: a.bold } };
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
    this.text = { at: a.at, content: a.content, color: a.color, fontSize: a.fontSize, bold: a.bold, editing: a.id };
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
    this.text = { at: { x: p.x, y: p.y - (size * 1.4) / 2 }, content: '', color: o.color, fontSize: size, bold: o.bold };
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
      this.annotations.push({ kind: 'text', id: t.editing ?? newId(), at: t.at, content: t.content.replace(/\s+$/, ''), color: t.color, fontSize: t.fontSize, bold: t.bold });
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

  renderCommitted() {
    const c = this.committed;
    const ctx = c?.getContext('2d');
    if (!c || !ctx) return;
    // 拖选区时每动一下都会走到这里（裁剪区变了）。画布和屏幕一样大，哪怕只是清空一遍，
    // 浏览器也得把整块画布重新提交给合成器；上面本来就是空的、也没有要画的，就什么都别做
    if (this.annotations.length === 0 && !this.committedDirty) return;
    this.committedDirty = this.annotations.length > 0;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, c.width, c.height);
    this.committedEmpty = this.annotations.length === 0;
    ctx.setTransform(1, 0, 0, 1, -this.origin.x, -this.origin.y);
    this.withClip(ctx, () => {
      for (const a of this.annotations) drawAnnotation(ctx, a, this.backdrop);
    });
  }

  private renderDraft() {
    const c = this.drafting;
    const ctx = c?.getContext('2d');
    const d = this.draft;
    if (!c || !ctx || !d) return;
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
      const b = annotationBounds(a);
      acc = acc ? union(acc, b) : b;
    }
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
    for (const a of this.visible) drawAnnotation(ctx, a, this.backdrop);
    const blob = await canvas.convertToBlob({ type: 'image/png' });
    return { png: new Uint8Array(await blob.arrayBuffer()), at: [b.x, b.y] };
  }
}
