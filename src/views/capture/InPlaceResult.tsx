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

import { useElementSize } from '@/lib/hooks';
import { capture, ocr, system } from '@/lib/ipc';
import { shotUrl } from '@/lib/platform';
import { useSettings } from '@/lib/settings';
import type { CaptureTable, LaidCell, RegionStyle } from '@/lib/types';
import { Spinner } from '@/ui/controls';
import { notify, Tooltip } from '@/ui/overlays';
import { parseTable, splitFormula } from '@/views/ocr/StructuredResult';
import { tableGrid, toExcelText, toHtmlTable, toMarkdown, type Cell } from '@/views/ocr/structured';

import type { Rect } from './geometry';
import { latexToMathml } from './formulaRender';
import { layoutTable, mergeTable } from './tableLayout';
import { TableSheet } from './TableSheet';

type Viewport = { width: number; height: number };
type CssRect = { x: number; y: number; width: number; height: number };

const css = (r: Rect, s: number): CssRect => ({ x: r.x / s, y: r.y / s, width: r.width / s, height: r.height / s });
const stop = (e: React.SyntheticEvent) => e.stopPropagation();

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
      // 用 left / top 定位：入场动画的 scale 要以条自己为中心缩放（位置写在 transform 上的话，缩放会把位移也一起缩，
      // 看起来像从屏幕左上角飞进来）
      style={{ left: x, top: y }}
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

/** 原图摆在选区左边（放不下就右边）时的横坐标；两边都放不下给 null（那就用小条上的按钮切换） */
function sideX(sel: CssRect, viewport: Viewport): number | null {
  const gap = 12;
  if (sel.x - gap - sel.width >= 4) return sel.x - gap - sel.width;
  if (sel.x + sel.width + gap + sel.width <= viewport.width - 4) return sel.x + sel.width + gap;
  return null;
}

/** 原图怎么看：设置里选的（默认摆在左边）；摆不下时退回按钮切换 */
function useOriginalSide(selection: Rect, s: number, viewport: Viewport): number | null {
  const mode = useSettings()?.capture.inplaceOriginal ?? 'side';
  return mode === 'side' ? sideX(css(selection, s), viewport) : null;
}

/** 选区旁边的原图：和选区一样大，不挡操作（点击穿过去） */
function OriginalBeside({ source, selection, s, x }: { source: string; selection: Rect; s: number; x: number }) {
  const { t } = useTranslation();
  const sel = css(selection, s);
  return (
    <div className="cap-original" style={{ left: x, top: sel.y, width: sel.width, height: sel.height }} aria-hidden>
      <img src={shotUrl(source)} alt="" draggable={false} />
      <span className="cap-original__tag">{t('capture.structured.original')}</span>
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
  const [original, setOriginal] = useState(false);
  const side = useOriginalSide(selection, s, viewport);
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

  return (
    <>
      <TableSheet
        data={data}
        s={s}
        texts={texts}
        onTexts={setTexts}
        busy={busy}
        hidden={side === null && original}
        style={{ left: sel.x, top: sel.y, width: sel.width, height: sel.height }}
      />
      {side !== null && <OriginalBeside source={data.source} selection={selection} s={s} x={side} />}
      <StructuredBar selection={selection} s={s} viewport={viewport}>
        {side === null && (
          <>
            <BarButton icon={Table2} label={t('capture.structured.table')} tip={t('capture.structured.showResult')} on={!original} onClick={() => setOriginal(false)} />
            <BarButton icon={Eye} label={t('capture.structured.original')} tip={t('capture.structured.showOriginal')} on={original} onClick={() => setOriginal(true)} />
            <span className="cap-trbar__sep" />
          </>
        )}
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
      style={{ left: x, top: y, width }}
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
  const side = useOriginalSide(selection, s, viewport);
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
        data-peek={(side === null && original) || undefined}
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
      {side !== null && <OriginalBeside source={region.source} selection={selection} s={s} x={side} />}
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
        {side === null && (
          <>
            <BarButton icon={Sigma} label={t('capture.structured.formula')} tip={t('capture.structured.showResult')} on={!original} onClick={() => setOriginal(false)} />
            <BarButton icon={Eye} label={t('capture.structured.original')} tip={t('capture.structured.showOriginal')} on={original} onClick={() => setOriginal(true)} />
            <span className="cap-trbar__sep" />
          </>
        )}
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
