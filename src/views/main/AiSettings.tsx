// 设置页的"AI 对话"分组：服务商（接口地址 + 密钥 + 检测模型 + 挑选模型）、对话参数、快捷提问、外观。

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ChevronDown, KeyRound, Plus, Radar, RotateCcw, Trash2, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ai } from '@/lib/ipc';
import type { AiProvider, QuickPrompt, Settings } from '@/lib/types';
import { Button, IconButton, Segmented, Select, Slider, Switch, TextField } from '@/ui/controls';
import { DropdownMenu, notify, promptDialog } from '@/ui/overlays';

import { Group, Row } from './settingsParts';

/** 常见服务商的预设：只是帮用户填好名称、格式和接口地址。 */
const PRESETS: { name: string; kind: AiProvider['kind']; baseUrl: string }[] = [
  { name: 'OpenAI', kind: 'openai', baseUrl: 'https://api.openai.com/v1' },
  { name: 'Anthropic', kind: 'anthropic', baseUrl: 'https://api.anthropic.com' },
  { name: 'DeepSeek', kind: 'openai', baseUrl: 'https://api.deepseek.com/v1' },
  { name: '通义千问', kind: 'openai', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1' },
  { name: 'Kimi', kind: 'openai', baseUrl: 'https://api.moonshot.cn/v1' },
  { name: '智谱 GLM', kind: 'openai', baseUrl: 'https://open.bigmodel.cn/api/paas/v4' },
  { name: '硅基流动', kind: 'openai', baseUrl: 'https://api.siliconflow.cn/v1' },
  { name: 'OpenRouter', kind: 'openai', baseUrl: 'https://openrouter.ai/api/v1' },
  { name: 'Ollama', kind: 'openai', baseUrl: 'http://localhost:11434/v1' },
];

const newId = () => crypto.randomUUID().replace(/-/g, '').slice(0, 10);

export function AiSettingsGroups({ settings, set }: { settings: Settings; set: (mutate: (d: Settings) => void) => void }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const keys = useQuery({ queryKey: ['ai-keys'], queryFn: ai.keys });
  const [detected, setDetected] = useState<Record<string, string[]>>({});
  const [detecting, setDetecting] = useState<string | null>(null);
  const cfg = settings.ai;

  const editProvider = (id: string, mutate: (p: AiProvider) => void) =>
    set((d) => {
      const p = d.ai.providers.find((x) => x.id === id);
      if (p) mutate(p);
    });

  const addProvider = (preset?: (typeof PRESETS)[number]) =>
    set((d) => {
      d.ai.providers.push({
        id: newId(),
        name: preset?.name ?? t('settings.ai.customProvider'),
        kind: preset?.kind ?? 'openai',
        baseUrl: preset?.baseUrl ?? '',
        models: [],
        enabled: true,
      });
    });

  const editKey = async (p: AiProvider) => {
    const value = await promptDialog({ title: t('settings.ai.enterKey', { name: p.name }), placeholder: 'sk-...' });
    if (value === null) return;
    await ai.setKey(p.id, value.trim() || null).catch(notify.error);
    void qc.invalidateQueries({ queryKey: ['ai-keys'] });
  };

  const detect = async (p: AiProvider) => {
    setDetecting(p.id);
    try {
      const list = await ai.models(p.id);
      setDetected((d) => ({ ...d, [p.id]: list }));
      if (list.length === 0) notify.info(t('settings.ai.noModelsFound'));
    } catch (err) {
      notify.error(err);
    } finally {
      setDetecting(null);
    }
  };

  const addModel = async (p: AiProvider) => {
    const value = await promptDialog({ title: t('settings.ai.addModelManually'), placeholder: 'gpt-4o-mini' });
    const name = value?.trim();
    if (name) editProvider(p.id, (x) => void (x.models.includes(name) || x.models.push(name)));
  };

  const allModels = cfg.providers
    .filter((p) => p.enabled)
    .flatMap((p) => p.models.map((m) => ({ value: `${p.id}/${m}`, label: `${p.name} · ${m}` })));

  const setPrompt = (id: string, patch: Partial<QuickPrompt>) =>
    set((d) => {
      const q = d.ai.quickPrompts.find((x) => x.id === id);
      if (q) Object.assign(q, patch);
    });

  return (
    <>
      <Group id="ai" title={t('settings.section.ai')} note={t('settings.ai.note')}>
        {cfg.providers.length === 0 && <Row title={t('settings.ai.noProviders')} desc={t('settings.ai.noProvidersDesc')} />}
        {cfg.providers.map((p) => {
          const found = detected[p.id];
          return (
            <div key={p.id} className="ai-provider">
              <Row
                title={
                  <TextField
                    key={`${p.id}-name`}
                    defaultValue={p.name}
                    className="ai-provider__name"
                    onBlur={(e) => e.target.value.trim() && editProvider(p.id, (x) => void (x.name = e.target.value.trim()))}
                  />
                }
              >
                <Segmented
                  value={p.kind}
                  options={[
                    { value: 'openai', label: t('settings.ai.kindOpenai') },
                    { value: 'anthropic', label: 'Anthropic' },
                  ]}
                  onChange={(v) => editProvider(p.id, (x) => void (x.kind = v))}
                />
                <Switch checked={p.enabled} onChange={(v) => editProvider(p.id, (x) => void (x.enabled = v))} />
                <IconButton
                  icon={Trash2}
                  size="sm"
                  label={t('settings.ai.removeProvider')}
                  onClick={() => set((d) => void (d.ai.providers = d.ai.providers.filter((x) => x.id !== p.id)))}
                />
              </Row>
              <Row title={t('settings.ai.baseUrl')} desc={p.kind === 'anthropic' ? t('settings.ai.baseUrlAnthropic') : t('settings.ai.baseUrlOpenai')}>
                <TextField
                  key={`${p.id}-url`}
                  defaultValue={p.baseUrl}
                  placeholder="https://…/v1"
                  style={{ width: 300 }}
                  onBlur={(e) => editProvider(p.id, (x) => void (x.baseUrl = e.target.value.trim()))}
                />
              </Row>
              <Row title={t('settings.ai.apiKey')} desc={t('settings.ai.apiKeyDesc')}>
                <span className="set-row__value cn-mono">{keys.data?.[p.id] ?? t('settings.translate.needKey')}</span>
                <Button size="sm" icon={KeyRound} onClick={() => void editKey(p)}>
                  {keys.data?.[p.id] ? t('settings.translate.changeKey') : t('settings.translate.setKey')}
                </Button>
              </Row>
              <Row title={t('settings.ai.models')} desc={p.models.length === 0 ? t('settings.ai.modelsEmpty') : undefined}>
                <Button size="sm" icon={Radar} loading={detecting === p.id} disabled={!p.baseUrl} onClick={() => void detect(p)}>
                  {t('settings.ai.detect')}
                </Button>
                <IconButton icon={Plus} size="sm" label={t('settings.ai.addModelManually')} onClick={() => void addModel(p)} />
              </Row>
              {(p.models.length > 0 || found) && (
                <div className="ai-models">
                  {p.models.map((m) => (
                    <span key={m} className="ai-model" data-added>
                      {m}
                      <button type="button" aria-label={t('common.remove')} onClick={() => editProvider(p.id, (x) => void (x.models = x.models.filter((y) => y !== m)))}>
                        <X size={11} />
                      </button>
                    </span>
                  ))}
                  {found
                    ?.filter((m) => !p.models.includes(m))
                    .map((m) => (
                      <button key={m} type="button" className="ai-model" title={t('settings.ai.clickToAdd')} onClick={() => editProvider(p.id, (x) => void x.models.push(m))}>
                        <Plus size={11} />
                        {m}
                      </button>
                    ))}
                </div>
              )}
            </div>
          );
        })}
        <Row title={t('settings.ai.addProvider')} desc={t('settings.ai.addProviderDesc')}>
          <DropdownMenu
            align="end"
            items={[
              ...PRESETS.map((preset) => ({ label: preset.name, onSelect: () => addProvider(preset) })),
              { separator: true as const },
              { label: t('settings.ai.customProvider'), onSelect: () => addProvider() },
            ]}
          >
            <Button size="sm" icon={Plus}>
              {t('settings.ai.add')}
              <ChevronDown size={12} strokeWidth={1.5} />
            </Button>
          </DropdownMenu>
        </Row>
      </Group>

      <Group id="ai-chat" title={t('settings.ai.chatTitle')}>
        <Row title={t('settings.ai.defaultModel')}>
          <Select
            value={allModels.some((m) => m.value === cfg.defaultModel) ? cfg.defaultModel : (allModels[0]?.value ?? '')}
            disabled={allModels.length === 0}
            width={220}
            options={allModels.length ? allModels : [{ value: '', label: t('settings.ai.noModelYet') }]}
            onChange={(v) => set((d) => void (d.ai.defaultModel = v))}
          />
        </Row>
        <Row title={t('settings.ai.systemPrompt')} desc={t('settings.ai.systemPromptDesc')}>
          <textarea
            key="system-prompt"
            className="ai-textarea"
            defaultValue={cfg.systemPrompt}
            rows={3}
            onBlur={(e) => set((d) => void (d.ai.systemPrompt = e.target.value))}
          />
        </Row>
        <Row title={t('settings.ai.temperature')} desc={t('settings.ai.temperatureDesc')}>
          <span className="set-row__value cn-numeric">{cfg.temperature.toFixed(1)}</span>
          <Slider value={cfg.temperature} min={0} max={2} step={0.1} onChange={(v) => set((d) => void (d.ai.temperature = Math.round(v * 10) / 10))} />
        </Row>
        <Row title={t('settings.ai.maxTokens')}>
          <Select
            value={String(cfg.maxTokens)}
            options={['0', '1024', '2048', '4096', '8192', '16384'].map((v) => ({ value: v, label: v === '0' ? t('settings.ai.providerDefault') : v }))}
            onChange={(v) => set((d) => void (d.ai.maxTokens = Number(v)))}
          />
        </Row>
      </Group>

      <Group id="ai-prompts" title={t('settings.ai.quickTitle')} note={t('settings.ai.quickNote')}>
        {cfg.quickPrompts.map((q) => (
          <div key={q.id} className="ai-prompt">
            <TextField
              key={`${q.id}-label`}
              defaultValue={q.label}
              className="ai-prompt__label"
              onBlur={(e) => e.target.value.trim() && setPrompt(q.id, { label: e.target.value.trim() })}
            />
            <textarea
              key={`${q.id}-prompt`}
              className="ai-textarea ai-prompt__text"
              defaultValue={q.prompt}
              rows={2}
              onBlur={(e) => setPrompt(q.id, { prompt: e.target.value })}
            />
            <IconButton
              icon={Trash2}
              size="sm"
              label={t('settings.ai.removePrompt')}
              onClick={() => set((d) => void (d.ai.quickPrompts = d.ai.quickPrompts.filter((x) => x.id !== q.id)))}
            />
          </div>
        ))}
        <Row title={t('settings.ai.addPrompt')}>
          <Button
            size="sm"
            icon={Plus}
            onClick={() => set((d) => void d.ai.quickPrompts.push({ id: newId(), label: t('settings.ai.newPrompt'), prompt: '{text}' }))}
          >
            {t('settings.ai.add')}
          </Button>
        </Row>
      </Group>

      <Group id="ai-look" title={t('settings.ai.lookTitle')}>
        <Row title={t('settings.ai.fontSize')}>
          <span className="set-row__value cn-numeric">{cfg.panel.fontSize}px</span>
          <Slider value={cfg.panel.fontSize} min={12} max={20} onChange={(v) => set((d) => void (d.ai.panel.fontSize = v))} />
        </Row>
        <Row title={t('settings.ai.layout')}>
          <Segmented
            value={cfg.panel.layout}
            options={[
              { value: 'bubble', label: t('settings.ai.bubble') },
              { value: 'plain', label: t('settings.ai.plain') },
            ]}
            onChange={(v) => set((d) => void (d.ai.panel.layout = v))}
          />
        </Row>
        <Row title={t('settings.ai.windowSize')} desc={t('settings.ai.windowSizeDesc')}>
          <span className="set-row__value cn-numeric">
            {cfg.panel.width} × {cfg.panel.height}
          </span>
          <Slider value={cfg.panel.width} min={380} max={1000} step={20} onChange={(v) => set((d) => void (d.ai.panel.width = v))} />
          <Slider value={cfg.panel.height} min={420} max={1100} step={20} onChange={(v) => set((d) => void (d.ai.panel.height = v))} />
          <IconButton
            icon={RotateCcw}
            size="sm"
            label={t('settings.ai.resetSize')}
            onClick={() =>
              set((d) => {
                d.ai.panel.width = 520;
                d.ai.panel.height = 620;
              })
            }
          />
        </Row>
      </Group>
    </>
  );
}
