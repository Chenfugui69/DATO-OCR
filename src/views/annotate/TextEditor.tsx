// 文字工具的输入框（规格 02 §5.2 文字）：点击位置出现可直接打字的输入框，
// 字体/字号/颜色与最终绘制完全一致；Esc 或点击别处提交；Enter 换行。

import { useEffect, useRef, useSyncExternalStore } from 'react';

import type { AnnotationEngine } from './engine';
import { TEXT_FONT, TEXT_LINE_HEIGHT } from './model';

export function useEngineVersion(engine: AnnotationEngine): number {
  return useSyncExternalStore(
    (fn) => engine.subscribe(fn),
    () => engine.version,
  );
}

export function TextEditor({ engine, displayScale }: { engine: AnnotationEngine; displayScale: number }) {
  useEngineVersion(engine);
  const ref = useRef<HTMLTextAreaElement>(null);
  const draft = engine.text;
  const editingKey = draft ? `${draft.at.x},${draft.at.y},${draft.editing ?? ''}` : null;

  useEffect(() => {
    if (!editingKey) return;
    const el = ref.current;
    if (!el) return;
    // 等这一帧布局完成再聚焦，否则点击事件的默认行为会把焦点抢回去
    const id = requestAnimationFrame(() => {
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
    });
    return () => cancelAnimationFrame(id);
  }, [editingKey]);

  if (!draft) return null;
  const size = engine.textSize(draft);
  const fontPx = draft.fontSize / displayScale;
  return (
    <textarea
      ref={ref}
      className="an-text-input"
      value={draft.content}
      spellCheck={false}
      style={{
        left: draft.at.x / displayScale,
        top: draft.at.y / displayScale,
        width: size.width / displayScale + fontPx,
        height: size.height / displayScale,
        font: `${draft.bold ? 600 : 400} ${fontPx}px ${TEXT_FONT}`,
        lineHeight: TEXT_LINE_HEIGHT,
        color: draft.color,
      }}
      onChange={(e) => engine.updateText(e.target.value)}
      onPointerDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === 'Escape' || (e.key === 'Enter' && (e.ctrlKey || e.metaKey))) {
          e.preventDefault();
          engine.commitText();
        }
      }}
      onBlur={() => engine.commitText()}
    />
  );
}
