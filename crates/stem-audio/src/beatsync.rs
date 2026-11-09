//! Beat-synchronous features shared by section and chord detection: per-beat chroma (full
//! range and bass), log band energies, and the RMS of the harmony and the rhythm parts.
//!
//! Frames are averaged between consecutive beats of the grid, so everything downstream works
//! in musical time. Without a grid a steady 0.5 s pseudo-beat stands in (4 to a "bar").

use std::f32::consts::PI;

use crate::AudioBuffer;
use crate::analysis::{BeatGrid, for_each_spectrum, mono_low_rate};

/// Log-spaced bands in the timbre vector.
pub(crate) const N_BANDS: usize = 8;

const CHROMA_FFT: usize = 4096;
const CHROMA_HOP: usize = 1024;
const BAND_FFT: usize = 1024;
const BAND_HOP: usize = 512;
/// Pitch range for the chroma: C2 .. ~C7.
const CHROMA_LO: f32 = 65.0;
const CHROMA_HI: f32 = 2100.0;
/// Bass chroma: up to ~B3.
const BASS_HI: f32 = 250.0;
/// Pseudo-beat when there is no grid.
const PSEUDO_BEAT: f64 = 0.5;

pub(crate) struct BeatFeatures {
    /// `n + 1` beat boundaries in seconds, from 0 to the end of the track.
    pub times: Vec<f64>,
    /// Beat `i` starts a bar.
    pub bar: Vec<bool>,
    pub beats_per_bar: usize,
    /// Pitch-class magnitude, 65 Hz .. 2.1 kHz.
    pub chroma: Vec<[f32; 12]>,
    /// Pitch-class magnitude under 250 Hz (the bass note).
    pub bass: Vec<[f32; 12]>,
    /// Natural log of the mean band energies.
    pub bands: Vec<[f32; N_BANDS]>,
    /// RMS of the harmony buffer.
    pub harm_rms: Vec<f32>,
    /// RMS of the rhythm buffer.
    pub drum_rms: Vec<f32>,
}

impl BeatFeatures {
    pub fn len(&self) -> usize {
        self.times.len() - 1
    }
}

/// Beat boundaries covering `[0, dur]` and their bar flags.
pub(crate) fn beat_bounds(grid: Option<&BeatGrid>, dur: f64) -> (Vec<f64>, Vec<bool>, usize) {
    let mut times = Vec::new();
    let mut bar = Vec::new();
    let bpb;
    match grid {
        Some(g) if g.period() > 0.05 => {
            bpb = g.beats_per_bar.max(1);
            let i0 = g.beat_index_at(0.0);
            let i1 = g.beat_index_at(dur);
            for i in i0..=i1 {
                let t = g.beat_time(i).max(0.0);
                if t >= dur {
                    break;
                }
                // Drop a sliver before the first full beat (keep the bar flag of what follows).
                if let Some(&last) = times.last()
                    && t - last < 0.05
                {
                    *bar.last_mut().unwrap() |= g.is_downbeat(i);
                    continue;
                }
                times.push(t);
                bar.push(g.is_downbeat(i));
            }
        }
        _ => {
            bpb = 4;
            let n = (dur / PSEUDO_BEAT).ceil() as usize;
            for i in 0..n {
                times.push(i as f64 * PSEUDO_BEAT);
                bar.push(i % 4 == 0);
            }
        }
    }
    if times.is_empty() {
        times.push(0.0);
        bar.push(true);
    }
    // A last beat that's only a sliver merges into the one before.
    while times.len() > 1 && dur - times[times.len() - 1] < 0.05 {
        times.pop();
        bar.pop();
    }
    times.push(dur.max(times[times.len() - 1] + 1e-3));
    (times, bar, bpb)
}

/// Binomial-filtered decimation by 2.
fn halve(x: &[f32]) -> Vec<f32> {
    let m = x.len() / 2;
    if x.is_empty() {
        return Vec::new();
    }
    let at = |i: isize| x[i.clamp(0, x.len() as isize - 1) as usize];
    (0..m as isize)
        .map(|j| {
            let i = 2 * j;
            (at(i - 1) + 3.0 * at(i) + 3.0 * at(i + 1) + at(i + 2)) * 0.125
        })
        .collect()
}

/// Spread per-frame values (frame `f` centred at `f / fps` s) over beats: the mean of the
/// frames whose centre falls in each beat, or the nearest frame for a beat with none.
fn per_beat<const N: usize>(frames: &[[f32; N]], fps: f64, times: &[f64]) -> Vec<[f32; N]> {
    let n = times.len() - 1;
    let mut out = vec![[0.0f32; N]; n];
    if frames.is_empty() {
        return out;
    }
    let mut counts = vec![0u32; n];
    let mut b = 0;
    for (f, v) in frames.iter().enumerate() {
        let t = f as f64 / fps;
        while b + 1 < n && t >= times[b + 1] {
            b += 1;
        }
        if t < times[0] || t >= times[n] {
            continue;
        }
        for k in 0..N {
            out[b][k] += v[k];
        }
        counts[b] += 1;
    }
    for i in 0..n {
        if counts[i] > 0 {
            let c = counts[i] as f32;
            out[i].iter_mut().for_each(|v| *v /= c);
        } else {
            let mid = 0.5 * (times[i] + times[i + 1]);
            let f = ((mid * fps).round() as usize).min(frames.len() - 1);
            out[i] = frames[f];
        }
    }
    out
}

/// RMS of all channels of `buf` within each beat.
fn rms_per_beat(buf: &AudioBuffer, times: &[f64]) -> Vec<f32> {
    let sr = buf.sample_rate as f64;
    let frames = buf.frames();
    let ch = buf.n_channels().max(1) as f64;
    times
        .windows(2)
        .map(|w| {
            let a = ((w[0] * sr) as usize).min(frames);
            let b = ((w[1] * sr) as usize).min(frames);
            if b <= a {
                return 0.0;
            }
            let s: f64 = buf
                .channels
                .iter()
                .map(|c| {
                    c[a..b]
                        .iter()
                        .map(|v| (*v as f64) * (*v as f64))
                        .sum::<f64>()
                })
                .sum();
            (s / ((b - a) as f64 * ch)).sqrt() as f32
        })
        .collect()
}

/// Per-frame chroma and bass chroma of a mono signal at `sr`, tuning-compensated.
fn chroma_frames(x: &[f32], sr: f32) -> (Vec<[f32; 12]>, Vec<[f32; 12]>, f64) {
    let fps = sr as f64 / CHROMA_HOP as f64;
    if x.len() < CHROMA_FFT / 4 {
        return (Vec::new(), Vec::new(), fps);
    }
    let n_bins = CHROMA_FFT / 2 + 1;
    let bin_hz = sr / CHROMA_FFT as f32;
    let k_lo = ((CHROMA_LO / bin_hz).ceil() as usize).max(1);
    let k_hi = ((CHROMA_HI.min(sr * 0.45) / bin_hz).floor() as usize).min(n_bins - 2);
    if k_hi <= k_lo {
        return (Vec::new(), Vec::new(), fps);
    }
    // Pass 1: keep the spectra (small), estimate tuning from spectral peaks.
    let mut spectra: Vec<Vec<f32>> = Vec::new();
    let (mut tc, mut ts) = (0.0f32, 0.0f32);
    for_each_spectrum(x, CHROMA_FFT, CHROMA_HOP, |_, mags| {
        for k in k_lo..=k_hi {
            let (a, b, c) = (mags[k - 1], mags[k], mags[k + 1]);
            if b > a && b >= c && b > 1e-4 {
                let d = a - 2.0 * b + c;
                let off = if d.abs() > 1e-12 {
                    0.5 * (a - c) / d
                } else {
                    0.0
                };
                let midi = 69.0 + 12.0 * (((k as f32 + off) * bin_hz) / 440.0).log2();
                let dev = (midi - midi.round()) * 2.0 * PI;
                tc += b * dev.cos();
                ts += b * dev.sin();
            }
        }
        spectra.push(mags[k_lo..=k_hi].to_vec());
    });
    let tuning = if tc.abs() + ts.abs() > 0.0 {
        ts.atan2(tc) / (2.0 * PI)
    } else {
        0.0
    };
    // Bin -> (pitch class, triangular weight, is bass).
    let map: Vec<(usize, f32, bool)> = (k_lo..=k_hi)
        .map(|k| {
            let f = k as f32 * bin_hz;
            let midi = 69.0 + 12.0 * (f / 440.0).log2() - tuning;
            let near = midi.round();
            let w = (1.0 - 2.0 * (midi - near).abs()).max(0.0);
            ((near as i32).rem_euclid(12) as usize, w, f < BASS_HI)
        })
        .collect();
    let mut chroma = Vec::with_capacity(spectra.len());
    let mut bass = Vec::with_capacity(spectra.len());
    for m in &spectra {
        let mut c = [0.0f32; 12];
        let mut b = [0.0f32; 12];
        for (v, &(pc, w, low)) in m.iter().zip(&map) {
            c[pc] += w * v;
            if low {
                b[pc] += w * v;
            }
        }
        chroma.push(c);
        bass.push(b);
    }
    (chroma, bass, fps)
}

/// Log energies in [`N_BANDS`] log-spaced bands, 60 Hz .. 10 kHz.
fn band_frames(x: &[f32], sr: f32) -> (Vec<[f32; N_BANDS]>, f64) {
    let fps = sr as f64 / BAND_HOP as f64;
    if x.len() < BAND_FFT / 4 {
        return (Vec::new(), fps);
    }
    let n_bins = BAND_FFT / 2 + 1;
    let bin_hz = sr / BAND_FFT as f32;
    let (lo, hi) = (60.0f32, 10_000.0f32.min(sr * 0.48));
    let band_of: Vec<Option<usize>> = (0..n_bins)
        .map(|k| {
            let f = k as f32 * bin_hz;
            (f >= lo && f <= hi).then(|| {
                (((f / lo).ln() / (hi / lo).ln() * N_BANDS as f32) as usize).min(N_BANDS - 1)
            })
        })
        .collect();
    let mut out = Vec::new();
    for_each_spectrum(x, BAND_FFT, BAND_HOP, |_, mags| {
        let mut e = [0.0f32; N_BANDS];
        for (m, b) in mags.iter().zip(&band_of) {
            if let Some(b) = b {
                e[*b] += m * m;
            }
        }
        out.push(e);
    });
    (out, fps)
}

/// Beat-synchronous features: chroma / bands from `harmony`, RMS from both.
pub(crate) fn beat_features(
    rhythm: &AudioBuffer,
    harmony: &AudioBuffer,
    grid: Option<&BeatGrid>,
) -> BeatFeatures {
    let dur = harmony.duration_secs().max(rhythm.duration_secs());
    let (times, bar, beats_per_bar) = beat_bounds(grid, dur);
    let (x, sr) = mono_low_rate(harmony);
    let (bands, band_fps) = band_frames(&x, sr);
    // Chroma at ~11 kHz: same frequency resolution for a quarter of the FFT work.
    let (cx, csr) = if sr > 16_000.0 {
        (halve(&x), sr / 2.0)
    } else {
        (x, sr)
    };
    let (chroma, bass, chroma_fps) = chroma_frames(&cx, csr);
    let bands = per_beat(&bands, band_fps, &times)
        .into_iter()
        .map(|b| b.map(|e| (e + 1e-6).ln()))
        .collect();
    BeatFeatures {
        chroma: per_beat(&chroma, chroma_fps, &times),
        bass: per_beat(&bass, chroma_fps, &times),
        bands,
        harm_rms: rms_per_beat(harmony, &times),
        drum_rms: rms_per_beat(rhythm, &times),
        times,
        bar,
        beats_per_bar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_cover_the_track() {
        let g = BeatGrid {
            bpm: 120.0,
            beats: (0..20).map(|i| 0.3 + i as f64 * 0.5).collect(),
            downbeat: 1,
            beats_per_bar: 4,
        };
        let (t, bar, bpb) = beat_bounds(Some(&g), 12.0);
        assert_eq!(bpb, 4);
        assert_eq!(t[0], 0.0);
        assert_eq!(*t.last().unwrap(), 12.0);
        assert_eq!(t.len(), bar.len() + 1);
        assert!(t.windows(2).all(|w| w[1] > w[0]));
        // 0.8 s is beat 1, the downbeat.
        let i = t.iter().position(|&x| (x - 0.8).abs() < 1e-9).unwrap();
        assert!(bar[i]);
        assert!(bar[i + 4]);
        assert!(!bar[i + 1]);
        let (t, bar, _) = beat_bounds(None, 3.2);
        assert_eq!(t.len(), 8);
        assert!(bar[0] && bar[4]);
    }
}
