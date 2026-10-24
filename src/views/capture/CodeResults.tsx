// 截图里识别出来的二维码 / 条码。两种入口：
// - 工具条上的"识别二维码"：只认选区里的，码的位置画框，选区旁边一张卡片列出全部
// - 自动识别（设置里默认开）：截图一开始就在后台把整屏扫一遍，每个码中间放一个小图标，
//   鼠标移上去（或点一下钉住）就在旁边弹出这个码的卡片，像 Snow Shot 那样几乎无感
// 复制、打开都算用完了这次截图：先关掉截图，不然打开的网页（视频之类）在截图遮罩后面跑起来了。

import { Copy, ExternalLink, QrCode, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useElementSize } from '@/lib/hooks';
import { capture, system } from '@/lib/ipc';
import type { ScannedCode } from '@/lib/types';
import { notify } from '@/ui/overlays';

import type { Rect } from './geometry';

const isUrl = (text: string) => /^https?:\/\/\S+$/i.test(text.trim());

/** 一个码：类型、内容、复制 / 打开。用完调 `onDone`（结束截图） */
function CodeItem({ code, onDone }: { code: ScannedCode; onDone: () => void }) {
  const { t } = useTranslation();
  const copy = (text: string) =>
    void capture
      .writeText(text)
      .then(() => {
        onDone();
        void system.toast('success', t('capture.codeCopied')).catch(() => {});
      })
      .catch(notify.error);
  const open = (url: string) => {
    onDone();
    void system.openUrl(url).catch((err: unknown) => void system.toast('error', err instanceof Error ? err.message : String(err)).catch(() => {}));
  };
  return (
    <div className="cap-codes__item">
      <span className="cap-codes__kind">{code.kind}</span>
      <span className="cap-codes__text" title={code.text}>
        {code.text}
      </span>
      <div className="cap-codes__actions">
        <button type="button" className="cap-codes__btn" onClick={() => copy(code.text)}>
          <Copy size={12} strokeWidth={1.75} />
          {t('capture.copyCode')}
        </button>
        {isUrl(code.text) && (
          <button type="button" className="cap-codes__btn" onClick={() => open(code.text.trim())}>
            <ExternalLink size={12} strokeWidth={1.75} />
            {t('capture.openLink')}
          </button>
        )}
      </div>
    </div>
  );
}

export function CodeResults({
  codes,
  selection,
  s,
  viewport,
  onClose,
  onDone,
}: {
  codes: ScannedCode[];
  selection: Rect;
  s: number;
  viewport: { width: number; height: number };
  onClose: () => void;
  /** 复制 / 打开之后：结束这次截图 */
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  const size = useElementSize(ref, { width: 300, height: 120 });
  // 卡片放在选区右边；右边放不下放左边，再放不下就贴着选区里面的右上角
  const sel = { x: selection.x / s, y: selection.y / s, width: selection.width / s, height: selection.height / s };
  const gap = 10;
  let x = sel.x + sel.width + gap;
  if (x + size.width > viewport.width - 8) x = sel.x - gap - size.width;
  if (x < 8) x = Math.max(8, sel.x + sel.width - size.width - 8);
  const y = Math.max(8, Math.min(sel.y, viewport.height - size.height - 8));

  return (
    <>
      {codes.map((c, i) => (
        <span
          key={`box-${i}`}
          className="cap-code-box"
          style={{ left: c.x / s - 3, top: c.y / s - 3, width: c.width / s + 6, height: c.height / s + 6 }}
        />
      ))}
      <div
        ref={ref}
        className="cap-codes cn-glass"
        style={{ transform: `translate(${x}px, ${y}px)` }}
        onPointerDown={(e) => e.stopPropagation()}
      >
        <div className="cap-codes__head">
          <span>{t('capture.codesFound', { count: codes.length })}</span>
          <button type="button" className="cap-codes__icon" aria-label={t('common.close')} onClick={onClose}>
            <X size={13} strokeWidth={1.75} />
          </button>
        </div>
        {codes.map((c, i) => (
          <CodeItem key={i} code={c} onDone={onDone} />
        ))}
      </div>
    </>
  );
}

/** 自动识别出来的码：每个码中间一个小图标，鼠标移上去弹卡片 */
export function CodeBadges({
  codes,
  s,
  viewport,
  onDone,
}: {
  codes: ScannedCode[];
  s: number;
  viewport: { width: number; height: number };
  onDone: () => void;
}) {
  const { t } = useTranslation();
  // 鼠标在哪个图标（或它的卡片）上；pinned = 点过图标，鼠标移开也不收
  const [open, setOpen] = useState<{ index: number; pinned: boolean } | null>(null);
  const leaveTimer = useRef<number | undefined>(undefined);
  const enter = (index: number) => {
    window.clearTimeout(leaveTimer.current);
    setOpen((cur) => (cur?.index === index ? cur : { index, pinned: false }));
  };
  // 从图标挪到卡片上要经过一小段空隙，稍等一下再收
  const leave = () => {
    window.clearTimeout(leaveTimer.current);
    leaveTimer.current = window.setTimeout(() => setOpen((cur) => (cur?.pinned ? cur : null)), 220);
  };
  useEffect(() => () => window.clearTimeout(leaveTimer.current), []);

  const card = open ? codes[open.index] : undefined;
  const ref = useRef<HTMLDivElement>(null);
  const size = useElementSize(ref, { width: 280, height: 110 });
  let pos = { x: 0, y: 0 };
  if (card) {
    const cx = (card.x + card.width / 2) / s;
    const cy = (card.y + card.height / 2) / s;
    // 卡片在图标右边，放不下就放左边；上下居中对着图标
    let x = cx + 20;
    if (x + size.width > viewport.width - 8) x = cx - 20 - size.width;
    pos = { x: Math.max(8, x), y: Math.max(8, Math.min(cy - size.height / 2, viewport.height - size.height - 8)) };
  }

  return (
    <>
      {codes.map((c, i) => (
        <button
          key={i}
          type="button"
          className="cap-code-badge"
          data-open={open?.index === i || undefined}
          style={{ left: (c.x + c.width / 2) / s, top: (c.y + c.height / 2) / s }}
          aria-label={t('capture.codeBadge')}
          title={t('capture.codeBadge')}
          onPointerEnter={() => enter(i)}
          onPointerLeave={leave}
          onPointerDown={(e) => e.stopPropagation()}
          onClick={(e) => {
            e.stopPropagation();
            setOpen((cur) => (cur?.index === i && cur.pinned ? null : { index: i, pinned: true }));
          }}
        >
          <QrCode size={14} strokeWidth={2} />
        </button>
      ))}
      {card && (
        <div
          ref={ref}
          className="cap-codes cap-codes--float cn-glass"
          style={{ transform: `translate(${pos.x}px, ${pos.y}px)` }}
          onPointerEnter={() => window.clearTimeout(leaveTimer.current)}
          onPointerLeave={leave}
          onPointerDown={(e) => e.stopPropagation()}
          onClick={(e) => e.stopPropagation()}
        >
          <CodeItem code={card} onDone={onDone} />
        </div>
      )}
    </>
  );
}
