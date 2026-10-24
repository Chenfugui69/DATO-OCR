import { describe, expect, it } from 'vitest';

import { candidatesAt, ElementCache, tableMagnet, thresholds, withTable } from './elements';

const el = (x: number, y: number, width: number, height: number, role = 'AXCell') => ({ rect: { x, y, width, height }, role });

describe('candidatesAt', () => {
  const items = [
    el(0, 0, 1000, 800, 'AXGroup'), // 整页
    el(100, 100, 400, 300, 'AXTable'),
    el(100, 100, 400, 30, 'AXRow'),
    el(100, 100, 100, 30), // 第一格
    el(200, 100, 100, 30), // 第二格
    el(105, 105, 40, 12, 'AXStaticText'),
  ];
  const opts = { ...thresholds('element', 1), maxArea: 2_000_000 };

  it('由小到大、不要文字片段', () => {
    expect(candidatesAt(items, { x: 120, y: 110 }, opts).map((r) => r.width)).toEqual([100, 400, 400, 1000]);
  });

  it('挪到旁边那格马上就有它（不会先跳到整张表）', () => {
    expect(candidatesAt(items, { x: 250, y: 110 }, opts)[0]).toEqual({ x: 200, y: 100, width: 100, height: 30 });
  });

  it('区块档只要大块', () => {
    const block = { ...thresholds('block', 1), maxArea: 2_000_000 };
    expect(candidatesAt(items, { x: 120, y: 110 }, block).map((r) => r.width)).toEqual([400, 1000]);
  });

  it('和窗口一样大的不算', () => {
    expect(candidatesAt(items, { x: 120, y: 110 }, { ...opts, maxArea: 800_000 }).map((r) => r.width)).toEqual([100, 400, 400]);
  });
});

describe('ElementCache', () => {
  it('去重，附近元素每个锚点只取一次', () => {
    const c = new ElementCache();
    c.add(1, [el(0, 0, 10, 10), el(0, 0, 10, 10)]);
    expect(c.items(1)).toHaveLength(1);
    expect(c.claimNear(1, { x: 0, y: 0, width: 10, height: 10 })).toBe(true);
    expect(c.claimNear(1, { x: 0, y: 0, width: 10, height: 10 })).toBe(false);
  });
});

describe('tableMagnet', () => {
  const page = el(0, 0, 1000, 800, 'AXGroup');
  const upper = el(100, 100, 400, 200, 'Table');
  const lower = el(100, 330, 400, 200, 'Table');
  const cell = el(400, 150, 100, 30, 'DataItem');
  const items = [page, upper, lower, cell];
  const opts = { margin: 27, band: 7.5, minSide: 4 };

  it('表格里贴着边：框整张表，格子去掉', () => {
    const p = { x: 496, y: 160 };
    const t = tableMagnet(items, p, [cell.rect, page.rect], opts);
    expect(t).toEqual(upper.rect);
    expect(withTable(t, [cell.rect, page.rect])).toEqual([upper.rect, page.rect]);
  });

  it('表格中间：不吸', () => {
    expect(tableMagnet(items, { x: 450, y: 165 }, [cell.rect, page.rect], opts)).toBeNull();
  });

  it('两张表中间的缝：吸离得近的', () => {
    expect(tableMagnet(items, { x: 300, y: 309 }, [page.rect], opts)).toEqual(upper.rect);
    expect(tableMagnet(items, { x: 300, y: 324 }, [page.rect], opts)).toEqual(lower.rect);
  });

  it('离得太远：不吸', () => {
    expect(tableMagnet(items, { x: 300, y: 50 }, [page.rect], opts)).toBeNull();
  });
});
