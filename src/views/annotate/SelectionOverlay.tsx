// 选中标注的虚线框和控制点（只负责显示，点中哪个控制点由引擎按坐标判断）。
// 鼠标指针：画笔、手绘马赛克是一个跟粗细一样大的圆圈，其他绘制工具是十字，文字是 I 形。

import { HANDLE_CURSORS } from '@/views/capture/geometry';

import type { AnnotationEngine, AnnotationHandle } from './engine';
import type { Tool, ToolOptions } from './model';
import { useEngineVersion } from './TextEditor';

/** 虚线框离图形的距离（和 .an-sel 的 outline-offset 3px + 1px 线宽的一半对应），CSS 像素 */
const OUTLINE_GAP = 3.5;

export function SelectionOverlay({ engine, displayScale }: { engine: AnnotationEngine; displayScale: number }) {
  useEngineVersion(engine);
  const info = engine.selectionInfo();
  if (!info || engine.text || engine.drawing) return null;
  const s = displayScale;
  const o = info.outline;
  return (
    <>
      {o && <div className="an-sel" style={{ left: o.x / s, top: o.y / s, width: o.width / s, height: o.height / s }} />}
      {info.handles.map((h) => {
        // 有虚线框（矩形、椭圆、文字、译文）时，控制点挪到虚线上：虚线框比图形大一圈（outline-offset 3px + 半个线宽）
        const id = String(h.id);
        const compass = o && /^[nsew]{1,2}$/.test(id);
        const dx = compass ? (id.includes('w') ? -OUTLINE_GAP : id.includes('e') ? OUTLINE_GAP : 0) : 0;
        const dy = compass ? (id.includes('n') ? -OUTLINE_GAP : id.includes('s') ? OUTLINE_GAP : 0) : 0;
        const kind = !compass || id.length === 2 ? '' : id === 'n' || id === 's' ? ' an-sel-handle--h' : ' an-sel-handle--v';
        return <span key={id} className={`an-sel-handle${kind}`} style={{ left: h.at.x / s + dx, top: h.at.y / s + dy }} />;
      })}
    </>
  );
}

export function handleCursor(h: AnnotationHandle): string {
  return h === 'from' || h === 'to' ? 'move' : HANDLE_CURSORS[h];
}

const ringCache = new Map<string, string>();

/** 圆圈指针：黑白双圈在深浅背景上都看得见；画笔的圈里淡淡填上画笔颜色。 */
export function ringCursor(diameter: number, fill?: string): string {
  const d = Math.max(6, Math.min(120, Math.round(diameter)));
  const key = `${d}:${fill ?? ''}`;
  const hit = ringCache.get(key);
  if (hit) return hit;
  const size = d + 4;
  const c = size / 2;
  const r = d / 2;
  const svg =
    `<svg xmlns='http://www.w3.org/2000/svg' width='${size}' height='${size}' viewBox='0 0 ${size} ${size}'>` +
    (fill ? `<circle cx='${c}' cy='${c}' r='${r}' fill='${fill}' fill-opacity='0.35'/>` : '') +
    `<circle cx='${c}' cy='${c}' r='${r}' fill='none' stroke='black' stroke-opacity='0.55' stroke-width='2.5'/>` +
    `<circle cx='${c}' cy='${c}' r='${r}' fill='none' stroke='white' stroke-width='1.2'/>` +
    `<circle cx='${c}' cy='${c}' r='1' fill='white' stroke='black' stroke-opacity='0.55' stroke-width='0.6'/>` +
    `</svg>`;
  const hot = Math.round(c);
  const css = `url("data:image/svg+xml,${encodeURIComponent(svg)}") ${hot} ${hot}, crosshair`;
  ringCache.set(key, css);
  return css;
}

/** 当前工具在选区里的指针。 */
export function toolCursor(tool: Tool | null, o: ToolOptions): string {
  switch (tool) {
    case 'pen':
      return ringCursor(o.pen.lineWidth, o.pen.color);
    case 'mosaic':
      return o.mosaic.shape === 'rect' ? 'crosshair' : ringCursor(o.mosaic.brushSize);
    case 'text':
      return 'text';
    case null:
      return 'default';
    default:
      return 'crosshair';
  }
}

/** 能点选已有标注的工具（画笔、手绘马赛克时点哪都是接着画） */
export function canPick(tool: Tool | null, o: ToolOptions): boolean {
  return tool === null || tool === 'rect' || tool === 'ellipse' || tool === 'arrow' || tool === 'text' || (tool === 'mosaic' && o.mosaic.shape === 'rect');
}
