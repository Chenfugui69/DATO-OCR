// 识别出的表格每格的字怎么摆（截图原位、贴图里共用）：按这台电脑的界面字体量字的墨迹，摆得和原图里的字一样高、
// 一样宽、落在同一条线上；粗体按笔画选半粗 / 全粗。

import { isMac } from '@/lib/platform';
import type { LaidCell } from '@/lib/types';

const median = (v: number[]) => (v.length ? [...v].sort((a, b) => a - b)[Math.floor(v.length / 2)]! : undefined);

/** 摆字用的字体：和系统界面一样（截图里的表格多半就是系统界面字体写的） */
export const FONT = isMac
  ? "-apple-system, 'Helvetica Neue', Helvetica, Arial, 'PingFang SC', sans-serif"
  : "'Segoe UI', 'Microsoft YaHei UI', 'Microsoft YaHei', sans-serif";

/**
 * 粗体用半粗（600）还是全粗（700）：笔画比这张表一般的字粗一半以上的算全粗（网页表头、Excel 的粗体），
 * 粗一点的是半粗（系统界面的标题、表头多半是这种）。量不到笔画按半粗
 */
function boldWeights(cells: LaidCell[]): (c: LaidCell) => number {
  const normal = median(cells.filter((c) => !c.bold && c.stroke).map((c) => c.stroke!));
  const own = (c: LaidCell) => (normal && c.stroke && c.stroke / normal >= 1.5 ? 700 : 600);
  // 同一行的粗体（一般是表头）是同一种粗细：括号、上标多的格子笔画量出来偏细，按这一行多数的来
  const byRow = new Map<number, number>();
  for (const row of new Set(cells.filter((c) => c.bold).map((c) => c.row))) {
    const ws = cells.filter((c) => c.bold && c.row === row).map(own);
    byRow.set(row, ws.filter((w) => w === 700).length * 2 >= ws.length ? 700 : 600);
  }
  return (c) => (!c.bold ? 400 : (byRow.get(c.row) ?? own(c)));
}

// ───────────────────────── 量字 ─────────────────────────

let measureCtx: CanvasRenderingContext2D | null | undefined;

/** 按 100px 的字号量一段字：墨迹的上下左右（以基线、起点为 0）、字体的上下沿、前进宽度 */
/** 代码、终端里的表（后端量出来是等宽的）用等宽字体 */
export const MONO = isMac ? "ui-monospace, Menlo, 'PingFang SC', monospace" : "'Cascadia Mono', Consolas, 'Microsoft YaHei UI', monospace";

/** 这一格用的字体（CSS 的 font-family）和斜体 */
const familyOf = (c: LaidCell) => (c.mono ? MONO : FONT);
const styleOf = (c: LaidCell) => (c.italic ? 'italic' : 'normal');

function measure(text: string, weight: number, family: string = FONT, italic = false) {
  measureCtx ??= document.createElement('canvas').getContext('2d');
  if (!measureCtx) return null;
  measureCtx.font = `${italic ? 'italic ' : ''}${weight} 100px ${family}`;
  const m = measureCtx.measureText(text);
  return {
    left: m.actualBoundingBoxLeft,
    right: m.actualBoundingBoxRight,
    ascent: m.actualBoundingBoxAscent,
    descent: m.actualBoundingBoxDescent,
    fontAscent: m.fontBoundingBoxAscent,
    fontDescent: m.fontBoundingBoxDescent,
    advance: m.width,
  };
}

export interface CellFit {
  /** 字的样式和在格子里的位置（CSS 像素，相对格子左上角） */
  style: React.CSSProperties;
  /** 空格子打字时用的字号 */
  font: number;
}

/**
 * 每格的字怎么摆：字号按墨迹高量（原图里的字有多高，这个字体的同一段字就要多高），横向再拉伸到和原图一样宽，
 * 墨迹的中心对上原图墨迹的中心。同一列（表头和表身分开）量出来差不多的字号统一成一样。
 * 只在刚认出来时算一次：之后改字，字号、拉伸、位置都不变（右对齐的格子从右边往左长）。
 */
export function fitCells(cells: LaidCell[], s: number, plain: boolean): CellFit[] {
  const weightOf = boldWeights(cells);
  const raw = cells.map((c) => {
    const text = c.text.trim();
    // 墨迹框量的是原图里那段字（AI 换过字的格子，`inkText` 是原来那段）：字号、基线按它量，摆的是现在的字
    const inked = (c.inkText ?? c.text).trim();
    if (!c.textBox || !text || !inked || c.lines !== 1) return null;
    const weight = weightOf(c);
    const m = measure(inked, weight, familyOf(c), !!c.italic);
    const inkH = m ? m.ascent + m.descent : 0;
    // 量不到、或者只有标点（墨迹太矮，按它算会把字号放得很大）：用后端按字形估的字号
    const font = c.byInk && m && inkH > 30 ? (100 * c.textBox[3]) / s / inkH : c.fontPx / s;
    return { font, weight, m, shown: text === inked ? m : measure(text, weight, familyOf(c), !!c.italic) };
  });
  // 同一列差不多的字号统一（原表格一列一般是一个字号，量出来会差一两个像素）
  const groups = new Map<string, number[]>();
  cells.forEach((c, i) => {
    const r = raw[i];
    if (r && c.colSpan === 1) groups.set(`${c.col}:${c.header}`, [...(groups.get(`${c.col}:${c.header}`) ?? []), r.font]);
  });
  const fonts = raw.filter(Boolean).map((r) => r!.font);
  const typical = median(fonts) ?? median(cells.map((c) => c.fontPx / s)) ?? 14;

  return cells.map((c, i) => {
    const r = raw[i];
    const tb = c.textBox;
    if (r && tb && r.m) {
      const sizes = groups.get(`${c.col}:${c.header}`) ?? [];
      const m0 = sizes.length >= 2 ? median(sizes)! : r.font;
      // 整张表差不多的（差不到 8%）统一成一样（表格一般通篇一个字号，表头每格各是一列，按列统一不了）；
      // 再按列统一差得多一点的。原图的字号都是整像素：取整，基线也就落得更稳
      const near = (a: number, b: number, k: number) => Math.abs(a - b) / b < k;
      const picked = near(r.font, typical, 0.08) ? typical : near(r.font, m0, 0.15) ? m0 : r.font;
      const font = Math.round(picked * s) / s;
      const k = font / 100;
      const m = r.m;
      const inkW = (m.left + m.right) * k;
      let sx = c.byInk && inkW > 0 ? tb[2] / s / inkW : 1;
      sx = Math.abs(sx - 1) < 0.03 ? 1 : Math.min(1.25, Math.max(0.8, sx));
      const lineH = font * 1.25;
      // 行框里基线在哪：上下各留一半多出来的行高，再加字体的上沿
      const baselineInBox = (lineH - (m.fontAscent + m.fontDescent) * k) / 2 + m.fontAscent * k;
      const inkCenterY = (tb[1] + tb[3] / 2 - c.y) / s;
      const baseline = inkCenterY + ((m.ascent - m.descent) / 2) * k;
      const tbL = (tb[0] - c.x) / s;
      const tbR = tbL + tb[2] / s;
      // 横向按现在的字摆（左对齐的左边、右对齐的右边、居中的中心对上原来的字）
      const h = r.shown ?? m;
      const style: React.CSSProperties = {
        fontFamily: familyOf(c),
        fontStyle: styleOf(c),
        fontWeight: r.weight,
        fontSize: font,
        lineHeight: `${lineH}px`,
        color: c.color,
        whiteSpace: 'pre',
        top: baseline - baselineInBox,
      };
      if (c.align === 'right') {
        style.right = c.width / s - (tbR + (h.advance - h.right) * k * sx);
        style.transformOrigin = 'right center';
      } else if (c.align === 'center') {
        style.left = (tbL + tbR) / 2 - ((h.right - h.left) / 2) * k * sx;
        style.transformOrigin = 'left center';
      } else {
        style.left = tbL + h.left * k * sx;
        style.transformOrigin = 'left center';
      }
      if (sx !== 1) style.transform = `scaleX(${sx})`;
      return { style, font };
    }
    if (tb && c.lines > 1) {
      // 一格里好几行字：字号按后端量的，每行平分墨迹的高
      const font = c.fontPx / s;
      const lineH = tb[3] / c.lines / s;
      return {
        font,
        style: {
          fontFamily: familyOf(c),
        fontStyle: styleOf(c),
          fontWeight: weightOf(c),
          fontSize: font,
          lineHeight: `${lineH}px`,
          color: c.color,
          whiteSpace: 'pre-wrap',
          textAlign: c.align,
          left: (tb[0] - c.x) / s,
          width: tb[2] / s,
          top: (tb[1] - c.y) / s,
        },
      };
    }
    // 空格子（或 AI 画的表）：一般的字号，竖着居中，左右留点边
    const font = plain ? c.fontPx / s : typical;
    const lineH = font * 1.3;
    return {
      font,
      style: {
        fontFamily: familyOf(c),
        fontStyle: styleOf(c),
        fontWeight: weightOf(c),
        fontSize: font,
        lineHeight: `${lineH}px`,
        color: c.color,
        whiteSpace: 'pre',
        textAlign: c.align,
        left: 6,
        right: 6,
        top: (c.height / s - lineH) / 2,
        overflow: 'hidden',
      },
    };
  });
}
