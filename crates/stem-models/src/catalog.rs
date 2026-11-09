//! The model table. Adding a model is one [`Entry`] in [`ENTRIES`].
//!
//! MDX-Net parameters were checked against UVR's `model_data.json` / `model_data_new.json`
//! (TRvlvr/application_data, keyed by the md5 of each file's last 10 MiB) and against each
//! ONNX file's declared input shape `[batch, 4, dim_f, dim_t]`. sha256 and sizes are of the
//! files at the URLs below as downloaded on 2026-11-08.
//!
//! Hugging Face entries: sizes and sha256 are the LFS metadata from the Hub API
//! (`/api/models/<repo>?blobs=true`) on 2026-11-08, at these revisions (use them in place of
//! `main` if a repo ever changes a file under us):
//!
//! | repo | revision |
//! |------|----------|
//! | audiojs/mel-roformer | c1aba02c00a0bee852d6754fe6ede8164f90a4b3 |
//! | silverdaw/bs-roformer-rhythm-onnx | 9fa8c3acdbd488b97309e4b78c9c605389cec777 |
//! | audiojs/scnet | e71bcdb42bce1d62b7fae25ba90fcbcf90f73f46 |
//! | audiojs/scnet-large | 95f22d9e68f6cab78aef358f1a0de248d4edf805 |
//!
//! RoFormer / SCNet parameters come from the model cards and from @audio/neural-separate 0.7.0
//! (`separate.js` presets `MEL` and `SCNET`, MIT), whose author made the audiojs exports.

use crate::{ModelArch, ModelInfo, StemKind, mdx23c};

/// Static description of one model; [`registry`] turns these into [`ModelInfo`]s.
struct Entry {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    /// MDX-Net: `[primary, complement]`, primary first. HTDemucs: network output order.
    stems: &'static [StemKind],
    sample_rate: u32,
    arch: ModelArch,
    url: &'static str,
    sha256: Option<&'static str>,
    size_bytes: u64,
    license: &'static str,
    /// Non-commercial or unlicensed weights: a "personal use" extra (see
    /// [`ModelInfo::non_commercial`]).
    non_commercial: bool,
}

macro_rules! uvr_url {
    ($file:literal) => {
        concat!(
            "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/",
            $file
        )
    };
}

/// Our own release for models nobody hosts yet; see [`ModelInfo::needs_manual_export`].
pub(crate) const UNHOSTED_URL_PREFIX: &str =
    "https://github.com/orth4x0r/stem-splitter/releases/download/models-v1/";

macro_rules! hf_url {
    ($repo:literal, $file:literal) => {
        concat!("https://huggingface.co/", $repo, "/resolve/main/", $file)
    };
}

macro_rules! our_url {
    ($file:literal) => {
        concat!(
            "https://github.com/orth4x0r/stem-splitter/releases/download/models-v1/",
            $file
        )
    };
}

/// UVR's MDX-Net models: hop is fixed at 1024 and every public model uses `dim_t = 2^8`.
const fn mdx(n_fft: usize, dim_f: usize, compensate: f32) -> ModelArch {
    ModelArch::MdxNet {
        n_fft,
        hop: 1024,
        dim_f,
        dim_t: 256,
        compensate,
    }
}

const UVR_LICENSE: &str = "MIT (UVR public model repository); weights by the UVR team / Kimberley Jensen \
                           - check the upstream terms before redistributing";

/// HTDemucs' training segment: `int(7.8 s * 44100)`.
pub(crate) const HTDEMUCS_SEGMENT: usize = 343_980;

/// The RoFormer exports' traced segment: 8 s at 44.1 kHz, 801 frames at hop 441.
pub(crate) const ROFORMER_SEGMENT: usize = 352_800;

/// The RoFormer exports' STFT and chunking. `overlap` 0.5 is Music-Source-Separation-Training's
/// `num_overlap: 2` (Kim's config). The BS-RoFormer config asks for 4 (0.75), which doubles the
/// run time for a fraction of a dB, so we default to 2 there too.
const fn roformer(overlap: f32) -> ModelArch {
    ModelArch::RoFormer {
        n_fft: 2048,
        hop: 441,
        segment_samples: ROFORMER_SEGMENT,
        overlap,
    }
}

/// SCNet: n_fft 4096, hop 1024, 11 s segments every 2.75 s (the reference's step 0.25).
const SCNET: ModelArch = ModelArch::Scnet {
    n_fft: 4096,
    hop: 1024,
    segment_samples: 485_100,
    overlap: 0.75,
};

/// The HTDemucs weights' terms; the code is MIT.
const DEMUCS_LICENSE: &str = "MIT code (facebookresearch/demucs). Weights: published with the MIT \
                              code, but the author has since said they are for research only \
                              (demucs issues #327, #508) - check before commercial use";

/// "Personal use" extras: non-commercial or unlicensed weights, downloaded only when picked.
/// Each is either unhosted (`our_url!`, exported by hand with the script its description names)
/// or hosted by UVR, and each licence string says who made the weights and what is (not) known
/// about their terms. Left out on purpose: anvuew's de-reverb Mel-/BS-RoFormers (GPL-3.0
/// weights) and LarsNet (CC BY-NC 4.0, but a third spectrogram U-Net arch for a weaker split).
const PERSONAL_USE: &[Entry] = &[
    Entry {
        id: "drumsep-mdx23c",
        name: "DrumSep MDX23C (6 drum parts)",
        description: "Splits a drums stem into kick, snare, toms, hi-hat, ride and crash \
                      (aufr33 & jarredou's DrumSep, MDX23C; on MSST's DrumSep test set kick \
                      14.5, snare 9.8, toms 10.6 dB SDR; hi-hat and cymbals are weaker and \
                      bleed into each other). Feed it a drums stem, not a full mix. Not hosted - \
                      export it with tools/export_mdx23c_onnx.py drumsep.",
        stems: &[
            StemKind::Kick,
            StemKind::Snare,
            StemKind::Toms,
            StemKind::HiHat,
            StemKind::Ride,
            StemKind::Crash,
        ],
        sample_rate: 44_100,
        arch: mdx23c::DRUMSEP,
        // mix_spec [1, 4, 1024, 256] -> stems_spec [1, 6, 4, 1024, 256]. Checkpoint
        // aufr33-jarredou_DrumSep_model_mdx23c_ep_141_sdr_10.8059.ckpt, sha256 d2a4aa53eb58...
        // (openmirlab/mdxnet-infer release weights-drumsep-v1; Politrees/UVR_resources).
        url: our_url!("drumsep_mdx23c.onnx"),
        sha256: None,
        size_bytes: DRUMSEP_ONNX_BYTES,
        license: "Personal use only: no stated licence. aufr33 & jarredou published the weights \
                  without terms and the original release (jarredou/models) is gone; mirrors \
                  disagree (Politrees/UVR_resources tags MIT, Intel's OpenVINO conversion CC \
                  BY-NC-SA 4.0), so treat as non-commercial. Code: MIT (ZFTurbo/Music-Source-\
                  Separation-Training)",
        non_commercial: true,
    },
    Entry {
        id: "mdx23c-dereverb",
        name: "MDX23C De-Reverb",
        description: "Takes the reverb off a vocal (or any source): a dry stem plus the reverb \
                      it removed. aufr33 & jarredou's MDX23C model (6.9 dB SDR). Run it on a \
                      vocal stem. Not hosted - export it with tools/export_mdx23c_onnx.py \
                      dereverb.",
        stems: &[StemKind::Dry, StemKind::Reverb],
        sample_rate: 44_100,
        arch: mdx23c::DEREVERB,
        // mix_spec [1, 4, 4096, 256] -> stems_spec [1, 2, 4, 4096, 256]. Checkpoint
        // dereverb_mdx23c_sdr_6.9096.ckpt (= MDX23C-De-Reverb-aufr33-jarredou.ckpt), sha256
        // eae2471b7077... (UVR / audio-separator; noblebarkrr/mvsepless_resources).
        url: our_url!("mdx23c_dereverb.onnx"),
        sha256: None,
        size_bytes: DEREVERB_ONNX_BYTES,
        license: "Personal use only: no stated licence (aufr33 & jarredou published the weights \
                  without terms; distributed through UVR and audio-separator). Code: MIT \
                  (ZFTurbo/Music-Source-Separation-Training)",
        non_commercial: true,
    },
    Entry {
        id: "mdx-reverb-hq",
        name: "MDX-Net Reverb HQ (FoxJoy)",
        description: "Quick de-reverb: FoxJoy's MDX-Net model predicts the reverb and the dry \
                      stem is the input minus that. Lighter than MDX23C De-Reverb, a little less \
                      clean. Hosted by UVR, so it downloads like the other MDX-Net models.",
        stems: &[StemKind::Reverb, StemKind::Dry],
        sample_rate: 44_100,
        // UVR model_data_new.json (md5 cd5b2989...): n_fft 6144, dim_f 3072, dim_t 2^9,
        // compensate 1.035, primary stem "Reverb".
        arch: ModelArch::MdxNet {
            n_fft: 6144,
            hop: 1024,
            dim_f: 3072,
            dim_t: 512,
            compensate: 1.035,
        },
        url: uvr_url!("Reverb_HQ_By_FoxJoy.onnx"),
        // From audio-separator 0.48.0's model-checksums.json; not downloaded by us (GitHub
        // releases were out of reach when this entry was written).
        sha256: Some("233bb5c6aaa365e568659a0a81211746fa881f8f47f82d9e864fce1f7692db80"),
        // Estimate: UVR's other dim_f 3072 MDX-Net graphs are this size.
        size_bytes: 66_759_214,
        license: "Personal use only: no stated licence (FoxJoy's weights, distributed in the UVR \
                  public model repository without terms)",
        non_commercial: true,
    },
    Entry {
        id: "mel-roformer-denoise",
        name: "Mel-RoFormer Denoise (aufr33)",
        description: "Removes hiss, hum and background noise: a clean stem plus the noise. \
                      aufr33's Mel-Band RoFormer (27.99 dB SDR on its author's set). Not hosted - \
                      export it with tools/export_mel_roformer_onnx.py denoise.",
        stems: &[StemKind::Clean, StemKind::Noise],
        sample_rate: 44_100,
        arch: roformer(0.5),
        // mix_spec [1, 4, 1025, 801] -> stems_spec [1, 4, 1025, 801] (the clean stem; the noise
        // is the rest). Checkpoint denoise_mel_band_roformer_aufr33_sdr_27.9959.ckpt, sha256
        // 7c1c39191edc... (ZFTurbo MSST release v.1.0.7).
        url: our_url!("mel_roformer_denoise.onnx"),
        sha256: None,
        size_bytes: MEL_ROFORMER_ONNX_BYTES,
        license: "Personal use only: no stated licence (aufr33's weights, published in ZFTurbo's \
                  MSST release v.1.0.7 without terms). Code: MIT (ZFTurbo/Music-Source-\
                  Separation-Training, lucidrains/BS-RoFormer)",
        non_commercial: true,
    },
    Entry {
        id: "mel-roformer-dereverb-echo",
        name: "Mel-RoFormer De-Reverb-Echo (Sucial)",
        description: "Dry vocals: takes off reverb and delay/echo (and some doubled harmonies). \
                      Sucial's Mel-Band RoFormer v2 (13.48 dB SDR). Best on a vocal stem. Not \
                      hosted - export it with tools/export_mel_roformer_onnx.py dereverb-echo.",
        stems: &[StemKind::Dry, StemKind::Reverb],
        sample_rate: 44_100,
        arch: roformer(0.5),
        // Checkpoint dereverb-echo_mel_band_roformer_sdr_13.4843_v2.ckpt, sha256
        // 396432f5af25... (huggingface.co/Sucial/Dereverb-Echo_Mel_Band_Roformer).
        url: our_url!("mel_roformer_dereverb_echo.onnx"),
        sha256: None,
        size_bytes: MEL_ROFORMER_ONNX_BYTES,
        license: "CC BY-NC-SA 4.0 (weights: Sucial / SUC-DriverOld): non-commercial, \
                  attribution, share-alike. Code: MIT (ZFTurbo/Music-Source-Separation-Training, \
                  lucidrains/BS-RoFormer)",
        non_commercial: true,
    },
];

/// fp32 export sizes of the unhosted personal-use models, shown before the file is there. The
/// MDX23C figures are `export_mdx23c_onnx.py --random-weights` outputs (torch 2.15, opset 17);
/// the Mel-RoFormer one is an estimate (twice audiojs' fp16 Kim export). The scripts print the
/// exact size of each real export.
const DRUMSEP_ONNX_BYTES: u64 = 437_415_924;
const DEREVERB_ONNX_BYTES: u64 = 447_862_588;
const MEL_ROFORMER_ONNX_BYTES: u64 = 920_000_000;

const ENTRIES: &[Entry] = &[
    Entry {
        id: "mdx-inst-hq3",
        name: "MDX-Net Inst HQ 3",
        description: "Instrumental / vocals split, tuned for clean instrumentals (karaoke tracks, \
                      beats). The network predicts the instrumental; vocals are mix minus that.",
        stems: &[StemKind::Instrumental, StemKind::Vocals],
        sample_rate: 44_100,
        arch: mdx(6144, 3072, 1.022),
        url: uvr_url!("UVR-MDX-NET-Inst_HQ_3.onnx"),
        sha256: Some("317554b07fe1ea5279a77f2b1520a41ea4b93432560c4ffd08792c30fddf9adc"),
        size_bytes: 66_759_214,
        license: UVR_LICENSE,
        non_commercial: false,
    },
    Entry {
        id: "mdx-inst-hq4",
        name: "MDX-Net Inst HQ 4",
        description: "Newer instrumental model (smaller, 2560 bins). Predicts the instrumental; \
                      vocals are mix minus that.",
        stems: &[StemKind::Instrumental, StemKind::Vocals],
        sample_rate: 44_100,
        arch: mdx(5120, 2560, 1.019),
        url: uvr_url!("UVR-MDX-NET-Inst_HQ_4.onnx"),
        sha256: Some("3c4b5b9b05090fdf238f38ba5046813982d50e2a652e9cb3324ea79720c3c9c8"),
        size_bytes: 59_074_342,
        license: UVR_LICENSE,
        non_commercial: false,
    },
    Entry {
        id: "mdx-kim-vocal-2",
        name: "MDX-Net Kim Vocal 2",
        description: "Clean vocal extraction (acapellas for sampling). Predicts the vocals; the \
                      instrumental is mix minus vocals.",
        stems: &[StemKind::Vocals, StemKind::Instrumental],
        sample_rate: 44_100,
        arch: mdx(7680, 3072, 1.009),
        url: uvr_url!("Kim_Vocal_2.onnx"),
        sha256: Some("ce74ef3b6a6024ce44211a07be9cf8bc6d87728cc852a68ab34eb8e58cde9c8b"),
        size_bytes: 66_759_214,
        license: UVR_LICENSE,
        non_commercial: false,
    },
    Entry {
        id: "mdx-voc-ft",
        name: "MDX-Net Voc FT",
        description: "Fine-tuned vocal model, an alternative to Kim Vocal 2. Predicts the vocals; \
                      the instrumental is mix minus vocals.",
        stems: &[StemKind::Vocals, StemKind::Instrumental],
        sample_rate: 44_100,
        arch: mdx(7680, 3072, 1.021),
        url: uvr_url!("UVR-MDX-NET-Voc_FT.onnx"),
        sha256: Some("534b2070fcc7df514b13ef660dc8cbb328679c2374d04354a5c42bb14ecce111"),
        size_bytes: 66_762_490,
        license: UVR_LICENSE,
        non_commercial: false,
    },
    Entry {
        id: "mdx-karaoke-2",
        name: "MDX-Net Karaoke 2 (lead / backing vocals)",
        description: "Splits the lead vocal from backing vocals. Best run on a vocal stem (e.g. \
                      the output of Kim Vocal 2); on a full mix the 'Backing vocals' stem also \
                      holds the instrumental. UVR marks its predicted stem as 'Instrumental', \
                      i.e. everything but the lead; the lead vocal is mix minus that.",
        stems: &[StemKind::BackingVocals, StemKind::LeadVocals],
        sample_rate: 44_100,
        arch: mdx(5120, 2048, 1.065),
        url: uvr_url!("UVR_MDXNET_KARA_2.onnx"),
        sha256: Some("bf32e15105a09c0f7dddd2b67346146334d6f3ecb399ed7638eba2ab07cbf5f4"),
        size_bytes: 52_786_726,
        license: UVR_LICENSE,
        non_commercial: false,
    },
    Entry {
        id: "htdemucs",
        name: "HTDemucs (4 stems)",
        description: "Meta's Hybrid Transformer Demucs v4: drums, bass, other, vocals. A solid \
                      all-round 4-stem split. Prebuilt ONNX export by Ghilda (the same I/O as \
                      tools/export_htdemucs_onnx.py).",
        stems: &[
            StemKind::Drums,
            StemKind::Bass,
            StemKind::Other,
            StemKind::Vocals,
        ],
        sample_rate: 44_100,
        arch: ModelArch::HtDemucs {
            segment_samples: HTDEMUCS_SEGMENT,
        },
        // Inputs mix [1, 2, 343980] + mag [1, 4, 2048, 336], outputs freq [1, 4, 4, 2048, 336]
        // + time [1, 4, 2, 343980]: our contract (demucs.rs matches I/O by rank, not name).
        url: hf_url!("Ghilda/htdemucs-onnx", "htdemucs.onnx"),
        sha256: Some("e528a932a7d091e15938369135569884b62c2193fb11044c3d4a0d4c7b9221af"),
        size_bytes: 174_266_088,
        license: DEMUCS_LICENSE,
        non_commercial: false,
    },
    Entry {
        id: "htdemucs-6s",
        name: "HTDemucs 6s (6 stems)",
        description: "Experimental 6-stem Demucs v4: drums, bass, other, vocals, guitar, piano \
                      (piano is weak). Not hosted yet - export it with \
                      tools/export_htdemucs_onnx.py --six and put htdemucs_6s.onnx in the models \
                      folder.",
        stems: &[
            StemKind::Drums,
            StemKind::Bass,
            StemKind::Other,
            StemKind::Vocals,
            StemKind::Guitar,
            StemKind::Piano,
        ],
        sample_rate: 44_100,
        arch: ModelArch::HtDemucs {
            segment_samples: HTDEMUCS_SEGMENT,
        },
        url: our_url!("htdemucs_6s.onnx"),
        sha256: Some("f0a08e1774fd85c3bf23d6c1bda4db7f99de70e2512ac35f84b7c954ec1bb4d0"),
        // tools/export_htdemucs_onnx.py output (torch 2.15, demucs 4.1.0, opset 17).
        size_bytes: 114_560_305,
        license: DEMUCS_LICENSE,
        non_commercial: false,
    },
    Entry {
        id: "mel-roformer-kim",
        name: "Mel-RoFormer Vocals (Kim)",
        description: "Best vocals: Kimberley Jensen's Mel-Band RoFormer, the cleanest acapellas \
                      and instrumentals here (median vocal SDR 12.1 dB on MUSDB18 previews vs \
                      about 9-10 for MDX-Net). The network predicts the vocals; the \
                      instrumental is mix minus vocals. Big (464 MB) and slow on the CPU.",
        stems: &[StemKind::Vocals, StemKind::Instrumental],
        sample_rate: 44_100,
        arch: roformer(0.5),
        // mix_spec [1, 4, 1025, 801] -> stems_spec [1, 4, 1025, 801]; fp16 weights, fp32 compute.
        url: hf_url!("audiojs/mel-roformer", "mel-roformer.fp16.onnx"),
        sha256: Some("adcbbb826f12453fbb86652b503242d565a9c81cdef1bca2fe0e54752eba91ef"),
        size_bytes: 464_050_097,
        license: "MIT (weights: Kimberley Jensen; model code: ZFTurbo/Music-Source-Separation-\
                  Training and lucidrains/BS-RoFormer; ONNX export: audiojs)",
        non_commercial: false,
    },
    Entry {
        id: "bs-roformer-4stem",
        name: "BS-RoFormer (4 stems)",
        description: "Best drums and bass: ZFTurbo's 4-stem Band-Split RoFormer (drums, bass, \
                      other, vocals; SDR 9.66 averaged over the stems on MUSDB18). Tighter \
                      drums and bass than HTDemucs for beat-making; vocals are better from \
                      Mel-RoFormer. Big (269 MB) and slow on the CPU.",
        stems: &[
            StemKind::Drums,
            StemKind::Bass,
            StemKind::Other,
            StemKind::Vocals,
        ],
        sample_rate: 44_100,
        arch: roformer(0.5),
        // spec_real / spec_imag [1, 2, 1025, 801] -> out_spec_real / out_spec_imag
        // [1, 4, 2, 1025, 801]; fp16 weights, opset 17.
        url: hf_url!(
            "silverdaw/bs-roformer-rhythm-onnx",
            "bs_roformer_4stem_rhythm_fp16.onnx"
        ),
        sha256: Some("4bac38e42c085b5d69964e2b7ff161318a1315122c10b025c718b20e94d85d6b"),
        size_bytes: 269_457_840,
        license: "MIT (weights: ZFTurbo MSST v1.0.12 model_bs_roformer_ep_17_sdr_9.6568, \
                  trained on MUSDB18-HQ; ONNX export: silverdaw)",
        non_commercial: false,
    },
    Entry {
        id: "scnet",
        name: "SCNet (4 stems, fast)",
        description: "Fast 4-stem: a 13 MB int8 SCNet (drums, bass, other, vocals) at about \
                      HTDemucs quality (vocals 9.9, drums 9.4, bass 8.4, other 6.1 dB median \
                      SDR on MUSDB18 previews) in a fraction of the memory. Good for quick \
                      previews and weak GPUs.",
        stems: &[
            StemKind::Drums,
            StemKind::Bass,
            StemKind::Other,
            StemKind::Vocals,
        ],
        sample_rate: 44_100,
        arch: SCNET,
        // mix_spec [1, 4, 2049, 476] -> stems_spec [1, 16, 2049, 476].
        url: hf_url!("audiojs/scnet", "scnet.int8.onnx"),
        sha256: Some("98228931494151762a1c4ab1ec7899a894b1f81fd4a509921a9d2f9bacc50845"),
        size_bytes: 12_900_277,
        license: "MIT (starrytong/SCNet code and weights; the author confirmed converted \
                  redistribution in SCNet issue #35; trained on MUSDB18-HQ; ONNX: audiojs)",
        non_commercial: false,
    },
    Entry {
        id: "scnet-large",
        name: "SCNet Large (4 stems)",
        description: "Best 4-stem quality per MB: SCNet-large int8 (45 MB), ahead of HTDemucs on \
                      every stem in its author's MUSDB18 test (vocals 10.9, drums 11.2, bass \
                      9.4, other 7.3 dB). About 3x slower than SCNet, still far lighter than \
                      the RoFormers.",
        stems: &[
            StemKind::Drums,
            StemKind::Bass,
            StemKind::Other,
            StemKind::Vocals,
        ],
        sample_rate: 44_100,
        arch: SCNET,
        url: hf_url!("audiojs/scnet-large", "scnet-large.int8.onnx"),
        sha256: Some("b2dc586a1e0e6c0afe4915e9057ea29111397b7bbf589edc3d85de91de79cd72"),
        size_bytes: 45_082_486,
        license: "MIT (starrytong/SCNet code and weights, via ZFTurbo MSST v1.0.9; trained on \
                  MUSDB18-HQ; ONNX: audiojs)",
        non_commercial: false,
    },
];

fn entries() -> impl Iterator<Item = &'static Entry> {
    ENTRIES.iter().chain(PERSONAL_USE)
}

/// Every model the app offers (the personal-use extras last).
pub fn registry() -> Vec<ModelInfo> {
    entries().map(to_info).collect()
}

/// Look a model up by [`ModelInfo::id`].
pub fn find_model(id: &str) -> Option<ModelInfo> {
    entries().find(|e| e.id == id).map(to_info)
}

fn to_info(e: &Entry) -> ModelInfo {
    ModelInfo {
        id: e.id,
        name: e.name,
        description: e.description,
        stems: e.stems.to_vec(),
        sample_rate: e.sample_rate,
        arch: e.arch.clone(),
        url: e.url,
        sha256: e.sha256,
        size_bytes: e.size_bytes,
        license: e.license,
        non_commercial: e.non_commercial,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_and_files_are_unique() {
        let r = registry();
        let ids: HashSet<_> = r.iter().map(|m| m.id).collect();
        let files: HashSet<_> = r.iter().map(|m| m.file_name()).collect();
        assert_eq!(ids.len(), r.len());
        assert_eq!(files.len(), r.len());
    }

    #[test]
    fn entries_are_consistent() {
        for m in registry() {
            assert!(m.file_name().ends_with(".onnx"), "{}", m.id);
            assert!(!m.license.is_empty());
            if let Some(h) = m.sha256 {
                assert_eq!(h.len(), 64);
                assert!(h.bytes().all(|b| b.is_ascii_hexdigit()));
            }
            match m.arch {
                ModelArch::MdxNet {
                    n_fft,
                    hop,
                    dim_f,
                    dim_t,
                    compensate,
                } => {
                    assert_eq!(m.stems.len(), 2, "{}", m.id);
                    assert!(dim_f <= n_fft / 2 + 1);
                    // The chunk must be longer than the two trimmed halves of the FFT.
                    assert!(hop * (dim_t - 1) > n_fft);
                    assert!((1.0..1.2).contains(&compensate));
                }
                ModelArch::HtDemucs { segment_samples } => {
                    assert!(m.stems.len() == 4 || m.stems.len() == 6);
                    assert_eq!(segment_samples, HTDEMUCS_SEGMENT);
                }
                ModelArch::RoFormer { .. } | ModelArch::Scnet { .. } | ModelArch::Mdx23c { .. } => {
                    // The segment geometry must resolve (frames consistent with the segment).
                    let p = crate::roformer::SpecParams::new(&m.arch, None, None).unwrap();
                    assert!(p.step < p.segment && p.fade > 0, "{}", m.id);
                    assert!(!m.stems.is_empty());
                }
            }
            if m.url.starts_with("https://huggingface.co/") {
                assert!(m.url.contains("/resolve/main/"), "{}", m.id);
                assert!(m.sha256.is_some(), "{}: pin hosted files", m.id);
                assert!(!m.needs_manual_export());
            }
        }
        assert_eq!(
            find_model("mdx-kim-vocal-2").unwrap().stems[0],
            StemKind::Vocals
        );
        assert!(find_model("nope").is_none());
        assert!(!find_model("htdemucs").unwrap().needs_manual_export());
        assert!(find_model("htdemucs-6s").unwrap().needs_manual_export());
        let frames = |id: &str| {
            crate::roformer::SpecParams::new(&find_model(id).unwrap().arch, None, None)
                .unwrap()
                .frames
        };
        assert_eq!(frames("mel-roformer-kim"), 801);
        assert_eq!(frames("bs-roformer-4stem"), 801);
        assert_eq!(frames("scnet"), 476);
        assert_eq!(frames("scnet-large"), 476);
        assert_eq!(frames("drumsep-mdx23c"), 256);
        assert_eq!(frames("mdx23c-dereverb"), 256);
        assert_eq!(frames("mel-roformer-denoise"), 801);
    }

    #[test]
    fn personal_use_models_are_marked() {
        let r = registry();
        for m in r.iter().filter(|m| m.non_commercial) {
            // An honest licence string, and no GPL weights.
            let l = m.license.to_ascii_lowercase();
            assert!(
                l.contains("personal use") || l.contains("non-commercial"),
                "{}: {}",
                m.id,
                m.license
            );
            assert!(!l.contains("gpl"), "{}", m.id);
        }
        let nc: Vec<_> = r
            .iter()
            .filter(|m| m.non_commercial)
            .map(|m| m.id)
            .collect();
        assert!(nc.contains(&"drumsep-mdx23c") && nc.contains(&"mdx-reverb-hq"));
        assert!(!find_model("htdemucs").unwrap().non_commercial);
        let drums = find_model("drumsep-mdx23c").unwrap();
        assert!(drums.needs_manual_export());
        assert_eq!(drums.stems, StemKind::DRUM_PARTS.to_vec());
        // The FoxJoy model predicts the reverb; the dry stem is the rest.
        assert_eq!(
            find_model("mdx-reverb-hq").unwrap().stems[0],
            StemKind::Reverb
        );
        // Personal-use extras come after every regular model.
        let first = r.iter().position(|m| m.non_commercial).unwrap();
        assert!(r[first..].iter().all(|m| m.non_commercial));
    }
}
