import { describe, expect, it } from 'vitest';

import { tableOf } from './tablePreview';

describe('剪贴板表格预览', () => {
  it('制表符分隔的几行画成表', () => {
    expect(tableOf('测试图\t数字个数\t认对\n资产负债表\t26\t26\n', false)).toEqual([
      ['测试图', '数字个数', '认对'],
      ['资产负债表', '26', '26'],
    ]);
  });

  it('长数字前面的 \' 去掉', () => {
    expect(tableOf("卡号\t'6222021234567890\n金额\t1,200.00", false)).toEqual([
      ['卡号', '6222021234567890'],
      ['金额', '1,200.00'],
    ]);
  });

  it('不像表的不画', () => {
    expect(tableOf('普通的一段话', false)).toBeNull();
    expect(tableOf('a\tb', false)).toBeNull();
    expect(tableOf('a\tb\nc', false)).toBeNull();
    expect(tableOf('a\tb\tc\nd\te', false)).toBeNull();
  });

  it('截断了的话丢掉最后半行', () => {
    expect(tableOf('a\tb\nc\td\ne\tf\ng\t', true)).toEqual([
      ['a', 'b'],
      ['c', 'd'],
      ['e', 'f'],
    ]);
  });
});
