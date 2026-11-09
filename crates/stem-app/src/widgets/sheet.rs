//! Modal sheets with a smooth open / close: the sheet scales up from 96 % and fades in over a
//! dimmed, vignetted backdrop (a blur-like "frosted" dim), and plays the same in reverse when
//! it closes. Close requests are deferred until the exit animation has finished, so callers
//! drop their state only once the sheet is gone (see [`Sheet::finished`]).
//!
//! Under reduced motion the sheet appears and closes instantly.

use eframe::egui::{
    self, Color32, CornerRadius, Id, Rect, Shape, emath::TSTransform, epaint::RectShape, vec2,
};

use super::paint;
use crate::theme::{self, motion};

const OPEN: f32 = 0.26;
const CLOSE: f32 = 0.16;

#[derive(Clone, Copy)]
struct State {
    born: f64,
    last_pass: u64,
    closing: Option<f64>,
}

/// One frame of an animated sheet.
pub struct Sheet {
    id: Id,
    /// 0 (hidden) .. 1 (fully open).
    pub t: f32,
    /// A close was requested and the exit animation is playing: ignore further actions.
    pub closing: bool,
    done: bool,
}

impl Sheet {
    /// The sheet's animation state for this frame. Restarts the entrance when the sheet was
    /// not shown in the previous pass.
    pub fn begin(ctx: &egui::Context, id: Id) -> Self {
        let now = ctx.input(|i| i.time);
        let pass = ctx.cumulative_pass_nr();
        let key = id.with("sheet-anim");
        let mut s = ctx.data(|d| d.get_temp::<State>(key)).unwrap_or(State {
            born: now,
            last_pass: pass,
            closing: None,
        });
        if pass > s.last_pass + 1 {
            s = State {
                born: now,
                last_pass: pass,
                closing: None,
            };
        }
        s.last_pass = pass;
        ctx.data_mut(|d| d.insert_temp(key, s));
        let reduced = motion::reduced();
        let open = if reduced {
            1.0
        } else {
            ((now - s.born) as f32 / OPEN).clamp(0.0, 1.0)
        };
        let (t, done) = match s.closing {
            Some(c) if reduced => (0.0, now >= c),
            Some(c) => {
                let k = ((now - c) as f32 / CLOSE).clamp(0.0, 1.0);
                (open.min(1.0 - k), k >= 1.0)
            }
            None => (open, false),
        };
        if (t < 1.0 && !done) || s.closing.is_some() {
            ctx.request_repaint();
        }
        Self {
            id,
            t,
            closing: s.closing.is_some(),
            done,
        }
    }

    /// Start the exit animation (idempotent).
    pub fn close(&mut self, ctx: &egui::Context) {
        if self.closing {
            return;
        }
        let key = self.id.with("sheet-anim");
        let now = ctx.input(|i| i.time);
        ctx.data_mut(|d| {
            if let Some(mut s) = d.get_temp::<State>(key) {
                s.closing = Some(now);
                d.insert_temp(key, s);
            }
        });
        self.closing = true;
        ctx.request_repaint();
    }

    /// The exit animation has finished: drop the sheet's state now.
    pub fn finished(&self) -> bool {
        self.done
    }

    /// Show the sheet as a modal: the dimmed backdrop, then a glass card (tinted by `accent`)
    /// with `add` inside, scaled and faded by the animation.
    pub fn show<R>(
        &self,
        ctx: &egui::Context,
        accent: Color32,
        margin: i8,
        add: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::ModalResponse<R> {
        let t = self.t;
        let e = if self.closing {
            motion::ease_in_out(t)
        } else {
            motion::ease_out(t)
        };
        let screen = ctx.content_rect();
        let accent_glow = accent;
        egui::Modal::new(self.id)
            .frame(egui::Frame::NONE)
            .backdrop_color(Color32::TRANSPARENT)
            .area(egui::Modal::default_area(self.id).fade_in(false))
            .show(ctx, |ui| {
                // Backdrop: dim + vignette, painted under the sheet, never transformed.
                let p = ui.painter().with_clip_rect(screen);
                backdrop(&p, screen, e, accent_glow);
                let scale = 0.955 + 0.045 * motion::ease_out_back(t.max(0.0));
                let scale = if self.closing { 0.97 + 0.03 * e } else { scale };
                let lift = 14.0 * (1.0 - e);
                let c = screen.center();
                let xf = TSTransform::from_translation(c.to_vec2() + vec2(0.0, lift))
                    * TSTransform::from_scaling(scale)
                    * TSTransform::from_translation(-c.to_vec2());
                let identity = (1.0 - scale).abs() < 1e-4 && lift.abs() < 0.05;
                let body = |ui: &mut egui::Ui| {
                    ui.multiply_opacity(e);
                    let slot = ui.painter().add(Shape::Noop);
                    let r = egui::Frame::new()
                        .inner_margin(egui::Margin::same(margin))
                        .show(ui, add);
                    let rect = r.response.rect;
                    ui.painter()
                        .set(slot, Shape::Vec(card_shapes(ui.ctx(), rect, accent)));
                    r.inner
                };
                if identity {
                    body(ui)
                } else {
                    ui.with_visual_transform(xf, body).inner
                }
            })
    }
}

/// The sheet surface: deep shadow, accent bloom behind, near-opaque glass, accent hairline.
fn card_shapes(ctx: &egui::Context, rect: Rect, accent: Color32) -> Vec<Shape> {
    let r = CornerRadius::same(18);
    let mut out = vec![
        paint::glow_shape(
            rect.translate(vec2(0.0, 18.0)),
            r,
            Color32::from_black_alpha(170),
            56.0,
        ),
        paint::glow_shape(rect, r, theme::alpha(accent, 0.10), 40.0),
        RectShape::new(
            rect,
            r,
            Color32::from_rgba_premultiplied(17, 17, 29, 250),
            egui::Stroke::new(1.0, theme::lerp_color(theme::GLASS_STROKE, accent, 0.22)),
            egui::StrokeKind::Inside,
        )
        .into(),
    ];
    out.extend(paint::glass_sheen_shapes(ctx, rect, r, 0.6));
    // Accent light along the top edge.
    out.extend(paint::edge_light_shapes(
        rect,
        None,
        (rect.width() * 0.6).min(460.0),
        theme::alpha(accent, 0.8),
    ));
    out
}

/// Dim the app behind a sheet: a flat dim, darker edges and a faint accent bloom in the
/// middle, which reads like frosted glass without an actual blur pass.
fn backdrop(p: &egui::Painter, screen: Rect, e: f32, accent: Color32) {
    if e <= 0.0 {
        return;
    }
    p.rect_filled(screen, 0, Color32::from_black_alpha((120.0 * e) as u8));
    // A wash of the panel colour flattens the busy content underneath (the "frost").
    p.rect_filled(
        screen,
        0,
        Color32::from_rgba_premultiplied(
            (10.0 * 0.35 * e) as u8,
            (10.0 * 0.35 * e) as u8,
            (20.0 * 0.35 * e) as u8,
            (90.0 * e) as u8,
        ),
    );
    crate::effects::vignette(p, screen, (110.0 * e) as u8);
    paint::soft_disc(
        p,
        screen.center(),
        screen.width().min(screen.height()) * 0.6,
        theme::alpha(accent, 0.07 * e),
    );
}
