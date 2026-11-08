//! Waveform envelopes for drawing.
//!
//! `AudioBuffer::peaks` in stem-audio does the min/max part of this; we compute our own here
//! so the app also gets an RMS layer and works before stem-audio lands. Swap
//! [`Envelope::from_buffer`]'s peak loop for `buf.peaks(n)` if you want a single source of truth.

use stem_audio::AudioBuffer;

/// Envelope resolution: plenty for a 4K-wide lane, cheap to aggregate per frame.
pub const BUCKETS: usize = 4096;

#[derive(Debug, Clone, Default)]
pub struct Envelope {
    pub min: Vec<f32>,
    pub max: Vec<f32>,
    pub rms: Vec<f32>,
}

impl Envelope {
    pub fn from_buffer(buf: &AudioBuffer, buckets: usize) -> Self {
        let frames = buf.frames();
        let buckets = buckets.max(1);
        let mut env = Envelope {
            min: vec![0.0; buckets],
            max: vec![0.0; buckets],
            rms: vec![0.0; buckets],
        };
        if frames == 0 || buf.n_channels() == 0 {
            return env;
        }
        for b in 0..buckets {
            let start = b * frames / buckets;
            let end = ((b + 1) * frames / buckets).max(start + 1).min(frames);
            let (mut lo, mut hi, mut sq) = (0.0f32, 0.0f32, 0.0f32);
            for ch in &buf.channels {
                for &v in &ch[start..end] {
                    lo = lo.min(v);
                    hi = hi.max(v);
                    sq += v * v;
                }
            }
            env.min[b] = lo;
            env.max[b] = hi;
            env.rms[b] = (sq / ((end - start) * buf.n_channels()) as f32).sqrt();
        }
        env
    }

    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.max.len()
    }

    /// Largest absolute value, to scale lanes so quiet stems are still visible.
    pub fn peak(&self) -> f32 {
        self.max
            .iter()
            .zip(&self.min)
            .fold(0.0f32, |m, (hi, lo)| m.max(hi.abs()).max(lo.abs()))
    }

    /// `(min, max, rms)` aggregated over the fractional range `[t0, t1)` of the track.
    pub fn range(&self, t0: f32, t1: f32) -> (f32, f32, f32) {
        let n = self.len();
        if n == 0 {
            return (0.0, 0.0, 0.0);
        }
        let a = ((t0.clamp(0.0, 1.0) * n as f32) as usize).min(n - 1);
        let b = ((t1.clamp(0.0, 1.0) * n as f32).ceil() as usize).clamp(a + 1, n);
        let mut lo = 0.0f32;
        let mut hi = 0.0f32;
        let mut rms = 0.0f32;
        for i in a..b {
            lo = lo.min(self.min[i]);
            hi = hi.max(self.max[i]);
            rms = rms.max(self.rms[i]);
        }
        (lo, hi, rms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_of_square() {
        let buf = AudioBuffer::new(10, vec![vec![0.5, -0.5, 0.5, -0.5, 0.0, 0.0, 0.0, 0.0]]);
        let env = Envelope::from_buffer(&buf, 2);
        assert_eq!(env.max, vec![0.5, 0.0]);
        assert_eq!(env.min, vec![-0.5, 0.0]);
        assert!((env.rms[0] - 0.5).abs() < 1e-6);
        assert_eq!(env.range(0.0, 1.0).1, 0.5);
        assert_eq!(env.peak(), 0.5);
    }
}
