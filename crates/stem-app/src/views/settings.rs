//! Settings: output, formats, sample export, downloads, processing, storage.

use eframe::egui::{self, Align2, Rect, Sense, pos2, vec2};

use crate::app::StemApp;
use crate::settings::{COOKIE_BROWSERS, WavChoice};
use crate::theme;
use crate::widgets::buttons::{self, Button};
use crate::widgets::icons::Icon;
use crate::widgets::toast::elide;
use crate::widgets::{page_title, progress};

fn section(ui: &mut egui::Ui, title: &str, subtitle: &str, add: impl FnOnce(&mut egui::Ui)) {
    let shown = theme::glass_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            egui::RichText::new(title)
                .font(theme::semi(16.0))
                .color(theme::TEXT),
        );
        if !subtitle.is_empty() {
            ui.label(
                egui::RichText::new(subtitle)
                    .font(theme::regular(12.5))
                    .color(theme::TEXT_DIM),
            );
        }
        ui.add_space(8.0);
        add(ui);
    });
    crate::widgets::paint::glass_sheen(ui.painter(), shown.response.rect, theme::R_CARD, 0.0);
    ui.add_space(14.0);
}

pub(super) fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(190.0, 34.0), Sense::hover());
        ui.painter().text(
            pos2(r.left(), r.center().y),
            Align2::LEFT_CENTER,
            label,
            theme::regular(14.0),
            theme::TEXT_DIM,
        );
        add(ui);
    });
}

pub fn page(app: &mut StemApp, ui: &mut egui::Ui) {
    page_title(ui, "Settings", "");
    let max = ui.available_width().min(860.0);
    ui.set_max_width(max);

    section(
        ui,
        "Output",
        "Where stems and samples are saved. Each track gets its own folder.",
        |ui| {
            row(ui, "Folder", |ui| {
                let path = app.settings.output_dir.display().to_string();
                let (r, _) = ui.allocate_exact_size(
                    vec2((ui.available_width() - 250.0).max(120.0), 34.0),
                    Sense::hover(),
                );
                ui.painter()
                    .rect_filled(r, 10, theme::alpha(eframe::egui::Color32::BLACK, 0.35));
                ui.painter().text(
                    pos2(r.left() + 12.0, r.center().y),
                    Align2::LEFT_CENTER,
                    elide(&path, 52),
                    theme::mono(12.5),
                    theme::TEXT,
                );
                if Button::new("Browse")
                    .icon(Icon::Folder)
                    .height(34.0)
                    .show(ui)
                    .clicked()
                    && app.shot.is_none()
                    && let Some(d) = rfd::FileDialog::new()
                        .set_directory(&app.settings.output_dir)
                        .pick_folder()
                {
                    app.settings.output_dir = d;
                }
                if Button::ghost("Open").height(34.0).show(ui).clicked() {
                    let d = app.settings.output_dir.clone();
                    app.open_folder(&d);
                }
            });
            row(ui, "Stem files (WAV)", |ui| {
                let opts: Vec<(WavChoice, &str)> =
                    WavChoice::ALL.iter().map(|w| (*w, w.label())).collect();
                buttons::segmented(ui, "wav", &mut app.settings.wav, &opts);
            });
            super::settings_tools::export_format_row(app, ui);
            super::settings_tools::naming_row(app, ui);
        },
    );

    section(
        ui,
        "Samples",
        "Applied when you export a selection.",
        |ui| {
            row(ui, "Normalize to -0.1 dB", |ui| {
                buttons::toggle(ui, &mut app.settings.normalize_samples);
            });
            row(ui, "Fade in / out", |ui| {
                buttons::slider(
                    ui,
                    &mut app.settings.sample_fade_ms,
                    0.0..=50.0,
                    " ms",
                    240.0,
                );
            });
            row(ui, "Loop selection", |ui| {
                buttons::toggle(ui, &mut app.settings.loop_region);
            });
        },
    );

    section(
        ui,
        "Downloads",
        "Links are fetched with yt-dlp, which the app installs and keeps up to date.",
        |ui| {
            row(ui, "Browser cookies", |ui| {
                let current = app
                    .settings
                    .cookies_from_browser
                    .clone()
                    .unwrap_or_else(|| "None".into());
                egui::ComboBox::from_id_salt("cookies")
                    .selected_text(capitalize(&current))
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut app.settings.cookies_from_browser, None, "None");
                        for b in COOKIE_BROWSERS {
                            ui.selectable_value(
                                &mut app.settings.cookies_from_browser,
                                Some(b.to_owned()),
                                capitalize(b),
                            );
                        }
                    });
                ui.label(
                    egui::RichText::new("private / age-restricted posts · Firefox works best")
                        .font(theme::regular(12.0))
                        .color(theme::TEXT_FAINT),
                );
            });
            row(ui, "yt-dlp", |ui| {
                let busy = app.ytdlp_busy;
                if Button::new(if busy { "Updating…" } else { "Update yt-dlp" })
                    .icon(Icon::Refresh)
                    .enabled(!busy)
                    .height(34.0)
                    .show(ui)
                    .clicked()
                {
                    app.update_ytdlp();
                }
                if busy {
                    let (r, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                    progress::dots(
                        ui.painter(),
                        r.center(),
                        8.0,
                        ui.input(|i| i.time),
                        theme::CYAN,
                    );
                }
                ui.label(
                    egui::RichText::new("Sites change often. Update if a link stops working.")
                        .font(theme::regular(12.0))
                        .color(theme::TEXT_FAINT),
                );
            });
            super::settings_tools::ffmpeg_row(app, ui);
        },
    );

    section(
        ui,
        "Processing",
        "GPU (DirectML) is much faster. Auto picks the best adapter and falls back to CPU.",
        |ui| {
            super::models::device_picker(app, ui);
        },
    );

    section(
        ui,
        "Storage",
        "Models, yt-dlp, downloads and cached model output live here.",
        |ui| {
            row(ui, "Data folder", |ui| {
                let path = app.backend.paths.data.display().to_string();
                let (r, _) = ui.allocate_exact_size(
                    vec2((ui.available_width() - 110.0).max(120.0), 34.0),
                    Sense::hover(),
                );
                ui.painter()
                    .rect_filled(r, 10, theme::alpha(eframe::egui::Color32::BLACK, 0.35));
                ui.painter().text(
                    pos2(r.left() + 12.0, r.center().y),
                    Align2::LEFT_CENTER,
                    elide(&path, 60),
                    theme::mono(12.5),
                    theme::TEXT,
                );
                if Button::ghost("Open")
                    .icon(Icon::Folder)
                    .height(34.0)
                    .show(ui)
                    .clicked()
                {
                    let d = app.backend.paths.data.clone();
                    app.open_folder(&d);
                }
            });
            super::settings_tools::cache_row(app, ui);
            super::presets::cache_rows(app, ui);
        },
    );

    appearance(app, ui);

    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let _ = Rect::NOTHING;
    ui.painter().text(
        pos2(r.left(), r.center().y),
        Align2::LEFT_CENTER,
        format!(
            "Stem Splitter {}  ·  made for flips, chops and remixes",
            env!("CARGO_PKG_VERSION")
        ),
        theme::regular(12.0),
        theme::TEXT_FAINT,
    );
}

/// Look & motion: accessibility switches for the animated parts of the UI.
fn appearance(app: &mut StemApp, ui: &mut egui::Ui) {
    section(
        ui,
        "Look & motion",
        "Ambient light, springs, confetti and the launch intro. Progress indicators always move.",
        |ui| {
            row(ui, "Reduce motion", |ui| {
                buttons::toggle_labeled(ui, &mut app.settings.reduce_motion, "Reduce motion");
                ui.label(
                    egui::RichText::new("Calmer UI: no drifting background, springs or bursts")
                        .font(theme::regular(12.0))
                        .color(theme::TEXT_FAINT),
                );
            });
            row(ui, "Launch intro", |ui| {
                let enabled = !app.settings.reduce_motion;
                ui.add_enabled_ui(enabled, |ui| {
                    buttons::toggle_labeled(ui, &mut app.settings.intro, "Launch intro");
                });
                ui.label(
                    egui::RichText::new("One-second logo build-up when the app starts")
                        .font(theme::regular(12.0))
                        .color(theme::TEXT_FAINT),
                );
            });
            row(ui, "Keyboard", |ui| {
                ui.label(
                    egui::RichText::new(
                        "Tab moves focus · Ctrl+1–4 switch pages · Space plays / pauses · ? lists every shortcut",
                    )
                    .font(theme::regular(12.5))
                    .color(theme::TEXT_DIM),
                );
            });
        },
    );
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or(String::new(), |f| {
        f.to_uppercase().collect::<String>() + c.as_str()
    })
}
