// 截图界面里的"识别表格 / 公式"：AI 认出来的表格 / 公式直接盖在选区上（像 Snow Shot），
// 表格的格子能直接改，改完复制 / 导出；按住眼睛按钮能看一眼原图。
// 复制完就结束截图（和二维码卡片一样），导出前先结束截图，保存对话框才不会被遮罩挡住。

import { Eye, FileSearch, RotateCw, Sparkles, X } from 'lucide-react';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useElementSize } from '@/lib/hooks';
import { capture, system } from '@/lib/ipc';
import { Button, EmptyState, Spinner } from '@/ui/controls';
import { notify } from '@/ui/overlays';
import { canAskAi, FormulaView, TableView, type StructuredHooks, type StructuredState } from '@/views/ocr/StructuredResult';

import type { Rect } from './geometry';

const MIN_W = 380;
const MIN_H = 220;

export function StructuredCard({
  state,
  selection,
  s,
  viewport,
  onClose,
  onRetry,
  onDone,
}: {
  state: StructuredState;
  selection: Rect;
  s: number;
  viewport: { width: number; height: number };
  onClose: () => void;
  /** 再识别一次；`engine` = ai 时换 AI 看图 */
  onRetry: (engine?: 'local' | 'ai') => void;
  /** 复制完 / 要导出：结束这次截图 */
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  const size = useElementSize(ref, { width: MIN_W, height: MIN_H });
  const [peek, setPeek] = useState(false);
  // 盖在选区上：左上角对齐选区，至少 MIN_W × MIN_H；放不下就往左 / 往上挪
  const sel = { x: selection.x / s, y: selection.y / s, width: selection.width / s, height: selection.height / s };
  const width = Math.min(Math.max(sel.width, MIN_W), viewport.width - 16);
  const x = Math.max(8, Math.min(sel.x, viewport.width - size.width - 8));
  const y = Math.max(8, Math.min(sel.y, viewport.height - size.height - 8));
  // 不比选区高（表格在卡片里滚动），免得盖住选区下面的工具条；选区太矮时至少 MIN_H
  const maxHeight = Math.max(MIN_H, Math.min(viewport.height - 16, sel.height));

  const hooks: StructuredHooks = {
    onCopied: (message) => {
      onDone();
      void system.toast('success', message).catch(() => {});
    },
    beforeExport: onDone,
  };
  const copyText = (text: string) => void capture.writeText(text).then(() => hooks.onCopied?.(t('ocr.structured.copied'))).catch(notify.error);
  const title = state.kind === 'table' ? t('actions.table') : t('actions.formula');

  return (
    <div
      ref={ref}
      className="cap-struct cn-glass"
      data-peek={peek || undefined}
      style={{ transform: `translate(${x}px, ${y}px)`, width, maxHeight }}
      onPointerDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
    >
      <div className="cap-struct__head">
        <span className="cap-struct__title">{title}</span>
        {canAskAi(state) && (
          <button type="button" className="cap-struct__ai" title={t('ocr.structured.askAiTip')} onClick={() => onRetry('ai')}>
            <Sparkles size={12} strokeWidth={1.75} />
            {t('ocr.structured.askAi')}
          </button>
        )}
        {state.status === 'done' && (
          <button
            type="button"
            className="cap-codes__icon"
            aria-label={t('capture.structured.peek')}
            title={t('capture.structured.peek')}
            onPointerDown={() => setPeek(true)}
            onPointerUp={() => setPeek(false)}
            onPointerLeave={() => setPeek(false)}
          >
            <Eye size={14} strokeWidth={1.75} />
          </button>
        )}
        <button type="button" className="cap-codes__icon" aria-label={t('common.close')} onClick={onClose}>
          <X size={14} strokeWidth={1.75} />
        </button>
      </div>
      <div className="cap-struct__content ocr-struct">
        {state.status === 'running' ? (
          <div className="ocr-struct__loading">
            <Spinner size={16} />
            {state.kind === 'table' ? t('capture.structured.runningTable') : t('capture.structured.runningFormula')}
          </div>
        ) : state.status === 'error' ? (
          <EmptyState
            icon={FileSearch}
            title={t('ocr.structured.failed')}
            description={state.error ?? ''}
            action={
              <Button size="sm" icon={RotateCw} onClick={() => onRetry(state.engine)}>
                {t('common.retry')}
              </Button>
            }
          />
        ) : state.kind === 'table' ? (
          <TableView html={state.text ?? ''} ocrText={null} hooks={hooks} />
        ) : (
          <FormulaView text={state.text ?? ''} onCopy={copyText} />
        )}
      </div>
    </div>
  );
}
