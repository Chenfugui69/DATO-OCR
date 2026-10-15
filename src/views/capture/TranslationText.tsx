// 原位翻译之后能直接用鼠标选字：在选区上叠一层透明的真文字，位置、字号、换行和画布上画的一致，
// 选中时只显示选区高亮，看到的还是底下画布上的字。
// - 看译文：一行行跟着画布上的译文标注走（用户挪过标注也跟着）
// - 看原文：铺在识别出来的原文块上，字号按原文行高估，选中高亮大致对得上
// 打开"移动"开关时这层让开，鼠标交回给画布，拖动译文标注挪位置。

import type { AnnotationEngine } from '@/views/annotate/engine';
import { TEXT_FONT } from '@/views/annotate/model';
import { LABEL_LINE_HEIGHT, wrapLabel } from '@/views/annotate/render';
import { TRANSLATION_GROUP, type TranslatedBlock } from '@/views/annotate/translateLayer';

interface Line {
  key: string;
  x: number;
  y: number;
  width: number;
  height: number;
  fontSize: number;
  lineHeight: number;
  text: string;
}

/** 画布上译文标注的每一行（和 render.ts 的 drawLabel 同一套排法） */
function labelLines(engine: AnnotationEngine): Line[] {
  const out: Line[] = [];
  for (const a of engine.annotations) {
    if (a.kind !== 'label' || a.group !== TRANSLATION_GROUP) continue;
    const r = a.rect;
    const pad = Math.max(1, a.fontSize * 0.08);
    const lines = wrapLabel(a.text, a.fontSize, r.width - pad * 2);
    const lh = a.fontSize * LABEL_LINE_HEIGHT;
    const total = lines.length * lh;
    const top = total < r.height ? r.y + (r.height - total) / 2 : r.y;
    lines.forEach((text, i) =>
      out.push({ key: `${a.id}-${i}`, x: r.x + pad, y: top + i * lh, width: r.width - pad * 2, height: lh, fontSize: a.fontSize, lineHeight: lh, text }),
    );
  }
  return out;
}

/** 原文块：按行高拆成几行，字铺满行宽（原文行是识别框，画布上不画字，选中高亮能对上大概位置就行） */
function sourceLines(blocks: TranslatedBlock[]): Line[] {
  return blocks.map((b, i) => {
    const lh = b.lineHeight > 0 ? b.lineHeight : b.height;
    // 块有几行：按高度除行高；每行高度均分，字号取行高的八成
    const rows = Math.max(1, Math.round(b.height / lh));
    return { key: `src-${i}`, x: b.x, y: b.y, width: b.width, height: b.height, fontSize: lh * 0.78, lineHeight: b.height / rows, text: b.source };
  });
}

export function TranslationText({
  engine,
  blocks,
  showOriginal,
  s,
}: {
  engine: AnnotationEngine;
  blocks: TranslatedBlock[];
  showOriginal: boolean;
  /** 显示缩放：画布坐标（物理像素）除以它就是 CSS 像素 */
  s: number;
}) {
  const lines = showOriginal ? sourceLines(blocks) : labelLines(engine);
  return (
    <div
      className="cap-trtext"
      style={{ transform: `scale(${1 / s})` }}
      // 在字上按下是选字，不能冒到遮罩上变成拖选区 / 挪标注
      onPointerDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.stopPropagation()}
    >
      {lines.map((l) => (
        <div
          key={l.key}
          className={showOriginal ? 'cap-trtext__line cap-trtext__line--wrap' : 'cap-trtext__line'}
          style={{
            left: l.x,
            top: l.y,
            width: l.width,
            height: l.height,
            font: `400 ${l.fontSize}px ${TEXT_FONT}`,
            lineHeight: `${l.lineHeight}px`,
          }}
        >
          {l.text}
        </div>
      ))}
    </div>
  );
}
