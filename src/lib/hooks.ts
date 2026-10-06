import { useLayoutEffect, useState, type RefObject } from 'react';

/** 元素的实际尺寸（ResizeObserver），用来给工具条这类内容会变的浮层定位。 */
/**
 * 量元素的大小，变了自动更新。元素是条件渲染的（一开始不在、后来才出现）就把"它在不在"放进 `deps`，
 * 不然只在第一次挂载时找元素，找不到就再也不量了（二级工具条对不齐就是这么来的）。
 */
export function useElementSize(
  ref: RefObject<HTMLElement | null>,
  fallback: { width: number; height: number },
  deps: unknown[] = [],
) {
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
    // eslint-disable-next-line react-hooks/exhaustive-deps -- deps 由调用方给
  }, [ref, ...deps]);
  return size;
}
