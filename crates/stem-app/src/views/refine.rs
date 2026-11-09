//! Stem-card pieces for "Split drums" / "Clean vocals" (see `crate::refine`): the action pill,
//! its model menu, the in-lane progress strip, the tree line that ties sub-stems to their
//! parent, and the "NC · personal use" badge shared with the model cards.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Id, Order, Pos2, Rect, Sense, Stroke, StrokeKind, pos2,
    vec2,
};

use crate::app::StemApp;
use crate::backend::human_bytes;
use crate::pipeline::refine::RefineOp;
use crate::theme::{self, motion};
use crate::widgets::buttons;
use crate::widgets::icons::{self, Icon};
use crate::widgets::{paint, progress};

/// Amber, like other "mind this" notes in the app.
pub const NC_COLOR: Color32 = theme::WARN;
const NC_TEXT: &str = "NC · personal use";
pub const NC_TIP: &str = "Non-commercial: these weights carry a non-commercial licence or no stated \
                      licence at all. Fine for your own beats and practice; don't sell or \
                      release music made with them without checking the licence.";

/// Width of the "NC · personal use" badge at `size`.
pub fn nc_badge_width(p: &egui::Painter, size: f32) -> f32 {
    p.layout_no_wrap(NC_TEXT.into(), theme::semi(size), NC_COLOR)
        .size()
        .x
        + 2.4 * size
}

/// Paint the badge with its left edge at `left`, centred on `cy`. Returns its rect.
pub fn nc_badge(p: &egui::Painter, left: f32, cy: f32, size: f32) -> Rect {
    let g = p.layout_no_wrap(NC_TEXT.into(), theme::semi(size), NC_COLOR);
    let h = size + 9.0;
    let r = Rect::from_min_size(pos2(left, cy - h / 2.0), vec2(g.size().x + 2.4 * size, h));
    p.rect(
        r,
        CornerRadius::same((h / 2.0) as u8),
        theme::alpha(NC_COLOR, 0.12),
        Stroke::new(1.0, theme::alpha(NC_COLOR, 0.55)),
        StrokeKind::Inside,
    );
    // A small shield dot before the text.
    let dot = pos2(r.left() + 0.8 * size, cy);
    p.circle_filled(dot, 0.22 * size, NC_COLOR);
    p.galley(
        pos2(r.left() + 1.4 * size, cy - g.size().y / 2.0),
        g,
        NC_COLOR,
    );
    r
}

/// What the card's action pill shows.
pub enum PillState {
    Idle,
    /// This stem is being split: progress so far.
    Running(Option<f32>),
    /// Already split into this many sub-stems.
    Done(usize),
    /// Another split runs elsewhere.
    Busy,
}

/// The "Split drums" / "Clean vocals" pill with its right edge at `right`, centred on `cy`.
/// Returns its rect and whether it was clicked.
pub fn action_pill(
    ui: &mut egui::Ui,
    right: f32,
    cy: f32,
    i: usize,
    op: RefineOp,
    color: Color32,
    state: &PillState,
) -> (Rect, bool) {
    let label = match state {
        PillState::Running(Some(f)) => format!("{:.0}%", f * 100.0),
        PillState::Running(None) => "…".to_owned(),
        PillState::Done(n) => format!("{n} parts"),
        _ => match op {
            RefineOp::DrumParts => "Split".to_owned(),
            RefineOp::CleanUp => "Clean".to_owned(),
        },
    };
    let g = ui
        .painter()
        .layout_no_wrap(label, theme::semi(11.5), Color32::WHITE);
    let w = g.size().x + 34.0;
    let rect = Rect::from_min_size(pos2(right - w, cy - 11.0), vec2(w, 22.0));
    let id = Id::unique(("refine-pill", i));
    let clickable = matches!(state, PillState::Idle | PillState::Done(_));
    let tip = match (op, state) {
        (_, PillState::Busy) => "Another split is running".to_owned(),
        (_, PillState::Running(_)) => format!("{}…", op.doing()),
        (RefineOp::DrumParts, PillState::Done(_)) => {
            "Split into drum parts: they're in the group below".to_owned()
        }
        (RefineOp::DrumParts, _) => {
            "Split drums: kick, snare, toms, hi-hat, ride and crash as their own stems".to_owned()
        }
        (RefineOp::CleanUp, _) => {
            "Clean vocals: take off the reverb and echo (a dry vocal), or the noise".to_owned()
        }
    };
    let resp = ui
        .interact(
            rect,
            id,
            if clickable {
                Sense::click()
            } else {
                Sense::hover()
            },
        )
        .on_hover_text(tip);
    let resp = if clickable {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    };
    let h = ui
        .ctx()
        .animate_bool_with_time(id, resp.hovered(), motion::FAST);
    let pop = motion::bouncy(
        ui.ctx(),
        id.with("pop"),
        if resp.is_pointer_button_down_on() {
            0.94
        } else {
            1.0
        },
    );
    let rect_draw = Rect::from_center_size(rect.center(), rect.size() * pop);
    let p = ui.painter();
    let (fill, stroke) = match state {
        PillState::Running(_) => (theme::alpha(color, 0.18), theme::alpha(color, 0.7)),
        PillState::Busy => (Color32::from_white_alpha(6), theme::STROKE),
        PillState::Done(_) => (
            theme::lerp_color(Color32::from_white_alpha(8), theme::alpha(color, 0.16), h),
            theme::alpha(color, 0.3 + 0.3 * h),
        ),
        PillState::Idle => (
            theme::lerp_color(theme::alpha(color, 0.10), theme::alpha(color, 0.26), h),
            theme::alpha(color, 0.45 + 0.4 * h),
        ),
    };
    if h > 0.0 && clickable {
        paint::glow(
            p,
            rect_draw,
            CornerRadius::same(11),
            theme::alpha(color, 0.25 * h),
            10.0,
        );
    }
    p.rect(
        rect_draw,
        CornerRadius::same(11),
        fill,
        Stroke::new(1.0, stroke),
        StrokeKind::Inside,
    );
    paint::glass_sheen(p, rect_draw, CornerRadius::same(11), h);
    let ic = Rect::from_center_size(pos2(rect_draw.left() + 13.0, cy), vec2(12.0, 12.0));
    let t = ui.input(|inp| inp.time);
    match state {
        PillState::Running(f) => progress::ring(p, ic.center(), 5.0, 1.8, *f, t),
        PillState::Done(_) => icons::paint(p, ic, Icon::Check, color),
        PillState::Busy => icons::paint(p, ic, glyph(op), theme::TEXT_FAINT),
        PillState::Idle => icons::paint(p, ic, glyph(op), color),
    }
    let text = match state {
        PillState::Busy => theme::TEXT_FAINT,
        PillState::Done(_) => theme::TEXT_DIM,
        _ => theme::lerp_color(theme::TEXT, Color32::WHITE, h),
    };
    p.galley(
        pos2(rect_draw.left() + 23.0, cy - g.size().y / 2.0),
        g,
        text,
    );
    paint::focus(p, &resp, rect_draw, CornerRadius::same(11));
    (rect, resp.clicked() && clickable)
}

fn glyph(op: RefineOp) -> Icon {
    match op {
        RefineOp::DrumParts => Icon::Split,
        RefineOp::CleanUp => Icon::Sparkle,
    }
}

fn menu_id() -> Id {
    Id::unique("refine-menu")
}

/// Open (or close) the model menu of track `i`'s pill.
pub fn toggle_menu(ctx: &egui::Context, i: usize, anchor: Rect) {
    let open = ctx.data(|d| d.get_temp::<(usize, Rect)>(menu_id()));
    ctx.data_mut(|d| {
        if open.is_some_and(|(j, _)| j == i) {
            d.remove::<(usize, Rect)>(menu_id());
        } else {
            d.insert_temp(menu_id(), (i, anchor));
        }
    });
}

/// Open the menu without a click (screenshot scenes).
pub fn open_menu(ctx: &egui::Context, i: usize, anchor: Rect) {
    ctx.data_mut(|d| d.insert_temp(menu_id(), (i, anchor)));
}

/// Keep an open menu's anchor in step with the card it hangs from (it scrolls).
pub fn track_anchor(ctx: &egui::Context, i: usize, anchor: Rect) {
    if let Some((j, _)) = ctx.data(|d| d.get_temp::<(usize, Rect)>(menu_id()))
        && j == i
    {
        ctx.data_mut(|d| d.insert_temp(menu_id(), (i, anchor)));
    }
}

const MENU_W: f32 = 380.0;
const ROW_H: f32 = 58.0;

/// The model menu, if one is open: a glass panel under the pill listing the models for its op.
/// Picking a ready model starts the run.
pub fn menu(app: &mut StemApp, ctx: &egui::Context) {
    let Some((i, anchor)) = ctx.data(|d| d.get_temp::<(usize, Rect)>(menu_id())) else {
        return;
    };
    let Some((kind, op)) = app
        .session
        .as_ref()
        .and_then(|s| s.tracks.get(i))
        .and_then(|t| RefineOp::for_stem(t.kind).map(|op| (t.kind, op)))
    else {
        ctx.data_mut(|d| d.remove::<(usize, Rect)>(menu_id()));
        return;
    };
    let offers = app.refine_offers(op);
    let have = app.session.as_ref().map(|s| s.kinds()).unwrap_or_default();
    let color = theme::stem_color(kind);
    let any_nc = offers.iter().any(|o| o.model.non_commercial);
    let h = 52.0 + offers.len().max(1) as f32 * ROW_H + if any_nc { 46.0 } else { 0.0 };
    let screen = ctx.content_rect();
    // Hangs from the pill's left edge, over the waveform lanes.
    let mut pos = pos2(anchor.left() - 14.0, anchor.bottom() + 8.0);
    if pos.y + h > screen.bottom() - 12.0 {
        pos.y = anchor.top() - 8.0 - h;
    }
    pos.x = pos
        .x
        .clamp(screen.left() + 12.0, screen.right() - MENU_W - 12.0);
    let enter = motion::ease_out_back(motion::appear(ctx, menu_id().with("in"), 0.0, motion::BASE));
    let mut pick = None;
    let mut close = false;
    let area = egui::Area::new(menu_id().with("area"))
        .order(Order::Foreground)
        .fixed_pos(pos + vec2(0.0, -8.0 * (1.0 - enter)))
        .show(ctx, |ui| {
            ui.set_opacity(enter.clamp(0.0, 1.0));
            let (rect, _) = ui.allocate_exact_size(vec2(MENU_W, h), Sense::click());
            let p = ui.painter();
            paint::glass(
                p,
                rect,
                CornerRadius::same(16),
                Some(theme::alpha(color, 0.16)),
                0.0,
            );
            // Little caret towards the pill.
            let cx = (anchor.center().x).clamp(rect.left() + 24.0, rect.right() - 24.0);
            if pos.y > anchor.bottom() {
                p.add(egui::Shape::convex_polygon(
                    vec![
                        pos2(cx - 7.0, rect.top() + 0.5),
                        pos2(cx + 7.0, rect.top() + 0.5),
                        pos2(cx, rect.top() - 6.5),
                    ],
                    theme::GLASS,
                    Stroke::NONE,
                ));
            }
            p.text(
                pos2(rect.left() + 18.0, rect.top() + 22.0),
                Align2::LEFT_CENTER,
                match op {
                    RefineOp::DrumParts => "SPLIT DRUMS INTO PARTS",
                    RefineOp::CleanUp => "CLEAN UP THE VOCAL",
                },
                theme::semi(10.5),
                theme::TEXT_FAINT,
            );
            p.text(
                pos2(rect.right() - 18.0, rect.top() + 22.0),
                Align2::RIGHT_CENTER,
                format!("on {}", kind.label().to_lowercase()),
                theme::mono(11.0),
                color,
            );
            if offers.is_empty() {
                p.text(
                    pos2(rect.left() + 18.0, rect.top() + 60.0),
                    Align2::LEFT_CENTER,
                    "No model for this yet",
                    theme::regular(13.0),
                    theme::TEXT_DIM,
                );
            }
            for (k, o) in offers.iter().enumerate() {
                let row = Rect::from_min_size(
                    pos2(rect.left() + 8.0, rect.top() + 40.0 + k as f32 * ROW_H),
                    vec2(MENU_W - 16.0, ROW_H - 4.0),
                );
                let adds = o.model.stems.iter().any(|s| !have.contains(s));
                let usable = o.blocked.is_none() && adds;
                let rid = menu_id().with(("row", k));
                let resp = ui.interact(
                    row,
                    rid,
                    if usable {
                        Sense::click()
                    } else {
                        Sense::hover()
                    },
                );
                let resp = match (&o.blocked, adds) {
                    (Some(why), _) => resp.on_hover_text(why.as_str()),
                    (None, false) => resp.on_hover_text("Those stems are already here"),
                    (None, true) => resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(format!("License: {}", o.model.license)),
                };
                let hv =
                    ui.ctx()
                        .animate_bool_with_time(rid, resp.hovered() && usable, motion::FAST);
                let p = ui.painter();
                if hv > 0.0 {
                    p.rect_filled(row, CornerRadius::same(10), theme::alpha(color, 0.10 * hv));
                    if let Some(ptr) = resp.hover_pos() {
                        paint::spotlight(p, row, ptr, color, hv);
                    }
                }
                let dim = if usable { 1.0 } else { 0.45 };
                let name_c = theme::alpha(theme::TEXT, dim);
                let name = crate::pipeline::chain::short_model_name(o.model.name);
                let ng = p.layout_no_wrap(name, theme::semi(13.5), name_c);
                let name_w = ng.size().x;
                p.galley(pos2(row.left() + 12.0, row.top() + 9.0), ng, name_c);
                if o.model.non_commercial {
                    nc_badge(p, row.left() + 20.0 + name_w, row.top() + 17.0, 9.5);
                }
                // The stems it makes, as colour dots and names.
                let mut x = row.left() + 12.0;
                let y = row.top() + 38.0;
                for s in &o.model.stems {
                    let c = theme::alpha(theme::stem_color(*s), dim);
                    p.circle_filled(pos2(x + 3.0, y), 3.0, c);
                    let g = p.layout_no_wrap(
                        s.label().to_owned(),
                        theme::regular(11.0),
                        theme::alpha(theme::TEXT_DIM, dim),
                    );
                    let w = g.size().x;
                    p.galley(pos2(x + 10.0, y - g.size().y / 2.0), g, theme::TEXT_DIM);
                    x += w + 20.0;
                }
                // Status on the right.
                let (status, sc) = if o.blocked.is_some() {
                    ("export needed".to_owned(), theme::TEXT_FAINT)
                } else if !adds {
                    ("added".to_owned(), theme::TEXT_FAINT)
                } else if o.downloaded {
                    ("ready".to_owned(), theme::GOOD)
                } else {
                    (
                        format!("↓ {}", human_bytes(o.model.size_bytes)),
                        theme::CYAN,
                    )
                };
                p.text(
                    pos2(row.right() - 12.0, row.top() + 17.0),
                    Align2::RIGHT_CENTER,
                    status,
                    theme::mono(11.0),
                    sc,
                );
                if resp.clicked() && usable {
                    pick = Some(o.model.id);
                }
            }
            if any_nc {
                let note = Rect::from_min_max(
                    pos2(rect.left() + 18.0, rect.bottom() - 44.0),
                    pos2(rect.right() - 18.0, rect.bottom() - 10.0),
                );
                let p = ui.painter();
                p.hline(
                    note.x_range(),
                    note.top() - 4.0,
                    Stroke::new(1.0, theme::GLASS_STROKE),
                );
                let mut job = egui::text::LayoutJob::simple(
                    "NC = personal use: non-commercial or unstated licence terms. Nothing \
                     downloads until you pick a model."
                        .into(),
                    theme::regular(11.0),
                    theme::TEXT_FAINT,
                    note.width(),
                );
                job.wrap.max_rows = 2;
                let g = p.fonts_mut(|f| f.layout_job(job));
                p.galley(note.min + vec2(0.0, 4.0), g, theme::TEXT_FAINT);
            }
        });
    // Click outside (or Escape) closes it.
    let clicked_out = ctx.input(|inp| inp.pointer.any_click())
        && ctx
            .pointer_interact_pos()
            .is_some_and(|ptr| !area.response.rect.contains(ptr) && !anchor.contains(ptr));
    if clicked_out || ctx.input(|inp| inp.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if let Some(id) = pick {
        close = true;
        let now = ctx.input(|inp| inp.time);
        app.start_refine(i, id, now);
    }
    if close {
        ctx.data_mut(|d| d.remove::<(usize, Rect)>(menu_id()));
    }
}

/// While a split runs on this card: a glass strip along the bottom of its waveform lane with
/// the step, a progress bar and a cancel button.
pub fn progress_strip(app: &mut StemApp, ui: &mut egui::Ui, lane: Rect, i: usize, color: Color32) {
    let Some(job) = app.refine.as_mut().filter(|j| j.track == i) else {
        return;
    };
    let t = ui.input(|inp| inp.time);
    let dt = ui.input(|inp| inp.stable_dt).min(0.1);
    if let Some(f) = job.frac {
        job.shown += (f - job.shown) * (1.0 - (-dt * 8.0).exp());
    }
    let enter = motion::ease_out(motion::appear(
        ui.ctx(),
        Id::unique(("refine-strip", i)),
        0.0,
        motion::SLOW,
    ));
    let h = 34.0;
    let strip = Rect::from_min_size(
        pos2(
            lane.left() + 8.0,
            lane.bottom() - h - 6.0 + 10.0 * (1.0 - enter),
        ),
        vec2(lane.width() - 16.0, h),
    );
    let p = ui.painter();
    // Dim the waveform under it a little, so the strip reads.
    p.rect_filled(
        lane,
        CornerRadius::same(10),
        Color32::from_black_alpha((70.0 * enter) as u8),
    );
    paint::glass(
        p,
        strip,
        CornerRadius::same(12),
        Some(theme::alpha(color, 0.18 * enter)),
        0.0,
    );
    progress::ring(
        p,
        pos2(strip.left() + 18.0, strip.center().y),
        7.0,
        2.2,
        job.frac.map(|_| job.shown),
        t,
    );
    let elapsed = (t - job.started).max(0.0);
    let pct = job
        .frac
        .map_or(String::new(), |_| format!("  {:.0}%", job.shown * 100.0));
    p.text(
        pos2(strip.left() + 34.0, strip.center().y),
        Align2::LEFT_CENTER,
        format!("{} · {}", job.op.doing(), job.model_name),
        theme::semi(12.5),
        theme::TEXT,
    );
    let right_text = format!("{}{pct}", fmt_elapsed(elapsed));
    let detail = if job.detail.is_empty() {
        String::new()
    } else {
        format!("{}   ", job.detail)
    };
    p.text(
        pos2(strip.right() - 44.0, strip.center().y),
        Align2::RIGHT_CENTER,
        format!("{detail}{right_text}"),
        theme::mono(11.0),
        theme::TEXT_DIM,
    );
    // Thin bar along the strip's bottom edge.
    let bar = Rect::from_min_max(
        pos2(strip.left() + 12.0, strip.bottom() - 4.0),
        pos2(strip.right() - 12.0, strip.bottom() - 2.0),
    );
    progress::bar(p, bar, job.frac.map(|_| job.shown), t);
    let cr = Rect::from_center_size(
        pos2(strip.right() - 20.0, strip.center().y),
        vec2(24.0, 24.0),
    );
    let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(cr));
    if buttons::icon_button(&mut cu, Icon::Close, 24.0, false, theme::BAD, "Cancel").clicked() {
        app.cancel_refine();
    }
    ui.ctx().request_repaint();
}

fn fmt_elapsed(s: f64) -> String {
    let s = s as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Tree line from a parent card down to a sub-stem card: a vertical run at `x` from `top`
/// (the gap under the card above) to `mid`, a rounded elbow, and a stub to the card's left
/// edge `to_x`. `more` continues the line past this card for the next sibling.
pub fn tree_line(
    p: &egui::Painter,
    x: f32,
    top: f32,
    mid: f32,
    to_x: f32,
    more: Option<f32>,
    color: Color32,
) {
    let stroke = Stroke::new(1.5, theme::alpha(color, 0.55));
    let r = 8.0;
    p.line_segment([pos2(x, top), pos2(x, mid - r)], stroke);
    let elbow: Vec<Pos2> = (0..=8)
        .map(|k| {
            let a = std::f32::consts::PI - k as f32 / 8.0 * std::f32::consts::FRAC_PI_2;
            pos2(x + r + r * a.cos(), mid - r + r * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(elbow, stroke));
    p.line_segment([pos2(x + r, mid), pos2(to_x, mid)], stroke);
    p.circle_filled(pos2(to_x, mid), 2.5, theme::alpha(color, 0.9));
    if let Some(bottom) = more {
        p.line_segment([pos2(x, mid - r), pos2(x, bottom)], stroke);
    }
}
