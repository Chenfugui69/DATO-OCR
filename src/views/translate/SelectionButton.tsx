// 划词悬浮按钮：选中文字后出现在选区旁边的一颗玻璃珠，里面是 DATO OCR 的标志（灰色）。
//
// 点击由 Rust 侧的全局鼠标钩子截获（这个窗口永远不激活，被选文字的程序焦点和选区都不受
// 影响），所以这里只负责长相：悬停反馈、每次出现时重放一遍出场动画。
//
// 玻璃是页面自己画的。试过 macOS 26 系统的液态玻璃控件（NSGlassEffectView）：它折射的是**自己窗口里**的
// 内容，放在一个透明的小浮窗里、底下是别的程序的画面时，画出来只是一块几乎看不见的平色；系统的毛玻璃
// （NSVisualEffectView）能模糊窗口后面，但出来是一块灰，深浅也不跟着底下走。所以自己做：
//
// - 后端在按钮出现前把它要盖住的那块屏幕抓下来（`backdrop`），放进珠子里当"透过玻璃看到的东西"：
//   液态玻璃 = 按一颗凸起的玻璃珠折射（中间略放大，边缘把画面压弯、带一圈彩边，见 `liquidLens.ts`）；
//   磨砂玻璃 = 模糊得很重，图标在密集的文字上也看得清。
// - 上面盖一层淡色，再画高光：上沿一弯月牙、下沿一圈回光、边缘一道细亮线，底下一圈柔和的投影。
// - 抓不到那块画面时（macOS 没有屏幕录制权限、抓屏出错）不透底，画一个实一些的。
// - 自定义：液态玻璃的色差、模糊、折射、放大、雾、淡色、高光都按设置里的滑块来。
//
// 玻璃珠本身（`GlassDot`）设置页也拿去做预览，调滑块时就能看到效果。

import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { translate } from '@/lib/ipc';
import { useSettings } from '@/lib/settings';
import type { GlassTuning, SolidDot } from '@/lib/types';
import { BrandMark } from '@/ui/BrandMark';

import { drawLens, lensBlur, lensFromTuning } from './liquidLens';

export type DotStyle = 'liquid' | 'frosted' | 'solid' | 'custom';

/**
 * 一颗玻璃珠。外面那个元素要比珠子大一圈（四周各 12px，和 Rust 的 BUTTON_INSET 一致），留给投影和悬停放大；
 * `backdrop` 是这一圈范围里底下的画面（data URL），`dark` 是后端按那块画面判断的深浅（不知道时 null）。
 * `replay` 变了就重放一遍出场动画
 */
export function GlassDot({
  style,
  tuning,
  solid: solidColors,
  size,
  backdrop,
  dark,
  replay,
  title,
  onPointerDown,
}: {
  style: DotStyle;
  tuning?: GlassTuning;
  /** 纯色圆点的配色（底色、图标、边框、阴影） */
  solid?: SolidDot;
  size: number;
  backdrop: string | null;
  dark: boolean | null;
  replay?: number;
  title?: string;
  onPointerDown?: () => void;
}) {
  // 玻璃的深浅跟着它底下的画面走（液态玻璃就是这样）；后端不知道底下是什么时跟应用的深浅色
  // 液态玻璃下以图标正底下那一小块的明暗为准（整块画面偏亮、图标底下却正好是一团黑字的情况不少）
  const [lensDark, setLensDark] = useState<{ backdrop: string; dark: boolean } | null>(null);
  const centerDark = lensDark && lensDark.backdrop === backdrop ? lensDark.dark : dark;
  const tone = centerDark == null ? undefined : centerDark ? 'dark' : 'light';
  const custom = style === 'custom' && tuning ? tuning : null;
  // 液态玻璃 / 自定义：底下的画面折射后画到画布上。画布按实际像素开（Retina 上是两倍），不然边缘的折射是糊的
  const liquid = (style === 'liquid' || style === 'custom') && !!backdrop;
  // 纯色：不画玻璃、不透出底下的画面（后端这时也不抓屏），最省事
  const solid = style === 'solid';
  const lens = useRef<HTMLCanvasElement>(null);
  const params = custom ? lensFromTuning(custom) : undefined;
  const paramsKey = JSON.stringify(params ?? null);
  useEffect(() => {
    if (!liquid || !backdrop) return;
    const image = new Image();
    let stale = false;
    image.onload = () => {
      const canvas = lens.current;
      if (stale || !canvas) return;
      const px = Math.max(1, Math.round(size * window.devicePixelRatio));
      canvas.width = px;
      canvas.height = px;
      const isDark = drawLens(canvas, image, image.naturalWidth, image.naturalHeight, params);
      if (isDark != null) setLensDark({ backdrop, dark: isDark });
    };
    image.src = backdrop;
    return () => {
      stale = true;
    };
    // params 按内容比（每次渲染都是新对象）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [liquid, backdrop, size, paramsKey]);
  // 自定义：淡色、高光按滑块（深色底上淡色是黑的，浓一点才看得出）
  const vars: Record<string, string> = {};
  if (custom) {
    const a = custom.tint / 100;
    vars['--tint'] = tone === 'dark' ? `rgba(20, 20, 26, ${Math.min(1, a * 1.6).toFixed(3)})` : `rgba(255, 255, 255, ${a.toFixed(3)})`;
    vars['--hi'] = ((custom.highlight / 100) * (tone === 'dark' ? 0.45 : 1)).toFixed(3);
    vars['--glow'] = ((custom.highlight / 200) * (tone === 'dark' ? 0.4 : 1)).toFixed(3);
  }
  // 纯色：用户配的颜色、边框、阴影（没配的颜色跟着主题，见 GlassDotStyles）
  if (solid && solidColors) {
    if (solidColors.background) vars['--solid-bg'] = solidColors.background;
    if (solidColors.icon) vars['--solid-fg'] = solidColors.icon;
    if (solidColors.border) vars['--edge'] = solidColors.border;
    vars['--edge-w'] = `${solidColors.borderWidth}px`;
    vars['--drop'] = ((solidColors.shadow / 100) * 0.36).toFixed(3);
  }
  const blur = custom ? (lensBlur(custom) * size) / 28 : 0;
  return (
    <div
      key={replay}
      className="selbtn"
      data-tone={tone}
      // 自定义用液态玻璃那一套样式，淡色、高光由上面的变量盖掉
      data-style={style === 'custom' ? 'liquid' : style}
      data-backdrop={backdrop ? 'yes' : 'no'}
      title={title}
      style={vars as React.CSSProperties}
      onPointerDown={onPointerDown}
    >
      {!solid &&
        (liquid ? (
          <canvas ref={lens} className="selbtn__refract" style={blur > 0 ? { filter: `saturate(1.25) brightness(1.03) blur(${blur.toFixed(2)}px)` } : undefined} />
        ) : (
          backdrop && <i className="selbtn__lens" style={{ backgroundImage: `url(${backdrop})` }} />
        ))}
      {!solid && <i className="selbtn__glass" />}
      <BrandMark size={Math.round(size * 0.6)} mono />
    </div>
  );
}

/** 玻璃珠的样式（划词按钮窗口和设置页的预览都要挂一份） */
export function GlassDotStyles() {
  return (
    <style>{`
        /* 圆点比窗口小一圈（四周各 12px，和 Rust 的 BUTTON_INSET 一致），留给投影和悬停放大。
           投影最远到 3 + 8 = 11px，整个落在这圈留边里；超出去的会被窗口的边切成一条直线 */
        .selbtn {
          --tint: rgba(255, 255, 255, 0.3);
          --hi: 0.95; --glow: 0.6; --rim: 0.9; --edge: rgba(0, 0, 0, 0.16); --drop: 0.18;
          position: relative; isolation: isolate; overflow: hidden;
          width: calc(100% - 24px); height: calc(100% - 24px); border-radius: 50%;
          display: grid; place-items: center;
          color: rgba(50, 50, 56, 0.82);
          box-shadow: 0 0 0 var(--edge-w, 0.5px) var(--edge), 0 1px 2px rgba(0, 0, 0, calc(var(--drop) * 0.8)), 0 3px 8px rgba(0, 0, 0, var(--drop));
          transition: color 140ms var(--cn-ease-out), transform 160ms var(--cn-ease-out);
          /* 出场要快：选完字眼睛就在找它 */
          animation: selbtn-in 140ms var(--cn-ease-out);
        }
        .selbtn > svg { position: relative; z-index: 2; }
        /* 液态玻璃：图标再描一圈和它反色的柔光，压在什么画面上都认得出 */
        .selbtn[data-style='liquid'][data-backdrop='yes'] > svg { filter: drop-shadow(0 0 1.2px rgba(255, 255, 255, 0.9)) drop-shadow(0 0 3px rgba(255, 255, 255, 0.55)); }
        [data-theme='dark'] .selbtn[data-style='liquid'][data-backdrop='yes']:not([data-tone='light']) > svg,
        .selbtn[data-style='liquid'][data-backdrop='yes'][data-tone='dark'] > svg { filter: drop-shadow(0 0 1.2px rgba(0, 0, 0, 0.85)) drop-shadow(0 0 3px rgba(0, 0, 0, 0.5)); }
        .selbtn[data-style='liquid'][data-backdrop='yes'] { color: rgba(28, 28, 34, 0.92); }
        [data-theme='dark'] .selbtn[data-style='liquid'][data-backdrop='yes']:not([data-tone='light']),
        .selbtn[data-style='liquid'][data-backdrop='yes'][data-tone='dark'] { color: rgba(255, 255, 255, 0.96); }
        /* 透过玻璃看到的那块画面：和窗口一样大、对齐到窗口（圆点比窗口小 12px），再从中心放大一点 */
        .selbtn__lens {
          position: absolute; z-index: 0; inset: -12px;
          background-size: 100% 100%;
          filter: blur(1.4px) saturate(1.5) brightness(1.04);
          transform: scale(1.22);
        }
        .selbtn[data-style='frosted'] .selbtn__lens { filter: blur(7px) saturate(1.7); transform: scale(1.5); }
        /* 液态玻璃：折射好的画面（画布），只再提一点饱和度 */
        .selbtn__refract { position: absolute; z-index: 0; inset: 0; width: 100%; height: 100%; filter: saturate(1.25) brightness(1.03); }
        /* 液态玻璃要透：淡色压得很薄，高光收窄，让边缘的折射和彩边露出来 */
        .selbtn[data-style='liquid'][data-backdrop='yes'] { --tint: rgba(255, 255, 255, 0.1); --hi: 0.7; --glow: 0.35; }
        [data-theme='dark'] .selbtn[data-style='liquid'][data-backdrop='yes']:not([data-tone='light']),
        .selbtn[data-style='liquid'][data-backdrop='yes'][data-tone='dark'] { --tint: rgba(20, 20, 26, 0.16); --hi: 0.3; --glow: 0.12; }
        /* 玻璃本身：一层淡色 + 高光 */
        .selbtn__glass {
          position: absolute; z-index: 1; inset: 0; border-radius: 50%;
          background:
            radial-gradient(ellipse 78% 46% at 50% -4%, rgba(255,255,255,var(--hi)), rgba(255,255,255,0) 72%),
            radial-gradient(ellipse 70% 40% at 50% 108%, rgba(255,255,255,var(--glow)), rgba(255,255,255,0) 70%),
            var(--tint);
          box-shadow:
            inset 0 0 0 0.5px rgba(255,255,255,var(--rim)),
            inset 0 1px 0.5px rgba(255,255,255,var(--rim)),
            inset 0 -1px 1.5px rgba(0,0,0,0.12),
            inset 0 0 5px rgba(255,255,255,calc(var(--rim) * 0.4));
        }
        .selbtn[data-style='frosted'] { --tint: rgba(255, 255, 255, 0.58); --hi: 0.7; --glow: 0.35; }
        /* 抓不到底下的画面：不透底，画实一点 */
        .selbtn[data-backdrop='no'] { --tint: rgba(246, 246, 248, 0.82); }
        .selbtn[data-backdrop='no'][data-style='frosted'] { --tint: rgba(246, 246, 248, 0.94); }

        [data-theme='dark'] .selbtn:not([data-tone='light']), .selbtn[data-tone='dark'] {
          --tint: rgba(34, 34, 40, 0.34);
          --hi: 0.4; --glow: 0.16; --rim: 0.3; --edge: rgba(0, 0, 0, 0.5); --drop: 0.36;
          color: rgba(240, 240, 248, 0.88);
        }
        [data-theme='dark'] .selbtn[data-style='frosted']:not([data-tone='light']), .selbtn[data-style='frosted'][data-tone='dark'] { --tint: rgba(40, 40, 46, 0.6); --hi: 0.28; }
        [data-theme='dark'] .selbtn[data-backdrop='no']:not([data-tone='light']), .selbtn[data-backdrop='no'][data-tone='dark'] { --tint: rgba(52, 52, 58, 0.86); }
        [data-theme='dark'] .selbtn[data-backdrop='no'][data-style='frosted']:not([data-tone='light']), .selbtn[data-backdrop='no'][data-style='frosted'][data-tone='dark'] { --tint: rgba(52, 52, 58, 0.95); }

        .selbtn:hover { color: rgba(0, 0, 0, 0.9); transform: scale(1.08); }
        [data-theme='dark'] .selbtn:not([data-tone='light']):hover, .selbtn[data-tone='dark']:hover { color: #fff; }
        /* 纯色：一块实心的圆，图标用主题色；深色模式换成深灰底。放在最后、选择器写重一点，盖过上面玻璃的颜色规则 */
        .selbtn.selbtn[data-style='solid'], .selbtn.selbtn[data-style='solid']:hover { background: var(--solid-bg, #fff); color: var(--solid-fg, var(--cn-accent)); --edge: rgba(0, 0, 0, 0.14); }
        [data-theme='dark'] .selbtn.selbtn[data-style='solid'], [data-theme='dark'] .selbtn.selbtn[data-style='solid']:hover { background: var(--solid-bg, #2c2c2e); color: var(--solid-fg, var(--cn-accent)); --edge: rgba(0, 0, 0, 0.5); }
        @keyframes selbtn-in { from { opacity: 0; transform: scale(0.7); } }

    `}</style>
  );
}

export default function SelectionButton() {
  const { t } = useTranslation();
  const settings = useSettings();
  const [shown, setShown] = useState<{ generation: number; dark: boolean | null; backdrop: string | null }>({ generation: 0, dark: null, backdrop: null });
  useEvent('selection-button-show', setShown);
  const sel = settings?.translate.selection;
  return (
    <div className="selbtn-root">
      <GlassDot
        style={sel?.buttonStyle ?? 'liquid'}
        tuning={sel?.glass}
        solid={sel?.solid}
        // 圆点多大由设置定（窗口已经按它摆好了），里面的图标跟着按比例变
        size={sel?.buttonSize ?? 28}
        backdrop={shown.backdrop}
        dark={shown.dark}
        replay={shown.generation}
        title={t('translate.selectionButton')}
        // 正常情况下圆点上的点击在后端的监听那边就被拦下了，到不了这里；到了说明那边没认出来：
        // 告诉后端，由它决定按点了圆点处理，还是把残留的圆点收起来
        onPointerDown={() => void translate.buttonPressed().catch(() => undefined)}
      />
      <style>{`
        html[data-view='selbtn'], html[data-view='selbtn'] body { background: transparent !important; overflow: hidden; }
        .selbtn-root { position: fixed; inset: 0; display: grid; place-items: center; }
      `}</style>
      <GlassDotStyles />
    </div>
  );
}
