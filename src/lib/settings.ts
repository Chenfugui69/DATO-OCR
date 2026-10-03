// 设置 store：启动时从 Rust 拉一次，之后监听 settings-changed 同步（规格 01 §4.2）。
// 每个窗口是独立 WebView，各自一份实例；跨窗口一律走事件。

import { create } from 'zustand';

import { on } from './events';
import { system } from './ipc';
import type { Settings } from './types';

interface SettingsState {
  settings: Settings | null;
  load: () => Promise<Settings>;
  /** 以函数形式修改并保存；返回 Rust 校正后的结果 */
  update: (mutate: (draft: Settings) => void) => Promise<Settings | null>;
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settings: null,
  load: async () => {
    const s = await system.settings();
    set({ settings: s });
    return s;
  },
  update: async (mutate) => {
    const current = get().settings;
    if (!current) return null;
    const draft = structuredClone(current);
    mutate(draft);
    set({ settings: draft });
    try {
      const saved = await system.setSettings(draft);
      set({ settings: saved });
      return saved;
    } catch (err) {
      set({ settings: current });
      throw err;
    }
  },
}));

let subscribed = false;

export function initSettingsSync(): void {
  if (subscribed) return;
  subscribed = true;
  on('settings-changed', (s) => useSettingsStore.setState({ settings: s })).catch(() => undefined);
}

export function useSettings(): Settings | null {
  return useSettingsStore((s) => s.settings);
}
