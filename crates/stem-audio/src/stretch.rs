//! Time-stretch, pitch-shift, varispeed and reverse, in pure Rust.
//!
//! Two stretchers, picked per stem ([`Algorithm`]):
//!
//! - **Tonal**: a phase vocoder with identity phase locking (J. Laroche & M. Dolson, "Improved
//!   phase vocoder time-scale modification of audio", IEEE TSAP 7(3), 1999; reimplemented from
//!   the paper). Spectral peaks get their phase advanced by their instantaneous frequency; every
//!   other bin keeps its phase offset to the peak whose region it lies in, which keeps partials
//!   coherent and removes most of the classic "phasiness". The phase rotation is computed once
//!   on the channel sum and applied to every channel, so the stereo image (inter-channel phase)
//!   is preserved exactly.
//! - **Percussive**: WSOLA (W. Verhelst & M. Roelands, ICASSP 1993): overlap-add of 40 ms
//!   waveform segments, each shifted by up to ±12 ms to best continue the previous one. It
//!   copies waveform rather than resynthesising spectra, so drums keep their punch.
//!
//! Both are transient-aware: onsets are detected first ([`crate::onsets`]), and every frame
//! whose window contains an onset is played 1:1 (analysis hop = synthesis hop) with the phase
//! vocoder's phases reset to the input's. That region is therefore reproduced exactly, with no
//! pre-echo or smear; the time lost or gained is paid back gradually over the following frames
//! by a drift-correcting time map.
//!
//! Pitch shift = stretch by the pitch ratio, then resample back to the original length.
//! Varispeed = resample only (pitch follows speed, the classic sampler / tape sound).

// Frame / bin loops index several arrays at once; explicit indices read better here.
#![allow(clippy::needless_range_loop)]

use std::f32::consts::{PI, TAU};
use std::sync::atomic::{AtomicBool, Ordering};

use realfft::RealFftPlanner;
use realfft::num_complex::Complex32;

use crate::onsets::{OnsetOptions, detect_onsets, mono};
use crate::stft::hann_window;
use crate::{AudioBuffer, AudioError, Result};

/// Which stretcher to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Algorithm {
    /// Percussive material gets WSOLA, everything else the phase vocoder ([`is_percussive`]).
    #[default]
    Auto,
    /// Phase vocoder with phase locking: vocals, keys, bass, pads, full mixes.
    Tonal,
    /// WSOLA: drums and other percussive stems.
    Percussive,
}

/// How tempo and pitch relate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpeedMode {
    /// Tempo and pitch independent (time-stretch + pitch-shift).
    #[default]
    Keylock,
    /// Resample only: pitch follows speed, like a sampler or a turntable.
    Varispeed,
}

/// A complete render: tempo, pitch, mode and reverse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Playback speed: 1.0 unchanged, 1.25 = 25 % faster (shorter).
    pub speed: f64,
    /// Pitch shift in semitones; the fraction is cents. In varispeed mode it is folded into
    /// the speed (+12 st = double speed).
    pub semitones: f64,
    pub mode: SpeedMode,
    pub algorithm: Algorithm,
    pub reverse: bool,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            speed: 1.0,
            semitones: 0.0,
            mode: SpeedMode::Keylock,
            algorithm: Algorithm::Auto,
            reverse: false,
        }
    }
}

impl Transform {
    /// Overall resampling ratio in varispeed mode.
    pub fn varispeed_ratio(&self) -> f64 {
        self.speed * semitone_ratio(self.semitones)
    }
    /// Output duration relative to the input.
    pub fn length_factor(&self) -> f64 {
        match self.mode {
            SpeedMode::Keylock => 1.0 / self.speed,
            SpeedMode::Varispeed => 1.0 / self.varispeed_ratio(),
        }
    }
    /// Pitch change of the output in semitones.
    pub fn pitch_semitones(&self) -> f64 {
        match self.mode {
            SpeedMode::Keylock => self.semitones,
            SpeedMode::Varispeed => 12.0 * self.varispeed_ratio().log2(),
        }
    }
    /// Tempo multiplier of the output (1.1 = 10 % faster).
    pub fn tempo_factor(&self) -> f64 {
        1.0 / self.length_factor()
    }
    pub fn is_identity(&self) -> bool {
        !self.reverse && (self.speed - 1.0).abs() < 1e-6 && self.semitones.abs() < 1e-6
    }

    /// File-name suffix such as `+3st 95bpm`, `-2.5st 110%`, `vari +1st`, `rev`. `bpm` is the
    /// source tempo, if known. Empty for the identity transform.
    pub fn name_suffix(&self, bpm: Option<f32>) -> String {
        let mut parts = Vec::new();
        if self.mode == SpeedMode::Varispeed && !self.is_identity_speed() {
            parts.push("vari".to_owned());
        }
        let st = self.pitch_semitones();
        if st.abs() >= 0.005 {
            parts.push(format!("{}st", signed_amount(st)));
        }
        let tempo = self.tempo_factor();
        if (tempo - 1.0).abs() >= 0.0005 {
            match bpm {
                Some(b) => parts.push(format!("{}bpm", trim_num(b as f64 * tempo, 1))),
                None => parts.push(format!("{}%", trim_num(tempo * 100.0, 1))),
            }
        }
        if self.reverse {
            parts.push("rev".to_owned());
        }
        parts.join(" ")
    }

    fn is_identity_speed(&self) -> bool {
        (self.varispeed_ratio() - 1.0).abs() < 1e-6
    }
}

/// `2^(st/12)`.
pub fn semitone_ratio(st: f64) -> f64 {
    (st / 12.0).exp2()
}

/// "+3", "-2.5", "+0.25" (up to two decimals, no trailing zeros).
fn signed_amount(v: f64) -> String {
    let s = trim_num(v.abs(), 2);
    if v < 0.0 {
        format!("-{s}")
    } else {
        format!("+{s}")
    }
}

fn trim_num(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

/// Progress callback (`0..=1`) and cancel flag for the long-running functions.
pub struct Control<'a> {
    pub progress: &'a (dyn Fn(f32) + Sync),
    pub cancel: &'a AtomicBool,
}

impl Control<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    /// A sub-range `[a, b]` of the overall progress.
    fn report(&self, a: f32, b: f32, f: f32) {
        (self.progress)(a + (b - a) * f.clamp(0.0, 1.0));
    }
}

/// Render `t` on `buf`.
pub fn render(buf: &AudioBuffer, t: &Transform, ctl: &Control<'_>) -> Result<AudioBuffer> {
    let mut out = match t.mode {
        SpeedMode::Varispeed => {
            (ctl.progress)(0.1);
            varispeed(buf, t.varispeed_ratio())?
        }
        SpeedMode::Keylock => {
            let p = semitone_ratio(t.semitones);
            // Stretch by p / speed, then resample by p: length / speed, pitch * p.
            let alpha = p / t.speed;
            let stretched = time_stretch_ctl(buf, alpha, t.algorithm, ctl, 0.0, 0.92)?;
            if (p - 1.0).abs() > 1e-9 {
                let mut r = varispeed(&stretched, p)?;
                // Exact target length.
                let target = (buf.frames() as f64 / t.speed).round() as usize;
                for c in &mut r.channels {
                    c.resize(target, 0.0);
                }
                r
            } else {
                stretched
            }
        }
    };
    if ctl.cancelled() {
        return Err(AudioError::Cancelled);
    }
    if t.reverse {
        out = reverse(&out);
    }
    (ctl.progress)(1.0);
    Ok(out)
}

/// Reverse in time.
pub fn reverse(buf: &AudioBuffer) -> AudioBuffer {
    AudioBuffer::new(
        buf.sample_rate,
        buf.channels
            .iter()
            .map(|c| c.iter().rev().copied().collect())
            .collect(),
    )
}

/// Resample so the audio plays `ratio` times faster (and higher) at the same sample rate.
/// Output length is `round(frames / ratio)`.
///
/// Uses the rubato FFT resampler from [`crate::resample`] by relabelling the input's rate as
/// `round(sr * ratio)`: the rounding is at most 0.5 Hz, i.e. below 0.02 cents at 44.1 kHz.
pub fn varispeed(buf: &AudioBuffer, ratio: f64) -> Result<AudioBuffer> {
    if !(ratio.is_finite() && ratio > 0.0) {
        return Err(AudioError::Resample(format!("bad speed ratio {ratio}")));
    }
    let sr = buf.sample_rate;
    let fake = (sr as f64 * ratio).round().max(1.0) as u32;
    let target = (buf.frames() as f64 / ratio).round() as usize;
    let mut out = if fake == sr {
        buf.clone()
    } else {
        let relabelled = AudioBuffer {
            sample_rate: fake,
            channels: buf.channels.clone(),
        };
        crate::resample(&relabelled, sr)?
    };
    for c in &mut out.channels {
        c.resize(target, 0.0);
    }
    out.sample_rate = sr;
    Ok(out)
}

/// Time-stretch by `factor` (output duration / input duration), pitch unchanged.
pub fn time_stretch(
    buf: &AudioBuffer,
    factor: f64,
    algorithm: Algorithm,
    ctl: &Control<'_>,
) -> Result<AudioBuffer> {
    time_stretch_ctl(buf, factor, algorithm, ctl, 0.0, 1.0)
}

/// Pitch-shift by `semitones`, duration unchanged.
pub fn pitch_shift(
    buf: &AudioBuffer,
    semitones: f64,
    algorithm: Algorithm,
    ctl: &Control<'_>,
) -> Result<AudioBuffer> {
    render(
        buf,
        &Transform {
            semitones,
            algorithm,
            ..Transform::default()
        },
        ctl,
    )
}

fn time_stretch_ctl(
    buf: &AudioBuffer,
    factor: f64,
    algorithm: Algorithm,
    ctl: &Control<'_>,
    p0: f32,
    p1: f32,
) -> Result<AudioBuffer> {
    if !(factor.is_finite() && factor > 0.0) {
        return Err(AudioError::Resample(format!("bad stretch factor {factor}")));
    }
    let frames = buf.frames();
    let out_len = (frames as f64 * factor).round() as usize;
    if (factor - 1.0).abs() < 1e-9 || frames == 0 || buf.channels.is_empty() {
        let mut out = buf.clone();
        for c in &mut out.channels {
            c.resize(out_len, 0.0);
        }
        return Ok(out);
    }
    let percussive = match algorithm {
        Algorithm::Tonal => false,
        Algorithm::Percussive => true,
        Algorithm::Auto => is_percussive(buf),
    };
    ctl.report(p0, p1, 0.02);
    let onsets: Vec<usize> = detect_onsets(
        buf,
        &OnsetOptions {
            sensitivity: 0.5,
            min_gap: 0.05,
        },
    )
    .into_iter()
    .map(|o| o.pos)
    .collect();
    ctl.report(p0, p1, 0.06);
    let sub = |f: f32| ctl.report(p0, p1, 0.06 + 0.94 * f);
    let channels = if percussive {
        wsola(buf, factor, &onsets, true, &sub, ctl.cancel)?
    } else {
        phase_vocoder(buf, factor, &onsets, true, &sub, ctl.cancel)?
    };
    Ok(AudioBuffer::new(buf.sample_rate, channels))
}

// ---- time map ------------------------------------------------------------------------------

/// One analysis frame: input centre position, whether it is played 1:1 around an onset, and
/// whether to reset phases there (the first frame of a locked run).
#[derive(Debug, Clone, Copy)]
struct MapFrame {
    center: i64,
    locked: bool,
    reset: bool,
}

/// Input frame centres for synthesis frames at `m * hop`, `m = 0..n_frames`.
///
/// Nominally the input advances by `hop / factor` per frame. Frames whose window
/// (`center ± win / 2`) contains an onset advance by exactly `hop` instead (1:1, so the
/// transient is reproduced as is), the first of them with a phase reset. The resulting drift
/// is corrected gradually; while the drift is large, onsets are not locked (dense hi-hats at a
/// big stretch would otherwise never let the map catch up).
fn time_map(
    n_frames: usize,
    hop: usize,
    win: usize,
    factor: f64,
    onsets: &[usize],
    lock: bool,
) -> Vec<MapFrame> {
    let nominal = hop as f64 / factor;
    let half = (win / 2) as f64;
    let max_drift = win as f64;
    let mut out = Vec::with_capacity(n_frames);
    let mut a = 0.0f64;
    let mut next_onset = 0usize; // first onset that has not entered a window yet
    let mut locked_until: Option<f64> = None; // last onset being played through 1:1
    for m in 0..n_frames {
        let target = m as f64 * nominal;
        if m > 0 {
            if locked_until.is_some_and(|o| o >= a + hop as f64 - half) {
                a += hop as f64;
            } else {
                locked_until = None;
                let corr = ((target - a - nominal) * 0.25).clamp(-0.5 * nominal, 0.5 * nominal);
                a += nominal + corr;
            }
        }
        let was_locked = locked_until.is_some();
        while next_onset < onsets.len() && (onsets[next_onset] as f64) < a - half {
            next_onset += 1;
        }
        let mut first = None;
        while lock && next_onset < onsets.len() && (onsets[next_onset] as f64) < a + half {
            let o = onsets[next_onset] as f64;
            if was_locked || (target - a).abs() < max_drift {
                first.get_or_insert(o);
                locked_until = Some(o);
            }
            next_onset += 1;
        }
        let locked = locked_until.is_some();
        if let (false, Some(o)) = (was_locked, first) {
            // Start the 1:1 run so the onset lands exactly where the global map puts it
            // (`onset * factor` in the output): a small jump, hidden by the phase reset.
            a = ((m * hop) as f64 - o * factor + o).max(0.0);
        }
        out.push(MapFrame {
            center: a.round() as i64,
            locked,
            reset: locked && !was_locked,
        });
    }
    out
}

// ---- phase vocoder -------------------------------------------------------------------------

fn princarg(x: f32) -> f32 {
    x - TAU * ((x + PI) / TAU).floor()
}

/// Copy `len` samples of `src` starting at `start` (may be negative / past the end, zero
/// padded), multiplied by `win`.
fn windowed(src: &[f32], start: i64, win: &[f32], out: &mut [f32]) {
    for (i, (o, w)) in out.iter_mut().zip(win).enumerate() {
        let j = start + i as i64;
        *o = if j >= 0 && (j as usize) < src.len() {
            src[j as usize] * w
        } else {
            0.0
        };
    }
}

fn pv_fft_size(sr: u32) -> usize {
    match sr {
        0..=30_000 => 2048,
        30_001..=64_000 => 4096,
        _ => 8192,
    }
}

pub(crate) fn phase_vocoder(
    buf: &AudioBuffer,
    factor: f64,
    onsets: &[usize],
    lock_transients: bool,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<Vec<Vec<f32>>> {
    let n = pv_fft_size(buf.sample_rate);
    let hop = n / 8;
    let bins = n / 2 + 1;
    let in_len = buf.frames();
    let out_len = (in_len as f64 * factor).round() as usize;
    let n_frames = (out_len + n / 2) / hop + 1;
    let map = time_map(n_frames, hop, n, factor, onsets, lock_transients);
    let win = hann_window(n);
    let mut planner = RealFftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let n_ch = buf.n_channels();

    let mut frame = vec![0.0f32; n];
    let mut specs: Vec<Vec<Complex32>> = (0..n_ch).map(|_| fwd.make_output_vec()).collect();
    let mut scratch = fwd.make_scratch_vec();
    let mut iscratch = inv.make_scratch_vec();
    let mut sum = vec![Complex32::new(0.0, 0.0); bins];
    let mut prev_sum = vec![Complex32::new(0.0, 0.0); bins];
    let mut mag = vec![0.0f32; bins];
    // Synthesis phase minus analysis phase, per bin (the rotation applied).
    let mut delta = vec![0.0f32; bins];
    let mut new_delta = vec![0.0f32; bins];
    let mut peaks: Vec<usize> = Vec::with_capacity(bins / 2);
    let mut rot = vec![Complex32::new(1.0, 0.0); bins];

    let mut out: Vec<Vec<f32>> = vec![vec![0.0f32; out_len + n]; n_ch];
    let mut wss = vec![0.0f32; out_len + n];
    let scale = 1.0 / n as f32;
    let omega: Vec<f32> = (0..bins).map(|k| TAU * k as f32 / n as f32).collect();
    let mut prev_center: Option<i64> = None;

    for (m, mf) in map.iter().enumerate() {
        if m % 64 == 0 {
            if cancel.load(Ordering::Relaxed) {
                return Err(AudioError::Cancelled);
            }
            progress(m as f32 / map.len() as f32);
        }
        let start = mf.center - (n / 2) as i64;
        for (c, spec) in specs.iter_mut().enumerate() {
            windowed(&buf.channels[c], start, &win, &mut frame);
            let _ = fwd.process_with_scratch(&mut frame, spec, &mut scratch);
        }
        for k in 0..bins {
            let s: Complex32 = specs.iter().map(|sp| sp[k]).sum();
            sum[k] = s;
            mag[k] = s.norm_sqr();
        }

        match prev_center {
            Some(pc) if !mf.reset => {
                let ha = (mf.center - pc) as f32;
                // Peaks: local maxima over ±2 bins.
                peaks.clear();
                for k in 2..bins.saturating_sub(2) {
                    let v = mag[k];
                    if v > 1e-20
                        && v > mag[k - 1]
                        && v >= mag[k + 1]
                        && v > mag[k - 2]
                        && v >= mag[k + 2]
                    {
                        peaks.push(k);
                    }
                }
                if peaks.is_empty() {
                    new_delta.copy_from_slice(&delta);
                } else {
                    // Phase propagation at the peaks.
                    let mut pd = Vec::with_capacity(peaks.len());
                    for &k in &peaks {
                        let phi = sum[k].arg();
                        let phi_p = prev_sum[k].arg();
                        let inst = if ha > 0.0 {
                            omega[k] + princarg(phi - phi_p - omega[k] * ha) / ha
                        } else {
                            omega[k]
                        };
                        let psi = delta[k] + phi_p + hop as f32 * inst;
                        pd.push(princarg(psi - phi));
                    }
                    // Identity locking: every bin takes the rotation of the peak whose region
                    // (bounded by the magnitude minimum between peaks) it falls into.
                    let mut lo = 0usize;
                    for (i, &k) in peaks.iter().enumerate() {
                        let hi = if i + 1 < peaks.len() {
                            let k2 = peaks[i + 1];
                            let mut mi = k;
                            for j in k..=k2 {
                                if mag[j] < mag[mi] {
                                    mi = j;
                                }
                            }
                            mi
                        } else {
                            bins - 1
                        };
                        new_delta[lo..=hi].fill(pd[i]);
                        lo = hi + 1;
                    }
                }
            }
            _ => new_delta.fill(0.0),
        }
        std::mem::swap(&mut delta, &mut new_delta);
        std::mem::swap(&mut sum, &mut prev_sum);
        prev_center = Some(mf.center);

        // Rotations (one sin/cos per distinct value: runs of equal delta share it).
        let mut last = f32::NAN;
        let mut r = Complex32::new(1.0, 0.0);
        for k in 0..bins {
            if delta[k] != last {
                last = delta[k];
                let (s, c) = delta[k].sin_cos();
                r = Complex32::new(c, s);
            }
            rot[k] = r;
        }

        let s0 = (m * hop) as i64 - (n / 2) as i64;
        for (c, spec) in specs.iter_mut().enumerate() {
            for (x, r) in spec.iter_mut().zip(&rot) {
                *x *= r;
            }
            spec[0].im = 0.0;
            spec[bins - 1].im = 0.0;
            let _ = inv.process_with_scratch(spec, &mut frame, &mut iscratch);
            let o = &mut out[c];
            for i in 0..n {
                let j = s0 + i as i64;
                if j >= 0 && (j as usize) < o.len() {
                    o[j as usize] += frame[i] * win[i] * scale;
                }
            }
        }
        for i in 0..n {
            let j = s0 + i as i64;
            if j >= 0 && (j as usize) < wss.len() {
                wss[j as usize] += win[i] * win[i];
            }
        }
    }
    // Window-sum-square normalisation (constant inside, tapered at the very edges).
    let floor = wss.iter().copied().fold(0.0f32, f32::max) * 0.1;
    for o in &mut out {
        for (v, w) in o.iter_mut().zip(&wss) {
            *v /= w.max(floor).max(1e-9);
        }
        o.truncate(out_len);
    }
    progress(1.0);
    Ok(out)
}

// ---- WSOLA ---------------------------------------------------------------------------------

/// Box-filter decimation by `d` (good enough for a similarity search).
fn decimate(x: &[f32], d: usize) -> Vec<f32> {
    x.chunks(d)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect()
}

pub(crate) fn wsola(
    buf: &AudioBuffer,
    factor: f64,
    onsets: &[usize],
    lock_transients: bool,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<Vec<Vec<f32>>> {
    let sr = buf.sample_rate.max(1) as f64;
    let n = ((sr * 0.040) as usize / 2 * 2).max(64);
    let hop = n / 2;
    let tol = ((sr * 0.012) as usize).max(4);
    let in_len = buf.frames();
    let out_len = (in_len as f64 * factor).round() as usize;
    let n_frames = (out_len + n / 2) / hop + 1;
    let map = time_map(n_frames, hop, n, factor, onsets, lock_transients);
    let win = hann_window(n);
    let n_ch = buf.n_channels();

    // Search on a decimated mono mix, then refine at full rate.
    let x = mono(buf);
    let d = 4usize;
    let xd = decimate(&x, d);
    let sample = |v: &[f32], i: i64| -> f32 {
        if i >= 0 && (i as usize) < v.len() {
            v[i as usize]
        } else {
            0.0
        }
    };
    // Normalised cross-correlation of `v[a..a+len]` with `v[b..b+len]` (a = candidate).
    let ncc = |v: &[f32], a: i64, b: i64, len: usize, step: usize| -> f32 {
        let (mut xy, mut xx) = (0.0f32, 0.0f32);
        let mut i = 0;
        while i < len {
            let p = sample(v, a + i as i64);
            xy += p * sample(v, b + i as i64);
            xx += p * p;
            i += step;
        }
        xy / (xx.sqrt() + 1e-9)
    };

    let mut out: Vec<Vec<f32>> = vec![vec![0.0f32; out_len + n]; n_ch];
    let mut wsum = vec![0.0f32; out_len + n];
    let mut prev_pos: Option<i64> = None; // segment start used for the previous frame
    for (m, mf) in map.iter().enumerate() {
        if m % 256 == 0 {
            if cancel.load(Ordering::Relaxed) {
                return Err(AudioError::Cancelled);
            }
            progress(m as f32 / map.len() as f32);
        }
        let nominal = mf.center - (n / 2) as i64;
        // Locked frames continue naturally: an exact copy of the input around the onset.
        let still_locked = mf.locked && !mf.reset;
        let pos = match prev_pos {
            None => nominal,
            Some(p) if still_locked => p + hop as i64,
            Some(p) => {
                // The natural continuation of the previous segment's overlap half.
                let nat = p + hop as i64;
                let (nd, cd) = (nat / d as i64, nominal / d as i64);
                let td = (tol / d) as i64;
                let len = hop / d;
                let mut best = (f32::MIN, cd);
                for off in -td..=td {
                    let v = ncc(&xd, cd + off, nd, len, 1);
                    if v > best.0 {
                        best = (v, cd + off);
                    }
                }
                let coarse = best.1 * d as i64;
                let mut best = (f32::MIN, coarse);
                for off in -(d as i64)..=(d as i64) {
                    let c = coarse + off;
                    if (c - nominal).abs() > tol as i64 {
                        continue;
                    }
                    let v = ncc(&x, c, nat, hop, 2);
                    if v > best.0 {
                        best = (v, c);
                    }
                }
                best.1
            }
        };
        prev_pos = Some(pos);
        let s0 = (m * hop) as i64 - (n / 2) as i64;
        for c in 0..n_ch {
            let src = &buf.channels[c];
            let o = &mut out[c];
            for i in 0..n {
                let j = s0 + i as i64;
                if j < 0 || j as usize >= o.len() {
                    continue;
                }
                o[j as usize] += win[i] * sample(src, pos + i as i64);
            }
        }
        for i in 0..n {
            let j = s0 + i as i64;
            if j >= 0 && (j as usize) < wsum.len() {
                wsum[j as usize] += win[i];
            }
        }
    }
    let floor = wsum.iter().copied().fold(0.0f32, f32::max) * 0.1;
    for o in &mut out {
        for (v, w) in o.iter_mut().zip(&wsum) {
            *v /= w.max(floor).max(1e-9);
        }
        o.truncate(out_len);
    }
    progress(1.0);
    Ok(out)
}

// ---- percussive or tonal? ------------------------------------------------------------------

/// Rough harmonic / percussive split (median filters along time vs frequency, after
/// D. FitzGerald, "Harmonic/percussive separation using median filtering", DAFx 2010) on up to
/// 30 s of the buffer: true when the percussive part holds most of the energy.
pub fn is_percussive(buf: &AudioBuffer) -> bool {
    percussive_ratio(buf) > 0.5
}

/// Share of spectral energy (0..1) that looks percussive (vertical in the spectrogram).
pub fn percussive_ratio(buf: &AudioBuffer) -> f32 {
    let x = mono(buf);
    let sr = buf.sample_rate.max(1) as usize;
    // Middle 30 s.
    let len = x.len().min(sr * 30);
    let off = (x.len() - len) / 2;
    let x = &x[off..off + len];
    let n = 1024;
    let hop = 512;
    if x.len() < n * 4 {
        return 0.0;
    }
    let win = hann_window(n);
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
    let mut inp = fft.make_input_vec();
    let mut spec = fft.make_output_vec();
    let bins = n / 2 + 1;
    let frames: Vec<Vec<f32>> = (0..(x.len() - n) / hop)
        .map(|f| {
            for (i, v) in inp.iter_mut().enumerate() {
                *v = x[f * hop + i] * win[i];
            }
            let _ = fft.process(&mut inp, &mut spec);
            spec.iter().map(|c| c.norm()).collect()
        })
        .collect();
    let half = 8usize;
    let median = |v: &mut Vec<f32>| -> f32 {
        let mid = v.len() / 2;
        *v.select_nth_unstable_by(mid, f32::total_cmp).1
    };
    let (mut perc, mut total) = (0.0f64, 0.0f64);
    let mut tmp = Vec::with_capacity(2 * half + 1);
    for f in 0..frames.len() {
        for k in 1..bins {
            let e = frames[f][k];
            if e < 1e-7 {
                continue;
            }
            tmp.clear();
            tmp.extend(
                (f.saturating_sub(half)..(f + half + 1).min(frames.len())).map(|g| frames[g][k]),
            );
            let h = median(&mut tmp);
            tmp.clear();
            tmp.extend(
                frames[f][k.saturating_sub(half)..(k + half + 1).min(bins)]
                    .iter()
                    .copied(),
            );
            let p = median(&mut tmp);
            let e2 = (e * e) as f64;
            total += e2;
            if p > h {
                perc += e2;
            }
        }
    }
    if total <= 0.0 {
        0.0
    } else {
        (perc / total) as f32
    }
}

// ---- tests ---------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onsets::synth::Rng;

    const SR: u32 = 44_100;

    fn quiet() -> Control<'static> {
        static NEVER: AtomicBool = AtomicBool::new(false);
        Control {
            progress: &|_| {},
            cancel: &NEVER,
        }
    }

    fn sine(freq: f32, secs: f32) -> AudioBuffer {
        let n = (secs * SR as f32) as usize;
        let x: Vec<f32> = (0..n)
            .map(|i| 0.5 * (TAU * freq * i as f32 / SR as f32).sin())
            .collect();
        AudioBuffer::new(SR, vec![x.clone(), x])
    }

    /// Frequency of the strongest partial in the middle of `x` (zero-padded FFT + parabolic
    /// interpolation).
    fn dominant_freq(x: &[f32]) -> f32 {
        let n = 1 << 16;
        let mid = x.len() / 2;
        let len = n.min(x.len() / 2);
        let w = hann_window(len);
        let mut inp = vec![0.0f32; n];
        for i in 0..len {
            inp[i] = x[mid - len / 2 + i] * w[i];
        }
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
        let mut spec = fft.make_output_vec();
        fft.process(&mut inp, &mut spec).unwrap();
        let mags: Vec<f32> = spec.iter().map(|c| c.norm().max(1e-12).ln()).collect();
        let k = (1..mags.len() - 1)
            .max_by(|&a, &b| mags[a].total_cmp(&mags[b]))
            .unwrap();
        let (a, b, c) = (mags[k - 1], mags[k], mags[k + 1]);
        let p = 0.5 * (a - c) / (a - 2.0 * b + c);
        (k as f32 + p) * SR as f32 / n as f32
    }

    /// Clicks with decaying noise tails every `period` seconds.
    fn drum_loop(secs: f32, period: f32) -> (AudioBuffer, Vec<usize>) {
        let n = (secs * SR as f32) as usize;
        let mut x = vec![0.0f32; n];
        let mut rng = Rng(99);
        let mut at = Vec::new();
        let mut t = 0.1f32;
        while t < secs - 0.3 {
            let s = (t * SR as f32) as usize;
            at.push(s);
            for i in 0..(0.12 * SR as f32) as usize {
                let tt = i as f32 / SR as f32;
                x[s + i] += 0.7 * rng.next() * (-tt / 0.02).exp()
                    + 0.5 * (TAU * 120.0 * tt).sin() * (-tt / 0.04).exp();
            }
            t += period;
        }
        (AudioBuffer::new(SR, vec![x.clone(), x]), at)
    }

    /// Mean pre-echo in dB: energy in the 15 ms before each expected onset relative to the
    /// 15 ms after it. Lower is sharper (less transient smear).
    fn pre_echo_db(x: &[f32], onsets: &[usize]) -> f32 {
        let w = (0.015 * SR as f32) as usize;
        let mut sum = 0.0;
        let mut k = 0;
        for &o in onsets {
            if o < w || o + w > x.len() {
                continue;
            }
            // Skip the 1 ms right before the onset (onset placement tolerance).
            let g = (0.001 * SR as f32) as usize;
            let pre: f32 = x[o - w..o - g].iter().map(|v| v * v).sum();
            let post: f32 = x[o..o + w].iter().map(|v| v * v).sum();
            sum += 10.0 * ((pre + 1e-12) / (post + 1e-12)).log10();
            k += 1;
        }
        sum / k.max(1) as f32
    }

    #[test]
    fn stretch_hits_exact_length() {
        let buf = sine(440.0, 1.3);
        for f in [0.5, 0.8, 1.0, 1.25, 2.0] {
            for algo in [Algorithm::Tonal, Algorithm::Percussive] {
                let out = time_stretch(&buf, f, algo, &quiet()).unwrap();
                assert_eq!(
                    out.frames(),
                    (buf.frames() as f64 * f).round() as usize,
                    "{algo:?} x{f}"
                );
                assert_eq!(out.n_channels(), 2);
            }
        }
    }

    #[test]
    fn stretch_keeps_pitch() {
        let buf = sine(440.0, 2.0);
        for algo in [Algorithm::Tonal, Algorithm::Percussive] {
            for f in [0.75, 1.5] {
                let out = time_stretch(&buf, f, algo, &quiet()).unwrap();
                let hz = dominant_freq(&out.channels[0]);
                assert!((hz - 440.0).abs() < 1.0, "{algo:?} x{f}: {hz} Hz");
                // Level is kept (no comb-filter dips or OLA gain errors).
                let mid = &out.channels[0][out.frames() / 4..out.frames() * 3 / 4];
                let rms = (mid.iter().map(|v| v * v).sum::<f32>() / mid.len() as f32).sqrt();
                assert!((rms - 0.3536).abs() < 0.03, "{algo:?} x{f}: rms {rms}");
            }
        }
    }

    #[test]
    fn pitch_shift_moves_sine() {
        let buf = sine(440.0, 2.0);
        for st in [3.0, -5.0, 0.5, 12.0] {
            let out = pitch_shift(&buf, st, Algorithm::Tonal, &quiet()).unwrap();
            assert_eq!(out.frames(), buf.frames());
            let want = 440.0 * semitone_ratio(st) as f32;
            let hz = dominant_freq(&out.channels[1]);
            let cents = 1200.0 * (hz / want).log2();
            assert!(
                cents.abs() < 3.0,
                "{st} st: {hz} Hz vs {want} ({cents} cents)"
            );
        }
    }

    #[test]
    fn varispeed_changes_both() {
        let buf = sine(440.0, 1.0);
        let out = varispeed(&buf, semitone_ratio(12.0)).unwrap();
        assert_eq!(out.frames(), buf.frames() / 2);
        let hz = dominant_freq(&out.channels[0]);
        assert!((hz - 880.0).abs() < 1.5, "{hz}");
        let t = Transform {
            speed: 0.9,
            mode: SpeedMode::Varispeed,
            ..Transform::default()
        };
        let out = render(&buf, &t, &quiet()).unwrap();
        assert_eq!(out.frames(), (buf.frames() as f64 / 0.9).round() as usize);
    }

    #[test]
    fn reverse_twice_is_identity() {
        let (buf, _) = drum_loop(1.0, 0.25);
        assert_eq!(reverse(&reverse(&buf)), buf);
        let r = reverse(&buf);
        assert_eq!(r.channels[0][0], buf.channels[0][buf.frames() - 1]);
    }

    #[test]
    fn stereo_image_is_kept() {
        // L = R in, L = R out (the rotation is shared between channels).
        let (buf, _) = drum_loop(1.5, 0.3);
        let out = time_stretch(&buf, 1.2, Algorithm::Tonal, &quiet()).unwrap();
        let diff = out.channels[0]
            .iter()
            .zip(&out.channels[1])
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(diff < 1e-5, "{diff}");
    }

    #[test]
    fn transients_stay_sharp() {
        let (buf, at) = drum_loop(4.0, 0.5);
        let f = 1.25;
        let expected: Vec<usize> = at.iter().map(|&o| (o as f64 * f) as usize).collect();
        let onsets: Vec<usize> = detect_onsets(&buf, &OnsetOptions::default())
            .into_iter()
            .map(|o| o.pos)
            .collect();
        let x = &buf.channels[0];
        let original = pre_echo_db(x, &at);
        let none = |_: f32| {};
        let never = AtomicBool::new(false);
        let measure = |ch: Vec<Vec<f32>>| {
            // Align on the actual attack nearest each expected position (the map may shift
            // an onset by a few ms while it pays back drift).
            let y = &ch[0];
            let w = (0.02 * SR as f32) as usize;
            let actual: Vec<usize> = expected
                .iter()
                .map(|&e| {
                    let a = e.saturating_sub(w);
                    let b = (e + w).min(y.len());
                    // First sample above half the local peak.
                    let pk = y[a..b].iter().fold(0.0f32, |m, v| m.max(v.abs()));
                    a + y[a..b].iter().position(|v| v.abs() > 0.5 * pk).unwrap_or(0)
                })
                .collect();
            pre_echo_db(y, &actual)
        };
        let pv_plain = measure(phase_vocoder(&buf, f, &onsets, false, &none, &never).unwrap());
        let pv = measure(phase_vocoder(&buf, f, &onsets, true, &none, &never).unwrap());
        let ws = measure(wsola(&buf, f, &onsets, true, &none, &never).unwrap());
        eprintln!(
            "pre-echo dB: original {original:.1}, PV plain {pv_plain:.1}, PV transient-locked \
             {pv:.1}, WSOLA {ws:.1}"
        );
        assert!(
            pv < pv_plain - 10.0,
            "locking should cut smear: {pv} vs {pv_plain}"
        );
        assert!(pv < -30.0, "PV pre-echo {pv}");
        assert!(ws < -30.0, "WSOLA pre-echo {ws}");
    }

    #[test]
    fn auto_picks_by_material() {
        let (drums, _) = drum_loop(4.0, 0.25);
        let mut chord = vec![0.0f32; SR as usize * 4];
        for (i, v) in chord.iter_mut().enumerate() {
            let t = i as f32 / SR as f32;
            *v = [220.0f32, 277.2, 329.6]
                .iter()
                .map(|f| 0.2 * (TAU * f * t).sin())
                .sum();
        }
        let tonal = AudioBuffer::new(SR, vec![chord]);
        let (pd, pt) = (percussive_ratio(&drums), percussive_ratio(&tonal));
        assert!(pd > 0.5 && pt < 0.2, "drums {pd}, chord {pt}");
    }

    #[test]
    fn suffixes() {
        let t = Transform {
            speed: 95.0 / 90.0,
            semitones: 3.0,
            ..Transform::default()
        };
        assert_eq!(t.name_suffix(Some(90.0)), "+3st 95bpm");
        let t = Transform {
            speed: 1.1,
            semitones: -2.5,
            reverse: true,
            ..Transform::default()
        };
        assert_eq!(t.name_suffix(None), "-2.5st 110% rev");
        let t = Transform {
            semitones: 1.0,
            mode: SpeedMode::Varispeed,
            ..Transform::default()
        };
        assert_eq!(t.name_suffix(Some(100.0)), "vari +1st 105.9bpm");
        assert_eq!(Transform::default().name_suffix(Some(90.0)), "");
    }

    #[test]
    fn cancel_stops() {
        let buf = sine(440.0, 3.0);
        let stop = AtomicBool::new(true);
        let ctl = Control {
            progress: &|_| {},
            cancel: &stop,
        };
        assert!(matches!(
            time_stretch(&buf, 1.3, Algorithm::Tonal, &ctl),
            Err(AudioError::Cancelled)
        ));
    }

    /// `cargo test -p stem-audio --release -- --ignored --nocapture speed`
    #[test]
    #[ignore]
    fn speed_four_minutes() {
        let secs = 240.0;
        let (drums, _) = drum_loop(secs, 0.25);
        let tonal = sine(330.0, secs);
        for (name, buf, algo) in [
            ("tonal PV", &tonal, Algorithm::Tonal),
            ("drums WSOLA", &drums, Algorithm::Percussive),
        ] {
            let t0 = std::time::Instant::now();
            let _ = pitch_shift(buf, 3.0, algo, &quiet()).unwrap();
            eprintln!("{name}: +3 st on 4 min stereo in {:.2?}", t0.elapsed());
            let t0 = std::time::Instant::now();
            let _ = time_stretch(buf, 1.06, algo, &quiet()).unwrap();
            eprintln!(
                "{name}: x1.06 stretch on 4 min stereo in {:.2?}",
                t0.elapsed()
            );
        }
        let t0 = std::time::Instant::now();
        let _ = is_percussive(&drums);
        eprintln!("auto detect: {:.2?}", t0.elapsed());
    }
}
