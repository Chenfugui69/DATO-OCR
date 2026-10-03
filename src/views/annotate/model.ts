// 标注数据模型（规格 02 §5.1）。坐标、线宽、字号全部是**图像物理像素**。

import type { Point, Rect } from '@/views/capture/geometry';

export type Tool = 'rect' | 'ellipse' | 'arrow' | 'pen' | 'mosaic' | 'text';

export type Annotation =
  | { kind: 'rect'; id: string; rect: Rect; color: string; lineWidth: number; filled: boolean }
  | { kind: 'ellipse'; id: string; rect: Rect; color: string; lineWidth: number; filled: boolean }
  | { kind: 'arrow'; id: string; from: Point; to: Point; color: string; lineWidth: number; style: 'thin' | 'thick' }
  | { kind: 'pen'; id: string; points: Point[]; color: string; lineWidth: number }
  | { kind: 'mosaic'; id: string; points: Point[]; brushSize: number; mode: 'pixelate' | 'blur' }
  | { kind: 'text'; id: string; at: Point; content: string; color: string; fontSize: number; bold: boolean };

/** 二级工具条里的选项（CSS 像素，创建标注时乘缩放因子）。会话内记忆（规格 02 §3.7.4）。 */
export interface ToolOptions {
  rect: { lineWidth: number; filled: boolean; color: string };
  ellipse: { lineWidth: number; filled: boolean; color: string };
  arrow: { lineWidth: number; style: 'thin' | 'thick'; color: string };
  pen: { lineWidth: number; color: string };
  mosaic: { brushSize: number; mode: 'pixelate' | 'blur' };
  text: { fontSize: number; bold: boolean; color: string };
}

/** 苹果系统色板，不用微信那套（规格 02 §3.7.3） */
export const PRESET_COLORS = ['#FF3B30', '#FF9500', '#FFCC00', '#34C759', '#007AFF', '#5856D6', '#FFFFFF', '#000000'];

export const LINE_WIDTHS = [2, 4, 6];
export const PEN_WIDTHS = [2, 4, 8];
export const BRUSH_SIZES = [12, 24, 40];
export const FONT_SIZES = [14, 18, 24];

export function defaultToolOptions(): ToolOptions {
  return {
    rect: { lineWidth: 4, filled: false, color: '#FF3B30' },
    ellipse: { lineWidth: 4, filled: false, color: '#FF3B30' },
    arrow: { lineWidth: 4, style: 'thin', color: '#FF3B30' },
    pen: { lineWidth: 4, color: '#FF3B30' },
    mosaic: { brushSize: 24, mode: 'pixelate' },
    text: { fontSize: 18, bold: false, color: '#FF3B30' },
  };
}

let seq = 0;
export const newId = () => `a${Date.now().toString(36)}${(seq++).toString(36)}`;

export const TEXT_FONT = `'Segoe UI Variable Text', 'Segoe UI', 'Microsoft YaHei UI', 'PingFang SC', sans-serif`;
export const TEXT_LINE_HEIGHT = 1.4;
