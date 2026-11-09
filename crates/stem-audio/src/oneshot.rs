//! One-shot / drum-kit extraction from a drums stem (or any stem).
//!
//! 1. **Hits**: onsets from [`crate::onsets`], each refined in the time domain to the start of
//!    its attack (where a 1 ms peak envelope first rises to -20 dB of the attack peak).
//! 2. **Cut**: from the attack to where the 5 ms RMS envelope falls 45 dB below its peak, or
//!    to the next hit, with a 0.5 ms fade-in and a short fade-out.
//! 3. **Features**: spectral centroid, low (< 150 Hz) and high (> 5 kHz) energy ratios,
//!    zero-crossing rate, decay time, and a 24-band log-mel shape.
//! 4. **Class**: kick / snare / hat / perc from those features (simple rules; no ML).
//! 5. **Clusters**: per class, greedy grouping of near-duplicates by cosine similarity of the
//!    mean-removed log-mel vectors (so level doesn't matter); the best-scoring hit (loud,
//!    decays naturally, little bleed before it) represents each cluster.
//!
//! [`write_kit`] then writes `Kick 01.wav`, `Snare 01.wav`, ... plus an SFZ that maps them to
//! General MIDI drum notes (kick C1 = 36, snare D1 = 38, closed hat F#1 = 42, ...).

// Frame / bin loops index several arrays at once; explicit indices read better here.
#![allow(clippy::needless_range_loop)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use realfft::RealFftPlanner;

use crate::onsets::{OnsetOptions, detect_onsets, mono};
use crate::{AudioBuffer, AudioError, AudioFormat, Result};

/// Rough drum class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HitClass {
    Kick,
    Snare,
    Hat,
    Perc,
}

impl HitClass {
    pub const ALL: [HitClass; 4] = [Self::Kick, Self::Snare, Self::Hat, Self::Perc];

    /// File-name stem: "Kick", "Snare", "Hat", "Perc".
    pub fn label(self) -> &'static str {
        match self {
            Self::Kick => "Kick",
            Self::Snare => "Snare",
            Self::Hat => "Hat",
            Self::Perc => "Perc",
        }
    }
    /// General MIDI drum notes to map samples of this class to, in order.
    fn notes(self, open_hat: bool) -> &'static [u8] {
        match self {
            Self::Kick => &[36, 35],
            Self::Snare => &[38, 40, 37, 39],
            Self::Hat if open_hat => &[46, 44],
            Self::Hat => &[42, 44],
            Self::Perc => &[41, 43, 45, 47, 48, 50, 49, 51, 39, 54, 56],
        }
    }
}

/// Note name with C1 = 36 (the convention most DAWs use for drum maps).
pub fn note_name(note: u8) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", N[note as usize % 12], note as i32 / 12 - 2)
}

/// Spectral / temporal description of one hit.
#[derive(Debug, Clone, PartialEq)]
pub struct HitFeatures {
    pub centroid_hz: f32,
    /// Share of energy below 150 Hz.
    pub low_ratio: f32,
    /// Share of energy above 5 kHz.
    pub high_ratio: f32,
    /// Share of energy in 150..800 Hz (drum body: snares and toms have it, hats don't).
    pub body_ratio: f32,
    /// Zero crossings per second over the first 50 ms.
    pub zcr: f32,
    /// Seconds from the peak until the envelope is 20 dB down.
    pub decay_s: f32,
    /// Mean-removed 24-band log-mel spectrum (for similarity).
    pub shape: Vec<f32>,
}

/// One detected hit in the source.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Frames `[start, end)` in the source.
    pub start: usize,
    pub end: usize,
    pub class: HitClass,
    /// Peak absolute sample value.
    pub peak: f32,
    /// True when the hit decayed below the threshold before the next one started.
    pub natural_end: bool,
    pub features: HitFeatures,
    /// Higher is a better representative (loud, clean, complete).
    pub score: f32,
    /// Index into [`Kit::samples`] of the cluster this hit belongs to.
    pub cluster: usize,
}

/// A kit sample: the representative of one cluster.
#[derive(Debug, Clone, PartialEq)]
pub struct KitSample {
    /// "Kick 01".
    pub name: String,
    pub class: HitClass,
    /// Index into [`Kit::hits`] of the representative.
    pub hit: usize,
    /// How many hits sound like this one.
    pub count: usize,
    /// MIDI note in the SFZ map.
    pub note: u8,
    /// The cut, faded, normalised audio.
    pub audio: AudioBuffer,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Kit {
    pub hits: Vec<Hit>,
    /// Ordered by class, then by how often each sound occurs.
    pub samples: Vec<KitSample>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KitOptions {
    /// Onset sensitivity `0..=1`.
    pub sensitivity: f32,
    /// Where a hit ends: this many dB below its peak (negative).
    pub end_db: f32,
    /// Longest one-shot, seconds.
    pub max_len: f32,
    /// Hits quieter than this many dB below the loudest are ignored (bleed, ghost notes).
    pub min_level_db: f32,
    /// Cosine similarity above which two hits count as the same sound.
    pub similarity: f32,
    /// At most this many samples per class.
    pub max_per_class: usize,
    /// Peak-normalise each sample to -0.1 dBFS.
    pub normalize: bool,
}

impl Default for KitOptions {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            end_db: -45.0,
            max_len: 2.0,
            min_level_db: -30.0,
            similarity: 0.9,
            max_per_class: 8,
            normalize: true,
        }
    }
}

/// Find, classify and de-duplicate the hits in `buf`.
pub fn extract_kit(
    buf: &AudioBuffer,
    opts: &KitOptions,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<Kit> {
    let sr = buf.sample_rate.max(1) as f32;
    let x = mono(buf);
    if x.is_empty() {
        return Ok(Kit::default());
    }
    progress(0.05);
    let onsets = detect_onsets(
        buf,
        &OnsetOptions {
            sensitivity: opts.sensitivity,
            min_gap: 0.03,
        },
    );
    progress(0.35);
    if cancel.load(Ordering::Relaxed) {
        return Err(AudioError::Cancelled);
    }

    // Envelopes: 1 ms peak (attack refinement), 5 ms RMS (decay / end).
    let pk_block = ((sr * 0.001) as usize).max(1);
    let pk_env: Vec<f32> = x
        .chunks(pk_block)
        .map(|c| c.iter().fold(0.0f32, |m, v| m.max(v.abs())))
        .collect();
    let rms_block = ((sr * 0.005) as usize).max(1);
    let rms_env: Vec<f32> = x
        .chunks(rms_block)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .collect();

    // 1. Refine starts.
    let search = (sr * 0.015) as usize;
    let mut starts: Vec<usize> = Vec::new();
    for o in &onsets {
        let a = o.pos.saturating_sub(search) / pk_block;
        let b = ((o.pos + search) / pk_block + 1).min(pk_env.len());
        if a >= b {
            continue;
        }
        let (pi, pv) = (a..b).fold((a, 0.0f32), |(bi, bv), i| {
            if pk_env[i] > bv {
                (i, pk_env[i])
            } else {
                (bi, bv)
            }
        });
        if pv <= 0.0 {
            continue;
        }
        // Walk back to where the envelope is 20 dB under the peak.
        let lim = starts.last().map_or(0, |s| s / pk_block + 1);
        let mut i = pi;
        while i > lim && pk_env[i - 1] > pv * 0.1 {
            i -= 1;
        }
        // Exact sample: first one above 10 % of the peak in that block, minus 0.5 ms.
        let blk = i * pk_block;
        let first = (blk..(blk + pk_block).min(x.len()))
            .find(|&j| x[j].abs() > pv * 0.1)
            .unwrap_or(blk);
        let s = first.saturating_sub((sr * 0.0005) as usize);
        if starts.last().is_none_or(|&l| s > l + (sr * 0.02) as usize) {
            starts.push(s);
        }
    }
    progress(0.5);

    // 2. Cut each hit.
    let max_len = (opts.max_len * sr) as usize;
    let end_gain = 10f32.powf(opts.end_db / 20.0);
    struct Cut {
        start: usize,
        end: usize,
        natural: bool,
        peak: f32,
        decay_s: f32,
        pre_db: f32,
    }
    let mut cuts: Vec<Cut> = Vec::new();
    for (n, &s) in starts.iter().enumerate() {
        let next = starts.get(n + 1).copied().unwrap_or(x.len());
        let hard_end = next.min(s + max_len).min(x.len());
        let b0 = s / rms_block;
        let b1 = hard_end.div_ceil(rms_block).min(rms_env.len());
        if b0 >= b1 {
            continue;
        }
        let (pb, peak_rms) = (b0..b1.min(b0 + 20)).fold((b0, 0.0f32), |(bi, bv), i| {
            if rms_env[i] > bv {
                (i, rms_env[i])
            } else {
                (bi, bv)
            }
        });
        let mut end = hard_end;
        let mut natural = false;
        if let Some(e) = (pb..b1).find(|&i| rms_env[i] < peak_rms * end_gain) {
            end = (e * rms_block).min(hard_end);
            natural = true;
        } else if hard_end == x.len() || hard_end == s + max_len {
            natural = hard_end == x.len();
        }
        let decay_s = (pb..b1)
            .find(|&i| rms_env[i] < peak_rms * 0.1)
            .map_or((b1 - pb) as f32, |e| (e - pb) as f32)
            * rms_block as f32
            / sr;
        let peak = x[s..end].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        // Energy in the 20 ms before the hit relative to the hit: bleed / overlapping tails.
        let pre_a = s.saturating_sub((sr * 0.02) as usize);
        let pre = rms(&x[pre_a..s]);
        let pre_db = 20.0 * ((pre + 1e-9) / (peak_rms + 1e-9)).log10();
        if end > s + (sr * 0.015) as usize {
            cuts.push(Cut {
                start: s,
                end,
                natural,
                peak,
                decay_s,
                pre_db,
            });
        }
    }
    let loudest = cuts.iter().fold(0.0f32, |m, c| m.max(c.peak));
    let min_peak = loudest * 10f32.powf(opts.min_level_db / 20.0);
    cuts.retain(|c| c.peak >= min_peak && c.peak > 1e-4);
    progress(0.6);
    if cancel.load(Ordering::Relaxed) {
        return Err(AudioError::Cancelled);
    }

    // 3. Features + 4. class.
    let mut fx = Features::new(sr);
    let mut hits: Vec<Hit> = cuts
        .iter()
        .map(|c| {
            let pre = &x[c.start.saturating_sub(FEAT_FFT)..c.start];
            let features = fx.compute(&x[c.start..c.end], pre, c.decay_s);
            let class = classify(&features);
            let peak_db = 20.0 * c.peak.max(1e-9).log10();
            let score =
                peak_db + if c.natural { 6.0 } else { 0.0 } - 0.8 * (c.pre_db + 30.0).max(0.0);
            Hit {
                start: c.start,
                end: c.end,
                class,
                peak: c.peak,
                natural_end: c.natural,
                features,
                score,
                cluster: usize::MAX,
            }
        })
        .collect();
    progress(0.8);

    // 5. Cluster per class.
    let mut order: Vec<usize> = (0..hits.len()).collect();
    order.sort_by(|&a, &b| hits[b].score.total_cmp(&hits[a].score));
    // (class, representative hit, members)
    let mut clusters: Vec<(HitClass, usize, usize)> = Vec::new();
    for &h in &order {
        let best = clusters
            .iter()
            .enumerate()
            .filter(|(_, c)| c.0 == hits[h].class)
            .map(|(i, c)| (i, similarity(&hits[h].features, &hits[c.1].features)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, sim)) if sim >= opts.similarity => {
                clusters[i].2 += 1;
                hits[h].cluster = i;
            }
            _ => {
                hits[h].cluster = clusters.len();
                clusters.push((hits[h].class, h, 1));
            }
        }
    }

    // Kit samples: per class, most frequent first (ties: better score), capped.
    let mut samples = Vec::new();
    let mut remap = vec![usize::MAX; clusters.len()];
    for class in HitClass::ALL {
        let mut cs: Vec<usize> = (0..clusters.len())
            .filter(|&i| clusters[i].0 == class)
            .collect();
        cs.sort_by(|&a, &b| {
            clusters[b].2.cmp(&clusters[a].2).then(
                hits[clusters[b].1]
                    .score
                    .total_cmp(&hits[clusters[a].1].score),
            )
        });
        let mut used: Vec<u8> = Vec::new();
        for (n, &ci) in cs.iter().take(opts.max_per_class).enumerate() {
            let h = &hits[clusters[ci].1];
            let open = class == HitClass::Hat && h.features.decay_s > 0.2;
            let note = class
                .notes(open)
                .iter()
                .chain(HitClass::Perc.notes(false))
                .copied()
                .find(|n| !used.contains(n) && !samples.iter().any(|s: &KitSample| s.note == *n))
                .unwrap_or_else(|| 60 + samples.len() as u8);
            used.push(note);
            remap[ci] = samples.len();
            samples.push(KitSample {
                name: format!("{} {:02}", class.label(), n + 1),
                class,
                hit: clusters[ci].1,
                count: clusters[ci].2,
                note,
                audio: cut_audio(buf, h, opts.normalize),
            });
        }
    }
    for h in &mut hits {
        h.cluster = remap.get(h.cluster).copied().unwrap_or(usize::MAX);
    }
    progress(1.0);
    Ok(Kit { hits, samples })
}

/// The hit's audio with a 0.5 ms fade-in and a fade-out (10 ms, or 15 ms when the next hit
/// cut it short; never more than a fifth of its length), optionally peak-normalised.
pub fn cut_audio(buf: &AudioBuffer, hit: &Hit, normalize: bool) -> AudioBuffer {
    let sr = buf.sample_rate as f32;
    let mut a = buf.slice(hit.start, hit.end);
    let len = a.frames();
    let fin = ((sr * 0.0005) as usize).min(len / 4);
    let fout = ((sr * if hit.natural_end { 0.010 } else { 0.015 }) as usize).min(len / 5);
    a.fade(fin, fout);
    if normalize {
        a.normalize(0.989);
    }
    a
}

fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        0.0
    } else {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }
}

/// Kick / snare / hat / perc from the features.
pub fn classify(f: &HitFeatures) -> HitClass {
    if f.low_ratio > 0.45 || (f.centroid_hz < 350.0 && f.high_ratio < 0.1) {
        HitClass::Kick
    } else if f.centroid_hz > 4500.0 && f.body_ratio < 0.06 && f.high_ratio > 0.4 {
        HitClass::Hat
    } else if f.centroid_hz > 700.0
        && f.zcr > 1500.0
        && f.low_ratio < 0.35
        && ((f.centroid_hz < 7500.0 && f.body_ratio > 0.03) || f.body_ratio > 0.15)
    {
        HitClass::Snare
    } else {
        HitClass::Perc
    }
}

/// Cosine similarity of two hits' log-mel shapes, scaled down when their lengths of decay
/// differ a lot (a tight and a ringing snare are different samples).
pub fn similarity(a: &HitFeatures, b: &HitFeatures) -> f32 {
    let dot: f32 = a.shape.iter().zip(&b.shape).map(|(x, y)| x * y).sum();
    let na: f32 = a.shape.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.shape.iter().map(|x| x * x).sum::<f32>().sqrt();
    let cos = dot / (na * nb + 1e-9);
    let (d1, d2) = (a.decay_s.max(0.005), b.decay_s.max(0.005));
    let ratio = d1.max(d2) / d1.min(d2);
    if ratio > 2.0 { cos * 0.9 } else { cos }
}

const N_MEL: usize = 24;
const FEAT_FFT: usize = 4096;

struct Features {
    sr: f32,
    fft: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
    /// Triangular mel filters as (first bin, weights).
    mel: Vec<(usize, Vec<f32>)>,
}

impl Features {
    fn new(sr: f32) -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FEAT_FFT);
        let hz_to_mel = |f: f32| 2595.0 * (1.0 + f / 700.0).log10();
        let mel_to_hz = |m: f32| 700.0 * (10f32.powf(m / 2595.0) - 1.0);
        let (lo, hi) = (hz_to_mel(30.0), hz_to_mel((sr * 0.5).min(16_000.0)));
        let pts: Vec<f32> = (0..N_MEL + 2)
            .map(|i| mel_to_hz(lo + (hi - lo) * i as f32 / (N_MEL + 1) as f32))
            .collect();
        let bin_hz = sr / FEAT_FFT as f32;
        let mel = (0..N_MEL)
            .map(|m| {
                let (f0, f1, f2) = (pts[m], pts[m + 1], pts[m + 2]);
                let b0 = (f0 / bin_hz).floor() as usize;
                let b2 = ((f2 / bin_hz).ceil() as usize).min(FEAT_FFT / 2);
                let w = (b0..=b2)
                    .map(|b| {
                        let f = b as f32 * bin_hz;
                        if f < f1 {
                            ((f - f0) / (f1 - f0)).max(0.0)
                        } else {
                            ((f2 - f) / (f2 - f1)).max(0.0)
                        }
                    })
                    .collect();
                (b0, w)
            })
            .collect();
        Self { sr, fft, mel }
    }

    /// Power spectrum of the first `n` samples of `x` (zero padded to the FFT size), flat
    /// at the start and tapered over the last quarter.
    fn power(&self, x: &[f32], n: usize) -> Vec<f32> {
        let mut inp = self.fft.make_input_vec();
        let taper = n / 4;
        for (i, v) in x.iter().take(n).enumerate() {
            let w = if i + taper >= n {
                let t = (n - i) as f32 / taper.max(1) as f32;
                0.5 - 0.5 * (std::f32::consts::PI * t).cos()
            } else {
                1.0
            };
            inp[i] = v * w;
        }
        let mut spec = self.fft.make_output_vec();
        let _ = self.fft.process(&mut inp, &mut spec);
        spec.iter().map(|c| c.norm_sqr()).collect()
    }

    /// Features of hit `x`. `pre` is the audio just before it: its spectrum is subtracted so
    /// a ringing tail (a kick under the next hat) doesn't decide the class.
    fn compute(&mut self, x: &[f32], pre: &[f32], decay_s: f32) -> HitFeatures {
        let n = FEAT_FFT.min(x.len());
        let mut pow = self.power(x, n);
        if !pre.is_empty() {
            let tail = &pre[pre.len().saturating_sub(n)..];
            let pp = self.power(tail, n);
            for (p, q) in pow.iter_mut().zip(&pp) {
                *p = (*p - q).max(0.0);
            }
        }
        let bin_hz = self.sr / FEAT_FFT as f32;
        let total: f32 = pow.iter().sum::<f32>() + 1e-12;
        let centroid_hz = pow
            .iter()
            .enumerate()
            .map(|(k, p)| k as f32 * bin_hz * p)
            .sum::<f32>()
            / total;
        let lo_bin = (150.0 / bin_hz) as usize;
        let hi_bin = (5000.0 / bin_hz) as usize;
        let low_ratio = pow[..=lo_bin.min(pow.len() - 1)].iter().sum::<f32>() / total;
        let high_ratio = pow[hi_bin.min(pow.len())..].iter().sum::<f32>() / total;
        let body = (lo_bin + 1).min(pow.len())..((800.0 / bin_hz) as usize).min(pow.len());
        let body_ratio = pow[body].iter().sum::<f32>() / total;
        let z = x.len().min((self.sr * 0.05) as usize);
        let crossings = x[..z]
            .windows(2)
            .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
            .count();
        let zcr = crossings as f32 / (z.max(1) as f32 / self.sr);
        let mut shape: Vec<f32> = self
            .mel
            .iter()
            .map(|(b0, w)| {
                let e: f32 = w.iter().enumerate().map(|(i, w)| w * pow[b0 + i]).sum();
                (e + 1e-10).log10()
            })
            .collect();
        // Bands more than 30 dB under the strongest are floored: bleed from other drums
        // (a hat over a kick) lives down there and shouldn't split a cluster.
        let top = shape.iter().copied().fold(f32::MIN, f32::max);
        shape.iter_mut().for_each(|v| *v = v.max(top - 3.0));
        let mean = shape.iter().sum::<f32>() / N_MEL as f32;
        shape.iter_mut().for_each(|v| *v -= mean);
        HitFeatures {
            centroid_hz,
            low_ratio,
            high_ratio,
            body_ratio,
            zcr,
            decay_s,
            shape,
        }
    }
}

// ---- writing a kit -------------------------------------------------------------------------

/// Write `samples` into `dir` as `<name>.<ext>` plus `<sfz_name>.sfz`. Returns the written
/// paths (SFZ last).
pub fn write_kit(
    dir: &Path,
    samples: &[&KitSample],
    format: AudioFormat,
    sfz_name: &str,
    title: &str,
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let mut out = Vec::new();
    let mut files = Vec::new();
    for s in samples {
        let file = format!("{}.{}", s.name, format.extension());
        let path = dir.join(&file);
        crate::write_audio(&path, &s.audio, format)?;
        out.push(path);
        files.push(file);
    }
    let sfz = sfz(samples, &files, title);
    let path = dir.join(format!("{sfz_name}.sfz"));
    std::fs::write(&path, sfz)?;
    out.push(path);
    Ok(out)
}

/// SFZ text for `samples` (file names in `files`, same order). One group per class; hats
/// choke each other (an open hat is cut by the next closed one).
pub fn sfz(samples: &[&KitSample], files: &[String], title: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "// {title}: drum kit extracted by Stem Splitter\n// Notes: C1 = 36 (General MIDI drum map)\n\n<control>\ndefault_path=./\n\n<global>\nloop_mode=one_shot\namp_veltrack=80\n"
    ));
    for class in HitClass::ALL {
        let items: Vec<(usize, &&KitSample)> = samples
            .iter()
            .enumerate()
            .filter(|(_, k)| k.class == class)
            .collect();
        if items.is_empty() {
            continue;
        }
        s.push_str(&format!("\n// {}\n<group>\n", class.label()));
        if class == HitClass::Hat {
            s.push_str("group=1\noff_by=1\n");
        }
        for (i, k) in items {
            s.push_str(&format!(
                "<region> sample={} key={} // {}\n",
                files[i],
                k.note,
                note_name(k.note)
            ));
        }
    }
    s
}

// ---- tests ---------------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::onsets::synth::Rng;
    use std::f32::consts::TAU;

    const SR: u32 = 44_100;

    pub fn kick(x: &mut [f32], at: usize, gain: f32) {
        let sr = SR as f32;
        let mut ph = 0.0f32;
        for k in 0..(0.4 * sr) as usize {
            if at + k >= x.len() {
                break;
            }
            let t = k as f32 / sr;
            ph += TAU * (48.0 + 90.0 * (-t * 30.0).exp()) / sr;
            x[at + k] += gain * ph.sin() * (-t * 9.0).exp();
        }
    }

    pub fn snare(x: &mut [f32], at: usize, gain: f32, rng: &mut Rng) {
        let sr = SR as f32;
        let mut lp = 0.0f32;
        for k in 0..(0.25 * sr) as usize {
            if at + k >= x.len() {
                break;
            }
            let t = k as f32 / sr;
            let n = rng.next();
            lp += 0.45 * (n - lp); // one-pole low-pass (~4 kHz): snare wires, not sizzle
            let body = (TAU * 190.0 * t).sin() * (-t * 25.0).exp() * 0.6;
            x[at + k] += gain * (body + lp * (-t * 16.0).exp() * 1.2);
        }
    }

    pub fn hat(x: &mut [f32], at: usize, gain: f32, len: f32, rng: &mut Rng) {
        let sr = SR as f32;
        let mut prev = 0.0f32;
        for k in 0..(len * sr) as usize {
            if at + k >= x.len() {
                break;
            }
            let t = k as f32 / sr;
            let n = rng.next();
            let hp = n - prev; // first difference: high-passed noise
            prev = n;
            x[at + k] += gain * 0.6 * hp * (-t * 3.0 / len).exp();
        }
    }

    /// Two bars of boom-bap at 90 BPM, repeated `bars / 2` times: kick on 1 and 3, snare on
    /// 2 and 4, closed hats on eighths, one open hat per two bars.
    pub fn pattern(bars: usize) -> (AudioBuffer, Vec<(usize, HitClass)>) {
        let sr = SR as f32;
        let eighth = 60.0 / 90.0 / 2.0;
        let n = ((bars * 8) as f32 * eighth * sr) as usize + SR as usize;
        let mut x = vec![0.0f32; n];
        let mut rng = Rng(1234);
        let mut truth = Vec::new();
        for s in 0..bars * 8 {
            let at = (s as f32 * eighth * sr) as usize + 2000;
            let pos = s % 8;
            let vel = 0.8 + 0.2 * ((s * 7 % 5) as f32 / 4.0);
            match pos {
                0 | 4 => {
                    kick(&mut x, at, 0.8 * vel);
                    truth.push((at, HitClass::Kick));
                }
                2 | 6 => {
                    snare(&mut x, at, 0.6 * vel, &mut rng);
                    truth.push((at, HitClass::Snare));
                }
                7 if s % 16 == 15 => {
                    hat(&mut x, at, 0.35, 0.3, &mut rng);
                    truth.push((at, HitClass::Hat));
                }
                _ => {
                    hat(&mut x, at, 0.3 * vel, 0.05, &mut rng);
                    truth.push((at, HitClass::Hat));
                }
            }
        }
        (AudioBuffer::new(SR, vec![x.clone(), x]), truth)
    }

    fn run(buf: &AudioBuffer) -> Kit {
        extract_kit(
            buf,
            &KitOptions::default(),
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn single_hits_classify() {
        let mut rng = Rng(5);
        let one = |f: &dyn Fn(&mut Vec<f32>)| {
            let mut x = vec![0.0f32; SR as usize];
            f(&mut x);
            let kit = run(&AudioBuffer::new(SR, vec![x]));
            assert_eq!(kit.hits.len(), 1, "{:?}", kit.hits);
            (kit.hits[0].class, kit.hits[0].features.clone())
        };
        let (c, f) = one(&|x| kick(x, 4000, 0.8));
        assert_eq!(c, HitClass::Kick, "{f:?}");
        let (c, f) = one(&|x| snare(x, 4000, 0.6, &mut Rng(9)));
        assert_eq!(c, HitClass::Snare, "{f:?}");
        let (c, f) = one(&|x| hat(x, 4000, 0.3, 0.05, &mut Rng(3)));
        assert_eq!(c, HitClass::Hat, "{f:?}");
        let _ = &mut rng;
    }

    #[test]
    fn pattern_detects_and_classifies() {
        let (buf, truth) = pattern(4);
        let kit = run(&buf);
        let tol = (0.004 * SR as f32) as usize;
        let mut found = 0;
        let mut right = 0;
        for &(at, class) in &truth {
            if let Some(h) = kit.hits.iter().find(|h| h.start.abs_diff(at) <= tol) {
                found += 1;
                if h.class == class {
                    right += 1;
                }
            }
        }
        eprintln!(
            "{} hits, {found}/{} found within 4 ms, {right} classified right",
            kit.hits.len(),
            truth.len()
        );
        assert!(
            found as f32 >= truth.len() as f32 * 0.95,
            "found {found}/{}",
            truth.len()
        );
        assert!(
            right as f32 >= found as f32 * 0.9,
            "classified {right}/{found}"
        );
        assert!(
            kit.hits.len() <= truth.len() + 1,
            "false positives: {}",
            kit.hits.len()
        );
    }

    #[test]
    fn clusters_collapse_repeats() {
        let (buf, _) = pattern(4);
        let kit = run(&buf);
        let count = |c: HitClass| kit.samples.iter().filter(|s| s.class == c).count();
        let names: Vec<_> = kit
            .samples
            .iter()
            .map(|s| format!("{} x{} {}", s.name, s.count, note_name(s.note)))
            .collect();
        eprintln!("{names:?}");
        assert_eq!(count(HitClass::Kick), 1, "{names:?}");
        assert_eq!(count(HitClass::Snare), 1, "{names:?}");
        // Closed and open hats are different sounds.
        assert!((1..=3).contains(&count(HitClass::Hat)), "{names:?}");
        assert_eq!(kit.samples[0].name, "Kick 01");
        assert_eq!(kit.samples[0].note, 36);
        let snare = kit
            .samples
            .iter()
            .find(|s| s.class == HitClass::Snare)
            .unwrap();
        assert_eq!(snare.note, 38);
        assert_eq!(snare.count, 8);
    }

    #[test]
    fn cuts_end_at_decay_or_next_hit() {
        let (buf, truth) = pattern(2);
        let kit = run(&buf);
        let sr = SR as f32;
        for h in &kit.hits {
            let next = truth.iter().map(|t| t.0).find(|&t| t > h.start + 100);
            if let Some(nx) = next {
                assert!(
                    h.end <= nx,
                    "hit {}..{} runs into the next at {nx}",
                    h.start,
                    h.end
                );
            }
            assert!(h.end - h.start >= (0.015 * sr) as usize);
        }
        // The kick alone (no next hit for a while) rings ~0.35 s to -45 dB.
        let mut x = vec![0.0f32; SR as usize * 2];
        kick(&mut x, 1000, 0.8);
        let k = run(&AudioBuffer::new(SR, vec![x]));
        let len = (k.hits[0].end - k.hits[0].start) as f32 / sr;
        assert!(
            k.hits[0].natural_end && (0.25..0.45).contains(&len),
            "{len}"
        );
        // Fades: starts and ends near zero.
        let a = &k.samples[0].audio.channels[0];
        assert!(a[0].abs() < 1e-3 && a[a.len() - 1].abs() < 1e-3);
    }

    #[test]
    fn writes_kit_and_sfz() {
        let (buf, _) = pattern(2);
        let kit = run(&buf);
        let dir = std::env::temp_dir().join(format!("stem-kit-test-{}", std::process::id()));
        let picks: Vec<&KitSample> = kit.samples.iter().collect();
        let paths = write_kit(
            &dir,
            &picks,
            AudioFormat::Wav(crate::WavFormat::Pcm24),
            "Demo Kit",
            "Demo",
        )
        .unwrap();
        assert!(dir.join("Kick 01.wav").exists());
        assert!(dir.join("Snare 01.wav").exists());
        assert!(dir.join("Hat 01.wav").exists());
        let sfz = std::fs::read_to_string(paths.last().unwrap()).unwrap();
        assert!(
            sfz.contains("<region> sample=Kick 01.wav key=36 // C1"),
            "{sfz}"
        );
        assert!(sfz.contains("sample=Snare 01.wav key=38 // D1"), "{sfz}");
        assert!(sfz.contains("key=42 // F#1"), "{sfz}");
        assert!(sfz.contains("off_by=1"));
        let back = crate::decode_file(&dir.join("Kick 01.wav")).unwrap();
        assert_eq!(back.frames(), kit.samples[0].audio.frames());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn note_names() {
        assert_eq!(note_name(36), "C1");
        assert_eq!(note_name(38), "D1");
        assert_eq!(note_name(42), "F#1");
        assert_eq!(note_name(60), "C3");
    }
}
