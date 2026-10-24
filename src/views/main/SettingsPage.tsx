// 设置页（规格 06 §6.2）：iOS 设置那种分组卡片。
// 分组：通用 · 截图 · 长截图 · 文字识别 · 翻译 · 剪贴板 · 快捷键 · 外观 · 存储 · 关于

import { useQuery, useQueryClient } from '@tanstack/react-query';
import {
  ClipboardList,
  Crop,
  ExternalLink,
  FolderOpen,
  HardDrive,
  Info,
  Keyboard,
  Languages,
  type LucideIcon,
  Palette,
  Plug,
  RefreshCw,
  RotateCcw,
  ScanText,
  Search,
  ScrollText,
  Settings2,
  ShieldCheck,
  Sparkles,
  TextSelect,
  Video,
  Wifi,
} from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { formatBytes, relativeTime } from '@/lib/format';
import { useEvent } from '@/lib/events';
import { clipboard, ocr, record, system, update as updater } from '@/lib/ipc';
import { accelLabel, isMac } from '@/lib/platform';
import { useSettings, useSettingsStore } from '@/lib/settings';
import { currentVisuals } from '@/lib/theme';
import type { EngineStatus, HotkeyAction, MediaKind, PaddleDownload, Permissions, Settings } from '@/lib/types';
import { BrandMark } from '@/ui/BrandMark';
import { Button, SearchField, Segmented, Select, Slider, Switch, TextField } from '@/ui/controls';
import { HotkeyInput } from '@/ui/HotkeyInput';
import { confirmDialog, notify } from '@/ui/overlays';

import { AiSettingsGroups } from './AiSettings';
import { MailCodesRows } from './MailCodesSettings';
import { McpSettingsGroup } from './McpSettings';
import { ORB_COLORS, WaterOrb } from '@/views/clipboard-panel/WaterOrb';
import '@/views/clipboard-panel/orb.css';
import { FrameStyleRows } from './FrameStyleRows';
import { ToolbarOrder } from './ToolbarOrder';
import { Group, Row, SettingsSectionContext } from './settingsParts';
import { TranslateSettingsGroups } from './TranslateSettings';
import { UpdateBanner, UpdateDialog, useUpdate } from './UpdateDialog';

/** 验证码保留时间的预设（秒）；0 = 不删 */
const OTP_EXPIRE_PRESETS = ['30', '60', '300', '0'];

const SECTIONS = ['general', 'permissions', 'capture', 'longshot', 'record', 'ocr', 'translate', 'selection', 'ai', 'mcp', 'network', 'clipboard', 'hotkeys', 'appearance', 'storage', 'update', 'about'] as const;
type Section = (typeof SECTIONS)[number];

/** 左边栏每个分类的图标和底色（像 macOS 的系统设置：彩色圆角方块里一个白色图标） */
const SECTION_ICON: Record<Section, [LucideIcon, string]> = {
  general: [Settings2, '#8e8e93'],
  permissions: [ShieldCheck, '#007aff'],
  capture: [Crop, '#0a84ff'],
  longshot: [ScrollText, '#5856d6'],
  record: [Video, '#ff3b30'],
  ocr: [ScanText, '#34c759'],
  translate: [Languages, '#00a6b4'],
  selection: [TextSelect, '#32ade6'],
  ai: [Sparkles, '#af52de'],
  mcp: [Plug, '#7d5cf0'],
  network: [Wifi, '#007aff'],
  clipboard: [ClipboardList, '#ff9500'],
  hotkeys: [Keyboard, '#636366'],
  appearance: [Palette, '#ff2d55'],
  storage: [HardDrive, '#8e8e93'],
  update: [RefreshCw, '#30b158'],
  about: [Info, '#8e8e93'],
};

function SectionIcon({ section, large }: { section: Section; large?: boolean }) {
  const [Icon, color] = SECTION_ICON[section];
  return (
    <span className={large ? 'set-icon set-icon--lg' : 'set-icon'} style={{ '--tile': color } as React.CSSProperties}>
      <Icon size={large ? 24 : 14} strokeWidth={2} />
    </span>
  );
}

function isSection(s: string | null | undefined): s is Section {
  return !!s && (SECTIONS as readonly string[]).includes(s);
}

export function SettingsPage({ section }: { section: string | null }) {
  const { t } = useTranslation();
  const settings = useSettings();
  const update = useSettingsStore((s) => s.update);
  const scroller = useRef<HTMLDivElement>(null);
  const qc = useQueryClient();
  const info = useQuery({ queryKey: ['app-info'], queryFn: system.appInfo });
  const hotkeys = useQuery({ queryKey: ['hotkeys'], queryFn: system.hotkeys });
  const { status: upd, prompt } = useUpdate();
  const [updOpen, setUpdOpen] = useState(false);
  // 验证码保留时间选了"自定义"（数值还没改成预设以外的值时也要显示输入框）
  const [otpCustom, setOtpCustom] = useState(false);
  // macOS 的屏幕录制 / 辅助功能权限；别的平台两项都是 null，不显示这一组。
  // 用户去系统设置里改完回来时窗口重新获得焦点，顺带重查一次
  const perms = useQuery({ queryKey: ['permissions'], queryFn: system.permissions, refetchOnWindowFocus: true });
  const mediaAccess = useQuery({
    queryKey: ['media-access'],
    queryFn: async () => ({ microphone: await record.mediaAccess('microphone'), camera: await record.mediaAccess('camera') }),
    enabled: isMac,
    refetchOnWindowFocus: true,
  });
  const permRows = (['screenCapture', 'accessibility'] as (keyof Permissions)[]).filter((k) => perms.data?.[k] != null);

  // 左边栏选中的分类；别处（剪贴板面板的"剪贴板设置…"、托盘）可以指定打开哪一类
  const [current, setCurrent] = useState<Section>(isSection(section) ? section : 'general');
  useEffect(() => {
    if (isSection(section)) setCurrent(section);
  }, [section]);
  useEffect(() => {
    scroller.current?.scrollTo({ top: 0 });
  }, [current]);

  // 搜索：有字时右边显示所有分类，只留标题 / 说明里有这些字的行（分组标题、分类名对上了整组都留）
  const [query, setQuery] = useState('');
  const [hitSections, setHitSections] = useState<Set<string> | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const searching = query.trim() !== '';
  // 设置一变，条件显示的行可能多出来 / 少掉，跟着重新过滤
  useLayoutEffect(() => {
    const found = filterSettings(listRef.current, query);
    setHitSections((prev) => (sameSet(prev, found) ? prev : found));
  }, [query, settings, permRows.length]);
  useEffect(() => {
    scroller.current?.scrollTo({ top: 0 });
  }, [query]);
  // ⌘F / Ctrl+F 跳到搜索框
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'f') {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

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
  const rec = settings.record;
  // 打开麦克风 / 摄像头：先问系统要权限（第一次会弹框），被拒了就不打开，提示去系统设置里开
  const toggleMedia = async (kind: MediaKind, on: boolean) => {
    if (on) {
      const access = await record.mediaRequest(kind).catch(() => 'denied' as const);
      void qc.invalidateQueries({ queryKey: ['media-access'] });
      if (access !== 'granted') {
        notify.error(t('settings.record.denied', { name: t(`settings.record.${kind}`) }));
        return;
      }
    }
    set((d) => void (d.record[kind] = on));
  };
  const mediaDesc = (kind: MediaKind) => {
    const access = mediaAccess.data?.[kind];
    return access === 'denied' ? t('settings.record.deniedDesc') : t(`settings.record.${kind}Desc`);
  };
  const visuals = currentVisuals();
  const c = settings.capture;
  const cb = settings.clipboard;
  const smartModels = settings.ai.providers
    .filter((p) => p.enabled)
    .flatMap((p) => p.models.map((m) => ({ value: `${p.id}/${m}`, label: `${p.name} · ${m}` })));
  const otpExpireMode = otpCustom || !OTP_EXPIRE_PRESETS.includes(String(cb.otpExpireSecs)) ? 'custom' : String(cb.otpExpireSecs);

  return (
    <section className="page settings-page">
      {/* 左：分类列表（彩色图标）；右：只显示选中的那一类 */}
      <aside className="settings-side">
        <h1 className="settings-side__title">{t('nav.settings')}</h1>
        <SearchField
          ref={searchRef}
          className="settings-side__search"
          value={query}
          placeholder={t('settings.search.placeholder')}
          onChange={setQuery}
          onKeyDown={(e) => {
            if (e.key === 'Escape' && query) {
              e.stopPropagation();
              setQuery('');
            }
          }}
        />
        <nav className="settings-side__list">
          {SECTIONS.filter((s) => s !== 'permissions' || permRows.length > 0).map((s) => (
            <button
              key={s}
              type="button"
              className="settings-side__item"
              data-active={!searching && current === s}
              data-dim={searching && !hitSections?.has(s)}
              onClick={() => {
                setQuery('');
                setCurrent(s);
              }}
            >
              <SectionIcon section={s} />
              <span className="cn-truncate">{t(`settings.section.${s}`)}</span>
            </button>
          ))}
        </nav>
      </aside>
      <div ref={scroller} className="settings-main">
        {searching ? (
          <header className="settings-main__head">
            <span className="set-icon set-icon--lg" style={{ '--tile': '#8e8e93' } as React.CSSProperties}>
              <Search size={24} strokeWidth={2} />
            </span>
            <div>
              <h2>{t('settings.search.title')}</h2>
              <p>{hitSections?.size ? t('settings.search.desc', { query: query.trim() }) : t('settings.search.empty', { query: query.trim() })}</p>
            </div>
          </header>
        ) : (
          <header className="settings-main__head">
            <SectionIcon section={current} large />
            <div>
              <h2>{t(`settings.section.${current}`)}</h2>
              <p>{t(`settings.sectionDesc.${current}`)}</p>
            </div>
          </header>
        )}
        <SettingsSectionContext.Provider value={searching ? null : current}>
        <div ref={listRef} className="settings" data-searching={searching}>
          {prompt && upd && <UpdateBanner status={upd} onOpen={() => setUpdOpen(true)} />}
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
                  { value: 'system', label: t('settings.general.languageSystem') },
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

          {permRows.length > 0 && (
            <Group id="permissions" title={t('settings.section.permissions')} note={t('settings.permissions.note')}>
              {permRows.map((k) => {
                const name = k === 'screenCapture' ? 'screen' : 'accessibility';
                return (
                  <Row key={k} title={t(`settings.permissions.${name}`)} desc={t(`settings.permissions.${name}Desc`)}>
                    {perms.data?.[k] ? (
                      <span className="set-row__ok">{t('settings.permissions.granted')}</span>
                    ) : (
                      <Button size="sm" onClick={() => void system.requestPermission(k).catch(notify.error)}>
                        {t('settings.permissions.grant')}
                      </Button>
                    )}
                  </Row>
                );
              })}
            </Group>
          )}

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
            <Row title={t('settings.capture.instantAction')} desc={t('settings.capture.instantActionDesc', { hotkey: accelLabel(settings.hotkeys.instant) })}>
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
            <div className="set-row set-row--stack">
              <div className="set-row__label">
                <div className="set-row__title">{t('settings.capture.toolbarOrder')}</div>
                <div className="set-row__desc">{t('settings.capture.toolbarOrderDesc')}</div>
              </div>
              <ToolbarOrder value={c} onChange={(v) => set((d) => void Object.assign(d.capture, v))} />
            </div>
            <Row title={t('settings.capture.toolbarStyle')} desc={t('settings.capture.toolbarStyleDesc')}>
              <Segmented
                value={c.toolbarStyle === 'frosted' ? 'frosted' : 'glass'}
                options={[
                  { value: 'glass', label: t('settings.capture.toolbarStyleOpt.glass') },
                  { value: 'frosted', label: t('settings.capture.toolbarStyleOpt.frosted') },
                ]}
                onChange={(v) => set((d) => void (d.capture.toolbarStyle = v))}
              />
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
            <Row title={t('settings.capture.detectLevel')} desc={t('settings.capture.detectLevelDesc')}>
              <Segmented
                value={!c.detectWindows ? 'off' : !c.detectChildWindows ? 'window' : c.elementLevel === 'block' ? 'block' : 'element'}
                options={(['off', 'window', 'block', 'element'] as const).map((v) => ({ value: v, label: t(`settings.capture.detectLevelOpt.${v}`) }))}
                onChange={(v) =>
                  set((d) => {
                    d.capture.detectWindows = v !== 'off';
                    d.capture.detectChildWindows = v === 'block' || v === 'element';
                    if (v === 'block' || v === 'element') d.capture.elementLevel = v;
                  })
                }
              />
            </Row>
            <Row title={t('settings.capture.autoScanCodes')} desc={t('settings.capture.autoScanCodesDesc')}>
              <Switch checked={c.autoScanCodes} onChange={(v) => set((d) => void (d.capture.autoScanCodes = v))} />
            </Row>
            <Row title={t('settings.capture.snap')} desc={t('settings.capture.snapDesc')}>
              <span className="set-row__value cn-numeric">{c.snapThreshold}px</span>
              <Slider value={c.snapThreshold} min={0} max={24} onChange={(v) => set((d) => void (d.capture.snapThreshold = v))} />
            </Row>
            <Row title={t('settings.capture.rightClick')}>
              <Select
                value={c.rightClick}
                options={[
                  { value: 'none', label: t('settings.capture.rightClickNone') },
                  { value: 'exit', label: t('settings.capture.rightClickExit') },
                  { value: 'cancelSelection', label: t('settings.capture.rightClickCancel') },
                ]}
                onChange={(v) => set((d) => void (d.capture.rightClick = v))}
              />
            </Row>
          </Group>

          <Group id="record" title={t('settings.section.record')}>
            <Row title={t('settings.record.gifResolution')} desc={t('settings.record.gifResolutionDesc')}>
              <Segmented
                value={rec.gifResolution}
                options={[
                  { value: 'original', label: t('settings.record.original') },
                  { value: '1080p', label: '1080p' },
                  { value: '720p', label: '720p' },
                  { value: '480p', label: '480p' },
                ]}
                onChange={(v) => set((d) => void (d.record.gifResolution = v))}
              />
            </Row>
            <Row title={t('settings.capture.gifFps')} desc={t('settings.capture.gifFpsDesc')}>
              <Segmented
                value={String(c.gifFps)}
                options={['5', '10', '15', '20', '30'].map((v) => ({ value: v, label: `${v} fps` }))}
                onChange={(v) => set((d) => void (d.capture.gifFps = Number(v)))}
              />
            </Row>
            <Row title={t('settings.record.gifLoop')} desc={t('settings.record.gifLoopDesc')}>
              <Switch checked={rec.gifLoop} onChange={(v) => set((d) => void (d.record.gifLoop = v))} />
            </Row>
            <Row title={t('settings.record.cursor')}>
              <Switch checked={c.gifCursor} onChange={(v) => set((d) => void (d.capture.gifCursor = v))} />
            </Row>
            <Row title={t('settings.record.editBeforeSave')} desc={t('settings.record.editBeforeSaveDesc')}>
              <Switch checked={rec.editBeforeSave} onChange={(v) => set((d) => void (d.record.editBeforeSave = v))} />
            </Row>
          </Group>
          {isMac && (
            <Group id="record-video" title={t('settings.record.videoTitle')} note={t('settings.record.videoNote')}>
              <Row title={t('settings.record.resolution')} desc={t('settings.record.resolutionDesc')}>
                <Segmented
                  value={rec.resolution}
                  options={[
                    { value: 'original', label: t('settings.record.original') },
                    { value: '1080p', label: '1080p' },
                    { value: '720p', label: '720p' },
                  ]}
                  onChange={(v) => set((d) => void (d.record.resolution = v))}
                />
              </Row>
              <Row title={t('settings.record.fps')}>
                <Segmented
                  value={String(rec.fps)}
                  options={['15', '24', '30', '60'].map((v) => ({ value: v, label: `${v} fps` }))}
                  onChange={(v) => set((d) => void (d.record.fps = Number(v) as Settings['record']['fps']))}
                />
              </Row>
              <Row title={t('settings.record.quality')} desc={t('settings.record.qualityDesc')}>
                <span className="set-row__value cn-numeric">{rec.quality}</span>
                <Slider value={rec.quality} min={10} max={100} step={5} onChange={(v) => set((d) => void (d.record.quality = v))} />
              </Row>
              <Row title={t('settings.record.codec')} desc={t('settings.record.codecDesc')}>
                <Segmented
                  value={rec.codec}
                  options={[
                    { value: 'h264', label: 'H.264' },
                    { value: 'hevc', label: 'H.265' },
                  ]}
                  onChange={(v) => set((d) => void (d.record.codec = v))}
                />
              </Row>
              <Row title={t('settings.record.maxSeconds')}>
                <Select
                  value={String(rec.maxSeconds)}
                  options={[60, 180, 300, 600, 1800, 3600].map((v) => ({ value: String(v), label: t('settings.record.minutes', { count: v / 60 }) }))}
                  onChange={(v) => set((d) => void (d.record.maxSeconds = Number(v)))}
                />
              </Row>
              <Row title={t('settings.record.systemAudio')} desc={t('settings.record.systemAudioDesc')}>
                <Switch checked={rec.systemAudio} onChange={(v) => set((d) => void (d.record.systemAudio = v))} />
              </Row>
              <Row title={t('settings.record.microphone')} desc={mediaDesc('microphone')}>
                <Switch checked={rec.microphone} onChange={(v) => void toggleMedia('microphone', v)} />
              </Row>
              {rec.systemAudio && rec.microphone && (
                <Row title={t('settings.record.separateTracks')} desc={t('settings.record.separateTracksDesc')}>
                  <Switch checked={rec.separateAudioTracks} onChange={(v) => set((d) => void (d.record.separateAudioTracks = v))} />
                </Row>
              )}
              <Row title={t('settings.record.camera')} desc={mediaDesc('camera')}>
                <Switch checked={rec.camera} onChange={(v) => void toggleMedia('camera', v)} />
              </Row>
              {rec.camera && (
                <>
                  <Row title={t('record.fx.cameraShape')} desc={t('settings.record.cameraPlace')}>
                    <Segmented
                      value={rec.cameraShape}
                      options={(['circle', 'square', 'rounded'] as const).map((v) => ({ value: v, label: t(`record.fx.shape.${v}`) }))}
                      onChange={(v) => set((d) => void (d.record.cameraShape = v))}
                    />
                  </Row>
                  <Row title={t('settings.record.cameraSize')}>
                    <span className="set-row__value cn-numeric">{rec.cameraPx}px</span>
                    <Slider value={rec.cameraPx} min={60} max={480} step={10} onChange={(v) => set((d) => void (d.record.cameraPx = v))} />
                  </Row>
                </>
              )}
            </Group>
          )}

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
                  ...(info.data?.ocr.paddle || settings.ocr.engine === 'paddle' ? [{ value: 'paddle' as const, label: 'PaddleOCR' }] : []),
                  { value: 'system', label: `${t('settings.ocr.system')}${info.data?.ocr.system === false ? ` (${t('settings.ocr.unavailable')})` : ''}` },
                ]}
                onChange={(v) => set((d) => void (d.ocr.engine = v))}
              />
            </Row>
            <Row title={t('settings.ocr.autoCopy')} desc={t('settings.ocr.autoCopyDesc')}>
              <Switch checked={settings.ocr.autoCopy} onChange={(v) => set((d) => void (d.ocr.autoCopy = v))} />
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
            {/* 可下载的 PaddleOCR 引擎只有 Windows 版 */}
            {!isMac && <PaddleEngineRow status={info.data?.ocr} />}
          </Group>

          <TranslateSettingsGroups settings={settings} set={set} />

          <AiSettingsGroups settings={settings} set={set} />

          <McpSettingsGroup settings={settings} set={set} />

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
            {cb.panelStyle === 'bottom' && (
              <Row title={t('settings.clipboard.docked')} desc={t('settings.clipboard.dockedDesc')}>
                <Switch checked={cb.panelDocked} onChange={(v) => set((d) => void (d.clipboard.panelDocked = v))} />
              </Row>
            )}
            {cb.panelStyle === 'bottom' && (
              <Row
                title={t(isMac ? 'settings.clipboard.overDock' : 'settings.clipboard.overTaskbar')}
                desc={t(isMac ? 'settings.clipboard.overDockDesc' : 'settings.clipboard.overTaskbarDesc')}
              >
                <Switch checked={cb.panelOverDock} onChange={(v) => set((d) => void (d.clipboard.panelOverDock = v))} />
              </Row>
            )}
            <Row title={t('settings.clipboard.animation')} desc={t('settings.clipboard.animationDesc')}>
              <Switch checked={cb.panelAnimation} onChange={(v) => set((d) => void (d.clipboard.panelAnimation = v))} />
            </Row>
            <Row title={t('settings.clipboard.blur')} desc={t('settings.clipboard.blurDesc')}>
              <Switch checked={cb.panelBlur} onChange={(v) => set((d) => void (d.clipboard.panelBlur = v))} />
            </Row>
            <Row title={t('settings.clipboard.orbColor')} desc={t('settings.clipboard.orbColorDesc')}>
              <div className="orb-picker" role="radiogroup">
                {ORB_COLORS.map((c) => (
                  <button
                    key={c}
                    type="button"
                    role="radio"
                    aria-checked={(cb.orbColor || 'ocean') === c}
                    className="orb-picker__item"
                    title={t(`settings.clipboard.orb.${c}`)}
                    onClick={() => set((d) => void (d.clipboard.orbColor = c))}
                  >
                    <WaterOrb color={c} size={22} interactive />
                  </button>
                ))}
              </div>
            </Row>
            <Row title={t('settings.clipboard.smartAi')} desc={t('settings.clipboard.smartAiDesc')}>
              <Switch checked={cb.smartAi} onChange={(v) => set((d) => void (d.clipboard.smartAi = v))} />
            </Row>
            {cb.smartAi && (
              <Row title={t('settings.clipboard.smartModel')} desc={t('settings.clipboard.smartModelDesc')}>
                <Select
                  value={smartModels.some((m) => m.value === cb.smartModel) ? cb.smartModel : ''}
                  width={220}
                  options={[{ value: '', label: t('settings.clipboard.smartModelDefault') }, ...smartModels]}
                  onChange={(v) => set((d) => void (d.clipboard.smartModel = v))}
                />
              </Row>
            )}
            {isMac && (
              <Row title={t('settings.clipboard.smsCodes')} desc={t('settings.clipboard.smsCodesDesc')}>
                <Switch checked={cb.smsCodes} onChange={(v) => set((d) => void (d.clipboard.smsCodes = v))} />
              </Row>
            )}
            {isMac && cb.smsCodes && perms.data?.fullDiskAccess === false && (
              <Row title={t('settings.permissions.fullDisk')} desc={t('settings.permissions.fullDiskDesc')}>
                <Button size="sm" onClick={() => void system.requestPermission('fullDiskAccess').catch(notify.error)}>
                  {t('settings.permissions.grant')}
                </Button>
              </Row>
            )}
            <MailCodesRows settings={settings} set={set} />
            <Row title={t('settings.clipboard.otpMask')} desc={t('settings.clipboard.otpMaskDesc')}>
              <Switch checked={cb.otpMask} onChange={(v) => set((d) => void (d.clipboard.otpMask = v))} />
            </Row>
            <Row title={t('settings.clipboard.otpExpire')} desc={t('settings.clipboard.otpExpireDesc')}>
              <>
                {otpExpireMode === 'custom' && (
                  <>
                    <TextField
                      key={cb.otpExpireSecs}
                      type="number"
                      min={5}
                      max={86400}
                      defaultValue={cb.otpExpireSecs || 120}
                      style={{ width: 84 }}
                      onBlur={(e) => {
                        const secs = Math.round(Number(e.target.value));
                        if (secs >= 5 && secs <= 86400) set((d) => void (d.clipboard.otpExpireSecs = secs));
                      }}
                    />
                    <span className="set-row__value">{t('settings.clipboard.otpSeconds')}</span>
                  </>
                )}
                <Select
                  value={otpExpireMode}
                  width={120}
                  options={[...OTP_EXPIRE_PRESETS, 'custom'].map((v) => ({ value: v, label: t(`settings.clipboard.otpExpireOpt.${v}`) }))}
                  onChange={(v) => {
                    setOtpCustom(v === 'custom');
                    if (v !== 'custom') set((d) => void (d.clipboard.otpExpireSecs = Number(v)));
                  }}
                />
              </>
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
            {(['capture', 'instant', 'longshot', 'ocr', 'clipboard', 'translate', 'inputTranslate', 'ai', 'pinClipboard'] as HotkeyAction[]).map((a) => {
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
                  // 默认热键每个平台一套，由 Rust 那边给
                  void system
                    .defaultHotkeys()
                    .then((defaults) => set((d) => void (d.hotkeys = { ...d.hotkeys, ...defaults, ai: d.hotkeys.ai, pinClipboard: d.hotkeys.pinClipboard, inputTranslate: d.hotkeys.inputTranslate })))
                    .catch(notify.error)
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

          <Group id="update" title={t('settings.section.update')}>
            <Row
              title={t('update.current', { version: upd?.current ?? info.data?.version ?? '' })}
              desc={
                upd?.error ? (
                  <span className="set-row__error">{upd.error}</span>
                ) : upd?.available ? (
                  t('update.found', { version: upd.available.version })
                ) : upd?.lastCheck ? (
                  t('update.latest', { time: relativeTime(upd.lastCheck) })
                ) : (
                  t('update.never')
                )
              }
            >
              {upd?.available && (
                <Button size="sm" variant="primary" onClick={() => setUpdOpen(true)}>
                  {t('update.view')}
                </Button>
              )}
              <Button
                size="sm"
                loading={upd?.checking}
                onClick={() =>
                  void updater
                    .check()
                    .then((s) => {
                      if (s.available) setUpdOpen(true);
                      else if (!s.error) notify.success(t('update.upToDate'));
                    })
                    .catch(notify.error)
                }
              >
                {t('update.check')}
              </Button>
            </Row>
            <Row title={t('update.auto')} desc={t('update.autoDesc')}>
              <Switch checked={settings.update.autoCheck} onChange={(v) => set((d) => void (d.update.autoCheck = v))} />
            </Row>
            <Row title={t('update.channel')} desc={t('update.channelDesc')}>
              <Select
                value={settings.update.channel}
                options={[
                  { value: 'cn', label: t('update.channelCn') },
                  { value: 'global', label: t('update.channelGlobal') },
                ]}
                onChange={(v) => set((d) => void (d.update.channel = v))}
              />
            </Row>
            <Row title={t('update.github')} desc={t('update.githubDesc')}>
              <Button size="sm" onClick={() => void system.openUrl('https://github.com/Chenfugui69/DATO-OCR').catch(notify.error)}>
                <ExternalLink size={14} strokeWidth={1.75} />
                {t('update.githubOpen')}
              </Button>
            </Row>
          </Group>

          <Group id="about" title={t('settings.section.about')} note={t('settings.about.privacy')}>
            <div className="about">
              <span className="about__logo">
                <BrandMark size={38} />
              </span>
              <div>
                <div style={{ font: 'var(--cn-text-title-2)' }}>DATO OCR</div>
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
        </SettingsSectionContext.Provider>
      </div>
      {upd && <UpdateDialog status={upd} open={updOpen} onClose={() => setUpdOpen(false)} />}
    </section>
  );
}

/** PaddleOCR：下载（带进度）/ 已安装可删除 / 处理器不支持 */
function PaddleEngineRow({ status }: { status?: EngineStatus }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [dl, setDl] = useState<PaddleDownload | null>(null);
  useEvent('ocr-engine-download', (p) => {
    setDl(p);
    if (!p.stage) void qc.invalidateQueries({ queryKey: ['app-info'] });
  });
  const cur = dl ?? status?.paddleDownload;
  const busy = !!cur?.stage;
  let desc = t('settings.ocr.paddleDesc');
  if (status && !status.avx) desc = t('settings.ocr.paddleNoAvx');
  else if (cur?.stage === 'downloading') desc = t('settings.ocr.paddleDownloading', { percent: Math.round((cur.progress ?? 0) * 100) });
  else if (cur?.stage === 'extracting') desc = t('settings.ocr.paddleExtracting');
  else if (cur?.error) desc = t('settings.ocr.paddleFailed', { error: cur.error });
  else if (status?.paddle) desc = t('settings.ocr.paddleInstalled');
  return (
    <Row title={t('settings.ocr.paddle')} desc={desc}>
      {status?.paddle && !busy ? (
        <Button
          size="sm"
          variant="ghost"
          onClick={() =>
            void ocr
              .paddleRemove()
              .then(() => qc.invalidateQueries({ queryKey: ['app-info'] }))
              .catch(notify.error)
          }
        >
          {t('settings.ocr.remove')}
        </Button>
      ) : (
        <Button size="sm" loading={busy} disabled={!status?.avx || busy} onClick={() => void ocr.paddleInstall().catch(() => undefined)}>
          {t('settings.ocr.download')}
        </Button>
      )}
    </Row>
  );
}

/** 按搜索词藏掉对不上的行和分组（直接改 DOM 上的标记，不动各个设置项自己的渲染）。
 *  返回有命中的分类；没有搜索词时全部恢复、返回 null */
function filterSettings(root: HTMLElement | null, query: string): Set<string> | null {
  if (!root) return null;
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const hits = new Set<string>();
  for (const group of root.querySelectorAll<HTMLElement>('.set-group')) {
    const head = (group.dataset.search ?? '').toLowerCase();
    const headHit = words.length > 0 && words.every((w) => head.includes(w));
    let any = false;
    const card = group.querySelector('.set-card');
    for (const item of card ? [...card.children] : []) {
      const el = item as HTMLElement;
      const label = (el.querySelector('.set-row__label') ?? el).textContent?.toLowerCase() ?? '';
      const ok = !words.length || headHit || words.every((w) => head.includes(w) || label.includes(w));
      el.classList.toggle('set-miss', !ok);
      any ||= ok;
    }
    group.classList.toggle('set-miss', words.length > 0 && !any);
    if (words.length && any && group.dataset.section) hits.add(group.dataset.section);
  }
  return words.length ? hits : null;
}

function sameSet(a: Set<string> | null, b: Set<string> | null) {
  if (!a || !b) return a === b;
  return a.size === b.size && [...a].every((x) => b.has(x));
}
