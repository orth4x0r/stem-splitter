//! Window chrome: glass header with the live logo and status chips, navigation sidebar with
//! the engine / output meter card, the audio-reactive transport edge and the drop overlay.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Key, Modifiers, Order, Rect, Sense,
    Stroke, StrokeKind, pos2, vec2,
};

use crate::app::{Page, StemApp};
use crate::splash;
use crate::theme::{self, motion};
use crate::widgets::icons::{self, Icon};
use crate::widgets::{meter, paint, progress};

const HEADER_H: f32 = 64.0;

pub fn header(app: &mut StemApp, ui: &mut egui::Ui) {
    egui::Panel::top("header")
        .exact_size(HEADER_H)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(theme::GLASS_CHROME))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            let t = ui.input(|i| i.time);
            let playing = app.player.is_playing();
            let busy = app.job.is_some() || playing;
            let level = app.fx.master;
            let p = ui.painter();

            // Flowing accent line along the bottom edge; breathes with the music.
            let flow = ui.ctx().animate_value_with_time(
                Id::unique("header-flow"),
                if busy { 1.0 } else { 0.0 },
                0.6,
            );
            let u = if (busy || flow > 0.0) && !motion::reduced() {
                (t * 0.12) as f32
            } else {
                0.0
            };
            let line = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 1.5), rect.max);
            paint::glow(
                p,
                line,
                CornerRadius::ZERO,
                theme::alpha(theme::VIOLET, 0.22 + 0.3 * flow + 0.4 * level),
                10.0 + 16.0 * level,
            );
            paint::gradient_strip(p, line, u, u + 1.0);

            // Logo mark: bars dance to the stems while playing, ripple while splitting.
            let tile = splash::tile_at(pos2(rect.left() + 38.0, rect.center().y), 36.0);
            let bars: Vec<f32> = (0..5)
                .map(|i| {
                    if playing || (app.shot.is_some() && level > 0.0) {
                        let tr = app.session.as_ref().map_or(0.0, |s| {
                            if s.tracks.is_empty() {
                                0.0
                            } else {
                                s.tracks[i % s.tracks.len()].level
                            }
                        });
                        0.35 + 0.75 * tr.min(1.0)
                    } else if app.job.is_some() && !motion::reduced() {
                        0.6 + 0.4 * ((t * 6.0 + i as f64 * 1.3).sin() as f32 * 0.5 + 0.5)
                    } else {
                        1.0
                    }
                })
                .collect();
            splash::logo_mark(p, tile, &bars, 0.5 + 0.5 * level, 1.0, playing);

            // Wordmark: white + gradient.
            let x = tile.right() + 14.0;
            let g = p.layout_no_wrap("STEM ".into(), theme::bold(17.0), theme::TEXT);
            let w = g.size().x;
            p.galley(
                pos2(x, rect.center().y - 9.0 - g.size().y / 2.0),
                g,
                theme::TEXT,
            );
            paint::gradient_text(
                p,
                pos2(x + w, rect.center().y - 9.0),
                Align2::LEFT_CENTER,
                "SPLITTER",
                theme::bold(17.0),
                (0.0, 0.9),
                1.0,
            );
            p.text(
                pos2(x + 1.0, rect.center().y + 11.0),
                Align2::LEFT_CENTER,
                "AI stems · samples · flips",
                theme::regular(11.5),
                theme::TEXT_FAINT,
            );

            // Centre: live job pill, so a split stays visible from any page.
            if let Some(job) = &app.job
                && app.page != Page::Split
            {
                let r = Rect::from_center_size(rect.center(), vec2(300.0, 34.0));
                let resp = ui
                    .interact(r, Id::unique("job-pill"), Sense::click())
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text("Back to the split");
                let p = ui.painter();
                paint::glow(
                    p,
                    r,
                    CornerRadius::same(17),
                    theme::alpha(theme::VIOLET, 0.25),
                    14.0,
                );
                p.rect(
                    r,
                    CornerRadius::same(17),
                    theme::GLASS,
                    Stroke::new(1.0, theme::alpha(theme::VIOLET, 0.6)),
                    StrokeKind::Inside,
                );
                progress::ring(
                    p,
                    pos2(r.left() + 18.0, r.center().y),
                    8.0,
                    2.5,
                    Some(job.shown),
                    t,
                );
                p.text(
                    pos2(r.left() + 36.0, r.center().y),
                    Align2::LEFT_CENTER,
                    crate::widgets::toast::elide(&job.title, 24),
                    theme::semi(12.5),
                    theme::TEXT,
                );
                p.text(
                    pos2(r.right() - 14.0, r.center().y),
                    Align2::RIGHT_CENTER,
                    format!("{:.0}%", job.shown * 100.0),
                    theme::mono(12.5),
                    theme::CYAN,
                );
                paint::focus(p, &resp, r, CornerRadius::same(17));
                if resp.clicked() {
                    app.page = Page::Split;
                }
            }

            // Right side status chips.
            let mut right = rect.right() - 24.0;
            let model = app
                .backend
                .model(app.settings.model_id.as_deref())
                .map_or("No model".to_owned(), |m| m.name.to_owned());
            let device = app.backend.device_name(app.settings.device);
            let device_short = device.replace(" (DirectML)", "");
            for (icon, text, color, page, tip) in [
                (
                    Icon::Chip,
                    device_short,
                    theme::GOOD,
                    Page::Settings,
                    "Processing device",
                ),
                (
                    Icon::Models,
                    model,
                    theme::VIOLET,
                    Page::Models,
                    "Separation model",
                ),
            ] {
                let galley = p.layout_no_wrap(text.clone(), theme::semi(12.5), theme::TEXT);
                let w = galley.size().x + 46.0;
                let r = Rect::from_min_size(pos2(right - w, rect.center().y - 15.0), vec2(w, 30.0));
                right -= w + 10.0;
                let resp = ui
                    .interact(r, Id::unique(("chip", text.as_str())), Sense::click())
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text(tip);
                let h = ui
                    .ctx()
                    .animate_bool_with_time(resp.id, resp.hovered(), motion::FAST);
                let p = ui.painter();
                if h > 0.0 {
                    paint::glow(
                        p,
                        r,
                        CornerRadius::same(15),
                        theme::alpha(color, 0.2 * h),
                        12.0,
                    );
                }
                p.rect(
                    r,
                    CornerRadius::same(15),
                    theme::lerp_color(theme::GLASS, theme::CARD_HI, h),
                    Stroke::new(1.0, theme::lerp_color(theme::STROKE, color, 0.3 + 0.5 * h)),
                    StrokeKind::Inside,
                );
                paint::glass_sheen(p, r, CornerRadius::same(15), h);
                paint::ripple(p, &resp, r, color);
                let ir =
                    Rect::from_center_size(pos2(r.left() + 18.0, r.center().y), vec2(14.0, 14.0));
                paint::glow_circle(p, ir.center(), 4.0, theme::alpha(color, 0.35), 8.0);
                icons::paint(p, ir, icon, color);
                p.galley(
                    pos2(r.left() + 32.0, r.center().y - galley.size().y / 2.0),
                    galley,
                    theme::TEXT,
                );
                paint::focus(p, &resp, r, CornerRadius::same(15));
                if resp.clicked() {
                    app.page = page;
                }
            }
            if app.demo() {
                let r = Rect::from_min_size(
                    pos2(right - 62.0, rect.center().y - 11.0),
                    vec2(62.0, 22.0),
                );
                let p = ui.painter();
                paint::glow(
                    p,
                    r,
                    CornerRadius::same(11),
                    theme::alpha(theme::WARN, 0.12),
                    10.0,
                );
                p.rect(
                    r,
                    CornerRadius::same(11),
                    theme::alpha(theme::WARN, 0.12),
                    Stroke::new(1.0, theme::alpha(theme::WARN, 0.5)),
                    StrokeKind::Inside,
                );
                p.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    "DEMO",
                    theme::bold(11.0),
                    theme::WARN,
                );
            }
        });
}

const NAV: [(Page, Icon, &str, Key); 4] = [
    (Page::Split, Icon::Split, "Split", Key::Num1),
    (Page::Library, Icon::Library, "Library", Key::Num2),
    (Page::Models, Icon::Models, "Models", Key::Num3),
    (Page::Settings, Icon::Settings, "Settings", Key::Num4),
];

pub fn sidebar(app: &mut StemApp, ui: &mut egui::Ui) {
    // Ctrl+1..4 switch pages.
    for (page, _, _, key) in NAV {
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, key)) {
            app.page = page;
        }
    }
    egui::Panel::left("nav")
        .exact_size(212.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(theme::GLASS_CHROME)
                .inner_margin(egui::Margin::symmetric(14, 20)),
        )
        .show(ui, |ui| {
            let full = ui.max_rect();
            // Right edge: hairline that fades out at both ends.
            let edge = Rect::from_min_max(
                pos2(full.right() + 13.5, full.top() - 20.0),
                pos2(full.right() + 14.5, full.bottom() + 20.0),
            );
            let mut mesh = egui::Mesh::default();
            for (i, f) in [0.0f32, 0.3, 0.7, 1.0].iter().enumerate() {
                let y = edge.top() + edge.height() * f;
                let c = if i == 0 || i == 3 {
                    Color32::TRANSPARENT
                } else {
                    theme::GLASS_EDGE
                };
                mesh.colored_vertex(pos2(edge.left(), y), c);
                mesh.colored_vertex(pos2(edge.right(), y), c);
                if i > 0 {
                    let b = (2 * i) as u32;
                    mesh.add_triangle(b - 2, b - 1, b);
                    mesh.add_triangle(b - 1, b + 1, b);
                }
            }
            ui.painter().add(egui::Shape::mesh(mesh));

            crate::widgets::overline(ui, "Workspace");
            ui.add_space(4.0);
            let pill_slot = ui.painter().add(egui::Shape::Noop);
            let mut rects = Vec::new();
            let t = ui.input(|i| i.time);
            for (page, icon, label, key) in NAV {
                let (r, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                let resp = resp
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text(format!("{label}  (Ctrl+{})", key.name()));
                resp.widget_info(|| {
                    egui::WidgetInfo::selected(egui::Role::Button, true, app.page == page, label)
                });
                if resp.clicked() {
                    app.page = page;
                }
                rects.push((page, r));
                let h = ui
                    .ctx()
                    .animate_bool_with_time(resp.id, resp.hovered(), motion::FAST);
                let sel = app.page == page;
                let nudge = motion::bouncy(
                    ui.ctx(),
                    resp.id.with("nudge"),
                    if resp.hovered() || sel { 3.0 } else { 0.0 },
                );
                let p = ui.painter();
                if h > 0.0 && !sel {
                    p.rect_filled(
                        r,
                        CornerRadius::same(10),
                        Color32::from_white_alpha((8.0 * h) as u8),
                    );
                }
                let color = if sel {
                    Color32::WHITE
                } else {
                    theme::lerp_color(theme::TEXT_DIM, theme::TEXT, h)
                };
                let ic = pos2(r.left() + 24.0 + nudge * 0.5, r.center().y);
                let ir = Rect::from_center_size(ic, vec2(17.0, 17.0));
                if sel {
                    paint::glow_circle(p, ic, 6.0, theme::alpha(theme::PINK, 0.45), 14.0);
                }
                icons::paint(p, ir, icon, if sel { theme::PINK } else { color });
                p.text(
                    pos2(r.left() + 46.0 + nudge, r.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    theme::semi(14.0),
                    color,
                );
                paint::focus(p, &resp, r, CornerRadius::same(10));

                // Badges: running job, history count.
                let badge = match page {
                    Page::Split if app.job.is_some() => Some(String::new()),
                    Page::Library if !app.settings.history.is_empty() => {
                        Some(app.settings.history.len().to_string())
                    }
                    Page::Models if !app.model_downloads.is_empty() => Some(String::new()),
                    _ => None,
                };
                if let Some(b) = badge {
                    let c = pos2(r.right() - 20.0, r.center().y);
                    if b.is_empty() {
                        progress::dots(p, c, 7.0, t, theme::CYAN);
                    } else {
                        let br = Rect::from_center_size(c, vec2(26.0, 18.0));
                        p.rect(
                            br,
                            CornerRadius::same(9),
                            if sel {
                                theme::alpha(theme::PINK, 0.18)
                            } else {
                                theme::CARD_HI
                            },
                            Stroke::new(
                                1.0,
                                if sel {
                                    theme::alpha(theme::PINK, 0.45)
                                } else {
                                    theme::STROKE
                                },
                            ),
                            StrokeKind::Inside,
                        );
                        p.text(
                            c,
                            Align2::CENTER_CENTER,
                            b,
                            theme::mono(11.0),
                            if sel { theme::TEXT } else { theme::TEXT_DIM },
                        );
                    }
                }
            }
            // Spring-loaded selection pill behind the active item.
            if let Some((_, r)) = rects.iter().find(|(p, _)| *p == app.page) {
                let y = motion::smooth(ui.ctx(), Id::unique("nav-sel"), r.top());
                let sel = Rect::from_min_size(pos2(r.left(), y), r.size());
                let ctx = ui.ctx().clone();
                let bar = Rect::from_min_size(
                    pos2(sel.left() - 14.0, sel.top() + 9.0),
                    vec2(3.0, sel.height() - 18.0),
                );
                let shapes = vec![
                    paint::glow_shape(
                        sel,
                        CornerRadius::same(10),
                        theme::alpha(theme::VIOLET, 0.16),
                        16.0,
                    ),
                    egui::epaint::RectShape::new(
                        sel,
                        CornerRadius::same(10),
                        Color32::from_rgb(30, 26, 54),
                        Stroke::new(1.0, theme::alpha(theme::VIOLET, 0.5)),
                        StrokeKind::Inside,
                    )
                    .into(),
                    paint::gradient_rect_shape(
                        &ctx,
                        sel.shrink(1.0),
                        CornerRadius::same(9),
                        0.0,
                        1.0,
                        Color32::from_white_alpha(14),
                    ),
                    paint::glow_shape(
                        bar,
                        CornerRadius::same(2),
                        theme::alpha(theme::PINK, 0.8),
                        8.0,
                    ),
                    paint::gradient_rect_shape(
                        &ctx,
                        bar,
                        CornerRadius::same(2),
                        0.0,
                        0.6,
                        Color32::WHITE,
                    ),
                ];
                ui.painter().set(pill_slot, egui::Shape::Vec(shapes));
            }

            engine_card(app, ui, full);
        });
}

/// Bottom-of-sidebar status card: engine state, or a live output meter while a session is open.
fn engine_card(app: &StemApp, ui: &egui::Ui, full: Rect) {
    let t = ui.input(|i| i.time);
    let bottom = ui.max_rect().bottom();
    let has_session = app.session.is_some() && app.job.is_none();
    let h = if has_session || app.job.is_some() {
        138.0
    } else {
        92.0
    };
    let card = Rect::from_min_max(pos2(full.left(), bottom - h), pos2(full.right(), bottom));
    let p = ui.painter();
    if has_session {
        meter::level_glow(p, card, CornerRadius::same(12), theme::PINK, app.fx.master);
    }
    paint::glass(p, card, CornerRadius::same(12), None, 0.0);
    let (dot, label, status) = if app.job.is_some() {
        (theme::CYAN, "ENGINE", "Working")
    } else if app.backend.problem.is_some() {
        (theme::WARN, "ENGINE", "Backend missing")
    } else if has_session && app.player.is_playing() {
        (theme::PINK, "OUTPUT", "Playing")
    } else if has_session {
        (theme::GOOD, "OUTPUT", "Ready")
    } else {
        (theme::GOOD, "ENGINE", "Ready")
    };
    p.text(
        pos2(card.left() + 14.0, card.top() + 18.0),
        Align2::LEFT_CENTER,
        label,
        theme::semi(10.5),
        theme::TEXT_FAINT,
    );
    let pulse = if app.job.is_some() || app.player.is_playing() {
        paint::pulse(t, 1.2)
    } else {
        0.0
    };
    let dc = pos2(card.left() + 19.0, card.top() + 42.0);
    paint::glow_circle(
        p,
        dc,
        3.0,
        theme::alpha(dot, 0.35 + 0.35 * pulse),
        4.0 + 3.0 * pulse,
    );
    p.circle_filled(dc, 4.0, dot);
    p.text(
        pos2(card.left() + 32.0, card.top() + 42.0),
        Align2::LEFT_CENTER,
        status,
        theme::semi(13.0),
        theme::TEXT,
    );

    if let Some(job) = &app.job {
        p.text(
            pos2(card.right() - 14.0, card.top() + 42.0),
            Align2::RIGHT_CENTER,
            format!("{:.0}%", job.shown * 100.0),
            theme::mono(12.0),
            theme::CYAN,
        );
        let bars = Rect::from_min_max(
            pos2(card.left() + 14.0, card.top() + 60.0),
            pos2(card.right() - 14.0, card.bottom() - 18.0),
        );
        progress::spectrum(p, bars, t, 0.8, 20);
    } else if let Some(s) = app.session.as_ref().filter(|_| has_session) {
        // Master VU with peak hold.
        let vu = Rect::from_min_max(
            pos2(card.left() + 14.0, card.top() + 60.0),
            pos2(card.right() - 14.0, card.top() + 68.0),
        );
        meter::vu(p, Id::unique("master-vu"), vu, app.fx.master, theme::GOOD);
        // Per-stem mini strips in stem colours.
        let n = s.tracks.len().max(1);
        let strip = Rect::from_min_max(
            pos2(card.left() + 14.0, card.top() + 80.0),
            pos2(card.right() - 14.0, card.bottom() - 12.0),
        );
        let gap = 4.0;
        let w = ((strip.width() - gap * (n as f32 - 1.0)) / n as f32).min(12.0);
        for (i, tr) in s.tracks.iter().enumerate() {
            let x = strip.left() + i as f32 * (w + gap);
            let r = Rect::from_min_size(pos2(x, strip.top()), vec2(w, strip.height()));
            let lvl = if s.audible(i) { tr.level } else { 0.0 };
            meter::bar(
                p,
                Id::unique(("stem-meter", i)),
                r,
                lvl,
                theme::stem_color(tr.kind),
            );
        }
    } else {
        let dev = app.backend.device_name(app.settings.device);
        let dev = crate::widgets::toast::elide(&dev, 26);
        p.text(
            pos2(card.left() + 14.0, card.top() + 66.0),
            Align2::LEFT_CENTER,
            dev,
            theme::regular(11.5),
            theme::TEXT_DIM,
        );
    }
}

/// Audio-reactive top edge of the transport bar: a gradient line whose glow follows the
/// master level. Painted over the transport panel, so `results.rs` stays untouched.
pub fn transport_edge(app: &StemApp, ui: &egui::Ui, screen: Rect, height: f32) {
    let level = app.fx.master;
    let y = screen.bottom() - height;
    let line = Rect::from_min_max(
        pos2(screen.left() + 212.0, y),
        pos2(screen.right(), y + 1.5),
    );
    let p = ui.painter();
    let t = ui.input(|i| i.time);
    let u = if motion::reduced() {
        0.0
    } else {
        (t * 0.1) as f32
    };
    if level > 0.01 {
        paint::glow(
            p,
            line,
            CornerRadius::ZERO,
            theme::alpha(theme::PINK, 0.08 + 0.22 * level),
            6.0 + 12.0 * level,
        );
    }
    let mut mesh = egui::Mesh::default();
    let n = 48;
    for i in 0..=n {
        let f = i as f32 / n as f32;
        let x = line.left() + line.width() * f;
        // Bright in the middle, fading out at both ends, brighter with the music.
        let env = (1.0 - (2.0 * f - 1.0).powi(2)).max(0.0);
        let c = theme::alpha(theme::accent_at(u + f), (0.25 + 0.75 * level) * env);
        mesh.colored_vertex(pos2(x, line.top()), c);
        mesh.colored_vertex(pos2(x, line.bottom()), c);
        if i > 0 {
            let b = (2 * i) as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    p.add(egui::Shape::mesh(mesh));
}

/// Full-window overlay while a file is dragged over the app.
pub fn drop_overlay(ctx: &egui::Context) {
    let t = ctx.input(|i| i.time);
    let screen = ctx.content_rect();
    egui::Area::new(Id::unique("drop-overlay"))
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .interactable(false)
        .show(ctx, |ui| {
            let appear = ctx.animate_bool_with_time(Id::unique("drop-appear"), true, motion::BASE);
            let p = ui.painter();
            p.rect_filled(
                screen,
                CornerRadius::ZERO,
                Color32::from_rgba_unmultiplied(5, 5, 10, (222.0 * appear) as u8),
            );
            let inner = screen.shrink(28.0);
            let pulse = paint::pulse(t, 1.1);
            let c = inner.center() - vec2(0.0, 30.0);
            // Room light.
            paint::glow_circle(
                p,
                c,
                160.0,
                theme::alpha(theme::VIOLET, 0.14 + 0.06 * pulse),
                260.0,
            );
            // Neon rim: thin glowing strips along each edge, so the middle stays dark.
            let glow_c = theme::alpha(theme::VIOLET, 0.35 + 0.3 * pulse);
            for edge in [
                Rect::from_min_max(inner.left_top(), pos2(inner.right(), inner.top() + 2.0)),
                Rect::from_min_max(
                    pos2(inner.left(), inner.bottom() - 2.0),
                    inner.right_bottom(),
                ),
                Rect::from_min_max(inner.left_top(), pos2(inner.left() + 2.0, inner.bottom())),
                Rect::from_min_max(pos2(inner.right() - 2.0, inner.top()), inner.right_bottom()),
            ] {
                paint::glow(
                    p,
                    edge.shrink2(vec2(20.0, 20.0).min(edge.size() / 2.0 - vec2(0.5, 0.5))),
                    CornerRadius::same(2),
                    glow_c,
                    28.0,
                );
            }
            paint::dashed_rect(
                p,
                inner,
                28.0,
                Stroke::new(3.0, theme::accent_at((t * 0.2) as f32)),
                16.0,
                10.0,
                if motion::reduced() {
                    0.0
                } else {
                    (t * 40.0) as f32
                },
            );
            // Sonar rings pulling the file in.
            if !motion::reduced() {
                for k in 0..3 {
                    let f = ((t * 0.6 + k as f64 / 3.0) % 1.0) as f32;
                    let r = 220.0 - 160.0 * f;
                    p.circle_stroke(
                        c,
                        r,
                        Stroke::new(
                            1.5,
                            theme::alpha(
                                theme::accent_at(k as f32 * 0.4),
                                0.5 * f * (1.0 - f) * 4.0 * 0.5,
                            ),
                        ),
                    );
                }
            }
            // Stem-coloured satellites orbiting the target.
            let sats = [
                theme::stem_color(stem_models::StemKind::Vocals),
                theme::stem_color(stem_models::StemKind::Drums),
                theme::stem_color(stem_models::StemKind::Bass),
                theme::stem_color(stem_models::StemKind::Other),
            ];
            for (i, col) in sats.iter().enumerate() {
                let a = (t * 0.9) as f32 + i as f32 * std::f32::consts::TAU / 4.0;
                let sp = c + vec2(a.cos() * 96.0, a.sin() * 96.0 * 0.92);
                paint::glow_circle(p, sp, 4.0, theme::alpha(*col, 0.6), 12.0);
                p.circle_filled(sp, 4.5, *col);
            }
            let rr = 52.0 + 6.0 * pulse;
            paint::glow_circle(p, c, rr, theme::alpha(theme::PINK, 0.35), 30.0);
            let disc = Rect::from_center_size(c, vec2(2.0 * rr, 2.0 * rr));
            paint::gradient_rect(
                p,
                disc,
                CornerRadius::same(rr as u8),
                (t * 0.1) as f32,
                (t * 0.1) as f32 + 1.0,
                Color32::WHITE,
            );
            paint::glass_sheen(p, disc, CornerRadius::same(rr as u8), 1.0);
            let bob = if motion::reduced() {
                0.0
            } else {
                4.0 * (t * 3.0).sin() as f32
            };
            icons::paint(
                p,
                Rect::from_center_size(disc.center() + vec2(0.0, bob), vec2(40.0, 40.0)),
                Icon::Download,
                Color32::WHITE,
            );
            p.text(
                c + vec2(0.0, 132.0),
                Align2::CENTER_CENTER,
                "Drop to split",
                theme::bold(30.0),
                theme::TEXT,
            );
            p.text(
                c + vec2(0.0, 166.0),
                Align2::CENTER_CENTER,
                "WAV · MP3 · FLAC · M4A · OGG",
                theme::mono(13.0),
                theme::TEXT_DIM,
            );
        });
}
