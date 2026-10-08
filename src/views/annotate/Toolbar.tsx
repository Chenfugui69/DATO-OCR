// 截图工具条（规格 02 §3.6–3.7）。截图遮罩和图片编辑器共用。

import clsx from 'clsx';
import {
  ArrowUpRight,
  Bold,
  Brush,
  Check,
  Circle,
  Download,
  Grid2x2,
  Languages,
  Pencil,
  Pin,
  ScanText,
  Sparkles,
  Square,
  SquareDashed,
  Type,
  Undo2,
  X,
  type LucideIcon,
} from 'lucide-react';
import { forwardRef, useLayoutEffect, useState, type ReactNode, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';

import { Tooltip } from '@/ui/overlays';

import { BRUSH_SIZES, FONT_SIZES, LINE_WIDTHS, PEN_WIDTHS, PRESET_COLORS, type Tool, type ToolOptions } from './model';

export const TOOL_ICONS: Record<Tool, LucideIcon> = {
  rect: Square,
  ellipse: Circle,
  arrow: ArrowUpRight,
  pen: Pencil,
  mosaic: Grid2x2,
  text: Type,
};

export const TOOL_KEYS: Record<Tool, string> = { rect: 'R', ellipse: 'O', arrow: 'A', pen: 'P', mosaic: 'M', text: 'T' };
const TOOLS: Tool[] = ['rect', 'ellipse', 'arrow', 'pen', 'mosaic', 'text'];

/** `undo` 放进哪一组，撤销按钮就排在那一组里；哪组都没有就单独排在绘制工具后面 */
export type ActionId = 'ocr' | 'translate' | 'ai' | 'longshot' | 'gif' | 'pin' | 'save' | 'cancel' | 'done' | 'undo';

type IconProps = { size?: number | string; strokeWidth?: number | string; absoluteStrokeWidth?: boolean };
type ActionIcon = React.ComponentType<IconProps>;

// 识字、长截图、GIF 原来三个图标都是"方框里几道线"，挤在一起很容易点错。长截图和 GIF 是自己画的，
// 画法跟 lucide 一致（24 的画布、同样粗细的圆头线条、占位接近正方形），放在一排里不显得是另一套：
// 试过纯文字的"GIF"，没有外框、笔画又是实心的，看着像单独贴上去的

const svgProps = (size: number | string, strokeWidth: number | string) =>
  ({
    width: size,
    height: size,
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    strokeWidth,
    strokeLinecap: 'round',
    strokeLinejoin: 'round',
    'aria-hidden': true,
  }) as const;

/** 长截图：一页内容，旁边一根上下滚动的箭头 */
const LongshotIcon: ActionIcon = ({ size = 18, strokeWidth = 1.5 }) => (
  <svg {...svgProps(size, strokeWidth)}>
    <rect x="3" y="3" width="13" height="18" rx="2" />
    <path d="M20 5v14M18 7l2-2 2 2M18 17l2 2 2-2M6.5 8h6M6.5 12h6M6.5 16h4" />
  </svg>
);

/** GIF：方框里用线条写的 G、I、F */
const GifIcon: ActionIcon = ({ size = 18, strokeWidth = 1.5 }) => (
  <svg {...svgProps(size, strokeWidth)}>
    <rect x="3" y="3" width="18" height="18" rx="3" />
    <path d="M9.6 10.2a2.1 2.1 0 1 0 .4 3.3V12H8.8M12.6 9.6v4.8M15.4 14.4V9.6h2.4M15.4 12.1h1.9" />
  </svg>
);

export const ACTION_ICONS: Record<ActionId, ActionIcon> = {
  ocr: ScanText,
  translate: Languages,
  ai: Sparkles,
  longshot: LongshotIcon,
  gif: GifIcon,
  pin: Pin,
  save: Download,
  cancel: X,
  done: Check,
  undo: Undo2,
};

const ACTION_KEYS: Record<ActionId, string> = {
  undo: 'Ctrl+Z',
  ocr: 'F3',
  translate: 'Ctrl+T',
  ai: '',
  longshot: 'F2',
  gif: 'Ctrl+G',
  pin: 'Ctrl+P',
  save: 'Ctrl+S',
  cancel: 'Esc',
  done: 'Enter',
};

function ToolButton({
  icon: I,
  label,
  shortcut,
  active,
  disabled,
  primary,
  danger,
  toolId,
  onClick,
}: {
  icon: LucideIcon | ActionIcon;
  label: string;
  shortcut?: string;
  active?: boolean;
  disabled?: boolean;
  primary?: boolean;
  danger?: boolean;
  /** 绘制工具按钮：二级工具条按它对齐 */
  toolId?: Tool;
  onClick: () => void;
}) {
  return (
    <Tooltip content={label} shortcut={shortcut} side="top">
      <button
        type="button"
        data-tool={toolId}
        className={clsx('an-btn', active && 'an-btn--active', primary && 'an-btn--primary', danger && 'an-btn--danger')}
        disabled={disabled}
        aria-label={label}
        aria-pressed={active}
        onClick={onClick}
        onPointerDown={(e) => e.stopPropagation()}
      >
        <I size={18} strokeWidth={1.5} absoluteStrokeWidth />
      </button>
    </Tooltip>
  );
}

const Sep = () => <span className="an-sep" />;

export interface ToolbarProps {
  /** 绘制工具的顺序（设置里可调），不给就是默认顺序 */
  tools?: Tool[];
  tool: Tool | null;
  onTool: (t: Tool | null) => void;
  canUndo: boolean;
  onUndo: () => void;
  actions: ActionId[][];
  onAction: (a: ActionId) => void;
  disabledTools?: Partial<Record<Tool, boolean>>;
  disabledActions?: Partial<Record<ActionId, boolean>>;
  className?: string;
  style?: React.CSSProperties;
}

export const Toolbar = forwardRef<HTMLDivElement, ToolbarProps>(function Toolbar(
  { tools, tool, onTool, canUndo, onUndo, actions, onAction, disabledTools, disabledActions, className, style },
  ref,
) {
  const { t } = useTranslation();
  return (
    <div ref={ref} className={clsx('an-toolbar cn-glass', className)} style={style} onPointerDown={(e) => e.stopPropagation()}>
      {(tools?.length ? tools : TOOLS).map((id) => (
        <ToolButton
          key={id}
          icon={TOOL_ICONS[id]}
          label={t(`tools.${id}`)}
          shortcut={TOOL_KEYS[id]}
          active={tool === id}
          disabled={disabledTools?.[id]}
          toolId={id}
          onClick={() => onTool(tool === id ? null : id)}
        />
      ))}
      {!actions.some((g) => g.includes('undo')) && (
        <>
          <Sep />
          <ToolButton icon={Undo2} label={t('tools.undo')} shortcut="Ctrl+Z" disabled={!canUndo} onClick={onUndo} />
        </>
      )}
      {actions.map((group, gi) => (
        <span key={gi} style={{ display: 'contents' }}>
          <Sep />
          {group.map((id) =>
            id === 'undo' ? (
              <ToolButton key={id} icon={Undo2} label={t('tools.undo')} shortcut={ACTION_KEYS.undo} disabled={!canUndo} onClick={onUndo} />
            ) : (
              <ToolButton
                key={id}
                icon={ACTION_ICONS[id]}
                label={t(`actions.${id}`)}
                shortcut={ACTION_KEYS[id]}
                primary={id === 'done'}
                danger={id === 'cancel'}
                disabled={disabledActions?.[id]}
                onClick={() => onAction(id)}
              />
            ),
          )}
        </span>
      ))}
    </div>
  );
});

// ───────────────────────── 二级工具条 ─────────────────────────

/**
 * 二级工具条要对准的位置：主工具条上 `tool` 那个按钮的水平中心（相对工具条左边，CSS 像素）。
 * 以前二级条贴着主工具条右边放，点左边的画笔，选项却出在最右边，看不出是谁的。
 */
export function useToolAnchor(barRef: RefObject<HTMLElement | null>, tool: Tool | null): number | null {
  const [x, setX] = useState<number | null>(null);
  useLayoutEffect(() => {
    const btn = tool ? barRef.current?.querySelector<HTMLElement>(`[data-tool="${tool}"]`) : null;
    setX(btn ? btn.offsetLeft + btn.offsetWidth / 2 : null);
  }, [barRef, tool]);
  return x;
}

function SizeDots({ sizes, value, onChange, label }: { sizes: number[]; value: number; onChange: (v: number) => void; label: string }) {
  const max = Math.max(...sizes);
  return (
    <div className="an-group" role="radiogroup" aria-label={label}>
      {sizes.map((s) => (
        <button
          key={s}
          type="button"
          className={clsx('an-size', s === value && 'an-size--active')}
          aria-checked={s === value}
          role="radio"
          onClick={() => onChange(s)}
        >
          <span style={{ width: 4 + (s / max) * 10, height: 4 + (s / max) * 10 }} />
        </button>
      ))}
    </div>
  );
}

function Choice({ active, onClick, children, label }: { active: boolean; onClick: () => void; children: ReactNode; label: string }) {
  return (
    <button type="button" className={clsx('an-choice', active && 'an-choice--active')} aria-pressed={active} aria-label={label} onClick={onClick}>
      {children}
    </button>
  );
}

function Colors({ value, onChange }: { value: string; onChange: (c: string) => void }) {
  const { t } = useTranslation();
  const custom = !PRESET_COLORS.includes(value.toUpperCase());
  return (
    <div className="an-group">
      {PRESET_COLORS.map((c) => (
        <button
          key={c}
          type="button"
          className={clsx('an-color', c === value.toUpperCase() && 'an-color--active')}
          style={{ background: c }}
          aria-label={c}
          onClick={() => onChange(c)}
        />
      ))}
      <label className={clsx('an-color an-color--custom', custom && 'an-color--active')} title={t('tools.customColor')} style={custom ? { background: value } : undefined}>
        <input type="color" value={value} onChange={(e) => onChange(e.target.value.toUpperCase())} />
      </label>
    </div>
  );
}

export const SubToolbar = forwardRef<
  HTMLDivElement,
  { tool: Tool; options: ToolOptions; onChange: (o: ToolOptions) => void; style?: React.CSSProperties; className?: string }
>(function SubToolbar({ tool, options, onChange, style, className }, ref) {
  const { t } = useTranslation();
  const set = <K extends Tool>(k: K, patch: Partial<ToolOptions[K]>) => onChange({ ...options, [k]: { ...options[k], ...patch } });

  let body: ReactNode = null;
  switch (tool) {
    case 'rect':
    case 'ellipse': {
      const o = options[tool];
      body = (
        <>
          <SizeDots label={t('tools.lineWidth')} sizes={LINE_WIDTHS} value={o.lineWidth} onChange={(v) => set(tool, { lineWidth: v })} />
          <Sep />
          <div className="an-group">
            <Choice label={t('tools.outline')} active={!o.filled} onClick={() => set(tool, { filled: false })}>
              {tool === 'rect' ? <Square size={14} strokeWidth={1.5} /> : <Circle size={14} strokeWidth={1.5} />}
            </Choice>
            <Choice label={t('tools.filled')} active={o.filled} onClick={() => set(tool, { filled: true })}>
              {tool === 'rect' ? <Square size={14} fill="currentColor" strokeWidth={1.5} /> : <Circle size={14} fill="currentColor" strokeWidth={1.5} />}
            </Choice>
          </div>
          <Sep />
          <Colors value={o.color} onChange={(c) => set(tool, { color: c })} />
        </>
      );
      break;
    }
    case 'arrow': {
      const o = options.arrow;
      body = (
        <>
          <SizeDots label={t('tools.lineWidth')} sizes={LINE_WIDTHS} value={o.lineWidth} onChange={(v) => set('arrow', { lineWidth: v })} />
          <Sep />
          <div className="an-group">
            <Choice label={t('tools.arrowThin')} active={o.style === 'thin'} onClick={() => set('arrow', { style: 'thin' })}>
              <ArrowUpRight size={14} strokeWidth={1.5} />
            </Choice>
            <Choice label={t('tools.arrowThick')} active={o.style === 'thick'} onClick={() => set('arrow', { style: 'thick' })}>
              <ArrowUpRight size={14} strokeWidth={3} />
            </Choice>
          </div>
          <Sep />
          <Colors value={o.color} onChange={(c) => set('arrow', { color: c })} />
        </>
      );
      break;
    }
    case 'pen': {
      const o = options.pen;
      body = (
        <>
          <SizeDots label={t('tools.lineWidth')} sizes={PEN_WIDTHS} value={o.lineWidth} onChange={(v) => set('pen', { lineWidth: v })} />
          <Sep />
          <Colors value={o.color} onChange={(c) => set('pen', { color: c })} />
        </>
      );
      break;
    }
    case 'mosaic': {
      const o = options.mosaic;
      body = (
        <>
          <div className="an-group">
            <Choice label={t('tools.mosaicBrush')} active={o.shape === 'brush'} onClick={() => set('mosaic', { shape: 'brush' })}>
              <Brush size={14} strokeWidth={1.5} />
            </Choice>
            <Choice label={t('tools.mosaicRect')} active={o.shape === 'rect'} onClick={() => set('mosaic', { shape: 'rect' })}>
              <SquareDashed size={14} strokeWidth={1.5} />
            </Choice>
          </div>
          <Sep />
          <SizeDots
            label={o.shape === 'rect' ? t('tools.cellSize') : t('tools.brushSize')}
            sizes={BRUSH_SIZES}
            value={o.brushSize}
            onChange={(v) => set('mosaic', { brushSize: v })}
          />
          <Sep />
          <div className="an-group">
            <Choice label={t('tools.pixelate')} active={o.mode === 'pixelate'} onClick={() => set('mosaic', { mode: 'pixelate' })}>
              <span className="an-choice__text">{t('tools.pixelate')}</span>
            </Choice>
            <Choice label={t('tools.blur')} active={o.mode === 'blur'} onClick={() => set('mosaic', { mode: 'blur' })}>
              <span className="an-choice__text">{t('tools.blur')}</span>
            </Choice>
          </div>
        </>
      );
      break;
    }
    case 'text': {
      const o = options.text;
      body = (
        <>
          <div className="an-group">
            {FONT_SIZES.map((s, i) => (
              <Choice key={s} label={t('tools.fontSize')} active={o.fontSize === s} onClick={() => set('text', { fontSize: s })}>
                <span className="an-choice__text" style={{ fontSize: 11 + i * 2 }}>
                  A
                </span>
              </Choice>
            ))}
            <Choice label={t('tools.bold')} active={o.bold} onClick={() => set('text', { bold: !o.bold })}>
              <Bold size={14} strokeWidth={2} />
            </Choice>
          </div>
          <Sep />
          <Colors value={o.color} onChange={(c) => set('text', { color: c })} />
        </>
      );
      break;
    }
  }
  return (
    <div ref={ref} className={clsx('an-subtoolbar cn-glass', className)} style={style} onPointerDown={(e) => e.stopPropagation()}>
      {body}
    </div>
  );
});
