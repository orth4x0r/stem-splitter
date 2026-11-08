//! UVR MDX-Net separator.
//!
//! Port of the inference in python-audio-separator's `mdx_separator.py` (MIT), itself taken
//! from UVR (MIT):
//!
//! * `trim = n_fft / 2`, `chunk = hop * (dim_t - 1)` (so a chunk's centred STFT has exactly
//!   `dim_t` frames), `gen = chunk - 2 * trim`.
//! * The mix is padded with `trim` zeros in front and `gen + trim - n % gen` zeros behind, then
//!   cut into chunks every `step = int((1 - overlap) * chunk)` samples; the last chunks are
//!   zero-padded to `chunk`.
//! * Per chunk: STFT (`n_fft`, `hop`, periodic Hann, centred, reflect) of each channel, keep
//!   the first `dim_f` bins, pack as `[batch, 4, dim_f, dim_t]` with channels
//!   `(L.re, L.im, R.re, R.im)`, zero the first 3 bins, run the network, zero-pad the bins
//!   back to `n_fft / 2 + 1`, iSTFT to `chunk` samples.
//! * Chunks are weighted by `numpy.hanning(actual_len)` and normalised by the summed weights
//!   (UVR's `divider`), the leading `trim` is dropped, the result cut to the input length and
//!   multiplied by `compensate`.
//!
//! The network predicts the model's primary stem (`ModelInfo::stems[0]`); the other stem is
//! `mix - primary`, computed at the input's sample rate so the two always sum to the input.
//!
//! Differences from UVR, all deliberate: no "denoise" double pass, no "match mix" pass for the
//! complement (UVR's match-mix only removes the 3 lowest bins, < 22 Hz, from the mix before
//! subtracting), and the input is limited to a 0.9 peak like audio-separator does
//! (outputs are scaled back).

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
use stem_audio::AudioBuffer;
use stem_audio::stft::{Complex32, Spectrogram, Stft};

use crate::network::{Network, TensorIn};
use crate::{
    Device, LoadOptions, ModelArch, ModelError, ModelInfo, Progress, Result, Separator, StemKind,
    dsp,
};

/// Bins zeroed before inference (`spek[:, :, :3, :] *= 0` in UVR).
const ZEROED_LOW_BINS: usize = 3;
const DEFAULT_OVERLAP: f32 = 0.25;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MdxParams {
    pub n_fft: usize,
    pub hop: usize,
    pub dim_f: usize,
    pub dim_t: usize,
    pub compensate: f32,
    pub overlap: f32,
    pub batch: usize,
}

/// Where the chunks go, for an input of `n` samples.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MdxPlan {
    pub trim: usize,
    pub chunk: usize,
    /// Padded length: `trim + n + pad`.
    pub total: usize,
    pub starts: Vec<usize>,
}

impl MdxParams {
    pub fn chunk(&self) -> usize {
        self.hop * (self.dim_t - 1)
    }

    pub fn plan(&self, n: usize) -> MdxPlan {
        let trim = self.n_fft / 2;
        let chunk = self.chunk();
        let gen_size = chunk - 2 * trim;
        let pad = gen_size + trim - n % gen_size;
        let total = trim + n + pad;
        let step = (((1.0 - self.overlap as f64) * chunk as f64) as usize).max(1);
        MdxPlan {
            trim,
            chunk,
            total,
            starts: (0..total).step_by(step).collect(),
        }
    }
}

/// Run MDX-Net over a stereo signal at the model rate; returns the primary stem (compensated).
pub(crate) fn demix(
    net: &mut dyn Network,
    stft: &Stft,
    p: &MdxParams,
    mix: &[Vec<f32>; 2],
    progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<[Vec<f32>; 2]> {
    let n = mix[0].len();
    let plan = p.plan(n);
    let MdxPlan {
        trim,
        chunk,
        total,
        ref starts,
    } = plan;
    let (dim_f, dim_t) = (p.dim_f, p.dim_t);
    let n_bins = p.n_fft / 2 + 1;
    let plane = dim_f * dim_t;

    // Padded mixture, with an extra `chunk` of zeros so every chunk slice is in bounds.
    let mixture: Vec<Vec<f32>> = mix
        .iter()
        .map(|c| {
            let mut v = vec![0.0f32; total + chunk];
            v[trim..trim + n].copy_from_slice(c);
            v
        })
        .collect();

    let mut acc = [vec![0.0f32; total], vec![0.0f32; total]];
    let mut divider = vec![0.0f32; total];
    let full_window = window(chunk, p.overlap);
    let mut input = vec![0.0f32; p.batch * 4 * plane];
    let batches: Vec<&[usize]> = starts.chunks(p.batch).collect();

    for (bi, batch) in batches.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(ModelError::Cancelled);
        }
        // STFT every (item, channel) into its two planes of the input tensor.
        input
            .par_chunks_mut(2 * plane)
            .enumerate()
            .for_each(|(idx, planes)| {
                let (item, ch) = (idx / 2, idx % 2);
                planes.fill(0.0);
                let Some(&start) = batch.get(item) else {
                    return;
                };
                let spec = stft.forward(&mixture[ch][start..start + chunk]);
                let (re, im) = planes.split_at_mut(plane);
                for (t, frame) in spec.iter().take(dim_t).enumerate() {
                    for f in ZEROED_LOW_BINS..dim_f {
                        re[f * dim_t + t] = frame[f].re;
                        im[f * dim_t + t] = frame[f].im;
                    }
                }
            });

        let out = net.run(&[TensorIn {
            shape: vec![p.batch, 4, dim_f, dim_t],
            data: &input,
        }])?;
        let out = out
            .into_iter()
            .next()
            .ok_or_else(|| ModelError::Runtime("model returned no outputs".into()))?;
        if out.data.len() != input.len() {
            return Err(ModelError::Runtime(format!(
                "MDX output shape {:?}, expected [{}, 4, {dim_f}, {dim_t}]",
                out.shape, p.batch
            )));
        }

        // iSTFT every (item, channel).
        let waves: Vec<Vec<f32>> = (0..batch.len() * 2)
            .into_par_iter()
            .map(|idx| {
                let planes = &out.data[idx * 2 * plane..(idx + 1) * 2 * plane];
                let (re, im) = planes.split_at(plane);
                let spec: Spectrogram = (0..dim_t)
                    .map(|t| {
                        let mut frame = vec![Complex32::new(0.0, 0.0); n_bins];
                        for (f, bin) in frame.iter_mut().enumerate().take(dim_f) {
                            *bin = Complex32::new(re[f * dim_t + t], im[f * dim_t + t]);
                        }
                        frame
                    })
                    .collect();
                stft.inverse(&spec, chunk)
            })
            .collect();

        for (item, &start) in batch.iter().enumerate() {
            let end = (start + chunk).min(total);
            let actual = end - start;
            let partial;
            let w: &[f32] = if actual == chunk {
                &full_window
            } else {
                partial = window(actual, p.overlap);
                &partial
            };
            for ch in 0..2 {
                let wave = &waves[item * 2 + ch];
                for ((a, &x), &wi) in acc[ch][start..end].iter_mut().zip(wave).zip(w) {
                    *a += x * wi;
                }
            }
            for (d, &wi) in divider[start..end].iter_mut().zip(w) {
                *d += wi;
            }
        }
        progress((bi + 1) as f32 / batches.len() as f32);
    }

    let comp = p.compensate;
    let finish = |a: &[f32]| -> Vec<f32> {
        a[trim..trim + n]
            .iter()
            .zip(&divider[trim..trim + n])
            .map(|(&x, &d)| if d > 0.0 { x / d * comp } else { 0.0 })
            .collect()
    };
    Ok([finish(&acc[0]), finish(&acc[1])])
}

fn window(len: usize, overlap: f32) -> Vec<f32> {
    if overlap == 0.0 {
        vec![1.0; len]
    } else {
        dsp::np_hanning(len)
    }
}

pub(crate) struct MdxSeparator {
    info: ModelInfo,
    net: Box<dyn Network>,
    params: MdxParams,
    stft: Stft,
}

impl MdxSeparator {
    pub fn new(info: ModelInfo, net: Box<dyn Network>, options: &LoadOptions) -> Result<Self> {
        let ModelArch::MdxNet {
            n_fft,
            hop,
            mut dim_f,
            mut dim_t,
            compensate,
        } = info.arch
        else {
            return Err(ModelError::Runtime(format!(
                "{} is not an MDX-Net model",
                info.id
            )));
        };
        if info.stems.len() != 2 {
            return Err(ModelError::Runtime(format!(
                "{}: MDX-Net models need exactly two stems (primary first)",
                info.id
            )));
        }
        let shapes = net.input_shapes();
        let shape = shapes.first().cloned().unwrap_or_default();
        if shapes.len() != 1 || shape.len() != 4 || (shape[1] > 0 && shape[1] != 4) {
            return Err(ModelError::Runtime(format!(
                "{}: expected one [batch, 4, dim_f, dim_t] input, model has {shapes:?}",
                info.id
            )));
        }
        // Trust the graph over the registry for the dimensions it fixes.
        if shape[2] > 0 && shape[2] as usize != dim_f {
            tracing::warn!(
                "{}: graph has dim_f {}, registry {dim_f}",
                info.id,
                shape[2]
            );
            dim_f = shape[2] as usize;
        }
        if shape[3] > 0 && shape[3] as usize != dim_t {
            tracing::warn!(
                "{}: graph has dim_t {}, registry {dim_t}",
                info.id,
                shape[3]
            );
            dim_t = shape[3] as usize;
        }
        if dim_f > n_fft / 2 + 1 || hop * (dim_t - 1) <= n_fft {
            return Err(ModelError::Runtime(format!(
                "{}: inconsistent MDX parameters (n_fft {n_fft}, hop {hop}, dim_f {dim_f}, dim_t {dim_t})",
                info.id
            )));
        }
        let batch = if shape[0] > 0 {
            shape[0] as usize
        } else {
            options.batch_size.unwrap_or(match net.device() {
                Device::Cpu => 1,
                _ => 2,
            })
        }
        .max(1);
        let params = MdxParams {
            n_fft,
            hop,
            dim_f,
            dim_t,
            compensate,
            overlap: options.overlap.unwrap_or(DEFAULT_OVERLAP).clamp(0.0, 0.95),
            batch,
        };
        Ok(Self {
            stft: Stft::new(n_fft, hop),
            info,
            net,
            params,
        })
    }
}

impl Separator for MdxSeparator {
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
        separate_two_stems(
            &self.info,
            mix,
            progress,
            cancel,
            &mut |model_mix: &[Vec<f32>; 2], progress: &mut dyn FnMut(f32), cancel| {
                demix(
                    self.net.as_mut(),
                    &self.stft,
                    &self.params,
                    model_mix,
                    progress,
                    cancel,
                )
            },
        )
    }
}

type Demix<'a> =
    dyn FnMut(&[Vec<f32>; 2], &mut dyn FnMut(f32), &AtomicBool) -> Result<[Vec<f32>; 2]> + 'a;

/// Shared by MDX: resample to the model rate, predict the primary stem, resample back and
/// derive the complement as `mix - primary`.
pub(crate) fn separate_two_stems(
    info: &ModelInfo,
    mix: &AudioBuffer,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
    demix: &mut Demix<'_>,
) -> Result<Vec<(StemKind, AudioBuffer)>> {
    let rate = mix.sample_rate;
    let n = mix.frames();
    let input = dsp::stereo(mix);
    progress(Progress::Separating(0.0));
    if n == 0 {
        return Ok(info
            .stems
            .iter()
            .map(|&k| (k, AudioBuffer::silence(rate, 2, 0)))
            .collect());
    }
    let mut model_mix = dsp::resample_stereo(input.clone(), rate, info.sample_rate, None)?;
    let gain = dsp::limit_peak(&mut model_mix, 0.9);
    let mut primary = demix(
        &model_mix,
        &mut |f| progress(Progress::Separating(f)),
        cancel,
    )?;
    drop(model_mix);
    if gain != 1.0 {
        primary
            .iter_mut()
            .for_each(|c| c.iter_mut().for_each(|x| *x *= gain));
    }
    let primary = dsp::resample_stereo(primary, info.sample_rate, rate, Some(n))?;
    let secondary: Vec<Vec<f32>> = input
        .iter()
        .zip(&primary)
        .map(|(m, p)| m.iter().zip(p).map(|(a, b)| a - b).collect())
        .collect();
    progress(Progress::Separating(1.0));
    Ok(vec![
        (info.stems[0], AudioBuffer::new(rate, primary.to_vec())),
        (info.stems[1], AudioBuffer::new(rate, secondary)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{FnNetwork, TensorOut};

    fn small() -> MdxParams {
        MdxParams {
            n_fft: 1024,
            hop: 256,
            dim_f: 400,
            dim_t: 64,
            compensate: 1.0,
            overlap: 0.25,
            batch: 2,
        }
    }

    #[test]
    fn plan_matches_uvr() {
        // Real model numbers (Kim Vocal 2): n_fft 7680, chunk 261120, gen 253440.
        let p = MdxParams {
            n_fft: 7680,
            hop: 1024,
            dim_f: 3072,
            dim_t: 256,
            compensate: 1.009,
            overlap: 0.25,
            batch: 1,
        };
        let n = 44_100 * 10;
        let plan = p.plan(n);
        assert_eq!(plan.chunk, 261_120);
        assert_eq!(plan.trim, 3840);
        let gen_size = 261_120 - 7680;
        let pad = gen_size + 3840 - n % gen_size;
        assert_eq!(plan.total, 3840 + n + pad);
        // step = int(0.75 * 261120) = 195840
        assert_eq!(plan.starts[1], 195_840);
        assert_eq!(plan.starts.len(), plan.total.div_ceil(195_840));
        // Every real sample is covered by some chunk.
        assert!(plan.starts.last().unwrap() + plan.chunk >= plan.trim + n);
    }

    fn signal(n: usize, rate: f32) -> [Vec<f32>; 2] {
        let s =
            |f: f32, a: f32, i: usize| a * (2.0 * std::f32::consts::PI * f * i as f32 / rate).sin();
        [
            (0..n)
                .map(|i| s(440.0, 0.4, i) + s(2000.0, 0.2, i))
                .collect(),
            (0..n)
                .map(|i| s(660.0, 0.3, i) + s(3000.0, 0.1, i))
                .collect(),
        ]
    }

    /// Max |out - g * mix| away from the track edges: the test tones start and stop abruptly,
    /// and that step has energy in the 3 zeroed low bins.
    fn max_interior_err(out: &[Vec<f32>; 2], mix: &[Vec<f32>; 2], g: f32, margin: usize) -> f32 {
        out.iter()
            .zip(mix)
            .flat_map(|(a, b)| {
                let n = b.len();
                a[margin..n - margin]
                    .iter()
                    .zip(&b[margin..n - margin])
                    .map(move |(x, y)| (x - g * y).abs())
            })
            .fold(0.0f32, f32::max)
    }

    /// Identity network: the pipeline must reconstruct the (band-limited) input.
    #[test]
    fn identity_network_reconstructs_input() {
        let p = small();
        let stft = Stft::new(p.n_fft, p.hop);
        let shape = vec![-1, 4, p.dim_f as i64, p.dim_t as i64];
        let mut net = FnNetwork {
            shapes: vec![shape],
            f: |inp: &[TensorIn<'_>]| {
                vec![TensorOut {
                    shape: inp[0].shape.clone(),
                    data: inp[0].data.to_vec(),
                }]
            },
        };
        let mix = signal(44_100, 44_100.0);
        let mut fractions = vec![];
        let out = demix(
            &mut net,
            &stft,
            &p,
            &mix,
            &mut |f| fractions.push(f),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(out[0].len(), mix[0].len());
        let err = max_interior_err(&out, &mix, 1.0, p.n_fft);
        assert!(err < 1e-3, "max error {err}");
        assert_eq!(*fractions.last().unwrap(), 1.0);
        assert!(fractions.windows(2).all(|w| w[0] < w[1]));
        let plan = p.plan(mix[0].len());
        assert_eq!(fractions.len(), plan.starts.len().div_ceil(p.batch));
    }

    #[test]
    fn scaled_network_and_compensate() {
        let mut p = small();
        p.compensate = 1.5;
        p.overlap = 0.5;
        p.batch = 3;
        let stft = Stft::new(p.n_fft, p.hop);
        let mut net = FnNetwork {
            shapes: vec![vec![-1, 4, p.dim_f as i64, p.dim_t as i64]],
            f: |inp: &[TensorIn<'_>]| {
                vec![TensorOut {
                    shape: inp[0].shape.clone(),
                    data: inp[0].data.iter().map(|x| x * 0.5).collect(),
                }]
            },
        };
        let mix = signal(30_000, 44_100.0);
        let out = demix(
            &mut net,
            &stft,
            &p,
            &mix,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        let err = max_interior_err(&out, &mix, 0.75, p.n_fft);
        assert!(err < 1e-3, "max error {err}");
    }

    #[test]
    fn cancel_stops_before_first_chunk() {
        let p = small();
        let stft = Stft::new(p.n_fft, p.hop);
        let mut net = FnNetwork {
            shapes: vec![vec![-1, 4, p.dim_f as i64, p.dim_t as i64]],
            f: |_: &[TensorIn<'_>]| -> Vec<TensorOut> { panic!("must not run") },
        };
        let mix = signal(10_000, 44_100.0);
        let r = demix(
            &mut net,
            &stft,
            &p,
            &mix,
            &mut |_| {},
            &AtomicBool::new(true),
        );
        assert!(matches!(r, Err(ModelError::Cancelled)));
    }

    #[test]
    fn separator_outputs_sum_to_mix() {
        let info = crate::find_model("mdx-kim-vocal-2").unwrap();
        let p = small();
        let net = FnNetwork {
            shapes: vec![vec![-1, 4, p.dim_f as i64, p.dim_t as i64]],
            f: |inp: &[TensorIn<'_>]| {
                vec![TensorOut {
                    shape: inp[0].shape.clone(),
                    data: inp[0].data.iter().map(|x| x * 0.25).collect(),
                }]
            },
        };
        // Swap in small dimensions so the test is fast; the graph shape wins over the registry.
        let mut info = info;
        info.arch = ModelArch::MdxNet {
            n_fft: p.n_fft,
            hop: p.hop,
            dim_f: 999,
            dim_t: 999,
            compensate: 1.0,
        };
        let mut sep = MdxSeparator::new(info, Box::new(net), &LoadOptions::default()).unwrap();
        assert_eq!(sep.params.dim_f, p.dim_f);
        assert_eq!(sep.params.dim_t, p.dim_t);
        assert_eq!(sep.params.batch, 1);
        let [l, r] = signal(20_000, 44_100.0);
        // Peak above 0.9 exercises the input limiter.
        let l: Vec<f32> = l.iter().map(|x| x * 2.0).collect();
        let mix = AudioBuffer::new(44_100, vec![l.clone(), r.clone()]);
        let stems = sep
            .separate(&mix, &mut |_| {}, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(stems[0].0, StemKind::Vocals);
        assert_eq!(stems[1].0, StemKind::Instrumental);
        for (c, m) in [l, r].iter().enumerate() {
            let a = &stems[0].1.channels[c];
            let b = &stems[1].1.channels[c];
            assert_eq!(a.len(), m.len());
            for i in 0..m.len() {
                assert!((a[i] + b[i] - m[i]).abs() < 1e-6);
                // The abrupt start/end of the test tone has energy in the zeroed low bins.
                if (p.n_fft..m.len() - p.n_fft).contains(&i) {
                    assert!(
                        (a[i] - 0.25 * m[i]).abs() < 2e-3,
                        "{i}: {} vs {}",
                        a[i],
                        m[i]
                    );
                }
            }
        }
    }
}
