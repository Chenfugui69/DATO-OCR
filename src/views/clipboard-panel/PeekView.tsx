// 剪贴板面板里按住 Shift 时卡片正上方的悬浮预览（窗口见 Rust 的 clipboard::peek）：
// 只是看的，不抢焦点、不挡鼠标。图片铺满窗口（窗口按图片比例开），文字显示前面一大段。

import { useQuery } from '@tanstack/react-query';
import { File, Globe } from 'lucide-react';
import { useEffect, useState } from 'react';

import { useEvent } from '@/lib/events';
import { clipboard } from '@/lib/ipc';
import { assetUrl } from '@/lib/platform';

import { isGifItem } from './ClipCard';
import { OtpBody } from './OtpBody';
import './peek.css';

export default function PeekView() {
  const [id, setId] = useState<number | null>(null);
  // 页面加载完之前事件可能已经发过了：先问一次
  useEffect(() => {
    void clipboard.peekCurrent().then((cur) => cur !== null && setId(cur));
  }, []);
  useEvent('clip-peek', (next) => setId(next));
  const q = useQuery({ queryKey: ['clip-peek', id], queryFn: () => clipboard.get(id!), enabled: id !== null, staleTime: 30_000 });
  const d = id !== null ? q.data : undefined;
  if (!d) return <div className="peek" />;

  let body: React.ReactNode;
  if (d.type === 'image' || isGifItem(d)) {
    body = <img className="peek__image" src={assetUrl(d.type === 'image' ? d.filePath : (d.thumbPath ?? null))} alt="" draggable={false} />;
  } else if (d.type === 'color') {
    const c = (d.preview ?? '').trim();
    body = <div className="peek__color cn-mono" style={{ background: c }}>{c}</div>;
  } else if (d.type === 'otp') {
    body = (
      <div className="peek__text">
        <OtpBody item={d} variant="detail" />
      </div>
    );
  } else if (d.type === 'files') {
    body = (
      <ul className="peek__files">
        {d.files.slice(0, 30).map((f) => (
          <li key={f}>
            <File size={14} strokeWidth={1.6} />
            <span className="cn-truncate">{f}</span>
          </li>
        ))}
      </ul>
    );
  } else if (d.type === 'link') {
    body = (
      <div className="peek__text peek__link">
        <Globe size={16} strokeWidth={1.6} />
        <span>{(d.contentText ?? d.preview ?? '').trim()}</span>
      </div>
    );
  } else {
    body = <div className="peek__text">{(d.contentText ?? d.preview ?? '').slice(0, 4000)}</div>;
  }
  return <div className={d.type === 'image' || isGifItem(d) ? 'peek peek--image' : 'peek'}>{body}</div>;
}
