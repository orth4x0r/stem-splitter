//! Headless screenshot mode for design reviews and docs.
//!
//! `STEM_DEMO=1 STEM_SHOT=out.png STEM_SCENE=results stem-splitter` sets up a scene with
//! synthetic data, waits for animations to settle, saves a PNG and exits.
//! Scenes: home, drop, progress, results, results-zoom, models, library, settings, error,
//! queue, export, chop (transients on drums, pads lit), chop-grid (a selection chopped by beat),
//! transform (Transform panel with a rendered preview), kit (drum-kit sheet),
//! plus splash (launch intro), celebrate (completion burst) and empty (empty library).
//! `STEM_SHOT_SIZE=WxH`, `STEM_SHOT_DELAY=secs`, `STEM_SHOT_POINTER=x,y`, `STEM_REDUCE_MOTION=1`.
//! More scenes: midi, midi-quantize (the "→ MIDI" sheet on the bass stem; set `STEM_MIDI_MODEL=nmp.onnx`
//! and `ORT_DYLIB_PATH` to run the real model instead of the demo stand-in).
//! Presets and chains: presets (start screen with a chain preset picked), chains (preset cards
//! on the Models page), preset-editor (editing a chain with an ensemble step), chain-progress
//! (step 2 of a chain running), stem-cache (Settings with the stem cache rows; use a tall
//! `STEM_SHOT_SIZE` to reach Storage).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use eframe::egui;
use stem_models::StemKind;

use crate::app::{Failure, Page, RunningJob, StemApp};
use crate::pipeline::{InputSource, JobOutput, Stage, StemOut};
use crate::settings::{HistoryEntry, now_unix};
use crate::waveform::{BUCKETS, Envelope};

pub struct Shot {
    pub path: PathBuf,
    pub scene: String,
    pub force_drop: bool,
    requested: bool,
    /// Seconds to wait before capturing.
    delay: f64,
}

impl Shot {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("STEM_SHOT")?;
        Some(Self {
            path: PathBuf::from(path),
            scene: std::env::var("STEM_SCENE").unwrap_or_else(|_| "home".into()),
            force_drop: false,
            requested: false,
            delay: std::env::var("STEM_SHOT_DELAY")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.6),
        })
    }

    pub fn setup(&mut self, app: &mut StemApp, _ctx: &egui::Context) {
        if std::env::var_os("STEM_REDUCE_MOTION").is_some() {
            app.settings.reduce_motion = true;
        }
        let ago = |h: u64| now_unix().saturating_sub(h * 3600);
        app.settings.history = vec![
            HistoryEntry {
                title: "Midnight Drive (demo beat)".into(),
                source: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
                model_name: "HTDemucs 4-stem".into(),
                stems: vec![
                    StemKind::Vocals,
                    StemKind::Drums,
                    StemKind::Bass,
                    StemKind::Other,
                ],
                out_dir: std::env::temp_dir().join("stem-splitter-demo/Midnight Drive"),
                when: ago(0),
                duration_secs: 32.0,
                bpm: Some(90.0),
                key: Some("C major".into()),
            },
            HistoryEntry {
                title: "Soul Sample Flip 1972".into(),
                source: "https://soundcloud.com/someone/soul-flip".into(),
                model_name: "HTDemucs 6-stem".into(),
                stems: vec![
                    StemKind::Vocals,
                    StemKind::Drums,
                    StemKind::Bass,
                    StemKind::Guitar,
                    StemKind::Piano,
                    StemKind::Other,
                ],
                out_dir: std::env::temp_dir().join("stem-splitter-demo/Soul Sample Flip 1972"),
                when: ago(5),
                duration_secs: 214.0,
                bpm: Some(96.0),
                key: Some("Eb major".into()),
            },
            HistoryEntry {
                title: "voice memo 03".into(),
                source: "C:\\Users\\me\\Music\\voice memo 03.m4a".into(),
                model_name: "MDX-Net Inst HQ3".into(),
                stems: vec![StemKind::Vocals, StemKind::Instrumental],
                out_dir: std::env::temp_dir().join("stem-splitter-demo/voice memo 03"),
                when: ago(30),
                duration_secs: 61.0,
                bpm: None,
                key: None,
            },
            HistoryEntry {
                title: "Interview clip - x.com".into(),
                source: "https://x.com/someone/status/1".into(),
                model_name: "Cinematic DnR".into(),
                stems: vec![StemKind::Dialogue, StemKind::Music, StemKind::Effects],
                out_dir: std::env::temp_dir().join("stem-splitter-demo/Interview clip"),
                when: ago(80),
                duration_secs: 95.0,
                bpm: None,
                key: None,
            },
        ];
        match self.scene.as_str() {
            "drop" => self.force_drop = true,
            "progress" => {
                app.url_input = "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into();
                let model = app.backend.model(None).cloned();
                app.job = Some(RunningJob {
                    id: 999,
                    source: InputSource::Url(app.url_input.clone()),
                    title: "Midnight Drive (demo beat)".into(),
                    model_name: model.as_ref().map_or("", |m| m.name).into(),
                    stems: model.map(|m| m.stems).unwrap_or_default(),
                    stage: Stage::Separating,
                    detail: "HTDemucs 4-stem".into(),
                    frac: Some(0.62),
                    started: -48.0,
                    cancel: Arc::new(AtomicBool::new(false)),
                    shown: Stage::Separating.overall(0.62),
                    reopening: false,
                });
            }
            "queue" => {
                use crate::queue::ItemStatus;
                let model = app.backend.model(None).cloned();
                let src = |u: &str| InputSource::Url(u.to_owned());
                let q = &mut app.queue;
                let done = q.push(src("https://www.youtube.com/watch?v=a1"), false);
                let run = q.push(src("https://soundcloud.com/someone/soul-flip"), false);
                let p1 = q.push(src("https://www.youtube.com/watch?v=c3"), false);
                q.push(src("https://x.com/someone/status/1"), false);
                let pl = q.push(src("https://www.youtube.com/playlist?list=PLdemo"), true);
                let bad = q.push(src("https://www.youtube.com/watch?v=gone"), false);
                let entry = app.settings.history[0].clone();
                let titles = [
                    (done, "Midnight Drive (demo beat)"),
                    (run, "Soul Sample Flip 1972"),
                    (p1, "Dusty Breaks Vol. 2"),
                    (pl, "Playlist: crate digging"),
                ];
                for (id, t) in titles {
                    if let Some(i) = q.get_mut(id) {
                        i.title = t.into();
                    }
                }
                if let Some(i) = q.get_mut(done) {
                    i.status = ItemStatus::Done(Box::new(entry));
                }
                if let Some(i) = q.get_mut(run) {
                    i.status = ItemStatus::Running { job_id: 999 };
                }
                if let Some(i) = q.get_mut(bad) {
                    i.status = ItemStatus::Failed("yt-dlp: Video unavailable".into());
                }
                app.job = Some(RunningJob {
                    id: 999,
                    source: src("https://soundcloud.com/someone/soul-flip"),
                    title: "Soul Sample Flip 1972".into(),
                    model_name: model.as_ref().map_or("", |m| m.name).into(),
                    stems: model.map(|m| m.stems).unwrap_or_default(),
                    stage: Stage::Separating,
                    detail: "HTDemucs 4-stem".into(),
                    frac: Some(0.37),
                    started: -21.0,
                    cancel: Arc::new(AtomicBool::new(false)),
                    shown: Stage::Separating.overall(0.37),
                    reopening: false,
                });
            }
            "results" | "results-zoom" | "celebrate" | "export" | "chop" | "chop-grid"
            | "transform" | "kit" | "midi" | "midi-quantize" => {
                let kinds = [
                    StemKind::Vocals,
                    StemKind::Drums,
                    StemKind::Bass,
                    StemKind::Other,
                ];
                let stems = kinds
                    .iter()
                    .map(|k| {
                        let audio = crate::demo::stem(*k);
                        let env = Envelope::from_buffer(&audio, BUCKETS);
                        StemOut {
                            kind: *k,
                            audio,
                            path: PathBuf::from(format!("{}.wav", k.label().to_lowercase())),
                            env,
                        }
                    })
                    .collect::<Vec<StemOut>>();
                let refs: Vec<_> = stems.iter().map(|s| (s.kind, &s.audio)).collect();
                let analysis = crate::analysis::analyze_stems(&refs);
                app.open_session(JobOutput {
                    title: crate::demo::DEMO_TITLE.into(),
                    out_dir: std::env::temp_dir().join("stem-splitter-demo/Midnight Drive"),
                    stems,
                    analysis,
                });
                if let Some(s) = &mut app.session {
                    s.selection = Some((10.67, 13.33));
                    s.sel_track = 1;
                    s.tracks[2].solo = false;
                    s.tracks[3].mute = true;
                    s.tracks[0].volume = 0.8;
                    s.tracks[0].level = 0.7;
                    s.tracks[1].level = 0.9;
                    s.tracks[2].level = 0.5;
                }
                if self.scene == "results-zoom"
                    && let Some(s) = &mut app.session
                {
                    s.timeline.zoom_around(12.0, 3.5);
                    s.selection = s.analysis.as_ref().and_then(|a| a.grid.as_ref()).map(|g| {
                        (
                            g.snap(10.4, stem_audio::analysis::GridDivision::Bar),
                            g.snap(13.1, stem_audio::analysis::GridDivision::Beat),
                        )
                    });
                }
                app.player.seek(12.1);
                if let Some(s) = &mut app.session
                    && self.scene.starts_with("chop")
                {
                    use crate::chop::{ChopMode, ChopSettings, GridChoice};
                    s.chop.open = true;
                    s.chop.settings = if self.scene == "chop" {
                        ChopSettings {
                            stem: Some(StemKind::Drums),
                            // Two bars from the selection.
                            region: s.selection.map(|(a, b)| (a, a + 2.0 * (b - a))),
                            ..Default::default()
                        }
                    } else {
                        ChopSettings {
                            stem: Some(StemKind::Other),
                            mode: ChopMode::Grid,
                            grid: GridChoice::HalfBeat,
                            region: s.selection.map(|(a, _)| (a, a + 8.0 * 0.6667)),
                            choke: true,
                            ..Default::default()
                        }
                    };
                    s.chop.retarget();
                }
                if self.scene.starts_with("midi") {
                    if let Some(s) = &mut app.session {
                        s.selection = None;
                    }
                    if self.scene == "midi-quantize" {
                        app.midi_prefs.quantize =
                            Some(stem_models::transcribe::QuantizeGrid::Sixteenth);
                    }
                    crate::views::midi::open_sheet(app, 2, false, _ctx);
                }
                if self.scene == "transform" || self.scene == "kit" {
                    dsp_scene(app, &self.scene);
                } else if self.scene == "export" {
                    app.settings.export_without = true;
                    app.settings.export_format = crate::settings::ExportFormat::Flac24;
                    app.export_all_stems();
                } else {
                    if !self.scene.starts_with("chop") && !self.scene.starts_with("midi") {
                        app.toasts.success("4 stems ready", crate::demo::DEMO_TITLE);
                    }
                }
                if self.scene == "celebrate" {
                    app.fx.celebrate(
                        kinds.iter().map(|k| crate::theme::stem_color(*k)).collect(),
                        "4 STEMS READY",
                    );
                }
            }
            "empty" => {
                app.page = Page::Library;
                app.settings.history.clear();
            }
            "models" => {
                app.page = Page::Models;
                if let Some(m) = app.backend.models.get(1) {
                    let total = m.size_bytes.max(1);
                    app.model_downloads.insert(
                        m.id.to_owned(),
                        crate::app::ModelDl {
                            status: stem_models::DownloadStatus {
                                done: total * 46 / 100,
                                total: Some(total),
                                bytes_per_sec: 9.4e6,
                                resumed_from: 0,
                            },
                            cancel: Arc::new(AtomicBool::new(false)),
                        },
                    );
                }
            }
            "presets" | "chains" | "preset-editor" | "chain-progress" | "stem-cache" => {
                preset_scene(app, &self.scene);
            }
            "library" => app.page = Page::Library,
            "settings" => app.page = Page::Settings,
            "error" => {
                app.failure = Some(Failure {
                    title: "Link from x.com".into(),
                    message: "yt-dlp: This post is age-restricted. Pick a browser under Settings → Cookies so yt-dlp can use your login.".into(),
                    source: Some(InputSource::Url("https://x.com/someone/status/1".into())),
                });
            }
            _ => {
                app.url_input = String::new();
            }
        }
    }

    /// Screenshots ignore the real (virtual-display) pointer, so nothing looks hovered by
    /// accident. `STEM_SHOT_POINTER=x,y` parks a pointer there instead, for hover shots.
    pub fn filter_input(&self, raw: &mut egui::RawInput) {
        raw.events.retain(|e| {
            !matches!(
                e,
                egui::Event::PointerMoved(_)
                    | egui::Event::PointerButton { .. }
                    | egui::Event::PointerGone
            )
        });
        if let Some((x, y)) = std::env::var("STEM_SHOT_POINTER").ok().and_then(|s| {
            let (x, y) = s.split_once(',')?;
            Some((x.parse::<f32>().ok()?, y.parse::<f32>().ok()?))
        }) {
            raw.events.push(egui::Event::PointerMoved(egui::pos2(x, y)));
        } else {
            raw.events.push(egui::Event::PointerGone);
        }
    }

    pub fn frame(&mut self, app: &mut StemApp, ctx: &egui::Context) {
        ctx.request_repaint();
        let t = ctx.input(|i| i.time);
        // Chop scenes: finger-drum a few pads just before the capture so they flash.
        if self.scene.starts_with("chop")
            && let Some(s) = &mut app.session
        {
            // Relative to now, so the flash looks the same whenever the capture lands.
            let at = t;
            let hits: &[(usize, f64, f32)] = if self.scene == "chop" {
                &[
                    (0, 0.5, 1.0),
                    (5, 0.12, 0.8),
                    (2, 0.04, 1.0),
                    (10, 0.3, 0.9),
                ]
            } else {
                &[(3, 0.03, 1.0), (8, 0.25, 0.7)]
            };
            for &(i, ago, v) in hits {
                if let Some(h) = s.chop.hits.get_mut(i) {
                    h.at = at - ago;
                    h.velocity = v;
                }
            }
            s.chop.held = if self.scene == "chop" {
                vec![2]
            } else {
                vec![3]
            };
            s.chop.hover = Some(if self.scene == "chop" { 7 } else { 5 });
        }
        if !self.requested && t > self.delay {
            self.requested = true;
            let path = self.path.clone();
            let ctx2 = ctx.clone();
            ctx.request_screenshot(move |img| {
                let [w, h] = img.size;
                let mut bytes = Vec::with_capacity(w * h * 4);
                for c in &img.pixels {
                    bytes.extend_from_slice(&c.to_srgba_unmultiplied());
                }
                match image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                    Some(buf) => match buf.save(&path) {
                        Ok(()) => eprintln!("saved screenshot to {}", path.display()),
                        Err(e) => eprintln!("screenshot save failed: {e}"),
                    },
                    None => eprintln!("bad screenshot buffer"),
                }
                ctx2.send_viewport_cmd(egui::ViewportCommand::Close);
            });
        }
    }
}

/// Transform panel with a finished preview render, or the kit sheet with extracted hits.
/// Work runs inline so the capture never races a worker thread.
fn dsp_scene(app: &mut StemApp, scene: &str) {
    use std::sync::atomic::AtomicU32;
    crate::fx::poll(app);
    let Some(drums) = app
        .session
        .as_ref()
        .and_then(|s| s.tracks.iter().position(|t| t.kind == StemKind::Drums))
    else {
        return;
    };
    let ctx = app.dsp.ctx();
    if scene == "kit" {
        let audio = app.session.as_ref().map(|s| s.audio[drums].clone());
        if let Some(audio) = audio {
            crate::fx::open_kit_blocking(app, drums, &audio);
        }
        return;
    }
    if let Some(s) = &mut app.session {
        s.selection = Some((10.67, 13.33));
        s.sel_track = 0;
    }
    app.dsp.ui.stem = 0;
    app.dsp.ui.bpm = 95.0;
    app.dsp.ui.semis = 3;
    app.dsp.scroll_to_panel = true;
    if let Ok(key) = crate::fx::current_key(app)
        && let Some(s) = &app.session
    {
        let t0 = std::time::Instant::now();
        let never = AtomicBool::new(false);
        if let Ok(audio) = crate::fx::render_key(&s.audio, &key, &AtomicU32::new(0), &never, &ctx) {
            app.dsp.last = Some(crate::fx::Rendered {
                key,
                audio: Arc::new(audio),
                secs: t0.elapsed().as_secs_f64(),
            });
        }
    }
}

/// Presets and chains scenes: one user preset next to the built-ins, a chain picked.
fn preset_scene(app: &mut StemApp, scene: &str) {
    use crate::pipeline::chain::{Chain, Step, StepInput};
    use crate::settings::Preset;
    use stem_audio::ensemble::EnsembleMode;
    let flip = Preset {
        id: String::new(),
        name: "Flip kit (6-stem)".into(),
        description: "Vocal ensemble, then guitar and piano out of the beat for chops.".into(),
        chain: Chain {
            steps: vec![
                Step {
                    input: StepInput::Mix,
                    models: vec!["mel-roformer-kim".into(), "mdx-kim-vocal-2".into()],
                    ensemble: EnsembleMode::MaxSpec,
                },
                Step::new(StepInput::Stem(StemKind::Instrumental), "htdemucs-6s"),
            ],
        },
    };
    let id = app.settings.save_preset(flip);
    app.settings.preset_id = Some("builtin:beatmaker-4".into());
    match scene {
        "presets" => {}
        "chains" => app.page = Page::Models,
        "preset-editor" => {
            app.page = Page::Models;
            app.settings.preset_id = Some(id.clone());
            if let Some(preset) = app.settings.find_preset(&id) {
                app.preset_editor = Some(crate::views::presets::PresetEditor {
                    preset,
                    existing: true,
                });
            }
        }
        "chain-progress" => {
            let spec = app
                .settings
                .find_preset("builtin:beatmaker-4")
                .and_then(|p| app.chain_spec(&p).ok());
            app.job = Some(RunningJob {
                id: 999,
                source: InputSource::Url("https://www.youtube.com/watch?v=dQw4w9WgXcQ".into()),
                title: "Midnight Drive (demo beat)".into(),
                model_name: "Beatmaker 4-stem".into(),
                stems: spec
                    .and_then(|s| s.plan().ok())
                    .map(|p| p.kinds())
                    .unwrap_or_default(),
                stage: Stage::Separating,
                detail: "Step 2/2 · HTDemucs on instrumental".into(),
                frac: Some(0.71),
                started: -63.0,
                cancel: Arc::new(AtomicBool::new(false)),
                shown: Stage::Separating.overall(0.71),
                reopening: false,
            });
        }
        _ => {
            app.page = Page::Settings;
            app.stem_cache_size = Some(3_420_000_000);
        }
    }
}
