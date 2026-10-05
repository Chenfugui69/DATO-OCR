// 设置页（规格 06 §6.2）：iOS 设置那种分组卡片。
// 分组：通用 · 截图 · 长截图 · 文字识别 · 翻译 · 剪贴板 · 快捷键 · 外观 · 存储 · 关于

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Crop, FolderOpen, RotateCcw } from 'lucide-react';
import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { formatBytes } from '@/lib/format';
import { clipboard, system } from '@/lib/ipc';
import { useSettings, useSettingsStore } from '@/lib/settings';
import { currentVisuals } from '@/lib/theme';
import type { HotkeyAction, Settings } from '@/lib/types';
import { Button, Segmented, Select, Slider, Switch, TextField } from '@/ui/controls';
import { HotkeyInput } from '@/ui/HotkeyInput';
import { confirmDialog, notify } from '@/ui/overlays';

import { AiSettingsGroups } from './AiSettings';
import { FrameStyleRows } from './FrameStyleRows';
import { Group, Row } from './settingsParts';
import { TranslateSettingsGroups } from './TranslateSettings';

const SECTIONS = ['general', 'capture', 'longshot', 'ocr', 'translate', 'selection', 'ai', 'network', 'clipboard', 'hotkeys', 'appearance', 'storage', 'about'] as const;

export function SettingsPage({ section }: { section: string | null }) {
  const { t } = useTranslation();
  const settings = useSettings();
  const update = useSettingsStore((s) => s.update);
  const scroller = useRef<HTMLDivElement>(null);
  const qc = useQueryClient();
  const info = useQuery({ queryKey: ['app-info'], queryFn: system.appInfo });
  const hotkeys = useQuery({ queryKey: ['hotkeys'], queryFn: system.hotkeys });

  useEffect(() => {
    if (!section) return;
    // 等这一帧布局完再滚
    const id = window.setTimeout(() => document.getElementById(`set-${section}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' }), 60);
    return () => window.clearTimeout(id);
  }, [section]);

  if (!settings) return null;
  const set = (mutate: (d: Settings) => void) => {
    update(mutate).catch(notify.error);
  };
  const setHotkey = (action: HotkeyAction, accel: string) => {
    update((d) => {
      d.hotkeys[action] = accel;
    })
      .then(() => window.setTimeout(() => void qc.invalidateQueries({ queryKey: ['hotkeys'] }), 300))
      .catch(notify.error);
  };
  const hk = (action: HotkeyAction) => hotkeys.data?.find((h) => h.action === action);
  const visuals = currentVisuals();
  const c = settings.capture;
  const cb = settings.clipboard;

  return (
    <section className="page">
      <div className="page__head">
        <h1 className="page__title">{t('nav.settings')}</h1>
      </div>
      <nav className="settings-nav">
        {SECTIONS.map((s) => (
          <button
            key={s}
            type="button"
            className="chip"
            data-active={section === s}
            onClick={() => document.getElementById(`set-${s}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' })}
          >
            {t(`settings.section.${s}`)}
          </button>
        ))}
      </nav>
      <div ref={scroller} className="page__body">
        <div className="settings">
          <Group id="general" title={t('settings.section.general')}>
            <Row title={t('settings.general.autoStart')}>
              <Switch checked={settings.general.autoStart} onChange={(v) => set((d) => void (d.general.autoStart = v))} />
            </Row>
            <Row title={t('settings.general.closeToTray')} desc={t('settings.general.closeToTrayDesc')}>
              <Switch checked={settings.general.closeToTray} onChange={(v) => set((d) => void (d.general.closeToTray = v))} />
            </Row>
            <Row title={t('settings.general.language')}>
              <Select
                value={settings.general.language}
                options={[
                  { value: 'zh-CN', label: '简体中文' },
                  { value: 'en-US', label: 'English' },
                ]}
                onChange={(v) => set((d) => void (d.general.language = v))}
              />
            </Row>
            <Row title={t('settings.general.offline')} desc={t('settings.general.offlineDesc')}>
              <Switch checked={settings.general.offlineMode} onChange={(v) => set((d) => void (d.general.offlineMode = v))} />
            </Row>
          </Group>

          <Group id="capture" title={t('settings.section.capture')}>
            <Row title={t('settings.capture.finishAction')}>
              <Select
                value={c.finishAction}
                options={[
                  { value: 'copy', label: t('settings.capture.finishCopy') },
                  { value: 'copyAndSave', label: t('settings.capture.finishCopySave') },
                ]}
                onChange={(v) => set((d) => void (d.capture.finishAction = v))}
              />
            </Row>
            <Row title={t('settings.capture.saveToLibrary')} desc={t('settings.capture.saveToLibraryDesc')}>
              <Switch checked={c.saveToLibrary} onChange={(v) => set((d) => void (d.capture.saveToLibrary = v))} />
            </Row>
            <Row title={t('settings.capture.saveDir')} desc={<span className="cn-selectable">{info.data?.saveDir}</span>}>
              <Button
                size="sm"
                onClick={() =>
                  void system.pickDirectory().then((dir) => {
                    if (dir) {
                      set((d) => void (d.capture.saveDirectory = dir));
                      void qc.invalidateQueries({ queryKey: ['app-info'] });
                    }
                  })
                }
              >
                {t('common.change')}
              </Button>
              <Button size="sm" variant="ghost" icon={FolderOpen} onClick={() => void system.openFolder('save')}>
                {t('common.open')}
              </Button>
            </Row>
            <Row title={t('settings.capture.fileName')} desc={t('settings.capture.fileNameDesc')}>
              <TextField
                defaultValue={c.fileNameTemplate}
                style={{ width: 240 }}
                onBlur={(e) => e.target.value.trim() && set((d) => void (d.capture.fileNameTemplate = e.target.value.trim()))}
              />
            </Row>
            <Row title={t('settings.capture.format')}>
              <Segmented
                value={c.imageFormat}
                options={[
                  { value: 'png', label: 'PNG' },
                  { value: 'jpg', label: 'JPG' },
                ]}
                onChange={(v) => set((d) => void (d.capture.imageFormat = v))}
              />
            </Row>
            {c.imageFormat === 'jpg' && (
              <Row title={t('settings.capture.jpgQuality')}>
                <span className="set-row__value cn-numeric">{c.jpgQuality}</span>
                <Slider value={c.jpgQuality} min={60} max={100} onChange={(v) => set((d) => void (d.capture.jpgQuality = v))} />
              </Row>
            )}
            <Row title={t('settings.capture.maskOpacity')} desc={t('settings.capture.maskOpacityDesc')}>
              <span className="set-row__value cn-numeric">{c.maskOpacity > 0 ? `${Math.round(c.maskOpacity * 100)}%` : t('settings.capture.noDim')}</span>
              <Slider value={Math.round(c.maskOpacity * 100)} min={0} max={80} onChange={(v) => set((d) => void (d.capture.maskOpacity = v / 100))} />
            </Row>
            <Row title={t('settings.capture.instantAction')} desc={t('settings.capture.instantActionDesc', { hotkey: settings.hotkeys.instant })}>
              <Select
                value={c.instantAction}
                options={[
                  { value: 'select', label: t('settings.capture.instantSelect') },
                  { value: 'copy', label: t('settings.capture.instantCopy') },
                  { value: 'save', label: t('settings.capture.instantSave') },
                ]}
                onChange={(v) => set((d) => void (d.capture.instantAction = v))}
              />
            </Row>
            <Row title={t('settings.capture.instantCursor')} desc={t('settings.capture.instantCursorDesc')}>
              <Switch checked={c.instantCursor} onChange={(v) => set((d) => void (d.capture.instantCursor = v))} />
            </Row>
            <Row title={t('settings.capture.gifFps')} desc={t('settings.capture.gifFpsDesc')}>
              <Segmented
                value={String(c.gifFps)}
                options={['10', '15', '20', '30'].map((v) => ({ value: v, label: `${v} fps` }))}
                onChange={(v) => set((d) => void (d.capture.gifFps = Number(v)))}
              />
            </Row>
            <Row title={t('settings.capture.gifCursor')}>
              <Switch checked={c.gifCursor} onChange={(v) => set((d) => void (d.capture.gifCursor = v))} />
            </Row>
            <FrameStyleRows name={t('settings.frame.captureName')} value={c.frame} onChange={(v) => set((d) => void (d.capture.frame = v))} />
            <Row title={t('settings.capture.magnifier')}>
              <Switch checked={c.showMagnifier} onChange={(v) => set((d) => void (d.capture.showMagnifier = v))} />
            </Row>
            <Row title={t('settings.capture.colorFormat')} desc={t('settings.capture.colorFormatDesc')}>
              <Segmented
                value={c.colorFormat}
                options={[
                  { value: 'hex', label: 'HEX' },
                  { value: 'rgb', label: 'RGB' },
                  { value: 'hsl', label: 'HSL' },
                ]}
                onChange={(v) => set((d) => void (d.capture.colorFormat = v))}
              />
            </Row>
            <Row title={t('settings.capture.detectWindows')}>
              <Switch checked={c.detectWindows} onChange={(v) => set((d) => void (d.capture.detectWindows = v))} />
            </Row>
            <Row title={t('settings.capture.detectChildren')} desc={t('settings.capture.detectChildrenDesc')}>
              <Switch checked={c.detectChildWindows} disabled={!c.detectWindows} onChange={(v) => set((d) => void (d.capture.detectChildWindows = v))} />
            </Row>
            <Row title={t('settings.capture.snap')} desc={t('settings.capture.snapDesc')}>
              <span className="set-row__value cn-numeric">{c.snapThreshold}px</span>
              <Slider value={c.snapThreshold} min={0} max={24} onChange={(v) => set((d) => void (d.capture.snapThreshold = v))} />
            </Row>
            <Row title={t('settings.capture.rightClick')}>
              <Select
                value={c.rightClick}
                options={[
                  { value: 'exit', label: t('settings.capture.rightClickExit') },
                  { value: 'cancelSelection', label: t('settings.capture.rightClickCancel') },
                ]}
                onChange={(v) => set((d) => void (d.capture.rightClick = v))}
              />
            </Row>
          </Group>

          <Group id="longshot" title={t('settings.section.longshot')} note={t('settings.longshot.note')}>
            <Row title={t('settings.longshot.debounce')} desc={t('settings.longshot.debounceDesc')}>
              <span className="set-row__value cn-numeric">{settings.longshot.scrollDebounceMs}ms</span>
              <Slider
                value={settings.longshot.scrollDebounceMs}
                min={100}
                max={400}
                step={20}
                onChange={(v) => set((d) => void (d.longshot.scrollDebounceMs = v))}
              />
            </Row>
            <Row title={t('settings.longshot.maxHeight')}>
              <Select
                value={String(settings.longshot.maxHeight)}
                options={['8000', '16000', '32000'].map((v) => ({ value: v, label: `${Number(v).toLocaleString()} px` }))}
                onChange={(v) => set((d) => void (d.longshot.maxHeight = Number(v)))}
              />
            </Row>
          </Group>

          <Group
            id="ocr"
            title={t('settings.section.ocr')}
            note={info.data && !info.data.ocr.rapid ? t('settings.ocr.rapidMissing') : t('settings.ocr.note')}
          >
            <Row title={t('settings.ocr.engine')}>
              <Select
                value={settings.ocr.engine}
                options={[
                  { value: 'rapid', label: `RapidOCR${info.data?.ocr.rapid === false ? ` (${t('settings.ocr.unavailable')})` : ''}` },
                  { value: 'system', label: `${t('settings.ocr.system')}${info.data?.ocr.system === false ? ` (${t('settings.ocr.unavailable')})` : ''}` },
                ]}
                onChange={(v) => set((d) => void (d.ocr.engine = v))}
              />
            </Row>
            <Row title={t('settings.ocr.instant')} desc={t('settings.ocr.instantDesc')}>
              <Switch checked={c.ocrInstant} onChange={(v) => set((d) => void (d.capture.ocrInstant = v))} />
            </Row>
            <Row title={t('settings.ocr.maskOpacity')} desc={t('settings.ocr.maskOpacityDesc')}>
              <span className="set-row__value cn-numeric">{c.ocrMaskOpacity > 0 ? `${Math.round(c.ocrMaskOpacity * 100)}%` : t('settings.capture.noDim')}</span>
              <Slider value={Math.round(c.ocrMaskOpacity * 100)} min={0} max={80} onChange={(v) => set((d) => void (d.capture.ocrMaskOpacity = v / 100))} />
            </Row>
            <FrameStyleRows name={t('settings.frame.ocrName')} value={c.ocrFrame} onChange={(v) => set((d) => void (d.capture.ocrFrame = v))} />
            <Row title={t('settings.ocr.idle')} desc={t('settings.ocr.idleDesc')}>
              <Select
                value={String(settings.ocr.idleTimeoutMinutes)}
                options={[
                  { value: '1', label: t('settings.ocr.minutes', { count: 1 }) },
                  { value: '5', label: t('settings.ocr.minutes', { count: 5 }) },
                  { value: '15', label: t('settings.ocr.minutes', { count: 15 }) },
                  { value: '0', label: t('settings.ocr.never') },
                ]}
                onChange={(v) => set((d) => void (d.ocr.idleTimeoutMinutes = Number(v)))}
              />
            </Row>
            <Row title={t('settings.ocr.keepBreaks')}>
              <Switch checked={settings.ocr.keepLineBreaks} onChange={(v) => set((d) => void (d.ocr.keepLineBreaks = v))} />
            </Row>
            <Row title={t('settings.ocr.paddle')} desc={info.data?.ocr.avx ? t('settings.ocr.paddleSoon') : t('settings.ocr.paddleNoAvx')}>
              <Button size="sm" disabled>
                {t('settings.ocr.download')}
              </Button>
            </Row>
          </Group>

          <TranslateSettingsGroups settings={settings} set={set} />

          <AiSettingsGroups settings={settings} set={set} />

          <Group id="clipboard" title={t('settings.section.clipboard')} note={t('settings.clipboard.privacyNote')}>
            <Row title={t('settings.clipboard.enabled')}>
              <Switch checked={cb.enabled} onChange={(v) => set((d) => void (d.clipboard.enabled = v))} />
            </Row>
            <Row title={t('settings.clipboard.panelStyle')}>
              <Segmented
                value={cb.panelStyle}
                options={[
                  { value: 'bottom', label: t('settings.clipboard.bottom') },
                  { value: 'vertical', label: t('settings.clipboard.vertical') },
                ]}
                onChange={(v) => set((d) => void (d.clipboard.panelStyle = v))}
              />
            </Row>
            <Row title={t('settings.clipboard.respectPrivacy')} desc={t('settings.clipboard.respectPrivacyDesc')}>
              <Switch checked={cb.respectPrivacyFlag} onChange={(v) => set((d) => void (d.clipboard.respectPrivacyFlag = v))} />
            </Row>
            <Row title={t('settings.clipboard.blacklist')} desc={t('settings.clipboard.blacklistDesc')}>
              <Switch checked={cb.useBlacklist} onChange={(v) => set((d) => void (d.clipboard.useBlacklist = v))} />
            </Row>
            {cb.useBlacklist && (
              <Row title={t('settings.clipboard.blacklistApps')}>
                <TextField
                  defaultValue={cb.blacklist.join(', ')}
                  placeholder="keepass.exe, 1password.exe"
                  style={{ width: 280 }}
                  onBlur={(e) =>
                    set((d) => {
                      d.clipboard.blacklist = e.target.value
                        .split(/[,，\s]+/)
                        .map((s) => s.trim())
                        .filter(Boolean);
                    })
                  }
                />
              </Row>
            )}
            <Row title={t('settings.clipboard.retention')} desc={t('settings.clipboard.retentionDesc')}>
              <Select
                value={String(cb.retentionDays)}
                options={['0', '30', '90', '180', '365'].map((v) => ({ value: v, label: v === '0' ? t('settings.clipboard.forever') : t('settings.clipboard.days', { count: Number(v) }) }))}
                onChange={(v) => set((d) => void (d.clipboard.retentionDays = Number(v)))}
              />
            </Row>
            <Row title={t('settings.clipboard.maxItems')}>
              <Select
                value={String(cb.retentionMaxItems)}
                options={['0', '10000', '50000'].map((v) => ({ value: v, label: v === '0' ? t('settings.clipboard.unlimited') : Number(v).toLocaleString() }))}
                onChange={(v) => set((d) => void (d.clipboard.retentionMaxItems = Number(v)))}
              />
            </Row>
            <Row title={t('settings.clipboard.limits')}>
              <Select
                value={String(cb.maxTextMb)}
                width={96}
                options={['1', '5', '20'].map((v) => ({ value: v, label: t('settings.clipboard.textMb', { mb: v }) }))}
                onChange={(v) => set((d) => void (d.clipboard.maxTextMb = Number(v)))}
              />
              <Select
                value={String(cb.maxImageMb)}
                width={96}
                options={['10', '30', '100'].map((v) => ({ value: v, label: t('settings.clipboard.imageMb', { mb: v }) }))}
                onChange={(v) => set((d) => void (d.clipboard.maxImageMb = Number(v)))}
              />
            </Row>
            <Row title={t('settings.clipboard.clear')} desc={t('settings.clipboard.clearDesc')}>
              <Button
                size="sm"
                variant="danger"
                onClick={() =>
                  void confirmDialog({ title: t('clip.clearTitle'), body: t('clip.clearBody'), confirmLabel: t('clip.clear'), danger: true }).then((ok) => {
                    if (!ok) return;
                    clipboard
                      .clear('unpinned')
                      .then((n) => notify.success(t('clip.cleared', { count: n })))
                      .catch(notify.error);
                  })
                }
              >
                {t('clip.clear')}
              </Button>
            </Row>
          </Group>

          <Group id="hotkeys" title={t('settings.section.hotkeys')} note={t('settings.hotkeys.note')}>
            {(['capture', 'instant', 'longshot', 'ocr', 'clipboard', 'translate', 'ai'] as HotkeyAction[]).map((a) => {
              const status = hk(a);
              return (
                <Row key={a} title={t(`settings.hotkeys.${a}`)} desc={status && !status.ok ? <span className="set-row__error">{status.error}</span> : undefined}>
                  <HotkeyInput value={settings.hotkeys[a]} error={status ? !status.ok : false} onChange={(v) => setHotkey(a, v)} />
                </Row>
              );
            })}
            <Row title={t('settings.hotkeys.reset')}>
              <Button
                size="sm"
                icon={RotateCcw}
                onClick={() =>
                  set((d) => {
                    d.hotkeys = { ...d.hotkeys, capture: 'F1', longshot: 'F2', ocr: 'F3', clipboard: 'Alt+V', translate: 'Ctrl+Alt+T', instant: 'Shift+F1' };
                  })
                }
              >
                {t('settings.hotkeys.resetButton')}
              </Button>
            </Row>
          </Group>

          <Group
            id="appearance"
            title={t('settings.section.appearance')}
            note={visuals && !visuals.transparencyEnabled ? t('settings.appearance.transparencyOff') : visuals?.powerSaver ? t('settings.appearance.powerSaver') : undefined}
          >
            <Row title={t('settings.appearance.theme')}>
              <Segmented
                value={settings.appearance.theme}
                options={[
                  { value: 'system', label: t('settings.appearance.system') },
                  { value: 'light', label: t('settings.appearance.light') },
                  { value: 'dark', label: t('settings.appearance.dark') },
                ]}
                onChange={(v) => set((d) => void (d.appearance.theme = v))}
              />
            </Row>
            <Row title={t('settings.appearance.glass')} desc={t('settings.appearance.glassDesc')}>
              <Segmented
                value={settings.appearance.glassEffect}
                options={[
                  { value: 'auto', label: t('settings.appearance.auto') },
                  { value: 'on', label: t('settings.appearance.on') },
                  { value: 'off', label: t('settings.appearance.off') },
                ]}
                onChange={(v) => set((d) => void (d.appearance.glassEffect = v))}
              />
            </Row>
          </Group>

          <Group id="storage" title={t('settings.section.storage')}>
            <Row title={t('settings.storage.dataDir')} desc={<span className="cn-selectable">{info.data?.dataDir}</span>}>
              <Button size="sm" icon={FolderOpen} onClick={() => void system.openFolder('data')}>
                {t('common.open')}
              </Button>
            </Row>
            <Row title={t('settings.storage.used')}>
              <span className="set-row__value">{info.data ? formatBytes(info.data.dataBytes) : '…'}</span>
            </Row>
            <Row title={t('settings.storage.logs')} desc={t('settings.storage.logsDesc')}>
              <Button size="sm" icon={FolderOpen} onClick={() => void system.openFolder('logs')}>
                {t('common.open')}
              </Button>
            </Row>
            <Row title={t('settings.storage.reset')} desc={t('settings.storage.resetDesc')}>
              <Button
                size="sm"
                onClick={() =>
                  void confirmDialog({ title: t('settings.storage.resetTitle'), confirmLabel: t('settings.storage.reset'), danger: true }).then((ok) => {
                    if (!ok) return;
                    system
                      .resetSettings()
                      .then((next) => useSettingsStore.setState({ settings: next }))
                      .catch(notify.error);
                  })
                }
              >
                {t('settings.storage.reset')}
              </Button>
            </Row>
          </Group>

          <Group id="about" title={t('settings.section.about')} note={t('settings.about.privacy')}>
            <div className="about">
              <span className="about__logo">
                <Crop size={26} strokeWidth={2} />
              </span>
              <div>
                <div style={{ font: 'var(--cn-text-title-2)' }}>DATO COR</div>
                <div className="set-row__desc">
                  {t('settings.about.version', { version: info.data?.version ?? '' })} · {t('settings.about.tagline')}
                </div>
              </div>
            </div>
            <Row title={t('settings.about.credits')} desc="Tauri · React · xcap · RapidOCR · SQLite · Lucide · Radix UI" />
            <Row title={t('settings.about.quit')}>
              <Button size="sm" onClick={() => void system.quit()}>
                {t('settings.about.quitButton')}
              </Button>
            </Row>
          </Group>
        </div>
      </div>
    </section>
  );
}
