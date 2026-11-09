//! Notes to a Standard MIDI File, with optional quantisation to a beat grid.
//!
//! The file is format 1: track 0 holds tempo, time signature and the name, track 1 the notes
//! on channel 1. Tick 0 is the start of the analysed audio, so the clip lines up with the
//! stem it came from when both are dropped at the same position in a DAW.

use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, PitchBend, Smf, Timing, TrackEvent};
use midly::{TrackEventKind, num::u14};
use stem_audio::analysis::BeatGrid;

use crate::basic_pitch::Note;

/// Ticks per quarter note.
pub const PPQ: u16 = 480;

/// Grid to quantise to, as notes per beat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum QuantizeGrid {
    Quarter,
    Eighth,
    Sixteenth,
    EighthTriplet,
    SixteenthTriplet,
}

impl QuantizeGrid {
    pub const ALL: [QuantizeGrid; 5] = [
        QuantizeGrid::Quarter,
        QuantizeGrid::Eighth,
        QuantizeGrid::Sixteenth,
        QuantizeGrid::EighthTriplet,
        QuantizeGrid::SixteenthTriplet,
    ];

    /// Grid steps per beat (quarter note).
    pub fn per_beat(self) -> u32 {
        match self {
            QuantizeGrid::Quarter => 1,
            QuantizeGrid::Eighth => 2,
            QuantizeGrid::Sixteenth => 4,
            QuantizeGrid::EighthTriplet => 3,
            QuantizeGrid::SixteenthTriplet => 6,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            QuantizeGrid::Quarter => "1/4",
            QuantizeGrid::Eighth => "1/8",
            QuantizeGrid::Sixteenth => "1/16",
            QuantizeGrid::EighthTriplet => "1/8T",
            QuantizeGrid::SixteenthTriplet => "1/16T",
        }
    }
}

/// Quantise settings: `strength` 1.0 snaps fully, 0.5 moves notes halfway to the grid.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Quantize {
    pub grid: QuantizeGrid,
    pub strength: f32,
}

/// Where the beats are. `grid` (from [`stem_audio::analysis`]) follows tempo drift; without
/// it a constant `bpm` grid starts at 0. `offset` is the time of note time 0 on that grid
/// (the selection start when only part of a stem was transcribed).
#[derive(Debug, Clone, Copy)]
pub struct Beats<'a> {
    pub bpm: f32,
    pub grid: Option<&'a BeatGrid>,
    pub offset: f64,
}

impl Beats<'_> {
    fn period(&self) -> f64 {
        60.0 / self.bpm.max(1.0) as f64
    }

    /// Fractional beat index of note time `t`.
    pub fn position(&self, t: f64) -> f64 {
        match self.grid {
            Some(g) => g.beat_position(t + self.offset),
            None => (t + self.offset) / self.period(),
        }
    }

    /// Note time of fractional beat index `pos`.
    pub fn time(&self, pos: f64) -> f64 {
        let abs = match self.grid {
            Some(g) => {
                let i = pos.floor();
                let (a, b) = (g.beat_time(i as i64), g.beat_time(i as i64 + 1));
                a + (pos - i) * (b - a)
            }
            None => pos * self.period(),
        };
        abs - self.offset
    }
}

/// Snap note starts and ends towards the grid. Notes never collapse: an end that would land
/// on or before its start keeps one grid step (or the note's own length, if shorter).
pub fn quantize(notes: &[Note], q: &Quantize, beats: &Beats<'_>) -> Vec<Note> {
    let d = q.grid.per_beat() as f64;
    let s = q.strength.clamp(0.0, 1.0) as f64;
    let snap = |t: f64| {
        let pos = beats.position(t);
        let target = beats.time((pos * d).round() / d);
        t + s * (target - t)
    };
    notes
        .iter()
        .map(|n| {
            let start = snap(n.start);
            let mut end = snap(n.end);
            if end <= start + 1e-4 {
                let pos = beats.position(start);
                let step = beats.time(pos + 1.0 / d) - start;
                end = start + step.min(n.end - n.start).max(0.01);
            }
            Note {
                start,
                end,
                ..n.clone()
            }
        })
        .collect()
}

/// What goes into the file besides the notes.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiOptions {
    /// Written as the file's tempo; also maps seconds to ticks.
    pub bpm: f32,
    pub track_name: String,
    /// General MIDI program (0-based), e.g. 33 = fingered bass.
    pub program: u8,
    /// Write each note's pitch bends (dropped for notes that overlap another note, as
    /// basic-pitch does, since MIDI bends apply to the whole channel).
    pub pitch_bends: bool,
}

impl Default for MidiOptions {
    fn default() -> Self {
        Self {
            bpm: 120.0,
            track_name: "Basic Pitch".into(),
            program: 4,
            pitch_bends: false,
        }
    }
}

/// Seconds to ticks at `bpm`.
pub fn seconds_to_ticks(t: f64, bpm: f32) -> u64 {
    (t.max(0.0) * bpm.max(1.0) as f64 / 60.0 * PPQ as f64).round() as u64
}

/// Build the `.mid` bytes.
pub fn write_midi(notes: &[Note], opts: &MidiOptions) -> Vec<u8> {
    let bpm = opts.bpm.clamp(20.0, 400.0);
    let tick = |t: f64| seconds_to_ticks(t, bpm);

    // Same-pitch notes must not overlap, or the first note-off would end the second note.
    let mut sorted: Vec<Note> = notes
        .iter()
        .filter(|n| n.end > n.start && (0..=127).contains(&n.pitch))
        .cloned()
        .collect();
    sorted.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    for i in 0..sorted.len() {
        if let Some(next) = sorted[i + 1..].iter().find(|n| n.pitch == sorted[i].pitch) {
            let next_start = next.start;
            if sorted[i].end > next_start {
                sorted[i].end = next_start;
            }
        }
    }
    sorted.retain(|n| tick(n.end) > tick(n.start));
    let bends_ok: Vec<bool> = (0..sorted.len())
        .map(|i| {
            opts.pitch_bends
                && sorted[i].bends.as_ref().is_some_and(|b| !b.is_empty())
                && !sorted
                    .iter()
                    .enumerate()
                    .any(|(j, o)| j != i && o.start < sorted[i].end && sorted[i].start < o.end)
        })
        .collect();

    // (tick, order, message): at equal ticks note-offs go first, then bends, then note-ons.
    let ch = u4::new(0);
    let mut events: Vec<(u64, u8, MidiMessage)> = Vec::with_capacity(sorted.len() * 2);
    for (n, &bend) in sorted.iter().zip(&bends_ok) {
        let key = u7::new(n.pitch);
        events.push((
            tick(n.start),
            2,
            MidiMessage::NoteOn {
                key,
                vel: u7::new(n.velocity()),
            },
        ));
        events.push((
            tick(n.end),
            0,
            MidiMessage::NoteOff {
                key,
                vel: u7::new(0),
            },
        ));
        if bend && let Some(b) = &n.bends {
            let steps = b.len();
            for (k, &v) in b.iter().enumerate() {
                let t = if steps > 1 {
                    n.start + (n.end - n.start) * k as f64 / (steps - 1) as f64
                } else {
                    n.start
                };
                let ticks = (v as f64 * 4096.0 / 3.0).round().clamp(-8192.0, 8191.0) as i16;
                events.push((
                    tick(t),
                    1,
                    MidiMessage::PitchBend {
                        bend: PitchBend::from_int(ticks),
                    },
                ));
            }
            events.push((
                tick(n.end),
                1,
                MidiMessage::PitchBend {
                    bend: PitchBend(u14::new(8192)),
                },
            ));
        }
    }
    events.sort_by_key(|e| (e.0, e.1));

    let name = opts.track_name.as_bytes();
    let us_per_beat = (60_000_000.0 / bpm as f64).round() as u32;
    let meta = |m| TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(m),
    };
    let tempo_track = vec![
        meta(MetaMessage::TrackName(name)),
        meta(MetaMessage::Tempo(u24::new(us_per_beat))),
        meta(MetaMessage::TimeSignature(4, 2, 24, 8)),
        meta(MetaMessage::EndOfTrack),
    ];
    let mut note_track = vec![
        meta(MetaMessage::TrackName(name)),
        TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Midi {
                channel: ch,
                message: MidiMessage::ProgramChange {
                    program: u7::new(opts.program.min(127)),
                },
            },
        },
    ];
    let mut last = 0u64;
    for (t, _, message) in events {
        note_track.push(TrackEvent {
            delta: u28::new((t - last).min(u28::max_value().as_int() as u64) as u32),
            kind: TrackEventKind::Midi {
                channel: ch,
                message,
            },
        });
        last = t;
    }
    note_track.push(meta(MetaMessage::EndOfTrack));

    let smf = Smf {
        header: Header::new(Format::Parallel, Timing::Metrical(u15::new(PPQ))),
        tracks: vec![tempo_track, note_track],
    };
    let mut out = Vec::new();
    smf.write_std(&mut out)
        .expect("writing to a Vec cannot fail");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: f64, end: f64, pitch: u8, amplitude: f32) -> Note {
        Note {
            start,
            end,
            pitch,
            amplitude,
            bends: None,
        }
    }

    /// (start tick, end tick, pitch, velocity) of every note in `bytes`.
    type Parsed = (u32, Vec<(u64, u64, u8, u8)>, Vec<(u64, i16)>);

    fn parse(bytes: &[u8]) -> Parsed {
        let smf = Smf::parse(bytes).unwrap();
        assert_eq!(smf.header.timing, Timing::Metrical(u15::new(PPQ)));
        let mut tempo = 0;
        let mut out = Vec::new();
        let mut bends = Vec::new();
        for track in &smf.tracks {
            let mut t = 0u64;
            let mut open: Vec<(u8, u64, u8)> = Vec::new();
            for ev in track {
                t += ev.delta.as_int() as u64;
                match ev.kind {
                    TrackEventKind::Meta(MetaMessage::Tempo(us)) => tempo = us.as_int(),
                    TrackEventKind::Midi { message, .. } => match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                            open.push((key.as_int(), t, vel.as_int()))
                        }
                        MidiMessage::NoteOff { key, .. } | MidiMessage::NoteOn { key, .. } => {
                            let i = open.iter().position(|o| o.0 == key.as_int()).unwrap();
                            let (k, s, v) = open.remove(i);
                            out.push((s, t, k, v));
                        }
                        MidiMessage::PitchBend { bend } => bends.push((t, bend.as_int())),
                        _ => {}
                    },
                    _ => {}
                }
            }
            assert!(open.is_empty());
        }
        out.sort();
        (tempo, out, bends)
    }

    #[test]
    fn round_trip() {
        let notes = vec![
            note(0.0, 0.5, 60, 0.5),
            note(0.5, 1.0, 64, 1.0),
            note(0.25, 1.5, 48, 0.1),
        ];
        let bytes = write_midi(
            &notes,
            &MidiOptions {
                bpm: 90.0,
                ..MidiOptions::default()
            },
        );
        let (tempo, got, bends) = parse(&bytes);
        assert_eq!(tempo, 666_667);
        // 90 bpm: one beat = 0.6667 s = 480 ticks.
        assert_eq!(
            got,
            vec![(0, 360, 60, 64), (180, 1080, 48, 13), (360, 720, 64, 127)]
        );
        assert!(bends.is_empty());
    }

    #[test]
    fn same_pitch_overlaps_are_trimmed() {
        let notes = vec![note(0.0, 1.0, 60, 0.5), note(0.5, 1.5, 60, 0.5)];
        let (_, got, _) = parse(&write_midi(
            &notes,
            &MidiOptions {
                bpm: 120.0,
                ..MidiOptions::default()
            },
        ));
        assert_eq!(got, vec![(0, 480, 60, 64), (480, 1440, 60, 64)]);
    }

    #[test]
    fn pitch_bends_only_on_lone_notes() {
        let mut a = note(0.0, 1.0, 60, 0.5);
        a.bends = Some(vec![0, 3, -3]);
        let mut b = note(2.0, 3.0, 62, 0.5);
        b.bends = Some(vec![1]);
        let mut c = note(2.5, 3.5, 65, 0.5);
        c.bends = Some(vec![2]);
        let opts = MidiOptions {
            bpm: 120.0,
            pitch_bends: true,
            ..MidiOptions::default()
        };
        let (_, got, bends) = parse(&write_midi(&[a, b, c], &opts));
        assert_eq!(got.len(), 3);
        // a: 0, +1 semitone (4096), -1 semitone, then reset; b and c overlap: none.
        assert_eq!(bends, vec![(0, 0), (480, 4096), (960, -4096), (960, 0)]);
    }

    #[test]
    fn quantize_constant_grid() {
        let beats = Beats {
            bpm: 120.0,
            grid: None,
            offset: 0.0,
        };
        let notes = vec![note(0.27, 0.49, 60, 0.5), note(1.01, 1.02, 62, 0.5)];
        let q = |grid, strength| quantize(&notes, &Quantize { grid, strength }, &beats);
        // 1/8 at 120 bpm = 0.25 s.
        let full = q(QuantizeGrid::Eighth, 1.0);
        assert!((full[0].start - 0.25).abs() < 1e-9 && (full[0].end - 0.5).abs() < 1e-9);
        // The short note would collapse onto 1.0: keeps its own 10 ms.
        assert!((full[1].start - 1.0).abs() < 1e-9);
        assert!((full[1].end - 1.01).abs() < 1e-9);
        let half = q(QuantizeGrid::Eighth, 0.5);
        assert!((half[0].start - 0.26).abs() < 1e-9);
        // Triplets: 1/8T = 1/6 s.
        let trip = q(QuantizeGrid::EighthTriplet, 1.0);
        assert!((trip[0].start - 1.0 / 3.0).abs() < 1e-9);
        let sixteenth = q(QuantizeGrid::Sixteenth, 1.0);
        assert!((sixteenth[0].start - 0.25).abs() < 1e-9);
        assert!((sixteenth[0].end - 0.5).abs() < 1e-9);
        let trip16 = q(QuantizeGrid::SixteenthTriplet, 1.0);
        assert!((trip16[0].start - 0.25).abs() < 1e-9);
    }

    #[test]
    fn quantize_follows_beat_grid_and_offset() {
        // Beats at 0.1, 0.6, 1.1, ... (120 bpm shifted by 100 ms).
        let grid = BeatGrid {
            bpm: 120.0,
            beats: (0..16).map(|i| 0.1 + i as f64 * 0.5).collect(),
            downbeat: 0,
            beats_per_bar: 4,
        };
        let beats = Beats {
            bpm: 120.0,
            grid: Some(&grid),
            offset: 0.0,
        };
        let q = Quantize {
            grid: QuantizeGrid::Quarter,
            strength: 1.0,
        };
        let out = quantize(&[note(0.55, 1.2, 60, 0.5)], &q, &beats);
        assert!((out[0].start - 0.6).abs() < 1e-9);
        assert!((out[0].end - 1.1).abs() < 1e-9);
        // Same note transcribed from a selection starting at 0.5 s.
        let sel = Beats {
            offset: 0.5,
            ..beats
        };
        let out = quantize(&[note(0.05, 0.7, 60, 0.5)], &q, &sel);
        assert!((out[0].start - 0.1).abs() < 1e-9);
        assert!((out[0].end - 0.6).abs() < 1e-9);
    }
}
