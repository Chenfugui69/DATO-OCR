// 设置 → 存储：一键清理（展开后每一类单独清）和定时自动清理。
// 置顶、收藏、有备注、已分组的剪贴板记录和收藏的截图不删（和剪贴板面板里"清空"一样）。

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ChevronDown, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { formatBytes } from '@/lib/format';
import { system } from '@/lib/ipc';
import type { Settings, StorageTarget } from '@/lib/types';
import { Button, Select, Switch } from '@/ui/controls';
import { confirmDialog, notify } from '@/ui/overlays';

import { Row } from './settingsParts';

const TARGETS: StorageTarget[] = ['clipboard', 'screenshots', 'ocr', 'logs'];
const AUTO_DAYS = ['0', '7', '15', '30', '90'] as const;

export function StorageCleanRows({ settings, set }: { settings: Settings; set: (mutate: (d: Settings) => void) => void }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<StorageTarget | 'all' | null>(null);
  const usage = useQuery({ queryKey: ['storage-usage'], queryFn: system.storageUsage });
  const total = usage.data?.reduce((n, u) => n + u.bytes, 0);
  // 老版本的设置文件里还没有这一节（后端补上默认值之前）
  const auto = settings.storage ?? { autoCleanDays: 0, autoCleanTargets: [] };

  const clean = (targets: StorageTarget[], key: StorageTarget | 'all') => {
    const names = targets.map((k) => t(`settings.storage.target.${k}`)).join('、');
    void confirmDialog({
      title: t('settings.storage.cleanTitle', { names }),
      body: t('settings.storage.cleanBody'),
      confirmLabel: t('settings.storage.cleanConfirm'),
      danger: true,
    }).then((ok) => {
      if (!ok) return;
      setBusy(key);
      system
        .storageClean(targets)
        .then((r) => notify.success(t('settings.storage.cleaned', { size: formatBytes(r.freedBytes) })))
        .catch(notify.error)
        .finally(() => {
          setBusy(null);
          void qc.invalidateQueries({ queryKey: ['storage-usage'] });
          void qc.invalidateQueries({ queryKey: ['app-info'] });
        });
    });
  };

  return (
    <>
      <Row title={t('settings.storage.clean')} desc={total != null ? t('settings.storage.cleanDesc', { size: formatBytes(total) }) : '…'}>
        <Button size="sm" icon={Trash2} loading={busy === 'all'} disabled={busy != null} onClick={() => clean(TARGETS, 'all')}>
          {t('settings.storage.cleanAll')}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          aria-label={t('settings.storage.cleanEach')}
          aria-expanded={open}
          onClick={() => setOpen((v) => !v)}
        >
          {t('settings.storage.cleanEach')}
          <ChevronDown size={14} strokeWidth={2} style={{ transform: open ? 'rotate(180deg)' : undefined, transition: 'transform 160ms' }} />
        </Button>
      </Row>
      {open && (
        <div className="set-sub">
          {TARGETS.map((k) => {
            const u = usage.data?.find((x) => x.key === k);
            return (
              <Row
                key={k}
                title={t(`settings.storage.target.${k}`)}
                desc={u ? t(k === 'logs' ? 'settings.storage.usageFiles' : 'settings.storage.usageItems', { size: formatBytes(u.bytes), count: u.count }) : '…'}
              >
                <Button size="sm" loading={busy === k} disabled={busy != null || !u?.count} onClick={() => clean([k], k)}>
                  {t('settings.storage.cleanOne')}
                </Button>
              </Row>
            );
          })}
        </div>
      )}
      <Row title={t('settings.storage.auto')} desc={t('settings.storage.autoDesc')}>
        <Select
          value={String(auto.autoCleanDays)}
          options={AUTO_DAYS.map((v) => ({ value: v, label: t(`settings.storage.autoDays.${v}`) }))}
          onChange={(v) => set((d) => void (d.storage = { ...auto, ...d.storage, autoCleanDays: Number(v) }))}
        />
      </Row>
      {auto.autoCleanDays > 0 && (
        <div className="set-sub">
          {TARGETS.map((k) => (
            <Row key={k} title={t(`settings.storage.target.${k}`)}>
              <Switch
                checked={auto.autoCleanTargets.includes(k)}
                onChange={(on) =>
                  set((d) => {
                    const rest = auto.autoCleanTargets.filter((x) => x !== k);
                    d.storage = { ...auto, ...d.storage, autoCleanTargets: on ? [...rest, k] : rest };
                  })
                }
              />
            </Row>
          ))}
        </div>
      )}
    </>
  );
}
