//! Background work. Everything slow runs on worker threads and reports back through one
//! channel of [`AppEvent`]s; the UI thread only polls it.
//!
//! Chains / presets (several models in a row, ensembles) and the stem cache live in the
//! `chain` and `cache` submodules.

pub mod cache;
pub mod chain;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::Sender;
use eframe::egui;
use stem_audio::{AudioBuffer, AudioFormat, WavFormat};
use stem_fetch::{FetchProgress, Fetcher};
use stem_models::{Device, DownloadStatus, ModelInfo, ModelManager, Progress, StemKind};

use crate::backend::guard;
use crate::paths::{AppPaths, sanitize_file_name};
use crate::section::{Origin, SplitRange};
use crate::waveform::{BUCKETS, Envelope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Fetching,
    Decoding,
    PreparingModel,
    Separating,
    Writing,
}

impl Stage {
    pub const ALL: [Stage; 5] = [
        Stage::Fetching,
        Stage::Decoding,
        Stage::PreparingModel,
        Stage::Separating,
        Stage::Writing,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Stage::Fetching => "Fetching",
            Stage::Decoding => "Decoding",
            Stage::PreparingModel => "Loading model",
            Stage::Separating => "Separating",
            Stage::Writing => "Writing stems",
        }
    }
    pub fn index(self) -> usize {
        Stage::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
    /// Share of the total progress bar each stage gets.
    fn weight(self) -> f32 {
        match self {
            Stage::Fetching => 0.12,
            Stage::Decoding => 0.05,
            Stage::PreparingModel => 0.08,
            Stage::Separating => 0.70,
            Stage::Writing => 0.05,
        }
    }
    /// Overall progress in `[0, 1]` given this stage's own fraction.
    pub fn overall(self, frac: f32) -> f32 {
        let before: f32 = Stage::ALL[..self.index()].iter().map(|s| s.weight()).sum();
        before + self.weight() * frac.clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone)]
pub enum InputSource {
    Url(String),
    File(PathBuf),
}

impl InputSource {
    pub fn display(&self) -> String {
        match self {
            InputSource::Url(u) => u.clone(),
            InputSource::File(p) => p.display().to_string(),
        }
    }
    /// Best guess at a title before we know the real one.
    pub fn provisional_title(&self) -> String {
        match self {
            InputSource::File(p) => p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Untitled".into()),
            InputSource::Url(u) => {
                let host = u
                    .split("://")
                    .nth(1)
                    .unwrap_or(u)
                    .split('/')
                    .next()
                    .unwrap_or(u)
                    .trim_start_matches("www.");
                format!("Link from {host}")
            }
        }
    }
}

#[derive(Clone)]
pub struct JobRequest {
    pub id: u64,
    pub source: InputSource,
    pub model: ModelInfo,
    pub device: Device,
    pub output_root: PathBuf,
    pub wav: WavFormat,
    pub cookies_from_browser: Option<String>,
    pub paths: AppPaths,
    pub demo: bool,
    /// Split only this part of the input (pre-split step / "Re-split this selection").
    pub range: Option<SplitRange>,
    /// Title to use instead of guessing one from the source (an already fetched file).
    pub title: Option<String>,
    /// Run a preset's chain (through the stem cache) instead of the single `model`.
    pub chain: Option<chain::ChainSpec>,
}

pub struct StemOut {
    pub kind: StemKind,
    pub audio: AudioBuffer,
    pub path: PathBuf,
    pub env: Envelope,
}

pub struct JobOutput {
    pub title: String,
    pub out_dir: PathBuf,
    pub stems: Vec<StemOut>,
    /// Tempo / beat grid / key, `None` if analysis failed.
    pub analysis: Option<crate::analysis::TrackAnalysis>,
    /// Where the stems came from (source, local input, section), for "Re-split".
    pub origin: Origin,
}

/// Tempo and key detection over the finished stems (well under a second).
fn analyze(stems: &[StemOut]) -> Option<crate::analysis::TrackAnalysis> {
    let refs: Vec<_> = stems.iter().map(|s| (s.kind, &s.audio)).collect();
    guard("analysis", || crate::analysis::analyze_stems(&refs))
        .ok()
        .flatten()
}

pub enum JobEvent {
    Stage(Stage, String),
    /// Fraction of the current stage, `None` = indeterminate.
    Progress(Option<f32>),
    Title(String),
    /// The pre-split step has the decoded input ready for picking a section.
    Prepared(Box<crate::section::Prepared>),
    Done(Box<JobOutput>),
    Failed(String),
    Cancelled,
}

pub enum AppEvent {
    Job(u64, JobEvent),
    ModelProgress(String, DownloadStatus),
    ModelDone(String, Result<(), String>),
    YtDlpUpdated(Result<String, String>),
    SampleExported(Result<PathBuf, String>),
    /// Queue item id, and the playlist's (title, url) entries.
    PlaylistExpanded(u64, Result<Vec<(Option<String>, String)>, String>),
    FfmpegProgress(Option<f32>, String),
    FfmpegDone(Result<PathBuf, String>),
    /// Message and destination folder.
    StemsExported(Result<(String, PathBuf), String>),
}

/// Sends events and wakes the UI.
#[derive(Clone)]
pub struct Notifier {
    tx: Sender<AppEvent>,
    ctx: egui::Context,
}

impl Notifier {
    pub fn new(tx: Sender<AppEvent>, ctx: egui::Context) -> Self {
        Self { tx, ctx }
    }
    pub fn send(&self, ev: AppEvent) {
        let _ = self.tx.send(ev);
        self.ctx.request_repaint();
    }
}

/// Spawn the full split pipeline. Returns immediately.
pub fn spawn_job(req: JobRequest, cancel: Arc<AtomicBool>, n: Notifier) {
    std::thread::Builder::new()
        .name(format!("job-{}", req.id))
        .spawn(move || {
            let id = req.id;
            let send = |ev| n.send(AppEvent::Job(id, ev));
            let result = guard("split", || {
                if let Some(spec) = &req.chain {
                    chain::run(&req, spec, &cancel, &send)
                } else if req.demo {
                    run_demo(&req, &cancel, &send)
                } else {
                    run_real(&req, &cancel, &send)
                }
            });
            match result {
                Ok(Ok(out)) => send(JobEvent::Done(Box::new(out))),
                Ok(Err(_)) if cancel.load(Ordering::Relaxed) => send(JobEvent::Cancelled),
                Ok(Err(e)) => send(JobEvent::Failed(e)),
                Err(e) => send(JobEvent::Failed(e)),
            }
        })
        .expect("spawn job thread");
}

type Emit<'a> = &'a dyn Fn(JobEvent);

fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("cancelled".into())
    } else {
        Ok(())
    }
}

fn model_progress(send: Emit<'_>) -> impl FnMut(Progress) + '_ {
    move |p| match p {
        Progress::Downloading { fraction } => send(JobEvent::Progress(fraction)),
        Progress::LoadingModel => send(JobEvent::Progress(None)),
        Progress::Separating(f) => send(JobEvent::Progress(Some(f))),
    }
}

/// Fetch `req.source` to a local file (a no-op for files). Returns the path and the title.
pub(crate) fn fetch_input(
    req: &JobRequest,
    cancel: &AtomicBool,
    send: Emit<'_>,
) -> Result<(PathBuf, String), String> {
    Ok(match &req.source {
        InputSource::File(p) => (
            p.clone(),
            req.title
                .clone()
                .unwrap_or_else(|| req.source.provisional_title()),
        ),
        InputSource::Url(url) => {
            send(JobEvent::Stage(Stage::Fetching, "Grabbing audio".into()));
            let mut fetcher = Fetcher::new(req.paths.tools.clone(), req.paths.downloads.clone());
            fetcher.cookies_from_browser = req.cookies_from_browser.clone();
            let fetched = fetcher
                .fetch(
                    url,
                    &mut |p| match p {
                        FetchProgress::Preparing(msg) => {
                            send(JobEvent::Stage(Stage::Fetching, msg));
                            send(JobEvent::Progress(None));
                        }
                        FetchProgress::Downloading { fraction, .. } => {
                            send(JobEvent::Progress(fraction))
                        }
                        FetchProgress::Converting => {
                            send(JobEvent::Stage(Stage::Fetching, "Extracting audio".into()));
                            send(JobEvent::Progress(None));
                        }
                    },
                    cancel,
                )
                .map_err(|e| e.to_string())?;
            send(JobEvent::Title(fetched.title.clone()));
            (fetched.path, fetched.title)
        }
    })
}

fn run_real(req: &JobRequest, cancel: &AtomicBool, send: Emit<'_>) -> Result<JobOutput, String> {
    // 1. Fetch
    let (path, title) = fetch_input(req, cancel, send)?;
    check(cancel)?;

    // 2. Decode
    send(JobEvent::Stage(Stage::Decoding, "Reading audio".into()));
    send(JobEvent::Progress(None));
    let mix = stem_audio::decode_file(&path).map_err(|e| e.to_string())?;
    // Only the chosen section (plus a little padding the model sees as context).
    let cut = crate::section::cut(&mix, req.range);
    drop(mix);
    check(cancel)?;

    // 3. Runtime + model
    let mgr = ModelManager::new(req.paths.models.clone());
    send(JobEvent::Stage(
        Stage::PreparingModel,
        "Starting ONNX Runtime".into(),
    ));
    mgr.ensure_runtime(&mut model_progress(send))
        .map_err(|e| e.to_string())?;
    check(cancel)?;
    let model_path = if mgr.is_downloaded(&req.model) {
        mgr.model_path(&req.model)
    } else {
        send(JobEvent::Stage(
            Stage::PreparingModel,
            format!("Downloading {}", req.model.name),
        ));
        mgr.download(&req.model, &mut model_progress(send), cancel)
            .map_err(|e| e.to_string())?
    };
    send(JobEvent::Stage(
        Stage::PreparingModel,
        format!("Loading {}", req.model.name),
    ));
    send(JobEvent::Progress(None));
    let mut sep =
        stem_models::load(&req.model, &model_path, req.device).map_err(|e| e.to_string())?;
    check(cancel)?;

    // 4. Separate
    let on = match sep.active_device() {
        Device::Cpu if req.device != Device::Cpu => "CPU (GPU unavailable)",
        Device::Cpu => "CPU",
        _ => "GPU · DirectML",
    };
    send(JobEvent::Stage(
        Stage::Separating,
        format!("{} on {on}", req.model.name),
    ));
    send(JobEvent::Progress(Some(0.0)));
    let stems = sep
        .separate(&cut.audio, &mut model_progress(send), cancel)
        .map_err(|e| e.to_string())?;
    let stems = cut.trim(stems);
    check(cancel)?;

    // 5. Write
    let mut out = write_stems(req, &title, stems, send)?;
    out.origin.input = Some(path);
    out.origin.save(&out.out_dir);
    Ok(out)
}

fn write_stems(
    req: &JobRequest,
    title: &str,
    stems: Vec<(StemKind, AudioBuffer)>,
    send: Emit<'_>,
) -> Result<JobOutput, String> {
    send(JobEvent::Stage(Stage::Writing, "Saving WAVs".into()));
    let dir_name = crate::section::dir_name(title, req.range);
    let out_dir = unique_dir(&req.output_root.join(sanitize_file_name(&dir_name)));
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let n = stems.len().max(1);
    let mut out = Vec::new();
    for (i, (kind, audio)) in stems.into_iter().enumerate() {
        let path = out_dir.join(format!("{}.wav", kind.label().to_lowercase()));
        stem_audio::write_wav(&path, &audio, req.wav).map_err(|e| e.to_string())?;
        let env = Envelope::from_buffer(&audio, BUCKETS);
        out.push(StemOut {
            kind,
            audio,
            path,
            env,
        });
        send(JobEvent::Progress(Some((i + 1) as f32 / n as f32)));
    }
    send(JobEvent::Stage(
        Stage::Writing,
        "Detecting tempo & key".into(),
    ));
    send(JobEvent::Progress(None));
    let analysis = analyze(&out);
    // Tempo / key in the WAVs themselves, so a stem dragged into a DAW lands in time.
    let meta = crate::export::wav_meta(analysis.as_ref(), false, None);
    if !meta.is_empty() {
        for s in &out {
            let _ = stem_audio::wavmeta::apply_to_file(&s.path, &meta);
        }
    }
    Ok(JobOutput {
        title: title.to_owned(),
        out_dir,
        stems: out,
        analysis,
        origin: Origin {
            source: req.source.display(),
            input: None,
            range: req.range,
            model_name: req.model.name.to_owned(),
        },
    })
}

/// `dir`, or `dir (2)`, `dir (3)`... if it already has files in it.
fn unique_dir(dir: &Path) -> PathBuf {
    let busy = |p: &Path| p.read_dir().is_ok_and(|mut d| d.next().is_some());
    if !busy(dir) {
        return dir.to_path_buf();
    }
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    (2..1000)
        .map(|i| dir.with_file_name(format!("{name} ({i})")))
        .find(|p| !busy(p))
        .unwrap_or_else(|| dir.to_path_buf())
}

fn run_demo(req: &JobRequest, cancel: &AtomicBool, send: Emit<'_>) -> Result<JobOutput, String> {
    let tick = |stage: Stage, msg: &str, steps: usize, ms: u64| -> Result<(), String> {
        send(JobEvent::Stage(stage, msg.into()));
        for i in 0..=steps {
            check(cancel)?;
            send(JobEvent::Progress(Some(i as f32 / steps as f32)));
            std::thread::sleep(Duration::from_millis(ms));
        }
        Ok(())
    };
    if matches!(req.source, InputSource::Url(_)) {
        tick(Stage::Fetching, "Downloading audio with yt-dlp", 40, 30)?;
        send(JobEvent::Title(crate::demo::DEMO_TITLE.into()));
    }
    send(JobEvent::Stage(Stage::Decoding, "Reading audio".into()));
    send(JobEvent::Progress(None));
    std::thread::sleep(Duration::from_millis(500));
    send(JobEvent::Stage(
        Stage::PreparingModel,
        format!("Loading {}", req.model.name),
    ));
    std::thread::sleep(Duration::from_millis(700));
    // After the pre-split step the demo plays a longer arrangement, cut to the section.
    let arranged = req.title.is_some() || req.range.is_some();
    let stems: Vec<_> = req
        .model
        .stems
        .iter()
        .map(|k| {
            let audio = if arranged {
                crate::section::demo_arrangement(*k)
            } else {
                crate::demo::stem(*k)
            };
            (*k, crate::section::slice(&audio, req.range))
        })
        .collect();
    tick(Stage::Separating, req.model.name, 120, 40)?;
    let title = match (&req.title, &req.source) {
        (Some(t), _) => t.clone(),
        (None, InputSource::File(_)) => req.source.provisional_title(),
        (None, InputSource::Url(_)) => crate::demo::DEMO_TITLE.to_owned(),
    };
    let mut req = req.clone();
    req.output_root = std::env::temp_dir().join("stem-splitter-demo");
    let mut out = write_stems(&req, &title, stems, send)?;
    if let InputSource::File(p) = &req.source {
        out.origin.input = Some(p.clone());
    }
    out.origin.save(&out.out_dir);
    Ok(out)
}

// ---- smaller tasks -------------------------------------------------------------------------

/// Download a model; `cancel` pauses it (the partial file is kept and resumed next time).
pub fn spawn_model_download(
    info: ModelInfo,
    models_dir: PathBuf,
    demo: bool,
    cancel: Arc<AtomicBool>,
    n: Notifier,
) {
    std::thread::spawn(move || {
        let id = info.id.to_owned();
        let result = guard("model download", || {
            if demo {
                let total = info.size_bytes.max(1);
                for i in 0..=100u64 {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("cancelled".to_owned());
                    }
                    let st = DownloadStatus {
                        done: total * i / 100,
                        total: Some(total),
                        bytes_per_sec: total as f64 / 2.5,
                        resumed_from: 0,
                    };
                    n.send(AppEvent::ModelProgress(id.clone(), st));
                    std::thread::sleep(Duration::from_millis(25));
                }
                return Ok(());
            }
            let mgr = ModelManager::new(models_dir);
            mgr.download_with_status(
                &info,
                &mut |st| n.send(AppEvent::ModelProgress(id.clone(), *st)),
                &cancel,
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
        });
        n.send(AppEvent::ModelDone(id, result.and_then(|r| r)));
    });
}

pub fn spawn_ytdlp_update(paths: AppPaths, demo: bool, n: Notifier) {
    std::thread::spawn(move || {
        let result = guard("yt-dlp update", || {
            if demo {
                std::thread::sleep(Duration::from_millis(900));
                return Ok("yt-dlp is up to date (2026.10.31)".to_owned());
            }
            let f = Fetcher::new(paths.tools.clone(), paths.downloads.clone());
            f.ensure_ytdlp(&mut |_| {}).map_err(|e| e.to_string())?;
            f.update_ytdlp().map_err(|e| e.to_string())
        });
        n.send(AppEvent::YtDlpUpdated(result.and_then(|r| r)));
    });
}

pub struct SampleExport {
    pub stems: Arc<Vec<AudioBuffer>>,
    /// Which stems to include; more than one are summed.
    pub include: Vec<usize>,
    pub start: f64,
    pub end: f64,
    pub fade_ms: f32,
    pub normalize: bool,
    pub format: AudioFormat,
    pub path: PathBuf,
    /// Tempo / key / loop chunks for WAV output (loop end is clamped to the clip).
    pub meta: stem_audio::wavmeta::WavMeta,
}

/// Slice + fade + normalise + write one sample (blocking). Returns the written path.
pub fn render_sample(job: &SampleExport) -> Result<PathBuf, String> {
    let parts: Vec<AudioBuffer> = job
        .include
        .iter()
        .filter_map(|&i| job.stems.get(i))
        .map(|b| {
            let sr = b.sample_rate as f64;
            b.slice((job.start * sr) as usize, (job.end * sr) as usize)
        })
        .collect();
    let mut clip = match parts.len() {
        0 => return Err("nothing selected".to_owned()),
        1 => parts.into_iter().next().unwrap_or_default(),
        _ => AudioBuffer::mix(&parts.iter().collect::<Vec<_>>()),
    };
    let sr = clip.sample_rate as f64;
    let fade = (job.fade_ms as f64 / 1000.0 * sr) as usize;
    let fade = fade.min(clip.frames() / 2);
    clip.fade(fade, fade);
    if job.normalize {
        clip.normalize(0.989); // -0.1 dBFS
    }
    if let Some(dir) = job.path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let n = clip.frames() as u32;
    let mut meta = job.meta;
    meta.loop_frames = meta.loop_frames.map(|(a, b)| (a.min(n), b.min(n)));
    stem_audio::wavmeta::write_audio_meta(&job.path, &clip, job.format, &meta)
        .map_err(|e| e.to_string())?;
    Ok(job.path.clone())
}

/// Slice + fade + normalise + write one sample.
pub fn spawn_sample_export(job: SampleExport, n: Notifier) {
    std::thread::spawn(move || {
        let result = guard("export sample", || render_sample(&job));
        n.send(AppEvent::SampleExported(result.and_then(|r| r)));
    });
}

/// List a playlist's entries for the queue.
pub fn spawn_playlist_expand(
    item: u64,
    url: String,
    paths: AppPaths,
    cookies: Option<String>,
    demo: bool,
    n: Notifier,
) {
    std::thread::spawn(move || {
        let result = guard(
            "playlist",
            || -> Result<Vec<(Option<String>, String)>, String> {
                if demo {
                    std::thread::sleep(Duration::from_millis(600));
                    return Ok((1..=3)
                        .map(|i| {
                            (
                                Some(format!("Playlist track {i}")),
                                format!("https://www.youtube.com/watch?v=demo{i}"),
                            )
                        })
                        .collect());
                }
                let mut f = Fetcher::new(paths.tools.clone(), paths.downloads.clone());
                f.cookies_from_browser = cookies;
                let never = AtomicBool::new(false);
                let list = f.expand_playlist(&url, &never).map_err(|e| e.to_string())?;
                Ok(list.entries.into_iter().map(|e| (e.title, e.url)).collect())
            },
        );
        n.send(AppEvent::PlaylistExpanded(item, result.and_then(|r| r)));
    });
}

/// Install ffmpeg into the tools folder (Windows; elsewhere this reports how to get it).
pub fn spawn_ffmpeg_install(paths: AppPaths, demo: bool, n: Notifier) {
    std::thread::spawn(move || {
        let result = guard("ffmpeg install", || -> Result<PathBuf, String> {
            if demo {
                for i in 0..=50 {
                    n.send(AppEvent::FfmpegProgress(
                        Some(i as f32 / 50.0),
                        "Downloading ffmpeg".into(),
                    ));
                    std::thread::sleep(Duration::from_millis(30));
                }
                return Ok(paths.tools.join("ffmpeg.exe"));
            }
            let f = Fetcher::new(paths.tools.clone(), paths.downloads.clone());
            let mut msg = String::from("Starting");
            f.ensure_ffmpeg(&mut |p| match p {
                FetchProgress::Preparing(m) => {
                    msg = m;
                    n.send(AppEvent::FfmpegProgress(None, msg.clone()));
                }
                FetchProgress::Downloading { fraction, .. } => {
                    n.send(AppEvent::FfmpegProgress(fraction, msg.clone()));
                }
                FetchProgress::Converting => {}
            })
            .map_err(|e| e.to_string())
        });
        n.send(AppEvent::FfmpegDone(result.and_then(|r| r)));
    });
}

/// Re-open a finished job from history (decodes the stem WAVs).
pub fn spawn_load_history(
    id: u64,
    title: String,
    out_dir: PathBuf,
    kinds: Vec<StemKind>,
    demo: bool,
    n: Notifier,
) {
    std::thread::spawn(move || {
        let send = |ev| n.send(AppEvent::Job(id, ev));
        send(JobEvent::Stage(Stage::Decoding, "Loading stems".into()));
        let result = guard("load stems", || -> Result<JobOutput, String> {
            let mut stems = Vec::new();
            for (i, kind) in kinds.iter().enumerate() {
                let path = out_dir.join(format!("{}.wav", kind.label().to_lowercase()));
                let audio = if demo && !path.exists() {
                    crate::demo::stem(*kind)
                } else {
                    stem_audio::decode_file(&path)
                        .map_err(|e| format!("{}: {e}", path.display()))?
                };
                let env = Envelope::from_buffer(&audio, BUCKETS);
                stems.push(StemOut {
                    kind: *kind,
                    audio,
                    path,
                    env,
                });
                send(JobEvent::Progress(Some(
                    (i + 1) as f32 / kinds.len().max(1) as f32,
                )));
            }
            let analysis = analyze(&stems);
            Ok(JobOutput {
                title: title.clone(),
                out_dir: out_dir.clone(),
                stems,
                analysis,
                origin: Origin::load(&out_dir),
            })
        });
        match result.and_then(|r| r) {
            Ok(out) => send(JobEvent::Done(Box::new(out))),
            Err(e) => send(JobEvent::Failed(e)),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overall_progress_is_monotonic() {
        let mut last = 0.0;
        for s in Stage::ALL {
            for f in [0.0, 0.5, 1.0] {
                let o = s.overall(f);
                assert!(o >= last);
                last = o;
            }
        }
        assert!((last - 1.0).abs() < 1e-5);
    }

    /// End-to-end on the real backends: writes a short test tone, downloads ONNX Runtime and
    /// the smallest MDX model, separates, writes WAVs. Needs network; run with `--ignored`.
    #[test]
    #[ignore]
    fn real_pipeline_end_to_end() {
        let tmp = std::env::temp_dir().join("stem-app-e2e");
        let _ = std::fs::remove_dir_all(tmp.join("out"));
        std::fs::create_dir_all(&tmp).unwrap();
        let input = tmp.join("tone.wav");
        let sr = 44_100;
        let tone: Vec<f32> = (0..sr * 4)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / sr as f32).sin() * 0.3)
            .collect();
        stem_audio::write_wav(
            &input,
            &AudioBuffer::new(sr as u32, vec![tone.clone(), tone]),
            WavFormat::Pcm16,
        )
        .unwrap();
        let data = std::env::var_os("STEM_E2E_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| tmp.join("data"));
        let paths = AppPaths {
            models: data.clone(),
            tools: data.join("tools"),
            downloads: data.join("downloads"),
            data,
        };
        let model = stem_models::find_model("mdx-inst-hq3").expect("model in registry");
        let req = JobRequest {
            id: 1,
            source: InputSource::File(input),
            model,
            device: Device::Cpu,
            output_root: tmp.join("out"),
            wav: WavFormat::Pcm24,
            cookies_from_browser: None,
            paths,
            demo: false,
            range: None,
            title: None,
            chain: None,
        };
        let cancel = AtomicBool::new(false);
        let out = run_real(&req, &cancel, &|ev| {
            if let JobEvent::Stage(s, d) = ev {
                eprintln!("{s:?}: {d}");
            }
        })
        .expect("pipeline");
        assert_eq!(out.stems.len(), 2);
        for s in &out.stems {
            assert!(s.path.is_file(), "{}", s.path.display());
            assert_eq!(s.audio.frames(), sr * 4);
        }
    }

    #[test]
    fn titles() {
        let u = InputSource::Url("https://www.youtube.com/watch?v=x".into());
        assert_eq!(u.provisional_title(), "Link from youtube.com");
        let f = InputSource::File(PathBuf::from("/a/b/My Song.mp3"));
        assert_eq!(f.provisional_title(), "My Song");
    }
}
