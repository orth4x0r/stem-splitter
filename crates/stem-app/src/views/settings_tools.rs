//! Settings rows for tools and storage: export format, "Install ffmpeg", "Clear downloads cache".

use eframe::egui::{self, Rect, Sense, pos2, vec2};

use super::settings::row;
use crate::app::StemApp;
use crate::backend::human_bytes;
use crate::theme;
use crate::widgets::buttons::Button;
use crate::widgets::icons::Icon;
use crate::widgets::progress;
use crate::widgets::toast::elide;

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .font(theme::regular(12.0))
            .color(theme::TEXT_FAINT),
    );
}

pub fn export_format_row(app: &mut StemApp, ui: &mut egui::Ui) {
    row(ui, "Export format", |ui| {
        super::export_dialog::format_combo(ui, "settings-export", &mut app.settings.export_format);
        hint(
            ui,
            "samples and \"Export stems\" · FLAC is lossless and smaller",
        );
    });
}

pub fn ffmpeg_row(app: &mut StemApp, ui: &mut egui::Ui) {
    row(ui, "ffmpeg", |ui| {
        if app.ffmpeg.busy {
            let t = ui.input(|i| i.time);
            let (r, _) = ui.allocate_exact_size(vec2(220.0, 34.0), Sense::hover());
            let bar = Rect::from_min_size(pos2(r.left(), r.center().y + 6.0), vec2(r.width(), 5.0));
            progress::bar(ui.painter(), bar, app.ffmpeg.frac, t);
            let label = match app.ffmpeg.frac {
                Some(f) => format!("{}  {:.0}%", app.ffmpeg.msg, f * 100.0),
                None => format!("{}…", app.ffmpeg.msg),
            };
            ui.painter().text(
                pos2(r.left(), r.center().y - 6.0),
                egui::Align2::LEFT_CENTER,
                label,
                theme::mono(11.5),
                theme::TEXT_DIM,
            );
            return;
        }
        match app.ffmpeg_path() {
            Some(p) => {
                let (r, _) = ui.allocate_exact_size(vec2(10.0, 34.0), Sense::hover());
                ui.painter()
                    .circle_filled(pos2(r.center().x, r.center().y), 4.0, theme::GOOD);
                ui.label(
                    egui::RichText::new(elide(&p.display().to_string(), 56))
                        .font(theme::mono(12.0))
                        .color(theme::TEXT),
                );
            }
            None if cfg!(windows) || app.demo() => {
                if Button::new("Install ffmpeg")
                    .icon(Icon::Download)
                    .height(34.0)
                    .show(ui)
                    .clicked()
                {
                    app.install_ffmpeg();
                }
                hint(ui, "lets yt-dlp convert any stream (Opus / WebM) · ~100 MB");
            }
            None => hint(
                ui,
                "not found · install it with your package manager (e.g. apt install ffmpeg)",
            ),
        }
    });
}

pub fn cache_row(app: &mut StemApp, ui: &mut egui::Ui) {
    row(ui, "Downloads cache", |ui| {
        let size = app.downloads_size();
        let busy = app.job.is_some() || app.queue.has_work();
        let label = format!("Clear  ·  {}", human_bytes(size));
        if Button::new(&label)
            .icon(Icon::Close)
            .height(34.0)
            .enabled(size > 0 && !busy)
            .tooltip("Delete audio downloaded from links (your stems are kept)")
            .show(ui)
            .clicked()
        {
            app.clear_downloads();
        }
        hint(
            ui,
            if busy {
                "wait for the current split to finish"
            } else {
                "raw audio from links; stems and samples are not touched"
            },
        );
    });
}
