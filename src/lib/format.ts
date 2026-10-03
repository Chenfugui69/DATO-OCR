import i18n from '@/i18n';

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null) return '';
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

/** "刚刚 / 3 分钟前 / 昨天 14:20 / 9月3日"。 */
export function relativeTime(ms: number, now = Date.now()): string {
  const t = i18n.t.bind(i18n);
  const diff = Math.max(0, now - ms);
  const min = Math.floor(diff / 60000);
  if (min < 1) return t('time.justNow');
  if (min < 60) return t('time.minutesAgo', { count: min });
  const hours = Math.floor(min / 60);
  const d = new Date(ms);
  const today = new Date(now);
  const sameDay = d.toDateString() === today.toDateString();
  const hm = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  if (sameDay) return hours < 6 ? t('time.hoursAgo', { count: hours }) : hm;
  const yesterday = new Date(now - 86400000);
  if (d.toDateString() === yesterday.toDateString()) return t('time.yesterday', { time: hm });
  if (d.getFullYear() === today.getFullYear()) return t('time.monthDay', { month: d.getMonth() + 1, day: d.getDate() });
  return t('time.fullDate', { year: d.getFullYear(), month: d.getMonth() + 1, day: d.getDate() });
}

export function formatDateTime(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 日期分组标题：今天 / 昨天 / 本周 / 更早的按月。 */
export function dayGroup(ms: number, now = Date.now()): string {
  const t = i18n.t.bind(i18n);
  const d = new Date(ms);
  const today = new Date(now);
  if (d.toDateString() === today.toDateString()) return t('time.today');
  if (d.toDateString() === new Date(now - 86400000).toDateString()) return t('time.yesterdayLabel');
  if (now - ms < 7 * 86400000) return t('time.thisWeek');
  if (d.getFullYear() === today.getFullYear()) return t('time.month', { month: d.getMonth() + 1 });
  return t('time.yearMonth', { year: d.getFullYear(), month: d.getMonth() + 1 });
}

export function rgbToHex(r: number, g: number, b: number): string {
  return `#${[r, g, b].map((v) => v.toString(16).padStart(2, '0')).join('').toUpperCase()}`;
}

export function rgbToHsl(r: number, g: number, b: number): [number, number, number] {
  const rn = r / 255;
  const gn = g / 255;
  const bn = b / 255;
  const max = Math.max(rn, gn, bn);
  const min = Math.min(rn, gn, bn);
  const l = (max + min) / 2;
  if (max === min) return [0, 0, Math.round(l * 100)];
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h: number;
  if (max === rn) h = (gn - bn) / d + (gn < bn ? 6 : 0);
  else if (max === gn) h = (bn - rn) / d + 2;
  else h = (rn - gn) / d + 4;
  return [Math.round(h * 60), Math.round(s * 100), Math.round(l * 100)];
}

export function formatColor(rgb: [number, number, number], format: 'hex' | 'rgb' | 'hsl'): string {
  const [r, g, b] = rgb;
  if (format === 'rgb') return `rgb(${r}, ${g}, ${b})`;
  if (format === 'hsl') {
    const [h, s, l] = rgbToHsl(r, g, b);
    return `hsl(${h}, ${s}%, ${l}%)`;
  }
  return rgbToHex(r, g, b);
}

/** 颜色卡片上文字该用黑还是白（相对亮度）。 */
export function readableOn(color: string): 'black' | 'white' {
  const m = color.trim().match(/^#?([0-9a-f]{3,8})$/i);
  let r = 128;
  let g = 128;
  let b = 128;
  if (m?.[1]) {
    let hex = m[1];
    if (hex.length <= 4) hex = hex.split('').map((c) => c + c).join('');
    r = parseInt(hex.slice(0, 2), 16);
    g = parseInt(hex.slice(2, 4), 16);
    b = parseInt(hex.slice(4, 6), 16);
  } else {
    const nums = color.match(/\d+(\.\d+)?/g)?.map(Number);
    if (nums && nums.length >= 3 && color.toLowerCase().startsWith('rgb')) [r, g, b] = nums as [number, number, number];
  }
  const lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
  return lum > 150 ? 'black' : 'white';
}
