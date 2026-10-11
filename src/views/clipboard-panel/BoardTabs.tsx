// 剪贴板面板顶上的分组标签（参照 Paste 的"图钉板"）：第一个是"剪贴板"（全部历史），后面每个分组一个带色点的标签，
// 最后一个"+"新建分组。卡片拖到标签上就归进那个分组（拖到"剪贴板"上 = 移出分组）；右键分组标签改名、换颜色、删除。

import clsx from 'clsx';
import { History, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { clipboard } from '@/lib/ipc';
import type { ClipGroup } from '@/lib/types';
import { popupMenu } from '@/ui/nativeMenu';
import { confirmDialog, notify, promptDialog } from '@/ui/overlays';

/** 分组可选的颜色（苹果的系统色），新建时按顺序轮着给 */
export const BOARD_COLORS = ['red', 'orange', 'yellow', 'green', 'blue', 'purple', 'pink', 'gray'] as const;
const HEX: Record<(typeof BOARD_COLORS)[number], string> = {
  red: '#ff3b30',
  orange: '#ff9500',
  yellow: '#f7c600',
  green: '#34c759',
  blue: '#007aff',
  purple: '#af52de',
  pink: '#ff2d55',
  gray: '#8e8e93',
};

/** 分组的颜色：存的是 `#rrggbb`；没存过的（主窗口里建的老分组）按 ID 轮一个 */
export function boardColor(group: Pick<ClipGroup, 'id' | 'color'>): string {
  if (group.color && /^#[0-9a-f]{6}$/i.test(group.color)) return group.color;
  return HEX[BOARD_COLORS[group.id % BOARD_COLORS.length]!];
}

/** 拖卡片时放在 dataTransfer 里的类型 */
export const CLIP_DRAG_TYPE = 'application/x-dato-clip';

export function BoardTabs({
  groups,
  board,
  onSelect,
  onMenuClosed,
}: {
  groups: ClipGroup[];
  /** 当前分组；null = 剪贴板（全部历史） */
  board: number | null;
  onSelect: (board: number | null) => void;
  /** 原生菜单 / 对话框关掉之后（面板要盯一下焦点，见 PanelView 的 watchFocus） */
  onMenuClosed: () => void;
}) {
  const { t } = useTranslation();
  // 拖着卡片悬停在哪个标签上（高亮）；'clipboard' = 剪贴板
  const [over, setOver] = useState<number | 'clipboard' | null>(null);

  const dropProps = (target: number | null) => ({
    onDragOver: (e: React.DragEvent) => {
      if (!e.dataTransfer.types.includes(CLIP_DRAG_TYPE)) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      setOver(target ?? 'clipboard');
    },
    onDragLeave: () => setOver(null),
    onDrop: (e: React.DragEvent) => {
      setOver(null);
      const id = Number(e.dataTransfer.getData(CLIP_DRAG_TYPE));
      if (!id) return;
      e.preventDefault();
      const name = groups.find((g) => g.id === target)?.name;
      void clipboard
        .setGroup([id], target)
        .then(() => notify.success(name ? t('clip.board.added', { name }) : t('clip.board.removed')))
        .catch(notify.error);
    },
  });

  const create = async () => {
    const name = await promptDialog({ title: t('clip.board.new'), placeholder: t('clip.board.namePlaceholder') });
    onMenuClosed();
    if (!name?.trim()) return;
    const used = new Set(groups.map((g) => boardColor(g)));
    const color = Object.values(HEX).find((c) => !used.has(c)) ?? HEX[BOARD_COLORS[groups.length % BOARD_COLORS.length]!];
    try {
      const id = await clipboard.createGroup(name.trim(), color);
      onSelect(id);
    } catch (err) {
      notify.error(err);
    }
  };

  const groupMenu = (g: ClipGroup) => {
    const current = boardColor(g);
    void popupMenu([
      {
        label: t('clip.board.rename'),
        onSelect: () =>
          void promptDialog({ title: t('clip.board.rename'), initial: g.name })
            .then((name) => (name?.trim() && name.trim() !== g.name ? clipboard.renameGroup(g.id, name.trim()) : undefined))
            .catch(notify.error)
            .finally(onMenuClosed),
      },
      {
        label: t('clip.board.color'),
        submenu: BOARD_COLORS.map((c) => ({
          label: t(`clip.board.colors.${c}`),
          checked: HEX[c] === current,
          onSelect: () => void clipboard.setGroupColor(g.id, HEX[c]).catch(notify.error),
        })),
      },
      { separator: true },
      {
        label: t('clip.board.delete'),
        onSelect: () =>
          void confirmDialog({ title: t('clip.board.deleteAsk', { name: g.name }), body: t('clip.board.deleteBody'), danger: true })
            .then((ok) => {
              if (!ok) return;
              if (board === g.id) onSelect(null);
              return clipboard.deleteGroup(g.id);
            })
            .catch(notify.error)
            .finally(onMenuClosed),
      },
    ]).finally(onMenuClosed);
  };

  return (
    <nav className="boards">
      <button
        type="button"
        className={clsx('boards__tab', board === null && 'boards__tab--active', over === 'clipboard' && 'boards__tab--over')}
        onClick={() => onSelect(null)}
        {...dropProps(null)}
      >
        <History size={13} strokeWidth={2} />
        {t('clip.board.clipboard')}
      </button>
      {groups.map((g) => (
        <button
          key={g.id}
          type="button"
          className={clsx('boards__tab', board === g.id && 'boards__tab--active', over === g.id && 'boards__tab--over')}
          title={t('clip.board.count', { count: g.count })}
          onClick={() => onSelect(g.id)}
          onContextMenu={(e) => {
            e.preventDefault();
            groupMenu(g);
          }}
          {...dropProps(g.id)}
        >
          <span className="boards__dot" style={{ background: boardColor(g) }} />
          {g.name}
        </button>
      ))}
      <button type="button" className="boards__add" title={t('clip.board.new')} onClick={() => void create()}>
        <Plus size={14} strokeWidth={2} />
      </button>
    </nav>
  );
}
