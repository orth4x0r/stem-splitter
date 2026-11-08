//! Animated progress indicators: spinning vinyl, glowing ring, spectrum bars, shimmer bar.

use std::f32::consts::{FRAC_PI_2, TAU};

use eframe::egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

use super::paint;
use crate::theme;

/// A vinyl record spinning at `angle` radians, label in the accent gradient.
pub fn vinyl(p: &Painter, center: Pos2, r: f32, angle: f32, t: f64) {
    paint::glow_circle(
        p,
        center,
        r * 0.98,
        theme::alpha(theme::VIOLET, 0.35),
        r * 0.35,
    );
    p.circle_filled(center, r, Color32::from_rgb(10, 10, 14));
    // Grooves.
    let n = 14;
    for i in 0..n {
        let rr = r * (0.42 + 0.55 * i as f32 / n as f32);
        let a = if i % 3 == 0 { 26 } else { 14 };
        p.circle_stroke(center, rr, Stroke::new(1.0, Color32::from_white_alpha(a)));
    }
    // Light sheen: two opposite wedges that rotate with the record.
    for k in 0..2 {
        let a0 = angle + k as f32 * std::f32::consts::PI - 0.35;
        for j in 0..10 {
            let f = j as f32 / 10.0;
            let rr = r * (0.45 + 0.52 * f);
            let alpha = 0.10 * (1.0 - (f - 0.5).abs() * 2.0);
            paint::arc(
                p,
                center,
                rr,
                a0,
                a0 + 0.7,
                Stroke::new(r * 0.05, Color32::from_white_alpha((alpha * 255.0) as u8)),
            );
        }
    }
    // Label.
    let lr = r * 0.36;
    let label = Rect::from_center_size(center, vec2(2.0 * lr, 2.0 * lr));
    let u = (t * 0.08) as f32;
    paint::gradient_rect(
        p,
        label,
        CornerRadius::same(lr as u8),
        u,
        u + 1.0,
        Color32::WHITE,
    );
    // A mark on the label so the spin is visible.
    let mark = center + vec2(angle.cos(), angle.sin()) * lr * 0.62;
    p.circle_filled(mark, lr * 0.12, Color32::from_white_alpha(200));
    let mark2 = center + vec2((angle + 2.4).cos(), (angle + 2.4).sin()) * lr * 0.7;
    p.circle_filled(mark2, lr * 0.07, Color32::from_white_alpha(120));
    p.circle_filled(center, lr * 0.16, Color32::from_rgb(10, 10, 14));
    p.circle_stroke(center, r, Stroke::new(1.5, Color32::from_white_alpha(18)));
}

/// Circular progress around `center`. `frac == None` draws an indeterminate spinner.
pub fn ring(p: &Painter, center: Pos2, r: f32, width: f32, frac: Option<f32>, t: f64) {
    p.circle_stroke(center, r, Stroke::new(width, Color32::from_white_alpha(14)));
    let start = -FRAC_PI_2;
    let (a0, a1) = match frac {
        Some(f) => (start, start + TAU * f.clamp(0.0, 1.0)),
        None => {
            let spin = (t * 4.0) as f32;
            let len = 1.2 + 0.8 * paint::pulse(t, 1.6);
            (spin, spin + len)
        }
    };
    if a1 - a0 > 0.01 {
        let phase = (t * 0.15) as f32;
        // Glow layer, then crisp layer.
        paint::gradient_arc(p, center, r, a0, a1, width * 3.0, phase, 0.12);
        paint::gradient_arc(p, center, r, a0, a1, width, phase, 1.0);
        let head = center + vec2(a1.cos(), a1.sin()) * r;
        paint::glow_circle(
            p,
            head,
            width * 1.2,
            theme::alpha(theme::CYAN, 0.8),
            width * 3.0,
        );
        p.circle_filled(head, width * 0.7, Color32::WHITE);
    }
}

/// Fake-but-lively spectrum bars. `energy` in `[0, 1]` scales the motion.
pub fn spectrum(p: &Painter, rect: Rect, t: f64, energy: f32, bars: usize) {
    spectrum_in(p, rect, t, energy, bars, &[]);
}

/// [`spectrum`] coloured by `palette` (e.g. the job's stem colours), spread left to right;
/// an empty palette uses the accent gradient. Peak caps float above the bars.
pub fn spectrum_in(p: &Painter, rect: Rect, t: f64, energy: f32, bars: usize, palette: &[Color32]) {
    let bw = rect.width() / bars as f32;
    for i in 0..bars {
        let x = i as f32 / bars as f32;
        let tt = t as f32;
        let wob = (tt * 3.1 + x * 9.0).sin() * 0.5 + 0.5;
        let wob2 = (tt * 5.7 - x * 17.0).sin() * 0.5 + 0.5;
        let wob3 = (tt * 1.3 + x * 3.0).sin() * 0.5 + 0.5;
        let shape = (1.0 - (x - 0.35).abs() * 1.2).max(0.15);
        let h = (0.12 + energy * shape * (0.45 * wob + 0.35 * wob2 + 0.2 * wob3)).min(1.0);
        let bh = rect.height() * h;
        let bar = Rect::from_min_max(
            pos2(rect.left() + i as f32 * bw + bw * 0.18, rect.bottom() - bh),
            pos2(rect.left() + (i + 1) as f32 * bw - bw * 0.18, rect.bottom()),
        );
        let c = if palette.is_empty() {
            theme::accent_at(x + tt * 0.05)
        } else {
            theme::gradient_at(palette, x)
        };
        paint::glow(p, bar, CornerRadius::same(2), theme::alpha(c, 0.25), 6.0);
        p.rect_filled(bar, CornerRadius::same(2), c);
        if !palette.is_empty() && rect.height() > 30.0 {
            // Floating peak cap.
            let cap =
                Rect::from_min_size(pos2(bar.left(), bar.top() - 5.0), vec2(bar.width(), 2.0));
            p.rect_filled(
                cap,
                CornerRadius::ZERO,
                theme::lerp_color(c, Color32::WHITE, 0.6),
            );
        }
        // Reflection.
        let refl = Rect::from_min_size(
            pos2(bar.left(), rect.bottom() + 3.0),
            vec2(bar.width(), bh * 0.35),
        );
        p.rect_filled(refl, CornerRadius::same(2), theme::alpha(c, 0.12));
    }
}

/// Horizontal gradient bar with a moving shimmer. `None` = indeterminate sweep.
pub fn bar(p: &Painter, rect: Rect, frac: Option<f32>, t: f64) {
    let r = CornerRadius::same((rect.height() / 2.0) as u8);
    p.rect_filled(rect, r, Color32::from_white_alpha(10));
    let fill = match frac {
        Some(f) => Rect::from_min_size(
            rect.min,
            vec2(rect.width() * f.clamp(0.0, 1.0), rect.height()),
        ),
        None => {
            let w = rect.width() * 0.3;
            let x = rect.left() - w + (rect.width() + w) * ((t * 0.6) % 1.0) as f32;
            Rect::from_min_max(
                pos2(x.max(rect.left()), rect.top()),
                pos2((x + w).min(rect.right()), rect.bottom()),
            )
        }
    };
    if fill.width() > 1.0 {
        paint::glow(p, fill, r, theme::alpha(theme::VIOLET, 0.5), 10.0);
        let u = (t * 0.25) as f32;
        paint::gradient_rect(p, fill, r, u, u + 1.0, Color32::WHITE);
        // Shimmer.
        let sx = fill.left() + fill.width() * ((t * 0.8) % 1.4) as f32 - 30.0;
        let shimmer: Vec<Pos2> = vec![
            pos2(sx, fill.bottom()),
            pos2(sx + 18.0, fill.top()),
            pos2(sx + 34.0, fill.top()),
            pos2(sx + 16.0, fill.bottom()),
        ];
        let clip = p.with_clip_rect(fill.intersect(p.clip_rect()));
        clip.add(Shape::convex_polygon(
            shimmer,
            Color32::from_white_alpha(40),
            Stroke::NONE,
        ));
    }
}

/// Small spinner dot-ring for inline "busy" states.
pub fn dots(p: &Painter, center: Pos2, r: f32, t: f64, color: Color32) {
    for i in 0..8 {
        let a = i as f32 / 8.0 * TAU;
        let phase = ((t * 1.4) as f32 - i as f32 / 8.0).rem_euclid(1.0);
        let pos = center + vec2(a.cos(), a.sin()) * r;
        p.circle_filled(
            pos,
            r * 0.22 * (0.5 + 0.5 * (1.0 - phase)),
            theme::alpha(color, 1.0 - phase * 0.8),
        );
    }
}
