//! Audio meters: LED-style VU with peak hold, and a level-driven glow for cards.
//!
//! State (ballistics and peak hold) lives in egui temp memory under the caller's `Id`, so the
//! functions can be called from anywhere without threading state through the app.

use eframe::egui::{self, Color32, CornerRadius, Id, Painter, Rect, pos2, vec2};

use super::paint;
use crate::theme;

#[derive(Clone, Copy, Default)]
struct Ballistics {
    level: f32,
    peak: f32,
    peak_at: f64,
    last: f64,
}

/// How long the peak marker holds before it falls.
const HOLD: f64 = 0.9;

/// Smooth `input` with fast attack / slow release and track a held peak.
/// Returns `(level, peak)`; keeps repainting while either is still moving.
pub fn ballistics(ctx: &egui::Context, id: Id, input: f32) -> (f32, f32) {
    let now = ctx.input(|i| i.time);
    let mut b = ctx
        .data(|d| d.get_temp::<Ballistics>(id))
        .unwrap_or(Ballistics {
            last: now,
            ..Default::default()
        });
    let dt = ((now - b.last) as f32).clamp(0.0, 0.1);
    b.last = now;
    let input = input.clamp(0.0, 1.0);
    b.level = if input > b.level {
        input
    } else {
        b.level + (input - b.level) * (1.0 - (-dt * 7.0).exp())
    };
    if b.level >= b.peak {
        b.peak = b.level;
        b.peak_at = now;
    } else if now - b.peak_at > HOLD {
        b.peak = (b.peak - dt * 0.7).max(b.level);
    }
    ctx.data_mut(|d| d.insert_temp(id, b));
    if b.level > 0.002 || b.peak > 0.002 {
        ctx.request_repaint();
    }
    (b.level, b.peak)
}

/// Horizontal LED-segment VU meter with peak hold. `color` lights the lower segments; the top
/// ones run warm (warn / clip).
pub fn vu(p: &Painter, id: Id, rect: Rect, input: f32, color: Color32) {
    let (level, peak) = ballistics(p.ctx(), id, input);
    let seg_w = 4.0;
    let gap = 2.0;
    let n = ((rect.width() + gap) / (seg_w + gap)).floor().max(1.0) as usize;
    let peak_seg = ((peak * n as f32).ceil() as usize).min(n);
    for i in 0..n {
        let f = (i as f32 + 0.5) / n as f32;
        let x = rect.left() + i as f32 * (seg_w + gap);
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(seg_w, rect.height()));
        let c = if f > 0.9 {
            theme::BAD
        } else if f > 0.72 {
            theme::WARN
        } else {
            theme::lerp_color(color, Color32::WHITE, f * 0.25)
        };
        let on = level >= f;
        let is_peak = peak > 0.02 && i + 1 == peak_seg;
        if on || is_peak {
            paint::glow(p, r, CornerRadius::same(1), theme::alpha(c, 0.45), 5.0);
            p.rect_filled(r, CornerRadius::same(1), c);
        } else {
            p.rect_filled(r, CornerRadius::same(1), Color32::from_white_alpha(12));
        }
    }
}

/// Vertical bar meter (mixer strip style) with a held peak tick.
pub fn bar(p: &Painter, id: Id, rect: Rect, input: f32, color: Color32) {
    let (level, peak) = ballistics(p.ctx(), id, input);
    let r = CornerRadius::same((rect.width() / 2.0).min(3.0) as u8);
    p.rect_filled(rect, r, Color32::from_white_alpha(10));
    if level > 0.003 {
        let fill = Rect::from_min_max(
            pos2(rect.left(), rect.bottom() - rect.height() * level),
            rect.max,
        );
        paint::glow(p, fill, r, theme::alpha(color, 0.5 * level), 8.0);
        p.rect_filled(fill, r, color);
    }
    if peak > 0.02 {
        let y = rect.bottom() - rect.height() * peak;
        p.rect_filled(
            Rect::from_min_max(pos2(rect.left(), y - 1.0), pos2(rect.right(), y + 1.0)),
            CornerRadius::ZERO,
            theme::lerp_color(color, Color32::WHITE, 0.6),
        );
    }
}

/// Level-driven neon glow around a card: call before painting the card body. Cheap: one
/// blurred rect, skipped when silent.
pub fn level_glow(p: &Painter, rect: Rect, radius: impl Into<CornerRadius>, color: Color32, level: f32) {
    if level <= 0.01 {
        return;
    }
    let l = level.clamp(0.0, 1.0);
    paint::glow(p, rect, radius, theme::alpha(color, 0.12 + 0.35 * l), 10.0 + 22.0 * l);
}
