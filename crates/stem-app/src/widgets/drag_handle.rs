//! Grip pill you can drag a file out of (into a DAW / Explorer), or click to reveal it.

use eframe::egui::{
    Align2, Color32, CornerRadius, CursorIcon, Id, Painter, Rect, Response, Sense, Stroke,
    StrokeKind, Ui, pos2, vec2,
};

use crate::theme;

/// What the user did with a [`drag_pill`] this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillAction {
    /// Started dragging: hand the file to the OS drag (or reveal it where that's unsupported).
    Drag,
    /// Plain click: reveal the file in Explorer.
    Click,
}

/// Six-dot grip glyph centred on `c`.
pub fn grip(p: &Painter, c: eframe::egui::Pos2, color: Color32) {
    for row in 0..3 {
        for col in 0..2 {
            p.circle_filled(
                pos2(c.x - 2.5 + col as f32 * 5.0, c.y - 5.0 + row as f32 * 5.0),
                1.3,
                color,
            );
        }
    }
}

/// Width a pill with `label` needs.
pub fn pill_width(ui: &Ui, label: &str) -> f32 {
    let g = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::semi(11.5), Color32::WHITE);
    g.size().x + 34.0
}

/// A small pill with a grip and `label`, drawn in `rect` and tinted `color`.
pub fn drag_pill(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    label: &str,
    color: Color32,
    tooltip: &str,
) -> (Response, Option<PillAction>) {
    let resp = ui
        .interact(rect, id, Sense::click_and_drag())
        .on_hover_cursor(CursorIcon::Grab)
        .on_hover_text(tooltip);
    let h = ui
        .ctx()
        .animate_bool_with_time(id.with("h"), resp.hovered() || resp.dragged(), 0.12);
    let p = ui.painter();
    p.rect(
        rect,
        CornerRadius::same((rect.height() / 2.0) as u8),
        theme::lerp_color(theme::CARD_HI, theme::alpha(color, 0.22), h),
        Stroke::new(
            1.0,
            theme::lerp_color(theme::STROKE_HI, theme::alpha(color, 0.8), h),
        ),
        StrokeKind::Inside,
    );
    let fg = theme::lerp_color(theme::TEXT_DIM, Color32::WHITE, h);
    grip(p, pos2(rect.left() + 13.0, rect.center().y), fg);
    p.text(
        pos2(rect.left() + 23.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::semi(11.5),
        fg,
    );
    let action = if resp.drag_started() {
        Some(PillAction::Drag)
    } else if resp.clicked() {
        Some(PillAction::Click)
    } else {
        None
    };
    (resp, action)
}

/// Pill rect of `w` x 24 whose right edge is at `right`, vertically centred on `cy`.
pub fn pill_rect(right: f32, cy: f32, w: f32) -> Rect {
    Rect::from_min_size(pos2(right - w, cy - 12.0), vec2(w, 24.0))
}
