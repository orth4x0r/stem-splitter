//! Full-rate onset (transient) detection, shared by the time-stretcher (phase resets) and the
//! one-shot extractor (hit segmentation).
//!
//! Log-magnitude spectral flux with a SuperFlux-style maximum filter over the previous frame
//! (Böck & Widmer, "Maximum filter vibrato suppression for onset detection", DAFx 2013,
//! reimplemented from the paper), then the usual peak picking: a local maximum that also clears
//! a moving average by a margin, with a minimum gap between onsets.

// Frame / bin loops index several arrays at once; explicit indices read better here.
#![allow(clippy::needless_range_loop)]

use realfft::RealFftPlanner;

use crate::AudioBuffer;
use crate::stft::hann_window;

/// One detected onset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Onset {
    /// Approximate frame (sample index) of the attack, accurate to about a quarter window
    /// (±4 ms at 44.1 kHz). [`crate::oneshot`] refines it in the time domain.
    pub pos: usize,
    /// Peak-picked flux value, normalised so a typical onset is around 1..10.
    pub strength: f32,
}

/// Detection settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnsetOptions {
    /// `0..=1`, higher finds quieter onsets.
    pub sensitivity: f32,
    /// Minimum time between two onsets, seconds.
    pub min_gap: f32,
}

impl Default for OnsetOptions {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            min_gap: 0.03,
        }
    }
}

/// Analysis window: ~23 ms. Hop: a quarter of it.
fn fft_size(sr: u32) -> usize {
    if sr > 64_000 { 2048 } else { 1024 }
}

/// Mono downmix.
pub(crate) fn mono(buf: &AudioBuffer) -> Vec<f32> {
    let n = buf.n_channels().max(1) as f32;
    let mut out = vec![0.0f32; buf.frames()];
    for ch in &buf.channels {
        for (o, s) in out.iter_mut().zip(ch) {
            *o += s / n;
        }
    }
    out
}

/// Spectral-flux onset strength per hop and the hop size in samples.
pub fn onset_flux(buf: &AudioBuffer) -> (Vec<f32>, usize) {
    let x = mono(buf);
    let n = fft_size(buf.sample_rate);
    let hop = n / 4;
    if x.is_empty() {
        return (Vec::new(), hop);
    }
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak < 1e-6 {
        return (vec![0.0; x.len() / hop + 1], hop);
    }
    let gain = 1.0 / peak;
    let win = hann_window(n);
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
    let mut inp = fft.make_input_vec();
    let mut spec = fft.make_output_vec();
    let mut scratch = fft.make_scratch_vec();
    let bins = n / 2 + 1;
    let mut prev_max = vec![0.0f32; bins];
    let mut cur = vec![0.0f32; bins];
    let n_frames = x.len() / hop + 1;
    let mut flux = vec![0.0f32; n_frames];
    for (f, fl) in flux.iter_mut().enumerate() {
        // Frame centred on f * hop.
        let start = (f * hop) as isize - (n / 2) as isize;
        for (i, v) in inp.iter_mut().enumerate() {
            let j = start + i as isize;
            *v = if j >= 0 && (j as usize) < x.len() {
                x[j as usize] * gain * win[i]
            } else {
                0.0
            };
        }
        let _ = fft.process_with_scratch(&mut inp, &mut spec, &mut scratch);
        for (c, s) in cur.iter_mut().zip(&spec) {
            *c = (1.0 + 100.0 * s.norm()).ln();
        }
        if f > 0 {
            *fl = cur
                .iter()
                .zip(&prev_max)
                .map(|(c, p)| (c - p).max(0.0))
                .sum();
        }
        // Max filter over ±1 bin for the next frame's reference.
        for k in 0..bins {
            let lo = k.saturating_sub(1);
            let hi = (k + 1).min(bins - 1);
            prev_max[k] = cur[lo..=hi].iter().fold(0.0f32, |m, v| m.max(*v));
        }
    }
    // Scale: mean of the non-zero flux, so thresholds are level independent.
    let nz: Vec<f32> = flux.iter().copied().filter(|v| *v > 1e-6).collect();
    let mean = if nz.is_empty() {
        1.0
    } else {
        nz.iter().sum::<f32>() / nz.len() as f32
    };
    if mean > 1e-9 {
        flux.iter_mut().for_each(|v| *v /= mean);
    }
    (flux, hop)
}

/// Detect onsets in `buf`.
pub fn detect_onsets(buf: &AudioBuffer, opts: &OnsetOptions) -> Vec<Onset> {
    let (flux, hop) = onset_flux(buf);
    let fps = buf.sample_rate.max(1) as f32 / hop as f32;
    let frames = |s: f32| ((s * fps).round() as usize).max(1);
    let (pre_max, post_max) = (frames(0.03), frames(0.03));
    let (pre_avg, post_avg) = (frames(0.10), frames(0.07));
    let gap = frames(opts.min_gap);
    // Margin over the moving average: sensitivity 0 -> 3.0, 1 -> 0.3.
    let delta = 3.0 - 2.7 * opts.sensitivity.clamp(0.0, 1.0);
    let n = flux.len();
    let mut prefix = vec![0.0f64; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + flux[i] as f64;
    }
    let mut out: Vec<Onset> = Vec::new();
    let mut last: Option<usize> = None;
    for f in 1..n {
        let v = flux[f];
        if v <= 0.0 {
            continue;
        }
        let a = f.saturating_sub(pre_max);
        let b = (f + post_max + 1).min(n);
        if flux[a..b].iter().any(|&w| w > v) {
            continue;
        }
        // Plateaus: only the first frame of equal maxima.
        if flux[a..f].contains(&v) {
            continue;
        }
        let a = f.saturating_sub(pre_avg);
        let b = (f + post_avg + 1).min(n);
        let avg = ((prefix[b] - prefix[a]) / (b - a) as f64) as f32;
        if v < avg + delta {
            continue;
        }
        if let Some(l) = last
            && f - l < gap
        {
            continue;
        }
        last = Some(f);
        // The flux fires once the attack is inside the window's main lobe, roughly a quarter
        // window after the centre.
        let pos = f * hop + hop;
        out.push(Onset { pos, strength: v });
    }
    out
}

/// Synthetic test signals shared by the DSP tests.
#[cfg(test)]
pub(crate) mod synth {
    /// Tiny deterministic PRNG, uniform in `[-1, 1)`.
    pub struct Rng(pub u64);
    impl Rng {
        pub fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decaying noise bursts at the given seconds.
    fn bursts(sr: u32, len: f32, at: &[f32]) -> AudioBuffer {
        let mut x = vec![0.0f32; (len * sr as f32) as usize];
        let mut rng = synth::Rng(7);
        for &t in at {
            let s = (t * sr as f32) as usize;
            for i in 0..(0.08 * sr as f32) as usize {
                if s + i < x.len() {
                    x[s + i] += rng.next() * (-(i as f32) / (0.015 * sr as f32)).exp() * 0.5;
                }
            }
        }
        AudioBuffer::new(sr, vec![x])
    }

    #[test]
    fn finds_bursts() {
        let sr = 44_100;
        let at = [0.25f32, 0.5, 0.875, 1.0, 1.6];
        let buf = bursts(sr, 2.0, &at);
        let on = detect_onsets(&buf, &OnsetOptions::default());
        assert_eq!(on.len(), at.len(), "{on:?}");
        for (o, t) in on.iter().zip(at) {
            let err = o.pos as f32 / sr as f32 - t;
            assert!(err.abs() < 0.006, "onset at {t}: err {err}");
        }
    }

    #[test]
    fn silence_has_none() {
        let buf = AudioBuffer::silence(44_100, 2, 44_100);
        assert!(detect_onsets(&buf, &OnsetOptions::default()).is_empty());
    }
}
