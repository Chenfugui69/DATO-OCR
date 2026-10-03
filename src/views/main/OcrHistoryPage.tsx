// 识字记录：所有识字结果都在这里，可搜索、重新打开、删除。

import { Copy, ScanText, Trash2 } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { formatDateTime } from '@/lib/format';
import { capture, ocr } from '@/lib/ipc';
import { assetUrl } from '@/lib/platform';
import { useOcrHistory } from '@/lib/queries';
import { EmptyState, SearchField, Spinner } from '@/ui/controls';
import { ContextMenu, confirmDialog, notify } from '@/ui/overlays';

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
        <span className="page__spacer" />
        <SearchField value={keyword} onChange={setKeyword} placeholder={t('ocrHistory.search')} style={{ width: 240 }} />
      </div>
      <div className="page__body">
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
                <div className="ocr-item" onClick={() => void ocr.openRecord(r.id).catch(notify.error)}>
                  <div className="ocr-item__thumb">{r.imagePath && <img src={assetUrl(r.imagePath)} alt="" loading="lazy" />}</div>
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
