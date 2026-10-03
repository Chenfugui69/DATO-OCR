// 翻译区（规格 04 §8.1）：原文语言 ▾ · ⇅ 交换 · 译文语言 ▾ · 译文 · 复制译文。
// 加载中用骨架屏（三条灰线呼吸），不用转圈。
//
// 两种模式（设置 → 翻译 → 同时显示多个翻译源）：
// - 多源：每个已开启的源各占一张卡片、各自并发请求，按设置里的顺序排，第一张是默认源
// - 单源：只显示一个结果，默认源失败时 Rust 侧自动降级到下一个；可手动指定源

import { useQuery } from '@tanstack/react-query';
import { ArrowDownUp, Copy, RotateCw } from 'lucide-react';
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { capture, system, translate } from '@/lib/ipc';
import { useSettings } from '@/lib/settings';
import type { ProviderInfo, TranslateResult } from '@/lib/types';
import { Button, IconButton, Select, Skeleton } from '@/ui/controls';
import { notify } from '@/ui/overlays';

export const LANGS = ['zh', 'zh-TW', 'en', 'ja', 'ko', 'fr', 'de', 'es', 'ru', 'pt', 'it', 'ar', 'th', 'vi'];

type State = { loading: boolean; result: TranslateResult | null; error: string | null };

/** 发一次翻译请求，文字或语言变了自动重翻（等用户停手 350ms，识字结果可能正在被编辑）。 */
function useTranslate(text: string, from: string, to: string, provider: string) {
  const [state, setState] = useState<State>({ loading: false, result: null, error: null });
  const seq = useRef(0);
  const run = useCallback(async () => {
    if (!text.trim()) return;
    const id = ++seq.current;
    setState((s) => ({ ...s, loading: true, error: null }));
    try {
      const result = await translate.text({ text, from, to, provider });
      if (id === seq.current) setState({ loading: false, result, error: null });
    } catch (err) {
      if (id === seq.current) setState({ loading: false, result: null, error: err instanceof Error ? err.message : String(err) });
    }
  }, [text, from, to, provider]);
  useEffect(() => {
    const id = window.setTimeout(() => void run(), 350);
    return () => window.clearTimeout(id);
  }, [run]);
  return { ...state, run };
}

function copyText(text: string, done: string) {
  capture
    .writeText(text)
    .then(() => notify.success(done))
    .catch(notify.error);
}

export function TranslateBox({
  text,
  compact = false,
  actions,
  fontSize,
}: {
  text: string;
  compact?: boolean;
  /** 语言那一行右侧的按钮（划词面板放"问 AI"和关闭） */
  actions?: ReactNode;
  fontSize?: number;
}) {
  const { t } = useTranslation();
  const settings = useSettings();
  const [from, setFrom] = useState('auto');
  const [to, setTo] = useState('auto');
  const [detected, setDetected] = useState<{ from: string; to: string } | null>(null);
  const providers = useQuery({ queryKey: ['translate-providers'], queryFn: translate.providers });
  const active = (providers.data ?? []).filter((p) => p.enabled && p.configured);
  const multi = (settings?.translate.showAllProviders ?? true) && active.length > 1;

  const langOptions = LANGS.map((l) => ({ value: l, label: t(`lang.${l}`) }));
  const swap = () => {
    const src = from === 'auto' ? (detected?.from ?? 'auto') : from;
    const dst = to === 'auto' ? (detected?.to ?? 'auto') : to;
    setFrom(dst);
    setTo(src === 'auto' ? 'auto' : src);
  };

  return (
    <div className={compact ? 'tr-box tr-box--compact' : 'tr-box'} style={fontSize ? ({ '--tr-font': `${fontSize}px` } as React.CSSProperties) : undefined}>
      <div className="tr-box__langs">
        <Select value={from} width={compact ? 96 : 110} options={[{ value: 'auto', label: t('lang.auto') }, ...langOptions]} onChange={setFrom} />
        <IconButton icon={ArrowDownUp} size="sm" label={t('translate.swap')} onClick={swap} />
        <Select value={to} width={compact ? 96 : 110} options={[{ value: 'auto', label: t('translate.autoTarget') }, ...langOptions]} onChange={setTo} />
        {/* 空白处可以拖动面板 */}
        <span className="tr-box__spacer" data-tauri-drag-region />
        {actions}
      </div>
      {multi ? (
        <div className="tr-box__cards">
          {active.map((p, i) => (
            <ProviderCard key={p.id} text={text} from={from} to={to} provider={p} primary={i === 0} onDetected={i === 0 ? setDetected : undefined} />
          ))}
        </div>
      ) : (
        <SingleResult text={text} from={from} to={to} providers={active} compact={compact} onDetected={setDetected} />
      )}
    </div>
  );
}

function ProviderCard({
  text,
  from,
  to,
  provider,
  primary,
  onDetected,
}: {
  text: string;
  from: string;
  to: string;
  provider: ProviderInfo;
  primary: boolean;
  onDetected?: (d: { from: string; to: string }) => void;
}) {
  const { t } = useTranslation();
  const { loading, result, error, run } = useTranslate(text, from, to, provider.id);
  useEffect(() => {
    if (result) onDetected?.({ from: result.from, to: result.to });
  }, [result, onDetected]);
  return (
    <section className="tr-card" data-primary={primary || undefined}>
      <header className="tr-card__head">
        <span className="tr-card__name">{provider.name}</span>
        {primary && <span className="tr-card__badge">{t('settings.translate.default')}</span>}
        <span style={{ flex: 1 }} />
        {error && <IconButton icon={RotateCw} size="sm" label={t('common.retry')} onClick={() => void run()} />}
        <IconButton icon={Copy} size="sm" label={t('translate.copy')} disabled={!result} onClick={() => result && copyText(result.text, t('translate.copied'))} />
      </header>
      <div className="tr-card__body cn-selectable">
        {loading && !result ? (
          <Skeleton />
        ) : error ? (
          <div className="tr-box__error">{error}</div>
        ) : (
          <div style={{ opacity: loading ? 0.5 : 1 }}>{result?.text}</div>
        )}
      </div>
    </section>
  );
}

function SingleResult({
  text,
  from,
  to,
  providers,
  compact,
  onDetected,
}: {
  text: string;
  from: string;
  to: string;
  providers: ProviderInfo[];
  compact: boolean;
  onDetected: (d: { from: string; to: string }) => void;
}) {
  const { t } = useTranslation();
  const [provider, setProvider] = useState('auto');
  const { loading, result, error, run } = useTranslate(text, from, to, provider);
  useEffect(() => {
    if (result) onDetected({ from: result.from, to: result.to });
  }, [result, onDetected]);
  const usedProvider = providers.find((p) => p.id === result?.provider)?.name;
  return (
    <>
      <div className="tr-box__result cn-selectable">
        {loading && !result ? (
          <Skeleton />
        ) : error ? (
          <div className="tr-box__error">
            <div>{error}</div>
            <div className="tr-box__error-actions">
              <Button size="sm" icon={RotateCw} onClick={() => void run()}>
                {t('common.retry')}
              </Button>
              <Button size="sm" onClick={() => void system.showMain('settings:translate')}>
                {t('translate.openSettings')}
              </Button>
            </div>
          </div>
        ) : (
          <div style={{ opacity: loading ? 0.5 : 1 }}>{result?.text}</div>
        )}
      </div>
      <div className="tr-box__foot">
        {compact ? (
          <span className="tr-box__provider">{usedProvider && `${usedProvider}${result?.cached ? ` · ${t('translate.cached')}` : ''}`}</span>
        ) : (
          <Select
            value={provider}
            width={120}
            options={[{ value: 'auto', label: t('translate.autoProvider') }, ...providers.map((p) => ({ value: p.id, label: p.name }))]}
            onChange={setProvider}
          />
        )}
        <Button size="sm" icon={Copy} disabled={!result} onClick={() => result && copyText(result.text, t('translate.copied'))}>
          {t('translate.copy')}
        </Button>
      </div>
    </>
  );
}
