# stem-splitter

Grab audio from YouTube, X/Twitter, SoundCloud or any direct link and split it into stems with AI:
vocals, drums, bass, instruments, and more. Built in Rust with an egui desktop UI, GPU-accelerated on
Windows through DirectML (works on AMD, NVIDIA and Intel GPUs).

Made for sampling, beat-making and remixes. Only download material you have the right to use.

Status: early development. See [docs/PLAN.md](docs/PLAN.md).

## Features

- **Get audio**: paste a link (YouTube, X, SoundCloud, playlists, direct files), drop a file, or
  type words into the same box to search YouTube or SoundCloud and split or queue a result.
  yt-dlp and ffmpeg are downloaded for you.
- **Split**: MDX-Net, HTDemucs (4 and 6 stems), Mel-/BS-RoFormer, SCNet and MDX23C models on ONNX
  Runtime. Chain models (e.g. RoFormer vocals, then HTDemucs on the instrumental) or ensemble them.
  Built-in presets include Beatmaker 4-stem, Lead / backing vocals, Drums to kit pieces and Dry
  vocals. A Fast / Balanced / Best quality setting trades speed for fewer seams.
- **GPU**: DirectML on Windows with a CPU fallback. If a model fails or produces broken output on
  the GPU, the step is rerun on the CPU and that model skips the GPU for 7 days (you can reset this in Settings).
- **Analyse**: BPM, key with Camelot code, a beat grid you can halve, double, nudge or re-anchor
  to bar 1, song sections with repeats lettered (A B A C ...) and the likely chorus marked, and a
  chord lane.
- **Sample**: pick a section before splitting, chop on transients or the grid, play pads, time
  stretch, render one-shots and kits, extract MIDI (Basic Pitch), and drag stems straight into a
  DAW. Exports carry tempo metadata (acid / smpl chunks).

## Build

```
cargo run --release -p stem-app
```

## License

MIT. Model weights are downloaded at runtime and carry their own licenses (shown in the app).
