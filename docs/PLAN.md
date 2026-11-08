# stem-splitter: plan

A Rust tool that grabs audio from a URL (or a local file) and uses AI models to split it into stems: vocals, drums, bass, other instruments, and for non-music audio, dialogue / music / effects or a sound you describe.

Status: draft for discussion. Nothing is built yet.

## What it does

```
stem-splitter split https://example.com/song.wav --stems 4 --out ./stems
  -> ./stems/song/vocals.wav, drums.wav, bass.wav, other.wav
```

1. **Fetch**: download the file (direct link) or pull audio from a media page (via yt-dlp).
2. **Decode**: any common format (wav, flac, mp3, aac/m4a, ogg) to float PCM, resampled to the model's rate (usually 44.1 kHz stereo).
3. **Separate**: run an AI model over the audio in overlapping chunks, stitch the chunks back together.
4. **Write**: one WAV (or FLAC) per stem, plus an optional "instrumental" (everything minus vocals).

## Architecture

Cargo workspace, four crates so the core can later back a GUI or a server:

| Crate | Job |
|---|---|
| `stem-fetch` | URL to local file. `reqwest` for direct links; shells out to `yt-dlp` for media pages (it's the only sane way to keep up with site changes). |
| `stem-audio` | Decode (`symphonia`), resample (`rubato`), write (`hound` for WAV, `flacenc` optional), STFT/iSTFT (`realfft`). |
| `stem-models` | A `Separator` trait plus backends. Model registry, downloads weights from Hugging Face (`hf-hub`) into a local cache, chunking + overlap-add. |
| `stem-cli` | `clap` CLI, progress bars (`indicatif`), config. |

```
URL ──stem-fetch──> file ──stem-audio──> PCM 44.1k ──stem-models──> stems PCM ──stem-audio──> stem WAVs
```

### Inference backend

**Primary: ONNX Runtime via the `ort` crate.** Most of the useful open separation models already exist as ONNX or export cleanly, and `ort` gives GPU acceleration for free (CUDA/TensorRT on NVIDIA, CoreML on Mac, DirectML on Windows) with a CPU fallback.

**Later, optional: `candle`.** Pure Rust, no C++ runtime to ship. Worth it only if packaging ONNX Runtime becomes a pain, since it means porting each model architecture by hand.

The STFT lives in Rust, outside the model graph. Models like HTDemucs and RoFormer use complex STFTs that export poorly to ONNX, so the usual trick is to export just the network and do the spectrogram math ourselves.

### Models (candidates)

| Use | Model | Notes |
|---|---|---|
| Vocals / instrumental (2 stems) | MDX-Net family (UVR models) | Already ONNX, fast. Best first target. |
| 4 stems (vocals, drums, bass, other) | HTDemucs | The standard. Needs the STFT-outside export. |
| 6 stems (+ guitar, piano) | HTDemucs 6s | Same pipeline as above. |
| Highest quality vocals | BS-RoFormer / Mel-RoFormer | State of the art, heavier. |
| Film / video audio | Dialogue / music / effects models (DnR-trained) | Covers "environmental sound". |
| "Any sound you describe" | Text-queried separation (AudioSep-style) | Research-grade, last milestone. |

Licenses vary per model and need checking before we bundle or auto-download any of them.

## Milestones

- **M0, skeleton**: workspace, CI (fmt, clippy, test), README.
- **M1, audio I/O**: decode any format, resample, write WAV. Round-trip tests.
- **M2, fetch**: direct-link download with progress and retry; yt-dlp integration for pages.
- **M3, first AI split**: 2-stem vocals/instrumental with an MDX-Net ONNX model on CPU. End-to-end CLI works.
- **M4, 4/6 stems**: HTDemucs with Rust-side STFT, chunking with overlap-add, GPU execution providers.
- **M5, quality**: RoFormer models, ensemble/averaging options, model selection flag.
- **M6, beyond music**: dialogue/music/effects separation, then text-queried "extract this sound".
- **M7, polish**: batch mode, cached model manager, maybe a GUI (egui or Tauri).

## Open questions for you

1. **Repo**: name `stem-splitter`, private? (I can't create GitHub repos from here, so you'd click "New repository" and I take it from there.)
2. **Your machine**: OS and GPU? Decides which execution provider we test first.
3. **Interface**: CLI first is the plan. Do you want a GUI eventually?
4. **Sources**: direct WAV links only, or sites like YouTube/SoundCloud/Bandcamp too?
5. **Priority stems**: vocals/instrumental, full band split, or environmental/film audio first?

A note on downloading: pulling audio from sites is fine for your own or freely licensed material; the tool won't try to get around DRM.
