// 截图里识别出来的二维码 / 条码：码的位置上画个框，选区旁边一张卡片列出内容，能复制、是网址的能打开。
// 复制、打开都算用完了这次截图：先关掉截图，不然打开的网页（视频之类）在截图遮罩后面跑起来了。

import { Copy, ExternalLink, X } from 'lucide-react';
import { useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { useElementSize } from '@/lib/hooks';
import { capture, system } from '@/lib/ipc';
import type { ScannedCode } from '@/lib/types';
import { notify } from '@/ui/overlays';

import type { Rect } from './geometry';

const isUrl = (text: string) => /^https?:\/\/\S+$/i.test(text.trim());

export function CodeResults({
  codes,
  selection,
  s,
  viewport,
  onClose,
  onDone,
}: {
  codes: ScannedCode[];
  selection: Rect;
  s: number;
  viewport: { width: number; height: number };
  onClose: () => void;
  /** 复制 / 打开之后：结束这次截图 */
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  const size = useElementSize(ref, { width: 300, height: 120 });
  // 卡片放在选区右边；右边放不下放左边，再放不下就贴着选区里面的右上角
  const sel = { x: selection.x / s, y: selection.y / s, width: selection.width / s, height: selection.height / s };
  const gap = 10;
  let x = sel.x + sel.width + gap;
  if (x + size.width > viewport.width - 8) x = sel.x - gap - size.width;
  if (x < 8) x = Math.max(8, sel.x + sel.width - size.width - 8);
  const y = Math.max(8, Math.min(sel.y, viewport.height - size.height - 8));

  const copy = (text: string) =>
    void capture
      .writeText(text)
      .then(() => {
        onDone();
        void system.toast('success', t('capture.codeCopied')).catch(() => {});
      })
      .catch(notify.error);
  const open = (url: string) => {
    onDone();
    void system.openUrl(url).catch((err: unknown) => void system.toast('error', err instanceof Error ? err.message : String(err)).catch(() => {}));
  };

  return (
    <>
      {codes.map((c, i) => (
        <span
          key={`box-${i}`}
          className="cap-code-box"
          style={{ left: c.x / s - 3, top: c.y / s - 3, width: c.width / s + 6, height: c.height / s + 6 }}
        />
      ))}
      <div
        ref={ref}
        className="cap-codes cn-glass"
        style={{ transform: `translate(${x}px, ${y}px)` }}
        onPointerDown={(e) => e.stopPropagation()}
      >
        <div className="cap-codes__head">
          <span>{t('capture.codesFound', { count: codes.length })}</span>
          <button type="button" className="cap-codes__icon" aria-label={t('common.close')} onClick={onClose}>
            <X size={13} strokeWidth={1.75} />
          </button>
        </div>
        {codes.map((c, i) => (
          <div key={i} className="cap-codes__item">
            <span className="cap-codes__kind">{c.kind}</span>
            <span className="cap-codes__text" title={c.text}>
              {c.text}
            </span>
            <div className="cap-codes__actions">
              <button type="button" className="cap-codes__btn" onClick={() => copy(c.text)}>
                <Copy size={12} strokeWidth={1.75} />
                {t('capture.copyCode')}
              </button>
              {isUrl(c.text) && (
                <button type="button" className="cap-codes__btn" onClick={() => open(c.text.trim())}>
                  <ExternalLink size={12} strokeWidth={1.75} />
                  {t('capture.openLink')}
                </button>
              )}
            </div>
          </div>
        ))}
      </div>
    </>
  );
}
