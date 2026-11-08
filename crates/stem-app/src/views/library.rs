//! Library: past splits, re-open or reveal in Explorer.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2,
};

use crate::app::StemApp;
use crate::backend::mmss;
use crate::settings::relative_time;
use crate::theme;
use crate::widgets::buttons::Button;
use crate::widgets::icons::{self, Icon};
use crate::widgets::toast::elide;
use crate::widgets::{page_title, paint};

pub fn page(app: &mut StemApp, ui: &mut egui::Ui) {
    page_title(
        ui,
        "Library",
        "Everything you've split. Re-open a session to keep chopping.",
    );
    if app.settings.history.is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 260.0), Sense::hover());
        crate::widgets::empty_state(
            ui,
            r,
            Icon::Library,
            "Nothing here yet",
            "Split a track and it shows up here.",
        );
        return;
    }
    let now = ui.input(|i| i.time);
    let entries = app.settings.history.clone();
    let mut reopen = None;
    let mut reveal = None;
    let mut remove = None;
    for (i, e) in entries.iter().enumerate() {
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 84.0), Sense::hover());
        let h = ui
            .ctx()
            .animate_bool_with_time(resp.id, resp.hovered(), 0.15);
        {
            let p = ui.painter();
            p.rect(
                rect,
                CornerRadius::same(16),
                theme::lerp_color(theme::CARD, Color32::from_rgb(24, 24, 40), h),
                Stroke::new(1.0, theme::lerp_color(theme::STROKE, theme::STROKE_HI, h)),
                StrokeKind::Inside,
            );
            // Generated "cover art": gradient tile with a hash-picked offset and mini bars.
            let art = Rect::from_min_size(rect.min + vec2(12.0, 12.0), vec2(60.0, 60.0));
            let seed = e
                .title
                .bytes()
                .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
            let u = (seed % 100) as f32 / 100.0;
            paint::gradient_rect(p, art, CornerRadius::same(12), u, u + 0.6, Color32::WHITE);
            for (j, k) in e.stems.iter().enumerate().take(6) {
                let bh = 10.0 + ((seed >> (j * 3)) % 24) as f32;
                let x = art.left() + 10.0 + j as f32 * 7.5;
                p.rect_filled(
                    Rect::from_min_max(
                        pos2(x, art.bottom() - 10.0 - bh),
                        pos2(x + 4.0, art.bottom() - 10.0),
                    ),
                    CornerRadius::same(1),
                    theme::lerp_color(theme::stem_color(*k), Color32::WHITE, 0.5),
                );
            }
            let x = art.right() + 16.0;
            p.text(
                pos2(x, rect.top() + 26.0),
                Align2::LEFT_CENTER,
                elide(&e.title, 50),
                theme::semi(16.0),
                theme::TEXT,
            );
            let mut meta = format!(
                "{}  ·  {}  ·  {}",
                e.model_name,
                mmss(e.duration_secs),
                relative_time(e.when)
            );
            if let Some(m) = crate::analysis::history_tag(e.bpm, e.key.as_deref()) {
                meta = format!("{m}  ·  {meta}");
            }
            p.text(
                pos2(x, rect.top() + 48.0),
                Align2::LEFT_CENTER,
                meta,
                theme::regular(12.5),
                theme::TEXT_DIM,
            );
            let mut sx = x;
            for k in &e.stems {
                p.circle_filled(
                    pos2(sx + 4.0, rect.top() + 66.0),
                    3.5,
                    theme::stem_color(*k),
                );
                sx += 12.0;
            }
            p.text(
                pos2(sx + 6.0, rect.top() + 66.0),
                Align2::LEFT_CENTER,
                elide(&e.source, 60),
                theme::mono(11.0),
                theme::TEXT_FAINT,
            );
        }
        let br = Rect::from_min_max(
            pos2(rect.right() - 340.0, rect.top()),
            pos2(rect.right() - 16.0, rect.bottom()),
        );
        let mut bu = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(br)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        if Button::ghost("")
            .icon(Icon::Close)
            .height(34.0)
            .tooltip("Remove from library (keeps files)")
            .show(&mut bu)
            .clicked()
        {
            remove = Some(i);
        }
        if Button::new("Folder")
            .icon(Icon::Folder)
            .height(34.0)
            .show(&mut bu)
            .clicked()
        {
            reveal = Some(e.out_dir.clone());
        }
        if Button::new("Open")
            .icon(Icon::Play)
            .height(34.0)
            .enabled(app.job.is_none())
            .show(&mut bu)
            .clicked()
        {
            reopen = Some(e.clone());
        }
        ui.add_space(8.0);
    }
    if let Some(e) = reopen {
        app.reopen(&e, now);
    }
    if let Some(d) = reveal {
        app.open_folder(&d);
    }
    if let Some(i) = remove {
        app.settings.history.remove(i);
    }
    let _ = icons::paint;
}
