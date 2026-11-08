//! Start screen: paste a link or drop a file, pick a model.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Rect, Sense, Stroke, StrokeKind, pos2,
    vec2,
};

use crate::app::{Page, StemApp};
use crate::pipeline::InputSource;
use crate::theme;
use crate::widgets::buttons::{Button, Kind};
use crate::widgets::icons::{self, Icon};
use crate::widgets::{overline, paint};

pub fn page(app: &mut StemApp, ui: &mut egui::Ui) {
    let now = ui.input(|i| i.time);
    let max_w = ui.available_width().min(980.0);
    ui.vertical_centered(|ui| {
        ui.set_max_width(max_w);
        ui.add_space(14.0);
        hero(ui);
        ui.add_space(22.0);
        if let Some(f) = &app.failure {
            let (title, msg, src) = (f.title.clone(), f.message.clone(), f.source.clone());
            failure_card(app, ui, &title, &msg, src, now);
            ui.add_space(16.0);
        }
        url_bar(app, ui, now);
        ui.add_space(8.0);
        platforms(ui);
        ui.add_space(20.0);
        drop_zone(app, ui, now);
        ui.add_space(26.0);
        model_strip(app, ui);
        ui.add_space(26.0);
        recent(app, ui, now);
    });
}

fn hero(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("Pull any track apart.")
            .font(theme::bold(38.0))
            .color(theme::TEXT),
    );
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new("Paste a link or drop a file — AI splits it into vocals, drums, bass and more, ready to chop.")
            .font(theme::regular(15.0))
            .color(theme::TEXT_DIM),
    );
}

fn url_bar(app: &mut StemApp, ui: &mut egui::Ui, now: f64) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
    let id = Id::unique("url-input");
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    let f = ui.ctx().animate_bool_with_time(id.with("f"), focused, 0.2);
    let p = ui.painter();
    paint::glow(
        p,
        rect,
        CornerRadius::same(18),
        theme::alpha(theme::VIOLET, 0.10 + 0.25 * f),
        18.0 + 10.0 * f,
    );
    p.rect_filled(rect, CornerRadius::same(18), theme::CARD);
    if f > 0.0 {
        // Gradient border when focused: gradient rect under a slightly smaller fill.
        paint::gradient_rect(
            p,
            rect,
            CornerRadius::same(18),
            0.0,
            1.0,
            Color32::from_white_alpha((255.0 * f) as u8),
        );
        p.rect_filled(rect.shrink(1.5), CornerRadius::same(17), theme::CARD);
    } else {
        p.rect_stroke(
            rect,
            CornerRadius::same(18),
            Stroke::new(1.0, theme::STROKE_HI),
            StrokeKind::Inside,
        );
    }
    icons::paint(
        p,
        Rect::from_center_size(pos2(rect.left() + 28.0, rect.center().y), vec2(18.0, 18.0)),
        Icon::Link,
        theme::lerp_color(theme::TEXT_FAINT, theme::PINK, f),
    );

    let btn_w = 132.0;
    let edit_rect = Rect::from_min_max(
        pos2(rect.left() + 50.0, rect.center().y - 11.0),
        pos2(rect.right() - btn_w - 18.0, rect.center().y + 11.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(edit_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let resp = child.add(
        egui::TextEdit::singleline(&mut app.url_input)
            .id(id)
            .frame(egui::Frame::NONE)
            .hint_text(
                egui::RichText::new("Paste a YouTube, X, SoundCloud or audio link…")
                    .color(theme::TEXT_FAINT),
            )
            .font(theme::regular(16.0))
            .text_color(theme::TEXT)
            .desired_width(edit_rect.width()),
    );
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

    let btn_rect = Rect::from_min_size(
        pos2(rect.right() - btn_w - 10.0, rect.top() + 10.0),
        vec2(btn_w, 40.0),
    );
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(btn_rect));
    let go = Button::primary("Split it")
        .icon(Icon::Sparkle)
        .min_width(btn_w)
        .height(40.0)
        .show(&mut bui)
        .clicked();
    if go || enter {
        app.start_from_input(now);
    }
}

fn platforms(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let total = 5.0 * 110.0;
        ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
        for (name, color) in [
            ("YouTube", Color32::from_rgb(255, 72, 72)),
            ("X / Twitter", Color32::from_rgb(200, 200, 220)),
            ("SoundCloud", Color32::from_rgb(255, 136, 40)),
            ("Bandcamp", Color32::from_rgb(98, 154, 169)),
            ("Direct links", theme::CYAN),
        ] {
            let galley =
                ui.painter()
                    .layout_no_wrap(name.to_owned(), theme::regular(12.5), theme::TEXT_DIM);
            let (r, _) = ui.allocate_exact_size(vec2(galley.size().x + 22.0, 20.0), Sense::hover());
            ui.painter()
                .circle_filled(pos2(r.left() + 6.0, r.center().y), 3.0, color);
            ui.painter().galley(
                pos2(r.left() + 15.0, r.center().y - galley.size().y / 2.0),
                galley,
                theme::TEXT_DIM,
            );
        }
    });
}

fn drop_zone(app: &mut StemApp, ui: &mut egui::Ui, now: f64) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 230.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.2);
    let drag = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("d"), app.drop_hover, 0.2);
    let active = hover.max(drag);
    let p = ui.painter();
    let pulse = if app.drop_hover {
        paint::pulse(now, 1.0)
    } else {
        0.0
    };

    p.rect_filled(
        rect,
        CornerRadius::same(22),
        theme::lerp_color(theme::alpha(theme::CARD, 0.7), theme::CARD_HI, 0.4 * hover),
    );
    paint::glow(
        p,
        rect,
        CornerRadius::same(22),
        theme::alpha(
            theme::VIOLET,
            0.02 + 0.03 * hover + 0.16 * drag + 0.12 * pulse,
        ),
        30.0,
    );
    // Faint waveform watermark across the zone.
    let mid = rect.center().y + 20.0;
    for i in 0..120 {
        let x = rect.left() + 30.0 + i as f32 * (rect.width() - 60.0) / 120.0;
        let f = i as f32 / 120.0;
        let h = ((f * 37.0).sin() * 0.5 + 0.5)
            * ((f * 7.0 + 1.0).sin() * 0.5 + 0.6)
            * 46.0
            * (1.0 - (f - 0.5).abs() * 1.5).max(0.1);
        let c = theme::alpha(theme::accent_at(f), 0.05 + 0.08 * active);
        p.rect_filled(
            Rect::from_center_size(pos2(x, mid), vec2(2.5, h.max(2.0))),
            CornerRadius::same(1),
            c,
        );
    }
    let stroke_c = theme::lerp_color(
        theme::STROKE_HI,
        theme::accent_at((now * 0.15) as f32),
        active,
    );
    let march = if active > 0.0 {
        (now * 30.0) as f32
    } else {
        0.0
    };
    paint::dashed_rect(
        p,
        rect,
        22.0,
        Stroke::new(1.5 + active, stroke_c),
        10.0,
        8.0,
        march,
    );

    if !app.drop_hover {
        let c = pos2(rect.center().x, rect.top() + 78.0);
        let rr = 30.0 + 4.0 * pulse + 2.0 * hover;
        paint::glow_circle(
            p,
            c,
            rr,
            theme::alpha(theme::PINK, 0.25 + 0.3 * active),
            20.0,
        );
        let disc = Rect::from_center_size(c, vec2(2.0 * rr, 2.0 * rr));
        paint::gradient_rect(
            p,
            disc,
            CornerRadius::same(rr as u8),
            0.0,
            1.0,
            Color32::WHITE,
        );
        let lift = -3.0 * hover;
        icons::paint(
            p,
            Rect::from_center_size(c + vec2(0.0, lift), vec2(24.0, 24.0)),
            Icon::Download,
            Color32::WHITE,
        );

        p.text(
            pos2(rect.center().x, rect.top() + 140.0),
            Align2::CENTER_CENTER,
            "Drop an audio file here",
            theme::semi(19.0),
            theme::TEXT,
        );
        p.text(
            pos2(rect.center().x, rect.top() + 166.0),
            Align2::CENTER_CENTER,
            "or click to browse  ·  wav  mp3  flac  m4a  ogg",
            theme::regular(13.0),
            theme::TEXT_DIM,
        );
    }
    if resp.clicked()
        && app.shot.is_none()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Audio",
                &[
                    "wav", "mp3", "flac", "m4a", "aac", "ogg", "opus", "aiff", "aif", "mp4", "webm",
                ],
            )
            .pick_file()
    {
        app.start_job(InputSource::File(path), now);
    }
}

fn model_strip(app: &mut StemApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        overline(ui, "Model");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if Button::ghost("All models")
                .icon(Icon::Models)
                .height(28.0)
                .show(ui)
                .clicked()
            {
                app.page = Page::Models;
            }
        });
    });
    ui.add_space(4.0);
    let models: Vec<_> = app
        .backend
        .models
        .iter()
        .filter(|m| app.backend.is_available(m))
        .take(4)
        .cloned()
        .collect();
    if models.is_empty() {
        ui.label(egui::RichText::new("No models registered yet.").color(theme::TEXT_DIM));
        return;
    }
    let selected = app
        .backend
        .model(app.settings.model_id.as_deref())
        .map(|m| m.id);
    let n = models.len() as f32;
    let gap = 12.0;
    let cw = (ui.available_width() - gap * (n - 1.0)) / n;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for m in &models {
            let (r, resp) = ui.allocate_exact_size(vec2(cw, 92.0), Sense::click());
            let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
            if resp.clicked() {
                app.settings.model_id = Some(m.id.to_owned());
            }
            let sel = selected == Some(m.id);
            let h = ui
                .ctx()
                .animate_bool_with_time(resp.id, resp.hovered(), 0.15);
            let s = ui.ctx().animate_bool_with_time(resp.id.with("s"), sel, 0.2);
            let p = ui.painter();
            let r = r.translate(vec2(0.0, -2.0 * h));
            if s > 0.0 {
                paint::glow(
                    p,
                    r,
                    CornerRadius::same(14),
                    theme::alpha(theme::VIOLET, 0.3 * s),
                    16.0,
                );
                paint::gradient_rect(
                    p,
                    r,
                    CornerRadius::same(14),
                    0.0,
                    1.0,
                    Color32::from_white_alpha((255.0 * s) as u8),
                );
            }
            p.rect(
                r.shrink(1.5 * s),
                CornerRadius::same(13),
                theme::lerp_color(theme::CARD, theme::CARD_HI, h * 0.6),
                Stroke::new(1.0, theme::alpha(theme::STROKE_HI, 1.0 - s)),
                StrokeKind::Inside,
            );
            p.text(
                pos2(r.left() + 16.0, r.top() + 22.0),
                Align2::LEFT_CENTER,
                m.name,
                theme::semi(14.0),
                theme::TEXT,
            );
            let stems = format!("{} stems", m.stems.len());
            p.text(
                pos2(r.right() - 14.0, r.top() + 22.0),
                Align2::RIGHT_CENTER,
                stems,
                theme::mono(11.0),
                theme::TEXT_FAINT,
            );
            // Stem colour dots + names.
            let mut x = r.left() + 16.0;
            for k in &m.stems {
                let label = k.label();
                let g = p.layout_no_wrap(label.to_owned(), theme::regular(11.5), theme::TEXT_DIM);
                if x + g.size().x + 14.0 > r.right() - 10.0 {
                    p.text(
                        pos2(x, r.top() + 50.0),
                        Align2::LEFT_CENTER,
                        "…",
                        theme::regular(11.5),
                        theme::TEXT_DIM,
                    );
                    break;
                }
                p.circle_filled(pos2(x + 3.0, r.top() + 50.0), 3.0, theme::stem_color(*k));
                p.galley(
                    pos2(x + 10.0, r.top() + 50.0 - g.size().y / 2.0),
                    g.clone(),
                    theme::TEXT_DIM,
                );
                x += g.size().x + 20.0;
            }
            let ready = app.backend.is_downloaded(m.id);
            let (txt, c) = if ready {
                ("Ready", theme::GOOD)
            } else {
                ("Downloads on first use", theme::TEXT_FAINT)
            };
            p.text(
                pos2(r.left() + 16.0, r.bottom() - 18.0),
                Align2::LEFT_CENTER,
                txt,
                theme::regular(11.5),
                c,
            );
            if sel {
                let cc = pos2(r.right() - 20.0, r.bottom() - 18.0);
                p.circle_filled(cc, 9.0, theme::VIOLET);
                icons::paint(
                    p,
                    Rect::from_center_size(cc, vec2(10.0, 10.0)),
                    Icon::Check,
                    Color32::WHITE,
                );
            }
        }
    });
}

fn failure_card(
    app: &mut StemApp,
    ui: &mut egui::Ui,
    title: &str,
    msg: &str,
    src: Option<InputSource>,
    now: f64,
) {
    egui::Frame::new()
        .fill(Color32::from_rgb(34, 16, 26))
        .stroke(Stroke::new(1.0, theme::alpha(theme::BAD, 0.5)))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
                ui.painter()
                    .circle_filled(r.center(), 17.0, theme::alpha(theme::BAD, 0.2));
                icons::paint(ui.painter(), r.shrink(10.0), Icon::Close, theme::BAD);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(format!("Couldn't split “{title}”"))
                            .font(theme::semi(15.0))
                            .color(theme::TEXT),
                    );
                    ui.add(
                        egui::Label::new(egui::RichText::new(msg).color(theme::TEXT_DIM)).wrap(),
                    );
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if let Some(src) = src
                    && Button::new("Try again")
                        .icon(Icon::Refresh)
                        .height(32.0)
                        .show(ui)
                        .clicked()
                {
                    app.start_job(src, now);
                }
                if Button::new("Settings")
                    .icon(Icon::Settings)
                    .height(32.0)
                    .show(ui)
                    .clicked()
                {
                    app.page = Page::Settings;
                }
                if Button::ghost("Dismiss")
                    .kind(Kind::Ghost)
                    .height(32.0)
                    .show(ui)
                    .clicked()
                {
                    app.failure = None;
                }
            });
        });
}

fn recent(app: &mut StemApp, ui: &mut egui::Ui, now: f64) {
    if app.settings.history.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        overline(ui, "Recent");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if Button::ghost("Library")
                .icon(Icon::Library)
                .height(28.0)
                .show(ui)
                .clicked()
            {
                app.page = Page::Library;
            }
        });
    });
    ui.add_space(4.0);
    let items: Vec<_> = app.settings.history.iter().take(3).cloned().collect();
    let gap = 12.0;
    let cw = (ui.available_width() - gap * 2.0) / 3.0;
    let mut open = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for e in &items {
            let (r, resp) = ui.allocate_exact_size(vec2(cw, 64.0), Sense::click());
            let resp = resp
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text("Re-open session");
            let h = ui
                .ctx()
                .animate_bool_with_time(resp.id, resp.hovered(), 0.15);
            let p = ui.painter();
            p.rect(
                r,
                CornerRadius::same(14),
                theme::lerp_color(theme::CARD, theme::CARD_HI, h),
                Stroke::new(1.0, theme::lerp_color(theme::STROKE, theme::STROKE_HI, h)),
                StrokeKind::Inside,
            );
            let art = Rect::from_min_size(r.min + vec2(10.0, 10.0), vec2(44.0, 44.0));
            let seed = e
                .title
                .bytes()
                .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
            let u = (seed % 100) as f32 / 100.0;
            paint::gradient_rect(p, art, CornerRadius::same(10), u, u + 0.6, Color32::WHITE);
            icons::paint(
                p,
                art.shrink(13.0),
                Icon::Play,
                Color32::from_white_alpha(220),
            );
            p.text(
                pos2(art.right() + 12.0, r.top() + 23.0),
                Align2::LEFT_CENTER,
                crate::widgets::toast::elide(&e.title, 28),
                theme::semi(13.5),
                theme::TEXT,
            );
            let mut x = art.right() + 12.0;
            for k in &e.stems {
                p.circle_filled(pos2(x + 3.0, r.top() + 44.0), 3.0, theme::stem_color(*k));
                x += 10.0;
            }
            p.text(
                pos2(x + 4.0, r.top() + 44.0),
                Align2::LEFT_CENTER,
                crate::settings::relative_time(e.when),
                theme::regular(11.5),
                theme::TEXT_FAINT,
            );
            if resp.clicked() {
                open = Some(e.clone());
            }
        }
    });
    if let Some(e) = open {
        app.reopen(&e, now);
    }
}
