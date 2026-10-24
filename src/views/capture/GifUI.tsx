// 录制中选区外面那一圈红框（录制工具条、移动把手、按键显示在 RecordUI.tsx）。红框画在选区外面，不会被录进去。

import type { Rect } from './geometry';

export function GifUI({ rect, s }: { rect: Rect; s: number }) {
  const css = { x: rect.x / s, y: rect.y / s, width: rect.width / s, height: rect.height / s };
  return <div className="gif-frame" style={{ left: css.x - 3, top: css.y - 3, width: css.width + 6, height: css.height + 6 }} />;
}
