#![allow(unused_variables)] // TODO: remove once stubs are implemented
//! Audio I/O and DSP shared by the rest of the workspace.
//!
//! Everything works on [`AudioBuffer`]: planar `f32` samples in `[-1, 1]`.

pub mod stft;

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported or corrupt audio: {0}")]
    Decode(String),
    #[error("resample: {0}")]
    Resample(String),
    #[error("encode: {0}")]
    Encode(String),
}

pub type Result<T> = std::result::Result<T, AudioError>;

/// Planar audio: `channels[c][i]` is sample `i` of channel `c`. All channels have equal length.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AudioBuffer {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl AudioBuffer {
    pub fn new(sample_rate: u32, channels: Vec<Vec<f32>>) -> Self {
        Self {
            sample_rate,
            channels,
        }
    }
    pub fn silence(sample_rate: u32, n_channels: usize, frames: usize) -> Self {
        Self {
            sample_rate,
            channels: vec![vec![0.0; frames]; n_channels],
        }
    }
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }
    pub fn n_channels(&self) -> usize {
        self.channels.len()
    }
    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / self.sample_rate.max(1) as f64
    }
    /// Mono is duplicated, more than two channels are downmixed.
    pub fn to_stereo(&self) -> AudioBuffer {
        todo!("stem-audio")
    }
    /// Copy of frames `[start, end)`, clamped to the buffer.
    pub fn slice(&self, start: usize, end: usize) -> AudioBuffer {
        todo!("stem-audio")
    }
    /// `self - other`, sample by sample (used for "instrumental = mix - vocals").
    pub fn subtract(&self, other: &AudioBuffer) -> AudioBuffer {
        todo!("stem-audio")
    }
    /// Min/max envelope for waveform drawing: `buckets` pairs over the whole buffer, all channels.
    pub fn peaks(&self, buckets: usize) -> Vec<(f32, f32)> {
        todo!("stem-audio")
    }
    /// Sum of several equal-rate buffers (shorter ones are zero-padded). Used for re-mixing stems.
    pub fn mix(buffers: &[&AudioBuffer]) -> AudioBuffer {
        todo!("stem-audio")
    }
    /// Scale so the peak hits `target` (e.g. 0.99). Silence is left alone.
    pub fn normalize(&mut self, target: f32) {
        todo!("stem-audio")
    }
    /// Linear fade in/out over the given number of frames (avoids clicks on exported samples).
    pub fn fade(&mut self, fade_in: usize, fade_out: usize) {
        todo!("stem-audio")
    }
    /// Peak absolute sample value.
    pub fn peak(&self) -> f32 {
        todo!("stem-audio")
    }
}

/// Decode any format symphonia understands (wav, flac, mp3, aac/m4a, ogg/vorbis, ...).
pub fn decode_file(path: &Path) -> Result<AudioBuffer> {
    todo!("stem-audio")
}

/// High-quality resample. Returns a clone when the rate already matches.
pub fn resample(buf: &AudioBuffer, target_rate: u32) -> Result<AudioBuffer> {
    todo!("stem-audio")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WavFormat {
    Pcm16,
    #[default]
    Pcm24,
    Float32,
}

pub fn write_wav(path: &Path, buf: &AudioBuffer, format: WavFormat) -> Result<()> {
    todo!("stem-audio")
}
