// 划词翻译面板：鼠标 / 划词按钮旁边弹出，点外面或 Esc 关闭。
//
// - 顶部默认不显示原文（设置里可开），语言那一行右侧是"问 AI"和关闭
// - 外观（宽高、字号、不透明度、圆角）按设置；右下角手柄拖拽改大小，改完记进设置
// - 点"问 AI"：面板就地展开成对话，带着选中的文字。对话期间点别处不收起（不然对话就没了），
//   也可以挪到独立窗口里接着聊

import '@/views/ocr/ocr.css';

import { LogicalSize } from '@tauri-apps/api/dpi';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { ArrowLeft, ExternalLink, Sparkles, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { ai, translate } from '@/lib/ipc';
import { useSettings, useSettingsStore } from '@/lib/settings';
import type { ChatTurn } from '@/lib/types';
import { IconButton } from '@/ui/controls';
import { AiChat } from '@/views/ai/AiChat';

import { TranslateBox } from './TranslateBox';

export default function TranslatePopup() {
  const { t } = useTranslation();
  const settings = useSettings();
  const popup = settings?.translate.popup;
  const [text, setText] = useState('');
  const [seq, setSeq] = useState(0);
  const [mode, setMode] = useState<'translate' | 'ai'>('translate');
  const modeRef = useRef(mode);
  modeRef.current = mode;
  /** 用户在拖右下角手柄：只有这时的尺寸变化才记进设置（弹出时 Rust 摆位、展开 AI 也会改大小） */
  const userResizing = useRef(false);
  const resizeTimer = useRef<number | undefined>(undefined);
  const turnsRef = useRef<ChatTurn[]>([]);
  const keepTurns = useCallback((turns: ChatTurn[]) => void (turnsRef.current = turns), []);

  const adopt = useCallback((next: string) => {
    setText(next);
    setSeq((n) => n + 1);
    setMode('translate');
  }, []);
  useEvent('translate-request', (p) => adopt(p.text));
  // 面板第一次出现时页面可能还没加载完、错过了事件，加载好后主动取一次
  useEffect(() => {
    translate
      .popupText()
      .then((current) => current && adopt(current))
      .catch(() => undefined);
  }, [adopt]);

  const hide = useCallback(() => void getCurrentWindow().hide(), []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && hide();
    window.addEventListener('keydown', onKey);
    const win = getCurrentWindow();
    const unlisteners: Promise<() => void>[] = [
      // 翻译模式点外面就收起；AI 模式不收，免得对话没了
      // 拖手柄改大小时系统会短暂拿走焦点，这时也不收
      win.onFocusChanged(({ payload }) => !payload && modeRef.current === 'translate' && !userResizing.current && hide()),
      // 用户拖右下角改了大小：记进设置，下次按这个尺寸弹出
      win.onResized(() => {
        if (modeRef.current !== 'translate' || !userResizing.current) return;
        window.clearTimeout(resizeTimer.current);
        resizeTimer.current = window.setTimeout(() => {
          userResizing.current = false;
          void win.innerSize().then(async (size) => {
            const scale = await win.scaleFactor();
            const w = Math.round(size.width / scale);
            const h = Math.round(size.height / scale);
            void useSettingsStore
              .getState()
              .update((d) => {
                d.translate.popup.width = w;
                d.translate.popup.height = h;
              })
              .catch(() => undefined);
          });
        }, 500);
      }),
    ];
    return () => {
      window.removeEventListener('keydown', onKey);
      for (const u of unlisteners) void u.then((fn) => fn());
    };
  }, [hide]);

  const resizeTo = async (w: number, h: number) => {
    userResizing.current = false;
    await getCurrentWindow().setSize(new LogicalSize(w, h));
  };

  const openAi = async () => {
    const panel = settings?.ai.panel;
    const win = getCurrentWindow();
    const scale = await win.scaleFactor();
    const size = await win.innerSize();
    setMode('ai');
    await resizeTo(Math.max(size.width / scale, Math.min(panel?.width ?? 520, 640)), panel?.height ?? 620);
  };

  const backToTranslate = async () => {
    setMode('translate');
    if (popup) await resizeTo(popup.width, popup.height || 300);
  };

  const popOut = () => {
    // 连同已经聊过的内容一起挪过去，接着聊
    void ai.open({ text, images: [], source: 'selection', history: turnsRef.current });
    hide();
  };

  const style = {
    '--pop-radius': `${popup?.radius ?? 14}px`,
    '--pop-alpha': String(popup?.opacity ?? 1),
  } as React.CSSProperties;

  return (
    <div className="pop-root" style={style}>
      <div className="pop" key={seq}>
        {mode === 'translate' ? (
          <>
            {popup?.showSource && (
              <div className="pop__source cn-selectable" title={text} data-tauri-drag-region>
                {text}
              </div>
            )}
            <div className="pop__body">
              {text && (
                <TranslateBox
                  text={text}
                  compact
                  fontSize={popup?.fontSize}
                  actions={
                    <>
                      <IconButton icon={Sparkles} size="sm" label={t('ai.ask')} onClick={() => void openAi()} />
                      <IconButton icon={X} size="sm" label={t('common.close')} onClick={hide} />
                    </>
                  }
                />
              )}
            </div>
          </>
        ) : (
          <div className="pop__body">
            <AiChat
              compact
              context={{ text, images: [] }}
              resetKey={seq}
              onTurnsChange={keepTurns}
              toolbar={
                <>
                  <IconButton icon={ArrowLeft} size="sm" label={t('ai.backToTranslate')} onClick={() => void backToTranslate()} />
                  <IconButton icon={ExternalLink} size="sm" label={t('ai.popOut')} onClick={popOut} />
                  <IconButton icon={X} size="sm" label={t('common.close')} onClick={hide} />
                </>
              }
            />
          </div>
        )}
        <span
          className="pop__grip"
          title={t('translate.resize')}
          onPointerDown={(e) => {
            e.preventDefault();
            userResizing.current = true;
            void getCurrentWindow().startResizeDragging('SouthEast');
          }}
        />
      </div>
      <style>{`
        html[data-view='translate'], html[data-view='translate'] body { background: transparent !important; }
        .pop-root { position: fixed; inset: 0; padding: 10px; display: flex; }
        /* 透明窗口里 backdrop-filter 模糊不了桌面，默认不透明；不透明度可在设置里调 */
        .pop { position: relative; flex: 1; display: flex; flex-direction: column; border-radius: var(--pop-radius); overflow: hidden;
               box-shadow: 0 6px 24px rgba(0,0,0,0.28), 0 0 0 0.5px rgba(0,0,0,0.3); animation: cn-pop-in 160ms var(--cn-ease-out); }
        [data-theme='light'] .pop { background: rgba(250,250,252,var(--pop-alpha)); }
        [data-theme='dark'] .pop { background: rgba(36,36,38,var(--pop-alpha)); }
        .pop__source { max-height: 66px; overflow: hidden; padding: 12px 14px 8px; font: var(--cn-text-callout); color: var(--cn-label-secondary);
                       display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical; box-shadow: inset 0 -0.5px 0 var(--cn-separator); flex: none; }
        .pop__body { flex: 1; min-height: 0; }
        .pop__grip { position: absolute; right: 0; bottom: 0; width: 14px; height: 14px; cursor: nwse-resize;
                     background: linear-gradient(135deg, transparent 55%, var(--cn-label-tertiary) 55%, var(--cn-label-tertiary) 62%, transparent 62%, transparent 75%, var(--cn-label-tertiary) 75%, var(--cn-label-tertiary) 82%, transparent 82%); opacity: 0.6; }
        .pop__grip:hover { opacity: 1; }
      `}</style>
    </div>
  );
}
