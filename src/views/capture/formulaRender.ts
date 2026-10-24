// 公式：LaTeX 在本地转成 MathML 显示（temml），截图原位和贴图里共用。

import temml from 'temml';

import { sanitizeMathml } from '@/views/ocr/StructuredResult';

/** LaTeX → MathML（本地，temml）。写错了返回 null */
export function latexToMathml(latex: string): string | null {
  if (!latex.trim()) return null;
  try {
    return sanitizeMathml(temml.renderToString(latex, { displayMode: true, throwOnError: true })) || null;
  } catch {
    return null;
  }
}
