//! MSST architectures through ONNX Runtime against PyTorch.
//!
//! `tools/export_mdx23c_onnx.py selftest` and `tools/export_mel_roformer_onnx.py selftest` build
//! tiny random-weight MDX23C / Mel-Band RoFormer models with the real MSST code, export them
//! with the same wrappers as the real checkpoints, and separate a short mix with the
//! *original* PyTorch forward (STFT and iSTFT inside the model) chunked the way stem-models
//! chunks. This test runs the exported graphs through our STFT, crop / pad, chunking and
//! overlap-add, and checks the stems match.
//!
//! ```text
//! python tools/export_mdx23c_onnx.py selftest crates/stem-models/tests/fixtures/
//! python tools/export_mel_roformer_onnx.py selftest crates/stem-models/tests/fixtures/
//! ORT_DYLIB_PATH=/path/to/libonnxruntime.so cargo test -p stem-models --test msst_reference
//! ```
//!
//! Skips (passes with a note) without `ORT_DYLIB_PATH` or the fixtures.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use stem_audio::AudioBuffer;
use stem_models::{Device, ModelArch, ModelManager, StemKind};

fn fixture(name: &str) -> Option<(PathBuf, serde_json::Value)> {
    if std::env::var_os("ORT_DYLIB_PATH").is_none() {
        eprintln!("skipped: ORT_DYLIB_PATH not set");
        return None;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let json = dir.join(format!("{name}.json"));
    let Ok(text) = std::fs::read_to_string(&json) else {
        eprintln!(
            "skipped: no {} (run the export script's selftest)",
            json.display()
        );
        return None;
    };
    ModelManager::new(std::env::temp_dir())
        .ensure_runtime(&mut |_| {})
        .expect("load onnxruntime");
    Some((dir, serde_json::from_str(&text).expect("fixture json")))
}

fn read_f32(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

fn usize_of(v: &serde_json::Value, key: &str) -> usize {
    v[key].as_u64().unwrap_or_else(|| panic!("{key}")) as usize
}

/// Separate `<name>_mix.f32` with the graph and compare with `<name>_stems.f32`.
fn compare(name: &str, model_id: &str, arch: ModelArch, kinds: Vec<StemKind>, dir: &Path) {
    let mix = read_f32(&dir.join(format!("{name}_mix.f32")));
    let want = read_f32(&dir.join(format!("{name}_stems.f32")));
    let n = mix.len() / 2;
    let input = AudioBuffer::new(44_100, vec![mix[..n].to_vec(), mix[n..].to_vec()]);
    let mut info = stem_models::find_model(model_id).expect("registry entry");
    info.arch = arch;
    info.stems = kinds;
    let mut sep = stem_models::load(&info, &dir.join(format!("{name}.onnx")), Device::Cpu)
        .expect("load graph");
    let stems = sep
        .separate(&input, &mut |_| {}, &AtomicBool::new(false))
        .expect("separate");
    let predicted = want.len() / (2 * n);
    assert!(stems.len() >= predicted);
    let peak = want.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    for (s, (kind, buf)) in stems.iter().take(predicted).enumerate() {
        for c in 0..2 {
            let w = &want[(s * 2 + c) * n..(s * 2 + c + 1) * n];
            let err = buf.channels[c]
                .iter()
                .zip(w)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            eprintln!("{name} {kind:?} ch {c}: max err {err:.2e} (peak {peak:.3})");
            assert!(err < 1e-3 * peak.max(0.01), "{kind:?} ch {c}: {err}");
        }
    }
    // A residual stem, if the registry lists one more than the graph predicts.
    if stems.len() > predicted {
        let rest = &stems[predicted].1;
        for c in 0..2 {
            let x = &input.channels[c];
            let err = (0..n)
                .map(|i| {
                    let sum: f32 = (0..predicted)
                        .map(|s| want[(s * 2 + c) * n + i])
                        .sum::<f32>();
                    (rest.channels[c][i] - (x[i] - sum)).abs()
                })
                .fold(0.0f32, f32::max);
            assert!(err < 2e-3 * peak.max(0.01), "residual ch {c}: {err}");
        }
    }
}

#[test]
fn mdx23c_matches_pytorch() {
    let Some((dir, p)) = fixture("mdx23c_tiny") else {
        return;
    };
    let arch = ModelArch::Mdx23c {
        n_fft: usize_of(&p, "n_fft"),
        hop: usize_of(&p, "hop"),
        dim_f: usize_of(&p, "dim_f"),
        segment_samples: usize_of(&p, "segment_samples"),
        overlap: p["overlap"].as_f64().unwrap() as f32,
    };
    let kinds = StemKind::DRUM_PARTS[..usize_of(&p, "stems")].to_vec();
    compare("mdx23c_tiny", "drumsep-mdx23c", arch, kinds, &dir);
}

#[test]
fn mel_roformer_matches_pytorch() {
    let Some((dir, p)) = fixture("mel_roformer_tiny") else {
        return;
    };
    let arch = ModelArch::RoFormer {
        n_fft: usize_of(&p, "n_fft"),
        hop: usize_of(&p, "hop"),
        segment_samples: usize_of(&p, "segment_samples"),
        overlap: p["overlap"].as_f64().unwrap() as f32,
    };
    // One predicted stem plus the residual, like the denoise / de-reverb exports.
    compare(
        "mel_roformer_tiny",
        "mel-roformer-denoise",
        arch,
        vec![StemKind::Clean, StemKind::Noise],
        &dir,
    );
}
