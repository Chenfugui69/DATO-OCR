// 剪贴板面板（Alt+V，规格 05 §3）。默认底部横向卡片条（Paste 风格），可选竖版小面板。
//
// 全程可以不用鼠标：Alt+V → 直接打字搜索 → ←/→ 选 → Enter 粘贴到之前的窗口。

import './panel.css';
import './orb.css';

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useVirtualizer } from '@tanstack/react-virtual';
import { getCurrentWindow } from '@tauri-apps/api/window';
import clsx from 'clsx';
import {
  AppWindow,
  Clipboard,
  Folder,
  Image as ImageIcon,
  LayoutGrid,
  Link2,
  ListFilter,
  type LucideIcon,
  MoreHorizontal,
  Pin,
  Search,
  Settings,
  Sparkles,
  Star,
  Type,
  X,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { useGlassSnapshot } from '@/lib/glassSnapshot';
import { ai, clipboard, ocr, system, translate } from '@/lib/ipc';
import { useSettings, useSettingsStore } from '@/lib/settings';
import { assetUrl, isMac, modKey } from '@/lib/platform';
import { useClipItems, useLiveInvalidation } from '@/lib/queries';
import type { ClipDetail, ClipItem, ClipType, SmartSuggestion } from '@/lib/types';
import { EmptyState, SearchField, Skeleton } from '@/ui/controls';
import { appMenuClosed, isAppMenuOpen, isNativeMenuOpen, popupAppMenu, runAppMenuPick } from '@/ui/nativeMenu';
import { DropdownMenu, notify } from '@/ui/overlays';

import { clipMenu } from './actions';
import { BoardTabs, boardColor, moveToBoard } from './BoardTabs';
import { useCardDrag, useKeyHeld, useMultiPick } from './cardGestures';
import { type CardAction, ClipCard, isGifItem } from './ClipCard';
import { type DateRange, FilterPanel, rangeBounds } from './FilterPanel';
import { HScrollbar } from './HScrollbar';
import { OtpBody } from './OtpBody';
import { PickBar } from './PickBar';
import { type OrbColor, ORB_COLORS, WaterOrb } from './WaterOrb';
import { ZoomImage } from './ZoomImage';

type Filter = 'all' | ClipType | 'pinned' | 'favorite';
/** 底部卡片条：卡片宽度和间距（高度见 panel.css 的 .panel__cell） */
/** 一张卡片的回调（按 id 缓存，见 PanelView 里的 handlersFor） */
type CardHandlers = {
  onHover: (inside: boolean) => void;
  onClick: () => void;
  onPaste: () => void;
  onContextMenu: (e: React.MouseEvent) => void;
  onPick: () => void;
  onDragPointerDown: (e: React.PointerEvent<HTMLDivElement>) => void;
};

// 卡片 232×232 的方卡片、间距 24、离面板左右边 24：都是照 Paste 实际截图量的
const CARD_W = 232;
const CARD_GAP = 24;
const CARD_INSET = 24;
/** 筛选面板弹在横向面板上方：窗口临时往上长这么多，筛选面板宽这么多（逻辑像素） */
const FILTER_RAISE = 440;
const FILTER_W = 460;
const FILTERS: Filter[] = ['all', 'text', 'image', 'link', 'files', 'pinned', 'favorite'];
const FILTER_ICONS: Partial<Record<Filter, LucideIcon>> = {
  all: LayoutGrid,
  text: Type,
  image: ImageIcon,
  link: Link2,
  files: Folder,
  pinned: Pin,
  favorite: Star,
};

function queryOf(filter: Filter, keyword: string, board: number | null, app: string | null, range: DateRange | null) {
  return {
    keyword: keyword || undefined,
    groupId: board,
    app,
    ...rangeBounds(range),
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
  useGlassSnapshot();
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
  // 筛选面板的另外两节：来源应用、日期（类型就是 filter，分组就是 board）
  const [app, setApp] = useState<string | null>(null);
  const [range, setRange] = useState<DateRange | null>(null);
  const [filterOpen, setFilterOpen] = useState(false);
  const filterPanelRef = useRef<HTMLDivElement>(null);
  const filterBtnRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  /** 横向面板的筛选面板弹在面板上方（照 Paste）：窗口临时往上长 FILTER_RAISE，弹出的位置（相对窗口） */
  const [raised, setRaised] = useState<{ panelH: number; left: number; bottom: number; arrow: number } | null>(null);
  /** 面板每弹出一次加一（窗口往上长着时面板被收起、又弹出来：那次的位置已经重新摆过，不能再按老位置缩回去） */
  const showGen = useRef(0);
  /** 当前看的分组（Paste 的图钉板）；null = 剪贴板（全部历史） */
  const [board, setBoard] = useState<number | null>(null);
  const [selected, setSelected] = useState(0);
  // 选中框只在用户自己挑过之后才画（方向键、点卡片）：刚打开时第一张就带着框，看着像点过它。
  // 回车粘贴的照样是第一张
  const [showSel, setShowSel] = useState(false);
  const [preview, setPreview] = useState<Preview | null>(null);
  // 按住 Ctrl 一小会儿才亮出卡片上的 Ctrl+1–9（Ctrl+P / Ctrl+D 这种一按就松的不闪一下）
  const [showKeys, setShowKeys] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  // 横向面板的搜索照 Paste 收在中间：平时只是一个放大镜，点它 / 直接打字 / 搜着词 / 选了筛选时才展开成一条搜索框
  const [searchFocus, setSearchFocus] = useState(false);
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

  const query = useClipItems(useMemo(() => queryOf(filter, debounced, board, app, range), [filter, debounced, board, app, range]), 50);
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
    paddingStart: horizontal ? CARD_INSET : 8,
    paddingEnd: horizontal ? CARD_INSET : 8,
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

  // ── 多选（Ctrl 点选 / 框选）、拖出去粘贴、按住 Shift 悬浮预览 ──
  // 顶栏的空白处也能按住 Ctrl 拉框（框的还是下面的卡片）
  const barRef = useRef<HTMLElement>(null);
  const { picked, toggle: togglePick, clear: clearPicks } = useMultiPick(scrollRef, visible, [barRef]);
  /** 勾上的按列表里的顺序排（复制、粘贴时就是这个顺序） */
  const pickedInOrder = useCallback(() => items.filter((i) => picked.has(i.id)).map((i) => i.id), [items, picked]);
  // 换了筛选 / 分组 / 搜索词，勾的那些多半不在眼前了：清掉
  useEffect(() => clearPicks(), [filter, app, range, board, debounced, clearPicks]);
  // 筛选面板开着：点它和筛选按钮以外的地方就收起
  useEffect(() => {
    if (!filterOpen) return;
    const onDown = (e: PointerEvent) => {
      const target = e.target as Node;
      if (filterPanelRef.current?.contains(target) || filterBtnRef.current?.contains(target)) return;
      // 点在窗口往上长出来的那截空白里：按下之后窗口就缩回去了，不拦的话松手时会在面板里拉出一片选中的字
      if (!panelRef.current?.contains(target)) e.preventDefault();
      setFilterOpen(false);
    };
    document.addEventListener('pointerdown', onDown, true);
    return () => document.removeEventListener('pointerdown', onDown, true);
  }, [filterOpen]);
  // 横向面板打开筛选面板：窗口一步往上长 FILTER_RAISE（毛玻璃背板同一帧改成只铺下面原来那块），
  // 面板在页面里钉在底部、高度不变，筛选面板画在长出来的那一截里；收起时窗口缩回去
  useEffect(() => {
    if (!filterOpen || !horizontal) return;
    const panel = panelRef.current;
    const btn = filterBtnRef.current;
    if (!panel || !btn) return;
    const p = panel.getBoundingClientRect();
    const b = btn.getBoundingClientRect();
    const cx = b.left + b.width / 2;
    const left = Math.round(Math.min(Math.max(cx - FILTER_W / 2, 12), window.innerWidth - FILTER_W - 12));
    const gen = showGen.current;
    const blurNow = blurRef.current;
    const win = getCurrentWindow();
    let done = false;
    let base: { x: number; y: number; w: number; h: number } | null = null;
    void (async () => {
      const s = await win.scaleFactor();
      const pos = await win.outerPosition();
      const size = await win.outerSize();
      if (done) return;
      base = { x: pos.x / s, y: pos.y / s, w: size.width / s, h: size.height / s };
      // 先把面板钉住（固定高度、靠底），再长窗口：窗口变高的那一帧面板不会被拉伸
      setRaised({ panelH: p.height, left, bottom: window.innerHeight - p.top + 2, arrow: cx - left });
      await new Promise(requestAnimationFrame);
      if (done) return;
      await system.setBounds(base.x, base.y - FILTER_RAISE, base.w, base.h + FILTER_RAISE, blurNow ? { x: 0, y: FILTER_RAISE, width: base.w, height: base.h } : undefined);
    })().catch(notify.error);
    return () => {
      done = true;
      const b0 = base;
      if (!b0 || showGen.current !== gen) {
        setRaised(null);
        return;
      }
      void system
        .setBounds(b0.x, b0.y, b0.w, b0.h, blurNow ? { x: 0, y: 0, width: b0.w, height: b0.h } : undefined)
        .catch(notify.error)
        .finally(() => {
          if (showGen.current === gen) setRaised(null);
        });
    };
  }, [filterOpen, horizontal]);
  // 一按下 Ctrl 就出勾选框（不像 Ctrl+1–9 的提示那样等一下）
  const ctrlHeld = useKeyHeld('Control', visible);
  const pickMode = ctrlHeld || picked.size > 0;
  const copyPicked = useCallback(() => {
    const ids = pickedInOrder();
    if (!ids.length) return;
    clipboard
      .copyMany(ids)
      .then(() => notify.success(t('clip.copiedMany', { count: ids.length })))
      .catch(notify.error);
  }, [pickedInOrder, t]);
  const { dragging, onPointerDown: onCardPointerDown } = useCardDrag({
    idsFor: (id) => (picked.has(id) ? pickedInOrder() : [id]),
    // 拖着的那张放最上面
    itemsFor: (ids) =>
      ids
        .map((id) => items.find((i) => i.id === id))
        .filter((i): i is ClipItem => !!i)
        .map((i) => ({ ...i, ghostTint: i.sourceIcon ? iconColors.data?.[i.sourceIcon] : undefined })),
    // 松手时面板还是"让开"的（滑走、玻璃淡掉、鼠标穿透），就这样收起；窗口藏好之后（粘贴完）再复原，
    // 不然先滑回来再收起会闪一下
    onDropOutside: (ids) => {
      if (hiding.current) {
        resetAway();
        return;
      }
      hiding.current = true;
      setVisible(false);
      clipboard
        .dropPaste(ids)
        .catch(notify.error)
        .finally(() => {
          hiding.current = false;
          resetAway();
        });
    },
    onDropOnBoard: (ids, target) => moveToBoard(ids, target, groups.data ?? [], t),
    // macOS：系统原生拖拽（跟手、目标程序自己收）。被别的程序收下时 Rust 已经藏好了面板，这里只复原状态
    native: isMac
      ? {
          onDropped: () => {
            setVisible(false);
            resetAway();
          },
        }
      : undefined,
    // 卡片拖出卡片区（往上拖到顶栏、或者出了面板）：面板滑下去让开、不挡鼠标，面板后面（比如屏幕底部的
    // 输入框）才看得见、拖得进去。顶栏上的分组标签除外，那是拖进分组用的
    awayFrom: () => scrollRef.current,
    onAway: (on) => {
      setAway(on);
      const win = getCurrentWindow();
      void win.setIgnoreCursorEvents(on).catch(() => undefined);
      // 毛玻璃：背板是系统挂在窗口底下的，页面滑走它还在，跟着淡出 / 淡入
      if (blurRef.current) void system.backdropFade(!on, 160).catch(() => undefined);
    },
  });
  const [away, setAway] = useState(false);
  // 撤销"让开"，不带动画（面板已经藏起来了，或者马上要重新出场）
  const resetAway = () => {
    setAway(false);
    void getCurrentWindow().setIgnoreCursorEvents(false).catch(() => undefined);
    void system.backdropFade(true, 0).catch(() => undefined);
  };
  const draggingIds = useMemo(() => new Set(dragging ?? []), [dragging]);
  // macOS 系统拖拽接管鼠标以后，面板窗口有时会失焦：那次不算"点了外面"，不收起；拖完面板还开着（取消、
  // 拖到分组标签上）就把焦点拿回来，不然之后点外面也不会再失焦、面板就收不起来了
  const draggingRef = useRef(false);
  draggingRef.current = !!dragging;
  const blurredWhileDragging = useRef(false);
  useEffect(() => {
    if (dragging || !blurredWhileDragging.current) return;
    blurredWhileDragging.current = false;
    if (visibleRef.current && !hiding.current) void getCurrentWindow().setFocus().catch(() => undefined);
  }, [dragging]);

  const shiftHeld = useKeyHeld('Shift', visible);
  const [hoverId, setHoverId] = useState<number | null>(null);
  useEffect(() => {
    if (!shiftHeld || hoverId === null || dragging) {
      void clipboard.peekHide().catch(() => undefined);
      return;
    }
    const card = scrollRef.current?.querySelector<HTMLElement>(`[data-clip-id="${hoverId}"]`);
    if (!card) return;
    const r = card.getBoundingClientRect();
    void clipboard.peekShow(hoverId, { x: r.left, y: r.top, width: r.width, height: r.height }).catch(() => undefined);
  }, [shiftHeld, hoverId, dragging]);

  // 横向卡片条：鼠标滚轮竖着滚 = 横着走，带一点缓动，不是一格一格地跳。
  // 触控板本来就会横向滑动，交给浏览器。两种手感（设置里切）：
  // - smooth（默认）：按时间算的指数缓动（高刷屏、普通屏一样快），位置用小数记（不在整像素上抖），
  //   连着快速滚几格会越滚越远，触控板竖着划时几乎直接跟手
  // - classic：原来的，每帧走剩下距离的 24%
  const scrollFeel = settings?.clipboard.scrollFeel === 'classic' ? 'classic' : 'smooth';
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || !horizontal || scrollFeel !== 'smooth') return;
    let pos = el.scrollLeft;
    let target = pos;
    let raf = 0;
    let last = 0;
    let tau = 90;
    let lastWheel = 0;
    let streak = 0;
    const max = () => el.scrollWidth - el.clientWidth;
    const step = (now: number) => {
      const dt = Math.min(48, now - (last || now - 16));
      last = now;
      pos += (target - pos) * (1 - Math.exp(-dt / tau));
      if (Math.abs(target - pos) < 0.5) pos = target;
      el.scrollLeft = pos;
      raf = pos === target ? 0 : requestAnimationFrame(step);
    };
    const onWheel = (e: WheelEvent) => {
      if (Math.abs(e.deltaX) > Math.abs(e.deltaY) || e.ctrlKey) return;
      e.preventDefault();
      // 别处挪过（拖滚动条、键盘选卡片自动滚过去）：从现在的位置接着走
      if (!raf || Math.abs(el.scrollLeft - pos) > 2) {
        pos = el.scrollLeft;
        target = pos;
      }
      const unit = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? el.clientWidth : 1;
      const dy = e.deltaY * unit;
      // 触控板：一次来很多个小增量（常带小数）。几乎直接跟手
      const pad = e.deltaMode === 0 && (Math.abs(dy) < 40 || !Number.isInteger(dy));
      const now = performance.now();
      streak = now - lastWheel < 140 ? Math.min(streak + 1, 6) : 0;
      lastWheel = now;
      const boost = pad ? 1 : 1 + streak * 0.18;
      tau = pad ? 35 : 90;
      target = Math.max(0, Math.min(max(), target + dy * (pad ? 1.2 : 1.5) * boost));
      if (!raf) {
        last = 0;
        raf = requestAnimationFrame(step);
      }
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => {
      el.removeEventListener('wheel', onWheel);
      cancelAnimationFrame(raf);
    };
  }, [horizontal, scrollFeel]);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || !horizontal || scrollFeel !== 'classic') return;
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
  }, [horizontal, scrollFeel]);

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
    showGen.current += 1;
    setRaised(null);
    hiding.current = false;
    resetAway();
    setStyle(p.style);
    setKeyword('');
    setDebounced('');
    setFilter('all');
    setApp(null);
    setRange(null);
    setFilterOpen(false);
    setSelected(0);
    setShowSel(false);
    setPreview(null);
    setBoard(null);
    clearPicks();
    // 拖卡片的影子窗口先建好，第一次拖不用等（macOS 用系统原生拖拽，没有影子窗口）
    if (!isMac) void clipboard.ghostPrepare().catch(() => undefined);
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
        if (focused) appMenuClosed();
        if (!focused && draggingRef.current) {
          blurredWhileDragging.current = true;
          return;
        }
        if (!focused && !isNativeMenuOpen() && !isAppMenuOpen()) hide();
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
      // 自己画的菜单开着：前台是菜单窗口，不算面板失焦（菜单关了会发 popmenu-closed，再接着看）
      if (isNativeMenuOpen() || isAppMenuOpen()) return;
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
      // 自己画的菜单窗口（和托盘、贴图的右键菜单同一个）：图标、圆角、深浅色和面板一致，比系统菜单好看；
      // 菜单窗口会拿走焦点，面板这边先别因为失焦收起，改成盯着"前台还是不是我们的窗口"
      void popupAppMenu(
        getCurrentWindow().label,
        clipMenu(item, t, { groups: groups.data ?? [], inPanel: true, inBoard: board !== null, onTranslate: (i) => void openPreview(i, true), onDialogClosed: watchFocus }),
      )
        .catch(notify.error)
        .finally(watchFocus);
    },
    [t, groups.data, board, openPreview, watchFocus],
  );
  useEvent('popmenu-select', runAppMenuPick);
  useEvent('popmenu-closed', () => {
    appMenuClosed();
    watchFocus();
  });

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
        else if (picked.size) clearPicks();
        else if (filterOpen) setFilterOpen(false);
        else if (searching || keyword || filter !== 'all' || app || range) {
          // 搜索 / 筛选着：先清掉搜索词和筛选（横向面板的搜索框跟着收起），再按一次才关面板
          setKeyword('');
          setFilter('all');
          setApp(null);
          setRange(null);
          setSelected(0);
          searchRef.current?.blur();
        } else if (smart?.phase === 'done') clearSmart();
        else hide();
        return;
      }
      if (e.key === nextKey) {
        e.preventDefault();
        // 第一次按：先把框画在现在这张上，再按才往后走
        if (showSel) setSelected((i) => Math.min(items.length - 1, i + 1));
        setShowSel(true);
        return;
      }
      if (e.key === prevKey) {
        e.preventDefault();
        if (showSel) setSelected((i) => Math.max(0, i - 1));
        setShowSel(true);
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
      // 多选了几张：Ctrl+C 复制勾上的（搜索框里选着字时照常复制那段字）
      if (picked.size && modKey(e) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'c') {
        const input = searchRef.current;
        const hasTextSel = searching && input && input.selectionStart !== input.selectionEnd;
        if (!hasTextSel) {
          e.preventDefault();
          copyPicked();
          return;
        }
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
  }, [visible, items, selected, showSel, horizontal, preview, filter, app, range, filterOpen, keyword, smart, clearSmart, hide, paste, openPreview, qc, t, picked, clearPicks, copyPicked]);

  // 每张卡片的回调按卡片 id 缓存成固定的函数，调用时再取最新的状态。卡片组件做了 memo，
  // 回调要是每次渲染都新建，memo 就白做了：面板跟着滚动、悬停换卡片一次次重新渲染时，
  // 可见的二十来张卡片会全部重画，滚动时一帧要 20~50ms（2026-11 在 165Hz 屏上量出来的卡顿）
  const live = useRef({ items, paste, showMenu, togglePick, onCardPointerDown });
  live.current = { items, paste, showMenu, togglePick, onCardPointerDown };
  const cardHandlers = useRef(new Map<number, CardHandlers>());
  const handlersFor = (id: number): CardHandlers => {
    let h = cardHandlers.current.get(id);
    if (!h) {
      const find = () => live.current.items.find((i) => i.id === id);
      const select = () => {
        const index = live.current.items.findIndex((i) => i.id === id);
        if (index >= 0) setSelected(index);
        setShowSel(true);
      };
      h = {
        onHover: (inside) => setHoverId((cur) => (inside ? id : cur === id ? null : cur)),
        onClick: select,
        onPaste: () => live.current.paste(find()),
        onContextMenu: (e) => {
          e.preventDefault();
          select();
          const item = find();
          if (item) live.current.showMenu(item);
        },
        onPick: () => live.current.togglePick(id),
        onDragPointerDown: (e) => live.current.onCardPointerDown(e, id),
      };
      cardHandlers.current.set(id, h);
    }
    return h;
  };


  const filtered = filter !== 'all' || app !== null || range !== null;
  const searchOpen = horizontal && (searchFocus || keyword !== '' || filtered || filterOpen);
  /** 筛选按钮上写着选了什么（"图片 · Claude · 今天"） */
  const filterLabel = [filter !== 'all' ? t(`clip.filter.${filter}`) : null, app, range ? t(`clip.filterPanel.${range}`) : null].filter(Boolean).join(' · ');
  const searchBox = (
    <div className="panel__searchbox">
      <SearchField
        ref={searchRef}
        className="panel__search"
        value={keyword}
        placeholder={t('clip.search')}
        onFocus={() => setSearchFocus(true)}
        onBlur={() => setSearchFocus(false)}
        onChange={(v) => {
          setKeyword(v);
          setSelected(0);
          setSmart(null);
        }}
      />
      {/* 筛选：点开筛选面板（FilterPanel，类型 / 应用 / 分组 / 日期）。按下时不抢搜索框的焦点 */}
      <button
        ref={filterBtnRef}
        type="button"
        className={clsx('panel__filterbtn', filtered && 'panel__filterbtn--on', filterOpen && 'panel__filterbtn--open')}
        title={t('clip.filterHint')}
        aria-expanded={filterOpen}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => setFilterOpen((o) => !o)}
      >
        {filterLabel && <span className="cn-truncate">{filterLabel}</span>}
        <ListFilter size={14} strokeWidth={1.9} />
      </button>
    </div>
  );
  const centerTabs =
    smart?.phase === 'done' ? (
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
        compact={searchOpen}
      >
        {/* 多选了几张：跟在分组标签后面，复制 / 取消（也可以 Ctrl+C、Esc），勾上的还能一起拖出去粘贴 */}
        <PickBar count={picked.size} onCopy={copyPicked} onClear={clearPicks} />
      </BoardTabs>
    );

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
        dragging && 'panel-root--dragging',
        away && 'panel-root--away',
        raised && 'panel-root--raised',
      )}
      style={raised ? ({ '--panel-h': `${raised.panelH}px` } as React.CSSProperties) : undefined}
      data-visible={visible}
      data-smart={orbColor}
    >
      {filterOpen && horizontal && raised && (
        <FilterPanel
          ref={filterPanelRef}
          above={{ left: raised.left, bottom: raised.bottom, arrow: raised.arrow, width: FILTER_W }}
          types={FILTERS.filter((f): f is Exclude<Filter, 'all'> => f !== 'all')}
          typeIcons={FILTER_ICONS}
          type={filter === 'all' ? null : filter}
          onType={(f) => {
            setFilter(f ?? 'all');
            setSelected(0);
            setSmart(null);
          }}
          app={app}
          onApp={(a) => {
            setApp(a);
            setSelected(0);
            setSmart(null);
          }}
          groups={groups.data ?? []}
          board={board}
          onBoard={(b) => {
            setBoard(b);
            setSelected(0);
            setSmart(null);
          }}
          range={range}
          onRange={(r) => {
            setRange(r);
            setSelected(0);
            setSmart(null);
          }}
        />
      )}
      <div ref={panelRef} className="panel cn-glass-chrome">
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
        <header ref={barRef} className={clsx('panel__bar', searchOpen && 'panel__bar--search')}>
          {/* 竖版：左边水珠 + 搜索框（类型筛选做在框里），中间分组标签 / 推荐结果，右边更多。
              横向（照 Paste）：左边只有水珠（和下面第一张卡片左边对齐），中间放大镜 + 分组标签 + 新建，
              搜索时中间展开成一条搜索框、分组标签缩成只剩图标；右边更多 */}
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
            {!horizontal && searchBox}
          </div>
          {horizontal ? (
            <div className="panel__center">
              <button
                type="button"
                className="panel__searchbtn"
                title={t('clip.search')}
                aria-label={t('clip.search')}
                tabIndex={-1}
                onClick={() => searchRef.current?.focus()}
              >
                <Search size={19} strokeWidth={2} />
              </button>
              {searchBox}
              {centerTabs}
            </div>
          ) : (
            centerTabs
          )}
          <div className="panel__side panel__side--end">
            {/* 应用自己的菜单（不用系统菜单：Windows 上系统菜单是老式样式）。只有几项、从顶栏往下开，面板里放得下 */}
            <DropdownMenu
              items={[
                {
                  label: t('clip.singleClickPaste'),
                  checked: singleClick,
                  onSelect: () => void useSettingsStore.getState().update((d) => void (d.clipboard.singleClickPaste = !singleClick)).catch(notify.error),
                },
                { separator: true },
                {
                  label: t('clip.panelSettings'),
                  icon: Settings,
                  onSelect: () => {
                    hide();
                    void system.showMain('settings:clipboard');
                  },
                },
                {
                  label: t('clip.openManager'),
                  icon: AppWindow,
                  onSelect: () => {
                    hide();
                    void system.showMain('clipboard');
                  },
                },
                { separator: true },
                { label: t('common.close'), icon: X, onSelect: hide },
              ]}
            >
              <button type="button" className="panel__more" aria-label={t('common.more')}>
                <MoreHorizontal size={18} strokeWidth={2.25} />
              </button>
            </DropdownMenu>
          </div>
        </header>
        {filterOpen && !horizontal && (
          <FilterPanel
            ref={filterPanelRef}
            types={FILTERS.filter((f): f is Exclude<Filter, 'all'> => f !== 'all')}
            typeIcons={FILTER_ICONS}
            type={filter === 'all' ? null : filter}
            onType={(f) => {
              setFilter(f ?? 'all');
              setSelected(0);
              setSmart(null);
            }}
            app={app}
            onApp={(a) => {
              setApp(a);
              setSelected(0);
              setSmart(null);
            }}
            groups={groups.data ?? []}
            board={board}
            onBoard={(b) => {
              setBoard(b);
              setSelected(0);
              setSmart(null);
            }}
            range={range}
            onRange={(r) => {
              setRange(r);
              setSelected(0);
              setSmart(null);
            }}
          />
        )}

        <div ref={scrollRef} className="panel__list">
          {query.isLoading ? (
            <div className="panel__loading">
              <Skeleton />
            </div>
          ) : items.length === 0 ? (
            <EmptyState icon={Clipboard} title={debounced ? t('clip.noMatch') : t('clip.empty')} description={debounced ? undefined : t('clip.emptyDesc')} />
          ) : (
            <div data-marquee-origin style={horizontal ? { width: virt.getTotalSize(), height: '100%', position: 'relative' } : { height: virt.getTotalSize(), position: 'relative' }}>
              {virtualItems.map((v) => {
                const item = items[v.index]!;
                const h = handlersFor(item.id);
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
                      selected={showSel && v.index === selected}
                      flashing={flashing === item.id}
                      pickMode={pickMode}
                      picked={picked.has(item.id)}
                      onPick={h.onPick}
                      dragging={draggingIds.has(item.id)}
                      onDragPointerDown={h.onDragPointerDown}
                      onHover={h.onHover}
                      onClick={h.onClick}
                      onActivate={singleClick ? h.onPaste : undefined}
                      onDoubleClick={singleClick ? undefined : h.onPaste}
                      onContextMenu={h.onContextMenu}
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
