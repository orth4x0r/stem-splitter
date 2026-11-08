//! Note creation against basic-pitch's Python (`tools/basic_pitch_reference.py golden`), on
//! synthetic posteriorgrams built by the same integer generator on both sides, plus the
//! windowing / unwrapping with a stub network.

use std::sync::atomic::AtomicBool;

use super::*;
use crate::network::{FnNetwork, TensorOut};

const N_FRAMES: usize = 300;

/// (start frame, length, key index, level /256, with onset, contour bin offset); mirrors
/// `BLOBS` in the Python script.
const BLOBS: &[(usize, usize, usize, u32, bool, i32)] = &[
    (5, 40, 27, 230, true, 0),
    (20, 30, 31, 200, true, 1),
    (60, 25, 27, 180, true, -1),
    (90, 60, 40, 210, false, 0),
    (100, 8, 50, 220, true, 0),
    (160, 50, 45, 190, true, 2),
    (175, 4, 45, 10, false, 0),
    (230, 66, 60, 240, true, -2),
    (240, 30, 0, 200, true, 0),
    (250, 30, 87, 200, true, 0),
];

fn synthetic() -> Posteriorgrams {
    let mut state: u64 = 12345;
    let mut rnd = || {
        state = (state * 1_103_515_245 + 12_345) % (1 << 31);
        (state >> 16) as u32
    };
    let mut note = vec![0f32; N_FRAMES * N_NOTES];
    let mut onset = vec![0f32; N_FRAMES * N_NOTES];
    let mut contour = vec![0f32; N_FRAMES * N_CONTOUR_BINS];
    for t in 0..N_FRAMES {
        for f in 0..N_NOTES {
            note[t * N_NOTES + f] = (rnd() % 48) as f32 / 256.0;
            onset[t * N_NOTES + f] = (rnd() % 40) as f32 / 256.0;
        }
        for c in 0..N_CONTOUR_BINS {
            contour[t * N_CONTOUR_BINS + c] = (rnd() % 64) as f32 / 256.0;
        }
    }
    for &(start, len, f, level, with_onset, bend) in BLOBS {
        for t in start..(start + len).min(N_FRAMES) {
            let v = level.saturating_sub((t - start) as u32) as f32 / 256.0;
            let i = t * N_NOTES + f;
            if level < 50 {
                note[i] = v;
            } else {
                note[i] = note[i].max(v);
                if f + 1 < N_NOTES {
                    note[i + 1] = note[i + 1].max(v / 2.0);
                }
            }
            let c = 3 * f as i32 + bend + if (t / 7) % 2 == 1 { 1 } else { 0 };
            if (0..N_CONTOUR_BINS as i32).contains(&c) && level >= 50 {
                contour[t * N_CONTOUR_BINS + c as usize] = 230.0 / 256.0;
            }
        }
        if with_onset {
            onset[start * N_NOTES + f] = 220.0 / 256.0;
            if start > 0 {
                onset[(start - 1) * N_NOTES + f] = 100.0 / 256.0;
            }
            if start + 1 < N_FRAMES {
                onset[(start + 1) * N_NOTES + f] = 140.0 / 256.0;
            }
        }
    }
    Posteriorgrams {
        n_frames: N_FRAMES,
        note,
        onset,
        contour,
    }
}

type Case = (
    &'static str,
    f32,
    f32,
    f32,
    Option<u8>,
    Option<u8>,
    bool,
    bool,
    &'static [(usize, usize, u8, f32, &'static [i32])],
);

/// Output of `python3 tools/basic_pitch_reference.py golden` (basic-pitch 0.4.0).
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
const GOLDEN: &[Case] = &[
    ("default", 0.5, 0.3, 127.7, None, None, true, true, &[
        (250, 280, 108, 0.724609375, &[1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1]),
        (240, 270, 21, 0.724609375, &[0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0]),
        (230, 296, 81, 0.810546875, &[-2, -1, -1, -1, -1, -1, -1, -1, -2, -2, -2, -2, -2, -2, -2, -1, -1, -1, -1, -1, -1, -1, -2, -2, -2, -2, -2, -2, -2, -1, -1, -1, -1, -1, -1, -1, -2, -2, -2, -2, -2, -2, -2, -1, -1, -1, -1, -1, -1, -1, -2, -2, -2, -2, -2, -2, -2, -1, -1, -1, -1, -1, -1, -1, -2, -2]),
        (179, 210, 66, 0.609375, &[3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3]),
        (160, 175, 66, 0.71484375, &[2, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2]),
        (90, 150, 61, 0.705078125, &[0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1]),
        (60, 85, 48, 0.65625, &[-1, -1, -1, 0, 0, 0, 0, 0, 0, 0, -1, -1, -1, -1, -1, -1, -1, 0, 0, 0, 0, 0, 0, 0, -1]),
        (20, 50, 52, 0.724609375, &[1, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 2]),
        (5, 45, 48, 0.822265625, &[0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0]),
    ]),
    ("strict", 0.6, 0.4, 60.0, None, None, true, false, &[
        (250, 280, 108, 0.724609375, &[1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1]),
        (240, 270, 21, 0.724609375, &[0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0]),
        (230, 296, 81, 0.810546875, &[-2, -1, -1, -1, -1, -1, -1, -1, -2, -2, -2, -2, -2, -2, -2, -1, -1, -1, -1, -1, -1, -1, -2, -2, -2, -2, -2, -2, -2, -1, -1, -1, -1, -1, -1, -1, -2, -2, -2, -2, -2, -2, -2, -1, -1, -1, -1, -1, -1, -1, -2, -2, -2, -2, -2, -2, -2, -1, -1, -1, -1, -1, -1, -1, -2, -2]),
        (179, 210, 66, 0.609375, &[3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3]),
        (160, 175, 66, 0.71484375, &[2, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2]),
        (100, 108, 71, 0.845703125, &[0, 0, 0, 0, 0, 1, 1, 1]),
        (90, 150, 61, 0.705078125, &[0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1]),
        (60, 85, 48, 0.65625, &[-1, -1, -1, 0, 0, 0, 0, 0, 0, 0, -1, -1, -1, -1, -1, -1, -1, 0, 0, 0, 0, 0, 0, 0, -1]),
        (20, 50, 52, 0.724609375, &[1, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 2]),
        (5, 45, 48, 0.822265625, &[0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0]),
    ]),
    ("no_infer", 0.5, 0.25, 127.7, None, None, false, true, &[
        (250, 280, 108, 0.724609375, &[1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1]),
        (240, 270, 21, 0.724609375, &[0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0]),
        (230, 296, 81, 0.810546875, &[-2, -1, -1, -1, -1, -1, -1, -1, -2, -2, -2, -2, -2, -2, -2, -1, -1, -1, -1, -1, -1, -1, -2, -2, -2, -2, -2, -2, -2, -1, -1, -1, -1, -1, -1, -1, -2, -2, -2, -2, -2, -2, -2, -1, -1, -1, -1, -1, -1, -1, -2, -2, -2, -2, -2, -2, -2, -1, -1, -1, -1, -1, -1, -1, -2, -2]),
        (160, 210, 66, 0.594921886920929, &[2, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3]),
        (60, 85, 48, 0.65625, &[-1, -1, -1, 0, 0, 0, 0, 0, 0, 0, -1, -1, -1, -1, -1, -1, -1, 0, 0, 0, 0, 0, 0, 0, -1]),
        (20, 50, 52, 0.724609375, &[1, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 2]),
        (5, 45, 48, 0.822265625, &[0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0]),
        (90, 149, 61, 0.70703125, &[0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1]),
    ]),
    ("range", 0.5, 0.3, 127.7, Some(40), Some(70), true, true, &[
        (179, 210, 66, 0.609375, &[3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3]),
        (160, 175, 66, 0.71484375, &[2, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2]),
        (90, 150, 61, 0.705078125, &[0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1]),
        (60, 85, 48, 0.65625, &[-1, -1, -1, 0, 0, 0, 0, 0, 0, 0, -1, -1, -1, -1, -1, -1, -1, 0, 0, 0, 0, 0, 0, 0, -1]),
        (20, 50, 52, 0.724609375, &[1, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 2]),
        (5, 45, 48, 0.822265625, &[0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0]),
    ]),
];

#[test]
fn note_creation_matches_python() {
    let post = synthetic();
    for &(name, on, fr, ms, lo, hi, infer, melodia, want) in GOLDEN {
        let opts = NoteOptions {
            onset_threshold: on,
            frame_threshold: fr,
            min_note_ms: ms,
            min_pitch: lo,
            max_pitch: hi,
            infer_onsets: infer,
            melodia_trick: melodia,
            pitch_bends: true,
        };
        let got = notes_in_frames(&post, &opts);
        assert_eq!(got.len(), want.len(), "{name}: {got:?}");
        let with_bends = notes(&post, &opts);
        for (i, ((g, n), w)) in got.iter().zip(&with_bends).zip(want).enumerate() {
            assert_eq!((g.0, g.1, g.2), (w.0, w.1, w.2), "{name} note {i}");
            assert!(
                (g.3 - w.3).abs() < 1e-6,
                "{name} note {i}: {} vs {}",
                g.3,
                w.3
            );
            assert_eq!(n.bends.as_deref(), Some(w.4), "{name} note {i} bends");
        }
    }
}

#[test]
fn frame_times_match_python() {
    let t = frame_times(N_FRAMES);
    assert!((t[172] - 1.986590022675737).abs() < 1e-12);
    assert!((t[299] - 3.461057142857143).abs() < 1e-12);
    assert_eq!(t[0], 0.0);
}

#[test]
fn min_note_frames_rounds_like_numpy() {
    assert_eq!(NoteOptions::default().min_note_frames(), 11);
    let ms = |v| NoteOptions {
        min_note_ms: v,
        ..NoteOptions::default()
    };
    assert_eq!(ms(0.0).min_note_frames(), 0);
    assert_eq!(ms(58.0).min_note_frames(), 5);
}

#[test]
fn velocity_never_zero() {
    let n = |a| Note {
        start: 0.0,
        end: 1.0,
        pitch: 60,
        amplitude: a,
        bends: None,
    };
    assert_eq!(n(0.0).velocity(), 1);
    assert_eq!(n(0.5).velocity(), 64);
    assert_eq!(n(1.0).velocity(), 127);
}

#[test]
fn empty_posteriorgrams_give_no_notes() {
    assert!(notes(&Posteriorgrams::default(), &NoteOptions::default()).is_empty());
}

#[test]
fn windows_overlap_and_unwrap() {
    // A stub model that writes each window's first sample (its index) plus the frame number
    // into every output cell, so the unwrapped frames show where they came from.
    let net = FnNetwork {
        shapes: vec![vec![-1, AUDIO_N_SAMPLES as i64, 1]],
        f: |inputs: &[TensorIn<'_>]| {
            let x = inputs[0].data;
            assert_eq!(x.len(), AUDIO_N_SAMPLES);
            let mk = |w: usize, base: f32| TensorOut {
                shape: vec![1, ANNOT_N_FRAMES, w],
                data: (0..ANNOT_N_FRAMES * w)
                    .map(|i| base + x[0] + (i / w) as f32 * 1e-3)
                    .collect(),
            };
            vec![
                mk(N_NOTES, 0.0),
                mk(N_NOTES, 10.0),
                mk(N_CONTOUR_BINS, 20.0),
            ]
        },
    };
    let mut bp = BasicPitch::with_network(Box::new(net));
    let len = 100_000;
    let pad = OVERLAP_LEN / 2;
    let mono: Vec<f32> = (0..len)
        .map(|i| {
            let p = i + pad;
            if p.is_multiple_of(HOP_SIZE) {
                (p / HOP_SIZE) as f32
            } else {
                0.0
            }
        })
        .collect();
    let mut calls = 0;
    let post = bp
        .transcribe_22k(&mono, &mut |_| calls += 1, &AtomicBool::new(false))
        .unwrap();
    let n_windows = (len + pad).div_ceil(HOP_SIZE);
    assert_eq!(calls, n_windows);
    let expected_frames = (len as f64 * (86.0 / 22050.0)).floor() as usize;
    assert_eq!(post.n_frames, expected_frames);
    assert_eq!(post.note.len(), expected_frames * N_NOTES);
    assert_eq!(post.contour.len(), expected_frames * N_CONTOUR_BINS);
    let keep = ANNOT_N_FRAMES - N_OVERLAPPING_FRAMES;
    for frame in [0, 1, keep - 1, keep, 2 * keep + 3, expected_frames - 1] {
        let window = frame / keep;
        let within = frame % keep + N_OVERLAPPING_FRAMES / 2;
        let want = window as f32 + within as f32 * 1e-3;
        assert!(
            (post.note[frame * N_NOTES] - want).abs() < 1e-4,
            "frame {frame}"
        );
        assert!((post.onset[frame * N_NOTES + 5] - 10.0 - want).abs() < 1e-3);
        assert!((post.contour[frame * N_CONTOUR_BINS + 263] - 20.0 - want).abs() < 1e-3);
    }
}

#[test]
fn cancel_stops_inference() {
    let net = FnNetwork {
        shapes: vec![vec![-1, AUDIO_N_SAMPLES as i64, 1]],
        f: |_: &[TensorIn<'_>]| -> Vec<TensorOut> { unreachable!() },
    };
    let mut bp = BasicPitch::with_network(Box::new(net));
    let r = bp.transcribe_22k(&[0.0; 50_000], &mut |_| {}, &AtomicBool::new(true));
    assert!(matches!(r, Err(ModelError::Cancelled)));
}
