# stem-splitter

Windows-first desktop app (egui) that downloads audio from YouTube / X / SoundCloud / direct links
and splits it into stems with AI models on ONNX Runtime (DirectML GPU on Windows, CPU fallback).
Used for making samples, beats and remixes. Target machine: Windows 11, AMD RX 6700S + Radeon iGPU.

## Layout
- `crates/stem-audio`  AudioBuffer, decode (symphonia), resample (rubato), WAV write (hound), STFT.
- `crates/stem-fetch`  URL -> audio file. reqwest for direct links, yt-dlp (auto-downloaded) for sites.
- `crates/stem-models` model registry, downloads, ONNX Runtime (`ort`, load-dynamic) separators.
- `crates/stem-app`    the egui GUI, binary `stem-splitter`.
- `docs/PLAN.md`       architecture and milestones.

## Rules
- Before committing: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, and `cargo check --workspace --target x86_64-pc-windows-gnu`
  (Windows is the real target; Linux is only where CI/dev boxes run).
- Public APIs between crates live in each crate's `lib.rs`. Change them only when needed and say so
  in the commit message.
- `ort` uses `load-dynamic`: never enable `download-binaries`; the app fetches onnxruntime itself.
- Long work (download, separation) runs off the UI thread, reports progress through callbacks and
  honours an `AtomicBool` cancel flag.
- Copying from other open-source projects is welcome only from MIT / Apache-2.0 / BSD / Zlib code.
  Note the source and license in `THIRD_PARTY.md`. No GPL code.
- Model weights are never committed (`*.onnx` is gitignored). Record each model's license in the
  registry.
