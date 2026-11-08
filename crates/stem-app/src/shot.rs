//! Headless screenshot mode for design reviews and docs.
//!
//! `STEM_DEMO=1 STEM_SHOT=out.png STEM_SCENE=results stem-splitter` sets up a scene with
//! synthetic data, waits for animations to settle, saves a PNG and exits.
//! Scenes: home, drop, progress, results, models, library, settings, error, queue, export.

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
            },
            HistoryEntry {
                title: "voice memo 03".into(),
                source: "C:\\Users\\me\\Music\\voice memo 03.m4a".into(),
                model_name: "MDX-Net Inst HQ3".into(),
                stems: vec![StemKind::Vocals, StemKind::Instrumental],
                out_dir: std::env::temp_dir().join("stem-splitter-demo/voice memo 03"),
                when: ago(30),
                duration_secs: 61.0,
            },
            HistoryEntry {
                title: "Interview clip - x.com".into(),
                source: "https://x.com/someone/status/1".into(),
                model_name: "Cinematic DnR".into(),
                stems: vec![StemKind::Dialogue, StemKind::Music, StemKind::Effects],
                out_dir: std::env::temp_dir().join("stem-splitter-demo/Interview clip"),
                when: ago(80),
                duration_secs: 95.0,
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
            "results" | "export" => {
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
                    .collect();
                app.open_session(JobOutput {
                    title: crate::demo::DEMO_TITLE.into(),
                    out_dir: std::env::temp_dir().join("stem-splitter-demo/Midnight Drive"),
                    stems,
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
                app.player.seek(12.1);
                if self.scene == "export" {
                    app.settings.export_without = true;
                    app.settings.export_format = crate::settings::ExportFormat::Flac24;
                    app.export_all_stems();
                } else {
                    app.toasts.success("4 stems ready", crate::demo::DEMO_TITLE);
                }
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
