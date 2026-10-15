// 预览里的图片：可以缩放、拖动看细节。
// - 一打开是"适应窗口"：大图缩到放得下，小图按原大小显示（不再硬拉满整个窗口）
// - 上下滚动（鼠标滚轮、触控板双指上下滑）：以鼠标位置为中心缩放。macOS 上分不出滚轮和触控板，统一当缩放；
//   同一帧里来的滚动先攒起来、每帧最多缩放一档，触控板的惯性滑动也不会一下冲到最大
// - 触控板捏合：缩放（macOS 的 WebKit 发 gesture 事件，Windows 的 WebView2 发带 Ctrl 的滚轮事件；捏合时两种都来的只认一种）
// - 能缩到比"适应窗口"更小；左右滑动、按住拖动：放大后挪位置；双击在"适应窗口"和放大之间切换
// - 底部一排按钮：缩小、百分比（点一下回到适应窗口）、放大；预览窗口里还能按 + / - / 0。
//   平时藏着，缩放时冒出来一会儿；鼠标移到底部那个位置也会出来（好点按钮）

import { Minus, Plus } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import './zoom.css';

interface Size {
  w: number;
  h: number;
}

/** scale：相对图片原始像素的倍数；null = 跟着窗口"适应窗口"。x、y：图片中心相对容器中心的偏移 */
interface View {
  scale: number | null;
  x: number;
  y: number;
}

/** WebKit 的捏合手势事件（不在标准 DOM 类型里） */
interface GestureEvent extends UIEvent {
  scale: number;
  clientX: number;
  clientY: number;
}

const STEP = 1.25;
/** 一帧里最多缩放的幅度（e 的指数）：约 ±1.35 倍 */
const MAX_FRAME = 0.3;

/** size：图片的原始像素尺寸，知道的话传进来（不用等图片加载完才知道怎么摆）；加载完以图片自己报的为准 */
export function ZoomImage({ src, size, keys = false }: { src: string | undefined; size?: Size; keys?: boolean }) {
  const { t } = useTranslation();
  const box = useRef<HTMLDivElement>(null);
  const [nat, setNat] = useState<Size | null>(size ?? null);
  const [stage, setStage] = useState<Size>({ w: 0, h: 0 });
  const [view, setViewState] = useState<View>({ scale: null, x: 0, y: 0 });
  /** 按钮、双击缩放时带一点过渡；滚轮、捏合跟手，不要过渡 */
  const [animate, setAnimate] = useState(false);

  // 换图时回到适应窗口（在渲染时就换，免得新图的 onLoad 被晚到的重置盖掉）
  const [shown, setShown] = useState(src);
  if (shown !== src) {
    setShown(src);
    setNat(size ?? null);
    setViewState({ scale: null, x: 0, y: 0 });
  }

  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setStage({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const fit = nat && stage.w > 0 ? Math.min(1, (stage.w - 24) / nat.w, (stage.h - 24) / nat.h) : 1;
  const minScale = fit * 0.2;
  const maxScale = Math.max(fit * 12, 4);
  const scale = view.scale ?? fit;

  // 事件回调里要用最新的值（触控板一帧里会来好几个事件，都要在上一个的基础上算）
  const live = useRef({ view, fit, minScale, maxScale, nat, stage });
  live.current = { ...live.current, fit, minScale, maxScale, nat, stage };
  const setView = useCallback((v: View) => {
    live.current.view = v;
    setViewState(v);
  }, []);
  useEffect(() => {
    live.current.view = view;
  }, [view]);

  /** 放大后不让图被拖出去：图比容器大时边缘最多拖到容器边上，比容器小时居中 */
  const clamp = useCallback((s: number, x: number, y: number): View => {
    const { nat: n, stage: st, fit: f } = live.current;
    if (!n) return { scale: null, x: 0, y: 0 };
    const lx = Math.max(0, (n.w * s - st.w) / 2);
    const ly = Math.max(0, (n.h * s - st.h) / 2);
    return {
      scale: Math.abs(s - f) < 0.001 ? null : s,
      x: Math.min(lx, Math.max(-lx, x)),
      y: Math.min(ly, Math.max(-ly, y)),
    };
  }, []);

  /** 以容器里的点 (cx, cy)（相对容器中心）为不动点缩放到 next */
  const zoomAt = useCallback(
    (next: number, cx: number, cy: number) => {
      const { view: v, fit: f, minScale: lo, maxScale: hi } = live.current;
      const cur = v.scale ?? f;
      let s = Math.min(hi, Math.max(lo, next));
      // 经过"适应窗口"时吸一下，方便回到原样
      if ((cur - f) * (s - f) < 0 || Math.abs(s - f) / f < 0.03) s = f;
      const k = s / cur;
      setView(clamp(s, cx - (cx - v.x) * k, cy - (cy - v.y) * k));
    },
    [clamp, setView],
  );

  const center = (clientX: number, clientY: number) => {
    const r = box.current?.getBoundingClientRect();
    if (!r) return { cx: 0, cy: 0 };
    return { cx: clientX - r.left - r.width / 2, cy: clientY - r.top - r.height / 2 };
  };

  /** 按钮、键盘：以中心缩放，带过渡 */
  const stepZoom = useCallback(
    (dir: 1 | -1 | 0) => {
      setAnimate(true);
      if (dir === 0) setView({ scale: null, x: 0, y: 0 });
      else {
        const { view: v, fit: f } = live.current;
        zoomAt((v.scale ?? f) * (dir > 0 ? STEP : 1 / STEP), 0, 0);
      }
    },
    [setView, zoomAt],
  );

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    let pending = 0;
    let anchor = { cx: 0, cy: 0 };
    let raf = 0;
    let gesturing = false;
    const flush = () => {
      raf = 0;
      const p = Math.min(MAX_FRAME, Math.max(-MAX_FRAME, pending));
      pending = 0;
      const { view: v, fit: f } = live.current;
      zoomAt((v.scale ?? f) * Math.exp(p), anchor.cx, anchor.cy);
    };
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (gesturing) return;
      setAnimate(false);
      const vertical = Math.abs(e.deltaY) >= Math.abs(e.deltaX);
      if (e.ctrlKey || vertical) {
        // 鼠标一格约 100，触控板一下几到几十；捏合（Ctrl+滚轮）的增量小、要灵敏些
        const unit = e.deltaMode === 1 ? 40 : 1;
        pending += -e.deltaY * unit * (e.ctrlKey ? 0.01 : 0.002);
        anchor = center(e.clientX, e.clientY);
        if (!raf) raf = requestAnimationFrame(flush);
      } else {
        const v = live.current.view;
        setView(clamp(v.scale ?? live.current.fit, v.x - e.deltaX, v.y));
      }
    };
    let start = 1;
    const onGestureStart = (e: Event) => {
      e.preventDefault();
      gesturing = true;
      setAnimate(false);
      start = live.current.view.scale ?? live.current.fit;
    };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      const g = e as GestureEvent;
      const { cx, cy } = center(g.clientX, g.clientY);
      zoomAt(start * g.scale, cx, cy);
    };
    const onGestureEnd = (e: Event) => {
      e.preventDefault();
      gesturing = false;
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    el.addEventListener('gesturestart', onGestureStart);
    el.addEventListener('gesturechange', onGestureChange);
    el.addEventListener('gestureend', onGestureEnd);
    return () => {
      cancelAnimationFrame(raf);
      el.removeEventListener('wheel', onWheel);
      el.removeEventListener('gesturestart', onGestureStart);
      el.removeEventListener('gesturechange', onGestureChange);
      el.removeEventListener('gestureend', onGestureEnd);
    };
  }, [zoomAt, setView, clamp]);

  useEffect(() => {
    if (!keys) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === '=' || e.key === '+') stepZoom(1);
      else if (e.key === '-' || e.key === '_') stepZoom(-1);
      else if (e.key === '0') stepZoom(0);
      else return;
      e.preventDefault();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [keys, stepZoom]);

  // 缩放条：倍数变了才出来，停下 1.2 秒后淡出；鼠标在它上面时一直显示
  const [barOn, setBarOn] = useState(false);
  const barTimer = useRef<number | undefined>(undefined);
  const barHover = useRef(false);
  const hideBarLater = useCallback(() => {
    window.clearTimeout(barTimer.current);
    barTimer.current = window.setTimeout(() => !barHover.current && setBarOn(false), 1200);
  }, []);
  const zoomed = useRef(view.scale);
  useEffect(() => {
    if (zoomed.current === view.scale) return;
    zoomed.current = view.scale;
    setBarOn(true);
    hideBarLater();
  }, [view.scale, hideBarLater]);
  useEffect(() => () => window.clearTimeout(barTimer.current), []);

  const drag = useRef<{ x: number; y: number; vx: number; vy: number } | null>(null);
  const pannable = !!nat && (nat.w * scale > stage.w + 1 || nat.h * scale > stage.h + 1);

  return (
    <div
      ref={box}
      className="zoom-image"
      data-zoomed={pannable}
      onPointerDown={(e) => {
        if (!pannable || e.button !== 0 || (e.target as HTMLElement).closest('.zoom-image__bar')) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        setAnimate(false);
        const v = live.current.view;
        drag.current = { x: e.clientX, y: e.clientY, vx: v.x, vy: v.y };
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d) return;
        setView(clamp(live.current.view.scale ?? live.current.fit, d.vx + e.clientX - d.x, d.vy + e.clientY - d.y));
      }}
      onPointerUp={() => (drag.current = null)}
      onPointerCancel={() => (drag.current = null)}
      onDoubleClick={(e) => {
        if ((e.target as HTMLElement).closest('.zoom-image__bar')) return;
        setAnimate(true);
        if (view.scale !== null) setView({ scale: null, x: 0, y: 0 });
        else {
          const { cx, cy } = center(e.clientX, e.clientY);
          zoomAt(Math.max(1, fit * 2.5), cx, cy);
        }
      }}
    >
      <img
        src={src}
        alt=""
        draggable={false}
        className={animate ? 'zoom-image__img--animate' : undefined}
        onLoad={(e) => {
          const { naturalWidth: w, naturalHeight: h } = e.currentTarget;
          if (w && h && (w !== nat?.w || h !== nat?.h)) setNat({ w, h });
        }}
        onTransitionEnd={() => setAnimate(false)}
        style={
          nat
            ? {
                width: nat.w,
                height: nat.h,
                transform: `translate(calc(-50% + ${view.x}px), calc(-50% + ${view.y}px)) scale(${scale})`,
              }
            : // 还不知道尺寸：先按窗口大小摆在中间，加载完再换成能缩放的摆法
              { maxWidth: '100%', maxHeight: '100%', transform: 'translate(-50%, -50%)' }
        }
      />
      <div
        className="zoom-image__bar"
        data-on={barOn}
        onClick={(e) => e.stopPropagation()}
        onPointerEnter={() => {
          barHover.current = true;
          window.clearTimeout(barTimer.current);
          setBarOn(true);
        }}
        onPointerLeave={() => {
          barHover.current = false;
          hideBarLater();
        }}
      >
        <button type="button" aria-label={t('clip.zoomOut')} title={t('clip.zoomOut')} disabled={scale <= minScale + 1e-6} onClick={() => stepZoom(-1)}>
          <Minus size={14} strokeWidth={2.2} />
        </button>
        <button type="button" className="zoom-image__pct" title={t('clip.zoomFit')} onClick={() => stepZoom(0)}>
          {Math.round(scale * 100)}%
        </button>
        <button type="button" aria-label={t('clip.zoomIn')} title={t('clip.zoomIn')} disabled={scale >= maxScale - 1e-6} onClick={() => stepZoom(1)}>
          <Plus size={14} strokeWidth={2.2} />
        </button>
      </div>
    </div>
  );
}
