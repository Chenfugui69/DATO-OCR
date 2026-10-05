// 剪贴板卡片（规格 05 §3.2）。
// - 底部卡片条：顶上一条按类型着色的标题栏（类型 + 时间，右边一个大号来源应用图标），中间内容铺满，
//   底下居中一行细节（字数 / 尺寸 / 来源）；按住 Ctrl 时细节换成粘贴快捷键
// - 竖版小面板（row）：内容 + 底部一行来源和时间

import clsx from 'clsx';
import { File, Globe, Image, Link2, MonitorSmartphone, Palette, Pin, Star, Type } from 'lucide-react';
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

/** 没有来源应用图标时，标题栏右边用类型图标顶上 */
const TYPE_ICON = { text: Type, image: Image, link: Link2, files: File, color: Palette } as const;

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
      break;
    case 'files':
      body = (
        <div className="clip-card__files">
          <File size={layout === 'card' ? 28 : 18} strokeWidth={1.5} />
          <div className="clip-card__fname">{fileName(item.files[0] ?? text)}</div>
          {item.files.length > 1 && <div className="clip-card__more">{t('clip.moreFiles', { count: item.files.length - 1 })}</div>}
        </div>
      );
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
  const appName = item.sourceApp ?? t('clip.unknownApp');
  // 从别的设备同步来的：右上角是设备图标，底栏的来源是那台设备的名字
  const TypeIcon = item.remote ? MonitorSmartphone : (TYPE_ICON[item.type] ?? Type);

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
            <div className="clip-card__title">
              <span className="clip-card__type">
                {t(`clip.type.${item.type}`)}
                {marks}
              </span>
              {time}
            </div>
            {item.sourceIcon ? (
              <img className="clip-card__appicon" src={assetUrl(item.sourceIcon)} alt="" title={appName} draggable={false} />
            ) : (
              <TypeIcon className="clip-card__appicon clip-card__appicon--type" strokeWidth={1.6} />
            )}
          </div>
          <div className="clip-card__body">{body}</div>
          <div className="clip-card__foot">
            <span className="clip-card__detail cn-truncate cn-numeric">{detail ?? appName}</span>
            {index < 9 && <span className="clip-card__index">Ctrl {index + 1}</span>}
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
