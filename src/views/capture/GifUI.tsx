// GIF 录制中的界面：选区外面一圈红框 + 选区下方的控制条（录制计时、取消、完成）。
//
// 遮罩此时是鼠标穿透的（用户要在选区里正常操作）。控制条的屏幕区域报给 Rust，
// 鼠标移上去时临时关掉穿透，按钮才点得到。红框画在选区外面，不会被录进去。

import { Check, X } from 'lucide-react';
import { useLayoutEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { useElementSize } from '@/lib/hooks';
import { gif } from '@/lib/ipc';

import { bottom, type Rect } from './geometry';
import { useOverlay } from './store';

const mmss = (ms: number) => {
  const s = Math.floor(ms / 1000);
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
};

export function GifUI({ rect, s, viewport }: { rect: Rect; s: number; viewport: { width: number; height: number } }) {
  const { t } = useTranslation();
  const progress = useOverlay((x) => x.gif);
  const barRef = useRef<HTMLDivElement>(null);
  const barSize = useElementSize(barRef, { width: 300, height: 44 });

  const css = { x: rect.x / s, y: rect.y / s, width: rect.width / s, height: rect.height / s };
  // 控制条放在选区下面，放不下放上面，再放不下才放进选区里（那样会被录进去，只是兜底）
  let barY = bottom(rect) / s + 10;
  if (barY + barSize.height > viewport.height) barY = css.y - 10 - barSize.height;
  if (barY < 0) barY = css.y + css.height - barSize.height - 10;
  const barX = Math.max(8, Math.min(css.x + css.width / 2 - barSize.width / 2, viewport.width - barSize.width - 8));

  useLayoutEffect(() => {
    const el = barRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    void gif.setRegions([{ x: Math.floor(r.left * s), y: Math.floor(r.top * s), width: Math.ceil(r.width * s), height: Math.ceil(r.height * s) }]);
  }, [barX, barY, barSize, s]);

  const elapsed = progress?.elapsedMs ?? 0;
  const max = progress?.maxMs ?? 60_000;
  const nearEnd = max - elapsed < 10_000;

  return (
    <>
      <div className="gif-frame" style={{ left: css.x - 3, top: css.y - 3, width: css.width + 6, height: css.height + 6 }} />
      <div ref={barRef} className="gif-bar cn-glass" style={{ transform: `translate(${barX}px, ${barY}px)` }}>
        <span className="gif-bar__dot" />
        <span className="gif-bar__label">{t('gif.recording')}</span>
        <span className={nearEnd ? 'gif-bar__time cn-numeric is-warn' : 'gif-bar__time cn-numeric'}>
          {mmss(elapsed)}
          <small> / {mmss(max)}</small>
        </span>
        <span className="gif-bar__actions">
          <button type="button" className="ls-btn" onClick={() => void gif.cancel()} title={t('common.cancel')}>
            <X size={14} strokeWidth={1.5} />
          </button>
          <button type="button" className="ls-btn ls-btn--primary" onClick={() => void gif.finish()}>
            <Check size={14} strokeWidth={2} />
            {t('gif.done')}
          </button>
        </span>
      </div>
    </>
  );
}
