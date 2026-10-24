// 识字窗口里的"识别成表格 / 公式"（用 AI 模型看图）。
//
// 截图界面里的"识别表格 / 公式"也用这里的 TableView / FormulaView。表格的单元格可以直接改，复制、导出用改过的。
//
// 模型的回答不直接塞进页面：表格只取单元格的文字和合并信息，用 React 重新生成；公式的 MathML 只留
// 数学相关的标签和少数几个属性。复制给别的程序用的是纯文本（制表符分隔 / Markdown / LaTeX / MathML）。

import { ArrowLeft, Copy, FileSearch, FileSpreadsheet, RotateCw, Sparkles } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { capture, ocr } from '@/lib/ipc';
import { Button, EmptyState, Spinner } from '@/ui/controls';
import { notify, Tooltip } from '@/ui/overlays';

import { tableGrid, toExcelText, toHtmlTable, toMarkdown, unmatchedNumbers, type Cell } from './structured';
import './structured.css';

export type StructuredKind = 'table' | 'formula';
export interface StructuredState {
  kind: StructuredKind;
  status: 'running' | 'done' | 'error';
  /** 表格：local = 本地识别（快、不联网）| ai = AI 看图；公式只有 AI */
  engine?: 'local' | 'ai';
  text?: string;
  error?: string;
}

/** 本地识别的表格可以再用 AI 认一遍（合并单元格多、表格线不清楚时 AI 更准） */
export const canAskAi = (s: StructuredState) => s.kind === 'table' && s.engine !== 'ai' && s.status !== 'running';

/** 从模型给的 HTML 里取出表格：每行的单元格（文字、是不是表头、合并了几行几列） */
export function parseTable(html: string): Cell[][] {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  const table = doc.querySelector('table');
  if (!table) return [];
  return [...table.rows].map((row) =>
    [...row.cells].map((c) => ({
      text: (c.textContent ?? '').replace(/\s+/g, ' ').trim(),
      header: c.tagName === 'TH',
      rowSpan: Math.min(50, Math.max(1, c.rowSpan || 1)),
      colSpan: Math.min(50, Math.max(1, c.colSpan || 1)),
    })),
  );
}

const MATHML_TAGS = new Set([
  'math', 'mrow', 'mi', 'mn', 'mo', 'ms', 'mtext', 'mspace', 'mfrac', 'msqrt', 'mroot', 'msub', 'msup', 'msubsup',
  'munder', 'mover', 'munderover', 'mtable', 'mtr', 'mtd', 'mstyle', 'mpadded', 'mphantom', 'menclose', 'semantics',
  'annotation', 'mmultiscripts', 'mprescripts', 'none',
]);
// 后面几个是本地把 LaTeX 转成 MathML（temml）时用的间距、字号属性：只是数值，不会带进脚本或链接
const MATHML_ATTRS = new Set(['display', 'mathvariant', 'stretchy', 'fence', 'separator', 'accent', 'accentunder', 'columnalign', 'rowspacing', 'columnspacing', 'notation', 'linethickness', 'largeop', 'movablelimits', 'form', 'lspace', 'rspace', 'encoding', 'width', 'height', 'depth', 'voffset', 'scriptlevel', 'displaystyle', 'minsize', 'maxsize', 'symmetric', 'mathsize']);

/** 只留 MathML 的标签和几个排版属性，其余（脚本、链接、样式、事件）都去掉。认不出就返回空 */
export function sanitizeMathml(source: string): string {
  const start = source.indexOf('<math');
  const end = source.lastIndexOf('</math>');
  if (start < 0 || end < 0) return '';
  // 用 HTML 的解析器读：比 XML 宽容（模型偶尔漏个闭合标签），<math> 里照样按 MathML 解析
  const doc = new DOMParser().parseFromString(source.slice(start, end + 7), 'text/html');
  const root = doc.querySelector('math');
  if (!root) return '';
  const clean = (el: Element): string => {
    const tag = el.localName;
    if (!MATHML_TAGS.has(tag)) return '';
    const attrs = [...el.attributes]
      .filter((a) => MATHML_ATTRS.has(a.localName))
      .map((a) => ` ${a.localName}="${a.value.replace(/[&"<>]/g, (ch) => `&#${ch.charCodeAt(0)};`)}"`)
      .join('');
    const inner = [...el.childNodes]
      .map((n) => (n.nodeType === Node.ELEMENT_NODE ? clean(n as Element) : n.nodeType === Node.TEXT_NODE ? (n.textContent ?? '').replace(/[&<>]/g, (ch) => `&#${ch.charCodeAt(0)};`) : ''))
      .join('');
    return `<${tag}${attrs}>${inner}</${tag}>`;
  };
  return clean(root);
}

/** 模型回答：LaTeX，一行 ===MATHML===，MathML */
export function splitFormula(text: string): { latex: string; mathml: string } {
  const [latex = '', mathml = ''] = text.split(/^\s*===MATHML===\s*$/m);
  return { latex: latex.trim(), mathml: sanitizeMathml(mathml) };
}

export function StructuredResult({
  state,
  ocrText,
  onBack,
  onRetry,
}: {
  state: StructuredState;
  /** 本地识字（不经过 AI）的全部文字：表格里的数字拿它对照 */
  ocrText: string;
  onBack: () => void;
  /** 再识别一次；`engine` = ai 时换 AI 看图 */
  onRetry: (engine?: 'local' | 'ai') => void;
}) {
  const { t } = useTranslation();
  const copy = (text: string) =>
    void capture
      .writeText(text)
      .then(() => notify.success(t('ocr.structured.copied')))
      .catch(notify.error);
  const title = state.kind === 'table' ? t('ocr.structured.table') : t('ocr.structured.formula');

  return (
    <div className="ocr-struct">
      <div className="ocr-struct__head">
        <Button size="sm" variant="ghost" icon={ArrowLeft} onClick={onBack}>
          {t('ocr.structured.back')}
        </Button>
        <span className="ocr-struct__title">{title}</span>
        {canAskAi(state) && (
          <Button size="sm" variant="ghost" icon={Sparkles} onClick={() => onRetry('ai')}>
            {t('ocr.structured.askAi')}
          </Button>
        )}
      </div>
      {state.status === 'running' ? (
        <div className="ocr-struct__loading">
          <Spinner size={18} />
          {t('ocr.structured.running')}
        </div>
      ) : state.status === 'error' ? (
        <EmptyState
          icon={FileSearch}
          title={t('ocr.structured.failed')}
          description={state.error ?? ''}
          action={
            <Button icon={RotateCw} onClick={() => onRetry(state.engine)}>
              {t('common.retry')}
            </Button>
          }
        />
      ) : state.kind === 'table' ? (
        <TableView html={state.text ?? ''} ocrText={ocrText} />
      ) : (
        <FormulaView text={state.text ?? ''} onCopy={copy} />
      )}
    </div>
  );
}

/** 复制 / 导出之后要做的事（截图界面里：复制完关掉截图；导出前先关掉截图，保存对话框才不会被遮罩挡住） */
export interface StructuredHooks {
  /** 复制成功之后；给了它就不再在页面里提示"已复制" */
  onCopied?: (message: string) => void;
  /** 弹保存对话框之前 */
  beforeExport?: () => void;
}

export function TableView({ html, ocrText, hooks }: { html: string; ocrText: string | null; hooks?: StructuredHooks }) {
  const { t } = useTranslation();
  const parsed = useMemo(() => parseTable(html), [html]);
  // 用户改过的单元格（失焦时写回）
  const [rows, setRows] = useState(parsed);
  useEffect(() => setRows(parsed), [parsed]);
  const grid = useMemo(() => tableGrid(rows), [rows]);
  // 和本地识字对数字：没有本地识字结果（截图界面里）就不对
  const flagged = useMemo(() => (ocrText == null ? new Set<string>() : unmatchedNumbers(rows, ocrText)), [rows, ocrText]);
  const copied = () => {
    const message = t('ocr.structured.copied');
    if (hooks?.onCopied) hooks.onCopied(message);
    else notify.success(message);
  };
  const copyTable = () => void capture.writeRich(toExcelText(grid), toHtmlTable(rows)).then(copied).catch(notify.error);
  const copyMarkdown = () => void capture.writeText(toMarkdown(grid)).then(copied).catch(notify.error);
  const exportXlsx = () => {
    hooks?.beforeExport?.();
    void ocr
      .exportXlsx(rows)
      .then((path) => path && notify.success(t('ocr.structured.exported')))
      .catch(notify.error);
  };
  const edit = (r: number, i: number, text: string) =>
    setRows((cur) => {
      const clean = text.replace(/\s+/g, ' ').trim();
      if (cur[r]?.[i]?.text === clean) return cur;
      return cur.map((row, y) => (y !== r ? row : row.map((c, x) => (x !== i ? c : { ...c, text: clean }))));
    });
  if (!rows.length) return <EmptyState icon={FileSearch} title={t('ocr.structured.noTable')} description="" />;
  return (
    <>
      {flagged.size > 0 && <p className="ocr-struct__warn">{t('ocr.structured.mismatch', { count: flagged.size })}</p>}
      <div className="ocr-struct__body">
        <table className="ocr-table cn-selectable">
          <tbody>
            {rows.map((row, r) => (
              <tr key={r}>
                {row.map((c, i) => {
                  const off = flagged.has(`${r},${i}`);
                  const props = {
                    rowSpan: c.rowSpan,
                    colSpan: c.colSpan,
                    className: off ? 'ocr-cell--check' : undefined,
                    title: off ? t('ocr.structured.checkCell') : undefined,
                    contentEditable: true,
                    suppressContentEditableWarning: true,
                    spellCheck: false,
                    onBlur: (e: React.FocusEvent<HTMLElement>) => edit(r, i, e.currentTarget.textContent ?? ''),
                    onKeyDown: (e: React.KeyboardEvent<HTMLElement>) => {
                      // 回车 = 改完这一格（单元格里不换行）；Esc 放弃焦点
                      if (e.key === 'Enter' || e.key === 'Escape') {
                        e.preventDefault();
                        e.currentTarget.blur();
                      }
                      e.stopPropagation();
                    },
                  };
                  // 文字一改就换 key 重建这一格：浏览器改单元格时可能拆出别的节点，React 再去对旧文字节点会对不上
                  const key = `${i}:${c.text}`;
                  return c.header ? (
                    <th key={key} {...props}>
                      {c.text}
                    </th>
                  ) : (
                    <td key={key} {...props}>
                      {c.text}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="ocr-struct__actions">
        <Tooltip content={t('ocr.structured.copyTableTip')} side="top">
          <Button size="sm" variant="primary" icon={Copy} onClick={copyTable}>
            {t('common.copy')}
          </Button>
        </Tooltip>
        <Tooltip content={t('ocr.structured.exportXlsxTip')} side="top">
          <Button size="sm" icon={FileSpreadsheet} onClick={exportXlsx}>
            {t('ocr.structured.exportXlsx')}
          </Button>
        </Tooltip>
        <Tooltip content={t('ocr.structured.copyMarkdownTip')} side="top">
          <Button size="sm" icon={Copy} onClick={copyMarkdown}>
            Markdown
          </Button>
        </Tooltip>
      </div>
    </>
  );
}

export function FormulaView({ text, onCopy }: { text: string; onCopy: (text: string) => void }) {
  const { t } = useTranslation();
  const parsed = useMemo(() => splitFormula(text), [text]);
  const [latex, setLatex] = useState(parsed.latex);
  return (
    <>
      <div className="ocr-struct__body">
        {parsed.mathml ? (
          // 已经过 sanitizeMathml：只剩 MathML 标签和排版属性
          <div className="ocr-formula" dangerouslySetInnerHTML={{ __html: parsed.mathml }} />
        ) : (
          <p className="ocr-struct__hint">{t('ocr.structured.noPreview')}</p>
        )}
        <textarea
          className="ocr-latex cn-selectable"
          value={latex}
          spellCheck={false}
          onChange={(e) => setLatex(e.currentTarget.value)}
          onKeyDown={(e) => e.stopPropagation()}
        />
      </div>
      <div className="ocr-struct__actions">
        <Tooltip content={t('ocr.structured.copyLatexTip')} side="top">
          <Button size="sm" variant="primary" icon={Copy} disabled={!latex.trim()} onClick={() => onCopy(latex.trim())}>
            LaTeX
          </Button>
        </Tooltip>
        <Tooltip content={t('ocr.structured.copyMathmlTip')} side="top">
          <Button size="sm" icon={Copy} disabled={!parsed.mathml} onClick={() => onCopy(parsed.mathml)}>
            MathML
          </Button>
        </Tooltip>
      </div>
    </>
  );
}
