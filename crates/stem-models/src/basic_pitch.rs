//! Audio to MIDI with Spotify's Basic Pitch (`nmp.onnx`, ICASSP 2022).
//!
//! Inference ([`BasicPitch::transcribe`]) and note creation ([`notes`]) are ports of
//! `basic_pitch/inference.py` (`run_inference`, `get_audio_input`, `window_audio_file`,
//! `unwrap_output`) and `basic_pitch/note_creation.py` (`model_output_to_notes`,
//! `output_to_notes_polyphonic`, `get_infered_onsets`, `constrain_frequency`,
//! `get_pitch_bends`, `model_frames_to_time`) from basic-pitch 0.4.0, Apache-2.0,
//! Copyright 2022 Spotify AB. `tools/basic_pitch_reference.py` runs the Python original for
//! comparison.
//!
//! The pipeline: mono 22.05 kHz audio, 3840 zeros in front (half the overlap), 43 844-sample
//! windows every 36 164 samples (30 frames of overlap), each window -> 172 frames of
//! `note` / `onset` (88 piano keys) and `contour` (264 bins, 3 per semitone) posteriors at
//! 256-sample hops. 15 frames are cut off each side of every window, the rest concatenated
//! and trimmed to the audio length. Notes then come from onset peaks followed while the
//! frame posterior stays above a threshold, plus the "melodia trick" for notes without a
//! clear onset.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use stem_audio::AudioBuffer;

use crate::download::{DownloadStatus, download_with_status};
use crate::network::{Network, TensorIn};
use crate::{Device, LoadOptions, ModelError, ModelManager, Result, runtime};

/// Model input rate.
pub const SAMPLE_RATE: u32 = 22_050;
/// Hop between posteriorgram frames, in samples.
pub const FFT_HOP: usize = 256;
/// Samples per model window (2 s minus one hop).
pub const AUDIO_N_SAMPLES: usize = 22_050 * 2 - FFT_HOP;
/// Frames per second of the posteriorgrams (`22050 // 256`).
pub const ANNOTATIONS_FPS: usize = 22_050 / FFT_HOP;
/// Output frames per window.
pub const ANNOT_N_FRAMES: usize = ANNOTATIONS_FPS * 2;
/// Frames shared by neighbouring windows.
pub const N_OVERLAPPING_FRAMES: usize = 30;
/// Piano keys (A0..C8): columns of `note` / `onset`.
pub const N_NOTES: usize = 88;
/// Pitch contour bins (3 per semitone): columns of `contour`.
pub const N_CONTOUR_BINS: usize = 264;
/// MIDI pitch of column 0 (A0).
pub const MIDI_OFFSET: u8 = 21;
const CONTOURS_BINS_PER_SEMITONE: f64 = 3.0;
const ANNOTATIONS_BASE_FREQUENCY: f64 = 27.5;
const MAX_FREQ_IDX: usize = 87;
const ENERGY_TOL: usize = 11;
const PITCH_BEND_TOLERANCE: usize = 25;

const OVERLAP_LEN: usize = N_OVERLAPPING_FRAMES * FFT_HOP;
const HOP_SIZE: usize = AUDIO_N_SAMPLES - OVERLAP_LEN;

/// A transcription model file: where to get it and under which license.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriberInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// File name under `<root>/models/`.
    pub file_name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size_bytes: u64,
    pub license: &'static str,
}

/// Basic Pitch `nmp.onnx` as shipped in the basic-pitch repo at tag v0.4.0 (also inside the
/// `basic-pitch` 0.4.0 wheel on PyPI, byte-identical). The weights live in the Apache-2.0
/// repository next to the code, with no separate model license.
pub const BASIC_PITCH: TranscriberInfo = TranscriberInfo {
    id: "basic-pitch-icassp2022",
    name: "Basic Pitch",
    file_name: "basic-pitch-nmp.onnx",
    url: "https://raw.githubusercontent.com/spotify/basic-pitch/v0.4.0/basic_pitch/saved_models/icassp_2022/nmp.onnx",
    sha256: "2c3c1d144bfa61ad236e92e169c13535c880469a12a047d4e73451f2c059a0ec",
    size_bytes: 230_444,
    license: "Apache-2.0 (Spotify AB, spotify/basic-pitch)",
};

impl ModelManager {
    /// `<root>/models/<info.file_name>`.
    pub fn transcriber_path(&self, info: &TranscriberInfo) -> PathBuf {
        self.models_dir().join(info.file_name)
    }

    /// Download `info` if it isn't there yet (sha256-verified, resumable, cancellable).
    pub fn download_transcriber(
        &self,
        info: &TranscriberInfo,
        progress: &mut dyn FnMut(&DownloadStatus),
        cancel: &AtomicBool,
    ) -> Result<PathBuf> {
        let dest = self.transcriber_path(info);
        if !dest.is_file() {
            download_with_status(info.url, &dest, Some(info.sha256), progress, cancel)?;
        }
        Ok(dest)
    }
}

// ---- posteriorgrams --------------------------------------------------------------------------

/// The model's three outputs for a whole track, row-major `[frame][bin]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Posteriorgrams {
    pub n_frames: usize,
    /// `n_frames x 88`: probability that a note sounds.
    pub note: Vec<f32>,
    /// `n_frames x 88`: probability that a note starts.
    pub onset: Vec<f32>,
    /// `n_frames x 264`: pitch salience, 3 bins per semitone from A0.
    pub contour: Vec<f32>,
}

impl Posteriorgrams {
    /// Length of the analysed audio, from the frame count.
    pub fn duration_secs(&self) -> f64 {
        self.n_frames as f64 * FFT_HOP as f64 / SAMPLE_RATE as f64
    }
}

/// Time in seconds of each output frame (`model_frames_to_time`, magic number included).
pub fn frame_times(n_frames: usize) -> Vec<f64> {
    let window_offset = (FFT_HOP as f64 / SAMPLE_RATE as f64)
        * (ANNOT_N_FRAMES as f64 - (AUDIO_N_SAMPLES as f64 / FFT_HOP as f64))
        + 0.0018;
    (0..n_frames)
        .map(|i| {
            let t = i as f64 * FFT_HOP as f64 / SAMPLE_RATE as f64;
            let window = (i / ANNOT_N_FRAMES) as f64;
            t - window_offset * window
        })
        .collect()
}

/// Mono (channel mean, as librosa's `to_mono`) at 22.05 kHz.
pub fn model_input(audio: &AudioBuffer) -> Result<Vec<f32>> {
    let n = audio.n_channels().max(1) as f32;
    let mono: Vec<f32> = (0..audio.frames())
        .map(|i| audio.channels.iter().map(|c| c[i]).sum::<f32>() / n)
        .collect();
    let buf = AudioBuffer::new(audio.sample_rate, vec![mono]);
    let buf = stem_audio::resample(&buf, SAMPLE_RATE)?;
    Ok(buf.channels.into_iter().next().unwrap_or_default())
}

/// A loaded Basic Pitch network.
pub struct BasicPitch {
    net: Box<dyn Network>,
}

impl BasicPitch {
    /// Load `nmp.onnx`. The model is tiny, so [`Device::Cpu`] is the sensible default;
    /// DirectML works too and falls back to the CPU on failure.
    pub fn load(model_path: &Path, device: Device) -> Result<Self> {
        if !model_path.is_file() {
            return Err(ModelError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("model file not found: {}", model_path.display()),
            )));
        }
        let session = runtime::build_session(model_path, device, &LoadOptions::default(), 1)?;
        Ok(Self::with_network(Box::new(session)))
    }

    pub(crate) fn with_network(net: Box<dyn Network>) -> Self {
        Self { net }
    }

    /// Where the network runs.
    pub fn active_device(&self) -> Device {
        self.net.device()
    }

    /// Run the model over a whole buffer (any rate / channel count). `progress` gets the
    /// fraction of windows done.
    pub fn transcribe(
        &mut self,
        audio: &AudioBuffer,
        progress: &mut dyn FnMut(f32),
        cancel: &AtomicBool,
    ) -> Result<Posteriorgrams> {
        let mono = model_input(audio)?;
        self.transcribe_22k(&mono, progress, cancel)
    }

    /// [`BasicPitch::transcribe`] for mono audio already at 22.05 kHz.
    pub fn transcribe_22k(
        &mut self,
        mono: &[f32],
        progress: &mut dyn FnMut(f32),
        cancel: &AtomicBool,
    ) -> Result<Posteriorgrams> {
        let original_length = mono.len();
        let mut padded = vec![0.0f32; OVERLAP_LEN / 2];
        padded.extend_from_slice(mono);
        let starts: Vec<usize> = (0..padded.len()).step_by(HOP_SIZE).collect();
        let order = output_order(&self.net.output_names());
        let n_olap = N_OVERLAPPING_FRAMES / 2;
        let keep = ANNOT_N_FRAMES - 2 * n_olap;
        let n_out = (original_length as f64 * (ANNOTATIONS_FPS as f64 / SAMPLE_RATE as f64)).floor()
            as usize;
        let mut out = Posteriorgrams {
            n_frames: 0,
            note: Vec::with_capacity(n_out * N_NOTES),
            onset: Vec::with_capacity(n_out * N_NOTES),
            contour: Vec::with_capacity(n_out * N_CONTOUR_BINS),
        };
        let mut window = vec![0.0f32; AUDIO_N_SAMPLES];
        for (w, &i) in starts.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(ModelError::Cancelled);
            }
            let end = (i + AUDIO_N_SAMPLES).min(padded.len());
            window.fill(0.0);
            window[..end - i].copy_from_slice(&padded[i..end]);
            let outputs = self.net.run(&[TensorIn {
                shape: vec![1, AUDIO_N_SAMPLES, 1],
                data: &window,
            }])?;
            if outputs.len() != 3 {
                return Err(ModelError::Runtime(format!(
                    "Basic Pitch: expected 3 outputs, got {}",
                    outputs.len()
                )));
            }
            let [note, onset, contour] = order.map(|k| &outputs[k]);
            for (t, width, dst) in [
                (note, N_NOTES, &mut out.note),
                (onset, N_NOTES, &mut out.onset),
                (contour, N_CONTOUR_BINS, &mut out.contour),
            ] {
                if t.data.len() != ANNOT_N_FRAMES * width {
                    return Err(ModelError::Runtime(format!(
                        "Basic Pitch: unexpected output shape {:?}",
                        t.shape
                    )));
                }
                dst.extend_from_slice(&t.data[n_olap * width..(n_olap + keep) * width]);
            }
            progress((w + 1) as f32 / starts.len() as f32);
        }
        let frames = (out.note.len() / N_NOTES).min(n_out);
        out.note.truncate(frames * N_NOTES);
        out.onset.truncate(frames * N_NOTES);
        out.contour.truncate(frames * N_CONTOUR_BINS);
        out.n_frames = frames;
        Ok(out)
    }
}

/// Indices of (note, onset, contour) among the graph outputs. nmp.onnx names them
/// `StatefulPartitionedCall:1`, `:2` and `:0`; without names, assume that order.
fn output_order(names: &[String]) -> [usize; 3] {
    let find = |suffix: &str| names.iter().position(|n| n.ends_with(suffix));
    match (find(":1"), find(":2"), find(":0")) {
        (Some(a), Some(b), Some(c)) => [a, b, c],
        _ => [0, 1, 2],
    }
}

// ---- note creation -------------------------------------------------------------------------

/// Knobs for [`notes`]; the defaults are basic-pitch's `predict()` defaults.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NoteOptions {
    /// Minimum onset posterior for a note to start (higher = fewer notes).
    pub onset_threshold: f32,
    /// Minimum frame posterior for a note to keep sounding (higher = shorter notes).
    pub frame_threshold: f32,
    /// Notes shorter than this are dropped.
    pub min_note_ms: f32,
    /// Lowest MIDI pitch kept (inclusive).
    pub min_pitch: Option<u8>,
    /// Highest MIDI pitch kept (inclusive).
    pub max_pitch: Option<u8>,
    /// Add onsets where the frame posterior jumps up.
    pub infer_onsets: bool,
    /// Also pick up notes without a detected onset, loudest first.
    pub melodia_trick: bool,
    /// Estimate per-frame pitch bends (1/3 semitone steps) from the contour output.
    pub pitch_bends: bool,
}

impl Default for NoteOptions {
    fn default() -> Self {
        Self {
            onset_threshold: 0.5,
            frame_threshold: 0.3,
            min_note_ms: 127.70,
            min_pitch: None,
            max_pitch: None,
            infer_onsets: true,
            melodia_trick: true,
            pitch_bends: true,
        }
    }
}

impl NoteOptions {
    /// Minimum note length in frames (`round(ms / 1000 * 22050 / 256)`, half to even).
    pub fn min_note_frames(&self) -> usize {
        (self.min_note_ms.max(0.0) as f64 / 1000.0 * (SAMPLE_RATE as f64 / FFT_HOP as f64))
            .round_ties_even() as usize
    }
}

/// One detected note.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// Seconds from the start of the analysed audio.
    pub start: f64,
    pub end: f64,
    /// MIDI pitch (21..=108).
    pub pitch: u8,
    /// Mean frame posterior over the note, `0..=1`.
    pub amplitude: f32,
    /// One bend per frame, in 1/3 semitones, when requested.
    pub bends: Option<Vec<i32>>,
}

impl Note {
    /// MIDI velocity: `round(127 * amplitude)` (basic-pitch's mapping), at least 1.
    pub fn velocity(&self) -> u8 {
        ((127.0 * self.amplitude as f64).round_ties_even() as i64).clamp(1, 127) as u8
    }
}

/// A note in frames: `(start, end, pitch, amplitude)`.
pub type FrameNote = (usize, usize, u8, f32);

/// `model_output_to_notes`: posteriorgrams to notes in seconds, in creation order (onset
/// notes from the end of the track backwards, then melodia notes).
pub fn notes(post: &Posteriorgrams, opts: &NoteOptions) -> Vec<Note> {
    let frame_notes = notes_in_frames(post, opts);
    let times = frame_times(post.n_frames);
    frame_notes
        .into_iter()
        .map(|(s, e, pitch, amplitude)| Note {
            start: times[s],
            end: times[e],
            pitch,
            amplitude,
            bends: opts
                .pitch_bends
                .then(|| pitch_bends(&post.contour, post.n_frames, s, e, pitch)),
        })
        .collect()
}

/// `output_to_notes_polyphonic` on `post`.
pub fn notes_in_frames(post: &Posteriorgrams, opts: &NoteOptions) -> Vec<FrameNote> {
    let n = post.n_frames;
    if n == 0 {
        return Vec::new();
    }
    let mut frames = post.note[..n * N_NOTES].to_vec();
    let mut onsets = post.onset[..n * N_NOTES].to_vec();
    constrain_frequency(&mut onsets, &mut frames, opts.max_pitch, opts.min_pitch);
    let onsets: Vec<f64> = if opts.infer_onsets {
        inferred_onsets(&onsets, &frames, n)
    } else {
        onsets.iter().map(|&v| v as f64).collect()
    };
    // Python loops forever on a non-positive frame threshold; a tiny floor keeps it finite.
    let frame_thresh = (opts.frame_threshold as f64).max(1e-4);
    let onset_thresh = opts.onset_threshold as f64;
    let min_len = opts.min_note_frames();
    let at = |t: usize, f: usize| t * N_NOTES + f;

    // scipy.signal.argrelmax(onsets, axis=0): strict local maxima in time, edges excluded.
    let mut onset_idx = Vec::new();
    for t in 0..n {
        for f in 0..N_NOTES {
            let v = onsets[at(t, f)];
            let peak = t > 0 && t + 1 < n && v > onsets[at(t - 1, f)] && v > onsets[at(t + 1, f)];
            let value = if peak { v } else { 0.0 };
            if value >= onset_thresh {
                onset_idx.push((t, f));
            }
        }
    }

    let mut remaining: Vec<f64> = frames.iter().map(|&v| v as f64).collect();
    let mean = |a: usize, b: usize, f: usize| -> f32 {
        if b <= a {
            return f32::NAN;
        }
        let s: f64 = (a..b).map(|t| frames[at(t, f)] as f64).sum();
        (s / (b - a) as f64) as f32
    };
    let mut events = Vec::new();
    for &(start, f) in onset_idx.iter().rev() {
        if start >= n - 1 {
            continue;
        }
        let mut i = start + 1;
        let mut k = 0;
        while i < n - 1 && k < ENERGY_TOL {
            if remaining[at(i, f)] < frame_thresh {
                k += 1;
            } else {
                k = 0;
            }
            i += 1;
        }
        i -= k;
        if i - start <= min_len {
            continue;
        }
        for t in start..i {
            remaining[at(t, f)] = 0.0;
            if f < MAX_FREQ_IDX {
                remaining[at(t, f + 1)] = 0.0;
            }
            if f > 0 {
                remaining[at(t, f - 1)] = 0.0;
            }
        }
        events.push((start, i, f as u8 + MIDI_OFFSET, mean(start, i, f)));
    }

    if opts.melodia_trick {
        // `while max(remaining) > thresh: argmax ...`. Cells only ever drop to zero, so
        // walking the cells above the threshold sorted by (value desc, index asc) and
        // skipping zeroed ones visits exactly the cells numpy's argmax would.
        let mut cand: Vec<usize> = (0..remaining.len())
            .filter(|&j| remaining[j] > frame_thresh)
            .collect();
        cand.sort_by(|&a, &b| remaining[b].total_cmp(&remaining[a]).then(a.cmp(&b)));
        let original: Vec<f64> = cand.iter().map(|&j| remaining[j]).collect();
        for (&j, &v) in cand.iter().zip(&original) {
            if remaining[j] != v {
                continue;
            }
            let (i_mid, f) = (j / N_NOTES, j % N_NOTES);
            remaining[j] = 0.0;
            let clear = |remaining: &mut Vec<f64>, i: usize| {
                remaining[at(i, f)] = 0.0;
                if f < MAX_FREQ_IDX {
                    remaining[at(i, f + 1)] = 0.0;
                }
                if f > 0 {
                    remaining[at(i, f - 1)] = 0.0;
                }
            };
            // Forward.
            let mut i = i_mid as isize + 1;
            let mut k = 0isize;
            while i < n as isize - 1 && k < ENERGY_TOL as isize {
                if remaining[at(i as usize, f)] < frame_thresh {
                    k += 1;
                } else {
                    k = 0;
                }
                clear(&mut remaining, i as usize);
                i += 1;
            }
            let i_end = i - 1 - k;
            // Backward.
            let mut i = i_mid as isize - 1;
            let mut k = 0isize;
            while i > 0 && k < ENERGY_TOL as isize {
                if remaining[at(i as usize, f)] < frame_thresh {
                    k += 1;
                } else {
                    k = 0;
                }
                clear(&mut remaining, i as usize);
                i -= 1;
            }
            let i_start = i + 1 + k;
            if i_end - i_start <= min_len as isize {
                continue;
            }
            let (s, e) = (i_start as usize, i_end as usize);
            events.push((s, e, f as u8 + MIDI_OFFSET, mean(s, e, f)));
        }
    }
    events
}

/// `constrain_frequency` with pitches instead of Hz: `max_freq = midi_to_hz(max_pitch + 1)`,
/// `min_freq = midi_to_hz(min_pitch)`.
fn constrain_frequency(
    onsets: &mut [f32],
    frames: &mut [f32],
    max_pitch: Option<u8>,
    min_pitch: Option<u8>,
) {
    let zero = |m: &mut [f32], lo: usize, hi: usize| {
        for row in m.chunks_mut(N_NOTES) {
            row[lo..hi].fill(0.0);
        }
    };
    if let Some(p) = max_pitch {
        let idx = (p as usize + 1)
            .saturating_sub(MIDI_OFFSET as usize)
            .min(N_NOTES);
        zero(onsets, idx, N_NOTES);
        zero(frames, idx, N_NOTES);
    }
    if let Some(p) = min_pitch {
        let idx = (p as usize)
            .saturating_sub(MIDI_OFFSET as usize)
            .min(N_NOTES);
        zero(onsets, 0, idx);
        zero(frames, 0, idx);
    }
}

/// `get_infered_onsets` (n_diff = 2), in f64 as numpy promotes it.
fn inferred_onsets(onsets: &[f32], frames: &[f32], n: usize) -> Vec<f64> {
    const N_DIFF: usize = 2;
    let at = |t: usize, f: usize| t * N_NOTES + f;
    let mut diff = vec![0.0f64; n * N_NOTES];
    for t in 0..n {
        for f in 0..N_NOTES {
            let cur = frames[at(t, f)] as f64;
            let mut m = f64::INFINITY;
            for d in 1..=N_DIFF {
                let prev = if t >= d {
                    frames[at(t - d, f)] as f64
                } else {
                    0.0
                };
                m = m.min(cur - prev);
            }
            diff[at(t, f)] = if t < N_DIFF { 0.0 } else { m.max(0.0) };
        }
    }
    let max_onset = onsets.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
    let max_diff = diff.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    onsets
        .iter()
        .zip(&diff)
        .map(|(&o, &d)| {
            // numpy gives NaN for 0/0 here; keep the predicted onsets instead.
            let d = if max_diff > 0.0 {
                max_onset * d / max_diff
            } else {
                0.0
            };
            (o as f64).max(d)
        })
        .collect()
}

/// `get_pitch_bends` for one note: per frame, the contour bin (within +-25 bins, Gaussian
/// weighted, std 5) with the most energy, relative to the note's own bin.
fn pitch_bends(contour: &[f32], n_frames: usize, start: usize, end: usize, pitch: u8) -> Vec<i32> {
    let tol = PITCH_BEND_TOLERANCE as isize;
    let window_length = 2 * tol + 1;
    let gaussian: Vec<f64> = (0..window_length)
        .map(|k| {
            let x = (k - tol) as f64 / 5.0;
            (-0.5 * x * x).exp()
        })
        .collect();
    let hz = 440.0 * 2f64.powf((pitch as f64 - 69.0) / 12.0);
    let bin = 12.0 * CONTOURS_BINS_PER_SEMITONE * (hz / ANNOTATIONS_BASE_FREQUENCY).log2();
    let freq_idx = bin.round_ties_even() as isize;
    let n_bins = N_CONTOUR_BINS as isize;
    let f0 = (freq_idx - tol).max(0);
    let f1 = n_bins.min(freq_idx + tol + 1);
    let g0 = (tol - freq_idx).max(0);
    let g1 = window_length - (freq_idx - (n_bins - tol - 1)).max(0);
    let shift = tol - (tol - freq_idx).max(0);
    let width = (f1 - f0).min(g1 - g0).max(0) as usize;
    (start..end.min(n_frames))
        .map(|t| {
            let row = &contour[t * N_CONTOUR_BINS..(t + 1) * N_CONTOUR_BINS];
            let mut best = 0usize;
            let mut best_v = f64::NEG_INFINITY;
            for j in 0..width {
                let v = row[f0 as usize + j] as f64 * gaussian[g0 as usize + j];
                if v > best_v {
                    best_v = v;
                    best = j;
                }
            }
            best as i32 - shift as i32
        })
        .collect()
}

#[cfg(test)]
mod tests;
