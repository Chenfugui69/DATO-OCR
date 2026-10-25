// 设置页：划词按钮的样子预览。一块示例画面（正文、蓝色链接、彩色色块，最能看出彩边和模糊），
// 上面一颗和真的一样的玻璃珠（同一个组件、同一套折射），拖到哪就折射哪块；调滑块马上看到效果。
// 可以换浅色 / 深色的示例画面。

import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { GlassTuning, SolidDot } from '@/lib/types';
import { Segmented } from '@/ui/controls';
import { GlassDot, GlassDotStyles, type DotStyle } from '@/views/translate/SelectionButton';

const W = 280;
const H = 132;
/** 圆点四周留的边（和 Rust 的 BUTTON_INSET 一致） */
const INSET = 12;

/** 画示例画面（按设备像素画，折射取样才清楚） */
function drawSample(canvas: HTMLCanvasElement, dark: boolean) {
  const dpr = window.devicePixelRatio || 1;
  canvas.width = W * dpr;
  canvas.height = H * dpr;
  const ctx = canvas.getContext('2d');
  if (!ctx) return;
  ctx.scale(dpr, dpr);
  ctx.fillStyle = dark ? '#1e1e22' : '#ffffff';
  ctx.fillRect(0, 0, W, H);
  // 色块：彩边在色块的边上最明显
  const blocks = ['#ff6b4a', '#ffc53d', '#3ccf7a', '#3b82f6', '#a855f7'];
  blocks.forEach((c, i) => {
    ctx.fillStyle = c;
    ctx.fillRect(W - 92 + (i % 3) * 28, 10 + Math.floor(i / 3) * 28, 24, 24);
  });
  ctx.font = '13px -apple-system, "PingFang SC", "Microsoft YaHei", sans-serif';
  const lines = [
    'The quick brown fox jumps over',
    '选中文字后旁边会出现这颗小圆点',
    'Liquid glass bends the edges,',
    '拖动圆点看它压在不同内容上的样子',
    'https://example.com/translate',
  ];
  lines.forEach((l, i) => {
    ctx.fillStyle = i === 4 ? (dark ? '#6aa8ff' : '#1a5fd6') : dark ? '#e8e8ec' : '#1d1d1f';
    ctx.fillText(l, 12, 24 + i * 22);
  });
}

export function GlassPreview({ style, tuning, solid, size }: { style: DotStyle; tuning: GlassTuning; solid?: SolidDot; size: number }) {
  const { t } = useTranslation();
  const [dark, setDark] = useState(false);
  const sample = useRef<HTMLCanvasElement>(null);
  const [pos, setPos] = useState({ x: 150, y: 56 });
  const [backdrop, setBackdrop] = useState<{ url: string; dark: boolean } | null>(null);
  const box = size + INSET * 2;

  useEffect(() => {
    if (sample.current) drawSample(sample.current, dark);
  }, [dark]);

  // 圆点那一圈底下的画面：从示例画布上裁下来，当成"后端抓到的那块屏幕"
  useEffect(() => {
    const src = sample.current;
    if (!src) return;
    const raf = requestAnimationFrame(() => {
      const dpr = window.devicePixelRatio || 1;
      const out = document.createElement('canvas');
      out.width = Math.round(box * dpr);
      out.height = Math.round(box * dpr);
      const ctx = out.getContext('2d', { willReadFrequently: true });
      if (!ctx) return;
      ctx.drawImage(src, (pos.x - box / 2) * dpr, (pos.y - box / 2) * dpr, box * dpr, box * dpr, 0, 0, out.width, out.height);
      const px = ctx.getImageData(0, 0, out.width, out.height).data;
      let sum = 0;
      for (let i = 0; i < px.length; i += 16) sum += 0.2126 * px[i]! + 0.7152 * px[i + 1]! + 0.0722 * px[i + 2]!;
      setBackdrop({ url: out.toDataURL('image/png'), dark: sum / (px.length / 16) < 128 });
    });
    return () => cancelAnimationFrame(raf);
  }, [pos, box, dark]);

  const drag = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const rect = e.currentTarget.parentElement!.getBoundingClientRect();
    const place = (cx: number, cy: number) =>
      setPos({
        x: Math.min(W - box / 2, Math.max(box / 2, cx - rect.left)),
        y: Math.min(H - box / 2, Math.max(box / 2, cy - rect.top)),
      });
    place(e.clientX, e.clientY);
    const move = (ev: PointerEvent) => place(ev.clientX, ev.clientY);
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  const dot = useMemo(
    () => (
      <GlassDot style={style} tuning={tuning} solid={solid} size={size} backdrop={style === 'solid' ? null : (backdrop?.url ?? null)} dark={backdrop?.dark ?? null} />
    ),
    [style, tuning, solid, size, backdrop],
  );

  return (
    <div className="glass-preview">
      <GlassDotStyles />
      <div className="glass-preview__stage" style={{ width: W, height: H }}>
        <canvas ref={sample} className="glass-preview__sample" style={{ width: W, height: H }} />
        <div
          className="glass-preview__dot"
          style={{ width: box, height: box, transform: `translate(${pos.x - box / 2}px, ${pos.y - box / 2}px)` }}
          title={t('settings.selection.previewDrag')}
          onPointerDown={drag}
        >
          {dot}
        </div>
      </div>
      <Segmented
        value={dark ? 'dark' : 'light'}
        options={[
          { value: 'light', label: t('settings.selection.previewLight') },
          { value: 'dark', label: t('settings.selection.previewDark') },
        ]}
        onChange={(v) => setDark(v === 'dark')}
      />
    </div>
  );
}
