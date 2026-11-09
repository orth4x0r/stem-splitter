//! First-run welcome: a sheet shown once, on the first launch with an empty library, that says
//! what the app does in three steps and where it runs. "Get started" or closing it sets
//! `settings.welcomed`, so it never comes back.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, pos2, vec2,
};

use crate::app::{Page, StemApp};
use crate::theme::{self, motion};
use crate::widgets::buttons::Button;
use crate::widgets::icons::{self, Icon};
use crate::widgets::sheet::Sheet;

/// Show the welcome while it's due: first launch, nothing split yet, no job or session.
pub fn show(app: &mut StemApp, ctx: &egui::Context) {
    if app.settings.welcomed
        || !app.settings.history.is_empty()
        || app.job.is_some()
        || app.session.is_some()
        || !app.splash.done
    {
        return;
    }
    let device = app
        .backend
        .device_name(app.settings.device)
        .replace(" (DirectML)", " · DirectML");
    let mut anim = Sheet::begin(ctx, Id::unique("welcome-sheet"));
    let mut done = false;
    let mut models = false;
    let resp = anim.show(ctx, theme::VIOLET, 28, |ui| {
        ui.set_width(720.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
            crate::splash::logo_mark(ui.painter(), r, &[1.0; 5], 0.0, 1.0, false);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Welcome to Stem Splitter")
                        .font(theme::bold(24.0))
                        .color(theme::TEXT),
                );
                ui.label(
                    egui::RichText::new(
                        "Pull vocals, drums, bass and more out of any track, then chop them into samples.",
                    )
                    .font(theme::regular(13.5))
                    .color(theme::TEXT_DIM),
                );
            });
        });
        ui.add_space(20.0);
        let steps = [
            (
                Icon::Link,
                "Bring a track",
                "Paste a YouTube, X or SoundCloud link, or drop an audio file.",
                theme::stem_color(stem_models::StemKind::Vocals),
            ),
            (
                Icon::Models,
                "Pick a model",
                "Or a preset chain. Models download once and stay on this machine.",
                theme::stem_color(stem_models::StemKind::Drums),
            ),
            (
                Icon::Scissors,
                "Chop and drag",
                "Select a sample, play the chop pads, drag stems into your DAW.",
                theme::stem_color(stem_models::StemKind::Bass),
            ),
        ];
        let gap = 12.0;
        let w = (ui.available_width() - 2.0 * gap) / 3.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (k, (icon, title, body, color)) in steps.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(vec2(w, 132.0), Sense::hover());
                let e = motion::ease_out(motion::appear(
                    ui.ctx(),
                    Id::unique(("welcome-step", k)),
                    0.12 + k as f32 * 0.08,
                    motion::ENTER,
                ));
                let r = r.translate(vec2(0.0, 10.0 * (1.0 - e)));
                let p = ui.painter();
                let a = e.clamp(0.0, 1.0);
                p.rect(
                    r,
                    CornerRadius::same(14),
                    theme::alpha(Color32::from_white_alpha(5), a),
                    Stroke::new(1.0, theme::alpha(theme::GLASS_STROKE, a)),
                    StrokeKind::Inside,
                );
                // Step number and icon.
                let c = pos2(r.left() + 30.0, r.top() + 30.0);
                p.circle_filled(c, 15.0, theme::alpha(*color, 0.16 * a));
                icons::paint(
                    p,
                    Rect::from_center_size(c, vec2(15.0, 15.0)),
                    *icon,
                    theme::alpha(*color, a),
                );
                p.text(
                    pos2(r.right() - 16.0, r.top() + 30.0),
                    Align2::RIGHT_CENTER,
                    format!("{}", k + 1),
                    theme::mono(12.0),
                    theme::alpha(theme::TEXT_FAINT, a),
                );
                p.text(
                    pos2(r.left() + 16.0, r.top() + 64.0),
                    Align2::LEFT_CENTER,
                    *title,
                    theme::semi(14.5),
                    theme::alpha(theme::TEXT, a),
                );
                let mut job = egui::text::LayoutJob::simple(
                    (*body).to_owned(),
                    theme::regular(12.5),
                    theme::alpha(theme::TEXT_DIM, a),
                    w - 32.0,
                );
                job.wrap.max_rows = 3;
                let g = p.fonts_mut(|f| f.layout_job(job));
                p.galley(pos2(r.left() + 16.0, r.top() + 78.0), g, theme::TEXT_DIM);
            }
        });
        ui.add_space(16.0);
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint(ui.painter(), r, Icon::Chip, theme::GOOD);
            ui.label(
                egui::RichText::new(format!("Runs on {device}. Change it under Models."))
                    .font(theme::regular(12.5))
                    .color(theme::TEXT_DIM),
            );
        });
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint(ui.painter(), r, Icon::Sparkle, theme::TEXT_DIM);
            ui.label(
                egui::RichText::new("Press ? any time for keyboard shortcuts.")
                    .font(theme::regular(12.5))
                    .color(theme::TEXT_DIM),
            );
        });
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            if Button::primary("Get started")
                .icon(Icon::Sparkle)
                .height(40.0)
                .min_width(150.0)
                .show(ui)
                .clicked()
            {
                done = true;
            }
            if Button::new("Browse models")
                .icon(Icon::Models)
                .height(40.0)
                .show(ui)
                .clicked()
            {
                done = true;
                models = true;
            }
        });
    });
    if resp.should_close() {
        done = true;
    }
    if done {
        anim.close(ctx);
        if models {
            app.page = Page::Models;
        }
    }
    if anim.finished() {
        app.settings.welcomed = true;
    }
}
