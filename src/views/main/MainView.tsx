// 主窗口（规格 06 §6.1）：毛玻璃侧边栏 + 内容区。平时关着，托盘里留图标。

import './main.css';

import { useQuery } from '@tanstack/react-query';
import { ClipboardList, Crop, Images, Keyboard, MonitorSmartphone, ScanText, ScrollText, Settings as SettingsIcon, type LucideIcon } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { capture, clipboard } from '@/lib/ipc';
import { useLiveInvalidation } from '@/lib/queries';
import { BrandMark } from '@/ui/BrandMark';
import { notify } from '@/ui/overlays';
import { TitleBar } from '@/ui/TitleBar';

import { ClipboardPage } from './ClipboardPage';
import { LibraryPage } from './LibraryPage';
import { OcrHistoryPage } from './OcrHistoryPage';
import { SettingsPage } from './SettingsPage';
import { SyncPage } from './SyncPage';
import { useUpdate } from './UpdateDialog';

export type Page = 'library' | 'clipboard' | 'ocr' | 'sync' | 'settings';

const NAV: { id: Page; icon: LucideIcon }[] = [
  { id: 'library', icon: Images },
  { id: 'clipboard', icon: ClipboardList },
  { id: 'ocr', icon: ScanText },
  { id: 'sync', icon: MonitorSmartphone },
  { id: 'settings', icon: SettingsIcon },
];

export default function MainView() {
  const { t } = useTranslation();
  useLiveInvalidation();
  const [page, setPage] = useState<Page>('library');
  const [section, setSection] = useState<string | null>(null);
  const stats = useQuery({ queryKey: ['clip-stats'], queryFn: clipboard.stats });
  // 有新版本（且没选"不显示更新提示"）：设置旁边亮个小红点
  const { prompt: updatePrompt } = useUpdate();

  // Rust 侧（托盘、剪贴板面板的齿轮按钮）可以指定打开哪一页：`settings` / `settings:translate`
  useEvent('navigate', (target) => {
    const [p, s] = target.split(':');
    if (p === 'library' || p === 'clipboard' || p === 'ocr' || p === 'sync' || p === 'settings') {
      setPage(p);
      setSection(s ?? null);
    }
  });

  const start = (intent: 'normal' | 'longshot' | 'ocr') => void capture.start(intent).catch(notify.error);

  return (
    <div className="main-root">
      <aside className="sidebar">
        <div className="sidebar__brand" data-tauri-drag-region>
          {/* 标志只在 Windows 上放：macOS 这个位置是系统的红黄绿灯，再挤一个标志太满（样式里藏掉） */}
          <span className="sidebar__logo">
            <BrandMark size={16} />
          </span>
          <span className="sidebar__name" data-tauri-drag-region>
            DATO OCR
          </span>
        </div>
        <div className="sidebar__quick">
          <button type="button" className="sidebar__qbtn" onClick={() => start('normal')}>
            <Crop size={16} strokeWidth={1.5} />
            {t('nav.capture')}
          </button>
          <button type="button" className="sidebar__qbtn" onClick={() => start('longshot')}>
            <ScrollText size={16} strokeWidth={1.5} />
            {t('nav.longshot')}
          </button>
          <button type="button" className="sidebar__qbtn" onClick={() => start('ocr')}>
            <ScanText size={16} strokeWidth={1.5} />
            {t('nav.ocrCapture')}
          </button>
        </div>
        {NAV.map(({ id, icon: Icon }) => (
          <button
            key={id}
            type="button"
            className="sidebar__item"
            data-active={page === id}
            onClick={() => {
              setPage(id);
              setSection(null);
            }}
          >
            <Icon size={16} strokeWidth={1.5} />
            {t(`nav.${id}`)}
            {id === 'clipboard' && stats.data && <span className="sidebar__count cn-numeric">{stats.data.total.toLocaleString()}</span>}
            {id === 'settings' && updatePrompt && <span className="sidebar__dot" title={t('update.dot')} />}
          </button>
        ))}
        <span className="sidebar__spacer" />
        <button
          type="button"
          className="sidebar__item"
          onClick={() => {
            setPage('settings');
            setSection('hotkeys');
          }}
        >
          <Keyboard size={16} strokeWidth={1.5} />
          {t('nav.hotkeys')}
        </button>
      </aside>
      <main className={page === 'settings' || page === 'sync' ? 'content content--grouped' : 'content'}>
        <TitleBar />
        {page === 'library' && <LibraryPage />}
        {page === 'clipboard' && <ClipboardPage />}
        {page === 'ocr' && <OcrHistoryPage />}
        {page === 'sync' && <SyncPage />}
        {page === 'settings' && <SettingsPage section={section} />}
      </main>
    </div>
  );
}
