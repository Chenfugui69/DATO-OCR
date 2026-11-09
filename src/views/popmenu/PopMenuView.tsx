// 自己画的右键菜单（贴图用，见 popmenu.rs）：和程序别处一样的样子 —— 图标、快捷键、勾选、深浅色。
// 一排小操作（旋转、翻转）做成一行图标按钮，不占好几行。
// ↑↓ 选、回车确定、Esc 或点别处（窗口失焦）收起。

import clsx from 'clsx';
import {
  AppWindow,
  Bot,
  Camera,
  Check,
  ClipboardList,
  Copy,
  Crop,
  History,
  ImageUp,
  ChevronDown,
  ClipboardPaste,
  ExternalLink,
  FolderInput,
  FolderOpen,
  StickyNote,
  Star,
  Tag,
  Trash2,
  Type,
  Power,
  ScrollText,
  Settings,
  Timer,
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
  // 托盘菜单（Windows，见 tray.rs 的 open_menu）
  capture: Crop,
  timer: Timer,
  instant: Camera,
  longshot: ScrollText,
  clipboard: ClipboardList,
  ai: Bot,
  pinImage: ImageUp,
  restore: History,
  window: AppWindow,
  settings: Settings,
  quit: Power,
  // 剪贴板卡片的右键菜单（见 clipboard-panel/actions.ts）
  paste: ClipboardPaste,
  pastePlain: Type,
  pinTop: Pin,
  star: Star,
  tag: Tag,
  note: StickyNote,
  folder: FolderInput,
  reveal: FolderOpen,
  link: ExternalLink,
  trash: Trash2,
};

/**
 * 菜单四周给阴影留的地方（CSS 像素）。要装得下下面 .pm-menu 的整个阴影（下方 6 + 18 = 24），
 * 装不下的话阴影在窗口边上被齐齐切断，菜单外面看着像套了一个方框
 */
const MARGIN = 24;

type Row = { it: PopItem; depth: number };

export default function PopMenuView() {
  const [items, setItems] = useState<PopItem[]>([]);
  const [active, setActive] = useState(-1);
  // 展开着的子菜单（"移动到分组"这种）：就地在下面展开，不另开一层
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    void popmenu.items().then((p) => p && setItems(p.items));
  }, []);
  useEvent('popmenu-items', (p) => {
    setItems(p.items);
    setActive(-1);
    setOpen(new Set());
  });

  // 量好大小再让 Rust 摆到鼠标那里显示（子菜单展开 / 收起后重新量）
  useLayoutEffect(() => {
    const el = box.current;
    if (!el || !items.length) return;
    const r = el.getBoundingClientRect();
    void popmenu.place(r.width + MARGIN * 2, r.height + MARGIN * 2, MARGIN);
  }, [items, open]);

  // 点别处：窗口失焦就收起
  useEffect(() => {
    const unlisten = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused) void popmenu.close();
    });
    return () => void unlisten.then((fn) => fn());
  }, []);

  // 摊平成一行一行（展开的子菜单跟在它下面）
  const rows: Row[] = [];
  const walk = (list: PopItem[], depth: number) => {
    for (const it of list) {
      rows.push({ it, depth });
      if (it.kind === 'item' && it.submenu && open.has(it.id)) walk(it.submenu, depth + 1);
    }
  };
  walk(items, 0);

  const toggle = (id: string) =>
    setOpen((old) => {
      const next = new Set(old);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const choose = (it: PopItem) => {
    if (it.kind !== 'item' || it.disabled) return;
    if (it.submenu) toggle(it.id);
    else void popmenu.pick(it.id);
  };

  const pickable = rows.map((r, i) => (r.it.kind === 'item' && !r.it.disabled ? i : -1)).filter((i) => i >= 0);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const cur = rows[active]?.it;
      if (e.key === 'Escape') void popmenu.close();
      else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const at = pickable.indexOf(active);
        const next = e.key === 'ArrowDown' ? pickable[(at + 1) % pickable.length] : pickable[(at - 1 + pickable.length) % pickable.length];
        if (next !== undefined) setActive(next);
      } else if (e.key === 'Enter' && cur) choose(cur);
      else if (e.key === 'ArrowRight' && cur?.kind === 'item' && cur.submenu && !open.has(cur.id)) toggle(cur.id);
      else if (e.key === 'ArrowLeft' && cur?.kind === 'item' && cur.submenu && open.has(cur.id)) toggle(cur.id);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  return (
    <div className="pm-root" style={{ padding: MARGIN }} onContextMenu={(e) => e.preventDefault()}>
      <div ref={box} className="pm-menu" role="menu">
        {rows.map(({ it, depth }, i) => {
          if (it.kind === 'separator') return <div key={i} className="pm-sep" role="separator" />;
          if (it.kind === 'row') {
            return (
              <div key={i} className="pm-row" role="group" aria-label={it.label}>
                <span className="pm-row__label">{it.label}</span>
                {it.items.map((b) => {
                  const I = (b.icon && ICONS[b.icon]) || Copy;
                  return (
                    <button
                      key={b.id}
                      type="button"
                      className={clsx('pm-icon', b.text && 'pm-icon--text', b.on && 'is-on')}
                      title={b.label}
                      aria-label={b.label}
                      aria-pressed={b.on}
                      onClick={() => void popmenu.pick(b.id)}
                    >
                      {b.text ?? <I size={16} strokeWidth={1.6} />}
                    </button>
                  );
                })}
              </div>
            );
          }
          const I = it.icon ? ICONS[it.icon] : undefined;
          const expanded = !!it.submenu && open.has(it.id);
          return (
            <button
              key={it.id}
              type="button"
              role={it.checked === undefined ? 'menuitem' : 'menuitemcheckbox'}
              aria-checked={it.checked}
              aria-expanded={it.submenu ? expanded : undefined}
              className={clsx('pm-item', i === active && 'is-active', it.danger && 'pm-item--danger', depth > 0 && 'pm-item--sub', it.checked && 'is-checked')}
              disabled={it.disabled}
              onPointerEnter={() => setActive(i)}
              onClick={() => choose(it)}
            >
              {/* 有图标的：图标照画，勾上的末尾打勾；没图标的勾画在图标的位置 */}
              <span className="pm-item__icon">{I ? <I size={15} strokeWidth={1.6} /> : it.checked ? <Check size={15} strokeWidth={2} /> : null}</span>
              <span className="pm-item__label">{it.label}</span>
              {it.shortcut && (
                <span className="pm-item__key">
                  {it.shortcut.split('+').map((k, n) => (
                    <kbd key={n}>{k}</kbd>
                  ))}
                </span>
              )}
              {I && it.checked && <Check className="pm-item__tick" size={14} strokeWidth={2.2} />}
              {it.submenu && <ChevronDown className={clsx('pm-item__chev', expanded && 'is-open')} size={14} strokeWidth={2} />}
            </button>
          );
        })}
      </div>
      <style>{`
        html[data-view='popmenu'], html[data-view='popmenu'] body { background: transparent !important; overflow: hidden; }
        .pm-root { position: fixed; inset: 0; }
        .pm-menu {
          display: inline-flex; flex-direction: column; min-width: 216px; padding: 6px;
          border-radius: 12px; background: var(--cn-bg-elevated, var(--cn-bg));
          box-shadow: 0 0 0 0.5px var(--cn-separator), 0 2px 6px rgba(0,0,0,0.12), 0 10px 22px -6px rgba(0,0,0,0.3);
          font: 400 13px/1 var(--cn-font-sans); color: var(--cn-label);
          animation: pm-in 140ms cubic-bezier(0.2, 0.9, 0.3, 1.1);
          transform-origin: var(--pm-origin, top left);
          user-select: none;
        }
        @keyframes pm-in { from { opacity: 0; transform: scale(0.96); } }
        .pm-item {
          display: flex; align-items: center; gap: 10px; height: 32px; padding: 0 8px 0 9px;
          border: 0; border-radius: 7px; background: transparent; color: inherit; font: inherit; text-align: left; cursor: default;
          transition: background 80ms ease;
        }
        .pm-item.is-active:not(:disabled) { background: color-mix(in srgb, var(--cn-accent) 16%, transparent); }
        .pm-item.is-active:not(:disabled) .pm-item__icon { color: var(--cn-accent); opacity: 1; }
        .pm-item:active:not(:disabled) { background: color-mix(in srgb, var(--cn-accent) 24%, transparent); }
        .pm-item:disabled { opacity: 0.4; }
        .pm-item--danger { color: var(--cn-red, #ff3b30); }
        .pm-item--danger .pm-item__icon { color: inherit; }
        .pm-item--danger.is-active:not(:disabled) { background: color-mix(in srgb, var(--cn-red, #ff3b30) 14%, transparent); }
        .pm-item--danger.is-active:not(:disabled) .pm-item__icon { color: inherit; }
        .pm-item__icon { display: inline-grid; place-items: center; width: 18px; color: var(--cn-label-secondary); opacity: 0.9; }
        .pm-item__label { flex: 1; white-space: nowrap; }
        .pm-item__key { display: inline-flex; gap: 3px; margin-left: 20px; }
        .pm-item__key kbd {
          min-width: 18px; height: 18px; padding: 0 5px; display: inline-grid; place-items: center;
          border-radius: 5px; font: 500 11px/1 var(--cn-font-sans); color: var(--cn-label-secondary);
          background: var(--cn-fill-tertiary, rgba(127,127,127,0.12));
        }
        .pm-sep { height: 0.5px; margin: 5px 10px; background: var(--cn-separator); }
        .pm-item--sub { padding-left: 37px; height: 30px; }
        .pm-item.is-checked { color: var(--cn-accent); }
        .pm-item.is-checked .pm-item__icon { color: var(--cn-accent); }
        .pm-item__tick { margin-left: 14px; color: var(--cn-accent); flex: none; }
        .pm-item__chev { margin-left: 14px; flex: none; opacity: 0.55; transition: transform 160ms ease; }
        .pm-item__chev.is-open { transform: rotate(180deg); }
        .pm-row { display: flex; align-items: center; gap: 2px; height: 32px; padding: 0 4px 0 34px; }
        .pm-row__label { flex: 1; white-space: nowrap; color: var(--cn-label-secondary); }
        .pm-icon {
          display: grid; place-items: center; width: 28px; height: 26px; border: 0; border-radius: 6px;
          background: transparent; color: var(--cn-label-secondary); cursor: default;
        }
        .pm-icon:hover { background: var(--cn-bg-hover); color: var(--cn-label); }
        .pm-icon--text { width: auto; min-width: 34px; padding: 0 5px; font: 500 11.5px/1 var(--cn-font-sans); font-variant-numeric: tabular-nums; }
        .pm-icon.is-on { background: var(--cn-accent); color: var(--cn-label-on-accent, #fff); }
      `}</style>
    </div>
  );
}
