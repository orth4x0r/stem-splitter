//! Tempo / key / beat-grid glue for the app: which stems to analyse, halve / double, snapping,
//! zero-crossing refinement, bar-loop trimming and sample file names.

use serde::{Deserialize, Serialize};
use stem_audio::AudioBuffer;
use stem_audio::analysis::{self, BeatGrid, GridDivision, Key, Tempo};
use stem_models::StemKind;

use crate::paths::sanitize_file_name;

/// Analysis results for one session. `grid` and `tempo` follow the user's halve / double.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackAnalysis {
    pub tempo: Option<Tempo>,
    pub key: Option<Key>,
    pub grid: Option<BeatGrid>,
    /// Tempo as detected, before halve / double.
    pub detected_bpm: Option<f32>,
}

impl TrackAnalysis {
    pub fn bpm(&self) -> Option<f32> {
        self.tempo.map(|t| t.bpm)
    }

    /// Multiply the tempo by 0.5 or 2 (within 40..=300 BPM), keeping bar lines in place.
    pub fn rescale(&mut self, factor: f32) -> bool {
        let (Some(t), Some(g)) = (&mut self.tempo, &mut self.grid) else {
            return false;
        };
        let bpm = t.bpm * factor;
        if !(40.0..=300.0).contains(&bpm) {
            return false;
        }
        *g = g.scaled(factor);
        t.bpm = g.bpm;
        true
    }

    /// Re-apply a tempo the user picked earlier (from history), if it's a halving / doubling.
    pub fn apply_saved_bpm(&mut self, saved: f32) {
        for _ in 0..2 {
            let Some(bpm) = self.bpm() else { return };
            let r = saved / bpm;
            if (r - 2.0).abs() < 0.1 {
                self.rescale(2.0);
            } else if (r - 0.5).abs() < 0.03 {
                self.rescale(0.5);
            } else {
                return;
            }
        }
    }
}

/// Analyse separated stems: tempo from the drums when there are drums, key from everything
/// that isn't drums / effects (falls back to the whole mix).
pub fn analyze_stems(stems: &[(StemKind, &AudioBuffer)]) -> Option<TrackAnalysis> {
    if stems.is_empty() {
        return None;
    }
    let mix_of = |pick: &dyn Fn(StemKind) -> bool| -> Option<AudioBuffer> {
        let parts: Vec<&AudioBuffer> = stems
            .iter()
            .filter(|(k, _)| pick(*k))
            .map(|(_, b)| *b)
            .collect();
        match parts.len() {
            0 => None,
            1 => Some(parts[0].clone()),
            _ => Some(AudioBuffer::mix(&parts)),
        }
    };
    let all = mix_of(&|_| true)?;
    let drums = stems
        .iter()
        .find(|(k, _)| *k == StemKind::Drums)
        .map(|(_, b)| *b);
    let harmony =
        mix_of(&|k| !matches!(k, StemKind::Drums | StemKind::Effects | StemKind::Dialogue));
    let rhythm = drums.unwrap_or(&all);
    let a = analysis::analyze(rhythm, harmony.as_ref().unwrap_or(&all));
    Some(TrackAnalysis {
        detected_bpm: a.tempo.map(|t| t.bpm),
        tempo: a.tempo,
        key: a.key,
        grid: a.grid,
    })
}

/// "92" or "92.5".
pub fn bpm_label(bpm: f32) -> String {
    if (bpm - bpm.round()).abs() < 0.05 {
        format!("{}", bpm.round() as i32)
    } else {
        format!("{bpm:.1}")
    }
}

/// "90 BPM · A minor" for the library list, `None` when neither is known.
pub fn history_tag(bpm: Option<f32>, key: Option<&str>) -> Option<String> {
    let parts: Vec<String> = [
        bpm.map(|b| format!("{} BPM", bpm_label(b))),
        key.map(str::to_owned),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

// ---- snapping ------------------------------------------------------------------------------

/// Selection-edge snapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SnapMode {
    Off,
    HalfBeat,
    #[default]
    Beat,
    Bar,
}

impl SnapMode {
    pub const ALL: [SnapMode; 4] = [
        SnapMode::Off,
        SnapMode::HalfBeat,
        SnapMode::Beat,
        SnapMode::Bar,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SnapMode::Off => "Off",
            SnapMode::HalfBeat => "½",
            SnapMode::Beat => "Beat",
            SnapMode::Bar => "Bar",
        }
    }
    pub fn division(self) -> Option<GridDivision> {
        match self {
            SnapMode::Off => None,
            SnapMode::HalfBeat => Some(GridDivision::HalfBeat),
            SnapMode::Beat => Some(GridDivision::Beat),
            SnapMode::Bar => Some(GridDivision::Bar),
        }
    }
}

/// Snap `t` to the grid, clamped to the track.
pub fn snap(grid: Option<&BeatGrid>, mode: SnapMode, t: f64, duration: f64) -> f64 {
    match (grid, mode.division()) {
        (Some(g), Some(d)) => g.snap(t, d).clamp(0.0, duration),
        _ => t,
    }
}

/// Nearest zero crossing of the summed `stems` around frame `at`, within `radius` frames.
/// Returns `at` itself when there is none.
pub fn zero_crossing(stems: &[&AudioBuffer], at: usize, radius: usize) -> usize {
    let Some(first) = stems.first() else {
        return at;
    };
    let frames = first.frames();
    if frames < 2 {
        return at;
    }
    let at = at.min(frames - 1);
    let v = |i: usize| -> f32 {
        stems
            .iter()
            .map(|b| {
                b.channels
                    .iter()
                    .map(|c| c.get(i).copied().unwrap_or(0.0))
                    .sum::<f32>()
            })
            .sum()
    };
    let lo = at.saturating_sub(radius).max(1);
    let hi = (at + radius).min(frames - 1);
    let mut best: Option<(usize, usize)> = None; // (distance, index)
    let mut prev = v(lo - 1);
    for i in lo..=hi {
        let cur = v(i);
        if (prev <= 0.0 && cur >= 0.0) || (prev >= 0.0 && cur <= 0.0) {
            // Pick whichever side of the crossing is closer to zero.
            let idx = if cur.abs() <= prev.abs() { i } else { i - 1 };
            let d = idx.abs_diff(at);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, idx));
            }
        }
        prev = cur;
    }
    best.map_or(at, |(_, i)| i)
}

/// Final sample boundaries in frames for exporting `[a, b)` seconds from `stems`.
///
/// With `loop_bars` and a grid, a selection within ~15 % of a whole number of bars becomes
/// exactly that many bars long (start kept, end placed from the tempo, so it loops cleanly).
/// Otherwise both edges move to the nearest zero crossing (≤ 3 ms away).
/// Returns `(start, end, bars)` where `bars` is set when the loop trim applied.
pub fn export_bounds(
    stems: &[&AudioBuffer],
    a: f64,
    b: f64,
    grid: Option<&BeatGrid>,
    loop_bars: bool,
) -> (usize, usize, Option<usize>) {
    let Some(first) = stems.first() else {
        return (0, 0, None);
    };
    let sr = first.sample_rate as f64;
    let frames = first.frames();
    let radius = (sr * 0.003) as usize;
    let start = zero_crossing(stems, (a * sr).round() as usize, radius);
    if loop_bars
        && let Some(g) = grid
        && let Some(n) = whole_bars(g, a, b)
    {
        let len = (n as f64 * g.beats_per_bar as f64 * g.period() * sr).round() as usize;
        let end = (start + len).min(frames);
        return (start, end, Some(n));
    }
    let end = zero_crossing(stems, (b * sr).round() as usize, radius).max(start + 1);
    (start, end.min(frames), None)
}

/// Whole number of bars that `[a, b]` is close to (within 15 %), at least one.
pub fn whole_bars(g: &BeatGrid, a: f64, b: f64) -> Option<usize> {
    let bars = g.bars_between(a, b);
    let n = bars.round();
    (n >= 1.0 && (bars - n).abs() <= 0.15).then_some(n as usize)
}

// ---- file names ----------------------------------------------------------------------------

/// "Track - Stem - 92bpm Am - 0m10s670" (no extension), sanitised for Windows.
/// `bars` adds "4 bar loop" before the start time.
pub fn sample_file_name(
    title: &str,
    stem: &str,
    bpm: Option<f32>,
    key: Option<&Key>,
    start: f64,
    bars: Option<usize>,
) -> String {
    let title: String = title.trim().chars().take(60).collect();
    let mut parts = vec![title.trim().to_owned(), stem.to_owned()];
    let musical = [
        bpm.map(|b| format!("{}bpm", bpm_label(b))),
        key.map(Key::short),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    if !musical.is_empty() {
        parts.push(musical);
    }
    if let Some(n) = bars {
        parts.push(format!("{n} bar loop"));
    }
    parts.push(
        crate::backend::mmss_ms(start)
            .replace(':', "m")
            .replace('.', "s"),
    );
    sanitize_file_name(&parts.join(" - "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use stem_audio::analysis::Mode;

    fn grid() -> BeatGrid {
        BeatGrid {
            bpm: 120.0,
            beats: (0..64).map(|i| 0.25 + i as f64 * 0.5).collect(),
            downbeat: 0,
            beats_per_bar: 4,
        }
    }

    #[test]
    fn file_names() {
        let am = Key {
            tonic: 9,
            mode: Mode::Minor,
            confidence: 0.9,
        };
        assert_eq!(
            sample_file_name(
                "Midnight: Drive",
                "Drums",
                Some(90.0),
                Some(&am),
                10.6705,
                None
            ),
            "Midnight_ Drive - Drums - 90bpm Am - 0m10s670"
        );
        assert_eq!(
            sample_file_name("Song", "Mix", Some(93.5), None, 61.0, Some(4)),
            "Song - Mix - 93.5bpm - 4 bar loop - 1m01s000"
        );
        assert_eq!(
            sample_file_name("Song", "Bass", None, None, 0.0, None),
            "Song - Bass - 0m00s000"
        );
        let long = "x".repeat(200);
        assert!(
            sample_file_name(&long, "Bass", Some(120.0), Some(&am), 0.0, None)
                .ends_with("120bpm Am - 0m00s000")
        );
    }

    #[test]
    fn snapping_modes() {
        let g = grid();
        assert_eq!(snap(Some(&g), SnapMode::Off, 1.1, 30.0), 1.1);
        assert!((snap(Some(&g), SnapMode::Beat, 1.1, 30.0) - 1.25).abs() < 1e-9);
        assert!((snap(Some(&g), SnapMode::HalfBeat, 1.1, 30.0) - 1.0).abs() < 1e-9);
        assert!((snap(Some(&g), SnapMode::Bar, 1.1, 30.0) - 0.25).abs() < 1e-9);
        assert_eq!(snap(None, SnapMode::Beat, 1.1, 30.0), 1.1);
        // Clamped to the track.
        assert_eq!(snap(Some(&g), SnapMode::Bar, 29.9, 30.0), 30.0);
    }

    #[test]
    fn zero_crossings() {
        // Sine with period 100 frames: crossings at 0, 50, 100, ...
        let x: Vec<f32> = (0..1000)
            .map(|i| (i as f32 * std::f32::consts::TAU / 100.0).sin())
            .collect();
        let b = AudioBuffer::new(1000, vec![x]);
        assert_eq!(zero_crossing(&[&b], 147, 10), 150);
        assert_eq!(zero_crossing(&[&b], 120, 10), 120); // none within reach
    }

    #[test]
    fn loop_trim() {
        let g = grid();
        let x = vec![0.0f32; 44_100 * 40];
        let b = AudioBuffer::new(44_100, vec![x]);
        // 2 bars = 4 s at 120 BPM; selection 0.25 .. 4.1 is close to 2 bars.
        let (s, e, bars) = export_bounds(&[&b], 0.25, 4.1, Some(&g), true);
        assert_eq!(bars, Some(2));
        assert_eq!(e - s, 4 * 44_100);
        // Too far from a whole bar: untouched.
        let (_, _, bars) = export_bounds(&[&b], 0.25, 3.25, Some(&g), true);
        assert_eq!(bars, None);
        assert_eq!(whole_bars(&g, 0.25, 0.5), None);
    }

    #[test]
    fn halve_double_and_restore() {
        let mut a = TrackAnalysis {
            tempo: Some(Tempo {
                bpm: 140.0,
                confidence: 0.8,
            }),
            key: None,
            grid: Some(BeatGrid {
                bpm: 140.0,
                ..grid()
            }),
            detected_bpm: Some(140.0),
        };
        assert!(a.rescale(0.5));
        assert_eq!(a.bpm(), Some(70.0));
        assert!(!a.rescale(0.5)); // 35 is out of range
        let mut b = a.clone();
        b.rescale(2.0);
        b.apply_saved_bpm(70.0);
        assert_eq!(b.bpm(), Some(70.0));
        assert_eq!(bpm_label(93.5), "93.5");
        assert_eq!(bpm_label(90.02), "90");
    }
}
