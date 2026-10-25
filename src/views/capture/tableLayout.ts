// AI 认出的表格怎么摆回截图选区里（纯计算，不碰 DOM，方便单测）。坐标都是选区图的物理像素。

import type { CaptureTable, LaidCell, RegionRect } from '@/lib/types';
import { tableGrid, type Cell } from '@/views/ocr/structured';


const median = (v: number[]) => (v.length ? [...v].sort((a, b) => a - b)[Math.floor(v.length / 2)]! : undefined);

/**
 * AI 重新认的表格套回原位的格子：行列对得上就只换字，样式、位置还是原来量的；对不上返回 null
 * （那就用 `layoutTable` 按原来的范围画一张）
 */
export function mergeTable(laid: CaptureTable, rows: Cell[][]): CaptureTable | null {
  if (laid.plain) return null;
  const grid = tableGrid(rows);
  const { rows: n, cols } = laid.table;
  if (grid.length !== n || grid.some((r) => r.length !== cols)) return null;
  // 原位的格子合并过的地方，AI 那边也得是空的（AI 把它认成了两格的话，套回来会丢字）
  const starts = new Set(laid.table.cells.map((c) => `${c.row},${c.col}`));
  if (grid.some((r, i) => r.some((text, j) => text.trim() && !starts.has(`${i},${j}`)))) return null;
  const cells = laid.table.cells.map((c) => {
    const text = grid[c.row]?.[c.col] ?? c.text;
    // 记下原图里那段字：墨迹框是按它量的，页面按它定字号和基线
    return text === c.text ? c : { ...c, text, inkText: c.inkText ?? c.text };
  });
  return { ...laid, table: { ...laid.table, cells } };
}

/** 一种颜色出现得最多的那个（格子的底色、字色） */
function mostCommon(values: string[]): string | undefined {
  const count = new Map<string, number>();
  for (const v of values) count.set(v, (count.get(v) ?? 0) + 1);
  return [...count.entries()].sort((a, b) => b[1] - a[1])[0]?.[0];
}

/** 估一段字有多宽（半角字算 1，中日韩的字算 2）：分列宽用 */
const textUnits = (text: string) => [...text].reduce((n, ch) => n + (/[⺀-鿿가-힯＀-￯]/.test(ch) ? 2 : 1), 0);
const NUMERIC = /^[\s(（]*[-+−]?[$¥€£]?\s*[\d.,，]+%?\s*[)）]?$/;

/**
 * 认出的表格画到原位：铺满原来那张表的范围（没有就整块 `area`，选区图里的物理像素），按每列字的多少分列宽、
 * 行高平均，底色、字色用原图的（`colors`，有原位的格子时取格子里最多的那种）。没有格子返回 null。
 * 坐标都是 `area` 里的（有原位的表时用那张表的范围）
 */
export function layoutTable(rows: Cell[][], area: RegionRect, colors: { background: string; color: string } | undefined, base?: CaptureTable): CaptureTable | null {
  // 摊开合并单元格，算出每格从哪行哪列开始
  const taken: boolean[][] = [];
  const placed: { cell: Cell; row: number; col: number }[] = [];
  rows.forEach((row, r) => {
    taken[r] ??= [];
    let c = 0;
    for (const cell of row) {
      while (taken[r]![c]) c += 1;
      for (let dr = 0; dr < cell.rowSpan; dr += 1) for (let dc = 0; dc < cell.colSpan; dc += 1) (taken[r + dr] ??= [])[c + dc] = true;
      placed.push({ cell, row: r, col: c });
      c += cell.colSpan;
    }
  });
  if (!placed.length) return null;
  const nRows = taken.length;
  const nCols = Math.max(1, ...taken.map((r) => r.length));

  const laid = base && !base.plain ? base.table.cells : [];
  const bounds = laid.length
    ? (() => {
        const x0 = Math.min(...laid.map((c) => c.x));
        const y0 = Math.min(...laid.map((c) => c.y));
        return { x: x0, y: y0, width: Math.max(...laid.map((c) => c.x + c.width)) - x0, height: Math.max(...laid.map((c) => c.y + c.height)) - y0 };
      })()
    : { x: 0, y: 0, width: (base?.rect ?? area).width, height: (base?.rect ?? area).height };
  // 表头、表身的底色分开取（表头常常是灰底）
  const background = mostCommon(laid.filter((c) => !c.header).map((c) => c.background)) ?? colors?.background ?? '#ffffff';
  const headerBackground = mostCommon(laid.filter((c) => c.header).map((c) => c.background)) ?? background;
  const color = mostCommon(laid.filter((c) => c.textBox).map((c) => c.color)) ?? colors?.color ?? '#1f2328';

  // 列宽：每列最宽的那格（不跨列的）按字数分，至少给 3 个半角字
  const units = Array.from({ length: nCols }, () => 3);
  for (const p of placed) if (p.cell.colSpan === 1) units[p.col] = Math.max(units[p.col]!, textUnits(p.cell.text) + 2);
  const total = units.reduce((a, b) => a + b, 0);
  const colX = [bounds.x];
  units.forEach((u) => colX.push(colX[colX.length - 1]! + (bounds.width * u) / total));
  const rowH = bounds.height / nRows;
  // 字号：原位格子一般的字号；没有就按行高，再按最挤的那列缩到放得下（半角字约 0.55 个字号宽）
  let fontPx = median(laid.filter((c) => c.textBox).map((c) => c.fontPx)) ?? rowH * 0.5;
  units.forEach((u, i) => {
    const w = colX[i + 1]! - colX[i]!;
    fontPx = Math.min(fontPx, (w * 0.9) / Math.max(1, (u - 2) * 0.55));
  });
  fontPx = Math.max(8, Math.min(fontPx, rowH * 0.7));
  const headerRow = rows[0]!.length > 0 && rows[0]!.every((c) => c.header);

  const cells: LaidCell[] = placed.map(({ cell, row, col }) => {
    const header = cell.header || (row === 0 && headerRow);
    return {
      row,
      col,
      rowSpan: cell.rowSpan,
      colSpan: cell.colSpan,
      header,
      text: cell.text,
      x: colX[col]!,
      y: bounds.y + row * rowH,
      width: colX[Math.min(nCols, col + cell.colSpan)]! - colX[col]!,
      height: rowH * cell.rowSpan,
      textBox: null,
      byInk: false,
      lines: 1,
      fontPx,
      centerShift: 0,
      color,
      background: header ? headerBackground : background,
      bold: header,
      align: cell.text.trim() && NUMERIC.test(cell.text) ? 'right' : header ? 'center' : 'left',
    };
  });
  const rect = base?.rect ?? area;
  return {
    table: { width: rect.width, height: rect.height, rows: nRows, cols: nCols, cells },
    background: '',
    rect,
    plain: { ...bounds, line: `color-mix(in srgb, ${color} 28%, ${background})` },
  };
}
