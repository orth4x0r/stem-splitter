//! A second, tiny player for auditioning one rendered buffer (a transform preview or a kit
//! one-shot) without touching the stem mixer in `player.rs`. It opens its own output stream
//! on first use; the OS mixes it with the stem player's.

use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

use rodio::Source;
use stem_audio::AudioBuffer;

struct Voice {
    alive: AtomicBool,
    pos: AtomicU64,
}

struct OneShot {
    audio: Arc<AudioBuffer>,
    voice: Arc<Voice>,
    frame: usize,
    right: Option<f32>,
}

impl Iterator for OneShot {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if let Some(r) = self.right.take() {
            return Some(r);
        }
        let a = &self.audio;
        if !self.voice.alive.load(Relaxed) || self.frame >= a.frames() || a.channels.is_empty() {
            self.voice.alive.store(false, Relaxed);
            return None;
        }
        let n = a.channels.len();
        let l = a.channels[0][self.frame];
        let r = a.channels[1.min(n - 1)][self.frame];
        self.frame += 1;
        self.voice.pos.store(self.frame as u64, Relaxed);
        self.right = Some(r.clamp(-1.0, 1.0));
        Some(l.clamp(-1.0, 1.0))
    }
}

impl Source for OneShot {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        NonZero::new(2).unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        NonZero::new(self.audio.sample_rate.max(1)).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Plays one buffer at a time; starting another stops the previous one.
#[derive(Default)]
pub struct Audition {
    sink: Option<rodio::MixerDeviceSink>,
    failed: bool,
    voice: Option<Arc<Voice>>,
    /// What is playing: a caller-chosen tag (which preview / which kit sample) and its length.
    pub tag: Option<String>,
    frames: u64,
}

impl Audition {
    /// Play `audio`, tagged `tag`. Returns false when there is no audio output.
    pub fn play(&mut self, audio: Arc<AudioBuffer>, tag: impl Into<String>) -> bool {
        self.stop();
        if self.sink.is_none() && !self.failed {
            match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut s) => {
                    s.log_on_drop(false);
                    self.sink = Some(s);
                }
                Err(e) => {
                    tracing::warn!("audition: no audio output: {e}");
                    self.failed = true;
                }
            }
        }
        self.tag = Some(tag.into());
        self.frames = audio.frames() as u64;
        let voice = Arc::new(Voice {
            alive: AtomicBool::new(true),
            pos: AtomicU64::new(0),
        });
        if let Some(sink) = &self.sink {
            sink.mixer().add(OneShot {
                audio,
                voice: voice.clone(),
                frame: 0,
                right: None,
            });
            self.voice = Some(voice);
            true
        } else {
            false
        }
    }

    pub fn stop(&mut self) {
        if let Some(v) = self.voice.take() {
            v.alive.store(false, Relaxed);
        }
        self.tag = None;
    }

    pub fn is_playing(&self) -> bool {
        self.voice.as_ref().is_some_and(|v| v.alive.load(Relaxed))
    }

    /// Is `tag` the buffer currently playing?
    pub fn playing(&self, tag: &str) -> bool {
        self.is_playing() && self.tag.as_deref() == Some(tag)
    }

    /// Playback position `0..=1` of the current buffer.
    pub fn progress(&self) -> f32 {
        match &self.voice {
            Some(v) if self.frames > 0 => v.pos.load(Relaxed) as f32 / self.frames as f32,
            _ => 0.0,
        }
    }
}

impl Drop for Audition {
    fn drop(&mut self) {
        self.stop();
    }
}
