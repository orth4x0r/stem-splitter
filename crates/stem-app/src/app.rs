//! Application state and the per-frame loop. Drawing lives in `views/`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, unbounded};
use eframe::egui;
use stem_audio::AudioBuffer;
use stem_models::StemKind;

use crate::backend::Backend;
use crate::paths::{AppPaths, sanitize_file_name};
use crate::pipeline::{
    self, AppEvent, InputSource, JobEvent, JobOutput, JobRequest, Notifier, Stage,
};
use crate::player::Player;
use crate::settings::{HistoryEntry, Settings, now_unix};
use crate::shot::Shot;
use crate::waveform::Envelope;
use crate::widgets::toast::Toasts;
use crate::{theme, views};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Split,
    Library,
    Models,
    Settings,
}

pub struct RunningJob {
    pub id: u64,
    pub source: InputSource,
    pub title: String,
    pub model_name: String,
    pub stems: Vec<StemKind>,
    pub stage: Stage,
    pub detail: String,
    pub frac: Option<f32>,
    pub started: f64,
    pub cancel: Arc<AtomicBool>,
    /// Displayed overall progress, eased towards the real value.
    pub shown: f32,
    /// From history: only decoding, no separation.
    pub reopening: bool,
}

impl RunningJob {
    pub fn overall(&self) -> f32 {
        self.stage.overall(self.frac.unwrap_or(0.0))
    }
}

pub struct Track {
    pub kind: StemKind,
    pub env: Envelope,
    pub path: PathBuf,
    pub volume: f32,
    pub mute: bool,
    pub solo: bool,
    /// Smoothed meter value.
    pub level: f32,
}

pub struct Session {
    pub title: String,
    pub out_dir: PathBuf,
    pub duration: f64,
    pub tracks: Vec<Track>,
    pub audio: Arc<Vec<AudioBuffer>>,
    pub selection: Option<(f64, f64)>,
    /// Lane the selection was made on (default export target).
    pub sel_track: usize,
}

impl Session {
    pub fn any_solo(&self) -> bool {
        self.tracks.iter().any(|t| t.solo)
    }
    pub fn audible(&self, i: usize) -> bool {
        let t = &self.tracks[i];
        if self.any_solo() { t.solo } else { !t.mute }
    }
    pub fn gain(&self, i: usize) -> f32 {
        if self.audible(i) {
            self.tracks[i].volume
        } else {
            0.0
        }
    }
}

#[derive(Default)]
pub struct Failure {
    pub title: String,
    pub message: String,
    pub source: Option<InputSource>,
}

pub struct StemApp {
    pub settings: Settings,
    pub backend: Backend,
    pub page: Page,
    pub url_input: String,
    pub job: Option<RunningJob>,
    pub session: Option<Session>,
    pub failure: Option<Failure>,
    pub player: Player,
    pub toasts: Toasts,
    pub notifier: Notifier,
    rx: Receiver<AppEvent>,
    pub model_downloads: HashMap<String, Option<f32>>,
    pub ytdlp_busy: bool,
    next_job_id: u64,
    pub shot: Option<Shot>,
    /// Drag-and-drop hover strength for the overlay, set each frame.
    pub drop_hover: bool,
    /// Ambient background, aurora and celebration state.
    pub fx: crate::effects::Fx,
    pub splash: crate::splash::Splash,
    perf: Option<Perf>,
}

/// Frame-time statistics, printed when `STEM_PERF` is set.
#[derive(Default)]
struct Perf {
    sum: f32,
    max: f32,
    n: u32,
}

impl Perf {
    fn add(&mut self, cpu: Option<f32>) {
        let Some(c) = cpu else { return };
        self.sum += c;
        self.max = self.max.max(c);
        self.n += 1;
        if self.n == 120 {
            eprintln!(
                "perf: cpu/frame avg {:.2} ms, max {:.2} ms over {} frames",
                1000.0 * self.sum / self.n as f32,
                1000.0 * self.max,
                self.n
            );
            *self = Self::default();
        }
    }
}

impl StemApp {
    pub fn new(cc: &eframe::CreationContext<'_>, demo: bool, shot: Option<Shot>) -> Self {
        theme::install(&cc.egui_ctx);
        let (tx, rx) = unbounded();
        let notifier = Notifier::new(tx, cc.egui_ctx.clone());
        let settings = if shot.is_some() {
            Settings::default()
        } else {
            Settings::load(cc.storage)
        };
        let backend = Backend::new(demo, AppPaths::detect());
        let mut app = Self {
            settings,
            backend,
            page: Page::Split,
            url_input: String::new(),
            job: None,
            session: None,
            failure: None,
            player: Player::new(),
            toasts: Toasts::default(),
            notifier,
            rx,
            model_downloads: HashMap::new(),
            ytdlp_busy: false,
            next_job_id: 1,
            shot: None,
            drop_hover: false,
            fx: crate::effects::Fx::default(),
            splash: crate::splash::Splash::new(false),
            perf: std::env::var_os("STEM_PERF").map(|_| Perf::default()),
        };
        theme::motion::set_reduced(app.settings.reduce_motion);
        let intro = app.settings.intro && shot.as_ref().is_none_or(|s| s.scene == "splash");
        app.splash = crate::splash::Splash::new(intro);
        if let Some(problem) = app.backend.problem.clone() {
            app.toasts.error("Backend not ready", problem);
        }
        if let Some(mut s) = shot {
            s.setup(&mut app, &cc.egui_ctx);
            app.shot = Some(s);
        }
        app
    }

    pub fn demo(&self) -> bool {
        self.backend.demo
    }

    // ---- actions -------------------------------------------------------------------------

    pub fn start_job(&mut self, source: InputSource, now: f64) {
        if self.job.is_some() {
            self.toasts.info(
                "Already splitting",
                "Wait for the current track or cancel it.",
            );
            return;
        }
        let Some(model) = self
            .backend
            .model(self.settings.model_id.as_deref())
            .cloned()
        else {
            self.toasts
                .error("No models available", "The model registry is empty.");
            return;
        };
        self.player.unload();
        self.session = None;
        self.failure = None;
        let id = self.next_job_id;
        self.next_job_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.job = Some(RunningJob {
            id,
            title: source.provisional_title(),
            source: source.clone(),
            model_name: model.name.to_owned(),
            stems: model.stems.clone(),
            stage: Stage::Fetching,
            detail: "Starting".into(),
            frac: None,
            started: now,
            cancel: cancel.clone(),
            shown: 0.0,
            reopening: false,
        });
        self.page = Page::Split;
        let req = JobRequest {
            id,
            source,
            model,
            device: self.settings.device,
            output_root: self.settings.output_dir.clone(),
            wav: self.settings.wav.format(),
            cookies_from_browser: self.settings.cookies_from_browser.clone(),
            paths: self.backend.paths.clone(),
            demo: self.demo(),
        };
        pipeline::spawn_job(req, cancel, self.notifier.clone());
    }

    pub fn start_from_input(&mut self, now: f64) {
        let text = self.url_input.trim().trim_matches('"').to_owned();
        if text.is_empty() {
            self.toasts.info(
                "Paste a link first",
                "YouTube, X, SoundCloud or a direct audio link.",
            );
            return;
        }
        let path = PathBuf::from(&text);
        let source = if path.is_file() {
            InputSource::File(path)
        } else if text.starts_with("http://") || text.starts_with("https://") || text.contains('.')
        {
            let url = if text.contains("://") {
                text
            } else {
                format!("https://{text}")
            };
            InputSource::Url(url)
        } else {
            self.toasts.error("That doesn't look like a link", text);
            return;
        };
        self.start_job(source, now);
    }

    pub fn reopen(&mut self, entry: &HistoryEntry, now: f64) {
        if self.job.is_some() {
            return;
        }
        self.player.unload();
        self.session = None;
        let id = self.next_job_id;
        self.next_job_id += 1;
        self.job = Some(RunningJob {
            id,
            source: InputSource::File(entry.out_dir.clone()),
            title: entry.title.clone(),
            model_name: entry.model_name.clone(),
            stems: entry.stems.clone(),
            stage: Stage::Decoding,
            detail: "Loading stems".into(),
            frac: None,
            started: now,
            cancel: Arc::new(AtomicBool::new(false)),
            shown: Stage::Decoding.overall(0.0),
            reopening: true,
        });
        self.page = Page::Split;
        pipeline::spawn_load_history(
            id,
            entry.title.clone(),
            entry.out_dir.clone(),
            entry.stems.clone(),
            self.demo(),
            self.notifier.clone(),
        );
    }

    pub fn cancel_job(&mut self) {
        if let Some(j) = &self.job {
            j.cancel.store(true, Ordering::Relaxed);
            if j.reopening {
                self.job = None;
            }
        }
    }

    pub fn open_session(&mut self, out: JobOutput) {
        let duration = out
            .stems
            .iter()
            .map(|s| s.audio.duration_secs())
            .fold(0.0, f64::max);
        let mut tracks = Vec::new();
        let mut audio = Vec::new();
        for s in out.stems {
            tracks.push(Track {
                kind: s.kind,
                env: s.env,
                path: s.path,
                volume: 1.0,
                mute: false,
                solo: false,
                level: 0.0,
            });
            audio.push(s.audio);
        }
        let audio = Arc::new(audio);
        self.player.load(audio.clone());
        self.session = Some(Session {
            title: out.title,
            out_dir: out.out_dir,
            duration,
            tracks,
            audio,
            selection: None,
            sel_track: 0,
        });
    }

    pub fn download_model(&mut self, id: &str) {
        if self.model_downloads.contains_key(id) {
            return;
        }
        if let Some(info) = self.backend.models.iter().find(|m| m.id == id).cloned() {
            self.model_downloads.insert(id.to_owned(), Some(0.0));
            pipeline::spawn_model_download(
                info,
                self.backend.paths.models.clone(),
                self.demo(),
                self.notifier.clone(),
            );
        }
    }

    pub fn update_ytdlp(&mut self) {
        if !self.ytdlp_busy {
            self.ytdlp_busy = true;
            pipeline::spawn_ytdlp_update(
                self.backend.paths.clone(),
                self.demo(),
                self.notifier.clone(),
            );
        }
    }

    /// Export the current selection from `include` stems as one WAV sample.
    pub fn export_selection(&mut self, include: Vec<usize>) {
        let Some(s) = &self.session else { return };
        let Some((a, b)) = s.selection else {
            self.toasts.info(
                "Select a region first",
                "Drag across a waveform to pick a sample.",
            );
            return;
        };
        let name = if include.len() == 1 {
            s.tracks[include[0]].kind.label().to_owned()
        } else {
            "Mix".to_owned()
        };
        let stamp = crate::backend::mmss_ms(a)
            .replace(':', "m")
            .replace('.', "s");
        let file = sanitize_file_name(&format!("{} - {} {}", s.title, name, stamp));
        let path = s.out_dir.join("Samples").join(format!("{file}.wav"));
        pipeline::spawn_sample_export(
            pipeline::SampleExport {
                stems: s.audio.clone(),
                include,
                start: a,
                end: b,
                fade_ms: self.settings.sample_fade_ms,
                normalize: self.settings.normalize_samples,
                format: self.settings.wav.format(),
                path,
            },
            self.notifier.clone(),
        );
    }

    pub fn export_all_stems(&mut self) {
        let Some(s) = &self.session else { return };
        let files: Vec<PathBuf> = s.tracks.iter().map(|t| t.path.clone()).collect();
        let dest = if self.shot.is_some() {
            None
        } else {
            rfd::FileDialog::new()
                .set_title("Export stems to…")
                .set_directory(&self.settings.output_dir)
                .pick_folder()
        };
        if let Some(dest) = dest {
            let dest = dest.join(sanitize_file_name(&s.title));
            pipeline::spawn_copy_stems(files, dest, self.notifier.clone());
        }
    }

    pub fn open_folder(&mut self, path: &std::path::Path) {
        let _ = std::fs::create_dir_all(path);
        if let Err(e) = crate::paths::open_in_file_manager(path) {
            self.toasts.error("Couldn't open folder", e.to_string());
        }
    }

    // ---- event pump ----------------------------------------------------------------------

    fn pump(&mut self, now: f64) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AppEvent::Job(id, ev) => self.on_job_event(id, ev, now),
                AppEvent::ModelProgress(id, f) => {
                    self.model_downloads.insert(id, f);
                }
                AppEvent::ModelDone(id, res) => {
                    self.model_downloads.remove(&id);
                    let name = self
                        .backend
                        .models
                        .iter()
                        .find(|m| m.id == id)
                        .map_or(id.clone(), |m| m.name.to_owned());
                    match res {
                        Ok(()) => {
                            self.backend.set_downloaded(&id, true);
                            self.toasts.success("Model ready", name);
                        }
                        Err(e) => self.toasts.error(format!("{name} download failed"), e),
                    }
                }
                AppEvent::Notice(Ok(msg)) => self.toasts.success("Done", msg),
                AppEvent::Notice(Err(msg)) => self.toasts.error("Something went wrong", msg),
                AppEvent::YtDlpUpdated(res) => {
                    self.ytdlp_busy = false;
                    match res {
                        Ok(msg) => self.toasts.success("yt-dlp updated", msg),
                        Err(e) => self.toasts.error("yt-dlp update failed", e),
                    }
                }
                AppEvent::SampleExported(res) => match res {
                    Ok(p) => self.toasts.success(
                        "Sample exported",
                        p.file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    ),
                    Err(e) => self.toasts.error("Export failed", e),
                },
            }
        }
    }

    fn on_job_event(&mut self, id: u64, ev: JobEvent, _now: f64) {
        let Some(job) = self.job.as_mut().filter(|j| j.id == id) else {
            return;
        };
        match ev {
            JobEvent::Stage(stage, detail) => {
                job.stage = stage;
                job.detail = detail;
                job.frac = None;
            }
            JobEvent::Progress(f) => job.frac = f,
            JobEvent::Title(t) => job.title = t,
            JobEvent::Done(out) => {
                let reopening = job.reopening;
                let model_name = job.model_name.clone();
                let source = job.source.display();
                self.job = None;
                let n = out.stems.len();
                if !reopening {
                    self.settings.push_history(HistoryEntry {
                        title: out.title.clone(),
                        source,
                        model_name,
                        stems: out.stems.iter().map(|s| s.kind).collect(),
                        out_dir: out.out_dir.clone(),
                        when: now_unix(),
                        duration_secs: out.stems.first().map_or(0.0, |s| s.audio.duration_secs()),
                    });
                    self.toasts
                        .success(format!("{n} stems ready"), out.title.clone());
                    self.fx.celebrate(
                        out.stems.iter().map(|s| theme::stem_color(s.kind)).collect(),
                        format!("{n} STEMS READY"),
                    );
                }
                self.open_session(*out);
            }
            JobEvent::Failed(msg) => {
                let title = job.title.clone();
                let source = job.source.clone();
                let reopening = job.reopening;
                self.job = None;
                self.toasts.error("Split failed", msg.clone());
                if !reopening {
                    self.failure = Some(Failure {
                        title,
                        message: msg,
                        source: Some(source),
                    });
                }
            }
            JobEvent::Cancelled => {
                self.job = None;
                self.toasts.info("Cancelled", "");
            }
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context, now: f64) {
        let (hovering, dropped) = ctx.input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw.dropped_files.first().map(|f| f.path().to_path_buf()),
            )
        });
        self.drop_hover = hovering || self.shot.as_ref().is_some_and(|s| s.force_drop);
        if let Some(path) = dropped {
            self.start_job(InputSource::File(path), now);
        }
    }

    /// Loudest audible stem right now (0..1): drives the audio-reactive chrome.
    pub fn master_level(&self) -> f32 {
        let Some(s) = &self.session else { return 0.0 };
        if !self.player.is_playing() && self.shot.is_none() {
            return 0.0;
        }
        (0..s.tracks.len())
            .filter(|i| s.audible(*i))
            .map(|i| s.tracks[i].level)
            .fold(0.0, f32::max)
            .min(1.0)
    }

    fn sync_player(&mut self, dt: f32) {
        let Some(s) = &mut self.session else { return };
        for i in 0..s.tracks.len() {
            let g = s.gain(i);
            self.player.set_gain(i, g);
        }
        let loop_region = if self.settings.loop_region {
            s.selection
        } else {
            None
        };
        self.player.set_loop(loop_region);
        if self.shot.is_some() {
            return;
        }
        let levels = self.player.take_levels();
        for (t, l) in s.tracks.iter_mut().zip(levels) {
            let decay = (-dt * 6.0).exp();
            t.level = (t.level * decay).max(l);
        }
    }
}

impl eframe::App for StemApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        if let Some(p) = &mut self.perf {
            p.add(frame.info().cpu_usage);
        }
        theme::motion::set_reduced(self.settings.reduce_motion);
        self.pump(now);
        self.handle_drops(&ctx, now);
        self.sync_player(dt);
        let master = self.master_level();
        self.fx.tick(dt, master);

        if let Some(job) = &mut self.job {
            let target = job.overall();
            job.shown += (target - job.shown) * (1.0 - (-dt * 6.0).exp());
        }

        // Space toggles playback.
        if self.session.is_some()
            && !ctx.egui_wants_keyboard_input()
            && ctx.input(|i| i.key_pressed(egui::Key::Space))
        {
            self.player.toggle();
        }

        views::draw(self, ui);
        self.toasts.show(&ctx);
        let intro = self.splash.show(&ctx);

        // Repaint only while something moves: idle = 0% CPU.
        let animating = intro
            || self.fx.is_animating()
            || self.job.is_some()
            || self.player.is_playing()
            || self.drop_hover
            || !self.model_downloads.is_empty()
            || self.ytdlp_busy
            || self.toasts.is_animating()
            || self
                .session
                .as_ref()
                .is_some_and(|s| s.tracks.iter().any(|t| t.level > 0.01));
        if animating {
            ctx.request_repaint();
        }

        if let Some(mut shot) = self.shot.take() {
            shot.frame(self, &ctx);
            self.shot = Some(shot);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.shot.is_none() {
            self.settings.save(storage);
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(shot) = &self.shot {
            shot.filter_input(raw);
        }
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }
}
