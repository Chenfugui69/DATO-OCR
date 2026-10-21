// 选区面板：点选区左上角的尺寸标签弹出。直接输入坐标 / 宽高、套一个宽高比（拖控制点时也保持）、
// 恢复上一次截图的选区。数值都是这块屏的物理像素，和尺寸标签上显示的一致。

import clsx from 'clsx';
import { History, Lock } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { applyRatio, clampMove, clampRect, type Rect } from './geometry';

/** 宽高比预设（宽 / 高）；null = 自由 */
const RATIOS: { label: string; value: number | null }[] = [
  { label: 'free', value: null },
  { label: '1:1', value: 1 },
  { label: '4:3', value: 4 / 3 },
  { label: '3:2', value: 3 / 2 },
  { label: '16:9', value: 16 / 9 },
  { label: '9:16', value: 9 / 16 },
];

const lastKey = (monitor: number) => `dato.capture.lastSelection.${monitor}`;

/** 记下这次截图的选区（完成截图时调）。浏览器存储不可用就算了，只是少了"上一次选区" */
export function rememberSelection(monitor: number, r: Rect) {
  try {
    localStorage.setItem(lastKey(monitor), JSON.stringify(r));
  } catch {
    /* 存不了就不记 */
  }
}

export function lastSelection(monitor: number): Rect | null {
  try {
    const raw = localStorage.getItem(lastKey(monitor));
    if (!raw) return null;
    const r = JSON.parse(raw) as Rect;
    return [r.x, r.y, r.width, r.height].every(Number.isFinite) && r.width > 0 && r.height > 0 ? r : null;
  } catch {
    return null;
  }
}

type Field = 'x' | 'y' | 'width' | 'height';

export function SelectionPanel({
  selection,
  bounds,
  monitor,
  ratio,
  onChange,
  onRatio,
}: {
  selection: Rect;
  bounds: Rect;
  monitor: number;
  ratio: number | null;
  onChange: (r: Rect) => void;
  onRatio: (r: number | null) => void;
}) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<Record<Field, string>>(() => toDraft(selection));
  const last = lastSelection(monitor);

  // 选区被拖动 / 改大小时，输入框跟着变
  useEffect(() => setDraft(toDraft(selection)), [selection]);

  const commit = (field: Field, text: string) => {
    const v = Math.round(Number(text));
    if (!Number.isFinite(v)) return setDraft(toDraft(selection));
    const next = { ...selection, [field]: v };
    if (field === 'width' || field === 'height') {
      next[field] = Math.max(1, v);
      // 锁着宽高比：改一边，另一边跟着算
      if (ratio) {
        if (field === 'width') next.height = Math.max(1, Math.round(next.width / ratio));
        else next.width = Math.max(1, Math.round(next.height * ratio));
      }
      onChange(clampRect(next, bounds));
    } else onChange(clampMove(next, bounds));
  };

  const field = (f: Field, label: string) => (
    <label className="cap-panel__field">
      <span>{label}</span>
      <input
        type="number"
        inputMode="numeric"
        value={draft[f]}
        onChange={(e) => {
          // 先取出来：setDraft 的回调稍后才跑，那时事件上的 currentTarget 已经清空了
          const v = e.currentTarget.value;
          setDraft((d) => ({ ...d, [f]: v }));
        }}
        onBlur={(e) => commit(f, e.currentTarget.value)}
        onKeyDown={(e) => {
          // 回车只确认这一格，不要冒泡成"完成截图"
          e.stopPropagation();
          if (e.key === 'Enter') commit(f, e.currentTarget.value);
          if (e.key === 'Escape') e.currentTarget.blur();
        }}
      />
    </label>
  );

  return (
    <div className="cap-panel cn-glass" onPointerDown={(e) => e.stopPropagation()} onClick={(e) => e.stopPropagation()}>
      <div className="cap-panel__row">
        {field('x', 'X')}
        {field('y', 'Y')}
        {field('width', t('capture.panel.width'))}
        {field('height', t('capture.panel.height'))}
      </div>
      <div className="cap-panel__row cap-panel__ratios" role="radiogroup" aria-label={t('capture.panel.ratio')}>
        {ratio !== null && <Lock size={12} strokeWidth={1.75} className="cap-panel__lock" aria-hidden />}
        {RATIOS.map((r) => (
          <button
            key={r.label}
            type="button"
            role="radio"
            aria-checked={ratio === r.value}
            className={clsx('cap-panel__chip', ratio === r.value && 'cap-panel__chip--active')}
            onClick={() => {
              onRatio(r.value);
              if (r.value) onChange(applyRatio(selection, r.value, bounds));
            }}
          >
            {r.value === null ? t('capture.panel.free') : r.label}
          </button>
        ))}
      </div>
      <button type="button" className="cap-panel__last" disabled={!last} onClick={() => last && onChange(clampMove(last, bounds))}>
        <History size={13} strokeWidth={1.75} />
        {t('capture.panel.last')}
        {last && (
          <span className="cn-numeric">
            {last.width} × {last.height}
          </span>
        )}
      </button>
    </div>
  );
}

function toDraft(r: Rect): Record<Field, string> {
  return { x: String(r.x), y: String(r.y), width: String(r.width), height: String(r.height) };
}
