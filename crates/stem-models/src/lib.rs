#![allow(unused_variables)] // TODO: remove once stubs are implemented
//! Source separation.
//!
//! * [`registry`] lists the models we know how to run and where to download them.
//! * [`ModelManager`] downloads/caches model files and the ONNX Runtime library.
//! * [`load`] builds a [`Separator`] for a downloaded model on a [`Device`].

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use stem_audio::AudioBuffer;

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("download: {0}")]
    Download(String),
    #[error("onnx runtime: {0}")]
    Runtime(String),
    #[error("audio: {0}")]
    Audio(#[from] stem_audio::AudioError),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, ModelError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum StemKind {
    Vocals,
    Instrumental,
    Drums,
    Bass,
    Guitar,
    Piano,
    Other,
    Dialogue,
    Music,
    Effects,
}

impl StemKind {
    pub fn label(self) -> &'static str {
        match self {
            StemKind::Vocals => "Vocals",
            StemKind::Instrumental => "Instrumental",
            StemKind::Drums => "Drums",
            StemKind::Bass => "Bass",
            StemKind::Guitar => "Guitar",
            StemKind::Piano => "Piano",
            StemKind::Other => "Other",
            StemKind::Dialogue => "Dialogue",
            StemKind::Music => "Music",
            StemKind::Effects => "Effects",
        }
    }
}

/// How to drive a model's network; parameters come from the model's published config.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ModelArch {
    /// UVR MDX-Net: STFT in Rust, network predicts one stem's complex spectrogram.
    MdxNet {
        n_fft: usize,
        hop: usize,
        dim_f: usize,
        dim_t: usize,
        compensate: f32,
    },
    /// HTDemucs exported with the STFT outside the graph (inputs: waveform + spectrogram).
    HtDemucs { segment_samples: usize },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Stems this model outputs (complements like "instrumental" included).
    pub stems: Vec<StemKind>,
    pub sample_rate: u32,
    pub arch: ModelArch,
    pub url: &'static str,
    /// Hex sha256, or `None` until we pin one.
    pub sha256: Option<&'static str>,
    pub size_bytes: u64,
    pub license: &'static str,
}

/// Every model the app offers.
pub fn registry() -> Vec<ModelInfo> {
    todo!("stem-models")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Device {
    /// DirectML on Windows if available, otherwise CPU.
    #[default]
    Auto,
    Cpu,
    /// DirectML adapter index (0 is usually the discrete GPU on laptops, but not always).
    DirectMl(u32),
}

/// Devices the user can pick from, with display names (e.g. "AMD Radeon RX 6700S (DirectML)").
/// Always includes [`Device::Cpu`]; DirectML adapters are listed on Windows only.
pub fn list_devices() -> Vec<(Device, String)> {
    todo!("stem-models")
}

#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Downloading {
        fraction: Option<f32>,
    },
    LoadingModel,
    /// Fraction of the audio processed so far.
    Separating(f32),
}

/// Downloads and caches models and the ONNX Runtime library under `root`.
#[derive(Debug, Clone)]
pub struct ModelManager {
    pub root: PathBuf,
}

impl ModelManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn model_path(&self, info: &ModelInfo) -> PathBuf {
        todo!("stem-models")
    }
    pub fn is_downloaded(&self, info: &ModelInfo) -> bool {
        todo!("stem-models")
    }
    pub fn download(
        &self,
        info: &ModelInfo,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<PathBuf> {
        todo!("stem-models")
    }
    /// Make sure onnxruntime (DirectML build on Windows) is present and initialise `ort` with it.
    /// Safe to call repeatedly.
    pub fn ensure_runtime(&self, progress: &mut dyn FnMut(Progress)) -> Result<()> {
        todo!("stem-models")
    }
}

pub trait Separator: Send {
    fn info(&self) -> &ModelInfo;
    /// Separate a full track. Input at any rate/channel count; output stems match the input's
    /// sample rate and length, stereo.
    fn separate(
        &mut self,
        mix: &AudioBuffer,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<Vec<(StemKind, AudioBuffer)>>;
}

/// Build a separator for an already-downloaded model.
pub fn load(info: &ModelInfo, model_path: &Path, device: Device) -> Result<Box<dyn Separator>> {
    todo!("stem-models")
}
