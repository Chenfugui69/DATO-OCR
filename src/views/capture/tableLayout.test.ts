import { describe, expect, it } from 'vitest';

import type { CaptureTable, LaidCell } from '@/lib/types';
import type { Cell } from '@/views/ocr/structured';

import { layoutTable, mergeTable } from './tableLayout';

const c = (text: string, header = false, rowSpan = 1, colSpan = 1): Cell => ({ text, header, rowSpan, colSpan });

/** 原位认出的 2×2 表：每格 100×30，从 (10, 20) 开始 */
function laid(): CaptureTable {
  const cell = (row: number, col: number, text: string): LaidCell => ({
    row,
    col,
    rowSpan: 1,
    colSpan: 1,
    header: row === 0,
    text,
    x: 10 + col * 100,
    y: 20 + row * 30,
    width: 100,
    height: 30,
    textBox: [14 + col * 100, 28 + row * 30, 40, 14],
    byInk: true,
    lines: 1,
    fontPx: 18,
    centerShift: 0.3,
    color: '#222222',
    background: row === 0 ? '#eeeeee' : '#ffffff',
    bold: row === 0,
    align: 'left',
  });
  return {
    table: { width: 300, height: 120, rows: 2, cols: 2, cells: [cell(0, 0, 'cty'), cell(0, 1, 'pop'), cell(1, 0, 'Bejing'), cell(1, 1, '21')] },
    background: 'bg',
    source: 'src',
  };
}

const sel = { x: 500, y: 400, width: 300, height: 120 };

describe('AI 认出的表格摆回选区', () => {
  it('行列对得上：只换字，位置、样式不动', () => {
    const merged = mergeTable(laid(), [[c('City', true), c('Population', true)], [c('Beijing'), c('21')]]);
    expect(merged?.table.cells.map((x) => x.text)).toEqual(['City', 'Population', 'Beijing', '21']);
    expect(merged?.table.cells[2]).toMatchObject({ x: 10, y: 50, fontPx: 18, background: '#ffffff' });
    expect(merged?.plain).toBeUndefined();
    // 换过字的格子记着原来那段（墨迹框量的是它），没换的不记
    expect(merged?.table.cells[0]).toMatchObject({ text: 'City', inkText: 'cty' });
    expect(merged?.table.cells[3]?.inkText).toBeUndefined();
  });

  it('行列对不上：不硬套', () => {
    expect(mergeTable(laid(), [[c('a'), c('b'), c('c')]])).toBeNull();
  });

  it('原位合并了的格子 AI 认成两格：不硬套（会丢字）', () => {
    const t = laid();
    t.table.cells = [{ ...t.table.cells[0]!, colSpan: 2 }, t.table.cells[2]!, t.table.cells[3]!];
    expect(mergeTable(t, [[c('a'), c('b')], [c('c'), c('d')]])).toBeNull();
    expect(mergeTable(t, [[c('ab', true, 1, 2)], [c('c'), c('d')]])?.table.cells.map((x) => x.text)).toEqual(['ab', 'c', 'd']);
  });

  it('按原来的范围重画：铺满原表、合并单元格、颜色取原表的', () => {
    const t = layoutTable(
      [
        [c('姓名', true), c('成绩', true, 1, 2)],
        [c('张三'), c('90'), c('95')],
        [c('李四'), c('88'), c('91')],
      ],
      sel,
      undefined,
      laid(),
    )!;
    expect(t.plain).toMatchObject({ x: 10, y: 20, width: 200, height: 60 });
    expect(t.table).toMatchObject({ rows: 3, cols: 3 });
    const score = t.table.cells.find((x) => x.text === '成绩')!;
    expect(score).toMatchObject({ row: 0, col: 1, colSpan: 2, header: true, bold: true, align: 'center', background: '#eeeeee' });
    // 跨两列的格子正好到表的右边
    expect(score.x + score.width).toBeCloseTo(210);
    // 数字靠右，字色、表身底色用原表里最多的那种
    expect(t.table.cells.find((x) => x.text === '95')).toMatchObject({ align: 'right', color: '#222222', background: '#ffffff', row: 1, col: 2 });
    // 行高平均分
    expect(t.table.cells.find((x) => x.text === '李四')!.y).toBeCloseTo(60);
  });

  it('本地没认出表：铺满整个选区，颜色按选区的', () => {
    const t = layoutTable([[c('a'), c('b')]], sel, { background: '#101010', color: '#f0f0f0' })!;
    expect(t.plain).toMatchObject({ x: 0, y: 0, width: 300, height: 120 });
    expect(t.table.cells[0]).toMatchObject({ background: '#101010', color: '#f0f0f0', textBox: null });
    expect(layoutTable([], sel, undefined)).toBeNull();
  });
});
