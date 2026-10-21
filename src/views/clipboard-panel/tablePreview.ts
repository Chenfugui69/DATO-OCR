// 剪贴板里复制的表格（Excel、Numbers、网页表格、我们识别出来的表格……）：纯文本是制表符分隔的几行。
// 看着像表就在卡片上画成小表格，而不是一串挤在一起的字。

/** 卡片上最多画几行 */
const MAX_ROWS = 8;

/**
 * 预览文字像不像一张表：至少两行、每行至少两列、各行列数一样。像的话返回各行的格子（最多 MAX_ROWS 行）。
 * `truncated`：预览只截了前面一段，最后一行可能不完整，丢掉。
 */
export function tableOf(text: string, truncated: boolean): string[][] | null {
  if (!text.includes('\t')) return null;
  let lines = text.replace(/\r\n?/g, '\n').split('\n');
  while (lines.length && !lines[lines.length - 1]!.trim()) lines.pop();
  if (truncated) lines = lines.slice(0, -1);
  if (lines.length < 2) return null;
  const rows = lines.map((l) => l.split('\t'));
  const cols = rows[0]!.length;
  if (cols < 2 || rows.some((r) => r.length !== cols)) return null;
  // 我们复制表格时长数字前面加了 '（让 Excel 当文本），显示时去掉
  return rows.slice(0, MAX_ROWS).map((r) => r.map((c) => c.replace(/^'(?=[+-]?\d{12,})/, '').trim()));
}
