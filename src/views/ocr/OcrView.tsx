// 识字结果窗口（规格 04 §4）：左图右文，悬停段落 ↔ 高亮图上区域；右侧文字可直接改；
// 翻译时右侧分成上下两栏（原文 / 译文），不开新窗口；问 AI 在最右边加一列。
// 列与列、原文与译文之间都能拖动调整，调好的比例记在本机（localStorage）。

import './ocr.css';

import { useQuery } from '@tanstack/react-query';
import { LogicalPosition, LogicalSize } from '@tauri-apps/api/dpi';
import { currentMonitor, getCurrentWindow } from '@tauri-apps/api/window';
import clsx from 'clsx';
import { ChevronDown, Copy, FileSearch, Languages, RotateCw, ScanText, Sparkles } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { capture, ocr } from '@/lib/ipc';
import { assetUrl, modKey } from '@/lib/platform';
import { useSettings, useSettingsStore } from '@/lib/settings';
import type { OcrJob } from '@/lib/types';
import { Button, EmptyState, Skeleton, Spinner, Switch } from '@/ui/controls';
import { DropdownMenu, notify } from '@/ui/overlays';
import { TitleBar } from '@/ui/TitleBar';
import { AiChat } from '@/views/ai/AiChat';
import { TranslateBox } from '@/views/translate/TranslateBox';

import { ImageViewer } from './ImageViewer';
import { StructuredResult, type StructuredKind, type StructuredState } from './StructuredResult';

/** AI 对话那一列的默认宽度和可调范围（逻辑像素） */
const AI_COLUMN = 420;
const AI_MIN = 300;
const AI_MAX = 900;
/** 列之间拖动条的宽度 */
const GUTTER = 8;
/** AI 列滑出去用多久；最大化时（窗口不变宽）几列让出地方的过渡用多久 */
const AI_OUT_MS = 180;
const AI_GRID_MS = 280;

/** AI 列的几个阶段：opening = 窗口在变宽、面板还藏着；closing = 面板在滑出去 */
type AiPhase = 'closed' | 'opening' | 'open' | 'closing';

const nextFrame = () => new Promise<void>((r) => requestAnimationFrame(() => r()));
const wait = (ms: number) => new Promise<void>((r) => window.setTimeout(r, ms));

function loadNumber(key: string, fallback: number, min: number, max: number): number {
  try {
    const v = Number(localStorage.getItem(key));
    return Number.isFinite(v) && v >= min && v <= max ? v : fallback;
  } catch {
    return fallback;
  }
}

function saveNumber(key: string, value: number) {
  try {
    localStorage.setItem(key, String(Math.round(value * 1000) / 1000));
  } catch {
    // 存不了就算了，下次用默认值
  }
}

/** 按下拖动条后跟着鼠标走，松手时回调一次（用来保存）。 */
function dragSplitter(e: React.PointerEvent, onMove: (ev: PointerEvent) => void, onEnd: () => void) {
  e.preventDefault();
  const handle = e.currentTarget as HTMLElement;
  handle.setPointerCapture(e.pointerId);
  handle.dataset.dragging = '';
  document.body.style.cursor = getComputedStyle(handle).cursor;
  const up = () => {
    window.removeEventListener('pointermove', onMove);
    window.removeEventListener('pointerup', up);
    delete handle.dataset.dragging;
    document.body.style.cursor = '';
    onEnd();
  };
  window.addEventListener('pointermove', onMove);
  window.addEventListener('pointerup', up);
}

export default function OcrView() {
  const { t } = useTranslation();
  const settings = useSettings();
  const [job, setJob] = useState<OcrJob | null>(null);
  const [texts, setTexts] = useState<string[]>([]);
  const [hover, setHover] = useState<number | null>(null);
  const [showTranslate, setShowTranslate] = useState(false);
  const [ai, setAi] = useState<AiPhase>('closed');
  const showAi = ai !== 'closed';
  /** 网格里给 AI 列留着地方（关的时候面板先滑出去，再收这一列） */
  const [aiCols, setAiCols] = useState(false);
  /** 窗口变宽 / 变窄的那一下，图片、文字两列按像素定住，不跟着挤一下再弹回来 */
  const [frozen, setFrozen] = useState<[number, number] | null>(null);
  /** 最大化时窗口不能变宽：几列的宽度用过渡挪出地方 */
  const [gridAnim, setGridAnim] = useState(false);
  const imageRef = useRef<HTMLDivElement>(null);
  const sideRef = useRef<HTMLDivElement>(null);
  const aiBusy = useRef(false);
  const [keepBreaks, setKeepBreaks] = useState(false);
  /** 图片占 图片 + 文字 两列的比例 */
  const [split, setSplit] = useState(() => loadNumber('ocr.split', 0.5, 0.2, 0.8));
  /** 翻译打开时原文占上下两栏的比例 */
  const [textSplit, setTextSplit] = useState(() => loadNumber('ocr.textSplit', 0.5, 0.2, 0.8));
  const [aiWidth, setAiWidth] = useState(() => loadNumber('ocr.aiWidth', AI_COLUMN, AI_MIN, AI_MAX));
  const aiWidthRef = useRef(aiWidth);
  aiWidthRef.current = aiWidth;
  const listRef = useRef<HTMLDivElement>(null);
  const saveTimer = useRef<number | undefined>(undefined);

  const adopt = useCallback((j: OcrJob | null) => {
    setJob(j);
    setHover(null);
    if (j?.status === 'done' && j.result) {
      setTexts(j.result.paragraphs.map((p) => p.text));
      if (j.translate) setShowTranslate(true);
    } else if (j?.status === 'running') {
      setTexts([]);
      if (j.translate) setShowTranslate(true);
    }
  }, []);

  useEffect(() => {
    ocr.currentJob().then(adopt).catch(notify.error);
  }, [adopt]);
  useEvent('ocr-job', adopt);

  useEffect(() => {
    if (settings) setKeepBreaks(settings.ocr.keepLineBreaks);
  }, [settings]);

  const paragraphs = useMemo(() => job?.result?.paragraphs ?? [], [job]);
  const fullText = useMemo(() => {
    if (!job?.result) return '';
    if (keepBreaks) {
      return paragraphs
        .map((p, i) => (texts[i] !== undefined && texts[i] !== p.text ? texts[i] : p.lines.map((l) => l.text).join('\n')))
        .join('\n\n');
    }
    const sep = paragraphs.every((p) => p.lines.length === 1) ? '\n' : '\n\n';
    // 跨段删除后被并掉的段落是空的，不留空行
    return texts.filter((v) => v.trim()).join(sep);
  }, [job, texts, paragraphs, keepBreaks]);

  const copyAll = useCallback(() => {
    if (!fullText) return;
    capture
      .writeText(fullText)
      .then(() => notify.success(t('ocr.copied')))
      .catch(notify.error);
  }, [fullText, t]);

  /** 文字被改了：按段落块把现在的内容读回来。跨段删除会把段落块并掉，没了的那段就是空的 */
  const onEditAll = (root: HTMLElement) => {
    const blocks = root.querySelectorAll<HTMLElement>('[data-index]');
    // 全选删光再打字，字是直接落在可编辑区里的，一个段落块都不剩：整个算第一段
    const next = paragraphs.map((_, i) => (blocks.length ? (root.querySelector<HTMLElement>(`[data-index="${i}"]`)?.innerText ?? '') : i === 0 ? root.innerText : ''));
    setTexts(next);
    window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      const recordId = job?.recordId;
      if (recordId) {
        void ocr
          .saveText(recordId, next.filter((v) => v.trim()).join('\n\n'))
          .catch(() => undefined);
      }
    }, 800);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const editing = (e.target as HTMLElement | null)?.isContentEditable;
      if (modKey(e) && e.key.toLowerCase() === 'w') {
        e.preventDefault();
        void getCurrentWindow().close();
      } else if (modKey(e) && e.key.toLowerCase() === 'c' && !editing && !window.getSelection()?.toString()) {
        e.preventDefault();
        copyAll();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [copyAll]);

  // 悬停图上区域 → 右侧对应段落滚动到可见
  useEffect(() => {
    if (hover === null) return;
    listRef.current?.querySelector(`[data-index="${hover}"]`)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }, [hover]);

  /** 窗口往右长出 / 收回一列 AI 的宽度（长不下就贴着屏幕右边往左挪），等页面真的变了宽再回来 */
  const resizeForAi = async (dir: 1 | -1) => {
    const win = getCurrentWindow();
    const scale = await win.scaleFactor();
    const size = (await win.innerSize()).toLogical(scale);
    const width = Math.max(640, size.width + dir * (aiWidthRef.current + GUTTER));
    await win.setSize(new LogicalSize(width, size.height));
    if (dir > 0) {
      const monitor = await currentMonitor();
      if (monitor) {
        const pos = (await win.outerPosition()).toLogical(scale);
        const right = (monitor.position.x + monitor.size.width) / scale;
        const x = Math.max(monitor.position.x / scale, Math.min(pos.x, right - width));
        if (x !== pos.x) await win.setPosition(new LogicalPosition(x, pos.y));
      }
    }
    // 改窗口大小要过进程，页面的宽度晚一两帧才变
    const t0 = performance.now();
    while (Math.abs(window.innerWidth - width) > 2 && performance.now() - t0 < 500) await nextFrame();
    await nextFrame();
  };

  // AI 是第三列：打开时窗口往右加宽一列、对话面板从右边滑进来；关掉时面板先滑出去，窗口再缩回去。
  // 以前是这一列直接冒出来、先把图片和文字两列挤窄，窗口变宽后再弹回去
  const toggleAi = async () => {
    if (aiBusy.current) return;
    aiBusy.current = true;
    try {
      const maximized = await getCurrentWindow().isMaximized();
      const measure = (): [number, number] | null => {
        const a = imageRef.current?.getBoundingClientRect().width;
        const b = sideRef.current?.getBoundingClientRect().width;
        return a && b ? [a, b] : null;
      };
      if (ai === 'closed') {
        if (maximized) {
          // 先按 0 宽排上这一列，下一帧再过渡到正常宽度
          flushSync(() => setAi('opening'));
          await nextFrame();
          setGridAnim(true);
          setAiCols(true);
          setAi('open');
          await wait(AI_GRID_MS);
          setGridAnim(false);
          return;
        }
        // 1. 两列按现在的宽度定住，AI 列排在它们右边（窗口还没变宽，它在窗口外面），面板先藏着
        flushSync(() => {
          setFrozen(measure());
          setAiCols(true);
          setAi('opening');
        });
        // 2. 窗口往右长出一列
        await resizeForAi(1);
        // 3. 两列换回按比例（窗口宽了，算出来还是这么宽，不会跳），面板滑进来
        setFrozen(null);
        setAi('open');
        return;
      }
      if (ai !== 'open') return;
      setAi('closing');
      if (maximized) {
        setGridAnim(true);
        setAiCols(false);
        await wait(AI_GRID_MS);
        setGridAnim(false);
        setAi('closed');
        return;
      }
      await wait(AI_OUT_MS);
      flushSync(() => {
        setFrozen(measure());
        setAiCols(false);
        setAi('closed');
      });
      await resizeForAi(-1);
      setFrozen(null);
    } finally {
      aiBusy.current = false;
    }
  };

  const rerun = (engine: string | null, upscale: boolean) => {
    if (job) void ocr.rerun(job.id, engine, upscale).catch(notify.error);
  };

  const engines = useQuery({ queryKey: ['ocr-status'], queryFn: ocr.status, staleTime: 30_000 });
  // 识别成表格 / 公式（AI 看图）：结果替换文字那一列，"返回"回到识字结果
  const [structured, setStructured] = useState<StructuredState | null>(null);
  const structJob = useRef<string | null>(null);
  const runStructure = useCallback(
    (kind: StructuredKind, wanted?: 'local' | 'ai') => {
      if (!job) return;
      structJob.current = job.id;
      // 表格默认本地识别；公式下载了本地模型也先本地认（没下载的 Rust 那边直接交给 AI）
      const engine = wanted ?? 'local';
      setStructured({ kind, engine, status: 'running' });
      ocr.structure(job.id, kind, engine).then(
        (text) => structJob.current === job.id && setStructured({ kind, engine, status: 'done', text }),
        (err: unknown) => structJob.current === job.id && setStructured({ kind, engine, status: 'error', error: err instanceof Error ? err.message : String(err) }),
      );
    },
    [job],
  );
  // 换了一张图（新的识字任务）就回到文字
  useEffect(() => {
    if (job?.id !== structJob.current) setStructured(null);
  }, [job?.id]);
  const autoCopy = settings?.ocr.autoCopy ?? true;
  const result = job?.result;
  const info = result
    ? `${result.engine} · ${(result.elapsedMs / 1000).toFixed(1)}s${job?.copied ? ` · ${t('ocr.autoCopied')}` : ''}`
    : job?.status === 'running'
      ? t('ocr.running')
      : '';

  return (
    <div className="ocr-root">
      <TitleBar
        title={t('ocr.title')}
        extra={<span className="ocr-info">{info}</span>}
      />
      {!job ? (
        <div className="ocr-empty">
          <EmptyState icon={ScanText} title={t('ocr.noJob')} description={t('ocr.noJobDesc')} />
        </div>
      ) : (
        <div
          className="ocr-body"
          style={{
            // 一直是五列（AI 那两列关着时宽 0）：最大化时开关 AI 列能过渡
            gridTemplateColumns: `${
              frozen ? `${frozen[0]}px ${GUTTER}px ${frozen[1]}px` : `minmax(0, ${split}fr) ${GUTTER}px minmax(0, ${1 - split}fr)`
            } ${aiCols ? `${GUTTER}px ${frozen ? `${aiWidth}px` : `minmax(${AI_MIN}px, ${aiWidth}px)`}` : '0px minmax(0px, 0px)'}`,
            transition: gridAnim ? `grid-template-columns ${AI_GRID_MS}ms var(--cn-ease-out)` : undefined,
          }}
        >
          <div ref={imageRef} className="ocr-image">
            <ImageViewer
              src={assetUrl(job.imagePath)}
              width={job.width}
              height={job.height}
              boxes={paragraphs.map((p) => p.bbox)}
              highlight={hover}
              onHover={setHover}
            />
            {job.status === 'running' && (
              <div className="ocr-scanning">
                <Spinner size={18} />
                {t('ocr.running')}
              </div>
            )}
          </div>
          <div
            className="ocr-splitter"
            onPointerDown={(e) => {
              // 图片和文字两列一起算比例，AI 列宽度不变
              const img = e.currentTarget.previousElementSibling?.getBoundingClientRect();
              const side = e.currentTarget.nextElementSibling?.getBoundingClientRect();
              if (!img || !side) return;
              const total = side.right - img.left - GUTTER;
              let last = split;
              dragSplitter(
                e,
                (ev) => {
                  last = Math.min(0.8, Math.max(0.2, (ev.clientX - img.left - GUTTER / 2) / total));
                  setSplit(last);
                },
                () => saveNumber('ocr.split', last),
              );
            }}
          />
          <div
            ref={sideRef}
            className={clsx('ocr-side', showTranslate && job.status === 'done' && 'ocr-side--split')}
            style={showTranslate && job.status === 'done' ? { gridTemplateRows: `minmax(0, ${textSplit}fr) ${GUTTER}px minmax(0, ${1 - textSplit}fr)` } : undefined}
          >
            <div ref={listRef} className="ocr-text">
              {structured ? (
                <StructuredResult
                  state={structured}
                  // 拼成行的文字和原始文字块都给：拼行时相邻两块偶尔会粘在一起（数字连成一串），
                  // 原始块里长数字又偶尔被拆成几块。数字在任何一份里找得到就算对上
                  ocrText={job.result ? `${job.result.rawText}\n${job.result.blocks.map((b) => b.text).join('\n')}` : fullText}
                  onBack={() => setStructured(null)} onRetry={(engine) => runStructure(structured.kind, engine)} />
              ) : job.status === 'running' ? (
                <div className="ocr-text__loading">
                  <Skeleton />
                  <Skeleton />
                </div>
              ) : job.status === 'error' ? (
                <EmptyState
                  icon={FileSearch}
                  title={t('ocr.failed')}
                  description={job.error ?? ''}
                  action={
                    <Button icon={RotateCw} onClick={() => rerun(null, false)}>
                      {t('common.retry')}
                    </Button>
                  }
                />
              ) : paragraphs.length === 0 ? (
                <EmptyState icon={FileSearch} title={t('ocr.nothing')} description={t('ocr.nothingDesc')} />
              ) : (
                // 整列是一个可编辑区，段落只是里面的块：每段各自可编辑的话，选区出不了自己那一段，
                // 鼠标没法从一行拖到另一行（用户报过）。换了识字结果 / 切换"保留换行"就整个重建
                <div
                  key={`${job.id}-${keepBreaks}`}
                  className="ocr-paras cn-selectable"
                  contentEditable="plaintext-only"
                  suppressContentEditableWarning
                  spellCheck={false}
                  onInput={(e) => onEditAll(e.currentTarget)}
                >
                  {paragraphs.map((p, i) => (
                    <div key={i} data-index={i} className={clsx('ocr-para', hover === i && 'ocr-para--active')} onPointerEnter={() => setHover(i)}>
                      {keepBreaks ? p.lines.map((l) => l.text).join('\n') : p.text}
                    </div>
                  ))}
                </div>
              )}
            </div>
            {showTranslate && job.status === 'done' && (
              <>
                <div
                  className="ocr-splitter ocr-splitter--row"
                  onPointerDown={(e) => {
                    const box = e.currentTarget.parentElement?.getBoundingClientRect();
                    if (!box) return;
                    let last = textSplit;
                    dragSplitter(
                      e,
                      (ev) => {
                        last = Math.min(0.8, Math.max(0.2, (ev.clientY - box.top - GUTTER / 2) / (box.height - GUTTER)));
                        setTextSplit(last);
                      },
                      () => saveNumber('ocr.textSplit', last),
                    );
                  }}
                />
                <div className="ocr-translate">
                  <TranslateBox text={fullText} />
                </div>
              </>
            )}
          </div>
          {showAi && job.status === 'done' && (
            <>
              <div
                className="ocr-splitter"
                data-phase={ai}
                onPointerDown={(e) => {
                  // 拖的是 AI 列的左边：往左拖变宽，图片和文字两列按比例让出空间
                  const body = e.currentTarget.parentElement?.getBoundingClientRect();
                  if (!body) return;
                  const right = body.right - 12;
                  const max = Math.min(AI_MAX, body.width - 360);
                  let last = aiWidthRef.current;
                  dragSplitter(
                    e,
                    (ev) => {
                      last = Math.round(Math.min(max, Math.max(AI_MIN, right - ev.clientX - GUTTER / 2)));
                      setAiWidth(last);
                    },
                    () => saveNumber('ocr.aiWidth', last),
                  );
                }}
              />
              <div className="ocr-translate ocr-ai" data-phase={ai} style={gridAnim ? { minWidth: aiWidth } : undefined}>
                <AiChat context={{ text: fullText, images: [], source: 'ocr' }} resetKey={job.id} />
              </div>
            </>
          )}
        </div>
      )}
      <footer className="ocr-foot cn-hairline-top">
        <Button variant="primary" icon={Copy} disabled={!fullText} onClick={copyAll}>
          {t('ocr.copyAll')}
        </Button>
        <Button
          icon={Languages}
          disabled={job?.status !== 'done' || !fullText}
          onClick={() => setShowTranslate((v) => !v)}
        >
          {showTranslate ? t('ocr.hideTranslate') : t('ocr.translate')}
        </Button>
        <Button
          icon={Sparkles}
          disabled={job?.status !== 'done' || !fullText}
          onClick={() => void toggleAi()}
        >
          {ai === 'opening' || ai === 'open' ? t('ai.hide') : t('ai.ask')}
        </Button>
        <DropdownMenu
          align="start"
          items={[
            { label: t('ocr.rerunRapid'), onSelect: () => rerun('rapid', false) },
            ...(engines.data?.paddle ? [{ label: t('ocr.rerunPaddle'), onSelect: () => rerun('paddle', false) }] : []),
            { label: t('ocr.rerunSystem'), onSelect: () => rerun('system', false) },
            { separator: true },
            { label: t('ocr.rerunUpscale'), onSelect: () => rerun(null, true) },
          ]}
        >
          <Button disabled={!job || job.status === 'running'}>
            {t('ocr.rerun')}
            <ChevronDown size={12} strokeWidth={1.5} />
          </Button>
        </DropdownMenu>
        <DropdownMenu
          align="start"
          items={[
            { label: t('ocr.structured.toTable'), onSelect: () => runStructure('table') },
            { label: t('ocr.structured.toFormula'), onSelect: () => runStructure('formula') },
          ]}
        >
          <Button disabled={!job || job.status === 'running' || structured?.status === 'running'}>
            {t('ocr.structured.menu')}
            <ChevronDown size={12} strokeWidth={1.5} />
          </Button>
        </DropdownMenu>
        <span style={{ flex: 1 }} />
        <label className="ocr-toggle">
          <Switch
            checked={autoCopy}
            onChange={(v) => void useSettingsStore.getState().update((d) => void (d.ocr.autoCopy = v)).catch(notify.error)}
            label={t('ocr.autoCopy')}
          />
          {t('ocr.autoCopy')}
        </label>
        <label className="ocr-toggle">
          <Switch checked={keepBreaks} onChange={setKeepBreaks} label={t('ocr.keepBreaks')} />
          {t('ocr.keepBreaks')}
        </label>
      </footer>
    </div>
  );
}
