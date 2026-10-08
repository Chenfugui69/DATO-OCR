// 贴图窗口（规格 02 §6，参照 Snipaste）：
// 左键拖动 · 滚轮缩放（以鼠标为锚点，20%–500%）· Ctrl+滚轮调不透明度（20%–100%）·
// 双击收成小圆点 / 恢复 · 右键菜单 · Esc 或中键关闭。

import { CheckMenuItem, Menu, MenuItem, PredefinedMenuItem } from '@tauri-apps/api/menu';
import { PhysicalPosition, PhysicalSize } from '@tauri-apps/api/dpi';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { pin } from '@/lib/ipc';
import { modKey, shotUrl, windowLabel } from '@/lib/platform';
import type { PinInfo } from '@/lib/types';
import { notify } from '@/ui/overlays';

const LABEL = windowLabel();
const DOT = 24;

export default function PinView() {
  const { t } = useTranslation();
  const [info, setInfo] = useState<PinInfo | null>(null);
  const [zoom, setZoom] = useState(1);
  const [opacity, setOpacity] = useState(1);
  const [collapsed, setCollapsed] = useState(false);
  const [onTop, setOnTop] = useState(true);
  const [closing, setClosing] = useState(false);
  const zoomRef = useRef(1);

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

  const showMenu = useCallback(async () => {
    const win = getCurrentWindow();
    const sep = () => PredefinedMenuItem.new({ item: 'Separator' });
    const menu = await Menu.new({
      items: [
        await MenuItem.new({ text: t('pin.copy'), action: () => void pin.copy(LABEL).catch(notify.error) }),
        await MenuItem.new({ text: t('pin.saveAs'), action: () => void pin.save(LABEL).catch(notify.error) }),
        await MenuItem.new({ text: t('pin.ocr'), action: () => void pin.ocr(LABEL, false).catch(notify.error) }),
        await MenuItem.new({ text: t('pin.translate'), action: () => void pin.ocr(LABEL, true).catch(notify.error) }),
        await sep(),
        await MenuItem.new({ text: t('pin.originalSize'), action: () => void applyZoom(1) }),
        await MenuItem.new({ text: t('pin.fullOpacity'), action: () => setOpacity(1) }),
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
  }, [t, applyZoom, onTop, close]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close();
      else if (modKey(e) && e.key.toLowerCase() === 'c') void pin.copy(LABEL).catch(notify.error);
      else if (modKey(e) && e.key.toLowerCase() === 's') void pin.save(LABEL).catch(notify.error);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [close]);

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
          <img src={shotUrl(info.imageId)} alt="" draggable={false} />
        </div>
      ) : (
        <div className="pin-frame" style={{ opacity }}>
          <img
            src={shotUrl(info.imageId)}
            alt=""
            draggable={false}
            // 窗口是藏着建的，图画出来了才让 Rust 显示（之后再触发也没关系，只生效一次）
            onLoad={() => requestAnimationFrame(() => void pin.ready(LABEL))}
            onError={() => void pin.ready(LABEL)}
            style={{ width: (info.width * zoom) / dpr, height: (info.height * zoom) / dpr, imageRendering: pixelated ? 'pixelated' : 'auto' }}
          />
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
        .pin-frame:hover { box-shadow: 0 4px 12px rgba(0,0,0,0.28), 0 0 0 1px var(--cn-accent); }
        .pin-zoom { position: absolute; right: 6px; bottom: 6px; padding: 2px 6px; border-radius: 4px; font: 500 11px/14px var(--cn-font-sans); color: var(--cn-label); line-height: 14px; pointer-events: none; }
        .pin-dot { border-radius: 50%; overflow: hidden; box-shadow: 0 2px 8px rgba(0,0,0,0.35), 0 0 0 2px #fff; }
        .pin-dot img { width: 100%; height: 100%; object-fit: cover; }
        @keyframes pin-out { to { opacity: 0; transform: scale(0.94); } }
      `}</style>
    </div>
  );
}
