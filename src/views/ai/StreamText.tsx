// AI 回答的正文。正在回复的那一条不是整段整段地蹦出来，而是：
//
// - 按词一段段淡入（中文 2–4 个字一段，英文一个词一段），每段从透明、4px 模糊变清晰；
// - 末尾一个闪烁的光标跟着走；
// - 气泡的高度平滑地长高（内容一行行多出来时不是一跳一跳的）。
//
// 回答是 Markdown，每来一点新内容整段重新渲染成 HTML（加粗、列表这些随时会变结构），所以淡入不能靠
// "新插进来的节点放动画"：重新渲染后所有节点都是新的。这里给每一段记下它第一次出现的时间，重新渲染时
// 还没放完的那几段用负的动画延迟接着放，放完的不再动。历史消息（没流式出过的）不拆段，就是普通的 HTML。

import { useLayoutEffect, useRef } from 'react';

import { renderMarkdown } from '@/lib/markdown';

/** 每段淡入多久（毫秒），和 ai.css 里 ai-chunk-in 的时长一致 */
const FADE_MS = 420;
/** 同一批到的几段，一段比一段晚这么久开始 */
const STAGGER_MS = 32;
/** 一批到得再多，最晚的那段也只等这么久 */
const STAGGER_MAX_MS = 480;

const CJK = /[぀-ヿ㐀-鿿가-힯豈-﫿]/;
const segmenter = typeof Intl !== 'undefined' && 'Segmenter' in Intl ? new Intl.Segmenter(undefined, { granularity: 'word' }) : null;

/** 把一段文字切成淡入用的小段：中文 2–4 个字，英文一个词（连着后面的空格、标点）。 */
export function chunkText(text: string): string[] {
  if (!segmenter) return text.split(/(?<=\s)/);
  const out: string[] = [];
  let buf = '';
  let bufCjk = false;
  const flush = () => {
    if (buf) out.push(buf);
    buf = '';
  };
  for (const seg of segmenter.segment(text)) {
    const s = seg.segment;
    if (seg.isWordLike) {
      const cjk = CJK.test(s);
      // 英文词各自成段；中文攒到 4 个字就另起一段
      if (buf.trim() && (!cjk || !bufCjk || [...buf].length + [...s].length > 4)) flush();
      buf += s;
      bufCjk = cjk;
    } else {
      // 空格、标点跟着前面那段走；英文词后面的空格之后另起
      buf += s;
      if (/\s/.test(s) && !bufCjk) flush();
    }
  }
  flush();
  return out;
}

/** 把 `root` 里的文字节点拆成一段段的 <span>，返回一共多少段。代码块不拆（保留里面的空白和排版）。 */
function wrapChunks(root: HTMLElement): HTMLSpanElement[] {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (node) => (node.parentElement?.closest('pre') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  });
  const texts: Text[] = [];
  while (walker.nextNode()) texts.push(walker.currentNode as Text);
  const spans: HTMLSpanElement[] = [];
  for (const node of texts) {
    const value = node.nodeValue ?? '';
    if (!value.trim()) continue;
    const fragment = document.createDocumentFragment();
    for (const piece of chunkText(value)) {
      const span = document.createElement('span');
      span.className = 'ai-chunk';
      span.textContent = piece;
      fragment.appendChild(span);
      spans.push(span);
    }
    node.replaceWith(fragment);
  }
  return spans;
}

/** 光标放在最后一段文字的后面：一路走到最后一个子元素，碰到代码块、表格这种就停在它外面。 */
function appendCaret(root: HTMLElement) {
  let host: HTMLElement = root;
  for (;;) {
    const last = host.lastElementChild as HTMLElement | null;
    if (!last || ['PRE', 'TABLE', 'HR', 'BR', 'IMG'].includes(last.tagName) || last.classList.contains('ai-chunk')) break;
    host = last;
  }
  const caret = document.createElement('span');
  caret.className = 'ai-caret';
  caret.setAttribute('aria-hidden', 'true');
  host.appendChild(caret);
}

export function StreamText({ content, streaming, className }: { content: string; streaming: boolean; className?: string }) {
  const clip = useRef<HTMLDivElement>(null);
  const inner = useRef<HTMLDivElement>(null);
  /** 这条消息流式出过（刷新出来的历史消息没有）：只有流式出过的才拆段 */
  const streamed = useRef(streaming);
  if (streaming) streamed.current = true;
  /** 每一段第一次出现、开始淡入的时刻（performance.now() 的毫秒） */
  const births = useRef<number[]>([]);

  useLayoutEffect(() => {
    const box = inner.current;
    const outer = clip.current;
    if (!box || !outer) return;
    box.innerHTML = renderMarkdown(content);
    if (!streamed.current) return;

    const spans = wrapChunks(box);
    const now = performance.now();
    const known = births.current;
    // Markdown 结构变了（比如 ** 闭合了）段数可能变少，多出来的记录丢掉
    known.length = Math.min(known.length, spans.length);
    const fresh = spans.length - known.length;
    for (let i = 0; i < fresh; i++) known.push(now + Math.min(i * STAGGER_MS, STAGGER_MAX_MS));
    spans.forEach((span, i) => {
      const elapsed = now - known[i]!;
      if (elapsed >= FADE_MS) return;
      // 还没放完的接着放：负的延迟 = 已经放了这么久
      span.classList.add('ai-chunk--in');
      span.style.animationDelay = `${-elapsed}ms`;
    });
    if (streaming) appendCaret(box);

    // 高度平滑长高：外面一层按里面的实际高度过渡；回复完了交还给自动高度
    if (streaming) {
      outer.style.height = `${box.offsetHeight}px`;
    } else if (outer.style.height) {
      outer.style.height = `${box.offsetHeight}px`;
      const done = window.setTimeout(() => (outer.style.height = ''), 240);
      return () => window.clearTimeout(done);
    }
  }, [content, streaming]);

  return (
    <div className={className}>
      <div ref={clip} className="ai-stream" data-streaming={streaming || undefined}>
        <div ref={inner} className="ai-md" />
      </div>
    </div>
  );
}
