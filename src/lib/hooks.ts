import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';

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

/**
 * 变得很勤的值隔 `ms` 才放行一次；停下来以后，最后那个值一定会放行。
 *
 * 高刷新率的屏上鼠标每秒动一百多次。位置这类靠 transform 挪的东西每帧跟着走不花什么；
 * 文字、放大镜画面每变一次就要重画一次，跟到每秒 60 次就够了。
 */
export function useThrottled<T>(value: T, ms: number): T {
  const state = useRef({ shown: value, at: 0 });
  const [, rerender] = useState(0);
  if (!Object.is(state.current.shown, value) && performance.now() - state.current.at >= ms) {
    state.current = { shown: value, at: performance.now() };
  }
  const pending = !Object.is(state.current.shown, value);
  useEffect(() => {
    if (!pending) return;
    // 这次没放行：到点再渲染一次，免得最后一个值一直压着
    const wait = Math.max(0, ms - (performance.now() - state.current.at));
    const id = window.setTimeout(() => rerender((n) => n + 1), wait);
    return () => window.clearTimeout(id);
  });
  return state.current.shown;
}

/** 内容类的更新最多这么勤（毫秒）。略小于 60Hz 的一帧，60Hz 的屏上每帧都放行 */
export const CONTENT_INTERVAL = 15;
