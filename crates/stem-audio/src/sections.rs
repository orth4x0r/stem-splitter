//! Song structure: bar-aligned sections, with repeats sharing a letter (A B A C ...).
//!
//! 1. Per-beat features: normalised chroma (harmony) and a timbre vector (log band energies
//!    plus the loudness of the harmony and the drums, z-scored over the track).
//! 2. A beat-level self-similarity matrix: chroma cosine and a Gaussian of the timbre
//!    distance, averaged.
//! 3. Novelty from a Gaussian-tapered checkerboard kernel slid along the diagonal (J. Foote,
//!    "Automatic audio segmentation using a measure of audio novelty", ICME 2000), read at
//!    the bar lines only, so boundaries always fall on a downbeat; peaks above an adaptive
//!    threshold, at least two bars apart, become boundaries.
//! 4. Segments get letters greedily: a segment takes the letter of the most similar earlier
//!    segment when their similarity (along the diagonal, so the same chord sequence in the
//!    same order scores high, mixed with the whole-block average) clears a threshold relative
//!    to the track's typical similarity; otherwise it gets the next letter. Neighbours with
//!    the same letter merge.
//!
//! Reimplemented from the paper; no code copied.

use crate::analysis::{BeatGrid, GridDivision};
use crate::beatsync::{BeatFeatures, N_BANDS};

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub start: f64,
    pub end: f64,
    /// 0 = A, 1 = B, ... Repeats of the same material share a label.
    pub label: u8,
    /// Mean loudness relative to the loudest section, `0..=1`.
    pub energy: f32,
}

impl Section {
    /// "A", "B", ... ("Z" and beyond share "Z").
    pub fn letter(&self) -> char {
        letter(self.label)
    }
}

pub fn letter(label: u8) -> char {
    (b'A' + label.min(25)) as char
}

/// The likely chorus: the loudest label that occurs at least twice, if it is also among the
/// loudest material in the track. `None` when nothing repeats.
pub fn chorus_label(sections: &[Section]) -> Option<u8> {
    let max_e = sections.iter().map(|s| s.energy).fold(0.0, f32::max);
    let mut best: Option<(u8, f32)> = None;
    let mut labels: Vec<u8> = sections.iter().map(|s| s.label).collect();
    labels.sort_unstable();
    labels.dedup();
    for l in labels {
        let of: Vec<&Section> = sections.iter().filter(|s| s.label == l).collect();
        if of.len() < 2 {
            continue;
        }
        let len: f64 = of.iter().map(|s| s.end - s.start).sum();
        let e = of
            .iter()
            .map(|s| s.energy as f64 * (s.end - s.start))
            .sum::<f64>()
            / len.max(1e-9);
        let e = e as f32;
        if best.is_none_or(|(_, be)| e > be) {
            best = Some((l, e));
        }
    }
    best.filter(|(_, e)| *e >= 0.8 * max_e).map(|(l, _)| l)
}

/// Move the inner boundaries to the nearest bar line of `grid` (after a grid nudge), dropping
/// sections that collapse. The first section still starts at 0 and the last ends at `dur`.
pub fn resnap(sections: &[Section], grid: &BeatGrid, dur: f64) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::with_capacity(sections.len());
    let min = 0.5 * grid.period() * grid.beats_per_bar.max(1) as f64;
    for (i, s) in sections.iter().enumerate() {
        let start = if i == 0 {
            0.0
        } else {
            grid.snap(s.start, GridDivision::Bar).clamp(0.0, dur)
        };
        if let Some(last) = out.last_mut() {
            if start - last.start < min {
                continue;
            }
            last.end = start;
        }
        out.push(Section {
            start,
            end: dur,
            ..s.clone()
        });
    }
    if let Some(last) = out.last_mut() {
        last.end = dur;
    }
    // A collapse can leave equal neighbours.
    merge_equal(&mut out);
    out
}

fn merge_equal(v: &mut Vec<Section>) {
    let mut i = 1;
    while i < v.len() {
        if v[i].label == v[i - 1].label {
            let (a, b) = (v[i - 1].end - v[i - 1].start, v[i].end - v[i].start);
            v[i - 1].energy =
                ((v[i - 1].energy as f64 * a + v[i].energy as f64 * b) / (a + b).max(1e-9)) as f32;
            v[i - 1].end = v[i].end;
            v.remove(i);
        } else {
            i += 1;
        }
    }
}

/// Same label when the relative similarity clears this (0 = typical pair, 1 = identical).
const SAME: f32 = 0.42;
/// Boundaries at least this many bars apart.
const MIN_BARS: usize = 2;

struct Ssm {
    n: usize,
    s: Vec<f32>,
}

impl Ssm {
    fn at(&self, i: usize, j: usize) -> f32 {
        self.s[i * self.n + j]
    }
}

fn ssm(f: &BeatFeatures) -> Ssm {
    let n = f.len();
    let chroma: Vec<Option<[f32; 12]>> = f
        .chroma
        .iter()
        .zip(&f.harm_rms)
        .map(|(c, &r)| {
            let norm = c.iter().map(|v| v * v).sum::<f32>().sqrt();
            (norm > 1e-9 && r > 1e-5).then(|| c.map(|v| v / norm))
        })
        .collect();
    // Timbre: bands + log loudness of both parts, z-scored per dimension.
    const D: usize = N_BANDS + 2;
    let mut tv: Vec<[f32; D]> = (0..n)
        .map(|i| {
            let mut v = [0.0f32; D];
            v[..N_BANDS].copy_from_slice(&f.bands[i]);
            v[N_BANDS] = (f.harm_rms[i] + 1e-4).ln();
            v[N_BANDS + 1] = (f.drum_rms[i] + 1e-4).ln();
            v
        })
        .collect();
    for d in 0..D {
        let m = tv.iter().map(|v| v[d]).sum::<f32>() / n as f32;
        let var = tv.iter().map(|v| (v[d] - m).powi(2)).sum::<f32>() / n as f32;
        let sd = var.sqrt();
        for v in &mut tv {
            v[d] = if sd > 1e-3 { (v[d] - m) / sd } else { 0.0 };
        }
    }
    let dist = |i: usize, j: usize| -> f32 {
        tv[i]
            .iter()
            .zip(&tv[j])
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            / D as f32
    };
    // Median distance (on a sample of pairs) sets the Gaussian's width.
    let mut sample: Vec<f32> = Vec::new();
    let step = (n * n / 20_000).max(1);
    let mut k = 0usize;
    for i in 0..n {
        for j in (i + 1)..n {
            if k.is_multiple_of(step) {
                sample.push(dist(i, j));
            }
            k += 1;
        }
    }
    sample.sort_by(|a, b| a.total_cmp(b));
    let med = sample
        .get(sample.len() / 2)
        .copied()
        .unwrap_or(1.0)
        .max(1e-4);
    let mut s = vec![0.0f32; n * n];
    for i in 0..n {
        s[i * n + i] = 1.0;
        for j in (i + 1)..n {
            let sc = match (&chroma[i], &chroma[j]) {
                (Some(a), Some(b)) => a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>(),
                (None, None) => 1.0,
                _ => 0.0,
            };
            let st = (-dist(i, j) / med).exp();
            let v = 0.5 * sc + 0.5 * st;
            s[i * n + j] = v;
            s[j * n + i] = v;
        }
    }
    Ssm { n, s }
}

/// Foote novelty at every beat, kernel half-width `l` beats.
fn novelty(m: &Ssm, l: usize) -> Vec<f32> {
    let n = m.n;
    let l = l as isize;
    let sigma = (l as f32 / 2.0).max(1.0);
    let w: Vec<f32> = (-l..l)
        .map(|a| (-((a as f32 + 0.5).powi(2)) / (2.0 * sigma * sigma)).exp())
        .collect();
    (0..n as isize)
        .map(|i| {
            let (mut num, mut den) = (0.0f32, 0.0f32);
            for a in -l..l {
                let x = i + a;
                if x < 0 || x >= n as isize {
                    continue;
                }
                for b in -l..l {
                    let y = i + b;
                    if y < 0 || y >= n as isize {
                        continue;
                    }
                    let g = w[(a + l) as usize] * w[(b + l) as usize];
                    let sign = if (a < 0) == (b < 0) { 1.0 } else { -1.0 };
                    num += sign * g * m.at(x as usize, y as usize);
                    den += g;
                }
            }
            if den > 0.0 { num / den } else { 0.0 }
        })
        .collect()
}

/// Similarity of beat ranges `p` and `q`: the best diagonal-path mean (sliding the shorter
/// along the longer in whole bars) mixed with the mean over the whole block.
fn segment_sim(m: &Ssm, p: (usize, usize), q: (usize, usize), bpb: usize) -> f32 {
    let (lp, lq) = (p.1 - p.0, q.1 - q.0);
    let short = lp.min(lq);
    if short == 0 {
        return 0.0;
    }
    let slack = lp.max(lq) - short;
    let mut path = f32::MIN;
    let mut o = 0;
    while o <= slack {
        let (op, oq) = if lp > lq { (o, 0) } else { (0, o) };
        let v = (0..short)
            .map(|k| m.at(p.0 + op + k, q.0 + oq + k))
            .sum::<f32>()
            / short as f32;
        path = path.max(v);
        o += bpb.max(1);
    }
    let mut block = 0.0f32;
    for i in p.0..p.1 {
        for j in q.0..q.1 {
            block += m.at(i, j);
        }
    }
    block /= (lp * lq) as f32;
    // Very different lengths: the short one matching a corner of the long one says less.
    let ratio = short as f32 / lp.max(lq) as f32;
    let path_w = 0.6 * ratio.min(0.5) * 2.0;
    path_w * path + (1.0 - path_w) * block
}

/// Sections over the whole track from beat features.
pub(crate) fn detect_sections(f: &BeatFeatures) -> Vec<Section> {
    let n = f.len();
    let dur = f.times[n];
    let bpb = f.beats_per_bar.max(1);
    let loud: Vec<f32> = (0..n)
        .map(|i| (f.harm_rms[i].powi(2) + f.drum_rms[i].powi(2)).sqrt())
        .collect();
    let whole = || {
        vec![Section {
            start: 0.0,
            end: dur,
            label: 0,
            energy: 1.0,
        }]
    };
    if n < 4 * bpb {
        return whole();
    }
    let m = ssm(f);
    let l = (4 * bpb).min(n / 4).max(2);
    let nov = novelty(&m, l);

    // Candidate boundaries: bar starts (not the very first beat).
    let bars: Vec<usize> = (1..n).filter(|&i| f.bar[i]).collect();
    if bars.is_empty() {
        return whole();
    }
    let val: Vec<f32> = bars
        .iter()
        .map(|&i| {
            let lo = i.saturating_sub(1);
            let hi = (i + 1).min(n - 1);
            nov[lo..=hi].iter().copied().fold(f32::MIN, f32::max)
        })
        .collect();
    let mean = val.iter().sum::<f32>() / val.len() as f32;
    let sd = (val.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / val.len() as f32).sqrt();
    let th = (mean + 0.5 * sd).max(0.04);
    let mut cand: Vec<usize> = (0..bars.len())
        .filter(|&b| {
            val[b] >= th
                && (b == 0 || val[b] >= val[b - 1])
                && (b + 1 == bars.len() || val[b] >= val[b + 1])
        })
        .collect();
    cand.sort_by(|&a, &b| val[b].total_cmp(&val[a]));
    let min_beats = MIN_BARS * bpb;
    let mut cuts: Vec<usize> = Vec::new();
    for b in cand {
        let i = bars[b];
        if i < min_beats || n - i < min_beats {
            continue;
        }
        if cuts.iter().all(|&c| c.abs_diff(i) >= min_beats) {
            cuts.push(i);
        }
    }
    cuts.sort_unstable();
    let mut bounds = vec![0];
    bounds.extend(cuts);
    bounds.push(n);
    let segs: Vec<(usize, usize)> = bounds.windows(2).map(|w| (w[0], w[1])).collect();

    // Typical similarity between beats, for the relative threshold.
    let mut tot = 0.0f64;
    let mut cnt = 0usize;
    let step = (n / 200).max(1);
    for i in (0..n).step_by(step) {
        for j in (0..n).step_by(step) {
            if i != j {
                tot += m.at(i, j) as f64;
                cnt += 1;
            }
        }
    }
    let base = if cnt > 0 {
        (tot / cnt as f64) as f32
    } else {
        0.5
    };

    let mut labels: Vec<u8> = Vec::with_capacity(segs.len());
    let mut next = 0u8;
    for (k, &q) in segs.iter().enumerate() {
        let mut best: Option<(u8, f32)> = None;
        for (j, &p) in segs[..k].iter().enumerate() {
            let s = segment_sim(&m, p, q, bpb);
            let r = (s - base) / (1.0 - base).max(1e-3);
            if r >= SAME && best.is_none_or(|(_, br)| r > br) {
                best = Some((labels[j], r));
            }
        }
        match best {
            Some((l, _)) => labels.push(l),
            None => {
                labels.push(next);
                next = next.saturating_add(1);
            }
        }
    }
    let mut out: Vec<Section> = segs
        .iter()
        .zip(&labels)
        .map(|(&(a, b), &label)| Section {
            start: f.times[a],
            end: f.times[b],
            label,
            energy: loud[a..b].iter().sum::<f32>() / (b - a).max(1) as f32,
        })
        .collect();
    merge_equal(&mut out);
    // Letters in order of first appearance again after merging.
    let mut map: Vec<(u8, u8)> = Vec::new();
    for s in &mut out {
        let l = match map.iter().find(|(o, _)| *o == s.label) {
            Some((_, nl)) => *nl,
            None => {
                let nl = map.len() as u8;
                map.push((s.label, nl));
                nl
            }
        };
        s.label = l;
    }
    let max_e = out.iter().map(|s| s.energy).fold(0.0, f32::max);
    if max_e > 0.0 {
        out.iter_mut().for_each(|s| s.energy /= max_e);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioBuffer;
    use crate::beatsync::beat_features;

    const SR: u32 = 22_050;
    const BEAT: f64 = 0.5;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        }
    }

    /// One bar (4 beats at 120 BPM) of `chord` (MIDI notes) with drums if `drums`, into `x`
    /// at `bar`.
    fn bar(x: &mut [f32], bar: usize, chord: &[f32], drums: bool, gain: f32, rng: &mut Rng) {
        let start = (bar as f64 * 4.0 * BEAT * SR as f64) as usize;
        let len = (4.0 * BEAT * SR as f64) as usize;
        for &m in chord {
            let f = 440.0 * 2f32.powf((m - 69.0) / 12.0);
            for k in 0..len {
                let t = k as f32 / SR as f32;
                let v = (std::f32::consts::TAU * f * t).sin()
                    + 0.3 * (std::f32::consts::TAU * 2.0 * f * t).sin();
                x[start + k] += 0.06 * gain * v;
            }
        }
        if drums {
            for b in 0..8 {
                let at = start + (b as f64 * BEAT / 2.0 * SR as f64) as usize;
                let kick = b % 2 == 0;
                for k in 0..(0.12 * SR as f64) as usize {
                    let t = k as f32 / SR as f32;
                    let env = (-t * 30.0).exp();
                    let v = if kick {
                        (std::f32::consts::TAU * 55.0 * t).sin()
                    } else {
                        rng.next() * 0.5
                    };
                    x[at + k] += 0.5 * env * v;
                }
            }
        }
    }

    /// Verse (Am F C G, no drums), chorus (F G C C, drums, louder), bridge (Dm E), at 120 BPM.
    fn song(plan: &[(char, usize)]) -> (AudioBuffer, Vec<f64>) {
        let verse: [&[f32]; 4] = [
            &[45.0, 57.0, 60.0, 64.0],
            &[41.0, 57.0, 60.0, 65.0],
            &[48.0, 55.0, 60.0, 64.0],
            &[43.0, 55.0, 59.0, 62.0],
        ];
        let chorus: [&[f32]; 4] = [
            &[41.0, 65.0, 69.0, 72.0],
            &[43.0, 67.0, 71.0, 74.0],
            &[48.0, 67.0, 72.0, 76.0],
            &[48.0, 67.0, 72.0, 76.0],
        ];
        let bridge: [&[f32]; 2] = [&[38.0, 62.0, 65.0, 69.0], &[40.0, 64.0, 68.0, 71.0]];
        let total: usize = plan.iter().map(|p| p.1).sum();
        let mut x = vec![0.0f32; (total as f64 * 4.0 * BEAT * SR as f64) as usize + 10];
        let mut rng = Rng(11);
        let mut at = 0;
        let mut bounds = Vec::new();
        for &(kind, bars) in plan {
            bounds.push(at as f64 * 4.0 * BEAT);
            for b in 0..bars {
                match kind {
                    'v' => bar(&mut x, at + b, verse[b % 4], false, 1.0, &mut rng),
                    'c' => bar(&mut x, at + b, chorus[b % 4], true, 1.6, &mut rng),
                    _ => bar(&mut x, at + b, bridge[b % 2], false, 0.7, &mut rng),
                }
            }
            at += bars;
        }
        (AudioBuffer::new(SR, vec![x]), bounds)
    }

    fn grid(beats: usize) -> BeatGrid {
        BeatGrid {
            bpm: 120.0,
            beats: (0..beats).map(|i| i as f64 * BEAT).collect(),
            downbeat: 0,
            beats_per_bar: 4,
        }
    }

    #[test]
    fn verse_chorus_structure() {
        let plan = [('v', 8), ('c', 8), ('v', 8), ('c', 8), ('b', 4), ('c', 8)];
        let (buf, bounds) = song(&plan);
        let g = grid(44 * 4);
        let t0 = std::time::Instant::now();
        let f = beat_features(&buf, &buf, Some(&g));
        let s = detect_sections(&f);
        let took = t0.elapsed();
        let letters: String = s.iter().map(|x| x.letter()).collect();
        assert_eq!(letters, "ABABCB", "{s:?}");
        for (sec, want) in s.iter().zip(&bounds) {
            assert!((sec.start - want).abs() < 1e-6, "{s:?}");
        }
        assert_eq!(chorus_label(&s), Some(1));
        assert!(s[1].energy > s[0].energy);
        // Debug builds are slow; this is a sanity bound, not the release target.
        assert!(took.as_secs_f64() < 5.0, "{took:?}");
    }

    #[test]
    fn steady_loop_is_one_section() {
        let plan = [('c', 16)];
        let (buf, _) = song(&plan);
        let f = beat_features(&buf, &buf, Some(&grid(16 * 4)));
        let s = detect_sections(&f);
        assert_eq!(s.len(), 1, "{s:?}");
        assert_eq!(chorus_label(&s), None);
    }

    #[test]
    fn resnap_follows_the_grid() {
        let secs = vec![
            Section {
                start: 0.0,
                end: 8.0,
                label: 0,
                energy: 0.5,
            },
            Section {
                start: 8.0,
                end: 16.0,
                label: 1,
                energy: 1.0,
            },
            Section {
                start: 16.0,
                end: 20.0,
                label: 0,
                energy: 0.5,
            },
        ];
        // Bar lines moved one beat later (downbeat on beat 1).
        let g = BeatGrid {
            downbeat: 1,
            ..grid(40)
        };
        let r = resnap(&secs, &g, 20.0);
        assert_eq!(r.len(), 3);
        assert!((r[1].start - 8.5).abs() < 1e-9 && (r[0].end - 8.5).abs() < 1e-9);
        assert!((r[2].start - 16.5).abs() < 1e-9);
        assert_eq!(r[2].end, 20.0);
    }
}

#[cfg(test)]
mod speed {
    use crate::AudioBuffer;

    /// Sections + chords on a 4-minute stereo 44.1 kHz track. The target is well under a
    /// second in release builds; debug builds get a loose bound.
    #[test]
    fn four_minutes_is_fast() {
        let sr = 44_100u32;
        let n = 240 * sr as usize;
        let mut s = 0x1234_5678_9abc_def0u64;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                let noise = ((s >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.1;
                let t = i as f32 / sr as f32;
                noise * ((t * 4.0).fract() < 0.1) as u8 as f32
                    + 0.2 * (std::f32::consts::TAU * 220.0 * t).sin()
            })
            .collect();
        let buf = AudioBuffer::new(sr, vec![x.clone(), x]);
        let grid = crate::analysis::BeatGrid {
            bpm: 120.0,
            beats: (0..480).map(|i| i as f64 * 0.5).collect(),
            downbeat: 0,
            beats_per_bar: 4,
        };
        let t0 = std::time::Instant::now();
        let f = crate::beatsync::beat_features(&buf, &buf, Some(&grid));
        let secs = super::detect_sections(&f);
        let chords = crate::chords::detect_chords(&f);
        let took = t0.elapsed().as_secs_f64();
        eprintln!("sections + chords, 4 min: {took:.3} s");
        assert!(!secs.is_empty() && !chords.is_empty());
        let limit = if cfg!(debug_assertions) { 20.0 } else { 0.6 };
        assert!(took < limit, "{took}");
    }
}
