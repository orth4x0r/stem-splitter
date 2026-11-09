#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Stem Splitter: paste a link or drop a file, split it into stems, chop samples.
//!
//! Environment switches (handy for development):
//! * `STEM_DEMO=1`  synthetic stems and fake progress; no backend needed.
//! * `STEM_SHOT=out.png STEM_SCENE=<scene>`  render a scene, save a screenshot, exit
//!   (see `shot.rs`).

mod analysis;
mod app;
mod audition;
mod backend;
mod chop;
mod chop_export;
mod demo;
mod dragout;
mod effects;
mod export;
mod fx;
mod midi;
mod model_cache;
mod pads;
mod paths;
mod pipeline;
mod player;
mod queue;
mod refine;
mod section;
mod settings;
mod shot;
mod splash;
mod theme;
mod timeline;
mod views;
mod waveform;
mod widgets;

use eframe::egui;

fn app_icon() -> egui::IconData {
    // A gradient disc with a dark hole: the vinyl / logo mark, drawn at startup.
    let n = 64usize;
    let mut rgba = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let fx = (x as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let fy = (y as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let d = (fx * fx + fy * fy).sqrt();
            let c = theme::gradient_at(&theme::ACCENT_GRADIENT, (fx + fy + 2.0) / 4.0);
            let a = ((1.0 - d) * n as f32 / 2.0).clamp(0.0, 1.0);
            let hole = d < 0.16;
            let groove = d > 0.4 && ((d * 40.0) as i32) % 3 == 0;
            let (r, g, b) = if hole {
                (12, 12, 18)
            } else if groove {
                (c.r() / 2 + 40, c.g() / 2 + 20, c.b() / 2 + 40)
            } else {
                (c.r(), c.g(), c.b())
            };
            rgba.extend_from_slice(&[r, g, b, (a * 255.0) as u8]);
        }
    }
    egui::IconData {
        rgba,
        width: n as u32,
        height: n as u32,
    }
}

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,stem_app=info".into()),
        )
        .init();
    let demo = std::env::var("STEM_DEMO").is_ok_and(|v| v != "0" && !v.is_empty());
    let shot = shot::Shot::from_env();
    let size = if shot.is_some() {
        // STEM_SHOT_SIZE=1920x1080 renders other window sizes.
        std::env::var("STEM_SHOT_SIZE")
            .ok()
            .and_then(|s| {
                let (w, h) = s.split_once('x')?;
                Some([w.parse().ok()?, h.parse().ok()?])
            })
            .unwrap_or([1440.0, 900.0])
    } else {
        [1320.0, 860.0]
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Stem Splitter")
            .with_inner_size(size)
            .with_min_inner_size([1000.0, 680.0])
            .with_drag_and_drop(true)
            .with_icon(app_icon()),
        persist_window: shot.is_none(),
        ..Default::default()
    };
    eframe::run_native(
        "Stem Splitter",
        options,
        Box::new(move |cc| Ok(Box::new(app::StemApp::new(cc, demo, shot)))),
    )
}
