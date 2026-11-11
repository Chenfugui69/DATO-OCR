// 剪贴板面板的筛选面板（参照 Paste）：点搜索框右端的筛选按钮，从搜索框下面弹出一块玻璃面板，
// 分四节摊开：类型、应用（历史里出现过的来源应用，带图标）、分组、日期。每节单选，再点一下已选的就取消；
// 几节可以叠着用（"图片 + 来自 Claude + 今天"）。横向面板里弹在面板上方（窗口临时往上长一截，见 PanelView），
// 底下一个小尖角指着筛选按钮；竖版面板里盖在卡片上。两种都是四节上下排。

import { useQuery } from '@tanstack/react-query';
import clsx from 'clsx';
import { CalendarDays, type LucideIcon } from 'lucide-react';
import { forwardRef } from 'react';
import { useTranslation } from 'react-i18next';

import { clipboard } from '@/lib/ipc';
import { assetUrl } from '@/lib/platform';
import type { ClipGroup } from '@/lib/types';

import { boardColor } from './BoardTabs';

export const DATE_RANGES = ['today', 'yesterday', 'week', 'month'] as const;
export type DateRange = (typeof DATE_RANGES)[number];

/** 日期一节换成查询的起止（毫秒，[since, until)），按本地日历的零点算 */
export function rangeBounds(range: DateRange | null, now = new Date()): { since?: number; until?: number } {
  if (!range) return {};
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const day = 86400000;
  switch (range) {
    case 'today':
      return { since: start };
    case 'yesterday':
      return { since: start - day, until: start };
    case 'week':
      return { since: start - 6 * day };
    case 'month':
      return { since: start - 29 * day };
  }
}

export interface FilterPanelProps<T extends string> {
  /** 类型一节：可选的类型（不含"全部"）和它们的图标 */
  types: readonly T[];
  typeIcons: Partial<Record<T, LucideIcon>>;
  type: T | null;
  onType: (t: T | null) => void;
  app: string | null;
  onApp: (app: string | null) => void;
  groups: ClipGroup[];
  board: number | null;
  onBoard: (board: number | null) => void;
  range: DateRange | null;
  onRange: (range: DateRange | null) => void;
  /** 弹在横向面板上方：位置相对窗口（left、离窗口底边 bottom），arrow = 尖角离面板左边多远 */
  above?: { left: number; bottom: number; arrow: number; width: number };
}

function Chip({ active, onClick, icon, children, title }: { active: boolean; onClick: () => void; icon: React.ReactNode; children: React.ReactNode; title?: string }) {
  return (
    <button type="button" className={clsx('filterpanel__chip', active && 'filterpanel__chip--on')} aria-pressed={active} title={title} onClick={onClick}>
      <span className="filterpanel__icon">{icon}</span>
      <span className="cn-truncate">{children}</span>
    </button>
  );
}

function FilterPanelInner<T extends string>(props: FilterPanelProps<T>, ref: React.ForwardedRef<HTMLDivElement>) {
  const { t } = useTranslation();
  // 每次打开都重新拿一遍（刚复制的新应用要出现在里面）
  const apps = useQuery({ queryKey: ['clip-source-apps'], queryFn: clipboard.sourceApps, staleTime: 0 });
  const toggle = <V,>(current: V | null, value: V, set: (v: V | null) => void) => set(current === value ? null : value);

  return (
    <div
      ref={ref}
      className={clsx('filterpanel', props.above && 'filterpanel--above')}
      style={props.above ? ({ left: props.above.left, bottom: props.above.bottom, width: props.above.width, '--arrow-x': `${props.above.arrow}px` } as React.CSSProperties) : undefined}
      role="dialog"
      aria-label={t('clip.filterHint')}
    >
      <div className="filterpanel__scroll">
      <section className="filterpanel__section filterpanel__section--type">
        <h3>{t('clip.filterPanel.type')}</h3>
        <div className="filterpanel__grid">
          {props.types.map((f) => {
            const Icon: LucideIcon | undefined = props.typeIcons[f];
            return (
              <Chip key={f} active={props.type === f} onClick={() => toggle(props.type, f, props.onType)} icon={Icon ? <Icon size={16} strokeWidth={1.9} /> : null}>
                {t(`clip.filter.${f}`)}
              </Chip>
            );
          })}
        </div>
      </section>
      <section className="filterpanel__section filterpanel__section--app">
        <h3>{t('clip.filterPanel.app')}</h3>
        {apps.data && apps.data.length === 0 ? (
          <p className="filterpanel__empty">{t('clip.filterPanel.noApps')}</p>
        ) : (
          <div className="filterpanel__grid">
            {(apps.data ?? []).map((a) => (
              <Chip
                key={a.name}
                active={props.app === a.name}
                onClick={() => toggle(props.app, a.name, props.onApp)}
                title={`${a.name} · ${t('clip.board.count', { count: a.count })}`}
                icon={a.icon ? <img src={assetUrl(a.icon)} alt="" draggable={false} /> : <span className="filterpanel__blank" />}
              >
                {a.name}
              </Chip>
            ))}
          </div>
        )}
      </section>
      {props.groups.length > 0 && (
        <section className="filterpanel__section filterpanel__section--board">
          <h3>{t('clip.filterPanel.board')}</h3>
          <div className="filterpanel__list">
            {props.groups.map((g) => (
              <Chip key={g.id} active={props.board === g.id} onClick={() => toggle(props.board, g.id, props.onBoard)} icon={<span className="filterpanel__dot" style={{ background: boardColor(g) }} />}>
                {g.name}
              </Chip>
            ))}
          </div>
        </section>
      )}
      <section className="filterpanel__section filterpanel__section--date">
        <h3>{t('clip.filterPanel.date')}</h3>
        <div className="filterpanel__list">
          {DATE_RANGES.map((r) => (
            <Chip key={r} active={props.range === r} onClick={() => toggle(props.range, r, props.onRange)} icon={<CalendarDays size={16} strokeWidth={1.9} />}>
              {t(`clip.filterPanel.${r}`)}
            </Chip>
          ))}
        </div>
      </section>
      </div>
      {props.above && <span className="filterpanel__arrow" aria-hidden />}
    </div>
  );
}

export const FilterPanel = forwardRef(FilterPanelInner) as <T extends string>(props: FilterPanelProps<T> & { ref?: React.Ref<HTMLDivElement> }) => React.ReactElement;
