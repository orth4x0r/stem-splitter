//! Headless screenshot mode for design reviews and docs.
//!
//! `STEM_DEMO=1 STEM_SHOT=out.png STEM_SCENE=results stem-splitter` sets up a scene with
//! synthetic data, waits for animations to settle, saves a PNG and exits.
//! Scenes: home, drop, progress, results, results-zoom, models, library, settings, error,
//! plus splash (launch intro), celebrate (completion burst) and empty (empty library).
//! `STEM_SHOT_SIZE=WxH`, `STEM_SHOT_DELAY=secs`, `STEM_SHOT_POINTER=x,y`, `STEM_REDUCE_MOTION=1`.

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
            "results" | "results-zoom" | "celebrate" => {
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
                app.toasts.success("4 stems ready", crate::demo::DEMO_TITLE);
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
                    app.model_downloads.insert(m.id.to_owned(), Some(0.46));
                }
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

    pub fn frame(&mut self, _app: &mut StemApp, ctx: &egui::Context) {
        ctx.request_repaint();
        let t = ctx.input(|i| i.time);
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
