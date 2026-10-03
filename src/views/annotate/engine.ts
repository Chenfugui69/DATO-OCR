// 标注引擎：截图遮罩和图片编辑器共用。
//
// 刻意写成命令式类而不是 React state：拖拽绘制时每个 pointermove 都要重画，走 React
// 重渲染达不到 60fps（规格 02 §4 最后一条）。React 只订阅"版本号"来刷新撤销按钮之类。
//
// 两块画布都是**图像物理像素分辨率**：已完成的标注一块、正在画的那一笔一块（画完合并）。

import { constrainAngle, constrainSquare, fromPoints, intersect, union, type Point, type Rect } from '@/views/capture/geometry';

import { newId, type Annotation, type Tool, type ToolOptions } from './model';
import { annotationBounds, drawAnnotation, measureText, type Backdrop } from './render';

export interface TextDraft {
  at: Point;
  content: string;
  color: string;
  fontSize: number;
  bold: boolean;
  /** 重新编辑已有文字时，原来那条的 id */
  editing?: string;
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

  private committed: HTMLCanvasElement | null = null;
  private drafting: HTMLCanvasElement | null = null;
  private listeners = new Set<() => void>();
  private start: Point | null = null;
  private lastDraftBounds: Rect | null = null;

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
    this.clearCanvas(this.committed);
    this.clearCanvas(this.drafting);
    this.changed();
  }

  setClip(clip: Rect | null) {
    this.clip = clip;
    this.renderCommitted();
  }

  setBackdrop(b: Backdrop) {
    this.backdrop = b;
    this.renderCommitted();
  }

  get canUndo(): boolean {
    return this.annotations.length > 0;
  }

  undo() {
    if (this.annotations.pop()) {
      this.renderCommitted();
      this.changed();
    }
  }

  // ───────────────────────── 绘制 ─────────────────────────

  begin(tool: Exclude<Tool, 'text'>, p: Point, opts: ToolOptions) {
    const s = this.scale;
    this.start = p;
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
      case 'mosaic':
        this.draft = { kind: 'mosaic', id: newId(), points: [p], brushSize: opts.mosaic.brushSize * s, mode: opts.mosaic.mode };
        break;
    }
    this.renderDraft();
  }

  update(p: Point, shift: boolean) {
    const d = this.draft;
    const start = this.start;
    if (!d || !start) return;
    switch (d.kind) {
      case 'rect':
      case 'ellipse':
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
      ((d.kind === 'rect' || d.kind === 'ellipse') && (d.rect.width < 3 || d.rect.height < 3)) ||
      (d.kind === 'arrow' && Math.hypot(d.to.x - d.from.x, d.to.y - d.from.y) < 4);
    if (tiny) return;
    this.annotations.push(d);
    this.renderCommitted();
    this.changed();
  }

  get drawing(): boolean {
    return this.draft !== null;
  }

  // ───────────────────────── 文字 ─────────────────────────

  /** 点中已有文字 → 返回它（用于重新编辑） */
  hitText(p: Point): Extract<Annotation, { kind: 'text' }> | null {
    for (let i = this.annotations.length - 1; i >= 0; i -= 1) {
      const a = this.annotations[i]!;
      if (a.kind !== 'text') continue;
      const b = annotationBounds(a);
      if (p.x >= b.x && p.x <= b.x + b.width && p.y >= b.y && p.y <= b.y + b.height) return a;
    }
    return null;
  }

  startText(p: Point, opts: ToolOptions) {
    const existing = this.hitText(p);
    if (existing) {
      this.annotations = this.annotations.filter((a) => a.id !== existing.id);
      this.text = { at: existing.at, content: existing.content, color: existing.color, fontSize: existing.fontSize, bold: existing.bold, editing: existing.id };
      this.renderCommitted();
    } else {
      const o = opts.text;
      // 点击位置作为第一行文字的垂直中心，体验上更像"在这里打字"
      const size = o.fontSize * this.scale;
      this.text = { at: { x: p.x, y: p.y - (size * 1.4) / 2 }, content: '', color: o.color, fontSize: size, bold: o.bold };
    }
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

  renderCommitted() {
    const c = this.committed;
    const ctx = c?.getContext('2d');
    if (!c || !ctx) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, c.width, c.height);
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

  /** 所有标注（∩ 裁剪区）的包围盒；没有标注返回 null。 */
  contentBounds(): Rect | null {
    let acc: Rect | null = null;
    for (const a of this.annotations) {
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
    for (const a of this.annotations) drawAnnotation(ctx, a, this.backdrop);
    const blob = await canvas.convertToBlob({ type: 'image/png' });
    return { png: new Uint8Array(await blob.arrayBuffer()), at: [b.x, b.y] };
  }
}
