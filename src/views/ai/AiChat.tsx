// AI 对话组件：划词面板、识字窗口、独立 AI 窗口共用。
//
// 上下文（选中的文字 / 识字结果 / 截图）附在第一条提问上一起发出去；快捷提问里的 {text}
// 直接换成上下文文字。回答是流式的，随时可以停。对话只留在这个窗口的内存里。

import './ai.css';

import clsx from 'clsx';
import * as Popover from '@radix-ui/react-popover';
import { ArrowUp, Brain, ChevronRight, Copy, FileText, ImageIcon, Plus, RotateCw, Settings2, Sparkles, Square, SquarePen, X } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';

import i18n from '@/i18n';

import { ai, capture, system } from '@/lib/ipc';
import { assetUrl, shotUrl } from '@/lib/platform';
import { useSettings, useSettingsStore } from '@/lib/settings';
import { THINKING_LEVELS, type ChatImage, type ChatMessage, type ChatTurn, type ThinkingLevel } from '@/lib/types';
import { Button, EmptyState, IconButton, Select, Slider } from '@/ui/controls';
import { notify } from '@/ui/overlays';
import { type OrbColor, ORB_COLORS, WaterOrb } from '@/views/clipboard-panel/WaterOrb';
import '@/views/clipboard-panel/orb.css';

import { StreamText } from './StreamText';

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
  if (template) return template.split('{text}').join(text ?? (context?.images.length ? i18n.t('ai.seeImage') : ''));
  if (!text) return question;
  return `${i18n.t('ai.contextIntro')}\n\n<context>\n${text}\n</context>\n\n${question}`;
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

/** 加进对话的图最长边不超过这么多像素（再大模型也用不上，白占内存和流量） */
const ATTACH_MAX_EDGE = 2048;

/** 把一个图片文件变成 data URL：让浏览器解码（webp、heic 这些后端不认的也行），太大的缩小。 */
async function fileToDataUrl(file: Blob): Promise<string> {
  const bitmap = await createImageBitmap(file);
  const scale = Math.min(1, ATTACH_MAX_EDGE / Math.max(bitmap.width, bitmap.height));
  const canvas = document.createElement('canvas');
  canvas.width = Math.max(1, Math.round(bitmap.width * scale));
  canvas.height = Math.max(1, Math.round(bitmap.height * scale));
  canvas.getContext('2d')?.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
  bitmap.close();
  // 小图、可能带透明的用 PNG，照片用 JPEG（PNG 的照片动辄十几兆）
  const small = canvas.width * canvas.height <= 1_200_000;
  return small && file.type !== 'image/jpeg' ? canvas.toDataURL('image/png') : canvas.toDataURL('image/jpeg', 0.92);
}

/**
 * 输入框上的图片小预览。鼠标移上去在它上方浮出一张大图（小图只有几十像素，看不出是哪张）。
 * 大图用 fixed 定位贴着窗口算位置，不会被输入框、列表的裁剪挡住。
 */
function Thumb({ url, onRemove, removeLabel }: { url: string | undefined; onRemove?: () => void; removeLabel?: string }) {
  const [zoom, setZoom] = useState<{ left: number; top?: number; bottom?: number; width: number; height: number } | null>(null);
  return (
    <span
      className="ai-chip ai-chip--image"
      onMouseEnter={(e) => {
        if (!url) return;
        const box = e.currentTarget.getBoundingClientRect();
        // 大图要够大才看得清：宽度几乎占满窗口，高度用缩略图上方（或下方，哪边地方大用哪边）的全部空间
        const above = box.top - 20;
        const below = window.innerHeight - box.bottom - 20;
        const up = above >= below;
        const width = Math.min(720, window.innerWidth - 24);
        const height = Math.max(140, Math.min(720, up ? above : below));
        const left = Math.min(Math.max(12, box.left), window.innerWidth - width - 12);
        setZoom(up ? { left, bottom: window.innerHeight - box.top + 8, width, height } : { left, top: box.bottom + 8, width, height });
      }}
      onMouseLeave={() => setZoom(null)}
    >
      {url ? <img src={url} alt="" /> : <ImageIcon size={13} />}
      {onRemove && (
        <button type="button" aria-label={removeLabel} onClick={onRemove}>
          <X size={12} />
        </button>
      )}
      {/* 挂到 body 上：留在缩略图里面的话，缩略图那几条"图片高 34px"的样式会把大图也压成小图 */}
      {zoom &&
        url &&
        createPortal(
          <span className="ai-zoom" style={{ left: zoom.left, top: zoom.top, bottom: zoom.bottom, width: zoom.width, height: zoom.height }}>
            <img src={url} alt="" />
          </span>,
          document.body,
        )}
    </span>
  );
}

/** 回答上方可折叠的思考过程。默认收着：还在想的时候只在这一行上放动效（图标呼吸、文字流光），想看内容自己点开。 */
function Thought({ text, active }: { text: string; active: boolean }) {
  const { t } = useTranslation();
  const [shown, setOpen] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
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
  /** 用户自己加的、还没发出去的图（点加号选的、拖进来的、粘贴的），跟下一条提问一起发 */
  const [attach, setAttach] = useState<ChatImage[]>([]);
  const [dropping, setDropping] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const [model, setModel] = useState('');
  const running = useRef<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    setContext(initial);
    setTurns((initialTurns ?? []).map((m) => ({ ...m, streaming: false })));
    setInput('');
    setAttach([]);
    if (running.current) void ai.cancel(running.current);
    running.current = null;
    // 不让浏览器为了露出输入框去滚动外层（划词面板的抽屉还在滑进来的路上，一滚整块内容就跳）
    window.setTimeout(() => inputRef.current?.focus({ preventScroll: true }), 50);
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

  const streamingNow = turns.some((m) => m.streaming);
  // 不在回复的时候（刚打开、换了一段对话）：直接到底。还没聊过（开场的说明、带上的内容、快捷提问）
  // 从顶上看起：窄的对话栏里放不下时，以前一打开就滚到最底下，标题和说明被卷到上面看不见
  useLayoutEffect(() => {
    const el = listRef.current;
    if (!el) return;
    if (turns.length === 0) el.scrollTop = 0;
    else if (stick.current && !streamingNow) el.scrollTop = el.scrollHeight;
  }, [turns, streamingNow]);
  // 正在回复：页面平滑地跟到底部（气泡是平滑长高的，一下子跳到底会一顿一顿）。
  // 每帧往底部靠一截；用户自己往上翻了（stick = false）就不跟，翻回底部再接着跟
  useEffect(() => {
    if (!streamingNow) return;
    let raf = 0;
    const follow = () => {
      const el = listRef.current;
      if (el && stick.current) {
        const gap = el.scrollHeight - el.clientHeight - el.scrollTop;
        if (gap > 0.5) el.scrollTop += Math.max(1, gap * 0.22);
      }
      raf = requestAnimationFrame(follow);
    };
    raf = requestAnimationFrame(follow);
    return () => cancelAnimationFrame(raf);
  }, [streamingNow]);
  /** 用户最后一次动滚动条 / 滚轮 / 触控板 / 键盘的时刻：区分"用户在翻"和"我们自己在跟到底" */
  const userScrollAt = useRef(0);

  const ask = useCallback(
    async (question: string, template?: string, history: Turn[] = turns) => {
      const mine = attach;
      // 只发图不打字也行：补一句"见附图"
      if (!question.trim() && !template && mine.length > 0) question = t('ai.seeImage');
      if (!question.trim() && !template) return;
      if (running.current) return;
      const first = history.length === 0;
      const user: Turn = {
        id: newId(),
        role: 'user',
        content: template ? question : question.trim(),
        sent: first ? compose(question.trim(), context, template) : template ? compose('', context, template) : question.trim(),
        images: [...(first ? (context?.images ?? []) : []), ...mine],
        template,
      };
      const reply: Turn = { id: newId(), role: 'assistant', content: '', streaming: true };
      const next = [...history, user];
      setTurns([...next, reply]);
      setInput('');
      setAttach([]);
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
    [turns, context, activeModel, thinking, attach, t],
  );

  /** 加图片：选的、拖进来的、粘贴的都走这里。不是图片的文件跳过 */
  const addImages = useCallback(async (files: Iterable<File>) => {
    for (const file of files) {
      if (!file.type.startsWith('image/')) continue;
      try {
        const id = await ai.attachImage(await fileToDataUrl(file));
        setAttach((all) => (all.length >= 8 ? all : [...all, { kind: 'store', id }]));
      } catch (err) {
        notify.error(err);
      }
    }
  }, []);
  const removeAttach = (img: ChatImage) => {
    setAttach((all) => all.filter((x) => x !== img));
    if (img.kind === 'store') void ai.releaseImages([img.id]).catch(() => undefined);
  };

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
  // 独立窗口（不是划词面板里那个小的）：开场是一颗水珠 + 内容卡片 + 快捷提问卡片，模型选择挪进输入框底部
  const roomy = !compact;
  const orb: OrbColor = (ORB_COLORS as readonly string[]).includes(settings?.clipboard.orbColor ?? '') ? (settings!.clipboard.orbColor as OrbColor) : 'ocean';
  const contextChips = hasContext && (
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
        <Thumb
          key={i}
          url={imageUrl(img)}
          removeLabel={t('common.remove')}
          onRemove={turns.length === 0 ? () => setContext({ text: context.text, images: context.images.filter((_, k) => k !== i) }) : undefined}
        />
      ))}
    </div>
  );
  const modelPickers = (
    <>
      <Select value={activeModel} width={compact ? 168 : 200} options={models} onChange={setModel} />
      <ThinkingPicker value={thinking} />
    </>
  );
  /** 独立窗口的开场：水珠 + 一句话 + 带进来的内容 + 快捷提问 */
  const hero = (
    <div className="ai-hero">
      <WaterOrb color={orb} size={58} interactive />
      <h2 className="ai-hero__title">{hasContext ? t('ai.heroContext') : t('ai.heroTitle')}</h2>
      <p className="ai-hero__sub">{hasContext ? t('ai.heroContextSub') : t('ai.heroSub')}</p>
      {hasContext && (
        <div className="ai-hero__ctx">
          <div className="ai-hero__ctx-head">
            <FileText size={13} strokeWidth={1.9} />
            <span>
              {context?.text?.trim()
                ? t(context.source === 'ocr' ? 'ai.contextOcr' : 'ai.contextText', { count: context.text.trim().length })
                : t('ai.contextImage')}
            </span>
            <span style={{ flex: 1 }} />
            <button type="button" className="ai-hero__ctx-x" aria-label={t('common.remove')} onClick={() => setContext(null)}>
              <X size={13} />
            </button>
          </div>
          {context?.text?.trim() && <div className="ai-hero__ctx-text cn-selectable">{context.text.trim()}</div>}
          {(context?.images.length ?? 0) > 0 && (
            <div className="ai-hero__ctx-images">
              {context!.images.map((img, i) => (imageUrl(img) ? <img key={i} src={imageUrl(img)} alt="" /> : null))}
            </div>
          )}
        </div>
      )}
      {hasContext && (cfg?.quickPrompts.length ?? 0) > 0 && (
        <div className="ai-hero__prompts">
          {cfg!.quickPrompts.map((q) => (
            <button key={q.id} type="button" className="ai-hero__prompt" onClick={() => void ask(q.label, q.prompt)}>
              <Sparkles size={14} strokeWidth={2} />
              <span className="cn-truncate">{q.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
  return (
    <div className={compact ? 'ai ai--compact' : 'ai ai--roomy'} data-layout={cfg?.panel.layout ?? 'bubble'} style={style}>
      {roomy ? (
        <div className="ai__top">
          <span style={{ flex: 1 }} />
          <IconButton icon={SquarePen} size="sm" label={t('ai.newChat')} disabled={busy || turns.length === 0} onClick={() => setTurns([])} />
          {toolbar}
        </div>
      ) : (
        <div className="ai__bar">
          {modelPickers}
          <span style={{ flex: 1 }} />
          <IconButton icon={SquarePen} size="sm" label={t('ai.newChat')} disabled={busy || turns.length === 0} onClick={() => setTurns([])} />
          {toolbar}
        </div>
      )}

      {!roomy && contextChips}

      <div
        ref={listRef}
        className="ai__list"
        onWheel={(e) => {
          userScrollAt.current = performance.now();
          // 往上滚：马上停止跟随，不等滚动事件
          if (e.deltaY < 0) stick.current = false;
        }}
        onPointerDown={() => (userScrollAt.current = performance.now())}
        onTouchMove={() => (userScrollAt.current = performance.now())}
        onKeyDown={() => (userScrollAt.current = performance.now())}
        onScroll={(e) => {
          const el = e.currentTarget;
          const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
          // 回到底部：恢复跟随。离开底部只有是用户自己翻的才算（跟随时页面本来就落后底部一截）
          if (atBottom) stick.current = true;
          else if (performance.now() - userScrollAt.current < 800) stick.current = false;
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
          roomy ? hero : <div className="ai__hello">{hasContext ? t('ai.helloContext') : t('ai.hello')}</div>
        ) : (
          turns.map((m, i) => (
            <div key={m.id} className={`ai-msg ai-msg--${m.role}`}>
              {m.role === 'user' ? (
                <>
                  {/* 这一条带的图（第一条带进来的内容在上面的卡片 / 标签里已经有了，这里只列后来自己加的） */}
                  {(m.images?.length ?? 0) > 0 && !(i === 0 && hasContext) && (
                    <div className="ai-msg__images">
                      {m.images!.map((img, k) => (
                        <Thumb key={k} url={imageUrl(img)} />
                      ))}
                    </div>
                  )}
                  <div className="ai-msg__body cn-selectable">{m.content}</div>
                </>
              ) : (
                <>
                  {m.reasoning && <Thought text={m.reasoning} active={!!m.streaming && !m.content} />}
                  {m.content ? (
                    <StreamText className="ai-msg__body cn-selectable" content={m.content} streaming={!!m.streaming} />
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

      {!roomy && hasContext && turns.length === 0 && (cfg?.quickPrompts.length ?? 0) > 0 && (
        <div className="ai__quick">
          {cfg!.quickPrompts.map((q) => (
            <button key={q.id} type="button" className="ai-quick__chip" onClick={() => void ask(q.label, q.prompt)}>
              {q.label}
            </button>
          ))}
        </div>
      )}

      <div
        className={clsx(roomy ? 'ai__input ai__composer' : 'ai__input', dropping && 'is-dropping')}
        onDragOver={(e) => {
          if (![...e.dataTransfer.types].includes('Files')) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = 'copy';
          setDropping(true);
        }}
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropping(false);
        }}
        onDrop={(e) => {
          if (e.dataTransfer.files.length === 0) return;
          e.preventDefault();
          setDropping(false);
          void addImages(e.dataTransfer.files);
        }}
      >
        <input
          ref={fileRef}
          type="file"
          accept="image/*"
          multiple
          hidden
          onChange={(e) => {
            if (e.target.files) void addImages(e.target.files);
            e.target.value = '';
          }}
        />
        {/* 对话开始后，带进来的内容缩成输入框顶上的小标签 */}
        {roomy && turns.length > 0 && contextChips}
        {attach.length > 0 && (
          <div className="ai__attach">
            {attach.map((img) => (
              <Thumb key={img.kind === 'store' ? img.id : img.path} url={imageUrl(img)} removeLabel={t('common.remove')} onRemove={() => removeAttach(img)} />
            ))}
          </div>
        )}
        {!roomy && <IconButton icon={Plus} size="sm" label={t('ai.addImage')} className="ai__plus" onClick={() => fileRef.current?.click()} />}
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
          onPaste={(e) => {
            // 粘贴的是图片（截图、复制的图）：加成附图，不往输入框里塞东西
            const images = [...e.clipboardData.files].filter((f) => f.type.startsWith('image/'));
            if (images.length === 0) return;
            e.preventDefault();
            void addImages(images);
          }}
        />
        {roomy ? (
          <div className="ai__composer-foot">
            <IconButton icon={Plus} size="sm" label={t('ai.addImage')} className="ai__plus" onClick={() => fileRef.current?.click()} />
            {modelPickers}
            <span style={{ flex: 1 }} />
            {busy ? (
              <IconButton icon={Square} label={t('ai.stop')} className="ai__send" onClick={stop} />
            ) : (
              <IconButton icon={ArrowUp} label={t('ai.send')} className="ai__send" disabled={!input.trim() && attach.length === 0} onClick={() => void ask(input)} />
            )}
          </div>
        ) : busy ? (
          <IconButton icon={Square} label={t('ai.stop')} className="ai__send" onClick={stop} />
        ) : (
          <IconButton icon={ArrowUp} label={t('ai.send')} className="ai__send" disabled={!input.trim() && attach.length === 0} onClick={() => void ask(input)} />
        )}
      </div>
    </div>
  );
}
