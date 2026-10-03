// 外观：深浅色、玻璃开关、减少动画，统一落到 <html> 的 data 属性上（规格 06 §3.3）。

import { on } from './events';
import { system } from './ipc';
import { useSettingsStore } from './settings';
import type { VisualCapabilities } from './types';

let visuals: VisualCapabilities | null = null;
const media = window.matchMedia('(prefers-color-scheme: dark)');

function apply(): void {
  const root = document.documentElement;
  const pref = useSettingsStore.getState().settings?.appearance.theme ?? 'system';
  const dark = pref === 'dark' || (pref === 'system' && (visuals?.darkMode ?? media.matches));
  root.dataset.theme = dark ? 'dark' : 'light';
  root.dataset.glass = visuals?.glass ? 'on' : 'off';
  root.dataset.reducedMotion = visuals?.reducedMotion ? 'true' : 'false';
}

/** `withBackdrop`：这个窗口启用了系统材质（Mica）时才让背景透明。 */
export async function initTheme(): Promise<void> {
  try {
    visuals = await system.visuals();
  } catch {
    visuals = null;
  }
  apply();
  media.addEventListener('change', apply);
  useSettingsStore.subscribe(apply);
  on('visuals-changed', (v) => {
    visuals = v;
    apply();
  }).catch(() => undefined);
}

export function currentVisuals(): VisualCapabilities | null {
  return visuals;
}
