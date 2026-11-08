//! Buttons, chips, toggles and segmented controls with animated hover / press states.

use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, Rect, Response, Role, Sense, Stroke, StrokeKind, Ui,
    Vec2, WidgetInfo, pos2, vec2,
};

use super::icons::{self, Icon};
use super::paint;
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Gradient, glowing. One per screen.
    Primary,
    /// Raised neutral surface.
    Secondary,
    /// Text-only until hovered.
    Ghost,
    /// Red-tinted (cancel / delete).
    Danger,
}

pub struct Button<'a> {
    label: &'a str,
    icon: Option<Icon>,
    kind: Kind,
    enabled: bool,
    min_width: f32,
    height: f32,
    tooltip: Option<&'a str>,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            icon: None,
            kind: Kind::Secondary,
            enabled: true,
            min_width: 0.0,
            height: 38.0,
            tooltip: None,
        }
    }
    pub fn primary(label: &'a str) -> Self {
        Self::new(label).kind(Kind::Primary)
    }
    pub fn ghost(label: &'a str) -> Self {
        Self::new(label).kind(Kind::Ghost)
    }
    pub fn kind(mut self, k: Kind) -> Self {
        self.kind = k;
        self
    }
    pub fn icon(mut self, i: Icon) -> Self {
        self.icon = Some(i);
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    pub fn tooltip(mut self, t: &'a str) -> Self {
        self.tooltip = Some(t);
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let font = if self.kind == Kind::Primary {
            theme::semi(14.5)
        } else {
            theme::semi(13.5)
        };
        let galley = ui
            .painter()
            .layout_no_wrap(self.label.to_owned(), font, Color32::WHITE);
        let icon_w = if self.icon.is_some() { 18.0 } else { 0.0 };
        let gap = if self.icon.is_some() && !self.label.is_empty() {
            8.0
        } else {
            0.0
        };
        let pad = if self.label.is_empty() { 10.0 } else { 18.0 };
        let content_w = icon_w + gap + galley.size().x;
        let w = (content_w + 2.0 * pad).max(self.min_width).max(self.height);
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, mut resp) = ui.allocate_exact_size(vec2(w, self.height), sense);
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, self.enabled, self.label));
        if self.enabled {
            resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        }
        if let Some(t) = self.tooltip {
            resp = resp.on_hover_text(t);
        }
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let ctx = ui.ctx();
        let hover =
            ctx.animate_bool_with_time(resp.id.with("h"), resp.hovered() && self.enabled, 0.15);
        let press =
            ctx.animate_bool_with_time(resp.id.with("p"), resp.is_pointer_button_down_on(), 0.08);
        let rect = rect.shrink(press * 1.5);
        let r = CornerRadius::same((self.height * 0.32) as u8);
        let p = ui.painter();

        let text_color;
        match (self.kind, self.enabled) {
            (Kind::Primary, true) => {
                paint::glow(
                    p,
                    rect.translate(vec2(0.0, 4.0)),
                    r,
                    theme::alpha(theme::VIOLET, 0.35 + 0.35 * hover),
                    14.0 + 10.0 * hover,
                );
                let shift = hover * 0.35;
                paint::gradient_rect(p, rect, r, shift, 1.0 + shift, Color32::WHITE);
                // Top sheen.
                let sheen = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.center().y));
                p.rect_filled(
                    sheen.shrink2(vec2(2.0, 1.0)),
                    CornerRadius {
                        nw: r.nw,
                        ne: r.ne,
                        sw: 0,
                        se: 0,
                    },
                    Color32::from_white_alpha((14.0 + 14.0 * hover) as u8),
                );
                text_color = Color32::WHITE;
            }
            (Kind::Danger, true) => {
                let fill = theme::lerp_color(
                    Color32::from_rgb(52, 22, 34),
                    Color32::from_rgb(90, 28, 48),
                    hover,
                );
                p.rect(
                    rect,
                    r,
                    fill,
                    Stroke::new(1.0, theme::alpha(theme::BAD, 0.5 + 0.5 * hover)),
                    StrokeKind::Inside,
                );
                text_color = theme::lerp_color(theme::BAD, Color32::WHITE, hover);
            }
            (Kind::Ghost, true) => {
                if hover > 0.0 {
                    p.rect_filled(rect, r, Color32::from_white_alpha((10.0 * hover) as u8));
                }
                text_color = theme::lerp_color(theme::TEXT_DIM, Color32::WHITE, hover);
            }
            (_, true) => {
                let fill = theme::lerp_color(theme::CARD_HI, Color32::from_rgb(40, 38, 66), hover);
                let stroke = theme::lerp_color(theme::STROKE_HI, theme::VIOLET, hover);
                if hover > 0.0 {
                    paint::glow(p, rect, r, theme::alpha(theme::VIOLET, 0.18 * hover), 12.0);
                }
                p.rect(rect, r, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
                text_color = theme::lerp_color(theme::TEXT, Color32::WHITE, hover);
            }
            (_, false) => {
                p.rect(
                    rect,
                    r,
                    theme::CARD_HI.gamma_multiply(0.6),
                    Stroke::new(1.0, theme::STROKE),
                    StrokeKind::Inside,
                );
                text_color = theme::TEXT_FAINT;
            }
        }

        let x0 = rect.center().x - content_w / 2.0;
        if let Some(icon) = self.icon {
            let ir =
                Rect::from_center_size(pos2(x0 + icon_w / 2.0, rect.center().y), Vec2::splat(15.0));
            icons::paint(p, ir, icon, text_color);
        }
        let tp = pos2(x0 + icon_w + gap, rect.center().y - galley.size().y / 2.0);
        p.galley(tp, galley, text_color);
        resp
    }
}

/// Round icon-only button. `active` lights it up in `color`.
pub fn icon_button(
    ui: &mut Ui,
    icon: Icon,
    size: f32,
    active: bool,
    color: Color32,
    tooltip: &str,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let resp = resp
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tooltip);
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, tooltip));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let ctx = ui.ctx();
    let hover = ctx.animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.12);
    let on = ctx.animate_bool_with_time(resp.id.with("a"), active, 0.15);
    let p = ui.painter();
    let r = CornerRadius::same((size * 0.3) as u8);
    if on > 0.0 {
        paint::glow(p, rect, r, theme::alpha(color, 0.45 * on), 10.0);
    }
    let fill = theme::lerp_color(
        theme::lerp_color(theme::CARD_HI, Color32::from_rgb(44, 42, 70), hover),
        color,
        on * 0.9,
    );
    let stroke = theme::lerp_color(theme::STROKE, color, (hover * 0.6).max(on));
    p.rect(rect, r, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    let fg = if on > 0.5 {
        Color32::from_rgb(12, 10, 20)
    } else {
        theme::lerp_color(theme::TEXT_DIM, Color32::WHITE, hover)
    };
    icons::paint(p, rect.shrink(size * 0.27), icon, fg);
    resp
}

/// Round button showing a letter (S for solo, M for mute).
pub fn letter_toggle(
    ui: &mut Ui,
    letter: &str,
    size: f32,
    active: bool,
    color: Color32,
    tooltip: &str,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let resp = resp
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tooltip);
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, active, tooltip));
    let ctx = ui.ctx();
    let hover = ctx.animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.12);
    let on = ctx.animate_bool_with_time(resp.id.with("a"), active, 0.15);
    let p = ui.painter();
    let r = CornerRadius::same((size * 0.3) as u8);
    if on > 0.0 {
        paint::glow(p, rect, r, theme::alpha(color, 0.5 * on), 10.0);
    }
    let fill = theme::lerp_color(
        theme::lerp_color(theme::CARD_HI, Color32::from_rgb(44, 42, 70), hover),
        color,
        on,
    );
    p.rect(
        rect,
        r,
        fill,
        Stroke::new(1.0, theme::lerp_color(theme::STROKE, color, hover.max(on))),
        StrokeKind::Inside,
    );
    let fg = theme::lerp_color(
        theme::lerp_color(theme::TEXT_DIM, Color32::WHITE, hover),
        Color32::from_rgb(14, 10, 22),
        on,
    );
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        theme::bold(size * 0.42),
        fg,
    );
    resp
}

/// Pill chip (model / stem tags). Clickable when `sense` is click.
pub fn chip(ui: &mut Ui, text: &str, color: Color32, selected: bool) -> Response {
    let font = theme::semi(12.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, Color32::WHITE);
    let size = vec2(galley.size().x + 26.0, 24.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let ctx = ui.ctx();
    let hover = ctx.animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.12);
    let on = ctx.animate_bool_with_time(resp.id.with("s"), selected, 0.15);
    let p = ui.painter();
    let fill = theme::lerp_color(
        theme::alpha(color, 0.10 + 0.06 * hover),
        theme::alpha(color, 0.28),
        on,
    );
    p.rect(
        rect,
        CornerRadius::same(12),
        fill,
        Stroke::new(1.0, theme::alpha(color, 0.35 + 0.5 * on.max(hover))),
        StrokeKind::Inside,
    );
    p.circle_filled(pos2(rect.left() + 11.0, rect.center().y), 3.0, color);
    p.galley(
        pos2(rect.left() + 18.0, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::lerp_color(theme::TEXT, Color32::WHITE, on),
    );
    resp
}

/// iOS-style switch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(40.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| WidgetInfo::selected(Role::Switch, true, *on, ""));
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.16);
    let p = ui.painter();
    let r = CornerRadius::same(11);
    if t > 0.0 {
        paint::glow(p, rect, r, theme::alpha(theme::VIOLET, 0.35 * t), 8.0);
        paint::gradient_rect(
            p,
            rect,
            r,
            0.0,
            1.0,
            Color32::from_white_alpha((255.0 * t) as u8),
        );
    }
    p.rect(
        rect,
        r,
        theme::alpha(theme::CARD_HI, 1.0 - t),
        Stroke::new(1.0, theme::alpha(theme::STROKE_HI, 1.0 - t)),
        StrokeKind::Inside,
    );
    let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, paint::ease_out(t));
    p.circle_filled(pos2(x, rect.center().y), 8.0, Color32::WHITE);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Segmented control. Returns true when the selection changed.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    id: &str,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    let font = theme::semi(13.0);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, l)| {
            ui.painter()
                .layout_no_wrap((*l).to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
                + 28.0
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, 34.0), Sense::hover());
    let p = ui.painter().clone();
    p.rect(
        rect,
        CornerRadius::same(10),
        theme::alpha(Color32::BLACK, 0.35),
        Stroke::new(1.0, theme::STROKE),
        StrokeKind::Inside,
    );
    let sel_idx = options.iter().position(|(v, _)| v == value).unwrap_or(0);
    let mut x = rect.left() + 3.0;
    let mut xs = Vec::new();
    for w in &widths {
        xs.push(x);
        x += w;
    }
    // Sliding highlight.
    let ctx = ui.ctx().clone();
    let sel_x = ctx.animate_value_with_time(ui.scope_id().with(id).with("x"), xs[sel_idx], 0.18);
    let sel_w =
        ctx.animate_value_with_time(ui.scope_id().with(id).with("w"), widths[sel_idx], 0.18);
    let hl = Rect::from_min_size(
        pos2(sel_x, rect.top() + 3.0),
        vec2(sel_w, rect.height() - 6.0),
    );
    paint::glow(
        &p,
        hl,
        CornerRadius::same(8),
        theme::alpha(theme::VIOLET, 0.3),
        10.0,
    );
    paint::gradient_rect(&p, hl, CornerRadius::same(8), 0.15, 0.85, Color32::WHITE);
    let mut changed = false;
    for (i, ((v, label), w)) in options.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(pos2(xs[i], rect.top()), vec2(*w, rect.height()));
        let resp = ui
            .interact(r, ui.scope_id().with(id).with(i), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if resp.clicked() && *value != *v {
            *value = *v;
            changed = true;
        }
        let selected = i == sel_idx;
        let color = if selected {
            Color32::WHITE
        } else if resp.hovered() {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        };
        p.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            font.clone(),
            color,
        );
    }
    changed
}

/// Gradient-filled slider with a value readout. Returns true when changed.
pub fn slider(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
    width: f32,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click_and_drag());
    let resp = resp.on_hover_cursor(CursorIcon::ResizeHorizontal);
    let (lo, hi) = (*range.start(), *range.end());
    let mut changed = false;
    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let f = ((pos.x - rect.left() - 8.0) / (rect.width() - 16.0)).clamp(0.0, 1.0);
        let v = (lo + f * (hi - lo)).round();
        if v != *value {
            *value = v;
            changed = true;
        }
    }
    let h = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered() || resp.dragged(), 0.12);
    let p = ui.painter();
    let track = Rect::from_min_max(
        pos2(rect.left() + 8.0, rect.center().y - 3.0),
        pos2(rect.right() - 8.0, rect.center().y + 3.0),
    );
    p.rect_filled(track, CornerRadius::same(3), Color32::from_white_alpha(16));
    let f = ((*value - lo) / (hi - lo)).clamp(0.0, 1.0);
    let kx = track.left() + track.width() * f;
    if kx > track.left() + 1.0 {
        paint::gradient_rect(
            p,
            Rect::from_min_max(track.min, pos2(kx, track.max.y)),
            CornerRadius::same(3),
            0.0,
            f.max(0.05),
            Color32::WHITE,
        );
    }
    paint::glow_circle(
        p,
        pos2(kx, track.center().y),
        8.0,
        theme::alpha(theme::VIOLET, 0.35 + 0.35 * h),
        10.0,
    );
    p.circle_filled(pos2(kx, track.center().y), 8.0 + 1.5 * h, Color32::WHITE);
    let _ = ui.label(
        egui::RichText::new(format!("{:.0}{suffix}", *value))
            .font(theme::mono(13.0))
            .color(theme::TEXT),
    );
    changed
}
