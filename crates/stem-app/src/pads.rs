//! One-shot voice mixer for the chop pads: a second rodio source next to the stem mixer.
//!
//! Slices are pre-rendered into memory (`Arc<AudioBuffer>`), so a trigger is just a message
//! on a lock-free channel; the audio thread picks it up within ~16 frames and starts the voice.
//! Voices are summed polyphonically. In choke (mono) mode a new hit fades out everything that
//! is still ringing over a few milliseconds, like an MPC mute group; re-hitting a pad always
//! restarts that pad.

use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use rodio::Source;
use stem_audio::AudioBuffer;

/// Most voices at once; the oldest is dropped beyond this.
const MAX_VOICES: usize = 32;
/// Choke / stop fade, frames.
const CHOKE_FADE: u32 = 192;
/// Commands are polled every this many frames.
const POLL_EVERY: u32 = 16;

enum Cmd {
    Play {
        pad: usize,
        buf: Arc<AudioBuffer>,
        gain: f32,
        choke: bool,
    },
    StopAll,
}

struct Voice {
    pad: usize,
    buf: Arc<AudioBuffer>,
    pos: usize,
    gain: f32,
    /// Frames left of a fade-out, when fading.
    fade: Option<u32>,
}

struct Shared {
    alive: AtomicBool,
    /// Number of voices sounding (for the UI).
    active: AtomicU32,
}

struct VoiceSource {
    rx: Receiver<Cmd>,
    shared: Arc<Shared>,
    voices: Vec<Voice>,
    rate: NonZero<u32>,
    tick: u32,
    right: Option<f32>,
}

impl VoiceSource {
    fn poll(&mut self) {
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                Cmd::Play {
                    pad,
                    buf,
                    gain,
                    choke,
                } => {
                    // Re-hitting a pad restarts it (no phasey doubling); choke cuts everything.
                    for v in &mut self.voices {
                        if choke || v.pad == pad {
                            v.fade.get_or_insert(CHOKE_FADE);
                        }
                    }
                    if self.voices.len() >= MAX_VOICES {
                        self.voices.remove(0);
                    }
                    self.voices.push(Voice {
                        pad,
                        buf,
                        pos: 0,
                        gain,
                        fade: None,
                    });
                }
                Cmd::StopAll => {
                    for v in &mut self.voices {
                        v.fade.get_or_insert(CHOKE_FADE);
                    }
                }
            }
        }
    }
}

impl Iterator for VoiceSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if let Some(r) = self.right.take() {
            return Some(r);
        }
        if !self.shared.alive.load(Relaxed) {
            return None;
        }
        self.tick += 1;
        if self.tick >= POLL_EVERY {
            self.tick = 0;
            self.poll();
        }
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for v in &mut self.voices {
            let n = v.buf.channels.len();
            if n == 0 || v.pos >= v.buf.frames() {
                v.pos = usize::MAX;
                continue;
            }
            let mut g = v.gain;
            if let Some(f) = &mut v.fade {
                g *= *f as f32 / CHOKE_FADE as f32;
                if *f == 0 {
                    v.pos = usize::MAX;
                    continue;
                }
                *f -= 1;
            }
            l += v.buf.channels[0][v.pos] * g;
            r += v.buf.channels[1.min(n - 1)][v.pos] * g;
            v.pos += 1;
        }
        if self.voices.iter().any(|v| v.pos == usize::MAX) {
            self.voices.retain(|v| v.pos != usize::MAX);
        }
        self.shared.active.store(self.voices.len() as u32, Relaxed);
        self.right = Some(soft_clip(r));
        Some(soft_clip(l))
    }
}

/// Transparent below ~-3 dBFS, then rounds off instead of hard clipping a big chord of hits.
fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.7 {
        x
    } else {
        let over = a - 0.7;
        x.signum() * (0.7 + 0.3 * (over / 0.3).tanh())
    }
}

impl Source for VoiceSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        NonZero::new(2).unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Handle to the pad voice source living in the output mixer. Dropping it ends the source.
pub struct PadVoices {
    tx: Sender<Cmd>,
    shared: Arc<Shared>,
}

impl PadVoices {
    /// Add a voice source running at `rate` to `mixer`.
    pub fn new(mixer: &rodio::mixer::Mixer, rate: u32) -> Self {
        let (tx, rx) = unbounded();
        let shared = Arc::new(Shared {
            alive: AtomicBool::new(true),
            active: AtomicU32::new(0),
        });
        mixer.add(VoiceSource {
            rx,
            shared: shared.clone(),
            voices: Vec::new(),
            rate: NonZero::new(rate.max(1)).unwrap_or(NonZero::<u32>::MIN),
            tick: 0,
            right: None,
        });
        Self { tx, shared }
    }

    /// Fire `buf` once. `choke` fades out every voice still sounding.
    pub fn trigger(&self, pad: usize, buf: Arc<AudioBuffer>, gain: f32, choke: bool) {
        let _ = self.tx.send(Cmd::Play {
            pad,
            buf,
            gain,
            choke,
        });
    }

    pub fn stop_all(&self) {
        let _ = self.tx.send(Cmd::StopAll);
    }

    pub fn active(&self) -> u32 {
        self.shared.active.load(Relaxed)
    }
}

impl Drop for PadVoices {
    fn drop(&mut self) {
        self.shared.alive.store(false, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> (Sender<Cmd>, VoiceSource) {
        let (tx, rx) = unbounded();
        let src = VoiceSource {
            rx,
            shared: Arc::new(Shared {
                alive: AtomicBool::new(true),
                active: AtomicU32::new(0),
            }),
            voices: Vec::new(),
            rate: NonZero::new(44_100).unwrap(),
            tick: 0,
            right: None,
        };
        (tx, src)
    }

    fn dc(v: f32, frames: usize) -> Arc<AudioBuffer> {
        Arc::new(AudioBuffer::new(44_100, vec![vec![v; frames]]))
    }

    #[test]
    fn voices_sum_and_end() {
        let (tx, mut src) = source();
        tx.send(Cmd::Play {
            pad: 0,
            buf: dc(0.2, 1000),
            gain: 1.0,
            choke: false,
        })
        .unwrap();
        tx.send(Cmd::Play {
            pad: 1,
            buf: dc(0.1, 100),
            gain: 1.0,
            choke: false,
        })
        .unwrap();
        // Skip the polling delay.
        let mut s: Vec<f32> = (0..2 * 40).map(|_| src.next().unwrap()).collect();
        assert!((s[2 * 30] - 0.3).abs() < 1e-6, "{}", s[60]);
        s = (0..2 * 200).map(|_| src.next().unwrap()).collect();
        assert!((s[2 * 199] - 0.2).abs() < 1e-6);
        assert_eq!(src.voices.len(), 1);
    }

    #[test]
    fn choke_fades_previous() {
        let (tx, mut src) = source();
        let play = |pad, choke| Cmd::Play {
            pad,
            buf: dc(0.5, 100_000),
            gain: 1.0,
            choke,
        };
        tx.send(play(0, false)).unwrap();
        for _ in 0..200 {
            src.next();
        }
        tx.send(play(1, true)).unwrap();
        for _ in 0..2 * (CHOKE_FADE as usize + 64) {
            src.next();
        }
        assert_eq!(src.voices.len(), 1);
        assert_eq!(src.voices[0].pad, 1);
        assert!(soft_clip(5.0) <= 1.0 && soft_clip(0.5) == 0.5);
    }
}
