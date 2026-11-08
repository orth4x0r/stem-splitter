//! Custom widgets and painting helpers.

pub mod buttons;
pub mod icons;
pub mod paint;
pub mod progress;
pub mod toast;
pub mod waveform_view;

use eframe::egui::{self, Align2, Color32, Rect, Sense, Ui, vec2};

use crate::theme;

/// Big page title with a small gradient underline and an optional subtitle.
pub fn page_title(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(title)
            .font(theme::bold(26.0))
            .color(theme::TEXT),
    );
    let (r, _) = ui.allocate_exact_size(vec2(44.0, 3.0), Sense::hover());
    paint::gradient_rect(ui.painter(), r, 2, 0.0, 1.0, Color32::WHITE);
    if !subtitle.is_empty() {
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(subtitle)
                .font(theme::regular(14.0))
                .color(theme::TEXT_DIM),
        );
    }
    ui.add_space(10.0);
}

/// Small uppercase label above a group of controls.
pub fn overline(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .font(theme::semi(11.0))
            .color(theme::TEXT_FAINT)
            .extra_letter_spacing(1.2),
    );
}

/// A centred empty-state message drawn in `rect`.
pub fn empty_state(ui: &Ui, rect: Rect, icon: icons::Icon, title: &str, body: &str) {
    let p = ui.painter();
    let c = rect.center();
    let ir = Rect::from_center_size(c - vec2(0.0, 40.0), vec2(44.0, 44.0));
    p.circle_filled(ir.center(), 34.0, theme::alpha(theme::VIOLET, 0.12));
    icons::paint(p, ir.shrink(8.0), icon, theme::VIOLET);
    p.text(
        c + vec2(0.0, 16.0),
        Align2::CENTER_CENTER,
        title,
        theme::semi(17.0),
        theme::TEXT,
    );
    p.text(
        c + vec2(0.0, 42.0),
        Align2::CENTER_CENTER,
        body,
        theme::regular(13.5),
        theme::TEXT_DIM,
    );
}
