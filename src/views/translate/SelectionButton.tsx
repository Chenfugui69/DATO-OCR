// 划词悬浮按钮：选中文字后出现在选区旁边的小圆钮。
//
// 点击由 Rust 侧的全局鼠标钩子截获（这个窗口永远不激活，被选文字的程序焦点和选区都不受
// 影响），所以这里只负责长相：悬停高亮、每次出现时重放一遍出场动画。

import { Languages } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';

export default function SelectionButton() {
  const { t } = useTranslation();
  const [generation, setGeneration] = useState(0);
  useEvent('selection-button-show', setGeneration);

  return (
    <div className="selbtn-root">
      <div key={generation} className="selbtn" title={t('translate.selectionButton')}>
        <Languages size={15} strokeWidth={1.75} />
      </div>
      <style>{`
        html[data-view='selbtn'], html[data-view='selbtn'] body { background: transparent !important; overflow: hidden; }
        .selbtn-root { position: fixed; inset: 0; display: grid; place-items: center; }
        .selbtn {
          width: calc(100% - 4px); height: calc(100% - 4px); border-radius: 50%;
          display: grid; place-items: center;
          color: var(--cn-accent);
          box-shadow: 0 1px 4px rgba(0,0,0,0.22), 0 0 0 0.5px rgba(0,0,0,0.18);
          transition: transform 120ms var(--cn-ease-out), background 120ms;
          animation: selbtn-in 160ms var(--cn-ease-out);
        }
        [data-theme='light'] .selbtn { background: rgba(255,255,255,0.97); }
        [data-theme='dark'] .selbtn { background: rgba(48,48,52,0.97); }
        .selbtn:hover { transform: scale(1.08); background: var(--cn-accent); color: #fff; }
        @keyframes selbtn-in { from { opacity: 0; transform: scale(0.6); } }
      `}</style>
    </div>
  );
}
