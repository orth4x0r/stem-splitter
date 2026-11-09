//! Model picker: one card per model with stems, size, license and download state.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Rect, Sense, Stroke, StrokeKind, pos2, vec2,
};

use crate::app::StemApp;
use crate::backend::human_bytes;
use crate::pipeline::refine::RefineOp;
use crate::theme::{self, motion};
use crate::widgets::buttons::{self, Button};
use crate::widgets::icons::{self, Icon};
use crate::widgets::{overline, page_title, paint, progress};

pub fn page(app: &mut StemApp, ui: &mut egui::Ui) {
    page_title(
        ui,
        "Models",
        "Pick the brain that does the splitting. Models download once and live on your machine.",
    );
    device_picker(app, ui);
    ui.add_space(22.0);
    super::presets::section(app, ui);
    ui.add_space(10.0);
    overline(ui, "Separation models");
    ui.add_space(6.0);

    let models = app.backend.models.clone();
    if models.is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 200.0), Sense::hover());
        crate::widgets::empty_state(
            ui,
            r,
            Icon::Models,
            "No models yet",
            "The model registry is empty — stem-models isn't wired up.",
        );
        return;
    }
    let selected = app
        .backend
        .model(app.settings.model_id.as_deref())
        .map(|m| m.id)
        .filter(|_| app.settings.active_preset().is_none());
    // Drum-part and clean-up models run on a stem, not the mix: they get their own group.
    let (extras, main): (Vec<_>, Vec<_>) =
        models.into_iter().partition(|m| RefineOp::of(m).is_some());
    cards(app, ui, &main, selected, false);
    if !extras.is_empty() {
        ui.add_space(14.0);
        overline(ui, "Drum parts & vocal clean-up  ·  personal use");
        // Legend: the badge these cards carry, and what it means.
        ui.horizontal(|ui| {
            let w = super::refine::nc_badge_width(ui.painter(), 10.5);
            let (r, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
            super::refine::nc_badge(ui.painter(), r.left(), r.center().y, 10.5);
            ui.label(
                egui::RichText::new(
                    "Non-commercial or no stated licence: fine for your own beats, check before \
                     releasing. Run them from a Drums or Vocals card (Split / Clean) or as a chain step.",
                )
                .font(theme::regular(12.5))
                .color(theme::TEXT_DIM),
            )
            .on_hover_text(super::refine::NC_TIP);
        });
        ui.add_space(8.0);
        cards(app, ui, &extras, None, true);
    }
}

/// One grid of model cards. `extras` = stem refiners: clicking doesn't make them the split
/// model, they're used from stem cards and chains.
fn cards(
    app: &mut StemApp,
    ui: &mut egui::Ui,
    models: &[stem_models::ModelInfo],
    selected: Option<&str>,
    extras: bool,
) {
    let t = ui.input(|i| i.time);
    let gap = 16.0;
    let cols = if ui.available_width() > 760.0 { 2 } else { 1 };
    let cw = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
    let ch = 214.0;
    for row in models.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for m in row {
                let (rect, resp) = ui.allocate_exact_size(vec2(cw, ch), Sense::click());
                let resp = resp
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text(if m.non_commercial {
                        format!("License: {}\n{}", m.license, super::refine::NC_TIP)
                    } else {
                        format!("License: {}", m.license)
                    });
                let sel = selected == Some(m.id);
                let h = ui
                    .ctx()
                    .animate_bool_with_time(resp.id, resp.hovered(), 0.15);
                let s = ui.ctx().animate_bool_with_time(resp.id.with("s"), sel, 0.2);
                let downloading = app.model_downloads.get(m.id).map(|d| d.status);
                let partial = if downloading.is_none() && !app.backend.is_downloaded(m.id) {
                    app.backend.manager().partial_bytes(m)
                } else {
                    0
                };
                let ready = app.backend.is_downloaded(m.id);
                let soon = app.backend.needs_manual_export(m);
                let idx = models.iter().position(|x| x.id == m.id).unwrap_or(0);
                let enter = motion::ease_out_back(motion::appear(
                    ui.ctx(),
                    resp.id.with("in"),
                    0.06 + idx as f32 * motion::STAGGER,
                    motion::ENTER,
                ));
                let lift = motion::bouncy(
                    ui.ctx(),
                    resp.id.with("lift"),
                    if resp.hovered() { -4.0 } else { 0.0 },
                );
                let rect = rect.translate(vec2(0.0, lift + 22.0 * (1.0 - enter)));
                {
                    let p = ui.painter();
                    let rr = CornerRadius::same(18);
                    if s > 0.0 {
                        paint::glow(
                            p,
                            rect.translate(vec2(0.0, 10.0)),
                            rr,
                            Color32::from_black_alpha(120),
                            30.0,
                        );
                        paint::glow(p, rect, rr, theme::alpha(theme::VIOLET, 0.4 * s), 24.0);
                        let u = (t * 0.05) as f32;
                        paint::gradient_rect(
                            p,
                            rect,
                            rr,
                            u,
                            u + 1.0,
                            Color32::from_white_alpha((255.0 * s) as u8),
                        );
                        p.rect_filled(
                            rect.shrink(1.5 * s),
                            CornerRadius::same(17),
                            theme::lerp_color(theme::CARD, Color32::from_rgb(24, 24, 40), h),
                        );
                        paint::glass_sheen(p, rect.shrink(1.5), CornerRadius::same(17), 0.6 + h);
                    } else {
                        paint::glass(p, rect, rr, None, h);
                    }
                    // Stem-coloured wash behind the card's top band.
                    if let Some(k) = m.stems.first() {
                        paint::soft_disc(
                            &p.with_clip_rect(rect.shrink(2.0).intersect(p.clip_rect())),
                            pos2(rect.left() + 60.0, rect.top()),
                            150.0,
                            theme::alpha(theme::stem_color(*k), 0.08 + 0.06 * h),
                        );
                    }
                    if let Some(ptr) = resp.hover_pos() {
                        paint::spotlight(p, rect, ptr, theme::VIOLET, h);
                    }
                    let sweep =
                        ui.ctx()
                            .animate_bool_with_time(resp.id.with("sweep"), resp.hovered(), 0.7);
                    if resp.hovered() {
                        paint::shine(p, rect, sweep, 1.0);
                    }
                    paint::focus(p, &resp, rect, rr);
                    let inner = rect.shrink(1.5 * s);

                    // Stem colour band across the top.
                    let band = Rect::from_min_size(
                        inner.min + vec2(20.0, 18.0),
                        vec2(inner.width() - 40.0, 4.0),
                    );
                    let n = m.stems.len().max(1) as f32;
                    for (i, k) in m.stems.iter().enumerate() {
                        let seg = Rect::from_min_size(
                            pos2(band.left() + band.width() * i as f32 / n, band.top()),
                            vec2(band.width() / n - 3.0, 4.0),
                        );
                        p.rect_filled(seg, CornerRadius::same(2), theme::stem_color(*k));
                    }

                    // The name stops short of the NC badge (long names would run under it).
                    let name_right = if m.non_commercial {
                        inner.right() - 28.0 - super::refine::nc_badge_width(p, 10.5)
                    } else {
                        inner.right() - 20.0
                    };
                    p.with_clip_rect(p.clip_rect().intersect(Rect::from_x_y_ranges(
                        inner.left()..=name_right,
                        inner.y_range(),
                    )))
                    .text(
                        pos2(inner.left() + 20.0, inner.top() + 46.0),
                        Align2::LEFT_CENTER,
                        m.name,
                        theme::bold(19.0),
                        theme::TEXT,
                    );
                    if sel {
                        let nw = p
                            .layout_no_wrap(m.name.to_owned(), theme::bold(19.0), theme::TEXT)
                            .size()
                            .x;
                        let r = Rect::from_min_size(
                            pos2(inner.left() + 32.0 + nw, inner.top() + 35.0),
                            vec2(84.0, 22.0),
                        );
                        paint::gradient_rect(
                            p,
                            r,
                            CornerRadius::same(11),
                            0.0,
                            1.0,
                            Color32::from_white_alpha(150),
                        );
                        icons::paint(
                            p,
                            Rect::from_center_size(
                                pos2(r.left() + 14.0, r.center().y),
                                vec2(11.0, 11.0),
                            ),
                            Icon::Check,
                            Color32::WHITE,
                        );
                        p.text(
                            pos2(r.left() + 25.0, r.center().y),
                            Align2::LEFT_CENTER,
                            "Selected",
                            theme::semi(11.5),
                            Color32::WHITE,
                        );
                    }
                    if m.non_commercial {
                        let w = super::refine::nc_badge_width(p, 10.5);
                        super::refine::nc_badge(
                            p,
                            inner.right() - 20.0 - w,
                            inner.top() + 46.0,
                            10.5,
                        );
                    }
                    // Description: up to two wrapped lines, cut with an ellipsis.
                    let mut job = egui::text::LayoutJob::simple(
                        m.description.to_owned(),
                        theme::regular(13.0),
                        theme::TEXT_DIM,
                        inner.width() - 40.0,
                    );
                    job.wrap.max_rows = 2;
                    let g = p.fonts_mut(|f| f.layout_job(job));
                    p.galley(inner.min + vec2(20.0, 64.0), g, theme::TEXT_DIM);
                }
                // Stem chips.
                let chips =
                    Rect::from_min_size(rect.min + vec2(20.0, 112.0), vec2(cw - 40.0, 28.0));
                let mut cu = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(chips)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                cu.spacing_mut().item_spacing.x = 6.0;
                for k in &m.stems {
                    buttons::chip(&mut cu, k.label(), theme::stem_color(*k), false);
                }
                // Footer: status + action.
                let foot = Rect::from_min_max(
                    pos2(rect.left() + 20.0, rect.bottom() - 58.0),
                    pos2(rect.right() - 20.0, rect.bottom() - 18.0),
                );
                let p = ui.painter().clone();
                if let Some(st) = downloading {
                    let frac = st.fraction();
                    let bar = Rect::from_min_size(
                        pos2(foot.left(), foot.center().y + 4.0),
                        vec2(foot.width() - 60.0, 6.0),
                    );
                    progress::bar(&p, bar, frac, t);
                    p.text(
                        pos2(foot.left(), foot.top() + 6.0),
                        Align2::LEFT_CENTER,
                        crate::views::models_dl::label(&st, m.size_bytes),
                        theme::mono(11.5),
                        theme::TEXT_DIM,
                    );
                    let cr = Rect::from_center_size(
                        pos2(foot.right() - 16.0, foot.center().y + 2.0),
                        vec2(30.0, 30.0),
                    );
                    let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(cr));
                    if buttons::icon_button(
                        &mut cu,
                        Icon::Close,
                        30.0,
                        false,
                        theme::BAD,
                        "Pause download (resumes where it left off)",
                    )
                    .clicked()
                    {
                        app.cancel_model_download(m.id);
                    }
                } else {
                    let (dot, status) = if ready {
                        (theme::GOOD, "Installed")
                    } else if soon && extras {
                        (theme::WARN, "Not hosted · export it with the tools/ script")
                    } else if soon {
                        (theme::WARN, "Coming soon · needs a manual ONNX export")
                    } else if partial > 0 {
                        (theme::WARN, "Paused")
                    } else {
                        (theme::TEXT_FAINT, "Not downloaded")
                    };
                    p.circle_filled(pos2(foot.left() + 4.0, foot.center().y - 8.0), 4.0, dot);
                    p.text(
                        pos2(foot.left() + 16.0, foot.center().y - 8.0),
                        Align2::LEFT_CENTER,
                        status,
                        theme::semi(12.5),
                        if ready { theme::TEXT } else { theme::TEXT_DIM },
                    );
                    // Size and a short license tag; the full license is in the tooltip.
                    let short_license = m.license.split([' ', '(']).next().unwrap_or(m.license);
                    let size = if partial > 0 {
                        format!("{} of {}", human_bytes(partial), human_bytes(m.size_bytes))
                    } else {
                        human_bytes(m.size_bytes)
                    };
                    let meta = format!(
                        "{}  ·  {}  ·  {:.1} kHz",
                        size,
                        short_license,
                        m.sample_rate as f32 / 1000.0
                    );
                    p.text(
                        pos2(foot.left(), foot.center().y + 12.0),
                        Align2::LEFT_CENTER,
                        meta,
                        theme::mono(11.5),
                        theme::TEXT_FAINT,
                    );
                    let br = Rect::from_min_max(
                        pos2(foot.right() - 150.0, foot.top() + 2.0),
                        pos2(foot.right(), foot.bottom() - 2.0),
                    );
                    let mut bu = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(br)
                            .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    );
                    if soon {
                        // Nothing to click: see the model's description / tools/export script.
                    } else if !ready {
                        if Button::new(if partial > 0 { "Resume" } else { "Download" })
                            .icon(Icon::Download)
                            .height(32.0)
                            .show(&mut bu)
                            .clicked()
                        {
                            app.download_model(m.id);
                        }
                    } else if extras {
                        // Picked from a stem card, not here.
                    } else if !sel
                        && Button::new("Use this")
                            .icon(Icon::Check)
                            .height(32.0)
                            .show(&mut bu)
                            .clicked()
                    {
                        app.pick_model(m.id);
                    }
                }
                if resp.clicked() && !soon && !extras {
                    app.pick_model(m.id);
                }
                if soon {
                    // Dim the whole card.
                    ui.painter().rect_filled(
                        rect,
                        CornerRadius::same(18),
                        Color32::from_black_alpha(90),
                    );
                }
            }
        });
        ui.add_space(gap);
    }
}

pub fn device_picker(app: &mut StemApp, ui: &mut egui::Ui) {
    overline(ui, "Run on");
    ui.add_space(6.0);
    let devices = app.backend.devices.clone();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
        for (dev, name) in devices {
            let sel = app.settings.device == dev;
            let gpu = matches!(dev, stem_models::Device::DirectMl(_));
            let galley = ui
                .painter()
                .layout_no_wrap(name.clone(), theme::semi(13.0), theme::TEXT);
            let (r, resp) =
                ui.allocate_exact_size(vec2(galley.size().x + 58.0, 44.0), Sense::click());
            let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
            let h = ui
                .ctx()
                .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
            let s = ui
                .ctx()
                .animate_bool_with_time(resp.id.with("s"), sel, 0.18);
            let p = ui.painter();
            let accent = if gpu { theme::GOOD } else { theme::CYAN };
            if s > 0.0 {
                paint::glow(
                    p,
                    r,
                    CornerRadius::same(12),
                    theme::alpha(accent, 0.25 * s),
                    12.0,
                );
            }
            p.rect(
                r,
                CornerRadius::same(12),
                theme::lerp_color(theme::CARD, theme::alpha(accent, 0.14), s.max(h * 0.3)),
                Stroke::new(1.0, theme::lerp_color(theme::STROKE_HI, accent, s)),
                StrokeKind::Inside,
            );
            let ir = Rect::from_center_size(pos2(r.left() + 22.0, r.center().y), vec2(17.0, 17.0));
            icons::paint(
                p,
                ir,
                if gpu {
                    Icon::Chip
                } else if dev == stem_models::Device::Auto {
                    Icon::Sparkle
                } else {
                    Icon::Settings
                },
                if sel { accent } else { theme::TEXT_DIM },
            );
            p.galley(
                pos2(r.left() + 40.0, r.center().y - galley.size().y / 2.0),
                galley,
                if sel { Color32::WHITE } else { theme::TEXT_DIM },
            );
            if resp.clicked() {
                app.settings.device = dev;
            }
        }
    });
}
