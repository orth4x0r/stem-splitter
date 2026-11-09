//! Presets and chains: the picker strip on the start screen, the preset cards on the Models
//! page, the preset editor sheet, and the stem cache rows in Settings.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Pos2, Rect, Sense, Stroke, StrokeKind,
    pos2, vec2,
};
use stem_audio::ensemble::EnsembleMode;
use stem_models::{ModelInfo, StemKind};

use crate::app::{Page, StemApp};
use crate::backend::human_bytes;
use crate::pipeline::chain::{Chain, Plan, Step, StepInput, short_model_name};
use crate::settings::Preset;
use crate::theme::{self, motion};
use crate::widgets::buttons::{self, Button};
use crate::widgets::icons::{self, Icon};
use crate::widgets::sheet::Sheet;
use crate::widgets::toast::elide;
use crate::widgets::{overline, paint};

/// The open editor sheet.
pub struct PresetEditor {
    pub preset: Preset,
    /// Editing an existing user preset (vs. a new one or a copy of a built-in).
    pub existing: bool,
}

fn model<'a>(app: &'a StemApp, id: &str) -> Option<&'a ModelInfo> {
    app.backend.models.iter().find(|m| m.id == id)
}

fn model_label(app: &StemApp, id: &str) -> String {
    model(app, id).map_or_else(|| id.to_owned(), |m| short_model_name(m.name))
}

fn plan(app: &StemApp, chain: &Chain) -> Result<Plan, String> {
    chain.plan(&|id| model(app, id).cloned())
}

/// Badge text and colour for a preset's shape.
fn kind_badge(chain: &Chain) -> (&'static str, Color32) {
    if chain.steps.iter().any(Step::is_ensemble) {
        ("ENSEMBLE", theme::CYAN)
    } else if chain.steps.len() > 1 {
        ("CHAIN", theme::PINK)
    } else {
        ("1 MODEL", theme::VIOLET)
    }
}

/// Everything a card needs, gathered before painting (the painter borrows the UI).
struct CardData {
    preset: Preset,
    plan: Result<Plan, String>,
    /// Problem that stops it from running here (model not hosted yet, unknown id).
    blocked: Option<String>,
    selected: bool,
}

fn cards(app: &StemApp) -> Vec<CardData> {
    let active = app.settings.active_preset().map(|p| p.id);
    app.settings
        .all_presets()
        .into_iter()
        .map(|preset| CardData {
            plan: plan(app, &preset.chain),
            blocked: app.chain_spec(&preset).err(),
            selected: active.as_deref() == Some(preset.id.as_str()),
            preset,
        })
        .collect()
}

// ---- shared painting -------------------------------------------------------------------------

/// The selected / hovered card surface, matching the model cards.
fn surface(p: &egui::Painter, r: Rect, rr: u8, s: f32, h: f32, t: f64) {
    let radius = CornerRadius::same(rr);
    if s > 0.0 {
        paint::glow(p, r, radius, theme::alpha(theme::VIOLET, 0.35 * s), 18.0);
        let u = (t * 0.05) as f32 * s;
        paint::gradient_rect(
            p,
            r,
            radius,
            u,
            u + 1.0,
            Color32::from_white_alpha((255.0 * s) as u8),
        );
        p.rect_filled(
            r.shrink(1.5),
            CornerRadius::same(rr - 1),
            theme::lerp_color(theme::CARD, Color32::from_rgb(24, 24, 40), h),
        );
        paint::glass_sheen(p, r.shrink(1.5), CornerRadius::same(rr - 1), 0.6 + h);
    } else {
        paint::glass(p, r, radius, None, h);
    }
}

/// Small arrow from `a` to `b` (horizontal).
fn arrow(p: &egui::Painter, a: Pos2, b: Pos2, color: Color32) {
    p.line_segment([a, b - vec2(3.0, 0.0)], Stroke::new(1.4, color));
    p.add(egui::Shape::convex_polygon(
        vec![b, b + vec2(-5.0, -3.5), b + vec2(-5.0, 3.5)],
        color,
        Stroke::NONE,
    ));
}

/// A small rounded pill with text; returns its width.
/// Text that doesn't fit in `max_w` is cut with an ellipsis.
fn pill(
    p: &egui::Painter,
    left: Pos2,
    max_w: f32,
    text: &str,
    font: egui::FontId,
    color: Color32,
) -> f32 {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, theme::TEXT);
    job.wrap.max_width = (max_w - 16.0).max(8.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let g = p.fonts_mut(|f| f.layout_job(job));
    let r = Rect::from_min_size(pos2(left.x, left.y - 10.0), vec2(g.size().x + 16.0, 20.0));
    p.rect(
        r,
        CornerRadius::same(10),
        theme::alpha(color, 0.14),
        Stroke::new(1.0, theme::alpha(color, 0.45)),
        StrokeKind::Inside,
    );
    p.galley(
        pos2(r.left() + 8.0, r.center().y - g.size().y / 2.0),
        g,
        theme::TEXT,
    );
    r.width()
}

/// One-line flow: "Mel-RoFormer → HTDemucs on instrumental" as pills and arrows, clipped to
/// `max_x`.
fn flow_line(app: &StemApp, p: &egui::Painter, left: Pos2, max_x: f32, chain: &Chain) {
    let p = p.with_clip_rect(
        Rect::from_min_max(pos2(left.x, left.y - 14.0), pos2(max_x, left.y + 14.0))
            .intersect(p.clip_rect()),
    );
    let mut x = left.x;
    for (i, step) in chain.steps.iter().enumerate() {
        if i > 0 {
            arrow(
                &p,
                pos2(x + 4.0, left.y),
                pos2(x + 18.0, left.y),
                theme::TEXT_FAINT,
            );
            x += 24.0;
        }
        let first = step
            .models
            .first()
            .map_or(String::new(), |m| model_label(app, m));
        let color = if step.is_ensemble() {
            theme::CYAN
        } else {
            [theme::VIOLET, theme::PINK, theme::CYAN][i % 3]
        };
        // The arrow says "on the stem before"; the Models page spells the input out.
        let text = if step.is_ensemble() {
            format!("{first} +{}", step.models.len() - 1)
        } else {
            first
        };
        if max_x - x < 40.0 {
            break;
        }
        x += pill(
            &p,
            pos2(x, left.y),
            max_x - x,
            &text,
            theme::regular(11.5),
            color,
        );
    }
}

/// Stem dots with names along a baseline; collapses to dots when they don't fit.
fn stem_row(p: &egui::Painter, left: Pos2, width: f32, kinds: &[StemKind]) {
    let font = theme::regular(11.5);
    let need: f32 = kinds
        .iter()
        .map(|k| {
            p.layout_no_wrap(k.label().to_owned(), font.clone(), theme::TEXT_DIM)
                .size()
                .x
                + 18.0
        })
        .sum();
    let mut x = left.x;
    for k in kinds {
        let c = theme::stem_color(*k);
        paint::glow_circle(p, pos2(x + 3.0, left.y), 2.0, theme::alpha(c, 0.5), 6.0);
        p.circle_filled(pos2(x + 3.0, left.y), 3.0, c);
        if need <= width {
            let g = p.layout_no_wrap(k.label().to_owned(), font.clone(), theme::TEXT_DIM);
            let w = g.size().x;
            p.galley(
                pos2(x + 10.0, left.y - g.size().y / 2.0),
                g,
                theme::TEXT_DIM,
            );
            x += w + 18.0;
        } else {
            x += 12.0;
        }
    }
    if need > width {
        // Names after the dots in whatever room is left, cut with an ellipsis.
        let names = kinds
            .iter()
            .map(|k| k.label())
            .collect::<Vec<_>>()
            .join(" · ");
        let mut job = egui::text::LayoutJob::simple_singleline(
            names,
            theme::regular(11.0),
            theme::TEXT_FAINT,
        );
        job.wrap.max_width = (left.x + width - (x + 4.0)).max(10.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        let g = p.fonts_mut(|f| f.layout_job(job));
        p.galley(
            pos2(x + 4.0, left.y - g.size().y / 2.0),
            g,
            theme::TEXT_FAINT,
        );
    }
}

/// Animated card frame shared by the strip and the Models page. Returns the moved rect and
/// the hover / selection amounts.
fn card_frame(
    ui: &egui::Ui,
    rect: Rect,
    resp: &egui::Response,
    idx: usize,
    selected: bool,
    rr: u8,
) -> (Rect, f32, f32) {
    let ctx = ui.ctx();
    let enter = motion::ease_out_back(motion::appear(
        ctx,
        resp.id.with("in"),
        0.08 + idx as f32 * motion::STAGGER,
        motion::ENTER,
    ));
    let h = ctx.animate_bool_with_time(resp.id, resp.hovered(), motion::BASE);
    let s = ctx.animate_bool_with_time(resp.id.with("s"), selected, motion::SLOW);
    let lift = motion::bouncy(
        ctx,
        resp.id.with("lift"),
        if resp.hovered() { -3.0 } else { 0.0 },
    );
    let r = rect.translate(vec2(0.0, lift + 18.0 * (1.0 - enter)));
    let p = ui.painter();
    let t = ui.input(|i| i.time);
    surface(p, r, rr, s, h, t);
    if let Some(ptr) = resp.hover_pos() {
        paint::spotlight(p, r, ptr, theme::PINK, h);
    }
    let sweep = ctx.animate_bool_with_time(resp.id.with("sweep"), resp.hovered(), 0.6);
    if resp.hovered() {
        paint::shine(p, r, sweep, 1.0);
    }
    (r, h, s)
}

fn check_badge(p: &egui::Painter, c: Pos2, s: f32) {
    let pop = motion::ease_out_back(s);
    paint::glow_circle(p, c, 6.0, theme::alpha(theme::VIOLET, 0.6 * s), 10.0);
    p.circle_filled(c, 9.0 * pop, theme::VIOLET);
    icons::paint(
        p,
        Rect::from_center_size(c, vec2(10.0, 10.0) * pop),
        Icon::Check,
        Color32::WHITE,
    );
}

fn badge(p: &egui::Painter, right_top: Pos2, text: &str, color: Color32) -> Rect {
    let g = p.layout_no_wrap(text.to_owned(), theme::semi(9.5), color);
    let r = Rect::from_min_size(
        pos2(right_top.x - g.size().x - 14.0, right_top.y),
        vec2(g.size().x + 14.0, 18.0),
    );
    p.rect(
        r,
        CornerRadius::same(9),
        theme::alpha(color, 0.12),
        Stroke::new(1.0, theme::alpha(color, 0.4)),
        StrokeKind::Inside,
    );
    p.galley(
        pos2(r.left() + 7.0, r.center().y - g.size().y / 2.0),
        g,
        color,
    );
    r
}

// ---- start screen ----------------------------------------------------------------------------

/// The preset picker above the model strip.
pub fn strip(app: &mut StemApp, ui: &mut egui::Ui) {
    let mut manage = false;
    ui.horizontal(|ui| {
        overline(ui, "Preset");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            manage = Button::ghost("Edit presets")
                .icon(Icon::Sparkle)
                .height(28.0)
                .show(ui)
                .clicked();
        });
    });
    if manage {
        app.page = Page::Models;
    }
    ui.add_space(4.0);
    let data = cards(app);
    let per_row = 4usize;
    let gap = 12.0;
    let mut pick: Option<Option<String>> = None;
    let t = ui.input(|i| i.time);
    for (row_i, row) in data.chunks(per_row).enumerate() {
        let n = per_row.min(data.len()).max(1) as f32;
        let cw = (ui.available_width() - gap * (n - 1.0)) / n;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (j, c) in row.iter().enumerate() {
                let idx = row_i * per_row + j;
                let (rect, resp) = ui.allocate_exact_size(vec2(cw, 98.0), Sense::click());
                let tip = match &c.blocked {
                    Some(b) => format!("{}\n\n{b}", c.preset.description),
                    None => c.preset.description.clone(),
                };
                let resp = resp
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text(tip);
                if resp.clicked() {
                    pick = Some(if c.selected {
                        None
                    } else {
                        Some(c.preset.id.clone())
                    });
                }
                let (r, _h, s) = card_frame(ui, rect, &resp, idx, c.selected, 14);
                let p = ui.painter();
                let dim = c.blocked.is_some() && !c.selected;
                let text_c = if dim { theme::TEXT_DIM } else { theme::TEXT };
                let (btxt, bcol) = kind_badge(&c.preset.chain);
                let b = badge(p, pos2(r.right() - 12.0, r.top() + 13.0), btxt, bcol);
                let name_w = (b.left() - 8.0 - (r.left() + 16.0)).max(20.0);
                p.text(
                    pos2(r.left() + 16.0, r.top() + 22.0),
                    Align2::LEFT_CENTER,
                    elide(&c.preset.name, (name_w / 7.5) as usize),
                    theme::semi(14.0),
                    text_c,
                );
                flow_line(
                    app,
                    p,
                    pos2(r.left() + 16.0, r.top() + 50.0),
                    r.right() - 12.0,
                    &c.preset.chain,
                );
                match (&c.plan, &c.blocked) {
                    (Ok(plan), None) => {
                        stem_row(
                            p,
                            pos2(r.left() + 16.0, r.bottom() - 18.0),
                            r.width() - 56.0,
                            &plan.kinds(),
                        );
                    }
                    _ => {
                        p.text(
                            pos2(r.left() + 16.0, r.bottom() - 18.0),
                            Align2::LEFT_CENTER,
                            "Not available yet",
                            theme::regular(11.5),
                            theme::WARN,
                        );
                    }
                }
                if s > 0.0 {
                    check_badge(p, pos2(r.right() - 20.0, r.bottom() - 18.0), s);
                }
                paint::ripple(p, &resp, r, theme::VIOLET);
                paint::focus(p, &resp, r, CornerRadius::same(14));
                let _ = t;
            }
        });
        ui.add_space(gap);
    }
    if let Some(id) = pick {
        app.pick_preset(id);
    }
}

// ---- Models page -----------------------------------------------------------------------------

/// Preset cards with the full chain drawn out, plus "New preset".
pub fn section(app: &mut StemApp, ui: &mut egui::Ui) {
    let mut new = false;
    ui.horizontal(|ui| {
        overline(ui, "Presets & chains");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            new = Button::new("New preset")
                .icon(Icon::Plus)
                .height(30.0)
                .show(ui)
                .clicked();
        });
    });
    ui.label(
        egui::RichText::new(
            "Run models one after another (split the instrumental further) or several at once \
             and combine them. Results are cached, so re-running a preset is instant.",
        )
        .font(theme::regular(12.5))
        .color(theme::TEXT_DIM),
    );
    ui.add_space(8.0);
    if new {
        open_new(app);
    }

    let data = cards(app);
    let gap = 16.0;
    let cols = if ui.available_width() > 760.0 { 2 } else { 1 };
    let cw = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
    let ch = 176.0;
    let mut action: Option<(Action, Preset)> = None;
    for (row_i, row) in data.chunks(cols).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (j, c) in row.iter().enumerate() {
                let idx = row_i * cols + j;
                let (rect, resp) = ui.allocate_exact_size(vec2(cw, ch), Sense::click());
                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                if resp.clicked() {
                    action = Some((Action::Use, c.preset.clone()));
                }
                let (r, h, s) = card_frame(ui, rect, &resp, idx, c.selected, 18);
                if let Some(a) = big_card(app, ui, r, c, h, s) {
                    action = Some((a, c.preset.clone()));
                }
                paint::focus(ui.painter(), &resp, r, CornerRadius::same(18));
            }
        });
        ui.add_space(gap);
    }
    if let Some((a, preset)) = action {
        match a {
            Action::Use => {
                let id = if app.settings.preset_id.as_deref() == Some(preset.id.as_str()) {
                    None
                } else {
                    Some(preset.id)
                };
                app.pick_preset(id);
            }
            Action::Edit => {
                app.preset_editor = Some(PresetEditor {
                    existing: !preset.is_builtin(),
                    preset: if preset.is_builtin() {
                        Preset {
                            id: String::new(),
                            name: format!("{} (copy)", preset.name),
                            ..preset
                        }
                    } else {
                        preset
                    },
                });
            }
            Action::Delete => {
                app.settings.delete_preset(&preset.id);
                app.toasts.info("Preset deleted", preset.name);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Action {
    Use,
    Edit,
    Delete,
}

fn open_new(app: &mut StemApp) {
    let first = app
        .backend
        .models
        .iter()
        .find(|m| app.backend.is_available(m))
        .map_or("htdemucs", |m| m.id);
    app.preset_editor = Some(PresetEditor {
        existing: false,
        preset: Preset {
            id: String::new(),
            name: "My preset".into(),
            description: String::new(),
            chain: Chain::single(first),
        },
    });
}

/// Contents of a Models-page preset card: title, description, the chain as nodes, outputs and
/// buttons.
fn big_card(
    app: &StemApp,
    ui: &mut egui::Ui,
    r: Rect,
    c: &CardData,
    h: f32,
    s: f32,
) -> Option<Action> {
    let mut action = None;
    let inner = r.shrink(20.0);
    {
        let p = ui.painter();
        let (btxt, bcol) = kind_badge(&c.preset.chain);
        // Accent wash in the badge colour.
        paint::soft_disc(
            &p.with_clip_rect(r.shrink(2.0).intersect(p.clip_rect())),
            pos2(r.left() + 70.0, r.top()),
            160.0,
            theme::alpha(bcol, 0.07 + 0.05 * h),
        );
        let b = badge(p, pos2(inner.right(), inner.top() - 2.0), btxt, bcol);
        p.text(
            pos2(inner.left(), inner.top() + 7.0),
            Align2::LEFT_CENTER,
            &c.preset.name,
            theme::semi(16.0),
            theme::TEXT,
        );
        if !c.preset.is_builtin() {
            badge(
                p,
                pos2(b.left() - 6.0, inner.top() - 2.0),
                "YOURS",
                theme::GOOD,
            );
        }
        let mut job = egui::text::LayoutJob::simple_singleline(
            c.preset.description.clone(),
            theme::regular(12.5),
            theme::TEXT_DIM,
        );
        job.wrap.max_width = inner.width();
        job.wrap.max_rows = 1;
        let g = p.fonts_mut(|f| f.layout_job(job));
        p.galley(pos2(inner.left(), inner.top() + 20.0), g, theme::TEXT_DIM);

        // The chain as nodes.
        nodes(
            app,
            p,
            Rect::from_min_size(
                pos2(inner.left(), inner.top() + 46.0),
                vec2(inner.width(), 46.0),
            ),
            &c.preset.chain,
        );
    }

    // Footer: outputs (or the problem) on the left, buttons on the right.
    let foot_y = inner.bottom() - 14.0;
    {
        let p = ui.painter();
        match (&c.plan, &c.blocked) {
            (Ok(plan), None) => {
                p.text(
                    pos2(inner.left(), foot_y),
                    Align2::LEFT_CENTER,
                    "OUT",
                    theme::semi(10.0),
                    theme::TEXT_FAINT,
                );
                stem_row(
                    p,
                    pos2(inner.left() + 32.0, foot_y),
                    inner.width() - 250.0,
                    &plan.kinds(),
                );
            }
            (_, Some(e)) | (Err(e), None) => {
                p.text(
                    pos2(inner.left(), foot_y),
                    Align2::LEFT_CENTER,
                    elide(e, 46),
                    theme::regular(11.5),
                    theme::WARN,
                );
            }
        }
    }
    let btns = Rect::from_min_max(
        pos2(inner.right() - 240.0, foot_y - 15.0),
        pos2(inner.right(), foot_y + 15.0),
    );
    let mut bui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(btns)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    bui.spacing_mut().item_spacing.x = 6.0;
    if c.selected {
        let (cr, _) = bui.allocate_exact_size(vec2(70.0, 30.0), Sense::hover());
        check_badge(bui.painter(), pos2(cr.right() - 10.0, cr.center().y), s);
        bui.painter().text(
            pos2(cr.right() - 24.0, cr.center().y),
            Align2::RIGHT_CENTER,
            "In use",
            theme::semi(12.5),
            theme::TEXT,
        );
    } else if Button::primary("Use")
        .height(30.0)
        .enabled(c.blocked.is_none())
        .show(&mut bui)
        .clicked()
    {
        action = Some(Action::Use);
    }
    let builtin = c.preset.is_builtin();
    if Button::ghost(if builtin { "Duplicate" } else { "Edit" })
        .height(30.0)
        .show(&mut bui)
        .clicked()
    {
        action = Some(Action::Edit);
    }
    if !builtin
        && buttons::icon_button(
            &mut bui,
            Icon::Close,
            30.0,
            false,
            theme::BAD,
            "Delete preset",
        )
        .clicked()
    {
        action = Some(Action::Delete);
    }
    action
}

/// Steps as connected nodes inside `area`.
fn nodes(app: &StemApp, p: &egui::Painter, area: Rect, chain: &Chain) {
    let n = chain.steps.len().max(1) as f32;
    let link = 30.0;
    let w = ((area.width() - link * (n - 1.0)) / n).min(260.0);
    for (i, step) in chain.steps.iter().enumerate() {
        let x = area.left() + i as f32 * (w + link);
        let r = Rect::from_min_size(pos2(x, area.top()), vec2(w, area.height()));
        let color = if step.is_ensemble() {
            theme::CYAN
        } else {
            [theme::VIOLET, theme::PINK, theme::CYAN][i % 3]
        };
        p.rect(
            r,
            CornerRadius::same(12),
            theme::alpha(color, 0.08),
            Stroke::new(1.0, theme::alpha(color, 0.38)),
            StrokeKind::Inside,
        );
        paint::glass_sheen(p, r, CornerRadius::same(12), 0.2);
        // Step number.
        let c = pos2(r.left() + 18.0, r.center().y);
        paint::glow_circle(p, c, 6.0, theme::alpha(color, 0.4), 8.0);
        p.circle_filled(c, 10.0, color);
        p.text(
            c,
            Align2::CENTER_CENTER,
            format!("{}", i + 1),
            theme::bold(11.0),
            Color32::WHITE,
        );
        let names: Vec<String> = step.models.iter().map(|m| model_label(app, m)).collect();
        let title = if step.is_ensemble() {
            format!("{} models · {}", names.len(), step.ensemble.label())
        } else {
            names.join("")
        };
        let clip = p.with_clip_rect(r.shrink(4.0).intersect(p.clip_rect()));
        let max_chars = ((w - 44.0) / 7.0).max(6.0) as usize;
        clip.text(
            pos2(r.left() + 36.0, r.top() + 15.0),
            Align2::LEFT_CENTER,
            elide(&title, max_chars),
            theme::semi(12.5),
            theme::TEXT,
        );
        let sub = if step.is_ensemble() {
            names.join(" + ")
        } else {
            format!("on {}", step.input.label().to_lowercase())
        };
        let sub = if step.is_ensemble() && step.input != StepInput::Mix {
            format!("{sub} · on {}", step.input.label().to_lowercase())
        } else {
            sub
        };
        clip.text(
            pos2(r.left() + 36.0, r.top() + 32.0),
            Align2::LEFT_CENTER,
            elide(&sub, max_chars + 4),
            theme::regular(11.0),
            theme::TEXT_DIM,
        );
        if i > 0 {
            let a = pos2(x - link + 4.0, r.center().y);
            arrow(p, a, pos2(x - 4.0, r.center().y), theme::alpha(color, 0.9));
            // What travels along the link.
            if let StepInput::Stem(k) = step.input {
                p.circle_filled(
                    pos2(x - link / 2.0, r.center().y - 9.0),
                    3.0,
                    theme::stem_color(k),
                );
            }
        }
    }
}

// ---- editor ----------------------------------------------------------------------------------

/// Stems the steps before `upto` produce (for the input picker), skipping broken steps.
fn produced_before(app: &StemApp, chain: &Chain, upto: usize) -> Vec<StemKind> {
    let mut out: Vec<StemKind> = Vec::new();
    for step in chain.steps.iter().take(upto) {
        let mut kinds: Option<Vec<StemKind>> = None;
        for id in &step.models {
            if let Some(m) = model(app, id) {
                kinds = Some(match kinds {
                    None => m.stems.clone(),
                    Some(k) => k.into_iter().filter(|s| m.stems.contains(s)).collect(),
                });
            }
        }
        for k in kinds.unwrap_or_default() {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

fn model_combo(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    models: &[ModelInfo],
    value: &mut String,
) {
    let current = models
        .iter()
        .find(|m| m.id == value.as_str())
        .map_or(value.clone(), |m| m.name.to_owned());
    egui::ComboBox::from_id_salt(id)
        .selected_text(elide(&current, 30))
        .width(230.0)
        .show_ui(ui, |ui| {
            for m in models {
                let stems = m
                    .stems
                    .iter()
                    .map(|k| k.label())
                    .collect::<Vec<_>>()
                    .join(" · ");
                ui.selectable_value(value, m.id.to_owned(), m.name)
                    .on_hover_text(stems);
            }
        });
}

/// The preset editor sheet.
pub fn editor(app: &mut StemApp, ctx: &egui::Context) {
    let Some(mut ed) = app.preset_editor.take() else {
        return;
    };
    let models: Vec<ModelInfo> = app.backend.models.clone();
    let mut close = false;
    let mut save = false;
    let mut delete = false;
    let mut sheet = Sheet::begin(ctx, Id::unique("preset-editor"));
    let resp = sheet.show(ctx, theme::VIOLET, 24, |ui| {
        ui.set_width(640.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
            crate::widgets::icon_badge(ui.painter(), r, theme::VIOLET, Icon::Models);
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(if ed.existing {
                    "Edit preset"
                } else {
                    "New preset"
                })
                .font(theme::bold(22.0))
                .color(theme::TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (b, c) = kind_badge(&ed.preset.chain);
                let (r, _) = ui.allocate_exact_size(vec2(80.0, 20.0), Sense::hover());
                badge(ui.painter(), pos2(r.right(), r.top()), b, c);
            });
        });
        ui.add_space(10.0);
        let field = |ui: &mut egui::Ui, label: &str, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(96.0, 32.0), Sense::hover());
                ui.painter().text(
                    pos2(r.left(), r.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    theme::regular(13.5),
                    theme::TEXT_DIM,
                );
                add(ui);
            });
        };
        field(ui, "Name", &mut |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut ed.preset.name)
                    .font(theme::semi(14.0))
                    .desired_width(ui.available_width()),
            );
        });
        field(ui, "Notes", &mut |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut ed.preset.description)
                    .hint_text("What it's for (optional)")
                    .font(theme::regular(13.0))
                    .desired_width(ui.available_width()),
            );
        });
        ui.add_space(12.0);
        overline(ui, "Steps");
        ui.add_space(4.0);

        let mut remove_step = None;
        let n_steps = ed.preset.chain.steps.len();
        egui::ScrollArea::vertical()
            .id_salt("preset-steps")
            // Short windows: the step list scrolls so the sheet keeps its header and buttons.
            .max_height((ui.ctx().content_rect().height() - 480.0).clamp(140.0, 430.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for i in 0..n_steps {
                    let inputs = produced_before(app, &ed.preset.chain, i);
                    let step = &mut ed.preset.chain.steps[i];
                    let color = if step.is_ensemble() {
                        theme::CYAN
                    } else {
                        [theme::VIOLET, theme::PINK, theme::CYAN][i % 3]
                    };
                    let frame = egui::Frame::new()
                        .fill(theme::alpha(color, 0.06))
                        .stroke(Stroke::new(1.0, theme::alpha(color, 0.35)))
                        .corner_radius(CornerRadius::same(14))
                        .inner_margin(egui::Margin::symmetric(14, 10));
                    let shown = frame.show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(24.0, 32.0), Sense::hover());
                            let c = r.center();
                            paint::glow_circle(ui.painter(), c, 6.0, theme::alpha(color, 0.4), 8.0);
                            ui.painter().circle_filled(c, 11.0, color);
                            ui.painter().text(
                                c,
                                Align2::CENTER_CENTER,
                                format!("{}", i + 1),
                                theme::bold(12.0),
                                Color32::WHITE,
                            );
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new("Input")
                                    .font(theme::regular(12.5))
                                    .color(theme::TEXT_FAINT),
                            );
                            egui::ComboBox::from_id_salt(("step-input", i))
                                .selected_text(step.input.label())
                                .width(140.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut step.input,
                                        StepInput::Mix,
                                        "Full mix",
                                    );
                                    for k in &inputs {
                                        ui.selectable_value(
                                            &mut step.input,
                                            StepInput::Stem(*k),
                                            k.label(),
                                        );
                                    }
                                });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if n_steps > 1
                                        && buttons::icon_button(
                                            ui,
                                            Icon::Close,
                                            28.0,
                                            false,
                                            theme::BAD,
                                            "Remove step",
                                        )
                                        .clicked()
                                    {
                                        remove_step = Some(i);
                                    }
                                },
                            );
                        });
                        let mut remove_model = None;
                        let n_models = step.models.len();
                        for (mi, m) in step.models.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.add_space(30.0);
                                let (r, _) =
                                    ui.allocate_exact_size(vec2(40.0, 30.0), Sense::hover());
                                ui.painter().text(
                                    pos2(r.left(), r.center().y),
                                    Align2::LEFT_CENTER,
                                    if mi == 0 { "Model" } else { "+" },
                                    theme::regular(12.5),
                                    theme::TEXT_FAINT,
                                );
                                model_combo(ui, ("step-model", i, mi), &models, m);
                                if let Some(info) = models.iter().find(|x| x.id == m.as_str()) {
                                    for k in &info.stems {
                                        let (r, _) =
                                            ui.allocate_exact_size(vec2(8.0, 30.0), Sense::hover());
                                        ui.painter().circle_filled(
                                            r.center(),
                                            3.0,
                                            theme::stem_color(*k),
                                        );
                                    }
                                }
                                if n_models > 1
                                    && buttons::icon_button(
                                        ui,
                                        Icon::Close,
                                        24.0,
                                        false,
                                        theme::BAD,
                                        "Remove from ensemble",
                                    )
                                    .clicked()
                                {
                                    remove_model = Some(mi);
                                }
                            });
                        }
                        if let Some(mi) = remove_model {
                            step.models.remove(mi);
                        }
                        ui.horizontal(|ui| {
                            ui.add_space(70.0);
                            if Button::ghost("Ensemble model")
                                .icon(Icon::Plus)
                                .height(28.0)
                                .tooltip(
                                    "Run another model on the same input and combine the stems \
                                 both make (slower, a little cleaner)",
                                )
                                .show(ui)
                                .clicked()
                            {
                                let next = models
                                    .iter()
                                    .find(|m| !step.models.iter().any(|s| s == m.id))
                                    .map_or("htdemucs", |m| m.id);
                                step.models.push(next.to_owned());
                            }
                            if step.is_ensemble() {
                                ui.add_space(8.0);
                                let opts: Vec<(EnsembleMode, &str)> =
                                    EnsembleMode::ALL.iter().map(|m| (*m, m.label())).collect();
                                buttons::segmented(
                                    ui,
                                    &format!("ens-{i}"),
                                    &mut step.ensemble,
                                    &opts,
                                );
                                ui.label(
                                    egui::RichText::new("ⓘ")
                                        .font(theme::regular(13.0))
                                        .color(theme::TEXT_FAINT),
                                )
                                .on_hover_text(step.ensemble.hint());
                            }
                        });
                    });
                    paint::glass_sheen(
                        ui.painter(),
                        shown.response.rect,
                        CornerRadius::same(14),
                        0.0,
                    );
                    // Link to the next step.
                    if i + 1 < n_steps {
                        let r = shown.response.rect;
                        let x = r.left() + 26.0;
                        let (lr, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
                        let top = pos2(x, r.bottom());
                        let bottom = pos2(x, lr.bottom() + 2.0);
                        ui.painter()
                            .line_segment([top, bottom - vec2(0.0, 3.0)], Stroke::new(1.4, color));
                        ui.painter().add(egui::Shape::convex_polygon(
                            vec![bottom, bottom + vec2(-3.5, -5.0), bottom + vec2(3.5, -5.0)],
                            color,
                            Stroke::NONE,
                        ));
                    } else {
                        ui.add_space(6.0);
                    }
                }
            });
        if let Some(i) = remove_step {
            ed.preset.chain.steps.remove(i);
        }
        if Button::new("Add step")
            .icon(Icon::Plus)
            .height(32.0)
            .tooltip("Run another model on a stem from the steps above")
            .show(ui)
            .clicked()
        {
            let inputs = produced_before(app, &ed.preset.chain, n_steps);
            let input = [StemKind::Instrumental, StemKind::Vocals, StemKind::Drums]
                .into_iter()
                .find(|k| inputs.contains(k))
                .map_or(StepInput::Mix, StepInput::Stem);
            let model = if input == StepInput::Stem(StemKind::Instrumental) {
                "htdemucs"
            } else {
                models.first().map_or("htdemucs", |m| m.id)
            };
            ed.preset.chain.steps.push(Step::new(input, model));
        }

        ui.add_space(14.0);
        overline(ui, "Output");
        ui.add_space(4.0);
        let result = plan(app, &ed.preset.chain);
        match &result {
            Ok(p) => {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for (step, k) in &p.outputs {
                        buttons::chip(ui, k.label(), theme::stem_color(*k), true)
                            .on_hover_text(format!("from step {}", step + 1));
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "Stems a later step splits further are left out, and the first \
                             step to make a stem wins.",
                    )
                    .font(theme::regular(11.5))
                    .color(theme::TEXT_FAINT),
                );
            }
            Err(e) => {
                ui.label(
                    egui::RichText::new(e)
                        .font(theme::regular(13.0))
                        .color(theme::BAD),
                );
            }
        }
        let valid = result.is_ok() && !ed.preset.name.trim().is_empty();

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if ed.existing
                && Button::ghost("Delete")
                    .icon(Icon::Close)
                    .height(38.0)
                    .show(ui)
                    .clicked()
            {
                delete = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if Button::primary("Save & use")
                    .icon(Icon::Check)
                    .enabled(valid)
                    .height(38.0)
                    .show(ui)
                    .clicked()
                {
                    save = true;
                }
                if Button::ghost("Cancel").height(38.0).show(ui).clicked() {
                    close = true;
                }
            });
        });
    });
    if sheet.closing {
        // Exit animation playing: the sheet is going away, ignore its buttons.
        (save, delete, close) = (false, false, false);
    }
    if save {
        let name = ed.preset.name.trim().to_owned();
        ed.preset.name = name.clone();
        let id = app.settings.save_preset(ed.preset.clone());
        app.settings.preset_id = Some(id);
        app.toasts.success("Preset saved", name);
        close = true;
    } else if delete {
        app.settings.delete_preset(&ed.preset.id);
        app.toasts.info("Preset deleted", ed.preset.name.clone());
        close = true;
    }
    if close || resp.should_close() {
        sheet.close(ctx);
    }
    // The sheet keeps its state until the exit animation has played.
    if !sheet.finished() {
        app.preset_editor = Some(ed);
    }
}

// ---- settings --------------------------------------------------------------------------------

/// "Stem cache" rows for the Storage section.
pub fn cache_rows(app: &mut StemApp, ui: &mut egui::Ui) {
    super::settings::row(ui, "Stem cache", |ui| {
        buttons::toggle_labeled(ui, &mut app.settings.stem_cache, "Stem cache");
        ui.add_space(6.0);
        let size = app.stem_cache_size();
        let busy = app.job.is_some() || app.queue.has_work();
        let label = format!("Clear  ·  {}", human_bytes(size));
        if Button::new(&label)
            .icon(Icon::Close)
            .height(34.0)
            .enabled(size > 0 && !busy)
            .tooltip("Delete cached model output (your stems are kept)")
            .show(ui)
            .clicked()
        {
            app.clear_stem_cache();
        }
        ui.label(
            egui::RichText::new(if busy {
                "wait for the current split to finish"
            } else {
                "re-running a preset on the same track skips the models"
            })
            .font(theme::regular(12.0))
            .color(theme::TEXT_FAINT),
        );
    });
    super::settings::row(ui, "Cache limit", |ui| {
        ui.add_enabled_ui(app.settings.stem_cache, |ui| {
            buttons::slider(
                ui,
                &mut app.settings.stem_cache_gb,
                1.0..=50.0,
                " GB",
                240.0,
            );
        });
        let used = app.stem_cache_size() as f64 / 1e9;
        let frac = (used / app.settings.stem_cache_gb.max(0.5) as f64).clamp(0.0, 1.0) as f32;
        let (r, _) = ui.allocate_exact_size(vec2(120.0, 34.0), Sense::hover());
        let bar = Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 3.0), vec2(104.0, 6.0));
        let p = ui.painter();
        p.rect_filled(bar, CornerRadius::same(3), Color32::from_white_alpha(16));
        if frac > 0.0 {
            let fill = Rect::from_min_size(bar.min, vec2(bar.width() * frac.max(0.04), 6.0));
            paint::gradient_rect(p, fill, CornerRadius::same(3), 0.0, frac, Color32::WHITE);
        }
        ui.label(
            egui::RichText::new(format!(
                "{:.0}% used · oldest results go first",
                frac * 100.0
            ))
            .font(theme::regular(12.0))
            .color(theme::TEXT_FAINT),
        );
    });
}
