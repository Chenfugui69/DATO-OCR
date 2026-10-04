// 自绘取色器：饱和度/明度面板 + 色相条 + 十六进制输入 + 吸管。
//
// 不用 <input type="color">：WebView2 里那个系统弹窗贴着窗口右边时会被裁掉一半，
// 拖动时还会每秒触发几十次 change。这里拖动只回调 onChange，松手才 onCommit。

import * as Popover from '@radix-ui/react-popover';
import { Pipette } from 'lucide-react';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

interface Hsv {
  h: number;
  s: number;
  v: number;
}

function hexToRgb(hex: string): [number, number, number] | null {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return null;
  const n = parseInt(m[1]!, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function rgbToHex(r: number, g: number, b: number): string {
  return `#${[r, g, b].map((c) => Math.round(c).toString(16).padStart(2, '0')).join('')}`.toUpperCase();
}

function rgbToHsv(r: number, g: number, b: number): Hsv {
  const rn = r / 255;
  const gn = g / 255;
  const bn = b / 255;
  const max = Math.max(rn, gn, bn);
  const d = max - Math.min(rn, gn, bn);
  let h = 0;
  if (d) {
    if (max === rn) h = ((gn - bn) / d) % 6;
    else if (max === gn) h = (bn - rn) / d + 2;
    else h = (rn - gn) / d + 4;
    h *= 60;
    if (h < 0) h += 360;
  }
  return { h, s: max ? d / max : 0, v: max };
}

function hsvToHex({ h, s, v }: Hsv): string {
  const f = (n: number) => {
    const k = (n + h / 60) % 6;
    return v - v * s * Math.max(0, Math.min(k, 4 - k, 1));
  };
  return rgbToHex(f(5) * 255, f(3) * 255, f(1) * 255);
}

const clamp01 = (x: number) => Math.min(1, Math.max(0, x));

interface EyeDropperCtor {
  new (): { open: () => Promise<{ sRGBHex: string }> };
}

/** 按住拖动：按下时算一次，拖动时持续算，松手回调。 */
function drag(e: React.PointerEvent<HTMLElement>, at: (x: number, y: number) => void, done: () => void) {
  e.preventDefault();
  const el = e.currentTarget;
  el.setPointerCapture(e.pointerId);
  const rect = el.getBoundingClientRect();
  const apply = (ev: { clientX: number; clientY: number }) => at(clamp01((ev.clientX - rect.left) / rect.width), clamp01((ev.clientY - rect.top) / rect.height));
  apply(e);
  const move = (ev: PointerEvent) => apply(ev);
  const up = () => {
    el.removeEventListener('pointermove', move);
    el.removeEventListener('pointerup', up);
    done();
  };
  el.addEventListener('pointermove', move);
  el.addEventListener('pointerup', up);
}

export function ColorPicker({
  value,
  onChange,
  onCommit,
  children,
}: {
  /** #RRGGBB */
  value: string;
  /** 拖动中持续回调（用来实时预览） */
  onChange?: (hex: string) => void;
  /** 定下来了（松手、输入完、吸管取到） */
  onCommit: (hex: string) => void;
  /** 触发按钮 */
  children: ReactNode;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [hsv, setHsv] = useState<Hsv>(() => rgbToHsv(...(hexToRgb(value) ?? [10, 132, 255])));
  const [text, setText] = useState(value.toUpperCase());
  const hsvRef = useRef(hsv);
  hsvRef.current = hsv;

  // 外面的值变了（选了预设色）：跟着变；色相在灰色时没意义，保留原来的免得色相条乱跳
  useEffect(() => {
    const rgb = hexToRgb(value);
    if (!rgb || hsvToHex(hsvRef.current) === value.toUpperCase()) return;
    const next = rgbToHsv(...rgb);
    setHsv(next.s === 0 ? { ...next, h: hsvRef.current.h } : next);
    setText(value.toUpperCase());
  }, [value]);

  const update = (next: Hsv) => {
    setHsv(next);
    const hex = hsvToHex(next);
    setText(hex);
    onChange?.(hex);
  };
  const commit = () => onCommit(hsvToHex(hsvRef.current));

  const EyeDropper = (window as unknown as { EyeDropper?: EyeDropperCtor }).EyeDropper;
  const pick = async () => {
    if (!EyeDropper) return;
    try {
      const { sRGBHex } = await new EyeDropper().open();
      const rgb = hexToRgb(sRGBHex);
      if (!rgb) return;
      const hex = rgbToHex(...rgb);
      setHsv(rgbToHsv(...rgb));
      setText(hex);
      onChange?.(hex);
      onCommit(hex);
    } catch {
      // 用户按 Esc 取消了
    }
  };

  const hue = `hsl(${hsv.h} 100% 50%)`;
  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>{children}</Popover.Trigger>
      <Popover.Portal>
        <Popover.Content className="cn-popover cn-color" side="bottom" align="end" sideOffset={6} collisionPadding={10}>
          <div
            className="cn-color__sv"
            style={{ background: `linear-gradient(to top, #000, transparent), linear-gradient(to right, #fff, ${hue})` }}
            onPointerDown={(e) => drag(e, (x, y) => update({ ...hsvRef.current, s: x, v: 1 - y }), commit)}
          >
            <span className="cn-color__thumb" style={{ left: `${hsv.s * 100}%`, top: `${(1 - hsv.v) * 100}%`, background: hsvToHex(hsv) }} />
          </div>
          <div className="cn-color__hue" onPointerDown={(e) => drag(e, (x) => update({ ...hsvRef.current, h: x * 360 }), commit)}>
            <span className="cn-color__thumb cn-color__thumb--hue" style={{ left: `${(hsv.h / 360) * 100}%`, background: hue }} />
          </div>
          <div className="cn-color__row">
            {EyeDropper && (
              <button type="button" className="cn-color__pick" title={t('common.eyedropper')} aria-label={t('common.eyedropper')} onClick={() => void pick()}>
                <Pipette size={15} strokeWidth={1.75} />
              </button>
            )}
            <span className="cn-color__preview" style={{ background: hsvToHex(hsv) }} />
            <input
              className="cn-color__hex cn-mono"
              value={text}
              maxLength={7}
              spellCheck={false}
              onChange={(e) => {
                const v = e.target.value.toUpperCase();
                setText(v.startsWith('#') ? v : `#${v}`);
                const rgb = hexToRgb(v);
                if (rgb) {
                  setHsv(rgbToHsv(...rgb));
                  onChange?.(rgbToHex(...rgb));
                }
              }}
              onBlur={() => {
                const rgb = hexToRgb(text);
                if (rgb) onCommit(rgbToHex(...rgb));
                else setText(hsvToHex(hsv));
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
              }}
            />
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
