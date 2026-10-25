// 截图遮罩的状态。每块屏一个遮罩窗口，各自一份。
//
// 交互态（规格 02 §1）：
//   detect   自动检测态：鼠标悬停的窗口被框住
//   pressing 按下但还没移动够 4px（松开 = 采纳高亮区域）
//   selecting 自由框选态
//   editing  编辑态：选区确定，工具条出现
//   passive  另一块屏上已有选区，本屏只显示遮罩
//   longshot 长截图采集中（遮罩鼠标穿透，露出真实桌面）
//   gif      GIF 录制中（同上）

import { create } from 'zustand';

import type { GifProgress, LongshotProgress, ScannedCode, SessionInfo } from '@/lib/types';
import { defaultToolOptions, type Tool, type ToolOptions } from '@/views/annotate/model';

import type { Point, Rect } from './geometry';
import type { TranslatedBlock } from '@/views/annotate/translateLayer';
import type { CaptureTable, RegionStyle } from '@/lib/types';
import type { StructuredState } from '@/views/ocr/StructuredResult';

/** 截图界面里的表格 / 公式识别：本地认出的表格带原位显示用的版面，公式带选区的底色、字色 */
export type CaptureStructured = StructuredState & { table?: CaptureTable; region?: RegionStyle };

export type Phase = 'idle' | 'detect' | 'pressing' | 'selecting' | 'editing' | 'passive' | 'longshot' | 'longshot-other' | 'gif' | 'gif-other';

export interface OverlayState {
  session: SessionInfo | null;
  phase: Phase;
  selection: Rect | null;
  hover: Rect | null;
  cursor: Point | null;
  pixelsReady: boolean;
  bitmapReady: boolean;
  /** 磨砂玻璃工具条的底：冻结画面缩小模糊后的图（blob URL），工具条拿它当背景；没生成好是 null */
  glass: string | null;
  tool: Tool | null;
  options: ToolOptions;
  colorFormat: 'hex' | 'rgb' | 'hsl';
  cursorStyle: string;
  longshot: LongshotProgress | null;
  gif: GifProgress | null;
  busy: boolean;
  /** 原位翻译进行中 */
  translating: boolean;
  /** 原位翻译后切到"看原文" */
  showOriginal: boolean;
  /** 原位翻译的全部译文（复制用） */
  translatedText: string;
  /** 原位翻译的各块（原文、位置）：看原文时在上面铺可选的文字 */
  translatedBlocks: TranslatedBlock[];
  /** 原位翻译后：开着时拖动译文挪位置，关着时（默认）鼠标在字上是选字 */
  moveLabels: boolean;
  /** 原位翻译的加载动画：翻的是哪块选区、识别出来要翻的每一行（识别完才有，之前是空的）；
   *  done：翻完了，正在做收尾（占位条化开），收尾完清空 */
  trLoading: { rect: Rect; lines: LoadingLine[]; done: boolean } | null;
  /** 锁定的宽高比（宽 / 高），拖控制点时保持；null = 自由 */
  ratio: number | null;
  /** 点开了尺寸标签上的选区面板（输入坐标 / 宽高、宽高比、上一次选区） */
  sizePanel: boolean;
  /** 自由选区的轮廓（本屏局部物理坐标），`selection` 是它的外接矩形；null = 普通矩形选区 */
  shape: Point[] | null;
  /** 识别出来的二维码 / 条码（显示结果卡片）；null = 没在看 */
  codes: ScannedCode[] | null;
  scanning: boolean;
  /** 开着二级工具条的按钮组 */
  toolbarGroup: string | null;
  /** 选区里看起来有表格（「识别」组换成表格图标） */
  /** 选区看起来是表格 / 公式（识别按钮换图标，点一下直接识别） */
  structureHint: 'table' | 'formula' | null;
  /** 截图一开始就在后台识别出来的整屏二维码 / 条码（本屏局部物理坐标） */
  autoCodes: ScannedCode[];
  /** 选区识别成表格 / 公式（AI）的结果卡片；null = 没开 */
  structured: CaptureStructured | null;
  /** 录制准备：点了「录制」之后、开始录之前（工具条换成录制的，下面一条录制栏） */
  recordPrep: boolean;
  /** 某块屏在录制准备：这块屏也不压暗（别的屏上的遮罩收到 record-prep 事件时设） */
  recordQuiet: boolean;
  /** 开始录之前的倒数还剩几秒；null = 没在倒数 */
  countdown: number | null;
  /** 点了开始、录制还没真正开始：选区里准备时才有的提示、预览先撤掉，免得录进第一帧 */
  recordGo: boolean;
  /** 正在摆摄像头：刚打开摄像头（或再点一下摄像头按钮）时摄像头框能拖，点别处就结束 */
  cameraPlacing: boolean;
  /** 录制栏里展开的那个弹层 */
  recordPanel: 'mouse' | 'progress' | 'countdown' | 'effects' | 'settings' | null;
}

/** 正在翻译的一行（物理像素）。bg / fg：这一行原图上的底色和字色，占位条按它配色；取不到色时为空 */
export interface LoadingLine {
  x: number;
  y: number;
  width: number;
  height: number;
  bg: string | null;
  fg: string | null;
  /** 底色是浅色 */
  light: boolean;
}

export const initialOverlay = (): Omit<OverlayState, 'options'> => ({
  session: null,
  phase: 'idle',
  selection: null,
  hover: null,
  cursor: null,
  pixelsReady: false,
  bitmapReady: false,
  glass: null,
  tool: null,
  colorFormat: 'hex',
  cursorStyle: 'crosshair',
  longshot: null,
  gif: null,
  busy: false,
  translating: false,
  showOriginal: false,
  translatedText: '',
  translatedBlocks: [],
  moveLabels: false,
  trLoading: null,
  ratio: null,
  sizePanel: false,
  shape: null,
  codes: null,
  scanning: false,
  structured: null,
  recordPrep: false,
  recordQuiet: false,
  countdown: null,
  recordGo: false,
  cameraPlacing: false,
  recordPanel: null,
  autoCodes: [],
  toolbarGroup: null,
  structureHint: null,
});

// 工具的线宽/颜色在一次运行期间记忆（规格 02 §3.7.4）—— 不随会话重置
export const useOverlay = create<OverlayState>(() => ({ ...initialOverlay(), options: defaultToolOptions() }));

export const set = useOverlay.setState;

/** 工具条（CSS 像素）位置写给磨砂玻璃的底图对齐用，见 capture.css 的 --gx / --gy */
export const glassAt = (x: number, y: number) => ({ '--gx': `${x}px`, '--gy': `${y}px` }) as React.CSSProperties;
export const get = useOverlay.getState;
