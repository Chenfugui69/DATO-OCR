// 剪贴板记录来自哪种设备的小图标，画在卡片底栏右边。和 lucide 图标一样按 currentColor 描线，
// 每种设备轮廓不同：iPhone 有灵动岛、安卓是挖孔、iPad 横着宽、iMac 有下巴、MacBook 屏幕上有刘海、
// 普通台式机 / 笔记本没有这些。`apple` = 通用剪贴板过来的，不知道是哪台苹果设备
// （手机叠平板，手机挡住平板的那部分用卡片底色填掉）。

import { useTranslation } from 'react-i18next';

export type DeviceKind = 'iphone' | 'ipad' | 'android' | 'mac' | 'macbook' | 'pc' | 'laptop' | 'apple';

const PATHS: Record<DeviceKind, React.ReactNode> = {
  iphone: (
    <>
      <rect x="6.5" y="2.5" width="11" height="19" rx="2.6" />
      <path d="M10.6 5h2.8" strokeWidth="1.9" />
    </>
  ),
  android: (
    <>
      <rect x="6.5" y="2.5" width="11" height="19" rx="1.6" />
      <circle cx="12" cy="5.2" r="0.55" fill="currentColor" />
      <path d="M10 18.6h4" />
    </>
  ),
  ipad: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="2.2" />
      <circle cx="12" cy="6" r="0.45" fill="currentColor" />
    </>
  ),
  mac: (
    <>
      <rect x="2.5" y="3.5" width="19" height="13.5" rx="1.6" />
      <path d="M2.5 13.5h19" />
      <path d="M10 17v3.5M14 17v3.5M8.5 20.5h7" />
    </>
  ),
  pc: (
    <>
      <rect x="2.5" y="3.5" width="19" height="12.5" rx="1" />
      <path d="M12 16v4M8 20h8" />
    </>
  ),
  macbook: (
    <>
      <path d="M4.5 16V5.6a1.6 1.6 0 0 1 1.6-1.6h11.8a1.6 1.6 0 0 1 1.6 1.6V16" />
      <path d="M10.6 4v1h2.8V4" />
      <path d="M2 16h20l-.6 1.6a1.6 1.6 0 0 1-1.5 1H4.1a1.6 1.6 0 0 1-1.5-1z" />
    </>
  ),
  laptop: (
    <>
      <rect x="4.5" y="4" width="15" height="11.5" rx="1" />
      <path d="M2.5 15.5h19l-1 3.5h-17z" />
    </>
  ),
  apple: (
    <>
      <rect x="9" y="4" width="12.5" height="16" rx="1.8" />
      <rect x="3" y="8" width="7.5" height="13" rx="1.8" fill="var(--cn-bg-elevated)" />
      <path d="M5.8 10.2h1.9" />
    </>
  ),
};

export function isDeviceKind(v: string | null | undefined): v is DeviceKind {
  return !!v && v in PATHS;
}

export function DeviceIcon({ kind, size = 14, title }: { kind: string | null | undefined; size?: number; title?: string }) {
  const { t } = useTranslation();
  if (!isDeviceKind(kind)) return null;
  const label = title ?? t(`clip.device.${kind}`);
  return (
    <svg
      className="device-icon"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
      strokeLinecap="round"
      strokeLinejoin="round"
      role="img"
      aria-label={label}
    >
      <title>{label}</title>
      {PATHS[kind]}
    </svg>
  );
}
