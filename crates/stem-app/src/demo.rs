//! Demo mode (`STEM_DEMO=1`): a little synthesised boom-bap loop split into "stems", so the
//! whole UI can be developed, screenshotted and played with before the backends exist.
//! Buffers are built directly with `AudioBuffer::new`; no stem-audio DSP is used.

use std::f32::consts::TAU;

use stem_audio::AudioBuffer;
use stem_models::StemKind;

pub const DEMO_TITLE: &str = "Midnight Drive (demo beat)";
const SR: u32 = 44_100;
const BPM: f32 = 90.0;
const BARS: usize = 12;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

fn beat_len() -> f32 {
    60.0 / BPM
}

pub fn demo_duration() -> f32 {
    BARS as f32 * 4.0 * beat_len()
}

fn frames() -> usize {
    (demo_duration() * SR as f32) as usize
}

fn midi(n: f32) -> f32 {
    440.0 * 2f32.powf((n - 69.0) / 12.0)
}

/// Am7 - Fmaj7 - Cmaj7 - G6, as MIDI notes (root first).
const CHORDS: [[f32; 4]; 4] = [
    [45.0, 60.0, 64.0, 67.0],
    [41.0, 57.0, 60.0, 64.0],
    [48.0, 59.0, 64.0, 67.0],
    [43.0, 59.0, 62.0, 64.0],
];

fn chord_at(t: f32) -> &'static [f32; 4] {
    let bar = (t / (4.0 * beat_len())) as usize;
    &CHORDS[bar % CHORDS.len()]
}

/// Adds `v` (scaled by left/right pan gains) at frame `i`.
fn put(l: &mut [f32], r: &mut [f32], i: usize, v: f32, pan: f32) {
    if i < l.len() {
        l[i] += v * (1.0 - pan.max(0.0));
        r[i] += v * (1.0 + pan.min(0.0));
    }
}

fn drums(l: &mut [f32], r: &mut [f32]) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let sr = SR as f32;
    let eighth = beat_len() / 2.0;
    let steps = BARS * 8;
    for s in 0..steps {
        let t0 = s as f32 * eighth;
        let start = (t0 * sr) as usize;
        let pos = s % 8;
        let bar = s / 8;
        // Kick on 1, the "and" of 2 (every other bar) and 3.
        if pos == 0 || pos == 4 || (pos == 3 && bar % 2 == 1) {
            let mut phase = 0.0f32;
            for k in 0..(0.45 * sr) as usize {
                let t = k as f32 / sr;
                let f = 45.0 + 85.0 * (-t * 28.0).exp();
                phase += TAU * f / sr;
                let env = (-t * 7.0).exp();
                let click = if k < 60 { 0.4 } else { 0.0 };
                put(
                    l,
                    r,
                    start + k,
                    (phase.sin() * env + click * rng.next()) * 0.9,
                    0.0,
                );
            }
        }
        // Snare on 2 and 4.
        if pos == 2 || pos == 6 {
            for k in 0..(0.28 * sr) as usize {
                let t = k as f32 / sr;
                let n = rng.next() * (-t * 16.0).exp() * 0.5;
                let tone = (TAU * 185.0 * t).sin() * (-t * 30.0).exp() * 0.35;
                put(l, r, start + k, n + tone, 0.05);
            }
        }
        // Hats on every eighth, swung, alternating accent.
        let swing = if s % 2 == 1 { 0.06 * beat_len() } else { 0.0 };
        let hstart = ((t0 + swing) * sr) as usize;
        let accent = if s % 2 == 0 { 0.22 } else { 0.12 };
        let mut prev = 0.0;
        for k in 0..(0.05 * sr) as usize {
            let t = k as f32 / sr;
            let n = rng.next();
            let hp = n - prev;
            prev = n;
            put(l, r, hstart + k, hp * (-t * 90.0).exp() * accent, -0.35);
        }
    }
}

fn bass(l: &mut [f32], r: &mut [f32]) {
    let sr = SR as f32;
    let b = beat_len();
    // (beat offset in bar, length in beats, octave shift)
    let pattern = [
        (0.0, 1.3, 0.0),
        (1.5, 0.45, 0.0),
        (2.0, 1.2, 0.0),
        (3.5, 0.4, 12.0),
    ];
    for bar in 0..BARS {
        let root = CHORDS[bar % CHORDS.len()][0];
        for &(off, len, oct) in &pattern {
            let start = ((bar as f32 * 4.0 + off) * b * sr) as usize;
            let f = midi(root + oct);
            let n = (len * b * sr) as usize;
            for k in 0..n {
                let t = k as f32 / sr;
                let env = (t * 200.0).min(1.0) * (1.0 - k as f32 / n as f32).powf(0.6);
                let x = (TAU * f * t).sin() + 0.35 * (TAU * 2.0 * f * t).sin();
                put(l, r, start + k, (x * 1.6).tanh() * env * 0.55, 0.0);
            }
        }
    }
}

fn keys(l: &mut [f32], r: &mut [f32]) {
    let sr = SR as f32;
    for i in 0..l.len() {
        let t = i as f32 / sr;
        let chord = chord_at(t);
        let bar_t = t % (4.0 * beat_len());
        let env = (bar_t * 3.0).min(1.0) * (0.75 + 0.25 * (TAU * 0.5 * t).sin());
        let trem = 0.85 + 0.15 * (TAU * 4.5 * t).sin();
        let mut vl = 0.0;
        let mut vr = 0.0;
        for (j, &n) in chord.iter().skip(1).enumerate() {
            let f = midi(n);
            vl += (TAU * f * 1.002 * t + j as f32).sin();
            vr += (TAU * f * 0.998 * t + j as f32 * 2.0).sin();
        }
        l[i] += vl * 0.075 * env * trem;
        r[i] += vr * 0.075 * env * trem;
    }
}

fn plucks(l: &mut [f32], r: &mut [f32], bright: f32, pan: f32) {
    let sr = SR as f32;
    let sixteenth = beat_len() / 4.0;
    let steps = BARS * 16;
    for s in 0..steps {
        if s % 3 == 2 {
            continue;
        }
        let t0 = s as f32 * sixteenth;
        let chord = chord_at(t0);
        let note = chord[1 + (s % 3)] + 12.0;
        let f = midi(note);
        let start = (t0 * sr) as usize;
        for k in 0..(0.35 * sr) as usize {
            let t = k as f32 / sr;
            let env = (-t * 9.0).exp();
            let x = (TAU * f * t).sin() + bright * (TAU * 3.0 * f * t).sin() * (-t * 20.0).exp();
            put(l, r, start + k, x * env * 0.12, pan);
        }
    }
}

fn vocals(l: &mut [f32], r: &mut [f32]) {
    let sr = SR as f32;
    let b = beat_len();
    // A minor pentatonic phrase: (beat, length, midi)
    let phrase = [
        (0.0, 0.9, 69.0),
        (1.0, 0.5, 72.0),
        (1.5, 1.4, 74.0),
        (3.0, 0.9, 72.0),
        (4.0, 1.8, 69.0),
        (6.0, 0.5, 67.0),
        (6.5, 1.3, 64.0),
    ];
    for rep in 0..(BARS / 2) {
        if rep % 3 == 2 {
            continue; // breathe
        }
        let base = rep as f32 * 8.0;
        for &(off, len, n) in &phrase {
            let start = ((base + off) * b * sr) as usize;
            let f0 = midi(n);
            let cnt = (len * b * sr) as usize;
            let mut phase = 0.0f32;
            for k in 0..cnt {
                let t = k as f32 / sr;
                let vib = 1.0 + 0.012 * (TAU * 5.5 * t).sin() * (t * 2.0).min(1.0);
                phase += TAU * f0 * vib / sr;
                let a = (t * 18.0).min(1.0) * (1.0 - k as f32 / cnt as f32).powf(0.35);
                let x = phase.sin()
                    + 0.45 * (2.0 * phase).sin()
                    + 0.25 * (3.0 * phase).sin()
                    + 0.12 * (5.0 * phase).sin();
                put(l, r, start + k, x * a * 0.22, 0.0);
            }
        }
    }
}

/// Synthesise one stem.
pub fn stem(kind: StemKind) -> AudioBuffer {
    let n = frames();
    let mut l = vec![0.0f32; n];
    let mut r = vec![0.0f32; n];
    match kind {
        StemKind::Vocals | StemKind::Dialogue | StemKind::LeadVocals => vocals(&mut l, &mut r),
        StemKind::BackingVocals => plucks(&mut l, &mut r, 0.05, 0.0),
        StemKind::Drums | StemKind::Effects => drums(&mut l, &mut r),
        StemKind::Bass => bass(&mut l, &mut r),
        StemKind::Other | StemKind::Music => keys(&mut l, &mut r),
        StemKind::Guitar => plucks(&mut l, &mut r, 0.6, -0.4),
        StemKind::Piano => plucks(&mut l, &mut r, 0.15, 0.4),
        StemKind::Instrumental => {
            drums(&mut l, &mut r);
            bass(&mut l, &mut r);
            keys(&mut l, &mut r);
        }
    }
    AudioBuffer::new(SR, vec![l, r])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_are_sane() {
        for k in [
            StemKind::Vocals,
            StemKind::Drums,
            StemKind::Bass,
            StemKind::Other,
        ] {
            let b = stem(k);
            assert_eq!(b.n_channels(), 2);
            assert!(b.frames() > SR as usize * 20);
            let peak = b.channels[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.05 && peak < 2.0, "{k:?} peak {peak}");
        }
    }
}
