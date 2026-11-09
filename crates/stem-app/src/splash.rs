//! Launch intro (~1.3 s, any click or key skips): the logo tile springs in, its five bars rise
//! one by one in stem colours, a gradient ring sweeps around it, the wordmark types itself
//! in, and the whole thing zooms through to reveal the app.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Id, Order, Painter, Pos2, Rect, Sense, pos2, vec2,
};

use crate::theme::{self, motion};
use crate::widgets::paint;

/// Bar heights of the logo mark (fractions of the tile).
pub const LOGO_BARS: [f32; 5] = [0.45, 0.8, 0.6, 0.95, 0.5];
/// Colours the bars light up in during the intro (vocals, drums, bass, other, keys).
const BAR_COLORS: [Color32; 5] = [
    Color32::from_rgb(255, 77, 157),
    Color32::from_rgb(255, 170, 51),
    Color32::from_rgb(150, 105, 255),
    Color32::from_rgb(34, 211, 238),
    Color32::from_rgb(253, 224, 71),
];

/// Total length; the reveal starts at `REVEAL`.
const LEN: f32 = 1.45;
const REVEAL: f32 = 1.05;

pub struct Splash {
    born: Option<f64>,
    skip_at: Option<f64>,
    pub done: bool,
}

impl Splash {
    pub fn new(enabled: bool) -> Self {
        Self {
            born: None,
            skip_at: None,
            done: !enabled || motion::reduced(),
        }
    }

    /// Paint the intro over everything. Returns true while it is still showing.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        if self.done {
            return false;
        }
        let now = ctx.input(|i| i.time);
        let born = *self.born.get_or_insert(now);
        let mut t = (now - born) as f32;
        // Skip: jump straight to the reveal.
        let skip = ctx.input(|i| i.pointer.any_pressed() || !i.keys_down.is_empty());
        if skip && self.skip_at.is_none() && t < REVEAL {
            self.skip_at = Some(now - REVEAL as f64);
        }
        if let Some(s) = self.skip_at {
            t = (now - s) as f32;
        }
        if t >= LEN {
            self.done = true;
            return false;
        }
        ctx.request_repaint();
        let screen = ctx.content_rect();
        egui::Area::new(Id::unique("splash"))
            .order(Order::Tooltip)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                // Swallow input while the intro shows.
                let _ = ui.allocate_rect(screen, Sense::click_and_drag());
                paint_frame(ui.painter(), screen, t);
            });
        true
    }
}

/// One frame of the intro at time `t` seconds.
pub fn paint_frame(p: &Painter, screen: Rect, t: f32) {
    let reveal = ((t - REVEAL) / (LEN - REVEAL)).clamp(0.0, 1.0);
    let out = motion::ease_in_out(reveal);
    let bg_a = 1.0 - out;
    p.rect_filled(
        screen,
        CornerRadius::ZERO,
        theme::alpha(theme::BG, bg_a.powf(0.7)),
    );
    let c = screen.center() - vec2(0.0, 30.0);
    // Room glow.
    let warm = (t / 0.6).min(1.0) * (1.0 - out);
    paint::glow_circle(p, c, 120.0, theme::alpha(theme::VIOLET, 0.22 * warm), 320.0);
    paint::glow_circle(
        p,
        c + vec2(160.0, 60.0),
        80.0,
        theme::alpha(theme::PINK, 0.12 * warm),
        260.0,
    );

    // Logo tile: springs in, then zooms through on reveal.
    let pop = motion::ease_out_back((t / 0.38).min(1.0));
    let zoom = 1.0 + 1.6 * out * out;
    let size = 96.0 * pop * zoom;
    if size < 1.0 {
        return;
    }
    let tile = Rect::from_center_size(c, vec2(size, size));
    let fade = 1.0 - out;
    // Sweeping ring.
    let ring_t = ((t - 0.12) / 0.7).clamp(0.0, 1.0);
    if ring_t > 0.0 {
        let a0 = -std::f32::consts::FRAC_PI_2;
        let a1 = a0 + std::f32::consts::TAU * motion::ease_out(ring_t);
        let r = size * 0.92;
        paint::gradient_arc(p, c, r, a0, a1, 8.0, t * 0.3, 0.12 * fade);
        paint::gradient_arc(p, c, r, a0, a1, 2.5, t * 0.3, 0.9 * fade);
    }
    let lit: Vec<f32> = (0..5)
        .map(|i| motion::ease_out_back(((t - 0.22 - i as f32 * 0.075) / 0.32).clamp(0.0, 1.0)))
        .collect();
    logo_mark(p, tile, &lit, 0.6 * fade, fade, true);

    // Wordmark, letter by letter.
    let word = "STEM SPLITTER";
    let font = theme::bold(34.0);
    let total = p
        .layout_no_wrap(word.to_owned(), font.clone(), Color32::WHITE)
        .size()
        .x;
    let mut x = c.x - total / 2.0;
    let y = c.y + size * 0.5 / zoom + 54.0;
    for (i, ch) in word.chars().enumerate() {
        let g = p.layout_no_wrap(ch.to_string(), font.clone(), Color32::WHITE);
        let w = g.size().x;
        let k = ((t - 0.38 - i as f32 * 0.03) / 0.22).clamp(0.0, 1.0);
        if k > 0.0 {
            let e = motion::ease_out(k);
            let col = theme::alpha(
                theme::lerp_color(theme::accent_at(i as f32 / 12.0), theme::TEXT, e),
                e * fade,
            );
            p.text(
                pos2(x, y + 10.0 * (1.0 - e)),
                Align2::LEFT_CENTER,
                ch,
                font.clone(),
                col,
            );
        }
        x += w;
    }
    let tag = ((t - 0.75) / 0.25).clamp(0.0, 1.0) * fade;
    if tag > 0.0 {
        p.text(
            pos2(c.x, y + 34.0),
            Align2::CENTER_CENTER,
            "Stems for samples and flips",
            theme::semi(12.0),
            theme::alpha(theme::TEXT_DIM, tag),
        );
    }
}

/// The logo: a gradient tile with five bars. `bars` scales each bar (0..1+, may overshoot);
/// `colored` tints the bars in stem colours (intro / audio-reactive), white otherwise.
pub fn logo_mark(p: &Painter, tile: Rect, bars: &[f32], glow: f32, opacity: f32, colored: bool) {
    let s = tile.width();
    let r = CornerRadius::same((s * 0.32).min(255.0) as u8);
    // Flat graphite tile with a hairline; the bars carry the stem colours.
    let _ = glow;
    p.rect(
        tile,
        r,
        theme::alpha(theme::CARD_HI, opacity),
        egui::Stroke::new(1.0, theme::alpha(theme::STROKE_HI, opacity)),
        egui::StrokeKind::Inside,
    );
    let bw = s * 0.077;
    let step = s * 0.132;
    let x0 = tile.center().x - 2.0 * step - bw / 2.0;
    for (i, h) in LOGO_BARS.iter().enumerate() {
        let k = bars.get(i).copied().unwrap_or(1.0).max(0.0);
        let bh = (s * 0.6 * h * k).max(0.0);
        if bh < 0.5 {
            continue;
        }
        let x = x0 + i as f32 * step;
        let bar = Rect::from_min_max(
            pos2(x, tile.center().y - bh / 2.0),
            pos2(x + bw, tile.center().y + bh / 2.0),
        );
        // One channel colour per bar: the mark is a tiny mixer.
        let col = if colored {
            theme::lerp_color(BAR_COLORS[i], Color32::WHITE, 0.2)
        } else {
            BAR_COLORS[i]
        };
        p.rect_filled(
            bar,
            CornerRadius::same((bw / 2.0) as u8),
            theme::alpha(col, opacity),
        );
    }
}

/// Centre of a rect as a point (helper for callers positioning the mark).
pub fn tile_at(center: Pos2, size: f32) -> Rect {
    Rect::from_center_size(center, vec2(size, size))
}
