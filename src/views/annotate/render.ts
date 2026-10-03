// 标注绘制（规格 02 §5.2）。

import type { Point, Rect } from '@/views/capture/geometry';

import { TEXT_FONT, TEXT_LINE_HEIGHT, type Annotation } from './model';
import type { PixelSource } from './pixels';

type Ctx = CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D;

export function mosaicCell(brushSize: number): number {
  return Math.max(4, Math.round(brushSize / 2.4));
}

/** 画笔经过的格子（按格子坐标去重）。快速移动时在相邻采样点之间插值，避免断档。 */
export function mosaicCells(points: Point[], brushSize: number): Set<string> {
  const cell = mosaicCell(brushSize);
  const radius = brushSize / 2;
  const cells = new Set<string>();
  const stamp = (p: Point) => {
    const x0 = Math.floor((p.x - radius) / cell);
    const x1 = Math.floor((p.x + radius) / cell);
    const y0 = Math.floor((p.y - radius) / cell);
    const y1 = Math.floor((p.y + radius) / cell);
    for (let cy = y0; cy <= y1; cy += 1) {
      for (let cx = x0; cx <= x1; cx += 1) {
        const ccx = (cx + 0.5) * cell;
        const ccy = (cy + 0.5) * cell;
        if (Math.hypot(ccx - p.x, ccy - p.y) <= radius + cell * 0.5) cells.add(`${cx},${cy}`);
      }
    }
  };
  for (let i = 0; i < points.length; i += 1) {
    const p = points[i]!;
    const prev = points[i - 1];
    if (prev) {
      const d = Math.hypot(p.x - prev.x, p.y - prev.y);
      const steps = Math.ceil(d / (cell * 0.5));
      for (let s = 1; s < steps; s += 1) {
        stamp({ x: prev.x + ((p.x - prev.x) * s) / steps, y: prev.y + ((p.y - prev.y) * s) / steps });
      }
    }
    stamp(p);
  }
  return cells;
}

type MosaicCache = { key: string; cells: Map<string, [number, number, number]> };
const mosaicCache = new WeakMap<Annotation, MosaicCache>();

/** 渲染时可用的原图：像素读取（取平均色）+ 可直接 drawImage 的位图（模糊用）。 */
export interface Backdrop {
  pixels: PixelSource | null;
  bitmap: CanvasImageSource | null;
}

function cellsOf(a: Extract<Annotation, { kind: 'mosaic' }>, src: PixelSource | null): MosaicCache {
  const key = `${a.points.length}:${a.mode}`;
  let cache = mosaicCache.get(a);
  if (cache && cache.key === key) return cache;
  // 绘制过程中 points 只增不减：已算过的格子直接复用，只算新增的（规格 02 §5.2 性能要求）
  const prev = cache?.cells;
  const cell = mosaicCell(a.brushSize);
  const cells = new Map<string, [number, number, number]>();
  for (const c of mosaicCells(a.points, a.brushSize)) {
    const known = prev?.get(c);
    if (known) {
      cells.set(c, known);
      continue;
    }
    if (a.mode === 'blur' || !src) {
      cells.set(c, [0, 0, 0]);
      continue;
    }
    const [cx, cy] = c.split(',').map(Number) as [number, number];
    cells.set(c, src.average(cx * cell, cy * cell, cell, cell));
  }
  cache = { key, cells };
  mosaicCache.set(a, cache);
  return cache;
}

function drawMosaic(ctx: Ctx, a: Extract<Annotation, { kind: 'mosaic' }>, bg: Backdrop) {
  const cell = mosaicCell(a.brushSize);
  const { cells } = cellsOf(a, bg.pixels);
  if (a.mode === 'pixelate') {
    if (!bg.pixels) return;
    for (const [c, v] of cells) {
      const [cx, cy] = c.split(',').map(Number) as [number, number];
      ctx.fillStyle = `rgb(${v[0]},${v[1]},${v[2]})`;
      // 多画半像素，避免相邻格子之间露出抗锯齿细缝
      ctx.fillRect(cx * cell, cy * cell, cell + 0.5, cell + 0.5);
    }
    return;
  }
  // 模糊：裁到画笔经过的格子，再用画布原生 blur 滤镜把原图画进去（GPU 加速、尊重裁剪）
  if (!bg.bitmap || cells.size === 0) return;
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  ctx.beginPath();
  for (const c of cells.keys()) {
    const [cx, cy] = c.split(',').map(Number) as [number, number];
    ctx.rect(cx * cell, cy * cell, cell, cell);
    x0 = Math.min(x0, cx * cell);
    y0 = Math.min(y0, cy * cell);
    x1 = Math.max(x1, (cx + 1) * cell);
    y1 = Math.max(y1, (cy + 1) * cell);
  }
  ctx.clip();
  const r = Math.max(3, Math.round(a.brushSize / 3));
  const sx = Math.max(0, x0 - r * 2);
  const sy = Math.max(0, y0 - r * 2);
  const sw = x1 - sx + r * 4;
  const sh = y1 - sy + r * 4;
  ctx.filter = `blur(${r}px)`;
  ctx.drawImage(bg.bitmap, sx, sy, sw, sh, sx, sy, sw, sh);
  ctx.filter = 'none';
}

function drawArrow(ctx: Ctx, a: Extract<Annotation, { kind: 'arrow' }>) {
  const dx = a.to.x - a.from.x;
  const dy = a.to.y - a.from.y;
  const len = Math.hypot(dx, dy);
  if (len < 1) return;
  // 微信那种"实心三角 + 渐宽尾巴"，不是一根线加两条斜线
  const w = a.style === 'thick' ? a.lineWidth * 1.6 : a.lineWidth;
  const ux = dx / len;
  const uy = dy / len;
  const nx = -uy;
  const ny = ux;
  let headLen = w * 6;
  let headHalf = w * 2;
  if (headLen > len * 0.7) {
    const k = (len * 0.7) / headLen;
    headLen *= k;
    headHalf *= k;
  }
  const tail = Math.max(0.5, w * 0.12);
  const bx = a.to.x - ux * headLen;
  const by = a.to.y - uy * headLen;
  const half = w / 2;
  ctx.beginPath();
  ctx.moveTo(a.from.x + nx * tail, a.from.y + ny * tail);
  ctx.lineTo(bx + nx * half, by + ny * half);
  ctx.lineTo(bx + nx * headHalf, by + ny * headHalf);
  ctx.lineTo(a.to.x, a.to.y);
  ctx.lineTo(bx - nx * headHalf, by - ny * headHalf);
  ctx.lineTo(bx - nx * half, by - ny * half);
  ctx.lineTo(a.from.x - nx * tail, a.from.y - ny * tail);
  ctx.closePath();
  ctx.fillStyle = a.color;
  ctx.fill();
  ctx.lineJoin = 'round';
  ctx.lineWidth = Math.max(1, w * 0.15);
  ctx.strokeStyle = a.color;
  ctx.stroke();
}

/** 二次贝塞尔平滑：相邻采样点的中点作端点、采样点作控制点，线条才是曲线不是折线。 */
function drawPen(ctx: Ctx, a: Extract<Annotation, { kind: 'pen' }>) {
  const pts = a.points;
  if (pts.length === 0) return;
  ctx.lineCap = 'round';
  ctx.lineJoin = 'round';
  ctx.lineWidth = a.lineWidth;
  ctx.strokeStyle = a.color;
  ctx.fillStyle = a.color;
  if (pts.length === 1) {
    ctx.beginPath();
    ctx.arc(pts[0]!.x, pts[0]!.y, a.lineWidth / 2, 0, Math.PI * 2);
    ctx.fill();
    return;
  }
  ctx.beginPath();
  ctx.moveTo(pts[0]!.x, pts[0]!.y);
  for (let i = 1; i < pts.length - 1; i += 1) {
    const p = pts[i]!;
    const n = pts[i + 1]!;
    ctx.quadraticCurveTo(p.x, p.y, (p.x + n.x) / 2, (p.y + n.y) / 2);
  }
  const last = pts[pts.length - 1]!;
  ctx.lineTo(last.x, last.y);
  ctx.stroke();
}

export function textFont(a: { fontSize: number; bold: boolean }): string {
  return `${a.bold ? 600 : 400} ${a.fontSize}px ${TEXT_FONT}`;
}

function drawText(ctx: Ctx, a: Extract<Annotation, { kind: 'text' }>) {
  ctx.font = textFont(a);
  ctx.textBaseline = 'top';
  const lh = a.fontSize * TEXT_LINE_HEIGHT;
  const pad = (lh - a.fontSize) / 2;
  const lines = a.content.split('\n');
  ctx.lineJoin = 'round';
  // 1px 深色描边：浅色背景上也看得清
  ctx.lineWidth = Math.max(1, a.fontSize / 16);
  ctx.strokeStyle = 'rgba(0,0,0,0.3)';
  ctx.fillStyle = a.color;
  lines.forEach((line, i) => {
    const y = a.at.y + i * lh + pad;
    if (a.color.toUpperCase() !== '#000000') ctx.strokeText(line, a.at.x, y);
    ctx.fillText(line, a.at.x, y);
  });
}

export function drawAnnotation(ctx: Ctx, a: Annotation, bg: Backdrop): void {
  ctx.save();
  switch (a.kind) {
    case 'rect': {
      const r = a.rect;
      if (a.filled) {
        ctx.globalAlpha = 0.6;
        ctx.fillStyle = a.color;
        ctx.fillRect(r.x, r.y, r.width, r.height);
      } else {
        ctx.lineWidth = a.lineWidth;
        ctx.strokeStyle = a.color;
        ctx.lineJoin = 'miter';
        const h = a.lineWidth / 2;
        ctx.strokeRect(r.x + h, r.y + h, Math.max(0, r.width - a.lineWidth), Math.max(0, r.height - a.lineWidth));
      }
      break;
    }
    case 'ellipse': {
      const r = a.rect;
      const inset = a.filled ? 0 : a.lineWidth / 2;
      ctx.beginPath();
      ctx.ellipse(
        r.x + r.width / 2,
        r.y + r.height / 2,
        Math.max(0.5, r.width / 2 - inset),
        Math.max(0.5, r.height / 2 - inset),
        0,
        0,
        Math.PI * 2,
      );
      if (a.filled) {
        ctx.globalAlpha = 0.6;
        ctx.fillStyle = a.color;
        ctx.fill();
      } else {
        ctx.lineWidth = a.lineWidth;
        ctx.strokeStyle = a.color;
        ctx.stroke();
      }
      break;
    }
    case 'arrow':
      drawArrow(ctx, a);
      break;
    case 'pen':
      drawPen(ctx, a);
      break;
    case 'mosaic':
      drawMosaic(ctx, a, bg);
      break;
    case 'text':
      drawText(ctx, a);
      break;
  }
  ctx.restore();
}

let measureCtx: OffscreenCanvasRenderingContext2D | null = null;

export function measureText(a: { content: string; fontSize: number; bold: boolean }): { width: number; height: number } {
  measureCtx ??= new OffscreenCanvas(1, 1).getContext('2d');
  const lines = a.content.split('\n');
  let width = 0;
  if (measureCtx) {
    measureCtx.font = textFont(a);
    for (const l of lines) width = Math.max(width, measureCtx.measureText(l).width);
  }
  return { width: Math.ceil(width) + 2, height: Math.ceil(lines.length * a.fontSize * TEXT_LINE_HEIGHT) };
}

/** 标注的包围盒（含线宽），用来只导出有内容的那一块。 */
export function annotationBounds(a: Annotation): Rect {
  const pad = (n: number) => Math.ceil(n) + 2;
  switch (a.kind) {
    case 'rect':
    case 'ellipse':
      return a.rect;
    case 'arrow': {
      const w = (a.style === 'thick' ? a.lineWidth * 1.6 : a.lineWidth) * 2 + 2;
      const x = Math.min(a.from.x, a.to.x) - w;
      const y = Math.min(a.from.y, a.to.y) - w;
      return { x, y, width: Math.abs(a.to.x - a.from.x) + w * 2, height: Math.abs(a.to.y - a.from.y) + w * 2 };
    }
    case 'pen':
    case 'mosaic': {
      const r = a.kind === 'pen' ? a.lineWidth : a.brushSize + mosaicCell(a.brushSize);
      const xs = a.points.map((p) => p.x);
      const ys = a.points.map((p) => p.y);
      const x = Math.min(...xs) - pad(r);
      const y = Math.min(...ys) - pad(r);
      return { x, y, width: Math.max(...xs) - x + pad(r), height: Math.max(...ys) - y + pad(r) };
    }
    case 'text': {
      const m = measureText(a);
      return { x: a.at.x - 2, y: a.at.y - 2, width: m.width + 4, height: m.height + 4 };
    }
  }
}
