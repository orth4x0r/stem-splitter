//! Mini overview of the whole track: summed stem envelopes, the visible zoom window, the
//! playhead and the selection. Drag the window (or click anywhere) to move the view;
//! double-click to zoom back out.

use eframe::egui::{
    Color32, CornerRadius, CursorIcon, Mesh, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2,
};

use crate::theme;
use crate::timeline::Timeline;
use crate::waveform::Envelope;

pub struct Overview<'a> {
    pub envs: Vec<(&'a Envelope, Color32)>,
    pub playhead: f64,
    pub selection: Option<(f64, f64)>,
}

impl Overview<'_> {
    /// Paints into `rect`; returns a time to seek to when the strip was clicked outside the
    /// window while not zoomed.
    pub fn show(self, ui: &mut Ui, rect: Rect, view: &mut Timeline) -> Option<f64> {
        let id = ui.scope_id().with("overview");
        let resp = ui
            .interact(rect, id, Sense::click_and_drag())
            .on_hover_cursor(if view.is_zoomed() {
                CursorIcon::Grab
            } else {
                CursorIcon::PointingHand
            });
        let dur = view.duration.max(1e-6);
        let to_x = |t: f64| rect.left() + (t / dur).clamp(0.0, 1.0) as f32 * rect.width();
        let to_t = |x: f32| ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * dur;

        let mut seek = None;
        if view.is_zoomed() {
            if resp.dragged() {
                let dx = resp.drag_delta().x;
                view.scroll(dx as f64 / rect.width() as f64 * dur);
            } else if resp.clicked()
                && let Some(p) = resp.interact_pointer_pos()
            {
                view.center_on(to_t(p.x));
            }
            if resp.double_clicked() {
                view.reset();
            }
        } else if (resp.clicked() || resp.dragged())
            && let Some(p) = resp.interact_pointer_pos()
        {
            seek = Some(to_t(p.x));
        }

        let p = ui.painter_at(rect.expand(2.0));
        p.rect_filled(
            rect,
            CornerRadius::same(5),
            Color32::from_rgba_unmultiplied(0, 0, 0, 110),
        );
        // Envelopes, summed peak per column, coloured by the loudest stem.
        let cols = (rect.width() / 2.0).max(1.0) as usize;
        let mid = rect.center().y;
        let half = rect.height() * 0.5 - 2.0;
        let peak = self
            .envs
            .iter()
            .map(|(e, _)| e.peak())
            .fold(0.0f32, f32::max)
            .max(0.05);
        let mut mesh = Mesh::default();
        for i in 0..cols {
            let f0 = i as f32 / cols as f32;
            let f1 = (i + 1) as f32 / cols as f32;
            let mut amp = 0.0f32;
            let mut color = theme::TEXT_FAINT;
            for (e, c) in &self.envs {
                let (lo, hi, _) = e.range(f0, f1);
                let a = hi.max(-lo);
                if a > amp {
                    amp = a;
                    color = *c;
                }
            }
            let h = (amp / peak).min(1.0).sqrt() * half;
            let x = rect.left() + f0 * rect.width();
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(x, mid - h), pos2(x + 1.2, mid + h.max(0.5))),
                theme::alpha(color, 0.8),
            );
        }
        p.add(Shape::mesh(mesh));

        if let Some((a, b)) = self.selection {
            let r = Rect::from_x_y_ranges(to_x(a)..=to_x(b).max(to_x(a) + 1.0), rect.y_range());
            p.rect_filled(r, CornerRadius::same(2), Color32::from_white_alpha(28));
        }

        // Visible window: dim what's outside it.
        if view.is_zoomed() {
            let w = Rect::from_x_y_ranges(
                to_x(view.start)..=to_x(view.end).max(to_x(view.start) + 3.0),
                rect.y_range(),
            );
            let shade = Color32::from_rgba_unmultiplied(9, 9, 15, 150);
            p.rect_filled(
                Rect::from_x_y_ranges(rect.left()..=w.left(), rect.y_range()),
                CornerRadius::ZERO,
                shade,
            );
            p.rect_filled(
                Rect::from_x_y_ranges(w.right()..=rect.right(), rect.y_range()),
                CornerRadius::ZERO,
                shade,
            );
            let hover = ui.ctx().animate_bool_with_time(
                id.with("h"),
                resp.hovered() || resp.dragged(),
                0.12,
            );
            super::paint::glow(
                &p,
                w,
                CornerRadius::same(3),
                theme::alpha(theme::CYAN, 0.18 + 0.2 * hover),
                8.0,
            );
            p.rect_filled(w, CornerRadius::same(3), Color32::from_white_alpha(10));
            p.rect_stroke(
                w,
                CornerRadius::same(3),
                Stroke::new(
                    1.0 + 0.5 * hover,
                    theme::alpha(theme::CYAN, 0.6 + 0.4 * hover),
                ),
                StrokeKind::Inside,
            );
        }

        let px = to_x(self.playhead);
        super::paint::glow(
            &p,
            Rect::from_x_y_ranges(px - 0.75..=px + 0.75, rect.y_range()),
            CornerRadius::ZERO,
            theme::alpha(theme::PINK, 0.6),
            6.0,
        );
        p.vline(px, rect.y_range(), Stroke::new(1.5, Color32::WHITE));
        super::paint::glass_sheen(&p, rect, CornerRadius::same(5), 0.0);
        p.rect_stroke(
            rect,
            CornerRadius::same(5),
            Stroke::new(1.0, theme::STROKE),
            StrokeKind::Inside,
        );
        let tip = if view.is_zoomed() {
            "Drag to scroll · click to jump · double-click to zoom out"
        } else {
            "Click to seek · Ctrl+wheel over a waveform to zoom"
        };
        resp.on_hover_text(tip);
        seek
    }
}
