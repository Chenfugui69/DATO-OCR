// 独立的 AI 对话窗口：托盘 / 热键打开（空白对话），截图工具条"问 AI"打开（带截图），
// 划词面板里点"在窗口中打开"（带选中的文字）。关闭 = 隐藏，对话还在。

import { getCurrentWindow } from '@tauri-apps/api/window';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { ai } from '@/lib/ipc';
import { modKey } from '@/lib/platform';
import type { AiContext } from '@/lib/types';
import { TitleBar } from '@/ui/TitleBar';

import { AiChat } from './AiChat';

export default function AiView() {
  const { t } = useTranslation();
  const [ctx, setCtx] = useState<AiContext | null>(null);

  const adopt = useCallback((next: AiContext | null) => {
    if (next) setCtx((cur) => (cur?.seq === next.seq ? cur : next));
  }, []);
  useEvent('ai-context', adopt);
  useEffect(() => {
    ai.context().then(adopt).catch(() => undefined);
  }, [adopt]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (modKey(e) && e.key.toLowerCase() === 'w') {
        e.preventDefault();
        void getCurrentWindow().close();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  return (
    <div style={{ height: '100%', display: 'flex', flexDirection: 'column' }}>
      <TitleBar title={t('ai.title')} />
      <div style={{ flex: 1, minHeight: 0 }}>
        <AiChat context={ctx ? { text: ctx.text, images: ctx.images, source: ctx.source } : null} resetKey={ctx?.seq ?? 0} initialTurns={ctx?.history} />
      </div>
    </div>
  );
}
