// 设置页里的翻译、划词翻译、网络三组。
//
// 翻译源是一个有序列表：排在最前面的可用源就是默认源，翻译失败时依次往后降级；
// 翻译面板"同时显示多个源"时也按这个顺序排。每个源都能单独"检测"一下通不通。

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ChevronDown, ChevronUp, KeyRound, RotateCcw } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { system, translate } from '@/lib/ipc';
import { accelLabel, isMac } from '@/lib/platform';
import type { ProviderId, Settings } from '@/lib/types';
import { Button, IconButton, Segmented, Select, Slider, Switch, TextField } from '@/ui/controls';
import { notify, promptDialog } from '@/ui/overlays';
import { ProviderLogo } from '@/views/translate/ProviderLogo';
import { LANGS } from '@/views/translate/TranslateBox';

import { Group, Row } from './settingsParts';

type Probe = { state: 'testing' } | { state: 'ok'; ms: number } | { state: 'fail'; error: string };

export function TranslateSettingsGroups({ settings, set }: { settings: Settings; set: (mutate: (d: Settings) => void) => void }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const secrets = useQuery({ queryKey: ['secrets'], queryFn: system.secrets });
  const [probes, setProbes] = useState<Partial<Record<ProviderId, Probe>>>({});
  const tr = settings.translate;
  const sel = tr.selection;
  const net = settings.network;
  const pop = tr.popup;

  const hasKey = (id: ProviderId) => (id === 'deepl' ? !!secrets.data?.deepl : id === 'openai' ? !!secrets.data?.openai : true);
  const defaultId = tr.providers.find((p) => p.enabled && hasKey(p.id))?.id;

  const refreshProviders = () => void qc.invalidateQueries({ queryKey: ['translate-providers'] });
  const change = (mutate: (d: Settings) => void) => {
    set(mutate);
    // 翻译面板按这个列表决定显示哪些源
    window.setTimeout(refreshProviders, 300);
  };

  const move = (index: number, delta: number) =>
    change((d) => {
      const list = d.translate.providers;
      const a = list[index];
      const b = list[index + delta];
      if (!a || !b) return;
      list[index] = b;
      list[index + delta] = a;
    });

  const editSecret = async (key: 'deepl' | 'openai') => {
    const value = await promptDialog({ title: t('settings.translate.enterKey'), placeholder: key === 'deepl' ? 'xxxxxxxx-xxxx-...:fx' : 'sk-...' });
    if (value === null) return;
    await system.setSecret(key, value.trim() || null).catch(notify.error);
    void qc.invalidateQueries({ queryKey: ['secrets'] });
    refreshProviders();
  };

  const probe = async (id: ProviderId) => {
    setProbes((p) => ({ ...p, [id]: { state: 'testing' } }));
    const started = performance.now();
    try {
      await translate.text({ text: 'Hello, world.', from: 'en', to: 'zh', provider: id, fresh: true });
      setProbes((p) => ({ ...p, [id]: { state: 'ok', ms: Math.round(performance.now() - started) } }));
    } catch (err) {
      setProbes((p) => ({ ...p, [id]: { state: 'fail', error: err instanceof Error ? err.message : String(err) } }));
    }
  };

  const probeText = (id: ProviderId) => {
    const p = probes[id];
    if (!p || p.state === 'testing') return null;
    return p.state === 'ok' ? (
      <span className="set-row__ok">{t('settings.translate.probeOk', { ms: p.ms })}</span>
    ) : (
      <span className="set-row__error" title={p.error}>
        {t('settings.translate.probeFail')}
      </span>
    );
  };

  return (
    <>
      <Group id="translate" title={t('settings.section.translate')} note={t('settings.translate.orderNote')}>
        <Row title={t('settings.translate.target')}>
          <Select
            value={tr.targetLanguage}
            options={[{ value: 'auto', label: t('translate.autoTarget') }, ...LANGS.map((l) => ({ value: l, label: t(`lang.${l}`) }))]}
            onChange={(v) => set((d) => void (d.translate.targetLanguage = v))}
          />
        </Row>
        <Row title={t('settings.translate.showAll')} desc={t('settings.translate.showAllDesc')}>
          <Switch checked={tr.showAllProviders} onChange={(v) => set((d) => void (d.translate.showAllProviders = v))} />
        </Row>
        {tr.providers.map((p, i) => {
          const keyed = p.id === 'deepl' || p.id === 'openai';
          const missingKey = keyed && !hasKey(p.id);
          return (
            <div key={p.id}>
              <Row
                title={
                  <span className="set-provider">
                    <ProviderLogo id={p.id} size={16} />
                    {t(`settings.translate.${p.id}`)}
                    {p.id === defaultId && <span className="set-provider__badge">{t('settings.translate.default')}</span>}
                  </span>
                }
                desc={missingKey ? t('settings.translate.needKey') : t(`settings.translate.${p.id}Desc`)}
              >
                {probeText(p.id)}
                <Button size="sm" variant="ghost" loading={probes[p.id]?.state === 'testing'} disabled={missingKey} onClick={() => void probe(p.id)}>
                  {t('settings.translate.probe')}
                </Button>
                {keyed && (
                  <Button size="sm" variant="ghost" icon={KeyRound} onClick={() => void editSecret(p.id as 'deepl' | 'openai')}>
                    {hasKey(p.id) ? t('settings.translate.changeKey') : t('settings.translate.setKey')}
                  </Button>
                )}
                <IconButton icon={ChevronUp} size="sm" label={t('settings.translate.moveUp')} disabled={i === 0} onClick={() => move(i, -1)} />
                <IconButton
                  icon={ChevronDown}
                  size="sm"
                  label={t('settings.translate.moveDown')}
                  disabled={i === tr.providers.length - 1}
                  onClick={() => move(i, 1)}
                />
                <Switch checked={p.enabled} onChange={(v) =>
                    change((d) => {
                      const entry = d.translate.providers.find((x) => x.id === p.id);
                      if (entry) entry.enabled = v;
                    })
                  } />
              </Row>
              {p.id === 'openai' && p.enabled && (
                <>
                  <Row title={t('settings.translate.baseUrl')} desc={t('settings.translate.baseUrlDesc')}>
                    <TextField
                      defaultValue={tr.customBaseUrl}
                      style={{ width: 280 }}
                      onBlur={(e) => set((d) => void (d.translate.customBaseUrl = e.target.value.trim()))}
                    />
                  </Row>
                  <Row title={t('settings.translate.model')}>
                    <TextField defaultValue={tr.customModel} style={{ width: 200 }} onBlur={(e) => set((d) => void (d.translate.customModel = e.target.value.trim()))} />
                  </Row>
                </>
              )}
            </div>
          );
        })}
      </Group>

      <Group id="selection" title={t('settings.section.selection')} note={t('settings.selection.note', { hotkey: accelLabel(settings.hotkeys.translate) })}>
        <Row title={t('settings.selection.showButton')} desc={t('settings.selection.showButtonDesc')}>
          <Switch checked={sel.showButton} onChange={(v) => set((d) => void (d.translate.selection.showButton = v))} />
        </Row>
        <Row title={t('settings.selection.position')}>
          <Select
            value={sel.buttonPosition}
            disabled={!sel.showButton}
            options={(['bottomRight', 'topRight', 'bottomLeft', 'topLeft'] as const).map((v) => ({ value: v, label: t(`settings.selection.${v}`) }))}
            onChange={(v) => set((d) => void (d.translate.selection.buttonPosition = v))}
          />
        </Row>
        <Row title={t('settings.selection.modifier')} desc={t('settings.selection.modifierDesc')}>
          <Segmented
            value={sel.modifier}
            options={[
              { value: 'none', label: t('settings.selection.modifierNone') },
              { value: 'alt', label: isMac ? '⌥ Option' : 'Alt' },
              { value: 'ctrl', label: isMac ? '⌃ Control' : 'Ctrl' },
            ]}
            onChange={(v) => set((d) => void (d.translate.selection.modifier = v))}
          />
        </Row>
      </Group>

      <Group id="popup" title={t('settings.popup.title')} note={t('settings.popup.note')}>
        <Row title={t('settings.popup.width')}>
          <span className="set-row__value cn-numeric">{pop.width}px</span>
          <Slider value={pop.width} min={320} max={900} step={10} onChange={(v) => set((d) => void (d.translate.popup.width = v))} />
        </Row>
        <Row title={t('settings.popup.height')} desc={t('settings.popup.heightDesc')}>
          <Segmented
            value={pop.height === 0 ? 'auto' : 'fixed'}
            options={[
              { value: 'auto', label: t('settings.popup.auto') },
              { value: 'fixed', label: t('settings.popup.fixed') },
            ]}
            onChange={(v) => set((d) => void (d.translate.popup.height = v === 'auto' ? 0 : 360))}
          />
          {pop.height > 0 && (
            <>
              <span className="set-row__value cn-numeric">{pop.height}px</span>
              <Slider value={pop.height} min={200} max={1000} step={10} onChange={(v) => set((d) => void (d.translate.popup.height = v))} />
            </>
          )}
        </Row>
        <Row title={t('settings.popup.fontSize')}>
          <span className="set-row__value cn-numeric">{pop.fontSize}px</span>
          <Slider value={pop.fontSize} min={12} max={22} onChange={(v) => set((d) => void (d.translate.popup.fontSize = v))} />
        </Row>
        <Row title={t('settings.popup.blur')} desc={t('settings.popup.blurDesc')}>
          <Switch
            checked={pop.blur}
            onChange={(v) =>
              set((d) => {
                d.translate.popup.blur = v;
                // 系统亚克力自带一层色调，页面上再铺的色调太浓就看不出模糊了，给个看得出效果的默认值
                if (v && d.translate.popup.opacity > 0.6) d.translate.popup.opacity = 0.45;
              })
            }
          />
        </Row>
        <Row title={t('settings.popup.opacity')} desc={pop.blur ? t('settings.popup.opacityDesc') : undefined}>
          <span className="set-row__value cn-numeric">{Math.round(pop.opacity * 100)}%</span>
          <Slider
            value={Math.round(pop.opacity * 100)}
            min={pop.blur ? 30 : 60}
            max={100}
            onChange={(v) => set((d) => void (d.translate.popup.opacity = v / 100))}
          />
        </Row>
        <Row title={t('settings.popup.radius')}>
          <span className="set-row__value cn-numeric">{pop.radius}px</span>
          <Slider value={pop.radius} min={0} max={24} onChange={(v) => set((d) => void (d.translate.popup.radius = v))} />
        </Row>
        <Row title={t('settings.popup.aiLayout')} desc={t('settings.popup.aiLayoutDesc')}>
          <Segmented
            value={pop.aiLayout}
            options={[
              { value: 'drawer', label: t('settings.popup.drawer') },
              { value: 'side', label: t('settings.popup.side') },
            ]}
            onChange={(v) => set((d) => void (d.translate.popup.aiLayout = v))}
          />
        </Row>
        <Row title={t('settings.popup.showSource')} desc={t('settings.popup.showSourceDesc')}>
          <Switch checked={pop.showSource} onChange={(v) => set((d) => void (d.translate.popup.showSource = v))} />
        </Row>
        <Row title={t('settings.popup.reset')}>
          <Button
            size="sm"
            icon={RotateCcw}
            onClick={() =>
              set((d) => {
                d.translate.popup = { ...d.translate.popup, width: 380, height: 0, fontSize: 15, opacity: 0.45, radius: 24, showSource: false, blur: true };
              })
            }
          >
            {t('settings.popup.resetButton')}
          </Button>
        </Row>
      </Group>

      <Group id="network" title={t('settings.section.network')} note={t('settings.network.note')}>
        <Row title={t('settings.network.proxy')}>
          <Segmented
            value={net.proxyMode}
            options={[
              { value: 'system', label: t('settings.network.system') },
              { value: 'none', label: t('settings.network.none') },
              { value: 'custom', label: t('settings.network.custom') },
            ]}
            onChange={(v) => set((d) => void (d.network.proxyMode = v))}
          />
        </Row>
        {net.proxyMode === 'custom' && (
          <Row title={t('settings.network.address')} desc={t('settings.network.addressDesc')}>
            <TextField
              defaultValue={net.proxyUrl}
              placeholder="http://127.0.0.1:7890"
              style={{ width: 280 }}
              onBlur={(e) => set((d) => void (d.network.proxyUrl = e.target.value.trim()))}
            />
          </Row>
        )}
      </Group>
    </>
  );
}
