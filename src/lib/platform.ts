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
