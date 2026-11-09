//! Tempo / key / beat-grid glue for the app: which stems to analyse, halve / double, grid
//! nudging, snapping, zero-crossing refinement, bar-loop trimming and sample file names.

use serde::{Deserialize, Serialize};
use stem_audio::AudioBuffer;
use stem_audio::analysis::{self, BeatGrid, GridDivision, Key, Tempo};
use stem_audio::chords::ChordSpan;
use stem_audio::sections::Section;
use stem_models::StemKind;

use crate::paths::sanitize_file_name;

/// Analysis results for one session. `grid` and `tempo` follow the user's halve / double and
/// grid nudges; `sections` follow the grid's bar lines.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrackAnalysis {
    pub tempo: Option<Tempo>,
    pub key: Option<Key>,
    pub grid: Option<BeatGrid>,
    /// Tempo as detected, before halve / double.
    pub detected_bpm: Option<f32>,
    /// Song sections (A, B, A, C ...) on the grid's bar lines.
    pub sections: Vec<Section>,
    /// Chords over the whole track (from the harmonic stems).
    pub chords: Vec<ChordSpan>,
    /// The user's grid nudge (saved in the history entry).
    pub nudge: GridNudge,
    /// The grid and sections as detected, for "reset" and re-snapping.
    pub detected_grid: Option<BeatGrid>,
    pub detected_sections: Vec<Section>,
    /// Track length, for re-snapping sections.
    pub duration: f64,
}

/// How the user moved the beat grid: a time shift and, optionally, which beat is bar 1.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct GridNudge {
    /// Seconds the whole grid moved (positive = later).
    #[serde(default)]
    pub offset: f64,
    /// Time of a beat that starts a bar (in the shifted grid), when the user picked one.
    #[serde(default)]
    pub downbeat_at: Option<f64>,
}

impl GridNudge {
    pub fn is_zero(&self) -> bool {
        self.offset.abs() < 1e-6 && self.downbeat_at.is_none()
    }
}

/// Nearest beat index to `t`.
fn nearest_beat(g: &BeatGrid, t: f64) -> i64 {
    let i = g.beat_index_at(t);
    if (g.beat_time(i + 1) - t).abs() < (t - g.beat_time(i)).abs() {
        i + 1
    } else {
        i
    }
}

/// Make beat `i` (any index, extrapolated ones included) a downbeat, keeping `downbeat`
/// inside `beats`.
fn set_downbeat(g: &mut BeatGrid, i: i64) {
    let bpb = g.beats_per_bar.max(1) as i64;
    let n = g.beats.len() as i64;
    g.downbeat = i.rem_euclid(bpb).clamp(0, (n - 1).max(0)) as usize;
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
        self.resnap();
        true
    }

    /// Re-apply the saved tempo choice and grid nudge (from history) to a fresh analysis.
    pub fn restore(&mut self, saved_bpm: Option<f32>, nudge: Option<GridNudge>) {
        let nudge = nudge.unwrap_or_default();
        if let Some(g) = &mut self.grid {
            g.beats.iter_mut().for_each(|b| *b += nudge.offset);
            // Bar 1 first on the full grid, so halving keeps the right beats.
            if let Some(at) = nudge.downbeat_at {
                let i = nearest_beat(g, at);
                if (g.beat_time(i) - at).abs() < 0.25 * g.period() {
                    set_downbeat(g, i);
                }
            }
        }
        if let Some(b) = saved_bpm {
            self.apply_saved_bpm(b);
        }
        if let (Some(g), Some(at)) = (&mut self.grid, nudge.downbeat_at) {
            let i = nearest_beat(g, at);
            set_downbeat(g, i);
        }
        self.nudge = nudge;
        self.resnap();
    }

    /// Shift the whole grid by `dt` seconds (positive = later).
    pub fn nudge_by(&mut self, dt: f64) -> bool {
        let Some(g) = &mut self.grid else {
            return false;
        };
        g.beats.iter_mut().for_each(|b| *b += dt);
        self.nudge.offset += dt;
        if let Some(at) = &mut self.nudge.downbeat_at {
            *at += dt;
        }
        self.resnap();
        true
    }

    /// Move the bar lines by `beats` beats (positive = later), beats stay where they are.
    pub fn shift_bar(&mut self, beats: i64) -> bool {
        let Some(g) = &mut self.grid else {
            return false;
        };
        let i = g.downbeat as i64 + beats;
        set_downbeat(g, i);
        self.nudge.downbeat_at = Some(g.beat_time(g.downbeat as i64));
        self.resnap();
        true
    }

    /// Make the beat nearest `t` the start of a bar ("set bar 1 here").
    pub fn set_bar_one(&mut self, t: f64) -> bool {
        let Some(g) = &mut self.grid else {
            return false;
        };
        let i = nearest_beat(g, t);
        set_downbeat(g, i);
        self.nudge.downbeat_at = Some(g.beat_time(i));
        self.resnap();
        true
    }

    /// Undo the nudge (keeps halve / double).
    pub fn reset_nudge(&mut self) {
        let bpm = self.bpm();
        if let Some(d) = &self.detected_grid {
            self.grid = Some(d.clone());
            if let (Some(t), Some(dbpm)) = (&mut self.tempo, self.detected_bpm) {
                t.bpm = dbpm;
            }
        }
        self.nudge = GridNudge::default();
        if let Some(b) = bpm {
            self.apply_saved_bpm(b);
        }
        self.resnap();
    }

    /// Sections back on the current grid's bar lines.
    fn resnap(&mut self) {
        if let Some(g) = &self.grid
            && !self.detected_sections.is_empty()
        {
            self.sections = stem_audio::sections::resnap(&self.detected_sections, g, self.duration);
        }
    }

    /// The section containing `t`.
    pub fn section_at(&self, t: f64) -> Option<&Section> {
        self.sections.iter().find(|s| t >= s.start && t < s.end)
    }

    /// Chord names sounding in `[a, b)`, e.g. "Am · F" (empty when unknown).
    pub fn chords_label(&self, a: f64, b: f64) -> String {
        stem_audio::chords::chords_in(&self.chords, a, b)
            .iter()
            .map(|c| c.name())
            .collect::<Vec<_>>()
            .join(" · ")
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
        detected_grid: a.grid.clone(),
        grid: a.grid,
        sections: a.sections.clone(),
        detected_sections: a.sections,
        chords: a.chords,
        nudge: GridNudge::default(),
        duration: all.duration_secs(),
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
            ..Default::default()
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

    fn with_sections() -> TrackAnalysis {
        let g = grid();
        let sec = |start: f64, end: f64, label: u8| Section {
            start,
            end,
            label,
            energy: 1.0,
        };
        let sections = vec![sec(0.0, 8.25, 0), sec(8.25, 16.25, 1), sec(16.25, 32.0, 0)];
        TrackAnalysis {
            tempo: Some(Tempo {
                bpm: 120.0,
                confidence: 1.0,
            }),
            grid: Some(g.clone()),
            detected_grid: Some(g),
            detected_bpm: Some(120.0),
            sections: sections.clone(),
            detected_sections: sections,
            duration: 32.0,
            ..Default::default()
        }
    }

    #[test]
    fn nudge_moves_grid_and_sections() {
        let mut a = with_sections();
        assert!(a.nudge_by(0.005));
        assert!(a.nudge_by(0.005));
        let g = a.grid.as_ref().unwrap();
        assert!((g.beats[0] - 0.26).abs() < 1e-9);
        assert!((a.nudge.offset - 0.01).abs() < 1e-9);
        // Sections follow the bar lines.
        assert!(
            (a.sections[1].start - 8.26).abs() < 1e-9,
            "{:?}",
            a.sections
        );
        // One beat later: bar lines (and sections) move by half a second.
        assert!(a.shift_bar(1));
        assert_eq!(a.grid.as_ref().unwrap().downbeat, 1);
        assert!(
            (a.sections[1].start - 8.76).abs() < 1e-9,
            "{:?}",
            a.sections
        );
        // Bar 1 at the beat nearest 3.0 s (beat 5 at 2.76 s): downbeat phase 1.
        assert!(a.set_bar_one(3.0));
        let g = a.grid.as_ref().unwrap();
        assert!(g.is_downbeat(5));
        assert_eq!(a.nudge.downbeat_at, Some(g.beat_time(5)));
        a.reset_nudge();
        assert_eq!(a.grid, a.detected_grid);
        assert_eq!(a.sections, a.detected_sections);
        assert!(a.nudge.is_zero());
    }

    #[test]
    fn restore_reapplies_nudge_and_tempo() {
        let mut a = with_sections();
        a.nudge_by(-0.02);
        a.set_bar_one(1.25 - 0.02); // beat 2
        a.rescale(0.5);
        let want = a.clone();
        let mut b = with_sections();
        b.restore(want.bpm(), Some(want.nudge));
        assert_eq!(b.bpm(), Some(60.0));
        assert_eq!(b.grid, want.grid);
        assert_eq!(b.sections, want.sections);
        // An old history entry without a nudge.
        let mut c = with_sections();
        c.restore(Some(120.0), None);
        assert_eq!(c.grid, c.detected_grid);
        // Chord labels for a range.
        use stem_audio::chords::{Chord, Quality};
        c.chords = vec![
            ChordSpan {
                start: 0.0,
                end: 2.0,
                chord: Some(Chord {
                    root: 9,
                    quality: Quality::Minor,
                }),
            },
            ChordSpan {
                start: 2.0,
                end: 4.0,
                chord: Some(Chord {
                    root: 5,
                    quality: Quality::Major,
                }),
            },
        ];
        assert_eq!(c.chords_label(1.0, 3.0), "Am · F");
        assert_eq!(c.section_at(9.0).map(|s| s.label), Some(1));
    }
}
