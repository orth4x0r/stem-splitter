//! Small DSP helpers shared by the separators.

use stem_audio::AudioBuffer;

use crate::Result;

/// Stereo copy of `buf`: mono is duplicated, more than two channels are averaged into both.
pub(crate) fn stereo(buf: &AudioBuffer) -> [Vec<f32>; 2] {
    match buf.channels.len() {
        0 => [Vec::new(), Vec::new()],
        1 => [buf.channels[0].clone(), buf.channels[0].clone()],
        2 => [buf.channels[0].clone(), buf.channels[1].clone()],
        n => {
            let frames = buf.frames();
            let mut m = vec![0.0f32; frames];
            for ch in &buf.channels {
                for (a, &b) in m.iter_mut().zip(ch) {
                    *a += b;
                }
            }
            let k = 1.0 / n as f32;
            m.iter_mut().for_each(|x| *x *= k);
            [m.clone(), m]
        }
    }
}

/// Resample stereo `channels` from `from` to `to` (no-op when equal), keeping `len` frames
/// when given (the resampler may be a few frames off).
pub(crate) fn resample_stereo(
    channels: [Vec<f32>; 2],
    from: u32,
    to: u32,
    len: Option<usize>,
) -> Result<[Vec<f32>; 2]> {
    let [l, r] = if from == to {
        channels
    } else {
        let out = stem_audio::resample(&AudioBuffer::new(from, channels.to_vec()), to)?;
        let mut it = out.channels.into_iter();
        [it.next().unwrap_or_default(), it.next().unwrap_or_default()]
    };
    Ok(match len {
        Some(n) => [fit(l, n), fit(r, n)],
        None => [l, r],
    })
}

/// Truncate or zero-pad to `n`.
pub(crate) fn fit(mut v: Vec<f32>, n: usize) -> Vec<f32> {
    v.resize(n, 0.0);
    v
}

/// `numpy.hanning(n)`: symmetric, zero at both ends.
pub(crate) fn np_hanning(n: usize) -> Vec<f32> {
    match n {
        0 => Vec::new(),
        1 => vec![1.0],
        _ => (0..n)
            .map(|i| {
                let x = 2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64;
                (0.5 - 0.5 * x.cos()) as f32
            })
            .collect(),
    }
}

/// Demucs' transition weights for `apply_model(split=True)`: a triangle
/// `[1, 2, .., L/2, L - L/2, .., 1] / max` (transition_power = 1).
pub(crate) fn demucs_weight(len: usize) -> Vec<f32> {
    let half = len / 2;
    let mut w: Vec<f32> = (1..=half)
        .map(|i| i as f32)
        .chain((1..=len - half).rev().map(|i| i as f32))
        .collect();
    let max = w.iter().copied().fold(0.0f32, f32::max).max(1.0);
    w.iter_mut().for_each(|x| *x /= max);
    w
}

/// `x[i]` with torch's `reflect` padding (edge sample not repeated) for any `i`.
pub(crate) fn reflect_index(i: isize, len: usize) -> usize {
    if len == 1 {
        return 0;
    }
    let period = 2 * (len as isize - 1);
    let mut j = i.rem_euclid(period);
    if j >= len as isize {
        j = period - j;
    }
    j as usize
}

/// `F.pad(x, (left, right), mode="reflect")`.
pub(crate) fn reflect_pad(x: &[f32], left: usize, right: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(left + x.len() + right);
    if x.is_empty() {
        out.resize(left + right, 0.0);
        return out;
    }
    let start = -(left as isize);
    let end = (x.len() + right) as isize;
    out.extend((start..end).map(|i| x[reflect_index(i, x.len())]));
    out
}

/// Peak-normalise the separators' input the way python-audio-separator does: if the peak is
/// above `max_peak`, scale down to it. Returns the factor to multiply the outputs by.
pub(crate) fn limit_peak(ch: &mut [Vec<f32>; 2], max_peak: f32) -> f32 {
    let peak = ch
        .iter()
        .flat_map(|c| c.iter())
        .fold(0.0f32, |m, &x| m.max(x.abs()));
    if peak > max_peak {
        let g = max_peak / peak;
        ch.iter_mut()
            .for_each(|c| c.iter_mut().for_each(|x| *x *= g));
        1.0 / g
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hanning_matches_numpy() {
        // numpy.hanning(5) == [0, 0.5, 1, 0.5, 0]
        let w = np_hanning(5);
        for (a, b) in w.iter().zip([0.0, 0.5, 1.0, 0.5, 0.0]) {
            assert!((a - b).abs() < 1e-6);
        }
        assert_eq!(np_hanning(1), vec![1.0]);
    }

    #[test]
    fn demucs_weight_shape() {
        // th.cat([arange(1, 3), arange(5 - 2, 0, -1)]) / 3 for L = 5
        let w = demucs_weight(5);
        let expect = [1.0, 2.0, 3.0, 2.0, 1.0].map(|x: f32| x / 3.0);
        assert_eq!(w.len(), 5);
        for (a, b) in w.iter().zip(expect) {
            assert!((a - b).abs() < 1e-6);
        }
        let w = demucs_weight(4); // [1, 2, 2, 1] / 2
        assert_eq!(w, vec![0.5, 1.0, 1.0, 0.5]);
        assert_eq!(demucs_weight(343_980).len(), 343_980);
    }

    #[test]
    fn reflect_pad_matches_torch() {
        // F.pad(tensor([1., 2., 3., 4.]), (2, 3), mode="reflect") == [3, 2, 1, 2, 3, 4, 3, 2, 1]
        let p = reflect_pad(&[1.0, 2.0, 3.0, 4.0], 2, 3);
        assert_eq!(p, vec![3.0, 2.0, 1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0]);
    }

    #[test]
    fn stereo_handles_channel_counts() {
        let mono = AudioBuffer::new(8, vec![vec![1.0, 2.0]]);
        assert_eq!(stereo(&mono), [vec![1.0, 2.0], vec![1.0, 2.0]]);
        let quad = AudioBuffer::new(8, vec![vec![4.0]; 4]);
        assert_eq!(stereo(&quad), [vec![4.0], vec![4.0]]);
    }

    #[test]
    fn peak_limit_roundtrip() {
        let mut ch = [vec![1.8, -0.2], vec![0.3, 0.0]];
        let back = limit_peak(&mut ch, 0.9);
        assert!((ch[0][0] - 0.9).abs() < 1e-6);
        assert!((ch[0][0] * back - 1.8).abs() < 1e-5);
        let mut quiet = [vec![0.5], vec![0.1]];
        assert_eq!(limit_peak(&mut quiet, 0.9), 1.0);
    }
}
