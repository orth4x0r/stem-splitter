//! End-to-end through ONNX Runtime with synthetic graphs (see tools/make_test_fixtures.py).
//!
//! Skips (passes with a note) unless `ORT_DYLIB_PATH` points at an onnxruntime >= 1.22 library
//! and the fixtures exist. Optionally `STEM_TEST_REAL_MDX=/path/Kim_Vocal_2.onnx` runs a real
//! UVR model over a few seconds of synthetic audio as a smoke test.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use stem_audio::AudioBuffer;
use stem_models::{Device, ModelArch, ModelInfo, ModelManager, Progress, StemKind};

fn fixtures() -> Option<PathBuf> {
    if std::env::var_os("ORT_DYLIB_PATH").is_none() {
        eprintln!("skipped: ORT_DYLIB_PATH not set");
        return None;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    if !dir.join("mdx_half.onnx").is_file() {
        eprintln!("skipped: run tools/make_test_fixtures.py first");
        return None;
    }
    // Uses ORT_DYLIB_PATH; the root is unused then.
    ModelManager::new(std::env::temp_dir())
        .ensure_runtime(&mut |_| {})
        .expect("load onnxruntime");
    Some(dir)
}

fn tones(n: usize, rate: f32) -> AudioBuffer {
    let s = |f: f32, a: f32, i: usize| a * (2.0 * std::f32::consts::PI * f * i as f32 / rate).sin();
    AudioBuffer::new(
        rate as u32,
        vec![
            (0..n)
                .map(|i| s(440.0, 0.4, i) + s(2000.0, 0.2, i))
                .collect(),
            (0..n)
                .map(|i| s(660.0, 0.3, i) + s(3000.0, 0.1, i))
                .collect(),
        ],
    )
}

fn max_err(a: &[f32], b: &[f32], g: f32, margin: usize) -> f32 {
    let n = b.len();
    a[margin..n - margin]
        .iter()
        .zip(&b[margin..n - margin])
        .map(|(x, y)| (x - g * y).abs())
        .fold(0.0, f32::max)
}

#[test]
fn mdx_through_onnxruntime() {
    let Some(dir) = fixtures() else { return };
    let info = ModelInfo {
        id: "test-mdx",
        name: "test",
        description: "",
        stems: vec![StemKind::Vocals, StemKind::Instrumental],
        sample_rate: 44_100,
        arch: ModelArch::MdxNet {
            n_fft: 1024,
            hop: 256,
            dim_f: 400,
            dim_t: 64,
            compensate: 1.2,
        },
        url: "https://example.invalid/mdx_half.onnx",
        sha256: None,
        size_bytes: 0,
        license: "test",
        non_commercial: false,
    };
    let mut sep = stem_models::load(&info, &dir.join("mdx_half.onnx"), Device::Auto).unwrap();
    if !cfg!(windows) {
        assert_eq!(sep.active_device(), Device::Cpu);
    }
    let mix = tones(100_000, 44_100.0);
    let mut seen = vec![];
    let stems = sep
        .separate(&mix, &mut |p| seen.push(p), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(seen.last(), Some(&Progress::Separating(1.0)));
    assert_eq!(stems.len(), 2);
    for c in 0..2 {
        let m = &mix.channels[c];
        let p = &stems[0].1.channels[c];
        let s = &stems[1].1.channels[c];
        assert_eq!(p.len(), m.len());
        let e = max_err(p, m, 0.6, 1024);
        assert!(e < 1e-3, "primary err {e}");
        let e = max_err(s, m, 0.4, 1024);
        assert!(e < 1e-3, "secondary err {e}");
    }

    // A 48 kHz input goes through the resampler both ways and comes back at 48 kHz.
    let mix48 = tones(48_000, 48_000.0);
    let stems = sep
        .separate(&mix48, &mut |_| {}, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(stems[0].1.sample_rate, 48_000);
    assert_eq!(stems[0].1.frames(), 48_000);
    let e = max_err(&stems[0].1.channels[0], &mix48.channels[0], 0.6, 4096);
    assert!(e < 1e-2, "resampled primary err {e}");

    // Cancel is honoured.
    let r = sep.separate(&mix, &mut |_| {}, &AtomicBool::new(true));
    assert!(matches!(r, Err(stem_models::ModelError::Cancelled)));
}

#[test]
fn htdemucs_through_onnxruntime() {
    let Some(dir) = fixtures() else { return };
    let info = ModelInfo {
        id: "test-demucs",
        name: "test",
        description: "",
        stems: vec![
            StemKind::Drums,
            StemKind::Bass,
            StemKind::Other,
            StemKind::Vocals,
        ],
        sample_rate: 44_100,
        arch: ModelArch::HtDemucs {
            segment_samples: 30_000,
        },
        url: "https://example.invalid/htdemucs_linear.onnx",
        sha256: None,
        size_bytes: 0,
        license: "test",
        non_commercial: false,
    };
    let mut sep = stem_models::load(&info, &dir.join("htdemucs_linear.onnx"), Device::Cpu).unwrap();
    let mix = tones(80_000, 44_100.0);
    let stems = sep
        .separate(&mix, &mut |_| {}, &AtomicBool::new(false))
        .unwrap();
    let gains = [1.0f32, 1.0, 0.25, 1.0]; // a + b from the fixture
    assert_eq!(stems.len(), 4);
    for (s, (kind, buf)) in stems.iter().enumerate() {
        assert_eq!(*kind, info.stems[s]);
        for c in 0..2 {
            // Up to ~1% near chunk edges (demucs' _ispec edge frames), see demucs.rs tests.
            let e = max_err(&buf.channels[c], &mix.channels[c], gains[s], 2048);
            assert!(e < 1e-2, "source {s} ch {c}: {e}");
        }
    }
}

/// Run a registry model's own parameters over a synthetic spectrogram graph with the real
/// export's I/O names and shapes; each stem must come back as `gains[s] * mix`.
fn spec_model_through_onnxruntime(id: &str, fixture: &str, gains: &[f32]) {
    let Some(dir) = fixtures() else { return };
    let path = dir.join(fixture);
    if !path.is_file() {
        eprintln!("skipped: {fixture} missing, rerun tools/make_test_fixtures.py");
        return;
    }
    let info = stem_models::find_model(id).unwrap();
    let mut sep = stem_models::load(&info, &path, Device::Cpu).unwrap();
    // 20 s: long enough for the reflected borders and several overlapping segments. The tones
    // are zero-mean, so SCNet's standardisation (which drops the mean) changes nothing.
    let mix = tones(20 * 44_100, 44_100.0);
    let mut seen = vec![];
    let t = std::time::Instant::now();
    let stems = sep
        .separate(&mix, &mut |p| seen.push(p), &AtomicBool::new(false))
        .unwrap();
    eprintln!(
        "{id}: 20 s through the stand-in graph in {:.2?}",
        t.elapsed()
    );
    assert_eq!(seen.last(), Some(&Progress::Separating(1.0)));
    assert_eq!(stems.len(), info.stems.len());
    for (s, (kind, buf)) in stems.iter().enumerate() {
        assert_eq!(*kind, info.stems[s]);
        for c in 0..2 {
            assert_eq!(buf.channels[c].len(), mix.frames());
            let e = max_err(&buf.channels[c], &mix.channels[c], gains[s], 0);
            assert!(e < 1e-3, "{id} source {s} ch {c}: {e}");
        }
    }
}

#[test]
fn mel_roformer_through_onnxruntime() {
    // A mask of 1: vocals = mix, and the instrumental (the residual) = 0.
    spec_model_through_onnxruntime(
        "mel-roformer-kim",
        "mel_roformer_identity.onnx",
        &[1.0, 0.0],
    );
}

#[test]
fn bs_roformer_through_onnxruntime() {
    let gains = [1.0, 0.5, 0.25, 0.0]; // SPEC_GAINS in the fixture script
    spec_model_through_onnxruntime("bs-roformer-4stem", "bs_roformer_gains.onnx", &gains);
}

#[test]
fn scnet_through_onnxruntime() {
    let gains = [1.0, 0.5, 0.25, 0.0];
    spec_model_through_onnxruntime("scnet", "scnet_gains.onnx", &gains);
}

#[test]
fn real_mdx_model_smoke() {
    let Some(path) = std::env::var_os("STEM_TEST_REAL_MDX").map(PathBuf::from) else {
        eprintln!("skipped: STEM_TEST_REAL_MDX not set");
        return;
    };
    if fixtures().is_none() {
        return;
    }
    let file = path.file_name().unwrap().to_string_lossy().to_string();
    let info = stem_models::registry()
        .into_iter()
        .find(|m| m.file_name() == file)
        .expect("file name matches a registry entry");
    let mut sep = stem_models::load(&info, &path, Device::Cpu).unwrap();
    let mix = tones(44_100 * 6, 44_100.0);
    let t = std::time::Instant::now();
    let stems = sep
        .separate(&mix, &mut |_| {}, &AtomicBool::new(false))
        .unwrap();
    eprintln!("{}: 6 s of audio in {:.2?}", info.id, t.elapsed());
    for (k, b) in &stems {
        let rms = (b.channels[0].iter().map(|x| x * x).sum::<f32>() / b.frames() as f32).sqrt();
        eprintln!("  {}: rms {rms:.4}", k.label());
        assert!(b.channels.iter().flatten().all(|x| x.is_finite()));
        assert_eq!(b.frames(), mix.frames());
    }
    // Compare with tools/mdx_reference.py's output for the same signal.
    if let Some(r) = std::env::var_os("STEM_TEST_REAL_MDX_REF") {
        let bytes = std::fs::read(r).unwrap();
        let reference: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        let n = mix.frames();
        assert_eq!(reference.len(), 2 * n);
        let mut worst = 0.0f32;
        let mut peak = 0.0f32;
        for c in 0..2 {
            for (x, y) in stems[0].1.channels[c]
                .iter()
                .zip(&reference[c * n..(c + 1) * n])
            {
                worst = worst.max((x - y).abs());
                peak = peak.max(y.abs());
            }
        }
        eprintln!("  vs python reference: max abs diff {worst:.2e} (reference peak {peak:.3})");
        assert!(worst < 1e-3);
    }
}
