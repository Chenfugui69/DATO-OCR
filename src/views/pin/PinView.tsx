// 贴图窗口（规格 02 §6，参照 Snipaste）：
// 左键拖动 · 滚轮缩放（以鼠标为锚点，20%–500%）· Ctrl+滚轮调不透明度（20%–100%）·
// 双击收成小圆点 / 恢复 · 右键菜单 · Esc 或中键关闭。
// 图里有字的话可以直接拖选：鼠标第一次移上来时识别一遍，在图上铺一层透明文字（位置、大小和图上的字对齐），
// 在字上按下是选字，在空白处按下仍是拖动贴图。

import { CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu } from '@tauri-apps/api/menu';
import { PhysicalPosition, PhysicalSize } from '@tauri-apps/api/dpi';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { capture, pin, system } from '@/lib/ipc';
import { modKey, shotUrl, windowLabel } from '@/lib/platform';
import type { PinInfo, PinText } from '@/lib/types';
import { notify } from '@/ui/overlays';

const LABEL = windowLabel();
const DOT = 24;
/** 透明文字的字号 = 框高 × 这个比例（行高留一点给上下的空白） */
const TEXT_EM = 0.86;
const TEXT_FONT = `-apple-system, 'PingFang SC', 'Segoe UI', 'Microsoft YaHei UI', sans-serif`;

/**
 * 当前选中的文字。各段文字是一个个绝对定位的块，浏览器拼起来时段与段之间什么都不加
 * （「第一行第二行」连成一串），这里自己拼：同一行的段之间加空格，换了行加换行。
 */
function selectedText(texts: PinText[] | null): string {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return '';
  const range = sel.getRangeAt(0);
  const spans = [...document.querySelectorAll<HTMLElement>('.pin-text span[data-i]')];
  let out = '';
  let prev: PinText | null = null;
  for (const span of spans) {
    if (!range.intersectsNode(span)) continue;
    const box = texts?.[Number(span.dataset.i)];
    const full = span.textContent ?? '';
    const start = span.contains(range.startContainer) ? range.startOffset : 0;
    const end = span.contains(range.endContainer) ? range.endOffset : full.length;
    const part = full.slice(start, end);
    if (!part) continue;
    if (prev && box) {
      // 下一段的竖直中线落在上一段的高度范围里，就算同一行
      const mid = box.y + box.height / 2;
      out += mid > prev.y && mid < prev.y + prev.height ? ' ' : '\n';
    }
    out += part;
    prev = box ?? prev;
  }
  return out.trim() || sel.toString().trim();
}

/** 每段文字横向要拉伸多少才和图上的字一样宽：按 100px 字号量一次，跟缩放无关 */
function useTextFits(texts: PinText[] | null): number[] {
  return useMemo(() => {
    if (!texts) return [];
    const ctx = document.createElement('canvas').getContext('2d');
    if (!ctx) return texts.map(() => 1);
    ctx.font = `100px ${TEXT_FONT}`;
    return texts.map((b) => {
      const natural = (ctx.measureText(b.text).width * b.height * TEXT_EM) / 100;
      return natural > 0 ? b.width / natural : 1;
    });
  }, [texts]);
}

export default function PinView() {
  const { t } = useTranslation();
  const [info, setInfo] = useState<PinInfo | null>(null);
  const [zoom, setZoom] = useState(1);
  const [opacity, setOpacity] = useState(1);
  const [collapsed, setCollapsed] = useState(false);
  const [onTop, setOnTop] = useState(true);
  const [closing, setClosing] = useState(false);
  const zoomRef = useRef(1);
  const [texts, setTexts] = useState<PinText[] | null>(null);
  /** 旋转 / 翻转后图变了，地址加个版本号让 img 重新取 */
  const [imageVersion, setImageVersion] = useState(0);
  const textAsked = useRef(false);
  const fits = useTextFits(texts);

  // 鼠标移上来才识别：大多数贴图只是看一眼，不必每张都跑一遍识字
  const loadText = useCallback(() => {
    if (textAsked.current) return;
    textAsked.current = true;
    pin
      .text(LABEL)
      .then(setTexts)
      .catch(() => setTexts([]));
  }, []);

  const copySelectedText = useCallback(async () => {
    const text = selectedText(texts);
    if (!text) return false;
    await capture.writeText(text);
    notify.success(t('pin.textCopied'));
    return true;
  }, [t, texts]);

  useEffect(() => {
    pin.info(LABEL).then(setInfo).catch(notify.error);
  }, []);

  const dpr = window.devicePixelRatio;

  /** 把窗口调到 zoom 下的尺寸。anchor：保持不动的点（窗口内 CSS 坐标）。 */
  const applyZoom = useCallback(
    async (next: number, anchor?: { x: number; y: number }) => {
      if (!info) return;
      const win = getCurrentWindow();
      const marginPx = Math.round(info.margin * dpr);
      const prev = zoomRef.current;
      zoomRef.current = next;
      setZoom(next);
      const w = Math.round(info.width * next) + marginPx * 2;
      const h = Math.round(info.height * next) + marginPx * 2;
      const pos = await win.outerPosition();
      let x = pos.x;
      let y = pos.y;
      if (anchor) {
        // 锚点相对图片的比例在缩放前后保持不变
        const ax = (anchor.x - info.margin) * dpr;
        const ay = (anchor.y - info.margin) * dpr;
        x = Math.round(pos.x + ax - (ax * next) / prev);
        y = Math.round(pos.y + ay - (ay * next) / prev);
      }
      await win.setSize(new PhysicalSize(w, h));
      await win.setPosition(new PhysicalPosition(x, y));
    },
    [info, dpr],
  );

  const close = useCallback(() => {
    setClosing(true);
    window.setTimeout(() => void pin.close(LABEL), 120);
  }, []);

  const toggleCollapse = useCallback(async () => {
    if (!info) return;
    const win = getCurrentWindow();
    const marginPx = Math.round(info.margin * dpr);
    if (collapsed) {
      setCollapsed(false);
      await win.setSize(new PhysicalSize(Math.round(info.width * zoomRef.current) + marginPx * 2, Math.round(info.height * zoomRef.current) + marginPx * 2));
    } else {
      setCollapsed(true);
      const s = Math.round(DOT * dpr) + marginPx * 2;
      await win.setSize(new PhysicalSize(s, s));
    }
  }, [collapsed, info, dpr]);

  // 识别二维码 / 条码：结果再弹一个菜单，点一项复制或打开
  const scanCodes = useCallback(async () => {
    const codes = await pin.scanCodes(LABEL);
    if (!codes.length) {
      notify.info(t('capture.noCode'));
      return;
    }
    const items = [];
    for (const c of codes) {
      const short = c.text.length > 42 ? `${c.text.slice(0, 42)}…` : c.text;
      items.push(
        await MenuItem.new({
          text: `${t('pin.copyCode')}  ${short}`,
          action: () =>
            void capture
              .writeText(c.text)
              .then(() => notify.success(t('capture.codeCopied')))
              .catch(notify.error),
        }),
      );
      if (/^https?:\/\/\S+$/i.test(c.text.trim())) {
        items.push(await MenuItem.new({ text: `${t('capture.openLink')}  ${short}`, action: () => void system.openUrl(c.text.trim()).catch(notify.error) }));
      }
    }
    await (await Menu.new({ items })).popup();
  }, [t]);

  // 旋转 / 翻转：Rust 那边改图，这边按新尺寸以原来的中心为准调窗口
  const transform = useCallback(
    async (op: 'rotateCw' | 'rotateCcw' | 'flipH' | 'flipV') => {
      if (!info) return;
      const next = await pin.transform(LABEL, op);
      const win = getCurrentWindow();
      const marginPx = Math.round(next.margin * dpr);
      const pos = await win.outerPosition();
      const size = await win.outerSize();
      const w = Math.round(next.width * zoomRef.current) + marginPx * 2;
      const h = Math.round(next.height * zoomRef.current) + marginPx * 2;
      setInfo(next);
      setImageVersion((v) => v + 1);
      // 识别过的字位置不对了，鼠标再移上来时重新识别
      setTexts(null);
      textAsked.current = false;
      await win.setSize(new PhysicalSize(w, h));
      await win.setPosition(new PhysicalPosition(Math.round(pos.x + (size.width - w) / 2), Math.round(pos.y + (size.height - h) / 2)));
    },
    [info, dpr],
  );

  const showMenu = useCallback(async () => {
    const win = getCurrentWindow();
    const sep = () => PredefinedMenuItem.new({ item: 'Separator' });
    const menu = await Menu.new({
      items: [
        ...(selectedText(texts) ? [await MenuItem.new({ text: t('pin.copyText'), action: () => void copySelectedText().catch(notify.error) })] : []),
        await MenuItem.new({ text: t('pin.copy'), action: () => void pin.copy(LABEL).catch(notify.error) }),
        await MenuItem.new({ text: t('pin.saveAs'), action: () => void pin.save(LABEL).catch(notify.error) }),
        await MenuItem.new({ text: t('pin.ocr'), action: () => void pin.ocr(LABEL, false).catch(notify.error) }),
        await MenuItem.new({ text: t('pin.translate'), action: () => void pin.ocr(LABEL, true).catch(notify.error) }),
        await MenuItem.new({ text: t('pin.scanCode'), action: () => void scanCodes().catch(notify.error) }),
        await MenuItem.new({ text: t('pin.edit'), action: () => void pin.edit(LABEL).catch(notify.error) }),
        await Submenu.new({
          text: t('pin.transform'),
          items: [
            await MenuItem.new({ text: t('pin.rotateCw'), action: () => void transform('rotateCw').catch(notify.error) }),
            await MenuItem.new({ text: t('pin.rotateCcw'), action: () => void transform('rotateCcw').catch(notify.error) }),
            await MenuItem.new({ text: t('pin.flipH'), action: () => void transform('flipH').catch(notify.error) }),
            await MenuItem.new({ text: t('pin.flipV'), action: () => void transform('flipV').catch(notify.error) }),
          ],
        }),
        await sep(),
        await MenuItem.new({ text: t('pin.originalSize'), action: () => void applyZoom(1) }),
        await MenuItem.new({ text: t('pin.fullOpacity'), action: () => setOpacity(1) }),
        // 鼠标穿透：点到贴图上等于点到它下面的窗口；从托盘菜单"取消贴图的鼠标穿透"恢复
        await MenuItem.new({
          text: t('pin.clickThrough'),
          action: () => {
            void win.setIgnoreCursorEvents(true);
            notify.info(t('pin.clickThroughHint'));
          },
        }),
        await CheckMenuItem.new({
          text: t('pin.alwaysOnTop'),
          checked: onTop,
          action: () => {
            const next = !onTop;
            setOnTop(next);
            void win.setAlwaysOnTop(next);
          },
        }),
        await sep(),
        await MenuItem.new({ text: t('pin.close'), action: close }),
        await MenuItem.new({ text: t('pin.closeAll'), action: () => void pin.closeAll() }),
      ],
    });
    await menu.popup();
  }, [t, applyZoom, onTop, close, copySelectedText, texts, scanCodes, transform]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close();
      // 选了字就复制字，没选就复制整张图
      else if (modKey(e) && e.key.toLowerCase() === 'c') {
        void copySelectedText()
          .then((copied) => (copied ? undefined : pin.copy(LABEL)))
          .catch(notify.error);
      }
      else if (modKey(e) && e.key.toLowerCase() === 's') void pin.save(LABEL).catch(notify.error);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [close, copySelectedText]);

  if (!info) return null;
  const pixelated = zoom >= 2;
  return (
    <div
      className="pin-root"
      data-closing={closing || undefined}
      style={{ padding: info.margin }}
      onWheel={(e) => {
        if (collapsed) return;
        if (modKey(e)) {
          setOpacity((o) => Math.min(1, Math.max(0.2, Math.round((o + (e.deltaY < 0 ? 0.1 : -0.1)) * 10) / 10)));
        } else {
          const next = Math.min(5, Math.max(0.2, Math.round((zoomRef.current + (e.deltaY < 0 ? 0.1 : -0.1)) * 10) / 10));
          if (next !== zoomRef.current) void applyZoom(next, { x: e.clientX, y: e.clientY });
        }
      }}
      onMouseDown={(e) => {
        if (e.button === 1) {
          e.preventDefault();
          close();
        } else if (e.button === 0 && e.detail === 1) {
          window.getSelection()?.removeAllRanges();
          void getCurrentWindow().startDragging();
        }
      }}
      onDoubleClick={() => void toggleCollapse()}
      onContextMenu={(e) => {
        e.preventDefault();
        void showMenu();
      }}
    >
      {collapsed ? (
        <div className="pin-dot" style={{ width: DOT, height: DOT }}>
          <img src={`${shotUrl(info.imageId)}?v=${imageVersion}`} alt="" draggable={false} />
        </div>
      ) : (
        <div className="pin-frame" style={{ opacity }} onPointerEnter={loadText}>
          <img
            src={`${shotUrl(info.imageId)}?v=${imageVersion}`}
            alt=""
            draggable={false}
            // 窗口是藏着建的，图解码好了才让 Rust 显示（之后再触发也没关系，只生效一次）。
            // 不能等 requestAnimationFrame：macOS 上藏着的网页不跑动画帧，回调永远不来，每次都要等满超时 1.5 秒
            onLoad={(e) => void e.currentTarget.decode().catch(() => undefined).then(() => pin.ready(LABEL))}
            onError={() => void pin.ready(LABEL)}
            style={{ width: (info.width * zoom) / dpr, height: (info.height * zoom) / dpr, imageRendering: pixelated ? 'pixelated' : 'auto' }}
          />
          {texts && texts.length > 0 && (
            <div className="pin-text" aria-hidden={false}>
              {texts.map((b, i) => {
                const k = zoom / dpr;
                return (
                  <span
                    key={i}
                    data-i={i}
                    style={{
                      left: b.x * k,
                      top: b.y * k,
                      height: b.height * k,
                      fontSize: b.height * k * TEXT_EM,
                      lineHeight: `${b.height * k}px`,
                      transform: `scaleX(${fits[i] ?? 1})`,
                    }}
                    // 在字上按下是选字：别让外层把它当成拖动窗口；双击选词，不收成小圆点
                    onMouseDown={(e) => e.button === 0 && e.stopPropagation()}
                    onDoubleClick={(e) => e.stopPropagation()}
                  >
                    {b.text}
                  </span>
                );
              })}
            </div>
          )}
          {zoom !== 1 && <span className="pin-zoom cn-glass-thin cn-numeric">{Math.round(zoom * 100)}%</span>}
        </div>
      )}
      <style>{`
        html[data-view='pin'], html[data-view='pin'] body { background: transparent !important; }
        /* 不做出现动画：贴图要原地、原样接替截图选区，一缩一放就成了"跳一下" */
        .pin-root { position: fixed; inset: 0; }
        .pin-root[data-closing] { animation: pin-out 120ms var(--cn-ease-in) forwards; }
        .pin-frame { position: relative; line-height: 0; border-radius: 2px; box-shadow: 0 4px 12px rgba(0,0,0,0.28), 0 0 0 0.5px rgba(0,0,0,0.25); transition: opacity 120ms; }
        .pin-frame img { display: block; }
        .pin-text { position: absolute; inset: 0; pointer-events: none; line-height: 1; }
        .pin-text span { position: absolute; white-space: pre; transform-origin: 0 0; color: transparent; font-family: ${TEXT_FONT}; cursor: text; pointer-events: auto; user-select: text; -webkit-user-select: text; }
        .pin-text span::selection { background: rgba(10, 132, 255, 0.32); color: transparent; }
        .pin-frame:hover { box-shadow: 0 4px 12px rgba(0,0,0,0.28), 0 0 0 1px var(--cn-accent); }
        .pin-zoom { position: absolute; right: 6px; bottom: 6px; padding: 2px 6px; border-radius: 4px; font: 500 11px/14px var(--cn-font-sans); color: var(--cn-label); line-height: 14px; pointer-events: none; }
        .pin-dot { border-radius: 50%; overflow: hidden; box-shadow: 0 2px 8px rgba(0,0,0,0.35), 0 0 0 2px #fff; }
        .pin-dot img { width: 100%; height: 100%; object-fit: cover; }
        @keyframes pin-out { to { opacity: 0; transform: scale(0.94); } }
      `}</style>
    </div>
  );
}
