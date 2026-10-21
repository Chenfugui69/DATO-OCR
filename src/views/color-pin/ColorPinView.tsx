// 颜色贴图：截图取色时按 P，鼠标下的颜色变成屏幕上的一张小卡片（色块 + HEX）。
// 点色块展开，看 HEX / RGB / HSL，每个都能一键复制；拖卡片挪位置，Esc 或 × 关掉。
// 颜色写在窗口标签里（pin-color-rrggbb-xxxx），页面不用再问后端。

import { LogicalSize } from '@tauri-apps/api/dpi';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Check, ChevronUp, Copy, X } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { formatColor } from '@/lib/format';
import { capture } from '@/lib/ipc';
import { windowLabel } from '@/lib/platform';

import './color-pin.css';

/** 窗口大小（逻辑像素，四周各留 12 给阴影）：收起 / 展开 */
const COLLAPSED = { width: 176, height: 60 };
const EXPANDED = { width: 256, height: 172 };

function colorFromLabel(label: string): [number, number, number] {
  const hex = /^pin-color-([0-9a-f]{6})/i.exec(label)?.[1] ?? '000000';
  return [parseInt(hex.slice(0, 2), 16), parseInt(hex.slice(2, 4), 16), parseInt(hex.slice(4, 6), 16)];
}

export default function ColorPinView() {
  const { t } = useTranslation();
  const [rgb] = useState(() => colorFromLabel(windowLabel()));
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const hex = formatColor(rgb, 'hex').toUpperCase();
  const css = `rgb(${rgb[0]}, ${rgb[1]}, ${rgb[2]})`;
  const values = [
    { key: 'HEX', text: hex },
    { key: 'RGB', text: `${rgb[0]}, ${rgb[1]}, ${rgb[2]}` },
    { key: 'HSL', text: formatColor(rgb, 'hsl').replace(/^hsl\((.*)\)$/, '$1') },
  ];

  const close = () => void getCurrentWindow().close();
  const toggle = (next: boolean) => {
    setOpen(next);
    const size = next ? EXPANDED : COLLAPSED;
    void getCurrentWindow().setSize(new LogicalSize(size.width, size.height));
  };
  const copy = (text: string) =>
    void capture.writeText(text).then(() => {
      setCopied(text);
      window.setTimeout(() => setCopied((c) => (c === text ? null : c)), 1200);
    });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  // 按在卡片空白处拖动窗口（按钮、色块除外）
  const drag = (e: React.PointerEvent) => {
    if (e.button !== 0 || (e.target as Element).closest('button')) return;
    void getCurrentWindow().startDragging();
  };

  const copyIcon = (text: string) =>
    copied === text ? <Check size={12} strokeWidth={2} /> : <Copy size={12} strokeWidth={1.75} />;

  return (
    <div className="cpin" onPointerDown={drag}>
      {open ? (
        <div className="cpin__card cpin__card--open cn-glass">
          <div className="cpin__head">
            <span className="cpin__label">{t('pin.color.title')}</span>
            <span className="cpin__swatch" style={{ background: css }} />
            <span className="cpin__spacer" />
            <button type="button" className="cpin__icon" aria-label={t('pin.color.collapse')} title={t('pin.color.collapse')} onClick={() => toggle(false)}>
              <ChevronUp size={13} strokeWidth={1.75} />
            </button>
            <button type="button" className="cpin__icon" aria-label={t('common.close')} title={t('common.close')} onClick={close}>
              <X size={13} strokeWidth={1.75} />
            </button>
          </div>
          {values.map((v) => (
            <div key={v.key} className="cpin__row">
              <span className="cpin__key">{v.key}:</span>
              <span className="cpin__value cn-numeric">{v.text}</span>
              <button type="button" className="cpin__icon" aria-label={t('pin.color.copy')} title={t('pin.color.copy')} onClick={() => copy(v.text)}>
                {copyIcon(v.text)}
              </button>
            </div>
          ))}
        </div>
      ) : (
        <div className="cpin__card cn-glass">
          <button type="button" className="cpin__swatch cpin__swatch--btn" style={{ background: css }} aria-label={t('pin.color.expand')} title={t('pin.color.expand')} onClick={() => toggle(true)} />
          <span className="cpin__value cn-numeric">{hex}</span>
          <button type="button" className="cpin__icon" aria-label={t('pin.color.copy')} title={t('pin.color.copy')} onClick={() => copy(hex)}>
            {copyIcon(hex)}
          </button>
          <button type="button" className="cpin__icon cpin__close" aria-label={t('common.close')} title={t('common.close')} onClick={close}>
            <X size={12} strokeWidth={1.75} />
          </button>
        </div>
      )}
    </div>
  );
}
