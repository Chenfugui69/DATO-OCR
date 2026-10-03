// 可缩放、可拖动的图片查看器（滚轮缩放、拖动平移、双击复原），上面叠一层文字区域框。

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';

export interface ViewerBox {
  x: number;
  y: number;
  w: number;
  h: number;
}

export function ImageViewer({
  src,
  width,
  height,
  boxes,
  highlight,
  onHover,
  children,
}: {
  src: string | undefined;
  /** 图片物理像素尺寸 */
  width: number;
  height: number;
  boxes: ViewerBox[];
  highlight: number | null;
  onHover: (index: number | null) => void;
  children?: ReactNode;
}) {
  const wrap = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ scale: 1, x: 0, y: 0 });
  const fitted = useRef(false);
  const dragging = useRef<{ x: number; y: number; vx: number; vy: number } | null>(null);

  const fit = useCallback(() => {
    const el = wrap.current;
    if (!el || !width || !height) return;
    const pad = 24;
    const s = Math.min((el.clientWidth - pad * 2) / width, (el.clientHeight - pad * 2) / height, 1 / window.devicePixelRatio);
    const scale = Math.max(s, 0.02);
    setView({ scale, x: (el.clientWidth - width * scale) / 2, y: (el.clientHeight - height * scale) / 2 });
  }, [width, height]);

  useLayoutEffect(() => {
    fitted.current = false;
    fit();
    fitted.current = true;
  }, [fit, src]);

  useEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const ro = new ResizeObserver(() => fit());
    ro.observe(el);
    return () => ro.disconnect();
  }, [fit]);

  const onWheel = (e: React.WheelEvent) => {
    const el = wrap.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    setView((v) => {
      const next = Math.min(8, Math.max(0.02, v.scale * (e.deltaY < 0 ? 1.15 : 1 / 1.15)));
      // 以鼠标位置为锚点缩放
      return { scale: next, x: mx - ((mx - v.x) * next) / v.scale, y: my - ((my - v.y) * next) / v.scale };
    });
  };

  return (
    <div
      ref={wrap}
      className="viewer"
      onWheel={onWheel}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        (e.target as Element).setPointerCapture(e.pointerId);
        dragging.current = { x: e.clientX, y: e.clientY, vx: view.x, vy: view.y };
      }}
      onPointerMove={(e) => {
        const d = dragging.current;
        if (d) setView((v) => ({ ...v, x: d.vx + e.clientX - d.x, y: d.vy + e.clientY - d.y }));
      }}
      onPointerUp={() => (dragging.current = null)}
      onDoubleClick={fit}
      onPointerLeave={() => onHover(null)}
    >
      <div
        className="viewer__stage"
        style={{ width, height, transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})`, transformOrigin: '0 0' }}
      >
        {src && <img src={src} width={width} height={height} alt="" draggable={false} style={{ imageRendering: view.scale * window.devicePixelRatio > 2 ? 'pixelated' : 'auto' }} />}
        <svg className="viewer__boxes" viewBox={`0 0 ${width} ${height}`} width={width} height={height}>
          {boxes.map((b, i) => (
            <rect
              key={i}
              x={b.x - 2}
              y={b.y - 2}
              width={b.w + 4}
              height={b.h + 4}
              rx={3}
              className={i === highlight ? 'viewer__box viewer__box--active' : 'viewer__box'}
              style={{ strokeWidth: 1 / view.scale }}
              onPointerEnter={() => onHover(i)}
            />
          ))}
        </svg>
        {children}
      </div>
    </div>
  );
}
