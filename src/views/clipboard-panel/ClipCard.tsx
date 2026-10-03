// 剪贴板卡片（规格 05 §3.2）。
// - 底部卡片条：顶上一行来源（应用图标 + 名称 + 置顶 / 收藏），中间内容，底下一行细节（字数 / 尺寸 / 域名）和时间
// - 竖版小面板（row）：内容 + 底部一行来源和时间

import clsx from 'clsx';
import { File, Globe, Pin, Star } from 'lucide-react';
import { memo } from 'react';
import { useTranslation } from 'react-i18next';

import { readableOn, relativeTime } from '@/lib/format';
import { assetUrl } from '@/lib/platform';
import type { ClipItem } from '@/lib/types';

function domainOf(url: string): string {
  try {
    return new URL(url.trim()).hostname.replace(/^www\./, '');
  } catch {
    return url;
  }
}

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

export const ClipCard = memo(function ClipCard({
  item,
  selected,
  index,
  layout,
  onClick,
  onDoubleClick,
  onContextMenu,
}: {
  item: ClipItem;
  selected: boolean;
  index: number;
  layout: 'card' | 'row';
  onClick: () => void;
  onDoubleClick: () => void;
  onContextMenu: (e: React.MouseEvent) => void;
}) {
  const { t } = useTranslation();
  const text = item.preview ?? '';
  let body: React.ReactNode;
  let detail: string | null = null;
  let colorStyle: React.CSSProperties | undefined;

  switch (item.type) {
    case 'image':
      body = (
        <div className="clip-card__image">
          <img src={assetUrl(item.thumbPath ?? item.filePath)} alt="" loading="lazy" draggable={false} />
        </div>
      );
      if (item.width && item.height) detail = `${item.width} × ${item.height}`;
      break;
    case 'color': {
      const c = text.trim();
      colorStyle = { background: c, color: readableOn(c) === 'black' ? 'rgba(0,0,0,0.85)' : 'rgba(255,255,255,0.95)' };
      body = <div className="clip-card__color cn-mono">{c}</div>;
      break;
    }
    case 'link':
      body = (
        <div className="clip-card__link">
          <Globe size={layout === 'card' ? 28 : 18} strokeWidth={1.5} />
          <div className="clip-card__domain">{domainOf(text)}</div>
          <div className="clip-card__url">{text}</div>
        </div>
      );
      detail = t('clip.type.link');
      break;
    case 'files':
      body = (
        <div className="clip-card__files">
          <File size={layout === 'card' ? 28 : 18} strokeWidth={1.5} />
          <div className="clip-card__fname">{fileName(item.files[0] ?? text)}</div>
          {item.files.length > 1 && <div className="clip-card__more">{t('clip.moreFiles', { count: item.files.length - 1 })}</div>}
        </div>
      );
      detail = t('clip.type.files');
      break;
    default:
      body = <div className="clip-card__text">{text}</div>;
      if (item.charCount != null) detail = t('clip.chars', { count: item.charCount });
  }

  const app = (
    <>
      {item.sourceIcon ? <img className="clip-card__app" src={assetUrl(item.sourceIcon)} alt="" draggable={false} /> : <span className="clip-card__app clip-card__app--blank" />}
      <span className="clip-card__appname cn-truncate">{item.sourceApp ?? t('clip.unknownApp')}</span>
    </>
  );
  const marks = (item.pinned || item.favorite) && (
    <span className="clip-card__marks">
      {item.pinned && <Pin size={11} strokeWidth={2} />}
      {item.favorite && <Star size={11} strokeWidth={2} fill="currentColor" />}
    </span>
  );
  const time = <span className="clip-card__time">{relativeTime(item.lastUsedAt)}</span>;

  return (
    <div
      className={clsx('clip-card', `clip-card--${layout}`, `clip-card--${item.type}`, selected && 'clip-card--selected')}
      style={colorStyle}
      onMouseDown={(e) => e.button === 0 && onClick()}
      onDoubleClick={onDoubleClick}
      onContextMenu={onContextMenu}
      draggable={item.type === 'text' || item.type === 'link'}
      onDragStart={(e) => e.dataTransfer.setData('text/plain', text)}
    >
      {layout === 'card' ? (
        <>
          <div className="clip-card__head">
            {app}
            {marks}
          </div>
          <div className="clip-card__body">{body}</div>
          <div className="clip-card__foot">
            {index < 9 && <span className="clip-card__index">{index + 1}</span>}
            <span className="clip-card__detail cn-truncate cn-numeric">{detail}</span>
            {time}
          </div>
        </>
      ) : (
        <>
          <div className="clip-card__body">{body}</div>
          <div className="clip-card__foot">
            {app}
            {marks}
            {time}
          </div>
        </>
      )}
    </div>
  );
});
