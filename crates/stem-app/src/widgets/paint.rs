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

/// Soft shadow behind `rect`. Only neutral (black) shadows are drawn: coloured glows were
/// part of the old neon look and are skipped, so depth comes from surface steps and hairlines.
pub fn glow(
    painter: &Painter,
    rect: Rect,
    radius: impl Into<CornerRadius>,
    color: Color32,
    blur: f32,
) {
    if color.a() == 0 || blur <= 0.0 || !is_shadow(color) {
        return;
    }
    painter.add(glow_shape(rect, radius, color, blur));
}

/// True for a neutral black shadow colour (the only kind of glow the quiet theme keeps).
fn is_shadow(c: Color32) -> bool {
    c.r() == 0 && c.g() == 0 && c.b() == 0
}

pub fn glow_shape(rect: Rect, radius: impl Into<CornerRadius>, color: Color32, blur: f32) -> Shape {
    if !is_shadow(color) {
        return Shape::Noop;
    }
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

/// A texture generated once per context and cached (gradients, grain).
pub fn cached_texture(
    ctx: &egui::Context,
    key: &str,
    size: [usize; 2],
    options: TextureOptions,
    pixel: impl Fn(usize, usize) -> Color32,
) -> TextureId {
    let id = Id::unique(("stem-splitter/tex", key));
    if let Some(tex) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return tex.id();
    }
    let mut pixels = Vec::with_capacity(size[0] * size[1]);
    for y in 0..size[1] {
        for x in 0..size[0] {
            pixels.push(pixel(x, y));
        }
    }
    let tex = ctx.load_texture(key, ColorImage::new(size, pixels), options);
    let tid = tex.id();
    ctx.data_mut(|d| d.insert_temp(id, tex));
    tid
}

/// Gaussian-ish radial falloff (white, alpha 1 in the centre, 0 at the edge).
fn disc_texture(ctx: &egui::Context) -> TextureId {
    cached_texture(
        ctx,
        "soft-disc",
        [96, 96],
        TextureOptions::LINEAR,
        |x, y| {
            let dx = (x as f32 + 0.5) / 48.0 - 1.0;
            let dy = (y as f32 + 0.5) / 48.0 - 1.0;
            let d2 = dx * dx + dy * dy;
            let a = ((-d2 * 3.2).exp() - (-3.2f32).exp()).max(0.0) / (1.0 - (-3.2f32).exp());
            Color32::from_white_alpha((a * 255.0).round() as u8)
        },
    )
}

/// A soft, round light of radius `r` (one textured quad: far cheaper than a big blur).
pub fn soft_disc(painter: &Painter, center: Pos2, r: f32, color: Color32) {
    // Coloured light washes are gone with the neon look; only dark falloffs remain.
    if color.a() == 0 || r <= 0.0 || !is_shadow(color) {
        return;
    }
    let tex = disc_texture(painter.ctx());
    painter.add(
        RectShape::filled(
            Rect::from_center_size(center, vec2(2.0 * r, 2.0 * r)),
            CornerRadius::ZERO,
            color,
        )
        .with_texture(tex, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
    );
}

/// The lit top half of a glass surface: a soft white falloff inside the rounded rect, plus a
/// brighter hairline on the top edge. `strength` 0..1 (hover can raise it).
pub fn glass_sheen(painter: &Painter, rect: Rect, radius: impl Into<CornerRadius>, strength: f32) {
    painter.extend(glass_sheen_shapes(painter.ctx(), rect, radius, strength));
}

/// [`glass_sheen`] as shapes (for painting behind content laid out later).
pub fn glass_sheen_shapes(
    ctx: &egui::Context,
    rect: Rect,
    radius: impl Into<CornerRadius>,
    strength: f32,
) -> Vec<Shape> {
    // Glass sheens were part of the neon look: surfaces are flat now.
    let _ = (ctx, rect, radius.into(), strength);
    Vec::new()
}

/// A full glass card: drop shadow, translucent fill, hairline, sheen, optional coloured rim
/// glow. The workhorse surface of the app; adopt it for new cards.
pub fn glass(
    painter: &Painter,
    rect: Rect,
    radius: impl Into<CornerRadius>,
    rim: Option<Color32>,
    hover: f32,
) {
    painter.extend(glass_shapes(painter.ctx(), rect, radius, rim, hover));
}

/// [`glass`] as shapes, so a frame can reserve a slot before its content and fill it after.
pub fn glass_shapes(
    ctx: &egui::Context,
    rect: Rect,
    radius: impl Into<CornerRadius>,
    rim: Option<Color32>,
    hover: f32,
) -> Vec<Shape> {
    let _ = ctx;
    let radius = radius.into();
    // A flat raised surface with a hairline. Hover is a small surface step; a rim colour
    // (selection, status) becomes a quiet coloured hairline instead of a glow.
    let stroke = match rim {
        Some(c) if c.a() > 0 => theme::alpha(opaque(c), 0.55),
        _ => theme::lerp_color(theme::GLASS_STROKE, theme::STROKE_HI, hover),
    };
    vec![
        glow_shape(
            rect.translate(vec2(0.0, 4.0)),
            radius,
            Color32::from_black_alpha(60),
            14.0,
        ),
        RectShape::new(
            rect,
            radius,
            theme::lerp_color(theme::GLASS, theme::CARD_HI, hover * 0.6),
            Stroke::new(1.0, stroke),
            egui::StrokeKind::Inside,
        )
        .into(),
    ]
}

/// A glass card around `add`: reserves a paint slot, lays the content out, then fills the
/// slot with [`glass_shapes`] sized to the result. `rim` tints the edge glow.
pub fn glass_card<R>(
    ui: &mut egui::Ui,
    margin: impl Into<egui::Margin>,
    rim: Option<Color32>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let slot = ui.painter().add(Shape::Noop);
    let r = egui::Frame::new().inner_margin(margin).show(ui, add);
    let rect = r.response.rect;
    let shapes = glass_shapes(ui.ctx(), rect, theme::R_CARD, rim, 0.0);
    ui.painter().set(slot, Shape::Vec(shapes));
    r
}

/// Soft inner glow along the inside edge of a rounded rect (neon tube look).
pub fn inner_glow(painter: &Painter, rect: Rect, radius: impl Into<CornerRadius>, color: Color32) {
    // One quiet hairline in the colour (was a three-ring neon tube).
    painter.rect_stroke(
        rect.shrink(1.0),
        radius,
        Stroke::new(1.0, theme::alpha(color, 0.5)),
        egui::StrokeKind::Inside,
    );
}

/// Pointer-following light inside `rect` (the "hover tilt" of premium plugin UIs).
pub fn spotlight(painter: &Painter, rect: Rect, pointer: Pos2, color: Color32, strength: f32) {
    // Decorative pointer light: retired with the neon look.
    let _ = (painter, rect, pointer, color, strength);
}

/// A diagonal light band that sweeps across `rect` as `t` goes 0 → 1 (hover shine).
pub fn shine(painter: &Painter, rect: Rect, t: f32, strength: f32) {
    // Decorative hover sweep: retired with the neon look.
    let _ = (painter, rect, t, strength);
}

/// Keyboard focus ring, drawn just outside `rect`.
pub fn focus_ring(painter: &Painter, rect: Rect, radius: impl Into<CornerRadius>) {
    let mut r: CornerRadius = radius.into();
    r.nw = r.nw.saturating_add(3);
    r.ne = r.ne.saturating_add(3);
    r.sw = r.sw.saturating_add(3);
    r.se = r.se.saturating_add(3);
    let outer = rect.expand(3.0);
    glow(painter, outer, r, theme::alpha(theme::FOCUS, 0.25), 8.0);
    painter.rect_stroke(
        outer,
        r,
        Stroke::new(2.0, theme::FOCUS),
        egui::StrokeKind::Middle,
    );
}

/// Draw the focus ring when `resp` has keyboard focus.
pub fn focus(
    painter: &Painter,
    resp: &egui::Response,
    rect: Rect,
    radius: impl Into<CornerRadius>,
) {
    if resp.has_focus() && painter.ctx().input(|i| !i.pointer.any_down()) {
        focus_ring(painter, rect, radius);
    }
}

/// Material-style ripple: call every frame for a clickable widget. Starts on click at the
/// pointer, expands and fades over 0.55 s, clipped to `rect`. No-op under reduced motion.
pub fn ripple(painter: &Painter, resp: &egui::Response, rect: Rect, color: Color32) {
    // Presses are a surface change now; the material ripple is retired.
    let _ = (painter, resp, rect, color);
}

/// Text whose glyphs run along the accent gradient (`range` = u0..u1, wrapping), faded by
/// `opacity`. Returns its rect.
pub fn gradient_text(
    painter: &Painter,
    pos: Pos2,
    align: egui::Align2,
    text: &str,
    font: egui::FontId,
    range: (f32, f32),
    opacity: f32,
) -> Rect {
    let (u0, u1) = range;
    let mut job = egui::text::LayoutJob::default();
    let n = text.chars().count().max(2) as f32 - 1.0;
    for (i, ch) in text.chars().enumerate() {
        // No gradient text: every glyph is the primary text colour.
        let _ = (u0, u1, n, i);
        let c = theme::alpha(theme::TEXT, opacity);
        let mut buf = [0u8; 4];
        job.append(
            ch.encode_utf8(&mut buf),
            0.0,
            egui::TextFormat::simple(font.clone(), c),
        );
    }
    let galley = painter.layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, Color32::WHITE);
    rect
}

/// Vertical two-colour gradient rect (no rounding) from vertex colours.
pub fn vgradient(painter: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(Shape::mesh(mesh));
}

/// A coloured light along the top edge of a card: a hairline that fades out towards both
/// ends, with a soft spill of the colour just inside. `width` is the lit span, centred on
/// `cx` (or the card's centre).
pub fn edge_light_shapes(rect: Rect, cx: Option<f32>, width: f32, color: Color32) -> Vec<Shape> {
    // Coloured edge lights were part of the neon look.
    let _ = (rect, cx, width, color);
    Vec::new()
}

/// [`edge_light_shapes`] painted now.
pub fn edge_light(painter: &Painter, rect: Rect, cx: Option<f32>, width: f32, color: Color32) {
    painter.extend(edge_light_shapes(rect, cx, width, color));
}

/// Opaque version of a (premultiplied, translucent) colour, for lights tinted by a rim colour.
pub fn opaque(c: Color32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgb(r, g, b)
}

/// Horizontal two-colour gradient rect (no rounding) from vertex colours.
pub fn hgradient(painter: &Painter, rect: Rect, left: Color32, right: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(Shape::mesh(mesh));
}
