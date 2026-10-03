// 数据库数据走 TanStack Query（规格 01 §4.2：服务端状态不放 Zustand）。

import { useInfiniteQuery, useQueryClient } from '@tanstack/react-query';

import { useEvent } from './events';
import { clipboard, library, ocr } from './ipc';
import type { ClipQuery, ShotQuery } from './types';

export function useClipItems(query: Omit<ClipQuery, 'cursor' | 'limit'>, limit = 50) {
  return useInfiniteQuery({
    queryKey: ['clips', query],
    queryFn: ({ pageParam }) => clipboard.query({ ...query, cursor: pageParam, limit }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
  });
}

export function useShots(query: Omit<ShotQuery, 'cursor' | 'limit'>, limit = 60) {
  return useInfiniteQuery({
    queryKey: ['shots', query],
    queryFn: ({ pageParam }) => library.query({ ...query, cursor: pageParam, limit }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor,
  });
}

export function useOcrHistory(keyword: string, limit = 40) {
  return useInfiniteQuery({
    queryKey: ['ocr-history', keyword],
    queryFn: ({ pageParam }) => ocr.history({ keyword, offset: pageParam, limit }),
    initialPageParam: 0,
    getNextPageParam: (last, pages) => {
      const loaded = pages.reduce((n, p) => n + p.items.length, 0);
      return loaded < last.total ? loaded : undefined;
    },
  });
}

/** 数据变化事件 → 让对应查询失效重拉。 */
export function useLiveInvalidation() {
  const qc = useQueryClient();
  useEvent('clipboard-changed', () => {
    void qc.invalidateQueries({ queryKey: ['clips'] });
    void qc.invalidateQueries({ queryKey: ['clip-stats'] });
    void qc.invalidateQueries({ queryKey: ['clip-groups'] });
  });
  useEvent('library-changed', () => void qc.invalidateQueries({ queryKey: ['shots'] }));
  useEvent('ocr-history-changed', () => void qc.invalidateQueries({ queryKey: ['ocr-history'] }));
}
