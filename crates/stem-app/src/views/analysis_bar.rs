//! Results-page pieces for music analysis: the BPM · key · Camelot chips with halve / double,
//! the snap control, and the overview strip in the transport bar.

use eframe::egui::{self, Rect, vec2};

use crate::analysis::{SnapMode, bpm_label};
use crate::app::StemApp;
use crate::theme;
use crate::widgets::buttons;
use crate::widgets::overview::Overview;

/// Chips row under the results title.
pub fn chips(app: &mut StemApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    let mut rescale = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        match &s.analysis {
            None => {
                ui.label(
                    egui::RichText::new("No tempo / key")
                        .font(theme::regular(12.5))
                        .color(theme::TEXT_FAINT),
                );
            }
            Some(a) => {
                if let Some(t) = a.tempo {
                    let detected = a.detected_bpm.map_or(String::new(), |d| {
                        if (d - t.bpm).abs() > 0.01 {
                            format!(" (detected {})", bpm_label(d))
                        } else {
                            String::new()
                        }
                    });
                    buttons::chip(ui, &format!("{} BPM", bpm_label(t.bpm)), theme::PINK, false)
                        .on_hover_text(format!(
                            "Tempo{detected} · confidence {:.0}%",
                            t.confidence * 100.0
                        ));
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if buttons::letter_toggle(ui, "½", 24.0, false, theme::PINK, "Halve tempo")
                        .clicked()
                    {
                        rescale = Some(0.5);
                    }
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if buttons::letter_toggle(ui, "×2", 24.0, false, theme::PINK, "Double tempo")
                        .clicked()
                    {
                        rescale = Some(2.0);
                    }
                    ui.add_space(6.0);
                }
                if let Some(k) = a.key {
                    let tip = format!("Key · confidence {:.0}%", k.confidence * 100.0);
                    buttons::chip(ui, &k.name(), theme::VIOLET, false).on_hover_text(&tip);
                    buttons::chip(ui, &k.camelot(), theme::CYAN, false)
                        .on_hover_text("Camelot wheel: mix with the same number, ±1, or A↔B");
                }
            }
        }
        if s.analysis.as_ref().is_some_and(|a| a.grid.is_some()) {
            ui.add_space(18.0);
            ui.label(
                egui::RichText::new("SNAP")
                    .font(theme::semi(10.5))
                    .color(theme::TEXT_FAINT),
            );
            let opts: Vec<(SnapMode, &str)> =
                SnapMode::ALL.iter().map(|m| (*m, m.label())).collect();
            buttons::segmented(ui, "snap", &mut app.settings.snap, &opts);
        }
    });
    if let Some(f) = rescale
        && let Some(a) = &mut s.analysis
        && a.rescale(f)
        && let Some(bpm) = a.bpm()
        && let Some(h) = app
            .settings
            .history
            .iter_mut()
            .find(|h| h.out_dir == s.out_dir)
    {
        h.bpm = Some(bpm);
    }
}

/// Overview strip at the top of the transport bar.
pub fn overview(app: &mut StemApp, ui: &mut egui::Ui, rect: Rect) {
    let playhead = app.player.position();
    let Some(s) = &mut app.session else { return };
    let envs = s
        .tracks
        .iter()
        .enumerate()
        .filter(|(i, _)| s.audible(*i))
        .map(|(_, t)| (&t.env, theme::stem_color(t.kind)))
        .collect();
    let seek = Overview {
        envs,
        playhead,
        selection: s.selection,
    }
    .show(ui, rect, &mut s.timeline);
    if let Some(t) = seek {
        app.player.seek(t);
    }
}

/// Height the overview strip takes at the top of the transport.
pub const OVERVIEW_H: f32 = 18.0;

/// Rect for the overview strip inside the transport panel's `panel` rect.
pub fn overview_rect(panel: Rect) -> Rect {
    Rect::from_min_size(panel.min + vec2(0.0, 8.0), vec2(panel.width(), OVERVIEW_H))
}
