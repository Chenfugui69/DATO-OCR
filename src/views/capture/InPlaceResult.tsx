// 截图界面里的「识别表格 / 公式」：认出来的结果直接盖在选区原来的位置上（和截图翻译一样），不另外弹卡片。
//
// - 表格：底图是选区原图把字抹掉（线、底色、边框都是原来的），每格的字按原来的位置、字号、颜色、粗细、
//   对齐摆上去，看着和原来几乎一样；点一下格子就能改，Tab 跳到下一格，复制 / 导出用改过的。
//   字号、位置按这台电脑上的界面字体量出来（canvas 量字的墨迹），和原图里的字一样高、一样宽、落在同一条线上
// - AI 认出的表格行列和原位的格子对不上（或者本地压根没认出表格）：按原来的范围自己画一张表，底色、线色、
//   字色从原图取，照样原位、照样能改
// - 公式：用选区的底色铺满，按选区大小摆上公式。点公式（或条上的笔）在选区边上出一条 LaTeX 输入框，
//   改了公式跟着变（LaTeX 在本地转成 MathML）
//
// 操作都在选区上方的一条小条上（和翻译的那条一样）：结果 / 原图切换、复制、导出、AI 重新识别、去掉。
// 复制完就结束截图（和二维码一样），导出前先结束截图，保存对话框才不会被遮罩挡住。

import clsx from 'clsx';
import { Copy, Eye, FileSpreadsheet, FileText, PenLine, Sigma, Sparkles, Table2, X } from 'lucide-react';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import temml from 'temml';

import { useElementSize } from '@/lib/hooks';
import { capture, ocr, system } from '@/lib/ipc';
import { isMac, shotUrl } from '@/lib/platform';
import type { CaptureTable, LaidCell, RegionStyle } from '@/lib/types';
import { Spinner } from '@/ui/controls';
import { notify, Tooltip } from '@/ui/overlays';
import { parseTable, sanitizeMathml, splitFormula } from '@/views/ocr/StructuredResult';
import { tableGrid, toExcelText, toHtmlTable, toMarkdown, type Cell } from '@/views/ocr/structured';

import type { Rect } from './geometry';
import { layoutTable, mergeTable } from './tableLayout';

type Viewport = { width: number; height: number };
type CssRect = { x: number; y: number; width: number; height: number };

const css = (r: Rect, s: number): CssRect => ({ x: r.x / s, y: r.y / s, width: r.width / s, height: r.height / s });
const stop = (e: React.SyntheticEvent) => e.stopPropagation();
const median = (v: number[]) => (v.length ? [...v].sort((a, b) => a - b)[Math.floor(v.length / 2)]! : undefined);

/** 摆字用的字体：和系统界面一样（截图里的表格多半就是系统界面字体写的） */
const FONT = isMac
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

/** 认出来的格子 + 改过的字 → 导出用的一行行单元格 */
function toRows(cells: LaidCell[], texts: string[]): Cell[][] {
  const rows: Cell[][] = [];
  cells.forEach((c, i) => {
    (rows[c.row] ??= []).push({ text: texts[i] ?? c.text, header: c.header, rowSpan: c.rowSpan, colSpan: c.colSpan });
  });
  return rows.filter(Boolean);
}

/** AI 重新认的表格（HTML）套回原位的格子：行列对得上就只换字；对不上返回 null */
export const mergeAiTable = (laid: CaptureTable, html: string) => mergeTable(laid, parseTable(html));

/** AI 认出的表格（HTML）按原来的范围画到原位（见 `layoutTable`） */
export const aiTableLayout = (html: string, selection: Rect, colors: { background: string; color: string } | undefined, base?: CaptureTable) =>
  layoutTable(parseTable(html), selection, colors, base);

// ───────────────────────── 量字 ─────────────────────────

let measureCtx: CanvasRenderingContext2D | null | undefined;

/** 按 100px 的字号量一段字：墨迹的上下左右（以基线、起点为 0）、字体的上下沿、前进宽度 */
function measure(text: string, weight: number) {
  measureCtx ??= document.createElement('canvas').getContext('2d');
  if (!measureCtx) return null;
  measureCtx.font = `${weight} 100px ${FONT}`;
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

interface CellFit {
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
function fitCells(cells: LaidCell[], s: number, plain: boolean): CellFit[] {
  const weightOf = boldWeights(cells);
  const raw = cells.map((c) => {
    const text = c.text.trim();
    // 墨迹框量的是原图里那段字（AI 换过字的格子，`inkText` 是原来那段）：字号、基线按它量，摆的是现在的字
    const inked = (c.inkText ?? c.text).trim();
    if (!c.textBox || !text || !inked || c.lines !== 1) return null;
    const weight = weightOf(c);
    const m = measure(inked, weight);
    const inkH = m ? m.ascent + m.descent : 0;
    // 量不到、或者只有标点（墨迹太矮，按它算会把字号放得很大）：用后端按字形估的字号
    const font = c.byInk && m && inkH > 30 ? (100 * c.textBox[3]) / s / inkH : c.fontPx / s;
    return { font, weight, m, shown: text === inked ? m : measure(text, weight) };
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
        fontFamily: FONT,
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
          fontFamily: FONT,
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
        fontFamily: FONT,
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

// ───────────────────────── 选区上方的小条 ─────────────────────────

function BarButton({ icon: I, label, tip, onClick, on, disabled }: { icon: typeof Copy; label?: string; tip: string; onClick: () => void; on?: boolean; disabled?: boolean }) {
  return (
    <Tooltip content={tip} side="top">
      <button type="button" className={on ? 'is-on' : ''} aria-label={tip} aria-pressed={on} disabled={disabled} onClick={onClick}>
        <I size={14} strokeWidth={1.75} />
        {label}
      </button>
    </Tooltip>
  );
}

/** 浮在选区右上角的条（左上角是尺寸提示）；选区贴着屏幕顶时压在选区里面 */
function StructuredBar({ selection, s, viewport, lift = 0, children }: { selection: Rect; s: number; viewport: Viewport; lift?: number; children: React.ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const size = useElementSize(ref, { width: 320, height: 34 });
  const sel = css(selection, s);
  const x = Math.max(0, Math.min(sel.x + sel.width - size.width, viewport.width - size.width));
  const above = sel.y - size.height - 6 - lift;
  const y = above >= 4 ? above : sel.y + 6;
  return (
    <div
      ref={ref}
      className="cap-trbar cap-trbar--structured cn-glass"
      style={{ transform: `translate(${x}px, ${y}px)` }}
      onPointerDown={stop}
      onDoubleClick={stop}
    >
      {children}
    </div>
  );
}

function useDoneToast(onDone: () => void) {
  const { t } = useTranslation();
  return (key: 'copied' | 'exported') => {
    onDone();
    void system.toast('success', t(`ocr.structured.${key}`)).catch(() => {});
  };
}

// ───────────────────────── 表格 ─────────────────────────

function CellText({ cell: c, text, s, fit, index, onChange }: { cell: LaidCell; text: string; s: number; fit: CellFit; index: number; onChange: (text: string) => void }) {
  const textRef = useRef<HTMLDivElement>(null);
  return (
    <div
      className="cap-inplace__cell"
      style={{ left: c.x / s, top: c.y / s, width: c.width / s, height: c.height / s, '--cell-bg': c.background } as React.CSSProperties}
      onPointerDown={(e) => {
        // 点在格子的空白处也进入编辑，光标放到最后
        if (e.target !== e.currentTarget || !textRef.current) return;
        e.preventDefault();
        focusEnd(textRef.current);
      }}
    >
      <div
        ref={textRef}
        className="cap-inplace__text cn-selectable"
        data-index={index}
        contentEditable
        suppressContentEditableWarning
        spellCheck={false}
        style={fit.style}
        onBlur={(e) => onChange((e.currentTarget.innerText ?? '').replace(/\s+/g, ' ').trim())}
        onKeyDown={(e) => {
          e.stopPropagation();
          // 回车 = 改完这一格；Esc 放弃焦点（不退出截图）；Tab / Shift+Tab 跳到下一格 / 上一格
          if (e.key === 'Enter' || e.key === 'Escape') {
            e.preventDefault();
            e.currentTarget.blur();
          } else if (e.key === 'Tab') {
            e.preventDefault();
            const layer = e.currentTarget.closest('.cap-inplace');
            const next = layer?.querySelector<HTMLElement>(`.cap-inplace__text[data-index="${index + (e.shiftKey ? -1 : 1)}"]`);
            e.currentTarget.blur();
            if (next) focusEnd(next);
          }
        }}
      >
        {text}
      </div>
    </div>
  );
}

function focusEnd(el: HTMLElement) {
  el.focus();
  const range = document.createRange();
  range.selectNodeContents(el);
  range.collapse(false);
  window.getSelection()?.removeAllRanges();
  window.getSelection()?.addRange(range);
}

export function TableInPlace({
  data,
  selection,
  s,
  viewport,
  busy,
  onAi,
  onClose,
  onDone,
}: {
  data: CaptureTable;
  selection: Rect;
  s: number;
  viewport: Viewport;
  /** AI 正在重新认 */
  busy: boolean;
  onAi: () => void;
  onClose: () => void;
  /** 复制完 / 要导出：结束这次截图 */
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const cells = data.table.cells;
  const [texts, setTexts] = useState(() => cells.map((c) => c.text));
  useEffect(() => setTexts(cells.map((c) => c.text)), [cells]);
  const [original, setOriginal] = useState(false);
  const fits = useMemo(() => fitCells(cells, s, !!data.plain), [cells, s, data.plain]);
  const sel = css(selection, s);
  const done = useDoneToast(onDone);

  const rows = useMemo(() => toRows(cells, texts), [cells, texts]);
  const grid = useMemo(() => tableGrid(rows), [rows]);
  const copyTable = () => void capture.writeRich(toExcelText(grid), toHtmlTable(rows)).then(() => done('copied')).catch(notify.error);
  const copyMarkdown = () => void capture.writeText(toMarkdown(grid)).then(() => done('copied')).catch(notify.error);
  const exportXlsx = () => {
    onDone();
    void ocr
      .exportXlsx(rows)
      .then((path) => path && void system.toast('success', t('ocr.structured.exported')).catch(() => {}))
      .catch(notify.error);
  };
  const plain = data.plain;

  return (
    <>
      <div
        className={clsx('cap-inplace', busy && 'cap-inplace--busy', plain && 'cap-inplace--plain')}
        data-peek={original || undefined}
        style={{ left: sel.x, top: sel.y, width: sel.width, height: sel.height, '--plain-line': plain?.line } as React.CSSProperties}
        onPointerDown={stop}
        onDoubleClick={stop}
        onContextMenu={stop}
      >
        {data.background && <img className="cap-inplace__bg" src={shotUrl(data.background)} alt="" draggable={false} />}
        {cells.map((c, i) => (
          <CellText
            // 改了字就换 key 重建这一格：浏览器改可编辑区域时可能拆出别的节点，React 再去对旧文字节点会对不上
            key={`${i}:${texts[i] ?? ''}`}
            cell={c}
            text={texts[i] ?? c.text}
            s={s}
            fit={fits[i]!}
            index={i}
            onChange={(v) => setTexts((cur) => (cur[i] === v ? cur : cur.map((x, j) => (j === i ? v : x))))}
          />
        ))}
        {plain && <div className="cap-inplace__frame" style={{ left: plain.x / s, top: plain.y / s, width: plain.width / s, height: plain.height / s }} />}
      </div>
      <StructuredBar selection={selection} s={s} viewport={viewport}>
        <BarButton icon={Table2} label={t('capture.structured.table')} tip={t('capture.structured.showResult')} on={!original} onClick={() => setOriginal(false)} />
        <BarButton icon={Eye} label={t('capture.structured.original')} tip={t('capture.structured.showOriginal')} on={original} onClick={() => setOriginal(true)} />
        <span className="cap-trbar__sep" />
        <BarButton icon={Copy} label={t('capture.structured.copy')} tip={t('ocr.structured.copyTableTip')} disabled={busy} onClick={copyTable} />
        <BarButton icon={FileSpreadsheet} tip={t('ocr.structured.exportXlsxTip')} disabled={busy} onClick={exportXlsx} />
        <BarButton icon={FileText} tip={t('ocr.structured.copyMarkdownTip')} disabled={busy} onClick={copyMarkdown} />
        <span className="cap-trbar__sep" />
        {busy ? (
          <span className="cap-trbar__spin" aria-label={t('capture.structured.runningTable')}>
            <Spinner size={14} />
          </span>
        ) : (
          <BarButton icon={Sparkles} tip={t('capture.structured.aiTableTip')} onClick={onAi} />
        )}
        <BarButton icon={X} tip={t('capture.structured.remove')} onClick={onClose} />
      </StructuredBar>
    </>
  );
}

// ───────────────────────── 公式 ─────────────────────────

/** LaTeX → MathML（本地，temml）。写错了返回 null */
function latexToMathml(latex: string): string | null {
  if (!latex.trim()) return null;
  try {
    return sanitizeMathml(temml.renderToString(latex, { displayMode: true, throwOnError: true })) || null;
  } catch {
    return null;
  }
}

/** 公式下面（或上面）那条 LaTeX 输入框 */
const BAR_ROOM = 34 + 6;

/** LaTeX 输入框放哪：选区上沿外面（下沿外面一般是工具条，上面还要留出小条的位置）；放不下就压在选区底部 */
const editorAbove = (sel: CssRect, height: number) => sel.y - height - 6 - BAR_ROOM >= 4;

function LatexEditor({
  value,
  invalid,
  selection,
  s,
  viewport,
  onChange,
  onHeight,
  onClose,
}: {
  value: string;
  invalid: boolean;
  selection: Rect;
  s: number;
  viewport: Viewport;
  onChange: (v: string) => void;
  onHeight: (h: number) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  const size = useElementSize(ref, { width: 320, height: 44 });
  useEffect(() => onHeight(size.height), [size.height, onHeight]);
  const sel = css(selection, s);
  const width = Math.min(viewport.width - 16, Math.max(sel.width, 320));
  const x = Math.max(8, Math.min(sel.x, viewport.width - width - 8));
  const y = editorAbove(sel, size.height) ? sel.y - size.height - 6 : sel.y + sel.height - size.height - 6;
  // 跟着内容长高
  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = '0px';
    el.style.height = `${Math.min(160, el.scrollHeight)}px`;
  }, [value, width]);
  return (
    <div
      ref={ref}
      className="cap-latex cn-glass"
      data-invalid={invalid || undefined}
      style={{ transform: `translate(${x}px, ${y}px)`, width }}
      onPointerDown={stop}
      onDoubleClick={stop}
      onWheel={stop}
    >
      <textarea
        ref={area}
        className="cn-selectable"
        value={value}
        spellCheck={false}
        autoFocus
        placeholder={t('capture.structured.latexPlaceholder')}
        title={invalid ? t('capture.structured.latexError') : undefined}
        onChange={(e) => onChange(e.currentTarget.value)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === 'Escape') {
            e.preventDefault();
            onClose();
          }
        }}
      />
    </div>
  );
}

export function FormulaInPlace({
  text,
  region,
  selection,
  s,
  viewport,
  busy,
  onAi,
  onClose,
  onDone,
}: {
  text: string;
  region: RegionStyle;
  selection: Rect;
  s: number;
  viewport: Viewport;
  /** AI 正在重新认 */
  busy: boolean;
  onAi: () => void;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const parsed = useMemo(() => splitFormula(text), [text]);
  const [latex, setLatex] = useState(parsed.latex);
  useEffect(() => setLatex(parsed.latex), [parsed]);
  // 显示用的 MathML：按现在的 LaTeX 本地转；转不了（写到一半、有错）就留着上一次转好的，最开始用模型给的
  const fresh = useMemo(() => latexToMathml(latex), [latex]);
  const lastGood = useRef<string>('');
  if (fresh) lastGood.current = fresh;
  const mathml = fresh ?? (lastGood.current || parsed.mathml);
  const [editing, setEditing] = useState(false);
  const [original, setOriginal] = useState(false);
  const [editorH, setEditorH] = useState(44);
  const sel = css(selection, s);
  const done = useDoneToast(onDone);
  const showEditor = editing && !original;

  // 按 24px 摆一遍量大小，再放大 / 缩小到撑满选区（宽不超过九成二、高不超过八成）
  const mathRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState(24);
  useLayoutEffect(() => {
    const el = mathRef.current;
    if (!el) return;
    el.style.fontSize = '24px';
    const r = el.getBoundingClientRect();
    if (!r.width || !r.height) return;
    const k = Math.min((sel.width * 0.92) / r.width, (sel.height * 0.8) / r.height);
    const next = Math.min(200, Math.max(8, 24 * k));
    el.style.fontSize = `${next}px`;
    setSize(next);
  }, [mathml, latex, sel.width, sel.height]);
  const copy = (value: string) => void capture.writeText(value).then(() => done('copied')).catch(notify.error);

  return (
    <>
      <div
        className={clsx('cap-inplace cap-inplace--formula', busy && 'cap-inplace--busy')}
        data-peek={original || undefined}
        data-editing={editing || undefined}
        style={{ left: sel.x, top: sel.y, width: sel.width, height: sel.height, background: region.background, color: region.color }}
        title={editing ? undefined : t('capture.structured.editLatex')}
        onPointerDown={(e) => {
          e.stopPropagation();
          setEditing(true);
        }}
        onDoubleClick={stop}
      >
        {mathml ? (
          // 已经过 sanitizeMathml：只剩 MathML 标签和排版属性
          <div ref={mathRef} className="cap-inplace__math" style={{ fontSize: size }} dangerouslySetInnerHTML={{ __html: mathml }} />
        ) : (
          <div ref={mathRef} className="cap-inplace__math cap-inplace__math--latex" style={{ fontSize: size }}>
            {latex}
          </div>
        )}
      </div>
      {showEditor && (
        <LatexEditor
          value={latex}
          invalid={!fresh && !!latex.trim()}
          selection={selection}
          s={s}
          viewport={viewport}
          onChange={setLatex}
          onHeight={setEditorH}
          onClose={() => setEditing(false)}
        />
      )}
      <StructuredBar selection={selection} s={s} viewport={viewport} lift={showEditor && editorAbove(sel, editorH) ? editorH + 6 : 0}>
        <BarButton icon={Sigma} label={t('capture.structured.formula')} tip={t('capture.structured.showResult')} on={!original} onClick={() => setOriginal(false)} />
        <BarButton icon={Eye} label={t('capture.structured.original')} tip={t('capture.structured.showOriginal')} on={original} onClick={() => setOriginal(true)} />
        <span className="cap-trbar__sep" />
        <BarButton icon={Copy} label="LaTeX" tip={t('ocr.structured.copyLatexTip')} disabled={busy || !latex.trim()} onClick={() => copy(latex.trim())} />
        <BarButton icon={Copy} label="MathML" tip={t('ocr.structured.copyMathmlTip')} disabled={busy || !mathml} onClick={() => copy(mathml)} />
        <BarButton icon={PenLine} tip={t('capture.structured.editLatex')} on={editing} disabled={busy} onClick={() => setEditing((v) => !v)} />
        <span className="cap-trbar__sep" />
        {busy ? (
          <span className="cap-trbar__spin" aria-label={t('capture.structured.runningFormula')}>
            <Spinner size={14} />
          </span>
        ) : (
          <BarButton icon={Sparkles} tip={t('capture.structured.aiFormulaTip')} onClick={onAi} />
        )}
        <BarButton icon={X} tip={t('capture.structured.remove')} onClick={onClose} />
      </StructuredBar>
    </>
  );
}
