// 设置 → 翻译里的翻译源列表：内置的（免费的网页接口、要自己填密钥的官方接口）和用户自己添加的。
//
// - 按住每行左边的把手上下拖，调整顺序（排在最前面的可用源是默认源）；把手上按 ↑ ↓ 也行。
// - 要填东西的源点"设置"在行下面展开：密钥、应用 ID、区域……填完移开焦点就存。密钥存在后端加密的密钥表里，
//   页面上读不回来，只看得到打码后的样子。
// - "添加翻译"加一个内置里没有的：AI 模型（OpenAI 兼容接口）、DeepLX、或者自己描述的任意 HTTP 接口。

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { GripVertical, Plus, Settings2, Trash2 } from 'lucide-react';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { translate } from '@/lib/ipc';
import type { CustomTranslator, ProviderField, ProviderInfo, Settings } from '@/lib/types';
import { Button, Select, Switch, TextField } from '@/ui/controls';
import { confirmDialog, notify } from '@/ui/overlays';
import { ProviderLogo } from '@/views/translate/ProviderLogo';

import { Row } from './settingsParts';

type Probe = { state: 'testing' } | { state: 'ok'; ms: number } | { state: 'fail'; error: string };
type Kind = CustomTranslator['kind'];
const KINDS: Kind[] = ['ai', 'deeplx', 'http'];

const newCustom = (kind: Kind, name: string): CustomTranslator => ({
  id: `custom-${Math.random().toString(36).slice(2, 10)}`,
  name,
  kind,
  url: kind === 'ai' ? 'https://api.openai.com/v1' : '',
  model: '',
  prompt: '',
  headers: '',
  body: kind === 'http' ? '{"text": "{text}", "source": "{from}", "target": "{to}"}' : '',
  path: kind === 'http' ? 'data.text' : '',
});

/** 一项要填的东西（密钥、应用 ID……）：移开焦点就存；存过的密钥显示成打码的占位字 */
function FieldRow({ provider, field, onSaved }: { provider: string; field: ProviderField; onSaved: () => void }) {
  const { t } = useTranslation();
  const save = (value: string | null) => translate.setField(provider, field.key, value).then(onSaved, notify.error);
  return (
    <Row title={field.label} desc={field.required ? undefined : t('settings.translate.optional')}>
      <>
        <TextField
          key={`${field.set}:${field.value ?? ''}`}
          type={field.secret ? 'password' : 'text'}
          autoComplete="new-password"
          spellCheck={false}
          defaultValue={field.secret ? '' : (field.value ?? '')}
          placeholder={field.secret && field.set ? (field.value ?? '') : field.hint}
          style={{ width: 260 }}
          onBlur={(e) => {
            const v = e.target.value.trim();
            if (field.secret ? v : v !== (field.value ?? '')) void save(v || null);
          }}
        />
        {field.set && (
          <Button size="sm" variant="ghost" onClick={() => void save(null)}>
            {t('settings.translate.clear')}
          </Button>
        )}
      </>
    </Row>
  );
}

/** 一行文字的设置项（自定义翻译的名字、地址、模型……存在设置里） */
function TextRow({
  title,
  desc,
  value,
  placeholder,
  multiline,
  onSave,
}: {
  title: string;
  desc?: string;
  value: string;
  placeholder?: string;
  multiline?: boolean;
  onSave: (v: string) => void;
}) {
  return (
    <Row title={title} desc={desc}>
      {multiline ? (
        <textarea
          key={value}
          className="set-textarea"
          spellCheck={false}
          defaultValue={value}
          placeholder={placeholder}
          rows={3}
          onBlur={(e) => e.target.value !== value && onSave(e.target.value)}
        />
      ) : (
        <TextField
          key={value}
          spellCheck={false}
          defaultValue={value}
          placeholder={placeholder}
          style={{ width: 300 }}
          onBlur={(e) => e.target.value.trim() !== value && onSave(e.target.value.trim())}
        />
      )}
    </Row>
  );
}

export function TranslateProviderList({ settings, set }: { settings: Settings; set: (mutate: (d: Settings) => void) => void }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const tr = settings.translate;
  const infos = useQuery({ queryKey: ['translate-providers'], queryFn: translate.providers });
  const info = (id: string): ProviderInfo | undefined => infos.data?.find((p) => p.id === id);
  const refresh = () => void qc.invalidateQueries({ queryKey: ['translate-providers'] });
  const change = (mutate: (d: Settings) => void) => {
    set(mutate);
    // 设置存好要一小会儿，翻译面板和这里都按后端给的列表显示
    window.setTimeout(refresh, 300);
  };

  const [probes, setProbes] = useState<Record<string, Probe>>({});
  const [open, setOpen] = useState<string | null>(null);
  const [addKind, setAddKind] = useState<Kind>('ai');
  const probe = async (id: string) => {
    setProbes((p) => ({ ...p, [id]: { state: 'testing' } }));
    const started = performance.now();
    try {
      await translate.text({ text: 'Hello, world.', from: 'en', to: 'zh', provider: id, fresh: true });
      setProbes((p) => ({ ...p, [id]: { state: 'ok', ms: Math.round(performance.now() - started) } }));
    } catch (err) {
      setProbes((p) => ({ ...p, [id]: { state: 'fail', error: err instanceof Error ? err.message : String(err) } }));
    }
  };

  // ── 拖动排序：按住把手拖的时候，顺序先在本地跟着变（看得到行在挪），松手才存进设置 ──
  const [dragOrder, setDragOrder] = useState<string[] | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const rows = useRef(new Map<string, HTMLDivElement>());
  const order = dragOrder ?? tr.providers.map((p) => p.id);
  const commit = (ids: string[]) =>
    change((d) => {
      const byId = new Map(d.translate.providers.map((p) => [p.id, p]));
      d.translate.providers = ids.flatMap((id) => byId.get(id) ?? []);
    });
  const moveBy = (id: string, delta: number) => {
    const ids = tr.providers.map((p) => p.id);
    const from = ids.indexOf(id);
    const to = from + delta;
    if (from < 0 || to < 0 || to >= ids.length) return;
    ids.splice(to, 0, ...ids.splice(from, 1));
    commit(ids);
  };
  const onGripDown = (e: React.PointerEvent<HTMLButtonElement>, id: string) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const grip = e.currentTarget;
    grip.setPointerCapture(e.pointerId);
    let current = tr.providers.map((p) => p.id);
    setDragOrder(current);
    setDragging(id);
    setOpen(null);
    const onMove = (ev: PointerEvent) => {
      // 指针越过了哪一行的中线，就挪到那一行的位置
      const from = current.indexOf(id);
      let to = from;
      current.forEach((other, i) => {
        const box = rows.current.get(other)?.getBoundingClientRect();
        if (!box || other === id) return;
        const middle = box.top + box.height / 2;
        if (i < from && ev.clientY < middle) to = Math.min(to, i);
        if (i > from && ev.clientY > middle) to = Math.max(to, i);
      });
      if (to === from) return;
      const next = [...current];
      next.splice(to, 0, ...next.splice(from, 1));
      current = next;
      setDragOrder(next);
    };
    const finish = () => {
      grip.removeEventListener('pointermove', onMove);
      grip.removeEventListener('pointerup', finish);
      grip.removeEventListener('pointercancel', finish);
      setDragging(null);
      setDragOrder(null);
      if (current.join() !== tr.providers.map((p) => p.id).join()) commit(current);
    };
    grip.addEventListener('pointermove', onMove);
    grip.addEventListener('pointerup', finish);
    grip.addEventListener('pointercancel', finish);
  };

  const defaultId = order.find((id) => tr.providers.find((p) => p.id === id)?.enabled && (info(id)?.configured ?? true));
  const updateCustom = (id: string, patch: Partial<CustomTranslator>) =>
    change((d) => {
      const c = d.translate.custom.find((x) => x.id === id);
      if (c) Object.assign(c, patch);
    });
  const removeCustom = async (c: CustomTranslator) => {
    if (!(await confirmDialog({ title: t('settings.translate.removeTitle', { name: c.name }), confirmLabel: t('common.delete'), danger: true }))) return;
    await translate.setField(c.id, 'key', null).catch(() => undefined);
    setOpen(null);
    change((d) => {
      d.translate.custom = d.translate.custom.filter((x) => x.id !== c.id);
      d.translate.providers = d.translate.providers.filter((x) => x.id !== c.id);
    });
  };
  const addCustom = () => {
    const c = newCustom(addKind, t(`settings.translate.kind.${addKind}`));
    change((d) => {
      d.translate.custom.push(c);
      d.translate.providers.push({ id: c.id, enabled: true });
    });
    setOpen(c.id);
  };

  return (
    <>
      {order.map((id) => {
        const entry = tr.providers.find((p) => p.id === id);
        if (!entry) return null;
        const meta = info(id);
        const custom = tr.custom.find((c) => c.id === id);
        const name = custom?.name ?? meta?.name ?? id;
        const fields = meta?.fields ?? [];
        const ready = meta?.configured ?? true;
        const hasSettings = !!custom || fields.length > 0;
        const p = probes[id];
        const desc = !ready
          ? t(custom ? 'settings.translate.needSetup' : 'settings.translate.needKey')
          : custom
            ? t(`settings.translate.kindDesc.${custom.kind}`)
            : t(`settings.translate.${id}Desc`, { defaultValue: '' });
        return (
          <div
            key={id}
            ref={(el) => {
              if (el) rows.current.set(id, el);
              else rows.current.delete(id);
            }}
            className="set-sortable"
            data-dragging={dragging === id || undefined}
          >
            <Row
              title={
                <span className="set-provider">
                  <button
                    type="button"
                    className="set-grip"
                    title={t('settings.translate.dragSort')}
                    aria-label={t('settings.translate.dragSort')}
                    onPointerDown={(e) => onGripDown(e, id)}
                    onKeyDown={(e) => {
                      if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return;
                      e.preventDefault();
                      moveBy(id, e.key === 'ArrowUp' ? -1 : 1);
                    }}
                  >
                    <GripVertical size={14} />
                  </button>
                  <ProviderLogo id={id} name={custom?.name} size={16} />
                  {name}
                  {id === defaultId && <span className="set-provider__badge">{t('settings.translate.default')}</span>}
                </span>
              }
              desc={desc || undefined}
            >
              {p && p.state !== 'testing' &&
                (p.state === 'ok' ? (
                  <span className="set-row__ok">{t('settings.translate.probeOk', { ms: p.ms })}</span>
                ) : (
                  <span className="set-row__error" title={p.error}>
                    {t('settings.translate.probeFail')}
                  </span>
                ))}
              <Button size="sm" variant="ghost" loading={p?.state === 'testing'} disabled={!ready} onClick={() => void probe(id)}>
                {t('settings.translate.probe')}
              </Button>
              {hasSettings && (
                <Button size="sm" variant={open === id ? 'secondary' : 'ghost'} icon={Settings2} onClick={() => setOpen(open === id ? null : id)}>
                  {t('settings.translate.configure')}
                </Button>
              )}
              <Switch
                checked={entry.enabled}
                onChange={(v) =>
                  change((d) => {
                    const target = d.translate.providers.find((x) => x.id === id);
                    if (target) target.enabled = v;
                  })
                }
              />
            </Row>
            {open === id && (
              <div className="set-sub">
                {custom && (
                  <>
                    <TextRow title={t('settings.translate.customName')} value={custom.name} onSave={(v) => v && updateCustom(id, { name: v })} />
                    <TextRow
                      title={t(custom.kind === 'ai' ? 'settings.translate.baseUrl' : 'settings.translate.customUrl')}
                      desc={t(`settings.translate.urlDesc.${custom.kind}`)}
                      value={custom.url}
                      placeholder={custom.kind === 'deeplx' ? 'https://example.com/translate' : 'https://'}
                      onSave={(v) => updateCustom(id, { url: v })}
                    />
                    {custom.kind === 'ai' && (
                      <>
                        <TextRow title={t('settings.translate.model')} value={custom.model} placeholder="gpt-4o-mini" onSave={(v) => updateCustom(id, { model: v })} />
                        <TextRow
                          title={t('settings.translate.customPrompt')}
                          desc={t('settings.translate.customPromptDesc')}
                          value={custom.prompt}
                          multiline
                          onSave={(v) => updateCustom(id, { prompt: v })}
                        />
                      </>
                    )}
                    {custom.kind === 'http' && (
                      <>
                        <TextRow
                          title={t('settings.translate.customHeaders')}
                          desc={t('settings.translate.customHeadersDesc')}
                          value={custom.headers}
                          placeholder="Authorization: Bearer {key}"
                          multiline
                          onSave={(v) => updateCustom(id, { headers: v })}
                        />
                        <TextRow
                          title={t('settings.translate.customBody')}
                          desc={t('settings.translate.customBodyDesc')}
                          value={custom.body}
                          multiline
                          onSave={(v) => updateCustom(id, { body: v })}
                        />
                        <TextRow
                          title={t('settings.translate.customPath')}
                          desc={t('settings.translate.customPathDesc')}
                          value={custom.path}
                          placeholder="data.translations.0.text"
                          onSave={(v) => updateCustom(id, { path: v })}
                        />
                      </>
                    )}
                  </>
                )}
                {fields.map((f) => (
                  <FieldRow key={f.key} provider={id} field={f} onSaved={refresh} />
                ))}
                {id === 'openai' && (
                  <>
                    <TextRow
                      title={t('settings.translate.baseUrl')}
                      desc={t('settings.translate.baseUrlDesc')}
                      value={tr.customBaseUrl}
                      onSave={(v) => set((d) => void (d.translate.customBaseUrl = v))}
                    />
                    <TextRow title={t('settings.translate.model')} value={tr.customModel} onSave={(v) => set((d) => void (d.translate.customModel = v))} />
                  </>
                )}
                {custom && (
                  <Row title={t('settings.translate.removeCustom')}>
                    <Button size="sm" variant="danger" icon={Trash2} onClick={() => void removeCustom(custom)}>
                      {t('common.delete')}
                    </Button>
                  </Row>
                )}
              </div>
            )}
          </div>
        );
      })}
      <Row title={t('settings.translate.addCustom')} desc={t('settings.translate.addCustomDesc')}>
        <>
          <Select value={addKind} width={170} options={KINDS.map((k) => ({ value: k, label: t(`settings.translate.kind.${k}`) }))} onChange={setAddKind} />
          <Button size="sm" icon={Plus} onClick={addCustom}>
            {t('settings.translate.add')}
          </Button>
        </>
      </Row>
    </>
  );
}
