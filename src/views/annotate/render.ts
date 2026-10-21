// 标注绘制（规格 02 §5.2）。

import { readableOn } from '@/lib/format';
import type { Point, Rect } from '@/views/capture/geometry';

import {
  DEFAULT_FILL_OPACITY,
  formatCounter,
  HIGHLIGHT_ALPHA,
  TEXT_FONT,
  TEXT_LINE_HEIGHT,
  type Annotation,
  type Dash,
  type Watermark,
} from './model';
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
  // 模糊：裁到画笔经过的格子，再把模糊后的原图画进去（见 `drawBlurred`）
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
  drawBlurred(ctx, bg.bitmap, sx, sy, sw, sh, r);
}

/**
 * 画布原生的 blur 滤镜（`ctx.filter`）macOS 的 WebKit 不支持：设了不报错也不生效，画出来就是原图，
 * 模糊马赛克等于没打。那里改成先缩小再平滑放大回去，效果接近高斯模糊，缩放走 GPU。
 */
const canvasFilter = typeof CanvasRenderingContext2D !== 'undefined' && 'filter' in CanvasRenderingContext2D.prototype;
let blurScratch: HTMLCanvasElement | OffscreenCanvas | null = null;

/** 把原图 (sx, sy, sw, sh) 这块模糊后画回同一位置（调用方先裁好剪切区域）。 */
function drawBlurred(ctx: Ctx, src: CanvasImageSource, sx: number, sy: number, sw: number, sh: number, radius: number) {
  if (canvasFilter) {
    ctx.filter = `blur(${radius}px)`;
    ctx.drawImage(src, sx, sy, sw, sh, sx, sy, sw, sh);
    ctx.filter = 'none';
    return;
  }
  // 缩到约 1/radius；一次缩太多会跳过像素（出来是噪点不是模糊），每次最多缩一半
  const target = Math.max(1, radius);
  let w = Math.max(1, Math.round(sw));
  let h = Math.max(1, Math.round(sh));
  if (!blurScratch) blurScratch = typeof OffscreenCanvas !== 'undefined' ? new OffscreenCanvas(w, h) : document.createElement('canvas');
  if (blurScratch.width < w || blurScratch.height < h) {
    blurScratch.width = Math.max(blurScratch.width, w);
    blurScratch.height = Math.max(blurScratch.height, h);
  }
  const tmp = blurScratch.getContext('2d') as CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D | null;
  if (!tmp) return;
  tmp.imageSmoothingEnabled = true;
  tmp.imageSmoothingQuality = 'high';
  tmp.clearRect(0, 0, w, h);
  tmp.drawImage(src, sx, sy, sw, sh, 0, 0, w, h);
  let scale = 1;
  while (scale * 2 <= target) {
    const nw = Math.max(1, Math.round(w / 2));
    const nh = Math.max(1, Math.round(h / 2));
    tmp.drawImage(blurScratch, 0, 0, w, h, 0, 0, nw, nh);
    w = nw;
    h = nh;
    scale *= 2;
  }
  const smoothing = ctx.imageSmoothingEnabled;
  const quality = ctx.imageSmoothingQuality;
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = 'high';
  ctx.drawImage(blurScratch, 0, 0, w, h, sx, sy, sw, sh);
  ctx.imageSmoothingEnabled = smoothing;
  ctx.imageSmoothingQuality = quality;
}

/** 矩形马赛克每个格子的颜色，按格子左上角的绝对坐标存 */
const rectCache = new WeakMap<Annotation, Map<string, [number, number, number]>>();

/**
 * 矩形马赛克：格子从框的左上角起排。拖动画框时每一帧都要画出打码效果，所以格子颜色按绝对位置
 * 缓存：往右下拖时起点不变，已经取过色的格子直接用，只算新露出来的。
 */
function drawMosaicRect(ctx: Ctx, a: Extract<Annotation, { kind: 'mosaicRect' }>, bg: Backdrop) {
  const r = a.rect;
  if (r.width < 1 || r.height < 1) return;
  if (a.mode === 'blur') {
    if (!bg.bitmap) return;
    ctx.beginPath();
    ctx.rect(r.x, r.y, r.width, r.height);
    ctx.clip();
    const blur = Math.max(4, Math.round(a.cell * 0.8));
    const sx = Math.max(0, r.x - blur * 2);
    const sy = Math.max(0, r.y - blur * 2);
    const sw = r.width + blur * 4;
    const sh = r.height + blur * 4;
    drawBlurred(ctx, bg.bitmap, sx, sy, sw, sh, blur);
    return;
  }
  const src = bg.pixels;
  if (!src) return;
  const cols = Math.ceil(r.width / a.cell);
  const rows = Math.ceil(r.height / a.cell);
  let cache = rectCache.get(a);
  if (!cache) {
    cache = new Map();
    rectCache.set(a, cache);
  }
  ctx.beginPath();
  ctx.rect(r.x, r.y, r.width, r.height);
  ctx.clip();
  for (let row = 0; row < rows; row += 1) {
    for (let col = 0; col < cols; col += 1) {
      const x = r.x + col * a.cell;
      const y = r.y + row * a.cell;
      const key = `${x},${y},${a.cell}`;
      let v = cache.get(key);
      if (!v) {
        // 按整格取色（框边上的半格也按整格算），格子位置不变颜色就不变，拖动时不闪
        v = src.average(x, y, a.cell, a.cell);
        cache.set(key, v);
      }
      ctx.fillStyle = `rgb(${v[0]},${v[1]},${v[2]})`;
      ctx.fillRect(x, y, a.cell + 0.5, a.cell + 0.5);
    }
  }
}

/** 文字块排版：按框宽自动换行。英文按词断，中日韩按字断。 */
export function wrapLabel(text: string, fontSize: number, maxWidth: number): string[] {
  measureCtx ??= new OffscreenCanvas(1, 1).getContext('2d');
  const ctx = measureCtx;
  if (!ctx) return [text];
  ctx.font = textFont({ fontSize, bold: false });
  const out: string[] = [];
  for (const para of text.split('\n')) {
    // 词：连续的拉丁字母数字算一个，其他字符（汉字、标点、空格）各算一个
    const tokens = para.match(/[A-Za-z0-9\u00C0-\u024F'’-]+|\s+|./gu) ?? [''];
    let line = '';
    for (const tok of tokens) {
      const next = line + tok;
      if (line && ctx.measureText(next).width > maxWidth) {
        out.push(line.trimEnd());
        line = tok.trimStart();
        // 单个词就比框宽：硬拆
        while (line && ctx.measureText(line).width > maxWidth) {
          let n = line.length - 1;
          while (n > 1 && ctx.measureText(line.slice(0, n)).width > maxWidth) n -= 1;
          out.push(line.slice(0, n));
          line = line.slice(n);
        }
      } else {
        line = next;
      }
    }
    out.push(line.trimEnd());
  }
  return out;
}

export const LABEL_LINE_HEIGHT = 1.28;

/** 译文显现动画一块的时长（毫秒），见 drawLabel 的 progress */
export const LABEL_REVEAL_MS = 640;

/** progress < 1：正在显现。底色一开始就铺满（盖住原文），字一行接一行由虚到实、从下面一点浮上来 */
function drawLabel(ctx: Ctx, a: Extract<Annotation, { kind: 'label' }>, progress = 1) {
  const r = a.rect;
  ctx.fillStyle = a.bg;
  ctx.fillRect(r.x, r.y, r.width, r.height);
  const pad = Math.max(1, a.fontSize * 0.08);
  const lines = wrapLabel(a.text, a.fontSize, r.width - pad * 2);
  const lh = a.fontSize * LABEL_LINE_HEIGHT;
  const total = lines.length * lh;
  // 字少就在框里竖直居中，多了从顶上排（超出的部分照画，不裁）
  const top = total < r.height ? r.y + (r.height - total) / 2 : r.y;
  ctx.font = textFont({ fontSize: a.fontSize, bold: false });
  ctx.textBaseline = 'middle';
  ctx.fillStyle = a.color;
  if (progress >= 1) {
    lines.forEach((line, i) => ctx.fillText(line, r.x + pad, top + i * lh + lh / 2));
    return;
  }
  // 多行时后一行比前一行晚一点起步，总时长不变
  const n = lines.length;
  const lag = n > 1 ? Math.min(0.14, 0.42 / (n - 1)) : 0;
  lines.forEach((line, i) => {
    const q = Math.min(1, Math.max(0, (progress - i * lag) / (1 - (n - 1) * lag)));
    if (q <= 0) return;
    const e = 1 - (1 - q) ** 4;
    ctx.globalAlpha = e;
    ctx.filter = e < 0.995 ? `blur(${((1 - e) * a.fontSize * 0.28).toFixed(2)}px)` : 'none';
    ctx.fillText(line, r.x + pad, top + i * lh + lh / 2 + (1 - e) * a.fontSize * 0.24);
  });
  ctx.globalAlpha = 1;
  ctx.filter = 'none';
}

/** 虚线 / 点线。点线用圆头的零长线段画成一个个圆点 */
function applyDash(ctx: Ctx, dash: Dash | undefined, lineWidth: number) {
  if (dash === 'dashed') ctx.setLineDash([lineWidth * 3, lineWidth * 2]);
  else if (dash === 'dotted') {
    ctx.lineCap = 'round';
    ctx.setLineDash([0.01, lineWidth * 2]);
  } else ctx.setLineDash([]);
}

/** 实心三角箭头头，尖在 tip，沿 (ux, uy) 方向 */
function arrowHead(ctx: Ctx, tip: Point, ux: number, uy: number, len: number, half: number) {
  const bx = tip.x - ux * len;
  const by = tip.y - uy * len;
  ctx.beginPath();
  ctx.moveTo(tip.x, tip.y);
  ctx.lineTo(bx - uy * half, by + ux * half);
  ctx.lineTo(bx + uy * half, by - ux * half);
  ctx.closePath();
  ctx.fill();
}

/** 直线、双向箭头、虚线 / 点线的箭头：一根等宽的线，两头按需要加三角 */
function drawLineArrow(ctx: Ctx, a: Extract<Annotation, { kind: 'arrow' }>) {
  const dx = a.to.x - a.from.x;
  const dy = a.to.y - a.from.y;
  const len = Math.hypot(dx, dy);
  if (len < 1) return;
  const ux = dx / len;
  const uy = dy / len;
  const w = a.lineWidth;
  const headEnd = a.style !== 'line';
  const headStart = a.style === 'double';
  let headLen = Math.max(w * 3.6, 8);
  let headHalf = Math.max(w * 1.8, 4);
  const heads = (headEnd ? 1 : 0) + (headStart ? 1 : 0);
  if (heads && headLen * heads > len * 0.8) {
    const k = (len * 0.8) / (headLen * heads);
    headLen *= k;
    headHalf *= k;
  }
  // 线身让开箭头头，免得虚线的一截从三角尖上戳出来
  const from = headStart ? { x: a.from.x + ux * headLen * 0.8, y: a.from.y + uy * headLen * 0.8 } : a.from;
  const to = headEnd ? { x: a.to.x - ux * headLen * 0.8, y: a.to.y - uy * headLen * 0.8 } : a.to;
  ctx.strokeStyle = a.color;
  ctx.fillStyle = a.color;
  ctx.lineWidth = w;
  ctx.lineCap = 'round';
  applyDash(ctx, a.dash, w);
  ctx.beginPath();
  ctx.moveTo(from.x, from.y);
  ctx.lineTo(to.x, to.y);
  ctx.stroke();
  ctx.setLineDash([]);
  if (headEnd) arrowHead(ctx, a.to, ux, uy, headLen, headHalf);
  if (headStart) arrowHead(ctx, a.from, -ux, -uy, headLen, headHalf);
}

function drawArrow(ctx: Ctx, a: Extract<Annotation, { kind: 'arrow' }>) {
  if (a.style === 'line' || a.style === 'double' || (a.dash && a.dash !== 'solid')) {
    drawLineArrow(ctx, a);
    return;
  }
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

/** 荧光笔涂抹：整条路径一次描完再半透明，交叠的地方不会越叠越深 */
function drawHighlight(ctx: Ctx, a: Extract<Annotation, { kind: 'highlight' }>) {
  ctx.globalAlpha = HIGHLIGHT_ALPHA;
  drawPen(ctx, { kind: 'pen', id: a.id, points: a.points, color: a.color, lineWidth: a.lineWidth });
}

/** 序号：实心圆 + 白描边，编号按圆的底色挑黑字或白字 */
function drawCounter(ctx: Ctx, a: Extract<Annotation, { kind: 'counter' }>) {
  const r = a.size / 2;
  ctx.beginPath();
  ctx.arc(a.at.x, a.at.y, r, 0, Math.PI * 2);
  ctx.fillStyle = a.color;
  ctx.shadowColor = 'rgba(0,0,0,0.3)';
  ctx.shadowBlur = Math.max(1, a.size * 0.12);
  ctx.shadowOffsetY = Math.max(0.5, a.size * 0.04);
  ctx.fill();
  ctx.shadowColor = 'transparent';
  ctx.lineWidth = Math.max(1, a.size * 0.07);
  ctx.strokeStyle = '#fff';
  ctx.stroke();
  const label = formatCounter(a.n, a.numbering);
  // 字多了缩小字号，别撑出圆外
  const base = a.size * 0.56;
  ctx.font = `600 ${base}px ${TEXT_FONT}`;
  const w = ctx.measureText(label).width;
  const fit = Math.min(1, (a.size * 0.78) / Math.max(1, w));
  ctx.font = `600 ${base * fit}px ${TEXT_FONT}`;
  ctx.fillStyle = readableOn(a.color) === 'white' ? '#fff' : 'rgba(0,0,0,0.85)';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText(label, a.at.x, a.at.y + a.size * 0.02);
}

/**
 * 聚光灯：在 `area`（选区 / 整张图）里铺一层压暗，再把每个聚光灯框擦掉。几个聚光灯一起画、只压暗一次
 * （各画各的话框和框之间会互相压暗）。必须在别的标注之前画：擦框用的是 destination-out，会把画布上
 * 已有的东西一起擦掉。
 */
export function drawSpotlights(ctx: Ctx, spots: Extract<Annotation, { kind: 'spotlight' }>[], area: Rect) {
  if (!spots.length) return;
  ctx.save();
  ctx.fillStyle = `rgba(0,0,0,${Math.max(...spots.map((s) => s.dim))})`;
  ctx.fillRect(area.x, area.y, area.width, area.height);
  ctx.globalCompositeOperation = 'destination-out';
  ctx.fillStyle = '#000';
  for (const s of spots) ctx.fillRect(s.rect.x, s.rect.y, s.rect.width, s.rect.height);
  ctx.restore();
}

/**
 * 放大镜标注：原处一个细框，旁边一个圆里画放大后的画面，中间连一根线。
 * 圆里显示的是以那块的中心为中心、边长等于那块长边的正方形（不是正方形的那块，短的方向多露一点）。
 */
function drawMagnifier(ctx: Ctx, a: Extract<Annotation, { kind: 'magnifier' }>, bg: Backdrop) {
  const s = a.source;
  const lw = Math.max(2, a.radius * 0.035);
  // 连线：从框的中心指向圆心，两头都截到边上
  const sx = s.x + s.width / 2;
  const sy = s.y + s.height / 2;
  const dx = a.center.x - sx;
  const dy = a.center.y - sy;
  const len = Math.hypot(dx, dy);
  if (len > a.radius + 2) {
    const ux = dx / len;
    const uy = dy / len;
    // 射线离开框的位置
    const tx = ux ? (s.width / 2) / Math.abs(ux) : Infinity;
    const ty = uy ? (s.height / 2) / Math.abs(uy) : Infinity;
    const t0 = Math.min(tx, ty);
    ctx.beginPath();
    ctx.moveTo(sx + ux * t0, sy + uy * t0);
    ctx.lineTo(a.center.x - ux * a.radius, a.center.y - uy * a.radius);
    ctx.strokeStyle = a.color;
    ctx.lineWidth = lw * 0.75;
    ctx.stroke();
  }
  ctx.strokeStyle = a.color;
  ctx.lineWidth = lw * 0.75;
  ctx.strokeRect(s.x, s.y, s.width, s.height);
  // 圆：先投影，再在圆里画放大的画面
  ctx.save();
  ctx.beginPath();
  ctx.arc(a.center.x, a.center.y, a.radius, 0, Math.PI * 2);
  ctx.shadowColor = 'rgba(0,0,0,0.35)';
  ctx.shadowBlur = a.radius * 0.12;
  ctx.shadowOffsetY = a.radius * 0.03;
  ctx.fillStyle = '#fff';
  ctx.fill();
  ctx.restore();
  if (bg.bitmap) {
    ctx.save();
    ctx.beginPath();
    ctx.arc(a.center.x, a.center.y, a.radius, 0, Math.PI * 2);
    ctx.clip();
    const side = Math.max(1, s.width, s.height);
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = 'high';
    ctx.drawImage(bg.bitmap, sx - side / 2, sy - side / 2, side, side, a.center.x - a.radius, a.center.y - a.radius, a.radius * 2, a.radius * 2);
    ctx.restore();
  }
  ctx.beginPath();
  ctx.arc(a.center.x, a.center.y, a.radius, 0, Math.PI * 2);
  ctx.lineWidth = lw * 1.6;
  ctx.strokeStyle = '#fff';
  ctx.stroke();
  ctx.lineWidth = lw;
  ctx.strokeStyle = a.color;
  ctx.stroke();
}

/** 放大镜标注的圆默认放在框的哪边、多大：放大两倍，摆在框的右边（右边放不下放左边） */
export function placeMagnifier(source: Rect, area: Rect | null): { center: Point; radius: number } {
  const radius = Math.max(24, Math.max(source.width, source.height));
  const gap = radius * 0.5;
  const cy = source.y + source.height / 2;
  let cx = source.x + source.width + gap + radius;
  if (area && cx + radius > area.x + area.width) cx = source.x - gap - radius;
  if (area) {
    cx = Math.min(Math.max(cx, area.x + radius), area.x + area.width - radius);
  }
  const y = area ? Math.min(Math.max(cy, area.y + radius), area.y + area.height - radius) : cy;
  return { center: { x: cx, y }, radius };
}

/** 测量标签：小圆角色块里写数值，色块用标注的颜色 */
function measureLabel(ctx: Ctx, text: string, at: Point, color: string, size: number) {
  ctx.font = `600 ${size}px ${TEXT_FONT}`;
  const w = ctx.measureText(text).width + size * 0.9;
  const h = size * 1.5;
  ctx.fillStyle = color;
  ctx.beginPath();
  ctx.roundRect(at.x - w / 2, at.y - h / 2, w, h, h / 2);
  ctx.fill();
  ctx.fillStyle = readableOn(color) === 'white' ? '#fff' : 'rgba(0,0,0,0.85)';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText(text, at.x, at.y + size * 0.03);
}

/** 测距离标签上的数值：物理像素，或者按创建时的缩放换算成点 */
export function distanceText(a: Extract<Annotation, { kind: 'distance' }>): string {
  const px = Math.hypot(a.to.x - a.from.x, a.to.y - a.from.y);
  return a.unit === 'pt' ? `${Math.round(px / Math.max(0.1, a.scale))} pt` : `${Math.round(px)} px`;
}

function drawDistance(ctx: Ctx, a: Extract<Annotation, { kind: 'distance' }>) {
  const dx = a.to.x - a.from.x;
  const dy = a.to.y - a.from.y;
  const len = Math.hypot(dx, dy);
  if (len < 1) return;
  const nx = -dy / len;
  const ny = dx / len;
  const tick = Math.max(6, a.lineWidth * 4);
  ctx.strokeStyle = a.color;
  ctx.lineWidth = a.lineWidth;
  ctx.lineCap = 'round';
  ctx.beginPath();
  ctx.moveTo(a.from.x, a.from.y);
  ctx.lineTo(a.to.x, a.to.y);
  // 两头的短横：标出量的是哪一段
  for (const p of [a.from, a.to]) {
    ctx.moveTo(p.x - nx * tick, p.y - ny * tick);
    ctx.lineTo(p.x + nx * tick, p.y + ny * tick);
  }
  ctx.stroke();
  const size = Math.max(10, a.lineWidth * 6);
  measureLabel(ctx, distanceText(a), { x: (a.from.x + a.to.x) / 2 + nx * size * 1.3, y: (a.from.y + a.to.y) / 2 + ny * size * 1.3 }, a.color, size);
}

/** 两条边夹的角（0–180°） */
export function angleDegrees(a: Extract<Annotation, { kind: 'angle' }>): number {
  const a1 = Math.atan2(a.a.y - a.vertex.y, a.a.x - a.vertex.x);
  const a2 = Math.atan2(a.b.y - a.vertex.y, a.b.x - a.vertex.x);
  let d = Math.abs(a2 - a1);
  if (d > Math.PI) d = Math.PI * 2 - d;
  return (d * 180) / Math.PI;
}

function drawAngle(ctx: Ctx, a: Extract<Annotation, { kind: 'angle' }>) {
  ctx.strokeStyle = a.color;
  ctx.lineWidth = a.lineWidth;
  ctx.lineCap = 'round';
  ctx.lineJoin = 'round';
  ctx.beginPath();
  ctx.moveTo(a.a.x, a.a.y);
  ctx.lineTo(a.vertex.x, a.vertex.y);
  ctx.lineTo(a.b.x, a.b.y);
  ctx.stroke();
  const la = Math.hypot(a.a.x - a.vertex.x, a.a.y - a.vertex.y);
  const lb = Math.hypot(a.b.x - a.vertex.x, a.b.y - a.vertex.y);
  if (la < 2 || lb < 2) return;
  // 圆弧画在小于 180° 的那一侧
  const a1 = Math.atan2(a.a.y - a.vertex.y, a.a.x - a.vertex.x);
  const a2 = Math.atan2(a.b.y - a.vertex.y, a.b.x - a.vertex.x);
  let delta = a2 - a1;
  while (delta > Math.PI) delta -= Math.PI * 2;
  while (delta < -Math.PI) delta += Math.PI * 2;
  const r = Math.min(Math.max(16, a.lineWidth * 12), la * 0.6, lb * 0.6);
  ctx.beginPath();
  ctx.arc(a.vertex.x, a.vertex.y, r, a1, a1 + delta, delta < 0);
  ctx.stroke();
  const mid = a1 + delta / 2;
  const size = Math.max(10, a.lineWidth * 6);
  const lr = r + size * 1.6;
  measureLabel(ctx, `${angleDegrees(a).toFixed(1)}°`, { x: a.vertex.x + Math.cos(mid) * lr, y: a.vertex.y + Math.sin(mid) * lr }, a.color, size);
}

/** 水印：文字转一个角度，在 `area` 里铺满（交错排开，像砖墙） */
export function drawWatermark(ctx: Ctx, w: Watermark, area: Rect) {
  const text = w.text.trim();
  if (!text) return;
  ctx.save();
  ctx.beginPath();
  ctx.rect(area.x, area.y, area.width, area.height);
  ctx.clip();
  ctx.globalAlpha = w.opacity;
  ctx.fillStyle = w.color;
  ctx.font = `500 ${w.fontSize}px ${TEXT_FONT}`;
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  const stepX = ctx.measureText(text).width + w.fontSize * 4;
  const stepY = w.fontSize * 5;
  const cx = area.x + area.width / 2;
  const cy = area.y + area.height / 2;
  ctx.translate(cx, cy);
  ctx.rotate((w.angle * Math.PI) / 180);
  // 转过之后要盖住整块：按对角线的长度铺
  const half = Math.hypot(area.width, area.height) / 2 + stepX;
  for (let row = 0, y = -half; y <= half; row += 1, y += stepY) {
    const offset = row % 2 ? stepX / 2 : 0;
    for (let x = -half + offset; x <= half; x += stepX) ctx.fillText(text, x, y);
  }
  ctx.restore();
}

/** 文字底色块的留白（字号的比例） */
const TEXT_FILL_PAD_X = 0.32;
const TEXT_FILL_PAD_Y = 0.1;

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
  if (a.style === 'fill') {
    // 底色块：选的颜色当底，字按底色深浅挑黑或白
    const m = measureText(a);
    const px = a.fontSize * TEXT_FILL_PAD_X;
    const py = a.fontSize * TEXT_FILL_PAD_Y;
    ctx.fillStyle = a.color;
    ctx.beginPath();
    ctx.roundRect(a.at.x - px, a.at.y - py, m.width + px * 2, m.height + py * 2, a.fontSize * 0.3);
    ctx.fill();
    ctx.fillStyle = readableOn(a.color) === 'white' ? '#fff' : 'rgba(0,0,0,0.88)';
    lines.forEach((line, i) => ctx.fillText(line, a.at.x, a.at.y + i * lh + pad));
    return;
  }
  if (a.style === 'outline') {
    // 白色粗描边：压在花哨背景上也看得清
    ctx.lineWidth = Math.max(2, a.fontSize / 5);
    ctx.strokeStyle = readableOn(a.color) === 'white' ? '#fff' : '#000';
    ctx.fillStyle = a.color;
    lines.forEach((line, i) => {
      const y = a.at.y + i * lh + pad;
      ctx.strokeText(line, a.at.x, y);
      ctx.fillText(line, a.at.x, y);
    });
    return;
  }
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

export function drawAnnotation(ctx: Ctx, a: Annotation, bg: Backdrop, progress = 1): void {
  ctx.save();
  switch (a.kind) {
    case 'rect': {
      const r = a.rect;
      if (a.filled) {
        ctx.globalAlpha = a.fillOpacity ?? DEFAULT_FILL_OPACITY;
        ctx.fillStyle = a.color;
        ctx.fillRect(r.x, r.y, r.width, r.height);
      } else {
        ctx.lineWidth = a.lineWidth;
        ctx.strokeStyle = a.color;
        ctx.lineJoin = 'miter';
        applyDash(ctx, a.dash, a.lineWidth);
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
        ctx.globalAlpha = a.fillOpacity ?? DEFAULT_FILL_OPACITY;
        ctx.fillStyle = a.color;
        ctx.fill();
      } else {
        ctx.lineWidth = a.lineWidth;
        ctx.strokeStyle = a.color;
        applyDash(ctx, a.dash, a.lineWidth);
        ctx.stroke();
      }
      break;
    }
    case 'highlight':
      drawHighlight(ctx, a);
      break;
    case 'highlightRect': {
      const r = a.rect;
      ctx.globalAlpha = HIGHLIGHT_ALPHA;
      ctx.fillStyle = a.color;
      ctx.fillRect(r.x, r.y, r.width, r.height);
      break;
    }
    case 'spotlight':
      // 压暗由引擎统一画（见 drawSpotlights）
      break;
    case 'counter':
      drawCounter(ctx, a);
      break;
    case 'magnifier':
      drawMagnifier(ctx, a, bg);
      break;
    case 'distance':
      drawDistance(ctx, a);
      break;
    case 'angle':
      drawAngle(ctx, a);
      break;
    case 'arrow':
      drawArrow(ctx, a);
      break;
    case 'pen':
      drawPen(ctx, a);
      break;
    case 'mosaic':
      drawMosaic(ctx, a, bg);
      break;
    case 'mosaicRect':
      drawMosaicRect(ctx, a, bg);
      break;
    case 'text':
      drawText(ctx, a);
      break;
    case 'label':
      if (!a.hidden) drawLabel(ctx, a, progress);
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
    case 'mosaicRect':
    case 'label':
    case 'highlightRect':
    case 'spotlight':
      return a.rect;
    case 'counter': {
      const r = a.size / 2 + Math.max(2, a.size * 0.2);
      return { x: a.at.x - r, y: a.at.y - r, width: r * 2, height: r * 2 };
    }
    case 'distance':
    case 'angle': {
      const pts = a.kind === 'distance' ? [a.from, a.to] : [a.vertex, a.a, a.b];
      // 标签在线外面一点，留足余量
      const pad = Math.max(10, a.lineWidth * 6) * 4 + a.lineWidth * 4;
      const xs = pts.map((p) => p.x);
      const ys = pts.map((p) => p.y);
      const x = Math.min(...xs) - pad;
      const y = Math.min(...ys) - pad;
      return { x, y, width: Math.max(...xs) - x + pad, height: Math.max(...ys) - y + pad };
    }
    case 'magnifier': {
      const r = a.radius * 1.2 + 4;
      const x = Math.min(a.source.x, a.center.x - r);
      const y = Math.min(a.source.y, a.center.y - r);
      return {
        x,
        y,
        width: Math.max(a.source.x + a.source.width, a.center.x + r) - x,
        height: Math.max(a.source.y + a.source.height, a.center.y + r) - y,
      };
    }
    case 'arrow': {
      const w = (a.style === 'thick' ? a.lineWidth * 1.6 : Math.max(a.lineWidth * 1.8, 4)) * 2 + 2;
      const x = Math.min(a.from.x, a.to.x) - w;
      const y = Math.min(a.from.y, a.to.y) - w;
      return { x, y, width: Math.abs(a.to.x - a.from.x) + w * 2, height: Math.abs(a.to.y - a.from.y) + w * 2 };
    }
    case 'pen':
    case 'highlight':
    case 'mosaic': {
      const r = a.kind === 'mosaic' ? a.brushSize + mosaicCell(a.brushSize) : a.lineWidth;
      const xs = a.points.map((p) => p.x);
      const ys = a.points.map((p) => p.y);
      const x = Math.min(...xs) - pad(r);
      const y = Math.min(...ys) - pad(r);
      return { x, y, width: Math.max(...xs) - x + pad(r), height: Math.max(...ys) - y + pad(r) };
    }
    case 'text': {
      const m = measureText(a);
      const px = a.style === 'fill' ? a.fontSize * TEXT_FILL_PAD_X : a.style === 'outline' ? a.fontSize / 10 : 0;
      const py = a.style === 'fill' ? a.fontSize * TEXT_FILL_PAD_Y : a.style === 'outline' ? a.fontSize / 10 : 0;
      return { x: a.at.x - 2 - px, y: a.at.y - 2 - py, width: m.width + 4 + px * 2, height: m.height + 4 + py * 2 };
    }
  }
}
