// 原生右键菜单。小窗口（剪贴板面板、贴图）里网页菜单会被窗口边界裁掉，用系统菜单。

import { CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu } from '@tauri-apps/api/menu';

export interface NativeItem {
  label?: string;
  separator?: boolean;
  checked?: boolean;
  disabled?: boolean;
  onSelect?: () => void;
  submenu?: NativeItem[];
}

async function build(items: NativeItem[]): Promise<(MenuItem | PredefinedMenuItem | CheckMenuItem | Submenu)[]> {
  const out: (MenuItem | PredefinedMenuItem | CheckMenuItem | Submenu)[] = [];
  for (const item of items) {
    if (item.separator) out.push(await PredefinedMenuItem.new({ item: 'Separator' }));
    else if (item.submenu) out.push(await Submenu.new({ text: item.label ?? '', items: await build(item.submenu) }));
    else if (item.checked !== undefined)
      out.push(await CheckMenuItem.new({ text: item.label ?? '', checked: item.checked, enabled: !item.disabled, action: item.onSelect }));
    else out.push(await MenuItem.new({ text: item.label ?? '', enabled: !item.disabled, action: item.onSelect }));
  }
  return out;
}

let open = false;

/** 弹出菜单；菜单打开期间 `isNativeMenuOpen()` 为 true（用来别因为失焦把面板藏掉）。 */
export async function popupMenu(items: NativeItem[]): Promise<void> {
  const menu = await Menu.new({ items: await build(items) });
  open = true;
  try {
    await menu.popup();
  } finally {
    // 菜单项的回调异步到达，稍等再清标志
    window.setTimeout(() => {
      open = false;
    }, 300);
  }
}

export function isNativeMenuOpen(): boolean {
  return open;
}
