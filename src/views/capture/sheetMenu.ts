// 表格的右键菜单：和 Excel 右键差不多的几项。截图里用页面里的菜单（ui/overlays 的 ContextMenu）；
// 贴图窗口小，菜单会被裁掉，交给单独的菜单窗口（popmenu），两边用同一份菜单项和图标。

import { ClipboardPaste, Columns3, Copy, Eraser, Grid3x3, Hash, Redo2, Rows3, Scissors, TextSelect, Undo2, type LucideIcon } from 'lucide-react';

export type SheetAction = 'cut' | 'copy' | 'copyText' | 'copyMarkdown' | 'paste' | 'clear' | 'selectRow' | 'selectCol' | 'selectAll' | 'undo' | 'redo';

export interface SheetMenuItem {
  id: SheetAction;
  /** 图标名（popmenu 的图标表里也有，见 SHEET_ICONS） */
  icon: string;
  /** 快捷键（Ctrl+C 这种写法，显示时按平台换成 ⌘） */
  shortcut?: string;
  disabled?: boolean;
}

/** 菜单里用到的图标；popmenu 窗口的图标表把它并进去 */
export const SHEET_ICONS: Record<string, LucideIcon> = {
  cut: Scissors,
  copy: Copy,
  copyText: TextSelect,
  copyMarkdown: Hash,
  paste: ClipboardPaste,
  clear: Eraser,
  selectRow: Rows3,
  selectCol: Columns3,
  selectAll: Grid3x3,
  undo: Undo2,
  redo: Redo2,
};

/** 菜单项，null 是分隔线 */
export function sheetMenu(state: { hasRange: boolean; canUndo: boolean; canRedo: boolean }): (SheetMenuItem | null)[] {
  const none = !state.hasRange;
  return [
    { id: 'cut', icon: 'cut', shortcut: 'Ctrl+X', disabled: none },
    { id: 'copy', icon: 'copy', shortcut: 'Ctrl+C', disabled: none },
    { id: 'copyText', icon: 'copyText', disabled: none },
    { id: 'copyMarkdown', icon: 'copyMarkdown', disabled: none },
    { id: 'paste', icon: 'paste', shortcut: 'Ctrl+V', disabled: none },
    null,
    { id: 'clear', icon: 'clear', shortcut: 'Delete', disabled: none },
    null,
    { id: 'selectRow', icon: 'selectRow', disabled: none },
    { id: 'selectCol', icon: 'selectCol', disabled: none },
    { id: 'selectAll', icon: 'selectAll', shortcut: 'Ctrl+A' },
    null,
    { id: 'undo', icon: 'undo', shortcut: 'Ctrl+Z', disabled: !state.canUndo },
    { id: 'redo', icon: 'redo', shortcut: 'Ctrl+Y', disabled: !state.canRedo },
  ];
}
