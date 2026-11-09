//! MDX23C (TFC-TDF v3) models: the drum-part splitter (aufr33 / jarredou DrumSep) and the
//! aufr33 / jarredou de-reverb.
//!
//! `TFC_TDF_net` in ZFTurbo's Music-Source-Separation-Training (`models/mdx23c_tfc_tdf_v3.py`,
//! MIT) runs `torch.stft` inside `forward`, which does not export to ONNX.
//! `tools/export_mdx23c_onnx.py` swaps the model's `stft` object for a pass-through, so the graph
//! is everything between the STFT and the iSTFT:
//!
//! ```text
//! inputs   mix_spec    float32 [1, 4, dim_f, T]      L.re, L.im, R.re, R.im; bins 0..dim_f
//! outputs  stems_spec  float32 [1, S, 4, dim_f, T]   per source, the same channel layout
//! ```
//!
//! The STFT around it is `STFT.__call__` / `STFT.inverse`: periodic Hann, `center=True`, reflect
//! padding, unnormalised; the input keeps the lowest `dim_f` bins, the output is zero-padded
//! back to `n_fft / 2 + 1`. That is the RoFormer "stacked" contract with fewer bins, so
//! [`crate::roformer::SpecSeparator`] runs these graphs ([`crate::ModelArch::Mdx23c`] sets
//! `dim_f`). A segment is the config's `audio.chunk_size` (`(dim_t - 1) * hop`, so `T = dim_t`)
//! and chunking is MSST's generic `demix()`, the same as for the RoFormers.
//!
//! Every network output is a stem (`target_instrument: null` in both configs), in the config's
//! `training.instruments` order, which is the order of `ModelInfo::stems`.

use crate::ModelArch;

/// MSST's MDX23C geometry: `chunk_size = hop * (dim_t - 1)`.
pub(crate) const fn arch(
    n_fft: usize,
    hop: usize,
    dim_f: usize,
    dim_t: usize,
    overlap: f32,
) -> ModelArch {
    ModelArch::Mdx23c {
        n_fft,
        hop,
        dim_f,
        segment_samples: hop * (dim_t - 1),
        overlap,
    }
}

/// `aufr33-jarredou_DrumSep_model_mdx23c_ep_141_sdr_10.8059.yaml`: n_fft 2048, hop 512,
/// dim_f 1024, dim_t 256 (chunk 130560 samples, about 3 s). The config asks for
/// `num_overlap: 4`; 2 (0.5) halves the run time and is what we default to, as for the RoFormers.
pub(crate) const DRUMSEP: ModelArch = arch(2048, 512, 1024, 256, 0.5);

/// `config_dereverb_mdx23c.yaml`: n_fft 8192, hop 1024, dim_f 4096, dim_t 256 (chunk 261120
/// samples, about 6 s).
pub(crate) const DEREVERB: ModelArch = arch(8192, 1024, 4096, 256, 0.5);

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use stem_audio::AudioBuffer;

    use crate::network::{Network, TensorIn, TensorOut};
    use crate::roformer::{SpecParams, SpecSeparator};
    use crate::{Device, LoadOptions, ModelArch, Result, Separator, StemKind};

    #[test]
    fn real_model_geometry() {
        let p = SpecParams::new(&super::DRUMSEP, None, Some(256)).unwrap();
        assert_eq!(p.segment, 130_560);
        assert_eq!(p.frames, 256);
        assert_eq!(p.padded_segment(), 130_560);
        assert_eq!((p.bins(), p.graph_bins()), (1025, 1024));
        assert_eq!(p.step, 65_280);
        assert_eq!(p.fade, 13_056);
        let p = SpecParams::new(&super::DEREVERB, None, Some(256)).unwrap();
        assert_eq!(p.segment, 261_120);
        assert_eq!((p.bins(), p.graph_bins()), (4097, 4096));
        // The config's num_overlap 4.
        let p = SpecParams::new(&super::DRUMSEP, Some(0.75), None).unwrap();
        assert_eq!(p.step, 32_640);
        // dim_f above the STFT's bins is refused.
        assert!(SpecParams::new(&super::arch(512, 128, 300, 64, 0.5), None, None).is_err());
    }

    /// A stand-in MDX23C graph: `[1, 4, dim_f, T]` in, `[1, S, 4, dim_f, T]` out, each source a
    /// gain on the cropped spectrogram. Checks the crop reached it.
    struct CropNet {
        dim_f: usize,
        frames: usize,
        gains: Vec<f32>,
        calls: usize,
    }

    impl Network for CropNet {
        fn input_shapes(&self) -> Vec<Vec<i64>> {
            vec![vec![1, 4, self.dim_f as i64, self.frames as i64]]
        }
        fn input_names(&self) -> Vec<String> {
            vec!["mix_spec".into()]
        }
        fn output_names(&self) -> Vec<String> {
            vec!["stems_spec".into()]
        }
        fn run(&mut self, inputs: &[TensorIn<'_>]) -> Result<Vec<TensorOut>> {
            self.calls += 1;
            assert_eq!(inputs.len(), 1);
            assert_eq!(inputs[0].shape, vec![1, 4, self.dim_f, self.frames]);
            assert_eq!(inputs[0].data.len(), 4 * self.dim_f * self.frames);
            let data = self
                .gains
                .iter()
                .flat_map(|&g| inputs[0].data.iter().map(move |v| v * g))
                .collect();
            Ok(vec![TensorOut {
                shape: vec![1, self.gains.len(), 4, self.dim_f, self.frames],
                data,
            }])
        }
        fn device(&self) -> Device {
            Device::Cpu
        }
    }

    fn info(arch: ModelArch, stems: Vec<StemKind>) -> crate::ModelInfo {
        let mut info = crate::find_model("drumsep-mdx23c").unwrap();
        info.arch = arch;
        info.stems = stems;
        info
    }

    /// Low tones (well under the cut-off) on both channels, different per channel.
    fn low_mix(n: usize) -> AudioBuffer {
        let tone = |f: f32, a: f32, ph: f32| -> Vec<f32> {
            (0..n)
                .map(|i| a * (2.0 * std::f32::consts::PI * f * i as f32 / 44_100.0 + ph).sin())
                .collect()
        };
        let l: Vec<f32> = tone(220.0, 0.4, 0.0)
            .iter()
            .zip(tone(1_500.0, 0.2, 0.3))
            .map(|(a, b)| a + b)
            .collect();
        let r: Vec<f32> = tone(330.0, 0.3, 1.0)
            .iter()
            .zip(tone(2_500.0, 0.1, 0.0))
            .map(|(a, b)| a + b)
            .collect();
        AudioBuffer::new(44_100, vec![l, r])
    }

    fn max_err(y: &[f32], x: &[f32], g: f32, margin: usize) -> f32 {
        y[margin..y.len() - margin]
            .iter()
            .zip(&x[margin..x.len() - margin])
            .map(|(a, b)| (a - g * b).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn cropped_bins_end_to_end_six_stems() {
        // A scaled-down DrumSep: n_fft 512, hop 128, dim_f 192 of 257 bins (cut-off about
        // 16.5 kHz), 63 frames per 7936-sample segment.
        let arch = super::arch(512, 128, 192, 63, 0.75);
        let gains = vec![0.5, 0.25, 0.125, 0.0625, 0.03125, 0.03125];
        let net = CropNet {
            dim_f: 192,
            frames: 63,
            gains: gains.clone(),
            calls: 0,
        };
        let stems_list = StemKind::DRUM_PARTS.to_vec();
        let mut sep = SpecSeparator::new(
            info(arch, stems_list.clone()),
            Box::new(net),
            &LoadOptions::default(),
        )
        .unwrap();
        let mix = low_mix(60_000);
        let mut last = None;
        let stems = sep
            .separate(&mix, &mut |p| last = Some(p), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(last, Some(crate::Progress::Separating(1.0)));
        assert_eq!(
            stems.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            stems_list
        );
        // Everything in the mix is below the cut-off, so each stem is its gain times the mix.
        for ((_, buf), &g) in stems.iter().zip(&gains) {
            for c in 0..2 {
                assert_eq!(buf.channels[c].len(), mix.frames());
                let e = max_err(&buf.channels[c], &mix.channels[c], g, 512);
                assert!(e < 2e-4, "gain {g} ch {c}: {e}");
            }
        }
    }

    #[test]
    fn bins_above_dim_f_come_back_empty() {
        // A tone above the cut-off disappears; one below goes through untouched.
        let arch = super::arch(512, 128, 64, 63, 0.5);
        let net = CropNet {
            dim_f: 64, // 64 of 257 bins: below about 5.5 kHz
            frames: 63,
            gains: vec![1.0, 1.0],
            calls: 0,
        };
        let mut sep = SpecSeparator::new(
            info(arch, vec![StemKind::Dry, StemKind::Reverb]),
            Box::new(net),
            &LoadOptions::default(),
        )
        .unwrap();
        let n = 30_000;
        let tone = |f: f32| -> Vec<f32> {
            (0..n)
                .map(|i| 0.5 * (2.0 * std::f32::consts::PI * f * i as f32 / 44_100.0).sin())
                .collect()
        };
        let low = tone(1_000.0);
        let mix_ch: Vec<f32> = low.iter().zip(tone(12_000.0)).map(|(a, b)| a + b).collect();
        let mix = AudioBuffer::new(44_100, vec![mix_ch.clone(), mix_ch]);
        let stems = sep
            .separate(&mix, &mut |_| {}, &AtomicBool::new(false))
            .unwrap();
        let e = max_err(&stems[0].1.channels[0], &low, 1.0, 512);
        assert!(e < 1e-3, "{e}");
    }

    #[test]
    fn wrong_bin_count_is_refused() {
        let net = CropNet {
            dim_f: 257,
            frames: 63,
            gains: vec![1.0],
            calls: 0,
        };
        let r = SpecSeparator::new(
            info(super::arch(512, 128, 192, 63, 0.5), vec![StemKind::Kick]),
            Box::new(net),
            &LoadOptions::default(),
        );
        assert!(r.is_err());
    }
}
