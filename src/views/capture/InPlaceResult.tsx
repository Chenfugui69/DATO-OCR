// 截图界面里的「识别表格 / 公式」：认出来的结果原位盖在原来的位置上（像截图翻译）。
//
// - 表格：底图是选区原图把字抹掉（线、底色、边框都是原来的），每格的字按原来的位置、字号、颜色、粗细、
//   对齐摆上去，看着和原来几乎一样；点一下就能改，复制 / 导出用改过的。系统字体和原图的字宽不一样，
//   摆好后量一下宽度，字号微调到和原来一样宽
// - 公式：用选区的底色铺满，按选区大小摆上认出来的公式；LaTeX 可以在旁边改
//
// 原来的样子被盖住了，所以选区左边（放不下就右边，两边都放不下就压在选区左下角）放一张原图给用户对照，
// 复制、导出这些按钮也在那儿。复制完就结束截图（和二维码卡片一样），导出前先结束截图，保存对话框才不会被遮罩挡住。

import clsx from 'clsx';
import { ChevronDown, ChevronUp, Copy, Eye, FileSpreadsheet, FileText, PenLine, Sparkles, X } from 'lucide-react';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useElementSize } from '@/lib/hooks';
import { capture, ocr, system } from '@/lib/ipc';
import { shotUrl } from '@/lib/platform';
import type { CaptureTable, LaidCell, RegionStyle } from '@/lib/types';
import { Button, Spinner } from '@/ui/controls';
import { notify, Tooltip } from '@/ui/overlays';
import { parseTable, splitFormula } from '@/views/ocr/StructuredResult';
import { tableGrid, toExcelText, toHtmlTable, toMarkdown, type Cell } from '@/views/ocr/structured';

import type { Rect } from './geometry';

type Viewport = { width: number; height: number };
type CssRect = { x: number; y: number; width: number; height: number };

const css = (r: Rect, s: number): CssRect => ({ x: r.x / s, y: r.y / s, width: r.width / s, height: r.height / s });
const stop = (e: React.SyntheticEvent) => e.stopPropagation();
const median = (v: number[]) => (v.length ? [...v].sort((a, b) => a - b)[Math.floor(v.length / 2)]! : undefined);

/** 认出来的格子 + 改过的字 → 导出用的一行行单元格 */
function toRows(cells: LaidCell[], texts: string[]): Cell[][] {
  const rows: Cell[][] = [];
  cells.forEach((c, i) => {
    (rows[c.row] ??= []).push({ text: texts[i] ?? c.text, header: c.header, rowSpan: c.rowSpan, colSpan: c.colSpan });
  });
  return rows.filter(Boolean);
}

/**
 * AI 重新认的表格（HTML）套回原位的格子：行列对得上就只换字，样式、位置还是原来量的；对不上返回 null
 * （那就用卡片显示 AI 的结果）
 */
export function mergeAiTable(laid: CaptureTable, html: string): CaptureTable | null {
  const grid = tableGrid(parseTable(html));
  const { rows, cols } = laid.table;
  if (grid.length !== rows || grid.some((r) => r.length !== cols)) return null;
  return { ...laid, table: { ...laid.table, cells: laid.table.cells.map((c) => ({ ...c, text: grid[c.row]?.[c.col] ?? c.text })) } };
}

// ───────────────────────── 原图对照面板 ─────────────────────────

const PANEL_GAP = 12;
const PANEL_MIN = 200;
const PANEL_MAX = 420;

function ReferencePanel({
  source,
  selection,
  s,
  viewport,
  title,
  extra,
  actions,
  onClose,
}: {
  source: string;
  selection: Rect;
  s: number;
  viewport: Viewport;
  title: string;
  /** 图下面、按钮上面的东西（公式的 LaTeX 编辑框） */
  extra?: React.ReactNode;
  actions: React.ReactNode;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  const size = useElementSize(ref, { width: PANEL_MIN, height: 160 });
  const [folded, setFolded] = useState(false);
  const sel = css(selection, s);
  // 左边放得下就放左边，否则右边；两边都放不下就压在选区左下角（可以把图收起来）
  const left = sel.x - PANEL_GAP - 8;
  const right = viewport.width - sel.x - sel.width - PANEL_GAP - 8;
  const side = left >= PANEL_MIN ? 'left' : right >= PANEL_MIN ? 'right' : 'inside';
  const width = Math.min(PANEL_MAX, Math.max(PANEL_MIN, side === 'left' ? left : side === 'right' ? right : PANEL_MIN), side === 'inside' ? PANEL_MIN : sel.width);
  const x = side === 'left' ? sel.x - PANEL_GAP - width : side === 'right' ? sel.x + sel.width + PANEL_GAP : Math.max(8, sel.x + 8);
  const y =
    side === 'inside'
      ? Math.max(8, Math.min(sel.y + sel.height - size.height - 8, viewport.height - size.height - 8))
      : Math.max(8, Math.min(sel.y, viewport.height - size.height - 8));
  // 图最高多少：别把按钮挤出屏幕
  const imgMax = Math.max(80, viewport.height - 16 - 96 - (extra ? 120 : 0));
  return (
    <div
      ref={ref}
      className="cap-ref cn-glass"
      data-side={side}
      style={{ transform: `translate(${x}px, ${y}px)`, width }}
      onPointerDown={stop}
      onDoubleClick={stop}
      onWheel={stop}
    >
      <div className="cap-ref__head">
        <span className="cap-ref__title">{title}</span>
        <span className="cap-ref__sub">{t('capture.inplace.original')}</span>
        <button
          type="button"
          className="cap-codes__icon"
          aria-label={folded ? t('capture.inplace.unfold') : t('capture.inplace.fold')}
          title={folded ? t('capture.inplace.unfold') : t('capture.inplace.fold')}
          onClick={() => setFolded((f) => !f)}
        >
          {folded ? <ChevronDown size={14} strokeWidth={1.75} /> : <ChevronUp size={14} strokeWidth={1.75} />}
        </button>
        <button type="button" className="cap-codes__icon" aria-label={t('common.close')} title={t('common.close')} onClick={onClose}>
          <X size={14} strokeWidth={1.75} />
        </button>
      </div>
      {!folded && <img className="cap-ref__img" src={shotUrl(source)} alt="" draggable={false} style={{ maxHeight: imgMax }} />}
      {extra}
      <div className="cap-ref__actions">{actions}</div>
    </div>
  );
}

/** 按住看一眼原图（松开回来） */
function PeekButton({ onPeek }: { onPeek: (on: boolean) => void }) {
  const { t } = useTranslation();
  return (
    <Tooltip content={t('capture.structured.peek')} side="top">
      <button
        type="button"
        className="cap-ref__icon"
        aria-label={t('capture.structured.peek')}
        onPointerDown={() => onPeek(true)}
        onPointerUp={() => onPeek(false)}
        onPointerLeave={() => onPeek(false)}
      >
        <Eye size={15} strokeWidth={1.6} />
      </button>
    </Tooltip>
  );
}

function IconAction({ icon: I, label, onClick, disabled }: { icon: typeof Copy; label: string; onClick: () => void; disabled?: boolean }) {
  return (
    <Tooltip content={label} side="top">
      <button type="button" className="cap-ref__icon" aria-label={label} disabled={disabled} onClick={onClick}>
        <I size={15} strokeWidth={1.6} />
      </button>
    </Tooltip>
  );
}

// ───────────────────────── 表格 ─────────────────────────

function CellText({
  cell: c,
  text,
  s,
  typical,
  fit,
  onChange,
}: {
  cell: LaidCell;
  text: string;
  s: number;
  /** 空格子打字用的字号（整张表一般的字号） */
  typical: number;
  /** 量完宽度后横向拉伸多少（系统字体和原图的字宽窄不同，字高不变） */
  fit: number;
  onChange: (text: string) => void;
}) {
  const textRef = useRef<HTMLDivElement>(null);
  const tb = c.textBox;
  const font = (tb ? c.fontPx : typical) / s;
  // 行高：多行的按墨迹高均分，单行的给字号的 1.25 倍（行框比字大一点，光标好看）
  const lineH = tb && c.lines > 1 ? tb[3] / c.lines / s : font * 1.25;
  const style: React.CSSProperties = {
    color: c.color,
    fontWeight: c.bold ? 700 : 400,
    fontSize: font,
    lineHeight: `${lineH}px`,
    textAlign: c.align,
    whiteSpace: c.lines > 1 ? 'pre-wrap' : 'nowrap',
  };
  if (tb) {
    // 行框的中心对上墨迹的中心（有下伸部分的字墨迹偏下，按 centerShift 挪回去）
    const inkCenter = (tb[1] + tb[3] / 2 - c.y) / s;
    const block = lineH * Math.max(1, c.lines);
    style.top = inkCenter - block / 2 - (c.lines > 1 ? 0 : c.centerShift * font);
    if (c.align === 'right') style.right = (c.x + c.width - tb[0] - tb[2]) / s;
    else style.left = (tb[0] - c.x) / s;
    if (c.align === 'center' || c.lines > 1) style.width = tb[2] / s;
    if (fit !== 1) {
      style.transform = `scaleX(${fit})`;
      style.transformOrigin = c.align === 'right' ? 'right center' : c.align === 'center' ? 'center' : 'left center';
    }
  } else {
    style.left = 4;
    style.right = 4;
    style.top = (c.height / s - lineH) / 2;
  }
  return (
    <div
      className="cap-inplace__cell"
      style={{ left: c.x / s, top: c.y / s, width: c.width / s, height: c.height / s, '--cell-bg': c.background } as React.CSSProperties}
      onPointerDown={(e) => {
        // 点在格子的空白处也进入编辑，光标放到最后
        if (e.target !== e.currentTarget || !textRef.current) return;
        e.preventDefault();
        const el = textRef.current;
        el.focus();
        const range = document.createRange();
        range.selectNodeContents(el);
        range.collapse(false);
        window.getSelection()?.removeAllRanges();
        window.getSelection()?.addRange(range);
      }}
    >
      <div
        ref={textRef}
        className="cap-inplace__text cn-selectable"
        contentEditable
        suppressContentEditableWarning
        spellCheck={false}
        data-fit={tb && c.lines === 1 && c.align !== 'center' ? tb[2] / s : undefined}
        style={style}
        onBlur={(e) => onChange((e.currentTarget.innerText ?? '').replace(/\s+/g, ' ').trim())}
        onKeyDown={(e) => {
          // 回车 = 改完这一格；Esc 放弃焦点（不退出截图）
          if (e.key === 'Enter' || e.key === 'Escape') {
            e.preventDefault();
            e.currentTarget.blur();
          }
          e.stopPropagation();
        }}
      >
        {text}
      </div>
    </div>
  );
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
  const [peek, setPeek] = useState(false);
  const [fits, setFits] = useState<number[]>([]);
  const layer = useRef<HTMLDivElement>(null);
  const typical = useMemo(() => median(cells.filter((c) => c.textBox).map((c) => c.fontPx)) ?? 14 * s, [cells, s]);
  const sel = css(selection, s);

  // 摆好后量一下每格字的宽度，横向拉伸到和原图里的字一样宽（系统字体和原图的字体宽窄不同，字高按墨迹已经对上了）。
  // 只在刚认出来时量一次，之后改字不再变
  useLayoutEffect(() => {
    const els = [...(layer.current?.querySelectorAll<HTMLElement>('.cap-inplace__text') ?? [])];
    setFits(
      els.map((el) => {
        const target = Number(el.dataset.fit);
        const natural = el.scrollWidth;
        if (!target || !natural) return 1;
        const k = target / natural;
        return Math.abs(k - 1) < 0.03 ? 1 : Math.min(1.2, Math.max(0.8, k));
      }),
    );
  }, [cells, s]);

  const rows = useMemo(() => toRows(cells, texts), [cells, texts]);
  const grid = useMemo(() => tableGrid(rows), [rows]);
  const copied = () => {
    onDone();
    void system.toast('success', t('ocr.structured.copied')).catch(() => {});
  };
  const copyTable = () => void capture.writeRich(toExcelText(grid), toHtmlTable(rows)).then(copied).catch(notify.error);
  const copyMarkdown = () => void capture.writeText(toMarkdown(grid)).then(copied).catch(notify.error);
  const exportXlsx = () => {
    onDone();
    void ocr
      .exportXlsx(rows)
      .then((path) => path && void system.toast('success', t('ocr.structured.exported')).catch(() => {}))
      .catch(notify.error);
  };

  return (
    <>
      <div
        ref={layer}
        className={clsx('cap-inplace', busy && 'cap-inplace--busy')}
        data-peek={peek || undefined}
        style={{ left: sel.x, top: sel.y, width: sel.width, height: sel.height }}
        onPointerDown={stop}
        onDoubleClick={stop}
        onContextMenu={stop}
      >
        <img className="cap-inplace__bg" src={shotUrl(data.background)} alt="" draggable={false} />
        {cells.map((c, i) => (
          <CellText
            // 改了字就换 key 重建这一格：浏览器改可编辑区域时可能拆出别的节点，React 再去对旧文字节点会对不上
            key={`${i}:${texts[i] ?? ''}`}
            cell={c}
            text={texts[i] ?? c.text}
            s={s}
            typical={typical}
            fit={fits[i] ?? 1}
            onChange={(v) => setTexts((cur) => (cur[i] === v ? cur : cur.map((x, j) => (j === i ? v : x))))}
          />
        ))}
      </div>
      <ReferencePanel
        source={data.source}
        selection={selection}
        s={s}
        viewport={viewport}
        title={t('actions.table')}
        onClose={onClose}
        actions={
          <>
            <Tooltip content={t('ocr.structured.copyTableTip')} side="top">
              <Button size="sm" variant="primary" icon={Copy} onClick={copyTable}>
                {t('common.copy')}
              </Button>
            </Tooltip>
            <IconAction icon={FileSpreadsheet} label={t('ocr.structured.exportXlsxTip')} onClick={exportXlsx} />
            <IconAction icon={FileText} label={t('ocr.structured.copyMarkdownTip')} onClick={copyMarkdown} />
            {busy ? (
              <span className="cap-ref__icon" aria-label={t('capture.structured.runningTable')}>
                <Spinner size={14} />
              </span>
            ) : (
              <IconAction icon={Sparkles} label={t('ocr.structured.askAiTip')} onClick={onAi} />
            )}
            <PeekButton onPeek={setPeek} />
          </>
        }
      />
    </>
  );
}

// ───────────────────────── 公式 ─────────────────────────

export function FormulaInPlace({
  text,
  region,
  selection,
  s,
  viewport,
  onClose,
  onDone,
}: {
  text: string;
  region: RegionStyle;
  selection: Rect;
  s: number;
  viewport: Viewport;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const parsed = useMemo(() => splitFormula(text), [text]);
  const [latex, setLatex] = useState(parsed.latex);
  useEffect(() => setLatex(parsed.latex), [parsed]);
  const [editing, setEditing] = useState(false);
  const [peek, setPeek] = useState(false);
  const sel = css(selection, s);
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
    setSize(Math.min(200, Math.max(8, 24 * k)));
  }, [parsed.mathml, latex, sel.width, sel.height]);
  const copy = (value: string) =>
    void capture
      .writeText(value)
      .then(() => {
        onDone();
        void system.toast('success', t('ocr.structured.copied')).catch(() => {});
      })
      .catch(notify.error);

  return (
    <>
      <div
        className="cap-inplace cap-inplace--formula"
        data-peek={peek || undefined}
        style={{ left: sel.x, top: sel.y, width: sel.width, height: sel.height, background: region.background, color: region.color }}
        onPointerDown={stop}
        onDoubleClick={stop}
      >
        {parsed.mathml ? (
          // 已经过 sanitizeMathml：只剩 MathML 标签和排版属性
          <div ref={mathRef} className="cap-inplace__math cn-selectable" style={{ fontSize: size }} dangerouslySetInnerHTML={{ __html: parsed.mathml }} />
        ) : (
          <div ref={mathRef} className="cap-inplace__math cap-inplace__math--latex cn-selectable" style={{ fontSize: size }}>
            {latex}
          </div>
        )}
      </div>
      <ReferencePanel
        source={region.source}
        selection={selection}
        s={s}
        viewport={viewport}
        title={t('actions.formula')}
        onClose={onClose}
        extra={
          editing && (
            <textarea
              className="cap-ref__latex cn-selectable"
              value={latex}
              spellCheck={false}
              autoFocus
              onChange={(e) => setLatex(e.currentTarget.value)}
              onKeyDown={stop}
            />
          )
        }
        actions={
          <>
            <Tooltip content={t('ocr.structured.copyLatexTip')} side="top">
              <Button size="sm" variant="primary" icon={Copy} disabled={!latex.trim()} onClick={() => copy(latex.trim())}>
                LaTeX
              </Button>
            </Tooltip>
            <Tooltip content={t('ocr.structured.copyMathmlTip')} side="top">
              <Button size="sm" icon={Copy} disabled={!parsed.mathml} onClick={() => copy(parsed.mathml)}>
                MathML
              </Button>
            </Tooltip>
            <IconAction icon={PenLine} label={t('capture.inplace.editLatex')} onClick={() => setEditing((v) => !v)} />
            <PeekButton onPeek={setPeek} />
          </>
        }
      />
    </>
  );
}
