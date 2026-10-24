// 识别出的表格像 Excel 一样操作：格子的选区、方向键移动、复制粘贴的文本。纯计算，不碰 DOM，方便单测。
//
// 格子有合并单元格（跨行 / 跨列）。位置都用"第几行第几列"（Pos）表示，一个合并格子占好几个位置；
// `owner[r][c]` 是盖住这个位置的那一格的序号。选区和 Excel 一样：框到合并格子的一部分就把整格框进来。

import type { Cell, CellStyle } from '@/views/ocr/structured';

export interface SheetCell {
  row: number;
  col: number;
  rowSpan: number;
  colSpan: number;
}

export interface Pos {
  r: number;
  c: number;
}

export interface Range {
  r0: number;
  c0: number;
  r1: number;
  c1: number;
}

export type Direction = 'up' | 'down' | 'left' | 'right';

/** 每个位置归哪一格（没有格子的位置是 -1） */
export function ownerMap(cells: SheetCell[], rows: number, cols: number): number[][] {
  const owner = Array.from({ length: rows }, () => Array<number>(cols).fill(-1));
  cells.forEach((cell, i) => {
    for (let r = cell.row; r < Math.min(rows, cell.row + cell.rowSpan); r += 1) {
      for (let c = cell.col; c < Math.min(cols, cell.col + cell.colSpan); c += 1) owner[r]![c] = i;
    }
  });
  return owner;
}

/** 两个位置框出来的选区，扩到把碰到的合并格子整个包进来 */
export function rangeOf(a: Pos, b: Pos, cells: SheetCell[], owner: number[][]): Range {
  let rg = { r0: Math.min(a.r, b.r), c0: Math.min(a.c, b.c), r1: Math.max(a.r, b.r), c1: Math.max(a.c, b.c) };
  for (let changed = true; changed; ) {
    changed = false;
    for (let r = rg.r0; r <= rg.r1; r += 1) {
      for (let c = rg.c0; c <= rg.c1; c += 1) {
        const cell = cells[owner[r]?.[c] ?? -1];
        if (!cell) continue;
        const next = {
          r0: Math.min(rg.r0, cell.row),
          c0: Math.min(rg.c0, cell.col),
          r1: Math.max(rg.r1, cell.row + cell.rowSpan - 1),
          c1: Math.max(rg.c1, cell.col + cell.colSpan - 1),
        };
        if (next.r0 !== rg.r0 || next.c0 !== rg.c0 || next.r1 !== rg.r1 || next.c1 !== rg.c1) {
          rg = next;
          changed = true;
        }
      }
    }
  }
  return rg;
}

/** 选区里的格子（序号） */
export function cellsIn(rg: Range, cells: SheetCell[]): number[] {
  const out: number[] = [];
  cells.forEach((cell, i) => {
    const r1 = cell.row + cell.rowSpan - 1;
    const c1 = cell.col + cell.colSpan - 1;
    if (cell.row <= rg.r1 && r1 >= rg.r0 && cell.col <= rg.c1 && c1 >= rg.c0) out.push(i);
  });
  return out;
}

/** 从 `pos` 往某个方向走一格：先走出当前这一格（合并格子整个跨过去），撞到边就停在原地 */
export function move(pos: Pos, dir: Direction, cells: SheetCell[], owner: number[][]): Pos {
  const rows = owner.length;
  const cols = owner[0]?.length ?? 0;
  const here = cells[owner[pos.r]?.[pos.c] ?? -1];
  let { r, c } = pos;
  if (here) {
    if (dir === 'down') r = here.row + here.rowSpan;
    else if (dir === 'up') r = here.row - 1;
    else if (dir === 'right') c = here.col + here.colSpan;
    else c = here.col - 1;
  } else if (dir === 'down') r += 1;
  else if (dir === 'up') r -= 1;
  else if (dir === 'right') c += 1;
  else c -= 1;
  if (r < 0 || c < 0 || r >= rows || c >= cols) return pos;
  return { r, c };
}

/** Tab / Shift+Tab：往右（左）走，到行尾换到下一行（上一行）的头上 */
export function tabMove(pos: Pos, back: boolean, cells: SheetCell[], owner: number[][]): Pos {
  const next = move(pos, back ? 'left' : 'right', cells, owner);
  if (next.r !== pos.r || next.c !== pos.c) return next;
  const cols = owner[0]?.length ?? 0;
  const r = pos.r + (back ? -1 : 1);
  if (r < 0 || r >= owner.length) return pos;
  return { r, c: back ? cols - 1 : 0 };
}

/** 一格的左上角位置 */
export const startOf = (cell: SheetCell): Pos => ({ r: cell.row, c: cell.col });

/** 选区里的字：一行行、每格一个（合并格子只在左上角那格有字）。复制成制表符分隔 / Markdown 用 */
export function rangeGrid(rg: Range, cells: SheetCell[], owner: number[][], texts: string[]): string[][] {
  const grid: string[][] = [];
  for (let r = rg.r0; r <= rg.r1; r += 1) {
    const row: string[] = [];
    for (let c = rg.c0; c <= rg.c1; c += 1) {
      const i = owner[r]?.[c] ?? -1;
      const cell = cells[i];
      // 合并格子的字放在它在选区里露出来的第一个位置
      const first = cell && Math.max(cell.row, rg.r0) === r && Math.max(cell.col, rg.c0) === c;
      row.push(first ? (texts[i] ?? '') : '');
    }
    grid.push(row);
  }
  return grid;
}

/**
 * 选区里的格子，按行排好、合并的行列数裁到选区里（复制成 HTML 表格，粘到 Excel / Word 里合并格子还在）。
 * `styled`：带上格子的样子（字色、底色、粗斜体、对齐、等宽）
 */
export function rangeRows(rg: Range, cells: (SheetCell & { header?: boolean } & CellStyle)[], texts: string[], styled = false): Cell[][] {
  const rows: Cell[][] = [];
  cells.forEach((cell, i) => {
    const r0 = Math.max(cell.row, rg.r0);
    const c0 = Math.max(cell.col, rg.c0);
    const r1 = Math.min(cell.row + cell.rowSpan - 1, rg.r1);
    const c1 = Math.min(cell.col + cell.colSpan - 1, rg.c1);
    if (r0 > r1 || c0 > c1) return;
    const style: CellStyle | undefined = styled
      ? { color: cell.color, background: cell.background, bold: cell.bold, italic: cell.italic, align: cell.align, mono: cell.mono }
      : undefined;
    (rows[r0 - rg.r0] ??= []).push({ text: texts[i] ?? '', header: !!cell.header, rowSpan: r1 - r0 + 1, colSpan: c1 - c0 + 1, style });
  });
  // 每行按列排（cells 本来就是按行、列排的，保险起见再排一次）
  return Array.from({ length: rg.r1 - rg.r0 + 1 }, (_, i) => rows[i] ?? []);
}

/**
 * 粘贴进来的文字 → 一行行的格子。和 Excel 复制出来的格式一样：行用换行分，格用制表符分；
 * 一格里有换行、制表符、引号的整格包在引号里，引号写成两个
 */
export function parseTsv(text: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = '';
  let quoted = false;
  const src = text.replace(/\r\n?/g, '\n').replace(/\n$/, '');
  for (let i = 0; i < src.length; i += 1) {
    const ch = src[i]!;
    if (quoted) {
      if (ch === '"' && src[i + 1] === '"') {
        field += '"';
        i += 1;
      } else if (ch === '"') quoted = false;
      else field += ch;
    } else if (ch === '"' && field === '') quoted = true;
    else if (ch === '\t') {
      row.push(field);
      field = '';
    } else if (ch === '\n') {
      row.push(field);
      rows.push(row);
      row = [];
      field = '';
    } else field += ch;
  }
  row.push(field);
  rows.push(row);
  return rows;
}

/**
 * 从 `at` 开始把一块字贴进去（超出表格的部分丢掉；合并格子只认它左上角那个位置）。
 * 只有一个值、选区不止一格时：选区里每格都填它（和 Excel 一样）。返回新的每格文字
 */
export function paste(at: Pos, data: string[][], cells: SheetCell[], owner: number[][], texts: string[], selection?: Range): string[] {
  const out = [...texts];
  const single = data.length === 1 && data[0]!.length === 1;
  if (single && selection && (selection.r1 > selection.r0 || selection.c1 > selection.c0)) {
    for (const i of cellsIn(selection, cells)) out[i] = data[0]![0]!;
    return out;
  }
  data.forEach((line, dr) =>
    line.forEach((value, dc) => {
      const r = at.r + dr;
      const c = at.c + dc;
      const i = owner[r]?.[c] ?? -1;
      const cell = cells[i];
      if (cell && cell.row === r && cell.col === c) out[i] = value;
    }),
  );
  return out;
}
