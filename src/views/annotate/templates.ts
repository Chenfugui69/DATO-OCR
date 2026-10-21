// 标注模板：把一组标注存起来，以后点一下整组插进新截图里（固定的框、序号、说明文字之类）。
//
// 存在本机的浏览器存储里（截图遮罩和编辑器是同一个页面来源，两边共用）。坐标按存的时候那组标注的
// 左上角为原点，记下当时的缩放；插到缩放不一样的屏上时按比例换算，看起来一样大。
// 存储不可用（隐私模式之类）时就是一个空列表，不影响别的功能。

import { newId, type Annotation } from './model';
import { annotationBounds } from './render';

export interface AnnotationTemplate {
  id: string;
  /** 存的时候的缩放（物理像素 / CSS 像素） */
  scale: number;
  /** 标注，坐标相对整组的左上角，物理像素 */
  items: Annotation[];
  width: number;
  height: number;
  createdAt: number;
}

const KEY = 'dato.annotate.templates';
const MAX = 20;

export function listTemplates(): AnnotationTemplate[] {
  try {
    const raw = localStorage.getItem(KEY);
    const list = raw ? (JSON.parse(raw) as AnnotationTemplate[]) : [];
    return Array.isArray(list) ? list.filter((t) => Array.isArray(t.items) && t.items.length > 0) : [];
  } catch {
    return [];
  }
}

function store(list: AnnotationTemplate[]) {
  try {
    localStorage.setItem(KEY, JSON.stringify(list.slice(-MAX)));
  } catch {
    /* 存不了就算了 */
  }
}

/** 把这组标注存成模板（译文块不存：它们跟着原图的字走）。返回新模板，没有可存的返回 null */
export function saveTemplate(annotations: Annotation[], scale: number): AnnotationTemplate | null {
  const items = annotations.filter((a) => a.kind !== 'label');
  if (!items.length) return null;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const a of items) {
    const b = annotationBounds(a);
    minX = Math.min(minX, b.x);
    minY = Math.min(minY, b.y);
    maxX = Math.max(maxX, b.x + b.width);
    maxY = Math.max(maxY, b.y + b.height);
  }
  const t: AnnotationTemplate = {
    id: newId(),
    scale,
    items: items.map((a) => transformed(a, -minX, -minY, 1)),
    width: maxX - minX,
    height: maxY - minY,
    createdAt: Date.now(),
  };
  store([...listTemplates(), t]);
  return t;
}

export function deleteTemplate(id: string) {
  store(listTemplates().filter((t) => t.id !== id));
}

/** 模板实例化：左上角放在 `at`，按 `scale / 存的时候的缩放` 缩放，每个标注换新 id */
export function instantiate(t: AnnotationTemplate, at: { x: number; y: number }, scale: number): Annotation[] {
  const k = scale / (t.scale || 1);
  return t.items.map((a) => ({ ...transformed(a, 0, 0, k, at), id: newId() }));
}

/** 先平移 (dx, dy)，再以原点为中心缩放 k 倍，最后挪到 `to` */
function transformed(a: Annotation, dx: number, dy: number, k: number, to = { x: 0, y: 0 }): Annotation {
  const p = (q: { x: number; y: number }) => ({ x: (q.x + dx) * k + to.x, y: (q.y + dy) * k + to.y });
  const r = (q: { x: number; y: number; width: number; height: number }) => ({ ...p(q), width: q.width * k, height: q.height * k });
  switch (a.kind) {
    case 'rect':
    case 'ellipse':
      return { ...a, rect: r(a.rect), lineWidth: a.lineWidth * k };
    case 'highlightRect':
    case 'spotlight':
    case 'label':
      return { ...a, rect: r(a.rect) };
    case 'mosaicRect':
      return { ...a, rect: r(a.rect), cell: a.cell * k };
    case 'arrow':
      return { ...a, from: p(a.from), to: p(a.to), lineWidth: a.lineWidth * k };
    case 'pen':
    case 'highlight':
      return { ...a, points: a.points.map(p), lineWidth: a.lineWidth * k };
    case 'mosaic':
      return { ...a, points: a.points.map(p), brushSize: a.brushSize * k };
    case 'text':
      return { ...a, at: p(a.at), fontSize: a.fontSize * k };
    case 'counter':
      return { ...a, at: p(a.at), size: a.size * k };
    case 'magnifier':
      return { ...a, source: r(a.source), center: p(a.center), radius: a.radius * k };
    case 'distance':
      return { ...a, from: p(a.from), to: p(a.to), lineWidth: a.lineWidth * k };
    case 'angle':
      return { ...a, vertex: p(a.vertex), a: p(a.a), b: p(a.b), lineWidth: a.lineWidth * k };
  }
}
