import { useLayoutEffect, useState, type RefObject } from 'react';

/** 元素的实际尺寸（ResizeObserver），用来给工具条这类内容会变的浮层定位。 */
export function useElementSize(ref: RefObject<HTMLElement | null>, fallback: { width: number; height: number }) {
  const [size, setSize] = useState(fallback);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const w = el.offsetWidth;
      const h = el.offsetHeight;
      setSize((s) => (s.width === w && s.height === h ? s : { width: w, height: h }));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return size;
}
