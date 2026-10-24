// 表格识别结果的纯数据处理（不碰 DOM，方便单测）：摊开合并单元格、转成制表符分隔 / Markdown。

export interface Cell {
  text: string;
  header: boolean;
  rowSpan: number;
  colSpan: number;
  /** 原位表格复制出去时带上的样子（字色、底色、粗斜体、对齐），粘到 Excel / Word 里还是这样 */
  style?: CellStyle;
}

export interface CellStyle {
  color?: string;
  background?: string;
  bold?: boolean;
  italic?: boolean;
  align?: 'left' | 'center' | 'right';
  mono?: boolean;
}

/** 把合并单元格摊开成规整的格子：合并的格子只在左上角那格写字，其余留空 */
export function tableGrid(rows: Cell[][]): string[][] {
  const grid: string[][] = [];
  rows.forEach((row, r) => {
    grid[r] ??= [];
    let c = 0;
    for (const cell of row) {
      while (grid[r]![c] !== undefined) c += 1;
      for (let dr = 0; dr < cell.rowSpan; dr += 1) {
        grid[r + dr] ??= [];
        for (let dc = 0; dc < cell.colSpan; dc += 1) grid[r + dr]![c + dc] = dr === 0 && dc === 0 ? cell.text : '';
      }
      c += cell.colSpan;
    }
  });
  const width = Math.max(0, ...grid.map((r) => r.length));
  return grid.map((r) => Array.from({ length: width }, (_, i) => r[i] ?? ''));
}

export const toTsv = (grid: string[][]) => grid.map((r) => r.map((c) => c.replace(/[\t\n]/g, ' ')).join('\t')).join('\n');

export function toMarkdown(grid: string[][]): string {
  if (!grid.length) return '';
  const esc = (s: string) => s.replace(/\|/g, '\\|');
  const [head, ...body] = grid;
  return [`| ${head!.map(esc).join(' | ')} |`, `| ${head!.map(() => '---').join(' | ')} |`, ...body.map((r) => `| ${r.map(esc).join(' | ')} |`)].join('\n');
}

/** 12 位及以上的纯数字（卡号、证件号、长订单号）：Excel 会变成科学计数法、超过 15 位还丢精度，得按文本粘贴 */
export function isLongNumber(text: string): boolean {
  const t = text.trim();
  return /^[+-]?\d+(\.\d+)?$/.test(t) && (t.match(/\d/g)?.length ?? 0) >= 12;
}

/** 粘贴到 Excel 用的纯文本：制表符分隔，长数字前面加 '（Excel 当文本处理，' 本身不显示） */
export function toExcelText(grid: string[][]): string {
  return toTsv(grid.map((r) => r.map((c) => (isLongNumber(c) ? `'${c}` : c))));
}

const escapeHtml = (s: string) => s.replace(/[&<>"]/g, (ch) => `&#${ch.charCodeAt(0)};`);

/** 格子样子 → 行内样式。颜色只认 #rrggbb / rgb()，别的不写，免得把引号之类带进 style 里 */
function styleCss(st: CellStyle | undefined): string[] {
  if (!st) return [];
  const color = (v?: string) => (v && /^(#[0-9a-f]{3,8}|rgba?\([\d.,\s%]+\))$/i.test(v.trim()) ? v.trim() : null);
  const out: string[] = [];
  const fg = color(st.color);
  const bg = color(st.background);
  if (fg) out.push(`color:${fg}`);
  if (bg) out.push(`background:${bg}`);
  if (st.bold) out.push('font-weight:700');
  if (st.italic) out.push('font-style:italic');
  if (st.align) out.push(`text-align:${st.align}`);
  if (st.mono) out.push('font-family:Consolas,Menlo,monospace');
  return out;
}

/** 粘贴到 Excel / Word 用的 HTML 表格：保留合并单元格和表头；长数字标成文本格式（Excel 认 mso-number-format） */
export function toHtmlTable(rows: Cell[][]): string {
  const body = rows
    .map((row) => {
      const cells = row
        .map((c) => {
          const tag = c.header ? 'th' : 'td';
          const span = `${c.rowSpan > 1 ? ` rowspan="${c.rowSpan}"` : ''}${c.colSpan > 1 ? ` colspan="${c.colSpan}"` : ''}`;
          const css = [...styleCss(c.style), ...(isLongNumber(c.text) ? ["mso-number-format:'\\@'"] : [])];
          const style = css.length ? ` style="${css.join(';')}"` : '';
          // 格子里换行：Excel 见到普通的 <br> 会拆成好几行格子，要标上 same-cell
          return `<${tag}${span}${style}>${escapeHtml(c.text).replace(/\n/g, '<br style="mso-data-placement:same-cell">')}</${tag}>`;
        })
        .join('');
      return `<tr>${cells}</tr>`;
    })
    .join('');
  return `<table border="1" style="border-collapse:collapse"><tbody>${body}</tbody></table>`;
}

/** 一段文字里的数字，统一成只剩数字和小数点（去掉千分位、括号、正负号、百分号），至少两位数字的才算 */
export function numberTokens(text: string): string[] {
  // 识字有时把 1,234 认成 "1, 234"、"1 234"：千分位中间的空格先并上
  const joined = text.replace(/(\d)[,，]?\s+(?=\d{3}(?!\d))/g, '$1');
  const out: string[] = [];
  for (const m of joined.matchAll(/\d[\d,，]*(?:\.\d+)?/g)) {
    const n = m[0].replace(/[,，]/g, '').replace(/\.$/, '');
    if ((n.match(/\d/g)?.length ?? 0) >= 2) out.push(n);
  }
  return out;
}

/**
 * 数字对照：AI 认出来的表格里，哪些格子的数字在本地识字（不经过 AI）的结果里找不到。
 * AI 偶尔会改错、编造数字，财务表格错一位就是错；对不上的标出来让人核对。返回格子的 `行,列序号`
 */
export function unmatchedNumbers(rows: Cell[][], ocrText: string): Set<string> {
  const known = new Set(numberTokens(ocrText));
  const flagged = new Set<string>();
  if (known.size === 0) return flagged;
  rows.forEach((row, r) =>
    row.forEach((c, i) => {
      if (numberTokens(c.text).some((n) => !known.has(n))) flagged.add(`${r},${i}`);
    }),
  );
  return flagged;
}
