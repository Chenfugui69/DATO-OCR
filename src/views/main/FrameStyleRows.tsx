// 设置页：截图选区框样式（颜色 / 粗细 / 线型 / 圆角），普通截图和截图识字各一份。

import { useTranslation } from 'react-i18next';

import type { FrameStyle } from '@/lib/types';
import { ColorPicker } from '@/ui/ColorPicker';
import { Segmented, Slider } from '@/ui/controls';

import { Row } from './settingsParts';

const PRESETS = ['accent', '#FFFFFF', '#FF3B30', '#FF9500', '#FFCC00', '#34C759', '#00C7BE', '#AF52DE', '#FF2D55', '#1C1C1E'];

const cssColor = (c: string) => (c === 'accent' ? 'var(--cn-accent)' : c);

export function FrameStyleRows({ name, value, onChange }: { name: string; value: FrameStyle; onChange: (next: FrameStyle) => void }) {
  const { t } = useTranslation();
  const patch = (p: Partial<FrameStyle>) => onChange({ ...value, ...p });
  const custom = value.color !== 'accent' && !PRESETS.includes(value.color.toUpperCase());
  return (
    <>
      <Row title={t('settings.frame.color', { name })}>
        <div className="frame-swatches">
          {PRESETS.map((c) => (
            <button
              key={c}
              type="button"
              className="frame-swatch"
              data-active={value.color.toUpperCase() === c.toUpperCase() || undefined}
              title={c === 'accent' ? t('settings.frame.accent') : c}
              style={{ background: cssColor(c) }}
              onClick={() => patch({ color: c })}
            />
          ))}
          <ColorPicker value={value.color === 'accent' ? '#0A84FF' : value.color} onChange={(c) => patch({ color: c })} onCommit={(c) => patch({ color: c })}>
            <button
              type="button"
              className="frame-swatch frame-swatch--custom"
              data-active={custom || undefined}
              title={t('settings.frame.custom')}
              style={custom ? { background: value.color } : undefined}
            />
          </ColorPicker>
        </div>
      </Row>
      <Row title={t('settings.frame.width', { name })}>
        <span className="set-row__value cn-numeric">{value.width}px</span>
        <Slider value={value.width} min={1} max={6} step={0.5} onChange={(v) => patch({ width: v })} />
      </Row>
      <Row title={t('settings.frame.style', { name })}>
        <Segmented
          value={value.style}
          options={[
            { value: 'solid', label: t('settings.frame.solid') },
            { value: 'dashed', label: t('settings.frame.dashed') },
            { value: 'dotted', label: t('settings.frame.dotted') },
          ]}
          onChange={(v) => patch({ style: v })}
        />
      </Row>
      <Row title={t('settings.frame.radius', { name })}>
        <span className="set-row__value cn-numeric">{value.radius}px</span>
        <Slider value={value.radius} min={0} max={24} onChange={(v) => patch({ radius: v })} />
        <span
          className="frame-preview"
          aria-hidden
          style={{ outline: `${value.width}px ${value.style} ${cssColor(value.color)}`, borderRadius: value.radius }}
        />
      </Row>
    </>
  );
}
