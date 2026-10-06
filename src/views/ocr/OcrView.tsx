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
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { capture, ocr } from '@/lib/ipc';
import { assetUrl } from '@/lib/platform';
import { useSettings, useSettingsStore } from '@/lib/settings';
import type { OcrJob } from '@/lib/types';
import { Button, EmptyState, Skeleton, Spinner, Switch } from '@/ui/controls';
import { DropdownMenu, notify } from '@/ui/overlays';
import { TitleBar } from '@/ui/TitleBar';
import { AiChat } from '@/views/ai/AiChat';
import { TranslateBox } from '@/views/translate/TranslateBox';

import { ImageViewer } from './ImageViewer';

/** AI 对话那一列的默认宽度和可调范围（逻辑像素） */
const AI_COLUMN = 420;
const AI_MIN = 300;
const AI_MAX = 900;
/** 列之间拖动条的宽度 */
const GUTTER = 8;

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
  const [showAi, setShowAi] = useState(false);
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
    return texts.join(sep);
  }, [job, texts, paragraphs, keepBreaks]);

  const copyAll = useCallback(() => {
    if (!fullText) return;
    capture
      .writeText(fullText)
      .then(() => notify.success(t('ocr.copied')))
      .catch(notify.error);
  }, [fullText, t]);

  const onEdit = (i: number, value: string) => {
    setTexts((prev) => prev.map((v, k) => (k === i ? value : v)));
    window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      const recordId = job?.recordId;
      if (recordId) {
        const next = texts.map((v, k) => (k === i ? value : v)).join('\n\n');
        void ocr.saveText(recordId, next).catch(() => undefined);
      }
    }, 800);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const editing = (e.target as HTMLElement | null)?.isContentEditable;
      if (e.ctrlKey && e.key.toLowerCase() === 'w') {
        e.preventDefault();
        void getCurrentWindow().close();
      } else if (e.ctrlKey && e.key.toLowerCase() === 'c' && !editing && !window.getSelection()?.toString()) {
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

  // AI 是第三列：打开时窗口往右加宽一列，关掉时缩回去（放不下就贴着屏幕右边往左挪）
  const toggleAi = async () => {
    const next = !showAi;
    setShowAi(next);
    const win = getCurrentWindow();
    if (await win.isMaximized()) return;
    const scale = await win.scaleFactor();
    const size = (await win.innerSize()).toLogical(scale);
    const width = size.width + (next ? 1 : -1) * (aiWidthRef.current + GUTTER);
    await win.setSize(new LogicalSize(Math.max(640, width), size.height));
    if (next) {
      const monitor = await currentMonitor();
      if (monitor) {
        const pos = (await win.outerPosition()).toLogical(scale);
        const right = (monitor.position.x + monitor.size.width) / scale;
        const x = Math.max(monitor.position.x / scale, Math.min(pos.x, right - width));
        if (x !== pos.x) await win.setPosition(new LogicalPosition(x, pos.y));
      }
    }
  };

  const rerun = (engine: string | null, upscale: boolean) => {
    if (job) void ocr.rerun(job.id, engine, upscale).catch(notify.error);
  };

  const engines = useQuery({ queryKey: ['ocr-status'], queryFn: ocr.status, staleTime: 30_000 });
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
            gridTemplateColumns: showAi
              ? `minmax(0, ${split}fr) ${GUTTER}px minmax(0, ${1 - split}fr) ${GUTTER}px minmax(${AI_MIN}px, ${aiWidth}px)`
              : `minmax(0, ${split}fr) ${GUTTER}px minmax(0, ${1 - split}fr)`,
          }}
        >
          <div className="ocr-image">
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
            className={clsx('ocr-side', showTranslate && job.status === 'done' && 'ocr-side--split')}
            style={showTranslate && job.status === 'done' ? { gridTemplateRows: `minmax(0, ${textSplit}fr) ${GUTTER}px minmax(0, ${1 - textSplit}fr)` } : undefined}
          >
            <div ref={listRef} className="ocr-text">
              {job.status === 'running' ? (
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
                paragraphs.map((p, i) => (
                  <div
                    key={`${job.id}-${i}`}
                    data-index={i}
                    className={clsx('ocr-para cn-selectable', hover === i && 'ocr-para--active')}
                    contentEditable="plaintext-only"
                    suppressContentEditableWarning
                    spellCheck={false}
                    onPointerEnter={() => setHover(i)}
                    onInput={(e) => onEdit(i, (e.target as HTMLElement).innerText)}
                  >
                    {keepBreaks ? p.lines.map((l) => l.text).join('\n') : p.text}
                  </div>
                ))
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
              <div className="ocr-translate ocr-ai">
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
          {showAi ? t('ai.hide') : t('ai.ask')}
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
