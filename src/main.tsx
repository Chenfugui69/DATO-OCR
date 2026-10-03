// 所有窗口共用这个入口，按窗口标签决定渲染哪个视图（各视图懒加载）。

import '@/styles/base.css';
import '@/ui/ui.css';

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { Component, StrictMode, type ComponentType, type ErrorInfo, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';

import { setLanguage } from '@/i18n';
import { reportError } from '@/lib/ipc';
import { loadDataDir, windowLabel } from '@/lib/platform';
import { initSettingsSync, useSettingsStore } from '@/lib/settings';
import { initTheme } from '@/lib/theme';
import { DialogHost, Toaster, TooltipProvider } from '@/ui/overlays';

type ViewModule = { default: ComponentType };

const views: [match: (label: string) => boolean, name: string, load: () => Promise<ViewModule>][] = [
  [(l) => l.startsWith('capture-'), 'capture', () => import('@/views/capture/CaptureView')],
  [(l) => l.startsWith('pin-'), 'pin', () => import('@/views/pin/PinView')],
  [(l) => l === 'clipboard', 'clipboard', () => import('@/views/clipboard-panel/PanelView')],
  [(l) => l === 'toast', 'toast', () => import('@/views/toast/ToastView')],
  [(l) => l === 'ocr', 'ocr', () => import('@/views/ocr/OcrView')],
  [(l) => l === 'editor', 'editor', () => import('@/views/editor/EditorView')],
  [(l) => l === 'translate', 'translate', () => import('@/views/translate/TranslatePopup')],
  [(l) => l === 'selbtn', 'selbtn', () => import('@/views/translate/SelectionButton')],
  [(l) => l === 'ai', 'ai', () => import('@/views/ai/AiView')],
  [() => true, 'main', () => import('@/views/main/MainView')],
];

class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error, info: ErrorInfo) {
    reportError('render', `${error.message}\n${info.componentStack ?? ''}`);
  }
  render() {
    if (this.state.error) {
      return (
        <div style={{ padding: 24, font: 'var(--cn-text-body)', color: 'var(--cn-label)' }}>
          {this.state.error.message}
        </div>
      );
    }
    return this.props.children;
  }
}

/** 桌面应用里屏蔽浏览器默认行为：刷新、打印、查找、右键菜单。 */
function lockBrowserDefaults() {
  window.addEventListener(
    'keydown',
    (e) => {
      const k = e.key.toLowerCase();
      if (k === 'f5' || (e.ctrlKey && ['r', 'p', 'f', 'g', 'j', 'u', 'o', 's'].includes(k) && !e.altKey)) {
        // 视图自己处理这些组合键（Ctrl+S 保存、Ctrl+P 贴图…），这里只拦浏览器默认动作
        e.preventDefault();
      }
    },
    true,
  );
  window.addEventListener('contextmenu', (e) => {
    const t = e.target as HTMLElement | null;
    const editable = t?.closest('input, textarea, [contenteditable="true"]');
    if (!editable) e.preventDefault();
  });
  window.addEventListener('unhandledrejection', (e) => reportError('promise', e.reason));
  window.addEventListener('error', (e) => reportError('window', e.error ?? e.message));
}

async function boot() {
  const label = windowLabel();
  const [, name, load] = views.find(([match]) => match(label)) ?? views[views.length - 1]!;
  document.documentElement.dataset.view = name;
  lockBrowserDefaults();
  initSettingsSync();

  const [mod, settings] = await Promise.all([
    load(),
    useSettingsStore.getState().load().catch(() => null),
    initTheme(),
    loadDataDir().catch(() => ''),
  ]);
  if (settings) setLanguage(settings.general.language);
  useSettingsStore.subscribe((s) => s.settings && setLanguage(s.settings.general.language));

  const View = mod.default;
  const queryClient = new QueryClient({
    defaultOptions: { queries: { staleTime: 5_000, refetchOnWindowFocus: false, retry: 1 } },
  });
  const container = document.getElementById('root');
  if (!container) throw new Error('missing #root');

  const tree = (
    <ErrorBoundary>
      <QueryClientProvider client={queryClient}>
        <TooltipProvider>
          <View />
          <DialogHost />
          <Toaster />
        </TooltipProvider>
      </QueryClientProvider>
    </ErrorBoundary>
  );
  // 截图遮罩不套 StrictMode：它会把 effect 跑两遍，热键到上屏的预算里容不下
  createRoot(container).render(name === 'capture' ? tree : <StrictMode>{tree}</StrictMode>);
}

boot().catch((err) => reportError('boot', err));
