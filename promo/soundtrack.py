#!/usr/bin/env python3
"""
DATO OCR 宣传片的配乐和音效，全部用 numpy 现场合成，不用任何采样。

    python3 soundtrack.py build/cues-zh.json build/soundtrack-zh.wav

cues JSON 由 render.mjs 从 film.js 导出：{ "duration": 78, "cues": [{ "t": 3.8, "type": "click", ... }] }
音乐本身（和弦、鼓、贝斯、琶音）按 120 BPM 写死在 SECTIONS 里，和画面的段落一一对应；
画面里每个动作（按键、点击、弹窗、扫描……）的音效照 cue 的时间点放。
"""
import json
import sys
import wave

import numpy as np

SR = 48000
BEAT = 0.5
rng = np.random.default_rng(20261016)


# ───────────────────────── 基础 ─────────────────────────
def mtof(m):
    return 440.0 * 2 ** ((m - 69) / 12)


def tt(dur):
    return np.arange(int(dur * SR)) / SR


def env_ad(n, a, d, curve=1.0):
    """attack 线性，decay 指数"""
    t = np.arange(n) / SR
    e = np.exp(-t / max(d, 1e-4) * curve)
    if a > 0:
        e *= np.clip(t / a, 0, 1)
    return e


def env_asr(n, a, r, total):
    t = np.arange(n) / SR
    e = np.clip(t / max(a, 1e-4), 0, 1)
    e *= np.clip((total - t) / max(r, 1e-4), 0, 1)
    return e


def noise(n):
    return rng.standard_normal(n)


def fft_filter(x, mask_fn):
    X = np.fft.rfft(x)
    f = np.fft.rfftfreq(len(x), 1 / SR)
    return np.fft.irfft(X * mask_fn(f), len(x))


def stft_filter(x, mask_fn, n=2048, hop=512):
    """随时间变化的滤波：mask_fn(帧中心时间[秒], 频率) -> 增益"""
    L = len(x)
    pad = n
    xp = np.concatenate([np.zeros(pad), x, np.zeros(pad + n)])
    nf = (len(xp) - n) // hop + 1
    idx = np.arange(n)[None, :] + hop * np.arange(nf)[:, None]
    win = np.hanning(n)
    X = np.fft.rfft(xp[idx] * win, axis=1)
    times = (hop * np.arange(nf) + n / 2 - pad) / SR
    freqs = np.fft.rfftfreq(n, 1 / SR)
    Y = X * mask_fn(times[:, None], freqs[None, :])
    frames = np.fft.irfft(Y, n, axis=1) * win
    out = np.zeros(len(xp))
    np.add.at(out, idx, frames)
    norm = np.zeros(len(xp))
    np.add.at(norm, idx, np.broadcast_to(win**2, frames.shape))
    out /= np.maximum(norm, 1e-3)
    return out[pad : pad + L]


def band(f, fc, width_oct):
    """对数频率上的高斯带通"""
    return np.exp(-0.5 * (np.log2(np.maximum(f, 1) / fc) / width_oct) ** 2)


def hp(f, fc, order=2):
    return 1 / np.sqrt(1 + (fc / np.maximum(f, 1)) ** (2 * order))


def lp(f, fc, order=2):
    return 1 / np.sqrt(1 + (np.maximum(f, 1) / fc) ** (2 * order))


# ───────────────────────── 混音台 ─────────────────────────
class Bus:
    def __init__(self, dur):
        self.n = int(dur * SR) + SR
        self.x = np.zeros((2, self.n))

    def add(self, sig, t0, gain=1.0, pan=0.0):
        """sig 可以是单声道（按 pan 摆位）或 (2, n) 立体声"""
        i = int(round(t0 * SR))
        if sig.ndim == 1:
            th = (pan + 1) * np.pi / 4
            sig = np.stack([sig * np.cos(th), sig * np.sin(th)]) * np.sqrt(2)
        if i < 0:
            sig = sig[:, -i:]
            i = 0
        m = min(sig.shape[1], self.n - i)
        if m > 0:
            self.x[:, i : i + m] += sig[:, :m] * gain


# ───────────────────────── 乐器 ─────────────────────────
def saw_stack(f, dur, cutoff, detune=(-9, 0, 9), seed=0):
    """带限锯齿波叠加（加法合成），返回立体声：低调的往左，高调的往右"""
    t = tt(dur)
    r = np.random.default_rng(seed)
    out = np.zeros((2, len(t)))
    for d in detune:
        ff = f * 2 ** (d / 1200)
        kmax = max(1, int(min(cutoff * 2.5, SR * 0.45) / ff))
        v = np.zeros(len(t))
        for k in range(1, kmax + 1):
            amp = (1 / k) / np.sqrt(1 + (k * ff / cutoff) ** 4)
            if amp < 0.004:
                break
            v += amp * np.sin(2 * np.pi * k * ff * t + r.uniform(0, 2 * np.pi))
        pan = 0 if d == 0 else (-0.7 if d < 0 else 0.7)
        th = (pan + 1) * np.pi / 4
        out[0] += v * np.cos(th)
        out[1] += v * np.sin(th)
    return out / len(detune)


def pad_chord(notes, dur, cutoff, attack=0.35, release=1.4, seed=0):
    total = dur + release
    out = np.zeros((2, int(total * SR)))
    for j, m in enumerate(notes):
        s = saw_stack(mtof(m), total, cutoff, seed=seed * 31 + j)
        out[:, : s.shape[1]] += s
    t = tt(total)
    e = np.clip(t / attack, 0, 1) * np.where(t < dur, 1, np.exp(-(t - dur) / (release / 4)))
    # 慢慢呼吸的音量，像真人拉的弦
    e *= 1 + 0.06 * np.sin(2 * np.pi * 0.25 * t + seed)
    return out * e / max(1, len(notes)) * 1.6


def fm_bell(f, dur=1.6, ratio=3.5, index=2.5, decay=0.9, bright=1.0):
    t = tt(dur)
    idx = index * bright * np.exp(-t / 0.18)
    s = np.sin(2 * np.pi * f * t + idx * np.sin(2 * np.pi * f * ratio * t))
    s += 0.35 * np.sin(2 * np.pi * f * 2 * t) * np.exp(-t / 0.3)
    return s * env_ad(len(t), 0.002, decay)


def pluck(f, dur=0.45, decay=0.2, bright=1.0):
    t = tt(dur)
    idx = 2.2 * bright * np.exp(-t / 0.06)
    s = np.sin(2 * np.pi * f * t + idx * np.sin(2 * np.pi * f * 2 * t))
    s += 0.25 * np.sin(2 * np.pi * f * 3 * t + 0.6) * np.exp(-t / 0.05)
    return s * env_ad(len(t), 0.003, decay)


def kick(level=1.0):
    t = tt(0.55)
    f = 46 + 110 * np.exp(-t / 0.035)
    ph = 2 * np.pi * np.cumsum(f) / SR
    body = np.sin(ph) * np.exp(-t / 0.22)
    click = fft_filter(noise(len(t)) * np.exp(-t / 0.004), lambda fr: band(fr, 3500, 1.2)) * 0.5
    return np.tanh((body + click) * 1.6) * 0.8 * level


def sub_boom(dur=2.2, f0=58, f1=32):
    t = tt(dur)
    f = f1 + (f0 - f1) * np.exp(-t / 0.5)
    return np.sin(2 * np.pi * np.cumsum(f) / SR) * env_ad(len(t), 0.005, 0.7)


def hat(open_=False, level=1.0):
    d = 0.25 if open_ else 0.04
    n = int((d * 5 + 0.02) * SR)
    s = fft_filter(noise(n), lambda f: hp(f, 7500, 3) * (0.6 + 0.4 * band(f, 11000, 1)))
    return s * env_ad(n, 0.0005, d) * level


def clap(level=1.0):
    n = int(0.5 * SR)
    t = np.arange(n) / SR
    e = np.zeros(n)
    for k, off in enumerate([0, 0.009, 0.019, 0.028]):
        e += np.where(t >= off, np.exp(-(t - off) / (0.005 if k < 3 else 0.11)), 0)
    s = fft_filter(noise(n), lambda f: band(f, 1500, 0.9) + 0.3 * band(f, 4000, 0.8))
    return s * e * 0.7 * level


def crash(dur=2.6):
    n = int(dur * SR)
    t = np.arange(n) / SR
    s = fft_filter(noise(n), lambda f: hp(f, 3500, 2))
    for _ in range(14):
        fr = rng.uniform(3000, 9000)
        s += 0.2 * np.sin(2 * np.pi * fr * t + rng.uniform(0, 6))
    return s * env_ad(n, 0.002, 0.75) * 0.6


def bass_note(f, dur):
    t = tt(dur)
    s = np.sin(2 * np.pi * f * t) + 0.35 * np.sin(4 * np.pi * f * t) + 0.12 * np.sin(6 * np.pi * f * t)
    e = np.clip(t / 0.006, 0, 1) * (0.75 + 0.25 * np.exp(-t / 0.08)) * np.clip((dur - t) / 0.03, 0, 1)
    return np.tanh(s * e * 1.3)


# ───────────────────────── 音效 ─────────────────────────
def sfx_click(up=False):
    n = int(0.06 * SR)
    t = np.arange(n) / SR
    f = 2600 if up else 1900
    s = np.sin(2 * np.pi * f * t) * np.exp(-t / 0.006)
    s += fft_filter(noise(n), lambda fr: band(fr, 5000, 1)) * np.exp(-t / 0.002) * 0.6
    return s * (0.6 if up else 1)


def sfx_key(soft=False):
    """机械键盘：按下的"咔"+ 键帽的闷响 + 回弹"""
    n = int(0.32 * SR)
    t = np.arange(n) / SR
    s = fft_filter(noise(n), lambda f: band(f, 2200, 1.1)) * np.exp(-t / 0.006) * 0.9
    s += np.sin(2 * np.pi * 170 * t) * np.exp(-t / 0.03) * 0.8
    s += np.sin(2 * np.pi * 420 * t) * np.exp(-t / 0.015) * 0.3
    rel = int(0.14 * SR)
    up = fft_filter(noise(n - rel), lambda f: band(f, 3200, 1)) * np.exp(-np.arange(n - rel) / SR / 0.004) * 0.35
    s[rel:] += up
    return s * (0.55 if soft else 1)


def sfx_type():
    n = int(0.08 * SR)
    t = np.arange(n) / SR
    f = rng.uniform(2400, 3400)
    s = fft_filter(noise(n), lambda fr: band(fr, f, 0.8)) * np.exp(-t / 0.004)
    s += np.sin(2 * np.pi * 240 * t) * np.exp(-t / 0.012) * 0.3
    return s * 0.7


def sfx_pop(n_=0, harsh=False, soft=False):
    dur = 0.18
    t = tt(dur)
    base = 520 * 2 ** (((n_ * 3) % 12) / 12) if not harsh else rng.uniform(380, 760)
    f = base * (1 + 0.8 * np.exp(-t / 0.012))
    ph = 2 * np.pi * np.cumsum(f) / SR
    s = np.sin(ph)
    if harsh:
        s = np.tanh(3 * s) * 0.8 + 0.3 * np.sin(ph * 1.5)
    return s * env_ad(len(t), 0.002, 0.05) * (0.5 if soft else 1)


def sfx_whoosh(dur=0.9, soft=False, up=False):
    n = int((dur + 0.3) * SR)
    x = noise(n)

    def mask(tm, f):
        p = np.clip(tm / dur, 0, 1)
        fc = 400 * 2 ** (4.5 * (p if up else np.sin(np.pi * p)))
        return band(f, fc, 0.9)

    s = stft_filter(x, mask)
    t = np.arange(n) / SR
    e = np.sin(np.pi * np.clip(t / dur, 0, 1)) ** 1.5
    s *= e
    # 左 → 右扫过去
    pan = np.clip(t / dur, 0, 1) * 1.6 - 0.8
    th = (pan + 1) * np.pi / 4
    return np.stack([s * np.cos(th), s * np.sin(th)]) * np.sqrt(2) * (0.5 if soft else 1)


def sfx_riser(dur=2.0):
    n = int(dur * SR)
    t = np.arange(n) / SR
    x = noise(n)
    s = stft_filter(x, lambda tm, f: band(f, 300 * 2 ** (5 * np.clip(tm / dur, 0, 1)), 0.7))
    s *= (t / dur) ** 2
    f = 200 + 1400 * (t / dur) ** 2
    s += 0.25 * np.sin(2 * np.pi * np.cumsum(f) / SR) * (t / dur) ** 2
    return s


def sfx_reverse_swell(dur=0.65):
    n = int(dur * SR)
    t = np.arange(n) / SR
    x = noise(n)
    s = stft_filter(x, lambda tm, f: lp(f, 300 * 2 ** (5 * np.clip(tm / dur, 0, 1)), 2))
    e = np.exp((t - dur) / (dur / 4))
    e[-int(0.004 * SR):] *= np.linspace(1, 0, int(0.004 * SR))
    bell = fm_bell(mtof(81), dur)[::-1] * 0.25
    return s * e + bell * e


def sfx_powerdown():
    t = tt(0.55)
    f = 500 * np.exp(-t / 0.18) + 40
    ph = 2 * np.pi * np.cumsum(f) / SR
    s = np.sign(np.sin(ph)) * 0.25 + np.sin(ph) * 0.5
    s = fft_filter(s, lambda fr: lp(fr, 1800, 2))
    return s * env_ad(len(t), 0.002, 0.25)


def sfx_ping(f=1760):
    return fm_bell(f, 2.0, ratio=2.0, index=0.8, decay=0.6) * 0.8


def sfx_particles(dur=2.0, count=46, lo=2200, hi=6000):
    n = int((dur + 0.5) * SR)
    out = np.zeros((2, n))
    for _ in range(count):
        t0 = rng.uniform(0, dur) ** 1.2 / dur ** 0.2
        f = rng.uniform(lo, hi)
        m = int(0.12 * SR)
        tm = np.arange(m) / SR
        s = np.sin(2 * np.pi * f * tm) * np.exp(-tm / 0.025) * rng.uniform(0.2, 1)
        pan = rng.uniform(-0.9, 0.9)
        th = (pan + 1) * np.pi / 4
        i = int(t0 * SR)
        k = min(m, n - i)
        out[0, i : i + k] += s[:k] * np.cos(th)
        out[1, i : i + k] += s[:k] * np.sin(th)
    return out


def sfx_error():
    out = np.zeros(int(0.4 * SR))
    for k, off in enumerate([0, 0.13]):
        t = tt(0.12)
        f = 196 if k == 0 else 165
        s = np.tanh(2.2 * np.sin(2 * np.pi * f * t)) * env_ad(len(t), 0.003, 0.05)
        i = int(off * SR)
        out[i : i + len(s)] += s
    return fft_filter(out, lambda f: lp(f, 2500, 2)) * 0.8


def sfx_hit(soft=False):
    s = sub_boom(1.6, 70, 40) * 0.9
    n = len(s)
    s += fft_filter(noise(n), lambda f: band(f, 600, 1.5)) * env_ad(n, 0.002, 0.08) * 0.4
    return s * (0.6 if soft else 1)


def sfx_scan(dur=1.8, soft=False):
    n = int((dur + 0.4) * SR)
    t = np.arange(n) / SR
    p = np.clip(t / dur, 0, 1)
    x = noise(n)
    s = stft_filter(x, lambda tm, f: band(f, 900 * 2 ** (3 * np.clip(tm / dur, 0, 1)), 0.35))
    s *= np.sin(np.pi * p) ** 0.7 * 0.7
    f = 700 + 2200 * p
    tone = np.sin(2 * np.pi * np.cumsum(f) / SR) * 0.18 * np.sin(np.pi * p)
    tone += np.sin(2 * np.pi * np.cumsum(f * 1.5) / SR) * 0.08 * np.sin(np.pi * p)
    pan = p * 1.6 - 0.8
    th = (pan + 1) * np.pi / 4
    m = s + tone
    return np.stack([m * np.cos(th), m * np.sin(th)]) * np.sqrt(2) * (0.55 if soft else 1)


def sfx_draw(dur=0.4):
    n = int((dur + 0.05) * SR)
    t = np.arange(n) / SR
    x = noise(n) * (0.6 + 0.4 * np.sin(2 * np.pi * 23 * t))
    s = fft_filter(x, lambda f: band(f, 3800, 0.9))
    return s * env_asr(n, 0.02, 0.04, dur) * 0.35


def sfx_mosaic(dur=0.43):
    n = int((dur + 0.2) * SR)
    out = np.zeros(n)
    k = 0
    for t0 in np.arange(0, dur, 0.035):
        m = int(0.05 * SR)
        tm = np.arange(m) / SR
        f = [880, 1175, 1320, 1760, 1568][k % 5]
        s = np.sign(np.sin(2 * np.pi * f * tm)) * np.exp(-tm / 0.012) * 0.35
        i = int(t0 * SR)
        out[i : i + m] += s[: n - i]
        k += 1
    return fft_filter(out, lambda f: lp(f, 5000, 2))


def sfx_shutter(soft=False):
    n = int(0.35 * SR)
    t = np.arange(n) / SR
    s = np.zeros(n)
    for off, f, a in [(0, 2600, 1), (0.07, 1700, 0.8)]:
        i = int(off * SR)
        m = n - i
        tm = np.arange(m) / SR
        s[i:] += fft_filter(noise(m), lambda fr: band(fr, f, 0.7)) * np.exp(-tm / 0.012) * a
        s[i:] += np.sin(2 * np.pi * f / 4 * tm) * np.exp(-tm / 0.02) * 0.3 * a
    s += fft_filter(noise(n), lambda fr: band(fr, 6000, 1)) * env_ad(n, 0.01, 0.06) * 0.2
    return s * (0.5 if soft else 1)


def sfx_success(big=False, soft=False):
    notes = [76, 81, 85] if not big else [74, 78, 81, 86]
    out = np.zeros(int(2.5 * SR))
    for k, m in enumerate(notes):
        b = fm_bell(mtof(m), 1.8, ratio=2.0, index=1.2, decay=0.5 if not big else 0.9) * (0.9 - 0.1 * k)
        i = int(k * 0.07 * SR)
        out[i : i + len(b)] += b[: len(out) - i]
    return out * (0.5 if soft else 1) * 0.6


def sfx_chime(soft=False):
    out = np.zeros(int(2.4 * SR))
    for k, (m, off) in enumerate([(81, 0), (86, 0.14)]):
        b = fm_bell(mtof(m), 2.0, ratio=3.01, index=1.6, decay=0.7)
        i = int(off * SR)
        out[i : i + len(b)] += b[: len(out) - i]
    return out * (0.5 if soft else 0.75)


def sfx_tick(soft=False):
    t = tt(0.04)
    return np.sin(2 * np.pi * 3300 * t) * np.exp(-t / 0.005) * (0.35 if soft else 0.6)


def sfx_scramble(count, gap):
    n = int((count * gap + 0.6) * SR)
    out = np.zeros(n)
    for c in range(count):
        for j in range(4):
            t0 = c * gap + j * 0.05
            m = int(0.03 * SR)
            tm = np.arange(m) / SR
            f = rng.uniform(1800, 4200)
            s = np.sign(np.sin(2 * np.pi * f * tm)) * np.exp(-tm / 0.006) * 0.25
            i = int(t0 * SR)
            out[i : i + m] += s
        # 落定的那一下，按音阶往上走
        b = pluck(mtof(74 + [0, 2, 4, 7, 9, 12, 14, 16, 19][c % 9]), 0.4, 0.12) * 0.5
        i = int((c * gap + 0.22) * SR)
        out[i : i + len(b)] += b[: n - i]
    return out


def sfx_stream(dur):
    n = int((dur + 0.2) * SR)
    out = np.zeros(n)
    for t0 in np.arange(0, dur, 0.045):
        m = int(0.03 * SR)
        tm = np.arange(m) / SR
        s = np.sin(2 * np.pi * rng.uniform(2500, 3500) * tm) * np.exp(-tm / 0.004) * rng.uniform(0.15, 0.35)
        i = int(t0 * SR)
        out[i : i + m] += s
    return out


def sfx_fly(dur=0.6):
    w = sfx_whoosh(dur, soft=True, up=True)
    t = tt(dur)
    f = 600 * 2 ** (2 * t / dur)
    s = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.sin(np.pi * t / dur) * 0.25
    w[:, : len(s)] += s
    return w


def sfx_word(n_):
    """快闪段每一拍的和弦短刺"""
    roots = [62, 57, 59, 55, 62, 57, 59, 55]
    m = roots[n_ % len(roots)]
    notes = [m, m + 4 if m in (62, 55) else m + 3, m + 7, m + 12]
    dur = 0.32
    s = np.zeros((2, int(0.6 * SR)))
    for k, mm in enumerate(notes):
        st = saw_stack(mtof(mm + 12), 0.6, 2600, seed=n_ * 7 + k)
        s += st
    t = tt(0.6)
    e = np.exp(-t / 0.12) * np.clip(t / 0.004, 0, 1) * np.clip((dur + 0.15 - t) / 0.15, 0, 1)
    return s * e * 0.45


def sfx_shimmer(dur=0.8, base=74):
    out = np.zeros(int((dur + 1.6) * SR))
    steps = [0, 4, 7, 11, 12, 16, 19, 24]
    for k, st in enumerate(steps):
        b = fm_bell(mtof(base + st), 1.5, ratio=2.0, index=1.0, decay=0.5) * (0.6 - 0.04 * k)
        i = int(k * dur / len(steps) * SR)
        out[i : i + len(b)] += b[: len(out) - i]
    return out * 0.5


# ───────────────────────── 编曲 ─────────────────────────
CH = {
    'Bm9': (35, [54, 57, 61, 62, 66]),
    'Gmaj9': (31, [54, 57, 59, 62, 66]),
    'Em9': (40, [55, 59, 62, 66]),
    'F#m7': (42, [57, 61, 64, 66]),
    'Asus': (33, [57, 59, 62, 64]),
    'D': (38, [54, 57, 61, 64, 69]),
    'A': (33, [52, 57, 59, 61, 64]),
    'Bm': (35, [54, 57, 61, 62, 66]),
    'G': (31, [54, 57, 59, 62, 67]),
}
# (开始, 结束, 和弦, 低通截止频率)
CHORDS = [(0.4, 4, 'Bm9', 700), (4, 8, 'Gmaj9', 800), (8, 10, 'Em9', 1000), (10, 12.05, 'F#m7', 1200)]
LOOP = ['D', 'A', 'Bm', 'G']
for i, s in enumerate(np.arange(14, 62, 2.0)):
    CHORDS.append((float(s), float(s + 2), LOOP[i % 4], 3600 if s >= 20 else 3000))
CHORDS += [(62, 64, 'Bm9', 1200), (64, 66, 'Bm9', 700), (66, 68, 'Gmaj9', 750)]
CHORDS += [(68, 70, 'D', 3600), (70, 72, 'A', 3600), (72, 74.55, 'G', 1800), (74.55, 78, 'D', 3000)]

GROOVE = [(14, 57.5), (58, 62), (68, 72.5)]  # 有鼓的段落
HATS = [(16, 57.5), (58, 62), (68, 72.5)]
CLAPS = [(22, 57.5), (58, 62), (70, 72.5)]
ARPS = [(20, 57.5), (58, 62), (68, 72.5)]


def in_any(t, spans):
    return any(a <= t < b for a, b in spans)


def build(cues, duration):
    music = Bus(duration)
    drums = Bus(duration)
    sfx = Bus(duration)
    send = Bus(duration)  # 混响

    # 和弦铺底
    for k, (a, b, name, cut) in enumerate(CHORDS):
        root, notes = CH[name]
        p = pad_chord(notes, b - a, cut, attack=0.25 if a >= 14 else 0.9, release=1.6 if b < 76 else 3.0, seed=k)
        g = 0.33 if a < 14 else 0.26
        if 62 <= a < 68:
            g = 0.3
        music.add(p, a, g)
        send.add(p, a, g * 0.5)
        # 低音：开场是长音，律动段是八分音符
        if in_any(a + 0.01, GROOVE):
            for t0 in np.arange(a, b, BEAT / 2):
                if not in_any(t0, GROOVE):
                    continue
                m = root + (12 if int(round((t0 - a) / (BEAT / 2))) % 4 == 3 else 0)
                music.add(bass_note(mtof(m + 12), BEAT / 2 * 0.9), t0, 0.22)
        elif a < 12:
            music.add(bass_note(mtof(root + 12), b - a) * env_asr(int((b - a) * SR), 0.6, 0.6, b - a), a, 0.12 if a < 8 else 0.16)
            if a >= 8:  # 紧张段：八分音符的脉冲
                for t0 in np.arange(a, b, BEAT / 2):
                    music.add(bass_note(mtof(root + 12), 0.12), t0, 0.18 * (1 + (t0 - 8) / 4))
        elif a >= 74.5:
            music.add(bass_note(mtof(root + 12), b - a) * env_asr(int((b - a) * SR), 0.01, 2.5, b - a), a, 0.3)
        elif 62 <= a < 68:
            music.add(bass_note(mtof(root + 12), b - a) * env_asr(int((b - a) * SR), 0.3, 0.5, b - a), a, 0.14)

        # 琶音
        if in_any(a + 0.01, ARPS):
            tones = sorted(set([n + 12 for n in notes] + [n + 24 for n in notes[:2]]))
            pat = [0, 2, 1, 3, 2, 4, 3, 1, 0, 2, 4, 5, 3, 2, 1, 3]
            for j, t0 in enumerate(np.arange(a, b, BEAT / 4)):
                if not in_any(t0, ARPS):
                    continue
                m = tones[pat[j % 16] % len(tones)]
                pan = -0.45 if j % 2 else 0.45
                lvl = 0.095 if t0 < 58 else 0.12
                s = pluck(mtof(m), 0.5, 0.16, bright=0.8 + 0.4 * ((j % 4) == 0))
                music.add(s, t0, lvl, pan)
                send.add(s, t0, lvl * 0.6, pan)
                # 三个十六分音符的乒乓延迟
                for d in range(1, 4):
                    music.add(s, t0 + 0.375 * d, lvl * 0.35 ** d, -pan * (1 if d % 2 else -1))

    # 鼓
    kicks = []
    for t0 in np.arange(14, duration, BEAT):
        if in_any(t0, GROOVE):
            kicks.append(t0)
            drums.add(kick(), t0, 0.5)
        if in_any(t0 + BEAT / 2, HATS):
            drums.add(hat(open_=(int(t0 / BEAT) % 8 == 7)), t0 + BEAT / 2, 0.14, 0.3)
        if in_any(t0, CLAPS) and int(round(t0 / BEAT)) % 2 == 1:
            c = clap()
            drums.add(c, t0, 0.26, -0.1)
            send.add(c, t0, 0.12)
    # 快闪段加十六分音符的踩镲
    for t0 in np.arange(58, 62, BEAT / 4):
        drums.add(hat(), t0, 0.06, -0.35)
    # 紧张段：踩镲越来越密
    for t0 in np.arange(8, 12, BEAT / 2):
        drums.add(hat(), t0, 0.07, 0.3)
    for t0 in np.arange(10, 12, BEAT / 4):
        drums.add(hat(), t0, 0.06 * (1 + (t0 - 10)), -0.3)
    # 进快闪段前的军鼓滚奏
    for t0 in np.arange(57.0, 58.0, BEAT / 4):
        p = (t0 - 57.0)
        drums.add(clap(), t0, 0.08 + 0.18 * p)
    # 侧链：底鼓一响，铺底的音乐让一让
    duck = np.ones(music.n)
    for t0 in kicks:
        i = int(t0 * SR)
        m = min(int(0.4 * SR), music.n - i)
        tm = np.arange(m) / SR
        duck[i : i + m] = np.minimum(duck[i : i + m], 1 - 0.55 * np.exp(-tm / 0.11))
    music.x *= duck

    # 落拍和结尾的重拍
    for t0, big in [(14.0, True), (67.98, False)]:
        drums.add(sub_boom(2.4, 62, 40), t0, 0.6 if big else 0.45)
        c = crash(3.0)
        drums.add(c, t0, 0.32 if big else 0.24, 0.2)
        send.add(c, t0, 0.15)
    drums.add(sub_boom(3.0, 62, 42), 74.55, 0.4)
    c = crash(3.5)
    drums.add(c, 74.55, 0.2, -0.2)
    send.add(c, 74.55, 0.15)

    # 音效
    for c in cues:
        t0 = c['t']
        k = c['type']
        soft = bool(c.get('soft'))

        def put(sig, gain, pan=0.0, rev=0.25):
            sfx.add(sig, t0, gain, pan)
            send.add(sig, t0, gain * rev, pan)

        if k == 'seed':
            put(sfx_ping(1760), 0.07 if t0 < 1 else 0.11, 0, 0.8)
        elif k == 'swell':
            put(sfx_particles(c.get('dur', 2)), 0.22, 0, 0.6)
        elif k == 'click':
            put(sfx_click(bool(c.get('up'))), 0.2, 0.1, 0.1)
        elif k == 'error':
            put(sfx_error(), 0.34, 0.2, 0.2)
        elif k == 'hit':
            put(sfx_hit(soft), 0.6, 0, 0.3)
        elif k == 'scatter':
            put(sfx_particles(c.get('dur', 0.8), 30, 1500, 5000), 0.18, 0, 0.5)
            put(sfx_whoosh(c.get('dur', 0.8), soft=True), 0.25, 0, 0.4)
        elif k == 'pop':
            pan = float(rng.uniform(-0.6, 0.6))
            put(sfx_pop(c.get('n', 0), bool(c.get('harsh')), soft), 0.17 if c.get('harsh') else 0.22, pan, 0.2)
        elif k == 'riser':
            put(sfx_riser(c.get('dur', 2)), 0.2, 0, 0.3)
        elif k == 'freeze':
            put(sfx_powerdown(), 0.3, 0, 0.3)
        elif k == 'implode':
            put(sfx_reverse_swell(c.get('dur', 0.65)), 0.3 if not soft else 0.2, 0, 0.2)
        elif k == 'drop':
            pass  # 已在上面
        elif k == 'blip':
            notes = [76, 78, 81, 86]
            put(fm_bell(mtof(notes[c.get('n', 0) % 4]), 1.6, ratio=2.0, index=1.4, decay=0.6), 0.22, [-0.4, -0.1, 0.1, 0.4][c.get('n', 0) % 4], 0.6)
        elif k == 'shimmer':
            put(sfx_shimmer(c.get('dur', 0.8)), 0.3, 0, 0.7)
        elif k == 'whoosh':
            put(sfx_whoosh(c.get('dur', 0.9), soft), 0.24, 0, 0.3)
        elif k == 'tick':
            put(sfx_tick(soft), 0.22, float(rng.uniform(-0.5, 0.5)), 0.2)
        elif k == 'key':
            put(sfx_key(soft), 0.5, -0.2, 0.15)
        elif k == 'draw':
            put(sfx_draw(c.get('dur', 0.4)), 0.4, 0.2, 0.1)
        elif k == 'mosaic':
            put(sfx_mosaic(c.get('dur', 0.43)), 0.2, 0.2, 0.2)
        elif k == 'shutter':
            put(sfx_shutter(soft), 0.45, 0, 0.25)
        elif k == 'success':
            put(sfx_success(bool(c.get('big')), soft), 0.34 if c.get('big') else 0.3, 0, 0.6)
        elif k == 'card':
            put(sfx_pop(c.get('n', 0) + 2, soft=True), 0.22, [-0.4, 0.4, -0.2, 0.2][c.get('n', 0) % 4], 0.3)
        elif k == 'scan':
            put(sfx_scan(c.get('dur', 1.8), soft), 0.32, 0, 0.35)
        elif k == 'scramble':
            put(sfx_scramble(c.get('n', 9), c.get('gap', 0.07)), 0.2, 0, 0.3)
        elif k == 'type':
            put(sfx_type(), 0.2, float(rng.uniform(-0.3, 0.3)), 0.1)
        elif k == 'stream':
            put(sfx_stream(c.get('dur', 1.4)), 0.16, 0.1, 0.2)
        elif k == 'chime':
            put(sfx_chime(soft), 0.3, 0.4, 0.5)
        elif k == 'fly':
            put(sfx_fly(c.get('dur', 0.6)), 0.26, 0, 0.3)
        elif k == 'word':
            put(sfx_word(c.get('n', 0)), 0.32, 0, 0.35)
        elif k == 'swarm':
            put(sfx_particles(c.get('dur', 1.2), 70, 1800, 6500), 0.2, 0, 0.6)
        elif k == 'finale':
            put(sfx_shimmer(1.0, 74), 0.25, 0, 0.8)
        else:
            print('未知音效', k, file=sys.stderr)

    return music, drums, sfx, send


def reverb(x, t60=2.6, predelay=0.02):
    n = int(t60 * SR)
    t = np.arange(n) / SR
    out = np.zeros((2, x.shape[1]))
    for ch in range(2):
        ir = noise(n) * np.exp(-6.9 * t / t60)
        ir = fft_filter(ir, lambda f: lp(f, 5000, 1))
        ir[: int(0.005 * SR)] *= np.linspace(0, 1, int(0.005 * SR))
        ir = np.concatenate([np.zeros(int(predelay * SR)), ir])
        ir /= np.sqrt(np.sum(ir**2))
        N = 1 << int(np.ceil(np.log2(x.shape[1] + len(ir))))
        y = np.fft.irfft(np.fft.rfft(x[ch], N) * np.fft.rfft(ir, N), N)
        out[ch] = y[: x.shape[1]]
    return out


def main():
    src, dst = sys.argv[1], sys.argv[2]
    data = json.load(open(src))
    duration = float(data['duration'])
    music, drums, sfx, send = build(data['cues'], duration)
    wet = reverb(send.x)
    mix = music.x * 0.9 + drums.x * 1.0 + sfx.x * 1.0 + wet * 0.55
    # 总线 EQ：切掉超低频，低频稍压，2–6 kHz 提一点让声音更亮、更"在前面"
    def master_eq(f):
        g = hp(f, 32, 2)
        g *= 10 ** ((-2.5 * lp(f, 110, 2)) / 20)
        g *= 10 ** ((3.5 * band(f, 3500, 1.1)) / 20)
        g *= 10 ** ((1.5 * band(f, 9000, 0.8)) / 20)
        return g

    for ch in range(2):
        mix[ch] = fft_filter(mix[ch], master_eq)
    n = int(duration * SR)
    mix = mix[:, :n]
    # 收尾：76.7 秒开始跟画面一起淡出
    t = np.arange(n) / SR
    mix *= np.clip((duration - 0.1 - t) / 1.2, 0, 1) ** 1.5
    mix *= np.clip(t / 0.05, 0, 1)
    # 总线：先按响度提一提，再用 tanh 软限幅压住峰值
    rms = np.sqrt(np.mean(mix[:, int(14 * SR) : int(58 * SR)] ** 2))
    mix *= 0.16 / max(rms, 1e-6)
    mix = np.tanh(mix * 1.1) / np.tanh(1.1)
    mix *= 0.89 / max(np.max(np.abs(mix)), 1e-6)
    pcm = (mix.T * 32767).astype(np.int16)
    with wave.open(dst, 'wb') as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(pcm.tobytes())
    print(dst)


if __name__ == '__main__':
    main()
