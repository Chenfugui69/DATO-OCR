// 截图界面里的「录制」：点了工具条上的「录制」不马上录，先进录制准备 ——
//
// - 主工具条换成录制的：开始 / 停止、计时、麦克风、系统声音、打开录制文件夹、取消
// - 下面一条录制栏：格式、鼠标轨迹颜色、点击颜色、鼠标设置（高亮 / 按键 / 滚动）、按键显示、摄像头、
//   进度条 / 播放时间、倒计时、效果设置、录制设置。都直接存进设置，下次进来还是这样
// - 选区右下角外面一个移动把手，拖它挪选区（录制中也能拖，录制区域跟着挪）；准备时把手左边提示格式、麦克风、声音
// - 开了摄像头：选区左下角一个摄像头画面框，拖到哪录出来就在哪
//
// 录制中（phase = gif）：选区外一圈红框、同一条录制工具条（停止、计时、取消），开了按键显示的话
// 选区左下角显示按下的键（遮罩这时能被抓进录像，所以选区里只画按键显示，别的都在选区外面）。
// 遮罩是鼠标穿透的，工具条、把手的屏幕区域报给 Rust，鼠标移上去临时关掉穿透。

import clsx from 'clsx';
import {
  Camera,
  FolderOpen,
  Gauge,
  Keyboard,
  Mic,
  MicOff,
  MousePointer2,
  MousePointerClick,
  Move,
  Settings2,
  SlidersHorizontal,
  Square,
  Timer,
  Video,
  Volume2,
  VolumeX,
  WandSparkles,
  X,
} from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import i18n from '@/i18n';
import { useEvent } from '@/lib/events';
import { useElementSize } from '@/lib/hooks';
import { gif, record, system } from '@/lib/ipc';
import { isMac } from '@/lib/platform';
import { useSettings, useSettingsStore } from '@/lib/settings';
import type { RecordFormat, RecordSettings, Settings } from '@/lib/types';
import { Select, Slider, Switch } from '@/ui/controls';
import { notify, Tooltip } from '@/ui/overlays';

import { placeToolbar, type Rect } from './geometry';
import { get, set, useOverlay } from './store';

type Viewport = { width: number; height: number };
type CssRect = { x: number; y: number; width: number; height: number };

const css = (r: Rect, s: number): CssRect => ({ x: r.x / s, y: r.y / s, width: r.width / s, height: r.height / s });
const stop = (e: React.SyntheticEvent) => e.stopPropagation();
/** 鼠标轨迹 / 点击的颜色可选这几种（空 = 不画） */
const SWATCHES = ['', '#ff3b30', '#34c759', '#0a84ff', '#ffd60a'];
const COUNTDOWNS = [0, 3, 5, 10];
/** 把手中心在选区右下角外面多远（别和调大小的角柄叠在一起） */
const HANDLE_OUT = 18;

const hms = (ms: number) => {
  const s = Math.floor(ms / 1000);
  return [Math.floor(s / 3600), Math.floor(s / 60) % 60, s % 60].map((v) => String(v).padStart(2, '0')).join(':');
};

const saveRecord = (mutate: (r: RecordSettings, all: Settings) => void) =>
  void useSettingsStore
    .getState()
    .update((d) => mutate(d.record, d))
    .catch(notify.error);

/** 动图（GIF / APNG）没有声音 */
const silent = (f: RecordFormat) => f !== 'mp4';

// ───────────────────────── 工具条 ─────────────────────────

export function RecordToolbar({
  rect,
  s,
  viewport,
  live,
  onStart,
  onExit,
}: {
  rect: Rect;
  s: number;
  viewport: Viewport;
  /** 正在录 */
  live: boolean;
  onStart: () => void;
  onExit: () => void;
}) {
  const { t } = useTranslation();
  const settings = useSettings();
  const rec = settings?.record;
  const progress = useOverlay((x) => x.gif);
  const countdown = useOverlay((x) => x.countdown);
  const ref = useRef<HTMLDivElement>(null);
  const size = useElementSize(ref, { width: 360, height: 44 });
  const sel = css(rect, s);
  const pos = placeToolbar(sel, size, viewport);
  const format = rec?.format ?? 'mp4';
  const mute = silent(format);
  const elapsed = live ? (progress?.elapsedMs ?? 0) : 0;
  const max = progress?.maxMs ?? 0;
  const autoHide = live && !!rec?.autoHideToolbar;

  // 录制中：工具条的屏幕区域报给 Rust（鼠标移上去才点得到）
  useReportRegion(ref, live, s, [pos.x, pos.y, size.width, size.height]);

  return (
    <>
      <div
        ref={ref}
        className={clsx('an-toolbar cn-glass rec-toolbar', pos.inside && 'cap-toolbar--inside', autoHide && 'rec-toolbar--autohide')}
        style={{ transform: `translate(${pos.x}px, ${pos.y}px)` }}
        onPointerDown={stop}
      >
        {live ? (
          <Tooltip content={t('record.stop')} side="top">
            <button type="button" className="an-btn rec-btn--stop" aria-label={t('record.stop')} onClick={() => void gif.finish()}>
              <Square size={14} strokeWidth={0} fill="currentColor" />
            </button>
          </Tooltip>
        ) : (
          <Tooltip content={countdown != null ? t('record.cancelCountdown') : t('record.start')} shortcut="Enter" side="top">
            <button type="button" className="an-btn rec-btn--start" aria-label={t('record.start')} onClick={countdown != null ? onExit : onStart}>
              <span className="rec-dot" />
            </button>
          </Tooltip>
        )}
        <span className={clsx('rec-time cn-numeric', live && 'rec-time--live', live && max - elapsed < 10_000 && 'is-warn')}>
          {hms(elapsed)}
        </span>
        <span className="an-sep" />
        <Tooltip content={mute ? t('record.noAudioForAnim') : rec?.microphone ? t('record.micOn') : t('record.micOff')} side="top">
          <button
            type="button"
            className={clsx('an-btn', rec?.microphone && !mute && 'an-btn--active')}
            disabled={mute || live}
            aria-label={t('settings.record.microphone')}
            onClick={() => void toggleMedia('microphone', !rec?.microphone)}
          >
            {rec?.microphone && !mute ? <Mic size={18} strokeWidth={1.5} /> : <MicOff size={18} strokeWidth={1.5} />}
          </button>
        </Tooltip>
        <Tooltip content={mute ? t('record.noAudioForAnim') : rec?.systemAudio ? t('record.soundOn') : t('record.soundOff')} side="top">
          <button
            type="button"
            className={clsx('an-btn', rec?.systemAudio && !mute && 'an-btn--active')}
            disabled={mute || live || !isMac}
            aria-label={t('settings.record.systemAudio')}
            onClick={() => saveRecord((r) => void (r.systemAudio = !r.systemAudio))}
          >
            {rec?.systemAudio && !mute ? <Volume2 size={18} strokeWidth={1.5} /> : <VolumeX size={18} strokeWidth={1.5} />}
          </button>
        </Tooltip>
        <Tooltip content={t('record.openFolder')} side="top">
          <button type="button" className="an-btn" aria-label={t('record.openFolder')} onClick={() => void system.openFolder('save').catch(notify.error)}>
            <FolderOpen size={18} strokeWidth={1.5} />
          </button>
        </Tooltip>
        <span className="an-sep" />
        <Tooltip content={live ? t('record.discard') : t('record.back')} shortcut="Esc" side="top">
          <button type="button" className="an-btn an-btn--danger" aria-label={live ? t('record.discard') : t('record.back')} onClick={live ? () => void gif.cancel() : onExit}>
            <X size={18} strokeWidth={1.5} />
          </button>
        </Tooltip>
      </div>
      {!live && <RecordBar toolbar={{ x: pos.x, y: pos.y, height: size.height }} above={!pos.inside && pos.y < sel.y} viewport={viewport} />}
    </>
  );
}

/** 开 / 关麦克风、摄像头：第一次开先问系统要权限 */
async function toggleMedia(kind: 'microphone' | 'camera', on: boolean) {
  if (on) {
    const access = await record.mediaAccess(kind).catch(() => 'unsupported' as const);
    const granted = access === 'granted' || (access === 'notDetermined' && (await record.mediaRequest(kind).catch(() => 'denied' as const)) === 'granted');
    if (!granted) {
      notify.error(new Error(i18n.t('settings.record.denied', { name: i18n.t(`settings.record.${kind}`) })));
      return;
    }
  }
  saveRecord((r) => void (kind === 'microphone' ? (r.microphone = on) : (r.camera = on)));
}


/** 录制中把这块的屏幕区域报给 Rust（和把手一起） */
const regions = new Map<string, { x: number; y: number; width: number; height: number }>();
function report(s: number) {
  void gif
    .setRegions([...regions.values()].map((r) => ({ x: Math.floor(r.x * s), y: Math.floor(r.y * s), width: Math.ceil(r.width * s), height: Math.ceil(r.height * s) })))
    .catch(() => {});
}
function useReportRegion(ref: React.RefObject<HTMLElement | null>, live: boolean, s: number, deps: unknown[]) {
  const key = useRef(`r${Math.random().toString(36).slice(2)}`);
  useLayoutEffect(() => {
    const id = key.current;
    if (!live || !ref.current) {
      if (regions.delete(id)) report(s);
      return;
    }
    const r = ref.current.getBoundingClientRect();
    regions.set(id, { x: r.left, y: r.top, width: r.width, height: r.height });
    report(s);
    return () => {
      regions.delete(id);
      report(s);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [live, s, ...deps]);
}

// ───────────────────────── 录制栏 ─────────────────────────

function Swatches({ value, onChange, label, icon: I }: { value: string; onChange: (v: string) => void; label: string; icon: typeof Move }) {
  const { t } = useTranslation();
  return (
    <div className="rec-swatches" role="radiogroup" aria-label={label}>
      <Tooltip content={label} side="top">
        <span className="rec-swatches__icon">
          <I size={16} strokeWidth={1.5} />
        </span>
      </Tooltip>
      {SWATCHES.map((c) => (
        <button
          key={c || 'none'}
          type="button"
          role="radio"
          aria-checked={value === c}
          className={clsx('rec-swatch', !c && 'rec-swatch--none', value === c && 'rec-swatch--on')}
          style={c ? { background: c } : undefined}
          title={c ? c : t('record.none')}
          onClick={() => onChange(c)}
        />
      ))}
    </div>
  );
}

function RecordBar({ toolbar, above, viewport }: { toolbar: { x: number; y: number; height: number }; above: boolean; viewport: Viewport }) {
  const { t } = useTranslation();
  const settings = useSettings();
  const panel = useOverlay((x) => x.recordPanel);
  const ref = useRef<HTMLDivElement>(null);
  const size = useElementSize(ref, { width: 560, height: 40 });
  if (!settings) return null;
  const rec = settings.record;
  const x = Math.max(8, Math.min(toolbar.x, viewport.width - size.width - 8));
  let y = above ? toolbar.y - 6 - size.height : toolbar.y + toolbar.height + 6;
  if (y + size.height > viewport.height - 4 || y < 4) y = above ? toolbar.y + toolbar.height + 6 : toolbar.y - 6 - size.height;
  const open = (p: typeof panel) => set({ recordPanel: get().recordPanel === p ? null : p });
  // 弹层往远离工具条的那边开
  const popUp = above || y + size.height + 260 > viewport.height;
  const formats: { value: RecordFormat | 'webp'; label: string; disabled?: boolean }[] = [
    ...(isMac ? [{ value: 'mp4' as const, label: 'MP4' }] : []),
    { value: 'gif', label: 'GIF' },
    { value: 'apng', label: 'APNG' },
    { value: 'webp', label: 'WebP', disabled: true },
  ];
  return (
    <div ref={ref} className={clsx('an-toolbar cn-glass rec-bar', popUp && 'rec-bar--up')} style={{ transform: `translate(${x}px, ${y}px)` }} onPointerDown={stop} onWheel={stop}>
      <div className="rec-formats" role="radiogroup" aria-label={t('record.format')}>
        {formats.map((f) => (
          <Tooltip key={f.value} content={f.disabled ? t('record.webpPending') : t(`record.formatTip.${f.value}`)} side="top">
            <button
              type="button"
              role="radio"
              aria-checked={rec.format === f.value}
              disabled={f.disabled}
              className={clsx('rec-format', rec.format === f.value && 'rec-format--on')}
              onClick={() => !f.disabled && saveRecord((r) => void (r.format = f.value as RecordFormat))}
            >
              {f.label}
            </button>
          </Tooltip>
        ))}
      </div>
      <span className="an-sep" />
      <Swatches icon={WandSparkles} label={t('record.trail')} value={rec.trailColor} onChange={(v) => saveRecord((r) => void (r.trailColor = v))} />
      <span className="an-sep" />
      <Swatches icon={MousePointerClick} label={t('record.click')} value={rec.clickColor} onChange={(v) => saveRecord((r) => void (r.clickColor = v))} />
      <span className="an-sep" />
      <BarButton icon={MousePointer2} label={t('record.mouse')} active={rec.mouseHighlight || rec.mouseButtons || rec.mouseScroll} open={panel === 'mouse'} onClick={() => open('mouse')}>
        {panel === 'mouse' && (
          <div className="rec-pop" onPointerDown={stop}>
            <PopSwitch label={t('record.highlight')} checked={rec.mouseHighlight} onChange={(v) => saveRecord((r) => void (r.mouseHighlight = v))} />
            <PopSwitch label={t('record.buttons')} checked={rec.mouseButtons} onChange={(v) => saveRecord((r) => void (r.mouseButtons = v))} />
            <PopSwitch label={t('record.scroll')} checked={rec.mouseScroll} onChange={(v) => saveRecord((r) => void (r.mouseScroll = v))} />
          </div>
        )}
      </BarButton>
      <BarButton icon={Keyboard} label={t('record.keys')} active={rec.keystrokes} onClick={() => saveRecord((r) => void (r.keystrokes = !r.keystrokes))} />
      {isMac && <BarButton icon={Camera} label={t('record.camera')} active={rec.camera} onClick={() => void toggleMedia('camera', !rec.camera)} />}
      <BarButton
        icon={Gauge}
        label={silent(rec.format) ? t('record.progress') : t('record.progressAnimOnly')}
        active={rec.progress !== 'off'}
        open={panel === 'progress'}
        onClick={() => open('progress')}
      >
        {panel === 'progress' && (
          <div className="rec-pop" onPointerDown={stop}>
            {(['off', 'bar', 'time'] as const).map((v) => (
              <label key={v} className="rec-pop__radio">
                <input type="radio" checked={rec.progress === v} onChange={() => saveRecord((r) => void (r.progress = v))} />
                {t(`record.progressOpt.${v}`)}
              </label>
            ))}
            <p className="rec-pop__hint">{t('record.progressHint')}</p>
          </div>
        )}
      </BarButton>
      <BarButton icon={Timer} label={t('record.countdown')} active={rec.countdown > 0} open={panel === 'countdown'} onClick={() => open('countdown')} text={`${rec.countdown}s`}>
        {panel === 'countdown' && (
          <div className="rec-pop rec-pop--row" onPointerDown={stop}>
            {COUNTDOWNS.map((v) => (
              <button key={v} type="button" className={clsx('rec-format', rec.countdown === v && 'rec-format--on')} onClick={() => saveRecord((r) => void (r.countdown = v))}>
                {v === 0 ? t('record.now') : `${v}s`}
              </button>
            ))}
          </div>
        )}
      </BarButton>
      <span className="an-sep" />
      <BarButton icon={SlidersHorizontal} label={t('record.effects')} open={panel === 'effects'} onClick={() => open('effects')}>
        {panel === 'effects' && <EffectsPanel rec={rec} />}
      </BarButton>
      <BarButton icon={Settings2} label={t('record.settings')} open={panel === 'settings'} onClick={() => open('settings')}>
        {panel === 'settings' && <SettingsPanel settings={settings} />}
      </BarButton>
    </div>
  );
}

function BarButton({
  icon: I,
  label,
  active,
  open,
  text,
  onClick,
  children,
}: {
  icon: typeof Move;
  label: string;
  active?: boolean;
  open?: boolean;
  text?: string;
  onClick: () => void;
  children?: React.ReactNode;
}) {
  return (
    <span className="rec-bar__slot">
      <Tooltip content={label} side="top">
        <button type="button" className={clsx('an-btn', text && 'rec-btn--text', (active || open) && 'an-btn--active')} aria-label={label} aria-expanded={open} onClick={onClick}>
          <I size={17} strokeWidth={1.5} />
          {text && <span className="cn-numeric">{text}</span>}
        </button>
      </Tooltip>
      {children}
    </span>
  );
}

function PopSwitch({ label, checked, onChange }: { label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="rec-pop__row">
      <span>{label}</span>
      <Switch checked={checked} onChange={onChange} />
    </label>
  );
}

function NumberField({ label, value, unit, min, max, onChange }: { label: string; value: number; unit: string; min: number; max: number; onChange: (v: number) => void }) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  const commit = () => {
    const n = Number(draft);
    if (Number.isFinite(n)) onChange(Math.round(Math.min(max, Math.max(min, n))));
    else setDraft(String(value));
  };
  return (
    <label className="rec-field">
      <span className="rec-field__label">{label}</span>
      <span className="rec-field__box">
        <input
          className="cn-numeric"
          value={draft}
          inputMode="numeric"
          onChange={(e) => setDraft(e.currentTarget.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === 'Enter') commit();
          }}
        />
        <em>{unit}</em>
      </span>
    </label>
  );
}

function ColorField({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const commit = () => (/^#([0-9a-f]{6}|[0-9a-f]{8})$/i.test(draft.trim()) ? onChange(draft.trim()) : setDraft(value));
  return (
    <label className="rec-field">
      <span className="rec-field__label">{label}</span>
      <span className="rec-field__box">
        <i className="rec-field__swatch" style={{ background: value }} />
        <input
          className="cn-numeric"
          value={draft}
          spellCheck={false}
          onChange={(e) => setDraft(e.currentTarget.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === 'Enter') commit();
          }}
        />
      </span>
    </label>
  );
}

/** 效果设置：鼠标轨迹多久、按键显示多大、摄像头大小形状、各种颜色 */
function EffectsPanel({ rec }: { rec: RecordSettings }) {
  const { t } = useTranslation();
  const e = rec.effects;
  const fx = (mutate: (e: RecordSettings['effects']) => void) => saveRecord((r) => mutate(r.effects));
  return (
    <div className="rec-pop rec-pop--panel" onPointerDown={stop} onKeyDown={stop}>
      <div className="rec-pop__title">{t('record.effects')}</div>
      <div className="rec-grid">
        <NumberField label={t('record.fx.trailMs')} unit={t('record.ms')} value={e.trailMs} min={100} max={3000} onChange={(v) => fx((x) => void (x.trailMs = v))} />
        <NumberField label={t('record.fx.keyboardPx')} unit="px" value={e.keyboardPx} min={20} max={120} onChange={(v) => fx((x) => void (x.keyboardPx = v))} />
        <NumberField label={t('record.fx.cameraPx')} unit="px" value={rec.cameraPx} min={60} max={480} onChange={(v) => saveRecord((r) => void (r.cameraPx = v))} />
        <label className="rec-field">
          <span className="rec-field__label">{t('record.fx.cameraShape')}</span>
          <Select
            value={rec.cameraShape}
            options={(['circle', 'square', 'rounded'] as const).map((v) => ({ value: v, label: t(`record.fx.shape.${v}`) }))}
            onChange={(v) => saveRecord((r) => void (r.cameraShape = v))}
          />
        </label>
        <ColorField label={t('record.fx.keyboardBg')} value={e.keyboardBg} onChange={(v) => fx((x) => void (x.keyboardBg = v))} />
        <ColorField label={t('record.fx.keyboardFg')} value={e.keyboardFg} onChange={(v) => fx((x) => void (x.keyboardFg = v))} />
        <NumberField label={t('record.fx.highlightRadius')} unit="px" value={e.highlightRadius} min={10} max={120} onChange={(v) => fx((x) => void (x.highlightRadius = v))} />
        <ColorField label={t('record.fx.highlightColor')} value={e.highlightColor} onChange={(v) => fx((x) => void (x.highlightColor = v))} />
        <ColorField label={t('record.fx.progressColor')} value={e.progressColor} onChange={(v) => fx((x) => void (x.progressColor = v))} />
        <div className="rec-field">
          <span className="rec-field__label">{t('record.fx.highlightPreview')}</span>
          <span className="rec-hl-preview">
            {['#ffffff', '#2c2c2e', '#4a90d9'].map((bg) => (
              <i key={bg} style={{ background: bg }}>
                <b style={{ background: e.highlightColor }} />
              </i>
            ))}
          </span>
        </div>
      </div>
    </div>
  );
}

/** 录制设置：清晰度、帧率、画质、编码器、音轨、动图的清晰度 / 帧率 / 循环，以及录制时的几个开关 */
function SettingsPanel({ settings }: { settings: Settings }) {
  const { t } = useTranslation();
  const rec = settings.record;
  return (
    <div className="rec-pop rec-pop--panel rec-pop--settings" onPointerDown={stop} onKeyDown={stop}>
      <div className="rec-pop__title">{t('record.settings')}</div>
      <div className="rec-grid">
        {isMac && (
          <>
            <label className="rec-field">
              <span className="rec-field__label">{t('settings.record.resolution')}</span>
              <Select
                value={rec.resolution}
                options={[
                  { value: 'original', label: t('settings.record.original') },
                  { value: '1080p', label: '1080p' },
                  { value: '720p', label: '720p' },
                ]}
                onChange={(v) => saveRecord((r) => void (r.resolution = v))}
              />
            </label>
            <label className="rec-field">
              <span className="rec-field__label">{t('settings.record.fps')}</span>
              <Select
                value={String(rec.fps)}
                options={['15', '24', '30', '60'].map((v) => ({ value: v, label: v }))}
                onChange={(v) => saveRecord((r) => void (r.fps = Number(v) as RecordSettings['fps']))}
              />
            </label>
            <label className="rec-field">
              <span className="rec-field__label">
                {t('settings.record.quality')} <em className="cn-numeric">{rec.quality}%</em>
              </span>
              <Slider value={rec.quality} min={10} max={100} step={5} onChange={(v) => saveRecord((r) => void (r.quality = v))} />
            </label>
            <label className="rec-field">
              <span className="rec-field__label">{t('settings.record.codec')}</span>
              <Select
                value={rec.codec}
                options={[
                  { value: 'h264', label: 'H.264' },
                  { value: 'hevc', label: 'H.265' },
                ]}
                onChange={(v) => saveRecord((r) => void (r.codec = v))}
              />
            </label>
          </>
        )}
        <label className="rec-field">
          <span className="rec-field__label">{t('settings.record.gifResolution')}</span>
          <Select
            value={rec.gifResolution}
            options={[
              { value: 'original', label: t('settings.record.original') },
              { value: '1080p', label: '1080p' },
              { value: '720p', label: '720p' },
              { value: '480p', label: '480p' },
            ]}
            onChange={(v) => saveRecord((r) => void (r.gifResolution = v))}
          />
        </label>
        <label className="rec-field">
          <span className="rec-field__label">{t('settings.capture.gifFps')}</span>
          <Select
            value={String(settings.capture.gifFps)}
            options={['5', '10', '15', '20', '30'].map((v) => ({ value: v, label: v }))}
            onChange={(v) => saveRecord((_, all) => void (all.capture.gifFps = Number(v)))}
          />
        </label>
        <label className="rec-field">
          <span className="rec-field__label">{t('settings.record.maxSeconds')}</span>
          <Select
            value={String(rec.maxSeconds)}
            options={[60, 180, 300, 600, 1800, 3600].map((v) => ({ value: String(v), label: t('settings.record.minutes', { count: v / 60 }) }))}
            onChange={(v) => saveRecord((r) => void (r.maxSeconds = Number(v)))}
          />
        </label>
      </div>
      <div className="rec-switches">
        <PopSwitch label={t('settings.record.gifLoop')} checked={rec.gifLoop} onChange={(v) => saveRecord((r) => void (r.gifLoop = v))} />
        {isMac && <PopSwitch label={t('settings.record.separateTracks')} checked={rec.separateAudioTracks} onChange={(v) => saveRecord((r) => void (r.separateAudioTracks = v))} />}
        <PopSwitch label={t('settings.record.cursor')} checked={settings.capture.gifCursor} onChange={(v) => saveRecord((_, all) => void (all.capture.gifCursor = v))} />
        <PopSwitch label={t('record.autoHide')} checked={rec.autoHideToolbar} onChange={(v) => saveRecord((r) => void (r.autoHideToolbar = v))} />
        <PopSwitch label={t('settings.record.editBeforeSave')} checked={rec.editBeforeSave} onChange={(v) => saveRecord((r) => void (r.editBeforeSave = v))} />
        <PopSwitch label={t('record.notifyDone')} checked={rec.notifyDone} onChange={(v) => saveRecord((r) => void (r.notifyDone = v))} />
      </div>
    </div>
  );
}

// ───────────────────────── 选区上的东西：移动把手、提示、摄像头框、按键显示、倒数 ─────────────────────────

/** 选区右下角外面的移动把手：拖它挪选区。录制中也能拖（录制区域跟着挪） */
export function MoveHandle({ rect, s, viewport, live }: { rect: Rect; s: number; viewport: Viewport; live: boolean }) {
  const { t } = useTranslation();
  const ref = useRef<HTMLButtonElement>(null);
  const sel = css(rect, s);
  const cx = Math.min(viewport.width - 16, sel.x + sel.width + HANDLE_OUT);
  const cy = Math.min(viewport.height - 16, sel.y + sel.height + HANDLE_OUT);
  const [dragging, setDragging] = useState(false);
  useReportRegion(ref, live, s, [cx, cy, dragging]);
  const down = (e: React.PointerEvent<HTMLButtonElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const start = { x: e.clientX, y: e.clientY };
    const from = get().selection;
    if (!from) return;
    const maxX = viewport.width * s - from.width;
    const maxY = viewport.height * s - from.height;
    setDragging(true);
    // 拖着的时候整块屏都要能收到鼠标（不然一离开把手遮罩又变成穿透，拖到一半就断了）
    if (live) {
      regions.set('drag', { x: 0, y: 0, width: viewport.width, height: viewport.height });
      report(s);
    }
    let last = 0;
    const move = (ev: PointerEvent) => {
      const next = {
        ...from,
        x: Math.round(Math.min(maxX, Math.max(0, from.x + (ev.clientX - start.x) * s))),
        y: Math.round(Math.min(maxY, Math.max(0, from.y + (ev.clientY - start.y) * s))),
      };
      set({ selection: next });
      // 录制中：告诉 Rust 挪录制区域（它每一拍只取最新的，发得勤一点也没关系）
      const now = performance.now();
      if (live && now - last > 30) {
        last = now;
        void gif.moveTo(next).catch(() => {});
      }
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      setDragging(false);
      const sel = get().selection;
      if (live && sel) void gif.moveTo(sel).catch(() => {});
      if (live) {
        regions.delete('drag');
        report(s);
      }
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };
  return (
    <Tooltip content={t('record.move')} side="top">
      <button
        ref={ref}
        type="button"
        className={clsx('rec-move cn-glass', dragging && 'rec-move--dragging')}
        style={{ transform: `translate(${cx - 14}px, ${cy - 14}px)` }}
        aria-label={t('record.move')}
        onPointerDown={down}
      >
        <Move size={15} strokeWidth={1.75} />
      </button>
    </Tooltip>
  );
}

/** 准备时选区右下角（把手左边）的提示：格式、麦克风、声音 */
export function RecordHints({ rect, s }: { rect: Rect; s: number }) {
  const { t } = useTranslation();
  const rec = useSettings()?.record;
  if (!rec) return null;
  const sel = css(rect, s);
  const mute = silent(rec.format);
  return (
    <div className="rec-hints" style={{ right: `calc(100% - ${sel.x + sel.width - 8}px)`, top: sel.y + sel.height - 34 }}>
      <span className="rec-hint">
        <Video size={12} strokeWidth={1.75} />
        {rec.format.toUpperCase()}
      </span>
      {!mute && (
        <>
          <span className={clsx('rec-hint', !rec.microphone && 'rec-hint--off')}>
            {rec.microphone ? <Mic size={12} strokeWidth={1.75} /> : <MicOff size={12} strokeWidth={1.75} />}
            {t('settings.record.microphone')}
          </span>
          <span className={clsx('rec-hint', !rec.systemAudio && 'rec-hint--off')}>
            {rec.systemAudio ? <Volume2 size={12} strokeWidth={1.75} /> : <VolumeX size={12} strokeWidth={1.75} />}
            {t('record.sound')}
          </span>
        </>
      )}
    </div>
  );
}

/** 开了摄像头：选区里摄像头画面的位置框（默认左下角），拖到哪录出来就在哪 */
export function CameraFrame({ rect, s }: { rect: Rect; s: number }) {
  const { t } = useTranslation();
  const rec = useSettings()?.record;
  const [drag, setDrag] = useState<[number, number] | null>(null);
  if (!rec?.camera) return null;
  const sel = css(rect, s);
  const size = Math.min(rec.cameraPx, sel.width - 16, sel.height - 16);
  if (size < 24) return null;
  const margin = Math.min(32, Math.max(8, size / 10));
  const at = drag ?? rec.cameraAt;
  const x = at ? at[0] * sel.width : margin;
  const y = at ? at[1] * sel.height : sel.height - margin - size;
  const radius = rec.cameraShape === 'circle' ? '50%' : rec.cameraShape === 'rounded' ? size * 0.18 : 4;
  const down = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const start = { x: e.clientX, y: e.clientY, ox: x, oy: y };
    let cur: [number, number] = [x / sel.width, y / sel.height];
    const move = (ev: PointerEvent) => {
      const nx = Math.min(sel.width - size, Math.max(0, start.ox + ev.clientX - start.x));
      const ny = Math.min(sel.height - size, Math.max(0, start.oy + ev.clientY - start.y));
      cur = [nx / sel.width, ny / sel.height];
      setDrag(cur);
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      saveRecord((r) => void (r.cameraAt = cur));
      setDrag(null);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };
  return (
    <div
      className="rec-camera"
      style={{ transform: `translate(${sel.x + x}px, ${sel.y + y}px)`, width: size, height: size, borderRadius: radius }}
      title={t('record.cameraDrag')}
      onPointerDown={down}
    >
      <Camera size={Math.max(14, size * 0.18)} strokeWidth={1.5} />
      <span>{t('record.cameraDrag')}</span>
    </div>
  );
}

/** 录制中按下的键：选区左下角一排键帽，按了 1.6 秒后淡出 */
export function KeyHud({ rect, s }: { rect: Rect; s: number }) {
  const rec = useSettings()?.record;
  const [keys, setKeys] = useState<{ id: number; label: string; at: number }[]>([]);
  const seq = useRef(0);
  useEvent('record-key', (label) => {
    const id = ++seq.current;
    setKeys((cur) => [...cur.slice(-5), { id, label, at: Date.now() }]);
    window.setTimeout(() => setKeys((cur) => cur.filter((k) => k.id !== id)), 1600);
  });
  if (!rec?.keystrokes || !keys.length) return null;
  const sel = css(rect, s);
  const h = rec.effects.keyboardPx;
  // 摄像头在左下角时，按键放到右下角
  const camLeft = rec.camera && (!rec.cameraAt || rec.cameraAt[0] < 0.5) && (!rec.cameraAt || rec.cameraAt[1] > 0.5);
  return (
    <div
      className={clsx('rec-keys', camLeft && 'rec-keys--right')}
      style={{
        left: camLeft ? undefined : sel.x + 16,
        right: camLeft ? `calc(100% - ${sel.x + sel.width - 16}px)` : undefined,
        top: sel.y + sel.height - 16 - h,
        height: h,
      }}
    >
      {keys.map((k) => (
        <span key={k.id} className="rec-key" style={{ background: rec.effects.keyboardBg, color: rec.effects.keyboardFg, fontSize: h * 0.42, height: h, borderRadius: h * 0.22 }}>
          {k.label}
        </span>
      ))}
    </div>
  );
}

/** 开始前的倒数：选区正中一个大数字 */
export function Countdown({ rect, s }: { rect: Rect; s: number }) {
  const n = useOverlay((x) => x.countdown);
  if (n == null) return null;
  const sel = css(rect, s);
  return (
    <div className="rec-countdown" style={{ left: sel.x, top: sel.y, width: sel.width, height: sel.height }}>
      <span key={n} className="cn-numeric">
        {n}
      </span>
    </div>
  );
}

/** 播放时间用的字形：页面画一排数字和符号，传给 Rust（它没有字体，录完画进动图时按灰度贴上） */
export function sendGlyphs() {
  const chars = '0123456789:/ ';
  const size = 64;
  const canvas = document.createElement('canvas');
  const ctx = canvas.getContext('2d');
  if (!ctx) return;
  const font = `600 ${size}px -apple-system, "SF Pro Text", "Segoe UI", sans-serif`;
  ctx.font = font;
  const widths = [...chars].map((c) => Math.ceil(ctx.measureText(c).width) + 2);
  canvas.width = widths.reduce((a, b) => a + b, 0);
  canvas.height = Math.ceil(size * 1.2);
  ctx.font = font;
  ctx.fillStyle = '#fff';
  ctx.textBaseline = 'middle';
  const xs: number[] = [];
  let x = 0;
  [...chars].forEach((c, i) => {
    xs.push(x);
    ctx.fillText(c, x + 1, canvas.height / 2);
    x += widths[i]!;
  });
  void gif.glyphs(canvas.toDataURL('image/png'), chars, xs, widths).catch(() => {});
}

