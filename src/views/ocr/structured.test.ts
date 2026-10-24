import { describe, expect, it } from 'vitest';

import { isLongNumber, numberTokens, tableGrid, toExcelText, toHtmlTable, toMarkdown, toTsv, unmatchedNumbers, type Cell } from './structured';

const c = (text: string, rowSpan = 1, colSpan = 1, header = false): Cell => ({ text, rowSpan, colSpan, header });

describe('表格识别结果', () => {
  it('合并单元格摊开成规整的格子', () => {
    // | 姓名 | 成绩(跨两列) |
    // | 张三 | 90 | 95 |
    const grid = tableGrid([[c('姓名', 1, 1, true), c('成绩', 1, 2, true)], [c('张三'), c('90'), c('95')]]);
    expect(grid).toEqual([
      ['姓名', '成绩', ''],
      ['张三', '90', '95'],
    ]);
  });

  it('跨行的单元格占住下面一行的位置', () => {
    const grid = tableGrid([[c('A', 2), c('B')], [c('C')]]);
    expect(grid).toEqual([
      ['A', 'B'],
      ['', 'C'],
    ]);
  });

  it('转成制表符分隔和 Markdown', () => {
    const grid = [
      ['a', 'b|c'],
      ['1', '2'],
    ];
    expect(toTsv(grid)).toBe('a\tb|c\n1\t2');
    expect(toMarkdown(grid)).toBe('| a | b\\|c |\n| --- | --- |\n| 1 | 2 |');
  });
});

describe('财务表格', () => {
  it('长数字按文本粘贴', () => {
    expect(isLongNumber('6222021234567890')).toBe(true);
    expect(isLongNumber('12345678901')).toBe(false);
    expect(isLongNumber('1,234,567,890,123')).toBe(false);
    expect(toExcelText([['卡号', '6222021234567890'], ['金额', '1,200.00']])).toBe("卡号\t'6222021234567890\n金额\t1,200.00");
  });

  it('HTML 保留合并单元格、表头，长数字标成文本', () => {
    const html = toHtmlTable([[c('项目', 1, 2, true)], [c('A<B'), c('6222021234567890')]]);
    expect(html).toContain('<th colspan="2">项目</th>');
    expect(html).toContain('<td>A&#60;B</td>');
    expect(html).toContain(`<td style="mso-number-format:'\\@'">6222021234567890</td>`);
  });

  it('HTML 带上格子的样子；格子里换行不拆行；奇怪的颜色值不写进去', () => {
    const styled = { ...c('合计\n含税'), style: { color: '#c00000', background: 'rgb(240, 240, 240)', bold: true, italic: true, align: 'right' as const } };
    const evil = { ...c('x'), style: { color: 'red;" onclick="x', background: 'url(x)' } };
    const html = toHtmlTable([[styled, evil]]);
    expect(html).toContain('<td style="color:#c00000;background:rgb(240, 240, 240);font-weight:700;font-style:italic;text-align:right">合计<br style="mso-data-placement:same-cell">含税</td>');
    expect(html).toContain('<td>x</td>');
  });

  it('数字统一写法', () => {
    expect(numberTokens('收入 1,234.50 元，支出 (300)，增长 12.5%，第 3 季')).toEqual(['1234.50', '300', '12.5']);
    expect(numberTokens('1, 234 和 1 234')).toEqual(['1234', '1234']);
  });

  it('AI 结果里本地识字没有的数字标出来', () => {
    const rows = [[c('收入'), c('1,234.50')], [c('支出'), c('(1,298.00)')], [c('备注'), c('第1季')]];
    // 本地识字认的是 1,289.00：AI 把 89 改成了 98
    const flagged = unmatchedNumbers(rows, '收入 1,234.50 支出 (1,289.00) 备注 第1季');
    expect([...flagged]).toEqual(['1,1']);
    expect(unmatchedNumbers(rows, '').size).toBe(0);
  });
});
