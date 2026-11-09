//! Source separation.
//!
//! * [`registry`] lists the models we know how to run and where to download them.
//! * [`ModelManager`] downloads/caches model files and the ONNX Runtime library.
//! * [`load`] builds a [`Separator`] for a downloaded model on a [`Device`].
//! * [`transcribe`] turns audio into MIDI notes (Spotify Basic Pitch) and writes `.mid` files.
//!
//! Module map:
//!
//! | module      | job |
//! |-------------|-----|
//! | `catalog`   | the model table behind [`registry`] |
//! | `manager`   | [`ModelManager`]: model downloads and `ensure_runtime` |
//! | `runtime`   | locating / downloading onnxruntime, `ort` initialisation, session building |
//! | `devices`   | DXGI adapter enumeration (Windows) and [`Device`] resolution |
//! | `download`  | resumable-safe HTTP download (`.part` + rename, sha256, cancel) |
//! | `network`   | the tiny "run a graph" abstraction the separators use (ORT or a test stub) |
//! | `dsp`       | windows, overlap-add, stereo / rate helpers |
//! | `mdx`       | UVR MDX-Net separator |
//! | `demucs`    | HTDemucs (STFT outside the graph) separator |
//! | `roformer`  | complex-spectrogram separators: Mel-/BS-RoFormer and SCNet (STFT outside the graph) |
//! | `basic_pitch` | audio -> note posteriorgrams -> notes (Basic Pitch port), behind [`transcribe`] |
//! | `midi_file` | notes -> Standard MIDI File, quantisation, behind [`transcribe`] |

mod basic_pitch;
mod catalog;
mod demucs;
mod devices;
mod download;
mod dsp;
mod manager;
mod mdx;
mod midi_file;
mod network;
mod roformer;
mod runtime;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use stem_audio::AudioBuffer;

pub use catalog::{find_model, registry};
pub use devices::{GpuAdapter, gpu_adapters, list_devices};
pub use download::DownloadStatus;
pub use runtime::{DIRECTML_VERSION, ORT_VERSION};

/// Audio to MIDI: [`transcribe::BasicPitch`] runs the model, [`transcribe::notes`] turns its
/// posteriorgrams into notes (cheap, so re-run it whenever a threshold changes), and
/// [`transcribe::write_midi`] / [`transcribe::quantize`] make the `.mid` file. The model file
/// comes from [`ModelManager::download_transcriber`] with [`transcribe::BASIC_PITCH`].
pub mod transcribe {
    pub use crate::basic_pitch::{
        ANNOTATIONS_FPS, BASIC_PITCH, BasicPitch, FFT_HOP, FrameNote, MIDI_OFFSET, N_CONTOUR_BINS,
        N_NOTES, Note, NoteOptions, Posteriorgrams, SAMPLE_RATE, TranscriberInfo, frame_times,
        model_input, notes, notes_in_frames,
    };
    pub use crate::midi_file::{
        Beats, MidiOptions, PPQ, Quantize, QuantizeGrid, quantize, seconds_to_ticks, write_midi,
    };
}

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
    /// Lead (main) vocal, from karaoke models.
    LeadVocals,
    /// What a karaoke model leaves after removing the lead vocal: the backing vocals, plus the
    /// instrumental when it was fed a full mix rather than a vocal stem.
    BackingVocals,
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
            StemKind::LeadVocals => "Lead vocals",
            StemKind::BackingVocals => "Backing vocals",
        }
    }
}

/// How to drive a model's network; parameters come from the model's published config.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ModelArch {
    /// UVR MDX-Net: STFT in Rust, network predicts one stem's complex spectrogram.
    ///
    /// The network predicts the *primary* stem, which is `ModelInfo::stems[0]`; the second stem
    /// is `mix - primary`. Parameters map to UVR's `model_data.json` as `n_fft =
    /// mdx_n_fft_scale_set`, `dim_f = mdx_dim_f_set`, `dim_t = 2^mdx_dim_t_set`, `hop = 1024`.
    MdxNet {
        n_fft: usize,
        hop: usize,
        dim_f: usize,
        dim_t: usize,
        compensate: f32,
    },
    /// HTDemucs exported with the STFT outside the graph (inputs: waveform + spectrogram).
    /// See `tools/export_htdemucs_onnx.py`. `ModelInfo::stems` lists the sources in the
    /// network's output order.
    HtDemucs { segment_samples: usize },
    /// Mel-Band / BS-RoFormer exported with the STFT outside the graph (audiojs/mel-roformer,
    /// silverdaw/bs-roformer-rhythm-onnx contracts; see `roformer.rs`): periodic Hann window,
    /// unnormalised, centred reflect STFT; segments of `segment_samples` overlap-added as
    /// Music-Source-Separation-Training's `demix()` does. `overlap` is the default fraction of
    /// a segment shared with the next (`1 - 1/num_overlap`), overridable by
    /// [`LoadOptions::overlap`]. `ModelInfo::stems` lists the network's sources in output
    /// order, plus optionally one last stem computed as `mix - sum(predicted)`.
    RoFormer {
        n_fft: usize,
        hop: usize,
        segment_samples: usize,
        overlap: f32,
    },
    /// SCNet exported with the STFT outside the graph (audiojs/scnet contract, same I/O as
    /// [`ModelArch::RoFormer`]'s `mix_spec` graphs): rectangular window, `normalized=True`
    /// STFT, input standardised by its mono mean/std, segments padded as `SCNet.forward` pads
    /// them.
    Scnet {
        n_fft: usize,
        hop: usize,
        segment_samples: usize,
        overlap: f32,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Stems this model outputs (complements like "instrumental" included).
    /// For MDX-Net the first entry is the stem the network predicts.
    pub stems: Vec<StemKind>,
    pub sample_rate: u32,
    pub arch: ModelArch,
    pub url: &'static str,
    /// Hex sha256, or `None` until we pin one.
    pub sha256: Option<&'static str>,
    pub size_bytes: u64,
    pub license: &'static str,
}

impl ModelInfo {
    /// File name under `<root>/models/` (the last path segment of [`ModelInfo::url`]).
    pub fn file_name(&self) -> &'static str {
        self.url.rsplit('/').next().unwrap_or(self.id)
    }

    /// True when [`ModelInfo::url`] is not hosted yet and the file has to be exported by hand
    /// (see `tools/`) and dropped into the models folder.
    pub fn needs_manual_export(&self) -> bool {
        self.url.starts_with(catalog::UNHOSTED_URL_PREFIX)
    }

    /// A short note for the UI about how this model behaves on the GPU / CPU, if worth saying.
    pub fn device_note(&self) -> Option<&'static str> {
        match self.arch {
            ModelArch::RoFormer { .. } => Some(
                "Large transformer: runs one 8 s segment at a time and needs over 1 GB \
                 of GPU memory on DirectML. On the CPU expect several minutes per song.",
            ),
            ModelArch::Scnet { .. } => Some(
                "Small int8 model: quick on DirectML and still usable on the CPU, so pick the \
                 CPU if the GPU gives trouble.",
            ),
            ModelArch::HtDemucs { .. } | ModelArch::MdxNet { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Device {
    /// DirectML on Windows if available, otherwise CPU.
    #[default]
    Auto,
    Cpu,
    /// DirectML adapter index (0 is usually the discrete GPU on laptops, but not always).
    /// This is the DXGI `EnumAdapters1` index, which is what onnxruntime's `device_id` means.
    DirectMl(u32),
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
///
/// Layout: `<root>/models/<file>.onnx`, `<root>/runtime/onnxruntime.dll` (+ `DirectML.dll`).
#[derive(Debug, Clone)]
pub struct ModelManager {
    pub root: PathBuf,
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
    /// The device the model actually runs on ([`Device::Cpu`] after a DirectML fallback).
    fn active_device(&self) -> Device {
        Device::Cpu
    }
}

/// Tuning knobs for [`load_with`]. `None` means "the model's default".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadOptions {
    /// Fraction of each chunk that overlaps the next (MDX default 0.25, HTDemucs 0.25).
    /// Clamped to `[0, 0.95]`.
    pub overlap: Option<f32>,
    /// Chunks per inference call. Default: 2 on DirectML, 1 on CPU (CPU gains nothing from
    /// batching). Only used when the graph has a dynamic batch dimension. RoFormer / SCNet
    /// always run one segment at a time.
    pub batch_size: Option<usize>,
    /// ONNX Runtime intra-op threads for CPU runs (default: ORT's choice, all physical cores).
    pub cpu_threads: Option<usize>,
}

/// Build a separator for an already-downloaded model.
pub fn load(info: &ModelInfo, model_path: &Path, device: Device) -> Result<Box<dyn Separator>> {
    load_with(info, model_path, device, &LoadOptions::default())
}

/// [`load`] with explicit [`LoadOptions`].
///
/// On a GPU, a separation that runs out of video memory (common on iGPUs with a few hundred MB
/// of dedicated memory) is retried on the CPU instead of failing.
pub fn load_with(
    info: &ModelInfo,
    model_path: &Path,
    device: Device,
    options: &LoadOptions,
) -> Result<Box<dyn Separator>> {
    let inner = load_on(info, model_path, device, options)?;
    if inner.active_device() == Device::Cpu {
        return Ok(inner);
    }
    Ok(Box::new(OomFallback {
        inner,
        model_path: model_path.to_path_buf(),
        options: options.clone(),
    }))
}

/// True for ONNX Runtime / DirectML errors that mean the device ran out of memory.
pub fn is_out_of_memory(err: &ModelError) -> bool {
    let ModelError::Runtime(msg) = err else {
        return false;
    };
    let m = msg.to_ascii_lowercase();
    m.contains("8007000e")
        || m.contains("not enough memory")
        || m.contains("out of memory")
        || m.contains("e_outofmemory")
        || m.contains("bad_alloc")
}

/// Wraps a GPU separator and redoes the whole separation on the CPU if the GPU runs out of memory.
struct OomFallback {
    inner: Box<dyn Separator>,
    model_path: PathBuf,
    options: LoadOptions,
}

impl Separator for OomFallback {
    fn info(&self) -> &ModelInfo {
        self.inner.info()
    }

    fn separate(
        &mut self,
        mix: &AudioBuffer,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<Vec<(StemKind, AudioBuffer)>> {
        match self.inner.separate(mix, progress, cancel) {
            Err(e) if self.inner.active_device() != Device::Cpu && is_out_of_memory(&e) => {
                tracing::warn!("GPU ran out of memory ({e}); retrying on the CPU");
                let info = self.inner.info().clone();
                self.inner = load_on(&info, &self.model_path, Device::Cpu, &self.options)?;
                self.inner.separate(mix, progress, cancel)
            }
            r => r,
        }
    }

    fn active_device(&self) -> Device {
        self.inner.active_device()
    }
}

fn load_on(
    info: &ModelInfo,
    model_path: &Path,
    device: Device,
    options: &LoadOptions,
) -> Result<Box<dyn Separator>> {
    if !model_path.is_file() {
        return Err(ModelError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("model file not found: {}", model_path.display()),
        )));
    }
    let batch_hint = match info.arch {
        // Big fixed-batch-1 graphs: one segment per call, also on DirectML.
        ModelArch::RoFormer { .. } | ModelArch::Scnet { .. } => 1,
        _ => options.batch_size.unwrap_or(match device {
            Device::Cpu => 1,
            _ => 2,
        }),
    };
    let session = runtime::build_session(model_path, device, options, batch_hint)?;
    match &info.arch {
        ModelArch::MdxNet { .. } => Ok(Box::new(mdx::MdxSeparator::new(
            info.clone(),
            Box::new(session),
            options,
        )?)),
        ModelArch::HtDemucs { .. } => Ok(Box::new(demucs::HtDemucsSeparator::new(
            info.clone(),
            Box::new(session),
            options,
        )?)),
        ModelArch::RoFormer { .. } | ModelArch::Scnet { .. } => Ok(Box::new(
            roformer::SpecSeparator::new(info.clone(), Box::new(session), options)?,
        )),
    }
}

#[cfg(test)]
mod oom_tests {
    use super::*;

    #[test]
    fn out_of_memory_errors_are_recognised() {
        let rt = |m: &str| ModelError::Runtime(m.into());
        assert!(is_out_of_memory(&rt(
            "Non-zero status code returned while running Gemm node. 8007000E Not enough memory resources"
        )));
        assert!(is_out_of_memory(&rt("std::bad_alloc")));
        assert!(!is_out_of_memory(&rt("invalid input shape")));
        assert!(!is_out_of_memory(&ModelError::Cancelled));
    }
}
