// 自己画的右键菜单（贴图用，见 popmenu.rs）：和程序别处一样的样子 —— 图标、快捷键、勾选、深浅色。
// 一排小操作（旋转、翻转）做成一行图标按钮，不占好几行。
// ↑↓ 选、回车确定、Esc 或点别处（窗口失焦）收起。

import clsx from 'clsx';
import {
  Check,
  Copy,
  Eye,
  FlipHorizontal2,
  FlipVertical2,
  Languages,
  type LucideIcon,
  Maximize2,
  MousePointerClick,
  PenLine,
  Pin,
  RotateCcw,
  RotateCw,
  Save,
  ScanQrCode,
  ScanText,
  Sigma,
  Table2,
  TextSelect,
  X,
  XCircle,
} from 'lucide-react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';

import { useEvent } from '@/lib/events';
import { SHEET_ICONS } from '@/views/capture/sheetMenu';
import { popmenu } from '@/lib/ipc';
import type { PopItem } from '@/lib/types';

const ICONS: Record<string, LucideIcon> = {
  copy: Copy,
  save: Save,
  ocr: ScanText,
  translate: Languages,
  qr: ScanQrCode,
  table: Table2,
  formula: Sigma,
  edit: PenLine,
  rotateCw: RotateCw,
  rotateCcw: RotateCcw,
  flipH: FlipHorizontal2,
  flipV: FlipVertical2,
  originalSize: Maximize2,
  opacity: Eye,
  clickThrough: MousePointerClick,
  onTop: Pin,
  close: X,
  closeAll: XCircle,
  copyText: TextSelect,
  // 贴图里表格的右键菜单
  ...SHEET_ICONS,
};

/** 菜单四周给阴影留的地方（CSS 像素） */
const MARGIN = 10;

export default function PopMenuView() {
  const [items, setItems] = useState<PopItem[]>([]);
  const [active, setActive] = useState(-1);
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    void popmenu.items().then((p) => p && setItems(p.items));
  }, []);
  useEvent('popmenu-items', (p) => {
    setItems(p.items);
    setActive(-1);
  });

  // 量好大小再让 Rust 摆到鼠标那里显示
  useLayoutEffect(() => {
    const el = box.current;
    if (!el || !items.length) return;
    const r = el.getBoundingClientRect();
    void popmenu.place(r.width + MARGIN * 2, r.height + MARGIN * 2);
  }, [items]);

  // 点别处：窗口失焦就收起
  useEffect(() => {
    const unlisten = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused) void popmenu.close();
    });
    return () => void unlisten.then((fn) => fn());
  }, []);

  const pickable = items.map((it, i) => (it.kind === 'item' && !it.disabled ? i : -1)).filter((i) => i >= 0);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') void popmenu.close();
      else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const at = pickable.indexOf(active);
        const next = e.key === 'ArrowDown' ? pickable[(at + 1) % pickable.length] : pickable[(at - 1 + pickable.length) % pickable.length];
        if (next !== undefined) setActive(next);
      } else if (e.key === 'Enter') {
        const it = items[active];
        if (it?.kind === 'item') void popmenu.pick(it.id);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  return (
    <div className="pm-root" style={{ padding: MARGIN }} onContextMenu={(e) => e.preventDefault()}>
      <div ref={box} className="pm-menu" role="menu">
        {items.map((it, i) => {
          if (it.kind === 'separator') return <div key={i} className="pm-sep" role="separator" />;
          if (it.kind === 'row') {
            return (
              <div key={i} className="pm-row" role="group" aria-label={it.label}>
                <span className="pm-row__label">{it.label}</span>
                {it.items.map((b) => {
                  const I = ICONS[b.icon] ?? Copy;
                  return (
                    <button key={b.id} type="button" className="pm-icon" title={b.label} aria-label={b.label} onClick={() => void popmenu.pick(b.id)}>
                      <I size={16} strokeWidth={1.6} />
                    </button>
                  );
                })}
              </div>
            );
          }
          const I = it.icon ? ICONS[it.icon] : undefined;
          return (
            <button
              key={it.id}
              type="button"
              role={it.checked === undefined ? 'menuitem' : 'menuitemcheckbox'}
              aria-checked={it.checked}
              className={clsx('pm-item', i === active && 'is-active', it.danger && 'pm-item--danger')}
              disabled={it.disabled}
              onPointerEnter={() => setActive(i)}
              onClick={() => void popmenu.pick(it.id)}
            >
              <span className="pm-item__icon">{it.checked ? <Check size={15} strokeWidth={2} /> : I ? <I size={15} strokeWidth={1.6} /> : null}</span>
              <span className="pm-item__label">{it.label}</span>
              {it.shortcut && <span className="pm-item__key">{it.shortcut}</span>}
            </button>
          );
        })}
      </div>
      <style>{`
        html[data-view='popmenu'], html[data-view='popmenu'] body { background: transparent !important; overflow: hidden; }
        .pm-root { position: fixed; inset: 0; }
        .pm-menu {
          display: inline-flex; flex-direction: column; min-width: 200px; padding: 5px;
          border-radius: 10px; background: var(--cn-bg-elevated, var(--cn-bg));
          box-shadow: 0 8px 28px rgba(0,0,0,0.28), 0 0 0 0.5px var(--cn-separator);
          font: 400 13px/1 var(--cn-font-sans); color: var(--cn-label);
          animation: pm-in 120ms var(--cn-ease-out);
          user-select: none;
        }
        @keyframes pm-in { from { opacity: 0; transform: scale(0.97); } }
        .pm-item {
          display: flex; align-items: center; gap: 8px; height: 30px; padding: 0 10px 0 8px;
          border: 0; border-radius: 6px; background: transparent; color: inherit; font: inherit; text-align: left; cursor: default;
        }
        .pm-item.is-active:not(:disabled) { background: var(--cn-accent); color: var(--cn-label-on-accent, #fff); }
        .pm-item:disabled { opacity: 0.4; }
        .pm-item--danger.is-active:not(:disabled) { background: var(--cn-red, #ff3b30); }
        .pm-item__icon { display: inline-grid; place-items: center; width: 18px; color: inherit; opacity: 0.85; }
        .pm-item__label { flex: 1; white-space: nowrap; }
        .pm-item__key { margin-left: 18px; font-size: 12px; opacity: 0.55; }
        .pm-sep { height: 0.5px; margin: 4px 8px; background: var(--cn-separator); }
        .pm-row { display: flex; align-items: center; gap: 2px; height: 32px; padding: 0 4px 0 34px; }
        .pm-row__label { flex: 1; white-space: nowrap; color: var(--cn-label-secondary); }
        .pm-icon {
          display: grid; place-items: center; width: 28px; height: 26px; border: 0; border-radius: 6px;
          background: transparent; color: var(--cn-label-secondary); cursor: default;
        }
        .pm-icon:hover { background: var(--cn-bg-hover); color: var(--cn-label); }
      `}</style>
    </div>
  );
}
