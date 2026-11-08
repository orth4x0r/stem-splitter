//! STFT / iSTFT matching `torch.stft(center=True, pad_mode="reflect", window=hann(periodic))`,
//! which is what the separation models were trained with.
//!
//! Exactly:
//! - `forward(x)` == `torch.stft(x, n_fft, hop, window=torch.hann_window(n_fft), center=True,
//!   pad_mode="reflect", normalized=False, onesided=True, return_complex=True)`, transposed to
//!   `[frame][bin]`.
//! - `inverse(spec, length)` == `torch.istft(spec, n_fft, hop, window=torch.hann_window(n_fft),
//!   center=True, normalized=False, onesided=True, length=length)`, including the division by
//!   the window-sum-square envelope.

use std::sync::Arc;

use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

pub use realfft::num_complex::Complex32;

/// Spectrogram of one channel, `[frame][bin]`, `bins = n_fft / 2 + 1`.
pub type Spectrogram = Vec<Vec<Complex32>>;

/// Envelope values below this are treated as zero (same threshold torch uses).
const ENVELOPE_EPS: f32 = 1e-11;

/// Reusable STFT with cached FFT plans and window. `Send + Sync`; `forward`/`inverse` take
/// `&self`, so one instance can be shared between threads.
pub struct Stft {
    pub n_fft: usize,
    pub hop: usize,
    window: Vec<f32>,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
}

impl std::fmt::Debug for Stft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stft")
            .field("n_fft", &self.n_fft)
            .field("hop", &self.hop)
            .finish_non_exhaustive()
    }
}

/// Periodic Hann window, as `torch.hann_window(n)` (default `periodic=True`).
pub fn hann_window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let x = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            (0.5 - 0.5 * x.cos()) as f32
        })
        .collect()
}

/// Index into `len` samples with reflection (no edge repeat, like `np.pad(mode="reflect")`),
/// repeating the reflection as often as needed.
fn reflect_index(i: isize, len: usize) -> usize {
    if len == 1 {
        return 0;
    }
    let period = 2 * (len as isize - 1);
    let m = i.rem_euclid(period);
    if m < len as isize {
        m as usize
    } else {
        (period - m) as usize
    }
}

impl Stft {
    /// # Panics
    /// If `n_fft` or `hop` is zero.
    pub fn new(n_fft: usize, hop: usize) -> Self {
        assert!(n_fft > 0 && hop > 0, "n_fft and hop must be > 0");
        let mut planner = RealFftPlanner::<f32>::new();
        Self {
            n_fft,
            hop,
            window: hann_window(n_fft),
            fwd: planner.plan_fft_forward(n_fft),
            inv: planner.plan_fft_inverse(n_fft),
        }
    }

    /// Number of frequency bins per frame, `n_fft / 2 + 1`.
    pub fn n_bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// Number of frames `forward` produces for a signal of `len` samples (`1 + len / hop`).
    pub fn n_frames(&self, len: usize) -> usize {
        let pad = self.n_fft / 2;
        (len + 2 * pad).saturating_sub(self.n_fft) / self.hop + 1
    }

    /// The analysis window (periodic Hann).
    pub fn window(&self) -> &[f32] {
        &self.window
    }

    pub fn forward(&self, signal: &[f32]) -> Spectrogram {
        let n_fft = self.n_fft;
        let pad = (n_fft / 2) as isize;
        let n_frames = self.n_frames(signal.len());
        let mut input = self.fwd.make_input_vec();
        let mut scratch = self.fwd.make_scratch_vec();
        let mut out = Vec::with_capacity(n_frames);
        for f in 0..n_frames {
            let start = (f * self.hop) as isize - pad;
            for (k, v) in input.iter_mut().enumerate() {
                let i = start + k as isize;
                let s = if signal.is_empty() {
                    0.0
                } else if i >= 0 && (i as usize) < signal.len() {
                    signal[i as usize]
                } else {
                    signal[reflect_index(i, signal.len())]
                };
                *v = s * self.window[k];
            }
            let mut spectrum = self.fwd.make_output_vec();
            self.fwd
                .process_with_scratch(&mut input, &mut spectrum, &mut scratch)
                .expect("buffer sizes come from the plan");
            out.push(spectrum);
        }
        out
    }

    /// Inverse with overlap-add and window-sum normalisation; output is trimmed to `length`.
    ///
    /// Like `torch.istft`, the imaginary parts of the DC (and, for even `n_fft`, Nyquist) bins are
    /// ignored, and samples past the end of the reconstructable signal are zero.
    pub fn inverse(&self, spec: &Spectrogram, length: usize) -> Vec<f32> {
        let n_fft = self.n_fft;
        let pad = n_fft / 2;
        let n_bins = self.n_bins();
        let n_frames = spec.len();
        if n_frames == 0 {
            return vec![0.0; length];
        }
        let total = n_fft + self.hop * (n_frames - 1);
        let mut y = vec![0.0f32; total];
        let mut env = vec![0.0f32; total];
        let mut freq = self.inv.make_input_vec();
        let mut frame = self.inv.make_output_vec();
        let mut scratch = self.inv.make_scratch_vec();
        let scale = 1.0 / n_fft as f32;
        for (f, bins) in spec.iter().enumerate() {
            let n = bins.len().min(n_bins);
            freq[..n].copy_from_slice(&bins[..n]);
            freq[n..].fill(Complex32::new(0.0, 0.0));
            freq[0].im = 0.0;
            if n_fft.is_multiple_of(2) {
                freq[n_bins - 1].im = 0.0;
            }
            self.inv
                .process_with_scratch(&mut freq, &mut frame, &mut scratch)
                .expect("buffer sizes come from the plan");
            let off = f * self.hop;
            for k in 0..n_fft {
                let w = self.window[k];
                y[off + k] += frame[k] * scale * w;
                env[off + k] += w * w;
            }
        }
        (0..length)
            .map(|i| {
                let j = i + pad;
                if j < total && env[j] > ENVELOPE_EPS {
                    y[j] / env[j]
                } else {
                    0.0
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic test signal; must match `testdata/gen_stft_ref.py`.
    fn test_signal(len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| {
                let t = i as f64;
                ((0.3 * t).sin() + 0.5 * (1.7 * t + 0.2).cos() + 0.1 * (i % 7) as f64 - 0.3) as f32
            })
            .collect()
    }

    struct RefCase {
        n_fft: usize,
        hop: usize,
        len: usize,
        frames: usize,
        bins: usize,
        /// `[frame][bin]` as (re, im)
        spec: Vec<(f32, f32)>,
        /// istft of `spec * mask` with `length = len`
        masked_istft: Vec<f32>,
    }

    fn parse_ref() -> Vec<RefCase> {
        let text = include_str!("../testdata/stft_ref.txt");
        let mut cases = Vec::new();
        let mut lines = text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty());
        while let Some(header) = lines.next() {
            let h: Vec<usize> = header
                .split_whitespace()
                .skip(1)
                .map(|v| v.parse().unwrap())
                .collect();
            let (n_fft, hop, len, frames, bins) = (h[0], h[1], h[2], h[3], h[4]);
            let nums = |l: &str| -> Vec<f32> {
                l.split_whitespace().map(|v| v.parse().unwrap()).collect()
            };
            let s = nums(lines.next().unwrap());
            let spec = s.chunks(2).map(|c| (c[0], c[1])).collect::<Vec<_>>();
            assert_eq!(spec.len(), frames * bins);
            let masked_istft = nums(lines.next().unwrap());
            assert_eq!(masked_istft.len(), len);
            cases.push(RefCase {
                n_fft,
                hop,
                len,
                frames,
                bins,
                spec,
                masked_istft,
            });
        }
        cases
    }

    /// Mask applied to the spectrum before the istft reference; must match the generator.
    fn mask(f: usize, b: usize) -> f32 {
        (0.5 + 0.5 * ((f as f64) * 0.7 + (b as f64) * 1.3).sin()) as f32
    }

    #[test]
    fn hann_matches_torch_periodic() {
        let w = hann_window(4);
        let expect = [0.0, 0.5, 1.0, 0.5];
        for (a, b) in w.iter().zip(expect) {
            assert!((a - b).abs() < 1e-7);
        }
    }

    #[test]
    fn reflect_index_matches_numpy() {
        // np.pad(np.arange(4), 5, mode="reflect") -> [1 2 3 2 1 | 0 1 2 3 | 2 1 0 1 2]
        let got: Vec<usize> = (-5..9).map(|i| reflect_index(i, 4)).collect();
        assert_eq!(got, vec![1, 2, 3, 2, 1, 0, 1, 2, 3, 2, 1, 0, 1, 2]);
    }

    #[test]
    fn forward_matches_numpy_reference() {
        let cases = parse_ref();
        assert!(!cases.is_empty());
        for c in &cases {
            let stft = Stft::new(c.n_fft, c.hop);
            let x = test_signal(c.len);
            let spec = stft.forward(&x);
            assert_eq!(spec.len(), c.frames, "frames for n_fft={}", c.n_fft);
            assert_eq!(stft.n_frames(c.len), c.frames);
            for (f, frame) in spec.iter().enumerate() {
                assert_eq!(frame.len(), c.bins);
                for (b, v) in frame.iter().enumerate() {
                    let (re, im) = c.spec[f * c.bins + b];
                    assert!(
                        (v.re - re).abs() < 1e-4 && (v.im - im).abs() < 1e-4,
                        "n_fft={} frame {f} bin {b}: got {v}, want {re}+{im}i",
                        c.n_fft
                    );
                }
            }
        }
    }

    #[test]
    fn inverse_matches_numpy_reference_for_modified_spectrum() {
        for c in &parse_ref() {
            let stft = Stft::new(c.n_fft, c.hop);
            let spec: Spectrogram = (0..c.frames)
                .map(|f| {
                    (0..c.bins)
                        .map(|b| {
                            let (re, im) = c.spec[f * c.bins + b];
                            Complex32::new(re, im) * mask(f, b)
                        })
                        .collect()
                })
                .collect();
            let y = stft.inverse(&spec, c.len);
            for (i, (a, b)) in y.iter().zip(&c.masked_istft).enumerate() {
                assert!(
                    (a - b).abs() < 1e-4,
                    "n_fft={} sample {i}: got {a}, want {b}",
                    c.n_fft
                );
            }
        }
    }

    #[test]
    fn perfect_reconstruction() {
        for &(n_fft, hop, len) in &[
            (4096usize, 1024usize, 44100usize),
            (2048, 512, 10_000),
            (16, 4, 50),
            (15, 5, 64),
        ] {
            let stft = Stft::new(n_fft, hop);
            let x: Vec<f32> = (0..len)
                .map(|i| ((i as f32) * 0.01).sin() * 0.7 + ((i * 7919 % 1000) as f32 / 5000.0))
                .collect();
            let spec = stft.forward(&x);
            assert_eq!(spec.len(), stft.n_frames(len));
            if n_fft.is_multiple_of(2) {
                assert_eq!(spec.len(), 1 + len / hop);
            }
            assert_eq!(spec[0].len(), n_fft / 2 + 1);
            let y = stft.inverse(&spec, len);
            assert_eq!(y.len(), len);
            let err = x
                .iter()
                .zip(&y)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(err < 1e-4, "n_fft={n_fft} hop={hop}: max err {err}");
        }
    }

    #[test]
    fn inverse_pads_when_length_exceeds_signal() {
        let stft = Stft::new(16, 4);
        let x = test_signal(40);
        let y = stft.inverse(&stft.forward(&x), 60);
        assert_eq!(y.len(), 60);
        assert!(y[..40].iter().zip(&x).all(|(a, b)| (a - b).abs() < 1e-4));
    }

    #[test]
    fn short_signal_does_not_panic() {
        let stft = Stft::new(64, 16);
        assert_eq!(stft.forward(&[0.5; 5]).len(), 1);
        assert_eq!(stft.forward(&[]).len(), 1);
    }

    #[test]
    fn stft_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Stft>();
    }
}
