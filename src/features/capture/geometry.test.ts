import { describe, expect, it } from 'vitest';

import {
  clampRect,
  cssToVirtual,
  frameRect,
  isUsableSelection,
  physicalToCssLength,
  rectFromPoints,
  virtualToCss,
  type MonitorFrame,
} from './geometry';

/** 主屏：2560×1440 物理，CSS 视口 1706.67，比例 1.5。 */
const primary: MonitorFrame = {
  originX: 0,
  originY: 0,
  width: 2560,
  height: 1440,
  pixelRatio: 1.5,
};

/** 副屏：1920×1080 物理，比例 1，摆在主屏**左边** → 负坐标。 */
const secondaryLeft: MonitorFrame = {
  originX: -1920,
  originY: 0,
  width: 1920,
  height: 1080,
  pixelRatio: 1,
};

/** 副屏：摆在主屏**上方** → 负 y。 */
const secondaryAbove: MonitorFrame = {
  originX: 0,
  originY: -1080,
  width: 1920,
  height: 1080,
  pixelRatio: 1,
};

/** 100% 缩放：CSS 像素和物理像素一比一。 */
const nativePixels: MonitorFrame = {
  originX: 0,
  originY: 0,
  width: 3840,
  height: 2160,
  pixelRatio: 1,
};

describe('cssToVirtual', () => {
  it('按实测的像素比放大', () => {
    expect(cssToVirtual({ x: 100, y: 200 }, primary)).toEqual({ x: 150, y: 300 });
  });

  it('比例为 1 时坐标原样透传', () => {
    expect(cssToVirtual({ x: 600, y: 400 }, nativePixels)).toEqual({ x: 600, y: 400 });
    const rect = rectFromPoints(
      cssToVirtual({ x: 600, y: 400 }, nativePixels),
      cssToVirtual({ x: 1000, y: 700 }, nativePixels),
    );
    expect(rect).toEqual({ x: 600, y: 400, width: 400, height: 300 });
  });

  it('窗口右下角映射到显示器的物理右下角', () => {
    const cssWidth = physicalToCssLength(primary.width, primary);
    const cssHeight = physicalToCssLength(primary.height, primary);
    expect(cssToVirtual({ x: cssWidth, y: cssHeight }, primary)).toEqual({
      x: 2560,
      y: 1440,
    });
  });

  it('副屏在左侧时得到负的虚拟坐标', () => {
    expect(cssToVirtual({ x: 0, y: 0 }, secondaryLeft)).toEqual({ x: -1920, y: 0 });
    expect(cssToVirtual({ x: 1920, y: 1080 }, secondaryLeft)).toEqual({ x: 0, y: 1080 });
  });

  it('副屏在上方时得到负的 y', () => {
    expect(cssToVirtual({ x: 10, y: 0 }, secondaryAbove)).toEqual({ x: 10, y: -1080 });
  });

  it('取整到整数物理像素', () => {
    // 1.5 倍下，奇数 CSS 坐标会落在半个物理像素上
    expect(cssToVirtual({ x: 1, y: 3 }, primary)).toEqual({ x: 2, y: 5 });
  });
});

describe('virtualToCss', () => {
  it('是 cssToVirtual 的逆运算（在整数物理像素上）', () => {
    for (const frame of [primary, secondaryLeft, secondaryAbove]) {
      const css = { x: 64, y: 32 };
      const virtual = cssToVirtual(css, frame);
      expect(virtualToCss(virtual, frame)).toEqual(css);
    }
  });

  it('把负的虚拟坐标搬回窗口内的正 CSS 坐标', () => {
    expect(virtualToCss({ x: -1920, y: 0 }, secondaryLeft)).toEqual({ x: 0, y: 0 });
    expect(virtualToCss({ x: -1420, y: 200 }, secondaryLeft)).toEqual({ x: 500, y: 200 });
  });
});

describe('rectFromPoints', () => {
  it('向右下拖', () => {
    expect(rectFromPoints({ x: 10, y: 20 }, { x: 110, y: 220 })).toEqual({
      x: 10,
      y: 20,
      width: 100,
      height: 200,
    });
  });

  it('向左上拖也得到非负宽高', () => {
    expect(rectFromPoints({ x: 110, y: 220 }, { x: 10, y: 20 })).toEqual({
      x: 10,
      y: 20,
      width: 100,
      height: 200,
    });
  });

  it('跨过 x=0 的接缝时宽度是两屏之和', () => {
    const rect = rectFromPoints({ x: -20, y: 0 }, { x: 30, y: 10 });
    expect(rect).toEqual({ x: -20, y: 0, width: 50, height: 10 });
  });

  it('没有移动时宽高为 0', () => {
    expect(rectFromPoints({ x: 5, y: 5 }, { x: 5, y: 5 })).toEqual({
      x: 5,
      y: 5,
      width: 0,
      height: 0,
    });
  });
});

describe('clampRect', () => {
  it('超出右下边界时被裁到边界', () => {
    const clamped = clampRect({ x: 2500, y: 1400, width: 200, height: 200 }, frameRect(primary));
    expect(clamped).toEqual({ x: 2500, y: 1400, width: 60, height: 40 });
  });

  it('超出左上边界时被裁到边界', () => {
    const clamped = clampRect({ x: -50, y: -50, width: 100, height: 100 }, frameRect(primary));
    expect(clamped).toEqual({ x: 0, y: 0, width: 50, height: 50 });
  });

  it('在负坐标显示器上正常工作', () => {
    const clamped = clampRect(
      { x: -2000, y: -10, width: 200, height: 100 },
      frameRect(secondaryLeft),
    );
    expect(clamped).toEqual({ x: -1920, y: 0, width: 120, height: 90 });
  });

  it('完全不相交时返回零尺寸', () => {
    const clamped = clampRect({ x: 9000, y: 9000, width: 10, height: 10 }, frameRect(primary));
    expect(clamped.width).toBe(0);
    expect(clamped.height).toBe(0);
  });
});

describe('isUsableSelection', () => {
  it('零尺寸不可用', () => {
    expect(isUsableSelection({ x: 0, y: 0, width: 0, height: 10 })).toBe(false);
    expect(isUsableSelection({ x: 0, y: 0, width: 10, height: 0 })).toBe(false);
  });

  it('1px 起可用', () => {
    expect(isUsableSelection({ x: 0, y: 0, width: 1, height: 1 })).toBe(true);
  });
});
