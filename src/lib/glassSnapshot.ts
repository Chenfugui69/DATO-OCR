// 本程序截图时，开着毛玻璃的面板临时换成页面自己画的实心底（Rust 的 `wm::GlassSnapshot`）。
//
// Windows 的抓屏里，系统模糊背板会无视圆角裁剪、画成整块矩形；所以抓屏前 Rust 发 `glass-snapshot`，
// 页面在 <html> 上挂 data-snapshot（各面板的样式里把色调换成不透明、停掉过渡），画完两帧回话，
// Rust 再把背板藏起来抓屏。冻结的画面里面板的圆角就全是页面画的，带抗锯齿。

import { useEvent } from './events';
import { system } from './ipc';

export function useGlassSnapshot() {
  useEvent('glass-snapshot', (on) => {
    const root = document.documentElement;
    if (!on) {
      delete root.dataset.snapshot;
      return;
    }
    root.dataset.snapshot = '';
    // 等样式生效、真的画到屏幕上（两帧）再告诉 Rust 可以抓了
    requestAnimationFrame(() => requestAnimationFrame(() => void system.glassSnapshotReady().catch(() => undefined)));
  });
}
