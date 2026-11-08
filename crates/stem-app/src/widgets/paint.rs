//! Low-level painting helpers: gradients, glows, arcs.

use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, Id, Painter, Pos2, Rect, Shape, Stroke, TextureHandle,
    TextureId, TextureOptions, epaint::RectShape, pos2, vec2,
};

use crate::theme;

/// A 1D texture of the accent gradient (pink -> violet -> cyan), mirrored-repeat so it can be
/// scrolled for the "flowing" header and buttons.
pub fn accent_texture(ctx: &egui::Context) -> TextureId {
    let id = Id::unique("stem-splitter/accent-gradient");
    if let Some(tex) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return tex.id();
    }
    let w = 256;
    let pixels: Vec<Color32> = (0..w)
        .map(|x| theme::gradient_at(&theme::ACCENT_GRADIENT, x as f32 / (w - 1) as f32))
        .collect();
    let img = ColorImage::new([w, 1], pixels);
    let tex = ctx.load_texture(
        "accent-gradient",
        img,
        TextureOptions::LINEAR_MIRRORED_REPEAT,
    );
    let tid = tex.id();
    ctx.data_mut(|d| d.insert_temp(id, tex));
    tid
}

/// Rounded rectangle filled with the accent gradient. `u0..u1` picks the part of the gradient
/// (0..1 is the whole thing; values outside wrap mirrored). `tint` multiplies the colours.
pub fn gradient_rect(
    painter: &Painter,
    rect: Rect,
    radius: impl Into<CornerRadius>,
    u0: f32,
    u1: f32,
    tint: Color32,
) {
    painter.add(gradient_rect_shape(
        painter.ctx(),
        rect,
        radius,
        u0,
        u1,
        tint,
    ));
}

pub fn gradient_rect_shape(
    ctx: &egui::Context,
    rect: Rect,
    radius: impl Into<CornerRadius>,
    u0: f32,
    u1: f32,
    tint: Color32,
) -> Shape {
    let tex = accent_texture(ctx);
    let uv = Rect::from_min_max(pos2(u0, 0.0), pos2(u1, 1.0));
    RectShape::filled(rect, radius, tint)
        .with_texture(tex, uv)
        .into()
}

/// Soft coloured glow behind `rect`.
pub fn glow(
    painter: &Painter,
    rect: Rect,
    radius: impl Into<CornerRadius>,
    color: Color32,
    blur: f32,
) {
    if color.a() == 0 || blur <= 0.0 {
        return;
    }
    painter.add(glow_shape(rect, radius, color, blur));
}

pub fn glow_shape(rect: Rect, radius: impl Into<CornerRadius>, color: Color32, blur: f32) -> Shape {
    // The tessellator caps blur at the rect's smallest side, so grow thin rects to fit it.
    let rect = rect.expand(blur * 0.5);
    let mut r: CornerRadius = radius.into();
    let grow = (blur * 0.5).min(255.0) as u8;
    r.nw = r.nw.saturating_add(grow);
    r.ne = r.ne.saturating_add(grow);
    r.sw = r.sw.saturating_add(grow);
    r.se = r.se.saturating_add(grow);
    RectShape::filled(rect, r, color)
        .with_blur_width(blur)
        .into()
}

pub fn glow_circle(painter: &Painter, center: Pos2, r: f32, color: Color32, blur: f32) {
    let rect = Rect::from_center_size(center, vec2(2.0 * r, 2.0 * r));
    glow(
        painter,
        rect,
        CornerRadius::same(r.min(255.0) as u8),
        color,
        blur,
    );
}

/// Points on a circular arc (angles in radians, 0 = right, clockwise in screen space).
pub fn arc_points(center: Pos2, radius: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    let span = (a1 - a0).abs();
    let n = ((span * radius / 3.0).ceil() as usize).clamp(4, 256);
    (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            center + vec2(a.cos(), a.sin()) * radius
        })
        .collect()
}

pub fn arc(painter: &Painter, center: Pos2, radius: f32, a0: f32, a1: f32, stroke: Stroke) {
    painter.add(Shape::line(arc_points(center, radius, a0, a1), stroke));
}

/// Arc whose colour runs along the accent gradient, shifted by `phase`.
#[allow(clippy::too_many_arguments)]
pub fn gradient_arc(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    a0: f32,
    a1: f32,
    width: f32,
    phase: f32,
    opacity: f32,
) {
    let pts = arc_points(center, radius, a0, a1);
    let n = pts.len().saturating_sub(1).max(1);
    for (i, w) in pts.windows(2).enumerate() {
        let c = theme::accent_at(phase + i as f32 / n as f32);
        painter.line_segment([w[0], w[1]], Stroke::new(width, theme::alpha(c, opacity)));
    }
    // Round caps.
    if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
        painter.circle_filled(
            *a,
            width / 2.0,
            theme::alpha(theme::accent_at(phase), opacity),
        );
        painter.circle_filled(
            *b,
            width / 2.0,
            theme::alpha(theme::accent_at(phase + 1.0), opacity),
        );
    }
}

/// A smooth 0..1..0 pulse with period `period` seconds.
pub fn pulse(t: f64, period: f64) -> f32 {
    (0.5 - 0.5 * (t * std::f64::consts::TAU / period).cos()) as f32
}

/// Ease-out cubic.
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Dashed rounded-rectangle outline (for the drop zone), dashes marching with `offset`.
pub fn dashed_rect(
    painter: &Painter,
    rect: Rect,
    radius: f32,
    stroke: Stroke,
    dash: f32,
    gap: f32,
    offset: f32,
) {
    let mut path = Vec::new();
    let rr = rect.shrink(stroke.width / 2.0);
    let r = radius.min(rr.width() / 2.0).min(rr.height() / 2.0);
    let corners = [
        (
            pos2(rr.right() - r, rr.top() + r),
            -std::f32::consts::FRAC_PI_2,
        ),
        (pos2(rr.right() - r, rr.bottom() - r), 0.0),
        (
            pos2(rr.left() + r, rr.bottom() - r),
            std::f32::consts::FRAC_PI_2,
        ),
        (pos2(rr.left() + r, rr.top() + r), std::f32::consts::PI),
    ];
    for (c, a) in corners {
        path.extend(arc_points(c, r, a, a + std::f32::consts::FRAC_PI_2));
    }
    if let Some(first) = path.first().copied() {
        path.push(first);
    }
    // Walk the path, emitting dashes.
    let period = dash + gap;
    let mut dist = -(offset.rem_euclid(period));
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = (b - a).length();
        if len <= 0.0 {
            continue;
        }
        let mut s = 0.0;
        while s < len {
            let phase = (dist + s).rem_euclid(period);
            let step = if phase < dash {
                dash - phase
            } else {
                period - phase
            }
            .max(0.05);
            let e = (s + step).min(len);
            if phase < dash {
                let p0 = a + (b - a) * (s / len);
                let p1 = a + (b - a) * (e / len);
                painter.line_segment([p0, p1], stroke);
            }
            s = e;
        }
        dist += len;
    }
}

/// Thin horizontal gradient strip built from vertex colours (textured rects this thin render
/// as solid white in the tessellator). `u0..u1` as in [`gradient_rect`].
pub fn gradient_strip(painter: &Painter, rect: Rect, u0: f32, u1: f32) {
    let mut mesh = egui::Mesh::default();
    let n = 48;
    for i in 0..=n {
        let f = i as f32 / n as f32;
        let x = rect.left() + rect.width() * f;
        let c = theme::accent_at(u0 + (u1 - u0) * f);
        mesh.colored_vertex(pos2(x, rect.top()), c);
        mesh.colored_vertex(pos2(x, rect.bottom()), c);
        if i > 0 {
            let b = (2 * i) as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    painter.add(Shape::mesh(mesh));
}
