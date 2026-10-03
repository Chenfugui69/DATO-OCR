// 划词翻译面板：鼠标 / 划词按钮旁边弹出，点外面或 Esc 关闭。
//
// - 顶部默认不显示原文（设置里可开），语言那一行右侧是"问 AI"和关闭
// - 外观（宽高、字号、不透明度、圆角）按设置；右下角手柄拖拽改大小，改完记进设置
// - 点"问 AI"：面板往右长出对话区，翻译卡片浮在对话区上面（左边缘压住对话区），两块共用
//   一个外框，看起来是一个整体。中间的缝可以左右拖，右下角手柄改整体大小，都记进设置。
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

/** 对话区伸进翻译卡片下面的宽度（被盖住，只为让两块看起来是叠着的） */
const OVERLAP = 24;
/** 窗口四周留给阴影的透明边 */
const PAD = 10;
/** 对话区、翻译卡片的最小宽度 */
const MIN_AI = 280;
const MIN_TRANSLATE = 360;

const logicalWindowSize = () => ({ w: window.innerWidth, h: window.innerHeight });

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
  /** AI 模式下翻译卡片的宽度（逻辑像素）；中间的缝拖动改的就是它 */
  const [transW, setTransW] = useState(440);
  const transRef = useRef<HTMLDivElement>(null);
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
        if (!userResizing.current) return;
        window.clearTimeout(resizeTimer.current);
        resizeTimer.current = window.setTimeout(() => {
          userResizing.current = false;
          const { w, h } = logicalWindowSize();
          const translateW = transRef.current?.getBoundingClientRect().width ?? 0;
          void useSettingsStore
            .getState()
            .update((d) => {
              if (modeRef.current === 'translate') {
                d.translate.popup.width = Math.round(w);
                d.translate.popup.height = Math.round(h);
              } else {
                d.ai.panel.width = Math.round(w - PAD * 2 - translateW);
                d.ai.panel.height = Math.round(h);
              }
            })
            .catch(() => undefined);
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

  /** 展开对话前的窗口大小（逻辑像素），收起对话时还原；拖过中间的缝就按新的翻译卡片宽度 */
  const baseSize = useRef({ w: 460, h: 300 });

  const openAi = async () => {
    const panel = settings?.ai.panel;
    const win = getCurrentWindow();
    const scale = await win.scaleFactor();
    const base = logicalWindowSize();
    baseSize.current = base;
    const monitor = await currentMonitor();
    const screenW = monitor ? monitor.size.width / scale : 1920;
    const screenH = monitor ? monitor.size.height / scale : 1080;
    const trans = Math.max(MIN_TRANSLATE, base.w - PAD * 2);
    setTransW(trans);
    const w = Math.min(screenW - 16, trans + PAD * 2 + Math.max(MIN_AI, panel?.width ?? 520));
    const h = Math.min(screenH - 16, Math.max(base.h, panel?.height ?? 620));
    setMode('ai');
    await resizeTo(w, h);
    // 往右展开超出屏幕了：整个窗口往左挪
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

  /** 拖翻译卡片和对话区之间的缝：窗口大小不变，两边此消彼长 */
  const dragSeam = (e: React.PointerEvent<HTMLSpanElement>) => {
    e.preventDefault();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    handle.dataset.dragging = '';
    let last = transW;
    const move = (ev: PointerEvent) => {
      last = Math.round(Math.min(window.innerWidth - PAD * 2 - MIN_AI, Math.max(MIN_TRANSLATE, ev.clientX - PAD)));
      setTransW(last);
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      delete handle.dataset.dragging;
      const aiW = window.innerWidth - PAD * 2 - last;
      baseSize.current = { ...baseSize.current, w: last + PAD * 2 };
      void useSettingsStore
        .getState()
        .update((d) => {
          d.translate.popup.width = last + PAD * 2;
          d.ai.panel.width = Math.round(aiW);
        })
        .catch(() => undefined);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
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
      <div className={mode === 'ai' ? 'pop-shell is-ai' : 'pop-shell'}>
        <div
          ref={transRef}
          className="pop-pane pop-pane--translate"
          key={seq}
          style={mode === 'ai' ? { width: transW } : undefined}
        >
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
        </div>
        {mode === 'ai' && (
          <>
            <span className="pop-seam" style={{ left: transW - 5 }} title={t('translate.resize')} onPointerDown={dragSeam} />
            <div className="pop-pane pop-pane--ai">
              <AiChat
                compact
                context={{ text, images: [] }}
                resetKey={seq}
                onTurnsChange={keepTurns}
                toolbar={<IconButton icon={ExternalLink} size="sm" label={t('ai.popOut')} onClick={popOut} />}
              />
            </div>
          </>
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
        .pop-root { position: fixed; inset: 0; padding: ${PAD}px; display: flex; }
        /* 透明窗口里 backdrop-filter 模糊不了桌面，默认不透明；不透明度可在设置里调。
           外框统一一个圆角和阴影：翻译、对话两块装在同一个框里，不再是两张各自漂着的卡片 */
        .pop-shell { position: relative; flex: 1; min-width: 0; display: flex; border-radius: var(--pop-radius); overflow: hidden;
                     box-shadow: 0 8px 28px rgba(0,0,0,0.26), 0 0 0 0.5px rgba(0,0,0,0.28); animation: cn-pop-in 160ms var(--cn-ease-out);
                     --pop-top: rgba(252,252,254,var(--pop-alpha)); --pop-under: rgba(238,238,243,var(--pop-alpha)); background: var(--pop-top); }
        [data-theme='dark'] .pop-shell { --pop-top: rgba(46,46,48,var(--pop-alpha)); --pop-under: rgba(30,30,32,var(--pop-alpha)); }
        .pop-pane { position: relative; min-width: 0; display: flex; flex-direction: column; }
        .pop-pane--translate { flex: 1; z-index: 2; background: var(--pop-top); }
        .pop__source { max-height: 66px; overflow: hidden; padding: 12px 14px 8px; font: var(--cn-text-callout); color: var(--cn-label-secondary);
                       display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical; box-shadow: inset 0 -0.5px 0 var(--cn-separator); flex: none; }
        .pop__body { flex: 1; min-height: 0; }
        /* 对话模式：外框底色换成下层的颜色；翻译卡片右侧圆角 + 投影，像一张卡片压在对话区上 */
        .pop-shell.is-ai { background: var(--pop-under); }
        .is-ai .pop-pane--translate { flex: none; min-width: ${MIN_TRANSLATE}px; max-width: calc(100% - ${MIN_AI}px);
                                      border-radius: 0 var(--pop-radius) var(--pop-radius) 0;
                                      box-shadow: 0.5px 0 0 var(--cn-separator), 6px 0 20px -8px rgba(0,0,0,0.28); }
        .pop-pane--ai { flex: 1; z-index: 1; margin-left: -${OVERLAP}px; padding-left: ${OVERLAP}px; animation: pop-slide 220ms var(--cn-ease-out); }
        @keyframes pop-slide { from { opacity: 0; transform: translateX(-48px); } }
        /* 两边的标题行对齐：同样的上边距、同样高的下拉框 */
        .pop-pane--ai .ai__bar { padding: var(--cn-space-3) var(--cn-space-3) 6px; }
        .pop-pane--ai .ai__input { background: var(--pop-top); }
        /* 缝上的拖动条：平时看不见，悬停时出现一根竖胶囊 */
        .pop-seam { position: absolute; top: 0; bottom: 0; width: 10px; z-index: 3; cursor: col-resize; touch-action: none; }
        .pop-seam::after { content: ''; position: absolute; left: 50%; top: 50%; width: 4px; height: 40px; border-radius: 2px;
                           transform: translate(-50%, -50%); background: var(--cn-label-tertiary); opacity: 0; transition: opacity 120ms; }
        .pop-seam:hover::after { opacity: 0.8; }
        .pop-seam[data-dragging]::after { opacity: 1; background: var(--cn-accent); }
        .pop__grip { position: absolute; right: 0; bottom: 0; z-index: 4; width: 14px; height: 14px; cursor: nwse-resize;
                     background: linear-gradient(135deg, transparent 55%, var(--cn-label-tertiary) 55%, var(--cn-label-tertiary) 62%, transparent 62%, transparent 75%, var(--cn-label-tertiary) 75%, var(--cn-label-tertiary) 82%, transparent 82%); opacity: 0.5; }
        .pop__grip:hover { opacity: 1; }
      `}</style>
    </div>
  );
}
