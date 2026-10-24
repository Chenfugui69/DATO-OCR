// 截图框选界面元素（macOS 辅助功能）：遮罩这边的缓存和挑选。
//
// 后端给两种结果：鼠标正下方由里到外的一条链（elementsAt），和附近的元素（elementsNear：同一行的其他格、
// 其他行……）。两种都存进同一个缓存，挑预选框时在本地找"包含鼠标的、由小到大"的那些框，所以鼠标在
// 单元格之间挪动时马上就有新格子可框，不会先跳到整张表再缩回来。

import type { Point, Rect } from './geometry';

export interface UiElement {
  rect: Rect;
  role: string;
}

/**
 * 不拿来当预选框的：文字片段（框住一段字没用，要框的是它外面那一格 / 那个按钮）、表格的整列（竖着一长条，
 * 很少有人要；而且鼠标挪到还没取回来的那一行时它会先冒出来闪一下）、滚动条、认不出是什么的
 */
const SKIP_ROLES = new Set(['AXStaticText', 'AXInlineTextBox', 'AXTextRun', 'AXColumn', 'AXScrollBar', 'AXValueIndicator', 'AXUnknown']);

export type ElementLevel = 'block' | 'element';

/** 两档的最小尺寸（逻辑像素）：元素档连单元格、小按钮都框；区块档只框表格、面板、工具栏这种大块 */
export function thresholds(level: ElementLevel, scale: number) {
  const s = Math.max(1, scale);
  return level === 'block' ? { minSide: 40 * s, minArea: 12000 * s * s } : { minSide: 10 * s, minArea: 100 * s * s };
}

const area = (r: Rect) => r.width * r.height;
const contains = (r: Rect, p: Point) => p.x >= r.x && p.y >= r.y && p.x < r.x + r.width && p.y < r.y + r.height;
export const rectKey = (r: Rect) => `${r.x},${r.y},${r.width},${r.height}`;

/** 包含 `p` 的候选框，由小到大、去重；比 `maxArea`（窗口）小的才算 */
export function candidatesAt(items: UiElement[], p: Point, opts: { minSide: number; minArea: number; maxArea: number }): Rect[] {
  const seen = new Set<string>();
  const out: Rect[] = [];
  for (const e of items) {
    const r = e.rect;
    if (SKIP_ROLES.has(e.role) || !contains(r, p)) continue;
    if (r.width < opts.minSide || r.height < opts.minSide || area(r) < opts.minArea || area(r) >= opts.maxArea) continue;
    const k = rectKey(r);
    if (seen.has(k)) continue;
    seen.add(k);
    out.push(r);
  }
  return out.sort((a, b) => area(a) - area(b));
}

/** 每个窗口一份缓存（这次截图期间） */
export class ElementCache {
  private byWindow = new Map<number, { items: UiElement[]; keys: Set<string> }>();
  /** 已经取过附近元素的锚点（链上父元素的框），别重复取 */
  private near = new Set<string>();

  clear() {
    this.byWindow.clear();
    this.near.clear();
  }

  items(handle: number): UiElement[] {
    return this.byWindow.get(handle)?.items ?? [];
  }

  add(handle: number, list: UiElement[]) {
    let entry = this.byWindow.get(handle);
    if (!entry) {
      entry = { items: [], keys: new Set() };
      this.byWindow.set(handle, entry);
    }
    for (const e of list) {
      const k = `${e.role}|${rectKey(e.rect)}`;
      if (entry.keys.has(k)) continue;
      entry.keys.add(k);
      entry.items.push(e);
    }
    // 别无限长：太多了就丢掉最早的
    if (entry.items.length > 6000) {
      const drop = entry.items.splice(0, entry.items.length - 6000);
      for (const e of drop) entry.keys.delete(`${e.role}|${rectKey(e.rect)}`);
    }
  }

  /** 这个锚点附近的元素取过没有；没取过就记上，返回 true（该去取了） */
  claimNear(handle: number, anchor: Rect): boolean {
    const k = `${handle}|${rectKey(anchor)}`;
    if (this.near.has(k)) return false;
    this.near.add(k);
    return true;
  }
}
