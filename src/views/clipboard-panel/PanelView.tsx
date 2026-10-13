// 剪贴板面板（Alt+V，规格 05 §3）。默认底部横向卡片条（Paste 风格），可选竖版小面板。
//
// 全程可以不用鼠标：Alt+V → 直接打字搜索 → ←/→ 选 → Enter 粘贴到之前的窗口。

import './panel.css';
import './orb.css';

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useVirtualizer } from '@tanstack/react-virtual';
import { getCurrentWindow } from '@tauri-apps/api/window';
import clsx from 'clsx';
import { Clipboard, ListFilter, MoreHorizontal, Sparkles, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { ai, clipboard, ocr, system, translate } from '@/lib/ipc';
import { useSettings, useSettingsStore } from '@/lib/settings';
import { assetUrl, isMac, modKey } from '@/lib/platform';
import { useClipItems, useLiveInvalidation } from '@/lib/queries';
import type { ClipDetail, ClipItem, ClipType, SmartSuggestion } from '@/lib/types';
import { EmptyState, SearchField, Skeleton } from '@/ui/controls';
import { isNativeMenuOpen, popupMenu } from '@/ui/nativeMenu';
import { notify } from '@/ui/overlays';

import { clipMenu } from './actions';
import { BoardTabs, boardColor } from './BoardTabs';
import { type CardAction, ClipCard, isGifItem } from './ClipCard';
import { HScrollbar } from './HScrollbar';
import { OtpBody } from './OtpBody';
import { type OrbColor, ORB_COLORS, WaterOrb } from './WaterOrb';
import { ZoomImage } from './ZoomImage';

type Filter = 'all' | ClipType | 'pinned' | 'favorite';
/** 底部卡片条：卡片宽度和间距（高度见 panel.css 的 .panel__cell） */
/** 卡片条里每张卡片的宽度：和高度（240）差不多，接近 Paste 的方卡片 */
const CARD_W = 236;
const CARD_GAP = 14;
const FILTERS: Filter[] = ['all', 'text', 'image', 'link', 'files', 'pinned', 'favorite'];

function queryOf(filter: Filter, keyword: string, board: number | null) {
  return {
    keyword: keyword || undefined,
    groupId: board,
    kinds: filter === 'text' ? (['text', 'color', 'otp'] as ClipType[]) : ['image', 'link', 'files'].includes(filter) ? [filter as ClipType] : [],
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
  // 打开时边缘亮一圈光、卡片依次淡入，粘贴时卡片闪一下（参照 Paste）；跟着"面板动画"开关走
  const fx = settings?.clipboard.panelAnimation !== false;
  /** 智能推荐水珠的颜色，面板的光晕、推荐描边跟着它 */
  const orbColor: OrbColor = (ORB_COLORS as readonly string[]).includes(settings?.clipboard.orbColor ?? '') ? (settings!.clipboard.orbColor as OrbColor) : 'ocean';
  const fxRef = useRef(fx);
  fxRef.current = fx;
  /** 正在"闪一下"的卡片（粘贴前的反馈） */
  const [flashing, setFlashing] = useState<number | null>(null);
  /** 推荐结果刚出来的那一小会儿：卡片依次淡入（之后滚动出来的新卡片不放） */
  const [entering, setEntering] = useState(false);
  const enterTimer = useRef<number | undefined>(undefined);
  /** 智能推荐：thinking = 正在看你在做什么（面板边缘的暖光一直"呼吸"）；结果出来后推荐的排在最前 */
  const [smart, setSmart] = useState<{ phase: 'thinking' } | ({ phase: 'done'; refining?: boolean } & SmartSuggestion) | null>(null);
  /**
   * 面板边缘的光（颜色跟水珠）跟着推荐走，推荐模式下一直亮着：
   * think = 正在看你在做什么，最亮、光沿着边框快速流动；refine = 本机结果出来了、AI 还在细化，亮度降一档；
   * rest = 推荐结果在显示，只比 think 暗一点，照样呼吸、流动，直到清掉推荐；清掉之后 fade 慢慢退掉
   */
  const [glowFading, setGlowFading] = useState(false);
  const glowTimer = useRef<number | undefined>(undefined);
  /** 重新打开面板时清掉的推荐不放退场动画 */
  const glowQuiet = useRef(false);
  const hadSmart = useRef(false);
  useEffect(() => {
    const now = smart !== null;
    if (hadSmart.current && !now && !glowQuiet.current) {
      setGlowFading(true);
      window.clearTimeout(glowTimer.current);
      glowTimer.current = window.setTimeout(() => setGlowFading(false), 900);
    } else if (now) {
      window.clearTimeout(glowTimer.current);
      setGlowFading(false);
    }
    glowQuiet.current = false;
    hadSmart.current = now;
  }, [smart]);
  const glow: 'think' | 'refine' | 'rest' | 'fade' | null =
    smart?.phase === 'thinking' ? 'think' : smart?.phase === 'done' ? (smart.refining ? 'refine' : 'rest') : glowFading ? 'fade' : null;
  const [visible, setVisible] = useState(false);
  const [keyword, setKeyword] = useState('');
  const [debounced, setDebounced] = useState('');
  const [filter, setFilter] = useState<Filter>('all');
  /** 当前看的分组（Paste 的图钉板）；null = 剪贴板（全部历史） */
  const [board, setBoard] = useState<number | null>(null);
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

  const query = useClipItems(useMemo(() => queryOf(filter, debounced, board), [filter, debounced, board]), 50);
  const history = useMemo(() => query.data?.pages.flatMap((p) => p.items) ?? [], [query.data]);
  // 有推荐结果时：推荐的排最前（带暖色描边），后面照常是历史（去掉重复的）
  const suggestedIds = useMemo(() => new Set(smart?.phase === 'done' ? smart.items.map((i) => i.id) : []), [smart]);
  const items = useMemo(
    () => (smart?.phase === 'done' ? [...smart.items, ...history.filter((i) => !suggestedIds.has(i.id))] : history),
    [smart, history, suggestedIds],
  );
  const groups = useQuery({ queryKey: ['clip-groups'], queryFn: clipboard.groups });
  const boardGroup = board === null ? undefined : groups.data?.find((g) => g.id === board);
  const boardStyle = useMemo(() => (boardGroup ? { color: boardColor(boardGroup), name: boardGroup.name } : undefined), [boardGroup]);
  // 分组被删了（别的窗口里删的）：回到剪贴板
  useEffect(() => {
    if (board !== null && groups.data && !groups.data.some((g) => g.id === board)) setBoard(null);
  }, [board, groups.data]);
  // 卡片标题栏跟着来源应用图标的主色走（像 Paste）；后端按图标缓存，只在出现新图标时才再问
  const icons = useMemo(() => [...new Set(items.map((i) => i.sourceIcon).filter((s): s is string => !!s))].sort(), [items]);
  const iconColors = useQuery({
    queryKey: ['clip-icon-colors', icons],
    queryFn: () => clipboard.iconColors(icons),
    enabled: icons.length > 0,
    staleTime: Infinity,
    placeholderData: (prev) => prev,
  });

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
    setBoard(null);
    // 先回到起始位置再滑入（Alt+V 再按一次是 Rust 直接藏窗口的，那次没走退出动画）
    setVisible(false);
    window.clearInterval(focusWatch.current);
    window.setTimeout(() => setVisible(true), 16);
    glowQuiet.current = true;
    setSmart(null);
    setGlowFading(false);
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
    if (!item || hiding.current) return;
    hiding.current = true;
    const go = () => {
      setFlashing(null);
      setVisible(false);
      clipboard.paste(item.id, plain).catch(notify.error).finally(() => (hiding.current = false));
    };
    // 先让卡片闪一下再收起：看得出粘的是哪一张。只多 150ms
    if (fxRef.current) {
      setFlashing(item.id);
      window.setTimeout(go, 150);
    } else go();
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

  /** 每次点 ✨ 编个号：AI 细化的结果回来时，用户已经清掉 / 又点了一次的话就不要了 */
  const smartRun = useRef(0);
  /**
   * 点 ✨：看你在做什么，把最可能要粘贴的几条排到最前。
   * 先在本机按关键词挑（零点几秒就出来）；设置里开了"用 AI 模型"的，接着在后台让模型细化，回来再替换
   */
  const startSmart = useCallback(() => {
    if (smart?.phase === 'thinking') return;
    const run = ++smartRun.current;
    setSmart({ phase: 'thinking' });
    const show = (r: SmartSuggestion, refining: boolean) => {
      setSmart({ phase: 'done', refining, ...r });
      setSelected(0);
      scrollRef.current?.scrollTo({ left: 0, top: 0 });
      if (fxRef.current) {
        setEntering(true);
        window.clearTimeout(enterTimer.current);
        enterTimer.current = window.setTimeout(() => setEntering(false), 800);
      }
    };
    const useAi = !!useSettingsStore.getState().settings?.clipboard.smartAi;
    clipboard
      .smartSuggest(false)
      .then((local) => {
        if (run !== smartRun.current) return;
        if (local.items.length) show(local, useAi);
        else if (!useAi) {
          setSmart(null);
          notify.info(t('clip.smart.none'));
        } else setSmart({ phase: 'done', refining: true, ...local });
        if (!useAi) return;
        return clipboard.smartSuggest(true).then(
          (r) => {
            if (run !== smartRun.current) return;
            if (r.items.length) show(r, false);
            else if (!local.items.length) {
              setSmart(null);
              notify.info(t('clip.smart.none'));
            } else setSmart((s) => (s?.phase === 'done' ? { ...s, refining: false } : s));
          },
          // 模型不可用、超时：留着本机的结果
          () => {
            if (run !== smartRun.current) return;
            if (local.items.length) setSmart((s) => (s?.phase === 'done' ? { ...s, refining: false } : s));
            else {
              setSmart(null);
              notify.info(t('clip.smart.none'));
            }
          },
        );
      })
      .catch((err) => {
        if (run !== smartRun.current) return;
        setSmart(null);
        notify.error(err);
      });
  }, [smart, t]);
  const clearSmart = useCallback(() => {
    smartRun.current++;
    setSmart(null);
    setSelected(0);
  }, []);

  /** 卡片右上角悬停出来的快捷操作：识字、翻译、问 AI、预览 */
  const onCardAction = useCallback(
    (action: CardAction, item: ClipItem) => {
      switch (action) {
        case 'preview':
          // 图片在大预览窗口里看（能缩放），文字还在面板里就地看
          if (item.type === 'image' || isGifItem(item)) {
            hide();
            void clipboard.previewOpen(item.id).catch(notify.error);
          } else void openPreview(item);
          return;
        case 'translate':
          void openPreview(item, true);
          return;
        case 'ocr':
          hide();
          void ocr.fromClip(item.id).catch(notify.error);
          return;
        case 'ai':
          hide();
          if (item.type === 'image' && item.filePath) {
            void ai.open({ images: [{ kind: 'file', path: item.filePath }], source: 'free', text: null }).catch(notify.error);
          } else {
            void clipboard
              .get(item.id)
              .then((d) => ai.open({ images: [], source: 'free', text: d.contentText ?? item.preview ?? '' }))
              .catch(notify.error);
          }
      }
    },
    [hide, openPreview],
  );

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
      void popupMenu(
        clipMenu(item, t, { groups: groups.data ?? [], inPanel: true, inBoard: board !== null, onTranslate: (i) => void openPreview(i, true), onDialogClosed: watchFocus }),
      ).finally(watchFocus);
    },
    [t, groups.data, board, openPreview, watchFocus],
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
      // 对话框（新建分组、给卡片起名）里打字：不归面板管
      const active = document.activeElement;
      if ((active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement) && active !== searchRef.current) return;
      const searching = document.activeElement === searchRef.current;
      const item = items[selected];
      const prevKey = horizontal ? 'ArrowLeft' : 'ArrowUp';
      const nextKey = horizontal ? 'ArrowRight' : 'ArrowDown';
      if (e.key === 'Escape') {
        e.preventDefault();
        if (preview) setPreview(null);
        else if (smart?.phase === 'done') clearSmart();
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
        else if (item.type === 'image' || isGifItem(item)) {
          hide();
          void clipboard.previewOpen(item.id).catch(notify.error);
        } else void openPreview(item);
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
  }, [visible, items, selected, horizontal, preview, filter, smart, clearSmart, hide, paste, openPreview, qc, t]);

  return (
    <div
      className={clsx(
        'panel-root',
        `panel-root--${style}`,
        docked && 'panel-root--docked',
        blur && 'panel-root--blur',
        still && 'panel-root--still',
        fx && 'panel-root--fx',
        entering && 'panel-root--entering',
        smart?.phase === 'thinking' && 'panel-root--thinking',
        showKeys && 'panel-root--keys',
      )}
      data-visible={visible}
      data-smart={orbColor}
    >
      <div className="panel cn-glass-chrome">
        {/* 智能推荐模式：面板边缘的光（光晕 + 沿上下边缘游走的光 + 扫过面板的流光 + 卡片后面飘动的底光） */}
        {glow && (
          <>
            <div className={`panel__aurora panel__glow--${glow}`} aria-hidden />
            <div className={`panel__glow panel__glow--${glow}`} aria-hidden>
              <span className="panel__glow-halo" />
              <span className="panel__glow-sweep" />
              <span className="panel__glow-sheen" />
            </div>
          </>
        )}
        <header className="panel__bar">
          {/* 左：搜索（类型筛选做在搜索框里）；中：分组标签 / 推荐结果；右：智能推荐和更多 */}
          <div className="panel__side">
            {/* 智能推荐：面板最左边一颗暖色光球，只放图标 */}
            <button
              type="button"
              className={clsx('panel__smart', smart?.phase === 'thinking' && 'panel__smart--busy')}
              title={smart?.phase === 'thinking' ? t('clip.smart.thinking') : t('clip.smart.hint')}
              aria-label={t('clip.smart.button')}
              onClick={startSmart}
            >
              <WaterOrb color={orbColor} size={24} busy={smart?.phase === 'thinking'} alive={smart !== null} interactive />
            </button>
            <div className="panel__searchbox">
              <SearchField
                ref={searchRef}
                className="panel__search"
                value={keyword}
                placeholder={t('clip.search')}
                onChange={(v) => {
                  setKeyword(v);
                  setSelected(0);
                  setSmart(null);
                }}
              />
              <button
                type="button"
                className={clsx('panel__filterbtn', filter !== 'all' && 'panel__filterbtn--on')}
                title={t('clip.filterHint')}
                onClick={() =>
                  void popupMenu(
                    FILTERS.map((f) => ({
                      label: t(`clip.filter.${f}`),
                      checked: f === filter,
                      onSelect: () => {
                        setFilter(f);
                        setSelected(0);
                        setSmart(null);
                      },
                    })),
                  ).finally(watchFocus)
                }
              >
                {filter !== 'all' && <span>{t(`clip.filter.${filter}`)}</span>}
                <ListFilter size={14} strokeWidth={1.9} />
              </button>
            </div>
          </div>
          {smart?.phase === 'done' ? (
            <div className="panel__smartbar">
              {smart.refining ? <WaterOrb color={orbColor} size={14} busy /> : <Sparkles size={14} strokeWidth={2} />}
              <span className="cn-truncate">
                {smart.items.length === 0
                  ? t('clip.smart.refining')
                  : smart.app
                    ? t('clip.smart.forApp', { app: smart.app, count: smart.items.length })
                    : t('clip.smart.found', { count: smart.items.length })}
                {smart.refining && smart.items.length > 0 && <span className="panel__smartwhy"> · {t('clip.smart.refining')}</span>}
                {!smart.refining && smart.reason && <span className="panel__smartwhy"> · {smart.reason}</span>}
              </span>
              <button type="button" className="panel__smartclear" title={t('clip.smart.clear')} onClick={clearSmart}>
                <X size={13} strokeWidth={2.2} />
              </button>
            </div>
          ) : (
            <BoardTabs
              groups={groups.data ?? []}
              board={board}
              onSelect={(b) => {
                setBoard(b);
                setSelected(0);
                setSmart(null);
              }}
              onMenuClosed={watchFocus}
            />
          )}
          <div className="panel__side panel__side--end">
            <button
              type="button"
              className="panel__more"
              aria-label={t('common.more')}
              onClick={() =>
                void popupMenu([
                  {
                    label: t('clip.singleClickPaste'),
                    checked: singleClick,
                    onSelect: () => void useSettingsStore.getState().update((d) => void (d.clipboard.singleClickPaste = !singleClick)).catch(notify.error),
                  },
                  { separator: true },
                  {
                    label: t('clip.panelSettings'),
                    onSelect: () => {
                      hide();
                      void system.showMain('settings:clipboard');
                    },
                  },
                  {
                    label: t('clip.openManager'),
                    onSelect: () => {
                      hide();
                      void system.showMain('clipboard');
                    },
                  },
                  { separator: true },
                  { label: t('common.close'), onSelect: hide },
                ]).finally(watchFocus)
              }
            >
              <MoreHorizontal size={16} strokeWidth={2} />
            </button>
          </div>
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
                    style={{
                      ...(horizontal ? { transform: `translateX(${v.start}px)`, width: CARD_W } : { transform: `translateY(${v.start}px)`, height: v.size - 8, left: 8, right: 8 }),
                      // 打开时依次淡入：前几张错开一点，后面的不等
                      ['--enter-delay' as string]: `${Math.min(v.index, 7) * 32}ms`,
                    }}
                  >
                    <ClipCard
                      item={item}
                      tint={item.sourceIcon ? iconColors.data?.[item.sourceIcon] : undefined}
                      board={boardStyle}
                      suggested={suggestedIds.has(item.id)}
                      onAction={horizontal ? onCardAction : undefined}
                      index={v.index}
                      layout={horizontal ? 'card' : 'row'}
                      selected={v.index === selected}
                      flashing={flashing === item.id}
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
            <div
              className={clsx('panel__preview-box cn-glass', (preview.item.type === 'image' || isGifItem(preview.item)) && 'panel__preview-box--image')}
              onClick={(e) => e.stopPropagation()}
            >
              {preview.item.type === 'image' ? (
                <ZoomImage src={assetUrl(preview.item.filePath)} />
              ) : isGifItem(preview.item) && preview.item.thumbPath ? (
                <ZoomImage src={assetUrl(preview.item.thumbPath)} />
              ) : preview.item.type === 'otp' ? (
                <div className="panel__preview-text">
                  <OtpBody item={preview.item} variant="detail" />
                </div>
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
