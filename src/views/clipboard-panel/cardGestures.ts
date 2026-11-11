// 剪贴板卡片的手势，面板（PanelView）和主窗口的剪贴板页（ClipboardPage）共用：
//
// - 多选：按住 Ctrl（macOS 上 ⌘ 也行）点卡片 = 勾上 / 取消；按住 Ctrl 在列表上拖 = 拉一个框，框到的都勾上
// - 拖出去粘贴：按住卡片拖（勾了几张、拖的是其中一张就带着全部），在窗口外面松手 = 粘贴到松手的位置；
//   松在分组标签上 = 归进那个分组。不走系统的拖放（拖放只带得了文字，图片、文件带不出去，
//   很多程序也不收拖进来的东西），而是自己跟着鼠标，松手时由 Rust 在那儿点一下再粘贴
//
// 卡片元素上要有 `data-clip-id`，分组标签上要有 `data-board-drop`（数字 = 分组 ID，clipboard = 移出分组）。

import './gestures.css';

import { useCallback, useEffect, useRef, useState, type RefObject } from 'react';

import { on } from '@/lib/events';
import { clipboard } from '@/lib/ipc';
import { modKey } from '@/lib/platform';
import type { ClipItem } from '@/lib/types';

/** 拖动超过这么多像素才算拖（不然是点） */
const DRAG_MIN = 6;
/** 多选拖动时鼠标在那一摞左上角外面多远（和 ghost.css 里的一摞配合：一摞在鼠标右下方） */
const PILE_GRAB = { x: -12, y: -16 };

function cardIdAt(target: EventTarget | null): number | null {
  const el = target instanceof Element ? target.closest<HTMLElement>('[data-clip-id]') : null;
  const id = el ? Number(el.dataset.clipId) : NaN;
  return Number.isFinite(id) ? id : null;
}

/** 刚拖完松手时浏览器还会补一个 click：吞掉，不然"单击粘贴"模式下会把卡片粘出去 */
function swallowNextClick() {
  const stop = (e: MouseEvent) => {
    e.stopPropagation();
    e.preventDefault();
  };
  window.addEventListener('click', stop, { capture: true, once: true });
  window.setTimeout(() => window.removeEventListener('click', stop, { capture: true }), 0);
}

/**
 * Ctrl 多选：点选和框选。`picked` 是勾上的卡片 ID。`container` 是放卡片的列表（滚动的那层）；
 * `zones` 是另外也能按下去开始框选的地方（面板顶栏的空白处），框选的还是 `container` 里的卡片。
 *
 * 框是直接挂在 body 上的一个 fixed 元素、按屏幕坐标画，拖动时只改它的样式，不走 React 状态 ——
 * 主窗口截图库一页上百张图，每动一下都重新渲染整页会很卡。卡片的位置也只在开始时量一次（滚动了再量），
 * 每帧最多算一次，勾上的集合真的变了才更新
 */
export function useMultiPick(container: RefObject<HTMLElement | null>, enabled: boolean, zones: RefObject<HTMLElement | null>[] = []) {
  const [picked, setPicked] = useState<Set<number>>(() => new Set());
  const pickedRef = useRef(picked);
  pickedRef.current = picked;
  const zonesRef = useRef(zones);
  zonesRef.current = zones;

  const toggle = useCallback((id: number) => {
    setPicked((old) => {
      const next = new Set(old);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);
  const clear = useCallback(() => setPicked((old) => (old.size ? new Set() : old)), []);

  useEffect(() => {
    const list = container.current;
    if (!list || !enabled) return;
    const onDown = (e: PointerEvent) => {
      if (e.button !== 0 || !modKey(e) || e.altKey || e.shiftKey) return;
      // 卡片上的按钮（快捷操作、勾选框）、顶栏的标签、搜索框自己处理
      if (e.target instanceof Element && e.target.closest('button, input, textarea, a, [role="tab"], [data-board-drop]')) return;
      const zone = e.currentTarget as HTMLElement;
      // 不让卡片收到这次按下（选中、单击粘贴都不要）
      e.preventDefault();
      e.stopPropagation();
      zone.setPointerCapture(e.pointerId);
      const start = { x: e.clientX, y: e.clientY };
      const base = new Set(pickedRef.current);
      const tapped = cardIdAt(e.target);
      let moved = false;

      // 卡片的位置（屏幕坐标）：开始时量一次，列表滚动了重新量
      let cards: { id: number; r: DOMRect }[] = [];
      const measure = () => {
        cards = Array.from(list.querySelectorAll<HTMLElement>('[data-clip-id]'), (c) => ({ id: Number(c.dataset.clipId), r: c.getBoundingClientRect() }));
      };
      let box: HTMLDivElement | null = null;
      let last = { x: start.x, y: start.y };
      let frame = 0;
      let current = base;
      const update = () => {
        frame = 0;
        const x0 = Math.min(start.x, last.x);
        const y0 = Math.min(start.y, last.y);
        const x1 = Math.max(start.x, last.x);
        const y1 = Math.max(start.y, last.y);
        if (!box) {
          box = document.createElement('div');
          box.className = 'clip-marquee';
          document.body.appendChild(box);
          measure();
        }
        box.style.transform = `translate(${x0}px, ${y0}px)`;
        box.style.width = `${x1 - x0}px`;
        box.style.height = `${y1 - y0}px`;
        const next = new Set(base);
        for (const { id, r } of cards) if (r.right >= x0 && r.left <= x1 && r.bottom >= y0 && r.top <= y1) next.add(id);
        if (next.size !== current.size || [...next].some((id) => !current.has(id))) {
          current = next;
          setPicked(next);
        }
      };
      const onMove = (m: PointerEvent) => {
        if (!moved && Math.hypot(m.clientX - start.x, m.clientY - start.y) < DRAG_MIN) return;
        moved = true;
        last = { x: m.clientX, y: m.clientY };
        if (!frame) frame = requestAnimationFrame(update);
      };
      const onScroll = () => {
        measure();
        if (moved && !frame) frame = requestAnimationFrame(update);
      };
      const onUp = () => {
        zone.removeEventListener('pointermove', onMove);
        zone.removeEventListener('pointerup', onUp);
        zone.removeEventListener('pointercancel', onUp);
        list.removeEventListener('scroll', onScroll);
        if (frame) cancelAnimationFrame(frame);
        (box as HTMLDivElement | null)?.remove();
        if (!moved && tapped !== null) toggle(tapped);
        swallowNextClick();
      };
      zone.addEventListener('pointermove', onMove);
      zone.addEventListener('pointerup', onUp);
      zone.addEventListener('pointercancel', onUp);
      list.addEventListener('scroll', onScroll, { passive: true });
    };
    const els = [list, ...zonesRef.current.map((z) => z.current).filter((z): z is HTMLElement => !!z)];
    for (const el of els) el.addEventListener('pointerdown', onDown, { capture: true });
    return () => {
      for (const el of els) el.removeEventListener('pointerdown', onDown, { capture: true });
    };
  }, [container, enabled, toggle]);

  return { picked, setPicked, toggle, clear };
}

/**
 * 按住卡片拖。`idsFor(id)` 给出这次拖的是哪几张（拖的是勾上的其中一张就是全部勾上的）。
 * 在窗口外面松手调 `onDropOutside`，松在分组标签上调 `onDropOnBoard`，松在窗口里别的地方什么都不做
 */
/**
 * 多选拖动时的"收拢"：勾上的其他卡片飞向手上那张、缩小淡出，然后影子里叠成一叠。返回复原的函数（拖完调）
 */
function gather(ids: number[], grabbed: HTMLElement): () => void {
  const target = grabbed.getBoundingClientRect();
  const moved: HTMLElement[] = [];
  for (const id of ids) {
    const el = document.querySelector<HTMLElement>(`[data-clip-id="${id}"]`);
    if (!el || el === grabbed) continue;
    const r = el.getBoundingClientRect();
    el.style.transition = 'transform 260ms cubic-bezier(0.3, 0.8, 0.3, 1), opacity 260ms ease-out';
    el.style.transform = `translate(${target.left - r.left}px, ${target.top - r.top}px) scale(0.86)`;
    el.style.opacity = '0';
    el.style.zIndex = '6';
    moved.push(el);
  }
  return () => {
    for (const el of moved) {
      el.style.transition = 'transform 220ms cubic-bezier(0.3, 0.8, 0.3, 1), opacity 220ms ease-out';
      el.style.transform = '';
      el.style.opacity = '';
      window.setTimeout(() => {
        el.style.transition = '';
        el.style.zIndex = '';
      }, 240);
    }
  };
}

export function useCardDrag(opts: {
  idsFor: (id: number) => number[];
  /** 影子里画的那几条（带上标题栏的颜色 ghostTint） */
  itemsFor: (ids: number[]) => (ClipItem & { ghostTint?: string })[];
  /** 拖到外面松手（要粘贴）。这时面板还是"让开"的状态（不调 `onAway(false)`）：先滑回来再收起会闪一下，
   *  由调用方在窗口藏好之后自己复原 */
  onDropOutside: (ids: number[]) => void;
  onDropOnBoard?: (ids: number[], board: number | null) => void;
  /**
   * 拖出这个元素（剪贴板面板的卡片区；鼠标在分组标签上时不算）之后调一次 `onAway(true)`：面板让开（滑下去、不挡鼠标），后面的输入框才
   * 看得见、拖得进去。让开以后松手一律算"拖到外面"。拖完 / 取消时调 `onAway(false)`
   */
  awayFrom?: () => HTMLElement | null;
  onAway?: (away: boolean) => void;
  /**
   * 用系统原生拖拽（macOS 面板）：拖动图像由系统画、跟手，松手由目标程序自己收，不用 `onDropOutside`。
   * 被面板外的程序收下时 Rust 已经收起了面板，再调这个（复原"让开"的状态）
   */
  native?: { onDropped: () => void };
}) {
  const [dragging, setDragging] = useState<number[] | null>(null);
  const optsRef = useRef(opts);
  optsRef.current = opts;

  const onPointerDown = useCallback((e: React.PointerEvent<HTMLElement>, id: number) => {
    if (e.button !== 0 || modKey(e) || e.shiftKey || e.altKey) return;
    if (e.target instanceof Element && e.target.closest('button, input, textarea')) return;
    const el = e.currentTarget;
    const pointerId = e.pointerId;
    const start = { x: e.clientX, y: e.clientY };
    // 鼠标按在卡片上的哪个点：影子里的卡片也是这个点在鼠标下（主窗口的一行比卡片宽，收在卡片里面）
    const box = el.getBoundingClientRect();
    const grab = { x: Math.min(e.clientX - box.left, 200), y: Math.min(e.clientY - box.top, 200) };
    let ids: number[] | null = null;
    let over: HTMLElement | null = null;
    let restore: (() => void) | null = null;
    let away = false;
    let unlisten: (() => void)[] = [];
    let done = false;
    const setOver = (next: HTMLElement | null) => {
      if (next === over) return;
      over?.removeAttribute('data-drag-over');
      next?.setAttribute('data-drag-over', '');
      over = next;
    };
    // keepAway：拖到外面松手要粘贴了，面板别滑回来（见 onDropOutside）
    const finish = (keepAway = false) => {
      done = true;
      for (const fn of unlisten) fn();
      unlisten = [];
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('pointercancel', onCancel);
      window.removeEventListener('keydown', onKey, true);
      setOver(null);
      document.documentElement.removeAttribute('data-card-dragging');
      document.documentElement.removeAttribute('data-drag-snap');
      if (ids) {
        swallowNextClick();
        if (!optsRef.current.native) void clipboard.ghostEnd().catch(() => undefined);
      }
      restore?.();
      restore = null;
      if (away && !keepAway) optsRef.current.onAway?.(false);
      away = false;
      setDragging(null);
    };
    const onMove = (m: PointerEvent) => {
      if (!ids) {
        if (Math.hypot(m.clientX - start.x, m.clientY - start.y) < DRAG_MIN) return;
        ids = optsRef.current.idsFor(id);
        if (optsRef.current.native) {
          startNative(ids);
          return;
        }
        // 拿住鼠标：拖到窗口外面也收得到移动和松开
        try {
          el.setPointerCapture(pointerId);
        } catch {
          // 元素已经不在了（列表滚动回收了）：照样靠 window 上的监听
        }
        document.documentElement.setAttribute('data-card-dragging', '');
        // 拖到这么远之前网页可能已经开始选字了（macOS），先清掉
        window.getSelection()?.removeAllRanges();
        setDragging(ids);
        // 影子跟着鼠标走（拖出窗口也看得见）；拖好几张时先把它们收拢到手上这张
        if (ids.length > 1) restore = gather(ids, el);
        // 手上那张画在最上面（粘贴的顺序还是列表里的顺序）
        const shown = [id, ...ids.filter((x) => x !== id)];
        // 拖好几张时影子是一小摞、摆在鼠标右下方（和 macOS 系统拖拽的样子一样）：告诉影子窗口
        // "按住的点"在卡片左上角外面一点，鼠标就落在那一摞的左上方
        void clipboard.ghostStart(optsRef.current.itemsFor(shown), ids.length > 1 ? PILE_GRAB : grab).catch(() => undefined);
        // macOS 上鼠标拖出窗口后页面收不到移动，面板让开后（鼠标穿透）连松开也收不到：
        // 靠 Rust 那边盯着鼠标告诉这里"出窗口了""松手了"
        const keep = (p: Promise<() => void>) =>
          void p.then((fn) => {
            if (done) fn();
            else unlisten.push(fn);
          });
        keep(on('clip-ghost-left', () => {
          if (done || away) return;
          away = true;
          setOver(null);
          optsRef.current.onAway?.(true);
        }));
        keep(on('clip-ghost-released', (outside) => {
          if (done || !ids || !(away || outside)) return;
          const picked = ids;
          finish(true);
          optsRef.current.onDropOutside(picked);
        }));
      }
      const hitNow = away ? null : document.elementFromPoint(m.clientX, m.clientY);
      const onBoard = hitNow instanceof Element && !!hitNow.closest('[data-board-drop]');
      if (!away && !onBoard) {
        const zone = optsRef.current.awayFrom?.();
        const r = zone?.getBoundingClientRect();
        if (r && (m.clientX < r.left || m.clientX > r.right || m.clientY < r.top || m.clientY > r.bottom)) {
          away = true;
          setOver(null);
          optsRef.current.onAway?.(true);
        }
      }
      if (away) return;
      setOver(hitNow instanceof Element ? hitNow.closest<HTMLElement>('[data-board-drop]') : null);
    };
    const onUp = (u: PointerEvent) => {
      const picked = ids;
      const board = over?.dataset.boardDrop;
      const wasAway = away;
      const outside = !!picked && (wasAway || u.clientX < 0 || u.clientY < 0 || u.clientX >= window.innerWidth || u.clientY >= window.innerHeight);
      finish(outside);
      if (!picked) return;
      if (outside) optsRef.current.onDropOutside(picked);
      else if (board !== undefined) optsRef.current.onDropOnBoard?.(picked, board === 'clipboard' ? null : Number(board));
    };
    const onCancel = () => finish();
    // 系统原生拖拽：开始以后页面收不到 pointer 事件了，鼠标在面板里时靠 HTML 的 dragover / drop（分组标签、
    // 拖出卡片区让开），出了面板、松手靠 Rust 转来的事件
    const startNative = (picked: number[]) => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('pointercancel', onCancel);
      window.removeEventListener('keydown', onKey, true);
      window.getSelection()?.removeAllRanges();
      const goAway = () => {
        if (done || away) return;
        away = true;
        setOver(null);
        optsRef.current.onAway?.(true);
      };
      // dragover 只用来标出鼠标下的分组标签，松手后按 Rust 给的位置找标签（不靠 drop：WebKit 收下图片 / 文件
      // 会去打开它，面板窗口拦掉了导航）。鼠标在顶栏的空白处不让开（让开以后窗口鼠标穿透，就拖不到
      // 标签上了），出了面板窗口才让开（Rust 发 clip-ghost-left）：面板贴着屏幕底边，往上拖过顶栏马上就出窗口了
      const onDragOver = (d: DragEvent) => {
        if (done || away) return;
        setOver(d.target instanceof Element ? d.target.closest<HTMLElement>('[data-board-drop]') : null);
      };
      // 万一还是落到了页面上：别让 WebKit 打开拖进来的文件
      const onDrop = (d: DragEvent) => d.preventDefault();
      const onDragLeave = (d: DragEvent) => {
        if (!d.relatedTarget) setOver(null);
      };
      document.addEventListener('dragover', onDragOver, true);
      document.addEventListener('drop', onDrop, true);
      document.addEventListener('dragleave', onDragLeave, true);
      unlisten.push(() => {
        document.removeEventListener('dragover', onDragOver, true);
        document.removeEventListener('drop', onDrop, true);
        document.removeEventListener('dragleave', onDragLeave, true);
      });
      const keep = (p: Promise<() => void>) =>
        void p.then((fn) => {
          if (done) fn();
          else unlisten.push(fn);
        });
      keep(on('clip-ghost-left', goAway));
      keep(
        on('clip-native-drag-end', ({ accepted, inside, x, y }) => {
          if (done) return;
          if (accepted && !inside) {
            finish(true);
            optsRef.current.native?.onDropped();
            return;
          }
          const hit = inside && !away ? document.elementFromPoint(x, y)?.closest<HTMLElement>('[data-board-drop]') : null;
          const board = hit?.dataset.boardDrop;
          finish();
          if (board !== undefined) optsRef.current.onDropOnBoard?.(picked, board === 'clipboard' ? null : Number(board));
        }),
      );
      // 每张卡片的位置（滚出视野没画出来的是 null）：Rust 截下来叠成一摞当拖动图像（像 Paste）。
      // 先截图再把卡片标成"拖着"（变淡），所以标记等系统开始拖了（clip-native-drag-begin）再做；也不做多选的"收拢"动画
      keep(
        on('clip-native-drag-begin', () => {
          if (done) return;
          document.documentElement.removeAttribute('data-drag-snap');
          document.documentElement.setAttribute('data-card-dragging', '');
          setDragging(picked);
        }),
      );
      // 截图时藏起勾选圈、选中框（Paste 拖出来的卡片上没有这些），画好这一帧再截
      document.documentElement.setAttribute('data-drag-snap', '');
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          if (done) return;
          // 藏好以后再量（按下时卡片缩了一点，藏的时候也复原了）
          const rects = picked.map((pid): [number, number, number, number] | null => {
            const node = pid === id ? el : document.querySelector<HTMLElement>(`[data-clip-id="${pid}"]`);
            const r = node?.getBoundingClientRect();
            return r && r.width > 0 && r.right > 0 && r.left < window.innerWidth ? [r.left, r.top, r.width, r.height] : null;
          });
          const card = el.getBoundingClientRect();
          void clipboard
            .nativeDrag(picked, rects, Math.max(0, picked.indexOf(id)), [start.x - card.left, start.y - card.top])
            .catch(() => finish());
        }),
      );
    };
    // Esc：不拖了
    const onKey = (k: KeyboardEvent) => {
      if (k.key !== 'Escape' || !ids) return;
      k.preventDefault();
      k.stopPropagation();
      finish();
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    window.addEventListener('pointercancel', onCancel);
    window.addEventListener('keydown', onKey, true);
  }, []);

  return { dragging, onPointerDown };
}

/** 按住某个修饰键（Shift、Ctrl）的状态；窗口失焦时当松开 */
export function useKeyHeld(key: 'Shift' | 'Control', active = true): boolean {
  const [held, setHeld] = useState(false);
  useEffect(() => {
    if (!active) {
      setHeld(false);
      return;
    }
    const sync = (e: KeyboardEvent) => setHeld(key === 'Shift' ? e.shiftKey : e.ctrlKey || e.metaKey);
    const reset = () => setHeld(false);
    window.addEventListener('keydown', sync);
    window.addEventListener('keyup', sync);
    window.addEventListener('blur', reset);
    return () => {
      window.removeEventListener('keydown', sync);
      window.removeEventListener('keyup', sync);
      window.removeEventListener('blur', reset);
    };
  }, [key, active]);
  return held;
}
