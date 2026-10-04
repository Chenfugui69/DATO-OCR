// AI 对话组件：划词面板、识字窗口、独立 AI 窗口共用。
//
// 上下文（选中的文字 / 识字结果 / 截图）附在第一条提问上一起发出去；快捷提问里的 {text}
// 直接换成上下文文字。回答是流式的，随时可以停。对话只留在这个窗口的内存里。

import './ai.css';

import * as Popover from '@radix-ui/react-popover';
import { ArrowUp, Brain, ChevronRight, Copy, FileText, ImageIcon, RotateCw, Settings2, Square, SquarePen, X } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { ai, capture, system } from '@/lib/ipc';
import { renderMarkdown } from '@/lib/markdown';
import { assetUrl, shotUrl } from '@/lib/platform';
import { useSettings, useSettingsStore } from '@/lib/settings';
import { THINKING_LEVELS, type ChatImage, type ChatMessage, type ChatTurn, type ThinkingLevel } from '@/lib/types';
import { Button, EmptyState, IconButton, Select, Slider } from '@/ui/controls';
import { notify } from '@/ui/overlays';

export interface ChatContext {
  text?: string | null;
  images: ChatImage[];
  /** 上下文从哪来，决定标签怎么写：选中的文字 / 识字结果 */
  source?: 'selection' | 'ocr' | 'capture' | 'free';
}

type Turn = ChatTurn;

const newId = () => Math.random().toString(36).slice(2, 10);

export function imageUrl(img: ChatImage): string | undefined {
  return img.kind === 'store' ? shotUrl(img.id) : assetUrl(img.path);
}

/** 把上下文和提问拼成发给模型的一条消息。 */
function compose(question: string, context: ChatContext | null, template?: string): string {
  const text = context?.text?.trim();
  // 只带了截图没有文字时，快捷提问照样能用：{text} 指向图片
  if (template) return template.split('{text}').join(text ?? (context?.images.length ? '（见附图）' : ''));
  if (!text) return question;
  return `以下是参考内容：\n\n<context>\n${text}\n</context>\n\n${question}`;
}

/** 思考深度：顶栏上的小胶囊，点开是一条分档滑杆。改了就存进设置，所有对话窗口一起变。 */
function ThinkingPicker({ value }: { value: ThinkingLevel }) {
  const { t } = useTranslation();
  const index = Math.max(0, THINKING_LEVELS.indexOf(value));
  const setLevel = (level: ThinkingLevel) =>
    void useSettingsStore
      .getState()
      .update((d) => void (d.ai.thinking = level))
      .catch(notify.error);
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button type="button" className="ai-think-chip" data-level={value} title={t('ai.thinking')}>
          <Brain size={13} strokeWidth={1.75} />
          <span>{t(`ai.thinkingLevels.${value}`)}</span>
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content className="cn-popover ai-think-pop" side="bottom" align="start" sideOffset={6} collisionPadding={8}>
          <div className="ai-think-pop__head">
            <span>{t('ai.thinking')}</span>
            <b>{t(`ai.thinkingLevels.${value}`)}</b>
          </div>
          <Slider
            value={index}
            min={0}
            max={THINKING_LEVELS.length - 1}
            width={212}
            label={t('ai.thinking')}
            onChange={(v) => setLevel(THINKING_LEVELS[v] ?? 'auto')}
          />
          <div className="ai-think-pop__ticks">
            {THINKING_LEVELS.map((l) => (
              <button key={l} type="button" className={l === value ? 'is-on' : undefined} onClick={() => setLevel(l)}>
                {t(`ai.thinkingLevels.${l}`)}
              </button>
            ))}
          </div>
          <p className="ai-think-pop__desc">{t('ai.thinkingDesc')}</p>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** 回答上方可折叠的思考过程。还在想的时候自动展开，开始回答后自动收起（用户点过就听用户的）。 */
function Thought({ text, active }: { text: string; active: boolean }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState<boolean | null>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const shown = open ?? active;
  useLayoutEffect(() => {
    const el = bodyRef.current;
    if (el && active) el.scrollTop = el.scrollHeight;
  }, [text, active]);
  return (
    <div className={`ai-think${shown ? ' is-open' : ''}${active ? ' is-active' : ''}`}>
      <button type="button" className="ai-think__head" onClick={() => setOpen(!shown)}>
        <Brain size={13} strokeWidth={1.75} />
        <span>{active ? t('ai.thinkingNow') : t('ai.thought')}</span>
        <ChevronRight size={12} className="ai-think__chev" />
      </button>
      {shown && (
        <div ref={bodyRef} className="ai-think__body cn-selectable">
          {text}
        </div>
      )}
    </div>
  );
}

export function AiChat({
  context: initial,
  resetKey,
  compact = false,
  toolbar,
  initialTurns,
  onTurnsChange,
  ask: pendingAsk,
}: {
  context: ChatContext | null;
  /** 变了就开新对话（换了上下文） */
  resetKey: string | number;
  compact?: boolean;
  /** 顶栏右侧附加按钮（比如划词面板里的"返回翻译"） */
  toolbar?: ReactNode;
  /** 接着已有的对话聊（从划词面板挪到独立窗口时） */
  initialTurns?: ChatTurn[];
  onTurnsChange?: (turns: ChatTurn[]) => void;
  /** 打开就直接问这句（划词面板底部输入框里打的字）；id 变了才算新的一问 */
  ask?: { id: number; text: string } | null;
}) {
  const { t } = useTranslation();
  const settings = useSettings();
  const cfg = settings?.ai;
  const [context, setContext] = useState<ChatContext | null>(initial);
  const [turns, setTurns] = useState<Turn[]>([]);
  const [input, setInput] = useState('');
  const [model, setModel] = useState('');
  const running = useRef<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    setContext(initial);
    setTurns((initialTurns ?? []).map((m) => ({ ...m, streaming: false })));
    setInput('');
    if (running.current) void ai.cancel(running.current);
    running.current = null;
    window.setTimeout(() => inputRef.current?.focus(), 50);
    // initial 跟着 resetKey 一起变，只认 resetKey
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resetKey]);

  const models = useMemo(
    () =>
      (cfg?.providers ?? [])
        .filter((p) => p.enabled)
        .flatMap((p) => p.models.map((m) => ({ value: `${p.id}/${m}`, label: `${p.name} · ${m}` }))),
    [cfg?.providers],
  );
  const activeModel = models.some((m) => m.value === model) ? model : models.some((m) => m.value === cfg?.defaultModel) ? cfg!.defaultModel : (models[0]?.value ?? '');

  useEffect(() => onTurnsChange?.(turns), [turns, onTurnsChange]);

  const thinking: ThinkingLevel = cfg?.thinking ?? 'auto';

  // 新内容到了就滚到底，除非用户自己往上翻了
  useLayoutEffect(() => {
    const el = listRef.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [turns]);

  const ask = useCallback(
    async (question: string, template?: string, history: Turn[] = turns) => {
      if (!question.trim() && !template) return;
      if (running.current) return;
      const first = history.length === 0;
      const user: Turn = {
        id: newId(),
        role: 'user',
        content: template ? question : question.trim(),
        sent: first ? compose(question.trim(), context, template) : template ? compose('', context, template) : question.trim(),
        images: first ? context?.images : undefined,
        template,
      };
      const reply: Turn = { id: newId(), role: 'assistant', content: '', streaming: true };
      const next = [...history, user];
      setTurns([...next, reply]);
      setInput('');
      stick.current = true;
      const requestId = newId();
      running.current = requestId;
      const messages: ChatMessage[] = next
        .filter((m) => !m.error)
        .map((m) => ({ role: m.role, content: m.sent ?? m.content, images: m.images ?? [] }));
      const patch = (fn: (r: Turn) => Turn) => setTurns((all) => all.map((m) => (m.id === reply.id ? fn(m) : m)));
      try {
        await ai.chat({ id: requestId, model: activeModel || null, messages, thinking }, (e) => {
          if (e.type === 'start') patch((r) => ({ ...r, model: e.model }));
          else if (e.type === 'delta') patch((r) => ({ ...r, content: r.content + e.text }));
          else if (e.type === 'reasoning') patch((r) => ({ ...r, reasoning: (r.reasoning ?? '') + e.text }));
          else if (e.type === 'notice') patch((r) => ({ ...r, notice: e.message }));
          else {
            if (e.type === 'error') patch((r) => ({ ...r, error: e.message, streaming: false }));
            else patch((r) => ({ ...r, streaming: false }));
            if (running.current === requestId) running.current = null;
          }
        });
      } catch (err) {
        patch((r) => ({ ...r, error: err instanceof Error ? err.message : String(err), streaming: false }));
        running.current = null;
      }
    },
    [turns, context, activeModel, thinking],
  );

  // 外面递进来的提问：在"开新对话"那个副作用之后执行（声明顺序靠后），不会被它清掉
  const askRef = useRef(ask);
  askRef.current = ask;
  useEffect(() => {
    if (pendingAsk?.text) void askRef.current(pendingAsk.text);
  }, [pendingAsk?.id, pendingAsk?.text]);

  const stop = () => {
    if (!running.current) return;
    void ai.cancel(running.current);
    running.current = null;
    setTurns((all) => all.map((m) => (m.streaming ? { ...m, streaming: false } : m)));
  };

  const regenerate = () => {
    const lastUser = [...turns].reverse().find((m) => m.role === 'user');
    if (!lastUser || running.current) return;
    void ask(lastUser.content, lastUser.template, turns.slice(0, turns.indexOf(lastUser)));
  };

  const busy = turns.some((m) => m.streaming);
  const style = { '--ai-font': `${cfg?.panel.fontSize ?? 14}px` } as React.CSSProperties;

  if (cfg && models.length === 0) {
    return (
      <div className="ai ai--empty" style={style}>
        {toolbar && <div className="ai__bar">{toolbar}</div>}
        <EmptyState
          icon={Settings2}
          title={t('ai.noModel')}
          description={t('ai.noModelDesc')}
          action={<Button onClick={() => void system.showMain('settings:ai')}>{t('ai.openSettings')}</Button>}
        />
      </div>
    );
  }

  const hasContext = !!context?.text?.trim() || (context?.images.length ?? 0) > 0;
  return (
    <div className={compact ? 'ai ai--compact' : 'ai'} data-layout={cfg?.panel.layout ?? 'bubble'} style={style}>
      <div className="ai__bar">
        <Select value={activeModel} width={compact ? 168 : 220} options={models} onChange={setModel} />
        <ThinkingPicker value={thinking} />
        <span style={{ flex: 1 }} />
        <IconButton icon={SquarePen} size="sm" label={t('ai.newChat')} disabled={busy || turns.length === 0} onClick={() => setTurns([])} />
        {toolbar}
      </div>

      {hasContext && (
        <div className="ai__context">
          {context?.text?.trim() && (
            <span className="ai-chip" title={context.text}>
              <FileText size={13} strokeWidth={1.75} />
              <span className="ai-chip__text">
                {t(context.source === 'ocr' ? 'ai.contextOcr' : 'ai.contextText', { count: context.text.trim().length })}
              </span>
              {turns.length === 0 && (
                <button type="button" aria-label={t('common.remove')} onClick={() => setContext({ images: context.images, source: context.source })}>
                  <X size={12} />
                </button>
              )}
            </span>
          )}
          {context?.images.map((img, i) => (
            <span key={i} className="ai-chip ai-chip--image">
              {imageUrl(img) ? <img src={imageUrl(img)} alt="" /> : <ImageIcon size={13} />}
              {turns.length === 0 && (
                <button
                  type="button"
                  aria-label={t('common.remove')}
                  onClick={() => setContext({ text: context.text, images: context.images.filter((_, k) => k !== i) })}
                >
                  <X size={12} />
                </button>
              )}
            </span>
          ))}
        </div>
      )}

      <div
        ref={listRef}
        className="ai__list"
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
        onClick={(e) => {
          // 回答里的链接用系统浏览器打开，不在窗口里跳走
          const a = (e.target as HTMLElement).closest('a');
          if (a?.href) {
            e.preventDefault();
            void system.openUrl(a.href);
          }
        }}
      >
        {turns.length === 0 ? (
          <div className="ai__hello">{hasContext ? t('ai.helloContext') : t('ai.hello')}</div>
        ) : (
          turns.map((m, i) => (
            <div key={m.id} className={`ai-msg ai-msg--${m.role}`}>
              {m.role === 'user' ? (
                <div className="ai-msg__body cn-selectable">{m.content}</div>
              ) : (
                <>
                  {m.reasoning && <Thought text={m.reasoning} active={!!m.streaming && !m.content} />}
                  {m.content ? (
                    <div className="ai-msg__body ai-md cn-selectable" dangerouslySetInnerHTML={{ __html: renderMarkdown(m.content) }} />
                  ) : (
                    m.streaming && !m.reasoning && <div className="ai-msg__body ai-typing"><span /><span /><span /></div>
                  )}
                  {m.notice && <div className="ai-msg__notice">{m.notice}</div>}
                  {m.error && <div className="ai-msg__error">{m.error}</div>}
                  {!m.streaming && (
                    <div className="ai-msg__foot">
                      <span className="ai-msg__model">{m.model}</span>
                      {m.content && (
                        <IconButton
                          icon={Copy}
                          size="sm"
                          label={t('ai.copy')}
                          onClick={() =>
                            void capture
                              .writeText(m.content)
                              .then(() => notify.success(t('ai.copied')))
                              .catch(notify.error)
                          }
                        />
                      )}
                      {i === turns.length - 1 && <IconButton icon={RotateCw} size="sm" label={t('ai.regenerate')} onClick={regenerate} />}
                    </div>
                  )}
                </>
              )}
            </div>
          ))
        )}
      </div>

      {hasContext && turns.length === 0 && (cfg?.quickPrompts.length ?? 0) > 0 && (
        <div className="ai__quick">
          {cfg!.quickPrompts.map((q) => (
            <button key={q.id} type="button" className="ai-quick__chip" onClick={() => void ask(q.label, q.prompt)}>
              {q.label}
            </button>
          ))}
        </div>
      )}

      <div className="ai__input">
        <textarea
          ref={inputRef}
          value={input}
          rows={1}
          placeholder={t('ai.placeholder')}
          onChange={(e) => {
            setInput(e.target.value);
            const el = e.target;
            el.style.height = 'auto';
            el.style.height = `${Math.min(el.scrollHeight, 140)}px`;
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void ask(input);
            }
          }}
        />
        {busy ? (
          <IconButton icon={Square} label={t('ai.stop')} className="ai__send" onClick={stop} />
        ) : (
          <IconButton icon={ArrowUp} label={t('ai.send')} className="ai__send" disabled={!input.trim()} onClick={() => void ask(input)} />
        )}
      </div>
    </div>
  );
}
