//! Music analysis: tempo (BPM), beat grid with a downbeat guess, and musical key.
//!
//! Everything works on a mono downmix decimated to roughly 22 kHz and is fast enough to run
//! right after separation (well under a second for a four-minute track in release builds).
//!
//! - **Onsets**: log-compressed band energies from a short STFT; the onset-strength envelope is
//!   their half-wave-rectified frame-to-frame increase (spectral flux), high-passed and
//!   normalised.
//! - **Tempo**: autocorrelation of the onset envelope, scored as a comb over the first few
//!   multiples of each candidate period and weighted by a log-Gaussian tempo prior. That combo
//!   resolves octave errors: a period gets credit only if its multiples are also periodic, and the
//!   prior picks between a tempo and its double when both fit (70 vs 140 BPM).
//! - **Beats**: the dynamic-programming tracker from D. P. W. Ellis, "Beat Tracking by Dynamic
//!   Programming" (J. New Music Research, 2007), reimplemented from the paper. For steady tempos
//!   the tracked beats are replaced by a least-squares straight grid, which is what you want to
//!   snap sample edges to. The downbeat is the beat phase (mod 4) with the strongest low-band
//!   (kick) onsets.
//! - **Sections** and **chords**: see [`crate::sections`] and [`crate::chords`].
//! - **Key**: a chroma vector (pitch-class energy, tuning-compensated) correlated against the
//!   24 rotations of the Krumhansl-Kessler and Temperley key profiles.

use std::f32::consts::PI;

use realfft::RealFftPlanner;

use crate::AudioBuffer;

/// Tempo range the estimator reports.
pub const MIN_BPM: f32 = 60.0;
pub const MAX_BPM: f32 = 200.0;

/// Constant correction (seconds) for where the flux peak lands relative to the real transient:
/// the log-compressed flux fires as soon as the attack enters the analysis window, ~14 ms
/// early on click tracks. We correct by 10 ms, leaving a few ms of pre-roll so a sample cut on
/// the grid never clips the attack.
const ONSET_LAG: f64 = 0.010;

// ---- results -------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tempo {
    pub bpm: f32,
    /// `0..=1`: how clearly one periodicity stands out.
    pub confidence: f32,
}

/// Where snapping and grid lines go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridDivision {
    HalfBeat,
    Beat,
    Bar,
}

/// Grid point strength, for drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GridLevel {
    HalfBeat,
    Beat,
    Bar,
}

/// Beat positions plus which one starts a bar. Beats outside the analysed span are
/// extrapolated with the local period, so the grid covers the whole timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct BeatGrid {
    pub bpm: f32,
    /// Beat times in seconds, strictly increasing, at least two.
    pub beats: Vec<f64>,
    /// Index into `beats` of a beat that starts a bar.
    pub downbeat: usize,
    pub beats_per_bar: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Major,
    Minor,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    /// Pitch class of the tonic, 0 = C, 1 = C#/Db, ... 11 = B.
    pub tonic: u8,
    pub mode: Mode,
    /// `0..=1`.
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub tempo: Option<Tempo>,
    pub grid: Option<BeatGrid>,
    pub key: Option<Key>,
    /// Song sections on bar lines, covering the whole track.
    pub sections: Vec<crate::sections::Section>,
    /// Chords per beat run, covering the whole track.
    pub chords: Vec<crate::chords::ChordSpan>,
}

const MAJOR_NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];
const MINOR_NAMES: [&str; 12] = [
    "C", "C#", "D", "Eb", "E", "F", "F#", "G", "G#", "A", "Bb", "B",
];

impl Key {
    /// "A minor", "Eb major".
    pub fn name(&self) -> String {
        match self.mode {
            Mode::Major => format!("{} major", MAJOR_NAMES[self.tonic as usize % 12]),
            Mode::Minor => format!("{} minor", MINOR_NAMES[self.tonic as usize % 12]),
        }
    }
    /// "Am", "Eb" (for file names).
    pub fn short(&self) -> String {
        match self.mode {
            Mode::Major => MAJOR_NAMES[self.tonic as usize % 12].to_owned(),
            Mode::Minor => format!("{}m", MINOR_NAMES[self.tonic as usize % 12]),
        }
    }
    /// Camelot wheel code: "8A" = A minor, "8B" = C major.
    pub fn camelot(&self) -> String {
        let (major_pc, letter) = match self.mode {
            Mode::Major => (self.tonic as usize % 12, 'B'),
            Mode::Minor => ((self.tonic as usize + 3) % 12, 'A'),
        };
        format!("{}{letter}", (7 * major_pc + 7) % 12 + 1)
    }
}

impl BeatGrid {
    /// Average beat period in seconds.
    pub fn period(&self) -> f64 {
        60.0 / self.bpm.max(1.0) as f64
    }

    /// Time of beat `i`; negative or past-the-end indices are extrapolated.
    pub fn beat_time(&self, i: i64) -> f64 {
        let n = self.beats.len() as i64;
        if n == 0 {
            return i as f64 * self.period();
        }
        if i < 0 {
            let p = if n >= 2 {
                self.beats[1] - self.beats[0]
            } else {
                self.period()
            };
            self.beats[0] + i as f64 * p
        } else if i >= n {
            let p = if n >= 2 {
                self.beats[n as usize - 1] - self.beats[n as usize - 2]
            } else {
                self.period()
            };
            self.beats[n as usize - 1] + (i - n + 1) as f64 * p
        } else {
            self.beats[i as usize]
        }
    }

    /// Index of the last beat at or before `t` (may be negative / past the end).
    pub fn beat_index_at(&self, t: f64) -> i64 {
        let n = self.beats.len();
        if n == 0 {
            return (t / self.period()).floor() as i64;
        }
        if t < self.beats[0] {
            let p = self.beat_time(1) - self.beat_time(0);
            return ((t - self.beats[0]) / p.max(1e-6)).floor() as i64;
        }
        if t >= self.beats[n - 1] {
            let p = self.beat_time(n as i64) - self.beats[n - 1];
            return n as i64 - 1 + ((t - self.beats[n - 1]) / p.max(1e-6)).floor() as i64;
        }
        (self.beats.partition_point(|&b| b <= t) as i64) - 1
    }

    pub fn is_downbeat(&self, i: i64) -> bool {
        (i - self.downbeat as i64).rem_euclid(self.beats_per_bar.max(1) as i64) == 0
    }

    /// Grid points in `[t0, t1]` at `division` resolution, with their strength.
    pub fn points(&self, t0: f64, t1: f64, division: GridDivision) -> Vec<(f64, GridLevel)> {
        let mut out = Vec::new();
        if t1 < t0 {
            return out;
        }
        let first = self.beat_index_at(t0) - 1;
        let last = self.beat_index_at(t1) + 1;
        for i in first..=last {
            let t = self.beat_time(i);
            let bar = self.is_downbeat(i);
            match division {
                GridDivision::Bar if !bar => {}
                _ => {
                    if t >= t0 && t <= t1 {
                        let lvl = if bar { GridLevel::Bar } else { GridLevel::Beat };
                        out.push((t, lvl));
                    }
                }
            }
            if division == GridDivision::HalfBeat {
                let mid = 0.5 * (t + self.beat_time(i + 1));
                if mid >= t0 && mid <= t1 {
                    out.push((mid, GridLevel::HalfBeat));
                }
            }
        }
        out
    }

    /// Nearest grid point to `t`.
    pub fn snap(&self, t: f64, division: GridDivision) -> f64 {
        let span = match division {
            GridDivision::Bar => self.period() * self.beats_per_bar as f64 * 1.5,
            _ => self.period() * 1.5,
        };
        self.points(t - span, t + span, division)
            .into_iter()
            .map(|(p, _)| p)
            .min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs()))
            .unwrap_or(t)
    }

    /// Length in seconds of the bar containing `t`.
    pub fn bar_length_at(&self, t: f64) -> f64 {
        let i = self.beat_index_at(t);
        self.beat_time(i + self.beats_per_bar as i64) - self.beat_time(i)
    }

    /// Number of bars between `a` and `b` (fractional), measured on the beat grid.
    pub fn bars_between(&self, a: f64, b: f64) -> f64 {
        self.beats_between(a, b) / self.beats_per_bar.max(1) as f64
    }

    /// Fractional number of beats between `a` and `b`.
    pub fn beats_between(&self, a: f64, b: f64) -> f64 {
        self.beat_position(b) - self.beat_position(a)
    }

    /// Fractional beat index at `t`.
    pub fn beat_position(&self, t: f64) -> f64 {
        let i = self.beat_index_at(t);
        let t0 = self.beat_time(i);
        let t1 = self.beat_time(i + 1);
        i as f64 + ((t - t0) / (t1 - t0).max(1e-9)).clamp(0.0, 1.0)
    }

    /// The same grid at half (`factor` 0.5) or double (2.0) tempo, keeping bar lines.
    pub fn scaled(&self, factor: f32) -> BeatGrid {
        let n = self.beats.len();
        if factor > 1.5 {
            let mut beats = Vec::with_capacity(n * 2);
            for i in 0..n {
                beats.push(self.beats[i]);
                let next = self.beat_time(i as i64 + 1);
                if i + 1 < n {
                    beats.push(0.5 * (self.beats[i] + next));
                }
            }
            BeatGrid {
                bpm: self.bpm * 2.0,
                beats,
                downbeat: self.downbeat * 2,
                beats_per_bar: self.beats_per_bar,
            }
        } else if factor < 0.75 && n >= 4 {
            let parity = self.downbeat % 2;
            let beats: Vec<f64> = self
                .beats
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 2 == parity)
                .map(|(_, &b)| b)
                .collect();
            BeatGrid {
                bpm: self.bpm * 0.5,
                beats,
                downbeat: self.downbeat / 2,
                beats_per_bar: self.beats_per_bar,
            }
        } else {
            self.clone()
        }
    }
}

// ---- front door ----------------------------------------------------------------------------

/// Tempo and beats from `rhythm` (ideally the drums stem), key and chords from `harmony` (the
/// instrumental without drums, or the mix), sections from both.
pub fn analyze(rhythm: &AudioBuffer, harmony: &AudioBuffer) -> Analysis {
    let env = onset_envelope(rhythm);
    let tempo = estimate_tempo(&env);
    let grid = tempo.and_then(|t| track_beats(&env, t.bpm));
    // The tracker's straight-line fit is more precise than the autocorrelation peak.
    let tempo = match (tempo, &grid) {
        (Some(t), Some(g)) => Some(Tempo { bpm: g.bpm, ..t }),
        (t, _) => t,
    };
    let feats = crate::beatsync::beat_features(rhythm, harmony, grid.as_ref());
    Analysis {
        tempo,
        key: detect_key(harmony),
        sections: crate::sections::detect_sections(&feats),
        chords: crate::chords::detect_chords(&feats),
        grid,
    }
}

// ---- shared helpers ------------------------------------------------------------------------

/// Mono downmix, decimated by 2 while the rate is above 32 kHz (binomial pre-filter).
pub(crate) fn mono_low_rate(buf: &AudioBuffer) -> (Vec<f32>, f32) {
    let n = buf.frames();
    let ch = buf.n_channels().max(1) as f32;
    let mut mono: Vec<f32> = (0..n)
        .map(|i| buf.channels.iter().map(|c| c[i]).sum::<f32>() / ch)
        .collect();
    let mut sr = buf.sample_rate as f32;
    while sr > 32_000.0 && mono.len() > 8 {
        let m = mono.len() / 2;
        let at = |i: isize| mono[i.clamp(0, mono.len() as isize - 1) as usize];
        mono = (0..m as isize)
            .map(|j| {
                let i = 2 * j;
                (at(i - 1) + 3.0 * at(i) + 3.0 * at(i + 1) + at(i + 2)) * 0.125
            })
            .collect();
        sr /= 2.0;
    }
    (mono, sr)
}

/// Magnitude spectra of `x`, one frame every `hop` samples with frames centred on
/// `f * hop` (zero-padded at the edges). Calls `each(frame_index, magnitudes)`.
pub(crate) fn for_each_spectrum(
    x: &[f32],
    n_fft: usize,
    hop: usize,
    mut each: impl FnMut(usize, &[f32]),
) {
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n_fft);
    let window = crate::stft::hann_window(n_fft);
    let mut input = fft.make_input_vec();
    let mut out = fft.make_output_vec();
    let mut scratch = fft.make_scratch_vec();
    let mut mags = vec![0.0f32; n_fft / 2 + 1];
    let n_frames = x.len() / hop + 1;
    let half = (n_fft / 2) as isize;
    for f in 0..n_frames {
        let start = (f * hop) as isize - half;
        for (k, v) in input.iter_mut().enumerate() {
            let i = start + k as isize;
            *v = if i >= 0 && (i as usize) < x.len() {
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
        for (m, c) in mags.iter_mut().zip(&out) {
            *m = c.norm();
        }
        each(f, &mags);
    }
}

fn mean(x: &[f32]) -> f32 {
    if x.is_empty() {
        0.0
    } else {
        x.iter().sum::<f32>() / x.len() as f32
    }
}

// ---- onsets --------------------------------------------------------------------------------

/// Onset-strength envelope.
#[derive(Debug, Clone, Default)]
pub struct OnsetEnvelope {
    /// Normalised onset strength per frame (zero mean-ish, unit variance, non-negative).
    pub strength: Vec<f32>,
    /// Same, for the bands under ~150 Hz (kick drum), used for the downbeat.
    pub low: Vec<f32>,
    /// Frames per second.
    pub fps: f64,
}

impl OnsetEnvelope {
    fn frame_time(&self, f: f64) -> f64 {
        f / self.fps + ONSET_LAG
    }
}

const ONSET_FFT: usize = 1024;
const ONSET_HOP: usize = 128;
const N_BANDS: usize = 36;

/// Spectral-flux onset envelope of a buffer.
pub fn onset_envelope(buf: &AudioBuffer) -> OnsetEnvelope {
    let (mut x, sr) = mono_low_rate(buf);
    if x.len() < ONSET_FFT {
        return OnsetEnvelope::default();
    }
    // Level-independent log compression.
    let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
    if rms < 1e-5 {
        return OnsetEnvelope::default();
    }
    let g = 0.1 / rms;
    x.iter_mut().for_each(|v| *v *= g);

    // Log-spaced bands from 40 Hz to 10 kHz.
    let n_bins = ONSET_FFT / 2 + 1;
    let bin_hz = sr / ONSET_FFT as f32;
    let (f_lo, f_hi) = (40.0f32, 10_000.0f32.min(sr * 0.48));
    let band_of: Vec<Option<usize>> = (0..n_bins)
        .map(|k| {
            let f = k as f32 * bin_hz;
            if f < f_lo || f > f_hi {
                None
            } else {
                let b = ((f / f_lo).ln() / (f_hi / f_lo).ln() * N_BANDS as f32) as usize;
                Some(b.min(N_BANDS - 1))
            }
        })
        .collect();
    let low_bands = ((150.0f32 / f_lo).ln() / (f_hi / f_lo).ln() * N_BANDS as f32).ceil() as usize;

    let n_frames = x.len() / ONSET_HOP + 1;
    let mut flux = vec![0.0f32; n_frames];
    let mut low = vec![0.0f32; n_frames];
    let mut prev = [0.0f32; N_BANDS];
    let mut bands = [0.0f32; N_BANDS];
    for_each_spectrum(&x, ONSET_FFT, ONSET_HOP, |f, mags| {
        bands.iter_mut().for_each(|b| *b = 0.0);
        for (m, b) in mags.iter().zip(&band_of) {
            if let Some(b) = b {
                bands[*b] += m;
            }
        }
        let mut s = 0.0;
        let mut sl = 0.0;
        for b in 0..N_BANDS {
            let l = (1.0 + 10.0 * bands[b]).ln();
            let d = (l - prev[b]).max(0.0);
            if f > 0 {
                s += d;
                if b < low_bands {
                    sl += d;
                }
            }
            prev[b] = l;
        }
        flux[f] = s;
        low[f] = sl;
    });
    let fps = sr as f64 / ONSET_HOP as f64;
    OnsetEnvelope {
        strength: normalize_onsets(&flux, fps),
        low: normalize_onsets(&low, fps),
        fps,
    }
}

/// Subtract a ~0.5 s moving average, rectify, divide by the standard deviation.
fn normalize_onsets(x: &[f32], fps: f64) -> Vec<f32> {
    let w = ((fps * 0.25) as usize).max(1);
    let n = x.len();
    let mut prefix = vec![0.0f64; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + x[i] as f64;
    }
    let mut out: Vec<f32> = (0..n)
        .map(|i| {
            let a = i.saturating_sub(w);
            let b = (i + w + 1).min(n);
            let m = (prefix[b] - prefix[a]) / (b - a) as f64;
            (x[i] - m as f32).max(0.0)
        })
        .collect();
    let mu = mean(&out);
    let sd = (out.iter().map(|v| (v - mu) * (v - mu)).sum::<f32>() / n.max(1) as f32).sqrt();
    if sd > 1e-9 {
        out.iter_mut().for_each(|v| *v /= sd);
    }
    out
}

// ---- tempo ---------------------------------------------------------------------------------

/// Log-Gaussian tempo prior: centred on 120 BPM, one octave wide.
fn tempo_prior(bpm: f32) -> f32 {
    let o = (bpm / 120.0).log2();
    (-0.5 * o * o).exp()
}

/// Global tempo from an onset envelope, `None` for silence / no rhythm.
pub fn estimate_tempo(env: &OnsetEnvelope) -> Option<Tempo> {
    let o = &env.strength;
    let fps = env.fps;
    if fps <= 0.0 || o.len() < (fps * 4.0) as usize {
        return None;
    }
    let lag_of = |bpm: f32| fps * 60.0 / bpm as f64;
    let min_lag = lag_of(MAX_BPM).floor().max(1.0) as usize;
    let max_lag = lag_of(MIN_BPM).ceil() as usize;
    // Comb uses up to 4x the longest period.
    let ac_max = (max_lag * 4 + 4).min(o.len() - 1);
    let mu = mean(o);
    let c: Vec<f32> = o.iter().map(|v| v - mu).collect();
    let ac: Vec<f32> = (0..=ac_max)
        .map(|lag| {
            let n = c.len() - lag;
            let s: f32 = c[..n].iter().zip(&c[lag..]).map(|(a, b)| a * b).sum();
            s / n as f32
        })
        .collect();
    if ac[0] <= 1e-9 {
        return None;
    }
    // Peak of `ac` within a tolerance window around `x`.
    let peak_near = |x: f64, tol: f64| -> f32 {
        let a = (x - tol).floor().max(0.0) as usize;
        let b = ((x + tol).ceil() as usize).min(ac_max);
        (a..=b).map(|i| ac[i]).fold(f32::MIN, f32::max).max(0.0)
    };
    let mut scores = Vec::new();
    for lag in min_lag..=max_lag.min(ac_max) {
        let l = lag as f64;
        // Multiples only confirm a period: each is capped at the period's own strength, so
        // bar-level accents can't make half tempo look better than the real one.
        let base = peak_near(l, 0.5);
        let mut s = base;
        for k in 2..=4 {
            let x = l * k as f64;
            if x > ac_max as f64 {
                break;
            }
            s += peak_near(x, 0.5 * k as f64).min(base) / k as f32;
        }
        let bpm = (fps * 60.0 / l) as f32;
        scores.push((lag, s * tempo_prior(bpm)));
    }
    let &(best_lag, best) = scores.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
    if best <= 0.0 {
        return None;
    }
    // Sub-frame peak position: parabola through the autocorrelation around the lag.
    let refine = |l: usize| -> f64 {
        if l == 0 || l + 1 > ac_max {
            return l as f64;
        }
        let (y0, y1, y2) = (ac[l - 1], ac[l], ac[l + 1]);
        let d = y0 - 2.0 * y1 + y2;
        if d.abs() < 1e-12 || y1 < y0 || y1 < y2 {
            l as f64
        } else {
            l as f64 + (0.5 * (y0 - y2) / d).clamp(-0.5, 0.5) as f64
        }
    };
    // The local maximum of `ac` right around the chosen lag.
    let lag = (best_lag.saturating_sub(1)..=(best_lag + 1).min(ac_max))
        .max_by(|a, b| ac[*a].total_cmp(&ac[*b]))
        .unwrap_or(best_lag);
    let mut bpm = (fps * 60.0 / refine(lag)) as f32;
    // Octave check: when the off-beats hit as hard as the beats, the beat is twice as fast
    // (174 BPM drum & bass rather than 87).
    if bpm * 2.0 <= MAX_BPM && offbeat_ratio(o, refine(lag)) > 0.7 {
        bpm *= 2.0;
    }
    let mut sorted: Vec<f32> = scores.iter().map(|s| s.1).collect();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2];
    let contrast = ((best - median) / best).clamp(0.0, 1.0);
    let strength = (ac[lag].max(0.0) / ac[0]).clamp(0.0, 1.0).sqrt();
    Some(Tempo {
        bpm: bpm.clamp(MIN_BPM, MAX_BPM),
        confidence: (contrast * strength).clamp(0.0, 1.0),
    })
}

/// Onset strength half-way between beats relative to on the beat, for beat period `period`
/// (frames), from the envelope folded over one period.
fn offbeat_ratio(o: &[f32], period: f64) -> f32 {
    const BINS: usize = 48;
    let mut sum = [0.0f32; BINS];
    let mut cnt = [0u32; BINS];
    for (i, &v) in o.iter().enumerate() {
        let b = (((i as f64 / period).fract() * BINS as f64) as usize).min(BINS - 1);
        sum[b] += v;
        cnt[b] += 1;
    }
    let h: Vec<f32> = (0..BINS).map(|b| sum[b] / cnt[b].max(1) as f32).collect();
    let around = |c: usize| {
        (0..3)
            .map(|d| h[(c + BINS + d - 1) % BINS])
            .fold(0.0, f32::max)
    };
    let peak = (0..BINS)
        .max_by(|a, b| h[*a].total_cmp(&h[*b]))
        .unwrap_or(0);
    let on = around(peak);
    if on <= 1e-9 {
        return 0.0;
    }
    around((peak + BINS / 2) % BINS) / on
}

// ---- beats ---------------------------------------------------------------------------------

/// Ellis-style DP beat tracking at `bpm`, then straight-line refinement and a downbeat guess.
pub fn track_beats(env: &OnsetEnvelope, bpm: f32) -> Option<BeatGrid> {
    let o = &env.strength;
    let n = o.len();
    let fps = env.fps;
    let period = fps * 60.0 / bpm.max(1.0) as f64;
    if n < (period * 4.0) as usize || period < 2.0 {
        return None;
    }
    // Local score: onsets smoothed with a Gaussian ~period/32 wide.
    let sigma = (period / 32.0).max(1.0);
    let radius = (sigma * 3.0).ceil() as isize;
    let kernel: Vec<f32> = (-radius..=radius)
        .map(|i| (-0.5 * (i as f64 / sigma).powi(2)).exp() as f32)
        .collect();
    let local: Vec<f32> = (0..n as isize)
        .map(|i| {
            kernel
                .iter()
                .enumerate()
                .map(|(k, w)| {
                    let j = i + k as isize - radius;
                    if j >= 0 && (j as usize) < n {
                        o[j as usize] * w
                    } else {
                        0.0
                    }
                })
                .sum()
        })
        .collect();

    const TIGHTNESS: f64 = 100.0;
    let d_min = (period / 2.0).round().max(1.0) as usize;
    let d_max = (period * 2.0).round() as usize;
    let penalty: Vec<f32> = (0..=d_max)
        .map(|d| {
            if d < d_min {
                f32::NEG_INFINITY
            } else {
                (-TIGHTNESS * ((d as f64) / period).ln().powi(2)) as f32
            }
        })
        .collect();
    let mut cum = vec![0.0f32; n];
    let mut back = vec![usize::MAX; n];
    for t in 0..n {
        let mut best = f32::NEG_INFINITY;
        let mut arg = usize::MAX;
        if t >= d_min {
            let lo = t.saturating_sub(d_max);
            for tau in lo..=t - d_min {
                let v = cum[tau] + penalty[t - tau];
                if v > best {
                    best = v;
                    arg = tau;
                }
            }
        }
        if arg == usize::MAX || best < 0.0 {
            // Starting fresh beats a heavily penalised chain.
            cum[t] = local[t];
            back[t] = usize::MAX;
        } else {
            cum[t] = local[t] + best;
            back[t] = arg;
        }
    }
    // Last beat: the last local maximum of the cumulative score that is reasonably high.
    let maxima: Vec<usize> = (1..n.saturating_sub(1))
        .filter(|&t| cum[t] > cum[t - 1] && cum[t] >= cum[t + 1])
        .collect();
    if maxima.is_empty() {
        return None;
    }
    let mut vals: Vec<f32> = maxima.iter().map(|&t| cum[t]).collect();
    vals.sort_by(f32::total_cmp);
    let med = vals[vals.len() / 2];
    let tail_start = n.saturating_sub((period * 2.0) as usize);
    let mut end = *maxima
        .iter()
        .rev()
        .find(|&&t| cum[t] >= 0.5 * med)
        .unwrap_or(&maxima[maxima.len() - 1]);
    // Prefer the best-scoring beat in the last two periods so a weak last hit doesn't anchor us.
    if let Some(&t) = maxima
        .iter()
        .filter(|&&t| t >= tail_start.min(end))
        .max_by(|a, b| cum[**a].total_cmp(&cum[**b]))
        && t >= end
    {
        end = t;
    }
    let mut frames = vec![end];
    let mut t = end;
    while back[t] != usize::MAX {
        t = back[t];
        frames.push(t);
    }
    frames.reverse();
    // Trim weak beats at both ends (silence before the music starts / after it stops).
    let thresh = 0.5
        * (frames.iter().map(|&f| local[f] * local[f]).sum::<f32>() / frames.len() as f32).sqrt();
    while frames.len() > 2 && local[frames[0]] < thresh {
        frames.remove(0);
    }
    while frames.len() > 2 && local[*frames.last()?] < thresh {
        frames.pop();
    }
    if frames.len() < 2 {
        return None;
    }

    let mut beats: Vec<f64> = frames.iter().map(|&f| env.frame_time(f as f64)).collect();
    let mut bpm_out = bpm;
    // Least-squares straight grid; keep it when the tempo is steady.
    if beats.len() >= 8 {
        let m = beats.len() as f64;
        let xs: f64 = (0..beats.len()).map(|i| i as f64).sum::<f64>() / m;
        let ys: f64 = beats.iter().sum::<f64>() / m;
        let mut sxy = 0.0;
        let mut sxx = 0.0;
        for (i, &b) in beats.iter().enumerate() {
            sxy += (i as f64 - xs) * (b - ys);
            sxx += (i as f64 - xs).powi(2);
        }
        let slope = sxy / sxx;
        let icept = ys - slope * xs;
        let resid = (beats
            .iter()
            .enumerate()
            .map(|(i, &b)| (b - (icept + slope * i as f64)).powi(2))
            .sum::<f64>()
            / m)
            .sqrt();
        if slope > 0.0 {
            bpm_out = (60.0 / slope) as f32;
            // Producers' tracks are nearly always on an integer tempo.
            let rounded = bpm_out.round();
            let steady = resid < 0.02;
            if steady && (bpm_out - rounded).abs() < 0.08 {
                bpm_out = rounded;
            }
            if steady {
                let slope = 60.0 / bpm_out as f64;
                // Re-fit the offset for the (possibly rounded) slope.
                let icept = beats
                    .iter()
                    .enumerate()
                    .map(|(i, &b)| b - slope * i as f64)
                    .sum::<f64>()
                    / m;
                beats = (0..beats.len()).map(|i| icept + slope * i as f64).collect();
            }
        }
    } else {
        bpm_out = (60.0 / ((beats[beats.len() - 1] - beats[0]) / (beats.len() - 1) as f64)) as f32;
    }

    // Downbeat: the beat phase (mod 4) with the strongest kick-band onsets near the beats.
    let beats_per_bar = 4;
    let lowpeak = |t: f64| -> f32 {
        let f = ((t - ONSET_LAG) * fps).round() as isize;
        (f - 2..=f + 2)
            .filter(|&i| i >= 0 && (i as usize) < n)
            .map(|i| env.low[i as usize] + 0.25 * o[i as usize])
            .fold(0.0, f32::max)
    };
    let downbeat = (0..beats_per_bar.min(beats.len()))
        .map(|phase| {
            let v: Vec<f32> = beats
                .iter()
                .skip(phase)
                .step_by(beats_per_bar)
                .map(|&t| lowpeak(t))
                .collect();
            (phase, mean(&v))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(0, |(p, _)| p);

    Some(BeatGrid {
        bpm: bpm_out,
        beats,
        downbeat,
        beats_per_bar,
    })
}

// ---- key -----------------------------------------------------------------------------------

/// Krumhansl-Kessler probe-tone ratings (Krumhansl 1990), C major / C minor.
const KK_MAJOR: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const KK_MINOR: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];
/// Temperley's Kostka-Payne corpus profiles (Temperley 2001/2007), C major / C minor.
const TEMPERLEY_MAJOR: [f32; 12] = [
    0.748, 0.060, 0.488, 0.082, 0.670, 0.460, 0.096, 0.715, 0.104, 0.366, 0.057, 0.400,
];
const TEMPERLEY_MINOR: [f32; 12] = [
    0.712, 0.084, 0.474, 0.618, 0.049, 0.460, 0.105, 0.747, 0.404, 0.067, 0.133, 0.330,
];

const CHROMA_FFT: usize = 8192;
const CHROMA_HOP: usize = 4096;

/// Pearson correlation of `chroma` with `profile` rotated so its tonic is `tonic`.
fn correlate(chroma: &[f32; 12], profile: &[f32; 12], tonic: usize) -> f32 {
    let mc = mean(chroma);
    let mp = mean(profile);
    let (mut num, mut dc, mut dp) = (0.0, 0.0, 0.0);
    for pc in 0..12 {
        let c = chroma[pc] - mc;
        let p = profile[(pc + 12 - tonic) % 12] - mp;
        num += c * p;
        dc += c * c;
        dp += p * p;
    }
    if dc <= 1e-12 || dp <= 1e-12 {
        0.0
    } else {
        num / (dc * dp).sqrt()
    }
}

/// 12-bin pitch-class profile of a buffer (sums to 1), or `None` if it is silent.
pub fn chroma(buf: &AudioBuffer) -> Option<[f32; 12]> {
    let (x, sr) = mono_low_rate(buf);
    if x.len() < CHROMA_FFT / 2 {
        return None;
    }
    let n_bins = CHROMA_FFT / 2 + 1;
    let bin_hz = sr / CHROMA_FFT as f32;
    let (f_lo, f_hi) = (55.0f32, 1760.0f32.min(sr * 0.45));
    let k_lo = (f_lo / bin_hz).ceil() as usize;
    let k_hi = ((f_hi / bin_hz).floor() as usize).min(n_bins - 2);
    let midi_of = |k: usize| 69.0 + 12.0 * (k as f32 * bin_hz / 440.0).log2();

    // Pass 1: spectra (kept, they're small) and frame energies; tuning from spectral peaks.
    let mut frames: Vec<Vec<f32>> = Vec::new();
    let (mut tc, mut ts) = (0.0f32, 0.0f32);
    for_each_spectrum(&x, CHROMA_FFT, CHROMA_HOP, |_, mags| {
        let m = mags[k_lo - 1..=k_hi + 1].to_vec();
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
        frames.push(m);
    });
    let tuning = if tc.abs() + ts.abs() > 0.0 {
        ts.atan2(tc) / (2.0 * PI)
    } else {
        0.0
    };

    let energies: Vec<f32> = frames
        .iter()
        .map(|m| m.iter().map(|v| v * v).sum())
        .collect();
    let max_e = energies.iter().copied().fold(0.0, f32::max);
    if max_e <= 1e-10 {
        return None;
    }
    let mut total = [0.0f32; 12];
    for (m, &e) in frames.iter().zip(&energies) {
        if e < max_e * 1e-3 {
            continue;
        }
        let mut c = [0.0f32; 12];
        for k in k_lo..=k_hi {
            let midi = midi_of(k) - tuning;
            let near = midi.round();
            let w = (1.0 - 2.0 * (midi - near).abs()).max(0.0);
            let pc = (near as i32).rem_euclid(12) as usize;
            c[pc] += w * m[k - (k_lo - 1)];
        }
        let cmax = c.iter().copied().fold(0.0, f32::max);
        if cmax > 0.0 {
            for pc in 0..12 {
                total[pc] += c[pc] / cmax;
            }
        }
    }
    let sum: f32 = total.iter().sum();
    if sum <= 0.0 {
        return None;
    }
    total.iter_mut().for_each(|v| *v /= sum);
    Some(total)
}

/// Best-matching key for a pitch-class profile.
pub fn key_from_chroma(chroma: &[f32; 12]) -> Option<Key> {
    let mut scores = Vec::with_capacity(24);
    for tonic in 0..12 {
        for (mode, kk, tp) in [
            (Mode::Major, &KK_MAJOR, &TEMPERLEY_MAJOR),
            (Mode::Minor, &KK_MINOR, &TEMPERLEY_MINOR),
        ] {
            let r = 0.5 * (correlate(chroma, kk, tonic) + correlate(chroma, tp, tonic));
            scores.push((tonic as u8, mode, r));
        }
    }
    scores.sort_by(|a, b| b.2.total_cmp(&a.2));
    let (tonic, mode, r1) = scores[0];
    let r2 = scores[1].2;
    if r1 <= 0.0 {
        return None;
    }
    // Clear winner and a good fit both count.
    let margin = ((r1 - r2) / 0.15).clamp(0.0, 1.0);
    let confidence = (r1.clamp(0.0, 1.0) * (0.4 + 0.6 * margin)).clamp(0.0, 1.0);
    Some(Key {
        tonic,
        mode,
        confidence,
    })
}

/// Musical key of a buffer.
pub fn detect_key(buf: &AudioBuffer) -> Option<Key> {
    key_from_chroma(&chroma(buf)?)
}

// ---- tests ---------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        }
    }

    /// Adds a drum-ish hit at `t`: a pitched thump plus a noise click.
    fn hit(x: &mut [f32], t: f64, gain: f32, freq: f32, rng: &mut Rng) {
        let start = (t * SR as f64) as usize;
        let len = (0.12 * SR as f32) as usize;
        for k in 0..len {
            let i = start + k;
            if i >= x.len() {
                break;
            }
            let tt = k as f32 / SR as f32;
            let env = (-tt * 35.0).exp();
            let click = if k < 220 { rng.next() * 0.6 } else { 0.0 };
            x[i] += gain * env * ((2.0 * PI * freq * tt).sin() + click);
        }
    }

    /// Click track: `bpm` quarter notes from `offset`, accent on every 4th beat.
    fn clicks(bpm: f64, secs: f64, offset: f64) -> (AudioBuffer, Vec<f64>) {
        let mut x = vec![0.0f32; (secs * SR as f64) as usize];
        let mut rng = Rng(7);
        let mut truth = Vec::new();
        let p = 60.0 / bpm;
        let mut i = 0;
        loop {
            let t = offset + i as f64 * p;
            if t >= secs - 0.2 {
                break;
            }
            let accent = i % 4 == 0;
            let (g, f) = if accent { (0.9, 60.0) } else { (0.45, 180.0) };
            hit(&mut x, t, g, f, &mut rng);
            truth.push(t);
            i += 1;
        }
        (AudioBuffer::new(SR, vec![x]), truth)
    }

    fn bpm_of(buf: &AudioBuffer) -> f32 {
        let a = analyze(buf, buf);
        a.tempo.expect("tempo").bpm
    }

    #[test]
    fn click_tracks_at_known_tempos() {
        for bpm in [70.0, 85.0, 90.0, 100.0, 120.0, 128.0, 140.0, 160.0, 174.0] {
            let (buf, _) = clicks(bpm, 30.0, 0.31);
            let got = bpm_of(&buf);
            assert!(
                (got as f64 - bpm).abs() < 0.3,
                "expected {bpm} BPM, got {got}"
            );
        }
    }

    #[test]
    fn fractional_tempo_survives() {
        let (buf, _) = clicks(93.5, 40.0, 0.1);
        let got = bpm_of(&buf);
        assert!((got - 93.5).abs() < 0.15, "got {got}");
    }

    #[test]
    fn seventy_vs_one_forty() {
        // Plain quarter notes at 70: no energy at the 140 period, so 70 wins.
        let (buf, _) = clicks(70.0, 30.0, 0.2);
        assert!((bpm_of(&buf) - 70.0).abs() < 0.3);
        // Quarter notes at 140: 70 fits as well (every other beat) but the prior picks 140.
        let (buf, _) = clicks(140.0, 30.0, 0.2);
        assert!((bpm_of(&buf) - 140.0).abs() < 0.3);
        // Half-time groove: kick+snare at 70 with eighth-note hats. Either reading is musical;
        // the app lets the user halve / double.
        let mut x = vec![0.0f32; 30 * SR as usize];
        let mut rng = Rng(3);
        let p = 60.0 / 70.0;
        for i in 0..68 {
            let t = 0.2 + i as f64 * p / 2.0;
            if t > 29.5 {
                break;
            }
            hit(&mut x, t, 0.15, 6000.0, &mut rng);
            if i % 2 == 0 {
                hit(&mut x, t, 0.9, 55.0, &mut rng);
            }
        }
        let got = bpm_of(&AudioBuffer::new(SR, vec![x]));
        assert!(
            (got - 70.0).abs() < 0.5 || (got - 140.0).abs() < 1.0,
            "got {got}"
        );
    }

    #[test]
    fn beats_line_up_with_clicks() {
        let (buf, truth) = clicks(96.0, 30.0, 0.43);
        let a = analyze(&buf, &buf);
        let grid = a.grid.expect("grid");
        // Every true click has a tracked beat within 8 ms (the grid covers the whole track).
        for &t in &truth {
            let s = grid.snap(t, GridDivision::Beat);
            assert!((s - t).abs() < 0.008, "click {t:.3} snapped to {s:.3}");
        }
        // Downbeat on the accents (every 4th click from the first one).
        let first_bar = grid.beat_time(grid.downbeat as i64);
        let k = ((first_bar - 0.43) / (60.0 / 96.0)).round() as i64;
        assert_eq!(k.rem_euclid(4), 0, "downbeat at {first_bar}");
    }

    #[test]
    fn silence_has_no_tempo_or_key() {
        let buf = AudioBuffer::silence(SR, 2, SR as usize * 10);
        let a = analyze(&buf, &buf);
        assert!(a.tempo.is_none());
        assert!(a.grid.is_none());
        assert!(a.key.is_none());
    }

    fn midi_hz(n: f32) -> f32 {
        440.0 * 2f32.powf((n - 69.0) / 12.0)
    }

    /// Chords as MIDI notes, one per bar of 2 s, with a few harmonics each (like a pad).
    fn progression(chords: &[&[f32]], reps: usize) -> AudioBuffer {
        let bar = 2.0f32;
        let total = (chords.len() * reps) as f32 * bar;
        let n = (total * SR as f32) as usize;
        let mut x = vec![0.0f32; n];
        for (i, v) in x.iter_mut().enumerate() {
            let t = i as f32 / SR as f32;
            let c = chords[((t / bar) as usize) % chords.len()];
            let local = t % bar;
            let env = (local * 20.0).min(1.0) * (1.0 - local / bar).powf(0.3);
            let mut s = 0.0;
            for (j, &note) in c.iter().enumerate() {
                let f = midi_hz(note);
                let g = if j == 0 { 1.0 } else { 0.6 };
                for h in 1..=4 {
                    s += g / h as f32 * (2.0 * PI * f * h as f32 * t).sin();
                }
            }
            *v = s * env * 0.05;
        }
        AudioBuffer::new(SR, vec![x])
    }

    fn key_of(chords: &[&[f32]]) -> Key {
        detect_key(&progression(chords, 3)).expect("key")
    }

    #[test]
    fn keys_of_chord_progressions() {
        // i - iv - V - i in A minor (E major's G# marks it as minor, not C major).
        let am = key_of(&[
            &[45.0, 57.0, 60.0, 64.0],
            &[50.0, 57.0, 62.0, 65.0],
            &[40.0, 56.0, 59.0, 64.0],
            &[45.0, 57.0, 60.0, 64.0],
        ]);
        assert_eq!(am.name(), "A minor");
        assert_eq!(am.camelot(), "8A");
        // I - IV - V - I in C major.
        let c = key_of(&[
            &[36.0, 60.0, 64.0, 67.0],
            &[41.0, 60.0, 65.0, 69.0],
            &[43.0, 59.0, 62.0, 67.0],
            &[36.0, 60.0, 64.0, 67.0],
        ]);
        assert_eq!(c.name(), "C major");
        assert_eq!(c.camelot(), "8B");
        // I - vi - IV - V in Eb major.
        let eb = key_of(&[
            &[39.0, 63.0, 67.0, 70.0],
            &[36.0, 60.0, 63.0, 67.0],
            &[44.0, 63.0, 68.0, 72.0],
            &[46.0, 62.0, 65.0, 70.0],
        ]);
        assert_eq!(eb.name(), "Eb major");
        // i - VI - iv - V in F# minor.
        let fsm = key_of(&[
            &[42.0, 61.0, 66.0, 69.0],
            &[38.0, 62.0, 66.0, 69.0],
            &[47.0, 59.0, 62.0, 66.0],
            &[37.0, 61.0, 65.0, 68.0],
        ]);
        assert_eq!(fsm.name(), "F# minor");
        assert_eq!(fsm.camelot(), "11A");
    }

    #[test]
    fn key_survives_detuning() {
        // A minor progression played 40 cents sharp (an old record at the wrong speed).
        let detune = 0.4;
        let chords: [&[f32]; 4] = [
            &[45.0 + detune, 57.0 + detune, 60.0 + detune, 64.0 + detune],
            &[50.0 + detune, 57.0 + detune, 62.0 + detune, 65.0 + detune],
            &[40.0 + detune, 56.0 + detune, 59.0 + detune, 64.0 + detune],
            &[45.0 + detune, 57.0 + detune, 60.0 + detune, 64.0 + detune],
        ];
        assert_eq!(key_of(&chords).name(), "A minor");
    }

    #[test]
    fn camelot_wheel() {
        let k = |tonic, mode| {
            Key {
                tonic,
                mode,
                confidence: 1.0,
            }
            .camelot()
        };
        assert_eq!(k(0, Mode::Major), "8B");
        assert_eq!(k(7, Mode::Major), "9B");
        assert_eq!(k(5, Mode::Major), "7B");
        assert_eq!(k(11, Mode::Major), "1B");
        assert_eq!(k(6, Mode::Major), "2B");
        assert_eq!(k(9, Mode::Minor), "8A");
        assert_eq!(k(4, Mode::Minor), "9A");
        assert_eq!(k(0, Mode::Minor), "5A");
        assert_eq!(k(8, Mode::Minor), "1A");
        assert_eq!(k(2, Mode::Minor), "7A");
        let am = Key {
            tonic: 9,
            mode: Mode::Minor,
            confidence: 1.0,
        };
        assert_eq!(am.short(), "Am");
    }

    fn grid(bpm: f32, n: usize, offset: f64, downbeat: usize) -> BeatGrid {
        let p = 60.0 / bpm as f64;
        BeatGrid {
            bpm,
            beats: (0..n).map(|i| offset + i as f64 * p).collect(),
            downbeat,
            beats_per_bar: 4,
        }
    }

    #[test]
    fn grid_snapping_and_extrapolation() {
        let g = grid(120.0, 16, 1.0, 1); // beats every 0.5 s from 1.0; bars at 1.5, 3.5, ...
        assert!((g.snap(2.2, GridDivision::Beat) - 2.0).abs() < 1e-9);
        assert!((g.snap(2.2, GridDivision::HalfBeat) - 2.25).abs() < 1e-9);
        assert!((g.snap(2.2, GridDivision::Bar) - 1.5).abs() < 1e-9);
        assert!((g.snap(3.0, GridDivision::Bar) - 3.5).abs() < 1e-9);
        // Before the first and after the last beat.
        assert!((g.snap(0.1, GridDivision::Beat) - 0.0).abs() < 1e-9);
        assert!((g.snap(20.04, GridDivision::Beat) - 20.0).abs() < 1e-9);
        assert!(g.is_downbeat(1) && g.is_downbeat(5) && g.is_downbeat(-3));
        assert!((g.bars_between(1.5, 5.5) - 2.0).abs() < 1e-9);
        let pts = g.points(1.4, 3.6, GridDivision::Beat);
        assert_eq!(pts.len(), 5);
        assert_eq!(pts[0], (1.5, GridLevel::Bar));
        assert_eq!(pts[4], (3.5, GridLevel::Bar));
    }

    #[test]
    fn halve_and_double_keep_bars() {
        let g = grid(140.0, 32, 0.25, 2);
        let half = g.scaled(0.5);
        assert!((half.bpm - 70.0).abs() < 1e-6);
        assert!((half.beat_time(half.downbeat as i64) - g.beat_time(2)).abs() < 1e-9);
        let dbl = g.scaled(2.0);
        assert!((dbl.bpm - 280.0).abs() < 1e-6);
        assert!((dbl.beat_time(dbl.downbeat as i64) - g.beat_time(2)).abs() < 1e-9);
        assert!((dbl.beat_time(1) - (g.beat_time(0) + g.period() / 2.0)).abs() < 1e-9);
        // Round trip.
        let back = dbl.scaled(0.5);
        assert!((back.bpm - 140.0).abs() < 1e-6);
        assert!((back.snap(5.0, GridDivision::Bar) - g.snap(5.0, GridDivision::Bar)).abs() < 1e-9);
    }

    /// Timing on a 4-minute stereo track. `cargo test --release -p stem-audio -- --ignored`.
    #[test]
    #[ignore]
    fn four_minutes_is_fast() {
        let (mono, _) = clicks(124.0, 240.0, 0.0);
        let pad = progression(
            &[
                &[45.0, 57.0, 60.0, 64.0],
                &[50.0, 57.0, 62.0, 65.0],
                &[40.0, 56.0, 59.0, 64.0],
                &[45.0, 57.0, 60.0, 64.0],
            ],
            30,
        );
        let st = AudioBuffer::new(SR, vec![mono.channels[0].clone(), mono.channels[0].clone()]);
        let pad = AudioBuffer::new(SR, vec![pad.channels[0].clone(), pad.channels[0].clone()]);
        let t = std::time::Instant::now();
        let a = analyze(&st, &pad);
        let dt = t.elapsed();
        eprintln!("analysis of 4 min: {dt:?} -> {:?} {:?}", a.tempo, a.key);
        assert!(dt.as_secs_f64() < 1.0);
    }
}
