// 划词翻译面板：鼠标 / 划词按钮旁边弹出，点外面或 Esc 关闭。
//
// - 顶部默认不显示原文（设置里可开），语言那一行右侧是"问 AI"和关闭；底部一条"问 AI"输入框
// - 外观（宽高、字号、不透明度、圆角、毛玻璃）按设置；右下角手柄拖拽改大小，改完记进设置
// - 问 AI 有两种展开方式（设置里选）：
//   - 抽屉（默认）：面板往下长高，对话区像一张卡片从底部往上滑出来，压在翻译上面；
//     抽屉顶上的小横条可以上下拖，调翻译和对话各占多少
//   - 侧边：面板往右长出对话区，翻译卡片压在对话区上面；中间的缝可以左右拖
//   对话期间点别处不收起（不然对话就没了），也可以挪到独立窗口里接着聊
// - 每块区域只涂一层底色（卡片圆角外露出来的那一小块用径向渐变补上），调不透明度时
//   各处一样透，不会因为叠了两层而一块透一块不透

import '@/views/ocr/ocr.css';

import { LogicalPosition, LogicalSize } from '@tauri-apps/api/dpi';
import { currentMonitor, getCurrentWindow } from '@tauri-apps/api/window';
import { ArrowUp, ChevronDown, ExternalLink, Sparkles, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { ai, translate } from '@/lib/ipc';
import { useSettings, useSettingsStore } from '@/lib/settings';
import type { ChatTurn } from '@/lib/types';
import { IconButton } from '@/ui/controls';
import { AiChat } from '@/views/ai/AiChat';

import { TranslateBox } from './TranslateBox';

/** 窗口四周留给阴影的透明边（毛玻璃模式下阴影是系统画的，不留） */
const SHADOW_PAD = 10;
/** 底部"问 AI"输入框占的高度 */
const ASK_H = 44;
const MIN_AI = 280;
const MIN_TRANSLATE = 360;
const MIN_TRANSLATE_H = 110;
const MIN_DRAWER = 200;

type Layout = 'drawer' | 'side';

const logicalWindowSize = () => ({ w: window.innerWidth, h: window.innerHeight });

/** 按住拖动，松手回调（用来保存）。 */
function dragHandle(e: React.PointerEvent<HTMLElement>, move: (ev: PointerEvent) => void, done: () => void) {
  e.preventDefault();
  const handle = e.currentTarget;
  handle.setPointerCapture(e.pointerId);
  handle.dataset.dragging = '';
  const up = () => {
    window.removeEventListener('pointermove', move);
    window.removeEventListener('pointerup', up);
    delete handle.dataset.dragging;
    done();
  };
  window.addEventListener('pointermove', move);
  window.addEventListener('pointerup', up);
}

export default function TranslatePopup() {
  const { t } = useTranslation();
  const settings = useSettings();
  const popup = settings?.translate.popup;
  const blur = !!popup?.blur;
  const pad = blur ? 0 : SHADOW_PAD;
  const [text, setText] = useState('');
  const [seq, setSeq] = useState(0);
  const [mode, setMode] = useState<'translate' | 'ai'>('translate');
  /** 这次展开用的布局（展开后改设置不影响已经开着的） */
  const [layout, setLayout] = useState<Layout>('drawer');
  const [question, setQuestion] = useState('');
  const [pendingAsk, setPendingAsk] = useState<{ id: number; text: string } | null>(null);
  /** 侧边：翻译卡片宽度；抽屉：翻译区高度（逻辑像素） */
  const [transW, setTransW] = useState(440);
  const [transH, setTransH] = useState(240);
  const modeRef = useRef(mode);
  modeRef.current = mode;
  const layoutRef = useRef(layout);
  layoutRef.current = layout;
  const padRef = useRef(pad);
  padRef.current = pad;
  /** 用户在拖右下角手柄：只有这时的尺寸变化才记进设置（弹出时 Rust 摆位、展开 AI 也会改大小） */
  const userResizing = useRef(false);
  const resizeTimer = useRef<number | undefined>(undefined);
  /** 用户在拖面板挪位置（按住空白处拖）：系统拖动期间会短暂拿走焦点，这时不能当成"点了外面" */
  const userMoving = useRef(false);
  const moveTimer = useRef<number | undefined>(undefined);
  const turnsRef = useRef<ChatTurn[]>([]);
  const keepTurns = useCallback((turns: ChatTurn[]) => void (turnsRef.current = turns), []);
  const transRef = useRef<HTMLDivElement>(null);
  /** 展开对话前的窗口大小，收起时还原 */
  const baseSize = useRef({ w: 460, h: 300 });

  const adopt = useCallback((next: string) => {
    setText(next);
    setSeq((n) => n + 1);
    setMode('translate');
    setQuestion('');
    setPendingAsk(null);
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
    const settle = (ms: number) => {
      window.clearTimeout(moveTimer.current);
      moveTimer.current = window.setTimeout(() => {
        userMoving.current = false;
        // 拖完把焦点要回来，之后点外面才能照常收起
        const w = getCurrentWindow();
        void (async () => {
          if (!(await w.isFocused()) && (await w.isVisible())) await w.setFocus();
        })();
      }, ms);
    };
    const onDown = (e: PointerEvent) => {
      if (!(e.target as Element | null)?.closest?.('[data-tauri-drag-region]')) return;
      userMoving.current = true;
      settle(1500);
    };
    window.addEventListener('pointerdown', onDown, true);
    const win = getCurrentWindow();
    const unlisteners: Promise<() => void>[] = [
      // 翻译模式点外面就收起；AI 模式不收，免得对话没了
      // 拖手柄改大小、拖着挪位置时系统会短暂拿走焦点，这时也不收
      win.onFocusChanged(({ payload }) => !payload && modeRef.current === 'translate' && !userResizing.current && !userMoving.current && hide()),
      win.onMoved(() => userMoving.current && settle(400)),
      // 用户拖右下角改了大小：记进设置，下次按这个尺寸弹出
      win.onResized(() => {
        if (!userResizing.current) return;
        window.clearTimeout(resizeTimer.current);
        resizeTimer.current = window.setTimeout(() => {
          userResizing.current = false;
          const { w, h } = logicalWindowSize();
          const p = padRef.current;
          const box = transRef.current?.getBoundingClientRect();
          void useSettingsStore
            .getState()
            .update((d) => {
              if (modeRef.current === 'translate') {
                d.translate.popup.width = Math.round(w);
                d.translate.popup.height = Math.round(h);
              } else if (layoutRef.current === 'side') {
                d.ai.panel.width = Math.round(w - p * 2 - (box?.width ?? 0));
                d.ai.panel.height = Math.round(h);
              } else {
                d.translate.popup.width = Math.round(w);
                d.translate.popup.drawerHeight = Math.round(h - p * 2 - (box?.height ?? 0));
              }
            })
            .catch(() => undefined);
        }, 500);
      }),
    ];
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('pointerdown', onDown, true);
      for (const u of unlisteners) void u.then((fn) => fn());
    };
  }, [hide]);

  const resizeTo = async (w: number, h: number) => {
    userResizing.current = false;
    await getCurrentWindow().setSize(new LogicalSize(w, h));
  };

  const openAi = async (ask?: string) => {
    const next: Layout = popup?.aiLayout === 'side' ? 'side' : 'drawer';
    const panel = settings?.ai.panel;
    const win = getCurrentWindow();
    const scale = await win.scaleFactor();
    const base = logicalWindowSize();
    baseSize.current = base;
    const monitor = await currentMonitor();
    const screenW = monitor ? monitor.size.width / scale : 1920;
    const screenH = monitor ? monitor.size.height / scale : 1080;
    let w: number;
    let h: number;
    if (next === 'side') {
      const trans = Math.max(MIN_TRANSLATE, base.w - pad * 2);
      setTransW(trans);
      w = Math.min(screenW - 16, trans + pad * 2 + Math.max(MIN_AI, panel?.width ?? 520));
      h = Math.min(screenH - 16, Math.max(base.h, panel?.height ?? 620));
    } else {
      // 输入框收起，抽屉从底部接上；翻译区保持原来的高度
      const trans = Math.max(MIN_TRANSLATE_H, base.h - pad * 2 - ASK_H);
      w = Math.max(base.w, 380);
      h = Math.min(screenH - 16, pad * 2 + trans + (popup?.drawerHeight ?? 380));
      setTransH(Math.min(trans, h - pad * 2 - MIN_DRAWER));
    }
    setLayout(next);
    if (ask) setPendingAsk({ id: Date.now(), text: ask });
    setQuestion('');
    setMode('ai');
    await resizeTo(w, h);
    // 往右 / 往下展开超出屏幕了：整个窗口往回挪
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
    setPendingAsk(null);
    await resizeTo(baseSize.current.w, baseSize.current.h);
  };

  /** 侧边：拖翻译卡片和对话区之间的缝，窗口大小不变，两边此消彼长 */
  const dragSeam = (e: React.PointerEvent<HTMLSpanElement>) => {
    let last = transW;
    dragHandle(
      e,
      (ev) => {
        last = Math.round(Math.min(window.innerWidth - pad * 2 - MIN_AI, Math.max(MIN_TRANSLATE, ev.clientX - pad)));
        setTransW(last);
      },
      () => {
        baseSize.current = { ...baseSize.current, w: last + pad * 2 };
        void useSettingsStore
          .getState()
          .update((d) => {
            d.translate.popup.width = last + pad * 2;
            d.ai.panel.width = Math.round(window.innerWidth - pad * 2 - last);
          })
          .catch(() => undefined);
      },
    );
  };

  /** 抽屉：拖顶上的小横条，翻译区和对话区上下此消彼长 */
  const dragDrawer = (e: React.PointerEvent<HTMLSpanElement>) => {
    const offset = e.clientY - (pad + transH);
    let last = transH;
    dragHandle(
      e,
      (ev) => {
        last = Math.round(Math.min(window.innerHeight - pad * 2 - MIN_DRAWER, Math.max(MIN_TRANSLATE_H, ev.clientY - offset - pad)));
        setTransH(last);
      },
      () =>
        void useSettingsStore
          .getState()
          .update((d) => void (d.translate.popup.drawerHeight = Math.round(window.innerHeight - pad * 2 - last)))
          .catch(() => undefined),
    );
  };

  const popOut = () => {
    // 连同已经聊过的内容一起挪过去，接着聊
    void ai.open({ text, images: [], source: 'selection', history: turnsRef.current });
    hide();
  };

  const style = {
    '--pop-radius': `${popup?.radius ?? 14}px`,
    '--pop-alpha': String(popup?.opacity ?? 1),
    '--pop-pad': `${pad}px`,
  } as React.CSSProperties;

  const shellClass = ['pop-shell', mode === 'ai' ? `is-${layout}` : 'is-plain', blur ? 'is-blur' : ''].join(' ');
  const paneStyle = mode !== 'ai' ? undefined : layout === 'side' ? { width: transW } : { height: transH };

  return (
    <div className="pop-root" style={style}>
      <div className={shellClass}>
        <div ref={transRef} className="pop-pane pop-pane--translate" key={seq} style={paneStyle}>
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
          {mode === 'translate' && text && (
            <div className="pop-ask">
              <Sparkles size={14} strokeWidth={1.75} />
              <input
                value={question}
                placeholder={t('ai.askPlaceholder')}
                onChange={(e) => setQuestion(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !e.nativeEvent.isComposing && question.trim()) {
                    e.preventDefault();
                    void openAi(question.trim());
                  }
                }}
              />
              <button type="button" aria-label={t('ai.send')} disabled={!question.trim()} onClick={() => void openAi(question.trim())}>
                <ArrowUp size={14} strokeWidth={2.25} />
              </button>
            </div>
          )}
        </div>
        {mode === 'ai' && layout === 'side' && (
          <>
            <span className="pop-seam" style={{ left: transW - 5 }} title={t('translate.resize')} onPointerDown={dragSeam} />
            <div className="pop-pane pop-pane--ai">
              <AiChat
                compact
                context={{ text, images: [] }}
                resetKey={seq}
                onTurnsChange={keepTurns}
                ask={pendingAsk}
                toolbar={<IconButton icon={ExternalLink} size="sm" label={t('ai.popOut')} onClick={popOut} />}
              />
            </div>
          </>
        )}
        {mode === 'ai' && layout === 'drawer' && (
          <div className="pop-drawer">
            <span className="pop-drawer__grab" title={t('translate.resize')} onPointerDown={dragDrawer}>
              <i />
            </span>
            <AiChat
              compact
              context={{ text, images: [] }}
              resetKey={seq}
              onTurnsChange={keepTurns}
              ask={pendingAsk}
              toolbar={
                <>
                  <IconButton icon={ExternalLink} size="sm" label={t('ai.popOut')} onClick={popOut} />
                  <IconButton icon={ChevronDown} size="sm" label={t('ai.hide')} onClick={() => void closeAi()} />
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
      <style>{POPUP_CSS}</style>
    </div>
  );
}

const POPUP_CSS = `
html[data-view='translate'], html[data-view='translate'] body { background: transparent !important; }
.pop-root { position: fixed; inset: 0; padding: var(--pop-pad); display: flex; }

/* 两档底色：浮在上面的一层（hi）和压在下面的一层（lo）。透明窗口里 backdrop-filter 模糊不了
   桌面，不开毛玻璃时默认不透明；不透明度可在设置里调 */
.pop-shell { position: relative; flex: 1; min-width: 0; display: flex; border-radius: var(--pop-radius); overflow: hidden;
             box-shadow: 0 8px 28px rgba(0,0,0,0.26), 0 0 0 0.5px rgba(0,0,0,0.28); animation: cn-pop-in 160ms var(--cn-ease-out);
             --pop-hi: rgba(252,252,254,var(--pop-alpha)); --pop-lo: rgba(236,236,241,var(--pop-alpha));
             --r: min(var(--pop-radius), 16px); }
[data-theme='dark'] .pop-shell { --pop-hi: rgba(46,46,48,var(--pop-alpha)); --pop-lo: rgba(30,30,32,var(--pop-alpha)); }
/* 毛玻璃：系统亚克力在窗口背后，圆角和阴影都是系统的，这里只铺一层半透明的色调 */
.pop-shell.is-blur { border-radius: 0; box-shadow: none; animation: none; --r: 12px; }
.pop-shell.is-drawer { flex-direction: column; }

.pop-pane { position: relative; min-width: 0; min-height: 0; display: flex; flex-direction: column; }
.pop-pane--translate { flex: 1; z-index: 2; background: var(--pop-hi); }
.pop__source { max-height: 66px; overflow: hidden; padding: 12px 14px 8px; font: var(--cn-text-callout); color: var(--cn-label-secondary);
               display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical; box-shadow: inset 0 -0.5px 0 var(--cn-separator); flex: none; }
.pop__body { flex: 1; min-height: 0; }

/* 底部"问 AI"输入框 */
.pop-ask { flex: none; display: flex; align-items: center; gap: 8px; height: 32px; margin: 2px 12px 10px; padding: 0 4px 0 11px;
           border-radius: 16px; background: var(--cn-fill-quaternary); box-shadow: inset 0 0 0 0.5px var(--cn-separator); color: var(--cn-accent); }
.pop-ask:focus-within { box-shadow: inset 0 0 0 1px var(--cn-accent); }
.pop-ask input { flex: 1; min-width: 0; height: 100%; border: none; outline: none; background: transparent; color: var(--cn-label); font: var(--cn-text-body); }
.pop-ask input::placeholder { color: var(--cn-label-tertiary); }
.pop-ask button { flex: none; display: grid; place-items: center; width: 24px; height: 24px; padding: 0; border: none; border-radius: 50%;
                  background: var(--cn-accent); color: #fff; cursor: pointer; transition: opacity 120ms; }
.pop-ask button:disabled { opacity: 0.3; cursor: default; }

/* ── 侧边：翻译卡片在上层，右边圆角外露出下层的颜色；阴影只投在右边 ── */
.is-side .pop-pane--translate { flex: none; min-width: ${MIN_TRANSLATE}px; max-width: calc(100% - ${MIN_AI}px);
  background:
    radial-gradient(circle at 0 100%, var(--pop-hi) calc(var(--r) - 0.6px), var(--pop-lo) var(--r)) right top / var(--r) var(--r) no-repeat,
    radial-gradient(circle at 0 0, var(--pop-hi) calc(var(--r) - 0.6px), var(--pop-lo) var(--r)) right bottom / var(--r) var(--r) no-repeat,
    linear-gradient(var(--pop-hi), var(--pop-hi)) left top / calc(100% - var(--r)) 100% no-repeat,
    linear-gradient(var(--pop-hi), var(--pop-hi)) right center / var(--r) calc(100% - 2 * var(--r)) no-repeat; }
.is-side .pop-pane--translate::after { content: ''; position: absolute; inset: 0; border-radius: 0 var(--r) var(--r) 0; pointer-events: none;
  box-shadow: 0.5px 0 0 var(--cn-separator), 8px 0 18px -10px rgba(0,0,0,0.4); clip-path: inset(0 -40px 0 calc(100% - var(--r) - 1px)); }
.pop-pane--ai { flex: 1; z-index: 1; background: var(--pop-lo); animation: pop-slide 220ms var(--cn-ease-out); }
@keyframes pop-slide { from { opacity: 0; transform: translateX(-48px); } }
.pop-pane--ai .ai__bar { padding: var(--cn-space-3) var(--cn-space-3) 6px; }

/* ── 抽屉：翻译区退到下层，对话卡片从底部滑上来盖住它的下沿 ── */
.is-drawer .pop-pane--translate { flex: none; background: var(--pop-lo); }
.pop-drawer { position: relative; z-index: 3; flex: 1; min-height: 0; display: flex; flex-direction: column;
  background:
    radial-gradient(circle at 100% 100%, var(--pop-hi) calc(var(--r) - 0.6px), var(--pop-lo) var(--r)) left top / var(--r) var(--r) no-repeat,
    radial-gradient(circle at 0 100%, var(--pop-hi) calc(var(--r) - 0.6px), var(--pop-lo) var(--r)) right top / var(--r) var(--r) no-repeat,
    linear-gradient(var(--pop-hi), var(--pop-hi)) center top / calc(100% - 2 * var(--r)) var(--r) no-repeat,
    linear-gradient(var(--pop-hi), var(--pop-hi)) left bottom / 100% calc(100% - var(--r)) no-repeat;
  animation: drawer-up 280ms var(--cn-ease-out); }
.pop-drawer::before { content: ''; position: absolute; inset: 0; border-radius: var(--r) var(--r) 0 0; pointer-events: none;
  box-shadow: 0 -0.5px 0 var(--cn-separator), 0 -10px 22px -14px rgba(0,0,0,0.45); clip-path: inset(-40px 0 calc(100% - var(--r) - 1px) 0); }
@keyframes drawer-up { from { transform: translateY(100%); } }
.pop-drawer__grab { position: absolute; left: 0; right: 0; top: 0; height: 14px; z-index: 1; display: grid; place-items: center; cursor: row-resize; touch-action: none; }
.pop-drawer__grab i { width: 36px; height: 4px; border-radius: 2px; background: var(--cn-label-tertiary); opacity: 0.5; transition: opacity 120ms, background-color 120ms; }
.pop-drawer__grab:hover i { opacity: 0.9; }
.pop-drawer__grab[data-dragging] i { opacity: 1; background: var(--cn-accent); }
.pop-drawer .ai__bar { padding: 16px var(--cn-space-3) 6px; }
.pop-drawer .ai__input, .pop-pane--ai .ai__input { background: var(--cn-fill-quaternary); }

/* 侧边的缝：平时看不见，悬停时出现一根竖胶囊 */
.pop-seam { position: absolute; top: 0; bottom: 0; width: 10px; z-index: 4; cursor: col-resize; touch-action: none; }
.pop-seam::after { content: ''; position: absolute; left: 50%; top: 50%; width: 4px; height: 40px; border-radius: 2px;
                   transform: translate(-50%, -50%); background: var(--cn-label-tertiary); opacity: 0; transition: opacity 120ms; }
.pop-seam:hover::after { opacity: 0.8; }
.pop-seam[data-dragging]::after { opacity: 1; background: var(--cn-accent); }

.pop__grip { position: absolute; right: 0; bottom: 0; z-index: 5; width: 14px; height: 14px; cursor: nwse-resize;
             background: linear-gradient(135deg, transparent 55%, var(--cn-label-tertiary) 55%, var(--cn-label-tertiary) 62%, transparent 62%, transparent 75%, var(--cn-label-tertiary) 75%, var(--cn-label-tertiary) 82%, transparent 82%); opacity: 0.5; }
.pop__grip:hover { opacity: 1; }
`;
