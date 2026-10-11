// 设置 → AI 工具（MCP）：让 Claude、Codex、Cursor 之类的 AI 工具搜剪贴板、整理分组、查识字记录（参照 Paste 的 MCP）。
// 开关、只读 / 可修改、运行状态，以及各个客户端的配置（直接复制）。

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Copy, RefreshCw } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { capture, system } from '@/lib/ipc';
import type { McpStatus, Settings } from '@/lib/types';
import { Button, Segmented, Switch } from '@/ui/controls';
import { confirmDialog, notify } from '@/ui/overlays';

import { Group, Row } from './settingsParts';

type Client = 'claudeCode' | 'claudeDesktop' | 'cursor' | 'codex';

/** 各个客户端的配置写法。能填网址的直接连服务；只会起子进程的走 `--mcp` 命令行桥接 */
function snippet(client: Client, s: McpStatus): string {
  switch (client) {
    case 'claudeCode':
      return `claude mcp add --transport http dato-ocr ${s.url} --header "Authorization: Bearer ${s.token}"`;
    case 'claudeDesktop':
      return JSON.stringify({ mcpServers: { 'dato-ocr': { command: s.command, args: ['--mcp'] } } }, null, 2);
    case 'cursor':
      return JSON.stringify({ mcpServers: { 'dato-ocr': { url: s.url, headers: { Authorization: `Bearer ${s.token}` } } } }, null, 2);
    case 'codex':
      return `[mcp_servers.dato-ocr]\ncommand = ${JSON.stringify(s.command)}\nargs = ["--mcp"]`;
  }
}

export function McpSettingsGroup({ settings, set }: { settings: Settings; set: (mutate: (d: Settings) => void) => void }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const mcp = settings.mcp;
  const [client, setClient] = useState<Client>('claudeCode');
  // 开关之后服务要一会儿才起来：设置一变就重查，前几秒再补查几次
  const status = useQuery({
    queryKey: ['mcp-status', mcp.enabled, mcp.port],
    queryFn: system.mcpStatus,
    refetchInterval: (q) => (mcp.enabled && !q.state.data?.running && !q.state.data?.error ? 800 : false),
  });
  const s = status.data;
  const copy = (text: string) => void capture.writeText(text).then(() => notify.success(t('clip.copied')), notify.error);

  return (
    <Group id="mcp" title={t('settings.section.mcp')} note={t('settings.mcp.note')}>
      <Row title={t('settings.mcp.enabled')} desc={t('settings.mcp.enabledDesc')}>
        <Switch checked={mcp.enabled} onChange={(v) => set((d) => void (d.mcp.enabled = v))} />
      </Row>
      <Row title={t('settings.mcp.allowWrite')} desc={t('settings.mcp.allowWriteDesc')}>
        <Switch checked={mcp.allowWrite} onChange={(v) => set((d) => void (d.mcp.allowWrite = v))} />
      </Row>
      {mcp.enabled && s && (
        <>
          <Row title={t('settings.mcp.status')}>
            {s.error ? (
              <span className="set-row__error">{s.error}</span>
            ) : s.running ? (
              <span className="set-row__ok">{t('settings.mcp.running', { url: s.url })}</span>
            ) : (
              <span className="set-row__value">{t('settings.mcp.starting')}</span>
            )}
          </Row>
          <Row title={t('settings.mcp.token')} desc={t('settings.mcp.tokenDesc')}>
            <span className="set-row__value cn-mono">{`${s.token.slice(0, 6)}…${s.token.slice(-4)}`}</span>
            <Button size="sm" icon={Copy} onClick={() => copy(s.token)}>
              {t('common.copy')}
            </Button>
            <Button
              size="sm"
              icon={RefreshCw}
              onClick={() =>
                void confirmDialog({ title: t('settings.mcp.regenerateAsk'), body: t('settings.mcp.regenerateBody') })
                  .then((ok) => (ok ? system.mcpRegenerateToken().then(() => qc.invalidateQueries({ queryKey: ['mcp-status'] })) : undefined))
                  .catch(notify.error)
              }
            >
              {t('settings.mcp.regenerate')}
            </Button>
          </Row>
          <div className="set-row set-row--stack">
            <div className="set-row__label">
              <div className="set-row__title">{t('settings.mcp.setup')}</div>
              <div className="set-row__desc">{t(`settings.mcp.where.${client}`)}</div>
            </div>
            <Segmented
              value={client}
              options={(['claudeCode', 'claudeDesktop', 'cursor', 'codex'] as Client[]).map((c) => ({ value: c, label: t(`settings.mcp.clients.${c}`) }))}
              onChange={setClient}
            />
            <div className="mcp-snippet">
              <pre className="cn-mono cn-selectable">{snippet(client, s)}</pre>
              <Button size="sm" icon={Copy} onClick={() => copy(snippet(client, s))}>
                {t('common.copy')}
              </Button>
            </div>
          </div>
        </>
      )}
    </Group>
  );
}
