// 划词翻译面板：鼠标 / 划词按钮旁边弹出，点外面或 Esc 关闭。
//
// - 顶部默认不显示原文（设置里可开），语言那一行右侧是"问 AI"和关闭；底部一条"问 AI"输入框
// - 输入翻译（热键 / 托盘）：同一个面板，顶上是输入框，打字停一下就翻；默认钉住（点外面不收，免得打的字没了）
// - 原文、译文旁边都有小喇叭，点一下朗读
// - 外观（宽高、字号、不透明度、圆角、毛玻璃）按设置；右下角手柄拖拽改大小，改完记进设置
// - 问 AI 有两种展开方式（设置里选）：
//   - 抽屉（默认）：面板往下长高，对话区像一张卡片从底部往上滑出来，压在翻译上面；
//     抽屉顶上的小横条可以上下拖，调翻译和对话各占多少
//   - 侧边：面板往右长出对话区，翻译卡片压在对话区上面；中间的缝可以左右拖
//   对话期间点别处不收起（不然对话就没了），也可以挪到独立窗口里接着聊
// - 展开 / 收起的动画：窗口先一步到位（一次原生调用同时改位置和大小），外框先按原来的尺寸
//   定住，看起来什么都没变；然后外框用 CSS 过渡长到新尺寸、对话区滑进来。窗口尺寸和动画
//   分开做，动画过程中窗口不再变，才不会一顿一顿地跳
// - 每块区域只涂一层底色（卡片圆角外露出来的那一小块用径向渐变补上），调不透明度时
//   各处一样透，不会因为叠了两层而一块透一块不透

import '@/views/ocr/ocr.css';

import { getCurrentWindow } from '@tauri-apps/api/window';
import { ArrowUp, ChevronDown, ExternalLink, Pin, Sparkles, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { useGlassSnapshot } from '@/lib/glassSnapshot';
import { isMac } from '@/lib/platform';
import { ai, system, translate } from '@/lib/ipc';
import { useSettings, useSettingsStore } from '@/lib/settings';
import { stopSpeaking } from '@/lib/speech';
import type { ChatTurn } from '@/lib/types';
import { IconButton } from '@/ui/controls';
import { SpeakButton } from '@/ui/SpeakButton';
import { AiChat } from '@/views/ai/AiChat';

import { TranslateBox } from './TranslateBox';

/** 窗口四周留给阴影的透明边，要装得下下面 .pop-shell 的整个阴影（下方 4 + 12 = 16），
 *  装不下的话阴影在窗口边上被齐齐切断，看着像一圈方框。毛玻璃模式不画阴影，不留 */
const SHADOW_PAD = 16;
const MIN_AI = 280;
const MIN_TRANSLATE = 360;
const MIN_TRANSLATE_H = 110;
const MIN_DRAWER = 200;

type Layout = 'drawer' | 'side';

/**
 * 展开 / 收起过渡中外框的样子：尺寸、在窗口里的偏移（窗口为了放得下往回挪了多少，外框就往反方向
 * 偏多少，屏幕上看位置不变）、对话区有没有滑进来、这一步要不要动画（补偿窗口挪动的那一步不能有）
 */
interface Sheet {
  w: number;
  h: number;
  x: number;
  y: number;
  open: boolean;
  animate: boolean;
  /** 展开后外框的完整尺寸：对话区始终按这个大小摆，靠位移滑进滑出 */
  fw: number;
  fh: number;
  /**
   * 外框贴着底边长高 / 缩回（抽屉在窗口里原地往上长的时候）：只过渡高度，不再"一边往上挪一边长高"。
   * 位移是合成器做的、高度是主线程排版做的，两个不在一个节拍上，同时用的话底边会在动画中途抖上去几像素，
   * 露出一条没盖住的毛玻璃。
   */
  bottom?: boolean;
}

/** 面板四边、四角的拖动改大小区 */
const RESIZE_EDGES = [
  ['North', 'n'],
  ['South', 's'],
  ['West', 'w'],
  ['East', 'e'],
  ['NorthWest', 'nw'],
  ['NorthEast', 'ne'],
  ['SouthWest', 'sw'],
] as const;

const ANIM_MS = 340;
/** 面板弹出 / 收起的时长（Windows，见 appear / hide） */
const APPEAR_MS = 300;
const LEAVE_MS = 140;

/** 面板窗口在哪、所在屏的可用区域（逻辑像素）。 */
async function whereAmI(): Promise<{ pos: { x: number; y: number }; mx: number; my: number; mw: number; mh: number }> {
  const p = await system.placement().catch(() => null);
  if (p) return { pos: { x: p.x, y: p.y }, mx: p.workX, my: p.workY, mw: p.workWidth, mh: p.workHeight };
  const scr = window.screen as Screen & { availLeft?: number; availTop?: number };
  return { pos: { x: window.screenX, y: window.screenY }, mx: scr.availLeft ?? 0, my: scr.availTop ?? 0, mw: scr.availWidth, mh: scr.availHeight };
}

type ResizeDirection = Parameters<ReturnType<typeof getCurrentWindow>['startResizeDragging']>[0];

/** 面板最小多大（和 Rust 建窗时的 min_inner_size 一致） */
const MIN_W = 320;
const MIN_H = 200;

/**
 * 鼠标已经按在面板的边 / 角上：开始跟着鼠标改大小。`top`：窗口顶上不属于面板的预留区有多高。
 * macOS 上 Tauri 自带的那个没实现，走后端自己做的。
 */
function startResize(direction: ResizeDirection, top: number): void {
  void system
    .startResize(direction, MIN_W, MIN_H + top, top)
    .catch(() => false)
    .then((handled) => {
      if (!handled) void getCurrentWindow().startResizeDragging(direction);
    });
}

/** 鼠标已经按在面板上：开始挪面板。macOS 上系统自己的拖动会被顶上的预留区卡住，走后端自己做的。 */
function startMove(top: number): void {
  void system
    .startResize('Move', 0, 0, top)
    .catch(() => false)
    .then((handled) => {
      if (!handled) void getCurrentWindow().startDragging();
    });
}
const EASE = 'cubic-bezier(0.32, 0.72, 0, 1)';
const frame = () => new Promise<void>((r) => requestAnimationFrame(() => r()));
const wait = (ms: number) => new Promise<void>((r) => window.setTimeout(r, ms));


/**
 * 改窗口位置大小，网页这边外框已经提前往反方向挪好了：两件事要落在同一帧，不然会闪一下。
 * 网页改了之后要过一两帧才真正上屏，改窗口却几乎立刻生效，所以等网页这一帧交出去了再改窗口。
 */
async function moveWindow(
  x: number,
  y: number,
  w: number,
  h: number,
  backdrop?: { x: number; y: number; width: number; height: number },
) {
  const delay = (window as unknown as { __moveDelay?: number }).__moveDelay ?? MOVE_DELAY_FRAMES;
  for (let i = 0; i < delay; i += 1) await frame();
  await system.setBounds(x, y, w, h, backdrop);
}

/**
 * 面板往外长的时候，毛玻璃背板的动画晚几帧再开始。背板是系统合成器做的动画，命令一到就动；页面的
 * 过渡要过两三帧才上屏。同时开始的话背板跑在前面，面板还没长到那里就先露出一条空的毛玻璃
 * （连拍里看得到，背后是浅色窗口时很显眼）。晚一点只是面板最前沿那一小条暂时没有模糊，看不出来。
 * 收起时相反，背板先缩没关系，不用等
 */
async function lagBackdrop(shell?: HTMLElement | null) {
  // 先等页面这边的过渡真的开始（刚挂上对话区、主线程正忙的时候，可能要过好几帧），再多让两帧
  const running = shell?.getAnimations?.() ?? [];
  await Promise.race([Promise.all(running.map((a) => a.ready.catch(() => undefined))), wait(160)]);
  for (let i = 0; i < 2; i += 1) await frame();
}

/** 改窗口前等几帧（见 moveWindow） */
const MOVE_DELAY_FRAMES = 1;

/** 按住拖动，松手回调（用来保存）。 */
function dragHandle(e: React.PointerEvent<HTMLElement>, move: (ev: PointerEvent) => void, done: () => void) {
  e.preventDefault();
  const handle = e.currentTarget;
  handle.setPointerCapture(e.pointerId);
  handle.dataset.dragging = '';
  const up = () => {
    window.removeEventListener('pointermove', move);
    window.removeEventListener('pointerup', up);
    delete handle.dataset.dragging;
    done();
  };
  window.addEventListener('pointermove', move);
  window.addEventListener('pointerup', up);
}

export default function TranslatePopup() {
  const { t } = useTranslation();
  useGlassSnapshot();
  const settings = useSettings();
  const popup = settings?.translate.popup;
  const blur = !!popup?.blur;
  const pad = blur ? 0 : SHADOW_PAD;
  const [text, setText] = useState('');
  const [seq, setSeq] = useState(0);
  const [mode, setMode] = useState<'translate' | 'ai'>('translate');
  /** 这次展开用的布局（展开后改设置不影响已经开着的） */
  const [layout, setLayout] = useState<Layout>('drawer');
  const [question, setQuestion] = useState('');
  /** 原文在编辑：点原文那一栏就变成输入框，改完自动重翻 */
  const [editingSource, setEditingSource] = useState(false);
  /** 输入翻译：原文一直是输入框 */
  const [inputMode, setInputMode] = useState(false);
  const sourceRef = useRef<HTMLTextAreaElement>(null);
  const [pendingAsk, setPendingAsk] = useState<{ id: number; text: string } | null>(null);
  /** 侧边：翻译卡片宽度；抽屉：翻译区高度（逻辑像素） */
  const [transW, setTransW] = useState(440);
  const [transH, setTransH] = useState(240);
  const modeRef = useRef(mode);
  modeRef.current = mode;
  const layoutRef = useRef(layout);
  layoutRef.current = layout;
  const padRef = useRef(pad);
  padRef.current = pad;
  /** 用户在拖右下角手柄：只有这时的尺寸变化才记进设置（弹出时 Rust 摆位、展开 AI 也会改大小） */
  const userResizing = useRef(false);
  /** 这次改大小已经真的动过（没动过的话一会儿就把 userResizing 放掉，免得之后点外面收不起来） */
  const resized = useRef(false);
  const resizeTimer = useRef<number | undefined>(undefined);
  const resizeGuard = useRef<number | undefined>(undefined);
  /** 用户开始改大小：按住了面板边上的改大小区，或者窗口边上系统那圈边框 */
  const beginResize = useCallback(() => {
    userResizing.current = true;
    resized.current = false;
    window.clearTimeout(resizeGuard.current);
    resizeGuard.current = window.setTimeout(() => {
      if (!resized.current) userResizing.current = false;
    }, 1500);
  }, []);
  /** 置顶（钉住）：点面板外面也不收起（Esc、关闭按钮照样关） */
  const [pinned, setPinned] = useState(false);
  const pinnedRef = useRef(pinned);
  pinnedRef.current = pinned;
  /** 用户在拖面板挪位置（按住空白处拖）：系统拖动期间会短暂拿走焦点，这时不能当成"点了外面" */
  const userMoving = useRef(false);
  const moveTimer = useRef<number | undefined>(undefined);
  const turnsRef = useRef<ChatTurn[]>([]);
  const keepTurns = useCallback((turns: ChatTurn[]) => void (turnsRef.current = turns), []);
  const transRef = useRef<HTMLDivElement>(null);
  /** 展开对话前的窗口大小，收起时还原 */
  const baseSize = useRef({ w: 460, h: 300 });
  /** 展开时为了放得下，窗口往左 / 往上挪了多少；收起时挪回去 */
  const shift = useRef({ x: 0, y: 0 });
  const [sheet, setSheet] = useState<Sheet | null>(null);
  const shellRef = useRef<HTMLDivElement>(null);
  /** 窗口顶上不属于面板的那一截（逻辑像素）：弹出时为抽屉往上长预留的，裁掉了不显示（Rust 的 reserve_above） */
  const [topSpace, setTopSpace] = useState(0);
  const topRef = useRef(0);
  topRef.current = topSpace;
  /** 这次展开是在窗口里往上长的（没挪窗口）：长了多高，收起时缩回去 */
  const grownInPlace = useRef(0);
  /** 面板（不含顶上预留）的大小 */
  const logicalWindowSize = useCallback(() => ({ w: window.innerWidth, h: window.innerHeight - topRef.current }), []);
  const animating = useRef(false);

  const adopt = useCallback((next: string) => {
    setText(next);
    setSeq((n) => n + 1);
    setEditingSource(false);
    setMode('translate');
    setQuestion('');
    setPendingAsk(null);
  }, []);
  /** 进入 / 退出输入翻译。进入时钉住面板、光标放进输入框 */
  const inputRef = useRef(false);
  const enterInput = useCallback((on: boolean) => {
    // 从输入翻译换成划词翻译：输入翻译时自动加的钉住拿掉
    if (inputRef.current && !on) setPinned(false);
    inputRef.current = on;
    setInputMode(on);
    if (on) {
      setEditingSource(true);
      setPinned(true);
      window.setTimeout(() => sourceRef.current?.focus(), 30);
    }
  }, []);
  // 设置页里的预览：点外面、失焦都不收起（用户正在主窗口里调设置），由设置页来收
  const previewRef = useRef(false);
  // 出场 / 收起（Windows）：面板是同一个窗口藏了又显示，CSS 的入场动画只在页面第一次挂上时播一遍，
  // 之后每次都是"啪"地一下出现。所以每次弹出手动播：面板从略小、透明放大淡入，毛玻璃背板（窗口底下
  // 原生的一层，页面带不动）同一时刻、同一条曲线一起走。收起时先淡出再藏窗口：藏起来时最后一帧是空的，
  // 下次显示不会先闪一下上次的内容。macOS 的出场交给系统
  const blurRef = useRef(blur);
  blurRef.current = blur;
  const shownRef = useRef(false);
  const leaving = useRef<{ anim: Animation; timer: number } | null>(null);
  const appear = useCallback(() => {
    if (isMac) return;
    const wasLeaving = leaving.current;
    if (wasLeaving) {
      window.clearTimeout(wasLeaving.timer);
      wasLeaving.anim.cancel();
      leaving.current = null;
    }
    // 已经开着（又划了一段字）：只换内容，不再弹一次
    if (shownRef.current && !wasLeaving) return;
    shownRef.current = true;
    if (blurRef.current) void system.backdropFade(true, APPEAR_MS).catch(() => undefined);
    shellRef.current?.animate([{ opacity: 0, scale: '0.96' }, { opacity: 1, scale: '1' }], { duration: APPEAR_MS, easing: 'cubic-bezier(0.22, 1, 0.36, 1)' });
  }, []);
  useEvent('translate-request', (p) => {
    previewRef.current = !!p.preview;
    grownInPlace.current = 0;
    setTopSpace(p.reserve ?? 0);
    setSheet(null);
    adopt(p.text);
    enterInput(!!p.input);
    appear();
  });
  // 面板第一次出现时页面可能还没加载完、错过了事件，加载好后主动取一次
  useEffect(() => {
    translate
      .popupText()
      .then((current) => current && adopt(current))
      .catch(() => undefined);
    translate
      .popupInput()
      .then((input) => input && enterInput(true))
      .catch(() => undefined);
    translate
      .popupReserve()
      .then(setTopSpace)
      .catch(() => undefined);
  }, [adopt, enterInput]);

  // 设置页的预览是原生"显示但不激活"亮出来的，Tauri 的 hide() 藏不掉它；交给后端收（顺便让设置页把开关拨回去）
  const hide = useCallback(() => {
    // 收起面板时正在读的也停下（窗口只是藏起来，按钮还在，不会自己停）
    stopSpeaking();
    if (previewRef.current) {
      shownRef.current = false;
      void translate.popupPreview(false).catch(() => undefined);
      return;
    }
    const shell = shellRef.current;
    if (isMac || !shell) {
      void getCurrentWindow().hide();
      return;
    }
    if (leaving.current) return;
    const anim = shell.animate([{ opacity: 1, scale: '1' }, { opacity: 0, scale: '0.98' }], { duration: LEAVE_MS, easing: 'cubic-bezier(0.4, 0, 1, 1)', fill: 'forwards' });
    if (blurRef.current) void system.backdropFade(false, LEAVE_MS).catch(() => undefined);
    const timer = window.setTimeout(() => {
      shownRef.current = false;
      void getCurrentWindow().hide();
    }, LEAVE_MS + 16);
    leaving.current = { anim, timer };
  }, []);
  // 鼠标在面板外面按下了（Rust 的全局鼠标监听报的）。平时靠失焦收起，但面板有时拿不到键盘焦点、
  // 就等不到失焦；这条不依赖焦点。和失焦一样：翻译模式才收，对话模式、钉住、正在拖动 / 改大小时不收
  useEvent('translate-outside', () => {
    if (previewRef.current || modeRef.current !== 'translate' || pinnedRef.current || userResizing.current || userMoving.current) return;
    hide();
  });

  /** 面板在被拖着挪：过 ms 没再动就算挪完了 */
  const settle = useCallback((ms: number) => {
    window.clearTimeout(moveTimer.current);
    moveTimer.current = window.setTimeout(() => {
      userMoving.current = false;
      // 拖完把焦点要回来，之后点外面才能照常收起。不用 setFocus()：它会激活整个应用，主窗口也跟着跑到最前面
      const w = getCurrentWindow();
      void (async () => {
        if (!(await w.isFocused()) && (await w.isVisible())) await system.takeFocus(w.label);
      })();
    }, ms);
  }, []);

  /**
   * 原文那一栏：按住拖是挪面板，点一下才是改原文。语言那一行能拖的空白只有窄窄一条，
   * 面板顶上这一栏看着就像标题栏，用户会去拖它（钉住之后尤其想挪）
   */
  const dragBySource = (e: React.PointerEvent<HTMLElement>) => {
    if (e.button !== 0) return;
    const { clientX: x0, clientY: y0 } = e;
    const stop = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', stop);
    };
    const move = (ev: PointerEvent) => {
      if (Math.hypot(ev.clientX - x0, ev.clientY - y0) < 4) return;
      stop();
      userMoving.current = true;
      settle(1500);
      startMove(topRef.current);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', stop);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && hide();
    window.addEventListener('keydown', onKey);
    const onDown = (e: PointerEvent) => {
      if (!(e.target as Element | null)?.closest?.('[data-tauri-drag-region]')) return;
      userMoving.current = true;
      settle(1500);
    };
    window.addEventListener('pointerdown', onDown, true);
    // macOS：带 data-tauri-drag-region 的空白处，Tauri 会用系统自己的拖动去挪窗口，那个会被顶上的预留区卡住
    // （面板拖到半屏高就上不去）。赶在它前面接过来，用我们自己的
    const onMouseDown = (e: MouseEvent) => {
      const target = e.target as Element | null;
      if (!isMac || e.button !== 0 || !target?.hasAttribute?.('data-tauri-drag-region')) return;
      e.stopImmediatePropagation();
      startMove(topRef.current);
    };
    window.addEventListener('mousedown', onMouseDown, true);
    const win = getCurrentWindow();
    let blurTimer: number | undefined;
    let blurAt = 0;
    const onFocus = (focused: boolean) => {
      window.clearTimeout(blurTimer);
      if (focused) {
        // 刚失焦又马上拿回焦点：窗口边上那圈系统改大小的边框被按住了（按下去时焦点会先丢再回来，
        // 页面收不到这次按下），当成用户在拖边改大小，改完的尺寸照样记下来
        if (performance.now() - blurAt < 300) beginResize();
        return;
      }
      blurAt = performance.now();
      // 翻译模式点外面就收起；AI 模式、钉住时不收。先等一下再确认真的没焦点了，免得按住边框改大小时被收起
      blurTimer = window.setTimeout(() => {
        void (async () => {
          if (previewRef.current || modeRef.current !== 'translate' || pinnedRef.current || userResizing.current || userMoving.current) return;
          if (await win.isFocused()) return;
          hide();
        })();
      }, 200);
    };
    const unlisteners: Promise<() => void>[] = [
      win.onFocusChanged(({ payload }) => onFocus(payload)),
      win.onMoved(() => userMoving.current && settle(400)),
      // 用户拖右下角改了大小：记进设置，下次按这个尺寸弹出
      win.onResized(() => {
        // 顶上有预留时，用户拖边改了大小，窗口区域和毛玻璃背板要跟着面板的新大小。
        // 只管用户拖的：弹出时 Rust 摆位、展开 / 收起也会改窗口大小，那些各自设好了区域和背板；
        // 这里要是也跟着设，用的是上一次弹出留下的预留高度和还没更新的窗口高度，命令又排在
        // Rust 后面执行，正确的区域就被盖掉了 —— 面板下面被裁掉一截，或者顶上多出一块空的毛玻璃
        const top = topRef.current;
        if (top > 0 && userResizing.current && !animating.current) {
          const rect = { x: 0, y: top, width: window.innerWidth, height: window.innerHeight - top };
          void system.setRegion(rect);
          if (padRef.current === 0) void system.backdrop(rect);
        }
        if (!userResizing.current || animating.current) return;
        resized.current = true;
        window.clearTimeout(resizeTimer.current);
        resizeTimer.current = window.setTimeout(() => {
          userResizing.current = false;
          resized.current = false;
          const { w, h } = logicalWindowSize();
          const p = padRef.current;
          const box = transRef.current?.getBoundingClientRect();
          void useSettingsStore
            .getState()
            .update((d) => {
              if (modeRef.current === 'translate') {
                d.translate.popup.width = Math.round(w);
                d.translate.popup.height = Math.round(h);
              } else if (layoutRef.current === 'side') {
                d.ai.panel.width = Math.round(w - p * 2 - (box?.width ?? 0));
                d.ai.panel.height = Math.round(h);
              } else {
                d.translate.popup.width = Math.round(w);
                d.translate.popup.drawerHeight = Math.round(h - p * 2 - (box?.height ?? 0));
              }
            })
            .catch(() => undefined);
        }, 500);
      }),
    ];
    return () => {
      window.clearTimeout(blurTimer);
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('pointerdown', onDown, true);
      window.removeEventListener('mousedown', onMouseDown, true);
      for (const u of unlisteners) void u.then((fn) => fn());
    };
  }, [hide, logicalWindowSize, beginResize, settle]);

  const openAi = async (ask?: string) => {
    if (animating.current) return;
    animating.current = true;
    try {
      const next: Layout = popup?.aiLayout === 'side' ? 'side' : 'drawer';
      const panel = settings?.ai.panel;
      const base = logicalWindowSize();
      baseSize.current = base;
      // 位置、屏幕可用区域：Windows 上直接读浏览器的同步值（逻辑像素），少几次来回，点下去立刻开始动；
      // macOS 上那些值不准（窗口被程序挪过之后 screenX 不更新，面板会跳到屏幕左边），问后端
      const { pos, mx, my, mw, mh } = await whereAmI();
      let w: number;
      let h: number;
      if (next === 'side') {
        const trans = Math.max(MIN_TRANSLATE, base.w - pad * 2);
        setTransW(trans);
        w = Math.min(mw - 16, trans + pad * 2 + Math.max(MIN_AI, panel?.width ?? 520));
        h = Math.min(mh - 16, Math.max(base.h, panel?.height ?? 620));
      } else {
        // 对话区从输入框那里往上长，盖住翻译区下面一半：翻译区露出上面一半，窗口往上多长出一个对话区减半个翻译区
        const trans = Math.max(MIN_TRANSLATE_H, Math.round((base.h - pad * 2) / 2));
        // 宽度不变：宽度一变就没法在窗口里原地往上长，只能挪窗口，开头会跳一下
        w = base.w;
        h = Math.min(mh - 16, pad * 2 + trans + (popup?.drawerHeight ?? 380));
        setTransH(Math.min(trans, h - pad * 2 - MIN_DRAWER));
      }
      // 侧边：原地往右长。抽屉：底边（输入框）不动，往上长。放不下才整个往回挪
      const grow = h - base.h;
      if (next === 'drawer' && w === base.w && grow > 0 && topRef.current >= grow) {
        const top = topRef.current - grow;
        const full = { fw: w - pad * 2, fh: h - pad * 2 };
        grownInPlace.current = grow;
        // 1. 外框先按原来的大小、原来的位置（往下偏 grow）定住，顶上预留缩小同样多，屏幕上看不动；
        //    窗口区域放开到外框要长到的地方
        flushSync(() => {
          setLayout(next);
          if (ask) setPendingAsk({ id: Date.now(), text: ask });
          setQuestion('');
          setTopSpace(top);
          setSheet({ w: base.w - pad * 2, h: base.h - pad * 2, ...full, x: 0, y: 0, open: false, animate: false, bottom: true });
          setMode('ai');
        });
        userResizing.current = false;
        void system.setRegion({ x: 0, y: top, width: w, height: window.innerHeight - top });
        await frame();
        await frame();
        // 2. 外框往上长到新高度，对话区从输入框那里升上来，毛玻璃背板用同一条曲线跟着长
        flushSync(() => setSheet({ w: full.fw, h: full.fh, ...full, x: 0, y: 0, open: true, animate: true, bottom: true }));
        if (blur) {
          await lagBackdrop(shellRef.current);
          void system.backdrop({ x: 0, y: top, width: full.fw, height: full.fh }, ANIM_MS);
        }
        await wait(ANIM_MS + 40);
        setSheet(null);
        return;
      }
      // 挪窗口。顶上有预留（窗口区域裁掉的那一截）的话照样留着：下面的坐标都是面板的，窗口比面板高出 top
      const top = topRef.current;
      const panelY = pos.y + top;
      const x = Math.max(mx, Math.min(pos.x, mx + mw - w));
      const wantY = next === 'drawer' ? panelY + base.h - h : panelY;
      const y = Math.max(my, Math.min(wantY, my + mh - h));
      shift.current = { x: pos.x - x, y: panelY - y };
      const full = { fw: w - pad * 2, fh: h - pad * 2 };
      const start = { w: base.w - pad * 2, h: base.h - pad * 2, ...full };

      // 1. 外框按现在的尺寸定住（要往回挪的话同时往反方向偏，屏幕上看不动），对话区先藏着；
      //    同步提交到 DOM，然后窗口一步到位
      flushSync(() => {
        setLayout(next);
        if (ask) setPendingAsk({ id: Date.now(), text: ask });
        setQuestion('');
        setSheet({ ...start, x: shift.current.x, y: shift.current.y, open: false, animate: false });
        setMode('ai');
      });
      userResizing.current = false;
      // 窗口区域先放开到展开后面板的范围（比现在的窗口大没关系），挪完窗口不会有一帧被裁掉
      if (top > 0) await system.setRegion({ x: 0, y: top, width: w, height: h });
      // 毛玻璃：背板先只铺在外框现在的位置，之后跟着外框一起长
      await moveWindow(x, y - top, w, h + top, blur ? { x: shift.current.x, y: top + shift.current.y, width: start.w, height: start.h } : undefined);
      await frame();
      await frame();
      // 2. 外框长到新尺寸、回到原点，对话区滑进来
      flushSync(() => setSheet({ w: full.fw, h: full.fh, ...full, x: 0, y: 0, open: true, animate: true }));
      if (blur) {
        await lagBackdrop(shellRef.current);
        void system.backdrop({ x: 0, y: top, width: w, height: h }, ANIM_MS);
      }
      await wait(ANIM_MS + 40);
      setSheet(null);
      // 没有预留时背板恢复铺满窗口；有预留时就停在面板上（窗口顶上那截不铺）
      if (blur && top === 0) void system.backdrop(null);
    } finally {
      animating.current = false;
    }
  };

  const closeAi = async () => {
    if (animating.current) return;
    animating.current = true;
    try {
      const { pos } = await whereAmI();
      const cur = logicalWindowSize();
      const target = baseSize.current;
      const grown = grownInPlace.current;
      if (grown > 0) {
        // 在窗口里长出来的：缩回原来的大小和位置，顶上预留还原，窗口不动
        const full = { fw: cur.w - pad * 2, fh: cur.h - pad * 2 };
        const top = topRef.current + grown;
        flushSync(() => setSheet({ w: full.fw, h: full.fh, ...full, x: 0, y: 0, open: true, animate: false, bottom: true }));
        await frame();
        setSheet({ w: target.w - pad * 2, h: target.h - pad * 2, ...full, x: 0, y: 0, open: false, animate: true, bottom: true });
        if (blur) void system.backdrop({ x: 0, y: top, width: target.w, height: target.h }, ANIM_MS);
        await wait(ANIM_MS + 20);
        userResizing.current = false;
        grownInPlace.current = 0;
        flushSync(() => {
          setTopSpace(top);
          setSheet(null);
          setMode('translate');
          setPendingAsk(null);
        });
        void system.setRegion({ x: 0, y: top, width: target.w, height: target.h });
        return;
      }
      const { x: sx, y: sy } = shift.current;
      const top = topRef.current;
      const full = { fw: cur.w - pad * 2, fh: cur.h - pad * 2 };
      // 1. 外框按现在的尺寸定住，然后缩回原来的大小、偏到窗口挪回去之后的位置，对话区滑出去
      flushSync(() => setSheet({ w: full.fw, h: full.fh, ...full, x: 0, y: 0, open: true, animate: false }));
      await frame();
      setSheet({ w: target.w - pad * 2, h: target.h - pad * 2, ...full, x: sx, y: sy, open: false, animate: true });
      if (blur) void system.backdrop({ x: sx, y: top + sy, width: target.w, height: target.h }, ANIM_MS);
      await wait(ANIM_MS + 20);
      // 2. 窗口一步还原，外框同时回到原点
      userResizing.current = false;
      flushSync(() => {
        setSheet({ w: target.w - pad * 2, h: target.h - pad * 2, ...full, x: 0, y: 0, open: false, animate: false });
      });
      await moveWindow(pos.x + sx, pos.y + sy, target.w, target.h + top, blur ? { x: 0, y: top, width: target.w, height: target.h } : undefined);
      if (top > 0) void system.setRegion({ x: 0, y: top, width: target.w, height: target.h });
      else if (blur) void system.backdrop(null);
      await frame();
      shift.current = { x: 0, y: 0 };
      setMode('translate');
      setPendingAsk(null);
      setSheet(null);
    } finally {
      animating.current = false;
    }
  };

  /** 侧边：拖翻译卡片和对话区之间的缝，窗口大小不变，两边此消彼长 */
  const dragSeam = (e: React.PointerEvent<HTMLSpanElement>) => {
    let last = transW;
    dragHandle(
      e,
      (ev) => {
        last = Math.round(Math.min(window.innerWidth - pad * 2 - MIN_AI, Math.max(MIN_TRANSLATE, ev.clientX - pad)));
        setTransW(last);
      },
      () => {
        baseSize.current = { ...baseSize.current, w: last + pad * 2 };
        void useSettingsStore
          .getState()
          .update((d) => {
            d.translate.popup.width = last + pad * 2;
            d.ai.panel.width = Math.round(window.innerWidth - pad * 2 - last);
          })
          .catch(() => undefined);
      },
    );
  };

  /** 抽屉：拖顶上的小横条，翻译区和对话区上下此消彼长 */
  const dragDrawer = (e: React.PointerEvent<HTMLSpanElement>) => {
    const offset = e.clientY - (pad + topSpace + transH);
    let last = transH;
    dragHandle(
      e,
      (ev) => {
        last = Math.round(Math.min(window.innerHeight - topSpace - pad * 2 - MIN_DRAWER, Math.max(MIN_TRANSLATE_H, ev.clientY - offset - pad - topSpace)));
        setTransH(last);
      },
      () =>
        void useSettingsStore
          .getState()
          .update((d) => void (d.translate.popup.drawerHeight = Math.round(window.innerHeight - topSpace - pad * 2 - last)))
          .catch(() => undefined),
    );
  };

  const popOut = () => {
    // 连同已经聊过的内容一起挪过去，接着聊
    void ai.open({ text, images: [], source: 'selection', history: turnsRef.current });
    hide();
  };

  const style = {
    '--pop-radius': `${popup?.radius ?? 24}px`,
    '--pop-alpha': String(popup?.opacity ?? 1),
    '--pop-pad': `${pad}px`,
    '--pop-top': `${topSpace}px`,
  } as React.CSSProperties;

  const shellClass = ['pop-shell', mode === 'ai' ? `is-${layout}` : 'is-plain', blur ? 'is-blur' : ''].join(' ');
  const transition = (props: string[]) => props.map((p) => `${p} ${ANIM_MS}ms ${EASE}`).join(', ');
  const shellStyle: React.CSSProperties | undefined = sheet
    ? {
        flex: 'none',
        width: sheet.w,
        height: sheet.h,
        transform: `translate(${sheet.x}px, ${sheet.y}px)`,
        // 贴底：外框靠在面板区域的底边上，只有高度在变，底边一个像素都不动
        alignSelf: sheet.bottom ? 'flex-end' : undefined,
        transition: sheet.animate ? transition(sheet.bottom ? ['height'] : ['width', 'height', 'transform']) : 'none',
      }
    : undefined;
  // 过渡期间翻译区保持展开前的高度（带着输入框），抽屉滑上来盖住它的下沿。
  // 被盖住的那一截同步裁掉：面板调了不透明度时抽屉是半透明的，不裁的话下面的译文和输入框会透上来，
  // 动画一结束（翻译区变矮）又突然消失
  const paneFull = baseSize.current.h - pad * 2;
  const cut = sheet?.open ? Math.max(0, paneFull - transH) : 0;
  const paneStyle: React.CSSProperties | undefined =
    mode !== 'ai'
      ? undefined
      : layout === 'side'
        ? { width: transW, maxWidth: sheet ? 'none' : undefined }
        : sheet
          ? ({
              height: paneFull,
              clipPath: `inset(0 0 ${cut}px 0)`,
              // 译文最下面那段淡出贴着抽屉的上沿一起动（POPUP_CSS 的 .is-sheet），不是等抽屉升完才冒出来
              '--pop-cut': `${cut}px`,
              transition: `${sheet.animate ? `${transition(['clip-path', '--pop-cut'])}, ` : ''}background-color ${ANIM_MS}ms ${EASE}`,
            } as React.CSSProperties)
          : { height: transH };
  /**
   * 过渡期间对话区贴着外框底边（侧边是右边），高度（宽度）从 0 长到最终值，和外框用同一条缓动曲线：
   * 外框往下长多少，抽屉就往上长多少，顶边从翻译区底部平滑升到最终位置，中间不会露出空隙。
   * 里面的内容按最终大小固定在抽屉顶部（左边），只是被逐渐露出来，不跟着重新排版
   */
  const drawerStyle: React.CSSProperties | undefined = sheet
    ? {
        position: 'absolute',
        left: 0,
        right: 0,
        bottom: 0,
        height: sheet.open ? sheet.fh - transH : 0,
        transition: sheet.animate ? transition(['height']) : 'none',
      }
    : undefined;
  const drawerInner: React.CSSProperties | undefined = sheet
    ? { position: 'absolute', left: 0, right: 0, top: 0, height: sheet.fh - transH, flex: 'none' }
    : undefined;
  const sideStyle: React.CSSProperties | undefined = sheet
    ? {
        position: 'absolute',
        top: 0,
        bottom: 0,
        right: 0,
        width: sheet.open ? sheet.fw - transW : 0,
        transition: sheet.animate ? transition(['width']) : 'none',
      }
    : undefined;
  const sideInner: React.CSSProperties | undefined = sheet
    ? { position: 'absolute', top: 0, bottom: 0, left: 0, width: sheet.fw - transW, flex: 'none' }
    : undefined;

  return (
    <div className="pop-root" style={style}>
      <div ref={shellRef} className={shellClass} style={shellStyle}>
        <div
          ref={transRef}
          className={`pop-pane pop-pane--translate${(mode === 'translate' || (sheet && layout === 'drawer')) && text ? ' has-ask' : ''}${sheet && layout === 'drawer' ? ' is-sheet' : ''}`}
          key={seq}
          style={paneStyle}
        >
          {(popup?.showSource || inputMode) && (
            <div className="pop__source-wrap">
              {editingSource || inputMode ? (
                <textarea
                  ref={sourceRef}
                  className={`pop__source pop__source--edit${inputMode ? ' pop__source--input' : ''}`}
                  autoFocus
                  value={text}
                  placeholder={inputMode ? t('translate.inputPlaceholder') : undefined}
                  onChange={(e) => setText(e.target.value)}
                  onBlur={() => !inputMode && setEditingSource(false)}
                  onKeyDown={(e) => {
                    // 输入翻译里 Esc 照常关面板；编辑划词原文时 Esc 只退出编辑
                    if (e.key === 'Escape' && !inputMode) {
                      e.stopPropagation();
                      setEditingSource(false);
                    }
                  }}
                />
              ) : (
                <div className="pop__source" title={t('translate.editSource')} onPointerDown={dragBySource} onClick={() => setEditingSource(true)}>
                  {text}
                </div>
              )}
              {text.trim() && <SpeakButton className="pop__speak" text={text} label={t('translate.speakSource')} />}
            </div>
          )}
          <div className="pop__body">
            {inputMode && !text.trim() && <div className="pop__input-hint">{t('translate.inputHint')}</div>}
            {text && (
              <TranslateBox
                text={text}
                compact
                fontSize={popup?.fontSize}
                actions={
                  <span className="pop-actions">
                    <IconButton
                      icon={Pin}
                      size="sm"
                      active={pinned}
                      label={pinned ? t('translate.unpin') : t('translate.pin')}
                      onClick={() => setPinned((v) => !v)}
                    />
                    <IconButton
                      icon={Sparkles}
                      size="sm"
                      active={mode === 'ai'}
                      label={mode === 'ai' ? t('ai.hide') : t('ai.ask')}
                      onClick={() => void (mode === 'ai' ? closeAi() : openAi())}
                    />
                    <IconButton icon={X} size="sm" label={t('common.close')} onClick={hide} />
                  </span>
                }
              />
            )}
          </div>
          {(mode === 'translate' || (sheet && layout === 'drawer')) && text && (
            <div className="pop-ask-wrap">
              <div className="pop-ask">
                <Sparkles size={14} strokeWidth={1.75} />
                <input
                  value={question}
                  placeholder={t('ai.askPlaceholder')}
                  onChange={(e) => setQuestion(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' && !e.nativeEvent.isComposing && question.trim()) {
                      e.preventDefault();
                      void openAi(question.trim());
                    }
                  }}
                />
                <button type="button" aria-label={t('ai.send')} disabled={!question.trim()} onClick={() => void openAi(question.trim())}>
                  <ArrowUp size={14} strokeWidth={2.25} />
                </button>
              </div>
            </div>
          )}
        </div>
        {mode === 'ai' && layout === 'side' && (
          <>
            {!sheet && <span className="pop-seam" style={{ left: transW - 5 }} title={t('translate.resize')} onPointerDown={dragSeam} />}
            <div className="pop-pane pop-pane--ai" style={sideStyle}>
              <div className="pop-inner" style={sideInner}>
                <AiChat
                  compact
                  context={{ text, images: [] }}
                  resetKey={seq}
                  onTurnsChange={keepTurns}
                  ask={pendingAsk}
                  toolbar={<IconButton icon={ExternalLink} size="sm" label={t('ai.popOut')} onClick={popOut} />}
                />
              </div>
            </div>
          </>
        )}
        {mode === 'ai' && layout === 'drawer' && (
          <div className="pop-drawer" style={drawerStyle}>
            <span className="pop-drawer__grab" title={t('translate.resize')} onPointerDown={dragDrawer}>
              <i />
            </span>
            <div className="pop-inner" style={drawerInner}>
              <AiChat
                compact
                context={{ text, images: [] }}
                resetKey={seq}
                onTurnsChange={keepTurns}
                ask={pendingAsk}
                toolbar={
                  <>
                    <IconButton icon={ExternalLink} size="sm" label={t('ai.popOut')} onClick={popOut} />
                    <IconButton icon={ChevronDown} size="sm" label={t('ai.hide')} onClick={() => void closeAi()} />
                  </>
                }
              />
            </div>
          </div>
        )}
        <span
          className="pop__grip"
          title={t('translate.resize')}
          onPointerDown={(e) => {
            e.preventDefault();
            beginResize();
            startResize('SouthEast', topRef.current);
          }}
        />
        {!sheet &&
          RESIZE_EDGES.map(([dir, cls]) => (
            <span
              key={cls}
              className={`pop-edge pop-edge--${cls}`}
              onPointerDown={(e) => {
                e.preventDefault();
                beginResize();
                startResize(dir, topRef.current);
              }}
            />
          ))}
      </div>
      <style>{POPUP_CSS}</style>
    </div>
  );
}

const POPUP_CSS = `
html[data-view='translate'], html[data-view='translate'] body { background: transparent !important; }
.pop-root { position: fixed; inset: 0; padding: var(--pop-pad); padding-top: calc(var(--pop-pad) + var(--pop-top, 0px)); display: flex; }

/* 两档底色：浮在上面的一层（hi）和压在下面的一层（lo）。透明窗口里 backdrop-filter 模糊不了
   桌面，不开毛玻璃时默认不透明；不透明度可在设置里调 */
/* overflow: clip 而不是 hidden：hidden 的元素还能被滚动（聚焦藏在外面的输入框时浏览器会自动滚），一滚整块内容就跳 */
.pop-shell { position: relative; flex: 1; min-width: 0; display: flex; border-radius: var(--pop-radius); overflow: clip;
             box-shadow: 0 4px 12px rgba(0,0,0,0.22), 0 0 0 0.5px rgba(0,0,0,0.28); animation: cn-pop-in 160ms var(--cn-ease-out);
             --pop-hi: rgba(252,252,254,var(--pop-alpha)); --pop-lo: rgba(236,236,241,var(--pop-alpha));
             --r: min(var(--pop-radius), 16px); }
[data-theme='dark'] .pop-shell { --pop-hi: rgba(46,46,48,var(--pop-alpha)); --pop-lo: rgba(30,30,32,var(--pop-alpha)); }
/* 毛玻璃：系统亚克力在窗口背后，圆角和阴影都是系统的，这里只铺一层半透明的色调 */
/* 毛玻璃：系统模糊背板挂在窗口最底下、按同样的圆角裁好（Rust 的 set_glass），
   这里不画阴影，只在边上描一圈细线，让面板边缘清楚一点 */
.pop-shell.is-blur { box-shadow: inset 0 0 0 0.5px rgba(255,255,255,0.16); animation: none; }
[data-theme='light'] .pop-shell.is-blur { box-shadow: inset 0 0 0 0.5px rgba(0,0,0,0.14); }
/* 浅色 + 毛玻璃（Windows）：背板只是把桌面模糊了，本身没有颜色。面板背后是深色内容时，淡淡一层白盖不住，
   整块发灰发暗，上面浅色主题的深色字就看不清了。所以浅色下色调至少五成半实，不透明度滑块在这之上再调。
   macOS 的系统材质自己就是浅色的，不用垫 */
html:not([data-platform='mac'])[data-theme='light'] .pop-shell.is-blur {
  --pop-hi: rgba(252,252,254,calc(0.55 + var(--pop-alpha) * 0.45)); --pop-lo: rgba(236,236,241,calc(0.55 + var(--pop-alpha) * 0.45)); }
/* 本程序截图的那一刻（Windows）：系统背板藏起来、色调换成不透明，抓到的面板全是页面画的（圆角带抗锯齿），见 useGlassSnapshot */
html[data-snapshot] .pop-shell.is-blur { --pop-alpha: 1 !important; }
html[data-snapshot] .pop-shell.is-blur, html[data-snapshot] .pop-shell.is-blur * { transition: none !important; }
.pop-shell.is-drawer { flex-direction: column; }

.pop-pane { position: relative; min-width: 0; min-height: 0; display: flex; flex-direction: column; }
.pop-pane--translate { flex: 1; z-index: 2; background-color: var(--pop-hi); transition: background-color ${ANIM_MS}ms ${EASE}; }
.pop__source { max-height: 66px; overflow: hidden; padding: 12px 14px 8px; font: var(--cn-text-callout); color: var(--cn-label-secondary);
               display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical; box-shadow: inset 0 -0.5px 0 var(--cn-separator); flex: none; }
.pop__body { flex: 1; min-height: 0; }
/* 语言那一行右边的置顶、问 AI、关闭挨紧一点，窄面板里给语言下拉框多留点地方 */
.pop-actions { flex: none; display: flex; align-items: center; gap: 0; margin-right: -4px; }

/* 原文：点一下就变成输入框，改完自动重翻 */
.pop__source { cursor: text; }
.pop__source:hover { background: var(--cn-bg-hover); }
.pop__source--edit { display: block; width: 100%; max-height: 120px; min-height: 44px; field-sizing: content; resize: none; border: none; outline: none;
                     background: var(--cn-fill-quaternary); color: var(--cn-label); -webkit-line-clamp: unset; overflow: auto;
                     box-shadow: inset 0 -1px 0 var(--cn-accent); }

/* 底部"问 AI"输入框：一颗实心的胶囊浮在翻译区底部，背后不垫整条底色。
   - 胶囊自己是实的（九成多不透明），里面的字不会和背后的译文叠在一起；
   - 译文滚到胶囊跟前就渐渐淡出（遮罩加在滚动区上），不会有半行字从胶囊上下露出来。
   试过又放弃的：整条渐变底色（上半截是透的，字照样叠）；整条实底色（面板是毛玻璃，底下突然一条
   不透的色带，像另外贴上去的） */
.pop-ask-wrap { position: absolute; left: 0; right: 0; bottom: 0; z-index: 3; padding: 0 12px 10px; pointer-events: none; }
.has-ask .pop__body { -webkit-mask-image: linear-gradient(to bottom, #000 calc(100% - 86px), transparent calc(100% - 46px));
                      mask-image: linear-gradient(to bottom, #000 calc(100% - 86px), transparent calc(100% - 46px)); }
/* 对话抽屉开着：译文一路排到抽屉的上沿，在最后一小段渐渐淡出，像是滑到抽屉底下去了。
   以前这里是卡片被一刀切断、下面再留一条空的底色，看着像抽屉上方压着一条遮罩 */
.pop-shell.is-drawer .pop-pane--translate:not(.has-ask) .pop__body {
  -webkit-mask-image: linear-gradient(to bottom, #000 calc(100% - 46px), transparent 100%);
          mask-image: linear-gradient(to bottom, #000 calc(100% - 46px), transparent 100%); }
/* 抽屉升起 / 收回的过程中：同样的淡出，只是量的是还露在抽屉上面的那截（被盖住的 --pop-cut 不算），
   --pop-cut 和抽屉用同一条曲线过渡，淡出一直贴着抽屉的上沿。以前过渡时用的是输入框那套遮罩，
   抽屉升完才换成这个，淡出要晚零点几秒才出来 */
@property --pop-cut { syntax: '<length>'; inherits: true; initial-value: 0px; }
.pop-shell.is-drawer .pop-pane--translate.is-sheet .pop__body {
  -webkit-mask-image: linear-gradient(to bottom, #000 calc(100% - 46px), transparent 100%);
          mask-image: linear-gradient(to bottom, #000 calc(100% - 46px), transparent 100%);
  -webkit-mask-size: 100% calc(100% - var(--pop-cut)); mask-size: 100% calc(100% - var(--pop-cut));
  -webkit-mask-repeat: no-repeat; mask-repeat: no-repeat; }
.pop-shell.is-drawer .pop-pane--translate:not(.has-ask) .tr-box { padding-bottom: 0; }
.pop-shell.is-drawer .pop-pane--translate:not(.has-ask) .tr-box__cards { padding-bottom: 22px; }
.has-ask .tr-box { padding-bottom: 74px; }
.has-ask .tr-box:has(.tr-box__cards) { padding-bottom: 0; }
.has-ask .tr-box__cards { padding-bottom: 74px; }
/* 多个翻译源那一列的滚动条放进右边的留白里，而且位置一直留着：以前一出滚动条卡片就被挤窄 10px，
   左右留白不一样宽。用 overflow-y: scroll 占住这 10px（轨道是透明的，内容不够长时看不见滚动条）：
   scrollbar-gutter 在 WebKit（macOS）里对自定义样式的滚动条不起作用，没出滚动条时卡片会一直顶到右边 */
.pop-pane--translate .tr-box__cards { margin-right: -10px; overflow-x: hidden; overflow-y: scroll; scrollbar-gutter: stable; }
/* 翻译源卡片要从面板上"浮"起来：通用的那层淡灰底色在毛玻璃面板上几乎看不出来，卡片和面板糊成一片。
   浅色用更白的底，深色用更亮的底，再描一圈细边、压一点点阴影 */
.pop-pane--translate .tr-card { background: rgba(255,255,255,0.66); box-shadow: 0 0 0 0.5px rgba(0,0,0,0.07), 0 1px 2px rgba(0,0,0,0.06); }
[data-theme='dark'] .pop-pane--translate .tr-card { background: rgba(255,255,255,0.085); box-shadow: inset 0 0 0 0.5px rgba(255,255,255,0.09), 0 1px 2px rgba(0,0,0,0.22); }
/* 没开毛玻璃时面板背后的画面是清清楚楚透上来的（不透明度调低的话）：卡片要是也半透明，背后的字会和译文叠在一起。
   所以这时卡片用实底，只有面板本身的底色跟着不透明度走 */
.pop-shell:not(.is-blur) .pop-pane--translate .tr-card { background: #fff; }
[data-theme='dark'] .pop-shell:not(.is-blur) .pop-pane--translate .tr-card { background: #3a3a3d; }
/* 卡片：鼠标移上去浮起来一点 */
.pop-pane--translate .tr-card:hover { box-shadow: 0 0 0 0.5px rgba(0,0,0,0.09), 0 3px 10px rgba(0,0,0,0.1); }
[data-theme='dark'] .pop-pane--translate .tr-card:hover { box-shadow: inset 0 0 0 0.5px rgba(255,255,255,0.14), 0 3px 10px rgba(0,0,0,0.3); }
/* 底部"问 AI"：跟着面板一起浮上来；聚焦时抬起一点 */
/* ── 面板里的排版 ── */
/* 原文：一小块引用（左边一道强调色），不再是顶上通栏的一条 */
/* 上下的留白用透明边框而不是内边距：超出两行的字会被裁在内边距的外沿，用内边距的话第三行的上半截会露在留白里 */
.pop__source { position: relative; margin: 10px 12px 0; padding: 0 10px 0 15px; border: solid transparent; border-width: 7px 0; max-height: none; border-radius: 10px; box-shadow: none;
               background: var(--cn-fill-quaternary); font: var(--cn-text-footnote); line-height: 1.45; -webkit-line-clamp: 2;
               transition: background-color 140ms var(--cn-ease-out); }
.pop__source::before { content: ''; position: absolute; left: 6px; top: 1px; bottom: 1px; width: 2.5px; border-radius: 2px; background: var(--cn-accent); opacity: 0.75; }
.pop__source:hover { background: var(--cn-fill-tertiary); }
.pop__source--edit { width: calc(100% - 24px); padding: 7px 10px; border-width: 0; box-shadow: inset 0 0 0 1.5px var(--cn-accent); background: var(--cn-bg-elevated); }
/* 原文外面一层：右上角放朗读按钮，原文给它让出位置 */
.pop__source-wrap { position: relative; flex: none; }
.pop__source-wrap .pop__source { padding-right: 32px; }
.pop__speak { position: absolute; top: 12px; right: 16px; }
/* 输入翻译：输入框大一点、字正常大小 */
.pop__source--input { min-height: 64px; max-height: 160px; font: var(--cn-text-body); }
.pop__input-hint { padding: 18px 16px; color: var(--cn-label-tertiary); font: var(--cn-text-callout); }
/* 语言：两颗胶囊，中间的对调按钮点一下转半圈 */
.pop-pane--translate .tr-box__langs { gap: 2px; }
.pop-pane--translate .tr-box__langs .cn-select-trigger { height: 28px; padding: 0 7px 0 12px; border-radius: 999px; background: var(--cn-fill-quaternary);
               box-shadow: none; font: var(--cn-text-footnote); font-weight: 500; transition: background-color 140ms var(--cn-ease-out); }
.pop-pane--translate .tr-box__langs .cn-select-trigger:hover,
.pop-pane--translate .tr-box__langs .cn-select-trigger[data-state='open'] { background: var(--cn-fill-tertiary); box-shadow: none; }
.pop-pane--translate .tr-box__langs > button svg { transition: transform 260ms var(--cn-ease-out); }
.pop-pane--translate .tr-box__langs > button:active svg { transform: rotate(180deg); }
/* 卡片：留白松一点，来源的名字退到次要；默认源标成一颗淡色的小标签，它的译文稍微重一点 */
.pop-pane--translate .tr-box__cards { gap: 8px; }
.pop-pane--translate .tr-card { padding: 8px 8px 11px 12px; border-radius: 13px; }
.pop-pane--translate .tr-card[data-collapsed] { padding-bottom: 8px; }
.pop-pane--translate .tr-card__name { font-size: 11.5px; color: var(--cn-label-tertiary); }
.pop-pane--translate .tr-card__body { margin-top: 3px; line-height: 1.55; }
.pop-pane--translate .tr-card[data-primary] .tr-card__body { font-weight: 500; }
.pop-pane--translate .tr-card__badge { background: color-mix(in srgb, var(--cn-accent) 15%, transparent); color: var(--cn-accent); }
/* 复制、重试平时淡着，移到卡片上才清楚（刚复制成功的那颗对勾一直亮着） */
.pop-pane--translate .tr-card__tools { opacity: 0.38; transition: opacity 140ms var(--cn-ease-out); }
.pop-pane--translate .tr-card:hover .tr-card__tools,
.pop-pane--translate .tr-card__tools:focus-within,
.pop-pane--translate .tr-card__tools:has(.tr-card__copied) { opacity: 1; }
.pop-ask { animation: tr-card-in 320ms var(--cn-ease-out) 120ms both; transition: box-shadow 160ms var(--cn-ease-out), transform 160ms var(--cn-ease-out); }
.pop-ask:focus-within { transform: translateY(-1px); }
/* 开了毛玻璃的面板也要有出场：系统的背板没法跟着缩放，只让里面的内容淡入 */
.pop-shell.is-blur > .pop-pane--translate { animation: pop-fade-in 180ms var(--cn-ease-out); }
@keyframes pop-fade-in { from { opacity: 0; } }
.pop-ask { flex: none; display: flex; align-items: center; gap: 8px; height: 32px; padding: 0 4px 0 11px; pointer-events: auto;
           border-radius: 16px; background: rgba(255,255,255,0.94); color: var(--cn-accent);
           -webkit-backdrop-filter: blur(12px); backdrop-filter: blur(12px);
           box-shadow: 0 0 0 0.5px rgba(0,0,0,0.12), 0 3px 10px rgba(0,0,0,0.14); }
[data-theme='dark'] .pop-ask { background: rgba(62,62,66,0.94); box-shadow: inset 0 0 0 0.5px rgba(255,255,255,0.14), 0 3px 10px rgba(0,0,0,0.32); }
.pop-ask:focus-within { box-shadow: inset 0 0 0 1px var(--cn-accent), 0 3px 10px rgba(0,0,0,0.18); }
.pop-ask input { flex: 1; min-width: 0; height: 100%; border: none; outline: none; background: transparent; color: var(--cn-label); font: var(--cn-text-body); }
.pop-ask input::placeholder { color: var(--cn-label-tertiary); }
.pop-ask button { flex: none; display: grid; place-items: center; width: 24px; height: 24px; padding: 0; border: none; border-radius: 50%;
                  background: var(--cn-accent); color: #fff; cursor: pointer; transition: opacity 120ms; }
.pop-ask button:disabled { opacity: 0.3; cursor: default; }

/* ── 侧边：翻译卡片在上层，右边圆角外露出下层的颜色；阴影只投在右边 ── */
.is-side .pop-pane--translate { flex: none; min-width: ${MIN_TRANSLATE}px; max-width: calc(100% - ${MIN_AI}px);
  background:
    radial-gradient(circle at 0 100%, var(--pop-hi) calc(var(--r) - 0.6px), var(--pop-lo) var(--r)) right top / var(--r) var(--r) no-repeat,
    radial-gradient(circle at 0 0, var(--pop-hi) calc(var(--r) - 0.6px), var(--pop-lo) var(--r)) right bottom / var(--r) var(--r) no-repeat,
    linear-gradient(var(--pop-hi), var(--pop-hi)) left top / calc(100% - var(--r)) 100% no-repeat,
    linear-gradient(var(--pop-hi), var(--pop-hi)) right center / var(--r) calc(100% - 2 * var(--r)) no-repeat; }
.is-side .pop-pane--translate::after { content: ''; position: absolute; inset: 0; border-radius: 0 var(--r) var(--r) 0; pointer-events: none;
  box-shadow: 0.5px 0 0 var(--cn-separator), 8px 0 18px -10px rgba(0,0,0,0.4); clip-path: inset(0 -40px 0 calc(100% - var(--r) - 1px)); }
/* 过渡时里面的内容伸出对话区之外的部分由外框裁掉（对话区自己不裁，顶边的阴影要露出来） */
.pop-pane--ai { flex: 1; z-index: 1; background: var(--pop-lo); }
.pop-inner { flex: 1; min-width: 0; min-height: 0; display: flex; flex-direction: column; }
.pop-pane--ai .ai__bar { padding: var(--cn-space-3) var(--cn-space-3) 6px; }

/* ── 抽屉：翻译区退到下层，对话卡片从底部滑上来盖住它的下沿 ── */
.is-drawer .pop-pane--translate { flex: none; background-color: var(--pop-lo); }
.pop-drawer { position: relative; z-index: 3; flex: 1; min-height: 0; display: flex; flex-direction: column;
  background:
    radial-gradient(circle at 100% 100%, var(--pop-hi) calc(var(--r) - 0.6px), var(--pop-lo) var(--r)) left top / var(--r) var(--r) no-repeat,
    radial-gradient(circle at 0 100%, var(--pop-hi) calc(var(--r) - 0.6px), var(--pop-lo) var(--r)) right top / var(--r) var(--r) no-repeat,
    linear-gradient(var(--pop-hi), var(--pop-hi)) center top / calc(100% - 2 * var(--r)) var(--r) no-repeat,
    linear-gradient(var(--pop-hi), var(--pop-hi)) left bottom / 100% calc(100% - var(--r)) no-repeat;
}
.pop-drawer::before { content: ''; position: absolute; inset: 0; border-radius: var(--r) var(--r) 0 0; pointer-events: none;
  box-shadow: 0 -0.5px 0 var(--cn-separator), 0 -10px 22px -14px rgba(0,0,0,0.45); clip-path: inset(-40px 0 calc(100% - var(--r) - 1px) 0); }
.pop-drawer__grab { position: absolute; left: 0; right: 0; top: 0; height: 14px; z-index: 1; display: grid; place-items: center; cursor: row-resize; touch-action: none; }
.pop-drawer__grab i { width: 36px; height: 4px; border-radius: 2px; background: var(--cn-label-tertiary); opacity: 0.5; transition: opacity 120ms, background-color 120ms; }
.pop-drawer__grab:hover i { opacity: 0.9; }
.pop-drawer__grab[data-dragging] i { opacity: 1; background: var(--cn-accent); }
.pop-drawer .ai__bar { padding: 16px var(--cn-space-3) 6px; }
.pop-drawer .ai__input, .pop-pane--ai .ai__input { background: var(--cn-fill-quaternary); }

/* 侧边的缝：平时看不见，悬停时出现一根竖胶囊 */
.pop-seam { position: absolute; top: 0; bottom: 0; width: 10px; z-index: 4; cursor: col-resize; touch-action: none; }
.pop-seam::after { content: ''; position: absolute; left: 50%; top: 50%; width: 4px; height: 40px; border-radius: 2px;
                   transform: translate(-50%, -50%); background: var(--cn-label-tertiary); opacity: 0; transition: opacity 120ms; }
.pop-seam:hover::after { opacity: 0.8; }
.pop-seam[data-dragging]::after { opacity: 1; background: var(--cn-accent); }

/* 四边四角的改大小区：贴着面板边的一圈，看不见，鼠标放上去变成双向箭头 */
.pop-edge { position: absolute; z-index: 6; touch-action: none; }
.pop-edge--n { top: 0; left: 10px; right: 10px; height: 5px; cursor: ns-resize; }
.pop-edge--s { bottom: 0; left: 10px; right: 16px; height: 5px; cursor: ns-resize; }
.pop-edge--w { left: 0; top: 10px; bottom: 10px; width: 5px; cursor: ew-resize; }
.pop-edge--e { right: 0; top: 10px; bottom: 16px; width: 5px; cursor: ew-resize; }
.pop-edge--nw { left: 0; top: 0; width: 10px; height: 10px; cursor: nwse-resize; }
.pop-edge--ne { right: 0; top: 0; width: 10px; height: 10px; cursor: nesw-resize; }
.pop-edge--sw { left: 0; bottom: 0; width: 10px; height: 10px; cursor: nesw-resize; }
.pop__grip { position: absolute; right: 0; bottom: 0; z-index: 7; width: 14px; height: 14px; cursor: nwse-resize;
             background: linear-gradient(135deg, transparent 55%, var(--cn-label-tertiary) 55%, var(--cn-label-tertiary) 62%, transparent 62%, transparent 75%, var(--cn-label-tertiary) 75%, var(--cn-label-tertiary) 82%, transparent 82%); opacity: 0.5; }
.pop__grip:hover { opacity: 1; }
`;
