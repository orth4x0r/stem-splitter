//! Window chrome: header with logo and status chips, navigation sidebar, backdrop, drop overlay.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Order, Rect, Sense, Stroke, StrokeKind,
    pos2, vec2,
};

use crate::app::{Page, StemApp};
use crate::theme;
use crate::widgets::icons::{self, Icon};
use crate::widgets::{paint, progress};

pub fn header(app: &mut StemApp, ui: &mut egui::Ui) {
    egui::Panel::top("header")
        .exact_size(64.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(theme::BG_PANEL))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            let t = ui.input(|i| i.time);
            let busy = app.job.is_some() || app.player.is_playing();
            let p = ui.painter();

            // Flowing accent line along the bottom edge. Static when idle.
            let flow = ui.ctx().animate_value_with_time(
                Id::unique("header-flow"),
                if busy { 1.0 } else { 0.0 },
                0.6,
            );
            let u = if busy || flow > 0.0 {
                (t * 0.12) as f32
            } else {
                0.0
            };
            let line = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 2.0), rect.max);
            paint::glow(
                p,
                line,
                CornerRadius::ZERO,
                theme::alpha(theme::VIOLET, 0.25 + 0.35 * flow),
                10.0,
            );
            paint::gradient_strip(p, line, u, u + 1.0);

            // Logo mark: a tiny vinyl with three stem bars.
            let logo = Rect::from_center_size(
                pos2(rect.left() + 38.0, rect.center().y - 1.0),
                vec2(34.0, 34.0),
            );
            paint::glow(
                p,
                logo,
                CornerRadius::same(11),
                theme::alpha(theme::PINK, 0.35),
                12.0,
            );
            paint::gradient_rect(p, logo, CornerRadius::same(11), 0.0, 1.0, Color32::WHITE);
            let bars = [0.45, 0.8, 0.6, 0.95, 0.5];
            for (i, h) in bars.iter().enumerate() {
                let anim = if busy {
                    0.6 + 0.4 * ((t * 6.0 + i as f64 * 1.3).sin() as f32 * 0.5 + 0.5)
                } else {
                    1.0
                };
                let bh = 20.0 * h * anim;
                let x = logo.left() + 8.0 + i as f32 * 4.5;
                p.rect_filled(
                    Rect::from_min_max(
                        pos2(x, logo.center().y - bh / 2.0),
                        pos2(x + 2.6, logo.center().y + bh / 2.0),
                    ),
                    CornerRadius::same(1),
                    Color32::from_rgba_unmultiplied(255, 255, 255, 235),
                );
            }

            // Wordmark.
            let x = logo.right() + 14.0;
            p.text(
                pos2(x, rect.center().y - 9.0),
                Align2::LEFT_CENTER,
                "STEM SPLITTER",
                theme::bold(17.0),
                theme::TEXT,
            );
            p.text(
                pos2(x + 1.0, rect.center().y + 11.0),
                Align2::LEFT_CENTER,
                "AI stems · samples · flips",
                theme::regular(11.5),
                theme::TEXT_FAINT,
            );

            // Right side status chips.
            let mut right = rect.right() - 24.0;
            let model = app
                .backend
                .model(app.settings.model_id.as_deref())
                .map_or("No model".to_owned(), |m| m.name.to_owned());
            let device = app.backend.device_name(app.settings.device);
            let device_short = device.replace(" (DirectML)", "");
            for (icon, text, color, page) in [
                (Icon::Chip, device_short, theme::GOOD, Page::Settings),
                (Icon::Models, model, theme::VIOLET, Page::Models),
            ] {
                let galley = p.layout_no_wrap(text.clone(), theme::semi(12.5), theme::TEXT);
                let w = galley.size().x + 46.0;
                let r = Rect::from_min_size(pos2(right - w, rect.center().y - 15.0), vec2(w, 30.0));
                right -= w + 10.0;
                let resp = ui
                    .interact(r, Id::unique(("chip", text.as_str())), Sense::click())
                    .on_hover_cursor(CursorIcon::PointingHand);
                let h = ui
                    .ctx()
                    .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                let p = ui.painter();
                p.rect(
                    r,
                    CornerRadius::same(15),
                    theme::lerp_color(theme::CARD, theme::CARD_HI, h),
                    Stroke::new(1.0, theme::lerp_color(theme::STROKE, color, 0.3 + 0.5 * h)),
                    StrokeKind::Inside,
                );
                let ir =
                    Rect::from_center_size(pos2(r.left() + 18.0, r.center().y), vec2(14.0, 14.0));
                icons::paint(p, ir, icon, color);
                p.galley(
                    pos2(r.left() + 32.0, r.center().y - galley.size().y / 2.0),
                    galley,
                    theme::TEXT,
                );
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

pub fn sidebar(app: &mut StemApp, ui: &mut egui::Ui) {
    egui::Panel::left("nav")
        .exact_size(212.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(theme::BG_PANEL)
                .inner_margin(egui::Margin::symmetric(14, 20)),
        )
        .show(ui, |ui| {
            let full = ui.max_rect();
            ui.painter().vline(
                full.right() + 14.0,
                full.y_range().expand(20.0),
                Stroke::new(1.0, theme::STROKE),
            );
            crate::widgets::overline(ui, "Workspace");
            ui.add_space(4.0);
            let pill_slot = ui.painter().add(egui::Shape::Noop);
            let items = [
                (Page::Split, Icon::Split, "Split"),
                (Page::Library, Icon::Library, "Library"),
                (Page::Models, Icon::Models, "Models"),
                (Page::Settings, Icon::Settings, "Settings"),
            ];
            let mut rects = Vec::new();
            for (page, icon, label) in items {
                let (r, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                if resp.clicked() {
                    app.page = page;
                }
                rects.push((page, r));
                let h = ui
                    .ctx()
                    .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                let sel = app.page == page;
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
                let ir =
                    Rect::from_center_size(pos2(r.left() + 24.0, r.center().y), vec2(17.0, 17.0));
                icons::paint(p, ir, icon, if sel { theme::PINK } else { color });
                p.text(
                    pos2(r.left() + 46.0, r.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    theme::semi(14.0),
                    color,
                );

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
                        progress::dots(p, c, 7.0, ui.input(|i| i.time), theme::CYAN);
                    } else {
                        let br = Rect::from_center_size(c, vec2(24.0, 18.0));
                        p.rect_filled(br, CornerRadius::same(9), theme::CARD_HI);
                        p.text(
                            c,
                            Align2::CENTER_CENTER,
                            b,
                            theme::mono(11.0),
                            theme::TEXT_DIM,
                        );
                    }
                }
            }
            // Sliding selection pill behind the active item.
            if let Some((_, r)) = rects.iter().find(|(p, _)| *p == app.page) {
                let y = ui
                    .ctx()
                    .animate_value_with_time(Id::unique("nav-sel"), r.top(), 0.2);
                let sel = Rect::from_min_size(pos2(r.left(), y), r.size());
                let ctx = ui.ctx().clone();
                let bar = Rect::from_min_size(
                    pos2(sel.left() - 14.0, sel.top() + 9.0),
                    vec2(3.0, sel.height() - 18.0),
                );
                let shapes = vec![
                    egui::epaint::RectShape::new(
                        sel,
                        CornerRadius::same(10),
                        Color32::from_rgb(30, 26, 52),
                        Stroke::new(1.0, theme::alpha(theme::VIOLET, 0.45)),
                        StrokeKind::Inside,
                    )
                    .into(),
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

            // Engine card at the bottom.
            let bottom = ui.max_rect().bottom();
            let card =
                Rect::from_min_max(pos2(full.left(), bottom - 92.0), pos2(full.right(), bottom));
            let p = ui.painter();
            p.rect(
                card,
                CornerRadius::same(12),
                theme::CARD,
                Stroke::new(1.0, theme::STROKE),
                StrokeKind::Inside,
            );
            p.text(
                pos2(card.left() + 14.0, card.top() + 18.0),
                Align2::LEFT_CENTER,
                "ENGINE",
                theme::semi(10.5),
                theme::TEXT_FAINT,
            );
            let (dot, status) = if app.job.is_some() {
                (theme::CYAN, "Working")
            } else if app.backend.problem.is_some() {
                (theme::WARN, "Backend missing")
            } else {
                (theme::GOOD, "Ready")
            };
            let pulse = if app.job.is_some() {
                paint::pulse(ui.input(|i| i.time), 1.2)
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
            let dev = app.backend.device_name(app.settings.device);
            let dev = crate::widgets::toast::elide(&dev, 26);
            p.text(
                pos2(card.left() + 14.0, card.top() + 66.0),
                Align2::LEFT_CENTER,
                dev,
                theme::regular(11.5),
                theme::TEXT_DIM,
            );
        });
}

/// Subtle radial blobs of colour behind the content, for depth.
pub fn backdrop(ui: &egui::Ui) {
    let r = ui.max_rect();
    let p = ui.painter();
    paint::glow_circle(
        p,
        pos2(r.right() - 120.0, r.top() + 40.0),
        160.0,
        theme::alpha(theme::VIOLET, 0.07),
        260.0,
    );
    paint::glow_circle(
        p,
        pos2(r.left() + 160.0, r.bottom() - 60.0),
        140.0,
        theme::alpha(theme::PINK, 0.045),
        240.0,
    );
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
            let appear = ctx.animate_bool_with_time(Id::unique("drop-appear"), true, 0.2);
            let p = ui.painter();
            p.rect_filled(
                screen,
                CornerRadius::ZERO,
                Color32::from_rgba_unmultiplied(6, 6, 12, (190.0 * appear) as u8),
            );
            let inner = screen.shrink(28.0);
            let pulse = paint::pulse(t, 1.1);
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
            paint::glow_circle(
                p,
                inner.center() - vec2(0.0, 30.0),
                120.0,
                theme::alpha(theme::VIOLET, 0.10 + 0.06 * pulse),
                200.0,
            );
            paint::dashed_rect(
                p,
                inner,
                28.0,
                Stroke::new(3.0, theme::accent_at((t * 0.2) as f32)),
                16.0,
                10.0,
                (t * 40.0) as f32,
            );
            let c = inner.center();
            let rr = 52.0 + 6.0 * pulse;
            paint::glow_circle(
                p,
                c - vec2(0.0, 30.0),
                rr,
                theme::alpha(theme::PINK, 0.35),
                30.0,
            );
            let disc = Rect::from_center_size(c - vec2(0.0, 30.0), vec2(2.0 * rr, 2.0 * rr));
            paint::gradient_rect(
                p,
                disc,
                CornerRadius::same(rr as u8),
                (t * 0.1) as f32,
                (t * 0.1) as f32 + 1.0,
                Color32::WHITE,
            );
            icons::paint(
                p,
                Rect::from_center_size(disc.center(), vec2(40.0, 40.0)),
                Icon::Download,
                Color32::WHITE,
            );
            p.text(
                c + vec2(0.0, 52.0),
                Align2::CENTER_CENTER,
                "Drop to split",
                theme::bold(28.0),
                theme::TEXT,
            );
            p.text(
                c + vec2(0.0, 84.0),
                Align2::CENTER_CENTER,
                "WAV · MP3 · FLAC · M4A · OGG",
                theme::mono(13.0),
                theme::TEXT_DIM,
            );
        });
}
