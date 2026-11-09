//! Ensembles: combine the same stem as separated by several models, UVR-style.
//!
//! * [`EnsembleMode::Average`] averages the waveforms (the safe default: a few tenths of a dB
//!   better than either model, fewer one-model artefacts).
//! * [`EnsembleMode::MaxSpec`] keeps, per STFT bin, the input with the largest magnitude (its
//!   complex value, so phase comes with it): fuller stems, more bleed.
//! * [`EnsembleMode::MinSpec`] keeps the smallest magnitude: cleaner stems, thinner sound. UVR
//!   users pick it for instrumentals to cut vocal bleed.
//!
//! The per-bin max/min idea follows UVR's `spec_utils.ensembling` (MIT), reimplemented here on
//! [`crate::stft::Stft`].

use crate::stft::{Complex32, Stft};
use crate::{AudioBuffer, AudioError, Result};

/// FFT size and hop for the spectral modes: 46 ms at 44.1 kHz, the usual UVR setting.
pub const N_FFT: usize = 2048;
pub const HOP: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum EnsembleMode {
    #[default]
    Average,
    MaxSpec,
    MinSpec,
}

impl EnsembleMode {
    pub const ALL: [EnsembleMode; 3] = [
        EnsembleMode::Average,
        EnsembleMode::MaxSpec,
        EnsembleMode::MinSpec,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EnsembleMode::Average => "Average",
            EnsembleMode::MaxSpec => "Max spec",
            EnsembleMode::MinSpec => "Min spec",
        }
    }

    /// One line for tooltips.
    pub fn hint(self) -> &'static str {
        match self {
            EnsembleMode::Average => "Average the waveforms: balanced, fewer artefacts",
            EnsembleMode::MaxSpec => "Loudest bin wins: fuller, keeps more (and more bleed)",
            EnsembleMode::MinSpec => "Quietest bin wins: cleanest, can sound thin",
        }
    }
}

/// Combine `inputs` (the same stem from different models) into one buffer.
///
/// All inputs must share a sample rate. The output has the first input's channel count (a mono
/// input is used for every channel) and the shortest input's length. A single input is returned
/// as is.
pub fn ensemble(inputs: &[&AudioBuffer], mode: EnsembleMode) -> Result<AudioBuffer> {
    let Some(first) = inputs.first() else {
        return Err(AudioError::Decode("ensemble of nothing".into()));
    };
    if let Some(bad) = inputs.iter().find(|b| b.sample_rate != first.sample_rate) {
        return Err(AudioError::Decode(format!(
            "ensemble inputs differ in sample rate ({} vs {} Hz)",
            first.sample_rate, bad.sample_rate
        )));
    }
    if inputs.len() == 1 {
        return Ok((*first).clone());
    }
    let frames = inputs.iter().map(|b| b.frames()).min().unwrap_or(0);
    let stft = (mode != EnsembleMode::Average).then(|| Stft::new(N_FFT, HOP));
    let channels = (0..first.n_channels())
        .map(|c| {
            let chans: Vec<&[f32]> = inputs
                .iter()
                .map(|b| channel(b, c).map_or(&[][..], |ch| &ch[..frames.min(ch.len())]))
                .collect();
            match &stft {
                None => average(&chans, frames),
                Some(stft) => spectral(stft, &chans, frames, mode == EnsembleMode::MaxSpec),
            }
        })
        .collect();
    Ok(AudioBuffer::new(first.sample_rate, channels))
}

/// Channel `c` of `b`, or its only channel if it is mono.
fn channel(b: &AudioBuffer, c: usize) -> Option<&Vec<f32>> {
    match b.channels.len() {
        1 => b.channels.first(),
        _ => b.channels.get(c),
    }
}

fn average(chans: &[&[f32]], frames: usize) -> Vec<f32> {
    let k = 1.0 / chans.len() as f32;
    (0..frames)
        .map(|i| {
            chans
                .iter()
                .map(|ch| ch.get(i).copied().unwrap_or(0.0))
                .sum::<f32>()
                * k
        })
        .collect()
}

fn spectral(stft: &Stft, chans: &[&[f32]], frames: usize, max: bool) -> Vec<f32> {
    if frames == 0 {
        return Vec::new();
    }
    let mut specs = chans.iter().map(|ch| {
        // Inputs shorter than `frames` (missing channels) are zero-padded.
        if ch.len() == frames {
            stft.forward(ch)
        } else {
            let mut v = ch.to_vec();
            v.resize(frames, 0.0);
            stft.forward(&v)
        }
    });
    let Some(mut out) = specs.next() else {
        return vec![0.0; frames];
    };
    for spec in specs {
        for (of, sf) in out.iter_mut().zip(&spec) {
            for (o, s) in of.iter_mut().zip(sf) {
                if pick(*s, *o, max) {
                    *o = *s;
                }
            }
        }
    }
    stft.inverse(&out, frames)
}

/// True if `candidate` should replace `current`.
fn pick(candidate: Complex32, current: Complex32, max: bool) -> bool {
    let (a, b) = (candidate.norm_sqr(), current.norm_sqr());
    if max { a > b } else { a < b }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    fn tone(freq: f32, amp: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / SR as f32).sin())
            .collect()
    }

    fn stereo(ch: Vec<f32>) -> AudioBuffer {
        AudioBuffer::new(SR, vec![ch.clone(), ch])
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// RMS over the middle of the signal, away from STFT edge effects.
    fn rms_mid(x: &[f32]) -> f32 {
        let n = x.len();
        rms(&x[n / 8..n - n / 8])
    }

    fn max_diff(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn average_is_the_mean() {
        let a = stereo(tone(440.0, 0.5, 4000));
        let b = stereo(tone(660.0, 0.3, 4000));
        let out = ensemble(&[&a, &b], EnsembleMode::Average).unwrap();
        assert_eq!(out.frames(), 4000);
        assert_eq!(out.n_channels(), 2);
        for i in [0, 17, 999, 3999] {
            let want = (a.channels[0][i] + b.channels[0][i]) / 2.0;
            assert!((out.channels[1][i] - want).abs() < 1e-6);
        }
    }

    #[test]
    fn identical_inputs_come_back_unchanged() {
        let a = stereo(tone(523.0, 0.4, 20_000));
        for mode in EnsembleMode::ALL {
            let out = ensemble(&[&a, &a, &a], mode).unwrap();
            assert_eq!(out.frames(), a.frames());
            let d = max_diff(&out.channels[0], &a.channels[0]);
            assert!(d < 1e-4, "{mode:?}: max diff {d}");
        }
    }

    #[test]
    fn max_spec_keeps_both_tones_and_min_spec_drops_them() {
        // Two "models" that each found a different half of the stem.
        let n = 44_100;
        let low = tone(220.0, 0.5, n);
        let high = tone(3_520.0, 0.5, n);
        let a = stereo(low.clone());
        let b = stereo(high.clone());
        let max = ensemble(&[&a, &b], EnsembleMode::MaxSpec).unwrap();
        let both: Vec<f32> = low.iter().zip(&high).map(|(x, y)| x + y).collect();
        // Max spec is (close to) the sum: each tone's bins come from the model that had it.
        let err = rms_mid(
            &max.channels[0]
                .iter()
                .zip(&both)
                .map(|(x, y)| x - y)
                .collect::<Vec<_>>(),
        );
        assert!(err < 0.02 * rms_mid(&both), "max-spec error {err}");
        // Min spec keeps only what both agree on, which is (nearly) nothing.
        let min = ensemble(&[&a, &b], EnsembleMode::MinSpec).unwrap();
        assert!(
            rms_mid(&min.channels[0]) < 0.02 * rms_mid(&low),
            "min-spec rms {}",
            rms_mid(&min.channels[0])
        );
        // Average halves each tone.
        let avg = ensemble(&[&a, &b], EnsembleMode::Average).unwrap();
        let half = rms_mid(&both) / 2.0;
        assert!((rms_mid(&avg.channels[0]) - half).abs() < 1e-3);
    }

    #[test]
    fn min_spec_removes_bleed_only_one_model_has() {
        // Model A leaks a hi-hat-like tone into the vocal; model B doesn't.
        let n = 44_100;
        let vocal = tone(440.0, 0.4, n);
        let bleed = tone(8_000.0, 0.2, n);
        let a = stereo(vocal.iter().zip(&bleed).map(|(v, b)| v + b).collect());
        let b = stereo(vocal.clone());
        let min = ensemble(&[&a, &b], EnsembleMode::MinSpec).unwrap();
        let err: Vec<f32> = min.channels[0]
            .iter()
            .zip(&vocal)
            .map(|(x, y)| x - y)
            .collect();
        assert!(rms_mid(&err) < 0.05 * rms_mid(&bleed), "{}", rms_mid(&err));
        let max = ensemble(&[&a, &b], EnsembleMode::MaxSpec).unwrap();
        let err: Vec<f32> = max.channels[0]
            .iter()
            .zip(&a.channels[0])
            .map(|(x, y)| x - y)
            .collect();
        assert!(rms_mid(&err) < 0.05 * rms_mid(&bleed));
    }

    #[test]
    fn shapes_lengths_and_errors() {
        let a = stereo(tone(300.0, 0.5, 5000));
        let mono = AudioBuffer::new(SR, vec![tone(300.0, 0.5, 4000)]);
        let out = ensemble(&[&a, &mono], EnsembleMode::MaxSpec).unwrap();
        assert_eq!((out.n_channels(), out.frames()), (2, 4000));
        let out = ensemble(&[&a], EnsembleMode::MinSpec).unwrap();
        assert_eq!(out, a);
        assert!(ensemble(&[], EnsembleMode::Average).is_err());
        let other_rate = AudioBuffer::new(48_000, a.channels.clone());
        assert!(ensemble(&[&a, &other_rate], EnsembleMode::Average).is_err());
        let empty = AudioBuffer::new(SR, vec![Vec::new(), Vec::new()]);
        let out = ensemble(&[&a, &empty], EnsembleMode::MaxSpec).unwrap();
        assert_eq!(out.frames(), 0);
    }
}
