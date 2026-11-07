// 长截图采集态界面（规格 03 §2）：主色流动虚线框 + 选区下方的操作提示条 + 选区旁边的拼接预览条。
//
// 预览条跟着滚动方向长：往下滚，顶边不动、往下长；往上滚（内容拼在长图顶上），底边不动、往上长。
// 碰到屏幕上下边就整条往回挪；上下都放不下了，整张预览等比缩小（变窄），一直能看到完整的长图，
// 缩到最窄还放不下才改成在条里滚动、跟着最新接缝。
// 新拼好的图先在后台解码好再换上去，换的时候不闪。
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

/** 预览图默认宽、缩到最窄多宽（CSS 像素） */
const PREVIEW_W = 120;
const PREVIEW_MIN_W = 48;
/** 预览条四周的留白、图下面那行尺寸信息连同留白有多高（和 capture.css 的 .ls-preview 一致） */
const PREVIEW_PAD = 6;
const PREVIEW_CHROME = PREVIEW_PAD + 28;
/** 离屏幕边、离选区多远 */
const EDGE = 8;
const GAP = 12;

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
  // 预览条的大小：默认 120 宽；整张长图竖着放不下就等比缩窄，最窄 PREVIEW_MIN_W，再放不下就在条里滚动
  const availH = Math.max(80, viewport.height - EDGE * 2 - PREVIEW_CHROME);
  const baseK = shown ? PREVIEW_W / shown.w : 0;
  const k = shown ? (shown.h * baseK > availH ? Math.max(PREVIEW_MIN_W / shown.w, availH / shown.h) : baseK) : 0;
  const previewWidth = shown ? Math.round(shown.w * k) : PREVIEW_W;
  const imgH = shown ? shown.h * k : 0;
  const viewH = Math.min(availH, imgH);
  const boxH = viewH + PREVIEW_CHROME;
  const boxW = previewWidth + PREVIEW_PAD * 2;

  // 横向：放在选区右边外侧；右边放不下放左边，两边都放不下就贴在选区里面的右边
  const previewX =
    css.x + css.width + GAP + boxW <= viewport.width - EDGE
      ? css.x + css.width + GAP
      : css.x - GAP - boxW >= EDGE
        ? css.x - GAP - boxW
        : Math.max(EDGE, css.x + css.width - boxW - EDGE);
  // 纵向：第一次和选区顶边对齐；之后往下长时顶边不动，往上长时底边不动，碰到屏幕边就往回挪
  const anchor = useRef<{ top: number; h: number } | null>(null);
  const maxTop = Math.max(EDGE, viewport.height - EDGE - boxH);
  const clampTop = (y: number) => Math.min(maxTop, Math.max(EDGE, y));
  const prev = anchor.current;
  const previewY = !prev ? clampTop(css.y) : progress?.grewUp ? clampTop(prev.top + prev.h - boxH) : clampTop(prev.top);
  useLayoutEffect(() => {
    anchor.current = shown ? { top: previewY, h: boxH } : null;
  }, [shown, previewY, boxH]);

  // 图区高度、位置是过渡着变的：变完再报一次区域（过渡开始时量到的还是旧的）
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
  }, [barX, barY, barSize, previewX, previewY, previewWidth, viewH, grown, hasPreview, s]);

  // 整张放得下时不用滚；缩到最窄还放不下才滚到最近一次接缝附近（平滑地滚过去）
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || progress?.seam == null || !k || imgH <= viewH) return;
    el.scrollTo({ top: Math.max(0, progress.seam * k - viewH * 0.6), behavior: 'smooth' });
  }, [shown?.version, progress?.seam, k, viewH, imgH]);

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
        <div
          ref={previewRef}
          className="ls-preview cn-glass"
          style={{ left: previewX, top: previewY, width: boxW }}
          onTransitionEnd={(e) => e.target === e.currentTarget && setGrown((n) => n + 1)}
        >
          <div ref={scrollRef} className="ls-preview__scroll" style={{ height: viewH }}>
            <div className="ls-preview__img" style={{ height: imgH }}>
              <img src={shown.url} alt="" draggable={false} style={{ width: previewWidth }} />
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
