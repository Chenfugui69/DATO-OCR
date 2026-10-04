// 截图原位翻译：把每段译文变成一个"文字块"标注，盖在原文的位置上。
//
// - 底色：段落框四周一圈像素里出现最多的颜色（按 4 位量化分桶，取最大桶的平均）
// - 字色：框里离底色最远的那批像素的平均（抗锯齿的边缘像素混了底色，只取最"纯"的）；
//   框里几乎没有反差（识别到的是很淡的字）就按底色深浅给黑字或白字
// - 字号：从原文行高起步，放不下就一点点缩小，直到排得进框里

import type { Rect } from '@/views/capture/geometry';

import { newId, type Annotation } from './model';
import type { PixelSource } from './pixels';
import { LABEL_LINE_HEIGHT, wrapLabel } from './render';

export const TRANSLATION_GROUP = 'translation';

export interface TranslatedBlock {
  x: number;
  y: number;
  width: number;
  height: number;
  lineHeight: number;
  source: string;
  text: string;
}

type Rgb = [number, number, number];

const css = ([r, g, b]: Rgb) => `rgb(${r},${g},${b})`;
const dist = (a: Rgb, b: Rgb) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
const luma = ([r, g, b]: Rgb) => 0.299 * r + 0.587 * g + 0.114 * b;

function mean(list: Rgb[]): Rgb {
  const n = list.length || 1;
  const s = list.reduce<Rgb>((acc, c) => [acc[0] + c[0], acc[1] + c[1], acc[2] + c[2]], [0, 0, 0]);
  return [Math.round(s[0] / n), Math.round(s[1] / n), Math.round(s[2] / n)];
}

export function sampleColors(src: PixelSource, r: Rect): { bg: Rgb; fg: Rgb } {
  // 底色：框外扩 2 像素的一圈
  const ring: Rgb[] = [];
  const x0 = Math.floor(r.x - 2);
  const y0 = Math.floor(r.y - 2);
  const x1 = Math.ceil(r.x + r.width + 2);
  const y1 = Math.ceil(r.y + r.height + 2);
  const step = Math.max(1, Math.floor((x1 - x0 + y1 - y0) / 120));
  const push = (x: number, y: number) => {
    const c = src.get(x, y);
    if (c) ring.push(c);
  };
  for (let x = x0; x <= x1; x += step) {
    push(x, y0);
    push(x, y1);
  }
  for (let y = y0; y <= y1; y += step) {
    push(x0, y);
    push(x1, y);
  }
  const buckets = new Map<number, Rgb[]>();
  for (const c of ring) {
    const key = ((c[0] >> 4) << 8) | ((c[1] >> 4) << 4) | (c[2] >> 4);
    const list = buckets.get(key);
    if (list) list.push(c);
    else buckets.set(key, [c]);
  }
  let best: Rgb[] = [];
  for (const list of buckets.values()) if (list.length > best.length) best = list;
  const bg = best.length ? mean(best) : ([255, 255, 255] as Rgb);

  // 字色：框里采样，取离底色最远的那批
  const inner: Rgb[] = [];
  const istep = Math.max(1, Math.floor(Math.min(r.width, r.height) / 28));
  for (let y = Math.floor(r.y); y < r.y + r.height; y += istep) {
    for (let x = Math.floor(r.x); x < r.x + r.width; x += istep) {
      const c = src.get(x, y);
      if (c) inner.push(c);
    }
  }
  const far = Math.max(0, ...inner.map((c) => dist(c, bg)));
  const fg: Rgb = far < 48 ? (luma(bg) > 140 ? [29, 29, 31] : [245, 245, 247]) : mean(inner.filter((c) => dist(c, bg) >= far * 0.62));
  return { bg, fg };
}

/** 从原文行高起步，排不进框就缩小。 */
export function fitFontSize(text: string, r: Rect, lineHeight: number, minSize: number): number {
  let size = Math.max(minSize, lineHeight * 0.74);
  for (let i = 0; i < 24 && size > minSize; i += 1) {
    const pad = Math.max(1, size * 0.08);
    const lines = wrapLabel(text, size, r.width - pad * 2);
    if (lines.length * size * LABEL_LINE_HEIGHT <= r.height * 1.06) break;
    size = Math.max(minSize, size * 0.92);
  }
  return Math.round(size * 2) / 2;
}

const median = (xs: number[]) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)] ?? 0;

/** 行高相近（相差 15% 以内）的块归成一组：同一级标题、同一级正文，译文字号应该一样大。 */
function groupByLineHeight(heights: number[]): number[][] {
  const order = heights.map((h, i) => [h, i] as const).sort((a, b) => a[0] - b[0]);
  const groups: number[][] = [];
  let base = -1;
  for (const [h, i] of order) {
    if (base > 0 && h <= base * 1.15) groups[groups.length - 1]!.push(i);
    else {
      groups.push([i]);
      base = h;
    }
  }
  return groups;
}

/** 识别 + 翻译的结果 → 文字块标注。`clip` 是选区（块不出选区）。 */
export function buildLabels(blocks: TranslatedBlock[], src: PixelSource | null, scale: number, clip: Rect | null): Annotation[] {
  const list = blocks.filter((b) => b.text.trim());
  const rects = list.map((b) => {
    // 识别框贴着字，往外扩一点才盖得住字的边缘
    const grow = Math.max(2 * scale, b.lineHeight * 0.12);
    const rect: Rect = { x: b.x - grow, y: b.y - grow, width: b.width + grow * 2, height: b.height + grow * 2 };
    if (!clip) return rect;
    const x = Math.max(rect.x, clip.x);
    const y = Math.max(rect.y, clip.y);
    return { x, y, width: Math.min(rect.x + rect.width, clip.x + clip.width) - x, height: Math.min(rect.y + rect.height, clip.y + clip.height) - y };
  });
  // 字号：同组按组里的中位行高起步各自排版，再不超过组里排出来的中位字号 —— 一组里字号一致，
  // 只有实在放不下的那块会更小
  const sizes = new Array<number>(list.length).fill(0);
  for (const group of groupByLineHeight(list.map((b) => b.lineHeight))) {
    const lh = median(group.map((i) => list[i]!.lineHeight));
    const fits = group.map((i) => fitFontSize(list[i]!.text.trim(), rects[i]!, lh, 9 * scale));
    const cap = median(fits);
    group.forEach((i, k) => (sizes[i] = Math.min(fits[k]!, cap)));
  }
  return list.map((b, i) => {
    const colors = src ? sampleColors(src, { x: b.x, y: b.y, width: b.width, height: b.height }) : { bg: [255, 255, 255] as Rgb, fg: [29, 29, 31] as Rgb };
    return {
      kind: 'label' as const,
      id: newId(),
      rect: rects[i]!,
      text: b.text.trim(),
      color: css(colors.fg),
      bg: css(colors.bg),
      fontSize: sizes[i]!,
      group: TRANSLATION_GROUP,
    };
  });
}
