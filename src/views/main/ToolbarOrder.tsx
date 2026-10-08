// 设置页：截图工具条的按钮顺序。画一条和真工具条一样的条，按住图标左右拖就能换位置
// （只能在自己那一组里换：绘制工具 / 识字翻译那一组 / 最右边那一组）。
//
// 不用 HTML5 的拖放：Tauri 窗口默认接管了拖放（给拖文件进来用的），页面里的 draggable 不工作。
// 这里自己跟指针：拖到谁的位置上就挪到谁那里，松手时保存。

import '@/views/annotate/annotate.css';

import { RotateCcw } from 'lucide-react';
import { Fragment, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { MID_ACTIONS, TAIL_ACTIONS, TOOL_ORDER, type CaptureSettings } from '@/lib/types';
import { Button } from '@/ui/controls';
import { ACTION_ICONS, TOOL_ICONS, type ActionId } from '@/views/annotate/Toolbar';
import type { Tool } from '@/views/annotate/model';

type Order = Pick<CaptureSettings, 'toolbarTools' | 'toolbarActions' | 'toolbarOrder'>;
type GroupKey = keyof Order;
const GROUPS: GroupKey[] = ['toolbarTools', 'toolbarActions', 'toolbarOrder'];
const DEFAULTS: Order = { toolbarTools: TOOL_ORDER, toolbarActions: MID_ACTIONS, toolbarOrder: TAIL_ACTIONS };

export function ToolbarOrder({ value, onChange }: { value: Order; onChange: (next: Order) => void }) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<Order | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const filled = (o: Order): Order => ({
    toolbarTools: o.toolbarTools?.length ? o.toolbarTools : TOOL_ORDER,
    toolbarActions: o.toolbarActions?.length ? o.toolbarActions : MID_ACTIONS,
    toolbarOrder: o.toolbarOrder?.length ? o.toolbarOrder : TAIL_ACTIONS,
  });
  const order = filled(draft ?? value);
  const isDefault = GROUPS.every((g) => (order[g] as string[]).every((id, i) => id === DEFAULTS[g][i]));

  const onPointerDown = (e: React.PointerEvent<HTMLButtonElement>, group: GroupKey, id: string) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    let current = order;
    let moved = false;
    setDraft(current);
    setDragging(id);
    const move = (ev: PointerEvent) => {
      const items = [...(barRef.current?.querySelectorAll<HTMLElement>(`[data-group="${group}"]`) ?? [])];
      // 指针落在同组哪个图标的范围里，就把拖着的这个挪到它的位置
      const over = items.findIndex((el) => {
        const r = el.getBoundingClientRect();
        return ev.clientX >= r.left && ev.clientX < r.right;
      });
      const list = current[group] as string[];
      const from = list.indexOf(id);
      if (over < 0 || over === from) return;
      const next = [...list];
      next.splice(from, 1);
      next.splice(over, 0, id);
      current = { ...current, [group]: next };
      moved = true;
      setDraft(current);
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      setDragging(null);
      setDraft(null);
      if (moved) onChange(current);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  return (
    <div className="tb-order">
      <div ref={barRef} className="an-toolbar tb-order__bar">
        {GROUPS.map((group, gi) => (
          <Fragment key={group}>
            {gi > 0 && <span className="an-sep" />}
            {(order[group] as string[]).map((id) => {
              const Icon = group === 'toolbarTools' ? TOOL_ICONS[id as Tool] : ACTION_ICONS[id as ActionId];
              const label = group === 'toolbarTools' || id === 'undo' ? t(`tools.${id}`) : t(`actions.${id}`);
              return (
                <button
                  key={id}
                  type="button"
                  data-group={group}
                  data-dragging={dragging === id || undefined}
                  className={`an-btn tb-order__item${id === 'done' ? ' an-btn--primary' : ''}`}
                  title={label}
                  aria-label={label}
                  onPointerDown={(e) => onPointerDown(e, group, id)}
                >
                  <Icon size={18} strokeWidth={1.5} absoluteStrokeWidth />
                </button>
              );
            })}
          </Fragment>
        ))}
      </div>
      <Button size="sm" variant="ghost" icon={RotateCcw} disabled={isDefault} onClick={() => onChange({ toolbarTools: [...TOOL_ORDER], toolbarActions: [...MID_ACTIONS], toolbarOrder: [...TAIL_ACTIONS] })}>
        {t('settings.capture.toolbarOrderReset')}
      </Button>
    </div>
  );
}
