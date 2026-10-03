// 横向卡片条的滚动条：系统滚动条又粗又灰、带箭头，和卡片放在一起很扎眼，换成一根悬浮的细胶囊。
// 平时隐藏，滚动或鼠标移到面板上才出现；可以拖，也可以点轨道跳过去。

import { useEffect, useRef, useState, type RefObject } from 'react';

export function HScrollbar({ target, watch }: { target: RefObject<HTMLElement | null>; watch: unknown }) {
  const trackRef = useRef<HTMLDivElement>(null);
  const [thumb, setThumb] = useState({ left: 0, width: 0 });
  const [active, setActive] = useState(false);
  const hideTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    const el = target.current;
    if (!el) return;
    const update = (show: boolean) => {
      const track = trackRef.current?.clientWidth ?? 0;
      const { scrollLeft, scrollWidth, clientWidth } = el;
      if (scrollWidth <= clientWidth + 1 || track === 0) {
        setThumb({ left: 0, width: 0 });
        return;
      }
      const width = Math.max(48, (track * clientWidth) / scrollWidth);
      setThumb({ left: ((track - width) * scrollLeft) / (scrollWidth - clientWidth), width });
      if (show) {
        setActive(true);
        window.clearTimeout(hideTimer.current);
        hideTimer.current = window.setTimeout(() => setActive(false), 900);
      }
    };
    const onScroll = () => update(true);
    el.addEventListener('scroll', onScroll, { passive: true });
    // 内容变宽（翻页加载）、窗口变宽时重新算
    const ro = new ResizeObserver(() => update(false));
    ro.observe(el);
    if (el.firstElementChild) ro.observe(el.firstElementChild);
    update(false);
    return () => {
      el.removeEventListener('scroll', onScroll);
      ro.disconnect();
      window.clearTimeout(hideTimer.current);
    };
  }, [target, watch]);

  /** 轨道上的 x（相对轨道左边）→ 对应的 scrollLeft */
  const scrollFor = (x: number) => {
    const el = target.current!;
    const track = trackRef.current!.clientWidth;
    return ((x / (track - thumb.width)) * (el.scrollWidth - el.clientWidth)) | 0;
  };

  return (
    <div
      ref={trackRef}
      // 轨道一直在（要靠它量宽度），内容不够长时整条藏起来
      className={thumb.width === 0 ? 'hbar is-empty' : active ? 'hbar is-active' : 'hbar'}
      onPointerDown={(e) => {
        // 点轨道空白处：把滑块中心挪到那里
        if (e.target !== e.currentTarget) return;
        const rect = e.currentTarget.getBoundingClientRect();
        target.current?.scrollTo({ left: scrollFor(e.clientX - rect.left - thumb.width / 2), behavior: 'smooth' });
      }}
    >
      <div
        className="hbar__thumb"
        style={{ width: thumb.width, transform: `translateX(${thumb.left}px)` }}
        onPointerDown={(e) => {
          e.preventDefault();
          const el = target.current;
          if (!el) return;
          const handle = e.currentTarget;
          handle.setPointerCapture(e.pointerId);
          handle.dataset.dragging = '';
          const startX = e.clientX;
          const startLeft = thumb.left;
          const move = (ev: PointerEvent) => (el.scrollLeft = scrollFor(startLeft + ev.clientX - startX));
          const up = () => {
            handle.removeEventListener('pointermove', move);
            handle.removeEventListener('pointerup', up);
            delete handle.dataset.dragging;
          };
          handle.addEventListener('pointermove', move);
          handle.addEventListener('pointerup', up);
        }}
      />
    </div>
  );
}
