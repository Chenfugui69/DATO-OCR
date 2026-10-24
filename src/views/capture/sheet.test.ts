import { describe, expect, it } from 'vitest';

import { cellsIn, move, ownerMap, parseTsv, paste, rangeGrid, rangeOf, rangeRows, tabMove, type SheetCell } from './sheet';

// | 地区(跨两行) | 2025(跨两列)      |
// |              | 上半年 | 下半年   |
// | 华东         | 1      | 2        |
const cells: SheetCell[] = [
  { row: 0, col: 0, rowSpan: 2, colSpan: 1 },
  { row: 0, col: 1, rowSpan: 1, colSpan: 2 },
  { row: 1, col: 1, rowSpan: 1, colSpan: 1 },
  { row: 1, col: 2, rowSpan: 1, colSpan: 1 },
  { row: 2, col: 0, rowSpan: 1, colSpan: 1 },
  { row: 2, col: 1, rowSpan: 1, colSpan: 1 },
  { row: 2, col: 2, rowSpan: 1, colSpan: 1 },
];
const texts = ['地区', '2025', '上半年', '下半年', '华东', '1', '2'];
const owner = ownerMap(cells, 3, 3);

describe('表格选区', () => {
  it('位置归哪一格', () => {
    expect(owner).toEqual([
      [0, 1, 1],
      [0, 2, 3],
      [4, 5, 6],
    ]);
  });

  it('框到合并格子的一部分就把整格框进来', () => {
    // 从"上半年"拖到"华东"：碰到"地区"（跨两行）→ 从第 0 行起；第 0 行又碰到"2025"（跨两列）→ 到第 2 列
    expect(rangeOf({ r: 1, c: 1 }, { r: 2, c: 0 }, cells, owner)).toEqual({ r0: 0, c0: 0, r1: 2, c1: 2 });
    expect(rangeOf({ r: 2, c: 0 }, { r: 2, c: 1 }, cells, owner)).toEqual({ r0: 2, c0: 0, r1: 2, c1: 1 });
    // 碰到"2025"（跨两列）→ 一直到第 2 列
    expect(rangeOf({ r: 0, c: 1 }, { r: 1, c: 1 }, cells, owner)).toEqual({ r0: 0, c0: 1, r1: 1, c1: 2 });
    expect(cellsIn({ r0: 1, c0: 1, r1: 1, c1: 2 }, cells)).toEqual([2, 3]);
  });

  it('方向键跨过合并格子，到边上停住', () => {
    expect(move({ r: 0, c: 0 }, 'right', cells, owner)).toEqual({ r: 0, c: 1 });
    expect(move({ r: 0, c: 1 }, 'right', cells, owner)).toEqual({ r: 0, c: 1 });
    expect(move({ r: 0, c: 0 }, 'down', cells, owner)).toEqual({ r: 2, c: 0 });
    expect(move({ r: 2, c: 2 }, 'down', cells, owner)).toEqual({ r: 2, c: 2 });
    expect(tabMove({ r: 1, c: 2 }, false, cells, owner)).toEqual({ r: 2, c: 0 });
    expect(tabMove({ r: 2, c: 0 }, true, cells, owner)).toEqual({ r: 1, c: 2 });
  });
});

describe('复制粘贴', () => {
  it('选区复制成规整的格子和带合并的行', () => {
    const rg = { r0: 0, c0: 0, r1: 1, c1: 2 };
    expect(rangeGrid(rg, cells, owner, texts)).toEqual([
      ['地区', '2025', ''],
      ['', '上半年', '下半年'],
    ]);
    const rows = rangeRows(rg, cells, texts);
    expect(rows[0]!.map((c) => [c.text, c.rowSpan, c.colSpan])).toEqual([
      ['地区', 2, 1],
      ['2025', 1, 2],
    ]);
  });

  it('要样子时带上格子的字色、底色、粗体', () => {
    const styledCells = cells.map((c, i) => ({ ...c, color: '#111111', background: i === 0 ? '#eeeeee' : '#ffffff', bold: i === 0 }));
    const rows = rangeRows({ r0: 0, c0: 0, r1: 0, c1: 0 }, styledCells, texts, true);
    expect(rows[0]![0]!.style).toMatchObject({ color: '#111111', background: '#eeeeee', bold: true });
    expect(rangeRows({ r0: 0, c0: 0, r1: 0, c1: 0 }, styledCells, texts)[0]![0]!.style).toBeUndefined();
  });

  it('读 Excel 复制出来的文字（引号里的换行、制表符）', () => {
    expect(parseTsv('a\tb\r\nc\t"d\te ""x"""\n')).toEqual([
      ['a', 'b'],
      ['c', 'd\te "x"'],
    ]);
  });

  it('从一格开始贴一块；一个值贴到多格的选区里每格都填', () => {
    const out = paste({ r: 2, c: 1 }, [['7', '8', '9']], cells, owner, texts);
    expect(out.slice(4)).toEqual(['华东', '7', '8']);
    const filled = paste({ r: 1, c: 1 }, [['0']], cells, owner, texts, { r0: 1, c0: 1, r1: 2, c1: 2 });
    expect(filled).toEqual(['地区', '2025', '0', '0', '华东', '0', '0']);
  });
});
