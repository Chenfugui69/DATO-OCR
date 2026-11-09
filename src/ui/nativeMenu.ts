// 原生右键菜单。小窗口（剪贴板面板、贴图）里网页菜单会被窗口边界裁掉，用系统菜单。

import { CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu } from '@tauri-apps/api/menu';

import { popmenu } from '@/lib/ipc';
import type { PopItem } from '@/lib/types';

export interface NativeItem {
  label?: string;
  separator?: boolean;
  checked?: boolean;
  disabled?: boolean;
  onSelect?: () => void;
  submenu?: NativeItem[];
  /** 自己画的菜单（popmenu 窗口）里用的图标名，见 PopMenuView 的图标表；系统菜单不画图标 */
  popIcon?: string;
  /** 自己画的菜单里画成红色（删除这种） */
  danger?: boolean;
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

// ───────── 自己画的菜单（popmenu 窗口）：样子和程序别处一样（图标、圆角、深浅色），子菜单在菜单里就地展开 ─────────

let appOpen = false;
const handlers = new Map<string, () => void>();

function toPop(items: NativeItem[], prefix: string): PopItem[] {
  return items.map((it, i): PopItem => {
    if (it.separator) return { kind: 'separator' };
    const id = `${prefix}${i}`;
    if (it.onSelect) handlers.set(id, it.onSelect);
    return {
      kind: 'item',
      id,
      label: it.label ?? '',
      icon: it.popIcon,
      checked: it.checked,
      disabled: it.disabled,
      danger: it.danger,
      submenu: it.submenu ? toPop(it.submenu, `${id}.`) : undefined,
    };
  });
}

/** 在鼠标那里弹自己画的菜单；选了哪项由 `owner` 窗口收到 `popmenu-select` 后调 `runAppMenuPick` */
export async function popupAppMenu(owner: string, items: NativeItem[]): Promise<void> {
  handlers.clear();
  appOpen = true;
  await popmenu.open(owner, toPop(items, 'm'));
}

/** 菜单窗口报回来选了哪项 */
export function runAppMenuPick(id: string) {
  appOpen = false;
  handlers.get(id)?.();
}

/** 自己画的菜单开着（菜单窗口拿走了焦点，面板别因为失焦就收起）。面板重新拿到焦点时调 `appMenuClosed` */
export function isAppMenuOpen(): boolean {
  return appOpen;
}

export function appMenuClosed() {
  appOpen = false;
}
