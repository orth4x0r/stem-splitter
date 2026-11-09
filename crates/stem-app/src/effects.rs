//! Ambient and celebratory effects: the drifting neon background, the stem-coloured aurora
//! behind the progress card, and the confetti burst when a split finishes.
//!
//! Budget: every effect stays within a few hundred shapes, and nothing here asks for a repaint
//! on its own except the burst (which ends after ~2.6 s). The ambient background advances its
//! own clock only on frames that are being painted anyway, so an idle app stays at 0 % CPU and
//! the drift resumes where it stopped instead of jumping.

use std::f32::consts::{PI, TAU};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, TextureOptions, Vec2,
    pos2, vec2,
};

use crate::theme::{self, motion};
use crate::widgets::paint;

/// Effect state owned by the app.
#[derive(Default)]
pub struct Fx {
    /// Ambient clock (seconds), advanced only while frames are painted.
    clock: f64,
    burst: Option<Burst>,
    /// Burst requested by the app; spawned on the next frame (needs the layout rect).
    pending: Option<BurstSpec>,
    /// Smoothed master output level, 0..1.
    pub master: f32,
    /// Screen positions views register for micro-bursts (e.g. an Export button).
    anchors: Vec<(&'static str, Pos2)>,
}

pub struct BurstSpec {
    pub colors: Vec<Color32>,
    pub label: String,
    /// Where it starts (`None` = middle of the content area).
    pub at: Option<Pos2>,
    /// A small, quick sparkle instead of the full celebration.
    pub micro: bool,
}

impl Fx {
    /// Advance the ambient clock by this frame's dt (capped, so a long idle gap doesn't jump).
    pub fn tick(&mut self, dt: f32, master: f32) {
        if !motion::reduced() {
            // Speed up a touch while audio plays, so the room "breathes" with the music.
            self.clock += (dt.min(1.0 / 20.0) * (1.0 + 0.8 * master)) as f64;
        }
        let k = if master > self.master { 0.5 } else { 0.08 };
        self.master += (master - self.master) * k;
    }

    pub fn celebrate(&mut self, colors: Vec<Color32>, label: impl Into<String>) {
        self.pending = Some(BurstSpec {
            colors,
            label: label.into(),
            at: None,
            micro: false,
        });
    }

    /// Remember where `key` sits on screen (call each frame it is drawn).
    pub fn set_anchor(&mut self, key: &'static str, pos: Pos2) {
        match self.anchors.iter_mut().find(|(k, _)| *k == key) {
            Some(a) => a.1 = pos,
            None => self.anchors.push((key, pos)),
        }
    }

    /// A satisfying little sparkle from anchor `key` (a chop or kit export finishing).
    /// Doesn't replace a full celebration that is still running.
    pub fn sparkle_at(&mut self, key: &str, colors: Vec<Color32>) {
        if self.burst.as_ref().is_some_and(|b| !b.micro) {
            return;
        }
        let at = self.anchors.iter().find(|(k, _)| *k == key).map(|a| a.1);
        self.pending = Some(BurstSpec {
            colors,
            label: String::new(),
            at,
            micro: true,
        });
    }

    pub fn is_animating(&self) -> bool {
        self.burst.is_some() || self.pending.is_some()
    }
}

// ---- ambient background --------------------------------------------------------------------

/// Grain + faint scanlines, baked once into a small tiling texture.
fn grain_texture(ctx: &egui::Context) -> egui::TextureId {
    paint::cached_texture(
        ctx,
        "grain",
        [128, 128],
        TextureOptions::NEAREST_REPEAT,
        |x, y| {
            // Integer hash -> white noise.
            let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
            h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
            h ^= h >> 16;
            let n = (h & 0xff) as f32 / 255.0;
            if y % 3 == 0 {
                // Scanline: a whisper of black.
                Color32::from_black_alpha(9)
            } else {
                Color32::from_white_alpha((n * n * 7.0) as u8)
            }
        },
    )
}

/// Paint the window background: base fill, drifting colour blobs, grain, vignette.
/// `colors` tints the blobs (accent by default, stem colours when a session is open);
/// `energy` (0..1) brightens them (running job, playing audio).
pub fn ambient(painter: &Painter, screen: Rect, fx: &Fx, colors: &[Color32], energy: f32) {
    painter.rect_filled(screen, CornerRadius::ZERO, theme::BG);
    let t = fx.clock as f32;
    let (w, h) = (screen.width(), screen.height());
    let base_r = w.min(h) * 0.34;
    let n = colors.len().clamp(1, 5);
    for i in 0..n {
        let fi = i as f32;
        let c = colors[i % colors.len()];
        let ph = fi * 2.17;
        let cx = screen.left() + w * (0.5 + 0.38 * (t * (0.031 + 0.007 * fi) + ph).sin());
        let cy = screen.top() + h * (0.5 + 0.36 * (t * (0.023 + 0.006 * fi) + ph * 1.3).cos());
        let r = base_r * (1.5 + 0.3 * (t * 0.05 + fi).sin());
        let a = 0.13 + 0.06 * energy + 0.12 * fx.master;
        paint::soft_disc(painter, pos2(cx, cy), r, theme::alpha(c, a));
    }
    // Grain / scanlines: a single tiled quad.
    let tex = grain_texture(painter.ctx());
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(w / 128.0, h / 128.0));
    painter.add(
        egui::epaint::RectShape::filled(screen, CornerRadius::ZERO, Color32::WHITE)
            .with_texture(tex, uv),
    );
    vignette(painter, screen, 150);
}

/// Darken the window edges (8-vertex frame mesh).
pub fn vignette(painter: &Painter, rect: Rect, alpha: u8) {
    let inner = rect.shrink2(vec2(rect.width() * 0.22, rect.height() * 0.2));
    let edge = Color32::from_black_alpha(alpha);
    let mut mesh = egui::Mesh::default();
    for p in [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ] {
        mesh.colored_vertex(p, edge);
    }
    for p in [
        inner.left_top(),
        inner.right_top(),
        inner.right_bottom(),
        inner.left_bottom(),
    ] {
        mesh.colored_vertex(p, Color32::TRANSPARENT);
    }
    for i in 0..4u32 {
        let j = (i + 1) % 4;
        mesh.add_triangle(i, j, 4 + i);
        mesh.add_triangle(j, 4 + j, 4 + i);
    }
    painter.add(Shape::mesh(mesh));
}

// ---- aurora --------------------------------------------------------------------------------

/// Stem-coloured aurora curtains rising from the bottom of `rect`. `curtains` holds each
/// colour with its intensity (0..1). ~40 columns per curtain.
pub fn aurora(painter: &Painter, rect: Rect, t: f64, curtains: &[(Color32, f32)]) {
    let p = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let t = t as f32;
    let n = curtains.len().max(1) as f32;
    let cols = 40;
    for (i, (c, k)) in curtains.iter().enumerate() {
        if *k <= 0.01 {
            continue;
        }
        let fi = i as f32;
        let center = (fi + 0.5) / n;
        let mut mesh = egui::Mesh::default();
        for j in 0..=cols {
            let f = j as f32 / cols as f32;
            let x = rect.left() + rect.width() * f;
            let wave = 0.55
                + 0.22 * (f * 6.3 + t * 0.9 + fi * 1.7).sin()
                + 0.12 * (f * 13.1 - t * 1.6 + fi).sin();
            let hgt = rect.height() * wave * (0.55 + 0.45 * k);
            // Each curtain is brightest around its own slot, fading across the card.
            let d = (f - center) / (0.9 / n + 0.18);
            let env = (-d * d).exp() * k;
            let top = rect.bottom() - hgt;
            let mid = rect.bottom() - hgt * 0.45;
            let a = 0.33 * env;
            mesh.colored_vertex(pos2(x, top), Color32::TRANSPARENT);
            mesh.colored_vertex(pos2(x, mid), theme::alpha(*c, a));
            mesh.colored_vertex(pos2(x, rect.bottom()), theme::alpha(*c, a * 0.25));
            if j > 0 {
                let b = (3 * j) as u32;
                let a0 = b - 3;
                mesh.add_triangle(a0, a0 + 1, b);
                mesh.add_triangle(a0 + 1, b + 1, b);
                mesh.add_triangle(a0 + 1, a0 + 2, b + 1);
                mesh.add_triangle(a0 + 2, b + 2, b + 1);
            }
        }
        p.add(Shape::mesh(mesh));
    }
}

// ---- celebration burst ---------------------------------------------------------------------

struct Particle {
    p: Pos2,
    v: Vec2,
    rot: f32,
    spin: f32,
    size: Vec2,
    color: Color32,
    spark: bool,
    life: f32,
}

struct Burst {
    born: f64,
    origin: Pos2,
    parts: Vec<Particle>,
    colors: Vec<Color32>,
    label: String,
    last: f64,
    micro: bool,
    len: f32,
}

const BURST_LEN: f32 = 2.6;
const MICRO_LEN: f32 = 1.3;

/// Tiny deterministic RNG (xorshift), so screenshots are reproducible.
struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
}

/// Spawn a pending burst and paint the current one on a foreground layer over `area`.
pub fn show_burst(ctx: &egui::Context, fx: &mut Fx, area: Rect) {
    let now = ctx.input(|i| i.time);
    if let Some(spec) = fx.pending.take() {
        let origin = spec
            .at
            .unwrap_or(pos2(area.center().x, area.top() + area.height() * 0.42));
        let mut rng = Rng(0x9e37_79b9);
        let mut parts = Vec::new();
        if spec.micro && !motion::reduced() {
            let colors = if spec.colors.is_empty() {
                theme::ACCENT_GRADIENT.to_vec()
            } else {
                spec.colors.clone()
            };
            for i in 0..40 {
                let a = -PI / 2.0 + rng.range(-1.6, 1.6);
                let sp = rng.range(160.0, 460.0);
                parts.push(Particle {
                    p: origin + vec2(rng.range(-12.0, 12.0), rng.range(-4.0, 4.0)),
                    v: vec2(a.cos(), a.sin()) * sp,
                    rot: rng.range(0.0, TAU),
                    spin: rng.range(-16.0, 16.0),
                    size: vec2(rng.range(4.0, 7.0), rng.range(2.0, 3.5)),
                    color: colors[i % colors.len()],
                    spark: false,
                    life: rng.range(0.8, 1.25),
                });
            }
            for i in 0..22 {
                let a = i as f32 / 22.0 * TAU + rng.range(-0.08, 0.08);
                let sp = rng.range(300.0, 560.0);
                parts.push(Particle {
                    p: origin,
                    v: vec2(a.cos(), a.sin()) * sp,
                    rot: 0.0,
                    spin: 0.0,
                    size: vec2(1.5, 0.0),
                    color: theme::lerp_color(colors[i % colors.len()], Color32::WHITE, 0.45),
                    spark: true,
                    life: rng.range(0.3, 0.5),
                });
            }
        } else if !spec.micro && !motion::reduced() {
            let colors = if spec.colors.is_empty() {
                theme::ACCENT_GRADIENT.to_vec()
            } else {
                spec.colors.clone()
            };
            for i in 0..150 {
                let a = -PI / 2.0 + rng.range(-1.35, 1.35);
                let sp = rng.range(380.0, 1050.0);
                parts.push(Particle {
                    p: origin + vec2(rng.range(-20.0, 20.0), rng.range(-10.0, 10.0)),
                    v: vec2(a.cos(), a.sin()) * sp,
                    rot: rng.range(0.0, TAU),
                    spin: rng.range(-14.0, 14.0),
                    size: vec2(rng.range(6.0, 11.0), rng.range(3.0, 5.5)),
                    color: colors[i % colors.len()],
                    spark: false,
                    life: rng.range(1.8, 2.6),
                });
            }
            for i in 0..48 {
                let a = i as f32 / 48.0 * TAU + rng.range(-0.05, 0.05);
                let sp = rng.range(700.0, 1300.0);
                parts.push(Particle {
                    p: origin,
                    v: vec2(a.cos(), a.sin()) * sp,
                    rot: 0.0,
                    spin: 0.0,
                    size: vec2(2.0, 0.0),
                    color: theme::lerp_color(colors[i % colors.len()], Color32::WHITE, 0.4),
                    spark: true,
                    life: rng.range(0.45, 0.7),
                });
            }
        }
        fx.burst = Some(Burst {
            born: now,
            origin,
            parts,
            colors: spec.colors,
            label: spec.label,
            last: now,
            micro: spec.micro,
            len: if spec.micro { MICRO_LEN } else { BURST_LEN },
        });
    }
    let Some(b) = &mut fx.burst else { return };
    let age = (now - b.born) as f32;
    if age > b.len || (b.micro && motion::reduced()) {
        fx.burst = None;
        return;
    }
    // Simulate the real elapsed time in fixed 1/60 s steps, so slow frames don't slow the
    // physics down (capped, so a stalled window doesn't spin through the whole burst).
    let elapsed = ((now - b.last) as f32).clamp(0.0, 0.25);
    let steps = (elapsed * 60.0).round().max(1.0) as usize;
    let dt = elapsed / steps as f32;
    b.last = now;
    for _ in 0..steps {
        for q in &mut b.parts {
            if q.spark {
                q.v *= (-4.0 * dt).exp();
            } else {
                q.v.y += 1150.0 * dt;
                q.v *= (-1.6 * dt).exp();
                // Flutter: confetti drifts sideways as it falls.
                q.v.x += (q.rot * 1.7).sin() * 60.0 * dt;
            }
            q.p += q.v * dt;
            q.rot += q.spin * dt;
        }
    }
    ctx.request_repaint();

    let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("celebration"));
    let p = ctx.layer_painter(layer);
    let o = b.origin;
    let accent = b.colors.first().copied().unwrap_or(theme::PINK);

    // Flash.
    let k_size = if b.micro { 0.3 } else { 1.0 };
    let flash = (1.0 - age / 0.45).max(0.0);
    if flash > 0.0 {
        paint::glow_circle(
            &p,
            o,
            60.0 * k_size,
            theme::alpha(Color32::WHITE, 0.35 * flash),
            160.0 * k_size,
        );
        paint::glow_circle(
            &p,
            o,
            140.0 * k_size,
            theme::alpha(accent, 0.3 * flash),
            220.0 * k_size,
        );
    }
    // Shockwave rings.
    let ring_dur = if b.micro { 0.6 } else { 1.0 };
    for (k, delay) in [0.0f32, 0.12].iter().enumerate() {
        let t = ((age - delay) / ring_dur).clamp(0.0, 1.0);
        if t > 0.0 && t < 1.0 {
            let r = 30.0 * k_size + 460.0 * k_size * motion::ease_out_expo(t);
            let c = theme::accent_at(k as f32 * 0.5 + t);
            p.circle_stroke(
                o,
                r,
                Stroke::new(6.0 * (1.0 - t) + 0.5, theme::alpha(c, 0.8 * (1.0 - t))),
            );
        }
    }
    // Particles.
    for q in &b.parts {
        let fade = (1.0 - (age - q.life + 0.5).max(0.0) / 0.5).clamp(0.0, 1.0);
        if fade <= 0.0 {
            continue;
        }
        if q.spark {
            let tail = q.p - q.v * 0.035;
            p.line_segment([tail, q.p], Stroke::new(2.0, theme::alpha(q.color, fade)));
        } else {
            let flip = (q.rot * 1.3).cos().abs().max(0.15);
            let (s, c) = q.rot.sin_cos();
            let hx = vec2(c, s) * q.size.x * 0.5 * flip;
            let hy = vec2(-s, c) * q.size.y * 0.5;
            let shade = 0.65 + 0.35 * flip;
            p.add(Shape::convex_polygon(
                vec![q.p - hx - hy, q.p + hx - hy, q.p + hx + hy, q.p - hx + hy],
                theme::alpha(theme::lerp_color(Color32::BLACK, q.color, shade), fade),
                Stroke::NONE,
            ));
        }
    }
    // Headline pop.
    if !b.label.is_empty() {
        let tin = motion::ease_out_back((age / 0.5).min(1.0));
        let tout = ((age - 1.5) / 0.6).clamp(0.0, 1.0);
        let a = (1.0 - tout) * (age / 0.15).min(1.0);
        if a > 0.0 {
            let size = 34.0 + 22.0 * tin - 6.0 * tout;
            let y = o.y - 8.0 - 30.0 * tout;
            paint::glow(
                &p,
                Rect::from_center_size(pos2(o.x, y), vec2(size * 7.0, size * 1.1)),
                CornerRadius::same(40),
                Color32::from_black_alpha((150.0 * a) as u8),
                60.0,
            );
            let u = age * 0.3;
            paint::gradient_text(
                &p,
                pos2(o.x, y),
                Align2::CENTER_CENTER,
                &b.label,
                theme::bold(size),
                (u, u + 1.0),
                a,
            );
        }
    }
}
