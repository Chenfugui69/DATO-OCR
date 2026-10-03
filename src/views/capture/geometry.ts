// 截图选区几何（纯函数，单元测试见 geometry.test.ts）。
//
// 坐标约定：遮罩窗口只覆盖一块显示器。所有选区状态存**本屏局部物理像素**（整数），
// 只在渲染时除以缩放因子换成 CSS 像素。混合 DPI 下反过来存 CSS 像素会累积误差。

export interface Point {
  x: number;
  y: number;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type Handle = 'nw' | 'n' | 'ne' | 'e' | 'se' | 's' | 'sw' | 'w';

export const HANDLES: Handle[] = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

export const HANDLE_CURSORS: Record<Handle, string> = {
  nw: 'nwse-resize',
  se: 'nwse-resize',
  ne: 'nesw-resize',
  sw: 'nesw-resize',
  n: 'ns-resize',
  s: 'ns-resize',
  e: 'ew-resize',
  w: 'ew-resize',
};

export const right = (r: Rect) => r.x + r.width;
export const bottom = (r: Rect) => r.y + r.height;

export function contains(r: Rect, p: Point): boolean {
  return p.x >= r.x && p.x < right(r) && p.y >= r.y && p.y < bottom(r);
}

/** 两点 → 正矩形（任意方向拖都行），包含两个端点所在像素，最小 1×1。 */
export function fromPoints(a: Point, b: Point): Rect {
  const x = Math.min(a.x, b.x);
  const y = Math.min(a.y, b.y);
  return { x, y, width: Math.abs(a.x - b.x) + 1, height: Math.abs(a.y - b.y) + 1 };
}

export function intersect(a: Rect, b: Rect): Rect | null {
  const x = Math.max(a.x, b.x);
  const y = Math.max(a.y, b.y);
  const r = Math.min(right(a), right(b));
  const btm = Math.min(bottom(a), bottom(b));
  return r > x && btm > y ? { x, y, width: r - x, height: btm - y } : null;
}

export function union(a: Rect, b: Rect): Rect {
  const x = Math.min(a.x, b.x);
  const y = Math.min(a.y, b.y);
  return { x, y, width: Math.max(right(a), right(b)) - x, height: Math.max(bottom(a), bottom(b)) - y };
}

export function area(r: Rect): number {
  return r.width * r.height;
}

/** 整体移动后夹回边界内（选区不能移出屏幕）。 */
export function clampMove(r: Rect, bounds: Rect): Rect {
  const width = Math.min(r.width, bounds.width);
  const height = Math.min(r.height, bounds.height);
  const x = Math.max(bounds.x, Math.min(r.x, right(bounds) - width));
  const y = Math.max(bounds.y, Math.min(r.y, bottom(bounds) - height));
  return { x, y, width, height };
}

/** 裁到边界内（框选、调整时用）。 */
export function clampRect(r: Rect, bounds: Rect): Rect {
  return intersect(r, bounds) ?? { x: bounds.x, y: bounds.y, width: 1, height: 1 };
}

/**
 * 拖动某个控制点调整选区。允许拖过对边（自动翻转）：始终以"固定的对角/对边"和
 * 当前鼠标位置重新求矩形。
 */
export function resizeByHandle(orig: Rect, handle: Handle, p: Point): Rect {
  let x1 = orig.x;
  let y1 = orig.y;
  let x2 = right(orig) - 1;
  let y2 = bottom(orig) - 1;
  if (handle.includes('w')) x1 = p.x;
  if (handle.includes('e')) x2 = p.x;
  if (handle.includes('n')) y1 = p.y;
  if (handle.includes('s')) y2 = p.y;
  return fromPoints({ x: x1, y: y1 }, { x: x2, y: y2 });
}

export function handlePoint(r: Rect, h: Handle): Point {
  const cx = r.x + r.width / 2;
  const cy = r.y + r.height / 2;
  const x = h.includes('w') ? r.x : h.includes('e') ? right(r) : cx;
  const y = h.includes('n') ? r.y : h.includes('s') ? bottom(r) : cy;
  return { x, y };
}

/** 鼠标在哪个控制点上（`radius` 为命中半径，物理像素）。 */
export function hitHandle(r: Rect, p: Point, radius: number): Handle | null {
  for (const h of HANDLES) {
    const hp = handlePoint(r, h);
    if (Math.abs(hp.x - p.x) <= radius && Math.abs(hp.y - p.y) <= radius) return h;
  }
  // 边线本身也能拖（比控制点好点中）
  const nearX = (v: number) => Math.abs(p.x - v) <= radius / 2;
  const nearY = (v: number) => Math.abs(p.y - v) <= radius / 2;
  const inY = p.y > r.y && p.y < bottom(r);
  const inX = p.x > r.x && p.x < right(r);
  if (inY && nearX(r.x)) return 'w';
  if (inY && nearX(right(r))) return 'e';
  if (inX && nearY(r.y)) return 'n';
  if (inX && nearY(bottom(r))) return 's';
  return null;
}

/** 吸附：某个坐标距离任一候选线 ≤ threshold 时吸过去。 */
export function snapValue(v: number, lines: number[], threshold: number): number {
  let best = v;
  let bestDist = threshold + 1;
  for (const l of lines) {
    const d = Math.abs(l - v);
    if (d <= threshold && d < bestDist) {
      best = l;
      bestDist = d;
    }
  }
  return best;
}

export interface SnapLines {
  xs: number[];
  ys: number[];
}

/** 收集吸附线：各窗口的四条边 + 屏幕四条边（规格 02 §3.5.10）。 */
export function snapLines(rects: Rect[], screen: Rect): SnapLines {
  const xs = new Set<number>([screen.x, right(screen)]);
  const ys = new Set<number>([screen.y, bottom(screen)]);
  for (const r of rects) {
    xs.add(r.x);
    xs.add(right(r));
    ys.add(r.y);
    ys.add(bottom(r));
  }
  return { xs: [...xs], ys: [...ys] };
}

/** 给"正在拖的那几条边"做吸附。左/上边吸到线上；右/下边让 right/bottom 落到线上。 */
export function snapEdges(r: Rect, edges: { l?: boolean; t?: boolean; r?: boolean; b?: boolean }, lines: SnapLines, threshold: number): Rect {
  let x1 = r.x;
  let y1 = r.y;
  let x2 = right(r);
  let y2 = bottom(r);
  if (edges.l) x1 = snapValue(x1, lines.xs, threshold);
  if (edges.r) x2 = snapValue(x2, lines.xs, threshold);
  if (edges.t) y1 = snapValue(y1, lines.ys, threshold);
  if (edges.b) y2 = snapValue(y2, lines.ys, threshold);
  return { x: x1, y: y1, width: Math.max(1, x2 - x1), height: Math.max(1, y2 - y1) };
}

/** 整体移动时的吸附：四条边里离线最近的那条吸过去，尺寸不变。 */
export function snapMove(r: Rect, lines: SnapLines, threshold: number): Rect {
  const dx = bestShift([r.x, right(r)], lines.xs, threshold);
  const dy = bestShift([r.y, bottom(r)], lines.ys, threshold);
  return { ...r, x: r.x + dx, y: r.y + dy };
}

function bestShift(values: number[], lines: number[], threshold: number): number {
  let shift = 0;
  let best = threshold + 1;
  for (const v of values) {
    for (const l of lines) {
      const d = Math.abs(l - v);
      if (d <= threshold && d < best) {
        best = d;
        shift = l - v;
      }
    }
  }
  return shift;
}

// ───────────────────────── 浮层定位（CSS 像素） ─────────────────────────

export interface Size {
  width: number;
  height: number;
}

/**
 * 工具条位置（规格 02 §3.6.1–3.6.4）：
 * 选区正下方 8px、右对齐；下方放不下 → 上方 8px；上下都放不下 → 选区内部右下角；
 * 右对齐导致左边出屏 → 改为左对齐选区左边缘；最后整体夹回屏幕内。
 */
export function placeToolbar(sel: Rect, bar: Size, screen: Size, gap = 8): { x: number; y: number; inside: boolean } {
  let inside = false;
  let y: number;
  if (bottom(sel) + gap + bar.height <= screen.height) y = bottom(sel) + gap;
  else if (sel.y - gap - bar.height >= 0) y = sel.y - gap - bar.height;
  else {
    y = bottom(sel) - gap - bar.height;
    inside = true;
  }
  let x = right(sel) - bar.width;
  if (inside) x = right(sel) - gap - bar.width;
  if (x < 0) x = sel.x;
  x = Math.max(0, Math.min(x, screen.width - bar.width));
  y = Math.max(0, Math.min(y, screen.height - bar.height));
  return { x, y, inside };
}

/** 尺寸标签：选区上方外侧 6px、左对齐；上方不足 30px 时移到选区内部左上角。 */
export function placeSizeHint(sel: Rect, hint: Size, screen: Size): { x: number; y: number } {
  const y = sel.y >= 30 ? sel.y - 6 - hint.height : sel.y + 6;
  const x = Math.max(0, Math.min(sel.x + (sel.y >= 30 ? 0 : 6), screen.width - hint.width));
  return { x, y: Math.max(0, y) };
}

/** 放大镜：默认在鼠标右下方 (16,16)；靠右翻到左侧，靠下翻到上方。 */
export function placeMagnifier(cursor: Point, box: Size, screen: Size, offset = 16): { x: number; y: number } {
  let x = cursor.x + offset;
  let y = cursor.y + offset;
  if (x + box.width > screen.width) x = cursor.x - offset - box.width;
  if (y + box.height > screen.height) y = cursor.y - offset - box.height;
  return { x: Math.max(0, x), y: Math.max(0, y) };
}

/** 箭头 / 直线按住 Shift 时约束到 15° 的整数倍。 */
export function constrainAngle(from: Point, to: Point, stepDeg = 15): Point {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const len = Math.hypot(dx, dy);
  const step = (stepDeg * Math.PI) / 180;
  const angle = Math.round(Math.atan2(dy, dx) / step) * step;
  return { x: from.x + Math.cos(angle) * len, y: from.y + Math.sin(angle) * len };
}

/** 矩形/椭圆按住 Shift 约束为正方形/正圆。 */
export function constrainSquare(from: Point, to: Point): Point {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const s = Math.max(Math.abs(dx), Math.abs(dy));
  return { x: from.x + Math.sign(dx || 1) * s, y: from.y + Math.sign(dy || 1) * s };
}
