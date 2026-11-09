//! "Export stems" dialog: pick stems, format, naming template, and optional "without X" mixes.

use eframe::egui::{self, Align2, Sense, pos2, vec2};

use crate::app::StemApp;
use crate::export;
use crate::settings::ExportFormat;
use crate::theme::{self, motion};
use crate::widgets::buttons::{self, Button};
use crate::widgets::icons::Icon;
use crate::widgets::overline;
use crate::widgets::sheet::Sheet;
use crate::widgets::toast::elide;

/// Format picker shared by this dialog and Settings.
pub fn format_combo(ui: &mut egui::Ui, id: &str, value: &mut ExportFormat) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.label())
        .width(180.0)
        .show_ui(ui, |ui| {
            for f in ExportFormat::ALL {
                ui.selectable_value(value, f, f.label());
            }
        });
}

pub fn show(app: &mut StemApp, ctx: &egui::Context) {
    let (Some(dialog), Some(session)) = (&mut app.export_dialog, &app.session) else {
        return;
    };
    let kinds: Vec<_> = session.tracks.iter().map(|t| t.kind).collect();
    let title = session.title.clone();
    let mut close = false;
    let mut go = false;
    let mut browse = false;
    let mut sheet = Sheet::begin(ctx, egui::Id::unique("export-dialog"));
    let resp = sheet.show(ctx, theme::VIOLET, 24, |ui| {
        ui.set_width(560.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
            crate::widgets::icon_badge(ui.painter(), r, theme::VIOLET, Icon::Export);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Export stems")
                        .font(theme::bold(22.0))
                        .color(theme::TEXT),
                );
                ui.label(
                    egui::RichText::new(elide(&title, 60))
                        .font(theme::regular(13.0))
                        .color(theme::TEXT_DIM),
                );
            });
        });
        ui.add_space(14.0);

        overline(ui, "Stems");
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (i, k) in kinds.iter().enumerate() {
                let on = dialog.selected.get(i).copied().unwrap_or(false);
                if buttons::chip(ui, k.label(), theme::stem_color(*k), on)
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                    && let Some(v) = dialog.selected.get_mut(i)
                {
                    *v = !*v;
                }
            }
        });
        ui.add_space(12.0);

        let row = |ui: &mut egui::Ui, label: &str, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(150.0, 32.0), Sense::hover());
                ui.painter().text(
                    pos2(r.left(), r.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    theme::regular(13.5),
                    theme::TEXT_DIM,
                );
                add(ui);
            });
        };
        row(ui, "Format", &mut |ui| {
            format_combo(ui, "export-format", &mut app.settings.export_format);
        });
        row(ui, "File names", &mut |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut app.settings.export_template)
                    .font(theme::mono(13.0))
                    .desired_width(240.0),
            );
            ui.label(
                egui::RichText::new("{track} {stem} {n}")
                    .font(theme::mono(11.0))
                    .color(theme::TEXT_FAINT),
            );
        });
        row(ui, "\"Without\" mixes", &mut |ui| {
            buttons::toggle(ui, &mut app.settings.export_without);
            ui.label(
                egui::RichText::new(if kinds.len() >= 3 {
                    "also write every stem but one, e.g. no vocals"
                } else {
                    "needs a model with 3+ stems"
                })
                .font(theme::regular(12.0))
                .color(theme::TEXT_FAINT),
            );
        });
        row(ui, "Folder", &mut |ui| {
            ui.label(
                egui::RichText::new(elide(&dialog.dest.display().to_string(), 40))
                    .font(theme::mono(12.0))
                    .color(theme::TEXT),
            );
            if Button::ghost("Change…").height(28.0).show(ui).clicked() {
                browse = true;
            }
        });

        // Preview of the files that will be written.
        let items = export::plan(&kinds, &dialog.selected, app.settings.export_without);
        let fmt = app.settings.export_format.audio_format();
        ui.add_space(8.0);
        overline(ui, &format!("{} files", items.len()));
        ui.add_space(2.0);
        // The file preview sits in an inset well; rows cascade in.
        egui::Frame::new()
            .fill(egui::Color32::from_black_alpha(70))
            .stroke(egui::Stroke::new(
                1.0,
                theme::alpha(theme::GLASS_STROKE, 0.8),
            ))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.spacing_mut().interact_size.y = 18.0;
                for (n, item) in items.iter().enumerate().take(8) {
                    let path = export::item_path(
                        std::path::Path::new(""),
                        &app.settings.export_template,
                        &title,
                        item,
                        n,
                        fmt,
                    );
                    let e = motion::ease_out(motion::appear(
                        ui.ctx(),
                        egui::Id::unique(("export-row", n)),
                        0.12 + n as f32 * motion::STAGGER,
                        motion::SLOW,
                    ));
                    ui.scope(|ui| {
                        ui.multiply_opacity(e);
                        ui.horizontal(|ui| {
                            ui.add_space(10.0 * (1.0 - e));
                            // Single stems in their colour; "without" mixes neutral.
                            let c = match item.include.as_slice() {
                                [i] => kinds
                                    .get(*i)
                                    .map_or(theme::VIOLET, |k| theme::stem_color(*k)),
                                _ => theme::TEXT_FAINT,
                            };
                            let (r, _) = ui.allocate_exact_size(vec2(6.0, 14.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 2.5, c);
                            ui.label(
                                egui::RichText::new(elide(&path.display().to_string(), 70))
                                    .font(theme::mono(11.5))
                                    .color(theme::TEXT_DIM),
                            );
                        });
                    });
                }
                if items.len() > 8 {
                    ui.label(
                        egui::RichText::new(format!("… and {} more", items.len() - 8))
                            .font(theme::regular(11.5))
                            .color(theme::TEXT_FAINT),
                    );
                }
            });

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if Button::primary("Export")
                    .export_shimmer()
                    .icon(Icon::Export)
                    .enabled(!items.is_empty() && !app.export_busy)
                    .height(38.0)
                    .show(ui)
                    .clicked()
                {
                    go = true;
                }
                if Button::ghost("Cancel").height(38.0).show(ui).clicked() {
                    close = true;
                }
            });
        });
    });
    if sheet.closing {
        // Exit animation playing: the sheet is going away, ignore its buttons.
        (go, browse, close) = (false, false, false);
    }
    if browse
        && app.shot.is_none()
        && let Some(d) = app.export_dialog.as_mut()
        && let Some(dir) = rfd::FileDialog::new()
            .set_title("Export stems to…")
            .set_directory(&d.dest)
            .pick_folder()
    {
        d.dest = dir;
    }
    if go {
        app.run_export();
    } else if close || resp.should_close() {
        sheet.close(ctx);
    }
    if sheet.finished() {
        app.export_dialog = None;
    }
}
