// 多端同步（规格 09）：局域网设备码 + 手机网页，WebDAV 网盘账号同步，同步选项。

import { useQuery, useQueryClient } from '@tanstack/react-query';
import clsx from 'clsx';
import { Check, Copy, Globe, Laptop, Monitor, RefreshCw, Smartphone, X } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { relativeTime } from '@/lib/format';
import { capture, sync } from '@/lib/ipc';
import { useSettings, useSettingsStore } from '@/lib/settings';
import type { Settings, SyncPeer, SyncStatus } from '@/lib/types';
import { Button, Select, Switch, TextField } from '@/ui/controls';
import { confirmDialog, notify } from '@/ui/overlays';

import { Group, Row } from './settingsParts';

/** 常见 WebDAV 服务。115 没有官方 WebDAV，要用 Alist / CloudDrive2 之类挂出来。 */
const PRESETS = [
  { value: 'jianguoyun', url: 'https://dav.jianguoyun.com/dav/' },
  { value: '115', url: 'http://127.0.0.1:5244/dav/' },
  { value: 'custom', url: '' },
] as const;

function presetOf(url: string): string {
  if (url.startsWith('https://dav.jianguoyun.com')) return 'jianguoyun';
  if (/:5244\/dav/.test(url)) return '115';
  return 'custom';
}

function PeerIcon({ peer }: { peer: Pick<SyncPeer, 'kind' | 'platform'> }) {
  if (peer.kind === 'web') return peer.platform === 'ios' || peer.platform === 'android' ? <Smartphone size={16} /> : <Globe size={16} />;
  return peer.platform === 'macos' ? <Laptop size={16} /> : <Monitor size={16} />;
}

function formatCode(code: string) {
  return `${code.slice(0, 4)} ${code.slice(4)}`;
}

export function SyncPage() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const settings = useSettings();
  const update = useSettingsStore((s) => s.update);
  const status = useQuery({ queryKey: ['sync-status'], queryFn: sync.status, refetchInterval: 5000 });
  useEvent('sync-changed', () => void qc.invalidateQueries({ queryKey: ['sync-status'] }));

  if (!settings || !status.data) return null;
  const s = status.data;
  const set = (mutate: (d: Settings) => void) => {
    update(mutate).catch(notify.error);
  };

  return (
    <section className="page">
      <div className="page__head">
        <h1 className="page__title">{t('sync.title')}</h1>
      </div>
      <div className="page__body">
        <div className="settings">
          <p className="set-note sync-intro">{t('sync.intro')}</p>
          <ThisDevice status={s} settings={settings} set={set} />
          {s.pending.length > 0 && <PendingRequests status={s} />}
          <LanGroup status={s} settings={settings} set={set} />
          <WebdavGroup status={s} settings={settings} set={set} />
          <OptionsGroup settings={settings} set={set} />
        </div>
      </div>
    </section>
  );
}

type Setter = (mutate: (d: Settings) => void) => void;

function ThisDevice({ status, settings, set }: { status: SyncStatus; settings: Settings; set: Setter }) {
  const { t } = useTranslation();
  const [name, setName] = useState(settings.sync.deviceName);
  useEffect(() => setName(settings.sync.deviceName), [settings.sync.deviceName]);
  return (
    <Group id="sync-device" title={t('sync.device.title')}>
      <Row title={t('sync.device.name')} desc={t('sync.device.nameDesc')}>
        <TextField
          className="sync-name"
          value={name}
          placeholder={status.defaultName}
          maxLength={40}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => name.trim() !== settings.sync.deviceName && set((d) => void (d.sync.deviceName = name.trim()))}
          onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
        />
      </Row>
    </Group>
  );
}

function PendingRequests({ status }: { status: SyncStatus }) {
  const { t } = useTranslation();
  const decide = (id: string, ok: boolean) => void sync.approve(id, ok).catch(notify.error);
  return (
    <Group id="sync-pending" title={t('sync.pending.title')}>
      {status.pending.map((r) => (
        <div key={r.id} className="set-row sync-request">
          <div className="sync-request__icon">
            <PeerIcon peer={r} />
          </div>
          <div className="set-row__label">
            <div className="set-row__title">{t('sync.pending.ask', { name: r.name })}</div>
            <div className="set-row__desc">
              {r.kind === 'web' ? t('sync.pending.web', { address: r.address }) : t('sync.pending.desktop', { address: r.address })}
            </div>
          </div>
          {r.sas && (
            <div className="sync-sas" title={t('sync.pending.sasDesc')}>
              <span>{t('sync.pending.sas')}</span>
              <b className="cn-mono">{r.sas}</b>
            </div>
          )}
          <div className="set-row__control">
            <Button size="sm" variant="secondary" onClick={() => decide(r.id, false)}>
              {t('sync.pending.deny')}
            </Button>
            <Button size="sm" variant="primary" onClick={() => decide(r.id, true)}>
              {t('sync.pending.approve')}
            </Button>
          </div>
        </div>
      ))}
    </Group>
  );
}

function LanGroup({ status, settings, set }: { status: SyncStatus; settings: Settings; set: Setter }) {
  const { t } = useTranslation();
  const sy = settings.sync;
  return (
    <Group id="sync-lan" title={t('sync.lan.title')} note={sy.lanEnabled ? t('sync.lan.note') : undefined}>
      <Row title={t('sync.lan.enable')} desc={t('sync.lan.enableDesc')}>
        <Switch checked={sy.lanEnabled} onChange={(v) => set((d) => void (d.sync.lanEnabled = v))} />
      </Row>
      {sy.lanEnabled && status.lanError && (
        <Row title={<span className="set-row__error">{status.lanError}</span>} desc={t('sync.lan.portDesc')} />
      )}
      {sy.lanEnabled && status.lanMode === 'host' && <HostMode status={status} settings={settings} set={set} />}
      {sy.lanEnabled && status.lanMode === 'member' && <MemberMode status={status} />}
      {sy.lanEnabled && status.lanMode === 'host' && <JoinRow status={status} />}
    </Group>
  );
}

function HostMode({ status, settings, set }: { status: SyncStatus; settings: Settings; set: Setter }) {
  const { t } = useTranslation();
  const [urlIndex, setUrlIndex] = useState(0);
  const url = status.urls[urlIndex] ?? status.urls[0];
  const qr = useQuery({ queryKey: ['sync-qr', url], queryFn: () => sync.qr(url!), enabled: !!url, staleTime: Infinity });
  const copy = (text: string) => void capture.writeText(text).then(() => notify.success(t('sync.copied')), notify.error);

  return (
    <>
      <div className="set-row sync-code">
        <div className="set-row__label">
          <div className="set-row__title">{t('sync.lan.code')}</div>
          <div className="set-row__desc">{t('sync.lan.codeDesc')}</div>
        </div>
        <div className="sync-code__value cn-mono cn-numeric">{formatCode(status.code)}</div>
        <div className="set-row__control">
          <Button size="sm" variant="ghost" icon={Copy} onClick={() => copy(status.code)} title={t('sync.copy')} />
          <Button
            size="sm"
            variant="ghost"
            icon={RefreshCw}
            title={t('sync.lan.regenerate')}
            onClick={async () => {
              if (await confirmDialog({ title: t('sync.lan.regenerateAsk'), body: t('sync.lan.regenerateBody') })) {
                sync.regenerateCode().catch(notify.error);
              }
            }}
          />
        </div>
      </div>

      <Row title={t('sync.lan.web')} desc={t('sync.lan.webDesc')}>
        <Switch checked={settings.sync.webEnabled} onChange={(v) => set((d) => void (d.sync.webEnabled = v))} />
      </Row>
      {settings.sync.webEnabled && url && (
        <div className="set-row sync-web">
          {qr.data && <div className="sync-qr" dangerouslySetInnerHTML={{ __html: qr.data }} />}
          <div className="sync-web__info">
            <div className="set-row__title">{t('sync.lan.scan')}</div>
            <div className="set-row__desc">{t('sync.lan.scanDesc')}</div>
            <div className="sync-url">
              <code className="cn-mono">{url.replace(/#.*$/, '')}</code>
              <Button size="sm" variant="ghost" icon={Copy} onClick={() => copy(url)} title={t('sync.copy')} />
            </div>
            {status.urls.length > 1 && (
              <Select
                value={String(urlIndex)}
                options={status.urls.map((u, i) => ({ value: String(i), label: u.replace(/^http:\/\/|\/#.*$/g, '') }))}
                onChange={(v) => setUrlIndex(Number(v))}
              />
            )}
          </div>
        </div>
      )}

      {status.members.length > 0 && (
        <div className="set-row sync-peers">
          <div className="set-row__label">
            <div className="set-row__title">{t('sync.lan.members')}</div>
          </div>
        </div>
      )}
      {status.members.map((p) => (
        <PeerRow key={p.deviceId} peer={p} />
      ))}
    </>
  );
}

function PeerRow({ peer }: { peer: SyncPeer }) {
  const { t } = useTranslation();
  const remove = async () => {
    if (await confirmDialog({ title: t('sync.removeAsk', { name: peer.name }), body: t('sync.removeBody'), danger: true, confirmLabel: t('sync.remove') })) {
      sync.removePeer(peer.deviceId).catch(notify.error);
    }
  };
  const seen = peer.online ? t('sync.online') : peer.lastSeen ? t('sync.lastSeen', { time: relativeTime(peer.lastSeen) }) : t('sync.never');
  return (
    <div className="set-row sync-peer">
      <div className={clsx('sync-peer__icon', peer.online && 'is-online')}>
        <PeerIcon peer={peer} />
      </div>
      <div className="set-row__label">
        <div className="set-row__title">{peer.name}</div>
        <div className="set-row__desc">
          {peer.kind === 'web' ? t('sync.kindWeb') : t('sync.kindDesktop')} · {seen}
          {peer.address ? ` · ${peer.address}` : ''}
        </div>
      </div>
      <div className="set-row__control">
        <Button size="sm" variant="ghost" onClick={remove}>
          {t('sync.remove')}
        </Button>
      </div>
    </div>
  );
}

function MemberMode({ status }: { status: SyncStatus }) {
  const { t } = useTranslation();
  const host = status.host;
  const hs = status.hostStatus;
  if (!host) return null;
  const leave = async () => {
    if (await confirmDialog({ title: t('sync.member.leaveAsk', { name: host.name }), body: t('sync.member.leaveBody'), danger: true })) {
      sync.leave().catch(notify.error);
    }
  };
  return (
    <div className="set-row sync-peer">
      <div className={clsx('sync-peer__icon', hs?.online && 'is-online')}>
        <Monitor size={16} />
      </div>
      <div className="set-row__label">
        <div className="set-row__title">{t('sync.member.joined', { name: host.name })}</div>
        <div className="set-row__desc">
          {hs?.online ? t('sync.member.online', { address: hs.address ?? '' }) : (hs?.error ?? t('sync.member.connecting'))}
        </div>
      </div>
      <div className="set-row__control">
        <Button size="sm" variant="ghost" onClick={leave}>
          {t('sync.member.leave')}
        </Button>
      </div>
    </div>
  );
}

function JoinRow({ status }: { status: SyncStatus }) {
  const { t } = useTranslation();
  const [code, setCode] = useState('');
  const [address, setAddress] = useState('');
  const [manual, setManual] = useState(false);
  const j = status.join;
  const busy = j.phase === 'searching' || j.phase === 'waiting';
  const start = () => void sync.join(code, manual ? address : undefined).catch(notify.error);

  return (
    <div className="set-row sync-join">
      <div className="set-row__label">
        <div className="set-row__title">{t('sync.join.title')}</div>
        <div className="set-row__desc">{t('sync.join.desc')}</div>
        {j.phase === 'searching' && <div className="sync-join__state">{t('sync.join.searching')}</div>}
        {j.phase === 'waiting' && (
          <div className="sync-join__state">
            {t('sync.join.waiting')} <b className="cn-mono sync-join__sas">{j.sas}</b>
          </div>
        )}
        {j.phase === 'error' && <div className="set-row__error">{j.error}</div>}
        {j.phase === 'done' && (
          <div className="set-row__ok">
            <Check size={12} /> {t('sync.join.done', { name: j.hostName ?? '' })}
          </div>
        )}
        {manual && (
          <TextField className="sync-join__addr" value={address} placeholder="192.168.1.8" onChange={(e) => setAddress(e.target.value)} />
        )}
      </div>
      <div className="set-row__control sync-join__control">
        <TextField
          className="sync-join__code cn-mono"
          value={code}
          placeholder="0000 0000"
          inputMode="numeric"
          maxLength={9}
          disabled={busy}
          onChange={(e) => setCode(e.target.value.replace(/[^\d ]/g, ''))}
          onKeyDown={(e) => e.key === 'Enter' && !busy && start()}
        />
        {busy ? (
          <Button size="sm" variant="secondary" icon={X} onClick={() => void sync.cancelJoin()}>
            {t('sync.join.cancel')}
          </Button>
        ) : (
          <Button size="sm" variant="primary" disabled={code.replace(/\D/g, '').length !== 8} onClick={start}>
            {t('sync.join.join')}
          </Button>
        )}
        <Button size="sm" variant="ghost" onClick={() => setManual((m) => !m)}>
          {manual ? t('sync.join.auto') : t('sync.join.manual')}
        </Button>
      </div>
    </div>
  );
}

function WebdavGroup({ status, settings, set }: { status: SyncStatus; settings: Settings; set: Setter }) {
  const { t } = useTranslation();
  const sy = settings.sync;
  const [url, setUrl] = useState(sy.webdavUrl);
  const [user, setUser] = useState(sy.webdavUser);
  const [password, setPassword] = useState('');
  const [folder, setFolder] = useState(sy.webdavFolder);
  const [syncPassword, setSyncPassword] = useState('');
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState(!status.webdav.configured);
  const w = status.webdav;
  const devices = useQuery({ queryKey: ['sync-dav-devices'], queryFn: sync.webdavDevices, enabled: w.configured && sy.webdavEnabled, staleTime: 60_000 });

  const connect = async () => {
    setBusy(true);
    try {
      await sync.webdavConnect({ url, user, password, folder, syncPassword });
      setPassword('');
      setSyncPassword('');
      setEditing(false);
      notify.success(t('sync.dav.connected'));
    } catch (e) {
      notify.error(e);
    } finally {
      setBusy(false);
    }
  };
  const disconnect = async () => {
    if (await confirmDialog({ title: t('sync.dav.disconnectAsk'), body: t('sync.dav.disconnectBody'), danger: true })) {
      sync.webdavDisconnect().then(() => setEditing(true), notify.error);
    }
  };

  return (
    <Group id="sync-webdav" title={t('sync.dav.title')} note={t('sync.dav.note')}>
      {!editing && w.configured ? (
        <>
          <Row title={t('sync.dav.enable')} desc={`${sy.webdavUser} · ${sy.webdavUrl}${sy.webdavFolder}`}>
            <Switch checked={sy.webdavEnabled} onChange={(v) => set((d) => void (d.sync.webdavEnabled = v))} />
          </Row>
          {sy.webdavEnabled && (
            <Row
              title={t('sync.dav.status')}
              desc={
                w.error ? (
                  <span className="set-row__error">{w.error}</span>
                ) : w.lastSync ? (
                  t('sync.dav.lastSync', { time: relativeTime(w.lastSync) })
                ) : (
                  t('sync.dav.starting')
                )
              }
            >
              <Button size="sm" variant="ghost" icon={RefreshCw} onClick={() => void sync.webdavNow()}>
                {t('sync.dav.now')}
              </Button>
            </Row>
          )}
          <Row title={t('sync.dav.interval')} desc={t('sync.dav.intervalDesc')}>
            <Select
              value={String(sy.webdavInterval)}
              options={[5, 10, 15, 30, 60, 120].map((n) => ({ value: String(n), label: t('sync.dav.seconds', { n }) }))}
              onChange={(v) => set((d) => void (d.sync.webdavInterval = Number(v)))}
            />
          </Row>
          {(devices.data ?? []).map((d) => (
            <div key={d.deviceId} className="set-row sync-peer">
              <div className="sync-peer__icon">
                <PeerIcon peer={{ kind: 'desktop', platform: d.platform }} />
              </div>
              <div className="set-row__label">
                <div className="set-row__title">
                  {d.name}
                  {d.deviceId === status.deviceId && <span className="sync-me">{t('sync.dav.thisDevice')}</span>}
                </div>
                <div className="set-row__desc">{t('sync.lastSeen', { time: relativeTime(d.lastSeen) })}</div>
              </div>
            </div>
          ))}
          <Row title={t('sync.dav.account')}>
            <Button size="sm" variant="secondary" onClick={() => setEditing(true)}>
              {t('sync.dav.edit')}
            </Button>
            <Button size="sm" variant="ghost" onClick={disconnect}>
              {t('sync.dav.disconnect')}
            </Button>
          </Row>
        </>
      ) : (
        <>
          <Row title={t('sync.dav.provider')}>
            <Select
              value={presetOf(url)}
              options={PRESETS.map((p) => ({ value: p.value, label: t(`sync.dav.presets.${p.value}`) }))}
              onChange={(v) => {
                const p = PRESETS.find((x) => x.value === v);
                if (p && p.url) setUrl(p.url);
              }}
            />
          </Row>
          {presetOf(url) === '115' && <p className="set-row sync-dav-hint">{t('sync.dav.hint115')}</p>}
          {presetOf(url) === 'jianguoyun' && <p className="set-row sync-dav-hint">{t('sync.dav.hintJianguo')}</p>}
          <Row title={t('sync.dav.url')}>
            <TextField className="sync-input" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://" />
          </Row>
          <Row title={t('sync.dav.user')}>
            <TextField className="sync-input" value={user} onChange={(e) => setUser(e.target.value)} />
          </Row>
          <Row title={t('sync.dav.password')} desc={w.configured ? t('sync.dav.passwordKeep') : undefined}>
            <TextField className="sync-input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
          </Row>
          <Row title={t('sync.dav.folder')}>
            <TextField className="sync-input" value={folder} onChange={(e) => setFolder(e.target.value)} />
          </Row>
          <Row title={t('sync.dav.syncPassword')} desc={t('sync.dav.syncPasswordDesc')}>
            <TextField className="sync-input" type="password" value={syncPassword} onChange={(e) => setSyncPassword(e.target.value)} />
          </Row>
          <Row title="">
            {w.configured && (
              <Button size="sm" variant="ghost" onClick={() => setEditing(false)}>
                {t('sync.join.cancel')}
              </Button>
            )}
            <Button size="sm" variant="primary" loading={busy} disabled={!url || !user || syncPassword.length < 6} onClick={connect}>
              {t('sync.dav.connect')}
            </Button>
          </Row>
        </>
      )}
    </Group>
  );
}

function OptionsGroup({ settings, set }: { settings: Settings; set: Setter }) {
  const { t } = useTranslation();
  const sy = settings.sync;
  return (
    <Group id="sync-options" title={t('sync.options.title')} note={t('sync.options.note')}>
      <Row title={t('sync.options.autoWrite')} desc={t('sync.options.autoWriteDesc')}>
        <Switch checked={sy.autoWrite} onChange={(v) => set((d) => void (d.sync.autoWrite = v))} />
      </Row>
      <Row title={t('sync.options.images')} desc={t('sync.options.imagesDesc')}>
        <Switch checked={sy.sendImages} onChange={(v) => set((d) => void (d.sync.sendImages = v))} />
      </Row>
      {sy.sendImages && (
        <Row title={t('sync.options.maxImage')}>
          <Select
            value={String(sy.maxImageMb)}
            options={[2, 5, 10, 20, 50].map((n) => ({ value: String(n), label: `${n} MB` }))}
            onChange={(v) => set((d) => void (d.sync.maxImageMb = Number(v)))}
          />
        </Row>
      )}
    </Group>
  );
}
