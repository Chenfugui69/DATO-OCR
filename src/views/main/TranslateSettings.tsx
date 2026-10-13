// 设置页里的翻译、划词翻译、网络三组。
//
// 翻译源是一个有序列表：排在最前面的可用源就是默认源，翻译失败时依次往后降级；
// 翻译面板"同时显示多个源"时也按这个顺序排。每个源都能单独"检测"一下通不通。

import { RotateCcw } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { translate } from '@/lib/ipc';
import { accelLabel, isMac } from '@/lib/platform';
import type { Settings } from '@/lib/types';
import { Button, Segmented, Select, Slider, Switch, TextField } from '@/ui/controls';
import { notify } from '@/ui/overlays';
import { LANGS } from '@/views/translate/TranslateBox';

import { Group, Row } from './settingsParts';
import { TranslateProviderList } from './TranslateProviders';

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
            ]}
            onChange={(v) => set((d) => void (d.translate.selection.buttonStyle = v))}
          />
        </Row>
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
