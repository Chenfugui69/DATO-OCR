// 识字记录：所有识字结果都在这里，可搜索、重新打开、删除。

import { Copy, ScanText, Trash2 } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { formatDateTime } from '@/lib/format';
import { capture, ocr } from '@/lib/ipc';
import { assetUrl, modKey } from '@/lib/platform';
import { useOcrHistory } from '@/lib/queries';
import { EmptyState, SearchField, Spinner } from '@/ui/controls';
import { ContextMenu, confirmDialog, notify } from '@/ui/overlays';
import { useCardDrag, useKeyHeld, useMultiPick } from '@/views/clipboard-panel/cardGestures';
import { ghostItem, PickBar, PickCheck, usePickKeys } from '@/views/clipboard-panel/PickBar';

export function OcrHistoryPage() {
  const { t } = useTranslation();
  const [keyword, setKeyword] = useState('');
  const [debounced, setDebounced] = useState('');
  const sentinel = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const id = window.setTimeout(() => setDebounced(keyword.trim()), 250);
    return () => window.clearTimeout(id);
  }, [keyword]);

  const query = useOcrHistory(debounced);
  const items = useMemo(() => query.data?.pages.flatMap((p) => p.items) ?? [], [query.data]);
  const total = query.data?.pages[0]?.total ?? 0;

  // 多选（按住 Ctrl 点选 / 框选）、复制、拖出窗口粘贴，和剪贴板一样（见 cardGestures）
  const bodyRef = useRef<HTMLDivElement>(null);
  const { picked, toggle: togglePick, clear: clearPicks } = useMultiPick(bodyRef, true);
  const ctrlHeld = useKeyHeld('Control');
  const pickMode = ctrlHeld || picked.size > 0;
  useEffect(() => clearPicks(), [debounced, clearPicks]);
  const pickedInOrder = () => items.filter((r) => picked.has(r.id)).map((r) => r.id);
  const copyPicked = () => {
    const ids = pickedInOrder();
    if (ids.length)
      void ocr
        .copyMany(ids)
        .then(() => notify.success(t('clip.copiedMany', { count: ids.length })))
        .catch(notify.error);
  };
  usePickKeys(picked.size, copyPicked, clearPicks);
  const { dragging, onPointerDown } = useCardDrag({
    idsFor: (id) => (picked.has(id) ? pickedInOrder() : [id]),
    itemsFor: (ids) =>
      ids.flatMap((id) => {
        const r = items.find((x) => x.id === id);
        return r ? [ghostItem({ id: r.id, type: 'text', preview: r.preview, charCount: r.charCount, lastUsedAt: r.createdAt })] : [];
      }),
    onDropOutside: (ids) => void ocr.dropPaste(ids).catch(notify.error),
  });
  const draggingIds = new Set(dragging ?? []);

  useEffect(() => {
    const el = sentinel.current;
    if (!el) return;
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting) && query.hasNextPage && !query.isFetchingNextPage) void query.fetchNextPage();
    });
    io.observe(el);
    return () => io.disconnect();
  }, [query]);

  return (
    <section className="page">
      <div className="page__head">
        <div>
          <h1 className="page__title">{t('nav.ocr')}</h1>
          <div className="page__sub">{t('ocrHistory.count', { count: total })}</div>
        </div>
        <PickBar count={picked.size} onCopy={copyPicked} onClear={clearPicks} />
        <span className="page__spacer" />
        <SearchField value={keyword} onChange={setKeyword} placeholder={t('ocrHistory.search')} style={{ width: 240 }} />
      </div>
      <div ref={bodyRef} className="page__body page__body--pickable">
        {query.isLoading ? (
          <div className="load-more">
            <Spinner size={20} />
          </div>
        ) : items.length === 0 ? (
          <EmptyState icon={ScanText} title={debounced ? t('ocrHistory.noMatch') : t('ocrHistory.empty')} description={debounced ? undefined : t('ocrHistory.emptyDesc')} />
        ) : (
          <div className="ocr-list">
            {items.map((r) => (
              <ContextMenu
                key={r.id}
                items={[
                  { label: t('ocrHistory.open'), icon: ScanText, onSelect: () => void ocr.openRecord(r.id).catch(notify.error) },
                  {
                    label: t('ocrHistory.copy'),
                    icon: Copy,
                    onSelect: () =>
                      void capture
                        .writeText(r.preview)
                        .then(() => notify.success(t('ocr.copied')))
                        .catch(notify.error),
                  },
                  { separator: true },
                  {
                    label: t('common.delete'),
                    icon: Trash2,
                    danger: true,
                    onSelect: () =>
                      void confirmDialog({ title: t('ocrHistory.deleteTitle'), confirmLabel: t('common.delete'), danger: true }).then((ok) => {
                        if (ok) void ocr.deleteRecord(r.id).catch(notify.error);
                      }),
                  },
                ]}
              >
                <div
                  className="ocr-item"
                  data-clip-id={r.id}
                  data-picked={picked.has(r.id)}
                  data-dragging={draggingIds.has(r.id)}
                  onClick={(e) => !modKey(e) && void ocr.openRecord(r.id).catch(notify.error)}
                  onPointerDown={(e) => onPointerDown(e, r.id)}
                >
                  {pickMode && <PickCheck checked={picked.has(r.id)} onToggle={() => togglePick(r.id)} />}
                  <div className="ocr-item__thumb">{r.imagePath && <img src={assetUrl(r.imagePath)} alt="" loading="lazy" decoding="async" />}</div>
                  <div className="ocr-item__main">
                    <div className="ocr-item__text">{r.preview || t('ocr.nothing')}</div>
                    <div className="ocr-item__meta">
                      {formatDateTime(r.createdAt)} · {r.engine} · {t('ocrHistory.chars', { count: r.charCount })}
                    </div>
                  </div>
                </div>
              </ContextMenu>
            ))}
          </div>
        )}
        <div ref={sentinel} className="load-more">
          {query.isFetchingNextPage && <Spinner size={16} />}
        </div>
      </div>
    </section>
  );
}
