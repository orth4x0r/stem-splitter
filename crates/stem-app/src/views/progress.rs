//! The "working on it" screen: spinning vinyl, progress ring, stage stepper, live bars.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2,
};

use crate::app::StemApp;
use crate::backend::mmss;
use crate::pipeline::Stage;
use crate::theme::{self, motion};
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
    let chain = chain_steps(app, job);
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
        let rr = CornerRadius::same(16);
        paint::glass(p, rect, rr, None, 0.0);

        // ---- left: vinyl + ring
        let c = pos2(rect.left() + 200.0, rect.center().y);
        let spin = (t * 1.8) as f32; // ~33 rpm, near enough
        // Platter under the record.
        p.circle_filled(c, 132.0, Color32::from_rgb(14, 14, 22));
        p.circle_stroke(c, 132.0, Stroke::new(1.0, Color32::from_white_alpha(16)));
        progress::vinyl(p, c, 118.0, spin, t);
        let ring_frac = if job.frac.is_none() && job.stage != Stage::Separating {
            None
        } else {
            Some(overall)
        };
        progress::ring(p, c, 146.0, 5.0, ring_frac, t);
        tone_arm(p, c, t);

        // ---- right: text column
        let x = rect.left() + 400.0;
        let mut y = rect.top() + 48.0;
        let stage_label = if job.reopening {
            "Loading session"
        } else if job.preparing {
            "Loading track"
        } else {
            "Splitting"
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

        // Big number: digits in white, the % sign in the accent gradient.
        let num = format!("{:.0}", overall * 100.0);
        let g = p.layout_no_wrap(num, theme::bold(60.0), Color32::WHITE);
        let gsz = g.size();
        paint::soft_disc(
            p,
            pos2(x + gsz.x * 0.5, y),
            gsz.x,
            theme::alpha(theme::VIOLET, 0.18),
        );
        p.galley(pos2(x - 2.0, y - gsz.y / 2.0), g, Color32::WHITE);
        let pr = paint::gradient_text(
            p,
            pos2(x + gsz.x + 2.0, y + 6.0),
            Align2::LEFT_CENTER,
            "%",
            theme::bold(34.0),
            ((t * 0.1) as f32, (t * 0.1) as f32 + 0.6),
            1.0,
        );
        let sx = pr.right() + 18.0;
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
        } else if job.preparing {
            // The pre-split step stops after decoding; the section picker comes next.
            match job.source {
                crate::pipeline::InputSource::Url(_) => vec![Stage::Fetching, Stage::Decoding],
                crate::pipeline::InputSource::File(_) => vec![Stage::Decoding],
            }
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
                // A spark travels along the connector into the active step.
                if idx + 1 == cur && !motion::reduced() {
                    let f = ((t * 0.9) % 1.0) as f32;
                    let sp = pos2(x0 + (x1 - x0) * motion::ease_in_out(f), cc.y);
                    paint::glow_circle(p, sp, 2.0, theme::alpha(theme::GOOD, 0.8), 8.0);
                    p.circle_filled(sp, 2.0, Color32::WHITE);
                }
            }
            let lc = if idx == cur {
                theme::TEXT
            } else if idx < cur {
                theme::TEXT_DIM
            } else {
                theme::TEXT_FAINT
            };
            // When the steps are cramped (small windows) only the current one is named.
            if step_w >= 104.0 || idx == cur {
                p.text(
                    pos2(cx - 12.0, y + 34.0),
                    Align2::LEFT_CENTER,
                    s.label(),
                    theme::semi(12.0),
                    lc,
                );
            }
        }

        // A chain runs step by step: a timeline of its steps under the stage stepper.
        if let Some((steps, cur)) = &chain {
            let row = Rect::from_min_max(pos2(x, y + 56.0), pos2(bar.right(), y + 94.0));
            let n = steps.len() as f32;
            let step_frac = (job.frac.unwrap_or(0.0) * n - *cur as f32).clamp(0.0, 1.0);
            chain_timeline(p, row, steps, *cur, step_frac, t);
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
            "Stems",
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
                // Small windows: say how many more instead of dropping them silently.
                p.text(
                    pos2(sxp + 2.0, sy),
                    Align2::LEFT_CENTER,
                    format!("+{}", n - i),
                    theme::semi(12.0),
                    theme::TEXT_DIM,
                );
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
        let palette: Vec<Color32> = job.stems.iter().map(|k| theme::stem_color(*k)).collect();
        progress::spectrum_in(ui.painter(), bars, t, energy, 64, &palette);
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(
                "You can keep browsing. The header shows the progress and a notice appears when the stems are ready.",
            )
            .font(theme::regular(12.5))
            .color(theme::TEXT_FAINT),
        );
    });
    if cancel {
        app.cancel_job();
    }
}

/// Brushed-metal tone arm resting on the record, with a gentle tracking wobble.
fn tone_arm(p: &egui::Painter, c: egui::Pos2, t: f64) {
    let wob = if motion::reduced() {
        0.0
    } else {
        (t * 1.8).sin() as f32 * 1.2
    };
    let pivot = c + vec2(132.0, -132.0);
    let elbow = c + vec2(104.0, -54.0);
    let head = c + vec2(56.0 + wob, -30.0 + wob * 0.5);
    let metal = Color32::from_rgb(150, 150, 178);
    let shadow = Color32::from_black_alpha(120);
    for (a, b) in [(pivot, elbow), (elbow, head)] {
        p.line_segment(
            [a + vec2(3.0, 5.0), b + vec2(3.0, 5.0)],
            Stroke::new(5.0, shadow),
        );
    }
    for (a, b) in [(pivot, elbow), (elbow, head)] {
        p.line_segment([a, b], Stroke::new(4.0, Color32::from_rgb(92, 92, 118)));
        p.line_segment([a, b], Stroke::new(1.2, metal));
    }
    // Pivot: counterweight and bearing.
    paint::glow_circle(p, pivot, 10.0, Color32::from_black_alpha(140), 10.0);
    p.circle_filled(pivot, 13.0, Color32::from_rgb(44, 44, 60));
    p.circle_stroke(pivot, 13.0, Stroke::new(1.0, Color32::from_white_alpha(40)));
    p.circle_filled(pivot, 6.0, Color32::from_rgb(110, 110, 138));
    p.circle_filled(pivot - vec2(1.5, 1.5), 2.0, Color32::from_white_alpha(160));
    // Headshell with a tiny glowing stylus light.
    let dir = (head - elbow).normalized();
    let n = vec2(-dir.y, dir.x);
    let a = head + dir * 10.0;
    let quad = vec![
        head - n * 6.0 - dir * 4.0,
        head + n * 6.0 - dir * 4.0,
        a + n * 5.0,
        a - n * 5.0,
    ];
    p.add(egui::Shape::convex_polygon(
        quad,
        Color32::from_rgb(120, 120, 148),
        Stroke::new(1.0, Color32::from_white_alpha(50)),
    ));
    let led = a - n * 2.0;
    paint::glow_circle(p, led, 1.5, theme::alpha(theme::PINK, 0.9), 8.0);
    p.circle_filled(led, 1.6, theme::PINK);
}

/// The steps of the chain this job runs, as (model, input) labels, and the 0-based step that
/// is running now. Read from the "Step k/n" prefix the chain runner puts on its progress lines
/// and the preset named like the job; `None` for single-model splits.
fn chain_steps(
    app: &StemApp,
    job: &crate::app::RunningJob,
) -> Option<(Vec<(String, String)>, usize)> {
    let rest = job.detail.strip_prefix("Step ")?;
    let (k, rest) = rest.split_once('/')?;
    let n: usize = rest
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    let k: usize = k.parse().ok()?;
    if n < 2 || k == 0 || k > n {
        return None;
    }
    let preset = app
        .settings
        .all_presets()
        .into_iter()
        .find(|p| p.name == job.model_name && p.chain.steps.len() == n);
    let model = |id: &str| {
        app.backend.models.iter().find(|m| m.id == id).map_or_else(
            || id.to_owned(),
            |m| crate::pipeline::chain::short_model_name(m.name),
        )
    };
    let steps = match preset {
        Some(p) => p
            .chain
            .steps
            .iter()
            .map(|st| {
                let name = if st.models.len() > 1 {
                    format!("{} models · {}", st.models.len(), st.ensemble.label())
                } else {
                    st.models.first().map(|m| model(m)).unwrap_or_default()
                };
                (name, format!("on {}", st.input.label().to_lowercase()))
            })
            .collect(),
        None => (1..=n)
            .map(|i| (format!("Step {i}"), String::new()))
            .collect(),
    };
    Some((steps, k - 1))
}

/// Chain steps as numbered chips joined by connectors: done steps get a check, the running
/// one an accent outline with its own progress fill, later ones stay faint.
fn chain_timeline(
    p: &egui::Painter,
    row: Rect,
    steps: &[(String, String)],
    cur: usize,
    step_frac: f32,
    t: f64,
) {
    let n = steps.len().max(1);
    let gap = 26.0;
    let w = ((row.width() - gap * (n - 1) as f32) / n as f32).max(60.0);
    for (i, (name, input)) in steps.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(row.left() + i as f32 * (w + gap), row.top()),
            vec2(w, row.height()),
        );
        let done = i < cur;
        let now = i == cur;
        let rr = CornerRadius::same(theme::R_CTRL);
        p.rect(
            r,
            rr,
            if now {
                theme::alpha(theme::VIOLET, 0.10)
            } else {
                Color32::from_white_alpha(4)
            },
            Stroke::new(
                1.0,
                if now {
                    theme::alpha(theme::VIOLET, 0.8)
                } else if done {
                    theme::alpha(theme::GOOD, 0.45)
                } else {
                    theme::STROKE
                },
            ),
            StrokeKind::Inside,
        );
        if now {
            // This step's own progress, along the chip's bottom edge.
            let bar = Rect::from_min_max(
                pos2(r.left() + 8.0, r.bottom() - 3.0),
                pos2(r.right() - 8.0, r.bottom() - 1.5),
            );
            p.rect_filled(bar, CornerRadius::ZERO, Color32::from_white_alpha(14));
            p.rect_filled(
                Rect::from_min_size(bar.min, vec2(bar.width() * step_frac, bar.height())),
                CornerRadius::ZERO,
                theme::VIOLET,
            );
        }
        // Number badge.
        let c = pos2(r.left() + 18.0, r.center().y);
        if done {
            p.circle_filled(c, 9.0, theme::alpha(theme::GOOD, 0.2));
            icons::paint(
                p,
                Rect::from_center_size(c, vec2(10.0, 10.0)),
                Icon::Check,
                theme::GOOD,
            );
        } else {
            p.circle_filled(c, 9.0, if now { theme::VIOLET } else { theme::CARD_HI });
            if now {
                progress::dots(p, c, 4.5, t, Color32::WHITE);
            } else {
                p.text(
                    c,
                    Align2::CENTER_CENTER,
                    format!("{}", i + 1),
                    theme::semi(11.0),
                    theme::TEXT_FAINT,
                );
            }
        }
        let max_chars = ((w - 40.0) / 6.8).max(4.0) as usize;
        let clip = p.with_clip_rect(r.shrink(2.0).intersect(p.clip_rect()));
        let (nc, ic) = if now || done {
            (theme::TEXT, theme::TEXT_DIM)
        } else {
            (theme::TEXT_DIM, theme::TEXT_FAINT)
        };
        let one_line = input.is_empty();
        clip.text(
            pos2(
                r.left() + 34.0,
                if one_line {
                    r.center().y
                } else {
                    r.top() + 13.0
                },
            ),
            Align2::LEFT_CENTER,
            elide(name, max_chars),
            theme::semi(12.0),
            nc,
        );
        if !one_line {
            clip.text(
                pos2(r.left() + 34.0, r.top() + 26.0),
                Align2::LEFT_CENTER,
                elide(input, max_chars),
                theme::regular(11.0),
                ic,
            );
        }
        if i + 1 < n {
            let y = r.center().y;
            let (a, b) = (r.right() + 5.0, r.right() + gap - 5.0);
            p.line_segment(
                [pos2(a, y), pos2(b, y)],
                Stroke::new(
                    1.5,
                    if done {
                        theme::alpha(theme::GOOD, 0.6)
                    } else {
                        theme::STROKE_HI
                    },
                ),
            );
            p.add(egui::Shape::convex_polygon(
                vec![pos2(b, y), pos2(b - 5.0, y - 3.5), pos2(b - 5.0, y + 3.5)],
                if done {
                    theme::alpha(theme::GOOD, 0.6)
                } else {
                    theme::STROKE_HI
                },
                Stroke::NONE,
            ));
        }
    }
}
