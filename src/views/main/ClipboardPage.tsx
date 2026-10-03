// 剪贴板管理页（规格 05 §4）：面板是快速取用，这里是管理 —— 列表 + 详情、分组、备注、清理。

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useVirtualizer } from '@tanstack/react-virtual';
import { Clipboard, Copy, File, FolderPlus, Globe, Image as ImageIcon, Languages, MoreHorizontal, Palette, Pin, ScanText, Star, Trash2, Type } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { formatBytes, formatDateTime, relativeTime } from '@/lib/format';
import { clipboard, ocr, system } from '@/lib/ipc';
import { assetUrl } from '@/lib/platform';
import { useClipItems } from '@/lib/queries';
import type { ClipItem, ClipType } from '@/lib/types';
import { Button, EmptyState, IconButton, SearchField, Select, Spinner } from '@/ui/controls';
import { ContextMenu, DropdownMenu, confirmDialog, notify, promptDialog, type MenuItemSpec } from '@/ui/overlays';
import { clipMenu } from '@/views/clipboard-panel/actions';
import { TranslateBox } from '@/views/translate/TranslateBox';

type Filter = 'all' | ClipType | 'pinned' | 'favorite';
const FILTERS: Filter[] = ['all', 'text', 'image', 'link', 'files', 'pinned', 'favorite'];

const typeIcon: Record<ClipType, typeof Type> = { text: Type, link: Globe, color: Palette, image: ImageIcon, files: File };

export function ClipboardPage() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [keyword, setKeyword] = useState('');
  const [debounced, setDebounced] = useState('');
  const [filter, setFilter] = useState<Filter>('all');
  const [group, setGroup] = useState<string>('all');
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const id = window.setTimeout(() => setDebounced(keyword.trim()), 200);
    return () => window.clearTimeout(id);
  }, [keyword]);

  const stats = useQuery({ queryKey: ['clip-stats'], queryFn: clipboard.stats });
  const groups = useQuery({ queryKey: ['clip-groups'], queryFn: clipboard.groups });
  const query = useClipItems(
    useMemo(
      () => ({
        keyword: debounced || undefined,
        kinds: filter === 'text' ? (['text', 'color'] as ClipType[]) : ['image', 'link', 'files'].includes(filter) ? [filter as ClipType] : [],
        pinnedOnly: filter === 'pinned',
        favoriteOnly: filter === 'favorite',
        groupId: group === 'all' ? null : Number(group),
      }),
      [debounced, filter, group],
    ),
    80,
  );
  const items = useMemo(() => query.data?.pages.flatMap((p) => p.items) ?? [], [query.data]);
  const selectedIndex = items.findIndex((i) => i.id === selectedId);
  const selected = selectedIndex >= 0 ? items[selectedIndex] : undefined;

  const virt = useVirtualizer({
    count: items.length,
    getScrollElement: () => listRef.current,
    estimateSize: () => 54,
    overscan: 8,
  });
  const vItems = virt.getVirtualItems();
  const last = vItems[vItems.length - 1]?.index ?? 0;
  useEffect(() => {
    if (last >= items.length - 10 && query.hasNextPage && !query.isFetchingNextPage) void query.fetchNextPage();
  }, [last, items.length, query]);

  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['clips'] });
    void qc.invalidateQueries({ queryKey: ['clip-stats'] });
    void qc.invalidateQueries({ queryKey: ['clip-groups'] });
  };
  const run = (p: Promise<unknown>) => void p.then(refresh).catch(notify.error);

  const createGroup = async () => {
    const name = await promptDialog({ title: t('clip.newGroup'), placeholder: t('clip.groupName') });
    if (name?.trim()) run(clipboard.createGroup(name.trim()));
  };

  const clear = async (scope: 'unpinned' | 'everything') => {
    const ok = await confirmDialog({
      title: scope === 'everything' ? t('clip.clearAllTitle') : t('clip.clearTitle'),
      body: scope === 'everything' ? t('clip.clearAllBody') : t('clip.clearBody'),
      confirmLabel: t('clip.clear'),
      danger: true,
    });
    if (ok) {
      clipboard
        .clear(scope)
        .then((n) => {
          notify.success(t('clip.cleared', { count: n }));
          refresh();
        })
        .catch(notify.error);
    }
  };

  const moreMenu: MenuItemSpec[] = [
    { label: t('clip.newGroup'), icon: FolderPlus, onSelect: () => void createGroup() },
    ...(group !== 'all'
      ? [
          {
            label: t('clip.renameGroup'),
            onSelect: () => {
              const g = groups.data?.find((x) => String(x.id) === group);
              if (!g) return;
              void promptDialog({ title: t('clip.renameGroup'), initial: g.name }).then((n) => n?.trim() && run(clipboard.renameGroup(g.id, n.trim())));
            },
          },
          {
            label: t('clip.deleteGroup'),
            danger: true,
            onSelect: () => {
              run(clipboard.deleteGroup(Number(group)));
              setGroup('all');
            },
          },
        ]
      : []),
    { separator: true },
    { label: t('clip.clearKeep'), onSelect: () => void clear('unpinned') },
    { label: t('clip.clearAll'), danger: true, onSelect: () => void clear('everything') },
    { separator: true },
    { label: t('settings.openData'), onSelect: () => void system.openFolder('data') },
  ];

  const toMenuSpec = (item: ClipItem): MenuItemSpec[] =>
    clipMenu(item, t, { groups: groups.data ?? [], inPanel: false }).map((m) => ({
      ...m,
      submenu: m.submenu?.map((s) => ({ ...s })),
    }));

  return (
    <section className="page">
      <div className="page__head">
        <div>
          <h1 className="page__title">{t('nav.clipboard')}</h1>
          <div className="page__sub cn-numeric">
            {stats.data ? t('clip.stats', { total: stats.data.total.toLocaleString(), pinned: stats.data.pinned, favorite: stats.data.favorite }) : ' '}
          </div>
        </div>
        <span className="page__spacer" />
        <SearchField value={keyword} onChange={setKeyword} placeholder={t('clip.search')} style={{ width: 240 }} />
        <DropdownMenu items={moreMenu}>
          <IconButton icon={MoreHorizontal} label={t('common.more')} />
        </DropdownMenu>
      </div>
      <div className="page__head" style={{ paddingBottom: 'var(--cn-space-3)' }}>
        <div className="chips">
          {FILTERS.map((f) => (
            <button key={f} type="button" className="chip" data-active={filter === f} onClick={() => setFilter(f)}>
              {t(`clip.filter.${f}`)}
            </button>
          ))}
        </div>
        <span className="page__spacer" />
        <Select
          value={group}
          width={140}
          options={[{ value: 'all', label: t('clip.allGroups') }, ...(groups.data ?? []).map((g) => ({ value: String(g.id), label: `${g.name} (${g.count})` }))]}
          onChange={setGroup}
        />
      </div>
      <div className="clip-page">
        <div
          ref={listRef}
          className="clip-list"
          tabIndex={0}
          onKeyDown={(e) => {
            if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
              e.preventDefault();
              const next = Math.max(0, Math.min(items.length - 1, selectedIndex + (e.key === 'ArrowDown' ? 1 : -1)));
              const it = items[next];
              if (it) {
                setSelectedId(it.id);
                virt.scrollToIndex(next);
              }
            } else if (e.key === 'Delete' && selected) {
              run(clipboard.remove([selected.id]));
            } else if (e.key === 'Enter' && selected) {
              run(clipboard.copy(selected.id).then(() => notify.success(t('clip.copied'))));
            }
          }}
        >
          {query.isLoading ? (
            <div className="load-more">
              <Spinner size={18} />
            </div>
          ) : items.length === 0 ? (
            <EmptyState icon={Clipboard} title={debounced ? t('clip.noMatch') : t('clip.empty')} description={debounced ? undefined : t('clip.emptyDesc')} />
          ) : (
            <div style={{ height: virt.getTotalSize(), position: 'relative' }}>
              {vItems.map((v) => {
                const it = items[v.index]!;
                const Icon = typeIcon[it.type];
                return (
                  <ContextMenu key={it.id} items={toMenuSpec(it)}>
                    <div
                      className="clip-row"
                      data-selected={it.id === selectedId}
                      style={{ transform: `translateY(${v.start}px)` }}
                      onMouseDown={() => setSelectedId(it.id)}
                      onDoubleClick={() => run(clipboard.copy(it.id).then(() => notify.success(t('clip.copied'))))}
                    >
                      <span className="clip-row__icon" style={it.type === 'color' ? { background: it.preview ?? undefined } : undefined}>
                        {it.type === 'image' ? <img src={assetUrl(it.thumbPath)} alt="" loading="lazy" /> : it.type !== 'color' && <Icon size={16} strokeWidth={1.5} />}
                      </span>
                      <div className="clip-row__main">
                        <div className="clip-row__text">
                          {it.type === 'image'
                            ? t('clip.imageLabel', { w: it.width ?? 0, h: it.height ?? 0 })
                            : it.type === 'files'
                              ? (it.preview ?? '').split('\n').join('、')
                              : (it.preview ?? '').replace(/\s+/g, ' ')}
                        </div>
                        <div className="clip-row__sub">
                          {it.sourceIcon && <img src={assetUrl(it.sourceIcon)} alt="" />}
                          <span>{it.sourceApp ?? t('clip.unknownApp')}</span>
                          <span>·</span>
                          <span>{relativeTime(it.lastUsedAt)}</span>
                          {it.note && (
                            <>
                              <span>·</span>
                              <span className="cn-truncate">{it.note}</span>
                            </>
                          )}
                        </div>
                      </div>
                      <span className="clip-row__flags">
                        {it.pinned && <Pin size={12} strokeWidth={2} />}
                        {it.favorite && <Star size={12} strokeWidth={2} fill="currentColor" />}
                      </span>
                    </div>
                  </ContextMenu>
                );
              })}
            </div>
          )}
        </div>
        <ClipDetailPane item={selected} onChanged={refresh} />
      </div>
    </section>
  );
}

function ClipDetailPane({ item, onChanged }: { item: ClipItem | undefined; onChanged: () => void }) {
  const { t } = useTranslation();
  const [translating, setTranslating] = useState(false);
  const detail = useQuery({
    queryKey: ['clip-detail', item?.id, item?.lastUsedAt, item?.note],
    queryFn: () => clipboard.get(item!.id),
    enabled: !!item,
  });
  const [note, setNote] = useState('');
  useEffect(() => {
    setNote(item?.note ?? '');
    setTranslating(false);
  }, [item?.id, item?.note]);

  if (!item) {
    return (
      <aside className="clip-detail">
        <EmptyState icon={Clipboard} title={t('clip.selectHint')} />
      </aside>
    );
  }
  const d = detail.data;
  const run = (p: Promise<unknown>) => void p.then(onChanged).catch(notify.error);
  return (
    <aside className="clip-detail">
      <div className="clip-detail__content cn-selectable">
        {item.type === 'image' ? (
          <img src={assetUrl(item.filePath)} alt="" />
        ) : item.type === 'files' ? (
          item.files.map((f) => <div key={f}>{f}</div>)
        ) : item.type === 'color' ? (
          <div style={{ height: 120, borderRadius: 8, background: item.preview ?? undefined }} />
        ) : (
          (d?.contentText ?? item.preview)
        )}
        {translating && d?.contentText && (
          <div style={{ marginTop: 12, borderTop: '0.5px solid var(--cn-separator)', minHeight: 180 }}>
            <TranslateBox text={d.contentText} compact />
          </div>
        )}
      </div>
      <div className="clip-detail__meta">
        <dl>
          <dt>{t('clip.meta.type')}</dt>
          <dd>{t(`clip.type.${item.type}`)}</dd>
          {item.charCount != null && item.type !== 'files' && (
            <>
              <dt>{t('clip.meta.chars')}</dt>
              <dd className="cn-numeric">{item.charCount.toLocaleString()}</dd>
            </>
          )}
          {item.sizeBytes != null && (
            <>
              <dt>{t('clip.meta.size')}</dt>
              <dd>{formatBytes(item.sizeBytes)}</dd>
            </>
          )}
          <dt>{t('clip.meta.source')}</dt>
          <dd title={d?.sourceAppPath ?? ''}>{item.sourceApp ?? t('clip.unknownApp')}</dd>
          <dt>{t('clip.meta.created')}</dt>
          <dd>{formatDateTime(item.createdAt)}</dd>
          <dt>{t('clip.meta.used')}</dt>
          <dd>{formatDateTime(item.lastUsedAt)}</dd>
        </dl>
        {item.truncated && <p className="set-note">{t('clip.truncated')}</p>}
        <textarea
          className="cn-textarea"
          style={{ marginTop: 12, minHeight: 56 }}
          placeholder={t('clip.notePlaceholder')}
          value={note}
          onChange={(e) => setNote(e.target.value)}
          onBlur={() => note !== (item.note ?? '') && run(clipboard.setNote(item.id, note || null))}
        />
      </div>
      <div className="clip-detail__actions">
        <Button size="sm" variant="primary" icon={Copy} onClick={() => run(clipboard.copy(item.id).then(() => notify.success(t('clip.copied'))))}>
          {t('clip.copy')}
        </Button>
        <Button size="sm" icon={Pin} onClick={() => run(clipboard.setPinned(item.id, !item.pinned))}>
          {item.pinned ? t('clip.unpin') : t('clip.pin')}
        </Button>
        <Button size="sm" icon={Star} onClick={() => run(clipboard.setFavorite(item.id, !item.favorite))}>
          {item.favorite ? t('clip.unfavorite') : t('clip.favorite')}
        </Button>
        {(item.type === 'text' || item.type === 'link') && (
          <Button size="sm" icon={Languages} onClick={() => setTranslating((v) => !v)}>
            {t('clip.translate')}
          </Button>
        )}
        {item.type === 'image' && (
          <Button size="sm" icon={ScanText} onClick={() => run(ocr.fromClip(item.id))}>
            {t('clip.ocr')}
          </Button>
        )}
        <Button size="sm" icon={Trash2} onClick={() => run(clipboard.remove([item.id]))}>
          {t('common.delete')}
        </Button>
      </div>
    </aside>
  );
}
