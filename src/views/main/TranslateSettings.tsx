// 设置页里的翻译、划词翻译、网络三组。
//
// 翻译源是一个有序列表：排在最前面的可用源就是默认源，翻译失败时依次往后降级；
// 翻译面板"同时显示多个源"时也按这个顺序排。每个源都能单独"检测"一下通不通。

import { useQuery } from '@tanstack/react-query';
import { RotateCcw } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { system, translate } from '@/lib/ipc';
import { accelLabel, isMac } from '@/lib/platform';
import type { GlassTuning, Settings } from '@/lib/types';
import { Button, Segmented, Select, Slider, Switch, TextField } from '@/ui/controls';
import { notify } from '@/ui/overlays';
import { SpeakButton } from '@/ui/SpeakButton';
import { LANGS } from '@/views/translate/TranslateBox';

import { GlassPreview } from './GlassPreview';
import { Group, Row } from './settingsParts';
import { TranslateProviderList } from './TranslateProviders';

/** 自定义玻璃的滑块（顺序就是设置页里的顺序）和默认值（就是"液态玻璃"的样子，和 Rust 的 GlassTuning::default 一致） */
const GLASS_KEYS = ['dispersion', 'blur', 'refraction', 'magnify', 'veil', 'tint', 'highlight'] as const satisfies readonly (keyof GlassTuning)[];
const DEFAULT_GLASS: GlassTuning = { dispersion: 35, blur: 0, refraction: 48, magnify: 20, veil: 88, tint: 10, highlight: 70 };

/**
 * 悬浮预览：调面板外观时，在主窗口旁边把真的翻译面板亮出来（就是平时弹出来的那个窗口），大小、材质、圆角、
 * 字号都是实际的 —— 放在主窗口里面画一个的话，看不出平时用的时候到底多大。
 * 一动下面的选项就自己出现并跟着变；离开这一页、主窗口藏起来时收掉。
 */
function PopupPreviewRow({ popup }: { popup: Settings['translate']['popup'] }) {
  const { t } = useTranslation();
  const [on, setOn] = useState(false);
  const look = JSON.stringify([popup.width, popup.height, popup.fontSize, popup.blur, popup.opacity, popup.radius, popup.showSource, popup.aiLayout]);
  const first = useRef(look);
  // 动了外观的选项：自动打开预览
  useEffect(() => {
    if (look !== first.current) setOn(true);
  }, [look]);
  // 开着的时候，设置每变一次（等后端存好）就按新的重摆一次
  useEffect(() => {
    if (!on) return;
    const timer = window.setTimeout(() => void translate.popupPreview(true).catch(notify.error), 200);
    return () => window.clearTimeout(timer);
  }, [on, look]);
  // 面板上点了关闭：开关跟着拨回去
  useEvent('translate-preview-closed', () => setOn(false));
  useEffect(() => {
    if (!on) return;
    const hide = () => void translate.popupPreview(false).catch(() => undefined);
    const onVisibility = () => document.hidden && setOn(false);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      document.removeEventListener('visibilitychange', onVisibility);
      hide();
    };
  }, [on]);
  return (
    <Row title={t('settings.popup.preview')} desc={t('settings.popup.previewDesc')}>
      <Switch checked={on} onChange={setOn} />
    </Row>
  );
}


export function TranslateSettingsGroups({ settings, set }: { settings: Settings; set: (mutate: (d: Settings) => void) => void }) {
  const { t } = useTranslation();
  const tr = settings.translate;
  const sel = tr.selection;
  const net = settings.network;
  const pop = tr.popup;
  const providers = useQuery({ queryKey: ['translate-providers'], queryFn: translate.providers });
  const systemLang = useQuery({ queryKey: ['system-language'], queryFn: translate.systemLanguage, staleTime: Infinity });

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
        <Row title={t('settings.translate.regionProvider')} desc={t('settings.translate.regionProviderDesc')}>
          <Select
            value={tr.regionProvider || 'auto'}
            options={[
              { value: 'auto', label: t('settings.translate.regionAuto') },
              ...(providers.data ?? [])
                .filter((p) => (p.enabled && p.configured) || p.id === tr.regionProvider)
                .map((p) => ({ value: p.id, label: p.name })),
            ]}
            onChange={(v) => set((d) => void (d.translate.regionProvider = v))}
          />
        </Row>
        <TranslateProviderList settings={settings} set={set} />
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
        <Row title={t('settings.selection.buttonStyle')} desc={t('settings.selection.buttonStyleDesc')}>
          <Segmented
            value={sel.buttonStyle}
            options={[
              { value: 'liquid', label: t('settings.selection.liquid') },
              { value: 'frosted', label: t('settings.selection.frosted') },
              { value: 'solid', label: t('settings.selection.solid') },
              { value: 'custom', label: t('settings.selection.custom') },
            ]}
            onChange={(v) => set((d) => void (d.translate.selection.buttonStyle = v))}
          />
        </Row>
        <div className="set-row set-row--stack">
          <div className="set-row__label">
            <div className="set-row__title">{t('settings.selection.preview')}</div>
            <div className="set-row__desc">{t('settings.selection.previewDesc')}</div>
          </div>
          <div className="glass-tune">
            <GlassPreview style={sel.buttonStyle} tuning={sel.glass} size={sel.buttonSize} />
            {sel.buttonStyle === 'custom' && (
              <div className="glass-tune__sliders">
                {GLASS_KEYS.map((k) => (
                  <label key={k} className="glass-tune__row">
                    <span className="glass-tune__name">{t(`settings.selection.glass.${k}`)}</span>
                    <Slider value={sel.glass[k]} min={0} max={100} step={1} onChange={(v) => set((d) => void (d.translate.selection.glass[k] = v))} />
                    <span className="glass-tune__value cn-numeric">{sel.glass[k]}</span>
                  </label>
                ))}
                <Button
                  size="sm"
                  variant="ghost"
                  icon={RotateCcw}
                  onClick={() => set((d) => void (d.translate.selection.glass = { ...DEFAULT_GLASS }))}
                >
                  {t('settings.selection.glassReset')}
                </Button>
              </div>
            )}
          </div>
        </div>
        <Row title={t('settings.selection.skipSystem')} desc={t('settings.selection.skipSystemDesc', { lang: systemLang.data ? t(`lang.${systemLang.data}`, { defaultValue: systemLang.data }) : '' })}>
          <Switch checked={sel.skipSystemLanguage} disabled={!sel.showButton} onChange={(v) => set((d) => void (d.translate.selection.skipSystemLanguage = v))} />
        </Row>
        <div className="set-row set-row--stack">
          <div className="set-row__label">
            <div className="set-row__title">{t('settings.selection.skipLanguages')}</div>
            <div className="set-row__desc">{t('settings.selection.skipLanguagesDesc')}</div>
          </div>
          <div className="set-chips">
            {SKIP_LANGS.map((l) => {
              const on = sel.skipLanguages.includes(l);
              return (
                <button
                  key={l}
                  type="button"
                  className="set-chip"
                  data-on={on || undefined}
                  disabled={!sel.showButton}
                  onClick={() =>
                    set((d) => {
                      const list = d.translate.selection.skipLanguages;
                      d.translate.selection.skipLanguages = on ? list.filter((x) => x !== l) : [...list, l];
                    })
                  }
                >
                  {t(`lang.${l}`)}
                </button>
              );
            })}
          </div>
        </div>
        <Row title={t('settings.selection.buttonSize')}>
          <span className="set-row__value cn-numeric">{sel.buttonSize}px</span>
          <Slider value={sel.buttonSize} min={20} max={48} step={1} onChange={(v) => set((d) => void (d.translate.selection.buttonSize = v))} />
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
        <PopupPreviewRow popup={pop} />
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

      <SpeechGroup settings={settings} set={set} />

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

/** "这些语言不弹"能选的：本地能认出来的几种（别的拉丁字母语言都算英文） */
const SKIP_LANGS = ['zh', 'en', 'ja', 'ko', 'ru', 'ar', 'th'];

/** 朗读设置里能单独选声音的语言 */
const SPEECH_LANGS = ['zh', 'zh-TW', 'en', 'ja', 'ko'] as const;
const SPEECH_SAMPLES: Record<(typeof SPEECH_LANGS)[number], string> = {
  zh: '你好，这是朗读的效果。',
  'zh-TW': '你好，這是朗讀的效果。',
  en: 'Hello, this is how it sounds.',
  ja: 'こんにちは、読み上げのサンプルです。',
  ko: '안녕하세요, 읽어 주기 예시입니다.',
};

type VoiceOption = { id: string; name: string; lang: string; quality: number };
const SYSTEM_VOICE = '__system__';

/** 系统的声音（macOS 后端给）；别的平台用浏览器的语音列表 */
function useVoices(): VoiceOption[] {
  const [voices, setVoices] = useState<VoiceOption[]>([]);
  useEffect(() => {
    let alive = true;
    const fromWeb = () =>
      typeof window.speechSynthesis === 'undefined'
        ? []
        : window.speechSynthesis.getVoices().map((v) => ({ id: v.voiceURI, name: v.name, lang: v.lang.replace('_', '-'), quality: 1 }));
    void system
      .speakVoices()
      .then((list) => {
        if (!alive) return;
        if (list.length) return setVoices(list);
        setVoices(fromWeb());
        window.speechSynthesis?.addEventListener('voiceschanged', () => alive && setVoices(fromWeb()));
      })
      .catch(() => alive && setVoices(fromWeb()));
    return () => {
      alive = false;
    };
  }, []);
  return voices;
}

/** 某种语言能用的声音：繁体中文认 zh-TW / zh-HK，其他按语言部分 */
function voicesFor(voices: VoiceOption[], lang: string) {
  return voices.filter((v) => {
    const l = v.lang.toLowerCase();
    if (lang === 'zh-TW') return l === 'zh-tw' || l === 'zh-hk';
    if (lang === 'zh') return l === 'zh-cn' || l === 'zh';
    return l.split('-')[0] === lang;
  });
}

function SpeechGroup({ settings, set }: { settings: Settings; set: (mutate: (d: Settings) => void) => void }) {
  const { t } = useTranslation();
  const sp = settings.translate.speech;
  const voices = useVoices();
  const quality = (q: number) => (q >= 3 ? ` · ${t('settings.speech.premium')}` : q === 2 ? ` · ${t('settings.speech.enhanced')}` : '');
  return (
    <Group id="speech" title={t('settings.speech.title')} note={t('settings.speech.note')}>
      <Row title={t('settings.speech.rate')}>
        <span className="set-row__value cn-numeric">{sp.rate.toFixed(1)}×</span>
        <Slider value={Math.round(sp.rate * 10)} min={5} max={20} onChange={(v) => set((d) => void (d.translate.speech.rate = v / 10))} />
      </Row>
      <Row title={t('settings.speech.pitch')}>
        <span className="set-row__value cn-numeric">{sp.pitch.toFixed(1)}×</span>
        <Slider value={Math.round(sp.pitch * 10)} min={5} max={20} onChange={(v) => set((d) => void (d.translate.speech.pitch = v / 10))} />
      </Row>
      {SPEECH_LANGS.map((lang) => {
        const list = voicesFor(voices, lang);
        return (
          <Row key={lang} title={t('settings.speech.voiceFor', { lang: t(`lang.${lang}`) })}>
            <Select
              // 下拉框不收空值，"系统默认"用一个占位的值
              value={sp.voices[lang] && list.some((v) => v.id === sp.voices[lang]) ? sp.voices[lang] : SYSTEM_VOICE}
              width={220}
              options={[{ value: SYSTEM_VOICE, label: t('settings.speech.systemVoice') }, ...list.map((v) => ({ value: v.id, label: `${v.name}${quality(v.quality)}` }))]}
              onChange={(v) =>
                set((d) => {
                  if (v !== SYSTEM_VOICE) d.translate.speech.voices[lang] = v;
                  else delete d.translate.speech.voices[lang];
                })
              }
            />
            <SpeakButton text={SPEECH_SAMPLES[lang]} lang={lang} label={t('settings.speech.try')} />
          </Row>
        );
      })}
    </Group>
  );
}
