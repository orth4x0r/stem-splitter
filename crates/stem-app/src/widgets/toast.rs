//! Toast notifications that slide in at the bottom-right and fade away.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Id, Order, Rect, Sense, Stroke, StrokeKind, pos2, vec2,
};

use super::icons::{self, Icon};
use super::paint;
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Info,
    Error,
}

struct Toast {
    id: u64,
    kind: ToastKind,
    title: String,
    body: String,
    born: f64,
    ttl: f64,
}

#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
    next: u64,
    pending: Vec<(ToastKind, String, String)>,
    /// Space to keep free at the bottom (e.g. the transport bar).
    pub bottom_inset: f32,
}

impl Toasts {
    pub fn push(&mut self, kind: ToastKind, title: impl Into<String>, body: impl Into<String>) {
        self.pending.push((kind, title.into(), body.into()));
    }
    pub fn success(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.push(ToastKind::Success, title, body);
    }
    pub fn info(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.push(ToastKind::Info, title, body);
    }
    pub fn error(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.push(ToastKind::Error, title, body);
    }

    pub fn is_animating(&self) -> bool {
        !self.items.is_empty() || !self.pending.is_empty()
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        for (kind, title, body) in self.pending.drain(..) {
            let ttl = if kind == ToastKind::Error { 9.0 } else { 4.5 };
            self.items.push(Toast {
                id: self.next,
                kind,
                title,
                body,
                born: now,
                ttl,
            });
            self.next += 1;
        }
        self.items.retain(|t| now - t.born < t.ttl + 0.4);
        if self.items.is_empty() {
            return;
        }
        let screen = ctx.content_rect();
        let w = 340.0;
        let mut y = screen.bottom() - 18.0 - self.bottom_inset;
        let mut dismiss = None;
        for t in self.items.iter().rev() {
            let age = now - t.born;
            let appear = paint::ease_out((age / 0.35) as f32);
            let leave = paint::ease_out(((age - t.ttl) / 0.4).clamp(0.0, 1.0) as f32);
            let vis = appear * (1.0 - leave);
            let h = if t.body.is_empty() { 52.0 } else { 70.0 };
            let x = screen.right() - 18.0 - w + (1.0 - appear) * 60.0 + leave * 60.0;
            let rect = Rect::from_min_size(pos2(x, y - h), vec2(w, h));
            y -= (h + 10.0) * vis.max(0.0);
            let color = match t.kind {
                ToastKind::Success => theme::GOOD,
                ToastKind::Info => theme::CYAN,
                ToastKind::Error => theme::BAD,
            };
            egui::Area::new(Id::unique(("toast", t.id)))
                .order(Order::Foreground)
                .fixed_pos(rect.min)
                .interactable(true)
                .show(ctx, |ui| {
                    ui.set_opacity(vis);
                    let (r, resp) = ui.allocate_exact_size(rect.size(), Sense::click());
                    if resp.clicked() {
                        dismiss = Some(t.id);
                    }
                    let p = ui.painter();
                    paint::glow(
                        p,
                        r.translate(vec2(0.0, 6.0)),
                        CornerRadius::same(14),
                        Color32::from_black_alpha(160),
                        24.0,
                    );
                    paint::glow(
                        p,
                        r,
                        CornerRadius::same(14),
                        theme::alpha(color, 0.18),
                        18.0,
                    );
                    p.rect(
                        r,
                        CornerRadius::same(14),
                        Color32::from_rgb(24, 24, 40),
                        Stroke::new(1.0, theme::alpha(color, 0.45)),
                        StrokeKind::Inside,
                    );
                    // Countdown bar.
                    let left = (1.0 - (age / t.ttl) as f32).clamp(0.0, 1.0);
                    let bar = Rect::from_min_size(
                        pos2(r.left() + 14.0, r.bottom() - 4.0),
                        vec2((r.width() - 28.0) * left, 2.0),
                    );
                    p.rect_filled(bar, CornerRadius::same(1), theme::alpha(color, 0.6));
                    // Icon badge.
                    let badge = Rect::from_center_size(
                        pos2(r.left() + 30.0, r.center().y),
                        vec2(28.0, 28.0),
                    );
                    p.circle_filled(badge.center(), 14.0, theme::alpha(color, 0.18));
                    let icon = match t.kind {
                        ToastKind::Success => Icon::Check,
                        ToastKind::Info => Icon::Sparkle,
                        ToastKind::Error => Icon::Close,
                    };
                    icons::paint(p, badge.shrink(7.0), icon, color);
                    let tx = r.left() + 54.0;
                    let title_y = if t.body.is_empty() {
                        r.center().y
                    } else {
                        r.top() + 22.0
                    };
                    p.text(
                        pos2(tx, title_y),
                        Align2::LEFT_CENTER,
                        &t.title,
                        theme::semi(14.0),
                        theme::TEXT,
                    );
                    if !t.body.is_empty() {
                        let body = elide(&t.body, 46);
                        p.text(
                            pos2(tx, r.top() + 44.0),
                            Align2::LEFT_CENTER,
                            body,
                            theme::regular(12.5),
                            theme::TEXT_DIM,
                        );
                    }
                });
        }
        if let Some(id) = dismiss
            && let Some(t) = self.items.iter_mut().find(|t| t.id == id)
        {
            t.ttl = (now - t.born).min(t.ttl);
        }
        ctx.request_repaint();
    }
}

/// Shorten long strings with a middle ellipsis (keeps file names readable).
pub fn elide(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_owned();
    }
    let keep = max.saturating_sub(1) / 2;
    let head: String = s.chars().take(keep).collect();
    let tail: String = s.chars().skip(n - keep).collect();
    format!("{head}…{tail}")
}
