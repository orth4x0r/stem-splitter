//! Headless companion to the GUI: split a file or URL from the command line.
//!
//! ```text
//! stem-cli list                       # models and GPUs
//! stem-cli split <file-or-url> [--model mdx-inst-hq3] [--device auto|cpu|<n>] [--out DIR]
//! ```
//!
//! Shares the GUI's data folder, so models and the ONNX runtime are downloaded once for both.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use stem_audio::WavFormat;
use stem_fetch::{FetchProgress, Fetcher};
use stem_models::{Device, ModelManager, Progress};

fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("com", "orth4x0r", "Stem Splitter")
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("stem-splitter"))
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  stem-cli list\n  stem-cli split <file-or-url> [--model ID] [--device auto|cpu|N] [--out DIR]"
    );
    std::process::exit(2)
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("list") => list(),
        Some("split") => split(&args[1..]),
        _ => usage(),
    }
}

fn list() -> Result<()> {
    let manager = ModelManager::new(data_dir());
    println!("Models (data folder: {}):", data_dir().display());
    for m in stem_models::registry() {
        let stems: Vec<_> = m.stems.iter().map(|s| s.label()).collect();
        let have = if manager.is_downloaded(&m) {
            "downloaded"
        } else {
            ""
        };
        println!(
            "  {:<16} {:<28} [{}] {}",
            m.id,
            m.name,
            stems.join(", "),
            have
        );
    }
    println!("Devices:");
    for (d, name) in stem_models::list_devices() {
        println!("  {d:?}: {name}");
    }
    Ok(())
}

fn split(args: &[String]) -> Result<()> {
    let mut input = None;
    let mut model_id = "mdx-inst-hq3".to_string();
    let mut device = Device::Auto;
    let mut out = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--model" => model_id = it.next().cloned().unwrap_or_else(|| usage()),
            "--device" => {
                device = match it.next().map(String::as_str) {
                    Some("auto") => Device::Auto,
                    Some("cpu") => Device::Cpu,
                    Some(n) => Device::DirectMl(
                        n.parse()
                            .context("--device expects auto, cpu or a number")?,
                    ),
                    None => usage(),
                }
            }
            "--out" => out = Some(PathBuf::from(it.next().cloned().unwrap_or_else(|| usage()))),
            _ if input.is_none() => input = Some(a.clone()),
            _ => usage(),
        }
    }
    let Some(input) = input else { usage() };
    let Some(info) = stem_models::find_model(&model_id) else {
        bail!("unknown model {model_id:?}; run `stem-cli list`")
    };

    let data = data_dir();
    let cancel = AtomicBool::new(false);
    let t0 = Instant::now();

    // Fetch.
    let fetcher = Fetcher::new(data.join("tools"), data.join("downloads"));
    let mut last = -1i32;
    let fetched = fetcher
        .fetch(
            &input,
            &mut |p| match p {
                FetchProgress::Preparing(s) => eprintln!("preparing: {s}"),
                FetchProgress::Downloading {
                    fraction: Some(f), ..
                } => {
                    let pct = (f * 100.0) as i32;
                    if pct / 10 != last / 10 {
                        eprintln!("downloading {pct}%");
                        last = pct;
                    }
                }
                FetchProgress::Downloading { .. } => {}
                FetchProgress::Converting => eprintln!("converting"),
            },
            &cancel,
        )
        .context("fetch failed")?;
    eprintln!("got {:?} ({})", fetched.title, fetched.path.display());

    // Decode.
    let t = Instant::now();
    let mix = stem_audio::decode_file(&fetched.path).context("decode failed")?;
    eprintln!(
        "decoded {:.1}s of audio, {} ch @ {} Hz in {:.2}s",
        mix.duration_secs(),
        mix.n_channels(),
        mix.sample_rate,
        t.elapsed().as_secs_f32()
    );

    // Runtime + model.
    let manager = ModelManager::new(data.clone());
    let mut last = -1i32;
    let mut report = |p: Progress| {
        if let Progress::Downloading { fraction: Some(f) } = p {
            let pct = (f * 100.0) as i32;
            if pct / 10 != last / 10 {
                eprintln!("downloading {pct}%");
                last = pct;
            }
        }
    };
    manager
        .ensure_runtime(&mut report)
        .context("onnx runtime setup failed")?;
    let model_path = manager
        .download(&info, &mut report, &cancel)
        .context("model download failed")?;

    let t = Instant::now();
    let mut sep = stem_models::load(&info, &model_path, device).context("model load failed")?;
    eprintln!(
        "loaded {} on {:?} in {:.2}s",
        info.name,
        sep.active_device(),
        t.elapsed().as_secs_f32()
    );

    // Separate.
    let t = Instant::now();
    let mut last_pct = -1i32;
    let stems = sep
        .separate(
            &mix,
            &mut |p| match p {
                Progress::Separating(f) => {
                    let pct = (f * 100.0) as i32;
                    if pct / 10 != last_pct / 10 {
                        eprintln!("separating {pct}%");
                        last_pct = pct;
                    }
                }
                Progress::RetryingOnCpu(why) => {
                    eprintln!("{why}; retrying on the CPU");
                    last_pct = -1;
                }
                _ => {}
            },
            &cancel,
        )
        .context("separation failed")?;
    let secs = t.elapsed().as_secs_f32();
    eprintln!(
        "separated in {secs:.2}s ({:.1}x realtime)",
        mix.duration_secs() as f32 / secs.max(1e-3)
    );

    // Write.
    let out = out
        .unwrap_or_else(|| PathBuf::from(".").join(stem_fetch::sanitize_file_stem(&fetched.title)));
    std::fs::create_dir_all(&out)?;
    for (kind, buf) in &stems {
        let path = out.join(format!(
            "{}.wav",
            kind.label().to_lowercase().replace(' ', "_")
        ));
        stem_audio::write_wav(&path, buf, WavFormat::Pcm24)?;
        println!("{}", path.display());
    }
    eprintln!("done in {:.1}s total", t0.elapsed().as_secs_f32());
    Ok(())
}
