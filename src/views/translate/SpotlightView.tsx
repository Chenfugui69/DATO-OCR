// 输入翻译（窗口见 Rust 的 translate::spotlight）：像 macOS 的聚焦搜索，屏幕中间偏上一条大输入框，
// 打字停一下（或按回车）下面就出各个源的译文。窗口高度跟着内容走：页面量好自己多高，告诉窗口改大小，
// 顶边不动、往下长。Esc 或点别处收起；下次打开时上次的字还在、整段选中，直接打字就替换掉。

import '@/views/ocr/ocr.css';

import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import { Languages } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';

import { TranslateBox } from './TranslateBox';

/** 四周给阴影留的透明边（CSS 像素），和 Rust 那边窗口宽度里算的一致 */
const PAD = 24;
/** 窗口宽度（逻辑像素），和 Rust 的 spotlight::WIDTH 一致 */
const WIDTH = 720;
/** 打字停多久才翻译 */
const DEBOUNCE_MS = 450;

export default function SpotlightView() {
  const { t } = useTranslation();
  const [text, setText] = useState('');
  const [query, setQuery] = useState('');
  const input = useRef<HTMLInputElement>(null);
  const panel = useRef<HTMLDivElement>(null);

  const focusInput = useCallback(() => {
    const el = input.current;
    if (!el) return;
    el.focus();
    el.select();
  }, []);
  useEffect(focusInput, [focusInput]);
  useEvent('spotlight-show', () => {
    // 窗口刚显示：等一帧再拿焦点（不然偶尔落空）
    requestAnimationFrame(focusInput);
  });

  // 打字停一下再翻
  useEffect(() => {
    const id = window.setTimeout(() => setQuery(text.trim()), DEBOUNCE_MS);
    return () => window.clearTimeout(id);
  }, [text]);

  const hide = useCallback(() => void getCurrentWindow().hide(), []);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        // 下拉框（语言）开着时 Esc 先关下拉框
        if (document.querySelector('[data-radix-popper-content-wrapper], [role="listbox"]')) return;
        e.preventDefault();
        hide();
      }
    };
    window.addEventListener('keydown', onKey);
    const unlisten = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused) hide();
    });
    return () => {
      window.removeEventListener('keydown', onKey);
      void unlisten.then((fn) => fn());
    };
  }, [hide]);

  // 窗口高度跟着面板：量好面板多高，加上四周的阴影边
  useLayoutEffect(() => {
    const el = panel.current;
    if (!el) return;
    let last = 0;
    const fit = () => {
      const h = Math.ceil(el.getBoundingClientRect().height) + PAD * 2;
      if (h === last) return;
      last = h;
      void getCurrentWindow()
        .setSize(new LogicalSize(WIDTH, h))
        .catch(() => undefined);
    };
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 译文区最多多高（超出在里面滚）：屏幕可用高度的六成
  const maxResults = Math.max(240, Math.round(window.screen.availHeight * 0.6) - 72 - PAD * 2);

  return (
    <div className="sp-root" style={{ padding: PAD }}>
      <div ref={panel} className="sp-panel" data-has-results={query ? '' : undefined}>
        <div className="sp-bar" data-tauri-drag-region>
          <Languages className="sp-bar__icon" size={22} strokeWidth={1.8} />
          <input
            ref={input}
            className="sp-input"
            value={text}
            placeholder={t('translate.spotlightPlaceholder')}
            spellCheck={false}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              // 回车：不等了，马上翻
              if (e.key === 'Enter' && !e.nativeEvent.isComposing) setQuery(text.trim());
            }}
          />
          <kbd className="sp-kbd">Esc</kbd>
        </div>
        {query && (
          <div className="sp-results" style={{ maxHeight: maxResults }}>
            <TranslateBox text={query} compact />
          </div>
        )}
      </div>
      <style>{`
        html[data-view='spotlight'], html[data-view='spotlight'] body { background: transparent !important; overflow: hidden; }
        .sp-root { position: fixed; left: 0; top: 0; width: 100%; }
        .sp-panel {
          display: flex; flex-direction: column; overflow: hidden;
          border-radius: 18px; background: var(--cn-bg-elevated, var(--cn-bg));
          box-shadow: 0 0 0 0.5px var(--cn-separator), 0 2px 6px rgba(0,0,0,0.1), 0 14px 30px -6px rgba(0,0,0,0.32);
          animation: sp-in 160ms cubic-bezier(0.2, 0.9, 0.3, 1.1);
        }
        [data-theme='light'] .sp-panel { box-shadow: 0 0 0 0.5px rgba(0,0,0,0.16), 0 2px 6px rgba(0,0,0,0.08), 0 14px 30px -6px rgba(0,0,0,0.24); }
        @keyframes sp-in { from { opacity: 0; transform: scale(0.98) translateY(-4px); } }
        .sp-bar { display: flex; align-items: center; gap: 14px; height: 68px; padding: 0 18px 0 22px; flex: none; }
        .sp-bar__icon { color: var(--cn-accent); flex: none; }
        .sp-input {
          flex: 1; min-width: 0; height: 100%; border: 0; outline: 0; background: transparent;
          font: 400 22px/1.2 var(--cn-font-sans); color: var(--cn-label); letter-spacing: 0.01em;
        }
        .sp-input::placeholder { color: var(--cn-label-tertiary); }
        .sp-kbd {
          flex: none; height: 22px; padding: 0 7px; display: inline-grid; place-items: center; border-radius: 6px;
          font: 500 11px/1 var(--cn-font-sans); color: var(--cn-label-tertiary); background: var(--cn-fill-tertiary, rgba(127,127,127,0.12));
        }
        .sp-results {
          display: flex; flex-direction: column; min-height: 0;
          border-top: 0.5px solid var(--cn-separator);
          animation: sp-results-in 200ms var(--cn-ease-out);
        }
        @keyframes sp-results-in { from { opacity: 0; } }
        .sp-results .tr-box { height: auto; flex: 1 1 auto; min-height: 0; padding: 10px 14px 14px; gap: 10px; }
        .sp-results .tr-box__cards { flex: 1 1 auto; min-height: 0; overflow-y: auto; gap: 8px; }
        .sp-results .tr-card { border-radius: 12px; padding: 8px 10px 10px 12px; background: var(--cn-fill-quaternary); box-shadow: inset 0 0 0 1px rgba(0,0,0,0.05); }
        [data-theme='dark'] .sp-results .tr-card { background: rgba(255,255,255,0.06); box-shadow: inset 0 0 0 1px rgba(255,255,255,0.06); }
        .sp-results .tr-card__body { font-size: 15px; line-height: 1.55; }
        .sp-results .tr-box__langs .cn-select-trigger { height: 28px; border-radius: 999px; background: var(--cn-fill-quaternary); box-shadow: none; }
      `}</style>
    </div>
  );
}
