//! Settings → Processing rows: split quality, and the GPU failures the app remembers.

use eframe::egui;

use super::settings::row;
use crate::app::StemApp;
use crate::gpu_health::{self, GpuFailure};
use crate::quality::SplitQuality;
use crate::settings::now_unix;
use crate::theme;
use crate::widgets::buttons::{self, Button};
use crate::widgets::icons::Icon;

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .font(theme::regular(12.0))
            .color(theme::TEXT_FAINT),
    );
}

/// "Split quality: Fast / Balanced / Best".
pub fn quality_row(app: &mut StemApp, ui: &mut egui::Ui) {
    row(ui, "Split quality", |ui| {
        let opts: Vec<(SplitQuality, &str)> =
            SplitQuality::ALL.iter().map(|q| (*q, q.label())).collect();
        buttons::segmented(ui, "split-quality", &mut app.settings.split_quality, &opts);
        hint(
            ui,
            match app.settings.split_quality {
                SplitQuality::Fast => "each model's own chunk overlap",
                SplitQuality::Balanced => "more overlap, fewer seams · about 1.5–2× the time",
                SplitQuality::Best => "most overlap · about 3–4× the time",
            },
        );
    });
}

/// Failures to list: the file's, or the screenshot scene's stand-ins.
fn remembered(app: &StemApp, now: u64) -> Vec<GpuFailure> {
    if let Some(demo) = gpu_health::demo_entries() {
        return demo;
    }
    gpu_health::cached_active(&gpu_health::file(&app.backend.paths.data), now)
}

/// "GPU failures": models that failed on DirectML and run on the CPU for a week, with a way to
/// try the GPU again.
pub fn gpu_failures_row(app: &mut StemApp, ui: &mut egui::Ui) {
    let now = now_unix();
    let list = remembered(app, now);
    let path = gpu_health::file(&app.backend.paths.data);
    row(ui, "GPU failures", |ui| {
        if list.is_empty() {
            hint(
                ui,
                "None · a model that fails on the GPU retries on the CPU and stays there for 7 days",
            );
            return;
        }
        if Button::ghost("Forget all · retry GPU")
            .icon(Icon::Refresh)
            .height(32.0)
            .tooltip("Run these models on the GPU again next time")
            .show(ui)
            .clicked()
        {
            gpu_health::update(&path, now, |f| f.entries.clear());
            gpu_health::clear_demo_entries();
        }
        hint(ui, "each model stays on the CPU for 7 days per GPU");
    });
    for f in &list {
        ui.horizontal(|ui| {
            ui.add_space(190.0);
            let name = if f.model_name.is_empty() {
                f.model_id.as_str()
            } else {
                f.model_name.as_str()
            };
            ui.label(
                egui::RichText::new(crate::pipeline::chain::short_model_name(name))
                    .font(theme::semi(13.0))
                    .color(theme::TEXT),
            )
            .on_hover_text(&f.reason);
            ui.label(
                egui::RichText::new(format!(
                    "on {} · CPU for {}",
                    f.adapter,
                    gpu_health::time_left(f, now)
                ))
                .font(theme::regular(12.5))
                .color(theme::TEXT_DIM),
            )
            .on_hover_text(&f.reason);
            if Button::ghost("Retry GPU")
                .height(28.0)
                .tooltip("Forget this failure: the next run tries the GPU again")
                .show(ui)
                .clicked()
            {
                gpu_health::update(&path, now, |all| all.forget(&f.model_id, &f.adapter));
                gpu_health::forget_demo_entry(&f.model_id);
            }
        });
    }
}
