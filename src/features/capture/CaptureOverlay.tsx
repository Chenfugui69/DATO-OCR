/**
 * 截图遮罩层。
 *
 * M0 只做验收清单要求的四件事：显示冻结的屏幕画面、压暗、拖拽框选、
 * Enter 复制 / Esc 退出。工具条、自动窗口检测、放大镜、8 点调整都是 M1/M2 的活
 * （见 02-截图模块.md）。
 *
 * # 这一层**不画**冻结画面
 *
 * 画面来自本窗口下面的一个原生底图窗口（Rust 侧的 `platform::BackdropLayer`）。
 * 这里只画压暗层和选区，其余部分完全透明，让底图透上来。
 *
 * 这是对规格 02 §4 那张分层表的偏离：原方案第 1 层是 `<canvas id="background">`。
 * 改的原因是延迟 —— 底图进 WebView 光传输就要 134ms（4K 单屏，双 4K 翻倍），
 * 而预算总共 150ms；另外 Chromium 的色彩管理会在广色域屏上改动像素值，破坏
 * "与真实桌面像素级一致"。完整数据见 `src-tauri/src/capture/protocol.rs`。
 *
 * # 像素级一致是怎么做到的
 *
 * 窗口按显示器的**物理**尺寸摆放。CSS 像素和物理像素的比例靠实测得出
 * （`底图物理宽 / window.innerWidth`），**不能**用 `window.devicePixelRatio`
 * —— 两者在 WebView2 里不一定相等，细节见 `geometry.ts` 里 `pixelRatio` 的注释。
 * 选区坐标按这个实测比例换算成物理像素，交给 Rust 去裁真正的底图。
 *
 * # 两段式：先出画面，像素随后到
 *
 * 1. 收到会话通知 → 调 `capturePrepare()` 拿几何信息 → 压暗层进 DOM →
 *    调 `captureOverlayReady()`，Rust 把底图窗口和本窗口同帧显示出来
 * 2. 与此同时异步拉底图像素（`shot:` 协议）。放大镜、取色、马赛克要读背景原始
 *    像素，按需 IPC 撑不住 60fps，所以还是得整张拿进来 —— 但不该让它挡住出画面
 *
 * 实测画面约 75ms 出来、像素 150–250ms 到位。用户从看到画面到手开始动至少
 * 200–400ms，所以感知不到这个差。
 */

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';

import { on } from '@/lib/events';
import {
  captureCancel,
  captureFinish,
  captureImageUrl,
  captureOverlayBoot,
  captureOverlayPixelsReady,
  captureOverlayReady,
  capturePrepare,
  reportError,
  type MonitorSnapshot,
} from '@/lib/ipc';
import {
  clampRect,
  cssToVirtual,
  frameRect,
  isUsableSelection,
  physicalToCssLength,
  rectFromPoints,
  virtualToCss,
  type MonitorFrame,
  type Point,
  type Rect,
} from './geometry';

const LABEL_PREFIX = 'capture-';

function monitorIdFromLabel(label: string): number | null {
  if (!label.startsWith(LABEL_PREFIX)) return null;
  const parsed = Number(label.slice(LABEL_PREFIX.length));
  return Number.isFinite(parsed) ? parsed : null;
}

export function CaptureOverlay() {
  const [snapshot, setSnapshot] = useState<MonitorSnapshot | null>(null);
  const [selection, setSelection] = useState<Rect | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const anchorRef = useRef<Point | null>(null);
  const pendingRef = useRef<Rect | null>(null);
  const rafRef = useRef<number | null>(null);

  /**
   * 背景原始像素。M0 没有消费者 —— 放大镜（6 倍实时取色）、取色快捷键、马赛克
   * 都是 M1/M2 的活，届时从这里读。
   *
   * 现在就把通道打通并埋点，是因为它决定那几个功能的可用时机，而"多久能拿到"
   * 只有真跑起来才知道。存在 ref 里而不是 state：它不参与渲染。
   */
  const pixelsRef = useRef<ImageBitmap | null>(null);

  /** `capture_prepare` 那一段的耗时，只为了随 `captureOverlayReady` 报上去。 */
  const prepareMsRef = useRef<number>(0);

  const frame: MonitorFrame | null = useMemo(() => {
    if (!snapshot) return null;

    // 实测 CSS→物理 的比例。窗口是按物理像素摆的，所以 CSS 视口宽度和底图
    // 物理宽度之比就是真实比例，不用猜 WebView 内部怎么处理 DPI。
    const viewportWidth = window.innerWidth;
    const pixelRatio = viewportWidth > 0 ? snapshot.width / viewportWidth : 1;

    return {
      originX: snapshot.x,
      originY: snapshot.y,
      width: snapshot.width,
      height: snapshot.height,
      pixelRatio,
    };
  }, [snapshot]);

  // ── 每次会话开始 ──────────────────────────────────────────────────────────
  //
  // 窗口是常驻复用的，所以这里等的是 `capture-session-start` 事件，而不是
  // 在 mount 时干一次就完事。mount 只发生在启动预建的那一刻，那时还没有会话。
  useEffect(() => {
    const label = getCurrentWebviewWindow().label;
    const monitorId = monitorIdFromLabel(label);
    if (monitorId === null) {
      setFailure(`窗口标签 ${label} 不是遮罩窗口`);
      return;
    }

    const unlisteners: Array<() => void> = [];
    let disposed = false;
    // 只认最后一次会话。连按 F1 时上一轮的图可能比这一轮晚加载完，
    // 不挡住的话会把旧底图画到新会话上。
    let latest = 0;

    function track(pending: Promise<() => void>) {
      void pending.then((fn) => {
        if (disposed) fn();
        else unlisteners.push(fn);
      });
    }

    track(
      on('capture-session-end', () => {
        latest = 0;
        // ImageBitmap 是内存里的一整张 4K 图，不 close 就得等 GC，
        // 空闲内存会被它长期占着（规格 00 的 80MB 目标）。
        pixelsRef.current?.close();
        pixelsRef.current = null;
        setSnapshot(null);
        setSelection(null);
        anchorRef.current = null;
        pendingRef.current = null;
      }),
    );

    track(
      on('capture-session-start', ({ sessionId }) => {
        // 同一个会话可能被通知两次：热键路径广播过一次，前端 boot 时又主动问了
        // 一次（补的是"热键比 WebView 加载更快"那种情况）。去重靠会话号。
        if (sessionId === latest) return;
        latest = sessionId;

        void (async () => {
          const startedAt = performance.now();
          try {
            const prepared = await capturePrepare();
            const mine = prepared.monitors.find((monitor) => monitor.id === monitorId);
            if (!mine) {
              setFailure(`会话里没有显示器 ${monitorId} 的画面`);
              return;
            }
            if (disposed || sessionId !== latest) return;

            setFailure(null);
            setSelection(null);
            anchorRef.current = null;
            pendingRef.current = null;
            // 这一行让压暗层进 DOM。下面那个 useLayoutEffect 会接着调
            // captureOverlayReady()，把画面显示出来。
            setSnapshot(mine);
            prepareMsRef.current = performance.now() - startedAt;

            // 像素单独一条路，不 await 在出画面之前。
            void loadPixels(monitorId, sessionId, prepared.backdropFormat);
          } catch (error) {
            if (disposed) return;
            const message = error instanceof Error ? error.message : String(error);
            setFailure(message);
            void reportError('capture-overlay', message);
            // 会话必须收掉，否则它一直挂在 Rust 那边，之后每次 F1 都被
            // "已在截图中"挡回去 —— 而窗口是隐藏的，用户只看到热键彻底失灵，
            // 连报错都看不见。这条路我自己踩过一次（CSP 拦了 fetch）。
            void captureCancel().catch(() => {});
          }
        })();

        /** 把背景原始像素拉进来，供放大镜 / 取色 / 马赛克用。失败不影响截图。 */
        async function loadPixels(
          monitor: number,
          session: number,
          format: 'bmp' | 'png',
        ): Promise<void> {
          const startedAt = performance.now();
          try {
            // 刻意用 `blob()` 而不是 `arrayBuffer()`：后者要先把整张图落进 JS 堆，
            // 再 `new Blob([bytes])` 拷第二遍才能喂给 `createImageBitmap`。
            const response = await fetch(captureImageUrl(monitor, session, format));
            const blob = await response.blob();
            const fetchedAt = performance.now();

            const bitmap = await createImageBitmap(blob);
            const decodedAt = performance.now();

            if (disposed || session !== latest) {
              bitmap.close();
              return;
            }

            pixelsRef.current?.close();
            pixelsRef.current = bitmap;

            await captureOverlayPixelsReady(monitor, {
              fetchMs: fetchedAt - startedAt,
              decodeMs: decodedAt - fetchedAt,
            });
          } catch (error) {
            if (disposed) return;
            // 只报不炸：画面已经在屏幕上了，框选和复制都不依赖这些像素。
            // 真正受影响的是放大镜和取色，那属于降级而不是失败。
            const message = error instanceof Error ? error.message : String(error);
            void reportError('capture-overlay-pixels', message);
          }
        }
      }),
    );

    // 监听挂好之后再报到，顺序反了会漏掉 Rust 补发的那次通知。
    void captureOverlayBoot(monitorId).catch(() => {
      // 报到失败只影响"热键早于前端就绪"这种边角情况，不值得打断整个遮罩。
    });

    return () => {
      disposed = true;
      for (const fn of unlisteners) fn();
    };
  }, []);

  // ── 键盘 ─────────────────────────────────────────────────────────────────
  useEffect(() => {
    // 没有会话就不该响应按键。窗口是常驻隐藏的，理论上拿不到焦点，
    // 但真让隐藏窗口把用户的 Esc 吃掉会非常难查。
    if (!snapshot) return;

    function onKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault();
        void captureCancel().catch(() => {
          // 窗口正在关闭，IPC 断了是正常的
        });
        return;
      }

      if (event.key === 'Enter' && selection && isUsableSelection(selection)) {
        event.preventDefault();
        void captureFinish(selection, 'copy').catch(() => {
          // 失败已经在 ipc 层记过日志了
        });
      }
    }

    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [selection, snapshot]);

  useEffect(
    () => () => {
      if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    },
    [],
  );

  /**
   * 压暗层已经进 DOM，告诉 Rust 可以让画面出来了。
   *
   * 用 `useLayoutEffect` 而不是 `useEffect`：必须在浏览器有机会绘制之前就把这个
   * 请求发出去，但更重要的是它保证 DOM 已经更新完 —— 窗口显示出来的那一帧必须
   * 已经带着压暗层，否则会闪一下没压暗的原图。
   *
   * 这里不能等 `requestAnimationFrame` 再发：窗口此刻还是隐藏的，隐藏窗口的 rAF
   * 被掐到 1Hz，实测白等一秒。
   */
  useLayoutEffect(() => {
    if (!snapshot) return;

    void captureOverlayReady(snapshot.id, prepareMsRef.current).catch(() => {
      // 会话可能已经被 Esc 收掉了，这里失败无所谓
    });
  }, [snapshot]);

  /**
   * 每帧最多提交一次选区更新。
   *
   * 高刷屏上 pointermove 可能比渲染还密，直接 setState 会积压掉帧
   * （规格 00 §6.4 要求拖拽稳定 60fps）。
   */
  const scheduleSelection = useCallback((rect: Rect) => {
    pendingRef.current = rect;
    if (rafRef.current !== null) return;

    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = null;
      if (pendingRef.current) setSelection(pendingRef.current);
    });
  }, []);

  const onPointerDown = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (!frame || event.button !== 0) return;

      // 多屏时只有主屏那块窗口初始带焦点，点到哪块就把焦点给它，
      // 否则 Enter/Esc 会发到别的窗口去。
      void getCurrentWebviewWindow().setFocus().catch(() => {});

      event.currentTarget.setPointerCapture(event.pointerId);
      const anchor = cssToVirtual({ x: event.clientX, y: event.clientY }, frame);
      anchorRef.current = anchor;
      setSelection({ x: anchor.x, y: anchor.y, width: 0, height: 0 });
    },
    [frame],
  );

  const onPointerMove = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      const anchor = anchorRef.current;
      if (!frame || !anchor) return;

      const cursor = cssToVirtual({ x: event.clientX, y: event.clientY }, frame);
      scheduleSelection(clampRect(rectFromPoints(anchor, cursor), frameRect(frame)));
    },
    [frame, scheduleSelection],
  );

  const onPointerUp = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    anchorRef.current = null;
  }, []);

  if (failure) {
    return <div className="overlay-failure">CHENOCR 截图初始化失败：{failure}</div>;
  }

  // 还没拿到几何信息时什么都不画。此时窗口仍是隐藏的，用户看不到空白。
  if (!snapshot || !frame) return null;

  const viewport = {
    width: physicalToCssLength(snapshot.width, frame),
    height: physicalToCssLength(snapshot.height, frame),
  };

  const active = selection && isUsableSelection(selection) ? selection : null;
  const box = active
    ? (() => {
        const topLeft = virtualToCss({ x: active.x, y: active.y }, frame);
        return {
          left: topLeft.x,
          top: topLeft.y,
          width: physicalToCssLength(active.width, frame),
          height: physicalToCssLength(active.height, frame),
        };
      })()
    : null;

  return (
    <div
      className="overlay-root"
      style={{ width: viewport.width, height: viewport.height }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onContextMenu={(event) => {
        // 规格 07 §8 的 capture.rightClickBehavior 默认 'exit'
        event.preventDefault();
        void captureCancel().catch(() => {});
      }}
    >
      {/*
        没有 background 层：冻结画面在本窗口下面的原生底图窗口里（见文件头）。
        这里所有非压暗区域都必须保持完全透明，底图才能透上来。
      */}
      {box ? (
        <>
          <div className="overlay-dim" style={{ left: 0, top: 0, width: viewport.width, height: box.top }} />
          <div
            className="overlay-dim"
            style={{
              left: 0,
              top: box.top + box.height,
              width: viewport.width,
              height: Math.max(0, viewport.height - box.top - box.height),
            }}
          />
          <div className="overlay-dim" style={{ left: 0, top: box.top, width: box.left, height: box.height }} />
          <div
            className="overlay-dim"
            style={{
              left: box.left + box.width,
              top: box.top,
              width: Math.max(0, viewport.width - box.left - box.width),
              height: box.height,
            }}
          />
          <div
            className="overlay-selection"
            style={{ left: box.left, top: box.top, width: box.width, height: box.height }}
          />
          <SizeBadge selection={active!} box={box} viewport={viewport} />
        </>
      ) : (
        <div className="overlay-dim" style={{ left: 0, top: 0, width: viewport.width, height: viewport.height }} />
      )}

      {!active && snapshot.isPrimary ? (
        <div className="overlay-hint">拖拽框选　·　Enter 复制　·　Esc 取消</div>
      ) : null}
    </div>
  );
}

/** 选区尺寸标签，单位是物理像素 —— 验收要核对"尺寸精确"，得看得见数字。 */
function SizeBadge({
  selection,
  box,
  viewport,
}: {
  selection: Rect;
  box: { left: number; top: number; width: number; height: number };
  viewport: { width: number; height: number };
}) {
  const BADGE_HEIGHT = 24;
  const GAP = 6;

  // 默认贴在选区上方；顶部空间不够就翻到选区内侧，不要跑出屏幕。
  const above = box.top - BADGE_HEIGHT - GAP;
  const top = above >= 0 ? above : Math.min(box.top + GAP, viewport.height - BADGE_HEIGHT);
  const left = Math.min(box.left, Math.max(0, viewport.width - 120));

  return (
    <div className="overlay-size" style={{ left, top, height: BADGE_HEIGHT }}>
      {selection.width} × {selection.height}
    </div>
  );
}
