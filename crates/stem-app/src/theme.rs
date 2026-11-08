//! Colours, fonts and egui style for the "neon studio" look.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    TextStyle, Vec2,
};
use stem_models::StemKind;

// ---- palette -------------------------------------------------------------------------------

/// Window background: almost-black with a hint of indigo.
pub const BG: Color32 = Color32::from_rgb(9, 9, 15);
/// Sidebar / header background.
pub const BG_PANEL: Color32 = Color32::from_rgb(13, 13, 22);
/// Card surface.
pub const CARD: Color32 = Color32::from_rgb(20, 20, 33);
/// Raised surface inside a card (inputs, chips).
pub const CARD_HI: Color32 = Color32::from_rgb(28, 28, 45);
pub const STROKE: Color32 = Color32::from_rgb(38, 38, 60);
pub const STROKE_HI: Color32 = Color32::from_rgb(62, 60, 96);

pub const TEXT: Color32 = Color32::from_rgb(236, 236, 248);
pub const TEXT_DIM: Color32 = Color32::from_rgb(150, 150, 178);
/// Faint text still meets WCAG AA (4.5:1) on `BG` and ~4.4:1 on `CARD`.
pub const TEXT_FAINT: Color32 = Color32::from_rgb(124, 124, 156);
/// Purely decorative text (never the only carrier of information).
pub const TEXT_GHOST: Color32 = Color32::from_rgb(84, 84, 110);

/// Accent gradient stops: hot pink -> violet -> cyan.
pub const PINK: Color32 = Color32::from_rgb(255, 46, 136);
pub const VIOLET: Color32 = Color32::from_rgb(139, 92, 246);
pub const CYAN: Color32 = Color32::from_rgb(34, 211, 238);
pub const ACCENT_GRADIENT: [Color32; 3] = [PINK, VIOLET, CYAN];

pub const GOOD: Color32 = Color32::from_rgb(52, 211, 153);
pub const WARN: Color32 = Color32::from_rgb(251, 191, 36);
pub const BAD: Color32 = Color32::from_rgb(248, 113, 113);

pub fn stem_color(kind: StemKind) -> Color32 {
    match kind {
        StemKind::Vocals => Color32::from_rgb(255, 77, 157),
        StemKind::Drums => Color32::from_rgb(255, 170, 51),
        StemKind::Bass => Color32::from_rgb(150, 105, 255),
        StemKind::Other => Color32::from_rgb(34, 211, 238),
        StemKind::Instrumental => Color32::from_rgb(45, 212, 191),
        StemKind::Guitar => Color32::from_rgb(74, 222, 128),
        StemKind::Piano => Color32::from_rgb(253, 224, 71),
        StemKind::Dialogue => Color32::from_rgb(251, 146, 60),
        StemKind::Music => Color32::from_rgb(96, 165, 250),
        StemKind::Effects => Color32::from_rgb(244, 114, 182),
        StemKind::LeadVocals => Color32::from_rgb(255, 92, 120),
        StemKind::BackingVocals => Color32::from_rgb(232, 121, 249),
    }
}

/// Keyboard focus ring colour (high contrast against every surface and the accent gradient).
pub const FOCUS: Color32 = Color32::from_rgb(120, 230, 255);

// ---- glass ---------------------------------------------------------------------------------

/// Translucent card fill: the ambient background shows through a little.
pub const GLASS: Color32 = Color32::from_rgba_premultiplied(16, 16, 28, 222);
/// Header / sidebar chrome glass (a bit denser so text stays crisp).
pub const GLASS_CHROME: Color32 = Color32::from_rgba_premultiplied(10, 10, 18, 218);
/// Hairline on glass edges.
pub const GLASS_STROKE: Color32 = Color32::from_rgb(42, 42, 66);
/// Brighter hairline along the lit (top) edge of glass.
pub const GLASS_EDGE: Color32 = Color32::from_rgba_premultiplied(70, 68, 104, 255);

// ---- metrics -------------------------------------------------------------------------------

pub const R_CARD: u8 = 16;
pub const R_CTRL: u8 = 10;

/// Spacing rhythm: everything snaps to a 4 px grid.
pub mod space {
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
}

/// Type scale (points). Display/H1/H2 use ExtraBold, H3 SemiBold, the rest Regular.
pub mod size {
    pub const DISPLAY: f32 = 44.0;
    pub const H1: f32 = 30.0;
    pub const H2: f32 = 19.0;
    pub const H3: f32 = 16.0;
    pub const BODY: f32 = 14.0;
    pub const SMALL: f32 = 12.5;
    pub const CAPTION: f32 = 11.0;
}

/// The motion system: shared durations, easing curves, springs and entrance helpers.
/// Everything here honours "Reduce motion" (see [`motion::set_reduced`]).
pub mod motion {
    use std::sync::atomic::{AtomicBool, Ordering};

    use eframe::egui::{self, Id};

    /// Hover / focus feedback.
    pub const FAST: f32 = 0.12;
    /// Toggles, selections, small moves.
    pub const BASE: f32 = 0.18;
    /// Panels, cards, page content.
    pub const SLOW: f32 = 0.32;
    /// Entrances (page / list items).
    pub const ENTER: f32 = 0.42;
    /// Delay between staggered list items.
    pub const STAGGER: f32 = 0.05;

    static REDUCED: AtomicBool = AtomicBool::new(false);

    /// Set once per frame from the user's settings.
    pub fn set_reduced(on: bool) {
        REDUCED.store(on, Ordering::Relaxed);
    }
    /// True when the user asked for less motion: no ambient drift, confetti, springs or
    /// entrances. Progress indicators keep moving (they carry information).
    pub fn reduced() -> bool {
        REDUCED.load(Ordering::Relaxed)
    }

    pub fn ease_out(t: f32) -> f32 {
        egui::emath::easing::cubic_out(t.clamp(0.0, 1.0))
    }
    pub fn ease_in_out(t: f32) -> f32 {
        egui::emath::easing::cubic_in_out(t.clamp(0.0, 1.0))
    }
    /// Overshoots a little, then settles: springy pop-ins.
    pub fn ease_out_back(t: f32) -> f32 {
        egui::emath::easing::back_out(t.clamp(0.0, 1.0))
    }
    pub fn ease_out_expo(t: f32) -> f32 {
        egui::emath::easing::exponential_out(t.clamp(0.0, 1.0))
    }

    #[derive(Clone, Copy)]
    struct SpringState {
        x: f32,
        v: f32,
    }

    /// Damped spring towards `target` (stiffness `k`, damping `c`). Keeps repainting while it
    /// moves; jumps straight to `target` under reduced motion.
    pub fn spring(ctx: &egui::Context, id: Id, target: f32, k: f32, c: f32) -> f32 {
        if reduced() {
            ctx.data_mut(|d| d.insert_temp(id, SpringState { x: target, v: 0.0 }));
            return target;
        }
        let dt = ctx.input(|i| i.stable_dt).clamp(1.0 / 240.0, 1.0 / 30.0);
        let mut s = ctx
            .data(|d| d.get_temp::<SpringState>(id))
            .unwrap_or(SpringState { x: target, v: 0.0 });
        // Four sub-steps keep stiff springs stable at low frame rates.
        for _ in 0..4 {
            let h = dt / 4.0;
            let a = -k * (s.x - target) - c * s.v;
            s.v += a * h;
            s.x += s.v * h;
        }
        let scale = target.abs().max(1.0);
        if (s.x - target).abs() < 0.0005 * scale && s.v.abs() < 0.005 * scale {
            s = SpringState { x: target, v: 0.0 };
        } else {
            ctx.request_repaint();
        }
        ctx.data_mut(|d| d.insert_temp(id, s));
        s.x
    }

    /// Bouncy spring for UI "pops" (presses, toggles).
    pub fn bouncy(ctx: &egui::Context, id: Id, target: f32) -> f32 {
        spring(ctx, id, target, 520.0, 20.0)
    }
    /// Smooth, barely-overshooting spring for positions (pills, highlights).
    pub fn smooth(ctx: &egui::Context, id: Id, target: f32) -> f32 {
        spring(ctx, id, target, 300.0, 30.0)
    }

    #[derive(Clone, Copy)]
    struct Appear {
        born: f64,
        last_pass: u64,
    }

    /// Entrance progress 0..1 for something drawn every frame under `id`. Restarts when the
    /// item was not drawn in the previous pass (page switch, list shown again). `delay`
    /// staggers items. Always 1.0 under reduced motion.
    pub fn appear(ctx: &egui::Context, id: Id, delay: f32, dur: f32) -> f32 {
        if reduced() {
            return 1.0;
        }
        let now = ctx.input(|i| i.time);
        let pass = ctx.cumulative_pass_nr();
        let mut a = ctx.data(|d| d.get_temp::<Appear>(id)).unwrap_or(Appear {
            born: now,
            last_pass: pass,
        });
        if pass > a.last_pass + 1 {
            a.born = now;
        }
        a.last_pass = pass;
        ctx.data_mut(|d| d.insert_temp(id, a));
        let t = (((now - a.born) as f32 - delay) / dur).clamp(0.0, 1.0);
        if t < 1.0 {
            ctx.request_repaint();
        }
        t
    }

    #[derive(Clone, Copy)]
    struct Count {
        from: f32,
        to: f32,
        born: f64,
    }

    /// A number that counts up from zero on first sight, then glides to new targets
    /// (stats, percentages). Exponential ease-out, 0.9 s.
    pub fn count_up(ctx: &egui::Context, id: Id, target: f32) -> f32 {
        if reduced() {
            return target;
        }
        const DUR: f64 = 0.9;
        let now = ctx.input(|i| i.time);
        let value = |c: &Count| {
            let t = ((now - c.born) / DUR).clamp(0.0, 1.0) as f32;
            c.from + (c.to - c.from) * ease_out_expo(t)
        };
        let mut c = ctx.data(|d| d.get_temp::<Count>(id)).unwrap_or(Count {
            from: 0.0,
            to: target,
            born: now,
        });
        if c.to != target {
            c = Count {
                from: value(&c),
                to: target,
                born: now,
            };
        }
        ctx.data_mut(|d| d.insert_temp(id, c));
        if now - c.born < DUR {
            ctx.request_repaint();
        }
        value(&c)
    }
}

pub fn card_shadow() -> Shadow {
    Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(110),
    }
}

/// A glass card frame: translucent fill, hairline, deep soft shadow. Pair with
/// [`crate::widgets::paint::glass_sheen`] for the lit top edge.
pub fn glass_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(GLASS)
        .stroke(Stroke::new(1.0, GLASS_STROKE))
        .corner_radius(CornerRadius::same(R_CARD))
        .inner_margin(egui::Margin::same(20))
        .shadow(card_shadow())
}

/// A standard card frame.
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(CornerRadius::same(R_CARD))
        .inner_margin(egui::Margin::same(18))
        .shadow(card_shadow())
}

// ---- fonts ---------------------------------------------------------------------------------

pub const FONT_BOLD: &str = "inter-bold";
pub const FONT_SEMI: &str = "inter-semibold";
pub const FONT_MONO: &str = "jbmono";

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FONT_BOLD.into()))
}
pub fn semi(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FONT_SEMI.into()))
}
pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(
            name.to_owned(),
            std::sync::Arc::new(FontData::from_static(bytes)),
        );
    };
    add(
        &mut fonts,
        "inter",
        include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    );
    add(
        &mut fonts,
        FONT_SEMI,
        include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    );
    add(
        &mut fonts,
        FONT_BOLD,
        include_bytes!("../assets/fonts/Inter-ExtraBold.ttf"),
    );
    add(
        &mut fonts,
        FONT_MONO,
        include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf"),
    );

    // Keep egui's bundled fonts as fallbacks (symbols, emoji).
    let fallbacks: Vec<String> = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let with_fallbacks = |first: &str| {
        let mut v = vec![first.to_owned()];
        v.extend(fallbacks.iter().cloned());
        v
    };
    fonts
        .families
        .insert(FontFamily::Proportional, with_fallbacks("inter"));
    fonts
        .families
        .insert(FontFamily::Monospace, with_fallbacks(FONT_MONO));
    fonts.families.insert(
        FontFamily::Name(FONT_SEMI.into()),
        with_fallbacks(FONT_SEMI),
    );
    fonts.families.insert(
        FontFamily::Name(FONT_BOLD.into()),
        with_fallbacks(FONT_BOLD),
    );
    ctx.set_fonts(fonts);
}

// ---- style ---------------------------------------------------------------------------------

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, semi(22.0)),
            (TextStyle::Body, regular(14.0)),
            (TextStyle::Button, semi(13.5)),
            (TextStyle::Small, regular(11.5)),
            (TextStyle::Monospace, mono(13.0)),
        ]
        .into();
        style.animation_time = motion::BASE;
        style.spacing.item_spacing = Vec2::new(10.0, 10.0);
        style.spacing.button_padding = Vec2::new(14.0, 8.0);
        style.spacing.interact_size = Vec2::new(40.0, 30.0);
        style.spacing.slider_rail_height = 4.0;
        style.spacing.combo_width = 180.0;

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.window_stroke = Stroke::new(1.0, STROKE_HI);
        v.window_corner_radius = CornerRadius::same(R_CARD);
        v.window_shadow = card_shadow();
        v.popup_shadow = card_shadow();
        v.menu_corner_radius = CornerRadius::same(R_CTRL);
        v.extreme_bg_color = Color32::from_rgb(12, 12, 20);
        v.text_edit_bg_color = Some(Color32::from_rgb(12, 12, 20));
        v.faint_bg_color = CARD_HI;
        v.code_bg_color = CARD_HI;
        v.hyperlink_color = CYAN;
        v.warn_fg_color = WARN;
        v.error_fg_color = BAD;
        v.selection.bg_fill = VIOLET.gamma_multiply(0.55);
        v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        v.indent_has_left_vline = false;

        let r = CornerRadius::same(R_CTRL);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = CARD;
        w.noninteractive.weak_bg_fill = CARD;
        w.noninteractive.bg_stroke = Stroke::new(1.0, STROKE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
        w.noninteractive.corner_radius = r;

        w.inactive.bg_fill = CARD_HI;
        w.inactive.weak_bg_fill = CARD_HI;
        w.inactive.bg_stroke = Stroke::new(1.0, STROKE);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = r;

        w.hovered.bg_fill = Color32::from_rgb(38, 36, 62);
        w.hovered.weak_bg_fill = Color32::from_rgb(38, 36, 62);
        w.hovered.bg_stroke = Stroke::new(1.0, VIOLET.gamma_multiply(0.8));
        w.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 1.0;

        w.active.bg_fill = Color32::from_rgb(52, 44, 90);
        w.active.weak_bg_fill = Color32::from_rgb(52, 44, 90);
        w.active.bg_stroke = Stroke::new(1.0, PINK);
        w.active.fg_stroke = Stroke::new(2.0, Color32::WHITE);
        w.active.corner_radius = r;

        w.open = w.hovered;
    });
}

// ---- colour helpers ------------------------------------------------------------------------

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

/// Sample a multi-stop gradient at `t` in `[0, 1]`.
pub fn gradient_at(stops: &[Color32], t: f32) -> Color32 {
    match stops.len() {
        0 => Color32::WHITE,
        1 => stops[0],
        n => {
            let t = t.clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (t.floor() as usize).min(n - 2);
            lerp_color(stops[i], stops[i + 1], t - i as f32)
        }
    }
}

/// The accent gradient sampled at `t`, wrapping (mirrored) so it can scroll forever.
pub fn accent_at(t: f32) -> Color32 {
    let t = t.rem_euclid(2.0);
    let t = if t > 1.0 { 2.0 - t } else { t };
    gradient_at(&ACCENT_GRADIENT, t)
}

pub fn alpha(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}
