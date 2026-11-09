//! Basic Pitch through ONNX Runtime with the real `nmp.onnx`, against basic-pitch's own Python
//! `predict()` on the same audio.
//!
//! Skips (passes with a note) unless `ORT_DYLIB_PATH` points at an onnxruntime >= 1.22 library
//! and `python3 tools/basic_pitch_reference.py fixtures crates/stem-models/tests/fixtures/basic_pitch`
//! has been run (it writes the audio, the model copy and the reference outputs).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use stem_models::transcribe::{self, BasicPitch, NoteOptions};
use stem_models::{Device, ModelManager};

fn fixtures() -> Option<PathBuf> {
    if std::env::var_os("ORT_DYLIB_PATH").is_none() {
        eprintln!("skipped: ORT_DYLIB_PATH not set");
        return None;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic_pitch");
    if !dir.join("ref.json").is_file() || !dir.join("nmp.onnx").is_file() {
        eprintln!("skipped: run tools/basic_pitch_reference.py fixtures first");
        return None;
    }
    ModelManager::new(std::env::temp_dir())
        .ensure_runtime(&mut |_| {})
        .expect("load onnxruntime");
    Some(dir)
}

fn read_f32(path: &Path) -> Vec<f32> {
    std::fs::read(path)
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

#[test]
fn basic_pitch_matches_python() {
    let Some(dir) = fixtures() else { return };
    let audio = stem_audio::decode_file(&dir.join("melody.wav")).unwrap();
    assert_eq!(audio.sample_rate, transcribe::SAMPLE_RATE);
    let mut model = BasicPitch::load(&dir.join("nmp.onnx"), Device::Cpu).unwrap();
    let mut last = 0.0;
    let post = model
        .transcribe(&audio, &mut |f| last = f, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(last, 1.0);

    let reference: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("ref.json")).unwrap()).unwrap();
    assert_eq!(
        post.n_frames as u64,
        reference["n_frames"].as_u64().unwrap()
    );
    for (name, ours) in [
        ("note", &post.note),
        ("onset", &post.onset),
        ("contour", &post.contour),
    ] {
        let theirs = read_f32(&dir.join(format!("{name}.f32")));
        assert_eq!(ours.len(), theirs.len(), "{name}");
        let max = ours
            .iter()
            .zip(&theirs)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        eprintln!("{name}: max |diff| = {max:e}");
        assert!(max < 1e-4, "{name}: {max}");
    }

    let notes = transcribe::notes(&post, &NoteOptions::default());
    let want = reference["notes"].as_array().unwrap();
    eprintln!("{} notes (python {})", notes.len(), want.len());
    assert_eq!(notes.len(), want.len());
    let mut max_amp = 0.0f64;
    for (i, (n, w)) in notes.iter().zip(want).enumerate() {
        assert_eq!(n.pitch as u64, w["pitch"].as_u64().unwrap(), "note {i}");
        assert!(
            (n.start - w["start"].as_f64().unwrap()).abs() < 1e-9,
            "note {i}"
        );
        assert!(
            (n.end - w["end"].as_f64().unwrap()).abs() < 1e-9,
            "note {i}"
        );
        max_amp = max_amp.max((n.amplitude as f64 - w["amplitude"].as_f64().unwrap()).abs());
        let bends: Vec<i32> = w["bends"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_i64().unwrap() as i32)
            .collect();
        assert_eq!(n.bends.as_deref(), Some(bends.as_slice()), "note {i} bends");
    }
    eprintln!("amplitude: max |diff| = {max_amp:e}");
    assert!(max_amp < 1e-4);
}
