// 剪贴板卡片（规格 05 §3.2）。
// - 底部卡片条：顶上一条按类型着色的标题栏（类型 + 时间，右边一个大号来源应用图标），中间内容铺满，
//   底下居中一行细节（字数 / 尺寸 / 来源）；按住 Ctrl 时细节换成粘贴快捷键
// - 竖版小面板（row）：内容 + 底部一行来源和时间

import clsx from 'clsx';
import { Eye, File, Globe, Image, KeyRound, Languages, Link2, MonitorSmartphone, Palette, Pin, ScanText, Sparkles, Star, Type } from 'lucide-react';
import { useQuery } from '@tanstack/react-query';
import { memo, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { readableOn, relativeTime } from '@/lib/format';
import { clipboard } from '@/lib/ipc';
import { assetUrl } from '@/lib/platform';
import { Tooltip } from '@/ui/overlays';
import type { ClipItem } from '@/lib/types';

import { highlightCode, looksLikeCode } from './codeText';
import { CLIP_DRAG_TYPE } from './BoardTabs';
import { DeviceIcon, isDeviceKind } from './DeviceIcon';
import { OtpBody } from './OtpBody';

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

/** 第一段像个标题（不长、后面还有内容、末尾不是句号逗号这类标点）才整段加粗，像 Paste；
 *  一上来就是整句话的不加粗。段落之间留一点空，不挤成一坨 */
function TextBody({ text, layout }: { text: string; layout: 'card' | 'row' }) {
  if (layout === 'row') return <div className="clip-card__text">{text}</div>;
  const paras = text
    .replace(/\r\n?/g, '\n')
    .split('\n')
    .filter((p) => p.trim() !== '')
    .slice(0, 40);
  const first = paras[0]?.trim() ?? '';
  const titled = paras.length > 1 && first.length <= 90 && !/[。．.!！?？;；,，、…]$/.test(first);
  return (
    <div className="clip-card__text clip-card__text--paras">
      {paras.map((p, i) => (
        <p key={i} className={i === 0 && titled ? 'clip-card__lead' : undefined}>
          {p}
        </p>
      ))}
    </div>
  );
}

/** 文件路径去掉用户目录那一截（/Users/xx/、C:\Users\xx\），像 Paste 那样显示 Documents/website/backup.zip */
function shortPath(path: string): string {
  return path.replace(/^\/Users\/[^/]+\//, '').replace(/^[A-Za-z]:\\Users\\[^\\]+\\/, '');
}

/** 文件卡片的大图标：系统给这个文件的图标（同类文件后端只取一次）；取不到用线条图标 */
function FileArt({ path }: { path: string }) {
  const icon = useQuery({
    queryKey: ['clip-file-icon', path],
    queryFn: () => clipboard.fileIcon(path),
    staleTime: Infinity,
  });
  if (icon.data) return <img className="clip-card__fileicon" src={assetUrl(icon.data)} alt="" draggable={false} />;
  return <File className="clip-card__fileicon clip-card__fileicon--line" strokeWidth={1.2} />;
}

/** 卡片上悬停出来的快捷操作 */
export type CardAction = 'preview' | 'translate' | 'ocr' | 'ai';

/** 这类卡片有哪些快捷操作：图片能识字，文字能翻译，都能问 AI、放大预览（验证码、文件、颜色没有） */
function actionsFor(item: ClipItem): CardAction[] {
  switch (item.type) {
    case 'image':
      return ['ocr', 'ai', 'preview'];
    case 'text':
    case 'link':
      return ['translate', 'ai', 'preview'];
    default:
      return [];
  }
}
const ACTION_ICON = { preview: Eye, translate: Languages, ocr: ScanText, ai: Sparkles } as const;

/** 没有来源应用图标时，标题栏右边用类型图标顶上 */
const TYPE_ICON = { text: Type, image: Image, link: Link2, files: File, color: Palette, otp: KeyRound } as const;

export const ClipCard = memo(function ClipCard({
  item,
  tint,
  board,
  selected,
  flashing,
  suggested,
  onAction,
  index,
  layout,
  onClick,
  onActivate,
  onDoubleClick,
  onContextMenu,
}: {
  item: ClipItem;
  /** 来源应用图标的主色：标题栏用它，没有就按类型配色 */
  tint?: string;
  /** 正在看某个分组：标题栏统一用分组的颜色，标题是卡片的名字（备注），不放应用图标（像 Paste 的图钉板） */
  board?: { color: string; name: string };
  selected: boolean;
  /** 粘贴前闪一下 */
  flashing?: boolean;
  /** 智能推荐挑中的：暖色描边 + 角上一个 ✨ */
  suggested?: boolean;
  /** 卡片上的快捷操作（卡片条里才有） */
  onAction?: (action: CardAction, item: ClipItem) => void;
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

  // 卡片条里图片铺满标题栏以下（像 Paste），尺寸做成图片底部居中的小胶囊
  const fullBleed = layout === 'card' && (item.type === 'image' || gif);
  const code = item.type === 'text' && looksLikeCode(text, item.sourceApp);

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
      body =
        layout === 'card' ? (
          <div className="clip-card__weblink">
            <div className="clip-card__linkart">
              <Globe size={34} strokeWidth={1.3} />
            </div>
            <div className="clip-card__linkmeta">
              <div className="clip-card__domain">{domainOf(text)}</div>
              <div className="clip-card__url cn-truncate">{text.trim()}</div>
            </div>
          </div>
        ) : (
          <div className="clip-card__link">
            <Globe size={18} strokeWidth={1.5} />
            <div className="clip-card__domain">{domainOf(text)}</div>
            <div className="clip-card__url">{text}</div>
          </div>
        );
      break;
    case 'otp':
      body = <OtpBody item={item} variant={layout} />;
      break;
    case 'files':
      if (gif) {
        body = <GifPreview item={item} playing={hover} layout={layout} />;
        if (item.width && item.height) detail = `${item.width} × ${item.height}`;
        break;
      }
      if (layout === 'card') {
        body = (
          <div className="clip-card__fileart">
            <FileArt path={item.files[0] ?? text} />
            {item.files.length > 1 && <span className="clip-card__filecount">+{item.files.length - 1}</span>}
          </div>
        );
        detail = item.files.length > 1 ? t('clip.moreFilesNamed', { name: fileName(item.files[0] ?? ''), total: item.files.length, more: item.files.length - 1 }) : shortPath(item.files[0] ?? text);
        break;
      }
      body = (
        <div className="clip-card__files">
          <File size={18} strokeWidth={1.5} />
          <div className="clip-card__fname">{fileName(item.files[0] ?? text)}</div>
          {item.files.length > 1 && <div className="clip-card__more">{t('clip.moreFiles', { count: item.files.length - 1 })}</div>}
        </div>
      );
      break;
    default:
      body = code ? <pre className="clip-card__code cn-mono">{highlightCode(text)}</pre> : <TextBody text={text} layout={layout} />;
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
  // 底栏右边的设备图标；同步来的悬停时带上那台设备的名字（这时 sourceApp 就是设备名）
  const device = isDeviceKind(item.deviceKind) && (
    <span className="clip-card__device">
      <DeviceIcon
        kind={item.deviceKind}
        size={layout === 'card' ? 15 : 13}
        title={item.remote ? `${appName} · ${t(`clip.device.${item.deviceKind}`)}` : undefined}
      />
    </span>
  );
  // 从别的设备同步来的：右上角是设备图标，底栏的来源是那台设备的名字
  const TypeIcon = item.remote ? MonitorSmartphone : gif ? GifIcon : (TYPE_ICON[item.type] ?? Type);
  const headTint = board?.color ?? tint;
  const typeLabel = gif ? t('clip.gif') : t(`clip.type.${item.type}`);

  return (
    <div
      className={clsx(
        'clip-card',
        `clip-card--${layout}`,
        `clip-card--${gif ? 'gif' : item.type}`,
        selected && 'clip-card--selected',
        flashing && 'clip-card--flash',
        suggested && 'clip-card--suggested',
        fullBleed && 'clip-card--bleed',
        code && 'clip-card--code',
        // 文字、代码一直排到卡片底，字数浮在渐隐的最后几行上（Paste 的做法），不单独占一条底栏
        layout === 'card' && item.type === 'text' && 'clip-card--flow',
      )}
      style={headTint && item.type !== 'color' ? ({ ...colorStyle, '--clip-tint': headTint } as React.CSSProperties) : colorStyle}
      onMouseEnter={gif ? () => setHover(true) : undefined}
      onMouseLeave={gif ? () => setHover(false) : undefined}
      onMouseDown={(e) => e.button === 0 && onClick()}
      onClick={onActivate}
      onDoubleClick={onDoubleClick}
      onContextMenu={onContextMenu}
      // 拖到面板顶上的分组标签上归进分组；文字、链接还能直接拖进别的程序
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData(CLIP_DRAG_TYPE, String(item.id));
        if (item.type === 'text' || item.type === 'link') e.dataTransfer.setData('text/plain', text);
        e.dataTransfer.effectAllowed = 'copyMove';
      }}
    >
      {layout === 'card' ? (
        <>
          <div className={clsx('clip-card__head', board && 'clip-card__head--board')}>
            <div className="clip-card__title">
              <span className="clip-card__type cn-truncate">
                {board ? item.note || typeLabel : typeLabel}
                {marks}
              </span>
              {!board && time}
            </div>
            {board ? null : item.sourceIcon ? (
              <img className="clip-card__appicon" src={assetUrl(item.sourceIcon)} alt="" title={appName} draggable={false} />
            ) : (
              <span className="clip-card__typetile" title={item.remote ? appName : undefined}>
                <TypeIcon width={19} height={19} strokeWidth={2} />
              </span>
            )}
          </div>
          <div className="clip-card__body">{body}</div>
          {/* 快捷操作：标题栏底边正中，悬停才出来（不盖在图片、文字上） */}
          {onAction && actionsFor(item).length > 0 && (
            // 按钮自己处理点击：不能冒到卡片上变成"选中 / 粘贴"
            <div className="clip-card__actions" onMouseDown={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()}>
              {actionsFor(item).map((a) => {
                const Icon = ACTION_ICON[a];
                // 悬停时在按钮正上方出提示（系统 title 要停很久、还出在鼠标底下挡着卡片）
                return (
                  <Tooltip key={a} content={t(`clip.action.${a}`)} side="top">
                    <button
                      type="button"
                      className="clip-card__action"
                      aria-label={t(`clip.action.${a}`)}
                      onClick={(e) => {
                        e.stopPropagation();
                        onAction(a, item);
                      }}
                    >
                      <Icon size={14} strokeWidth={2} />
                    </button>
                  </Tooltip>
                );
              })}
            </div>
          )}
          {suggested && (
            <span className="clip-card__suggested" aria-label={t('clip.smart.badge')}>
              <Sparkles size={11} strokeWidth={2.4} />
              {t('clip.smart.badge')}
            </span>
          )}
          {/* 图片卡片的底栏浮在图片上：尺寸是底部居中的小胶囊 */}
          <div className="clip-card__foot">
            {(!fullBleed || detail) && <span className="clip-card__detail cn-truncate cn-numeric">{detail ?? appName}</span>}
            {index < 9 && <span className="clip-card__index">Ctrl {index + 1}</span>}
            {device}
          </div>
        </>
      ) : (
        <>
          <div className="clip-card__body">{body}</div>
          <div className="clip-card__foot">
            {app}
            {marks}
            {time}
            {device}
          </div>
        </>
      )}
    </div>
  );
});
