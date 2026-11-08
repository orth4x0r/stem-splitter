//! The "working on it" screen: spinning vinyl, progress ring, stage stepper, live bars.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2,
};

use crate::app::StemApp;
use crate::backend::mmss;
use crate::pipeline::Stage;
use crate::theme;
use crate::widgets::buttons::{Button, Kind};
use crate::widgets::icons::{self, Icon};
use crate::widgets::toast::elide;
use crate::widgets::{paint, progress};

pub fn page(app: &mut StemApp, ui: &mut egui::Ui) {
    let device = app
        .backend
        .device_name(app.settings.device)
        .replace(" (DirectML)", " · DirectML");
    let Some(job) = &app.job else { return };
    let t = ui.input(|i| i.time);
    let elapsed = (t - job.started).max(0.0);
    let overall = job.shown.clamp(0.0, 1.0);

    let w = ui.available_width().min(1000.0);
    let mut cancel = false;
    ui.vertical_centered(|ui| {
        ui.set_max_width(w);
        let free = ui.ctx().content_rect().height() - 64.0 - 52.0 - 540.0;
        // With a batch queue below, start at the top so the queue stays in view.
        let free = if app.queue.is_empty() { free } else { 0.0 };
        ui.add_space((free / 2.0).clamp(8.0, 200.0));
        let (rect, _) = ui.allocate_exact_size(vec2(w, 420.0), Sense::hover());
        let painter = ui.painter().clone();
        let p = &painter;
        p.rect(
            rect,
            CornerRadius::same(24),
            theme::CARD,
            Stroke::new(1.0, theme::STROKE),
            StrokeKind::Inside,
        );
        paint::glow(
            p,
            rect.shrink(40.0),
            CornerRadius::same(24),
            theme::alpha(theme::VIOLET, 0.06),
            80.0,
        );

        // ---- left: vinyl + ring
        let c = pos2(rect.left() + 200.0, rect.center().y);
        let spin = (t * 1.8) as f32; // ~33 rpm, near enough
        progress::vinyl(p, c, 118.0, spin, t);
        let ring_frac = if job.frac.is_none() && job.stage != Stage::Separating {
            None
        } else {
            Some(overall)
        };
        progress::ring(p, c, 146.0, 5.0, ring_frac, t);
        // Tone arm.
        let pivot = c + vec2(130.0, -130.0);
        let arm_end = c + vec2(58.0, -36.0);
        p.line_segment(
            [pivot, arm_end],
            Stroke::new(4.0, Color32::from_rgb(70, 70, 92)),
        );
        p.circle_filled(pivot, 9.0, Color32::from_rgb(70, 70, 92));
        p.circle_filled(pivot, 4.0, theme::CARD);
        p.rect_filled(
            Rect::from_center_size(arm_end, vec2(14.0, 10.0)),
            CornerRadius::same(2),
            Color32::from_rgb(110, 110, 140),
        );

        // ---- right: text column
        let x = rect.left() + 400.0;
        let mut y = rect.top() + 48.0;
        let stage_label = if job.reopening {
            "LOADING SESSION"
        } else {
            "SPLITTING"
        };
        let pulse = paint::pulse(t, 1.2);
        p.circle_filled(
            pos2(x + 4.0, y),
            4.0,
            theme::alpha(theme::PINK, 0.5 + 0.5 * pulse),
        );
        p.text(
            pos2(x + 16.0, y),
            Align2::LEFT_CENTER,
            stage_label,
            theme::semi(11.5),
            theme::PINK,
        );
        y += 28.0;
        p.text(
            pos2(x, y),
            Align2::LEFT_CENTER,
            elide(&job.title, 40),
            theme::bold(26.0),
            theme::TEXT,
        );
        y += 26.0;
        p.text(
            pos2(x, y),
            Align2::LEFT_CENTER,
            elide(&job.source.display(), 64),
            theme::mono(12.0),
            theme::TEXT_FAINT,
        );
        y += 46.0;

        let pct = format!("{:>2.0}%", overall * 100.0);
        let g = p.layout_no_wrap(pct, theme::bold(58.0), Color32::WHITE);
        let gsz = g.size();
        p.galley(pos2(x - 2.0, y - gsz.y / 2.0), g, Color32::WHITE);
        let sx = x + gsz.x + 16.0;
        p.text(
            pos2(sx, y - 12.0),
            Align2::LEFT_CENTER,
            job.stage.label(),
            theme::semi(16.0),
            theme::TEXT,
        );
        p.text(
            pos2(sx, y + 12.0),
            Align2::LEFT_CENTER,
            elide(&job.detail, 38),
            theme::regular(13.0),
            theme::TEXT_DIM,
        );
        y += 44.0;
        let bar = Rect::from_min_size(pos2(x, y), vec2(rect.right() - x - 48.0, 8.0));
        progress::bar(p, bar, Some(overall), t);
        y += 22.0;
        let eta = if overall > 0.05 {
            let total = elapsed / overall as f64;
            format!(
                "{} elapsed  ·  ~{} left",
                mmss(elapsed),
                mmss(total - elapsed)
            )
        } else {
            format!("{} elapsed", mmss(elapsed))
        };
        p.text(
            pos2(x, y),
            Align2::LEFT_CENTER,
            eta,
            theme::mono(12.0),
            theme::TEXT_DIM,
        );
        p.text(
            pos2(bar.right(), y),
            Align2::RIGHT_CENTER,
            &device,
            theme::mono(12.0),
            theme::TEXT_FAINT,
        );
        y += 36.0;

        // Stage stepper.
        let stages: Vec<Stage> = if job.reopening {
            vec![Stage::Decoding]
        } else {
            Stage::ALL.to_vec()
        };
        let step_w = (bar.width()) / stages.len() as f32;
        let cur = job.stage.index();
        for (i, s) in stages.iter().enumerate() {
            let cx = x + step_w * i as f32 + 12.0;
            let cc = pos2(cx, y + 8.0);
            let idx = s.index();
            let (fill, fg) = if idx < cur {
                (theme::GOOD, Some(Icon::Check))
            } else if idx == cur {
                (theme::VIOLET, None)
            } else {
                (theme::CARD_HI, None)
            };
            if idx == cur {
                paint::glow_circle(
                    p,
                    cc,
                    11.0,
                    theme::alpha(theme::VIOLET, 0.4 + 0.4 * pulse),
                    10.0,
                );
                p.circle_filled(cc, 11.0, fill);
                progress::dots(p, cc, 5.5, t, Color32::WHITE);
            } else {
                p.circle_filled(
                    cc,
                    11.0,
                    theme::alpha(fill, if idx < cur { 0.22 } else { 1.0 }),
                );
                p.circle_stroke(
                    cc,
                    11.0,
                    Stroke::new(1.0, if idx < cur { fill } else { theme::STROKE_HI }),
                );
            }
            if let Some(ic) = fg {
                icons::paint(p, Rect::from_center_size(cc, vec2(11.0, 11.0)), ic, fill);
            }
            if i + 1 < stages.len() {
                let x0 = cc.x + 16.0;
                let x1 = x + step_w * (i + 1) as f32 + 12.0 - 16.0;
                let done = idx < cur;
                p.line_segment(
                    [pos2(x0, cc.y), pos2(x1, cc.y)],
                    Stroke::new(
                        1.5,
                        if done {
                            theme::alpha(theme::GOOD, 0.6)
                        } else {
                            theme::STROKE
                        },
                    ),
                );
            }
            let lc = if idx == cur {
                theme::TEXT
            } else if idx < cur {
                theme::TEXT_DIM
            } else {
                theme::TEXT_FAINT
            };
            p.text(
                pos2(cx - 12.0, y + 34.0),
                Align2::LEFT_CENTER,
                s.label(),
                theme::semi(12.0),
                lc,
            );
        }

        // Cancel button, bottom-right of the card.
        let br = Rect::from_min_size(
            pos2(rect.right() - 48.0 - 120.0, rect.bottom() - 62.0),
            vec2(120.0, 36.0),
        );
        let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(br));
        cancel = Button::new("Cancel")
            .kind(Kind::Danger)
            .icon(Icon::Close)
            .min_width(120.0)
            .height(36.0)
            .show(&mut bui)
            .clicked();

        // Stems being extracted.
        let sy = rect.bottom() - 44.0;
        p.text(
            pos2(x, sy),
            Align2::LEFT_CENTER,
            "STEMS",
            theme::semi(10.5),
            theme::TEXT_FAINT,
        );
        let mut sxp = x + 52.0;
        let n = job.stems.len().max(1);
        let sep_frac = if job.stage == Stage::Separating {
            job.frac.unwrap_or(0.0)
        } else if cur > Stage::Separating.index() {
            1.0
        } else {
            0.0
        };
        for (i, k) in job.stems.iter().enumerate() {
            let col = theme::stem_color(*k);
            let lit = ((sep_frac * n as f32) - i as f32).clamp(0.0, 1.0);
            let g = p.layout_no_wrap(k.label().to_owned(), theme::semi(12.0), Color32::WHITE);
            let r = Rect::from_min_size(pos2(sxp, sy - 12.0), vec2(g.size().x + 26.0, 24.0));
            if sxp + r.width() > br.left() - 10.0 {
                break;
            }
            if lit > 0.0 {
                paint::glow(
                    p,
                    r,
                    CornerRadius::same(12),
                    theme::alpha(col, 0.35 * lit),
                    10.0,
                );
            }
            p.rect(
                r,
                CornerRadius::same(12),
                theme::alpha(col, 0.06 + 0.2 * lit),
                Stroke::new(1.0, theme::alpha(col, 0.25 + 0.6 * lit)),
                StrokeKind::Inside,
            );
            p.circle_filled(
                pos2(r.left() + 11.0, r.center().y),
                3.0,
                theme::alpha(col, 0.4 + 0.6 * lit),
            );
            p.galley(
                pos2(r.left() + 18.0, r.center().y - g.size().y / 2.0),
                g,
                theme::lerp_color(theme::TEXT_FAINT, Color32::WHITE, lit),
            );
            sxp += r.width() + 8.0;
        }

        ui.add_space(18.0);
        // Live bars under the card.
        let (bars, _) = ui.allocate_exact_size(vec2(w, 64.0), Sense::hover());
        let energy = if job.stage == Stage::Separating {
            0.9
        } else {
            0.45
        };
        progress::spectrum(ui.painter(), bars, t, energy, 64);
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(
                "Tip: you can keep browsing — we'll ping you when the stems are ready.",
            )
            .font(theme::regular(12.5))
            .color(theme::TEXT_FAINT),
        );
    });
    if cancel {
        app.cancel_job();
    }
}
