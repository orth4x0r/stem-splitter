# Third-party code and models

| What | From | License |
|---|---|---|

| MDX-Net inference (chunking, padding, Hann overlap-add, STFT packing, `compensate`) ported to `crates/stem-models/src/mdx.rs` and `tools/mdx_reference.py` | [python-audio-separator](https://github.com/nomadkaraoke/python-audio-separator) `mdx_separator.py`, from [UVR](https://github.com/Anjok07/ultimatevocalremovergui) | MIT |
| MDX model parameters (`compensate`, `dim_f`, `n_fft`, primary stem) in `crates/stem-models/src/catalog.rs` | UVR `model_data.json` ([TRvlvr/application_data](https://github.com/TRvlvr/application_data)) | MIT |
| HTDemucs `_spec` / `_ispec` / `_mask` / `apply_model` split logic (Rust port in `crates/stem-models/src/demucs.rs`), `HTDemucs.forward` body copied into `tools/export_htdemucs_onnx.py` | [facebookresearch/demucs](https://github.com/facebookresearch/demucs) | MIT |
| Approach of exporting HTDemucs with the STFT outside the ONNX graph | [sevagh/demucs.onnx](https://github.com/sevagh/demucs.onnx) | MIT |
| UVR MDX-Net ONNX models (downloaded at runtime, not bundled) | [TRvlvr/model_repo](https://github.com/TRvlvr/model_repo) | per registry `license` field |
| HTDemucs weights as a prebuilt ONNX (downloaded at runtime, not bundled) | [Ghilda/htdemucs-onnx](https://huggingface.co/Ghilda/htdemucs-onnx), weights from facebookresearch/demucs | export MIT; weights published with MIT code, but the demucs author has since called them research-only (demucs #327, #508) |
| HTDemucs 6s weights (exported by the user, not bundled) | facebookresearch/demucs | as above |
| Complex-spectrogram segmenting (`demix()`: segments, linear fades, edge reflection, standardisation) ported to `crates/stem-models/src/roformer.rs` | [ZFTurbo/Music-Source-Separation-Training](https://github.com/ZFTurbo/Music-Source-Separation-Training) `utils/model_utils.py`, via the JS translation in [@audio/neural-separate](https://www.npmjs.com/package/@audio/neural-separate) 0.7.0 `separate.js` (`separateComplex`, `MEL` / `SCNET` presets) | MIT (both) |
| Mel-Band RoFormer vocals ONNX (downloaded at runtime, not bundled) | [audiojs/mel-roformer](https://huggingface.co/audiojs/mel-roformer), weights by Kimberley Jensen | MIT |
| BS-RoFormer 4-stem ONNX (downloaded at runtime, not bundled) | [silverdaw/bs-roformer-rhythm-onnx](https://huggingface.co/silverdaw/bs-roformer-rhythm-onnx), weights by ZFTurbo (MSST v1.0.12) | MIT |
| SCNet / SCNet-large int8 ONNX (downloaded at runtime, not bundled) | [audiojs/scnet](https://huggingface.co/audiojs/scnet), [audiojs/scnet-large](https://huggingface.co/audiojs/scnet-large), weights by starrytong/SCNet | MIT |
| ONNX Runtime DirectML build (downloaded at runtime from NuGet, not bundled) | Microsoft.ML.OnnxRuntime.DirectML | MIT |
| DirectML.dll (downloaded at runtime from NuGet, not bundled) | Microsoft.AI.DirectML | Microsoft Software License Terms (DirectML redistributable) |
| Inter font (Regular, SemiBold, ExtraBold), embedded in `crates/stem-app/assets/fonts/` (converted from the npm `inter-ui` 4.1.1 woff2 to TTF) | [rsms/inter](https://rsms.me/inter/) | SIL OFL 1.1 (`Inter-OFL.txt`) |
| JetBrains Mono font (Medium), embedded in `crates/stem-app/assets/fonts/` (converted from the npm `jetbrains-mono` 1.0.6 woff2 to TTF) | JetBrains Mono | SIL OFL 1.1 (`JetBrainsMono-OFL.txt`) |
| FLAC encoder (dependency, `crates/stem-audio`) | [flacenc](https://github.com/yotarok/flacenc-rs) | Apache-2.0 |
| MP3 CBR encoder (dependency, `crates/stem-audio`) | [audio2mp3-core](https://github.com/reynierlabaut/audio2mp3-core) | MIT OR Apache-2.0 |
| Windows drag-out approach (shell item array -> `BHID_DataObject`, OLE drag) in `crates/stem-app/src/dragout.rs` | [drag](https://github.com/crabnebula-dev/drag-rs) by CrabNebula | MIT OR Apache-2.0 |
| Beat tracking by dynamic programming in `crates/stem-audio/src/analysis.rs` (algorithm reimplemented from the paper, no code copied) | D. P. W. Ellis, "Beat Tracking by Dynamic Programming", J. New Music Research 36(1), 2007 | n/a (published algorithm) |
| Key profiles in `crates/stem-audio/src/analysis.rs` (12-value tables) | Krumhansl & Kessler probe-tone ratings (Krumhansl 1990); Temperley's Kostka-Payne profiles (Temperley 2007) | n/a (published research data) |
| Basic Pitch inference windowing / unwrapping (`run_inference`, `get_audio_input`, `unwrap_output`) and note creation (`output_to_notes_polyphonic`, `get_infered_onsets`, `constrain_frequency`, `get_pitch_bends`, `model_frames_to_time`, `drop_overlapping_pitch_bends`, pitch-bend tick mapping) ported to `crates/stem-models/src/basic_pitch.rs` and `midi_file.rs`; the Python reference in `tools/basic_pitch_reference.py` calls the original | [spotify/basic-pitch](https://github.com/spotify/basic-pitch) 0.4.0 `inference.py`, `note_creation.py`, Copyright 2022 Spotify AB | Apache-2.0 |
| Basic Pitch `nmp.onnx` (ICASSP 2022 model, downloaded at runtime from the v0.4.0 tag, not bundled; sha256 `2c3c1d14…a0ec`) | [spotify/basic-pitch](https://github.com/spotify/basic-pitch) `basic_pitch/saved_models/icassp_2022/nmp.onnx` (weights ship in the Apache-2.0 repo, no separate model license) | Apache-2.0 |
| Standard MIDI file reading / writing (dependency, `crates/stem-models`) | [midly](https://github.com/kovaxis/midly) | Unlicense |
