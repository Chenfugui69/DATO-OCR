// 多选了几项时的"已选 N 项 · 复制 · ×"：剪贴板面板放在分组标签后面，主窗口各页放在页面顶栏里。
// 也可以 Ctrl+C 复制、Esc 取消。

import clsx from 'clsx';
import { Check, Copy, X } from 'lucide-react';
import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { modKey } from '@/lib/platform';
import type { ClipItem } from '@/lib/types';

export function PickBar({ count, onCopy, onClear, className }: { count: number; onCopy: () => void; onClear: () => void; className?: string }) {
  const { t } = useTranslation();
  if (!count) return null;
  return (
    <div className={clsx('clip-pickbar', className)} onMouseDown={(e) => e.stopPropagation()} onPointerDown={(e) => e.stopPropagation()}>
      <span className="clip-pickbar__count">{t('clip.pickedCount', { count })}</span>
      <button type="button" className="clip-pickbar__icon clip-pickbar__icon--primary" onClick={onCopy} title={t('clip.copy')} aria-label={t('clip.copy')}>
        <Copy size={14} strokeWidth={2} />
      </button>
      <button type="button" className="clip-pickbar__icon" onClick={onClear} title={t('common.cancel')} aria-label={t('common.cancel')}>
        <X size={14} strokeWidth={2} />
      </button>
    </div>
  );
}

/** 主窗口列表项左上角的勾选框（按着 Ctrl 或者已经勾了几项时出现） */
export function PickCheck({ checked, onToggle }: { checked: boolean; onToggle: () => void }) {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      className="pick-check"
      role="checkbox"
      aria-checked={checked}
      aria-label={t('clip.pick')}
      onMouseDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onClick={(e) => {
        e.stopPropagation();
        onToggle();
      }}
    >
      {checked && <Check size={11} strokeWidth={3} />}
    </button>
  );
}

/** 勾了几项时：Ctrl+C 复制、Esc 取消（焦点不一定在列表上，所以挂在窗口上；输入框里、选着字时不管） */
export function usePickKeys(count: number, onCopy: () => void, onClear: () => void) {
  const ref = useRef({ onCopy, onClear });
  ref.current = { onCopy, onClear };
  useEffect(() => {
    if (!count) return;
    const onKey = (e: KeyboardEvent) => {
      const a = document.activeElement;
      if (a instanceof HTMLInputElement || a instanceof HTMLTextAreaElement) return;
      if (e.key === 'Escape') {
        e.preventDefault();
        ref.current.onClear();
      } else if (modKey(e) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'c' && !window.getSelection()?.toString()) {
        e.preventDefault();
        ref.current.onCopy();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [count]);
}

/** 拖动影子里画的卡片：截图、识字记录不是剪贴板记录，凑成一张剪贴板卡片的样子 */
export function ghostItem(p: Partial<ClipItem> & Pick<ClipItem, 'id' | 'type'>): ClipItem {
  return {
    preview: null,
    filePath: null,
    thumbPath: null,
    files: [],
    width: null,
    height: null,
    charCount: null,
    sourceApp: null,
    sourceIcon: null,
    pinned: false,
    favorite: false,
    note: null,
    lastUsedAt: Date.now(),
    createdAt: Date.now(),
    ...p,
  } as unknown as ClipItem;
}
