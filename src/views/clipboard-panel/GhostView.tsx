// 拖剪贴板卡片时跟着鼠标走的影子（窗口见 Rust 的 clipboard::ghost）：拖一张就是那张卡片，
// 拖好几张是一叠 —— 后面的几张从正后方散开、稍微歪一点，右上角一个数字。不挡鼠标、不抢焦点。

import type React from 'react';
import { useState } from 'react';

import { useEvent } from '@/lib/events';
import type { ClipItem } from '@/lib/types';

import { ClipCard } from './ClipCard';
import './panel.css';
import './ghost.css';

/** 叠在后面的最多画几张（再多也看不出来） */
const BEHIND = 2;

type GhostItem = ClipItem & { ghostTint?: string };

export default function GhostView() {
  const [items, setItems] = useState<GhostItem[]>([]);
  const [grab, setGrab] = useState<[number, number]>([0, 0]);
  // 鼠标下面是输入框：整叠缩小到鼠标那儿，表示松手就贴在这里
  const [over, setOver] = useState(false);
  // 每次开始拖换一个 key，入场动画（拿起、叠起来）重新播一遍
  const [round, setRound] = useState(0);
  useEvent('clip-ghost', (next) => {
    setItems(next.items);
    setGrab(next.grab);
    setOver(false);
    if (next.items.length) setRound((r) => r + 1);
  });
  useEvent('clip-ghost-target', setOver);
  if (!items.length) return null;
  const [top, ...rest] = items;
  return (
    <div
      className="ghost"
      key={round}
      data-many={items.length > 1 || undefined}
      data-over={over || undefined}
      style={{ '--grab-x': `${grab[0]}px`, '--grab-y': `${grab[1]}px` } as React.CSSProperties}
    >
      {rest.slice(0, BEHIND).map((it, i) => (
        <div key={it.id} className={`ghost__card ghost__card--behind ghost__card--b${i + 1}`}>
          <ClipCard item={it} tint={it.ghostTint} selected={false} index={99} layout="card" onClick={noop} onContextMenu={noop} />
        </div>
      ))}
      <div className="ghost__card ghost__card--top">
        <ClipCard item={top!} tint={top!.ghostTint} selected={false} index={99} layout="card" onClick={noop} onContextMenu={noop} />
      </div>
      {items.length > 1 && <span className="ghost__count">{items.length}</span>}
    </div>
  );
}

function noop() {}
