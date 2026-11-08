//! Chop engine: cut a buffer (or a range of it) into slices for pads / sampler kits.
//!
//! Four ways to place slice points, all returning frame positions:
//! * [`OnsetCurve::pick`]: transients, with a sensitivity and a minimum gap. The onset curve is
//!   computed once per buffer/range so dragging the sensitivity slider stays cheap.
//! * [`grid_points`]: every ½ / 1 / 2 beats or 1 / 2 / 4 bars of a [`BeatGrid`].
//! * [`equal_points`]: N equal parts.
//! * manual markers, which only need [`slices_from_points`].
//!
//! [`slices_from_points`] turns points into contiguous slices, [`snap_to_zero`] moves an edge
//! onto a zero crossing and [`render_slice`] copies a slice out with tiny anti-click fades.

use realfft::RealFftPlanner;

use crate::AudioBuffer;
use crate::analysis::BeatGrid;

/// One slice, frames `[start, end)` of the source buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slice {
    pub start: usize,
    pub end: usize,
}

impl Slice {
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---- transients ----------------------------------------------------------------------------

const FFT: usize = 1024;
const HOP: usize = 256;

/// Spectral-flux onset curve of a range of a buffer, at `HOP`-frame resolution.
#[derive(Debug, Clone, Default)]
pub struct OnsetCurve {
    /// First frame of the analysed range in the source buffer.
    pub offset: usize,
    /// Frames in the analysed range.
    pub len: usize,
    pub hop: usize,
    /// Onset strength per hop, scaled so the strongest onset is 1.
    pub flux: Vec<f32>,
    /// Mono downmix of the range (kept for sample-accurate refinement).
    mono: Vec<f32>,
}

/// Transient-detection settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransientParams {
    /// 0 = only the hardest hits, 1 = every little ghost note.
    pub sensitivity: f32,
    /// Minimum distance between two slice points, seconds.
    pub min_gap_secs: f64,
}

impl Default for TransientParams {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            min_gap_secs: 0.08,
        }
    }
}

fn mono_range(buf: &AudioBuffer, start: usize, end: usize) -> Vec<f32> {
    let end = end.min(buf.frames());
    let start = start.min(end);
    let n = buf.n_channels().max(1) as f32;
    (start..end)
        .map(|i| buf.channels.iter().map(|c| c[i]).sum::<f32>() / n)
        .collect()
}

impl OnsetCurve {
    /// Analyse frames `[start, end)` of `buf`.
    pub fn compute(buf: &AudioBuffer, start: usize, end: usize) -> Self {
        let mono = mono_range(buf, start, end);
        let len = mono.len();
        let mut curve = OnsetCurve {
            offset: start.min(buf.frames()),
            len,
            hop: HOP,
            flux: Vec::new(),
            mono,
        };
        if len < HOP * 2 {
            return curve;
        }
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT);
        let window: Vec<f32> = (0..FFT)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT as f32).cos())
            .collect();
        let mut input = fft.make_input_vec();
        let mut out = fft.make_output_vec();
        let mut scratch = fft.make_scratch_vec();
        let bins = FFT / 2 + 1;
        let mut prev = vec![0.0f32; bins];
        // Frame f is centred on sample f * HOP.
        let n_frames = len / HOP + 1;
        let mut flux = vec![0.0f32; n_frames];
        let half = (FFT / 2) as isize;
        let x = &curve.mono;
        for (f, fl) in flux.iter_mut().enumerate() {
            let s0 = (f * HOP) as isize - half;
            for (k, v) in input.iter_mut().enumerate() {
                let i = s0 + k as isize;
                *v = if i >= 0 && (i as usize) < len {
                    x[i as usize] * window[k]
                } else {
                    0.0
                };
            }
            if fft
                .process_with_scratch(&mut input, &mut out, &mut scratch)
                .is_err()
            {
                continue;
            }
            let mut s = 0.0f32;
            for (k, c) in out.iter().enumerate() {
                // Square-root compression: quiet hits still register, but loud ones stand out
                // (a log would make a ghost note as strong as a snare).
                let m = c.norm().sqrt();
                if f > 0 {
                    // Lean towards the highs, where attacks live.
                    let w = 1.0 + k as f32 / bins as f32;
                    s += (m - prev[k]).max(0.0) * w;
                }
                prev[k] = m;
            }
            *fl = s;
        }
        let max = flux.iter().copied().fold(0.0f32, f32::max);
        if max > 1e-9 {
            flux.iter_mut().for_each(|v| *v /= max);
        }
        curve.flux = flux;
        curve
    }

    /// Transient positions (frames in the source buffer), each placed just before its attack.
    /// The range start itself is never returned.
    pub fn pick(&self, sample_rate: u32, params: TransientParams) -> Vec<usize> {
        let o = &self.flux;
        let n = o.len();
        if n < 3 {
            return Vec::new();
        }
        let sr = sample_rate.max(1) as f64;
        let sens = params.sensitivity.clamp(0.0, 1.0);
        // Fixed floor over a local mean: low sensitivity needs a big jump above the neighbourhood.
        let delta = 0.45 * (1.0 - sens).powf(1.6) + 0.015;
        let w = ((0.1 * sr) as usize / HOP).max(2);
        let mut prefix = vec![0.0f64; n + 1];
        for i in 0..n {
            prefix[i + 1] = prefix[i] + o[i] as f64;
        }
        let min_gap = (params.min_gap_secs.max(0.0) * sr) as usize;
        // Silence gate: ignore "onsets" in passages ~60 dB below the loudest part.
        let gate = {
            let peak = self.mono.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            peak * 1e-3
        };
        let mut out: Vec<usize> = Vec::new();
        for i in 1..n - 1 {
            let v = o[i];
            if v < o[i - 1] || v < o[i + 1] {
                continue;
            }
            // Local maximum over ±3 hops.
            let lo = i.saturating_sub(3);
            let hi = (i + 3).min(n - 1);
            if (lo..=hi).any(|j| o[j] > v) {
                continue;
            }
            let a = i.saturating_sub(w);
            let b = (i + w + 1).min(n);
            let mean = ((prefix[b] - prefix[a]) / (b - a) as f64) as f32;
            if v < mean + delta {
                continue;
            }
            let at = self.refine(i);
            let local_peak = self.mono[at.min(self.len)..(at + HOP * 2).min(self.len)]
                .iter()
                .fold(0.0f32, |m, x| m.max(x.abs()));
            if local_peak < gate {
                continue;
            }
            if at < HOP / 2 {
                // An attack right at the start of the range: that's the range start.
                continue;
            }
            match out.last() {
                Some(&last) if at < last + min_gap => {
                    // Too close: keep the stronger of the two.
                    let last_hop = (last + HOP / 2) / HOP;
                    if o[i] > o[last_hop.min(n - 1)] * 1.5 {
                        *out.last_mut().unwrap_or(&mut 0) = at;
                    }
                }
                _ => out.push(at),
            }
        }
        out.into_iter().map(|f| f + self.offset).collect()
    }

    /// Sample-accurate attack position (range-relative) for the onset peak at hop `i`: the
    /// loudest 32-frame block near the peak, walked back to where the level is still low.
    fn refine(&self, i: usize) -> usize {
        const BLK: usize = 32;
        let x = &self.mono;
        let centre = i * HOP;
        let lo = centre.saturating_sub(FFT / 2 + HOP);
        let hi = (centre + HOP * 2).min(self.len);
        if hi <= lo + BLK {
            return centre.min(self.len);
        }
        let env = |b: usize| -> f32 {
            let s = lo + b * BLK;
            x[s..(s + BLK).min(self.len)]
                .iter()
                .fold(0.0f32, |m, v| m.max(v.abs()))
        };
        let blocks = (hi - lo) / BLK;
        let (mut best, mut best_v) = (0, 0.0f32);
        for b in 0..blocks {
            let v = env(b);
            if v > best_v {
                best_v = v;
                best = b;
            }
        }
        // The level right before the attack (the quietest block before the peak).
        let floor = (0..best).map(env).fold(best_v, f32::min);
        let thresh = floor + (best_v - floor) * 0.12;
        let mut b = best;
        while b > 0 && env(b - 1) > thresh {
            b -= 1;
        }
        // Start a hair (~1 block) before the rise.
        lo + b.saturating_sub(1) * BLK
    }
}

// ---- grid / equal --------------------------------------------------------------------------

/// Grid chop resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridStep {
    HalfBeat,
    Beat,
    TwoBeats,
    Bar,
    TwoBars,
    FourBars,
}

/// Grid points (seconds) at `step` in `[t0, t1)`.
pub fn grid_points(grid: &BeatGrid, t0: f64, t1: f64, step: GridStep) -> Vec<f64> {
    let mut out = Vec::new();
    if t1 <= t0 {
        return out;
    }
    let bpb = grid.beats_per_bar.max(1) as i64;
    let every = match step {
        GridStep::HalfBeat | GridStep::Beat => 1,
        GridStep::TwoBeats => 2,
        GridStep::Bar => bpb,
        GridStep::TwoBars => 2 * bpb,
        GridStep::FourBars => 4 * bpb,
    };
    let first = grid.beat_index_at(t0) - 1;
    let last = grid.beat_index_at(t1) + 1;
    let down = grid.downbeat as i64;
    let push = |out: &mut Vec<f64>, t: f64| {
        if t >= t0 - 1e-9 && t < t1 {
            out.push(t);
        }
    };
    for i in first..=last {
        if (i - down).rem_euclid(every) != 0 {
            continue;
        }
        let t = grid.beat_time(i);
        push(&mut out, t);
        if step == GridStep::HalfBeat {
            push(&mut out, 0.5 * (t + grid.beat_time(i + 1)));
        }
    }
    out.sort_by(f64::total_cmp);
    out
}

/// Starts of `n` equal parts of `[start, end)` (the first one is `start`).
pub fn equal_points(start: usize, end: usize, n: usize) -> Vec<usize> {
    let n = n.max(1);
    let len = end.saturating_sub(start);
    (0..n).map(|i| start + i * len / n).collect()
}

// ---- slices --------------------------------------------------------------------------------

/// Contiguous slices of `[start, end)` cut at `points` (any order; points outside the range
/// are dropped). A slice shorter than `min_len` frames is merged into its neighbour.
pub fn slices_from_points(
    points: &[usize],
    start: usize,
    end: usize,
    min_len: usize,
) -> Vec<Slice> {
    if end <= start {
        return Vec::new();
    }
    let mut cuts: Vec<usize> = points
        .iter()
        .copied()
        .filter(|&p| p > start && p < end)
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut edges = vec![start];
    for c in cuts {
        if c - *edges.last().unwrap_or(&start) >= min_len.max(1) {
            edges.push(c);
        }
    }
    // The tail must be long enough too.
    while edges.len() > 1 && end - edges[edges.len() - 1] < min_len {
        edges.pop();
    }
    edges.push(end);
    edges
        .windows(2)
        .map(|w| Slice {
            start: w[0],
            end: w[1],
        })
        .collect()
}

/// Nearest zero crossing (of the channel sum) to `at`, within `radius` frames; `at` when none.
pub fn snap_to_zero(buf: &AudioBuffer, at: usize, radius: usize) -> usize {
    let frames = buf.frames();
    if frames < 2 || buf.channels.is_empty() {
        return at;
    }
    let at = at.min(frames - 1);
    let v = |i: usize| buf.channels.iter().map(|c| c[i]).sum::<f32>();
    let lo = at.saturating_sub(radius).max(1);
    let hi = (at + radius).min(frames - 1);
    let mut best: Option<usize> = None;
    let mut prev = v(lo - 1);
    for i in lo..=hi {
        let cur = v(i);
        if (prev <= 0.0 && cur >= 0.0) || (prev >= 0.0 && cur <= 0.0) {
            let idx = if cur.abs() <= prev.abs() { i } else { i - 1 };
            if best.is_none_or(|b| idx.abs_diff(at) < b.abs_diff(at)) {
                best = Some(idx);
            }
        }
        prev = cur;
    }
    best.unwrap_or(at)
}

/// Copy `slice` out of `buf` with a linear fade in / out of the given lengths (frames).
pub fn render_slice(
    buf: &AudioBuffer,
    slice: Slice,
    fade_in: usize,
    fade_out: usize,
) -> AudioBuffer {
    let mut out = buf.slice(slice.start, slice.end);
    let n = out.frames();
    out.fade(fade_in.min(n / 2), fade_out.min(n / 2));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::BeatGrid;

    const SR: u32 = 44_100;

    /// Silence with decaying noise bursts at `times` (seconds) with the given gains.
    fn bursts(secs: f64, hits: &[(f64, f32)]) -> AudioBuffer {
        let mut x = vec![0.0f32; (secs * SR as f64) as usize];
        let mut s = 12345u32;
        for &(t, g) in hits {
            let start = (t * SR as f64) as usize;
            for k in 0..(0.15 * SR as f64) as usize {
                let i = start + k;
                if i >= x.len() {
                    break;
                }
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let n = (s as f32 / u32::MAX as f32) * 2.0 - 1.0;
                let tt = k as f32 / SR as f32;
                x[i] += g * (-tt * 30.0).exp() * (0.6 * n + 0.4 * (tt * 900.0).sin());
            }
        }
        AudioBuffer::new(SR, vec![x.clone(), x])
    }

    #[test]
    fn finds_synthetic_transients() {
        let truth = [0.5, 1.0, 1.25, 2.0, 2.6, 3.1];
        let hits: Vec<(f64, f32)> = truth.iter().map(|&t| (t, 0.8)).collect();
        let buf = bursts(4.0, &hits);
        let curve = OnsetCurve::compute(&buf, 0, buf.frames());
        let found = curve.pick(SR, TransientParams::default());
        assert_eq!(found.len(), truth.len(), "{found:?}");
        for (f, t) in found.iter().zip(truth) {
            let ft = *f as f64 / SR as f64;
            // Just before the attack, never after it.
            assert!(ft <= t + 0.0005 && ft > t - 0.006, "{ft} vs {t}");
        }
    }

    #[test]
    fn sensitivity_and_gap() {
        // Two loud hits and two quiet ghost notes.
        let buf = bursts(3.0, &[(0.4, 0.9), (0.9, 0.04), (1.6, 0.9), (2.2, 0.03)]);
        let curve = OnsetCurve::compute(&buf, 0, buf.frames());
        let low = curve.pick(
            SR,
            TransientParams {
                sensitivity: 0.0,
                min_gap_secs: 0.05,
            },
        );
        let high = curve.pick(
            SR,
            TransientParams {
                sensitivity: 1.0,
                min_gap_secs: 0.05,
            },
        );
        assert_eq!(low.len(), 2, "{low:?}");
        assert!(high.len() >= 4, "{high:?}");
        // A big minimum gap swallows close hits.
        let close = bursts(2.0, &[(0.3, 0.8), (0.36, 0.8), (1.0, 0.8)]);
        let c = OnsetCurve::compute(&close, 0, close.frames());
        let wide = c.pick(
            SR,
            TransientParams {
                sensitivity: 0.6,
                min_gap_secs: 0.2,
            },
        );
        assert_eq!(wide.len(), 2, "{wide:?}");
    }

    #[test]
    fn transients_in_a_range_and_silence() {
        let buf = bursts(4.0, &[(0.5, 0.8), (1.5, 0.8), (2.5, 0.8), (3.5, 0.8)]);
        let (a, b) = (SR as usize, 3 * SR as usize);
        let curve = OnsetCurve::compute(&buf, a, b);
        let found = curve.pick(SR, TransientParams::default());
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|&f| f > a && f < b));
        let silent = AudioBuffer::silence(SR, 2, SR as usize * 2);
        let c = OnsetCurve::compute(&silent, 0, silent.frames());
        assert!(c.pick(SR, TransientParams::default()).is_empty());
    }

    #[test]
    fn equal_parts() {
        let p = equal_points(100, 1100, 4);
        assert_eq!(p, vec![100, 350, 600, 850]);
        let s = slices_from_points(&p, 100, 1100, 1);
        assert_eq!(s.len(), 4);
        assert_eq!(
            s[0],
            Slice {
                start: 100,
                end: 350
            }
        );
        assert_eq!(s[3].end, 1100);
        assert!(s.windows(2).all(|w| w[0].end == w[1].start));
    }

    #[test]
    fn slices_merge_short_and_drop_outside() {
        let s = slices_from_points(&[50, 5, 500, 505, 2000, 990], 0, 1000, 20);
        // 5 is too close to the start, 505 to 500, 990 to the end, 2000 is outside.
        assert_eq!(
            s,
            vec![
                Slice { start: 0, end: 50 },
                Slice {
                    start: 50,
                    end: 500
                },
                Slice {
                    start: 500,
                    end: 1000
                }
            ]
        );
        assert!(slices_from_points(&[], 10, 10, 1).is_empty());
        assert_eq!(slices_from_points(&[], 0, 10, 1).len(), 1);
    }

    fn grid_120() -> BeatGrid {
        BeatGrid {
            bpm: 120.0,
            beats: (0..32).map(|i| 0.25 + i as f64 * 0.5).collect(),
            beats_per_bar: 4,
            downbeat: 1,
        }
    }

    #[test]
    fn grid_steps() {
        let g = grid_120();
        let beats = grid_points(&g, 0.25, 4.25, GridStep::Beat);
        assert_eq!(beats.len(), 8);
        assert!((beats[1] - 0.75).abs() < 1e-9);
        let half = grid_points(&g, 0.25, 4.25, GridStep::HalfBeat);
        assert_eq!(half.len(), 16);
        let two = grid_points(&g, 0.25, 4.25, GridStep::TwoBeats);
        // Aligned to the downbeat (beat 1 at 0.75 s).
        assert_eq!(two, vec![0.75, 1.75, 2.75, 3.75]);
        let bars = grid_points(&g, 0.0, 8.0, GridStep::Bar);
        assert_eq!(bars, vec![0.75, 2.75, 4.75, 6.75]);
        let two_bars = grid_points(&g, 0.0, 8.0, GridStep::TwoBars);
        assert_eq!(two_bars, vec![0.75, 4.75]);
        assert!(grid_points(&g, 3.0, 3.0, GridStep::Beat).is_empty());
    }

    #[test]
    fn zero_snap_and_render() {
        let x: Vec<f32> = (0..1000)
            .map(|i| (i as f32 * std::f32::consts::TAU / 100.0).sin())
            .collect();
        let buf = AudioBuffer::new(SR, vec![x]);
        // Zero crossings every 50 frames.
        assert_eq!(snap_to_zero(&buf, 260, 30), 250);
        assert_eq!(snap_to_zero(&buf, 240, 30), 250);
        assert_eq!(snap_to_zero(&buf, 225, 5), 225);
        let r = render_slice(
            &buf,
            Slice {
                start: 250,
                end: 450,
            },
            10,
            20,
        );
        assert_eq!(r.frames(), 200);
        assert_eq!(r.channels[0][0], 0.0);
        assert!(r.channels[0][199].abs() < 1e-6);
    }
}
