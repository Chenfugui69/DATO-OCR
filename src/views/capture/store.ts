// 截图遮罩的状态。每块屏一个遮罩窗口，各自一份。
//
// 交互态（规格 02 §1）：
//   detect   自动检测态：鼠标悬停的窗口被框住
//   pressing 按下但还没移动够 4px（松开 = 采纳高亮区域）
//   selecting 自由框选态
//   editing  编辑态：选区确定，工具条出现
//   passive  另一块屏上已有选区，本屏只显示遮罩
//   longshot 长截图采集中（遮罩鼠标穿透，露出真实桌面）

import { create } from 'zustand';

import type { LongshotProgress, SessionInfo } from '@/lib/types';
import { defaultToolOptions, type Tool, type ToolOptions } from '@/views/annotate/model';

import type { Point, Rect } from './geometry';

export type Phase = 'idle' | 'detect' | 'pressing' | 'selecting' | 'editing' | 'passive' | 'longshot' | 'longshot-other';

export interface OverlayState {
  session: SessionInfo | null;
  phase: Phase;
  selection: Rect | null;
  hover: Rect | null;
  cursor: Point | null;
  pixelsReady: boolean;
  bitmapReady: boolean;
  tool: Tool | null;
  options: ToolOptions;
  colorFormat: 'hex' | 'rgb' | 'hsl';
  cursorStyle: string;
  longshot: LongshotProgress | null;
  busy: boolean;
}

export const initialOverlay = (): Omit<OverlayState, 'options'> => ({
  session: null,
  phase: 'idle',
  selection: null,
  hover: null,
  cursor: null,
  pixelsReady: false,
  bitmapReady: false,
  tool: null,
  colorFormat: 'hex',
  cursorStyle: 'crosshair',
  longshot: null,
  busy: false,
});

// 工具的线宽/颜色在一次运行期间记忆（规格 02 §3.7.4）—— 不随会话重置
export const useOverlay = create<OverlayState>(() => ({ ...initialOverlay(), options: defaultToolOptions() }));

export const set = useOverlay.setState;
export const get = useOverlay.getState;
