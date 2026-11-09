#!/usr/bin/env python3
"""Reference outputs from Spotify's basic-pitch (Apache-2.0) for the Rust port in
crates/stem-models/src/basic_pitch.rs.

    pip install basic-pitch==0.4.0 --no-deps   (plus onnxruntime numpy scipy librosa resampy
                                                pretty_midi mir_eval)

Subcommands:

  golden            Print the expected notes for the synthetic posteriorgrams that
                    `basic_pitch/tests.rs` builds (same integer generator on both sides), as
                    Rust literals. Exercises `model_output_to_notes` without onnxruntime.
  fixtures DIR      Write DIR/melody.wav (22.05 kHz mono float, so librosa does not resample),
                    run `basic_pitch.inference.predict` on it with the packaged nmp.onnx, and
                    save DIR/ref.json (notes) and DIR/{note,onset,contour}.f32 (raw
                    posteriorgrams). Also copies nmp.onnx to DIR. Used by
                    crates/stem-models/tests/basic_pitch_ort.rs.
"""

import json
import pathlib
import shutil
import sys

import numpy as np


# ---- synthetic posteriorgrams (mirrored in basic_pitch/tests.rs) -------------------------------

N_FRAMES = 300

# (start frame, length, key index, level /256, with onset, contour bin offset)
BLOBS = [
    (5, 40, 27, 230, True, 0),
    (20, 30, 31, 200, True, 1),
    (60, 25, 27, 180, True, -1),
    (90, 60, 40, 210, False, 0),  # no onset: melodia trick
    (100, 8, 50, 220, True, 0),  # too short
    (160, 50, 45, 190, True, 2),
    (175, 4, 45, 10, False, 0),  # dip inside the previous note (gap < energy_tol)
    (230, 66, 60, 240, True, -2),  # runs into the end
    (240, 30, 0, 200, True, 0),  # lowest key
    (250, 30, 87, 200, True, 0),  # highest key
]


def lcg(seed):
    state = seed
    while True:
        state = (state * 1103515245 + 12345) % (1 << 31)
        yield state >> 16


def synthetic_posteriors():
    rnd = lcg(12345)
    note = np.zeros((N_FRAMES, 88), np.float32)
    onset = np.zeros((N_FRAMES, 88), np.float32)
    contour = np.zeros((N_FRAMES, 264), np.float32)
    for t in range(N_FRAMES):
        for f in range(88):
            note[t, f] = (next(rnd) % 48) / 256
            onset[t, f] = (next(rnd) % 40) / 256
        for c in range(264):
            contour[t, c] = (next(rnd) % 64) / 256
    for start, length, f, level, with_onset, bend in BLOBS:
        for t in range(start, min(N_FRAMES, start + length)):
            v = max(level - (t - start), 0) / 256
            if level < 50:  # a dip: overwrite
                note[t, f] = v
            else:
                note[t, f] = max(note[t, f], v)
                if f + 1 < 88:
                    note[t, f + 1] = max(note[t, f + 1], v / 2)
            c = 3 * f + bend + (1 if (t // 7) % 2 else 0)
            if 0 <= c < 264 and level >= 50:
                contour[t, c] = 230 / 256
        if with_onset:
            onset[start, f] = 220 / 256
            if start > 0:
                onset[start - 1, f] = 100 / 256
            if start + 1 < N_FRAMES:
                onset[start + 1, f] = 140 / 256
    return note, onset, contour


GOLDEN_CASES = [
    # name, onset, frame, min_note_ms, min_pitch, max_pitch, infer_onsets, melodia
    ("default", 0.5, 0.3, 127.70, None, None, True, True),
    ("strict", 0.6, 0.4, 60.0, None, None, True, False),
    ("no_infer", 0.5, 0.25, 127.70, None, None, False, True),
    ("range", 0.5, 0.3, 127.70, 40, 70, True, True),
]


def golden():
    import librosa
    from basic_pitch import note_creation as nc

    base = synthetic_posteriors()
    for name, on, fr, ms, lo, hi, infer, melodia in GOLDEN_CASES:
        note, onset, contour = (a.copy() for a in base)
        min_len = int(np.round(ms / 1000 * (22050 / 256)))
        frames = nc.output_to_notes_polyphonic(
            note.copy(),
            onset.copy(),
            onset_thresh=on,
            frame_thresh=fr,
            min_note_len=min_len,
            infer_onsets=infer,
            max_freq=None if hi is None else librosa.midi_to_hz(hi + 1),
            min_freq=None if lo is None else librosa.midi_to_hz(lo),
            melodia_trick=melodia,
        )
        with_bends = nc.get_pitch_bends(contour, frames)
        rs = lambda v: "None" if v is None else f"Some({v})"
        print(f'    ("{name}", {on!r}, {fr!r}, {ms!r}, {rs(lo)}, {rs(hi)}, {str(infer).lower()}, {str(melodia).lower()}, &[')
        for s, e, p, a, b in with_bends:
            bends = ", ".join(str(int(x)) for x in b)
            print(f"        ({s}, {e}, {p}, {float(a)!r}, &[{bends}]),")
        print("    ]),")
    times = nc.model_frames_to_time(N_FRAMES)
    print(f"// frame_times({N_FRAMES}): [172] = {float(times[172])!r}, [299] = {float(times[299])!r}")


# ---- real model fixture --------------------------------------------------------------------------

SR = 22050


def melody():
    """12 s: a bass line, a lead with vibrato, then a few chords."""
    rng = np.random.default_rng(7)
    out = np.zeros(int(SR * 12.0), np.float64)

    def tone(start, dur, midi, amp, vibrato=0.0, harmonics=6):
        n = int(dur * SR)
        t = np.arange(n) / SR
        f0 = 440.0 * 2 ** ((midi - 69) / 12)
        f = f0 * 2 ** (vibrato * np.sin(2 * np.pi * 5.5 * t) / 12)
        phase = 2 * np.pi * np.cumsum(f) / SR
        y = sum(np.sin(k * phase) / k for k in range(1, harmonics + 1))
        env = np.minimum(1.0, t / 0.01) * np.exp(-t * 1.5)
        rel = np.minimum(1.0, (dur - t) / 0.03)
        i0 = int(start * SR)
        out[i0 : i0 + n] += amp * y * env * np.maximum(rel, 0)

    bass = [33, 33, 36, 38, 40, 38, 36, 31]
    for i, m in enumerate(bass):
        tone(i * 0.5, 0.45, m, 0.35)
    lead = [69, 72, 74, 76, 74, 72, 69, 67]
    for i, m in enumerate(lead):
        tone(4.0 + i * 0.5, 0.48, m, 0.25, vibrato=0.25 if i % 3 == 0 else 0.0)
    for i, chord in enumerate([[57, 60, 64], [53, 57, 60], [55, 59, 62], [52, 55, 59]]):
        for m in chord:
            tone(8.0 + i * 1.0, 0.9, m, 0.15)
    out += 0.002 * rng.standard_normal(out.shape)
    return (out / np.max(np.abs(out)) * 0.8).astype(np.float32)


def fixtures(dirname):
    import soundfile as sf
    from basic_pitch import ICASSP_2022_MODEL_PATH
    from basic_pitch.inference import Model, predict

    d = pathlib.Path(dirname)
    d.mkdir(parents=True, exist_ok=True)
    wav = d / "melody.wav"
    sf.write(wav, melody(), SR, subtype="FLOAT")
    onnx = pathlib.Path(str(ICASSP_2022_MODEL_PATH)).with_suffix(".onnx")
    shutil.copy(onnx, d / "nmp.onnx")
    model = Model(onnx)
    output, _midi, notes = predict(wav, model)
    for k in ("note", "onset", "contour"):
        output[k].astype("<f4").tofile(d / f"{k}.f32")
    ref = {
        "n_frames": int(output["note"].shape[0]),
        "notes": [
            {
                "start": float(s),
                "end": float(e),
                "pitch": int(p),
                "amplitude": float(a),
                "bends": [int(x) for x in b] if b is not None else None,
            }
            for s, e, p, a, b in notes
        ],
    }
    (d / "ref.json").write_text(json.dumps(ref, indent=1))
    print(f"{len(notes)} notes, {ref['n_frames']} frames -> {d}")


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "golden":
        golden()
    elif len(sys.argv) >= 3 and sys.argv[1] == "fixtures":
        fixtures(sys.argv[2])
    else:
        print(__doc__)
        sys.exit(2)
