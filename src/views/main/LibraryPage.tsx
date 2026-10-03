// 截图库：每次截图都会存一份，按日期分组浏览；双击用编辑器打开。

import { Copy, Download, FolderOpen, Images, PenLine, Pin, ScanText, Star, Trash2 } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { dayGroup, formatBytes, formatDateTime } from '@/lib/format';
import { editor, library } from '@/lib/ipc';
import { assetUrl } from '@/lib/platform';
import { useShots } from '@/lib/queries';
import type { Shot } from '@/lib/types';
import { Button, EmptyState, SearchField, Spinner } from '@/ui/controls';
import { ContextMenu, confirmDialog, notify, Tooltip } from '@/ui/overlays';

type Filter = 'all' | 'favorite' | 'longshot';

export function LibraryPage() {
  const { t } = useTranslation();
  const [keyword, setKeyword] = useState('');
  const [debounced, setDebounced] = useState('');
  const [filter, setFilter] = useState<Filter>('all');
  const [selected, setSelected] = useState<number | null>(null);
  const sentinel = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const id = window.setTimeout(() => setDebounced(keyword.trim()), 250);
    return () => window.clearTimeout(id);
  }, [keyword]);

  const query = useShots({
    keyword: debounced || undefined,
    favoriteOnly: filter === 'favorite',
    kind: filter === 'longshot' ? 'longshot' : null,
  });
  const shots = useMemo(() => query.data?.pages.flatMap((p) => p.items) ?? [], [query.data]);
  const total = query.data?.pages[0]?.total ?? 0;

  const groups = useMemo(() => {
    const out: { title: string; items: Shot[] }[] = [];
    for (const s of shots) {
      const title = dayGroup(s.createdAt);
      const last = out[out.length - 1];
      if (last?.title === title) last.items.push(s);
      else out.push({ title, items: [s] });
    }
    return out;
  }, [shots]);

  useEffect(() => {
    const el = sentinel.current;
    if (!el) return;
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting) && query.hasNextPage && !query.isFetchingNextPage) void query.fetchNextPage();
    });
    io.observe(el);
    return () => io.disconnect();
  }, [query]);

  const run = (p: Promise<unknown>) => void p.catch(notify.error);
  const remove = async (shot: Shot) => {
    if (await confirmDialog({ title: t('library.deleteTitle'), body: t('library.deleteBody'), confirmLabel: t('common.delete'), danger: true })) {
      run(library.remove([shot.id]));
    }
  };

  const menu = (s: Shot) => [
    { label: t('library.edit'), icon: PenLine, onSelect: () => run(editor.openShot(s.id)) },
    { label: t('library.copy'), icon: Copy, onSelect: () => run(library.copy(s.id)) },
    { label: t('library.pin'), icon: Pin, onSelect: () => run(library.pin(s.id)) },
    { label: t('library.ocr'), icon: ScanText, onSelect: () => run(library.ocr(s.id)) },
    { separator: true },
    { label: t('library.saveAs'), icon: Download, onSelect: () => run(library.saveAs(s.id)) },
    { label: t('library.reveal'), icon: FolderOpen, onSelect: () => run(library.reveal(s.id)) },
    { label: s.favorite ? t('library.unfavorite') : t('library.favorite'), icon: Star, onSelect: () => run(library.setFavorite(s.id, !s.favorite)) },
    { separator: true },
    { label: t('common.delete'), icon: Trash2, danger: true, onSelect: () => void remove(s) },
  ];

  return (
    <section className="page">
      <div className="page__head">
        <div>
          <h1 className="page__title">{t('nav.library')}</h1>
          <div className="page__sub">{t('library.count', { count: total })}</div>
        </div>
        <span className="page__spacer" />
        <div className="chips">
          {(['all', 'favorite', 'longshot'] as Filter[]).map((f) => (
            <button key={f} type="button" className="chip" data-active={filter === f} onClick={() => setFilter(f)}>
              {t(`library.filter.${f}`)}
            </button>
          ))}
        </div>
        <SearchField value={keyword} onChange={setKeyword} placeholder={t('library.search')} style={{ width: 220 }} />
      </div>
      <div className="page__body" onClick={() => setSelected(null)}>
        {query.isLoading ? (
          <div className="load-more">
            <Spinner size={20} />
          </div>
        ) : shots.length === 0 ? (
          <EmptyState
            icon={Images}
            title={debounced ? t('library.noMatch') : t('library.empty')}
            description={debounced ? t('library.noMatchDesc') : t('library.emptyDesc')}
          />
        ) : (
          groups.map((g) => (
            <div key={g.title} className="lib-group">
              <div className="lib-group__title">{g.title}</div>
              <div className="lib-grid">
                {g.items.map((s, i) => (
                  <ContextMenu key={s.id} items={menu(s)}>
                    <div
                      className="shot"
                      data-selected={selected === s.id}
                      style={{ animationDelay: `${Math.min(i, 8) * 30}ms` }}
                      onClick={(e) => {
                        e.stopPropagation();
                        setSelected(s.id);
                      }}
                      onDoubleClick={() => run(editor.openShot(s.id))}
                      title={formatDateTime(s.createdAt)}
                    >
                      <div className="shot__thumb">
                        <img src={assetUrl(s.thumbPath ?? s.filePath)} alt="" loading="lazy" draggable={false} />
                      </div>
                      <div className="shot__meta">
                        <span>{s.sourceApp ?? formatDateTime(s.createdAt).slice(11)}</span>
                        <span className="cn-numeric">
                          {s.width}×{s.height}
                        </span>
                        <span>{formatBytes(s.sizeBytes)}</span>
                      </div>
                      <div className="shot__badge">
                        {s.kind === 'longshot' && <span>{t('library.longshot')}</span>}
                        {s.favorite && (
                          <span>
                            <Star size={10} fill="currentColor" strokeWidth={0} />
                          </span>
                        )}
                      </div>
                      <div className="shot__hover" onClick={(e) => e.stopPropagation()}>
                        <Tooltip content={t('library.copy')}>
                          <button type="button" onClick={() => run(library.copy(s.id))}>
                            <Copy size={13} strokeWidth={1.75} />
                          </button>
                        </Tooltip>
                        <Tooltip content={t('library.pin')}>
                          <button type="button" onClick={() => run(library.pin(s.id))}>
                            <Pin size={13} strokeWidth={1.75} />
                          </button>
                        </Tooltip>
                        <Tooltip content={t('library.ocr')}>
                          <button type="button" onClick={() => run(library.ocr(s.id))}>
                            <ScanText size={13} strokeWidth={1.75} />
                          </button>
                        </Tooltip>
                      </div>
                    </div>
                  </ContextMenu>
                ))}
              </div>
            </div>
          ))
        )}
        <div ref={sentinel} className="load-more">
          {query.isFetchingNextPage && <Spinner size={16} />}
          {!query.hasNextPage && shots.length > 30 && <Button variant="ghost" size="sm" disabled>{t('library.end')}</Button>}
        </div>
      </div>
    </section>
  );
}
