// 翻译源的小标志（卡片标题、设置里的列表前面）。都是自己画的简化图形，按各家的主色区分，一眼认出是哪个源。

import { Sparkles } from 'lucide-react';
import type { CSSProperties } from 'react';

/** 圆角小方块（行内样式：主窗口和翻译面板都用，不依赖哪个页面的样式表） */
const tile = (size: number, extra?: CSSProperties): CSSProperties => ({
  flex: 'none',
  display: 'inline-grid',
  placeItems: 'center',
  width: size,
  height: size,
  overflow: 'hidden',
  borderRadius: '27%',
  color: '#fff',
  fontWeight: 700,
  lineHeight: 1,
  boxShadow: 'inset 0 0 0 0.5px rgba(0, 0, 0, 0.12)',
  ...extra,
});

const BADGES: Record<string, { bg: string; text: string }> = {
  bing: { bg: 'linear-gradient(135deg, #2fd1c4, #1b74e4)', text: 'b' },
  transmart: { bg: 'linear-gradient(135deg, #3d8bff, #6a4dff)', text: 'T' },
  youdao: { bg: '#e5322d', text: '有' },
  deepl: { bg: '#0f2b46', text: 'D' },
  baidu: { bg: '#2932e1', text: '百' },
  caiyun: { bg: 'linear-gradient(135deg, #ffb347, #ff5f6d)', text: '彩' },
  niutrans: { bg: '#f08c1a', text: '牛' },
  azure: { bg: '#0078d4', text: 'A' },
  tencentcloud: { bg: '#00a3ff', text: '云' },
  volcengine: { bg: 'linear-gradient(135deg, #ff7a45, #f5222d)', text: '火' },
};

/** 谷歌的四色 G：圆环分四段，右边留口，中间一道横杠 */
function GoogleMark({ size }: { size: number }) {
  const arc = (from: number, to: number) => {
    const p = (deg: number) => {
      const r = (deg * Math.PI) / 180;
      return `${(12 + 7.5 * Math.cos(r)).toFixed(2)} ${(12 + 7.5 * Math.sin(r)).toFixed(2)}`;
    };
    return `M ${p(from)} A 7.5 7.5 0 0 1 ${p(to)}`;
  };
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" strokeWidth={3.6} style={{ display: 'block' }} aria-hidden>
      <rect width="24" height="24" rx="6" fill="#fff" />
      <path d={arc(0, 45)} stroke="#4285f4" />
      <path d={arc(45, 140)} stroke="#34a853" />
      <path d={arc(140, 215)} stroke="#fbbc05" />
      <path d={arc(215, 318)} stroke="#ea4335" />
      <path d="M12.2 12 H19.5" stroke="#4285f4" />
    </svg>
  );
}

/** `name`：用户自己添加的翻译没有专门的标志，用名字的第一个字 */
export function ProviderLogo({ id, size = 16, name }: { id: string; size?: number; name?: string }) {
  if (id === 'google') {
    return (
      <span style={tile(size)}>
        <GoogleMark size={size} />
      </span>
    );
  }
  if (id === 'openai') {
    return (
      <span style={tile(size, { background: 'linear-gradient(135deg, #8b5cf6, #ec4899)' })}>
        <Sparkles size={size * 0.68} strokeWidth={2.2} color="#fff" />
      </span>
    );
  }
  const badge = BADGES[id];
  const text = badge?.text ?? [...(name?.trim() || id)][0]?.toUpperCase() ?? '?';
  // 汉字比字母占得满，小一号
  const wide = text.charCodeAt(0) > 0x2e80;
  return (
    <span style={tile(size, { background: badge?.bg ?? 'linear-gradient(135deg, #6b7280, #374151)', fontSize: Math.round(size * (wide ? 0.6 : 0.72)) })}>
      {text}
    </span>
  );
}
