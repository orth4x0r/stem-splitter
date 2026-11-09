//! Start screen: paste a link or drop a file, pick a model.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Rect, Sense, Stroke, StrokeKind, pos2,
    vec2,
};

use crate::app::{Page, StemApp};
use crate::pipeline::InputSource;
use crate::theme::{self, motion};
use crate::widgets::buttons::{self, Button, Kind};
use crate::widgets::icons::{self, Icon};
use crate::widgets::{overline, paint};

pub fn page(app: &mut StemApp, ui: &mut egui::Ui) {
    let now = ui.input(|i| i.time);
    let max_w = ui.available_width().min(980.0);
    // Shorter windows get a tighter rhythm so the recent row stays on screen.
    let compact = ui.ctx().content_rect().height() < 960.0;
    ui.vertical_centered(|ui| {
        ui.set_max_width(max_w);
        ui.add_space(if compact { 4.0 } else { 18.0 });
        hero(ui, compact);
        ui.add_space(if compact { 18.0 } else { 26.0 });
        if let Some(f) = &app.failure {
            let (title, msg, src) = (f.title.clone(), f.message.clone(), f.source.clone());
            failure_card(app, ui, &title, &msg, src, now);
            ui.add_space(16.0);
        }
        url_bar(app, ui, now);
        ui.add_space(10.0);
        platforms(ui);
        ui.add_space(10.0);
        section_toggle(app, ui);
        ui.add_space(if compact { 16.0 } else { 22.0 });
        super::queue::panel(app, ui);
        drop_zone(app, ui, now, if compact { 186.0 } else { 220.0 });
        ui.add_space(if compact { 20.0 } else { 28.0 });
        model_strip(app, ui);
        ui.add_space(if compact { 20.0 } else { 28.0 });
        recent(app, ui, now);
    });
}

fn hero(ui: &mut egui::Ui, compact: bool) {
    let size = if compact {
        theme::size::DISPLAY - 4.0
    } else {
        theme::size::DISPLAY
    };
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), size * 1.25), Sense::hover());
    let p = ui.painter();
    let a = "Pull any track ";
    let b = "apart.";
    let ga = p.layout_no_wrap(a.into(), theme::bold(size), theme::TEXT);
    let gb = p.layout_no_wrap(b.into(), theme::bold(size), theme::TEXT);
    let total = ga.size().x + gb.size().x;
    let x = r.center().x - total / 2.0;
    let y = r.center().y;
    // A neon underglow behind the gradient word.
    let word = Rect::from_min_size(
        pos2(x + ga.size().x, y - gb.size().y * 0.25),
        vec2(gb.size().x, gb.size().y * 0.5),
    );
    paint::soft_disc(
        p,
        word.center(),
        gb.size().x * 0.9,
        theme::alpha(theme::PINK, 0.16),
    );
    p.galley(pos2(x, y - ga.size().y / 2.0), ga, theme::TEXT);
    let t = ui.input(|i| i.time);
    let u = if motion::reduced() {
        0.0
    } else {
        (t * 0.05) as f32
    };
    paint::gradient_text(
        p,
        pos2(x + total - gb.size().x, y),
        Align2::LEFT_CENTER,
        b,
        theme::bold(size),
        (u, u + 0.9),
        1.0,
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
    let (rect, _) = ui.allocate_exact_size(vec2(w, 62.0), Sense::hover());
    let id = Id::unique("url-input");
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    let f = ui
        .ctx()
        .animate_bool_with_time(id.with("f"), focused, motion::SLOW);
    let has_text = !app.url_input.trim().is_empty();
    let p = ui.painter();
    let rr = CornerRadius::same(19);
    paint::glow(
        p,
        rect.translate(vec2(0.0, 10.0)),
        rr,
        Color32::from_black_alpha(120),
        30.0,
    );
    paint::glow(
        p,
        rect,
        rr,
        theme::alpha(theme::VIOLET, 0.10 + 0.28 * f),
        18.0 + 14.0 * f,
    );
    if f > 0.0 {
        // Gradient border when focused: gradient rect under a slightly smaller fill.
        let u = (now * 0.08) as f32;
        paint::gradient_rect(
            p,
            rect,
            rr,
            u,
            u + 1.0,
            Color32::from_white_alpha((255.0 * f) as u8),
        );
        p.rect_filled(rect.shrink(1.5), CornerRadius::same(18), theme::CARD);
    } else {
        p.rect(
            rect,
            rr,
            theme::GLASS,
            Stroke::new(1.0, theme::STROKE_HI),
            StrokeKind::Inside,
        );
    }
    paint::glass_sheen(p, rect, rr, 0.4);
    let ic = pos2(rect.left() + 30.0, rect.center().y);
    if f > 0.0 {
        paint::glow_circle(p, ic, 5.0, theme::alpha(theme::PINK, 0.4 * f), 14.0);
    }
    icons::paint(
        p,
        Rect::from_center_size(ic, vec2(18.0, 18.0)),
        Icon::Link,
        theme::lerp_color(theme::TEXT_FAINT, theme::PINK, f),
    );

    let btn_w = 136.0;
    let edit_rect = Rect::from_min_max(
        pos2(rect.left() + 52.0, rect.center().y - 11.0),
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
                egui::RichText::new(
                    "Paste YouTube, X, SoundCloud or audio links (several queue up)…",
                )
                .color(theme::TEXT_FAINT),
            )
            .font(theme::regular(16.0))
            .text_color(theme::TEXT)
            .desired_width(edit_rect.width()),
    );
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

    let btn_rect = Rect::from_min_size(
        pos2(rect.right() - btn_w - 10.0, rect.top() + 10.0),
        vec2(btn_w, 42.0),
    );
    // The button lights up once there's something to split (eases in, then holds still:
    // nothing animates while idle).
    let armed = ui
        .ctx()
        .animate_bool_with_time(id.with("armed"), has_text, motion::SLOW);
    if armed > 0.0 {
        paint::glow(
            ui.painter(),
            btn_rect,
            CornerRadius::same(13),
            theme::alpha(theme::PINK, 0.32 * armed),
            14.0 + 12.0 * armed,
        );
    }
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(btn_rect));
    let go = Button::primary("Split it")
        .icon(Icon::Sparkle)
        .min_width(btn_w)
        .height(42.0)
        .tooltip("Split (Enter)")
        .show(&mut bui)
        .clicked();
    if go || enter {
        app.start_from_input(now);
    }
}

/// "Pick a section first": after the download, show the waveform to trim before splitting.
fn section_toggle(app: &mut StemApp, ui: &mut egui::Ui) {
    let on = app.settings.pick_section;
    let label = "Pick a section before splitting";
    let hint = if on {
        "  ·  trim to the part you need, it splits faster"
    } else {
        ""
    };
    let g = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::semi(13.0), theme::TEXT);
    let gh = ui
        .painter()
        .layout_no_wrap(hint.to_owned(), theme::regular(12.5), theme::TEXT_FAINT);
    let total = 26.0 + 44.0 + 10.0 + g.size().x + gh.size().x;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
        let (ir, _) = ui.allocate_exact_size(vec2(26.0, 24.0), Sense::hover());
        let lit = ui
            .ctx()
            .animate_bool_with_time(Id::unique("pick-section-lit"), on, motion::BASE);
        icons::paint(
            ui.painter(),
            Rect::from_center_size(ir.center() - vec2(4.0, 0.0), vec2(15.0, 15.0)),
            Icon::Scissors,
            theme::lerp_color(theme::TEXT_FAINT, theme::CYAN, lit),
        );
        let mut v = on;
        buttons::toggle_labeled(ui, &mut v, label).on_hover_text(
            "After the download, pick the part of the track to split (snaps to bars)",
        );
        app.settings.pick_section = v;
        ui.add_space(10.0);
        let (lr, resp) = ui.allocate_exact_size(g.size() + vec2(0.0, 6.0), Sense::click());
        if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            app.settings.pick_section = !v;
        }
        ui.painter().galley(
            pos2(lr.left(), lr.center().y - g.size().y / 2.0),
            g,
            theme::lerp_color(theme::TEXT_DIM, theme::TEXT, lit),
        );
        if !hint.is_empty() {
            let (hr, _) = ui.allocate_exact_size(gh.size(), Sense::hover());
            ui.painter().galley(hr.min, gh, theme::TEXT_FAINT);
        }
    });
}

fn platforms(ui: &mut egui::Ui) {
    let items = [
        ("YouTube", Color32::from_rgb(255, 72, 72)),
        ("X / Twitter", Color32::from_rgb(200, 200, 220)),
        ("SoundCloud", Color32::from_rgb(255, 136, 40)),
        ("Bandcamp", Color32::from_rgb(98, 154, 169)),
        ("Direct links", theme::CYAN),
    ];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let galleys: Vec<_> = items
            .iter()
            .map(|(n, _)| {
                ui.painter()
                    .layout_no_wrap((*n).to_owned(), theme::regular(12.5), theme::TEXT_DIM)
            })
            .collect();
        let total: f32 = galleys.iter().map(|g| g.size().x + 30.0).sum::<f32>()
            + 8.0 * (items.len() as f32 - 1.0);
        ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
        for ((_, color), g) in items.iter().zip(galleys) {
            let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 30.0, 24.0), Sense::hover());
            let h = ui
                .ctx()
                .animate_bool_with_time(resp.id, resp.hovered(), motion::FAST);
            let p = ui.painter();
            p.rect(
                r,
                CornerRadius::same(12),
                theme::lerp_color(Color32::from_white_alpha(5), theme::alpha(*color, 0.12), h),
                Stroke::new(
                    1.0,
                    theme::lerp_color(Color32::from_white_alpha(14), theme::alpha(*color, 0.5), h),
                ),
                StrokeKind::Inside,
            );
            let dc = pos2(r.left() + 12.0, r.center().y);
            paint::glow_circle(p, dc, 2.0, theme::alpha(*color, 0.5), 6.0);
            p.circle_filled(dc, 3.0, *color);
            p.galley(
                pos2(r.left() + 21.0, r.center().y - g.size().y / 2.0),
                g,
                theme::lerp_color(theme::TEXT_DIM, theme::TEXT, h),
            );
        }
    });
}

fn drop_zone(app: &mut StemApp, ui: &mut egui::Ui, now: f64, height: f32) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    let resp = resp
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Browse for an audio file");
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("h"), resp.hovered(), motion::SLOW);
    let drag = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("d"), app.drop_hover, motion::BASE);
    let active = hover.max(drag);
    let p = ui.painter();
    let rr = CornerRadius::same(22);
    let pulse = if app.drop_hover {
        paint::pulse(now, 1.0)
    } else {
        0.0
    };

    paint::glass(p, rect, rr, None, 0.3 * hover);
    paint::glow(
        p,
        rect,
        rr,
        theme::alpha(
            theme::VIOLET,
            0.02 + 0.08 * hover + 0.16 * drag + 0.12 * pulse,
        ),
        30.0,
    );
    if let Some(ptr) = resp.hover_pos() {
        paint::spotlight(p, rect, ptr, theme::VIOLET, hover);
    }
    // Equaliser watermark across the zone; bars near the pointer swell (follows the mouse, so
    // it costs nothing while the pointer rests).
    let mid = rect.center().y + 18.0;
    let ptr_x = resp.hover_pos().map(|p| p.x);
    let n = 96;
    for i in 0..n {
        let f = i as f32 / n as f32;
        let x = rect.left() + 30.0 + f * (rect.width() - 60.0);
        let base = ((f * 37.0).sin() * 0.5 + 0.5)
            * ((f * 7.0 + 1.0).sin() * 0.5 + 0.6)
            * (1.0 - (f - 0.5).abs() * 1.5).max(0.1);
        let near = ptr_x.map_or(0.0, |px| {
            let d = (x - px) / 90.0;
            (-d * d).exp()
        });
        let wob = 0.75 + 0.7 * near * hover;
        let h = 46.0 * base * wob;
        let c = theme::alpha(theme::accent_at(f), 0.06 + 0.14 * active);
        p.rect_filled(
            Rect::from_center_size(pos2(x, mid), vec2(3.0, h.max(2.0))),
            CornerRadius::same(1),
            c,
        );
    }
    // Rim: a quiet hairline at rest, marching neon dashes on hover / drag.
    if active > 0.01 {
        let stroke_c = theme::alpha(theme::accent_at((now * 0.15) as f32), active);
        let march = if motion::reduced() {
            0.0
        } else {
            (now * 30.0) as f32
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
    } else {
        paint::dashed_rect(
            p,
            rect,
            22.0,
            Stroke::new(1.2, theme::alpha(theme::STROKE_HI, 0.9)),
            6.0,
            7.0,
            0.0,
        );
    }

    if !app.drop_hover {
        let c = pos2(rect.center().x, rect.top() + height * 0.33);
        let lift = motion::bouncy(
            ui.ctx(),
            resp.id.with("lift"),
            if resp.hovered() { -4.0 } else { 0.0 },
        );
        let rr = 30.0 + 2.0 * hover;
        let c = c + vec2(0.0, lift);
        paint::glow_circle(
            p,
            c,
            rr,
            theme::alpha(theme::PINK, 0.25 + 0.3 * active),
            20.0 + 10.0 * hover,
        );
        let disc = Rect::from_center_size(c, vec2(2.0 * rr, 2.0 * rr));
        let u = (now * 0.1) as f32 * hover;
        paint::gradient_rect(
            p,
            disc,
            CornerRadius::same(rr as u8),
            u,
            u + 1.0,
            Color32::WHITE,
        );
        paint::glass_sheen(p, disc, CornerRadius::same(rr as u8), 1.0);
        // Orbiting ring on hover.
        if hover > 0.0 {
            let a0 = (now * 2.0) as f32;
            paint::gradient_arc(p, c, rr + 9.0, a0, a0 + 2.2 * hover, 2.0, a0 * 0.1, hover);
        }
        icons::paint(
            p,
            Rect::from_center_size(c, vec2(24.0, 24.0)),
            Icon::Download,
            Color32::WHITE,
        );

        p.text(
            pos2(rect.center().x, rect.top() + height * 0.33 + 62.0),
            Align2::CENTER_CENTER,
            "Drop an audio file here",
            theme::semi(19.0),
            theme::TEXT,
        );
        p.text(
            pos2(rect.center().x, rect.top() + height * 0.33 + 88.0),
            Align2::CENTER_CENTER,
            "or click to browse  ·  wav  mp3  flac  m4a  ogg",
            theme::regular(13.0),
            theme::TEXT_DIM,
        );
    }
    paint::ripple(p, &resp, rect, theme::VIOLET);
    paint::focus(p, &resp, rect, rr);
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

fn section_header(ui: &mut egui::Ui, title: &str, link: &str, icon: Icon) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        overline(ui, title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            clicked = Button::ghost(link)
                .icon(icon)
                .height(28.0)
                .show(ui)
                .clicked();
        });
    });
    clicked
}

fn model_strip(app: &mut StemApp, ui: &mut egui::Ui) {
    if section_header(ui, "Model", "All models", Icon::Models) {
        app.page = Page::Models;
    }
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
    let ctx = ui.ctx().clone();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (idx, m) in models.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(cw, 94.0), Sense::click());
            let resp = resp
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(m.description);
            if resp.clicked() {
                app.settings.model_id = Some(m.id.to_owned());
            }
            let sel = selected == Some(m.id);
            let enter = motion::ease_out_back(motion::appear(
                &ctx,
                resp.id.with("in"),
                0.08 + idx as f32 * motion::STAGGER,
                motion::ENTER,
            ));
            let h = ctx.animate_bool_with_time(resp.id, resp.hovered(), motion::BASE);
            let s = ctx.animate_bool_with_time(resp.id.with("s"), sel, motion::SLOW);
            let lift = motion::bouncy(
                &ctx,
                resp.id.with("lift"),
                if resp.hovered() { -3.0 } else { 0.0 },
            );
            let p = ui.painter();
            let r = r.translate(vec2(0.0, lift + 18.0 * (1.0 - enter)));
            let rr = CornerRadius::same(14);
            if s > 0.0 {
                paint::glow(p, r, rr, theme::alpha(theme::VIOLET, 0.35 * s), 18.0);
                let u = (ui.input(|i| i.time) * 0.05) as f32 * s;
                paint::gradient_rect(
                    p,
                    r,
                    rr,
                    u,
                    u + 1.0,
                    Color32::from_white_alpha((255.0 * s) as u8),
                );
                p.rect_filled(r.shrink(1.5), CornerRadius::same(13), theme::CARD);
                // Stem-colour wash from the selected card's top.
                paint::glass_sheen(p, r.shrink(1.5), CornerRadius::same(13), 1.0);
            } else {
                paint::glass(p, r, rr, None, h);
            }
            if let Some(ptr) = resp.hover_pos() {
                paint::spotlight(p, r, ptr, theme::PINK, h);
            }
            let sweep = ctx.animate_bool_with_time(resp.id.with("sweep"), resp.hovered(), 0.6);
            if resp.hovered() {
                paint::shine(p, r, sweep, 1.0);
            }
            let name = p.layout_no_wrap(m.name.to_owned(), theme::semi(14.0), theme::TEXT);
            let stems = p.layout_no_wrap(
                format!("{} stems", m.stems.len()),
                theme::mono(11.0),
                theme::TEXT_FAINT,
            );
            // Narrow cards (small windows) drop the stem count rather than collide.
            if name.size().x + stems.size().x + 44.0 <= r.width() {
                p.galley(
                    pos2(
                        r.right() - 14.0 - stems.size().x,
                        r.top() + 22.0 - stems.size().y / 2.0,
                    ),
                    stems,
                    theme::TEXT_FAINT,
                );
            }
            let name_h = name.size().y;
            p.with_clip_rect(r.shrink(8.0).intersect(p.clip_rect()))
                .galley(
                    pos2(r.left() + 16.0, r.top() + 22.0 - name_h / 2.0),
                    name,
                    theme::TEXT,
                );
            // Stem colour dots + names; collapse to dots only when they don't fit.
            let labels: f32 = m
                .stems
                .iter()
                .map(|k| {
                    p.layout_no_wrap(k.label().to_owned(), theme::regular(11.5), theme::TEXT_DIM)
                        .size()
                        .x
                        + 18.0
                })
                .sum();
            let mut x = r.left() + 16.0;
            if labels <= r.width() - 26.0 {
                for k in &m.stems {
                    let g = p.layout_no_wrap(
                        k.label().to_owned(),
                        theme::regular(11.5),
                        theme::TEXT_DIM,
                    );
                    p.circle_filled(pos2(x + 3.0, r.top() + 50.0), 3.0, theme::stem_color(*k));
                    let gw = g.size().x;
                    p.galley(
                        pos2(x + 10.0, r.top() + 50.0 - g.size().y / 2.0),
                        g,
                        theme::TEXT_DIM,
                    );
                    x += gw + 18.0;
                }
            } else {
                for k in &m.stems {
                    let c = theme::stem_color(*k);
                    paint::glow_circle(
                        p,
                        pos2(x + 4.0, r.top() + 50.0),
                        2.0,
                        theme::alpha(c, 0.5),
                        6.0,
                    );
                    p.circle_filled(pos2(x + 4.0, r.top() + 50.0), 4.0, c);
                    x += 13.0;
                }
                // Names in whatever room is left, cut with an ellipsis.
                let names = m
                    .stems
                    .iter()
                    .map(|k| k.label())
                    .collect::<Vec<_>>()
                    .join(" · ");
                let mut job = egui::text::LayoutJob::simple_singleline(
                    names,
                    theme::regular(11.0),
                    theme::TEXT_FAINT,
                );
                job.wrap.max_width = (r.right() - 12.0 - (x + 4.0)).max(10.0);
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                let g = p.fonts_mut(|f| f.layout_job(job));
                p.galley(
                    pos2(x + 4.0, r.top() + 50.0 - g.size().y / 2.0),
                    g,
                    theme::TEXT_FAINT,
                );
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
            if s > 0.0 {
                let cc = pos2(r.right() - 20.0, r.bottom() - 18.0);
                let pop = motion::ease_out_back(s);
                paint::glow_circle(p, cc, 6.0, theme::alpha(theme::VIOLET, 0.6 * s), 10.0);
                p.circle_filled(cc, 9.0 * pop, theme::VIOLET);
                icons::paint(
                    p,
                    Rect::from_center_size(cc, vec2(10.0, 10.0) * pop),
                    Icon::Check,
                    Color32::WHITE,
                );
            }
            paint::focus(p, &resp, r, rr);
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
    // Entrance: drop in, then a short horizontal shake ("no").
    let a = motion::appear(ui.ctx(), Id::unique(("failure", title)), 0.0, 0.7);
    let shake = if a < 1.0 {
        (a * std::f32::consts::PI * 7.0).sin() * 8.0 * (1.0 - a).powi(2)
    } else {
        0.0
    };
    let frame = egui::Frame::new()
        .fill(Color32::from_rgba_premultiplied(36, 14, 24, 235))
        .stroke(Stroke::new(1.0, theme::alpha(theme::BAD, 0.55)))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(egui::Margin {
            left: (16.0 + shake).round() as i8,
            right: (16.0 - shake).round() as i8,
            top: 16,
            bottom: 16,
        })
        .shadow(egui::Shadow {
            offset: [0, 8],
            blur: 30,
            spread: 0,
            color: theme::alpha(theme::BAD, 0.16),
        });
    let resp = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
            let p = ui.painter();
            paint::glow_circle(p, r.center(), 12.0, theme::alpha(theme::BAD, 0.35), 16.0);
            p.circle_filled(r.center(), 18.0, theme::alpha(theme::BAD, 0.2));
            p.circle_stroke(
                r.center(),
                18.0,
                Stroke::new(1.0, theme::alpha(theme::BAD, 0.6)),
            );
            icons::paint(p, r.shrink(11.0), Icon::Close, theme::BAD);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(format!("Couldn't split “{title}”"))
                        .font(theme::semi(15.5))
                        .color(theme::TEXT),
                );
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(msg)
                            .font(theme::regular(13.5))
                            .color(theme::lerp_color(theme::TEXT_DIM, theme::TEXT, 0.3)),
                    )
                    .wrap(),
                );
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(46.0);
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
    let r = resp.response.rect;
    paint::glass_sheen(ui.painter(), r, CornerRadius::same(16), 0.2);
    // Red accent bar on the left edge.
    let bar = Rect::from_min_size(
        pos2(r.left() + 1.5, r.top() + 18.0),
        vec2(2.5, r.height() - 36.0),
    );
    paint::glow(
        ui.painter(),
        bar,
        CornerRadius::same(2),
        theme::alpha(theme::BAD, 0.7),
        8.0,
    );
    ui.painter()
        .rect_filled(bar, CornerRadius::same(2), theme::BAD);
}

fn recent(app: &mut StemApp, ui: &mut egui::Ui, now: f64) {
    if app.settings.history.is_empty() {
        return;
    }
    if section_header(ui, "Recent", "Library", Icon::Library) {
        app.page = Page::Library;
    }
    ui.add_space(4.0);
    let items: Vec<_> = app.settings.history.iter().take(3).cloned().collect();
    let gap = 12.0;
    let cw = (ui.available_width() - gap * 2.0) / 3.0;
    let mut open = None;
    let ctx = ui.ctx().clone();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (idx, e) in items.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(cw, 66.0), Sense::click());
            let resp = resp
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text("Re-open session");
            let h = ctx.animate_bool_with_time(resp.id, resp.hovered(), motion::BASE);
            let enter = motion::ease_out(motion::appear(
                &ctx,
                resp.id.with("in"),
                0.2 + idx as f32 * motion::STAGGER,
                motion::ENTER,
            ));
            let lift = motion::bouncy(
                &ctx,
                resp.id.with("lift"),
                if resp.hovered() { -2.0 } else { 0.0 },
            );
            let r = r.translate(vec2(0.0, lift + 14.0 * (1.0 - enter)));
            let p = ui.painter();
            paint::glass(p, r, CornerRadius::same(14), None, h);
            if let Some(ptr) = resp.hover_pos() {
                paint::spotlight(p, r, ptr, theme::CYAN, h);
            }
            let art = Rect::from_min_size(r.min + vec2(10.0, 10.0), vec2(46.0, 46.0));
            let seed = e
                .title
                .bytes()
                .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
            let u = (seed % 100) as f32 / 100.0;
            paint::glow(
                p,
                art,
                CornerRadius::same(10),
                theme::alpha(theme::accent_at(u), 0.25 + 0.3 * h),
                10.0,
            );
            paint::gradient_rect(p, art, CornerRadius::same(10), u, u + 0.6, Color32::WHITE);
            paint::glass_sheen(p, art, CornerRadius::same(10), 0.8);
            let play = 1.0 + 0.15 * h;
            icons::paint(
                p,
                Rect::from_center_size(art.center() + vec2(1.0, 0.0), vec2(18.0, 18.0) * play),
                Icon::Play,
                Color32::from_white_alpha(230),
            );
            p.text(
                pos2(art.right() + 12.0, r.top() + 24.0),
                Align2::LEFT_CENTER,
                crate::widgets::toast::elide(&e.title, 28),
                theme::semi(13.5),
                theme::TEXT,
            );
            let mut x = art.right() + 12.0;
            for k in &e.stems {
                p.circle_filled(pos2(x + 3.0, r.top() + 45.0), 3.0, theme::stem_color(*k));
                x += 10.0;
            }
            p.text(
                pos2(x + 4.0, r.top() + 45.0),
                Align2::LEFT_CENTER,
                crate::settings::relative_time(e.when),
                theme::regular(11.5),
                theme::TEXT_FAINT,
            );
            paint::ripple(p, &resp, r, theme::CYAN);
            paint::focus(p, &resp, r, CornerRadius::same(14));
            if resp.clicked() {
                open = Some(e.clone());
            }
        }
    });
    if let Some(e) = open {
        app.reopen(&e, now);
    }
}
