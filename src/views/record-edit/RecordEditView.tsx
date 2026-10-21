// 剪辑录制（设置 → 录制 → 录完先剪辑）：录完的 GIF / 视频先到这里，掐头去尾、改播放速度再保存。
//
// 预览：视频直接用 <video> 播临时文件；GIF 用录制时存下的缩略图（每 250ms 一张）按时间换。
// 时间轴：缩略图铺成一条，两头的把手拖动选段，点一下跳到那里；空格播放 / 暂停，I / O 把开头 / 结尾设在当前位置。
// 保存：后端导出（视频走系统的导出，声音保持音调；GIF 重新编码），导完自己关窗口、右下角提示。
// 直接关窗口 = 按原样保存；"不要了"才删掉。

import { convertFileSrc } from '@tauri-apps/api/core';
import { Pause, Play, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { record } from '@/lib/ipc';
import type { RecordEditInfo } from '@/lib/types';
import { Button, Segmented, Spinner } from '@/ui/controls';
import { confirmDialog, notify } from '@/ui/overlays';
import { TitleBar } from '@/ui/TitleBar';

import './record-edit.css';

const SPEEDS = ['0.5', '1', '1.5', '2', '4'] as const;
/** 选段最短多长 */
const MIN_SPAN = 200;

function fmt(ms: number) {
  const s = Math.max(0, ms) / 1000;
  const m = Math.floor(s / 60);
  return `${String(m).padStart(2, '0')}:${(s - m * 60).toFixed(1).padStart(4, '0')}`;
}

export default function RecordEditView() {
  const { t } = useTranslation();
  const [info, setInfo] = useState<RecordEditInfo | null>(null);
  const [range, setRange] = useState<[number, number]>([0, 0]);
  const [speed, setSpeed] = useState<(typeof SPEEDS)[number]>('1');
  const [at, setAt] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [saving, setSaving] = useState(false);
  const video = useRef<HTMLVideoElement>(null);
  const strip = useRef<HTMLDivElement>(null);

  const load = useCallback(() => {
    record
      .editInfo()
      .then((i) => {
        setInfo(i);
        setRange([0, i.durationMs]);
        setAt(0);
        setPlaying(false);
        setSaving(false);
      })
      .catch(notify.error);
  }, []);
  useEffect(load, [load]);
  useEvent('record-edit', load);

  const duration = info?.durationMs ?? 0;
  const [start, end] = range;
  const rate = Number(speed);

  // 播放：视频交给 <video>，GIF 自己按时间往前走；到选段末尾停下，回到开头
  useEffect(() => {
    if (!playing || !info) return;
    let raf = 0;
    let last = performance.now();
    const v = video.current;
    if (info.format === 'mp4' && v) {
      v.playbackRate = rate;
      void v.play().catch(() => setPlaying(false));
    }
    const tick = (now: number) => {
      let next: number;
      if (info.format === 'mp4' && v) next = v.currentTime * 1000;
      else next = at0.current + (now - last) * rate;
      last = now;
      at0.current = next;
      if (next >= end) {
        setPlaying(false);
        seek(start);
        return;
      }
      setAt(next);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      v?.pause();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [playing, info, rate, end, start]);

  // GIF 播放时用的"现在"（不随渲染重置）
  const at0 = useRef(0);
  const seek = (ms: number) => {
    const clamped = Math.min(Math.max(ms, 0), duration);
    at0.current = clamped;
    setAt(clamped);
    if (video.current) video.current.currentTime = clamped / 1000;
  };

  const togglePlay = () => {
    if (!playing && (at < start || at >= end - 30)) seek(start);
    setPlaying((p) => !p);
  };
  const setIn = (ms: number) => setRange(([, e]) => [Math.min(Math.max(0, ms), e - MIN_SPAN), e]);
  const setOut = (ms: number) => setRange(([s]) => [s, Math.max(Math.min(duration, ms), s + MIN_SPAN)]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as Element).closest('input, textarea')) return;
      if (e.key === ' ') {
        e.preventDefault();
        togglePlay();
      } else if (e.key.toLowerCase() === 'i') setIn(at);
      else if (e.key.toLowerCase() === 'o') setOut(at);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  // 时间轴上按下：拖把手改选段，其他地方拖动 = 拖播放头
  const drag = (e: React.PointerEvent, what: 'in' | 'out' | 'head') => {
    const el = strip.current;
    if (!el || !duration) return;
    e.preventDefault();
    e.stopPropagation();
    setPlaying(false);
    const box = el.getBoundingClientRect();
    const msAt = (x: number) => ((x - box.left) / box.width) * duration;
    const apply = (x: number) => {
      const ms = msAt(x);
      if (what === 'in') {
        setIn(ms);
        seek(Math.min(Math.max(0, ms), end - MIN_SPAN));
      } else if (what === 'out') {
        setOut(ms);
        seek(Math.max(Math.min(duration, ms), start + MIN_SPAN));
      } else seek(ms);
    };
    apply(e.clientX);
    const move = (ev: PointerEvent) => apply(ev.clientX);
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  // GIF 预览：时间上不晚于播放头的最后一张缩略图
  const frame = useMemo(() => {
    if (!info?.thumbs.length) return null;
    let pick = info.thumbs[0]!;
    for (const th of info.thumbs) {
      if (th.ms > at) break;
      pick = th;
    }
    return pick;
  }, [info, at]);

  // 时间轴上铺的缩略图：平均取 12 张
  const tiles = useMemo(() => {
    if (!info?.thumbs.length) return [];
    const n = 12;
    return Array.from({ length: n }, (_, i) => {
      const target = (duration * (i + 0.5)) / n;
      let pick = info.thumbs[0]!;
      for (const th of info.thumbs) {
        if (th.ms > target) break;
        pick = th;
      }
      return pick.url;
    });
  }, [info, duration]);

  const save = () => {
    if (!info) return;
    setPlaying(false);
    setSaving(true);
    record.editSave(info.id, Math.round(start), Math.round(end), rate).catch((err) => {
      setSaving(false);
      notify.error(err);
    });
  };
  const discard = async () => {
    if (!info) return;
    const ok = await confirmDialog({ title: t('recordEdit.discardTitle'), body: t('recordEdit.discardBody'), confirmLabel: t('recordEdit.discard'), danger: true });
    if (ok) void record.editDiscard(info.id).catch(notify.error);
  };

  const pct = (ms: number) => `${duration ? (ms / duration) * 100 : 0}%`;
  const outLength = (end - start) / rate;

  return (
    <div className="re-root">
      <TitleBar title={info ? `${t('recordEdit.title')}  ·  ${info.width} × ${info.height}` : t('recordEdit.title')} />
      <div className="re-preview">
        {!info ? (
          <Spinner size={20} />
        ) : info.format === 'mp4' ? (
          <video ref={video} src={convertFileSrc(info.path)} preload="auto" playsInline onLoadedMetadata={() => seek(start)} />
        ) : (
          frame && <img src={frame.url} alt="" draggable={false} />
        )}
        {saving && (
          <div className="re-saving">
            <Spinner size={18} />
            {t('recordEdit.exporting')}
          </div>
        )}
      </div>
      <div className="re-timeline">
        <div ref={strip} className="re-strip" onPointerDown={(e) => drag(e, 'head')}>
          <div className="re-tiles">
            {tiles.map((url, i) => (
              <img key={i} src={url} alt="" draggable={false} />
            ))}
          </div>
          <div className="re-dim" style={{ left: 0, width: pct(start) }} />
          <div className="re-dim" style={{ left: pct(end), right: 0 }} />
          <div className="re-sel" style={{ left: pct(start), width: `calc(${pct(end)} - ${pct(start)})` }}>
            <span className="re-handle re-handle--in" onPointerDown={(e) => drag(e, 'in')} title={t('recordEdit.inTip')} />
            <span className="re-handle re-handle--out" onPointerDown={(e) => drag(e, 'out')} title={t('recordEdit.outTip')} />
          </div>
          <div className="re-head" style={{ left: pct(at) }} />
        </div>
      </div>
      <div className="re-bar">
        <Button size="sm" variant="ghost" icon={playing ? Pause : Play} onClick={togglePlay} disabled={!info || saving}>
          {playing ? t('recordEdit.pause') : t('recordEdit.play')}
        </Button>
        <span className="re-time cn-numeric">
          {fmt(at)} / {fmt(duration)}
        </span>
        <span className="re-span cn-numeric">{t('recordEdit.span', { start: fmt(start), end: fmt(end), length: fmt(outLength) })}</span>
        <span className="re-spacer" />
        <span className="re-label">{t('recordEdit.speed')}</span>
        <Segmented value={speed} options={SPEEDS.map((v) => ({ value: v, label: `${v}×` }))} onChange={setSpeed} />
        <Button size="sm" variant="ghost" icon={Trash2} onClick={() => void discard()} disabled={!info || saving}>
          {t('recordEdit.discard')}
        </Button>
        <Button size="sm" variant="primary" onClick={save} disabled={!info || saving}>
          {t('recordEdit.save')}
        </Button>
      </div>
    </div>
  );
}
