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
import { useSettings, useSettingsStore } from '@/lib/settings';
import type { RecordFormat, RecordSettings, Settings } from '@/lib/types';
import { Select, Slider, Switch } from '@/ui/controls';
import { notify, Tooltip } from '@/ui/overlays';

import type { AnnotationEngine } from '@/views/annotate/engine';

import { placeToolbar, type Rect } from './geometry';
import { DemoTools } from './RecordPreview';
import { get, glassAt, set, useOverlay } from './store';

type Viewport = { width: number; height: number };
type CssRect = { x: number; y: number; width: number; height: number };

const css = (r: Rect, s: number): CssRect => ({ x: r.x / s, y: r.y / s, width: r.width / s, height: r.height / s });
const stop = (e: React.SyntheticEvent) => e.stopPropagation();
/** 鼠标轨迹 / 点击的颜色可选这几种（空 = 不画） */
const SWATCHES = ['', '#ff3b30', '#34c759', '#0a84ff', '#ffd60a'];
const COUNTDOWNS = [0, 3, 5, 10];
/** 把手中心在选区右边外面多远（别和调大小的角柄叠在一起） */
const HANDLE_OUT = 22;

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
  engine,
  onStart,
  onCancelCountdown,
  onExit,
}: {
  rect: Rect;
  s: number;
  viewport: Viewport;
  /** 正在录 */
  live: boolean;
  /** 截图的标注：演示工具画在它上面 */
  engine: AnnotationEngine;
  onStart: () => void;
  /** 倒数时再点开始：不录了，录制框还在 */
  onCancelCountdown: () => void;
  /** 关掉（准备时：整个截图收掉，不退回截图；录制中：这段不要了） */
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

  // 工具条的屏幕区域报给 Rust：准备和录制时遮罩别处都是鼠标穿透的，移到这上面才点得到
  useReportRegion(ref, true, s, [pos.x, pos.y, size.width, size.height]);

  return (
    <>
      <div
        ref={ref}
        className={clsx('an-toolbar cn-glass rec-toolbar', pos.inside && 'cap-toolbar--inside', autoHide && 'rec-toolbar--autohide')}
        style={{ transform: `translate(${pos.x}px, ${pos.y}px)`, ...glassAt(pos.x, pos.y) }}
        onPointerDown={stop}
      >
        <DemoTools engine={engine} />
        {live ? (
          <Tooltip content={t('record.stop')} side="top">
            <button type="button" className="an-btn rec-btn--stop" aria-label={t('record.stop')} onClick={() => void gif.finish()}>
              <Square size={14} strokeWidth={0} fill="currentColor" />
            </button>
          </Tooltip>
        ) : (
          <Tooltip content={countdown != null ? t('record.cancelCountdown') : t('record.start')} shortcut="Enter" side="top">
            <button type="button" className="an-btn rec-btn--start" aria-label={t('record.start')} onClick={countdown != null ? onCancelCountdown : onStart}>
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
            disabled={mute || live}
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
      {!live && <RecordBar toolbar={{ x: pos.x, y: pos.y, width: size.width, height: size.height }} above={!pos.inside && pos.y < sel.y} viewport={viewport} s={s} />}
    </>
  );
}

/** 开 / 关麦克风、摄像头：第一次开先问系统要权限。返回改没改成 */
async function toggleMedia(kind: 'microphone' | 'camera', on: boolean): Promise<boolean> {
  if (on) {
    const access = await record.mediaAccess(kind).catch(() => 'unsupported' as const);
    const granted = access === 'granted' || (access === 'notDetermined' && (await record.mediaRequest(kind).catch(() => 'denied' as const)) === 'granted');
    if (!granted) {
      notify.error(new Error(i18n.t('settings.record.denied', { name: i18n.t(`settings.record.${kind}`) })));
      return false;
    }
  }
  saveRecord((r) => void (kind === 'microphone' ? (r.microphone = on) : (r.camera = on)));
  return true;
}

/**
 * 录制栏上的摄像头按钮：关着 → 打开，并且进入"摆位置"（摄像头框能拖，点别处结束）；
 * 开着、没在摆 → 再摆一次；正在摆 → 关掉摄像头
 */
function cameraButton(on: boolean) {
  const placing = get().cameraPlacing;
  if (!on) void toggleMedia('camera', true).then((ok) => ok && set({ cameraPlacing: true }));
  else if (!placing) set({ cameraPlacing: true });
  else {
    set({ cameraPlacing: false });
    void toggleMedia('camera', false);
  }
}


/** 录制中把这块的屏幕区域报给 Rust（和把手一起） */
const regions = new Map<string, { x: number; y: number; width: number; height: number }>();

/** 录制中报一块能点的区域（CSS 像素）；null = 撤掉 */
export function setRegion(id: string, r: { x: number; y: number; width: number; height: number } | null, s: number) {
  if (r) regions.set(id, r);
  else if (!regions.delete(id)) return;
  report(s);
}
function report(s: number) {
  void gif
    .setRegions([...regions.values()].map((r) => ({ x: Math.floor(r.x * s), y: Math.floor(r.y * s), width: Math.ceil(r.width * s), height: Math.ceil(r.height * s) })))
    .catch(() => {});
}
/** `withPops`：连同里面弹出来的面板（.rec-pop，摆在条外面）一起算 */
function useReportRegion(ref: React.RefObject<HTMLElement | null>, live: boolean, s: number, deps: unknown[], withPops = false) {
  const key = useRef(`r${Math.random().toString(36).slice(2)}`);
  useLayoutEffect(() => {
    const id = key.current;
    if (!live || !ref.current) {
      if (regions.delete(id)) report(s);
      return;
    }
    const boxes = [ref.current, ...(withPops ? Array.from(ref.current.querySelectorAll('.rec-pop')) : [])].map((el) => el.getBoundingClientRect());
    const x0 = Math.min(...boxes.map((b) => b.left));
    const y0 = Math.min(...boxes.map((b) => b.top));
    const x1 = Math.max(...boxes.map((b) => b.right));
    const y1 = Math.max(...boxes.map((b) => b.bottom));
    regions.set(id, { x: x0, y: y0, width: x1 - x0, height: y1 - y0 });
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

function RecordBar({ toolbar, above, viewport, s }: { toolbar: { x: number; y: number; width: number; height: number }; above: boolean; viewport: Viewport; s: number }) {
  const { t } = useTranslation();
  const settings = useSettings();
  const panel = useOverlay((x) => x.recordPanel);
  const placing = useOverlay((x) => x.cameraPlacing);
  const ref = useRef<HTMLDivElement>(null);
  const size = useElementSize(ref, { width: 560, height: 40 });
  // 录制栏和它弹出来的面板：准备时遮罩别处是穿透的，这些地方要能点
  useReportRegion(ref, true, s, [toolbar.x, toolbar.y, toolbar.width, toolbar.height, above, size.width, size.height, panel, !!settings], true);
  if (!settings) return null;
  const rec = settings.record;
  // 和上面的录制工具条居中对齐
  const x = Math.max(8, Math.min(toolbar.x + (toolbar.width - size.width) / 2, viewport.width - size.width - 8));
  let y = above ? toolbar.y - 6 - size.height : toolbar.y + toolbar.height + 6;
  if (y + size.height > viewport.height - 4 || y < 4) y = above ? toolbar.y + toolbar.height + 6 : toolbar.y - 6 - size.height;
  const open = (p: typeof panel) => set({ recordPanel: get().recordPanel === p ? null : p });
  // 弹层往远离工具条的那边开
  const popUp = above || y + size.height + 260 > viewport.height;
  const formats: { value: RecordFormat | 'webp'; label: string; disabled?: boolean }[] = [
    { value: 'mp4', label: 'MP4' },
    { value: 'gif', label: 'GIF' },
    { value: 'apng', label: 'APNG' },
    { value: 'webp', label: 'WebP', disabled: true },
  ];
  return (
    <div
      ref={ref}
      className={clsx('an-toolbar cn-glass rec-bar', popUp && 'rec-bar--up')}
      style={{ transform: `translate(${x}px, ${y}px)`, ...glassAt(x, y) }}
      onPointerDown={stop}
      onWheel={stop}
    >
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
      <BarButton
        icon={Camera}
        label={!rec.camera ? t('record.camera') : placing ? t('record.cameraOff') : t('record.cameraMove')}
        active={rec.camera}
        onClick={() => cameraButton(rec.camera)}
      />
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
        <PopSwitch label={t('settings.record.separateTracks')} checked={rec.separateAudioTracks} onChange={(v) => saveRecord((r) => void (r.separateAudioTracks = v))} />
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
/**
 * 录制准备：选区四条边（调大小的把手在上面）报成能点的区域，别处鼠标都穿过去。
 * 在遮罩上按下鼠标（拖边、拖把手、画演示标注）到松开之前整块屏都收鼠标，不然拖出这几块就断了
 */
export function PrepEdges({ rect, s, viewport }: { rect: Rect; s: number; viewport: Viewport }) {
  const sel = css(rect, s);
  useLayoutEffect(() => {
    const band = 10;
    const edges = {
      'edge-t': { x: sel.x - band, y: sel.y - band, width: sel.width + band * 2, height: band * 2 },
      'edge-b': { x: sel.x - band, y: sel.y + sel.height - band, width: sel.width + band * 2, height: band * 2 },
      'edge-l': { x: sel.x - band, y: sel.y - band, width: band * 2, height: sel.height + band * 2 },
      'edge-r': { x: sel.x + sel.width - band, y: sel.y - band, width: band * 2, height: sel.height + band * 2 },
    };
    for (const [id, r] of Object.entries(edges)) regions.set(id, r);
    report(s);
    return () => {
      for (const id of Object.keys(edges)) regions.delete(id);
      report(s);
    };
  }, [sel.x, sel.y, sel.width, sel.height, s]);
  useEffect(() => {
    const down = () => {
      regions.set('press', { x: 0, y: 0, width: viewport.width, height: viewport.height });
      report(s);
    };
    const up = () => {
      if (regions.delete('press')) report(s);
    };
    window.addEventListener('pointerdown', down, true);
    window.addEventListener('pointerup', up, true);
    window.addEventListener('pointercancel', up, true);
    return () => {
      window.removeEventListener('pointerdown', down, true);
      window.removeEventListener('pointerup', up, true);
      window.removeEventListener('pointercancel', up, true);
      up();
    };
  }, [s, viewport.width, viewport.height]);
  return null;
}

export function MoveHandle({ rect, s, viewport, live }: { rect: Rect; s: number; viewport: Viewport; live: boolean }) {
  const { t } = useTranslation();
  const ref = useRef<HTMLButtonElement>(null);
  const sel = css(rect, s);
  // 贴在选区右边、底边和选区的下框齐平：不往下伸出去，和下面的录制工具条、录制栏挤在一起
  const cx = Math.min(viewport.width - 16, sel.x + sel.width + HANDLE_OUT);
  const cy = Math.max(16, sel.y + sel.height - 14);
  const [dragging, setDragging] = useState(false);
  useReportRegion(ref, true, s, [cx, cy, dragging]);
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
    regions.set('drag', { x: 0, y: 0, width: viewport.width, height: viewport.height });
    report(s);
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
      regions.delete('drag');
      report(s);
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
        style={{ transform: `translate(${cx - 14}px, ${cy - 14}px)`, ...glassAt(cx - 14, cy - 14) }}
        aria-label={t('record.move')}
        onPointerDown={down}
      >
        <Move size={15} strokeWidth={1.75} />
      </button>
    </Tooltip>
  );
}

/** 准备时选区底部正中的提示：格式、麦克风、声音（点开始时先撤掉，不会录进去） */
export function RecordHints({ rect, s }: { rect: Rect; s: number }) {
  const { t } = useTranslation();
  const rec = useSettings()?.record;
  const tool = useOverlay((x) => x.tool);
  if (!rec) return null;
  const sel = css(rect, s);
  const mute = silent(rec.format);
  return (
    <div className="rec-hints" style={{ left: sel.x + sel.width / 2, top: sel.y + sel.height - 34 }}>
      {tool && <span className="rec-hint">{t('record.toolHint')}</span>}
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

/**
 * 开了摄像头：选区里摄像头画面的位置框（默认左下角），拖到哪录出来就在哪。
 * 平时遮罩在这里是穿透的（框底下的程序照常点）；刚打开摄像头、或者再点一下摄像头按钮时进入"摆位置"：
 * 框能拖，点屏幕上别的地方（不管点在哪个程序上）就结束、恢复穿透
 */
export function CameraFrame({ rect, s }: { rect: Rect; s: number }) {
  const { t } = useTranslation();
  const rec = useSettings()?.record;
  const placing = useOverlay((x) => x.cameraPlacing);
  const [drag, setDrag] = useState<[number, number] | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const on = !!rec?.camera;
  const sel = css(rect, s);
  const size = rec ? Math.min(rec.cameraPx, sel.width - 16, sel.height - 16) : 0;
  // 和 Rust（record_fx 的 draw_camera）一样：边距按物理像素 8 ~ 32，位置夹在选区里（选区缩小过、
  // 存下的位置在外面时也不跑出去）
  const margin = Math.min(32, Math.max(8, (size * s) / 10)) / s;
  const at = drag ?? rec?.cameraAt ?? null;
  const x = Math.min(Math.max(0, at ? at[0] * sel.width : margin), Math.max(0, sel.width - size));
  const y = Math.min(Math.max(0, at ? at[1] * sel.height : sel.height - margin - size), Math.max(0, sel.height - size));
  const shown = on && size >= 24;
  // 摆位置时这块要能点
  useLayoutEffect(() => {
    if (!shown || !placing) return;
    setRegion('camera', { x: sel.x + x, y: sel.y + y, width: size, height: size }, s);
    return () => setRegion('camera', null, s);
  }, [shown, placing, sel.x, sel.y, x, y, size, s]);
  // 点了别处（Rust 转过来的鼠标，点在别的程序上也收得到）：摆完了。点在录制条、录制栏上的不算
  const pressed = useRef(0);
  useEvent('record-prep-pointer', (p) => {
    const down = p.buttons & ~pressed.current;
    pressed.current = p.buttons;
    if (!down || !get().cameraPlacing || !p.at) return;
    const cx = p.at[0] / s;
    const cy = p.at[1] / s;
    const inside = (el: Element | null) => {
      const r = el?.getBoundingClientRect();
      return !!r && cx >= r.left && cx <= r.right && cy >= r.top && cy <= r.bottom;
    };
    if (inside(ref.current) || [...document.querySelectorAll('.rec-toolbar, .rec-bar, .rec-pop')].some(inside)) return;
    set({ cameraPlacing: false });
  });
  // 摄像头关了、离开准备：不在摆了
  useEffect(() => {
    if (!on) set({ cameraPlacing: false });
    return () => set({ cameraPlacing: false });
  }, [on]);
  if (!rec || !shown) return null;
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
      ref={ref}
      className={clsx('rec-camera', placing && 'rec-camera--placing')}
      style={{ transform: `translate(${sel.x + x}px, ${sel.y + y}px)`, width: size, height: size, borderRadius: radius }}
      title={t('record.cameraDrag')}
      onPointerDown={down}
    >
      <Camera size={Math.max(14, size * 0.18)} strokeWidth={1.5} />
      <span>{placing ? t('record.cameraDrag') : t('record.cameraHint')}</span>
    </div>
  );
}

/** 录制中按下的键：选区左下角一排键帽，按了 1.6 秒后淡出 */
export function KeyHud({ rect, s }: { rect: Rect; s: number }) {
  const rec = useSettings()?.record;
  const [keys, setKeys] = useState<{ id: number; label: string; at: number }[]>([]);
  const seq = useRef(0);
  const push = (label: string) => {
    const id = ++seq.current;
    setKeys((cur) => [...cur.slice(-5), { id, label, at: Date.now() }]);
    window.setTimeout(() => setKeys((cur) => cur.filter((k) => k.id !== id)), 1600);
  };
  // 按键由后端（全局监听）报过来：录的时候、准备的时候都是（准备时遮罩鼠标穿透，焦点多半在别的程序上，
  // 页面自己收不到按键）
  useEvent('record-key', push);
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

