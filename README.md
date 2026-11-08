# stem-splitter

Grab audio from YouTube, X/Twitter, SoundCloud or any direct link and split it into stems with AI:
vocals, drums, bass, instruments, and more. Built in Rust with an egui desktop UI, GPU-accelerated on
Windows through DirectML (works on AMD, NVIDIA and Intel GPUs).

Made for sampling, beat-making and remixes. Only download material you have the right to use.

Status: early development. See [docs/PLAN.md](docs/PLAN.md).

## Build

```
cargo run --release -p stem-app
```

## License

MIT. Model weights are downloaded at runtime and carry their own licenses (shown in the app).
