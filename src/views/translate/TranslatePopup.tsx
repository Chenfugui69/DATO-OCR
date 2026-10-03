// 划词翻译面板：鼠标 / 划词按钮旁边弹出，点外面或 Esc 关闭。
//
// - 顶部默认不显示原文（设置里可开），语言那一行右侧是"问 AI"和关闭
// - 外观（宽高、字号、不透明度、圆角）按设置；右下角手柄拖拽改大小，改完记进设置
// - 点"问 AI"：右侧展开一张对话卡片，压在翻译卡片下面（翻译仍可见），带着选中的文字。
//   对话期间点别处不收起（不然对话就没了），也可以挪到独立窗口里接着聊

import '@/views/ocr/ocr.css';

import { LogicalPosition, LogicalSize } from '@tauri-apps/api/dpi';
import { currentMonitor, getCurrentWindow } from '@tauri-apps/api/window';
import { ExternalLink, Sparkles, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { ai, translate } from '@/lib/ipc';
import { useSettings, useSettingsStore } from '@/lib/settings';
import type { ChatTurn } from '@/lib/types';
import { IconButton } from '@/ui/controls';
import { AiChat } from '@/views/ai/AiChat';

import { TranslateBox } from './TranslateBox';

/** 对话卡片压在翻译卡片下面的宽度 */
const OVERLAP = 24;
const PAD = 10;

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

  /** 翻译卡片展开前的窗口宽高（逻辑像素），收起对话时还原 */
  const baseSize = useRef({ w: 460, h: 300 });

  const openAi = async () => {
    const panel = settings?.ai.panel;
    const win = getCurrentWindow();
    const scale = await win.scaleFactor();
    const size = await win.innerSize();
    const base = { w: size.width / scale, h: size.height / scale };
    baseSize.current = base;
    const aiW = Math.min(panel?.width ?? 520, 620);
    const w = base.w + aiW - OVERLAP;
    const h = Math.max(base.h, Math.min(panel?.height ?? 620, 720));
    setMode('ai');
    await resizeTo(w, h);
    // 往右展开超出屏幕了：整个窗口往左挪
    const monitor = await currentMonitor();
    if (monitor) {
      const pos = (await win.outerPosition()).toLogical(scale);
      const right = (monitor.position.x + monitor.size.width) / scale;
      const bottom = (monitor.position.y + monitor.size.height) / scale;
      const x = Math.max(monitor.position.x / scale, Math.min(pos.x, right - w));
      const y = Math.max(monitor.position.y / scale, Math.min(pos.y, bottom - h));
      if (x !== pos.x || y !== pos.y) await win.setPosition(new LogicalPosition(x, y));
    }
  };

  const closeAi = async () => {
    setMode('translate');
    await resizeTo(baseSize.current.w, baseSize.current.h);
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
      <div className="pop pop--translate" key={seq} style={mode === 'ai' ? { flex: 'none', width: baseSize.current.w - PAD * 2, height: baseSize.current.h - PAD * 2 } : undefined}>
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
                  <IconButton
                    icon={Sparkles}
                    size="sm"
                    active={mode === 'ai'}
                    label={mode === 'ai' ? t('ai.hide') : t('ai.ask')}
                    onClick={() => void (mode === 'ai' ? closeAi() : openAi())}
                  />
                  <IconButton icon={X} size="sm" label={t('common.close')} onClick={hide} />
                </>
              }
            />
          )}
        </div>
        {mode === 'translate' && (
          <span
            className="pop__grip"
            title={t('translate.resize')}
            onPointerDown={(e) => {
              e.preventDefault();
              userResizing.current = true;
              void getCurrentWindow().startResizeDragging('SouthEast');
            }}
          />
        )}
      </div>
      {mode === 'ai' && (
        <div className="pop pop--ai">
          <AiChat
            compact
            context={{ text, images: [] }}
            resetKey={seq}
            onTurnsChange={keepTurns}
            toolbar={
              <>
                <IconButton icon={ExternalLink} size="sm" label={t('ai.popOut')} onClick={popOut} />
                <IconButton icon={X} size="sm" label={t('ai.hide')} onClick={() => void closeAi()} />
              </>
            }
          />
        </div>
      )}
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
        /* 翻译卡片在上层；对话卡片从它右边伸出来，左边缘压在它下面 */
        .pop--translate { z-index: 2; align-self: flex-start; }
        .pop--ai { z-index: 1; margin-left: -${OVERLAP}px; padding-left: ${OVERLAP}px; animation: pop-slide 200ms var(--cn-ease-out); }
        @keyframes pop-slide { from { opacity: 0; transform: translateX(-40px); } }
        .pop__grip { position: absolute; right: 0; bottom: 0; width: 14px; height: 14px; cursor: nwse-resize;
                     background: linear-gradient(135deg, transparent 55%, var(--cn-label-tertiary) 55%, var(--cn-label-tertiary) 62%, transparent 62%, transparent 75%, var(--cn-label-tertiary) 75%, var(--cn-label-tertiary) 82%, transparent 82%); opacity: 0.6; }
        .pop__grip:hover { opacity: 1; }
      `}</style>
    </div>
  );
}
