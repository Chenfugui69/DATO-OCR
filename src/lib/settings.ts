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

/** 保存排队：一次只发一个；排队期间又改了好几次的，只发最后那份（拖滑杆、取色时每秒几十次）。 */
let chain: Promise<unknown> = Promise.resolve();
let pending: Settings | null = null;

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settings: null,
  load: async () => {
    const s = await system.settings();
    set({ settings: s });
    return s;
  },
  update: (mutate) => {
    const current = get().settings;
    if (!current) return Promise.resolve(null);
    const draft = structuredClone(current);
    mutate(draft);
    set({ settings: draft });
    pending = draft;
    const run = chain
      .catch(() => undefined)
      .then(async () => {
        const next = pending;
        // 前面排队的那次已经把这份一起存了
        if (!next) return get().settings;
        pending = null;
        try {
          const saved = await system.setSettings(next);
          // 存的过程中又有新改动：别用旧结果盖掉界面上的新值
          if (!pending) set({ settings: saved });
          return saved;
        } catch (err) {
          if (!pending) set({ settings: await system.settings().catch(() => current) });
          throw err;
        }
      });
    chain = run;
    return run;
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
