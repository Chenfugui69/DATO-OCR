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
//   液态玻璃 = 放大一点、轻轻模糊（透镜）；磨砂玻璃 = 模糊得很重。底下的字不再是清清楚楚地透过来，
//   图标在密集的文字上也看得清。
// - 上面盖一层淡色，再画高光：上沿一弯月牙、下沿一圈回光、边缘一道细亮线，底下一圈柔和的投影。
// - 抓不到那块画面时（Windows、没有屏幕录制权限）不透底，画一个实一些的。

import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { useSettings } from '@/lib/settings';
import { BrandMark } from '@/ui/BrandMark';

export default function SelectionButton() {
  const { t } = useTranslation();
  const settings = useSettings();
  const [shown, setShown] = useState<{ generation: number; dark: boolean | null; backdrop: string | null }>({ generation: 0, dark: null, backdrop: null });
  useEvent('selection-button-show', setShown);
  // 玻璃的深浅跟着它底下的画面走（液态玻璃就是这样）；后端不知道底下是什么时跟应用的深浅色
  const tone = shown.dark == null ? undefined : shown.dark ? 'dark' : 'light';
  const style = settings?.translate.selection.buttonStyle ?? 'liquid';
  // 圆点多大由设置定（窗口已经按它摆好了），里面的图标跟着按比例变
  const dot = settings?.translate.selection.buttonSize ?? 28;

  return (
    <div className="selbtn-root">
      <div
        key={shown.generation}
        className="selbtn"
        data-tone={tone}
        data-style={style}
        data-backdrop={shown.backdrop ? 'yes' : 'no'}
        title={t('translate.selectionButton')}
      >
        {shown.backdrop && <i className="selbtn__lens" style={{ backgroundImage: `url(${shown.backdrop})` }} />}
        <i className="selbtn__glass" />
        <BrandMark size={Math.round(dot * 0.6)} mono />
      </div>
      <style>{`
        html[data-view='selbtn'], html[data-view='selbtn'] body { background: transparent !important; overflow: hidden; }
        .selbtn-root { position: fixed; inset: 0; display: grid; place-items: center; }
        /* 圆点比窗口小一圈（四周各 7px，和 Rust 的 BUTTON_INSET 一致），留给投影和悬停放大 */
        .selbtn {
          --tint: rgba(255, 255, 255, 0.3);
          --hi: 0.95; --glow: 0.6; --rim: 0.9; --edge: rgba(0, 0, 0, 0.16); --drop: 0.18;
          position: relative; isolation: isolate; overflow: hidden;
          width: calc(100% - 14px); height: calc(100% - 14px); border-radius: 50%;
          display: grid; place-items: center;
          color: rgba(50, 50, 56, 0.82);
          box-shadow: 0 0 0 0.5px var(--edge), 0 1px 2px rgba(0, 0, 0, calc(var(--drop) * 0.8)), 0 4px 10px rgba(0, 0, 0, var(--drop));
          transition: color 140ms var(--cn-ease-out), transform 160ms var(--cn-ease-out);
          animation: selbtn-in 200ms var(--cn-ease-out);
        }
        .selbtn > svg { position: relative; z-index: 2; }
        /* 透过玻璃看到的那块画面：和窗口一样大、对齐到窗口（圆点比窗口小 7px），再从中心放大一点 */
        .selbtn__lens {
          position: absolute; z-index: 0; inset: -7px;
          background-size: 100% 100%;
          filter: blur(1.4px) saturate(1.5) brightness(1.04);
          transform: scale(1.22);
        }
        .selbtn[data-style='frosted'] .selbtn__lens { filter: blur(7px) saturate(1.7); transform: scale(1.5); }
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
        @keyframes selbtn-in { from { opacity: 0; transform: scale(0.6); } }
      `}</style>
    </div>
  );
}
