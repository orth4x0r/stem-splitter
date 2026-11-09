//! Chord estimation: per-beat chroma matched against chord templates, smoothed with Viterbi.
//!
//! Each beat's chroma (from the harmonic parts) is scored against binary templates for major,
//! minor, dominant-7th and minor-7th chords on all 12 roots (cosine similarity, sevenths
//! slightly penalised so a plain triad stays a triad), plus a bonus when the bass note is the
//! root. "N" (no chord) wins on quiet beats and on flat, noise-like chroma. A Viterbi pass
//! with a fixed switch penalty (an HMM with uniform transitions) removes one-beat flickers.
//! Template matching + HMM smoothing is the textbook baseline (e.g. Fujishima 1999; Sheh &
//! Ellis 2003); reimplemented here from those descriptions.

use crate::beatsync::BeatFeatures;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quality {
    Major,
    Minor,
    Dom7,
    Min7,
}

impl Quality {
    const ALL: [Quality; 4] = [Quality::Major, Quality::Minor, Quality::Dom7, Quality::Min7];

    fn intervals(self) -> &'static [usize] {
        match self {
            Quality::Major => &[0, 4, 7],
            Quality::Minor => &[0, 3, 7],
            Quality::Dom7 => &[0, 4, 7, 10],
            Quality::Min7 => &[0, 3, 7, 10],
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Quality::Major => "",
            Quality::Minor => "m",
            Quality::Dom7 => "7",
            Quality::Min7 => "m7",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    /// Pitch class of the root, 0 = C.
    pub root: u8,
    pub quality: Quality,
}

const ROOT_NAMES: [&str; 12] = [
    "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

impl Chord {
    /// "C", "F#m", "G7", "Bbm7".
    pub fn name(&self) -> String {
        format!(
            "{}{}",
            ROOT_NAMES[self.root as usize % 12],
            self.quality.suffix()
        )
    }
}

/// A run of beats with one chord; `chord` is `None` for "N" (no chord).
#[derive(Debug, Clone, PartialEq)]
pub struct ChordSpan {
    pub start: f64,
    pub end: f64,
    pub chord: Option<Chord>,
}

impl ChordSpan {
    /// The chord name, or "N".
    pub fn label(&self) -> String {
        self.chord.map_or_else(|| "N".to_owned(), |c| c.name())
    }
}

/// The span playing at `t`.
pub fn chord_at(spans: &[ChordSpan], t: f64) -> Option<&ChordSpan> {
    let i = spans.partition_point(|s| s.end <= t);
    spans.get(i).filter(|s| s.start <= t)
}

/// Chords sounding in `[a, b)`, in order, without repeats in a row and without "N".
/// A span has to cover at least a quarter of the range or 0.25 s to count.
pub fn chords_in(spans: &[ChordSpan], a: f64, b: f64) -> Vec<Chord> {
    let min = (0.25 * (b - a)).min(0.25);
    let mut out: Vec<Chord> = Vec::new();
    for s in spans {
        let overlap = s.end.min(b) - s.start.max(a);
        if overlap <= 0.0 || overlap < min {
            continue;
        }
        if let Some(c) = s.chord
            && out.last() != Some(&c)
        {
            out.push(c);
        }
    }
    out
}

/// Score multiplier for 7th chords: a triad wins ties.
const SEVENTH_PENALTY: f32 = 0.94;
/// Bonus for the bass note being the root.
const BASS_BONUS: f32 = 0.15;
/// Score of "no chord" on an ordinary beat; flat chroma scores ~0.5 against a triad.
const N_SCORE: f32 = 0.56;
/// Viterbi cost of changing chord.
const SWITCH: f32 = 0.22;

fn states() -> Vec<Chord> {
    let mut v = Vec::with_capacity(48);
    for q in Quality::ALL {
        for root in 0..12u8 {
            v.push(Chord { root, quality: q });
        }
    }
    v
}

fn normalized(c: &[f32; 12]) -> Option<[f32; 12]> {
    let n = c.iter().map(|v| v * v).sum::<f32>().sqrt();
    (n > 1e-9).then(|| c.map(|v| v / n))
}

/// Per-beat scores: one row per beat, one column per state in [`states`] then "N" last.
fn scores(f: &BeatFeatures, states: &[Chord]) -> Vec<Vec<f32>> {
    let n = f.len();
    let mut loud: Vec<f32> = f.harm_rms.clone();
    loud.sort_by(|a, b| a.total_cmp(b));
    let p90 = loud.get(loud.len() * 9 / 10).copied().unwrap_or(0.0);
    let quiet = (p90 * 0.04).max(1e-5);
    (0..n)
        .map(|i| {
            let mut row = vec![0.0f32; states.len() + 1];
            let c = if f.harm_rms[i] < quiet {
                None
            } else {
                normalized(&f.chroma[i])
            };
            let Some(c) = c else {
                row[states.len()] = 10.0;
                return row;
            };
            let bmax = f.bass[i].iter().copied().fold(0.0, f32::max);
            for (s, ch) in states.iter().enumerate() {
                let iv = ch.quality.intervals();
                let dot: f32 = iv.iter().map(|d| c[(ch.root as usize + d) % 12]).sum();
                let mut v = dot / (iv.len() as f32).sqrt();
                if iv.len() == 4 {
                    v *= SEVENTH_PENALTY;
                }
                if bmax > 1e-9 {
                    v += BASS_BONUS * f.bass[i][ch.root as usize] / bmax;
                }
                row[s] = v;
            }
            row[states.len()] = N_SCORE;
            row
        })
        .collect()
}

/// Most likely state path for per-beat `scores` with a constant `switch` cost.
fn viterbi(scores: &[Vec<f32>], switch: f32) -> Vec<usize> {
    let n = scores.len();
    if n == 0 {
        return Vec::new();
    }
    let k = scores[0].len();
    let mut best = scores[0].clone();
    let mut back = vec![vec![0u32; k]; n];
    for t in 1..n {
        let (arg, top) = best
            .iter()
            .enumerate()
            .fold((0, f32::MIN), |m, (j, &v)| if v > m.1 { (j, v) } else { m });
        let mut next = vec![0.0f32; k];
        for s in 0..k {
            let (from, v) = if best[s] >= top - switch {
                (s, best[s])
            } else {
                (arg, top - switch)
            };
            next[s] = v + scores[t][s];
            back[t][s] = from as u32;
        }
        best = next;
    }
    let mut path = vec![0usize; n];
    path[n - 1] = best
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(j, _)| j);
    for t in (1..n).rev() {
        path[t - 1] = back[t][path[t]] as usize;
    }
    path
}

/// Chord spans over the whole track (the first starts at 0, the last ends at the end).
pub(crate) fn detect_chords(f: &BeatFeatures) -> Vec<ChordSpan> {
    let states = states();
    let path = viterbi(&scores(f, &states), SWITCH);
    let mut out: Vec<ChordSpan> = Vec::new();
    for (i, &s) in path.iter().enumerate() {
        let chord = states.get(s).copied();
        let (a, b) = (f.times[i], f.times[i + 1]);
        match out.last_mut() {
            Some(last) if last.chord == chord => last.end = b,
            _ => out.push(ChordSpan {
                start: a,
                end: b,
                chord,
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioBuffer;
    use crate::analysis::BeatGrid;
    use crate::beatsync::beat_features;

    const SR: u32 = 44_100;

    fn midi_hz(n: f32) -> f32 {
        440.0 * 2f32.powf((n - 69.0) / 12.0)
    }

    /// Organ-ish chords (a few decaying harmonics), one per `secs`, starting at 0.
    pub(crate) fn chord_track(chords: &[&[f32]], secs: f32) -> AudioBuffer {
        let n = (chords.len() as f32 * secs * SR as f32) as usize;
        let mut x = vec![0.0f32; n];
        let per = (secs * SR as f32) as usize;
        for (ci, notes) in chords.iter().enumerate() {
            for &m in notes.iter() {
                let f = midi_hz(m);
                for k in 0..per {
                    let i = ci * per + k;
                    let t = k as f32 / SR as f32;
                    let env = (1.0 - (-t * 60.0).exp()) * (0.6 + 0.4 * (-t * 1.5).exp());
                    let mut v = 0.0;
                    for h in 1..=4 {
                        let fh = f * h as f32;
                        if fh < 8000.0 {
                            v += (std::f32::consts::TAU * fh * t).sin() / (h * h) as f32;
                        }
                    }
                    x[i] += 0.08 * env * v;
                }
            }
        }
        AudioBuffer::new(SR, vec![x])
    }

    fn grid(bpm: f32, n: usize) -> BeatGrid {
        let p = 60.0 / bpm as f64;
        BeatGrid {
            bpm,
            beats: (0..n).map(|i| i as f64 * p).collect(),
            downbeat: 0,
            beats_per_bar: 4,
        }
    }

    fn names(spans: &[ChordSpan]) -> Vec<String> {
        spans.iter().map(|s| s.label()).collect()
    }

    #[test]
    fn names_and_lookup() {
        let c = Chord {
            root: 10,
            quality: Quality::Min7,
        };
        assert_eq!(c.name(), "Bbm7");
        let spans = vec![
            ChordSpan {
                start: 0.0,
                end: 2.0,
                chord: Some(Chord {
                    root: 0,
                    quality: Quality::Major,
                }),
            },
            ChordSpan {
                start: 2.0,
                end: 4.0,
                chord: None,
            },
            ChordSpan {
                start: 4.0,
                end: 6.0,
                chord: Some(c),
            },
        ];
        assert_eq!(chord_at(&spans, 2.5).unwrap().label(), "N");
        assert_eq!(chord_at(&spans, 0.0).unwrap().label(), "C");
        assert!(chord_at(&spans, 7.0).is_none());
        let inside: Vec<String> = chords_in(&spans, 1.0, 5.0)
            .iter()
            .map(|c| c.name())
            .collect();
        assert_eq!(inside, ["C", "Bbm7"]);
        // A sliver at the edge doesn't count.
        assert_eq!(chords_in(&spans, 1.95, 3.0).len(), 0);
    }

    #[test]
    fn triads_and_sevenths_from_audio() {
        // C - Am - F - G7 - Em, one bar (2 s at 120 BPM) each, root in the bass.
        let prog: [&[f32]; 5] = [
            &[48.0, 60.0, 64.0, 67.0],
            &[45.0, 57.0, 60.0, 64.0],
            &[41.0, 57.0, 60.0, 65.0],
            &[43.0, 55.0, 59.0, 62.0, 65.0],
            &[40.0, 55.0, 59.0, 64.0],
        ];
        let buf = chord_track(&prog, 2.0);
        let g = grid(120.0, 20);
        let f = beat_features(&buf, &buf, Some(&g));
        let spans = detect_chords(&f);
        assert_eq!(names(&spans), ["C", "Am", "F", "G7", "Em"], "{spans:?}");
        // Changes land on the bar lines.
        for (s, want) in spans.iter().skip(1).zip([2.0, 4.0, 6.0, 8.0]) {
            assert!((s.start - want).abs() < 0.01, "{spans:?}");
        }
    }

    #[test]
    fn silence_is_no_chord() {
        let mut buf = chord_track(&[&[45.0, 57.0, 60.0, 64.0]], 4.0);
        let silent = vec![0.0f32; 4 * SR as usize];
        buf.channels[0].extend_from_slice(&silent);
        let g = grid(120.0, 16);
        let f = beat_features(&buf, &buf, Some(&g));
        assert_eq!(names(&detect_chords(&f)), ["Am", "N"]);
    }
}
