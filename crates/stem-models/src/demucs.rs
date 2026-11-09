//! HTDemucs (Demucs v4) with the STFT outside the ONNX graph.
//!
//! The graph (see `tools/export_htdemucs_onnx.py`, modelled on sevagh/demucs.onnx, MIT) is
//! HTDemucs' `forward` minus `_spec`/`_magnitude` at the start and `_mask`/`_ispec` at the end:
//!
//! * inputs: `mix [B, 2, L]` and `spec [B, 4, 2048, T]` (complex-as-channels `L.re, L.im,
//!   R.re, R.im` of `_spec(mix)`), with `L = 343980` (7.8 s at 44.1 kHz, the training segment)
//!   and `T = ceil(L / 1024) = 336`.
//! * outputs: `freq [B, S, 4, 2048, T]` (the frequency branch, already de-normalised) and
//!   `time [B, S, 2, L]` (the time branch).
//!
//! Inputs/outputs are recognised by rank, not name, so sevagh's export (names `input`,
//! `output`, ...) works too.
//!
//! Rust side, mirroring demucs (MIT):
//!
//! * `_spec`: `le = ceil(L / hop)`, reflect-pad by `(3*hop/2, 3*hop/2 + le*hop - L)`, centred
//!   reflect STFT with `n_fft = 4096`, `hop = 1024`, periodic Hann, `normalized=True`
//!   (×1/sqrt(4096)), drop the Nyquist bin, keep frames `2 .. 2 + le`.
//! * `_ispec`: add the Nyquist bin and 2 empty frames each side, normalised centred iSTFT to
//!   `hop * ceil(L / hop) + 3*hop` samples, keep `3*hop/2 .. 3*hop/2 + L`.
//! * stem = time branch + iSTFT(freq branch).
//! * `apply_model(split=True, overlap, shifts=0)`: chunks start every
//!   `int((1 - overlap) * L)` samples; each chunk is `TensorChunk.padded(L)`, i.e. centred in an
//!   `L` window that takes real neighbouring samples where they exist and zeros past the track
//!   ends; outputs are `center_trim`med back and blended with demucs' triangular weights.
//! * `separate.py`: the mix is normalised by the mean/std of its mono downmix first and the
//!   stems scaled back after.
//!
//! Not done: demucs' default `shifts=1` random time shift (it only adds a random offset, and we
//! prefer deterministic output) and bag-of-models averaging (htdemucs / htdemucs_6s are bags
//! of one model).

use std::sync::atomic::AtomicBool;

use rayon::prelude::*;
use stem_audio::AudioBuffer;
use stem_audio::stft::{Complex32, Spectrogram, Stft};

use crate::network::{Network, OwnedInput, TensorOut, run_pipelined};
use crate::{
    Device, LoadOptions, ModelArch, ModelError, ModelInfo, Progress, Result, Separator, StemKind,
    dsp,
};

pub(crate) const N_FFT: usize = 4096;
pub(crate) const HOP: usize = 1024;
/// Bins the network sees (`n_fft / 2`, Nyquist dropped).
pub(crate) const BINS: usize = N_FFT / 2;
const DEFAULT_OVERLAP: f32 = 0.25;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DemucsParams {
    pub segment: usize,
    pub overlap: f32,
    pub batch: usize,
    pub sources: usize,
}

impl DemucsParams {
    /// `T`: spectrogram frames per segment.
    pub fn frames(&self) -> usize {
        self.segment.div_ceil(HOP)
    }

    pub fn stride(&self) -> usize {
        (((1.0 - self.overlap as f64) * self.segment as f64) as usize).max(1)
    }

    /// Chunk offsets for a track of `n` samples (`range(0, length, stride)`).
    pub fn offsets(&self, n: usize) -> Vec<usize> {
        (0..n).step_by(self.stride()).collect()
    }
}

/// `TensorChunk(mix, offset, segment).padded(segment)` for one channel: the chunk centred in a
/// `segment`-long window, with real neighbouring samples where available and zeros elsewhere.
/// Returns the window and the left trim `center_trim` will remove.
pub(crate) fn padded_chunk(x: &[f32], offset: usize, segment: usize, out: &mut [f32]) -> usize {
    let chunk_len = segment.min(x.len() - offset);
    let delta = segment - chunk_len;
    let start = offset as isize - (delta / 2) as isize;
    for (i, o) in out.iter_mut().enumerate().take(segment) {
        let j = start + i as isize;
        *o = if j >= 0 && (j as usize) < x.len() {
            x[j as usize]
        } else {
            0.0
        };
    }
    delta / 2
}

/// demucs `_spec` + `_magnitude` (cac) for one channel, written into the two `[BINS, T]`
/// planes (re, im).
pub(crate) fn spec_planes(stft: &Stft, x: &[f32], frames: usize, re: &mut [f32], im: &mut [f32]) {
    let pad = HOP / 2 * 3;
    let le = x.len().div_ceil(HOP);
    debug_assert_eq!(le, frames);
    let padded = dsp::reflect_pad(x, pad, pad + le * HOP - x.len());
    let spec = stft.forward(&padded);
    debug_assert_eq!(spec.len(), le + 4);
    let norm = 1.0 / (N_FFT as f32).sqrt();
    for (t, frame) in spec[2..2 + le].iter().enumerate() {
        for f in 0..BINS {
            re[f * frames + t] = frame[f].re * norm;
            im[f * frames + t] = frame[f].im * norm;
        }
    }
}

/// demucs `_mask` (cac) + `_ispec` for one channel of one source: `re`/`im` are `[BINS, T]`.
pub(crate) fn ispec_planes(
    stft: &Stft,
    re: &[f32],
    im: &[f32],
    frames: usize,
    length: usize,
) -> Vec<f32> {
    let pad = HOP / 2 * 3;
    let n_bins = N_FFT / 2 + 1;
    let le = HOP * length.div_ceil(HOP) + 2 * pad;
    let norm = (N_FFT as f32).sqrt();
    let zero = Complex32::new(0.0, 0.0);
    let spec: Spectrogram = (0..frames + 4)
        .map(|tt| {
            let mut frame = vec![zero; n_bins];
            if (2..frames + 2).contains(&tt) {
                let t = tt - 2;
                for (f, bin) in frame.iter_mut().enumerate().take(BINS) {
                    *bin = Complex32::new(re[f * frames + t] * norm, im[f * frames + t] * norm);
                }
            }
            frame
        })
        .collect();
    let x = stft.inverse(&spec, le);
    x[pad..pad + length].to_vec()
}

/// Which graph input / output is which, found by rank.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Layout {
    /// Graph input order: `mix_first` = `(mix, spec)`, else `(spec, mix)`.
    pub mix_first: bool,
}

fn find_outputs(out: Vec<TensorOut>) -> Result<(TensorOut, TensorOut)> {
    let mut freq = None;
    let mut time = None;
    for o in out {
        match o.shape.len() {
            5 if freq.is_none() => freq = Some(o),
            4 if time.is_none() => time = Some(o),
            _ => {}
        }
    }
    match (freq, time) {
        (Some(f), Some(t)) => Ok((f, t)),
        _ => Err(ModelError::Runtime(
            "HTDemucs graph must output a rank-5 frequency branch and a rank-4 time branch".into(),
        )),
    }
}

/// Run the split over a normalised stereo signal; returns `[source][channel][sample]`.
pub(crate) fn demix(
    net: &mut dyn Network,
    stft: &Stft,
    p: &DemucsParams,
    layout: Layout,
    mix: &[Vec<f32>; 2],
    progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Vec<[Vec<f32>; 2]>> {
    let n = mix[0].len();
    let seg = p.segment;
    let frames = p.frames();
    let plane = BINS * frames;
    let weight = dsp::demucs_weight(seg);
    let offsets = p.offsets(n);
    let s_count = p.sources;

    let mut out: Vec<[Vec<f32>; 2]> = (0..s_count).map(|_| [vec![0.0; n], vec![0.0; n]]).collect();
    let mut sum_w = vec![0.0f32; n];
    let batches: Vec<&[usize]> = offsets.chunks(p.batch).collect();
    // `center_trim` of the chunk at `off`: what `padded_chunk` centred it by.
    let trim = |off: usize| (seg - seg.min(n - off)) / 2;
    let overlap = net.device() != Device::Cpu;

    let prepare = |bi: usize, inputs: &mut Vec<OwnedInput>| -> Result<()> {
        let batch = batches[bi];
        inputs.resize_with(2, OwnedInput::default);
        let (a, b) = inputs.split_at_mut(1);
        let (mix_buf, spec_buf) = if layout.mix_first {
            (&mut a[0], &mut b[0])
        } else {
            (&mut b[0], &mut a[0])
        };
        // Padded waveforms, then their spectrograms, per (item, channel).
        let wave = mix_buf.reshape(vec![p.batch, 2, seg]);
        for (idx, w) in wave.chunks_mut(seg).enumerate() {
            match batch.get(idx / 2) {
                Some(&off) => {
                    padded_chunk(&mix[idx % 2], off, seg, w);
                }
                None => w.fill(0.0),
            }
        }
        let spec = spec_buf.reshape(vec![p.batch, 4, BINS, frames]);
        spec.par_chunks_mut(2 * plane)
            .zip(wave.par_chunks(seg))
            .for_each(|(planes, w)| {
                let (re, im) = planes.split_at_mut(plane);
                spec_planes(stft, w, frames, re, im);
            });
        Ok(())
    };

    let finish = |bi: usize, outputs: Vec<TensorOut>| -> Result<()> {
        let batch = batches[bi];
        let (freq, time) = find_outputs(outputs)?;
        if freq.data.len() != p.batch * s_count * 4 * plane
            || time.data.len() != p.batch * s_count * 2 * seg
        {
            return Err(ModelError::Runtime(format!(
                "HTDemucs outputs {:?} / {:?}, expected [{}, {s_count}, 4, {BINS}, {frames}] / [{}, {s_count}, 2, {seg}]",
                freq.shape, time.shape, p.batch, p.batch
            )));
        }

        // Frequency branch back to audio, plus the time branch: per (item, source, channel).
        let used = batch.len();
        let stems: Vec<Vec<f32>> = (0..used * s_count * 2)
            .into_par_iter()
            .map(|idx| {
                let planes = &freq.data[idx * 2 * plane..(idx + 1) * 2 * plane];
                let (re, im) = planes.split_at(plane);
                let mut x = ispec_planes(stft, re, im, frames, seg);
                let t = &time.data[idx * seg..(idx + 1) * seg];
                x.iter_mut().zip(t).for_each(|(a, b)| *a += b);
                x
            })
            .collect();

        for (item, &off) in batch.iter().enumerate() {
            let chunk_len = seg.min(n - off);
            let trim = trim(off);
            let w = &weight[..chunk_len];
            for s in 0..s_count {
                for ch in 0..2 {
                    let x = &stems[(item * s_count + s) * 2 + ch][trim..trim + chunk_len];
                    let dst = &mut out[s][ch][off..off + chunk_len];
                    for ((d, &v), &wi) in dst.iter_mut().zip(x).zip(w) {
                        *d += v * wi;
                    }
                }
            }
            for (d, &wi) in sum_w[off..off + chunk_len].iter_mut().zip(w) {
                *d += wi;
            }
        }
        progress((bi + 1) as f32 / batches.len() as f32);
        Ok(())
    };
    run_pipelined(net, batches.len(), overlap, cancel, prepare, finish)?;

    for src in &mut out {
        for ch in src.iter_mut() {
            for (x, &w) in ch.iter_mut().zip(&sum_w) {
                *x = if w > 0.0 { *x / w } else { 0.0 };
            }
        }
    }
    Ok(out)
}

/// `separate.py`: mean / std (unbiased) of the mono downmix.
pub(crate) fn mono_stats(mix: &[Vec<f32>; 2]) -> (f32, f32) {
    let n = mix[0].len();
    if n == 0 {
        return (0.0, 0.0);
    }
    let mono = |i: usize| (mix[0][i] as f64 + mix[1][i] as f64) * 0.5;
    let mean = (0..n).map(mono).sum::<f64>() / n as f64;
    let var = (0..n).map(|i| (mono(i) - mean).powi(2)).sum::<f64>() / (n.max(2) - 1) as f64;
    (mean as f32, var.sqrt() as f32)
}

pub(crate) struct HtDemucsSeparator {
    info: ModelInfo,
    net: Box<dyn Network>,
    params: DemucsParams,
    layout: Layout,
    stft: Stft,
}

impl HtDemucsSeparator {
    pub fn new(info: ModelInfo, net: Box<dyn Network>, options: &LoadOptions) -> Result<Self> {
        let ModelArch::HtDemucs { segment_samples } = info.arch else {
            return Err(ModelError::Runtime(format!(
                "{} is not an HTDemucs model",
                info.id
            )));
        };
        let shapes = net.input_shapes();
        let bad = || {
            ModelError::Runtime(format!(
                "{}: expected inputs mix [B, 2, L] and spec [B, 4, 2048, T], model has {shapes:?}",
                info.id
            ))
        };
        if shapes.len() != 2 {
            return Err(bad());
        }
        let (mix_shape, spec_shape, mix_first) = match (shapes[0].len(), shapes[1].len()) {
            (3, 4) => (&shapes[0], &shapes[1], true),
            (4, 3) => (&shapes[1], &shapes[0], false),
            _ => return Err(bad()),
        };
        let mut segment = segment_samples;
        if mix_shape[2] > 0 && mix_shape[2] as usize != segment {
            tracing::warn!(
                "{}: graph segment {} != registry {segment}",
                info.id,
                mix_shape[2]
            );
            segment = mix_shape[2] as usize;
        }
        if (spec_shape[2] > 0 && spec_shape[2] as usize != BINS)
            || (spec_shape[3] > 0 && spec_shape[3] as usize != segment.div_ceil(HOP))
        {
            return Err(bad());
        }
        let batch = if mix_shape[0] > 0 {
            mix_shape[0] as usize
        } else {
            options.batch_size.unwrap_or(match net.device() {
                Device::Cpu => 1,
                _ => 2,
            })
        }
        .max(1);
        let params = DemucsParams {
            segment,
            overlap: options.overlap.unwrap_or(DEFAULT_OVERLAP).clamp(0.0, 0.95),
            batch,
            sources: info.stems.len(),
        };
        Ok(Self {
            info,
            net,
            params,
            layout: Layout { mix_first },
            stft: Stft::new(N_FFT, HOP),
        })
    }
}

impl Separator for HtDemucsSeparator {
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
        let mut x = dsp::resample_stereo(dsp::stereo(mix), rate, self.info.sample_rate, None)?;
        let (mean, std) = mono_stats(&x);
        let inv = 1.0 / (std + 1e-8);
        x.iter_mut()
            .for_each(|c| c.iter_mut().for_each(|v| *v = (*v - mean) * inv));
        let sources = demix(
            self.net.as_mut(),
            &self.stft,
            &self.params,
            self.layout,
            &x,
            &mut |f| progress(Progress::Separating(f)),
            cancel,
        )?;
        drop(x);
        let mut stems = Vec::with_capacity(sources.len());
        for (kind, mut src) in self.info.stems.iter().copied().zip(sources) {
            src.iter_mut()
                .for_each(|c| c.iter_mut().for_each(|v| *v = *v * std + mean));
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
    use crate::catalog::HTDEMUCS_SEGMENT;
    use crate::network::FnNetwork;
    use crate::network::TensorIn;

    #[test]
    fn real_segment_geometry() {
        let p = DemucsParams {
            segment: HTDEMUCS_SEGMENT,
            overlap: 0.25,
            batch: 1,
            sources: 4,
        };
        assert_eq!(p.frames(), 336);
        assert_eq!(p.stride(), 257_985); // int(0.75 * 343980)
        assert_eq!(p.offsets(600_000), vec![0, 257_985, 515_970]);
        // _spec padding: 343980 + 1536 + (1536 + 336*1024 - 343980) = 339 hops -> 340 frames
        let pad = HOP / 2 * 3;
        let padded = HTDEMUCS_SEGMENT + pad + pad + 336 * HOP - HTDEMUCS_SEGMENT;
        assert_eq!(padded / HOP + 1, 336 + 4);
    }

    #[test]
    fn padded_chunk_centres_short_chunks() {
        let x: Vec<f32> = (1..=10).map(|i| i as f32).collect();
        let mut out = vec![0.0; 6];
        // Full chunk: plain copy.
        assert_eq!(padded_chunk(&x, 2, 6, &mut out), 0);
        assert_eq!(out, vec![3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        // Last chunk of 2 samples (9, 10): delta 4, start = 8 - 2 = 6 -> real context on the left.
        assert_eq!(padded_chunk(&x, 8, 6, &mut out), 2);
        assert_eq!(out, vec![7.0, 8.0, 9.0, 10.0, 0.0, 0.0]);
        // Track shorter than a segment: zeros both sides.
        let mut out = vec![0.0; 8];
        assert_eq!(padded_chunk(&x[..4], 0, 8, &mut out), 2);
        assert_eq!(out, vec![0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 0.0, 0.0]);
    }

    #[test]
    fn mono_stats_unbiased() {
        let (m, s) = mono_stats(&[vec![1.0, 3.0], vec![1.0, 3.0]]);
        assert!((m - 2.0).abs() < 1e-6);
        assert!((s - 2f32.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn spec_ispec_roundtrip() {
        let stft = Stft::new(N_FFT, HOP);
        let len: usize = 20_000;
        let x: Vec<f32> = (0..len).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let frames = len.div_ceil(HOP);
        let mut re = vec![0.0; BINS * frames];
        let mut im = vec![0.0; BINS * frames];
        spec_planes(&stft, &x, frames, &mut re, &mut im);
        let y = ispec_planes(&stft, &re, &im, frames, len);
        // Like demucs' own `_ispec(_spec(x))`, the first/last 1.5 hops lose the two zeroed edge
        // frames, so only the interior reconstructs exactly.
        let m = 2 * HOP;
        let err = x[m..len - m]
            .iter()
            .zip(&y[m..len - m])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(err < 1e-4, "{err}");
    }

    /// Linear stand-in network: freq branch = a_s * spec, time branch = b_s * mix, so stem s
    /// must come out as (a_s + b_s) * mix after all the padding / overlap-add.
    #[test]
    fn linear_network_end_to_end() {
        let cpu = linear_network_end_to_end_on(false);
        let gpu = linear_network_end_to_end_on(true);
        assert!(cpu == gpu, "overlapped inference changes nothing");
    }

    fn linear_network_end_to_end_on(gpu: bool) -> Vec<Vec<Vec<f32>>> {
        let seg: usize = 30_000;
        let frames = seg.div_ceil(HOP);
        let a = [0.5f32, 0.0, 0.25];
        let b = [0.5f32, 1.0, 0.0];
        let s_count = a.len();
        let net = FnNetwork {
            shapes: vec![
                vec![-1, 4, BINS as i64, frames as i64],
                vec![-1, 2, seg as i64],
            ],
            f: move |inp: &[TensorIn<'_>]| {
                let (spec, mix) = (&inp[0], &inp[1]);
                let bsz = mix.shape[0];
                let mut freq = Vec::with_capacity(bsz * s_count * spec.data.len() / bsz);
                let mut time = Vec::new();
                for item in 0..bsz {
                    let sp = &spec.data[item * 4 * BINS * frames..(item + 1) * 4 * BINS * frames];
                    let mx = &mix.data[item * 2 * seg..(item + 1) * 2 * seg];
                    for s in 0..s_count {
                        freq.extend(sp.iter().map(|v| v * a[s]));
                        time.extend(mx.iter().map(|v| v * b[s]));
                    }
                }
                vec![
                    TensorOut {
                        shape: vec![bsz, s_count, 2, seg],
                        data: time,
                    },
                    TensorOut {
                        shape: vec![bsz, s_count, 4, BINS, frames],
                        data: freq,
                    },
                ]
            },
        };
        let mut info = crate::find_model("htdemucs").unwrap();
        info.stems = vec![StemKind::Drums, StemKind::Bass, StemKind::Other];
        let opts = LoadOptions {
            batch_size: Some(2),
            ..Default::default()
        };
        let net: Box<dyn Network> = if gpu {
            Box::new(crate::network::OnGpu(net))
        } else {
            Box::new(net)
        };
        let mut sep = HtDemucsSeparator::new(info, net, &opts).unwrap();
        assert_eq!(sep.params.segment, seg);
        assert!(!sep.layout.mix_first);
        // 2.5 segments, with a DC offset to exercise the mean/std normalisation.
        let n = 75_000;
        let l: Vec<f32> = (0..n)
            .map(|i| 0.1 + 0.4 * (i as f32 * 0.031).sin())
            .collect();
        let r: Vec<f32> = (0..n)
            .map(|i| -0.05 + 0.3 * (i as f32 * 0.017).cos())
            .collect();
        let mix = AudioBuffer::new(44_100, vec![l.clone(), r.clone()]);
        let offsets = sep.params.offsets(n);
        assert_eq!(offsets, vec![0, 22_500, 45_000, 67_500]);
        let mut fr = vec![];
        let stems = sep
            .separate(&mix, &mut |p| fr.push(p), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(stems.len(), 3);
        let (mean, _) = mono_stats(&[l.clone(), r.clone()]);
        for (s, (_, buf)) in stems.iter().enumerate() {
            let g = a[s] + b[s];
            for (c, m) in [&l, &r].iter().enumerate() {
                let y = &buf.channels[c];
                assert_eq!(y.len(), n);
                let errs: Vec<f32> = y
                    .iter()
                    .zip(m.iter())
                    .map(|(y, x)| (y - (g * (x - mean) + mean)).abs())
                    .collect();
                // demucs' `_ispec` zero-pads 2 frames at each segment edge, so the frequency
                // branch is slightly off for ~1.5 hops at every chunk edge (weighted down by the
                // triangle, but not to zero). Away from those spots the result must be exact.
                let near_edge = |i: usize| {
                    i < 2 * HOP
                        || i + 2 * HOP >= n
                        || offsets.iter().any(|&o| {
                            (o..o + 2 * HOP).contains(&i)
                                || (o + seg - 2 * HOP..o + seg).contains(&i)
                        })
                };
                let exact = (0..n)
                    .filter(|&i| !near_edge(i))
                    .map(|i| errs[i])
                    .fold(0.0f32, f32::max);
                assert!(exact < 1e-4, "source {s} ch {c}: {exact}");
                let err = errs[2 * HOP..n - 2 * HOP]
                    .iter()
                    .copied()
                    .fold(0.0f32, f32::max);
                assert!(err < 1e-2, "source {s} ch {c}: {err}");
            }
        }
        assert_eq!(fr.last(), Some(&Progress::Separating(1.0)));
        stems.into_iter().map(|(_, b)| b.channels).collect()
    }
}
