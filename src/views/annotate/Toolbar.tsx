// 截图工具条（规格 02 §3.6–3.7）。截图遮罩和图片编辑器共用。

import clsx from 'clsx';
import {
  ArrowUpRight,
  Bold,
  Brush,
  Check,
  Circle,
  Download,
  Eraser,
  Focus,
  Grid2x2,
  Highlighter,
  Languages,
  Pencil,
  Pin,
  Plus,
  Ruler,
  ScanQrCode,
  Shapes,
  Stamp,
  ScanText,
  Sparkles,
  Square,
  SquareDashed,
  Type,
  Undo2,
  Sigma,
  Table2,
  Video,
  ZoomIn,
  X,
  type LucideIcon,
} from 'lucide-react';
import { forwardRef, useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';

import { Tooltip } from '@/ui/overlays';

import {
  BRUSH_SIZES,
  COUNTER_SIZES,
  FONT_SIZES,
  formatCounter,
  HIGHLIGHT_COLORS,
  HIGHLIGHT_WIDTHS,
  LINE_WIDTHS,
  PEN_WIDTHS,
  PRESET_COLORS,
  type ArrowStyle,
  type Dash,
  type MeasureUnit,
  type Numbering,
  type TextStyle,
  type Tool,
  type ToolOptions,
} from './model';
import { deleteTemplate, listTemplates, type AnnotationTemplate } from './templates';

/** `undo` 放进哪一组，撤销按钮就排在那一组里；哪组都没有就单独排在绘制工具后面 */
export type ActionId = 'ocr' | 'translate' | 'qr' | 'table' | 'formula' | 'ai' | 'longshot' | 'gif' | 'video' | 'pin' | 'save' | 'cancel' | 'done' | 'undo';

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

/** 序号：圆圈里一个 1 */
const CounterIcon: ActionIcon = ({ size = 18, strokeWidth = 1.5 }) => (
  <svg {...svgProps(size, strokeWidth)}>
    <circle cx="12" cy="12" r="9" />
    <path d="M10.5 9.2 12.6 8v8M10.4 16h4.4" />
  </svg>
);

export const TOOL_ICONS: Record<Tool, LucideIcon | ActionIcon> = {
  rect: Square,
  ellipse: Circle,
  arrow: ArrowUpRight,
  pen: Pencil,
  highlight: Highlighter,
  counter: CounterIcon,
  mosaic: Grid2x2,
  text: Type,
  measure: Ruler,
  watermark: Stamp,
  template: Shapes,
  eraser: Eraser,
};

export const TOOL_KEYS: Record<Tool, string> = {
  rect: 'R',
  ellipse: 'O',
  arrow: 'A',
  pen: 'P',
  highlight: 'H',
  counter: 'N',
  mosaic: 'M',
  text: 'T',
  measure: 'D',
  watermark: 'W',
  template: 'S',
  eraser: 'E',
};
const TOOLS: Tool[] = ['rect', 'ellipse', 'arrow', 'pen', 'highlight', 'counter', 'mosaic', 'text', 'measure', 'watermark', 'template', 'eraser'];

export const ACTION_ICONS: Record<ActionId, ActionIcon> = {
  ocr: ScanText,
  translate: Languages,
  qr: ScanQrCode,
  table: Table2,
  formula: Sigma,
  ai: Sparkles,
  longshot: LongshotIcon,
  gif: GifIcon,
  video: Video,
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
  qr: '',
  table: '',
  formula: '',
  ai: '',
  longshot: 'F2',
  gif: 'Ctrl+G',
  video: '',
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
  /** 设置里藏起来的按钮 */
  hidden?: string[];
  className?: string;
  style?: React.CSSProperties;
}

export const Toolbar = forwardRef<HTMLDivElement, ToolbarProps>(function Toolbar(
  { tools, tool, onTool, canUndo, onUndo, actions: allActions, onAction, disabledTools, disabledActions, hidden, className, style },
  ref,
) {
  const { t } = useTranslation();
  const shown = (id: string) => !hidden?.includes(id);
  const actions = allActions.map((g) => g.filter(shown)).filter((g) => g.length > 0);
  return (
    <div ref={ref} className={clsx('an-toolbar cn-glass', className)} style={style} onPointerDown={(e) => e.stopPropagation()}>
      {(tools?.length ? tools : TOOLS).filter(shown).map((id) => (
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

/** 不透明度滑块（10%–100%），右边标着百分比 */
function Opacity({ value, onChange, label }: { value: number; onChange: (v: number) => void; label: string }) {
  const percent = Math.round(value * 100);
  return (
    <Tooltip content={label}>
      <label className="an-range">
        <input
          type="range"
          min={10}
          max={100}
          step={5}
          value={percent}
          aria-label={label}
          style={{ '--an-range-fill': `${((percent - 10) / 90) * 100}%` } as React.CSSProperties}
          onChange={(e) => onChange(Number(e.currentTarget.value) / 100)}
        />
        <span className="an-range__value cn-numeric">{percent}%</span>
      </label>
    </Tooltip>
  );
}

function Choice({ active, onClick, children, label }: { active: boolean; onClick: () => void; children: ReactNode; label: string }) {
  return (
    <button type="button" className={clsx('an-choice', active && 'an-choice--active')} aria-pressed={active} aria-label={label} onClick={onClick}>
      {children}
    </button>
  );
}

function Colors({ value, onChange, palette = PRESET_COLORS }: { value: string; onChange: (c: string) => void; palette?: string[] }) {
  const { t } = useTranslation();
  const custom = !palette.includes(value.toUpperCase());
  return (
    <div className="an-group">
      {palette.map((c) => (
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

const MEASURE_WIDTHS = [1, 2, 4];
const WATERMARK_SIZES = [14, 18, 28];
const WATERMARK_ANGLES = [0, -30, -45];
const WATERMARK_COLORS = ['#808080', '#FFFFFF', '#000000', '#FF3B30', '#007AFF'];

/** 水印的选项：开关、文字、字号、角度、透明度、颜色。选了这个工具就默认打开 */
function WatermarkOptions({ o, onChange }: { o: ToolOptions['watermark']; onChange: (patch: Partial<ToolOptions['watermark']>) => void }) {
  const { t } = useTranslation();
  const opened = useRef(false);
  useEffect(() => {
    if (opened.current) return;
    opened.current = true;
    if (!o.enabled) onChange({ enabled: true });
  }, [o.enabled, onChange]);
  return (
    <>
      <Choice label={t('tools.watermarkOn')} active={o.enabled} onClick={() => onChange({ enabled: !o.enabled })}>
        <span className="an-choice__text">{o.enabled ? t('tools.watermarkOn') : t('tools.watermarkOff')}</span>
      </Choice>
      <Sep />
      <input
        className="an-input"
        value={o.text}
        placeholder={t('tools.watermarkPlaceholder')}
        title={t('tools.watermarkTokens')}
        onChange={(e) => onChange({ text: e.currentTarget.value, enabled: true })}
        onKeyDown={(e) => e.stopPropagation()}
      />
      <Sep />
      <SizeDots label={t('tools.fontSize')} sizes={WATERMARK_SIZES} value={o.fontSize} onChange={(v) => onChange({ fontSize: v })} />
      <Sep />
      <div className="an-group">
        {WATERMARK_ANGLES.map((a) => (
          <Choice key={a} label={t('tools.watermarkAngle')} active={o.angle === a} onClick={() => onChange({ angle: a })}>
            <span className="an-choice__text">{a}°</span>
          </Choice>
        ))}
      </div>
      <Sep />
      <Opacity label={t('tools.watermarkOpacity')} value={o.opacity} onChange={(v) => onChange({ opacity: v })} />
      <Sep />
      <Colors value={o.color} palette={WATERMARK_COLORS} onChange={(c) => onChange({ color: c })} />
    </>
  );
}

/** 模板：存过的模板一排（点一个选中，再在图上点一下插入），最后是"存当前标注" */
function TemplateOptions({ selected, onSelect, onSave }: { selected: string | null; onSelect: (id: string | null) => void; onSave?: () => boolean }) {
  const { t } = useTranslation();
  const [list, setList] = useState<AnnotationTemplate[]>(() => listTemplates());
  // 没选的时候默认选最新的一个，打开就能直接点着插
  useEffect(() => {
    if (list.some((x) => x.id === selected)) return;
    const want = list[list.length - 1]?.id ?? null;
    // 只在真的要换时才改：改选项会生成新对象、重渲染，没变也改的话会一直循环下去
    if (want !== selected) onSelect(want);
  }, [list, selected, onSelect]);
  return (
    <>
      {list.length === 0 && <span className="an-hint">{t('tools.templateEmpty')}</span>}
      <div className="an-group">
        {list.map((x, i) => (
          <span key={x.id} className="an-template">
            <Choice label={t('tools.templateItem', { count: x.items.length })} active={x.id === selected} onClick={() => onSelect(x.id)}>
              <span className="an-choice__text">{t('tools.templateName', { n: i + 1 })}</span>
            </Choice>
            <button
              type="button"
              className="an-template__del"
              aria-label={t('common.delete')}
              onClick={() => {
                deleteTemplate(x.id);
                setList(listTemplates());
              }}
            >
              <X size={10} strokeWidth={2} />
            </button>
          </span>
        ))}
      </div>
      {onSave && (
        <>
          <Sep />
          <Choice
            label={t('tools.templateSave')}
            active={false}
            onClick={() => {
              if (onSave()) setList(listTemplates());
            }}
          >
            <span className="an-choice__row">
              <Plus size={13} strokeWidth={1.75} />
              {t('tools.templateSave')}
            </span>
          </Choice>
        </>
      )}
    </>
  );
}

/** 线型示意：一小段实线 / 虚线 / 点线 */
function DashIcon({ dash }: { dash: Dash }) {
  return (
    <svg width="20" height="14" viewBox="0 0 20 14" aria-hidden>
      <line
        x1="2"
        y1="7"
        x2="18"
        y2="7"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeDasharray={dash === 'dashed' ? '5 3.5' : dash === 'dotted' ? '0.1 4' : undefined}
      />
    </svg>
  );
}

function DashChoices({ value, onChange }: { value: Dash; onChange: (d: Dash) => void }) {
  const { t } = useTranslation();
  const labels: Record<Dash, string> = { solid: t('tools.dashSolid'), dashed: t('tools.dashDashed'), dotted: t('tools.dashDotted') };
  return (
    <div className="an-group">
      {(['solid', 'dashed', 'dotted'] as Dash[]).map((d) => (
        <Choice key={d} label={labels[d]} active={value === d} onClick={() => onChange(d)}>
          <DashIcon dash={d} />
        </Choice>
      ))}
    </div>
  );
}

/** 箭头样式示意 */
function ArrowStyleIcon({ style }: { style: ArrowStyle }) {
  if (style === 'thin') return <ArrowUpRight size={14} strokeWidth={1.5} />;
  if (style === 'thick') return <ArrowUpRight size={14} strokeWidth={3} />;
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M3 13 13 3" />
      {style === 'double' && <path d="M8 3h5v5M3 8v5h5" />}
    </svg>
  );
}

export const SubToolbar = forwardRef<
  HTMLDivElement,
  {
    tool: Tool;
    options: ToolOptions;
    onChange: (o: ToolOptions) => void;
    /** 模板工具：把当前的标注存成模板，返回是否存成了 */
    onSaveTemplate?: () => boolean;
    style?: React.CSSProperties;
    className?: string;
  }
>(function SubToolbar({ tool, options, onChange, onSaveTemplate, style, className }, ref) {
  const { t } = useTranslation();
  const set = <K extends Tool>(k: K, patch: Partial<ToolOptions[K]>) => onChange({ ...options, [k]: { ...options[k], ...patch } });

  let body: ReactNode = null;
  switch (tool) {
    case 'rect':
    case 'ellipse': {
      const o = options[tool];
      body = (
        <>
          {/* 实心时线宽用不上，换成填充的不透明度 */}
          {o.filled ? (
            <Opacity label={t('tools.fillOpacity')} value={o.fillOpacity} onChange={(v) => set(tool, { fillOpacity: v })} />
          ) : (
            <SizeDots label={t('tools.lineWidth')} sizes={LINE_WIDTHS} value={o.lineWidth} onChange={(v) => set(tool, { lineWidth: v })} />
          )}
          {!o.filled && (
            <>
              <Sep />
              <DashChoices value={o.dash} onChange={(d) => set(tool, { dash: d })} />
            </>
          )}
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
            {(['thin', 'thick', 'line', 'double'] as ArrowStyle[]).map((st) => (
              <Choice
                key={st}
                label={t(`tools.arrow${st[0]!.toUpperCase()}${st.slice(1)}`)}
                active={o.style === st}
                onClick={() => set('arrow', { style: st })}
              >
                <ArrowStyleIcon style={st} />
              </Choice>
            ))}
          </div>
          {/* 粗箭头是一整块实心形状，没有虚线 */}
          {o.style !== 'thick' && (
            <>
              <Sep />
              <DashChoices value={o.dash} onChange={(d) => set('arrow', { dash: d })} />
            </>
          )}
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
    case 'highlight': {
      const o = options.highlight;
      body = (
        <>
          <div className="an-group">
            <Choice label={t('tools.highlightBrush')} active={o.shape === 'brush'} onClick={() => set('highlight', { shape: 'brush' })}>
              <Highlighter size={14} strokeWidth={1.5} />
            </Choice>
            <Choice label={t('tools.highlightRect')} active={o.shape === 'rect'} onClick={() => set('highlight', { shape: 'rect' })}>
              <Square size={14} fill="currentColor" fillOpacity={0.4} strokeWidth={1.5} />
            </Choice>
            <Choice label={t('tools.spotlight')} active={o.shape === 'spotlight'} onClick={() => set('highlight', { shape: 'spotlight' })}>
              <Focus size={14} strokeWidth={1.5} />
            </Choice>
            <Choice label={t('tools.magnifier')} active={o.shape === 'magnifier'} onClick={() => set('highlight', { shape: 'magnifier' })}>
              <ZoomIn size={14} strokeWidth={1.5} />
            </Choice>
          </div>
          <Sep />
          {o.shape === 'spotlight' ? (
            <Opacity label={t('tools.spotlightDim')} value={o.dim} onChange={(v) => set('highlight', { dim: v })} />
          ) : (
            <>
              {o.shape === 'brush' && (
                <>
                  <SizeDots label={t('tools.lineWidth')} sizes={HIGHLIGHT_WIDTHS} value={o.lineWidth} onChange={(v) => set('highlight', { lineWidth: v })} />
                  <Sep />
                </>
              )}
              {/* 放大镜的颜色是描边，用普通色板；荧光笔用荧光色 */}
              <Colors value={o.color} palette={o.shape === 'magnifier' ? PRESET_COLORS : HIGHLIGHT_COLORS} onChange={(c) => set('highlight', { color: c })} />
            </>
          )}
        </>
      );
      break;
    }
    case 'counter': {
      const o = options.counter;
      const labels: Record<Numbering, string> = {
        arabic: t('tools.numberArabic'),
        chinese: t('tools.numberChinese'),
        letter: t('tools.numberLetter'),
        roman: t('tools.numberRoman'),
      };
      body = (
        <>
          <SizeDots label={t('tools.counterSize')} sizes={COUNTER_SIZES} value={o.size} onChange={(v) => set('counter', { size: v })} />
          <Sep />
          <div className="an-group">
            {(['arabic', 'chinese', 'letter', 'roman'] as Numbering[]).map((n) => (
              <Choice key={n} label={labels[n]} active={o.numbering === n} onClick={() => set('counter', { numbering: n })}>
                <span className="an-choice__text">{formatCounter(n === 'roman' ? 4 : 1, n)}</span>
              </Choice>
            ))}
          </div>
          <Sep />
          <Colors value={o.color} onChange={(c) => set('counter', { color: c })} />
        </>
      );
      break;
    }
    case 'eraser':
      body = <span className="an-hint">{t('tools.eraserHint')}</span>;
      break;
    case 'measure': {
      const o = options.measure;
      body = (
        <>
          <div className="an-group">
            <Choice label={t('tools.measureDistance')} active={o.shape === 'distance'} onClick={() => set('measure', { shape: 'distance' })}>
              <Ruler size={14} strokeWidth={1.5} />
            </Choice>
            <Choice label={t('tools.measureAngle')} active={o.shape === 'angle'} onClick={() => set('measure', { shape: 'angle' })}>
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden>
                <path d="M3 13h10M3 13 11 4M7.5 13a4.5 4.5 0 0 0-1.3-3.2" />
              </svg>
            </Choice>
          </div>
          <Sep />
          <SizeDots label={t('tools.lineWidth')} sizes={MEASURE_WIDTHS} value={o.lineWidth} onChange={(v) => set('measure', { lineWidth: v })} />
          {o.shape === 'distance' && (
            <>
              <Sep />
              <div className="an-group">
                {(['px', 'pt'] as MeasureUnit[]).map((u) => (
                  <Choice key={u} label={t(`tools.unit_${u}`)} active={o.unit === u} onClick={() => set('measure', { unit: u })}>
                    <span className="an-choice__text">{u}</span>
                  </Choice>
                ))}
              </div>
            </>
          )}
          <Sep />
          <Colors value={o.color} onChange={(c) => set('measure', { color: c })} />
          {o.shape === 'angle' && <span className="an-hint">{t('tools.angleHint')}</span>}
        </>
      );
      break;
    }
    case 'watermark':
      body = <WatermarkOptions o={options.watermark} onChange={(patch) => set('watermark', patch)} />;
      break;
    case 'template':
      body = <TemplateOptions selected={options.template.selected} onSelect={(id) => set('template', { selected: id })} onSave={onSaveTemplate} />;
      break;
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
          <div className="an-group">
            {(['plain', 'fill', 'outline'] as TextStyle[]).map((st) => (
              <Choice key={st} label={t(`tools.text${st[0]!.toUpperCase()}${st.slice(1)}`)} active={o.style === st} onClick={() => set('text', { style: st })}>
                <span className={`an-textstyle an-textstyle--${st}`}>
                  <i>A</i>
                </span>
              </Choice>
            ))}
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
