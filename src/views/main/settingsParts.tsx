// 设置页的分组卡片和行（iOS 设置那种样式）。

import { createContext, useContext, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

/** 设置页现在显示哪个分类（左边栏选的）；null = 全部显示 */
export const SettingsSectionContext = createContext<string | null>(null);

/** 卡片组属于哪个分类：大多数就是自己的 id，几组附属的归到它们的大类下 */
const SUB_GROUPS: Record<string, string> = { 'ai-chat': 'ai', 'ai-prompts': 'ai', 'ai-look': 'ai', popup: 'selection', 'record-video': 'record', speech: 'translate' };
export function sectionOf(id: string): string {
  return SUB_GROUPS[id] ?? id;
}

export function Group({ id, title, children, note }: { id: string; title: string; children: ReactNode; note?: ReactNode }) {
  const { t } = useTranslation();
  const current = useContext(SettingsSectionContext);
  if (current && sectionOf(id) !== current) return null;
  // 分类的第一组标题和右边顶上的大标题重复，不再写一遍
  const showTitle = !current || id !== current;
  const section = sectionOf(id);
  const sectionName = t(`settings.section.${section}`);
  // 搜索时（全部分类都显示）标题带上分类名，看得出是哪一类下面的
  const shown = !current && sectionName !== title ? `${sectionName} · ${title}` : title;
  return (
    <section className="set-group" id={`set-${id}`} data-section={section} data-search={`${sectionName} ${title}`}>
      {showTitle && <h2 className="set-group__title">{shown}</h2>}
      <div className="set-card">{children}</div>
      {note && <p className="set-note">{note}</p>}
    </section>
  );
}

export function Row({ title, desc, children }: { title: ReactNode; desc?: ReactNode; children?: ReactNode }) {
  return (
    <div className="set-row">
      <div className="set-row__label">
        <div className="set-row__title">{title}</div>
        {desc && <div className="set-row__desc">{desc}</div>}
      </div>
      {children && <div className="set-row__control">{children}</div>}
    </div>
  );
}
