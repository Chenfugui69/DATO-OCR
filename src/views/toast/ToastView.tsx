// 屏幕右下角的轻提示窗口（规格 06 §4.7）：鼠标穿透、不抢焦点、不进通知中心。
// 多个提示垂直堆叠，新的在下方；全部消失后把窗口藏起来。

import clsx from 'clsx';
import { CheckCircle2, Info, XCircle } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';

import { useEvent } from '@/lib/events';
import { system } from '@/lib/ipc';
import type { ToastPayload } from '@/lib/types';

interface Item extends ToastPayload {
  id: number;
  leaving: boolean;
}

const icons = { success: CheckCircle2, error: XCircle, info: Info };

export default function ToastView() {
  const [items, setItems] = useState<Item[]>([]);
  const seq = useRef(0);

  useEvent('toast', (p) => {
    const id = ++seq.current;
    setItems((list) => [...list.filter((i) => !i.leaving).slice(-2), { ...p, id, leaving: false }]);
    const stay = p.kind === 'error' ? 3000 : 1500;
    window.setTimeout(() => {
      setItems((list) => list.map((i) => (i.id === id ? { ...i, leaving: true } : i)));
      window.setTimeout(() => setItems((list) => list.filter((i) => i.id !== id)), 170);
    }, stay);
  });

  useEffect(() => {
    if (items.length === 0 && seq.current > 0) void system.hideToast();
  }, [items.length]);

  return (
    <div className="toast-stack">
      {items.map((item) => {
        const Icon = icons[item.kind];
        return (
          <div key={item.id} className={clsx('cn-toast cn-glass', `cn-toast--${item.kind}`)} data-leaving={item.leaving || undefined}>
            <Icon size={16} strokeWidth={1.5} />
            <span>{item.message}</span>
          </div>
        );
      })}
      <style>{`
        html[data-view='toast'], html[data-view='toast'] body { background: transparent !important; }
        .toast-stack { position: fixed; right: 8px; bottom: 8px; left: 8px; display: flex; flex-direction: column; align-items: flex-end; gap: 8px; }
        .toast-stack .cn-toast { box-shadow: var(--cn-shadow-md), 0 0 0 0.5px var(--cn-separator); }
      `}</style>
    </div>
  );
}
