//! Application state and the per-frame loop. Drawing lives in `views/`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, unbounded};
use eframe::egui;
use stem_audio::AudioBuffer;
use stem_models::{DownloadStatus, StemKind};

use crate::backend::Backend;
use crate::paths::AppPaths;
use crate::pipeline::{
    self, AppEvent, InputSource, JobEvent, JobOutput, JobRequest, Notifier, Stage,
};
use crate::player::Player;
use crate::queue::{ItemStatus, Queue};
use crate::settings::{HistoryEntry, Settings, now_unix};
use crate::shot::Shot;
use crate::waveform::Envelope;
use crate::widgets::drag_handle::PillAction;
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
    /// Tempo, key and beat grid.
    pub analysis: Option<crate::analysis::TrackAnalysis>,
    /// Visible window of the waveforms (zoom / scroll).
    pub timeline: crate::timeline::Timeline,
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

/// A model download in progress (the Models page shows speed / ETA and a cancel button).
pub struct ModelDl {
    pub status: DownloadStatus,
    pub cancel: Arc<AtomicBool>,
}

/// State of the "Export stems" dialog while it is open.
pub struct ExportDialog {
    /// One flag per session track.
    pub selected: Vec<bool>,
    pub dest: PathBuf,
}

/// The "Install ffmpeg" button in Settings.
#[derive(Default)]
pub struct FfmpegState {
    pub busy: bool,
    pub frac: Option<f32>,
    pub msg: String,
    /// Cached lookup: `None` = not checked yet.
    pub found: Option<Option<PathBuf>>,
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
    pub model_downloads: HashMap<String, ModelDl>,
    pub ytdlp_busy: bool,
    next_job_id: u64,
    pub shot: Option<Shot>,
    /// Drag-and-drop hover strength for the overlay, set each frame.
    pub drop_hover: bool,
    /// Batch queue of links / files split one after another.
    pub queue: Queue,
    pub export_dialog: Option<ExportDialog>,
    pub export_busy: bool,
    /// Files to drag out of the window at the end of this frame (Windows OLE drag).
    pending_drag: Option<Vec<PathBuf>>,
    /// The OS drag loop swallowed the mouse-up: tell egui the button was released.
    release_pointer: bool,
    /// Files we dragged out last, and when: if the user lets go over our own window, winit
    /// reports them as dropped files, which must not start a split.
    dragged_out: (Vec<PathBuf>, f64),
    pub ffmpeg: FfmpegState,
    /// Size of the downloads cache in bytes; `None` = not measured yet.
    pub downloads_size: Option<u64>,
    /// The open "→ MIDI" sheet.
    pub midi: Option<crate::midi::MidiSheet>,
    pub midi_prefs: crate::midi::MidiPrefs,
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
            queue: Queue::default(),
            export_dialog: None,
            export_busy: false,
            pending_drag: None,
            release_pointer: false,
            dragged_out: (Vec::new(), 0.0),
            ffmpeg: FfmpegState::default(),
            downloads_size: None,
            midi: None,
            midi_prefs: Default::default(),
        };
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

    /// Split `source` now, or queue it behind the running job.
    pub fn start_job(&mut self, source: InputSource, now: f64) {
        if self.job.is_some() || self.queue.has_work() {
            self.enqueue(vec![source]);
            return;
        }
        self.launch(source, now, None);
    }

    /// Start the split pipeline for `source`; `queue_item` links it to a queue entry.
    fn launch(&mut self, source: InputSource, now: f64, queue_item: Option<u64>) {
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
        if let Some(item) = queue_item.and_then(|q| self.queue.get_mut(q)) {
            item.status = ItemStatus::Running { job_id: id };
        }
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
        let text = self.url_input.trim().to_owned();
        if text.is_empty() {
            self.toasts.info(
                "Paste a link first",
                "YouTube, X, SoundCloud or a direct audio link. Several links queue up.",
            );
            return;
        }
        let (sources, bad) = crate::queue::parse_input(&text);
        if sources.is_empty() {
            self.toasts.error("That doesn't look like a link", text);
            return;
        }
        if !bad.is_empty() {
            self.toasts
                .info("Skipped what isn't a link", bad.join("  "));
        }
        self.url_input.clear();
        let single = sources.len() == 1
            && !matches!(&sources[0], InputSource::Url(u) if stem_fetch::is_playlist_url(u));
        if single {
            let src = sources.into_iter().next().unwrap_or(InputSource::Url(text));
            self.start_job(src, now);
        } else {
            self.enqueue(sources);
        }
    }

    /// Add sources to the batch queue (playlist URLs get listed first).
    pub fn enqueue(&mut self, sources: Vec<InputSource>) {
        let n = sources.len();
        for src in sources {
            let playlist = matches!(&src, InputSource::Url(u) if stem_fetch::is_playlist_url(u));
            let url = src.display();
            let id = self.queue.push(src, playlist);
            if playlist {
                pipeline::spawn_playlist_expand(
                    id,
                    url,
                    self.backend.paths.clone(),
                    self.settings.cookies_from_browser.clone(),
                    self.demo(),
                    self.notifier.clone(),
                );
            }
        }
        self.toasts.info(
            if n == 1 {
                "Added to the queue".to_owned()
            } else {
                format!("Added {n} to the queue")
            },
            "Tracks split one after another; results land in the Library.",
        );
    }

    /// Start the next queued item when nothing is running.
    fn tick_queue(&mut self, now: f64) {
        if self.job.is_some() {
            return;
        }
        if let Some(item) = self.queue.next_pending() {
            let (id, source) = (item.id, item.source.clone());
            self.launch(source, now, Some(id));
            if self.job.is_none()
                && let Some(item) = self.queue.get_mut(id)
            {
                // No model available: launch() already said why.
                item.status = ItemStatus::Failed("couldn't start".into());
            }
        }
    }

    /// Cancel a running queue item, or drop a waiting / finished one.
    pub fn queue_remove(&mut self, id: u64) {
        let running_job = self.queue.running().and_then(|i| match i.status {
            ItemStatus::Running { job_id } if i.id == id => Some(job_id),
            _ => None,
        });
        match running_job {
            Some(job_id) if self.job.as_ref().is_some_and(|j| j.id == job_id) => self.cancel_job(),
            _ => self.queue.remove(id),
        }
    }

    pub fn queue_retry(&mut self, id: u64) {
        if let Some(item) = self.queue.get_mut(id) {
            item.status = ItemStatus::Pending;
        }
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
        let mut analysis = out.analysis;
        // Keep a halve / double the user picked last time.
        if let (Some(a), Some(saved)) = (
            &mut analysis,
            self.settings
                .history
                .iter()
                .find(|h| h.out_dir == out.out_dir)
                .and_then(|h| h.bpm),
        ) {
            a.apply_saved_bpm(saved);
        }
        let timeline = crate::timeline::Timeline::new(duration, audio.clone());
        self.session = Some(Session {
            title: out.title,
            out_dir: out.out_dir,
            duration,
            tracks,
            audio,
            selection: None,
            sel_track: 0,
            analysis,
            timeline,
        });
    }

    pub fn download_model(&mut self, id: &str) {
        if self.model_downloads.contains_key(id) {
            return;
        }
        if let Some(info) = self.backend.models.iter().find(|m| m.id == id).cloned() {
            let cancel = Arc::new(AtomicBool::new(false));
            let resumed = self.backend.manager().partial_bytes(&info);
            self.model_downloads.insert(
                id.to_owned(),
                ModelDl {
                    status: DownloadStatus {
                        done: resumed,
                        total: Some(info.size_bytes),
                        resumed_from: resumed,
                        ..Default::default()
                    },
                    cancel: cancel.clone(),
                },
            );
            pipeline::spawn_model_download(
                info,
                self.backend.paths.models.clone(),
                self.demo(),
                cancel,
                self.notifier.clone(),
            );
        }
    }

    /// Stop a model download; its partial file stays and the next download resumes it.
    pub fn cancel_model_download(&mut self, id: &str) {
        if let Some(d) = self.model_downloads.get(id) {
            d.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn install_ffmpeg(&mut self) {
        if self.ffmpeg.busy {
            return;
        }
        self.ffmpeg.busy = true;
        self.ffmpeg.frac = None;
        self.ffmpeg.msg = "Starting".into();
        pipeline::spawn_ffmpeg_install(
            self.backend.paths.clone(),
            self.demo(),
            self.notifier.clone(),
        );
    }

    /// ffmpeg in the tools folder or on PATH (looked up once, then cached).
    pub fn ffmpeg_path(&mut self) -> Option<PathBuf> {
        if self.ffmpeg.found.is_none() {
            let p = &self.backend.paths;
            let found = if self.shot.is_some() {
                None
            } else {
                stem_fetch::Fetcher::new(p.tools.clone(), p.downloads.clone()).find_ffmpeg()
            };
            self.ffmpeg.found = Some(found);
        }
        self.ffmpeg.found.clone().flatten()
    }

    /// Size of the raw downloads folder (measured once, then cached until it changes).
    pub fn downloads_size(&mut self) -> u64 {
        *self
            .downloads_size
            .get_or_insert_with(|| crate::paths::dir_size(&self.backend.paths.downloads))
    }

    pub fn clear_downloads(&mut self) {
        let dir = self.backend.paths.downloads.clone();
        match crate::paths::clear_dir(&dir) {
            Ok(n) => self.toasts.success(
                "Downloads cache cleared",
                format!("Freed {}", crate::backend::human_bytes(n)),
            ),
            Err(e) => self.toasts.error("Couldn't clear the cache", e.to_string()),
        }
        self.downloads_size = None;
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

    /// The sample-export job for the current selection over `include` stems.
    fn sample_job(&mut self, include: Vec<usize>) -> Option<pipeline::SampleExport> {
        let s = self.session.as_ref()?;
        let Some((a, b)) = s.selection else {
            self.toasts.info(
                "Select a region first",
                "Drag across a waveform to pick a sample.",
            );
            return None;
        };
        let name = if include.len() == 1 {
            s.tracks[include[0]].kind.label().to_owned()
        } else {
            "Mix".to_owned()
        };
        // Zero-crossing edges, or an exact N-bar loop when the selection is close to one.
        let parts: Vec<&AudioBuffer> = include.iter().filter_map(|&i| s.audio.get(i)).collect();
        let grid = s.analysis.as_ref().and_then(|x| x.grid.as_ref());
        let (f0, f1, bars) =
            crate::analysis::export_bounds(&parts, a, b, grid, self.settings.export_loop_bars);
        let sr = parts.first().map_or(44_100.0, |p| p.sample_rate as f64);
        // Half a frame in, so the export's `(t * sr) as usize` lands on exactly these frames.
        let (a, b) = ((f0 as f64 + 0.5) / sr, (f1 as f64 + 0.5) / sr);
        let ana = s.analysis.as_ref();
        let file = crate::analysis::sample_file_name(
            &s.title,
            &name,
            ana.and_then(|x| x.bpm()),
            ana.and_then(|x| x.key.as_ref()),
            a,
            bars,
        );
        let format = self.settings.export_format.audio_format();
        let path = s
            .out_dir
            .join("Samples")
            .join(format!("{file}.{}", format.extension()));
        Some(pipeline::SampleExport {
            stems: s.audio.clone(),
            include,
            start: a,
            end: b,
            fade_ms: self.settings.sample_fade_ms,
            normalize: self.settings.normalize_samples,
            format,
            path,
        })
    }

    /// Export the current selection from `include` stems as one sample file.
    pub fn export_selection(&mut self, include: Vec<usize>) {
        if let Some(job) = self.sample_job(include) {
            pipeline::spawn_sample_export(job, self.notifier.clone());
        }
    }

    /// Drag files out of the app (into a DAW / Explorer). Without native drag support this
    /// reveals the first file's folder instead.
    pub fn drag_out(&mut self, paths: Vec<PathBuf>) {
        if crate::dragout::NATIVE {
            if !paths.is_empty() {
                self.pending_drag = Some(paths);
            }
        } else if let Some(dir) = paths.first().and_then(|p| p.parent()) {
            let dir = dir.to_path_buf();
            self.open_folder(&dir);
        }
    }

    /// A stem's drag pill: dragging starts an OS drag, a click reveals the file.
    pub fn file_pill(&mut self, path: PathBuf, action: PillAction) {
        match action {
            PillAction::Drag => self.drag_out(vec![path]),
            PillAction::Click => {
                if let Some(dir) = path.parent() {
                    let dir = dir.to_path_buf();
                    self.open_folder(&dir);
                }
            }
        }
    }

    /// The selection's drag pill: writes the sample to the Samples folder right away, then
    /// drags that file out (or, on a click, reveals it).
    pub fn selection_pill(&mut self, include: Vec<usize>, action: PillAction) {
        let Some(job) = self.sample_job(include) else {
            return;
        };
        if self.shot.is_some() {
            return;
        }
        match pipeline::render_sample(&job) {
            Ok(path) => self.file_pill(path, action),
            Err(e) => self.toasts.error("Export failed", e),
        }
    }

    /// Open the "Export stems" dialog.
    pub fn export_all_stems(&mut self) {
        let Some(s) = &self.session else { return };
        self.export_dialog = Some(ExportDialog {
            selected: vec![true; s.tracks.len()],
            dest: s.out_dir.join("Export"),
        });
    }

    /// Run the export described by the open dialog and close it.
    pub fn run_export(&mut self) {
        let (Some(s), Some(d)) = (&self.session, self.export_dialog.take()) else {
            return;
        };
        let kinds: Vec<StemKind> = s.tracks.iter().map(|t| t.kind).collect();
        let items = crate::export::plan(&kinds, &d.selected, self.settings.export_without);
        if items.is_empty() {
            self.toasts
                .info("Nothing to export", "Pick at least one stem.");
            return;
        }
        self.export_busy = true;
        crate::export::spawn(
            crate::export::StemExport {
                title: s.title.clone(),
                stems: s.audio.clone(),
                items,
                template: self.settings.export_template.clone(),
                format: self.settings.export_format.audio_format(),
                dest: d.dest,
            },
            self.notifier.clone(),
        );
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
                AppEvent::ModelProgress(id, st) => {
                    if let Some(d) = self.model_downloads.get_mut(&id) {
                        d.status = st;
                    }
                }
                AppEvent::ModelDone(id, res) => {
                    let cancelled = self
                        .model_downloads
                        .remove(&id)
                        .is_some_and(|d| d.cancel.load(Ordering::Relaxed));
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
                        Err(_) if cancelled => self.toasts.info(
                            "Download paused",
                            format!("{name} picks up where it left off next time."),
                        ),
                        Err(e) => self.toasts.error(format!("{name} download failed"), e),
                    }
                }
                AppEvent::PlaylistExpanded(item, res) => match res {
                    Ok(entries) => {
                        let n = entries.len();
                        self.queue.expand(item, entries);
                        self.toasts
                            .success("Playlist added", format!("{n} tracks queued"));
                    }
                    Err(e) => {
                        if let Some(i) = self.queue.get_mut(item) {
                            i.status = ItemStatus::Failed(e.clone());
                        }
                        self.toasts.error("Couldn't read the playlist", e);
                    }
                },
                AppEvent::FfmpegProgress(frac, msg) => {
                    self.ffmpeg.frac = frac;
                    self.ffmpeg.msg = msg;
                }
                AppEvent::FfmpegDone(res) => {
                    self.ffmpeg.busy = false;
                    self.ffmpeg.found = None;
                    match res {
                        Ok(p) => self
                            .toasts
                            .success("ffmpeg installed", p.display().to_string()),
                        Err(e) => self.toasts.error("ffmpeg install failed", e),
                    }
                }
                AppEvent::StemsExported(res) => {
                    self.export_busy = false;
                    match res {
                        Ok((msg, _dir)) => self.toasts.success("Stems exported", msg),
                        Err(e) => self.toasts.error("Export failed", e),
                    }
                }
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
            JobEvent::Title(t) => {
                job.title = t.clone();
                if let Some(item) = self.queue.by_job(id) {
                    item.title = t;
                }
            }
            JobEvent::Done(out) => {
                let reopening = job.reopening;
                let model_name = job.model_name.clone();
                let source = job.source.display();
                self.job = None;
                self.downloads_size = None;
                let n = out.stems.len();
                let (bpm, key) = out.analysis.as_ref().map_or((None, None), |a| {
                    (a.bpm(), a.key.as_ref().map(|k| k.name()))
                });
                if !reopening {
                    let entry = HistoryEntry {
                        title: out.title.clone(),
                        source,
                        model_name,
                        stems: out.stems.iter().map(|s| s.kind).collect(),
                        out_dir: out.out_dir.clone(),
                        when: now_unix(),
                        duration_secs: out.stems.first().map_or(0.0, |s| s.audio.duration_secs()),
                        bpm,
                        key,
                    };
                    self.settings.push_history(entry.clone());
                    self.toasts
                        .success(format!("{n} stems ready"), out.title.clone());
                    // Batch jobs land in the library; the queue panel offers "Open".
                    if let Some(item) = self.queue.by_job(id) {
                        item.title = out.title.clone();
                        item.status = ItemStatus::Done(Box::new(entry));
                        return;
                    }
                } else if let Some(h) = self
                    .settings
                    .history
                    .iter_mut()
                    .find(|h| h.out_dir == out.out_dir)
                {
                    // Entries from before analysis existed get their tempo / key now.
                    h.bpm = h.bpm.or(bpm);
                    h.key = h.key.take().or(key);
                }
                self.open_session(*out);
            }
            JobEvent::Failed(msg) => {
                let title = job.title.clone();
                let source = job.source.clone();
                let reopening = job.reopening;
                self.job = None;
                self.toasts.error("Split failed", msg.clone());
                if let Some(item) = self.queue.by_job(id) {
                    item.status = ItemStatus::Failed(msg);
                    return;
                }
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
                if let Some(item) = self.queue.by_job(id) {
                    item.status = ItemStatus::Cancelled;
                }
            }
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context, now: f64) {
        let (hovering, dropped) = ctx.input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect::<Vec<_>>(),
            )
        });
        self.drop_hover = hovering || self.shot.as_ref().is_some_and(|s| s.force_drop);
        let mut dropped = dropped;
        if now - self.dragged_out.1 < 3.0 {
            let ours: Vec<PathBuf> = self.dragged_out.0.iter().map(|p| abs(p)).collect();
            dropped.retain(|p| !ours.contains(&abs(p)));
        }
        match dropped.len() {
            0 => {}
            1 => {
                let path = dropped.into_iter().next().unwrap_or_default();
                self.start_job(InputSource::File(path), now);
            }
            _ => self.enqueue(dropped.into_iter().map(InputSource::File).collect()),
        }
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
        self.pump(now);
        self.handle_drops(&ctx, now);
        self.tick_queue(now);
        self.sync_player(dt);

        if let Some(job) = &mut self.job {
            let target = job.overall();
            job.shown += (target - job.shown) * (1.0 - (-dt * 6.0).exp());
        }

        // Space toggles playback.
        if self.session.is_some()
            && self.midi.is_none()
            && !ctx.egui_wants_keyboard_input()
            && ctx.input(|i| i.key_pressed(egui::Key::Space))
        {
            self.player.toggle();
        }

        views::draw(self, ui);
        self.toasts.show(&ctx);

        // Native drag-out runs the OS drag loop right here, while the button is still down.
        if let Some(paths) = self.pending_drag.take() {
            match crate::dragout::drag_files(frame, &paths) {
                Ok(()) => {
                    self.release_pointer = true;
                    self.dragged_out = (paths, now);
                }
                Err(e) => self.toasts.error("Couldn't start the drag", e),
            }
        }

        // Repaint only while something moves: idle = 0% CPU.
        let animating = self.job.is_some()
            || self.player.is_playing()
            || self.drop_hover
            || !self.model_downloads.is_empty()
            || self.ytdlp_busy
            || self.ffmpeg.busy
            || self.export_busy
            || self.queue.has_work()
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

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // The OS drag loop consumed the mouse-up, so egui still thinks the button is held.
        if self.release_pointer {
            self.release_pointer = false;
            let (pos, modifiers) = ctx.input(|i| (i.pointer.latest_pos(), i.modifiers));
            if let Some(pos) = pos {
                raw_input.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers,
                });
            }
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.shot.is_none() {
            self.settings.save(storage);
        }
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }
}

fn abs(p: &std::path::Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}
