// 智能推荐的图标：一颗水珠。几层叠起来——底色渐变、里面缓慢转动的光纹、左上的高光、一圈内沿亮边；
// 轮廓轻轻起伏（像晃动的水，不像果冻那样大幅变形）。鼠标在上面移动时整颗朝鼠标那边倾斜、高光跟着挪；
// 平时静止（省得一直重画），鼠标移上去、alive（推荐模式下）时缓慢流动，等结果的时候起伏和光纹都快起来。颜色有几套（暖橙、海蓝、珍珠白、薄荷、紫晶），面板的光晕跟着同一套。

import clsx from 'clsx';
import { useRef } from 'react';

export const ORB_COLORS = ['warm', 'ocean', 'pearl', 'mint', 'violet'] as const;
export type OrbColor = (typeof ORB_COLORS)[number];

export function WaterOrb({
  color,
  size = 26,
  busy,
  alive,
  interactive,
}: {
  color: OrbColor;
  size?: number;
  busy?: boolean;
  alive?: boolean;
  interactive?: boolean;
}) {
  const el = useRef<HTMLSpanElement>(null);
  const tilt = (e: React.PointerEvent) => {
    const node = el.current;
    if (!node) return;
    const r = node.getBoundingClientRect();
    // -1..1：鼠标相对水珠中心的位置
    const dx = Math.max(-1, Math.min(1, (e.clientX - r.left - r.width / 2) / (r.width / 2)));
    const dy = Math.max(-1, Math.min(1, (e.clientY - r.top - r.height / 2) / (r.height / 2)));
    node.style.setProperty('--orb-rx', `${(-dy * 16).toFixed(1)}deg`);
    node.style.setProperty('--orb-ry', `${(dx * 16).toFixed(1)}deg`);
    node.style.setProperty('--orb-hx', `${(dx * 18).toFixed(1)}%`);
    node.style.setProperty('--orb-hy', `${(dy * 18).toFixed(1)}%`);
  };
  const reset = () => {
    const node = el.current;
    if (!node) return;
    for (const p of ['--orb-rx', '--orb-ry', '--orb-hx', '--orb-hy']) node.style.removeProperty(p);
  };
  return (
    <span
      ref={el}
      className={clsx('orb', busy && 'orb--busy', alive && 'orb--alive')}
      data-orb={color}
      style={{ width: size, height: size }}
      onPointerMove={interactive ? tilt : undefined}
      onPointerLeave={interactive ? reset : undefined}
      aria-hidden
    >
      <span className="orb__body">
        <span className="orb__swirl" />
        <span className="orb__depth" />
      </span>
      <span className="orb__shine" />
    </span>
  );
}
