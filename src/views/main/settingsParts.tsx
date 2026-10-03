// 设置页的分组卡片和行（iOS 设置那种样式）。

import type { ReactNode } from 'react';

export function Group({ id, title, children, note }: { id: string; title: string; children: ReactNode; note?: ReactNode }) {
  return (
    <section className="set-group" id={`set-${id}`}>
      <h2 className="set-group__title">{title}</h2>
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
