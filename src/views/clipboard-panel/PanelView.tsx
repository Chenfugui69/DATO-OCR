// 剪贴板面板（Alt+V，规格 05 §3）。默认底部横向卡片条（Paste 风格），可选竖版小面板。
//
// 全程可以不用鼠标：Alt+V → 直接打字搜索 → ←/→ 选 → Enter 粘贴到之前的窗口。

import './panel.css';

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useVirtualizer } from '@tanstack/react-virtual';
import { getCurrentWindow } from '@tauri-apps/api/window';
import clsx from 'clsx';
import { Clipboard, MousePointerClick, Settings2, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { clipboard, system, translate } from '@/lib/ipc';
import { useSettings, useSettingsStore } from '@/lib/settings';
import { assetUrl, isMac, modKey } from '@/lib/platform';
import { useClipItems, useLiveInvalidation } from '@/lib/queries';
import type { ClipDetail, ClipItem, ClipType } from '@/lib/types';
import { EmptyState, IconButton, SearchField, Skeleton } from '@/ui/controls';
import { isNativeMenuOpen, popupMenu } from '@/ui/nativeMenu';
import { notify } from '@/ui/overlays';

import { clipMenu } from './actions';
import { ClipCard, isGifItem } from './ClipCard';
import { HScrollbar } from './HScrollbar';

type Filter = 'all' | ClipType | 'pinned' | 'favorite';
/** 底部卡片条：卡片宽度和间距（高度见 panel.css 的 .panel__cell） */
const CARD_W = 200;
const CARD_GAP = 14;
const FILTERS: Filter[] = ['all', 'text', 'image', 'link', 'files', 'pinned', 'favorite'];

function queryOf(filter: Filter, keyword: string) {
  return {
    keyword: keyword || undefined,
    kinds: filter === 'text' ? (['text', 'color'] as ClipType[]) : ['image', 'link', 'files'].includes(filter) ? [filter as ClipType] : [],
    pinnedOnly: filter === 'pinned',
    favoriteOnly: filter === 'favorite',
  };
}

interface Preview {
  item: ClipItem;
  detail?: ClipDetail;
  translation?: string | 'loading' | { error: string };
}

export default function PanelView() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  useLiveInvalidation();
  const [style, setStyle] = useState<'bottom' | 'vertical'>('bottom');
  const settings = useSettings();
  const blur = !!settings?.clipboard.panelBlur;
  const docked = style === 'bottom' && !!settings?.clipboard.panelDocked;
  const singleClick = !!settings?.clipboard.singleClickPaste;
  const blurRef = useRef(blur);
  blurRef.current = blur;
  // 页面里不做滑入滑出：用户关了动画；或者 macOS 的底部面板 —— 那里是系统在挪窗口本身（Rust 的 panel::show / dismiss）
  const still = settings?.clipboard.panelAnimation === false || (isMac && style === 'bottom');
  const stillRef = useRef(still);
  stillRef.current = still;
  const [visible, setVisible] = useState(false);
  const [keyword, setKeyword] = useState('');
  const [debounced, setDebounced] = useState('');
  const [filter, setFilter] = useState<Filter>('all');
  const [selected, setSelected] = useState(0);
  const [preview, setPreview] = useState<Preview | null>(null);
  // 按住 Ctrl 一小会儿才亮出卡片上的 Ctrl+1–9（Ctrl+P / Ctrl+D 这种一按就松的不闪一下）
  const [showKeys, setShowKeys] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const hiding = useRef(false);
  const visibleRef = useRef(false);
  visibleRef.current = visible;
  /** 右键菜单关掉之后盯一下焦点（见 watchFocus） */
  const focusWatch = useRef<number | undefined>(undefined);

  useEffect(() => {
    const id = window.setTimeout(() => setDebounced(keyword.trim()), 200);
    return () => window.clearTimeout(id);
  }, [keyword]);

  const query = useClipItems(useMemo(() => queryOf(filter, debounced), [filter, debounced]), 50);
  const items = useMemo(() => query.data?.pages.flatMap((p) => p.items) ?? [], [query.data]);
  const groups = useQuery({ queryKey: ['clip-groups'], queryFn: clipboard.groups });

  const horizontal = style === 'bottom';
  const virt = useVirtualizer({
    count: items.length,
    horizontal,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => (horizontal ? CARD_W + CARD_GAP : items[i]?.type === 'image' ? 120 + 8 : 72 + 8),
    overscan: 3,
    paddingStart: horizontal ? 16 : 8,
    paddingEnd: horizontal ? 16 : 8,
  });

  // 滚到接近末尾时加载下一页（keyset 分页）
  const virtualItems = virt.getVirtualItems();
  const lastIndex = virtualItems[virtualItems.length - 1]?.index ?? 0;
  useEffect(() => {
    if (lastIndex >= items.length - 6 && query.hasNextPage && !query.isFetchingNextPage) void query.fetchNextPage();
  }, [lastIndex, items.length, query]);

  useEffect(() => {
    if (selected >= items.length && items.length > 0) setSelected(items.length - 1);
  }, [items.length, selected]);

  useEffect(() => {
    if (items.length) virt.scrollToIndex(selected, { align: 'auto' });
  }, [selected, items.length, virt]);

  // 横向卡片条：鼠标滚轮竖着滚 = 横着走，带一点缓动，不是一格一格地跳。
  // 触控板本来就会横向滑动，交给浏览器
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || !horizontal) return;
    let target = el.scrollLeft;
    let raf = 0;
    const step = () => {
      const d = target - el.scrollLeft;
      const before = el.scrollLeft;
      el.scrollLeft += Math.abs(d) < 4 ? d : d * 0.24;
      // 到了，或者已经推不动了（到头 / 小数像素被吞）
      if (Math.abs(d) < 4 || el.scrollLeft === before) {
        raf = 0;
        return;
      }
      raf = requestAnimationFrame(step);
    };
    const onWheel = (e: WheelEvent) => {
      if (Math.abs(e.deltaX) > Math.abs(e.deltaY) || e.ctrlKey) return;
      e.preventDefault();
      const unit = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? el.clientWidth : 1;
      if (!raf) target = el.scrollLeft;
      target = Math.max(0, Math.min(el.scrollWidth - el.clientWidth, target + e.deltaY * unit * 1.6));
      if (!raf) raf = requestAnimationFrame(step);
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => {
      el.removeEventListener('wheel', onWheel);
      cancelAnimationFrame(raf);
    };
  }, [horizontal]);

  const hide = useCallback(() => {
    window.clearInterval(focusWatch.current);
    if (hiding.current) return;
    hiding.current = true;
    setVisible(false);
    // 退出比进入快（规格 06 §2.6），从哪来回哪去。毛玻璃时窗口本身就是面板，没法滑，直接藏
    window.setTimeout(
      () => {
        void clipboard.hidePanel();
        hiding.current = false;
      },
      blurRef.current || stillRef.current ? 0 : 200,
    );
  }, []);

  useEvent('clipboard-panel-show', (p) => {
    hiding.current = false;
    setStyle(p.style);
    setKeyword('');
    setDebounced('');
    setSelected(0);
    setPreview(null);
    // 先回到起始位置再滑入（Alt+V 再按一次是 Rust 直接藏窗口的，那次没走退出动画）
    setVisible(false);
    window.clearInterval(focusWatch.current);
    window.setTimeout(() => setVisible(true), 16);
    void qc.invalidateQueries({ queryKey: ['clips'] });
    scrollRef.current?.scrollTo({ left: 0, top: 0 });
  });

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        // 点面板外部 = 关闭。原生右键菜单弹出时也会失焦，那种不算
        if (!focused && !isNativeMenuOpen()) hide();
      })
      .then((fn) => (unlisten = fn))
      .catch(() => undefined);
    return () => unlisten?.();
  }, [hide]);

  const paste = useCallback((item: ClipItem | undefined, plain = false) => {
    if (!item) return;
    hiding.current = true;
    setVisible(false);
    clipboard.paste(item.id, plain).catch(notify.error).finally(() => (hiding.current = false));
  }, []);

  const openPreview = useCallback(async (item: ClipItem, withTranslation = false) => {
    setPreview({ item, translation: withTranslation ? 'loading' : undefined });
    try {
      const detail = await clipboard.get(item.id);
      setPreview((p) => (p?.item.id === item.id ? { ...p, detail } : p));
      if (withTranslation) {
        const r = await translate.text({ text: detail.contentText ?? item.preview ?? '' });
        setPreview((p) => (p?.item.id === item.id ? { ...p, translation: r.text } : p));
      }
    } catch (err) {
      setPreview((p) => (p?.item.id === item.id ? { ...p, translation: { error: String((err as Error).message ?? err) } } : p));
    }
  }, []);

  /**
   * 右键菜单弹出时窗口会失焦（那次不算点外面），菜单关掉后系统不一定再发一次失焦：
   * 比如点别的软件把菜单关掉，面板就一直挂着不收。所以菜单关掉后自己盯着，
   * 发现面板已经不是前台窗口了就收起
   */
  const watchFocus = useCallback(() => {
    window.clearInterval(focusWatch.current);
    focusWatch.current = window.setInterval(() => {
      if (!visibleRef.current || hiding.current) {
        window.clearInterval(focusWatch.current);
        return;
      }
      if (isNativeMenuOpen()) return;
      void system
        .isForeground()
        .then((fg) => {
          if (fg) return;
          window.clearInterval(focusWatch.current);
          hide();
        })
        .catch(() => undefined);
    }, 250);
  }, [hide]);
  useEffect(() => () => window.clearInterval(focusWatch.current), []);

  const showMenu = useCallback(
    (item: ClipItem) => {
      void popupMenu(clipMenu(item, t, { groups: groups.data ?? [], inPanel: true, onTranslate: (i) => void openPreview(i, true) })).finally(watchFocus);
    },
    [t, groups.data, openPreview, watchFocus],
  );

  useEffect(() => {
    if (!visible) {
      setShowKeys(false);
      return;
    }
    let timer: number | undefined;
    const sync = (e: KeyboardEvent) => {
      const held = e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey;
      if (!held) {
        window.clearTimeout(timer);
        timer = undefined;
        setShowKeys(false);
      } else if (timer === undefined) {
        timer = window.setTimeout(() => setShowKeys(true), 160);
      }
    };
    const reset = () => {
      window.clearTimeout(timer);
      timer = undefined;
      setShowKeys(false);
    };
    window.addEventListener('keydown', sync);
    window.addEventListener('keyup', sync);
    window.addEventListener('blur', reset);
    return () => {
      window.removeEventListener('keydown', sync);
      window.removeEventListener('keyup', sync);
      window.removeEventListener('blur', reset);
      window.clearTimeout(timer);
    };
  }, [visible]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!visible) return;
      const searching = document.activeElement === searchRef.current;
      const item = items[selected];
      const prevKey = horizontal ? 'ArrowLeft' : 'ArrowUp';
      const nextKey = horizontal ? 'ArrowRight' : 'ArrowDown';
      if (e.key === 'Escape') {
        e.preventDefault();
        if (preview) setPreview(null);
        else hide();
        return;
      }
      if (e.key === nextKey) {
        e.preventDefault();
        setSelected((i) => Math.min(items.length - 1, i + 1));
        return;
      }
      if (e.key === prevKey) {
        e.preventDefault();
        setSelected((i) => Math.max(0, i - 1));
        return;
      }
      if (e.key === 'Enter') {
        e.preventDefault();
        if (!item) return;
        if (e.shiftKey) {
          void clipboard.copy(item.id).then(() => notify.success(t('clip.copied')), notify.error);
        } else paste(item, modKey(e));
        return;
      }
      if (e.key === 'Tab') {
        e.preventDefault();
        const i = FILTERS.indexOf(filter);
        setFilter(FILTERS[(i + (e.shiftKey ? FILTERS.length - 1 : 1)) % FILTERS.length]!);
        setSelected(0);
        return;
      }
      // Ctrl+1–9：搜索框里打着字也能直接粘贴
      if (/^[1-9]$/.test(e.key) && e.ctrlKey && !e.altKey && !e.shiftKey) {
        e.preventDefault();
        paste(items[Number(e.key) - 1]);
        return;
      }
      if (searching) return;
      if (/^[1-9]$/.test(e.key) && !modKey(e) && !e.altKey) {
        e.preventDefault();
        paste(items[Number(e.key) - 1]);
        return;
      }
      if (e.key === ' ' && item) {
        e.preventDefault();
        if (preview) setPreview(null);
        else void openPreview(item);
        return;
      }
      if (e.key === 'Delete' && item) {
        e.preventDefault();
        void clipboard.remove([item.id]).catch(notify.error);
        return;
      }
      if (modKey(e) && e.key.toLowerCase() === 'p' && item) {
        e.preventDefault();
        void clipboard.setPinned(item.id, !item.pinned).then(() => qc.invalidateQueries({ queryKey: ['clips'] }));
        return;
      }
      if (modKey(e) && e.key.toLowerCase() === 'd' && item) {
        e.preventDefault();
        void clipboard.setFavorite(item.id, !item.favorite).then(() => qc.invalidateQueries({ queryKey: ['clips'] }));
        return;
      }
      // 直接打字 = 聚焦搜索框并输入
      if (e.key.length === 1 && !e.ctrlKey && !e.altKey && !e.metaKey) {
        searchRef.current?.focus();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [visible, items, selected, horizontal, preview, filter, hide, paste, openPreview, qc, t]);

  return (
    <div
      className={clsx('panel-root', `panel-root--${style}`, docked && 'panel-root--docked', blur && 'panel-root--blur', still && 'panel-root--still', showKeys && 'panel-root--keys')}
      data-visible={visible}
    >
      <div className="panel cn-glass-chrome">
        <header className="panel__bar">
          <SearchField
            ref={searchRef}
            className="panel__search"
            value={keyword}
            placeholder={t('clip.search')}
            onChange={(v) => {
              setKeyword(v);
              setSelected(0);
            }}
          />
          <nav className="panel__filters">
            {FILTERS.map((f) => (
              <button
                key={f}
                type="button"
                className={clsx('panel__filter', f === filter && 'panel__filter--active')}
                onClick={() => {
                  setFilter(f);
                  setSelected(0);
                }}
              >
                {t(`clip.filter.${f}`)}
              </button>
            ))}
          </nav>
          <span className="panel__spacer" />
          <button
            type="button"
            className="panel__clickmode"
            title={t('clip.clickModeHint')}
            onClick={() => void useSettingsStore.getState().update((d) => void (d.clipboard.singleClickPaste = !singleClick)).catch(notify.error)}
          >
            <MousePointerClick size={13} strokeWidth={1.75} />
            {singleClick ? t('clip.singleClickPaste') : t('clip.doubleClickPaste')}
          </button>
          <IconButton
            icon={Settings2}
            label={t('nav.settings')}
            size="sm"
            onClick={() => {
              hide();
              void system.showMain('clipboard');
            }}
          />
          <IconButton icon={X} label={t('common.close')} size="sm" onClick={hide} />
        </header>

        <div ref={scrollRef} className="panel__list">
          {query.isLoading ? (
            <div className="panel__loading">
              <Skeleton />
            </div>
          ) : items.length === 0 ? (
            <EmptyState icon={Clipboard} title={debounced ? t('clip.noMatch') : t('clip.empty')} description={debounced ? undefined : t('clip.emptyDesc')} />
          ) : (
            <div style={horizontal ? { width: virt.getTotalSize(), height: '100%', position: 'relative' } : { height: virt.getTotalSize(), position: 'relative' }}>
              {virtualItems.map((v) => {
                const item = items[v.index]!;
                return (
                  <div
                    key={item.id}
                    className="panel__cell"
                    style={horizontal ? { transform: `translateX(${v.start}px)`, width: CARD_W } : { transform: `translateY(${v.start}px)`, height: v.size - 8, left: 8, right: 8 }}
                  >
                    <ClipCard
                      item={item}
                      index={v.index}
                      layout={horizontal ? 'card' : 'row'}
                      selected={v.index === selected}
                      onClick={() => setSelected(v.index)}
                      onActivate={singleClick ? () => paste(item) : undefined}
                      onDoubleClick={singleClick ? undefined : () => paste(item)}
                      onContextMenu={(e) => {
                        e.preventDefault();
                        setSelected(v.index);
                        showMenu(item);
                      }}
                    />
                  </div>
                );
              })}
            </div>
          )}
        </div>

        {horizontal && <HScrollbar target={scrollRef} watch={`${items.length}:${query.isLoading}:${style}`} />}

        {preview && (
          <div className="panel__preview" onClick={() => setPreview(null)}>
            <div className="panel__preview-box cn-glass" onClick={(e) => e.stopPropagation()}>
              {preview.item.type === 'image' ? (
                <img src={assetUrl(preview.item.filePath)} alt="" />
              ) : isGifItem(preview.item) && preview.item.thumbPath ? (
                <img src={assetUrl(preview.item.thumbPath)} alt="" />
              ) : (
                <div className="panel__preview-text cn-selectable">{preview.detail?.contentText ?? preview.item.preview}</div>
              )}
              {preview.translation && (
                <div className="panel__preview-trans cn-selectable">
                  {preview.translation === 'loading' ? <Skeleton /> : typeof preview.translation === 'string' ? preview.translation : preview.translation.error}
                </div>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
