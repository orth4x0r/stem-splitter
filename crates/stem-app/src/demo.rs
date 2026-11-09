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

/// The pieces of the demo kit, so a drum split has real parts to show.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Piece {
    Kick,
    Snare,
    Hat,
    Toms,
    Ride,
    Crash,
}

const KIT: [Piece; 6] = [
    Piece::Kick,
    Piece::Snare,
    Piece::Hat,
    Piece::Toms,
    Piece::Ride,
    Piece::Crash,
];

fn drums(l: &mut [f32], r: &mut [f32]) {
    kit(l, r, &KIT);
}

/// The demo beat's drums, only the given pieces. Each piece has its own noise source, so the
/// parts add up to the full kit exactly.
fn kit(l: &mut [f32], r: &mut [f32], pieces: &[Piece]) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut snare_rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut hat_rng = Rng(0x1405_7B7E_F767_814F);
    let mut crash_rng = Rng(0x5851_F42D_4C95_7F2D);
    let mut ride_rng = Rng(0x6C62_272E_07BB_0142);
    let has = |p: Piece| pieces.contains(&p);
    let sr = SR as f32;
    let eighth = beat_len() / 2.0;
    let steps = BARS * 8;
    for s in 0..steps {
        let t0 = s as f32 * eighth;
        let start = (t0 * sr) as usize;
        let pos = s % 8;
        let bar = s / 8;
        // A tom fill closes every fourth bar; the kick and snare sit it out.
        let fill = bar % 4 == 3 && pos >= 6;
        // Kick on 1, the "and" of 2 (every other bar) and 3.
        if has(Piece::Kick) && !fill && (pos == 0 || pos == 4 || (pos == 3 && bar % 2 == 1)) {
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
        if has(Piece::Snare) && !fill && (pos == 2 || pos == 6) {
            for k in 0..(0.28 * sr) as usize {
                let t = k as f32 / sr;
                let n = snare_rng.next() * (-t * 16.0).exp() * 0.5;
                let tone = (TAU * 185.0 * t).sin() * (-t * 30.0).exp() * 0.35;
                put(l, r, start + k, n + tone, 0.05);
            }
        }
        // Toms: four sixteenths falling high to low.
        if has(Piece::Toms) && fill {
            for j in 0..2 {
                let hit = (pos - 6) * 2 + j;
                let f0 = [196.0, 165.0, 131.0, 98.0][hit];
                let ts = start + (j as f32 * eighth / 2.0 * sr) as usize;
                for k in 0..(0.32 * sr) as usize {
                    let t = k as f32 / sr;
                    let f = f0 * (1.0 + 0.35 * (-t * 30.0).exp());
                    let v = (TAU * f * t).sin() * (-t * 9.0).exp() * 0.55;
                    put(l, r, ts + k, v, 0.45 - 0.3 * hit as f32);
                }
            }
        }
        // Crash on the downbeat after each fill; ride on the quarters of the last four bars.
        let crash = pos == 0 && bar % 4 == 0 && bar > 0;
        let ride = bar >= BARS - 4 && pos % 2 == 0 && !crash;
        if (has(Piece::Crash) && crash) || (has(Piece::Ride) && ride) {
            let (len, decay, amp, pan) = if crash {
                (1.6, 2.4, 0.2, 0.4)
            } else {
                (0.7, 4.5, 0.09, 0.55)
            };
            let mut prev = 0.0;
            for k in 0..(len * sr) as usize {
                let t = k as f32 / sr;
                let n = if crash {
                    crash_rng.next()
                } else {
                    ride_rng.next()
                };
                let hp = n - prev;
                prev = n;
                let bell = if crash {
                    0.0
                } else {
                    (TAU * 3_150.0 * t).sin() * 0.6 + (TAU * 4_710.0 * t).sin() * 0.4
                };
                put(l, r, start + k, (hp + bell) * (-t * decay).exp() * amp, pan);
            }
        }
        // Hats on every eighth, swung, alternating accent; they rest under the ride.
        if !has(Piece::Hat) || bar >= BARS - 4 {
            continue;
        }
        let swing = if s % 2 == 1 { 0.06 * beat_len() } else { 0.0 };
        let hstart = ((t0 + swing) * sr) as usize;
        let accent = if s % 2 == 0 { 0.22 } else { 0.12 };
        let mut prev = 0.0;
        for k in 0..(0.05 * sr) as usize {
            let t = k as f32 / sr;
            let n = hat_rng.next();
            let hp = n - prev;
            prev = n;
            put(l, r, hstart + k, hp * (-t * 90.0).exp() * accent, -0.35);
        }
    }
}

/// A cheap stereo room on `l` / `r` (feedback combs, wet only): the demo's "reverb" stem.
fn room(l: &[f32], r: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let combs = [
        (1557usize, 0.82f32),
        (1617, 0.8),
        (1491, 0.83),
        (1422, 0.81),
    ];
    let tail = |x: &[f32], spread: usize| -> Vec<f32> {
        let mut out = vec![0.0f32; x.len()];
        for &(d, g) in &combs {
            let d = d + spread;
            let mut buf = vec![0.0f32; x.len()];
            for i in 0..x.len() {
                let fb = if i >= d { buf[i - d] } else { 0.0 };
                buf[i] = x[i] + g * fb;
                out[i] += fb * 0.16;
            }
        }
        out
    };
    (tail(l, 0), tail(r, 23))
}

/// Bass line per bar: (beat offset in bar, length in beats, octave shift).
const BASS_PATTERN: [(f32, f32, f32); 4] = [
    (0.0, 1.3, 0.0),
    (1.5, 0.45, 0.0),
    (2.0, 1.2, 0.0),
    (3.5, 0.4, 12.0),
];

/// A minor pentatonic vocal phrase over two bars: (beat, length, midi).
const PHRASE: [(f32, f32, f32); 7] = [
    (0.0, 0.9, 69.0),
    (1.0, 0.5, 72.0),
    (1.5, 1.4, 74.0),
    (3.0, 0.9, 72.0),
    (4.0, 1.8, 69.0),
    (6.0, 0.5, 67.0),
    (6.5, 1.3, 64.0),
];

/// The notes a stem was synthesised from, as (start s, length s, MIDI pitch, level 0..1):
/// demo mode's stand-in for running the transcription model.
pub fn notes(kind: StemKind) -> Vec<(f32, f32, u8, f32)> {
    let b = beat_len();
    let mut out = Vec::new();
    match kind {
        StemKind::Bass | StemKind::Instrumental => {
            for bar in 0..BARS {
                let root = CHORDS[bar % CHORDS.len()][0];
                for &(off, len, oct) in &BASS_PATTERN {
                    let vel = if oct > 0.0 { 0.62 } else { 0.85 };
                    out.push((
                        (bar as f32 * 4.0 + off) * b,
                        len * b,
                        (root + oct) as u8,
                        vel,
                    ));
                }
            }
        }
        StemKind::Vocals | StemKind::Dialogue | StemKind::LeadVocals => {
            for rep in 0..(BARS / 2) {
                if rep % 3 == 2 {
                    continue;
                }
                for (j, &(off, len, n)) in PHRASE.iter().enumerate() {
                    let vel = 0.62 + 0.06 * ((j * 5 + rep) % 5) as f32;
                    out.push(((rep as f32 * 8.0 + off) * b, len * b, n as u8, vel));
                }
            }
        }
        StemKind::Drums | StemKind::Effects => {}
        StemKind::Kick
        | StemKind::Snare
        | StemKind::Toms
        | StemKind::HiHat
        | StemKind::Ride
        | StemKind::Crash
        | StemKind::Reverb
        | StemKind::Noise => {}
        StemKind::Dry | StemKind::Clean => return notes(StemKind::Vocals),
        StemKind::Guitar | StemKind::Piano | StemKind::BackingVocals => {
            let sixteenth = b / 4.0;
            for s in 0..BARS * 16 {
                if s % 3 == 2 {
                    continue;
                }
                let t0 = s as f32 * sixteenth;
                let note = chord_at(t0)[1 + (s % 3)] + 12.0;
                out.push((t0, 0.3, note as u8, 0.55 + 0.1 * (s % 2) as f32));
            }
        }
        StemKind::Other | StemKind::Music => {
            for bar in 0..BARS {
                for (j, &n) in CHORDS[bar % CHORDS.len()].iter().skip(1).enumerate() {
                    out.push((
                        bar as f32 * 4.0 * b,
                        4.0 * b - 0.05,
                        n as u8,
                        0.7 - 0.06 * j as f32,
                    ));
                }
            }
        }
    }
    out
}

fn bass(l: &mut [f32], r: &mut [f32]) {
    let sr = SR as f32;
    let b = beat_len();
    for bar in 0..BARS {
        let root = CHORDS[bar % CHORDS.len()][0];
        for &(off, len, oct) in &BASS_PATTERN {
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
    for rep in 0..(BARS / 2) {
        if rep % 3 == 2 {
            continue; // breathe
        }
        let base = rep as f32 * 8.0;
        for &(off, len, n) in &PHRASE {
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
        StemKind::Kick => kit(&mut l, &mut r, &[Piece::Kick]),
        StemKind::Snare => kit(&mut l, &mut r, &[Piece::Snare]),
        StemKind::Toms => kit(&mut l, &mut r, &[Piece::Toms]),
        StemKind::HiHat => kit(&mut l, &mut r, &[Piece::Hat]),
        StemKind::Ride => kit(&mut l, &mut r, &[Piece::Ride]),
        StemKind::Crash => kit(&mut l, &mut r, &[Piece::Crash]),
        StemKind::Dry | StemKind::Clean => vocals(&mut l, &mut r),
        StemKind::Reverb => {
            vocals(&mut l, &mut r);
            (l, r) = room(&l, &r);
        }
        StemKind::Noise => {
            let mut rng = Rng(0xA076_1D64_78BD_642F);
            let (mut lp, mut rp) = (0.0f32, 0.0f32);
            for i in 0..n {
                // Soft hiss with a little 60 Hz hum.
                lp = 0.85 * lp + 0.15 * rng.next();
                rp = 0.85 * rp + 0.15 * rng.next();
                let hum = 0.012 * (TAU * 60.0 * i as f32 / SR as f32).sin();
                l[i] = lp * 0.05 + hum;
                r[i] = rp * 0.05 + hum;
            }
        }
    }
    AudioBuffer::new(SR, vec![l, r])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drum_parts_add_up_to_the_kit() {
        let full = stem(StemKind::Drums);
        let parts: Vec<_> = StemKind::DRUM_PARTS.iter().map(|k| stem(*k)).collect();
        let sum = AudioBuffer::mix(&parts.iter().collect::<Vec<_>>());
        let err = full.channels[0]
            .iter()
            .zip(&sum.channels[0])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 1e-4, "{err}");
        for p in &parts {
            let peak = p.channels[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.02, "every part sounds");
        }
        let reverb = stem(StemKind::Reverb);
        assert!(reverb.channels[1].iter().any(|v| v.abs() > 0.01));
    }

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
