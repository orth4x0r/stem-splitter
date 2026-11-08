//! Stem playback: one rodio source that mixes all stems with per-stem gains.
//!
//! The UI talks to the audio thread only through atomics in [`Shared`], so play/pause, seek,
//! gain changes and looping never block, and the playhead position is always readable.

use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

use rodio::Source;
use stem_audio::AudioBuffer;

const NO_SEEK: u64 = u64::MAX;

fn f32_atomic(v: f32) -> AtomicU32 {
    AtomicU32::new(v.to_bits())
}
fn load_f32(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Relaxed))
}
fn store_f32(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Relaxed);
}

struct Shared {
    playing: AtomicBool,
    /// Current frame.
    pos: AtomicU64,
    /// Pending seek target, or `NO_SEEK`.
    seek: AtomicU64,
    /// Loop region `[start, end)` in frames; `end == 0` disables looping.
    loop_start: AtomicU64,
    loop_end: AtomicU64,
    gains: Vec<AtomicU32>,
    /// Peak level per stem since the UI last read it (post-gain).
    levels: Vec<AtomicU32>,
    /// Bumped when the source should die (new stems loaded / player dropped).
    alive: AtomicBool,
}

struct MixSource {
    stems: Arc<Vec<AudioBuffer>>,
    shared: Arc<Shared>,
    rate: NonZero<u32>,
    frames: u64,
    frame: u64,
    /// Smoothed gains, ramped towards the targets to avoid zipper noise.
    gains: Vec<f32>,
    right: Option<f32>,
}

impl Iterator for MixSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if let Some(r) = self.right.take() {
            return Some(r);
        }
        let sh = &self.shared;
        if !sh.alive.load(Relaxed) {
            return None;
        }
        let seek = sh.seek.swap(NO_SEEK, Relaxed);
        if seek != NO_SEEK {
            self.frame = seek.min(self.frames);
            sh.pos.store(self.frame, Relaxed);
        }
        if !sh.playing.load(Relaxed) {
            self.right = Some(0.0);
            return Some(0.0);
        }
        let (ls, le) = (sh.loop_start.load(Relaxed), sh.loop_end.load(Relaxed));
        if le > ls && self.frame >= le {
            self.frame = ls;
        }
        if self.frame >= self.frames {
            sh.playing.store(false, Relaxed);
            self.frame = 0;
            sh.pos.store(0, Relaxed);
            self.right = Some(0.0);
            return Some(0.0);
        }
        let i = self.frame as usize;
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for (s, stem) in self.stems.iter().enumerate() {
            let target = load_f32(&sh.gains[s]);
            let g = &mut self.gains[s];
            *g += (target - *g) * 0.002;
            if *g < 1e-4 && target == 0.0 {
                *g = 0.0;
                continue;
            }
            let n = stem.channels.len();
            if n == 0 || i >= stem.frames() {
                continue;
            }
            let sl = stem.channels[0][i] * *g;
            let sr = stem.channels[1.min(n - 1)][i] * *g;
            l += sl;
            r += sr;
            let peak = sl.abs().max(sr.abs());
            if peak > load_f32(&sh.levels[s]) {
                store_f32(&sh.levels[s], peak);
            }
        }
        self.frame += 1;
        sh.pos.store(self.frame, Relaxed);
        self.right = Some(r.clamp(-1.0, 1.0));
        Some(l.clamp(-1.0, 1.0))
    }
}

impl Source for MixSource {
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

/// Owns the output device and the currently loaded stems.
pub struct Player {
    sink: Option<rodio::MixerDeviceSink>,
    shared: Option<Arc<Shared>>,
    rate: u32,
    frames: u64,
    /// Why there is no audio output, if there isn't.
    pub error: Option<String>,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    pub fn new() -> Self {
        // Opening the device is deferred until stems are loaded, so the app starts instantly
        // and machines without audio still run.
        Self {
            sink: None,
            shared: None,
            rate: 44_100,
            frames: 0,
            error: None,
        }
    }

    fn ensure_sink(&mut self) -> bool {
        if self.sink.is_none() && self.error.is_none() {
            match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut s) => {
                    s.log_on_drop(false);
                    self.sink = Some(s);
                }
                Err(e) => {
                    tracing::warn!("no audio output: {e}");
                    self.error = Some(format!("No audio output: {e}"));
                }
            }
        }
        self.sink.is_some()
    }

    /// Replace the loaded stems. All stems must share a sample rate.
    pub fn load(&mut self, stems: Arc<Vec<AudioBuffer>>) {
        self.unload();
        let rate = stems.first().map_or(44_100, |s| s.sample_rate).max(1);
        let frames = stems.iter().map(|s| s.frames()).max().unwrap_or(0) as u64;
        let n = stems.len();
        let shared = Arc::new(Shared {
            playing: AtomicBool::new(false),
            pos: AtomicU64::new(0),
            seek: AtomicU64::new(NO_SEEK),
            loop_start: AtomicU64::new(0),
            loop_end: AtomicU64::new(0),
            gains: (0..n).map(|_| f32_atomic(1.0)).collect(),
            levels: (0..n).map(|_| f32_atomic(0.0)).collect(),
            alive: AtomicBool::new(true),
        });
        self.rate = rate;
        self.frames = frames;
        if self.ensure_sink()
            && let Some(sink) = &self.sink
        {
            sink.mixer().add(MixSource {
                stems,
                shared: shared.clone(),
                rate: NonZero::new(rate).unwrap(),
                frames,
                frame: 0,
                gains: vec![1.0; n],
                right: None,
            });
        }
        self.shared = Some(shared);
    }

    pub fn unload(&mut self) {
        if let Some(sh) = self.shared.take() {
            sh.alive.store(false, Relaxed);
        }
        self.frames = 0;
    }

    pub fn is_loaded(&self) -> bool {
        self.shared.is_some()
    }

    pub fn has_output(&self) -> bool {
        self.sink.is_some()
    }

    pub fn is_playing(&self) -> bool {
        self.shared
            .as_ref()
            .is_some_and(|s| s.playing.load(Relaxed))
    }

    pub fn set_playing(&mut self, play: bool) {
        if let Some(sh) = &self.shared {
            if play && self.sink.is_none() {
                return;
            }
            sh.playing.store(play, Relaxed);
        }
    }

    pub fn toggle(&mut self) {
        let p = self.is_playing();
        self.set_playing(!p);
    }

    pub fn duration(&self) -> f64 {
        self.frames as f64 / self.rate as f64
    }

    /// Playhead in seconds.
    pub fn position(&self) -> f64 {
        self.shared.as_ref().map_or(0.0, |s| {
            let pending = s.seek.load(Relaxed);
            (if pending != NO_SEEK {
                pending
            } else {
                s.pos.load(Relaxed)
            }) as f64
        }) / self.rate as f64
    }

    pub fn seek(&mut self, secs: f64) {
        if let Some(sh) = &self.shared {
            let f = ((secs.max(0.0) * self.rate as f64) as u64).min(self.frames);
            sh.seek.store(f, Relaxed);
            if self.sink.is_none() {
                sh.pos.store(f, Relaxed);
            }
        }
    }

    pub fn set_gain(&mut self, stem: usize, gain: f32) {
        if let Some(g) = self.shared.as_ref().and_then(|s| s.gains.get(stem)) {
            store_f32(g, gain);
        }
    }

    /// Loop `[start, end)` seconds, or stop looping with `None`.
    pub fn set_loop(&mut self, region: Option<(f64, f64)>) {
        if let Some(sh) = &self.shared {
            let (a, b) = region
                .map(|(a, b)| ((a * self.rate as f64) as u64, (b * self.rate as f64) as u64))
                .unwrap_or((0, 0));
            sh.loop_start.store(a, Relaxed);
            sh.loop_end.store(b, Relaxed);
        }
    }

    /// Peak level per stem since the last call (resets the meters).
    pub fn take_levels(&mut self) -> Vec<f32> {
        self.shared.as_ref().map_or_else(Vec::new, |s| {
            s.levels
                .iter()
                .map(|l| f32::from_bits(l.swap(0f32.to_bits(), Relaxed)))
                .collect()
        })
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.unload();
    }
}
