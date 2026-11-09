//! Colours, fonts and egui style: quiet graphite chrome, colour in the stems.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke,
    TextStyle, Vec2,
};
use stem_models::StemKind;

// ---- palette -------------------------------------------------------------------------------
//
// Direction (unslop pass): a late-night studio desk. The chrome is quiet graphite, like a
// mixing console in a dark room; the colour lives in the stems (each one a channel colour
// that means "this is the vocals / drums / ...") and in one warm accent, the amber of a lit
// VU meter, used for the playhead, the selection and the one number that matters.

/// Window background: graphite.
pub const BG: Color32 = Color32::from_rgb(17, 17, 19);
/// Sidebar / header background.
pub const BG_PANEL: Color32 = Color32::from_rgb(21, 21, 23);
/// Card surface.
pub const CARD: Color32 = Color32::from_rgb(26, 26, 29);
/// Raised surface inside a card (inputs, chips).
pub const CARD_HI: Color32 = Color32::from_rgb(34, 34, 38);
/// Hairline (the text colour at ~8% on the surfaces).
pub const STROKE: Color32 = Color32::from_rgb(40, 40, 44);
pub const STROKE_HI: Color32 = Color32::from_rgb(58, 58, 63);

pub const TEXT: Color32 = Color32::from_rgb(236, 236, 232);
pub const TEXT_DIM: Color32 = Color32::from_rgb(164, 164, 160);
/// Faint text still meets WCAG AA (4.5:1) on `BG` and `CARD`.
pub const TEXT_FAINT: Color32 = Color32::from_rgb(132, 132, 130);
/// Purely decorative text (never the only carrier of information).
#[allow(dead_code)]
pub const TEXT_GHOST: Color32 = Color32::from_rgb(88, 88, 90);

/// The one accent: VU-lamp amber. Dark text on it (`ON_ACCENT`) clears 4.5:1.
pub const ACCENT: Color32 = Color32::from_rgb(236, 170, 72);
/// Text / icons drawn on an `ACCENT` or `PRIMARY` fill.
pub const ON_ACCENT: Color32 = Color32::from_rgb(20, 18, 16);
/// Primary button fill: off-white from the ramp, with dark text.
pub const PRIMARY: Color32 = Color32::from_rgb(232, 232, 226);

/// Legacy names from the neon theme. They all resolve to the single accent now, so old
/// call sites stay quiet; prefer `ACCENT` (or a neutral) in new code.
pub const PINK: Color32 = ACCENT;
pub const VIOLET: Color32 = ACCENT;
pub const CYAN: Color32 = ACCENT;
/// "Gradient" stops: one colour, so every gradient fill paints as a solid accent.
pub const ACCENT_GRADIENT: [Color32; 3] = [ACCENT, ACCENT, ACCENT];

pub const GOOD: Color32 = Color32::from_rgb(98, 196, 140);
pub const WARN: Color32 = Color32::from_rgb(226, 182, 80);
pub const BAD: Color32 = Color32::from_rgb(226, 108, 100);

/// Channel colours: one hue per stem, toned down so they sit on graphite like the coloured
/// scribble strips on a console. These are the screen's colour; the chrome stays grey.
pub fn stem_color(kind: StemKind) -> Color32 {
    // Values are pre-mixed 12% toward grey (150, 150, 150) so every channel sits at the same,
    // calmer intensity. Kept as plain constants: a runtime mix here crashed LLVM's Windows
    // codegen (rustc 1.97, x86_64-pc-windows-*, thin LTO).
    match kind {
        StemKind::Vocals => Color32::from_rgb(229, 115, 150),
        StemKind::Drums => Color32::from_rgb(229, 134, 85),
        StemKind::Bass => Color32::from_rgb(134, 131, 238),
        StemKind::Other => Color32::from_rgb(80, 190, 203),
        StemKind::Instrumental => Color32::from_rgb(74, 190, 166),
        StemKind::Guitar => Color32::from_rgb(124, 199, 115),
        StemKind::Piano => Color32::from_rgb(222, 201, 106),
        StemKind::Dialogue => Color32::from_rgb(233, 155, 92),
        StemKind::Music => Color32::from_rgb(106, 155, 229),
        StemKind::Effects => Color32::from_rgb(212, 124, 194),
        StemKind::LeadVocals => Color32::from_rgb(233, 106, 124),
        StemKind::BackingVocals => Color32::from_rgb(201, 131, 226),
        // Drum parts: the skins warm like the drums stem they come from, the metals cool.
        StemKind::Kick => Color32::from_rgb(226, 102, 80),
        StemKind::Snare => Color32::from_rgb(229, 194, 102),
        StemKind::Toms => Color32::from_rgb(229, 150, 124),
        StemKind::HiHat => Color32::from_rgb(176, 212, 115),
        StemKind::Ride => Color32::from_rgb(88, 173, 222),
        StemKind::Crash => Color32::from_rgb(150, 208, 219),
        // Clean-up: the kept part stays in the vocal family, what was taken out goes muted.
        StemKind::Dry => Color32::from_rgb(229, 138, 176),
        StemKind::Reverb => Color32::from_rgb(141, 146, 220),
        StemKind::Clean => Color32::from_rgb(115, 203, 168),
        StemKind::Noise => Color32::from_rgb(150, 154, 161),
    }
}

/// Keyboard focus ring colour (high contrast against every surface and the accent gradient).
pub const FOCUS: Color32 = Color32::from_rgb(236, 236, 232);

// ---- glass ---------------------------------------------------------------------------------

/// Translucent card fill: the ambient background shows through a little.
pub const GLASS: Color32 = CARD;
/// Header / sidebar chrome glass (a bit denser so text stays crisp).
pub const GLASS_CHROME: Color32 = BG_PANEL;
/// Hairline on glass edges.
pub const GLASS_STROKE: Color32 = STROKE;
/// Brighter hairline along the lit (top) edge of glass.
pub const GLASS_EDGE: Color32 = STROKE;

// ---- metrics -------------------------------------------------------------------------------

pub const R_CARD: u8 = 12;
pub const R_CTRL: u8 = 8;

/// Spacing rhythm: everything snaps to a 4 px grid.
#[allow(dead_code)] // Design tokens: not every step is used yet.
pub mod space {
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
}

/// Type scale (points). Headings use Medium (500), rare emphasis SemiBold, the rest Regular.
#[allow(dead_code)]
pub mod size {
    pub const DISPLAY: f32 = 40.0;
    pub const H1: f32 = 28.0;
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
    /// Entrance curve for pop-ins. Plain ease-out now: nothing overshoots.
    pub fn ease_out_back(t: f32) -> f32 {
        ease_out(t)
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

    /// Spring for presses and toggles. Critically damped now: it settles without bouncing.
    pub fn bouncy(ctx: &egui::Context, id: Id, target: f32) -> f32 {
        spring(ctx, id, target, 520.0, 46.0)
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
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(70),
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
#[allow(dead_code)] // Opaque variant kept for views that sit on busy backgrounds.
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, STROKE))
        .corner_radius(CornerRadius::same(R_CARD))
        .inner_margin(egui::Margin::same(18))
        .shadow(card_shadow())
}

// ---- fonts ---------------------------------------------------------------------------------

pub const FONT_BOLD: &str = "inter-medium";
pub const FONT_SEMI: &str = "inter-semibold";
pub const FONT_MONO: &str = "jbmono";

/// Headings: Inter Medium (500). Hierarchy comes from size and colour, not weight.
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
        include_bytes!("../assets/fonts/Inter-Medium.otf"),
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
        v.extreme_bg_color = Color32::from_rgb(14, 14, 16);
        v.text_edit_bg_color = Some(Color32::from_rgb(14, 14, 16));
        v.faint_bg_color = CARD_HI;
        v.code_bg_color = CARD_HI;
        v.hyperlink_color = TEXT;
        v.warn_fg_color = WARN;
        v.error_fg_color = BAD;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
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

        w.hovered.bg_fill = Color32::from_rgb(42, 42, 46);
        w.hovered.weak_bg_fill = Color32::from_rgb(42, 42, 46);
        w.hovered.bg_stroke = Stroke::new(1.0, STROKE_HI);
        w.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = Color32::from_rgb(50, 50, 55);
        w.active.weak_bg_fill = Color32::from_rgb(50, 50, 55);
        w.active.bg_stroke = Stroke::new(1.0, STROKE_HI);
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
