//! MPC-style 4×4 pad grid: rubbery dark pads with a neon edge, slice number, key cap and a mini
//! waveform; hits flash with their velocity and a progress bar runs while the slice plays.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Mesh, Rect, Sense, Stroke, StrokeKind, Ui,
    pos2, vec2,
};

use super::paint;
use crate::chop::Hit;
use crate::theme;

/// Neon colour for slice `i` (cycles through eight hues).
pub fn slice_color(i: usize) -> Color32 {
    const C: [Color32; 8] = [
        Color32::from_rgb(255, 46, 136),
        Color32::from_rgb(255, 150, 50),
        Color32::from_rgb(250, 220, 70),
        Color32::from_rgb(74, 222, 128),
        Color32::from_rgb(34, 211, 238),
        Color32::from_rgb(96, 140, 255),
        Color32::from_rgb(139, 92, 246),
        Color32::from_rgb(232, 121, 249),
    ];
    C[i % C.len()]
}

/// One pad's content.
pub struct PadInfo<'a> {
    /// Global slice index (`None` = empty pad).
    pub slice: Option<usize>,
    pub key: &'a str,
    pub mini: Option<&'a [f32]>,
    pub hit: Hit,
    pub held: bool,
    /// Slice length, for the play progress bar.
    pub secs: f64,
    /// Highlighted from the waveform (hovered there).
    pub linked: bool,
    /// Entrance progress 0..1 (pads pop in staggered when the chop changes).
    pub enter: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PadEvent {
    /// Mouse down on the pad; velocity from how close to the centre it was hit.
    Press(f32),
    Release,
    /// Pressed, then dragged off the pad: drag the slice out as a file.
    DragOut,
}

/// Grid position (row from the top, column) → pad number 0..16, MPC order: pad 1 bottom-left.
pub fn pad_at(row: usize, col: usize) -> usize {
    (3 - row.min(3)) * 4 + col.min(3)
}

/// Draw one pad in `rect`; returns what the mouse did to it.
pub fn pad(ui: &mut Ui, rect: Rect, id: Id, info: &PadInfo, now: f64) -> Option<PadEvent> {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let empty = info.slice.is_none();
    let resp = if empty {
        resp
    } else {
        resp.on_hover_cursor(CursorIcon::PointingHand)
    };
    let mut event = None;
    let pressed_id = id.with("down");
    let down: bool = ui.data(|d| d.get_temp(pressed_id).unwrap_or(false));
    let (primary_pressed, primary_down, pointer) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.pointer.interact_pos(),
        )
    });
    if !empty
        && primary_pressed
        && resp.contains_pointer()
        && let Some(p) = pointer
    {
        let d = ((p - rect.center()) / (rect.size() * 0.5))
            .length()
            .min(1.0);
        event = Some(PadEvent::Press(1.0 - 0.5 * d));
        ui.data_mut(|dm| dm.insert_temp(pressed_id, true));
    } else if down {
        if !primary_down {
            event = Some(PadEvent::Release);
            ui.data_mut(|dm| dm.insert_temp(pressed_id, false));
        } else if pointer.is_some_and(|p| !rect.expand(4.0).contains(p)) {
            event = Some(PadEvent::DragOut);
            ui.data_mut(|dm| dm.insert_temp(pressed_id, false));
        }
    }
    if !ui.is_rect_visible(rect) {
        return event;
    }

    let ctx = ui.ctx();
    // Pop-in: scale up from 80 % with a little overshoot, fading in.
    let enter = info.enter.clamp(0.0, 1.0);
    let pop = theme::motion::ease_out_back(enter);
    let rect = Rect::from_center_size(rect.center(), rect.size() * (0.8 + 0.2 * pop));
    let mut painter = ui.painter().clone();
    painter.multiply_opacity(theme::motion::ease_out(enter));
    let hover = ctx.animate_bool_with_time(id.with("h"), resp.hovered() && !empty, 0.1);
    let linked = ctx.animate_bool_with_time(id.with("l"), info.linked, 0.12);
    let color = info.slice.map_or(theme::STROKE_HI, slice_color);
    let age = (now - info.hit.at).max(0.0) as f32;
    // Flash: instant attack, ~0.35 s decay, scaled by velocity; full while held.
    let flash = if info.held {
        1.0
    } else if age.is_finite() {
        info.hit.velocity * (-age * 7.0).exp()
    } else {
        0.0
    };
    // Fade fully out by 0.5 s, when the view stops repainting for this hit.
    let flash = if info.held {
        1.0
    } else {
        flash * (1.0 - age / 0.5).clamp(0.0, 1.0).sqrt()
    };
    let flash = if flash < 0.004 { 0.0 } else { flash };
    let playing = info.slice.is_some() && (age as f64) < info.secs && info.hit.velocity > 0.0;
    let progress = if playing {
        (age as f64 / info.secs.max(1e-3)).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    // The slice's own loudness at the play position: the pad breathes with the audio.
    let env = match info.mini {
        Some(m) if playing && !m.is_empty() => {
            let k = ((progress * m.len() as f32) as usize).min(m.len() - 1);
            let peak = m.iter().copied().fold(0.05f32, f32::max);
            (m[k] / peak).clamp(0.0, 1.0)
        }
        _ => 0.0,
    };
    let pulse = env * info.hit.velocity.max(0.4);
    let r = CornerRadius::same(12);
    // Pressed pads sink a little, then swell with the audio while the slice plays.
    let body = rect.shrink(1.5 * flash).expand(1.5 * pulse);
    let p = &painter;

    if empty {
        p.rect_filled(rect, r, Color32::from_rgba_unmultiplied(255, 255, 255, 4));
        paint::dashed_rect(
            p,
            rect,
            12.0,
            Stroke::new(1.0, Color32::from_white_alpha(22)),
            5.0,
            5.0,
            0.0,
        );
        p.text(
            rect.right_top() + vec2(-10.0, 10.0),
            Align2::RIGHT_TOP,
            info.key,
            theme::mono(10.5),
            Color32::from_white_alpha(40),
        );
        return event;
    }

    // Bloom: a soft light spills out around the pad on a hit and while the slice sounds.
    let bloom = (0.75 * flash).max(0.45 * pulse);
    if bloom > 0.01 {
        paint::soft_disc(
            p,
            body.center(),
            body.width().max(body.height()) * (0.85 + 0.25 * bloom),
            theme::alpha(color, 0.42 * bloom),
        );
    }
    // Hover lift: a deeper shadow under the pad.
    paint::glow(
        p,
        body.translate(vec2(0.0, 4.0 + 3.0 * hover)),
        r,
        Color32::from_black_alpha((70.0 + 50.0 * hover) as u8),
        10.0 + 6.0 * hover,
    );
    // Glow.
    let glow_a = 0.55 * flash + 0.12 * linked + 0.10 * hover + 0.3 * pulse;
    if glow_a > 0.0 {
        paint::glow(
            p,
            body,
            r,
            theme::alpha(color, glow_a.min(1.0)),
            14.0 + 16.0 * flash.max(pulse),
        );
    }
    // Shock ring: a thin outline races outwards from the pad on every hit.
    if !theme::motion::reduced() && info.hit.velocity > 0.0 && age < 0.42 {
        let k = age / 0.42;
        let e = theme::motion::ease_out(k);
        p.rect_stroke(
            body.expand(1.0 + 9.0 * e),
            CornerRadius::same((12.0 + 5.0 * e) as u8),
            Stroke::new(
                2.0 * (1.0 - k) + 0.5,
                theme::alpha(color, 0.9 * (1.0 - k) * info.hit.velocity),
            ),
            StrokeKind::Middle,
        );
    }
    // Rubber body: a vertical gradient, lit towards the slice colour on hits.
    let top = theme::lerp_color(
        Color32::from_rgb(36, 35, 56),
        theme::lerp_color(color, Color32::WHITE, 0.25),
        0.85 * flash,
    );
    let bot = theme::lerp_color(Color32::from_rgb(22, 21, 36), color, 0.7 * flash);
    let top = theme::lerp_color(top, Color32::from_rgb(46, 44, 72), hover * 0.5);
    vertical_gradient(p, body, 12.0, top, bot);
    // Bloom from the centre on a hit.
    if flash > 0.05 {
        let c = body.center();
        let rr = body.width().min(body.height()) * (0.18 + 0.12 * (1.0 - flash));
        paint::glow_circle(
            &p.with_clip_rect(body.shrink(2.0)),
            c,
            rr,
            theme::alpha(Color32::WHITE, 0.35 * flash),
            rr * 1.6,
        );
    }
    // Glass sheen on the rubber, and a light that follows the pointer.
    paint::glass_sheen(p, body, r, 0.2 + 0.6 * hover + flash);
    if hover > 0.0
        && let Some(ptr) = resp.hover_pos()
    {
        paint::spotlight(p, body, ptr, color, 2.5 * hover);
    }
    if pulse > 0.05 {
        paint::inner_glow(p, body, r, theme::alpha(color, 0.8 * pulse));
    }
    // Neon edge.
    p.rect_stroke(
        body,
        r,
        Stroke::new(
            1.0 + flash,
            theme::alpha(color, 0.35 + 0.35 * hover + 0.3 * linked.max(flash)),
        ),
        StrokeKind::Inside,
    );

    // Mini waveform in the lower half.
    if let Some(mini) = info.mini
        && !mini.is_empty()
    {
        let wr = Rect::from_min_max(
            pos2(body.left() + 10.0, body.center().y + 2.0),
            pos2(body.right() - 10.0, body.bottom() - 12.0),
        );
        let n = mini.len();
        let bw = wr.width() / n as f32;
        let mid = wr.center().y;
        let mut m = Mesh::default();
        for (k, v) in mini.iter().enumerate() {
            let x = wr.left() + k as f32 * bw;
            let h = (v * wr.height() * 0.5).max(0.75);
            let played = playing && (k as f32 + 0.5) / n as f32 <= progress;
            let c = if flash > 0.3 {
                theme::alpha(Color32::from_rgb(14, 10, 24), 0.75)
            } else if played {
                Color32::WHITE
            } else {
                theme::alpha(color, 0.75 + 0.25 * hover)
            };
            m.add_colored_rect(
                Rect::from_min_max(pos2(x + 0.5, mid - h), pos2(x + bw.max(1.5) - 0.8, mid + h)),
                c,
            );
        }
        p.add(egui::Shape::mesh(m));
    }

    // Slice number and key cap.
    let i = info.slice.unwrap_or(0);
    let num_c = if flash > 0.3 {
        Color32::from_rgb(14, 10, 24)
    } else {
        theme::lerp_color(theme::TEXT, Color32::WHITE, hover)
    };
    p.text(
        body.left_top() + vec2(11.0, 8.0),
        Align2::LEFT_TOP,
        format!("{:02}", i + 1),
        theme::bold(19.0),
        num_c,
    );
    let key_rect = Rect::from_min_size(body.right_top() + vec2(-28.0, 9.0), vec2(18.0, 18.0));
    p.rect(
        key_rect,
        CornerRadius::same(5),
        if flash > 0.3 {
            Color32::from_rgba_unmultiplied(10, 8, 20, 90)
        } else {
            Color32::from_rgba_unmultiplied(0, 0, 0, 70)
        },
        Stroke::new(1.0, Color32::from_white_alpha(20)),
        StrokeKind::Inside,
    );
    p.text(
        key_rect.center(),
        Align2::CENTER_CENTER,
        info.key,
        theme::mono(10.5),
        if flash > 0.3 {
            Color32::WHITE
        } else {
            theme::TEXT_DIM
        },
    );
    // Play progress along the bottom edge, with a glowing head.
    if playing {
        let bar = Rect::from_min_size(
            pos2(body.left() + 10.0, body.bottom() - 6.0),
            vec2((body.width() - 20.0) * progress, 2.0),
        );
        p.rect_filled(
            bar,
            CornerRadius::same(1),
            theme::alpha(Color32::WHITE, 0.8),
        );
        paint::glow_circle(
            p,
            bar.right_center(),
            2.0,
            theme::alpha(Color32::WHITE, 0.7),
            8.0,
        );
    }
    event
}

/// Rounded rectangle with a vertical `top` → `bot` gradient (a fan over its outline).
fn vertical_gradient(p: &egui::Painter, rect: Rect, radius: f32, top: Color32, bot: Color32) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let mut pts = Vec::new();
    for (c, a) in [
        (pos2(rect.right() - r, rect.top() + r), -FRAC_PI_2),
        (pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (pos2(rect.left() + r, rect.bottom() - r), FRAC_PI_2),
        (pos2(rect.left() + r, rect.top() + r), PI),
    ] {
        pts.extend(paint::arc_points(c, r, a, a + FRAC_PI_2));
    }
    let col = |y: f32| theme::lerp_color(top, bot, (y - rect.top()) / rect.height().max(1.0));
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.center(), col(rect.center().y));
    for q in &pts {
        mesh.colored_vertex(*q, col(q.y));
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    p.add(egui::Shape::mesh(mesh));
}

/// The 4×4 grid in `rect` for pads `page*16 .. page*16+16`. `info(pad)` describes pad 0..16
/// (MPC numbering). Returns `(pad, event)` pairs.
pub fn grid<'a>(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    now: f64,
    info: impl Fn(usize) -> PadInfo<'a>,
) -> Vec<(usize, PadEvent)> {
    let gap = 10.0;
    let w = (rect.width() - 3.0 * gap) / 4.0;
    let h = (rect.height() - 3.0 * gap) / 4.0;
    let mut out = Vec::new();
    for row in 0..4 {
        for col in 0..4 {
            let n = pad_at(row, col);
            let r = Rect::from_min_size(
                rect.min + vec2(col as f32 * (w + gap), row as f32 * (h + gap)),
                vec2(w, h),
            );
            if let Some(e) = pad(ui, r, id.with(n), &info(n), now) {
                out.push((n, e));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpc_order() {
        assert_eq!(pad_at(3, 0), 0);
        assert_eq!(pad_at(3, 3), 3);
        assert_eq!(pad_at(0, 0), 12);
        assert_eq!(pad_at(0, 3), 15);
        assert_ne!(slice_color(0), slice_color(1));
        assert_eq!(slice_color(0), slice_color(8));
    }
}
