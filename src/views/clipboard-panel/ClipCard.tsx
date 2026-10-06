// 剪贴板卡片（规格 05 §3.2）。
// - 底部卡片条：顶上一条按类型着色的标题栏（类型 + 时间，右边一个大号来源应用图标），中间内容铺满，
//   底下居中一行细节（字数 / 尺寸 / 来源）；按住 Ctrl 时细节换成粘贴快捷键
// - 竖版小面板（row）：内容 + 底部一行来源和时间

import clsx from 'clsx';
import { File, Globe, Image, Link2, MonitorSmartphone, Palette, Pin, Star, Type } from 'lucide-react';
import { memo, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { readableOn, relativeTime } from '@/lib/format';
import { clipboard } from '@/lib/ipc';
import { assetUrl } from '@/lib/platform';
import type { ClipItem } from '@/lib/types';

/** 复制的是单个 GIF 文件：卡片按图片显示（标题、图标都是 GIF） */
export function isGifItem(item: ClipItem): boolean {
  return item.type === 'files' && item.files.length === 1 && /\.gif$/i.test(item.files[0] ?? '');
}

/** GIF 图标：圆角框里写着 GIF，和 lucide 图标一样按 currentColor 描线 */
function GifIcon({ className, strokeWidth = 1.6 }: { className?: string; strokeWidth?: number }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={strokeWidth} strokeLinecap="round" strokeLinejoin="round">
      <rect x="2.5" y="5" width="19" height="14" rx="3" />
      <path d="M10 10.2a2.2 2.2 0 1 0 0 3.6V12H8.8" />
      <path d="M12.6 9.8v4.4" />
      <path d="M17.6 9.8h-2.4v4.4M15.2 12h2" />
    </svg>
  );
}

/**
 * GIF 预览：平时是静止的第一帧（画在 canvas 上），鼠标放上去才放动图，移开就停。
 * 预览用的是数据目录里那份副本（原文件多半在面板读不到的地方），早先的记录现在补一份
 */
function GifPreview({ item, playing, layout }: { item: ClipItem; playing: boolean; layout: 'card' | 'row' }) {
  const [rel, setRel] = useState(item.thumbPath);
  const [failed, setFailed] = useState(false);
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    if (rel || failed) return;
    let alive = true;
    clipboard
      .gifPreview(item.id)
      .then((r) => alive && (r ? setRel(r) : setFailed(true)))
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
    };
  }, [item.id, rel, failed]);
  const src = assetUrl(rel);
  useEffect(() => {
    if (!src) return;
    const img = new window.Image();
    img.onload = () => {
      const c = canvas.current;
      if (!c || !img.naturalWidth) return;
      // 卡片就两百来像素宽，第一帧缩小了画，省内存
      const scale = Math.min(1, 480 / img.naturalWidth);
      c.width = Math.max(1, Math.round(img.naturalWidth * scale));
      c.height = Math.max(1, Math.round(img.naturalHeight * scale));
      c.getContext('2d')?.drawImage(img, 0, 0, c.width, c.height);
    };
    img.onerror = () => setFailed(true);
    img.src = src;
    return () => {
      img.onload = null;
      img.onerror = null;
    };
  }, [src]);
  if (!src || failed) {
    return (
      <div className="clip-card__files">
        <GifIcon className="clip-card__gif-fallback" />
        <div className="clip-card__fname">{(item.files[0] ?? '').split(/[\\/]/).pop()}</div>
      </div>
    );
  }
  return (
    <div className="clip-card__image clip-card__gif">
      <canvas ref={canvas} style={{ visibility: playing ? 'hidden' : undefined }} />
      {playing && <img src={src} alt="" draggable={false} />}
      {layout === 'card' && !playing && <span className="clip-card__gif-badge">GIF</span>}
    </div>
  );
}

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
  onActivate,
  onDoubleClick,
  onContextMenu,
}: {
  item: ClipItem;
  selected: boolean;
  index: number;
  layout: 'card' | 'row';
  /** 按下左键：选中 */
  onClick: () => void;
  /** 单击（松开）就粘贴的模式下传进来 */
  onActivate?: () => void;
  onDoubleClick?: () => void;
  onContextMenu: (e: React.MouseEvent) => void;
}) {
  const { t } = useTranslation();
  const text = item.preview ?? '';
  const gif = isGifItem(item);
  const [hover, setHover] = useState(false);
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
      if (gif) {
        body = <GifPreview item={item} playing={hover} layout={layout} />;
        if (item.width && item.height) detail = `${item.width} × ${item.height}`;
        break;
      }
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
  const TypeIcon = item.remote ? MonitorSmartphone : gif ? GifIcon : (TYPE_ICON[item.type] ?? Type);

  return (
    <div
      className={clsx('clip-card', `clip-card--${layout}`, `clip-card--${gif ? 'gif' : item.type}`, selected && 'clip-card--selected')}
      style={colorStyle}
      onMouseEnter={gif ? () => setHover(true) : undefined}
      onMouseLeave={gif ? () => setHover(false) : undefined}
      onMouseDown={(e) => e.button === 0 && onClick()}
      onClick={onActivate}
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
                {gif ? t('clip.gif') : t(`clip.type.${item.type}`)}
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
