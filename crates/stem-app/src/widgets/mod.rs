//! Custom widgets and painting helpers.

pub mod buttons;
pub mod drag_handle;
pub mod icons;
pub mod meter;
pub mod overview;
pub mod pads;
pub mod paint;
pub mod progress;
pub mod sheet;
pub mod toast;
pub mod waveform_view;

use eframe::egui::{self, Align2, Color32, CornerRadius, Id, Rect, Sense, Ui, pos2, vec2};

use crate::theme::{self, motion};

/// Page title with an optional subtitle. Hierarchy from size and colour, no decoration.
pub fn page_title(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(title)
            .font(theme::bold(theme::size::H1))
            .color(theme::TEXT),
    );
    if !subtitle.is_empty() {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(subtitle)
                .font(theme::regular(theme::size::BODY))
                .color(theme::TEXT_DIM),
        );
    }
    ui.add_space(theme::space::M);
}

/// Small sentence-case label above a group of controls.
pub fn overline(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(sentence_case(text))
            .font(theme::semi(theme::size::SMALL))
            .color(theme::TEXT_FAINT),
    );
}

/// "Stem cache" / "stem cache" -> "Stem cache": section labels are sentence case.
pub fn sentence_case(text: &str) -> String {
    let lower = if text.chars().any(|c| c.is_lowercase()) {
        text.to_owned()
    } else {
        text.to_lowercase()
    };
    let mut c = lower.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// A centred empty-state illustration drawn in `rect`: a glowing gradient disc with the icon,
/// a slowly orbiting ring of stem-coloured dots, title and body.
pub fn empty_state(ui: &Ui, rect: Rect, icon: icons::Icon, title: &str, body: &str) {
    let p = ui.painter();
    let c = rect.center() - vec2(0.0, 34.0);
    let a = motion::ease_out_back(motion::appear(
        ui.ctx(),
        Id::unique(("empty", title)),
        0.05,
        motion::ENTER * 1.4,
    ));
    paint::soft_disc(p, c, 130.0, theme::alpha(theme::VIOLET, 0.22 * a));
    // Orbit.
    p.circle_stroke(
        c,
        58.0 * a,
        egui::Stroke::new(1.0, Color32::from_white_alpha(14)),
    );
    let dots = [
        theme::stem_color(stem_models::StemKind::Vocals),
        theme::stem_color(stem_models::StemKind::Drums),
        theme::stem_color(stem_models::StemKind::Bass),
        theme::stem_color(stem_models::StemKind::Other),
    ];
    // The satellites swing into place during the entrance, then rest (idle = no repaints).
    for (i, col) in dots.iter().enumerate() {
        let ang = -0.8 + 2.4 * a + i as f32 * std::f32::consts::TAU / 4.0;
        let dp = c + vec2(ang.cos(), ang.sin()) * 58.0 * a;
        paint::glow_circle(p, dp, 3.0, theme::alpha(*col, 0.5), 10.0);
        p.circle_filled(dp, 4.0, *col);
    }
    let rr = 32.0 * a;
    if rr > 1.0 {
        paint::glow_circle(p, c, rr, theme::alpha(theme::PINK, 0.35), 24.0);
        p.circle(
            c,
            rr,
            theme::CARD_HI,
            egui::Stroke::new(1.0, theme::STROKE_HI),
        );
        icons::paint(
            p,
            Rect::from_center_size(c, vec2(24.0, 24.0) * a),
            icon,
            theme::TEXT_DIM,
        );
    }
    p.text(
        c + vec2(0.0, 92.0),
        Align2::CENTER_CENTER,
        title,
        theme::bold(theme::size::H2),
        theme::TEXT,
    );
    p.text(
        c + vec2(0.0, 120.0),
        Align2::CENTER_CENTER,
        body,
        theme::regular(13.5),
        theme::TEXT_DIM,
    );
}

/// A glass stat tile with a big count-up number (e.g. "4 sessions").
pub fn stat_tile(
    ui: &mut Ui,
    size: egui::Vec2,
    value: f32,
    fmt: impl Fn(f32) -> String,
    label: &str,
    color: Color32,
) {
    let (r, resp) = ui.allocate_exact_size(size, Sense::hover());
    let v = motion::count_up(ui.ctx(), resp.id.with("n"), value);
    let p = ui.painter();
    paint::glass(p, r, CornerRadius::same(14), None, 0.0);
    paint::soft_disc(
        p,
        pos2(r.left() + 30.0, r.center().y),
        60.0,
        theme::alpha(color, 0.14),
    );
    let bar = Rect::from_min_size(pos2(r.left(), r.top() + 14.0), vec2(3.0, r.height() - 28.0));
    paint::glow(p, bar, 2, theme::alpha(color, 0.6), 8.0);
    p.rect_filled(bar, 2, color);
    p.text(
        pos2(r.left() + 18.0, r.top() + r.height() * 0.42),
        Align2::LEFT_CENTER,
        fmt(v),
        theme::bold(24.0),
        theme::TEXT,
    );
    p.text(
        pos2(r.left() + 18.0, r.bottom() - 16.0),
        Align2::LEFT_CENTER,
        crate::widgets::sentence_case(label),
        theme::semi(10.5),
        theme::TEXT_FAINT,
    );
}

/// Round glowing icon badge (sheet and panel headers): tinted disc, rim, sheen, icon.
pub fn icon_badge(p: &egui::Painter, r: Rect, c: Color32, icon: icons::Icon) {
    let rad = r.width() / 2.0;
    paint::glow_circle(p, r.center(), rad - 2.0, theme::alpha(c, 0.3), 14.0);
    p.circle_filled(r.center(), rad, theme::alpha(c, 0.18));
    p.circle_stroke(
        r.center(),
        rad - 0.5,
        egui::Stroke::new(1.0, theme::alpha(c, 0.55)),
    );
    paint::glass_sheen(p, r, CornerRadius::same(rad as u8), 0.6);
    icons::paint(p, r.shrink(rad * 0.5), icon, c);
}
