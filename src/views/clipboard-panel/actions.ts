// 剪贴板条目的右键菜单（规格 05 §3.3）。面板用原生菜单，主窗口用网页菜单，共用这份定义。

import type { TFunction } from 'i18next';

import { clipboard, ocr } from '@/lib/ipc';
import type { ClipGroup, ClipItem } from '@/lib/types';
import { notify, promptDialog } from '@/ui/overlays';
import type { NativeItem } from '@/ui/nativeMenu';

export function clipMenu(
  item: ClipItem,
  t: TFunction,
  opts: {
    groups: ClipGroup[];
    onTranslate?: (item: ClipItem) => void;
    inPanel: boolean;
    /** 面板里正在看某个分组：可以给卡片起名（显示在卡片标题栏上，存在备注里） */
    inBoard?: boolean;
    /** 菜单里弹的对话框关掉之后（面板要盯一下焦点） */
    onDialogClosed?: () => void;
  },
): NativeItem[] {
  const run = (p: Promise<unknown>) => void p.catch(notify.error);
  const items: NativeItem[] = [
    { label: t('clip.paste'), onSelect: () => run(clipboard.paste(item.id)) },
  ];
  if (item.type === 'text' && item.hasHtml) {
    items.push({ label: t('clip.pastePlain'), onSelect: () => run(clipboard.paste(item.id, true)) });
  }
  items.push(
    {
      label: t('clip.copy'),
      onSelect: () => run(clipboard.copy(item.id).then(() => notify.success(t('clip.copied')))),
    },
    { separator: true },
    { label: t('clip.pin'), checked: item.pinned, onSelect: () => run(clipboard.setPinned(item.id, !item.pinned)) },
    { label: t('clip.favorite'), checked: item.favorite, onSelect: () => run(clipboard.setFavorite(item.id, !item.favorite)) },
  );
  if (opts.inBoard) {
    items.push({
      label: t('clip.board.nameCard'),
      onSelect: () =>
        run(
          promptDialog({ title: t('clip.board.nameCard'), initial: item.note ?? '', placeholder: t('clip.board.nameCardPlaceholder') })
            .then((note) => (note === null ? undefined : clipboard.setNote(item.id, note.trim() || null)))
            .finally(() => opts.onDialogClosed?.()),
        ),
    });
  } else if (!opts.inPanel) {
    items.push({
      label: t('clip.note'),
      onSelect: () =>
        run(
          promptDialog({ title: t('clip.note'), initial: item.note ?? '', placeholder: t('clip.notePlaceholder') }).then((note) =>
            note === null ? undefined : clipboard.setNote(item.id, note || null),
          ),
        ),
    });
  }
  if (opts.groups.length > 0) {
    items.push({
      label: t('clip.moveToGroup'),
      submenu: [
        { label: t('clip.noGroup'), checked: item.groupId === null, onSelect: () => run(clipboard.setGroup([item.id], null)) },
        { separator: true },
        ...opts.groups.map((g) => ({
          label: g.name,
          checked: item.groupId === g.id,
          onSelect: () => run(clipboard.setGroup([item.id], g.id)),
        })),
      ],
    });
  }
  items.push({ separator: true });
  if ((item.type === 'text' || item.type === 'link') && opts.onTranslate) {
    items.push({ label: t('clip.translate'), onSelect: () => opts.onTranslate?.(item) });
  }
  if (item.type === 'image') {
    items.push({ label: t('clip.ocr'), onSelect: () => run(ocr.fromClip(item.id)) });
    items.push({ label: t('clip.pinToScreen'), onSelect: () => run(clipboard.pinImage(item.id)) });
    items.push({ label: t('clip.reveal'), onSelect: () => run(clipboard.open(item.id)) });
  }
  if (item.type === 'files') items.push({ label: t('clip.reveal'), onSelect: () => run(clipboard.open(item.id)) });
  if (item.type === 'link') items.push({ label: t('clip.openLink'), onSelect: () => run(clipboard.open(item.id)) });
  items.push({ separator: true }, { label: t('common.delete'), onSelect: () => run(clipboard.remove([item.id])) });
  return items;
}
