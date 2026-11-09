//! Complex-spectrogram separators with the STFT outside the graph: Mel-Band / BS-RoFormer,
//! SCNet and MDX23C ONNX exports.
//!
//! The graph sees one fixed-length segment's STFT and returns each source's complex
//! spectrogram; the STFT, the iSTFT and the overlap-add happen here. Two I/O contracts are
//! recognised at load, by input count, rank and name:
//!
//! * **stacked** (audiojs/mel-roformer, audiojs/scnet): one input `mix_spec [1, 2C, F, T]`
//!   (channel `c` real at `2c`, imaginary at `2c + 1`), one output `stems_spec
//!   [1, 2·S·C, F, T]` or `[1, S, 2C, F, T]` (same memory order: source, channel, re/im).
//! * **split** (silverdaw/bs-roformer-rhythm-onnx): inputs `spec_real`, `spec_imag
//!   [1, C, F, T]`, outputs `out_spec_real`, `out_spec_imag [1, S, C, F, T]`. Real/imaginary are
//!   told apart by "imag" in the name, else by graph order (real first).
//!
//! `S` is read from the output size: either `ModelInfo::stems.len()` (every stem predicted) or
//! one fewer, in which case the last stem is the residual `mix - sum(predicted)` (e.g. the Kim
//! Mel-RoFormer predicts vocals only; the instrumental is the rest).
//!
//! The STFT is centred with reflect padding, `F = n_fft / 2 + 1`, except that MDX23C graphs see
//! only the lowest `dim_f` bins (`F = dim_f`; the bins above come back as zeros, as
//! `TFC_TDF_net`'s `STFT.inverse` pads them; see `mdx23c.rs`). RoFormer exports use a
//! periodic Hann window, unnormalised; SCNet uses no window (ones) with `normalized=True`
//! (×1/√n_fft in, ×√n_fft out). A segment of `L` samples is zero-padded to `(T - 1)·hop`
//! samples when that is longer (SCNet.forward's padding) and the iSTFT is cut back to `L`.
//!
//! Chunking follows `demix()` of ZFTurbo's Music-Source-Separation-Training
//! (`utils/model_utils.py`, MIT), as @audio/neural-separate's `separateComplex` translates it
//! (MIT, audiojs): segments of `L` samples every `step = (1 - overlap)·L`, each weighted by a
//! linear fade over `L / 10` samples (the first segment does not fade in, the last does not fade
//! out), the sum divided by the summed weights. An input longer than `2·(L - step)` is first
//! reflected `L - step` samples out at both ends; a segment that runs past the end is reflected
//! out when more than half of it is real input, zero-padded otherwise. Segments stop at the first
//! one that reaches the end (`demix` keeps going while a segment *starts* before the end; the
//! extra segments only re-cover the tail). SCNet additionally standardises the input by the
//! mean/std of its mono downmix and scales the sources back by the std (as the reference does,
//! the mean is not added back, so it ends up in nothing but a residual stem).
//!
//! Memory: segments are streamed one at a time (batch 1 - the exports have a fixed batch of 1,
//! and DirectML wants static shapes anyway); only the per-stem accumulators are track-sized.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rayon::prelude::*;
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use stem_audio::AudioBuffer;

use crate::network::{Network, OwnedInput, TensorOut, run_pipelined};
use crate::{
    Device, LoadOptions, ModelArch, ModelError, ModelInfo, Progress, Result, Separator, StemKind,
    demucs, dsp,
};

/// Analysis/synthesis window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Window {
    /// `torch.hann_window(n)` (periodic).
    Hann,
    /// `torch.stft(window=None)`: rectangular.
    Ones,
}

/// Everything the chunking / STFT needs, resolved from the arch, the graph and the options.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SpecParams {
    pub n_fft: usize,
    pub hop: usize,
    /// `L`: samples per segment.
    pub segment: usize,
    /// `T`: STFT frames per segment, as the graph wants them.
    pub frames: usize,
    /// Samples between segment starts.
    pub step: usize,
    /// Linear fade length at segment edges.
    pub fade: usize,
    pub window: Window,
    /// Multiplier applied to the spectrogram going in (`1/√n_fft` when normalised); its inverse
    /// is applied coming out.
    pub scale: f32,
    /// Standardise the mix by its mono mean/std first (SCNet).
    pub standardize: bool,
    /// Bins the graph sees: the lowest `dim_f` of [`SpecParams::bins`] (MDX23C), else all.
    pub dim_f: usize,
}

/// Where the segments go for a track of `n` samples.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Plan {
    /// Reflect padding added at each end of the track (0 when not padded).
    pub border: usize,
    /// Length of the (possibly padded) signal the segments cut.
    pub len: usize,
    pub starts: Vec<usize>,
}

/// SCNet.forward's padding: up to a multiple of `hop` with an odd number of hops, so the
/// centred STFT has `(L + pad) / hop + 1` frames (476 for 485100 samples at hop 1024).
pub(crate) fn scnet_frames(segment: usize, hop: usize) -> usize {
    let mut pad = hop - segment % hop;
    if ((segment + pad) / hop).is_multiple_of(2) {
        pad += hop;
    }
    (segment + pad) / hop + 1
}

impl SpecParams {
    /// Parameters for a RoFormer / SCNet arch. `graph_frames` (the graph's static `T`, if any)
    /// wins over the arch's default.
    pub fn new(
        arch: &ModelArch,
        overlap: Option<f32>,
        graph_frames: Option<usize>,
    ) -> Result<Self> {
        let mut dim_f = None;
        let (n_fft, hop, segment, default_overlap, window, scale, standardize, frames) = match *arch
        {
            ModelArch::RoFormer {
                n_fft,
                hop,
                segment_samples,
                overlap,
            } => (
                n_fft,
                hop,
                segment_samples,
                overlap,
                Window::Hann,
                1.0,
                false,
                segment_samples / hop + 1,
            ),
            ModelArch::Scnet {
                n_fft,
                hop,
                segment_samples,
                overlap,
            } => (
                n_fft,
                hop,
                segment_samples,
                overlap,
                Window::Ones,
                1.0 / (n_fft as f32).sqrt(),
                true,
                scnet_frames(segment_samples, hop),
            ),
            ModelArch::Mdx23c {
                n_fft,
                hop,
                dim_f: f,
                segment_samples,
                overlap,
            } => {
                dim_f = Some(f);
                (
                    n_fft,
                    hop,
                    segment_samples,
                    overlap,
                    Window::Hann,
                    1.0,
                    false,
                    segment_samples / hop + 1,
                )
            }
            _ => {
                return Err(ModelError::Runtime(
                    "not a RoFormer / SCNet / MDX23C architecture".into(),
                ));
            }
        };
        let dim_f = dim_f.unwrap_or(n_fft / 2 + 1);
        if dim_f == 0 || dim_f > n_fft / 2 + 1 {
            return Err(ModelError::Runtime(format!(
                "bad spectrogram parameters: dim_f {dim_f} for n_fft {n_fft}"
            )));
        }
        let frames = graph_frames.unwrap_or(frames);
        if n_fft == 0 || hop == 0 || segment < n_fft || frames < 2 {
            return Err(ModelError::Runtime(format!(
                "bad spectrogram parameters: n_fft {n_fft}, hop {hop}, segment {segment}, frames {frames}"
            )));
        }
        // The centred STFT of the padded segment must have exactly `frames` frames.
        let padded = segment.max((frames - 1) * hop);
        if padded / hop + 1 != frames {
            return Err(ModelError::Runtime(format!(
                "a {segment}-sample segment does not give the graph's {frames} frames at hop {hop}"
            )));
        }
        let overlap = overlap.unwrap_or(default_overlap).clamp(0.0, 0.95);
        let step = (((1.0 - overlap as f64) * segment as f64) as usize).clamp(1, segment);
        // Keep each fade inside the overlap so every sample has a non-zero total weight.
        let fade = (segment / 10).min(segment - step);
        Ok(Self {
            n_fft,
            hop,
            segment,
            frames,
            step,
            fade,
            window,
            scale,
            standardize,
            dim_f,
        })
    }

    /// Bins per frame of the STFT.
    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// Bins per frame the graph takes and returns (`dim_f`).
    pub fn graph_bins(&self) -> usize {
        self.dim_f
    }

    /// Samples the STFT of one segment covers (`L`, or `(T - 1)·hop` if longer).
    pub fn padded_segment(&self) -> usize {
        self.segment.max((self.frames - 1) * self.hop)
    }

    /// `demix`'s segment layout for a track of `n > 0` samples.
    pub fn plan(&self, n: usize) -> Plan {
        let border = self.segment - self.step;
        let border = if border > 0 && n > 2 * border {
            border
        } else {
            0
        };
        let len = n + 2 * border;
        let mut starts = Vec::new();
        let mut s = 0;
        loop {
            starts.push(s);
            if s + self.segment >= len {
                break;
            }
            s += self.step;
        }
        Plan {
            border,
            len,
            starts,
        }
    }

    /// Weight of sample `i` of segment `k` out of `count`.
    pub fn weight(&self, k: usize, count: usize, i: usize) -> f32 {
        let mut w = 1.0f32;
        if self.fade >= 2 {
            let d = (self.fade - 1) as f32;
            if k > 0 && i < self.fade {
                w = i as f32 / d;
            }
            if k + 1 < count && i >= self.segment - self.fade {
                w = w.min((self.segment - 1 - i) as f32 / d);
            }
        }
        w
    }
}

/// Copy segment `start..start + L` of `x` into `out` (length `L`): past the end, reflect the
/// segment's real part when it is more than half the segment, else zeros. Returns the number of
/// real samples.
pub(crate) fn cut_segment(x: &[f32], start: usize, out: &mut [f32]) -> usize {
    let seg = out.len();
    let len = seg.min(x.len().saturating_sub(start));
    let part = &x[start..start + len];
    out[..len].copy_from_slice(part);
    let reflect = len > seg / 2;
    for (i, o) in out.iter_mut().enumerate().skip(len) {
        *o = if reflect {
            part[dsp::reflect_index(i as isize, len)]
        } else {
            0.0
        };
    }
    len
}

/// Centred, reflect-padded STFT / iSTFT with a choice of window, writing straight into the
/// `[F, T]` planes the graphs use (frequency-major) so no per-frame vectors are allocated.
/// Matches `torch.stft` / `torch.istft` (`center=True`, `pad_mode="reflect"`, unnormalised;
/// normalisation is the caller's `scale`).
pub(crate) struct SpecStft {
    n_fft: usize,
    hop: usize,
    window: Vec<f32>,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
}

impl SpecStft {
    pub fn new(n_fft: usize, hop: usize, window: Window) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let window = match window {
            Window::Hann => stem_audio::stft::hann_window(n_fft),
            Window::Ones => vec![1.0; n_fft],
        };
        Self {
            n_fft,
            hop,
            window,
            fwd: planner.plan_fft_forward(n_fft),
            inv: planner.plan_fft_inverse(n_fft),
        }
    }

    /// STFT of `x` (`1 + x.len() / hop` frames, which must equal `frames`) times `scale`, into
    /// `re` / `im`, each `[bins, frames]`.
    pub fn forward_planes(
        &self,
        x: &[f32],
        frames: usize,
        scale: f32,
        re: &mut [f32],
        im: &mut [f32],
    ) {
        let n = self.n_fft;
        let bins = n / 2 + 1;
        let pad = (n / 2) as isize;
        debug_assert_eq!(x.len() / self.hop + 1, frames);
        let mut input = self.fwd.make_input_vec();
        let mut spectrum = self.fwd.make_output_vec();
        let mut scratch = self.fwd.make_scratch_vec();
        for t in 0..frames {
            let start = (t * self.hop) as isize - pad;
            for (k, v) in input.iter_mut().enumerate() {
                let i = start + k as isize;
                let s = if x.is_empty() {
                    0.0
                } else if i >= 0 && (i as usize) < x.len() {
                    x[i as usize]
                } else {
                    x[dsp::reflect_index(i, x.len())]
                };
                *v = s * self.window[k];
            }
            self.fwd
                .process_with_scratch(&mut input, &mut spectrum, &mut scratch)
                .expect("buffer sizes come from the plan");
            for (f, c) in spectrum.iter().enumerate().take(bins) {
                re[f * frames + t] = c.re * scale;
                im[f * frames + t] = c.im * scale;
            }
        }
    }

    /// iSTFT of the `[bins, frames]` planes times `scale`, window-sum-square normalised, cut to
    /// `length` samples (like `torch.istft(..., length=length)`). Planes with fewer rows than
    /// `bins` hold the lowest bins; the rest are taken as zero.
    pub fn inverse_planes(
        &self,
        re: &[f32],
        im: &[f32],
        frames: usize,
        scale: f32,
        length: usize,
    ) -> Vec<f32> {
        let n = self.n_fft;
        let bins = n / 2 + 1;
        let pad = n / 2;
        let total = n + self.hop * (frames - 1);
        let mut y = vec![0.0f32; total];
        let mut env = vec![0.0f32; total];
        let mut freq = self.inv.make_input_vec();
        let mut frame = self.inv.make_output_vec();
        let mut scratch = self.inv.make_scratch_vec();
        let s = scale / n as f32;
        let have = (re.len() / frames).min(bins);
        for t in 0..frames {
            for (f, c) in freq.iter_mut().enumerate().take(bins) {
                *c = if f < have {
                    Complex32::new(re[f * frames + t], im[f * frames + t])
                } else {
                    Complex32::new(0.0, 0.0)
                };
            }
            freq[0].im = 0.0;
            if n.is_multiple_of(2) {
                freq[bins - 1].im = 0.0;
            }
            self.inv
                .process_with_scratch(&mut freq, &mut frame, &mut scratch)
                .expect("buffer sizes come from the plan");
            let off = t * self.hop;
            for k in 0..n {
                let w = self.window[k];
                y[off + k] += frame[k] * s * w;
                env[off + k] += w * w;
            }
        }
        (0..length)
            .map(|i| {
                let j = i + pad;
                if j < total && env[j] > 1e-11 {
                    y[j] / env[j]
                } else {
                    0.0
                }
            })
            .collect()
    }
}

/// How the graph wants its spectrogram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IoLayout {
    /// One `[1, 2C, F, T]` input, one output.
    Stacked,
    /// Separate real / imaginary tensors. Indices are graph positions.
    Split {
        real_in: usize,
        real_out: usize,
        imag_out: usize,
    },
}

fn is_imag(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("imag") || n.ends_with("_im") || n.ends_with(".im")
}

/// Pick the layout from the graph's inputs / outputs. Returns the layout and the static frame
/// count, if the graph fixes one.
pub(crate) fn detect_layout(
    id: &str,
    shapes: &[Vec<i64>],
    in_names: &[String],
    out_names: &[String],
    bins: usize,
) -> Result<(IoLayout, Option<usize>)> {
    let bad = |why: &str| {
        ModelError::Runtime(format!(
            "{id}: {why}; expected mix_spec [1, 4, {bins}, T] or spec_real / spec_imag \
             [1, 2, {bins}, T], model has {in_names:?} {shapes:?}"
        ))
    };
    let check = |s: &[i64], channels: i64| -> Result<Option<usize>> {
        if s.len() != 4 {
            return Err(bad("inputs must be rank 4"));
        }
        if s[0] > 1 {
            return Err(bad("batch must be 1 or dynamic"));
        }
        if s[1] > 0 && s[1] != channels {
            return Err(bad("wrong channel count"));
        }
        if s[2] > 0 && s[2] as usize != bins {
            return Err(bad("wrong number of frequency bins"));
        }
        Ok((s[3] > 0).then_some(s[3] as usize))
    };
    match shapes.len() {
        1 => Ok((IoLayout::Stacked, check(&shapes[0], 4)?)),
        2 => {
            let f0 = check(&shapes[0], 2)?;
            let f1 = check(&shapes[1], 2)?;
            if f0.is_some() && f1.is_some() && f0 != f1 {
                return Err(bad("real and imaginary inputs differ in frames"));
            }
            let pick = |names: &[String]| match names {
                [a, b] if is_imag(a) && !is_imag(b) => 1,
                _ => 0,
            };
            let real_in = pick(in_names);
            let real_out = pick(out_names);
            Ok((
                IoLayout::Split {
                    real_in,
                    real_out,
                    imag_out: 1 - real_out,
                },
                f0.or(f1),
            ))
        }
        _ => Err(bad("wrong number of inputs")),
    }
}

pub(crate) struct SpecSeparator {
    info: ModelInfo,
    net: Box<dyn Network>,
    params: SpecParams,
    layout: IoLayout,
    stft: SpecStft,
}

impl SpecSeparator {
    pub fn new(info: ModelInfo, net: Box<dyn Network>, options: &LoadOptions) -> Result<Self> {
        let graph_bins = match info.arch {
            ModelArch::RoFormer { n_fft, .. } | ModelArch::Scnet { n_fft, .. } => n_fft / 2 + 1,
            ModelArch::Mdx23c { dim_f, .. } => dim_f,
            _ => {
                return Err(ModelError::Runtime(format!(
                    "{} is not a RoFormer / SCNet / MDX23C model",
                    info.id
                )));
            }
        };
        let (layout, graph_frames) = detect_layout(
            info.id,
            &net.input_shapes(),
            &net.input_names(),
            &net.output_names(),
            graph_bins,
        )?;
        if let Some(t) = graph_frames {
            tracing::debug!("{}: graph takes {t} frames, layout {layout:?}", info.id);
        }
        let params = SpecParams::new(&info.arch, options.overlap, graph_frames)?;
        if info.stems.is_empty() {
            return Err(ModelError::Runtime(format!("{}: no stems listed", info.id)));
        }
        Ok(Self {
            stft: SpecStft::new(params.n_fft, params.hop, params.window),
            info,
            net,
            params,
            layout,
        })
    }

    /// `demix` over a stereo signal at the model's rate; returns `[source][channel][n]` for the
    /// sources the graph predicts.
    fn demix(
        &mut self,
        x: &[Vec<f32>; 2],
        progress: &mut dyn FnMut(f32),
        cancel: &AtomicBool,
    ) -> Result<Vec<[Vec<f32>; 2]>> {
        let n = x[0].len();
        let plan = self.params.plan(n);
        let (seg_len, border) = (self.params.segment, plan.border);
        let padded: Option<[Vec<f32>; 2]> = (border > 0).then(|| {
            [
                dsp::reflect_pad(&x[0], border, border),
                dsp::reflect_pad(&x[1], border, border),
            ]
        });
        let src = padded.as_ref().unwrap_or(x);
        let count = plan.starts.len();
        let mut acc: Vec<[Vec<f32>; 2]> = Vec::new();
        let mut wsum = vec![0.0f32; plan.len];
        let mut seg = [vec![0.0f32; seg_len], vec![0.0f32; seg_len]];
        let overlap = self.net.device() != Device::Cpu;
        let io = SegmentIo {
            info: &self.info,
            params: &self.params,
            layout: self.layout,
            stft: &self.stft,
        };
        let params = &self.params;
        let prepare = |k: usize, inputs: &mut Vec<OwnedInput>| -> Result<()> {
            let start = plan.starts[k];
            cut_segment(&src[0], start, &mut seg[0]);
            cut_segment(&src[1], start, &mut seg[1]);
            io.inputs(&seg, inputs);
            Ok(())
        };
        let finish = |k: usize, outputs: Vec<TensorOut>| -> Result<()> {
            let start = plan.starts[k];
            let len = seg_len.min(src[0].len().saturating_sub(start));
            let stems = io.stems(outputs)?;
            if acc.is_empty() {
                acc = (0..stems.len())
                    .map(|_| [vec![0.0; plan.len], vec![0.0; plan.len]])
                    .collect();
            }
            let w: Vec<f32> = (0..len).map(|i| params.weight(k, count, i)).collect();
            for (a, s) in acc.iter_mut().zip(&stems) {
                for c in 0..2 {
                    let dst = &mut a[c][start..start + len];
                    for ((d, &v), &wi) in dst.iter_mut().zip(&s[c]).zip(&w) {
                        *d += v * wi;
                    }
                }
            }
            for (d, &wi) in wsum[start..start + len].iter_mut().zip(&w) {
                *d += wi;
            }
            progress((k + 1) as f32 / count as f32);
            Ok(())
        };
        run_pipelined(self.net.as_mut(), count, overlap, cancel, prepare, finish)?;
        drop(padded);
        Ok(acc
            .into_iter()
            .map(|a| {
                a.map(|ch| {
                    ch[border..border + n]
                        .iter()
                        .zip(&wsum[border..border + n])
                        .map(|(&v, &w)| if w > 0.0 { v / w } else { 0.0 })
                        .collect()
                })
            })
            .collect())
    }
}

/// The parts of a [`SpecSeparator`] the per-segment work needs, borrowed apart from the
/// network (which may be busy on the inference thread meanwhile).
struct SegmentIo<'a> {
    info: &'a ModelInfo,
    params: &'a SpecParams,
    layout: IoLayout,
    stft: &'a SpecStft,
}

impl SegmentIo<'_> {
    /// One segment's (`[channel][L]`) graph inputs.
    fn inputs(&self, seg: &[Vec<f32>; 2], inputs: &mut Vec<OwnedInput>) {
        let p = self.params;
        let (bins, frames) = (p.bins(), p.frames);
        let full = bins * frames;
        // The graph's planes: the lowest `dim_f` rows of each (frequency-major) STFT plane.
        let gbins = p.graph_bins();
        let plane = gbins * frames;
        let padded_len = p.padded_segment();
        // Per channel: real plane, imaginary plane.
        let mut re = vec![0.0f32; 2 * full];
        let mut im = vec![0.0f32; 2 * full];
        re.par_chunks_mut(full)
            .zip(im.par_chunks_mut(full))
            .zip(seg.par_iter())
            .for_each(|((r, i), x)| {
                let mut padded = x.clone();
                padded.resize(padded_len, 0.0);
                self.stft.forward_planes(&padded, frames, p.scale, r, i);
            });
        if gbins < bins {
            // Channel 1's planes move down to follow channel 0's cropped ones.
            for v in [&mut re, &mut im] {
                v.copy_within(full..full + plane, plane);
                v.truncate(2 * plane);
            }
        }
        match self.layout {
            IoLayout::Stacked => {
                inputs.resize_with(1, OwnedInput::default);
                let stacked = &mut inputs[0];
                stacked.shape = vec![1, 4, gbins, frames];
                stacked.data.clear();
                for c in 0..2 {
                    stacked
                        .data
                        .extend_from_slice(&re[c * plane..(c + 1) * plane]);
                    stacked
                        .data
                        .extend_from_slice(&im[c * plane..(c + 1) * plane]);
                }
            }
            IoLayout::Split { real_in, .. } => {
                inputs.resize_with(2, OwnedInput::default);
                let shape = vec![1, 2, gbins, frames];
                inputs[real_in] = OwnedInput {
                    shape: shape.clone(),
                    data: re,
                };
                inputs[1 - real_in] = OwnedInput { shape, data: im };
            }
        }
    }

    /// One segment's graph outputs back to audio, `[source][channel][L]`.
    fn stems(&self, outputs: Vec<TensorOut>) -> Result<Vec<[Vec<f32>; 2]>> {
        let p = self.params;
        let plane = p.graph_bins() * p.frames;
        let frames = p.frames;
        let (out_re, out_im, sources) = self.split_outputs(outputs)?;
        let stft = self.stft;
        let (scale, segment) = (1.0 / p.scale, p.segment);
        let planes: Vec<Vec<f32>> = (0..sources * 2)
            .into_par_iter()
            .map(|idx| {
                let (r, i) = (out_re.plane(idx, plane), out_im.plane(idx, plane));
                stft.inverse_planes(r, i, frames, scale, segment)
            })
            .collect();
        let mut it = planes.into_iter();
        Ok((0..sources)
            .map(|_| [it.next().unwrap(), it.next().unwrap()])
            .collect())
    }

    /// Real and imaginary views of the output, and the number of sources in it.
    fn split_outputs(&self, mut outputs: Vec<TensorOut>) -> Result<(Planes, Planes, usize)> {
        let plane = self.params.graph_bins() * self.params.frames;
        let want = self.info.stems.len();
        let sources_ok = |s: usize| s == want || s + 1 == want;
        let err = |shapes: Vec<Vec<usize>>| {
            ModelError::Runtime(format!(
                "{}: unexpected output shapes {shapes:?} for {want} stems of {} x {} bins/frames",
                self.info.id,
                self.params.graph_bins(),
                self.params.frames
            ))
        };
        let shapes = || outputs.iter().map(|o| o.shape.clone()).collect::<Vec<_>>();
        match self.layout {
            IoLayout::Stacked => {
                if outputs.is_empty() {
                    return Err(err(vec![]));
                }
                let o = outputs.swap_remove(0);
                let per = 4 * plane; // two channels, re + im
                let s = o.data.len() / per;
                if !o.data.len().is_multiple_of(per) || !sources_ok(s) {
                    return Err(err(vec![o.shape]));
                }
                let data = Arc::new(o.data);
                Ok((
                    Planes {
                        data: data.clone(),
                        stride: 2,
                        offset: 0,
                    },
                    Planes {
                        data,
                        stride: 2,
                        offset: 1,
                    },
                    s,
                ))
            }
            IoLayout::Split {
                real_out, imag_out, ..
            } => {
                if outputs.len() < 2 {
                    return Err(err(shapes()));
                }
                let per = 2 * plane;
                let (r_len, i_len) = (outputs[real_out].data.len(), outputs[imag_out].data.len());
                let s = r_len / per;
                if r_len != i_len || !r_len.is_multiple_of(per) || !sources_ok(s) {
                    return Err(err(shapes()));
                }
                let mut take = |i: usize| Arc::new(std::mem::take(&mut outputs[i].data));
                let (r, i) = (take(real_out), take(imag_out));
                Ok((
                    Planes {
                        data: r,
                        stride: 1,
                        offset: 0,
                    },
                    Planes {
                        data: i,
                        stride: 1,
                        offset: 0,
                    },
                    s,
                ))
            }
        }
    }
}

/// Every `stride`-th `[F, T]` plane of an output tensor, starting at `offset`.
struct Planes {
    data: Arc<Vec<f32>>,
    stride: usize,
    offset: usize,
}

impl Planes {
    fn plane(&self, idx: usize, plane: usize) -> &[f32] {
        let k = idx * self.stride + self.offset;
        &self.data[k * plane..(k + 1) * plane]
    }
}

impl Separator for SpecSeparator {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn active_device(&self) -> Device {
        self.net.device()
    }

    fn separate(
        &mut self,
        mix: &AudioBuffer,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<Vec<(StemKind, AudioBuffer)>> {
        let rate = mix.sample_rate;
        let n = mix.frames();
        progress(Progress::Separating(0.0));
        if n == 0 {
            return Ok(self
                .info
                .stems
                .iter()
                .map(|&k| (k, AudioBuffer::silence(rate, 2, 0)))
                .collect());
        }
        let x = dsp::resample_stereo(dsp::stereo(mix), rate, self.info.sample_rate, None)?;
        let mut sources = if self.params.standardize {
            let (mean, std) = demucs::mono_stats(&x);
            let inv = 1.0 / (std + 1e-8);
            let norm = x
                .clone()
                .map(|c| c.into_iter().map(|v| (v - mean) * inv).collect());
            let mut s = self.demix(&norm, &mut |f| progress(Progress::Separating(f)), cancel)?;
            s.iter_mut()
                .flat_map(|src| src.iter_mut())
                .for_each(|c| c.iter_mut().for_each(|v| *v *= std + 1e-8));
            s
        } else {
            self.demix(&x, &mut |f| progress(Progress::Separating(f)), cancel)?
        };
        if sources.len() + 1 == self.info.stems.len() {
            // The last stem is what the predicted ones leave.
            let mut rest = x;
            for s in &sources {
                for c in 0..2 {
                    rest[c].iter_mut().zip(&s[c]).for_each(|(r, v)| *r -= v);
                }
            }
            sources.push(rest);
        } else {
            drop(x);
        }
        let mut stems = Vec::with_capacity(sources.len());
        for (kind, src) in self.info.stems.iter().copied().zip(sources) {
            let src = dsp::resample_stereo(src, self.info.sample_rate, rate, Some(n))?;
            stems.push((kind, AudioBuffer::new(rate, src.to_vec())));
        }
        progress(Progress::Separating(1.0));
        Ok(stems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::TensorIn;
    use stem_audio::stft::Stft;

    fn roformer_arch() -> ModelArch {
        ModelArch::RoFormer {
            n_fft: 2048,
            hop: 441,
            segment_samples: 352_800,
            overlap: 0.5,
        }
    }

    #[test]
    fn real_model_geometry() {
        // Kim Mel-RoFormer / BS-RoFormer: 8 s, 801 frames, a step of 4 s, 0.8 s fades.
        let p = SpecParams::new(&roformer_arch(), None, Some(801)).unwrap();
        assert_eq!(p.frames, 801);
        assert_eq!(p.padded_segment(), 352_800);
        assert_eq!(p.step, 176_400);
        assert_eq!(p.fade, 35_280);
        assert_eq!(p.bins(), 1025);
        assert_eq!(p.scale, 1.0);
        assert_eq!(p.window, Window::Hann);
        // SCNet: 11 s padded to 486400 samples = 476 frames, a step of 2.75 s.
        let scnet = ModelArch::Scnet {
            n_fft: 4096,
            hop: 1024,
            segment_samples: 485_100,
            overlap: 0.75,
        };
        assert_eq!(scnet_frames(485_100, 1024), 476);
        let p = SpecParams::new(&scnet, None, None).unwrap();
        assert_eq!(p.frames, 476);
        assert_eq!(p.padded_segment(), 486_400);
        assert_eq!(p.step, 121_275);
        assert_eq!(p.fade, 48_510);
        assert_eq!(p.bins(), 2049);
        assert!((p.scale - 1.0 / 64.0).abs() < 1e-9);
        assert!(p.standardize);
        // A graph that disagrees with the segment length is refused.
        assert!(SpecParams::new(&roformer_arch(), None, Some(700)).is_err());
    }

    #[test]
    fn plan_matches_demix() {
        let p = SpecParams::new(&roformer_arch(), None, None).unwrap();
        let (l, step) = (p.segment, p.step);
        let border = l - step;
        // Short track: no reflection, a single segment.
        let plan = p.plan(100_000);
        assert_eq!(
            (plan.border, plan.len, plan.starts.clone()),
            (0, 100_000, vec![0])
        );
        // Exactly 2·border: still not reflected (demix needs length > 2·border).
        assert_eq!(p.plan(2 * border).border, 0);
        // A 3-minute song: reflected by L - step, segments every step until one reaches the end.
        let n = 180 * 44_100;
        let plan = p.plan(n);
        assert_eq!(plan.border, border);
        assert_eq!(plan.len, n + 2 * border);
        let last = *plan.starts.last().unwrap();
        assert!(last + l >= plan.len && last - step + l < plan.len);
        assert!(plan.starts.windows(2).all(|w| w[1] - w[0] == step));
        // With overlap 0 there is no border and no fade.
        let p0 = SpecParams::new(&roformer_arch(), Some(0.0), None).unwrap();
        assert_eq!((p0.step, p0.fade, p0.plan(n).border), (l, 0, 0));
    }

    #[test]
    fn weights_fade_like_demix() {
        let p = SpecParams::new(&roformer_arch(), None, None).unwrap();
        let (l, f) = (p.segment, p.fade);
        // First segment: no fade in, fades out.
        assert_eq!(p.weight(0, 3, 0), 1.0);
        assert_eq!(p.weight(0, 3, l - 1), 0.0);
        assert!((p.weight(0, 3, l - f) - 1.0).abs() < 1e-6);
        // Middle: torch.linspace(0, 1, fade) in, the reverse out.
        assert_eq!(p.weight(1, 3, 0), 0.0);
        assert!((p.weight(1, 3, f - 1) - 1.0).abs() < 1e-6);
        assert!((p.weight(1, 3, (f - 1) / 2) - ((f - 1) / 2) as f32 / (f - 1) as f32).abs() < 1e-6);
        assert_eq!(p.weight(1, 3, l / 2), 1.0);
        // Last: fades in, never out. A lone segment is flat.
        assert_eq!(p.weight(2, 3, l - 1), 1.0);
        assert_eq!(p.weight(0, 1, 0), 1.0);
        assert_eq!(p.weight(0, 1, l - 1), 1.0);
        // Overlap-add of the weights never leaves a hole.
        let n = 30 * 44_100;
        let plan = p.plan(n);
        let mut sum = vec![0.0f32; plan.len];
        let count = plan.starts.len();
        for (k, &s) in plan.starts.iter().enumerate() {
            let len = l.min(plan.len - s);
            for i in 0..len {
                sum[s + i] += p.weight(k, count, i);
            }
        }
        assert!(sum.iter().all(|&w| w > 0.0));
    }

    #[test]
    fn segments_past_the_end() {
        let x: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let mut out = vec![9.0; 6];
        // More than half real: reflected (torch reflect, edge not repeated).
        assert_eq!(cut_segment(&x, 6, &mut out), 4);
        assert_eq!(out, vec![6.0, 7.0, 8.0, 9.0, 8.0, 7.0]);
        // Half or less: zeros.
        assert_eq!(cut_segment(&x, 7, &mut out), 3);
        assert_eq!(out, vec![7.0, 8.0, 9.0, 0.0, 0.0, 0.0]);
    }

    fn signal(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32;
                0.3 * (0.013 * t).sin()
                    + 0.2 * (0.37 * t + 0.4).cos()
                    + 0.05 * ((i % 11) as f32 - 5.0) / 5.0
            })
            .collect()
    }

    #[test]
    fn hann_stft_matches_stem_audio() {
        let (n_fft, hop, len) = (512, 128, 4096);
        let x = signal(len);
        let ours = SpecStft::new(n_fft, hop, Window::Hann);
        let reference = Stft::new(n_fft, hop);
        let spec = reference.forward(&x);
        let (bins, frames) = (n_fft / 2 + 1, spec.len());
        let mut re = vec![0.0; bins * frames];
        let mut im = vec![0.0; bins * frames];
        ours.forward_planes(&x, frames, 0.5, &mut re, &mut im);
        for (t, fr) in spec.iter().enumerate() {
            for (f, c) in fr.iter().enumerate() {
                assert!((re[f * frames + t] - 0.5 * c.re).abs() < 1e-4);
                assert!((im[f * frames + t] - 0.5 * c.im).abs() < 1e-4);
            }
        }
        let y = ours.inverse_planes(&re, &im, frames, 2.0, len);
        let y_ref = reference.inverse(&spec, len);
        let err = y
            .iter()
            .zip(&y_ref)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(err < 1e-5, "{err}");
        let err = y
            .iter()
            .zip(&x)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(err < 1e-4, "{err}");
    }

    #[test]
    fn rectangular_stft_roundtrip() {
        let (n_fft, hop, len) = (1024, 256, 6 * 256);
        let x = signal(len);
        let s = SpecStft::new(n_fft, hop, Window::Ones);
        let frames = len / hop + 1;
        let bins = n_fft / 2 + 1;
        let mut re = vec![0.0; bins * frames];
        let mut im = vec![0.0; bins * frames];
        let scale = 1.0 / (n_fft as f32).sqrt();
        s.forward_planes(&x, frames, scale, &mut re, &mut im);
        // DC bin of frame 2 is the plain sum of its (unwindowed) samples, scaled.
        let dc: f32 = x[2 * hop - n_fft / 2..2 * hop + n_fft / 2].iter().sum();
        assert!((re[2] - dc * scale).abs() < 1e-3);
        let y = s.inverse_planes(&re, &im, frames, 1.0 / scale, len);
        let err = y
            .iter()
            .zip(&x)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(err < 1e-4, "{err}");
    }

    #[test]
    fn layouts_are_detected() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // audiojs: mix_spec -> stems_spec.
        let (l, t) = detect_layout(
            "m",
            &[vec![1, 4, 1025, 801]],
            &names(&["mix_spec"]),
            &names(&["stems_spec"]),
            1025,
        )
        .unwrap();
        assert_eq!((l, t), (IoLayout::Stacked, Some(801)));
        // silverdaw: spec_real, spec_imag -> out_spec_real, out_spec_imag.
        let s = vec![1, 2, 1025, 801];
        let (l, _) = detect_layout(
            "b",
            &[s.clone(), s.clone()],
            &names(&["spec_real", "spec_imag"]),
            &names(&["out_spec_real", "out_spec_imag"]),
            1025,
        )
        .unwrap();
        assert_eq!(
            l,
            IoLayout::Split {
                real_in: 0,
                real_out: 0,
                imag_out: 1
            }
        );
        // Swapped order is followed by name; unnamed falls back to graph order.
        let (l, _) = detect_layout(
            "b",
            &[s.clone(), s.clone()],
            &names(&["imag", "real"]),
            &names(&["o_imag", "o_real"]),
            1025,
        )
        .unwrap();
        assert_eq!(
            l,
            IoLayout::Split {
                real_in: 1,
                real_out: 1,
                imag_out: 0
            }
        );
        let (l, t) = detect_layout(
            "b",
            &[vec![-1, 2, -1, -1], vec![-1, 2, -1, -1]],
            &[],
            &[],
            1025,
        )
        .unwrap();
        assert_eq!(
            (l, t),
            (
                IoLayout::Split {
                    real_in: 0,
                    real_out: 0,
                    imag_out: 1
                },
                None
            )
        );
        // Wrong bins / channels / rank are refused.
        assert!(detect_layout("x", &[vec![1, 4, 2048, 801]], &[], &[], 1025).is_err());
        assert!(detect_layout("x", &[vec![1, 2, 1025, 801]], &[], &[], 1025).is_err());
        assert!(detect_layout("x", &[vec![1, 4, 1025]], &[], &[], 1025).is_err());
        assert!(detect_layout("x", &[vec![2, 4, 1025, 801]], &[], &[], 1025).is_err());
    }

    /// A stand-in graph with names: per-source gains on the spectrogram.
    struct GainNet {
        shapes: Vec<Vec<i64>>,
        in_names: Vec<String>,
        out_names: Vec<String>,
        gains: Vec<f32>,
        calls: usize,
        /// Report a GPU, which runs the graph overlapped on its own thread.
        gpu: bool,
    }

    impl Network for GainNet {
        fn input_shapes(&self) -> Vec<Vec<i64>> {
            self.shapes.clone()
        }
        fn input_names(&self) -> Vec<String> {
            self.in_names.clone()
        }
        fn output_names(&self) -> Vec<String> {
            self.out_names.clone()
        }
        fn run(&mut self, inputs: &[TensorIn<'_>]) -> Result<Vec<TensorOut>> {
            self.calls += 1;
            let gain = |data: &[f32]| -> Vec<f32> {
                self.gains
                    .iter()
                    .flat_map(|&g| data.iter().map(move |v| v * g))
                    .collect()
            };
            let s = self.gains.len();
            Ok(match inputs {
                [one] => {
                    let [_, ch, f, t] = one.shape[..] else {
                        panic!()
                    };
                    vec![TensorOut {
                        shape: vec![1, s * ch, f, t],
                        data: gain(one.data),
                    }]
                }
                [a, b] => {
                    // Inputs arrive in graph order; outputs are listed imag-first on purpose
                    // when the names say so.
                    let [_, ch, f, t] = a.shape[..] else { panic!() };
                    let shape = vec![1, s, ch, f, t];
                    vec![
                        TensorOut {
                            shape: shape.clone(),
                            data: gain(a.data),
                        },
                        TensorOut {
                            shape,
                            data: gain(b.data),
                        },
                    ]
                }
                _ => panic!("bad input count"),
            })
        }
        fn device(&self) -> Device {
            if self.gpu {
                Device::DirectMl(0)
            } else {
                Device::Cpu
            }
        }
    }

    fn small_info(arch: ModelArch, stems: Vec<StemKind>) -> ModelInfo {
        let mut info = crate::find_model("mel-roformer-kim").unwrap();
        info.arch = arch;
        info.stems = stems;
        info
    }

    fn stereo_mix(n: usize) -> AudioBuffer {
        let l = signal(n);
        let r: Vec<f32> = (0..n)
            .map(|i| 0.1 + 0.25 * (i as f32 * 0.021).cos())
            .collect();
        AudioBuffer::new(44_100, vec![l, r])
    }

    fn check(stems: &[(StemKind, AudioBuffer)], mix: &AudioBuffer, gains: &[f32], tol: f32) {
        for ((_, buf), &g) in stems.iter().zip(gains) {
            for c in 0..2 {
                assert_eq!(buf.channels[c].len(), mix.frames());
                let err = buf.channels[c]
                    .iter()
                    .zip(&mix.channels[c])
                    .map(|(y, x)| (y - g * x).abs())
                    .fold(0.0f32, f32::max);
                assert!(err < tol, "gain {g} ch {c}: {err}");
            }
        }
    }

    #[test]
    fn stacked_hann_end_to_end_with_residual() {
        // A scaled-down Mel-RoFormer: n_fft 512, hop 128, 8192-sample segments (65 frames).
        let arch = ModelArch::RoFormer {
            n_fft: 512,
            hop: 128,
            segment_samples: 8192,
            overlap: 0.5,
        };
        let net = GainNet {
            shapes: vec![vec![1, 4, 257, 65]],
            in_names: vec!["mix_spec".into()],
            out_names: vec!["stems_spec".into()],
            gains: vec![0.3],
            calls: 0,
            gpu: false,
        };
        let info = small_info(arch, vec![StemKind::Vocals, StemKind::Instrumental]);
        let mut sep = SpecSeparator::new(info, Box::new(net), &LoadOptions::default()).unwrap();
        let mix = stereo_mix(50_000);
        let mut seen = vec![];
        let stems = sep
            .separate(&mix, &mut |p| seen.push(p), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(stems.len(), 2);
        assert_eq!(stems[1].0, StemKind::Instrumental);
        check(&stems, &mix, &[0.3, 0.7], 1e-4);
        assert_eq!(seen.last(), Some(&Progress::Separating(1.0)));
        // Shorter than a segment: one zero-padded run.
        let short = stereo_mix(3000);
        let stems = sep
            .separate(&short, &mut |_| {}, &AtomicBool::new(false))
            .unwrap();
        check(&stems, &short, &[0.3, 0.7], 1e-4);
        // Cancel is honoured.
        let r = sep.separate(&mix, &mut |_| {}, &AtomicBool::new(true));
        assert!(matches!(r, Err(ModelError::Cancelled)));
    }

    /// Overlapped inference (GPU) gives bit-identical stems to the serial path, for both
    /// input layouts.
    #[test]
    fn overlapped_matches_serial() {
        let arch = ModelArch::RoFormer {
            n_fft: 512,
            hop: 128,
            segment_samples: 8192,
            overlap: 0.75,
        };
        let s = vec![1, 2, 257, 65];
        let mix = stereo_mix(40_000);
        for stacked in [true, false] {
            let run = |gpu: bool| {
                let net = if stacked {
                    GainNet {
                        shapes: vec![vec![1, 4, 257, 65]],
                        in_names: vec!["mix_spec".into()],
                        out_names: vec!["stems_spec".into()],
                        gains: vec![0.3, 0.7],
                        calls: 0,
                        gpu,
                    }
                } else {
                    GainNet {
                        shapes: vec![s.clone(), s.clone()],
                        in_names: vec!["spec_imag".into(), "spec_real".into()],
                        out_names: vec!["out_spec_imag".into(), "out_spec_real".into()],
                        gains: vec![0.3, 0.7],
                        calls: 0,
                        gpu,
                    }
                };
                let info = small_info(arch.clone(), vec![StemKind::Vocals, StemKind::Instrumental]);
                let mut sep =
                    SpecSeparator::new(info, Box::new(net), &LoadOptions::default()).unwrap();
                assert_eq!(sep.active_device() != Device::Cpu, gpu);
                sep.separate(&mix, &mut |_| {}, &AtomicBool::new(false))
                    .unwrap()
                    .into_iter()
                    .map(|(_, b)| b.channels)
                    .collect::<Vec<_>>()
            };
            let (serial, overlapped) = (run(false), run(true));
            assert!(serial == overlapped, "stacked: {stacked}");
        }
    }

    #[test]
    fn split_end_to_end_four_stems() {
        // A scaled-down BS-RoFormer with the silverdaw contract, inputs listed imag-first.
        let arch = ModelArch::RoFormer {
            n_fft: 512,
            hop: 128,
            segment_samples: 8192,
            overlap: 0.75,
        };
        let s = vec![1, 2, 257, 65];
        let gains = vec![1.0, 0.5, 0.25, 0.0];
        let net = GainNet {
            shapes: vec![s.clone(), s],
            in_names: vec!["spec_imag".into(), "spec_real".into()],
            out_names: vec!["out_spec_imag".into(), "out_spec_real".into()],
            gains: gains.clone(),
            calls: 0,
            gpu: false,
        };
        let info = small_info(
            arch,
            vec![
                StemKind::Drums,
                StemKind::Bass,
                StemKind::Other,
                StemKind::Vocals,
            ],
        );
        let mut sep = SpecSeparator::new(info, Box::new(net), &LoadOptions::default()).unwrap();
        assert_eq!(
            sep.layout,
            IoLayout::Split {
                real_in: 1,
                real_out: 1,
                imag_out: 0
            }
        );
        let mix = stereo_mix(40_000);
        let stems = sep
            .separate(&mix, &mut |_| {}, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(stems.len(), 4);
        check(&stems, &mix, &gains, 1e-4);
    }

    #[test]
    fn scnet_end_to_end_standardised() {
        // A scaled-down SCNet: rectangular window, normalised, standardised, padded frames.
        let arch = ModelArch::Scnet {
            n_fft: 1024,
            hop: 256,
            segment_samples: 10_000,
            overlap: 0.75,
        };
        let frames = scnet_frames(10_000, 256);
        // 10000 + 240 = 40 hops (even), so + 256 = 41 hops: 42 frames.
        assert_eq!(frames, 42);
        let gains = vec![0.1, 0.2, 0.3, 0.4];
        let net = GainNet {
            shapes: vec![vec![1, 4, 513, frames as i64]],
            in_names: vec!["mix_spec".into()],
            out_names: vec!["stems_spec".into()],
            gains: gains.clone(),
            calls: 0,
            gpu: false,
        };
        let info = small_info(
            arch,
            vec![
                StemKind::Drums,
                StemKind::Bass,
                StemKind::Other,
                StemKind::Vocals,
            ],
        );
        let mut sep = SpecSeparator::new(info, Box::new(net), &LoadOptions::default()).unwrap();
        let mix = stereo_mix(60_000);
        let stems = sep
            .separate(&mix, &mut |_| {}, &AtomicBool::new(false))
            .unwrap();
        // The mean is not added back (as in the reference), so compare against the mix less its
        // mono mean, scaled.
        let (mean, _) = demucs::mono_stats(&[mix.channels[0].clone(), mix.channels[1].clone()]);
        let centred = AudioBuffer::new(
            44_100,
            mix.channels
                .iter()
                .map(|c| c.iter().map(|v| v - mean).collect())
                .collect(),
        );
        check(&stems, &centred, &gains, 1e-4);
    }

    #[test]
    fn wrong_output_size_is_an_error() {
        let arch = ModelArch::RoFormer {
            n_fft: 512,
            hop: 128,
            segment_samples: 8192,
            overlap: 0.5,
        };
        let net = GainNet {
            shapes: vec![vec![1, 4, 257, 65]],
            in_names: vec![],
            out_names: vec![],
            gains: vec![1.0, 1.0, 1.0],
            calls: 0,
            gpu: false,
        };
        let info = small_info(arch, vec![StemKind::Vocals]);
        let mut sep = SpecSeparator::new(info, Box::new(net), &LoadOptions::default()).unwrap();
        let r = sep.separate(&stereo_mix(1000), &mut |_| {}, &AtomicBool::new(false));
        assert!(matches!(r, Err(ModelError::Runtime(_))));
    }
}
