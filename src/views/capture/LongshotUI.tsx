// 长截图采集态界面（规格 03 §2）：主色流动虚线框 + 选区下方的操作提示条 + 屏幕右侧的拼接预览条。
//
// 遮罩此时是鼠标穿透的（用户要滚动下面的页面）。提示条和预览条的屏幕区域报给 Rust，
// 鼠标移进去时临时关掉穿透，按钮才点得到。

import clsx from 'clsx';
import { AlertTriangle, ArrowUpDown, Check, CheckCircle2, Undo2, X } from 'lucide-react';
import { useLayoutEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { useElementSize } from '@/lib/hooks';
import { longshot } from '@/lib/ipc';
import { shotUrl } from '@/lib/platform';

import { bottom, type Rect } from './geometry';
import { useOverlay } from './store';

export function LongshotUI({ rect, s, viewport }: { rect: Rect; s: number; viewport: { width: number; height: number } }) {
  const { t } = useTranslation();
  const progress = useOverlay((x) => x.longshot);
  const barRef = useRef<HTMLDivElement>(null);
  const previewRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const barSize = useElementSize(barRef, { width: 460, height: 44 });

  const css = { x: rect.x / s, y: rect.y / s, width: rect.width / s, height: rect.height / s };
  const below = (bottom(rect) / s) + 8;
  let barY = below;
  if (barY + barSize.height > viewport.height) barY = css.y - 8 - barSize.height;
  if (barY < 0) barY = css.y + css.height - barSize.height - 8;
  const barX = Math.max(8, Math.min(css.x + css.width / 2 - barSize.width / 2, viewport.width - barSize.width - 8));

  // 预览条贴屏幕右边缘；和选区重叠就挪到左边
  const previewWidth = 120;
  const onRight = css.x + css.width < viewport.width - previewWidth - 32;
  const previewX = onRight ? viewport.width - previewWidth - 16 : 16;

  const hasPreview = !!progress && progress.previewVersion > 0;
  useLayoutEffect(() => {
    const regions = [barRef.current, previewRef.current]
      .filter((el): el is HTMLDivElement => !!el)
      .map((el) => {
        const r = el.getBoundingClientRect();
        return { x: Math.floor(r.left * s), y: Math.floor(r.top * s), width: Math.ceil(r.width * s), height: Math.ceil(r.height * s) };
      });
    void longshot.setRegions(regions);
  }, [barX, barY, barSize, previewX, hasPreview, s]);

  // 预览条始终滚到最近一次接缝附近
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el || progress?.seam == null) return;
    el.scrollTop = Math.max(0, progress.seam / s - el.clientHeight * 0.6);
  }, [progress?.previewVersion, progress?.seam, s]);

  const status = progress?.status ?? 'first';
  const frames = progress?.frames ?? 0;
  let tone: 'normal' | 'warn' | 'ok' = 'normal';
  let message: string;
  if (status === 'failed') {
    tone = 'warn';
    message = progress && progress.failures > 2 ? t('longshot.failedDynamic') : t('longshot.failed');
  } else if (status === 'bottom' && frames > 1) {
    tone = 'ok';
    message = t('longshot.bottom');
  } else if (status === 'toolong') {
    tone = 'warn';
    message = t('longshot.tooLong', { height: progress?.height ?? 0 });
  } else if (frames <= 1) {
    message = t('longshot.start');
  } else {
    message = t('longshot.collecting', { frames, height: progress?.height ?? 0 });
  }
  const Icon = tone === 'warn' ? AlertTriangle : tone === 'ok' ? CheckCircle2 : ArrowUpDown;

  return (
    <>
      <svg className="ls-ants" style={{ left: css.x - 2, top: css.y - 2, width: css.width + 4, height: css.height + 4 }}>
        <rect x="1" y="1" width={css.width + 2} height={css.height + 2} />
      </svg>

      <div ref={barRef} className={clsx('ls-bar cn-glass', `ls-bar--${tone}`)} style={{ transform: `translate(${barX}px, ${barY}px)` }}>
        <Icon size={16} strokeWidth={1.5} />
        <span className="ls-bar__msg">{message}</span>
        <span className="ls-bar__actions">
          <button type="button" className="ls-btn" disabled={frames <= 1} onClick={() => void longshot.undo()} title={t('longshot.undo')}>
            <Undo2 size={14} strokeWidth={1.5} />
            <kbd>⌫</kbd>
          </button>
          <button type="button" className="ls-btn" onClick={() => void longshot.abort()} title={t('common.cancel')}>
            <X size={14} strokeWidth={1.5} />
            <kbd>Esc</kbd>
          </button>
          <button type="button" className="ls-btn ls-btn--primary" onClick={() => void longshot.finish()}>
            <Check size={14} strokeWidth={2} />
            {t('longshot.done')}
          </button>
        </span>
      </div>

      {progress && progress.previewVersion > 0 && (
        <div ref={previewRef} className="ls-preview cn-glass" style={{ left: previewX }}>
          <div ref={scrollRef} className="ls-preview__scroll">
            <div className="ls-preview__img">
              <img src={shotUrl('longshot-preview', progress.previewVersion)} alt="" draggable={false} />
              {progress.seam != null && <span className="ls-preview__seam" style={{ top: progress.seam / s }} />}
            </div>
          </div>
          <div className="ls-preview__meta cn-numeric">
            {t('longshot.meta', { frames, width: progress.width, height: progress.height })}
          </div>
        </div>
      )}
    </>
  );
}
