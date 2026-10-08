/**
 * DATO OCR 的标志：左边散开的点是图片里的像素，往右连成线 = 认出来的字。
 * 和应用图标（src-tauri/icons/icon.svg）是同一个图形，这里只画图形，底板由外面的容器给。
 */
const ROWS: [y: number, dots: number, end: number][] = [
  [352, 1, 760],
  [512, 3, 664],
  [672, 2, 728],
];

export function BrandMark({ size }: { size: number }) {
  return (
    <svg width={size} height={size} viewBox="192 192 640 640" aria-hidden>
      <defs>
        <linearGradient id="brand-mark" gradientUnits="userSpaceOnUse" x1="232" y1="0" x2="800" y2="0">
          <stop offset="0" stopColor="#E9FF5A" />
          <stop offset="1" stopColor="#2DF5B0" />
        </linearGradient>
      </defs>
      {ROWS.map(([y, dots, end]) => (
        <g key={y}>
          {Array.from({ length: dots }, (_, i) => (
            <circle key={i} cx={272 + i * 104} cy={y} r={38} fill="url(#brand-mark)" opacity={0.38 + 0.2 * i} />
          ))}
          <path d={`M ${272 + dots * 104} ${y} H ${end}`} stroke="url(#brand-mark)" strokeWidth={76} strokeLinecap="round" fill="none" />
        </g>
      ))}
    </svg>
  );
}
