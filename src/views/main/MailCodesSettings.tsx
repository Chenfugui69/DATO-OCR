// 设置 → 剪贴板 → 邮箱验证码：连上邮箱（IMAP），新邮件里的验证码自动复制。Windows、macOS 一样。
// 密码（各家叫授权码 / 应用专用密码）只往后端送，存进加密的密钥表；页面上读不回来，只知道存没存。

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ExternalLink } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { clipboard, system } from '@/lib/ipc';
import type { Settings } from '@/lib/types';
import { Button, Segmented, Select, Switch, TextField } from '@/ui/controls';
import { notify } from '@/ui/overlays';

import { Row } from './settingsParts';

const PROVIDERS = ['qq', '163', 'gmail', 'icloud', 'proton', 'custom'] as const;
type Provider = (typeof PROVIDERS)[number];

/** 各家生成应用专用密码的官方说明页（QQ、163 的在邮箱网页版的设置里，下面那行说明写了怎么走） */
const HELP: Partial<Record<Provider, string>> = {
  gmail: 'https://support.google.com/accounts/answer/185833',
  icloud: 'https://support.apple.com/102654',
  proton: 'https://proton.me/mail/bridge',
};

export function MailCodesRows({ settings, set }: { settings: Settings; set: (mutate: (d: Settings) => void) => void }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const mail = settings.clipboard.mailCodes;
  const provider = (PROVIDERS as readonly string[]).includes(mail.provider) ? (mail.provider as Provider) : 'qq';
  const [password, setPassword] = useState('');
  const [testing, setTesting] = useState(false);
  // 连接是后台慢慢连的：开着的时候隔一会儿看一眼
  const status = useQuery({
    queryKey: ['mail-codes-status', mail],
    queryFn: clipboard.mailCodesStatus,
    refetchInterval: mail.enabled ? 2000 : false,
  });
  const s = status.data;
  const refresh = () => void qc.invalidateQueries({ queryKey: ['mail-codes-status'] });

  const savePassword = async () => {
    const value = password.trim();
    if (!value) return;
    await clipboard.mailCodesSetPassword(value).then(() => setPassword(''), notify.error);
    refresh();
  };
  const clearPassword = async () => {
    await clipboard.mailCodesSetPassword(null).catch(notify.error);
    refresh();
  };
  const test = async () => {
    setTesting(true);
    try {
      await clipboard.mailCodesTest(mail, password.trim() || null);
      notify.success(t('settings.mailCodes.testOk'));
    } catch (err) {
      notify.error(err);
    } finally {
      setTesting(false);
    }
  };

  const help = HELP[provider];
  const stateText =
    !s || s.state === 'off'
      ? null
      : s.state === 'error'
        ? t('settings.mailCodes.state.error', { message: s.message })
        : t(`settings.mailCodes.state.${s.state}`);

  return (
    <>
      <Row title={t('settings.mailCodes.title')} desc={t('settings.mailCodes.desc')}>
        <>
          {mail.enabled && stateText && (
            <span className="set-row__value" style={s?.state === 'error' ? { color: 'var(--cn-danger, #d92d20)' } : undefined}>
              {stateText}
            </span>
          )}
          <Switch checked={mail.enabled} onChange={(v) => set((d) => void (d.clipboard.mailCodes.enabled = v))} />
        </>
      </Row>
      {mail.enabled && (
        <>
          <Row title={t('settings.mailCodes.provider')} desc={t(`settings.mailCodes.providerDesc.${provider}`)}>
            <>
              {help && (
                <Button size="sm" variant="ghost" onClick={() => void system.openUrl(help).catch(notify.error)}>
                  <ExternalLink size={13} />
                  {t('settings.mailCodes.howTo')}
                </Button>
              )}
              <Select
                value={provider}
                width={150}
                options={PROVIDERS.map((p) => ({ value: p, label: t(`settings.mailCodes.providers.${p}`) }))}
                onChange={(v) => set((d) => void (d.clipboard.mailCodes.provider = v))}
              />
            </>
          </Row>
          <Row title={t('settings.mailCodes.address')}>
            <TextField
              key={mail.address}
              type="email"
              spellCheck={false}
              autoComplete="off"
              placeholder="name@example.com"
              defaultValue={mail.address}
              style={{ width: 240 }}
              onBlur={(e) => {
                const v = e.target.value.trim();
                if (v !== mail.address) set((d) => void (d.clipboard.mailCodes.address = v));
              }}
            />
          </Row>
          {provider === 'custom' && (
            <Row title={t('settings.mailCodes.server')} desc={t('settings.mailCodes.serverDesc')}>
              <>
                <Segmented
                  value={mail.security === 'starttls' ? 'starttls' : 'tls'}
                  options={[
                    { value: 'tls', label: 'SSL/TLS' },
                    { value: 'starttls', label: 'STARTTLS' },
                  ]}
                  onChange={(v) => set((d) => void (d.clipboard.mailCodes.security = v))}
                />
                <TextField
                  key={mail.host}
                  spellCheck={false}
                  placeholder="imap.example.com"
                  defaultValue={mail.host}
                  style={{ width: 180 }}
                  onBlur={(e) => {
                    const v = e.target.value.trim();
                    if (v !== mail.host) set((d) => void (d.clipboard.mailCodes.host = v));
                  }}
                />
              </>
            </Row>
          )}
          {(provider === 'custom' || provider === 'proton') && (
            <Row title={t('settings.mailCodes.port')} desc={provider === 'proton' ? t('settings.mailCodes.portDescProton') : undefined}>
              <TextField
                key={mail.port}
                type="number"
                min={0}
                max={65535}
                placeholder={provider === 'proton' ? '1143' : mail.security === 'starttls' ? '143' : '993'}
                defaultValue={mail.port || ''}
                style={{ width: 96 }}
                onBlur={(e) => {
                  const v = Math.round(Number(e.target.value)) || 0;
                  if (v >= 0 && v <= 65535 && v !== mail.port) set((d) => void (d.clipboard.mailCodes.port = v));
                }}
              />
            </Row>
          )}
          <Row title={t('settings.mailCodes.password')} desc={t('settings.mailCodes.passwordDesc')}>
            <>
              <TextField
                type="password"
                autoComplete="new-password"
                spellCheck={false}
                value={password}
                placeholder={s?.hasPassword ? t('settings.mailCodes.passwordSaved') : t('settings.mailCodes.passwordEmpty')}
                style={{ width: 200 }}
                onChange={(e) => setPassword(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && void savePassword()}
              />
              {password.trim() ? (
                <Button size="sm" variant="primary" onClick={() => void savePassword()}>
                  {t('settings.mailCodes.save')}
                </Button>
              ) : (
                s?.hasPassword && (
                  <Button size="sm" onClick={() => void clearPassword()}>
                    {t('settings.mailCodes.passwordClear')}
                  </Button>
                )
              )}
              <Button size="sm" disabled={testing || !mail.address || (!password.trim() && !s?.hasPassword)} onClick={() => void test()}>
                {testing ? t('settings.mailCodes.testing') : t('settings.mailCodes.test')}
              </Button>
            </>
          </Row>
        </>
      )}
    </>
  );
}
