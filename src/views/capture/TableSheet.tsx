// 识别出的表格原位盖在图上之后，像 Excel 一样操作（截图里、贴图里共用）。
//
// - 选格子：单击选一格，拖动框一片，Shift+单击扩到这一格，Ctrl+A 全选。框到合并格子的一部分就整格框进来
// - 移动：方向键（Shift+方向键扩选区）、Tab / Shift+Tab 左右、Enter / Shift+Enter 上下
// - 改字：直接打字 = 换掉这一格（中文输入法照样用）；双击或 F2 = 接着原来的字改；Enter / Tab 改完换格，Esc 放弃；
//   Alt+Enter 格子里换行
// - Delete 清空选中的格子；Ctrl+C / Ctrl+X 复制 / 剪切选区（制表符分隔 + 带样式的 HTML 表格，粘到 Excel 里格子对得上、
//   字色底色粗斜体都在）；Ctrl+V 从当前格开始贴（Excel 里复制来的一块）；Ctrl+Z / Ctrl+Y 撤销 / 重做
// - 右键：和 Excel 一样的菜单（剪切、复制、只复制文字、复制成 Markdown、粘贴、清除、选整行 / 整列 / 全部、撤销重做）。
//   右键点在选区外面先选中那一格
//
// 键盘都交给一个一直拿着焦点的输入框：它平时透明地压在当前格子上，打字时直接变成这一格的编辑框
// （输入法的候选框也就出在这一格旁边）。没在改字时按 Esc 先收起选区，再按一次才交还给截图（退出截图）。

import clsx from 'clsx';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { capture } from '@/lib/ipc';
import { shortcutLabel, shotUrl } from '@/lib/platform';
import type { CaptureTable, PopItem } from '@/lib/types';
import { ContextMenu, notify, type MenuItemSpec } from '@/ui/overlays';
import { toExcelText, toHtmlTable, toMarkdown, toTsv } from '@/views/ocr/structured';

import { cellsIn, move, ownerMap, parseTsv, paste, rangeGrid, rangeOf, rangeRows, startOf, tabMove, type Direction, type Pos, type Range } from './sheet';
import { SHEET_ICONS, sheetMenu, type SheetAction } from './sheetMenu';
import { fitCells } from './tableFit';

const ARROWS: Record<string, Direction> = { ArrowUp: 'up', ArrowDown: 'down', ArrowLeft: 'left', ArrowRight: 'right' };
const HISTORY = 100;

export function TableSheet({
  data,
  s,
  texts,
  onTexts,
  busy,
  hidden,
  style,
  menuHost,
}: {
  data: CaptureTable;
  /** 图上一个 CSS 像素是几个图的像素 */
  s: number;
  texts: string[];
  onTexts: (next: string[]) => void;
  busy?: boolean;
  /** 看原图：表格藏起来，点击也穿过去 */
  hidden?: boolean;
  /** 摆在哪、多大（CSS 像素） */
  style: React.CSSProperties;
  /**
   * 右键菜单交给外面显示（贴图：窗口小，用单独的菜单窗口），选了哪项调 `run(id)`；
   * 不给就在页面里弹（截图里）
   */
  menuHost?: (items: PopItem[], run: (id: string) => void) => void;
}) {
  const { t } = useTranslation();
  const cells = data.table.cells;
  const { rows, cols } = data.table;
  const owner = useMemo(() => ownerMap(cells, rows, cols), [cells, rows, cols]);
  const fits = useMemo(() => fitCells(cells, s, !!data.plain), [cells, s, data.plain]);
  const layer = useRef<HTMLDivElement>(null);
  const editor = useRef<HTMLTextAreaElement>(null);

  // 当前格（光标所在）和选区的另一头；null = 没在操作这张表
  const [cursor, setCursor] = useState<Pos | null>(null);
  const [anchor, setAnchor] = useState<Pos | null>(null);
  const [focused, setFocused] = useState(false);
  // 正在改的格子：replace = 打字进来的（方向键直接换格）；否则是双击 / F2（方向键在字里移光标）
  const [editing, setEditing] = useState<{ index: number; replace: boolean } | null>(null);
  const [draft, setDraft] = useState('');
  // 右键菜单开着：焦点在菜单上，选区照样画着
  const [menuOpen, setMenuOpen] = useState(false);
  const dragging = useRef(false);
  const undo = useRef<string[][]>([]);
  const redo = useRef<string[][]>([]);

  // 换了一张表（AI 重新认过）：选区、历史都作废
  useEffect(() => {
    setCursor(null);
    setAnchor(null);
    setEditing(null);
    undo.current = [];
    redo.current = [];
  }, [cells]);

  const range: Range | null = cursor && anchor ? rangeOf(anchor, cursor, cells, owner) : null;
  const activeIndex = cursor ? (owner[cursor.r]?.[cursor.c] ?? -1) : -1;
  const active = cells[activeIndex];

  const apply = (next: string[]) => {
    if (next.every((v, i) => v === texts[i])) return;
    undo.current = [...undo.current.slice(-HISTORY + 1), texts];
    redo.current = [];
    onTexts(next);
  };

  const focusEditor = () => editor.current?.focus({ preventScroll: true });

  const select = (pos: Pos, extend = false) => {
    setCursor(pos);
    if (!extend) setAnchor(pos);
  };

  // ── 改字 ──
  const beginEdit = (index: number, replace: boolean, initial?: string) => {
    setEditing({ index, replace });
    setDraft(initial ?? (replace ? '' : (texts[index] ?? '')));
    requestAnimationFrame(() => {
      const el = editor.current;
      if (!el) return;
      el.focus({ preventScroll: true });
      el.setSelectionRange(el.value.length, el.value.length);
    });
  };
  const commit = () => {
    if (!editing) return;
    const value = draft.replace(/[ \t]+\n/g, '\n').trim();
    apply(texts.map((v, i) => (i === editing.index ? value : v)));
    setEditing(null);
    setDraft('');
  };
  const cancelEdit = () => {
    setEditing(null);
    setDraft('');
  };

  // ── 鼠标 ──
  const posAt = (e: React.PointerEvent | React.MouseEvent): Pos | null => {
    const box = layer.current?.getBoundingClientRect();
    if (!box) return null;
    const x = (e.clientX - box.left) * s;
    const y = (e.clientY - box.top) * s;
    const i = cells.findIndex((c) => x >= c.x && x < c.x + c.width && y >= c.y && y < c.y + c.height);
    if (i < 0) return null;
    // 合并格子：落在它里面的哪个位置
    const c = cells[i]!;
    const r = Math.min(c.row + c.rowSpan - 1, c.row + Math.floor(((y - c.y) / c.height) * c.rowSpan));
    const col = Math.min(c.col + c.colSpan - 1, c.col + Math.floor(((x - c.x) / c.width) * c.colSpan));
    return { r, c: col };
  };
  const onPointerDown = (e: React.PointerEvent) => {
    e.stopPropagation();
    if (e.button !== 0) return;
    if (menuHost) setMenuOpen(false);
    const pos = posAt(e);
    if (!pos) return;
    if (editing) {
      // 点的是正在改的这一格：交给输入框自己放光标
      if ((owner[pos.r]?.[pos.c] ?? -1) === editing.index) return;
      commit();
    }
    e.preventDefault();
    select(pos, e.shiftKey && !!cursor);
    dragging.current = true;
    (e.currentTarget as Element).setPointerCapture?.(e.pointerId);
    focusEditor();
  };
  const onPointerMove = (e: React.PointerEvent) => {
    if (!dragging.current) return;
    const pos = posAt(e);
    if (pos && (pos.r !== cursor?.r || pos.c !== cursor?.c)) setCursor(pos);
  };
  const onPointerUp = () => {
    dragging.current = false;
  };
  const onDoubleClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    const pos = posAt(e);
    const i = pos ? (owner[pos.r]?.[pos.c] ?? -1) : -1;
    if (i >= 0) {
      select(startOf(cells[i]!));
      beginEdit(i, false);
    }
  };

  // ── 复制粘贴 ──
  /** 复制选区：`rich` = 制表符分隔 + 带样式的 HTML 表格（Excel / Word 里格子、样子都在）；`text` = 只要字；`markdown` */
  const copyRange = async (cut: boolean, as: 'rich' | 'text' | 'markdown' = 'rich') => {
    if (!range) return;
    const grid = rangeGrid(range, cells, owner, texts);
    try {
      if (as === 'rich') await capture.writeRich(toExcelText(grid), toHtmlTable(rangeRows(range, cells, texts, true)));
      else await capture.writeText(as === 'text' ? toTsv(grid) : toMarkdown(grid));
      if (cut) clearRange();
      notify.success(t('capture.sheet.copied', { count: cellsIn(range, cells).length }));
    } catch (err) {
      notify.error(err);
    }
  };
  const clearRange = () => {
    if (!range) return;
    const clear = new Set(cellsIn(range, cells));
    apply(texts.map((v, i) => (clear.has(i) ? '' : v)));
  };
  const undoOnce = () => {
    const prev = undo.current.pop();
    if (!prev) return;
    redo.current.push(texts);
    onTexts(prev);
  };
  const redoOnce = () => {
    const next = redo.current.pop();
    if (!next) return;
    undo.current.push(texts);
    onTexts(next);
  };
  const selectAll = () => {
    setAnchor({ r: 0, c: 0 });
    setCursor({ r: rows - 1, c: cols - 1 });
  };
  const pasteHere = async () => {
    if (!cursor) return;
    try {
      const text = await capture.readText();
      if (!text) return;
      const at = range ? { r: range.r0, c: range.c0 } : cursor;
      apply(paste(at, parseTsv(text), cells, owner, texts, range ?? undefined));
    } catch (err) {
      notify.error(err);
    }
  };

  // ── 键盘 ──
  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // 截图界面的快捷键（撤销标注、回车完成、Esc 退出……）不要收到这里的按键
    e.stopPropagation();
    if (e.nativeEvent.isComposing) return;
    const mod = e.ctrlKey || e.metaKey;
    if (editing) {
      if (e.key === 'Enter' && !e.altKey) {
        e.preventDefault();
        commit();
        if (cursor) select(move(cursor, e.shiftKey ? 'up' : 'down', cells, owner));
      } else if (e.key === 'Enter' && e.altKey) {
        e.preventDefault();
        const el = e.currentTarget;
        const at = el.selectionStart;
        setDraft(`${draft.slice(0, at)}\n${draft.slice(el.selectionEnd)}`);
        requestAnimationFrame(() => el.setSelectionRange(at + 1, at + 1));
      } else if (e.key === 'Tab') {
        e.preventDefault();
        commit();
        if (cursor) select(tabMove(cursor, e.shiftKey, cells, owner));
      } else if (e.key === 'Escape') {
        e.preventDefault();
        cancelEdit();
      } else if (editing.replace && ARROWS[e.key]) {
        e.preventDefault();
        commit();
        if (cursor) select(move(cursor, ARROWS[e.key]!, cells, owner));
      }
      return;
    }
    if (!cursor) return;
    const dir = ARROWS[e.key];
    if (dir) {
      e.preventDefault();
      const next = move(cursor, dir, cells, owner);
      select(next, e.shiftKey);
    } else if (e.key === 'Tab') {
      e.preventDefault();
      select(tabMove(cursor, e.shiftKey, cells, owner));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      select(move(cursor, e.shiftKey ? 'up' : 'down', cells, owner));
    } else if (e.key === 'F2') {
      e.preventDefault();
      if (activeIndex >= 0) beginEdit(activeIndex, false);
    } else if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      clearRange();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      // 选了一片：先收成一格；只有一格：放开这张表（再按 Esc 就是退出截图了）
      if (range && (range.r0 !== range.r1 || range.c0 !== range.c1)) setAnchor(cursor);
      else {
        setCursor(null);
        setAnchor(null);
        editor.current?.blur();
      }
    } else if (mod && e.key.toLowerCase() === 'a') {
      e.preventDefault();
      selectAll();
    } else if (mod && e.key.toLowerCase() === 'c') {
      e.preventDefault();
      void copyRange(false);
    } else if (mod && e.key.toLowerCase() === 'x') {
      e.preventDefault();
      void copyRange(true);
    } else if (mod && e.key.toLowerCase() === 'v') {
      e.preventDefault();
      void pasteHere();
    } else if (mod && e.key.toLowerCase() === 'z' && !e.shiftKey) {
      e.preventDefault();
      undoOnce();
    } else if (mod && (e.key.toLowerCase() === 'y' || (e.key.toLowerCase() === 'z' && e.shiftKey))) {
      e.preventDefault();
      redoOnce();
    }
    // 其余的键（字母、数字、标点……）让它打进输入框：onChange 里开始改这一格
  };
  const onChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const value = e.currentTarget.value;
    if (editing) setDraft(value);
    else if (activeIndex >= 0 && value) beginEdit(activeIndex, true, value);
  };
  const onCompositionStart = () => {
    // 输入法开始拼字：这一格进入"换掉重打"
    if (!editing && activeIndex >= 0) {
      setEditing({ index: activeIndex, replace: true });
      setDraft('');
    }
  };

  // ── 右键菜单 ──
  const run = (id: SheetAction) => {
    setMenuOpen(false);
    if (id === 'cut') void copyRange(true);
    else if (id === 'copy') void copyRange(false);
    else if (id === 'copyText') void copyRange(false, 'text');
    else if (id === 'copyMarkdown') void copyRange(false, 'markdown');
    else if (id === 'paste') void pasteHere();
    else if (id === 'clear') clearRange();
    else if (id === 'selectRow' && range) {
      setAnchor({ r: range.r0, c: 0 });
      setCursor({ r: range.r1, c: cols - 1 });
    } else if (id === 'selectCol' && range) {
      setAnchor({ r: 0, c: range.c0 });
      setCursor({ r: rows - 1, c: range.c1 });
    } else if (id === 'selectAll') selectAll();
    else if (id === 'undo') undoOnce();
    else if (id === 'redo') redoOnce();
    focusEditor();
  };
  // 贴图的菜单在另一个窗口里，选了哪项要过一会儿才回来：那时候用最新的选区（右键刚选中的那一格）
  const runLatest = useRef(run);
  runLatest.current = run;
  const label = (id: SheetAction) => t(`capture.sheet.menu.${id}`);
  const menuState = (hasRange: boolean) => ({ hasRange, canUndo: undo.current.length > 0, canRedo: redo.current.length > 0 });
  /** 右键：点在选区外面先选中那一格（和 Excel 一样），点在选区里面就对整个选区 */
  const onContextMenu = (e: React.MouseEvent) => {
    // 别让截图把右键当成退出；页面里的菜单（Radix）自己会拦下浏览器的右键菜单
    e.stopPropagation();
    if (editing) commit();
    const pos = posAt(e);
    const inRange = !!pos && !!range && pos.r >= range.r0 && pos.r <= range.r1 && pos.c >= range.c0 && pos.c <= range.c1;
    if (pos && !inRange) select(pos);
    if (!menuHost) return;
    e.preventDefault();
    // 菜单在另一个窗口里，这边失了焦点：选区照样画着，选完一项或再点表格再说
    setMenuOpen(true);
    // 刚选中的格子这一下还没进 state：菜单项能不能点按"有选区"算
    const items: PopItem[] = sheetMenu(menuState(!!(pos ?? range))).map((it) =>
      it
        ? { kind: 'item', id: `sheet:${it.id}`, label: label(it.id), icon: it.icon, shortcut: it.shortcut && shortcutLabel(it.shortcut), disabled: it.disabled }
        : { kind: 'separator' },
    );
    menuHost(items, (id) => runLatest.current(id.replace(/^sheet:/, '') as SheetAction));
  };
  const menuItems: MenuItemSpec[] = sheetMenu(menuState(!!range)).map((it) =>
    it ? { label: label(it.id), icon: SHEET_ICONS[it.icon], shortcut: it.shortcut, disabled: it.disabled, onSelect: () => run(it.id) } : { separator: true },
  );

  // ── 画 ──
  const rectOf = (indices: number[]) => {
    if (!indices.length) return null;
    const x0 = Math.min(...indices.map((i) => cells[i]!.x));
    const y0 = Math.min(...indices.map((i) => cells[i]!.y));
    const x1 = Math.max(...indices.map((i) => cells[i]!.x + cells[i]!.width));
    const y1 = Math.max(...indices.map((i) => cells[i]!.y + cells[i]!.height));
    return { left: x0 / s, top: y0 / s, width: (x1 - x0) / s, height: (y1 - y0) / s };
  };
  const shown = focused || menuOpen;
  const rangeBox = range && shown ? rectOf(cellsIn(range, cells)) : null;
  const multi = !!range && (range.r0 !== range.r1 || range.c0 !== range.c1);
  const activeBox = active && shown ? rectOf([activeIndex]) : null;
  const editingCell = editing ? cells[editing.index] : undefined;
  const editFit = editing ? fits[editing.index] : undefined;
  const editorStyle: React.CSSProperties =
    editingCell && editFit
      ? {
          left: editingCell.x / s,
          top: editingCell.y / s,
          minWidth: editingCell.width / s,
          height: editingCell.height / s,
          fontFamily: editFit.style.fontFamily,
          fontStyle: editFit.style.fontStyle,
          fontWeight: editFit.style.fontWeight,
          fontSize: editFit.font,
          color: editingCell.color,
          background: editingCell.background,
          textAlign: editingCell.align,
          lineHeight: `${Math.max(editFit.font * 1.3, editingCell.height / s - 6)}px`,
        }
      : activeBox
        ? { left: activeBox.left, top: activeBox.top, width: activeBox.width, height: activeBox.height }
        : { left: 0, top: 0, width: 1, height: 1 };

  const sheet = (
    <div
      ref={layer}
      className={clsx('cap-inplace cap-sheet', busy && 'cap-inplace--busy', data.plain && 'cap-inplace--plain')}
      data-peek={hidden || undefined}
      style={{ ...style, '--plain-line': data.plain?.line } as React.CSSProperties}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onDoubleClick={onDoubleClick}
      // 贴图里：按下鼠标是拖窗口，在表格上按下不要拖
      onMouseDown={(e) => e.stopPropagation()}
      onContextMenu={onContextMenu}
    >
      {data.background && <img className="cap-inplace__bg" src={shotUrl(data.background)} alt="" draggable={false} />}
      {cells.map((c, i) => (
        <div
          key={i}
          className="cap-inplace__cell"
          style={{ left: c.x / s, top: c.y / s, width: c.width / s, height: c.height / s, '--cell-bg': c.background } as React.CSSProperties}
        >
          {editing?.index !== i && (
            <div className="cap-inplace__text" style={fits[i]!.style}>
              {texts[i] ?? c.text}
            </div>
          )}
        </div>
      ))}
      {data.plain && <div className="cap-inplace__frame" style={{ left: data.plain.x / s, top: data.plain.y / s, width: data.plain.width / s, height: data.plain.height / s }} />}
      {rangeBox && multi && <div className="cap-sheet__range" style={rangeBox} />}
      {activeBox && !editing && <div className="cap-sheet__active" style={activeBox} />}
      <textarea
        ref={editor}
        className="cap-sheet__editor"
        data-editing={editing ? true : undefined}
        style={editorStyle}
        value={editing ? draft : ''}
        spellCheck={false}
        aria-label={t('capture.sheet.editor')}
        onChange={onChange}
        onKeyDown={onKeyDown}
        onCompositionStart={onCompositionStart}
        onFocus={() => setFocused(true)}
        onBlur={() => {
          setFocused(false);
          if (editing) commit();
        }}
        onPointerDown={(e) => e.stopPropagation()}
      />
    </div>
  );
  if (menuHost) return sheet;
  return (
    <ContextMenu items={menuItems} onOpenChange={setMenuOpen} onClosed={focusEditor}>
      {sheet}
    </ContextMenu>
  );
}
