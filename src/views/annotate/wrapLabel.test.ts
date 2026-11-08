import { beforeAll, describe, expect, it } from 'vitest';

import { wrapLabel } from './render';

// 测试环境（node）没有画布：每个字按 1em 宽、拉丁字母按半个 em 算，够验证换行逻辑
beforeAll(() => {
  class FakeCanvas {
    getContext() {
      const ctx = {
        font: '',
        measureText(text: string) {
          const size = Number(/(\d+(?:\.\d+)?)px/.exec(ctx.font)?.[1] ?? 10);
          let w = 0;
          for (const ch of text) w += (ch.codePointAt(0) ?? 0) <= 0x24f ? size / 2 : size;
          return { width: w };
        },
      };
      return ctx;
    }
  }
  (globalThis as { OffscreenCanvas?: unknown }).OffscreenCanvas = FakeCanvas;
});

describe('wrapLabel', () => {
  it('按框宽换行', () => {
    expect(wrapLabel('你好世界', 10, 20)).toEqual(['你好', '世界']);
  });

  it('框比一个字还窄：每行一个字，不会死循环', () => {
    expect(wrapLabel('好的', 20, 8)).toEqual(['好', '的']);
    expect(wrapLabel('不。', 20, 8)).toEqual(['不', '。']);
  });

  it('框宽是 0 或负数（选区只框住半块）也能返回', () => {
    expect(wrapLabel('是的', 20, 0)).toEqual(['是', '的']);
    expect(wrapLabel('是的', 20, -5)).toEqual(['是', '的']);
  });

  it('长单词硬拆时不把表情拆成半个', () => {
    const lines = wrapLabel('a 😀😀😀', 10, 12);
    expect(lines.join('')).toBe('a😀😀😀');
    for (const l of lines) expect(l).not.toMatch(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/);
  });
});
