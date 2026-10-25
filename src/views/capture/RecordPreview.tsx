// 录制准备 / 录制中的几样东西：
// - 效果预览：准备时（还没开始录）就在选区里按设置画出鼠标高亮、轨迹、点击涟漪、按着的鼠标键、滚动箭头和
//   动图的进度条 —— 用户选了什么效果当场就看得到，不用录了才知道。录的时候这些由 Rust 画进每一帧（屏幕上不画）
// - 演示工具：录制工具条左边一组画图工具（箭头、矩形、画笔……），准备时、录的时候都能画，选区里的会录进去，
//   画出选区的部分淡一点（录不进去）。选着工具时整块屏都接鼠标（哪里都能画），右键 / Esc / 再点一下工具放下；
//   没选工具时鼠标照常操作下面的程序
// - 录的时候选区里有标注、开着按键显示：遮罩要能被录进去（没有的时候不能，全屏录制时工具条压在选区里）

import clsx from 'clsx';
import { Eraser, Undo2 } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { gif } from '@/lib/ipc';
import { useSettings } from '@/lib/settings';
import type { AnnotationEngine } from '@/views/annotate/engine';
import type { Tool } from '@/views/annotate/model';
import { useEngineVersion } from '@/views/annotate/TextEditor';
import { TOOL_ICONS } from '@/views/annotate/Toolbar';
import { Tooltip } from '@/ui/overlays';

import type { Rect } from './geometry';
import { set, useOverlay } from './store';

const css = (r: Rect, s: number) => ({ x: r.x / s, y: r.y / s, width: r.width / s, height: r.height / s });
/** 和 Rust 那边（record_fx.rs）同样的时长 */
const RIPPLE_MS = 450;
const SCROLL_MS = 450;

// ───────────────────────── 效果预览 ─────────────────────────

/** 准备录制时，选区里按设置画鼠标的效果（和录出来的一样） */
export function EffectsPreview({ rect, s }: { rect: Rect; s: number }) {
  const rec = useSettings()?.record;
  const canvas = useRef<HTMLCanvasElement>(null);
  const sel = css(rect, s);
  const state = useRef({
    cursor: null as { x: number; y: number } | null,
    points: [] as { t: number; x: number; y: number }[],
    ripples: [] as { t: number; x: number; y: number }[],
    buttons: 0,
    scrolled: null as { t: number; dy: number } | null,
  });
  const frame = useRef(0);
  const selRef = useRef(sel);
  selRef.current = sel;
  const recRef = useRef(rec);
  recRef.current = rec;
  const sRef = useRef(s);
  sRef.current = s;
  const kick = useRef(() => {});

  // 鼠标由 Rust 转过来（准备时遮罩鼠标穿透，页面自己收不到）：位置是本屏的物理坐标
  useEvent('record-prep-pointer', (p) => {
    const st = state.current;
    const b = selRef.current;
    const k = sRef.current;
    const at = p.at ? { x: p.at[0] / k - b.x, y: p.at[1] / k - b.y } : null;
    const inside = at && at.x >= 0 && at.y >= 0 && at.x < b.width && at.y < b.height ? at : null;
    const now = performance.now();
    const last = st.points[st.points.length - 1];
    if (inside && (!last || last.x !== inside.x || last.y !== inside.y)) st.points.push({ t: now, ...inside });
    // 新按下的键：点一下的涟漪
    if (inside && p.buttons & ~st.buttons) st.ripples.push({ t: now, ...inside });
    if (inside && p.scroll) st.scrolled = { t: now, dy: p.scroll };
    st.cursor = inside;
    st.buttons = p.buttons;
    kick.current();
  });

  useEffect(() => {
    const draw = () => {
      frame.current = 0;
      const el = canvas.current;
      const r = recRef.current;
      const box = selRef.current;
      if (!el || !r) return;
      const dpr = window.devicePixelRatio || 1;
      if (el.width !== Math.round(box.width * dpr) || el.height !== Math.round(box.height * dpr)) {
        el.width = Math.round(box.width * dpr);
        el.height = Math.round(box.height * dpr);
      }
      const ctx = el.getContext('2d');
      if (!ctx) return;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, box.width, box.height);
      const now = performance.now();
      const st = state.current;
      const trailMs = Math.max(50, r.effects.trailMs);
      st.points = st.points.filter((p) => now - p.t < trailMs);
      st.ripples = st.ripples.filter((p) => now - p.t < RIPPLE_MS);
      const c = st.cursor;
      // 高亮
      if (r.mouseHighlight && c) {
        ctx.fillStyle = r.effects.highlightColor;
        ctx.beginPath();
        ctx.arc(c.x, c.y, r.effects.highlightRadius, 0, Math.PI * 2);
        ctx.fill();
      }
      // 轨迹：越早的越淡
      if (r.trailColor && st.points.length > 1) {
        ctx.lineWidth = 4;
        ctx.lineCap = 'round';
        ctx.strokeStyle = r.trailColor;
        for (let i = 1; i < st.points.length; i += 1) {
          const a = st.points[i - 1]!;
          const b = st.points[i]!;
          ctx.globalAlpha = Math.max(0, 1 - (now - b.t) / trailMs);
          ctx.beginPath();
          ctx.moveTo(a.x, a.y);
          ctx.lineTo(b.x, b.y);
          ctx.stroke();
        }
        ctx.globalAlpha = 1;
      }
      // 涟漪
      if (r.clickColor) {
        ctx.lineWidth = 3;
        ctx.strokeStyle = r.clickColor;
        for (const p of st.ripples) {
          const k = (now - p.t) / RIPPLE_MS;
          ctx.globalAlpha = 1 - k;
          ctx.beginPath();
          ctx.arc(p.x, p.y, 6 + 26 * Math.sqrt(k), 0, Math.PI * 2);
          ctx.stroke();
        }
        ctx.globalAlpha = 1;
      }
      const accent = r.clickColor || '#1677ff';
      // 按着的鼠标键：指针右下一个小鼠标，按着的那半边上色
      if (r.mouseButtons && c && st.buttons) {
        const cx = c.x + 22;
        const cy = c.y + 26;
        ctx.beginPath();
        ctx.roundRect(cx - 7, cy - 10, 14, 20, 6);
        ctx.fillStyle = 'rgba(255,255,255,0.92)';
        ctx.fill();
        ctx.lineWidth = 1.2;
        ctx.strokeStyle = 'rgba(30,30,30,0.9)';
        ctx.stroke();
        ctx.fillStyle = accent;
        if (st.buttons & 1) ctx.fillRect(cx - 6, cy - 9, 5.5, 8);
        if (st.buttons & 2) ctx.fillRect(cx + 0.5, cy - 9, 5.5, 8);
        if (st.buttons & 4) ctx.fillRect(cx - 1.5, cy - 9, 3, 8);
      }
      // 滚动：两道人字形箭头
      const sc = st.scrolled;
      if (r.mouseScroll && c && sc && now - sc.t < SCROLL_MS) {
        ctx.globalAlpha = 1 - (now - sc.t) / SCROLL_MS;
        ctx.strokeStyle = accent;
        ctx.lineWidth = 2.5;
        ctx.lineCap = 'round';
        const dir = sc.dy > 0 ? 1 : -1;
        for (const k of [0, 7]) {
          const tip = c.y + dir * (6 + k);
          const back = tip - dir * 6;
          ctx.beginPath();
          ctx.moveTo(c.x + 24 - 6, back);
          ctx.lineTo(c.x + 24, tip);
          ctx.lineTo(c.x + 24 + 6, back);
          ctx.stroke();
        }
        ctx.globalAlpha = 1;
      }
      // 还在动就接着画
      if (st.ripples.length || st.points.length > 1 || (sc && now - sc.t < SCROLL_MS)) frame.current = requestAnimationFrame(draw);
    };
    kick.current = () => {
      if (!frame.current) frame.current = requestAnimationFrame(draw);
    };
    kick.current();
    return () => {
      kick.current = () => {};
      cancelAnimationFrame(frame.current);
    };
  }, []);

  if (!rec) return null;
  const anim = rec.format !== 'mp4';
  return (
    <>
      <canvas ref={canvas} className="rec-preview" style={{ left: sel.x, top: sel.y, width: sel.width, height: sel.height }} />
      {anim && rec.progress !== 'off' && <ProgressPreview sel={sel} s={s} kind={rec.progress} color={rec.effects.progressColor} />}
    </>
  );
}

/** 动图底下的进度条 / 播放时间（准备时停在三成的位置给人看样子） */
function ProgressPreview({ sel, s, kind, color }: { sel: { x: number; y: number; width: number; height: number }; s: number; kind: 'bar' | 'time'; color: string }) {
  if (kind === 'bar') {
    // 和录出来的一样粗（Rust 的 draw_decor：画面高的 1/120，3 到 8 个像素），屏幕上至少 3px 看得清
    const h = Math.max(3, Math.min(8, Math.max(3, (sel.height * s) / 120)) / s);
    return (
      <div className="rec-progress-preview" style={{ left: sel.x, top: sel.y + sel.height - h, width: sel.width, height: h }}>
        <i style={{ width: '30%', background: color }} />
      </div>
    );
  }
  return (
    <span className="rec-time-preview cn-numeric" style={{ right: `calc(100% - ${sel.x + sel.width - 8}px)`, top: sel.y + sel.height - 26 }}>
      0:03 / 0:10
    </span>
  );
}

// ───────────────────────── 演示工具 ─────────────────────────

/** 录制时能用来演示的画图工具 */
const DEMO_TOOLS: Tool[] = ['arrow', 'rect', 'ellipse', 'pen', 'highlight', 'counter'];

export function DemoTools({ engine }: { engine: AnnotationEngine }) {
  const { t } = useTranslation();
  useEngineVersion(engine);
  const tool = useOverlay((x) => x.tool);
  return (
    <>
      {DEMO_TOOLS.map((id) => {
        const I = TOOL_ICONS[id];
        return (
          <Tooltip key={id} content={`${t(`tools.${id}`)} · ${t('record.toolHint')}`} side="top">
            <button
              type="button"
              className={clsx('an-btn', tool === id && 'an-btn--active')}
              aria-label={t(`tools.${id}`)}
              aria-pressed={tool === id}
              onClick={() => set({ tool: tool === id ? null : id, cursorStyle: 'tool' })}
            >
              <I size={18} strokeWidth={1.5} absoluteStrokeWidth />
            </button>
          </Tooltip>
        );
      })}
      <Tooltip content={t('tools.undo')} side="top">
        <button type="button" className="an-btn" aria-label={t('tools.undo')} disabled={!engine.canUndo} onClick={() => engine.undo()}>
          <Undo2 size={18} strokeWidth={1.5} />
        </button>
      </Tooltip>
      <Tooltip content={t('record.clearDrawings')} side="top">
        <button type="button" className="an-btn" aria-label={t('record.clearDrawings')} disabled={engine.annotations.length === 0} onClick={() => engine.reset()}>
          <Eraser size={18} strokeWidth={1.5} />
        </button>
      </Tooltip>
      <span className="an-sep" />
    </>
  );
}

/**
 * 选着画图工具：整块屏报给 Rust 当成能点的区域（遮罩接鼠标，哪里都能画，出了选区的部分画淡一点）；
 * 没选工具就不报（鼠标穿透，照常操作底下的程序）
 */
export function LiveDrawRegion({
  s,
  viewport,
  report,
}: {
  s: number;
  viewport: { width: number; height: number };
  report: (id: string, r: { x: number; y: number; width: number; height: number } | null) => void;
}) {
  const tool = useOverlay((x) => x.tool);
  useLayoutEffect(() => {
    report('draw', tool ? { x: 0, y: 0, width: viewport.width, height: viewport.height } : null);
    return () => report('draw', null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tool, viewport.width, viewport.height, s]);
  return null;
}

/** 录制中遮罩能不能被录进去：选区里有标注、选着画图工具、开着按键显示时要能 */
export function LiveCapturable({ engine }: { engine: AnnotationEngine }) {
  useEngineVersion(engine);
  const tool = useOverlay((x) => x.tool);
  const keys = !!useSettings()?.record.keystrokes;
  const on = keys || !!tool || engine.annotations.length > 0;
  const last = useRef<boolean | null>(null);
  useEffect(() => {
    if (last.current === on) return;
    last.current = on;
    void gif.setCapturable(on).catch(() => {});
  }, [on]);
  return null;
}
