// 自绘标题栏（规格 06 §4.6）。
// Windows 上用 Windows 自己的窗口按钮样式（右上角 46 宽、关闭悬停变红）——
// 不在 Windows 上放 macOS 的红黄绿灯，那是画虎不成。这是唯一允许平台差异化的地方。

import { getCurrentWindow } from '@tauri-apps/api/window';
import { useEffect, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

function MinimizeGlyph() {
  return (
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
      <path d="M0.5 5h9" stroke="currentColor" strokeWidth="1" />
    </svg>
  );
}

function MaximizeGlyph({ restored }: { restored: boolean }) {
  return restored ? (
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden fill="none" stroke="currentColor" strokeWidth="1">
      <rect x="0.5" y="2.5" width="7" height="7" rx="1" />
      <path d="M2.5 2.5V1.5a1 1 0 0 1 1-1h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-1" />
    </svg>
  ) : (
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden fill="none" stroke="currentColor" strokeWidth="1">
      <rect x="0.5" y="0.5" width="9" height="9" rx="1" />
    </svg>
  );
}

function CloseGlyph() {
  return (
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
      <path d="M0.5 0.5l9 9M9.5 0.5l-9 9" stroke="currentColor" strokeWidth="1" />
    </svg>
  );
}

export function TitleBar({
  title,
  extra,
  maximizable = true,
  onClose,
}: {
  title?: ReactNode;
  extra?: ReactNode;
  maximizable?: boolean;
  onClose?: () => void;
}) {
  const { t } = useTranslation();
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;
    const sync = () => {
      win.isMaximized().then(setMaximized).catch(() => undefined);
    };
    sync();
    win
      .onResized(sync)
      .then((fn) => (unlisten = fn))
      .catch(() => undefined);
    return () => unlisten?.();
  }, []);

  const win = getCurrentWindow();
  return (
    <header className="cn-titlebar" data-tauri-drag-region>
      {title && (
        <div className="cn-titlebar__title" data-tauri-drag-region>
          {title}
        </div>
      )}
      <div className="cn-titlebar__spacer" data-tauri-drag-region />
      {extra && <div className="cn-titlebar__extra">{extra}</div>}
      <div className="cn-caption">
        <button type="button" aria-label={t('window.minimize')} onClick={() => void win.minimize()}>
          <MinimizeGlyph />
        </button>
        {maximizable && (
          <button
            type="button"
            aria-label={maximized ? t('window.restore') : t('window.maximize')}
            onClick={() => void win.toggleMaximize()}
          >
            <MaximizeGlyph restored={maximized} />
          </button>
        )}
        <button
          type="button"
          className="cn-caption__close"
          aria-label={t('window.close')}
          onClick={() => (onClose ? onClose() : void win.close())}
        >
          <CloseGlyph />
        </button>
      </div>
    </header>
  );
}
