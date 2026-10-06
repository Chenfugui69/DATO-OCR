// 检查更新：设置页顶上的新版本提示条、更新简介弹窗（不显示更新提示 / 取消 / 更新）。

import * as RadixDialog from '@radix-ui/react-dialog';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ArrowUpCircle, ChevronRight, Sparkles, Wrench, Zap, type LucideIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { formatBytes } from '@/lib/format';
import { update } from '@/lib/ipc';
import { useSettings } from '@/lib/settings';
import type { UpdateChanges, UpdateStatus } from '@/lib/types';
import { Button } from '@/ui/controls';
import { notify } from '@/ui/overlays';

/** 更新状态：一处查询，Rust 推事件时直接替换。`prompt` = 有新版本、且用户没选"不显示更新提示"。 */
export function useUpdate() {
  const qc = useQueryClient();
  const settings = useSettings();
  const q = useQuery({ queryKey: ['update-status'], queryFn: update.status });
  useEvent('update-status', (s) => qc.setQueryData(['update-status'], s));
  const status = q.data;
  const prompt = !!status?.available && status.available.version !== settings?.update.skippedVersion;
  return { status, prompt };
}

export function UpdateBanner({ status, onOpen }: { status: UpdateStatus; onOpen: () => void }) {
  const { t } = useTranslation();
  if (!status.available) return null;
  return (
    <button type="button" className="update-banner" onClick={onOpen}>
      <ArrowUpCircle size={20} />
      <span className="update-banner__text">
        <b>{t('update.banner', { version: status.available.version })}</b>
        <span>{t('update.bannerHint')}</span>
      </span>
      <ChevronRight size={16} />
    </button>
  );
}

function Section({ icon: Icon, title, items, tone }: { icon: LucideIcon; title: string; items: string[]; tone: string }) {
  if (!items.length) return null;
  return (
    <section className="update-notes__section">
      <h4 className={`update-notes__title update-notes__title--${tone}`}>
        <Icon size={14} />
        {title}
      </h4>
      <ul>
        {items.map((item, i) => (
          <li key={i}>{item}</li>
        ))}
      </ul>
    </section>
  );
}

export function UpdateDialog({ status, open, onClose }: { status: UpdateStatus; open: boolean; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const info = status.available;
  if (!info) return null;
  const changes: UpdateChanges = (i18n.language.startsWith('en') && info.changesEn) || info.changes;
  const empty = !changes.added.length && !changes.fixed.length && !changes.improved.length;
  const busy = status.stage === 'downloading' || status.stage === 'verifying' || status.stage === 'installing';
  const percent = status.progress != null ? Math.round(status.progress * 100) : null;
  const meta = [
    t('update.from', { current: status.current }),
    info.date ? t('update.date', { date: info.date.slice(0, 10) }) : null,
    info.size ? formatBytes(info.size) : null,
  ].filter(Boolean);

  const skip = () => {
    update
      .skip(info.version)
      .then(() => {
        notify.success(t('update.skipped'));
        onClose();
      })
      .catch(notify.error);
  };
  const install = () => void update.install().catch(notify.error);

  return (
    <RadixDialog.Root open={open} onOpenChange={(o) => !o && onClose()}>
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="cn-dialog-overlay" />
        <RadixDialog.Content className="cn-dialog update-dialog" aria-describedby={undefined}>
          <div className="update-dialog__head">
            <span className="update-dialog__icon">
              <ArrowUpCircle size={22} />
            </span>
            <div>
              <RadixDialog.Title className="cn-dialog__title update-dialog__title">{t('update.title', { version: info.version })}</RadixDialog.Title>
              <div className="update-dialog__meta">{meta.join(' · ')}</div>
            </div>
          </div>

          <div className="update-notes">
            <Section icon={Sparkles} tone="added" title={t('update.added')} items={changes.added} />
            <Section icon={Wrench} tone="fixed" title={t('update.fixed')} items={changes.fixed} />
            <Section icon={Zap} tone="improved" title={t('update.improved')} items={changes.improved} />
            {empty && <p className="update-notes__plain">{info.notes || t('update.noNotes')}</p>}
          </div>

          {(busy || status.stage === 'ready' || status.error) && (
            <div className="update-progress">
              {busy && (
                <div className="update-progress__bar">
                  <div style={{ width: `${status.stage === 'downloading' ? (percent ?? 8) : 100}%` }} />
                </div>
              )}
              <div className={status.error && !busy ? 'update-progress__text is-error' : 'update-progress__text'}>
                {status.stage === 'downloading' && (percent != null ? t('update.downloading', { percent }) : t('update.downloadingNoSize'))}
                {status.stage === 'verifying' && t('update.verifying')}
                {status.stage === 'installing' && t('update.installing')}
                {status.stage === 'ready' && t('update.ready')}
                {!busy && status.stage !== 'ready' && status.error}
              </div>
            </div>
          )}

          <div className="cn-dialog__actions update-dialog__actions">
            <Button variant="ghost" className="update-dialog__skip" disabled={busy} onClick={skip}>
              {t('update.skip')}
            </Button>
            <span className="update-dialog__spacer" />
            <Button variant="secondary" onClick={onClose}>
              {t('update.cancel')}
            </Button>
            <Button variant="primary" loading={busy} onClick={install}>
              {t('update.install')}
            </Button>
          </div>
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
