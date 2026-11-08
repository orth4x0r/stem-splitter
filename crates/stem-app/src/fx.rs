//! State and workers behind the results page's sampler tools:
//!
//! - **Transform**: tempo / pitch / varispeed / reverse renders of the selection or a whole
//!   stem (`stem_audio::stretch`), previewed through [`Audition`] and exported with the
//!   usual formats and names plus a suffix such as "+3st 95bpm".
//! - **Kit**: one-shot extraction from a drum-like stem (`stem_audio::oneshot`), shown in a
//!   sheet where hits can be auditioned, picked and exported as a kit folder with an SFZ.
//!
//! All rendering runs on worker threads with progress in an atomic and a cancel flag; results
//! come back over a channel drained by [`poll`] once per frame.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;
use stem_audio::analysis::{Key, Mode};
use stem_audio::oneshot::{Kit, KitOptions, KitSample};
use stem_audio::stretch::{Algorithm, Control, SpeedMode, Transform};
use stem_audio::{AudioBuffer, AudioError, AudioFormat, WavFormat};
use stem_models::StemKind;

use crate::app::StemApp;
use crate::audition::Audition;

// ---- transform settings --------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Selection,
    Whole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempoUnit {
    Bpm,
    Percent,
}

/// What the Transform panel's controls hold.
#[derive(Debug, Clone, PartialEq)]
pub struct TransformUi {
    pub target: Target,
    /// Session track index.
    pub stem: usize,
    pub unit: TempoUnit,
    /// Target tempo when the source tempo is known.
    pub bpm: f32,
    /// Speed in percent (100 = unchanged), used without a known tempo or in % mode.
    pub percent: f32,
    pub semis: i32,
    pub cents: i32,
    pub mode: SpeedMode,
    pub algorithm: Algorithm,
    pub reverse: bool,
}

impl Default for TransformUi {
    fn default() -> Self {
        Self {
            target: Target::Selection,
            stem: 0,
            unit: TempoUnit::Bpm,
            bpm: 0.0,
            percent: 100.0,
            semis: 0,
            cents: 0,
            mode: SpeedMode::Keylock,
            algorithm: Algorithm::Auto,
            reverse: false,
        }
    }
}

impl TransformUi {
    /// The render this describes, given the session's tempo.
    pub fn transform(&self, source_bpm: Option<f32>, kind: Option<StemKind>) -> Transform {
        let speed = match (self.unit, source_bpm) {
            (TempoUnit::Bpm, Some(src)) if src > 0.0 && self.bpm > 0.0 => (self.bpm / src) as f64,
            _ => (self.percent / 100.0) as f64,
        };
        let algorithm = match (self.algorithm, kind) {
            // The stem name is a better hint than any detector.
            (Algorithm::Auto, Some(StemKind::Drums)) => Algorithm::Percussive,
            (Algorithm::Auto, Some(k)) if k != StemKind::Other && k != StemKind::Effects => {
                Algorithm::Tonal
            }
            (a, _) => a,
        };
        Transform {
            speed: speed.clamp(0.25, 4.0),
            semitones: self.semis as f64 + self.cents as f64 / 100.0,
            mode: self.mode,
            algorithm,
            reverse: self.reverse,
        }
    }
}

/// `key` moved by `semitones` (rounded), same mode.
pub fn transpose_key(key: &Key, semitones: f64) -> Key {
    let s = semitones.round() as i32;
    Key {
        tonic: (key.tonic as i32 + s).rem_euclid(12) as u8,
        ..*key
    }
}

/// Shortest move (−6..=+5 semitones) from `from` to the tonic `to`.
pub fn semitones_to(from: &Key, to: u8) -> i32 {
    (to as i32 - from.tonic as i32 + 6).rem_euclid(12) - 6
}

/// The 12 keys in `mode`, for the "to key" picker.
pub fn keys_in(mode: Mode) -> Vec<Key> {
    (0..12)
        .map(|tonic| Key {
            tonic,
            mode,
            confidence: 1.0,
        })
        .collect()
}

// ---- jobs ----------------------------------------------------------------------------------

/// Identifies one render: same key, same audio.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderKey {
    pub stem: usize,
    /// Frames, or `None` for the whole stem.
    pub range: Option<(usize, usize)>,
    pub transform: Transform,
    /// Fade (ms) and normalise applied after the render (selections only).
    pub fade_ms: f32,
    pub normalize: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Preview,
    Export,
}

/// A running worker: progress `0..=1` (f32 bits) and its cancel flag.
pub struct Running {
    pub id: u64,
    pub progress: Arc<AtomicU32>,
    pub cancel: Arc<AtomicBool>,
}

impl Running {
    pub fn frac(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }
}

pub struct Render {
    pub run: Running,
    pub key: RenderKey,
    pub purpose: Purpose,
    /// For exports: where the file goes.
    pub path: Option<PathBuf>,
}

pub struct Rendered {
    pub key: RenderKey,
    pub audio: Arc<AudioBuffer>,
    pub secs: f64,
}

// ---- kit sheet ------------------------------------------------------------------------------

pub enum KitState {
    Extracting(Running),
    Ready {
        kit: Arc<Kit>,
        /// One flag per kit sample.
        checked: Vec<bool>,
        /// Waveform peaks per kit sample (for the mini waveforms).
        peaks: Vec<Vec<(f32, f32)>>,
    },
    Failed(String),
}

pub struct KitSheet {
    /// Session track the kit comes from.
    #[allow(dead_code)]
    pub stem: usize,
    pub kind: StemKind,
    pub state: KitState,
    /// Folder of the last export.
    pub exported: Option<PathBuf>,
    pub busy_export: bool,
}

// ---- events / state ------------------------------------------------------------------------

enum FxEvent {
    Rendered(u64, Result<(AudioBuffer, f64), String>),
    Wrote(Result<PathBuf, String>),
    Kit(u64, Result<Kit, String>),
    KitWritten(Result<(PathBuf, usize), String>),
}

pub struct Fx {
    /// Session (output folder) this state belongs to; reset when it changes.
    session: Option<PathBuf>,
    pub ui: TransformUi,
    pub render: Option<Render>,
    pub last: Option<Rendered>,
    pub kit: Option<KitSheet>,
    pub audition: Audition,
    /// Scroll the Transform panel into view on the next frame.
    pub scroll_to_panel: bool,
    /// Last exported transform file (for the drag pill).
    pub last_export: Option<PathBuf>,
    tx: Sender<FxEvent>,
    rx: Receiver<FxEvent>,
    ctx: egui::Context,
    next_id: u64,
}

impl Fx {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = unbounded();
        Self {
            session: None,
            ui: TransformUi::default(),
            render: None,
            last: None,
            kit: None,
            audition: Audition::default(),
            scroll_to_panel: false,
            last_export: None,
            tx,
            rx,
            ctx,
            next_id: 1,
        }
    }

    pub fn ctx(&self) -> egui::Context {
        self.ctx.clone()
    }

    pub fn busy(&self) -> bool {
        self.render.is_some()
            || self.audition.is_playing()
            || self
                .kit
                .as_ref()
                .is_some_and(|k| k.busy_export || matches!(k.state, KitState::Extracting(_)))
    }

    fn running(&mut self, _now: f64) -> Running {
        let id = self.next_id;
        self.next_id += 1;
        Running {
            id,
            progress: Arc::new(AtomicU32::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel_render(&mut self) {
        if let Some(r) = self.render.take() {
            r.run.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn close_kit(&mut self) {
        if let Some(k) = self.kit.take()
            && let KitState::Extracting(r) = &k.state
        {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.audition.stop();
    }
}

/// Per-frame upkeep: follow session changes, collect worker results.
pub fn poll(app: &mut StemApp) {
    let key = app.session.as_ref().map(|s| s.out_dir.clone());
    if key != app.fx.session {
        app.fx.cancel_render();
        app.fx.close_kit();
        app.fx.last = None;
        app.fx.last_export = None;
        app.fx.ui = TransformUi::default();
        if let Some(s) = &app.session {
            app.fx.ui.stem = s.sel_track.min(s.tracks.len().saturating_sub(1));
            app.fx.ui.bpm = s
                .analysis
                .as_ref()
                .and_then(|a| a.bpm())
                .map_or(0.0, |b| (b * 10.0).round() / 10.0);
        }
        app.fx.session = key;
    }
    while let Ok(ev) = app.fx.rx.try_recv() {
        match ev {
            FxEvent::Rendered(id, res) => {
                let Some(r) = app.fx.render.take_if(|r| r.run.id == id) else {
                    continue;
                };
                match res {
                    Ok((audio, secs)) => {
                        let audio = Arc::new(audio);
                        app.fx.last = Some(Rendered {
                            key: r.key.clone(),
                            audio: audio.clone(),
                            secs,
                        });
                        match r.purpose {
                            Purpose::Preview => {
                                app.player.set_playing(false);
                                if !app.fx.audition.play(audio, "transform") {
                                    app.toasts.info(
                                        "Rendered",
                                        "No audio output for the preview; export still works.",
                                    );
                                }
                            }
                            Purpose::Export => {
                                if let Some(path) = r.path {
                                    write_file(
                                        &app.fx.tx,
                                        &app.fx.ctx,
                                        audio,
                                        path,
                                        app.settings.export_format.audio_format(),
                                    );
                                }
                            }
                        }
                    }
                    Err(e) if e == "cancelled" => {}
                    Err(e) => app.toasts.error("Transform failed", e),
                }
            }
            FxEvent::Wrote(res) => match res {
                Ok(p) => {
                    app.toasts.success(
                        "Transform exported",
                        p.file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    );
                    app.fx.last_export = Some(p);
                }
                Err(e) => app.toasts.error("Export failed", e),
            },
            FxEvent::Kit(id, res) => {
                let Some(sheet) = app.fx.kit.as_mut() else {
                    continue;
                };
                if !matches!(&sheet.state, KitState::Extracting(r) if r.id == id) {
                    continue;
                }
                sheet.state = match res {
                    Ok(kit) => {
                        let peaks = kit.samples.iter().map(|s| s.audio.peaks(96)).collect();
                        KitState::Ready {
                            checked: vec![true; kit.samples.len()],
                            kit: Arc::new(kit),
                            peaks,
                        }
                    }
                    Err(e) => KitState::Failed(e),
                };
            }
            FxEvent::KitWritten(res) => {
                if let Some(k) = &mut app.fx.kit {
                    k.busy_export = false;
                }
                match res {
                    Ok((dir, n)) => {
                        app.toasts.success(
                            format!("Kit exported: {n} samples"),
                            dir.display().to_string(),
                        );
                        if let Some(k) = &mut app.fx.kit {
                            k.exported = Some(dir);
                        }
                    }
                    Err(e) => app.toasts.error("Kit export failed", e),
                }
            }
        }
    }
}

fn err_text(e: AudioError) -> String {
    match e {
        AudioError::Cancelled => "cancelled".into(),
        e => e.to_string(),
    }
}

// ---- transform actions ---------------------------------------------------------------------

/// The render the panel currently describes, or why there is none.
pub fn current_key(app: &StemApp) -> Result<RenderKey, &'static str> {
    let s = app.session.as_ref().ok_or("No session")?;
    let ui = &app.fx.ui;
    let stem = ui.stem.min(s.tracks.len().saturating_sub(1));
    let audio = s.audio.get(stem).ok_or("No audio")?;
    let bpm = s.analysis.as_ref().and_then(|a| a.bpm());
    let transform = ui.transform(bpm, s.tracks.get(stem).map(|t| t.kind));
    let range = match ui.target {
        Target::Whole => None,
        Target::Selection => {
            let (a, b) = s
                .selection
                .ok_or("Select a region first, or pick Whole stem")?;
            let grid = s.analysis.as_ref().and_then(|x| x.grid.as_ref());
            let (f0, f1, _) =
                crate::analysis::export_bounds(&[audio], a, b, grid, app.settings.export_loop_bars);
            Some((f0, f1))
        }
    };
    if transform.is_identity() {
        return Err("Change tempo, pitch or reverse first");
    }
    Ok(RenderKey {
        stem,
        range,
        transform,
        fade_ms: if range.is_some() {
            app.settings.sample_fade_ms
        } else {
            0.0
        },
        normalize: range.is_some() && app.settings.normalize_samples,
    })
}

/// File for the transform export of `key`.
pub fn export_path(app: &StemApp, key: &RenderKey) -> Option<PathBuf> {
    let s = app.session.as_ref()?;
    let tr = s.tracks.get(key.stem)?;
    let ana = s.analysis.as_ref();
    let bpm = ana.and_then(|a| a.bpm());
    let suffix = key.transform.name_suffix(bpm);
    let format = app.settings.export_format.audio_format();
    let sr = s
        .audio
        .get(key.stem)
        .map_or(44_100.0, |a| a.sample_rate as f64);
    let key_after = ana
        .and_then(|a| a.key.as_ref())
        .filter(|_| tr.kind != StemKind::Drums)
        .map(|k| transpose_key(k, key.transform.pitch_semitones()));
    let (dir, name) = match key.range {
        Some((f0, _)) => {
            let base = crate::analysis::sample_file_name(
                &s.title,
                tr.kind.label(),
                None,
                key_after.as_ref(),
                (f0 as f64 + 0.5) / sr,
                None,
            );
            (
                s.out_dir.join("Samples"),
                crate::paths::sanitize_file_name(&format!("{base} - {suffix}")),
            )
        }
        None => {
            let mut stem = format!("{} {suffix}", tr.kind.label());
            if let Some(k) = &key_after {
                stem = format!("{stem} {}", k.short());
            }
            (
                s.out_dir.join("Export"),
                crate::export::render_name(
                    &app.settings.export_template,
                    &s.title,
                    &stem,
                    key.stem + 1,
                ),
            )
        }
    };
    Some(dir.join(format!("{name}.{}", format.extension())))
}

/// Start rendering `key` for `purpose`. Reuses the last render when it matches.
pub fn start(app: &mut StemApp, purpose: Purpose, now: f64) {
    let key = match current_key(app) {
        Ok(k) => k,
        Err(why) => {
            app.toasts.info("Nothing to render", why);
            return;
        }
    };
    let path = export_path(app, &key);
    if let Some(last) = app.fx.last.as_ref().filter(|l| l.key == key) {
        let audio = last.audio.clone();
        match purpose {
            Purpose::Preview => {
                if app.fx.audition.playing("transform") {
                    app.fx.audition.stop();
                } else {
                    app.player.set_playing(false);
                    app.fx.audition.play(audio, "transform");
                }
            }
            Purpose::Export => {
                if let Some(p) = path {
                    write_file(
                        &app.fx.tx,
                        &app.fx.ctx,
                        audio,
                        p,
                        app.settings.export_format.audio_format(),
                    );
                }
            }
        }
        return;
    }
    let Some(s) = &app.session else { return };
    app.fx.cancel_render();
    app.fx.audition.stop();
    let run = app.fx.running(now);
    let (stems, tx, ctx) = (s.audio.clone(), app.fx.tx.clone(), app.fx.ctx.clone());
    let (id, progress, cancel) = (run.id, run.progress.clone(), run.cancel.clone());
    let job = key.clone();
    std::thread::Builder::new()
        .name("transform".into())
        .spawn(move || {
            let t0 = std::time::Instant::now();
            let res = crate::backend::guard("transform", || {
                render_key(&stems, &job, &progress, &cancel, &ctx)
            })
            .and_then(|r| r)
            .map(|a| (a, t0.elapsed().as_secs_f64()));
            let _ = tx.send(FxEvent::Rendered(id, res));
            ctx.request_repaint();
        })
        .ok();
    app.fx.render = Some(Render {
        run,
        key,
        purpose,
        path,
    });
}

/// Blocking render of `key` (also used by the screenshot scene).
pub fn render_key(
    stems: &[AudioBuffer],
    key: &RenderKey,
    progress: &AtomicU32,
    cancel: &AtomicBool,
    ctx: &egui::Context,
) -> Result<AudioBuffer, String> {
    let src = stems.get(key.stem).ok_or("no such stem")?;
    let clip = match key.range {
        Some((a, b)) => src.slice(a, b),
        None => src.clone(),
    };
    let last = AtomicU32::new(0);
    let report = |f: f32| {
        progress.store(f.to_bits(), Ordering::Relaxed);
        // Repaint at most every 2 %.
        let step = (f * 50.0) as u32;
        if last.swap(step, Ordering::Relaxed) != step {
            ctx.request_repaint();
        }
    };
    let ctl = Control {
        progress: &report,
        cancel,
    };
    let mut out = stem_audio::stretch::render(&clip, &key.transform, &ctl).map_err(err_text)?;
    if key.fade_ms > 0.0 {
        let fade = ((key.fade_ms / 1000.0) * out.sample_rate as f32) as usize;
        let fade = fade.min(out.frames() / 2);
        out.fade(fade, fade);
    }
    if key.normalize {
        out.normalize(0.989);
    }
    Ok(out)
}

fn write_file(
    tx: &Sender<FxEvent>,
    ctx: &egui::Context,
    audio: Arc<AudioBuffer>,
    path: PathBuf,
    format: AudioFormat,
) {
    let (tx, ctx) = (tx.clone(), ctx.clone());
    std::thread::spawn(move || {
        let res = (|| -> Result<PathBuf, String> {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            stem_audio::write_audio(&path, &audio, format)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(path)
        })();
        let _ = tx.send(FxEvent::Wrote(res));
        ctx.request_repaint();
    });
}

// ---- kit actions ---------------------------------------------------------------------------

/// Open the kit sheet for session track `stem` and start extracting.
pub fn open_kit(app: &mut StemApp, stem: usize, now: f64) {
    let Some(s) = &app.session else { return };
    let (Some(audio), Some(tr)) = (s.audio.get(stem), s.tracks.get(stem)) else {
        return;
    };
    let kind = tr.kind;
    let audio = audio.clone();
    app.fx.close_kit();
    let run = app.fx.running(now);
    let (id, progress, cancel) = (run.id, run.progress.clone(), run.cancel.clone());
    let (tx, ctx) = (app.fx.tx.clone(), app.fx.ctx.clone());
    std::thread::Builder::new()
        .name("kit".into())
        .spawn(move || {
            let report = |f: f32| {
                progress.store(f.to_bits(), Ordering::Relaxed);
                ctx.request_repaint();
            };
            let res = crate::backend::guard("kit", || {
                stem_audio::oneshot::extract_kit(&audio, &KitOptions::default(), &report, &cancel)
                    .map_err(err_text)
            })
            .and_then(|r| r);
            let _ = tx.send(FxEvent::Kit(id, res));
            ctx.request_repaint();
        })
        .ok();
    app.fx.kit = Some(KitSheet {
        stem,
        kind,
        state: KitState::Extracting(run),
        exported: None,
        busy_export: false,
    });
}

/// Open the kit sheet with the extraction done on this thread (screenshot scenes).
pub fn open_kit_blocking(app: &mut StemApp, stem: usize, audio: &AudioBuffer) {
    let kind = app
        .session
        .as_ref()
        .and_then(|s| s.tracks.get(stem))
        .map_or(StemKind::Drums, |t| t.kind);
    let state = match stem_audio::oneshot::extract_kit(
        audio,
        &KitOptions::default(),
        &|_| {},
        &AtomicBool::new(false),
    ) {
        Ok(kit) => {
            let peaks = kit.samples.iter().map(|s| s.audio.peaks(96)).collect();
            KitState::Ready {
                checked: vec![true; kit.samples.len()],
                kit: Arc::new(kit),
                peaks,
            }
        }
        Err(e) => KitState::Failed(err_text(e)),
    };
    app.fx.kit = Some(KitSheet {
        stem,
        kind,
        state,
        exported: None,
        busy_export: false,
    });
}

/// Kit folder for the open sheet: `<out>/Kit`, or `<out>/Kit - <Stem>` for non-drum stems.
pub fn kit_dir(app: &StemApp) -> Option<PathBuf> {
    let s = app.session.as_ref()?;
    let k = app.fx.kit.as_ref()?;
    Some(if k.kind == StemKind::Drums {
        s.out_dir.join("Kit")
    } else {
        s.out_dir.join(format!("Kit - {}", k.kind.label()))
    })
}

/// The checked samples of the open sheet, renamed so each class counts from 01 again.
fn kit_picks(app: &StemApp) -> Vec<KitSample> {
    let Some(KitSheet {
        state: KitState::Ready { kit, checked, .. },
        ..
    }) = &app.fx.kit
    else {
        return Vec::new();
    };
    let mut counts = std::collections::HashMap::new();
    kit.samples
        .iter()
        .zip(checked)
        .filter(|(_, c)| **c)
        .map(|(s, _)| {
            let n = counts.entry(s.class).or_insert(0usize);
            *n += 1;
            KitSample {
                name: format!("{} {:02}", s.class.label(), n),
                ..s.clone()
            }
        })
        .collect()
}

/// Write the checked kit samples. `blocking` writes on this thread (for drag-out) and
/// returns the folder.
pub fn export_kit(app: &mut StemApp, blocking: bool) -> Option<PathBuf> {
    let dir = kit_dir(app)?;
    let picks = kit_picks(app);
    if picks.is_empty() {
        app.toasts
            .info("Nothing to export", "Tick at least one sample.");
        return None;
    }
    let title = app
        .session
        .as_ref()
        .map_or(String::new(), |s| s.title.clone());
    let sfz_name = crate::paths::sanitize_file_name(&format!("{title} Kit"));
    let job = move || write_kit_files(&dir, &picks, &sfz_name, &title);
    if blocking {
        match job() {
            Ok((dir, _)) => {
                if let Some(k) = &mut app.fx.kit {
                    k.exported = Some(dir.clone());
                }
                Some(dir)
            }
            Err(e) => {
                app.toasts.error("Kit export failed", e);
                None
            }
        }
    } else {
        if let Some(k) = &mut app.fx.kit {
            k.busy_export = true;
        }
        let (tx, ctx) = (app.fx.tx.clone(), app.fx.ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(FxEvent::KitWritten(job()));
            ctx.request_repaint();
        });
        None
    }
}

/// Clear old kit files (ours only) and write the new set. Returns (folder, sample count).
fn write_kit_files(
    dir: &Path,
    picks: &[KitSample],
    sfz_name: &str,
    title: &str,
) -> Result<(PathBuf, usize), String> {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let ours = ["Kick ", "Snare ", "Hat ", "Perc "]
                .iter()
                .any(|p| name.starts_with(p) && name.ends_with(".wav"))
                || name.ends_with(".sfz");
            if ours {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let refs: Vec<&KitSample> = picks.iter().collect();
    stem_audio::oneshot::write_kit(
        dir,
        &refs,
        AudioFormat::Wav(WavFormat::Pcm24),
        sfz_name,
        title,
    )
    .map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok((dir.to_path_buf(), picks.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_moves() {
        let am = Key {
            tonic: 9,
            mode: Mode::Minor,
            confidence: 0.8,
        };
        assert_eq!(semitones_to(&am, 0), 3); // A minor -> C minor: up 3
        assert_eq!(semitones_to(&am, 7), -2); // -> G minor: down 2
        assert_eq!(semitones_to(&am, 3), -6);
        assert_eq!(transpose_key(&am, 3.0).short(), "Cm");
        assert_eq!(transpose_key(&am, -10.0).short(), "Bm");
    }

    #[test]
    fn demo_drums_make_a_kit() {
        let drums = crate::demo::stem(StemKind::Drums);
        let kit = stem_audio::oneshot::extract_kit(
            &drums,
            &KitOptions::default(),
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        for h in kit.hits.iter().take(12) {
            let f = &h.features;
            eprintln!(
                "{:>7} {:?} c{:.0} lo{:.2} body{:.2} hi{:.2} z{:.0} d{:.2}",
                h.start,
                h.class,
                f.centroid_hz,
                f.low_ratio,
                f.body_ratio,
                f.high_ratio,
                f.zcr,
                f.decay_s
            );
        }
        let names: Vec<_> = kit
            .samples
            .iter()
            .map(|s| format!("{} x{}", s.name, s.count))
            .collect();
        eprintln!("{} hits: {names:?}", kit.hits.len());
        let has = |c| kit.samples.iter().any(|s| s.class == c);
        use stem_audio::oneshot::HitClass;
        assert!(
            has(HitClass::Kick) && has(HitClass::Snare) && has(HitClass::Hat),
            "{names:?}"
        );
        assert!(kit.samples.len() <= 8, "{names:?}");
    }

    #[test]
    fn transform_from_ui() {
        let ui = TransformUi {
            bpm: 95.0,
            semis: 3,
            ..TransformUi::default()
        };
        let t = ui.transform(Some(90.0), Some(StemKind::Drums));
        assert!((t.speed - 95.0 / 90.0).abs() < 1e-6);
        assert_eq!(t.algorithm, Algorithm::Percussive);
        assert_eq!(t.name_suffix(Some(90.0)), "+3st 95bpm");
        let t = ui.transform(None, Some(StemKind::Vocals));
        assert_eq!(t.speed, 1.0);
        assert_eq!(t.algorithm, Algorithm::Tonal);
    }
}
