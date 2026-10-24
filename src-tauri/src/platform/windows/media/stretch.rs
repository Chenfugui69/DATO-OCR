//! 变速不变调（剪辑导出时用）：WSOLA。
//!
//! 按 `speed` 倍的步子从输入里取一段段（每段 32ms，汉宁窗，一半重叠）叠起来输出：取的步子大于放的步子
//! 就是加快。每段不是死板地取在算出来的位置上，而是在前后 10ms 里找一个和"上一段自然接下去的那一截"
//! 最像的地方取，波形接得上，不会有咔哒声和回声。立体声交错的 f32，边进边出。

const FRAME: usize = 1536;
const HOP: usize = FRAME / 2;
const SEARCH: usize = 480;
/// 算相似度时隔几个样本取一个（够准，快好几倍）
const STRIDE: usize = 4;

pub struct Stretcher {
    speed: f64,
    window: Vec<f32>,
    /// 还没用完的输入（立体声交错），`base` 是它第一帧在整个输入里的位置
    input: Vec<f32>,
    base: usize,
    /// 下一段名义上从输入的哪一帧开始取
    nominal: f64,
    /// 上一段实际从哪一帧取的
    prev: Option<usize>,
    /// 叠加中的输出（FRAME 帧）
    acc: Vec<f32>,
}

impl Stretcher {
    pub fn new(speed: f64) -> Self {
        let window = (0..FRAME)
            .map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / FRAME as f64).cos()) as f32)
            .collect();
        Self {
            speed: speed.clamp(0.25, 4.0),
            window,
            input: Vec::new(),
            base: 0,
            nominal: 0.0,
            prev: None,
            acc: vec![0.0; FRAME * 2],
        }
    }

    fn frames(&self) -> usize {
        self.input.len() / 2
    }

    /// 输入里第 `abs` 帧（绝对位置）的单声道值（左右相加）
    fn mono(&self, abs: usize) -> f32 {
        let i = (abs - self.base) * 2;
        self.input[i] + self.input[i + 1]
    }

    /// 喂一段输入，取走能确定下来的输出
    pub fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        self.input.extend_from_slice(samples);
        let mut out = Vec::new();
        loop {
            let nominal = self.nominal.round() as usize;
            let lo = nominal.saturating_sub(SEARCH).max(self.base);
            let hi = nominal + SEARCH;
            // 要能取到 hi + FRAME，还要能取到上一段自然接下去的那一截
            let need = hi + FRAME;
            let natural_end = self.prev.map_or(0, |p| p + HOP + FRAME);
            if self.base + self.frames() < need.max(natural_end) {
                break;
            }
            let pick = match self.prev {
                None => nominal.max(self.base),
                Some(p) => self.best(p + HOP, lo, hi),
            };
            self.overlap_add(pick, &mut out);
            self.prev = Some(pick);
            self.nominal += HOP as f64 * self.speed;
            // 前面用不到的输入扔掉（留着上一段起点之后的）
            let keep_from = pick.min(self.nominal as usize).saturating_sub(SEARCH);
            if keep_from > self.base {
                let drop = keep_from - self.base;
                self.input.drain(..drop * 2);
                self.base = keep_from;
            }
        }
        out
    }

    /// 在 [lo, hi] 里找和 `target` 开始的那一截最像的位置（归一化互相关）
    fn best(&self, target: usize, lo: usize, hi: usize) -> usize {
        let len = FRAME / 2;
        let mut best = (f32::MIN, lo.max(target.min(hi)));
        let mut c = lo;
        while c <= hi {
            let (mut dot, mut energy) = (0f32, 1e-6f32);
            let mut k = 0;
            while k < len {
                let a = self.mono(c + k);
                dot += a * self.mono(target + k);
                energy += a * a;
                k += STRIDE;
            }
            let score = dot / energy.sqrt();
            if score > best.0 {
                best = (score, c);
            }
            c += 2;
        }
        best.1
    }

    fn overlap_add(&mut self, pick: usize, out: &mut Vec<f32>) {
        let start = (pick - self.base) * 2;
        for i in 0..FRAME {
            let w = self.window[i];
            self.acc[i * 2] += self.input[start + i * 2] * w;
            self.acc[i * 2 + 1] += self.input[start + i * 2 + 1] * w;
        }
        // 前一半已经叠完（汉宁窗一半重叠加起来正好是 1），输出；后一半挪到前面等下一段
        out.extend_from_slice(&self.acc[..HOP * 2]);
        self.acc.copy_within(HOP * 2.., 0);
        for v in &mut self.acc[HOP * 2..] {
            *v = 0.0;
        }
    }

    /// 输入完了：剩下的按原样接上（不到一段的尾巴不再变速，最多几十毫秒）
    pub fn finish(mut self) -> Vec<f32> {
        let mut out = Vec::new();
        let start = self.prev.map_or(self.nominal as usize, |p| p + HOP);
        let tail_from = start.max(self.base);
        let tail = &self.input[((tail_from - self.base) * 2).min(self.input.len())..];
        // 还没输出的是上一段的后半（窗的下降沿），剩下的输入按上升沿淡入叠上去，两边加起来正好是 1
        let pending = &mut self.acc[..HOP * 2];
        for (i, v) in pending.iter_mut().enumerate() {
            if let Some(t) = tail.get(i) {
                *v += t * self.window[i / 2];
            }
        }
        out.extend_from_slice(pending);
        if tail.len() > HOP * 2 {
            out.extend_from_slice(&tail[HOP * 2..]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, freq: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = (i as f32 * freq * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
                [v, v]
            })
            .collect()
    }

    fn run(speed: f64, input: &[f32]) -> Vec<f32> {
        let mut s = Stretcher::new(speed);
        let mut out = Vec::new();
        for c in input.chunks(2000) {
            out.extend(s.push(c));
        }
        out.extend(s.finish());
        out
    }

    /// 过零点数 / 时长 = 频率：变速后音调不变
    fn zero_crossings(x: &[f32]) -> usize {
        x.chunks(2)
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|w| (w[0][0] >= 0.0) != (w[1][0] >= 0.0))
            .count()
    }

    #[test]
    fn length_follows_speed_and_pitch_stays() {
        let input = sine(48_000 * 2, 440.0);
        for speed in [0.5, 2.0] {
            let out = run(speed, &input);
            let frames = out.len() / 2;
            let want = (96_000.0 / speed) as usize;
            assert!(
                frames.abs_diff(want) < 3_000,
                "speed {speed}: {frames} vs {want}"
            );
            // 中间一段的频率还是 440Hz 左右
            let mid = &out[frames / 4 * 2..frames * 3 / 4 * 2];
            let hz = zero_crossings(mid) as f64 / 2.0 / (mid.len() as f64 / 2.0 / 48_000.0);
            assert!((hz - 440.0).abs() < 15.0, "speed {speed}: {hz}Hz");
            // 没有爆音
            assert!(out.iter().all(|v| v.abs() <= 0.6));
        }
    }
}
