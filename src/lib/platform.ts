// 窗口身份与资源地址。

import { convertFileSrc } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

import { system } from './ipc';

export function windowLabel(): string {
  try {
    return getCurrentWindow().label;
  } catch {
    return 'main';
  }
}

/** 内存图片仓库里的图（冻结画面、贴图、长图…），经 `shot:` 协议取。 */
export function shotUrl(id: string, version?: number): string {
  const base = convertFileSrc(id, 'shot');
  return version === undefined ? base : `${base}?v=${version}`;
}

let dataDirCache: string | null = null;
let dataDirPromise: Promise<string> | null = null;

/** 启动时调一次，之后 `assetUrl` 同步可用。 */
export function loadDataDir(): Promise<string> {
  dataDirPromise ??= system.dataDir().then((dir) => {
    dataDirCache = dir;
    return dir;
  });
  return dataDirPromise;
}

/** 数据目录下的相对路径（数据库里存的就是这种）→ 可直接给 <img> 用的地址。 */
export function assetUrl(rel: string | null | undefined): string | undefined {
  if (!rel || !dataDirCache) return undefined;
  const sep = dataDirCache.includes('\\') ? '\\' : '/';
  return convertFileSrc(`${dataDirCache}${sep}${rel.replace(/\//g, sep)}`);
}

export const isMac = navigator.userAgent.includes('Mac');

const MAC_KEYS: Record<string, string> = {
  ctrl: '⌃',
  control: '⌃',
  alt: '⌥',
  option: '⌥',
  shift: '⇧',
  super: '⌘',
  cmd: '⌘',
  command: '⌘',
  meta: '⌘',
};

/** 全局热键里的一个键怎么显示（设置里存的是 `Ctrl+Alt+T` 这种写法）。macOS 上修饰键换成符号。 */
export function keyLabel(key: string): string {
  if (isMac) return MAC_KEYS[key.toLowerCase()] ?? key;
  return key === 'Super' ? 'Win' : key;
}

/** 整个全局热键的显示写法：Windows `Ctrl+Alt+T`，macOS `⌃⌥T`。 */
export function accelLabel(accel: string): string {
  const parts = accel.split('+').filter(Boolean).map(keyLabel);
  return parts.join(isMac ? '' : '+');
}

/**
 * 应用内快捷键的显示写法。代码里统一按 Windows 的习惯写（`Ctrl+Z`），处理按键时
 * Ctrl 和 ⌘ 都认；macOS 上显示成 `⌘Z`。
 */
export function shortcutLabel(shortcut: string): string {
  if (!isMac) return shortcut;
  return shortcut
    .split('+')
    .map((k) => (k === 'Ctrl' ? '⌘' : k === 'Alt' ? '⌥' : k === 'Shift' ? '⇧' : k))
    .join('');
}

/** "主修饰键"按下了没有：Windows 的 Ctrl，macOS 的 ⌘（Ctrl 也认）。 */
export function modKey(e: { ctrlKey: boolean; metaKey: boolean }): boolean {
  return e.ctrlKey || e.metaKey;
}
