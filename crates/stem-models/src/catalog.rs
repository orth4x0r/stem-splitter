//! The model table. Adding a model is one [`Entry`] in [`ENTRIES`].
//!
//! MDX-Net parameters were checked against UVR's `model_data.json` / `model_data_new.json`
//! (TRvlvr/application_data, keyed by the md5 of each file's last 10 MiB) and against each
//! ONNX file's declared input shape `[batch, 4, dim_f, dim_t]`. sha256 and sizes are of the
//! files at the URLs below as downloaded on 2026-11-08.

use crate::{ModelArch, ModelInfo, StemKind};

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
}

macro_rules! uvr_url {
    ($file:literal) => {
        concat!(
            "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/",
            $file
        )
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
    },
    Entry {
        id: "htdemucs",
        name: "HTDemucs (4 stems)",
        description: "Meta's Hybrid Transformer Demucs v4: drums, bass, other, vocals. Not hosted \
                      yet - export it with tools/export_htdemucs_onnx.py and put htdemucs.onnx \
                      in the models folder.",
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
        url: our_url!("htdemucs.onnx"),
        sha256: None,
        // Approximate (fp32 export of the 42M-parameter model).
        size_bytes: 170_000_000,
        license: "MIT (facebookresearch/demucs code and weights)",
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
        sha256: None,
        // Approximate.
        size_bytes: 110_000_000,
        license: "MIT (facebookresearch/demucs code and weights)",
    },
];

/// Every model the app offers.
pub fn registry() -> Vec<ModelInfo> {
    ENTRIES.iter().map(to_info).collect()
}

/// Look a model up by [`ModelInfo::id`].
pub fn find_model(id: &str) -> Option<ModelInfo> {
    ENTRIES.iter().find(|e| e.id == id).map(to_info)
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
            }
        }
        assert_eq!(
            find_model("mdx-kim-vocal-2").unwrap().stems[0],
            StemKind::Vocals
        );
        assert!(find_model("nope").is_none());
    }
}
