// 识字结果窗口（规格 04 §4）：左图右文，悬停段落 ↔ 高亮图上区域；右侧文字可直接改；
// 翻译时右侧分成上下两栏（原文 / 译文），不开新窗口。

import './ocr.css';

import { getCurrentWindow } from '@tauri-apps/api/window';
import clsx from 'clsx';
import { ChevronDown, Copy, FileSearch, Languages, RotateCw, ScanText, Sparkles } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { capture, ocr } from '@/lib/ipc';
import { assetUrl } from '@/lib/platform';
import { useSettings } from '@/lib/settings';
import type { OcrJob } from '@/lib/types';
import { Button, EmptyState, Skeleton, Spinner, Switch } from '@/ui/controls';
import { DropdownMenu, notify } from '@/ui/overlays';
import { TitleBar } from '@/ui/TitleBar';
import { AiChat } from '@/views/ai/AiChat';
import { TranslateBox } from '@/views/translate/TranslateBox';

import { ImageViewer } from './ImageViewer';

export default function OcrView() {
  const { t } = useTranslation();
  const settings = useSettings();
  const [job, setJob] = useState<OcrJob | null>(null);
  const [texts, setTexts] = useState<string[]>([]);
  const [hover, setHover] = useState<number | null>(null);
  const [showTranslate, setShowTranslate] = useState(false);
  const [showAi, setShowAi] = useState(false);
  const [keepBreaks, setKeepBreaks] = useState(false);
  const [split, setSplit] = useState(0.5);
  const listRef = useRef<HTMLDivElement>(null);
  const saveTimer = useRef<number | undefined>(undefined);

  const adopt = useCallback((j: OcrJob | null) => {
    setJob(j);
    setHover(null);
    setShowAi(false);
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

  const rerun = (engine: string | null, upscale: boolean) => {
    if (job) void ocr.rerun(job.id, engine, upscale).catch(notify.error);
  };

  const result = job?.result;
  const info = result ? `${result.engine} · ${(result.elapsedMs / 1000).toFixed(1)}s` : job?.status === 'running' ? t('ocr.running') : '';

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
        <div className="ocr-body" style={{ gridTemplateColumns: `minmax(0, ${split}fr) 6px minmax(0, ${1 - split}fr)` }}>
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
              const el = e.currentTarget.parentElement;
              if (!el) return;
              e.currentTarget.setPointerCapture(e.pointerId);
              const rect = el.getBoundingClientRect();
              const move = (ev: PointerEvent) => setSplit(Math.min(0.75, Math.max(0.25, (ev.clientX - rect.left) / rect.width)));
              const up = () => {
                window.removeEventListener('pointermove', move);
                window.removeEventListener('pointerup', up);
              };
              window.addEventListener('pointermove', move);
              window.addEventListener('pointerup', up);
            }}
          />
          <div className={clsx('ocr-side', (showTranslate || showAi) && 'ocr-side--split')}>
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
            {showTranslate && !showAi && job.status === 'done' && (
              <div className="ocr-translate">
                <TranslateBox text={fullText} />
              </div>
            )}
            {showAi && job.status === 'done' && (
              <div className="ocr-translate">
                <AiChat context={{ text: fullText, images: [], source: 'ocr' }} resetKey={job.id} />
              </div>
            )}
          </div>
        </div>
      )}
      <footer className="ocr-foot cn-hairline-top">
        <Button variant="primary" icon={Copy} disabled={!fullText} onClick={copyAll}>
          {t('ocr.copyAll')}
        </Button>
        <Button
          icon={Languages}
          disabled={job?.status !== 'done' || !fullText}
          onClick={() => {
            setShowAi(false);
            setShowTranslate((v) => !(v && !showAi));
          }}
        >
          {showTranslate && !showAi ? t('ocr.hideTranslate') : t('ocr.translate')}
        </Button>
        <Button
          icon={Sparkles}
          disabled={job?.status !== 'done' || !fullText}
          onClick={() => setShowAi((v) => !v)}
        >
          {showAi ? t('ai.hide') : t('ai.ask')}
        </Button>
        <DropdownMenu
          align="start"
          items={[
            { label: t('ocr.rerunRapid'), onSelect: () => rerun('rapid', false) },
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
          <Switch checked={keepBreaks} onChange={setKeepBreaks} label={t('ocr.keepBreaks')} />
          {t('ocr.keepBreaks')}
        </label>
      </footer>
    </div>
  );
}
