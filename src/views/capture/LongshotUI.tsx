// 长截图采集态界面（规格 03 §2）：主色流动虚线框 + 选区下方的操作提示条 + 选区右上角外侧的拼接预览条。
//
// 预览条跟着拼的方向长（同微信）：往下滚时顶边和选区顶边对齐、往下长；往上滚时底边和选区底边对齐、往上长，
// 高度跟着拼出来的长度平滑变高（以前上下居中，每拼一截整条往上跳一下）。
// 预览条始终显示整张长图（同微信）：放不下时整张等比缩小、条跟着变窄，不滚动、也不会跑出屏幕。
// 逐帧跟手：后端每帧报视口伸出长图多少（longshot-live），还没并入的那段先用当前画面的缩略图顶着，
// 预览条每帧都长一点；并入后换成新的预览图，总长度不变，看不出接缝。
// 抓屏一秒几十帧，屏幕刷新是 60～165Hz：显示的长度按刷新率平滑地追上实际长度（没追上的那点先裁掉），
// 不然每来一帧跳一截。
// 新拼好的图、当前画面都先在后台解码好再换上去，换的时候不闪。
//
// 遮罩此时是鼠标穿透的（用户要滚动下面的页面）。提示条和预览条的屏幕区域报给 Rust，
// 鼠标移进去时临时关掉穿透，按钮才点得到。

import clsx from 'clsx';
import { AlertTriangle, ArrowUpDown, Check, CheckCircle2, Undo2, X } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useElementSize } from '@/lib/hooks';
import { useEvent } from '@/lib/events';
import { longshot } from '@/lib/ipc';
import { shotUrl } from '@/lib/platform';

import { bottom, type Rect } from './geometry';
import { useOverlay } from './store';

/** 预览条四周的留白、图下面那行尺寸信息连同留白有多高（和 capture.css 的 .ls-preview 一致） */
const PREVIEW_PAD = 6;
const PREVIEW_CHROME = PREVIEW_PAD + 28;

export function LongshotUI({ rect, s, viewport }: { rect: Rect; s: number; viewport: { width: number; height: number } }) {
  const { t } = useTranslation();
  const progress = useOverlay((x) => x.longshot);
  const barRef = useRef<HTMLDivElement>(null);
  const previewRef = useRef<HTMLDivElement>(null);
  const barSize = useElementSize(barRef, { width: 460, height: 44 });

  const css = { x: rect.x / s, y: rect.y / s, width: rect.width / s, height: rect.height / s };
  const below = (bottom(rect) / s) + 8;
  let barY = below;
  if (barY + barSize.height > viewport.height) barY = css.y - 8 - barSize.height;
  if (barY < 0) barY = css.y + css.height - barSize.height - 8;
  const barX0 = Math.max(8, Math.min(css.x + css.width / 2 - barSize.width / 2, viewport.width - barSize.width - 8));

  const gap = 12;
  const previewY = Math.max(8, css.y);
  // 往上长时底边对齐选区底边（用 bottom 定位，长的时候底边不动）
  const previewBottom = Math.max(8, viewport.height - (css.y + css.height));

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

  // 逐帧的伸出量和当前画面缩略图：同样解码好再用；晚到的旧帧丢掉
  const [live, setLive] = useState<{ previewVersion: number; liveVersion: number; up: number; down: number; url: string | null; h: number } | null>(null);
  const liveSeq = useRef(0);
  useEvent('longshot-live', (p) => {
    if (p.liveVersion <= liveSeq.current) return;
    liveSeq.current = p.liveVersion;
    if (p.up <= 0 && p.down <= 0) {
      setLive({ ...p, url: null, h: 0 });
      return;
    }
    const url = shotUrl('longshot-live', p.liveVersion);
    const img = new Image();
    img.src = url;
    void img
      .decode()
      .catch(() => undefined)
      .then(() => {
        if (img.naturalWidth > 0) setLive((cur) => (cur && cur.liveVersion > p.liveVersion ? cur : { ...p, url, h: img.naturalHeight }));
      });
  });
  // 只认基于当前显示的那版预览图算的伸出量（新预览图一换上，旧的伸出量就作废了）
  const ext = live && shown && live.previewVersion === shown.version ? live : null;
  const extUp = ext?.up ?? 0;
  const extDown = ext?.down ?? 0;
  // 往哪边长：正往上伸出、或最近一次是往上拼的，底边不动往上长
  const up = extUp > 0 || (extDown <= 0 && !!progress?.up);

  // 图区最宽、最高多少（下面还有一行尺寸信息）
  const maxW = 180;
  const maxH = Math.max(120, (up ? viewport.height - previewBottom - 8 : viewport.height - previewY - 16) - PREVIEW_CHROME);
  // "虚拟长图"（已拼的 + 伸出的）的实际长度，和平滑追着它的显示长度
  const totalH = shown ? shown.h + extUp + extDown : 0;
  const shownH = useChase(totalH);
  // 按显示长度等比塞进 maxW × maxH：越长越窄
  const k = shown ? Math.min(maxW / shown.w, maxH / Math.max(1, shownH)) : 0;
  const previewWidth = shown ? Math.round(shown.w * k) : maxW;
  const boxW = previewWidth + PREVIEW_PAD * 2;
  const viewH = shownH * k;
  // 还没追上的那截先不露：往下长裁掉底部，往上长裁掉顶部
  const hidden = Math.max(0, totalH - shownH);
  // 预览条放在选区右上角外侧；右边放不下放左边，两边都放不下就贴在选区里面的右上角
  const previewX =
    css.x + css.width + gap + maxW + PREVIEW_PAD * 2 <= viewport.width - 8
      ? css.x + css.width + gap
      : css.x - gap - boxW >= 8
        ? css.x - gap - boxW
        : Math.max(8, css.x + css.width - boxW - 8);

  // 提示条别钻到预览条底下：预览在右边就把提示条往左推，在左边就往右推
  const previewRight = previewX > css.x;
  const barX = !shown
    ? barX0
    : previewRight
      ? Math.max(8, Math.min(barX0, previewX - 8 - barSize.width))
      : Math.min(viewport.width - barSize.width - 8, Math.max(barX0, previewX + boxW + 8));

  const hasPreview = !!shown;
  useLayoutEffect(() => {
    const regions = [barRef.current, previewRef.current]
      .filter((el): el is HTMLDivElement => !!el)
      .map((el) => {
        const r = el.getBoundingClientRect();
        return { x: Math.floor(r.left * s), y: Math.floor(r.top * s), width: Math.ceil(r.width * s), height: Math.ceil(r.height * s) };
      });
    void longshot.setRegions(regions);
  }, [barX, barY, barSize, previewX, previewY, previewBottom, up, previewWidth, viewH, hasPreview, s]);

  // 预览图切成几段摆：头部 | 往上伸出 | 中间 | 往下伸出 | 尾部（伸出的两段用当前画面顶着）
  const hd = Math.min(progress?.header ?? 0, shown?.h ?? 0);
  const ft = Math.min(progress?.footer ?? 0, (shown?.h ?? 0) - hd);
  const strips: { key: string; y: number; h: number; url: string; srcY: number; srcH: number }[] = [];
  if (shown) {
    const midEnd = shown.h - ft;
    if (hd > 0) strips.push({ key: 'head', y: 0, h: hd, url: shown.url, srcY: 0, srcH: shown.h });
    if (ext?.url && extUp > 0) strips.push({ key: 'up', y: hd, h: extUp, url: ext.url, srcY: 0, srcH: ext.h });
    strips.push({ key: 'mid', y: hd + extUp, h: midEnd - hd, url: shown.url, srcY: hd, srcH: shown.h });
    if (ext?.url && extDown > 0) strips.push({ key: 'down', y: midEnd + extUp, h: extDown, url: ext.url, srcY: ext.h - extDown, srcH: ext.h });
    if (ft > 0) strips.push({ key: 'foot', y: midEnd + extUp + extDown, h: ft, url: shown.url, srcY: midEnd, srcH: shown.h });
  }

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
      {/* 框线画在选区外面、离边留 1px：遮罩可能被抓屏看到（远程），框线不能被拍进长图 */}
      <svg className="ls-ants" style={{ left: css.x - 3, top: css.y - 3, width: css.width + 6, height: css.height + 6 }}>
        <rect x="1" y="1" width={css.width + 4} height={css.height + 4} />
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
        <div ref={previewRef} className="ls-preview cn-glass" style={up ? { left: previewX, bottom: previewBottom, width: boxW } : { left: previewX, top: previewY, width: boxW }}>
          <div className="ls-preview__img" style={{ width: previewWidth, height: viewH }}>
            <div className="ls-preview__track" style={{ height: totalH * k, transform: up ? `translateY(${-hidden * k}px)` : undefined }}>
            {strips.map((p) => (
              <div key={p.key} className="ls-preview__strip" style={{ top: p.y * k, height: p.h * k }}>
                <img src={p.url} alt="" draggable={false} style={{ height: p.srcH * k, transform: `translateY(${-p.srcY * k}px)` }} />
              </div>
            ))}
            {progress.seam != null && <span className="ls-preview__seam" style={{ top: (progress.seam + extUp) * k }} />}
            </div>
          </div>
          {previewWidth >= 120 && (
            <div className="ls-preview__meta cn-numeric">
              {t('longshot.meta', { frames, width: progress.width, height: progress.height })}
            </div>
          )}
        </div>
      )}
    </>
  );
}

/** 显示值按屏幕刷新率平滑地追目标值（指数趋近，约 60ms 追上大半）。第一次直接取目标值，变小时直接跟（撤销）。 */
function useChase(target: number): number {
  const [value, setValue] = useState(target);
  const cur = useRef(target);
  useEffect(() => {
    if (cur.current === 0 || target <= cur.current) {
      cur.current = target;
      setValue(target);
      return;
    }
    let raf = 0;
    let last = performance.now();
    const step = (now: number) => {
      const dt = now - last;
      last = now;
      cur.current += (target - cur.current) * (1 - Math.exp(-dt / 45));
      if (target - cur.current < 0.5) cur.current = target;
      setValue(cur.current);
      if (cur.current !== target) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [target]);
  return value;
}
