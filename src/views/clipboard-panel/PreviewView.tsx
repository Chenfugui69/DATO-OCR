// 剪贴板图片的大预览窗口（像「快速查看」）：屏幕正中间一个和主窗口差不多大的窗口，里面一张能缩放、拖动的图。
// 顶上是来源和尺寸（按住能拖动窗口），右边复制 / 识字 / 关闭；Esc、空格也关。

import { useQuery } from '@tanstack/react-query';
import { Copy, ScanText, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { relativeTime } from '@/lib/format';
import { useEvent } from '@/lib/events';
import { clipboard, ocr } from '@/lib/ipc';
import { assetUrl } from '@/lib/platform';
import { notify } from '@/ui/overlays';

import { isGifItem } from './ClipCard';
import { ZoomImage } from './ZoomImage';
import './preview.css';

export default function PreviewView() {
  const { t } = useTranslation();
  const [id, setId] = useState<number | null>(null);
  // 页面加载完之前事件可能已经发过了：先问一次当前是哪条
  useEffect(() => {
    void clipboard.previewCurrent().then((cur) => cur !== null && setId(cur));
  }, []);
  useEvent('clip-preview', (next) => setId(next));
  const item = useQuery({ queryKey: ['clip-detail-preview', id], queryFn: () => clipboard.get(id!), enabled: id !== null });
  const close = useCallback(() => {
    setId(null);
    void clipboard.previewClose();
  }, []);

  // 窗口是藏起来复用的：重新露出来时再问一次当前是哪条（事件可能在页面被挂起时丢了）
  useEffect(() => {
    const onShow = () => {
      if (document.visibilityState === 'visible') void clipboard.previewCurrent().then((cur) => cur !== null && setId(cur));
    };
    document.addEventListener('visibilitychange', onShow);
    return () => document.removeEventListener('visibilitychange', onShow);
  }, []);

  // 刚打开的一小会儿不认空格：在面板里按空格打开预览时，按住不放的自动重复会马上把它关掉
  const openedAt = useRef(0);
  useEffect(() => {
    if (id !== null) openedAt.current = performance.now();
  }, [id]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' || (e.key === ' ' && !e.repeat && performance.now() - openedAt.current > 400)) {
        e.preventDefault();
        close();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [close]);

  const d = item.data;
  const src = d ? assetUrl(d.type === 'image' ? d.filePath : isGifItem(d) ? d.thumbPath : null) : undefined;
  return (
    <div className="qv">
      <header className="qv__bar" data-tauri-drag-region>
        {d?.sourceIcon && <img className="qv__app" src={assetUrl(d.sourceIcon)} alt="" data-tauri-drag-region />}
        <div className="qv__meta" data-tauri-drag-region>
          <span className="qv__title" data-tauri-drag-region>
            {d ? (d.sourceApp ?? t('clip.unknownApp')) : ''}
          </span>
          <span className="qv__sub" data-tauri-drag-region>
            {d?.width && d.height ? `${d.width} × ${d.height} · ` : ''}
            {d ? relativeTime(d.lastUsedAt) : ''}
          </span>
        </div>
        <span className="qv__spacer" data-tauri-drag-region />
        {d && (
          <button type="button" className="qv__btn" onClick={() => void clipboard.copy(d.id).then(() => notify.success(t('clip.copied')), notify.error)}>
            <Copy size={15} strokeWidth={2} />
            {t('clip.copy')}
          </button>
        )}
        {d?.type === 'image' && (
          <button
            type="button"
            className="qv__btn"
            onClick={() => {
              close();
              void ocr.fromClip(d.id).catch(notify.error);
            }}
          >
            <ScanText size={15} strokeWidth={2} />
            {t('clip.action.ocr')}
          </button>
        )}
        <button type="button" className="qv__btn qv__btn--icon" aria-label={t('common.close')} onClick={close}>
          <X size={16} strokeWidth={2.2} />
        </button>
      </header>
      <div className="qv__stage">
        {/* key：换一张图时缩放回到原样；尺寸用记录里的，不用等图片加载完才知道怎么摆 */}
        {d && <ZoomImage key={d.id} src={src} size={d.width && d.height ? { w: d.width, h: d.height } : undefined} keys />}
      </div>
    </div>
  );
}
