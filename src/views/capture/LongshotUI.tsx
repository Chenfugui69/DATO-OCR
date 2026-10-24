// 长截图采集态界面（规格 03 §2）：主色流动虚线框 + 选区下方的操作提示条 + 选区右上角外侧的拼接预览条。
//
// 预览条只往下长：顶边和选区顶边对齐，高度跟着拼出来的长度平滑变高（以前上下居中，每拼一截整条往上跳一下）。
// 新拼好的图先在后台解码好再换上去，换的时候不闪；滚到最新接缝也是平滑地滚过去。
//
// 遮罩此时是鼠标穿透的（用户要滚动下面的页面）。提示条和预览条的屏幕区域报给 Rust，
// 鼠标移进去时临时关掉穿透，按钮才点得到。

import clsx from 'clsx';
import { AlertTriangle, ArrowUpDown, Check, CheckCircle2, Undo2, X } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
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

  // 预览条放在选区右上角外侧；右边放不下放左边，两边都放不下就贴在选区里面的右上角
  const previewWidth = 120;
  const gap = 12;
  const previewX =
    css.x + css.width + gap + previewWidth <= viewport.width - 8
      ? css.x + css.width + gap
      : css.x - gap - previewWidth >= 8
        ? css.x - gap - previewWidth
        : Math.max(8, css.x + css.width - previewWidth - 8);
  const previewY = Math.max(8, css.y);
  // 图区最高多少（下面还有一行尺寸信息）
  const viewMaxH = Math.max(120, viewport.height - previewY - 16 - 32);

  // 先在后台把新图解码好再换上（直接换 src 会先白一下）
  const [shown, setShown] = useState<{ version: number; url: string; w: number; h: number } | null>(null);
  const version = progress?.previewVersion ?? 0;
  useEffect(() => {
    if (!version) {
      setShown(null);
      return;
    }
    const url = shotUrl('longshot-preview', version);
    const img = new Image();
    img.src = url;
    let stale = false;
    void img
      .decode()
      .catch(() => undefined)
      .then(() => !stale && img.naturalWidth > 0 && setShown({ version, url, w: img.naturalWidth, h: img.naturalHeight }));
    return () => {
      stale = true;
    };
  }, [version]);
  // 预览图在条里显示的高度、预览像素 → CSS 像素的比例
  const k = shown ? previewWidth / shown.w : 0;
  const imgH = shown ? shown.h * k : 0;
  const viewH = Math.min(viewMaxH, imgH);

  // 图区高度是过渡着长的：长完再报一次区域（过渡开始时量到的还是旧高度）
  const [grown, setGrown] = useState(0);
  const hasPreview = !!shown;
  useLayoutEffect(() => {
    const regions = [barRef.current, previewRef.current]
      .filter((el): el is HTMLDivElement => !!el)
      .map((el) => {
        const r = el.getBoundingClientRect();
        return { x: Math.floor(r.left * s), y: Math.floor(r.top * s), width: Math.ceil(r.width * s), height: Math.ceil(r.height * s) };
      });
    void longshot.setRegions(regions);
  }, [barX, barY, barSize, previewX, previewY, viewH, grown, hasPreview, s]);

  // 预览条始终滚到最近一次接缝附近（平滑地滚过去）
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || progress?.seam == null || !k) return;
    el.scrollTo({ top: Math.max(0, progress.seam * k - viewH * 0.6), behavior: 'smooth' });
  }, [shown?.version, progress?.seam, k, viewH]);

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

      {progress && shown && (
        <div ref={previewRef} className="ls-preview cn-glass" style={{ left: previewX, top: previewY }}>
          <div
            ref={scrollRef}
            className="ls-preview__scroll"
            style={{ height: viewH }}
            onTransitionEnd={(e) => e.target === e.currentTarget && setGrown((n) => n + 1)}
          >
            <div className="ls-preview__img" style={{ height: imgH }}>
              <img src={shown.url} alt="" draggable={false} />
              {progress.seam != null && <span className="ls-preview__seam" style={{ top: progress.seam * k }} />}
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
