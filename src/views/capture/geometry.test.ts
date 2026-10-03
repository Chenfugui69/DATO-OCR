import { describe, expect, it } from 'vitest';

import {
  clampMove,
  constrainAngle,
  fromPoints,
  hitHandle,
  placeMagnifier,
  placeSizeHint,
  placeToolbar,
  resizeByHandle,
  snapEdges,
  snapLines,
  snapMove,
  snapValue,
} from './geometry';

describe('fromPoints', () => {
  it('normalizes any drag direction and includes both endpoints', () => {
    expect(fromPoints({ x: 10, y: 10 }, { x: 19, y: 29 })).toEqual({ x: 10, y: 10, width: 10, height: 20 });
    expect(fromPoints({ x: 19, y: 29 }, { x: 10, y: 10 })).toEqual({ x: 10, y: 10, width: 10, height: 20 });
    expect(fromPoints({ x: 19, y: 10 }, { x: 10, y: 29 })).toEqual({ x: 10, y: 10, width: 10, height: 20 });
  });
  it('allows a single pixel selection', () => {
    expect(fromPoints({ x: 5, y: 5 }, { x: 5, y: 5 })).toEqual({ x: 5, y: 5, width: 1, height: 1 });
  });
});

describe('resizeByHandle', () => {
  const r = { x: 100, y: 100, width: 50, height: 40 };
  it('moves only the dragged edges', () => {
    expect(resizeByHandle(r, 'se', { x: 199, y: 179 })).toEqual({ x: 100, y: 100, width: 100, height: 80 });
    expect(resizeByHandle(r, 'n', { x: 0, y: 90 })).toEqual({ x: 100, y: 90, width: 50, height: 50 });
  });
  it('flips when dragged past the opposite edge', () => {
    // 右边缘拖到左边缘左侧 → 翻转，原左边缘变成右边缘
    expect(resizeByHandle(r, 'e', { x: 80, y: 0 })).toEqual({ x: 80, y: 100, width: 21, height: 40 });
  });
});

describe('hitHandle', () => {
  const r = { x: 100, y: 100, width: 200, height: 100 };
  it('detects corners and edge midpoints', () => {
    expect(hitHandle(r, { x: 101, y: 99 }, 6)).toBe('nw');
    expect(hitHandle(r, { x: 200, y: 200 }, 6)).toBe('s');
    expect(hitHandle(r, { x: 301, y: 150 }, 6)).toBe('e');
  });
  it('detects plain edges between handles', () => {
    expect(hitHandle(r, { x: 140, y: 101 }, 8)).toBe('n');
    expect(hitHandle(r, { x: 150, y: 150 }, 8)).toBeNull();
  });
});

describe('clampMove', () => {
  it('keeps the rect inside the screen without resizing', () => {
    const screen = { x: 0, y: 0, width: 1000, height: 800 };
    expect(clampMove({ x: 950, y: -20, width: 100, height: 100 }, screen)).toEqual({ x: 900, y: 0, width: 100, height: 100 });
  });
});

describe('snapping', () => {
  it('snaps within threshold only', () => {
    expect(snapValue(103, [100, 200], 8)).toBe(100);
    expect(snapValue(110, [100, 200], 8)).toBe(110);
  });
  it('snaps dragged edges to window and screen edges', () => {
    const lines = snapLines([{ x: 100, y: 100, width: 300, height: 200 }], { x: 0, y: 0, width: 1920, height: 1080 });
    const r = snapEdges({ x: 95, y: 40, width: 302, height: 100 }, { l: true, r: true }, lines, 8);
    expect(r.x).toBe(100);
    expect(r.x + r.width).toBe(400);
    expect(r.y).toBe(40);
  });
  it('move snapping keeps size', () => {
    const lines = snapLines([], { x: 0, y: 0, width: 1000, height: 1000 });
    expect(snapMove({ x: 5, y: 500, width: 100, height: 100 }, lines, 8)).toEqual({ x: 0, y: 500, width: 100, height: 100 });
  });
});

describe('placeToolbar', () => {
  const screen = { width: 1920, height: 1080 };
  const bar = { width: 500, height: 40 };
  it('goes below and right-aligned by default', () => {
    expect(placeToolbar({ x: 600, y: 100, width: 600, height: 300 }, bar, screen)).toEqual({ x: 700, y: 408, inside: false });
  });
  it('moves above when there is no room below', () => {
    expect(placeToolbar({ x: 600, y: 700, width: 600, height: 360 }, bar, screen)).toEqual({ x: 700, y: 652, inside: false });
  });
  it('goes inside when neither fits', () => {
    const p = placeToolbar({ x: 0, y: 10, width: 1920, height: 1060 }, bar, screen);
    expect(p.inside).toBe(true);
    expect(p.y).toBe(10 + 1060 - 8 - 40);
  });
  it('left-aligns when right alignment would leave the screen', () => {
    expect(placeToolbar({ x: 20, y: 100, width: 100, height: 100 }, bar, screen).x).toBe(20);
  });
});

describe('placeSizeHint / placeMagnifier', () => {
  it('hint goes inside when the selection touches the top', () => {
    expect(placeSizeHint({ x: 50, y: 10, width: 100, height: 100 }, { width: 80, height: 20 }, { width: 1920, height: 1080 })).toEqual({ x: 56, y: 16 });
    expect(placeSizeHint({ x: 50, y: 200, width: 100, height: 100 }, { width: 80, height: 20 }, { width: 1920, height: 1080 })).toEqual({ x: 50, y: 174 });
  });
  it('magnifier flips near edges', () => {
    const box = { width: 120, height: 170 };
    expect(placeMagnifier({ x: 100, y: 100 }, box, { width: 1000, height: 800 })).toEqual({ x: 116, y: 116 });
    expect(placeMagnifier({ x: 950, y: 750 }, box, { width: 1000, height: 800 })).toEqual({ x: 814, y: 564 });
  });
});

describe('constrainAngle', () => {
  it('snaps to 15 degree steps', () => {
    const p = constrainAngle({ x: 0, y: 0 }, { x: 100, y: 3 });
    expect(p.y).toBeCloseTo(0, 5);
    const q = constrainAngle({ x: 0, y: 0 }, { x: 100, y: 95 });
    expect(q.x).toBeCloseTo(q.y, 5);
  });
});
