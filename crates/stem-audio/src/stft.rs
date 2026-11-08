//! STFT / iSTFT matching `torch.stft(center=True, pad_mode="reflect", window=hann(periodic))`,
//! which is what the separation models were trained with.

pub use realfft::num_complex::Complex32;

/// Spectrogram of one channel, `[frame][bin]`, `bins = n_fft / 2 + 1`.
pub type Spectrogram = Vec<Vec<Complex32>>;

pub struct Stft {
    pub n_fft: usize,
    pub hop: usize,
    // implementation-private fields (planner, window) go here
}

impl Stft {
    pub fn new(n_fft: usize, hop: usize) -> Self {
        todo!("stem-audio")
    }
    pub fn forward(&self, signal: &[f32]) -> Spectrogram {
        todo!("stem-audio")
    }
    /// Inverse with overlap-add and window-sum normalisation; output is trimmed to `length`.
    pub fn inverse(&self, spec: &Spectrogram, length: usize) -> Vec<f32> {
        todo!("stem-audio")
    }
}
