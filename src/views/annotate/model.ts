// 标注数据模型（规格 02 §5.1）。坐标、线宽、字号全部是**图像物理像素**。

import type { Point, Rect } from '@/views/capture/geometry';

export type Tool =
  | 'rect'
  | 'ellipse'
  | 'arrow'
  | 'pen'
  | 'highlight'
  | 'counter'
  | 'mosaic'
  | 'text'
  | 'measure'
  | 'watermark'
  | 'template'
  | 'eraser';

/** 测量：两点距离 / 三点角度 */
export type MeasureShape = 'distance' | 'angle';
/** 距离的单位：物理像素 / 逻辑像素（点） */
export type MeasureUnit = 'px' | 'pt';

/** 线型：实线 / 虚线 / 点线 */
export type Dash = 'solid' | 'dashed' | 'dotted';
/** 箭头样式：细（渐宽尾巴）、粗、无箭头的直线、两头都有箭头 */
export type ArrowStyle = 'thin' | 'thick' | 'line' | 'double';
/** 序号的写法 */
export type Numbering = 'arabic' | 'chinese' | 'letter' | 'roman';
/** 文字样式：普通（细描边）、底色块、白色粗描边 */
export type TextStyle = 'plain' | 'fill' | 'outline';
/** 荧光笔的形状：涂抹、矩形、聚光灯（压暗框外） */
export type HighlightShape = 'brush' | 'rect' | 'spotlight' | 'magnifier';

export type Annotation =
  | { kind: 'rect'; id: string; rect: Rect; color: string; lineWidth: number; filled: boolean; fillOpacity?: number; dash?: Dash }
  | { kind: 'ellipse'; id: string; rect: Rect; color: string; lineWidth: number; filled: boolean; fillOpacity?: number; dash?: Dash }
  | { kind: 'arrow'; id: string; from: Point; to: Point; color: string; lineWidth: number; style: ArrowStyle; dash?: Dash }
  | { kind: 'pen'; id: string; points: Point[]; color: string; lineWidth: number }
  /** 荧光笔涂抹：半透明的宽笔画 */
  | { kind: 'highlight'; id: string; points: Point[]; color: string; lineWidth: number }
  /** 荧光笔矩形：半透明色块 */
  | { kind: 'highlightRect'; id: string; rect: Rect; color: string }
  /** 聚光灯：选区里框外的部分都压暗（`dim` 是压暗的不透明度），几个聚光灯一起挖洞 */
  | { kind: 'spotlight'; id: string; rect: Rect; dim: number }
  /**
   * 放大镜：`source` 是要放大的那块，旁边一个圆（圆心 `center`、半径 `radius`）里画它放大后的样子，
   * 中间一根连线。放大倍数由圆的大小决定（圆的直径 / 那块的长边）
   */
  | { kind: 'magnifier'; id: string; source: Rect; center: Point; radius: number; color: string }
  /** 测距离：一根两头带短横的线，中间标长度。`scale` 是创建时的缩放（换算成点用） */
  | { kind: 'distance'; id: string; from: Point; to: Point; color: string; lineWidth: number; unit: MeasureUnit; scale: number }
  /** 测角度：从顶点出发的两条边，中间一段圆弧和度数 */
  | { kind: 'angle'; id: string; vertex: Point; a: Point; b: Point; color: string; lineWidth: number }
  /** 序号：实心圆里一个编号，`size` 是圆的直径 */
  | { kind: 'counter'; id: string; at: Point; n: number; numbering: Numbering; color: string; size: number }
  | { kind: 'mosaic'; id: string; points: Point[]; brushSize: number; mode: 'pixelate' | 'blur' }
  /** 矩形马赛克：框住一块整体打码 */
  | { kind: 'mosaicRect'; id: string; rect: Rect; cell: number; mode: 'pixelate' | 'blur' }
  | { kind: 'text'; id: string; at: Point; content: string; color: string; fontSize: number; bold: boolean; style?: TextStyle }
  /**
   * 文字块：铺一层底色盖住原来的字，再在框里自动换行写新字。截图原位翻译用（`group: 'translation'`），
   * `hidden` 时不画也不导出（"看原文"）
   */
  | { kind: 'label'; id: string; rect: Rect; text: string; color: string; bg: string; fontSize: number; bold?: boolean; group?: string; hidden?: boolean };

/** 二级工具条里的选项（CSS 像素，创建标注时乘缩放因子）。会话内记忆（规格 02 §3.7.4）。 */
export interface ToolOptions {
  rect: { lineWidth: number; filled: boolean; fillOpacity: number; dash: Dash; color: string };
  ellipse: { lineWidth: number; filled: boolean; fillOpacity: number; dash: Dash; color: string };
  arrow: { lineWidth: number; style: ArrowStyle; dash: Dash; color: string };
  pen: { lineWidth: number; color: string };
  highlight: { shape: HighlightShape; lineWidth: number; color: string; dim: number };
  counter: { size: number; numbering: Numbering; color: string };
  mosaic: { brushSize: number; mode: 'pixelate' | 'blur'; shape: 'brush' | 'rect' };
  text: { fontSize: number; bold: boolean; style: TextStyle; color: string };
  measure: { shape: MeasureShape; lineWidth: number; unit: MeasureUnit; color: string };
  /** 水印：整张图平铺。enabled 关着就不画；每次截图开始时关掉 */
  watermark: { enabled: boolean; text: string; fontSize: number; opacity: number; angle: number; color: string };
  /** 选中的标注模板（插入时用），见 templates.ts */
  template: { selected: string | null };
  eraser: Record<string, never>;
}

/** 水印画在图上时的参数（字号是物理像素，文字里的 {date} {time} 已经换掉） */
export interface Watermark {
  text: string;
  fontSize: number;
  opacity: number;
  angle: number;
  color: string;
}

/** 苹果系统色板，不用微信那套（规格 02 §3.7.3） */
export const PRESET_COLORS = ['#FF3B30', '#FF9500', '#FFCC00', '#34C759', '#007AFF', '#5856D6', '#FFFFFF', '#000000'];

export const LINE_WIDTHS = [2, 4, 6];
export const PEN_WIDTHS = [2, 4, 8];
export const BRUSH_SIZES = [12, 24, 40];
/** 滚轮调画笔 / 马赛克粗细的范围（CSS 像素） */
export const PEN_RANGE: [number, number] = [1, 40];
export const BRUSH_RANGE: [number, number] = [6, 96];
export const FONT_SIZES = [14, 18, 24];
/** 荧光笔涂抹的粗细 */
export const HIGHLIGHT_WIDTHS = [10, 16, 24];
/** 序号圆的直径 */
export const COUNTER_SIZES = [20, 26, 34];
/** 荧光笔的半透明度 */
export const HIGHLIGHT_ALPHA = 0.38;
/** 荧光笔色板：常见的几种荧光色 */
export const HIGHLIGHT_COLORS = ['#FFE500', '#7CFF4F', '#4FD8FF', '#FF6FD8', '#FF9F1A'];
/** 填充的矩形 / 椭圆默认透明度（以前写死的就是这个值，旧标注没有这个字段时也按它画） */
export const DEFAULT_FILL_OPACITY = 0.6;

export function defaultToolOptions(): ToolOptions {
  return {
    rect: { lineWidth: 4, filled: false, fillOpacity: DEFAULT_FILL_OPACITY, dash: 'solid', color: '#FF3B30' },
    ellipse: { lineWidth: 4, filled: false, fillOpacity: DEFAULT_FILL_OPACITY, dash: 'solid', color: '#FF3B30' },
    arrow: { lineWidth: 4, style: 'thin', dash: 'solid', color: '#FF3B30' },
    pen: { lineWidth: 4, color: '#FF3B30' },
    highlight: { shape: 'brush', lineWidth: 16, color: '#FFE500', dim: 0.55 },
    counter: { size: 26, numbering: 'arabic', color: '#FF3B30' },
    mosaic: { brushSize: 24, mode: 'pixelate', shape: 'brush' },
    text: { fontSize: 18, bold: false, style: 'plain', color: '#FF3B30' },
    measure: { shape: 'distance', lineWidth: 2, unit: 'px', color: '#FF3B30' },
    watermark: { enabled: false, text: 'DATO OCR {date}', fontSize: 18, opacity: 0.18, angle: -30, color: '#808080' },
    template: { selected: null },
    eraser: {},
  };
}

let seq = 0;
export const newId = () => `a${Date.now().toString(36)}${(seq++).toString(36)}`;

export const TEXT_FONT = `'Segoe UI Variable Text', 'Segoe UI', 'Microsoft YaHei UI', 'PingFang SC', sans-serif`;
export const TEXT_LINE_HEIGHT = 1.4;

/** 选项里的水印 → 画在图上的参数：字号换成物理像素，{date} {time} 换成现在的日期时间 */
export function watermarkOf(o: ToolOptions['watermark'], scale: number, now = new Date()): Watermark | null {
  if (!o.enabled || !o.text.trim()) return null;
  const pad = (n: number) => String(n).padStart(2, '0');
  const date = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
  const time = `${pad(now.getHours())}:${pad(now.getMinutes())}`;
  return {
    text: o.text.replaceAll('{date}', date).replaceAll('{time}', time),
    fontSize: o.fontSize * scale,
    opacity: o.opacity,
    angle: o.angle,
    color: o.color,
  };
}

const CHINESE_DIGITS = ['零', '一', '二', '三', '四', '五', '六', '七', '八', '九'];

/** 序号按选定的写法显示：1 / 一 / A / I。超出范围的退回阿拉伯数字 */
export function formatCounter(n: number, numbering: Numbering): string {
  if (n < 1) return String(n);
  switch (numbering) {
    case 'chinese': {
      if (n >= 100) return String(n);
      const tens = Math.floor(n / 10);
      const ones = n % 10;
      if (tens === 0) return CHINESE_DIGITS[ones]!;
      return `${tens === 1 ? '' : CHINESE_DIGITS[tens]}十${ones ? CHINESE_DIGITS[ones] : ''}`;
    }
    case 'letter': {
      // A…Z, AA, AB…
      let out = '';
      let k = n;
      while (k > 0) {
        k -= 1;
        out = String.fromCharCode(65 + (k % 26)) + out;
        k = Math.floor(k / 26);
      }
      return out;
    }
    case 'roman': {
      if (n >= 4000) return String(n);
      const table: [number, string][] = [
        [1000, 'M'], [900, 'CM'], [500, 'D'], [400, 'CD'], [100, 'C'], [90, 'XC'],
        [50, 'L'], [40, 'XL'], [10, 'X'], [9, 'IX'], [5, 'V'], [4, 'IV'], [1, 'I'],
      ];
      let out = '';
      let k = n;
      for (const [v, sym] of table) {
        while (k >= v) {
          out += sym;
          k -= v;
        }
      }
      return out;
    }
    default:
      return String(n);
  }
}
