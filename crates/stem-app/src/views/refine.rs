//! Stem-card pieces for "Split drums" / "Clean vocals" (see `crate::refine`): the action pill,
//! its model menu, the in-lane progress strip, the tree line that ties sub-stems to their
//! parent, and the "NC · personal use" badge shared with the model cards.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Id, Order, Rect, Sense, Stroke, StrokeKind, pos2, vec2,
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
    let rect = Rect::from_min_size(pos2(right - w, cy - 12.0), vec2(w, 24.0));
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
            CornerRadius::same(12),
            theme::alpha(color, 0.25 * h),
            10.0,
        );
    }
    p.rect(
        rect_draw,
        CornerRadius::same(12),
        fill,
        Stroke::new(1.0, stroke),
        StrokeKind::Inside,
    );
    paint::glass_sheen(p, rect_draw, CornerRadius::same(12), h);
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
    paint::focus(p, &resp, rect_draw, CornerRadius::same(12));
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
    let h = 56.0 + offers.len().max(1) as f32 * ROW_H + if any_nc { 46.0 } else { 0.0 };
    let screen = ctx.content_rect();
    // Opens to the right of the pill, over the waveform lanes, with its header level with the
    // pill (so it never half-covers the card's other controls).
    let mut pos = pos2(anchor.right() + 12.0, anchor.center().y - 22.0);
    pos.y = pos.y.clamp(
        screen.top() + 12.0,
        (screen.bottom() - 12.0 - h).max(screen.top() + 12.0),
    );
    pos.x = pos
        .x
        .clamp(screen.left() + 12.0, screen.right() - MENU_W - 12.0);
    let enter = motion::ease_out_back(motion::appear(ctx, menu_id().with("in"), 0.0, motion::BASE));
    let mut pick = None;
    let mut close = false;
    let area = egui::Area::new(menu_id().with("area"))
        .order(Order::Foreground)
        .fixed_pos(pos + vec2(-8.0 * (1.0 - enter), 0.0))
        .show(ctx, |ui| {
            ui.set_opacity(enter.clamp(0.0, 1.0));
            let (rect, _) = ui.allocate_exact_size(vec2(MENU_W, h), Sense::click());
            let p = ui.painter();
            // Solid backing: the menu floats over busy waveforms and must read cleanly.
            p.rect_filled(rect, CornerRadius::same(16), theme::BG_PANEL);
            paint::glass(
                p,
                rect,
                CornerRadius::same(16),
                Some(theme::alpha(color, 0.16)),
                0.0,
            );
            // Little caret towards the pill, in the menu's own fill and hairline.
            let cy = anchor
                .center()
                .y
                .clamp(rect.top() + 18.0, rect.bottom() - 18.0);
            if rect.left() > anchor.right() {
                let (base_x, tip_x) = (rect.left() + 1.0, rect.left() - 6.5);
                let fill = theme::lerp_color(theme::BG_PANEL, theme::GLASS, 0.5);
                p.add(egui::Shape::convex_polygon(
                    vec![
                        pos2(base_x, cy - 7.0),
                        pos2(tip_x, cy),
                        pos2(base_x, cy + 7.0),
                    ],
                    fill,
                    Stroke::NONE,
                ));
                let edge = Stroke::new(1.0, theme::GLASS_STROKE);
                p.line_segment([pos2(base_x, cy - 7.0), pos2(tip_x, cy)], edge);
                p.line_segment([pos2(base_x, cy + 7.0), pos2(tip_x, cy)], edge);
            }
            icons::paint(
                p,
                Rect::from_center_size(
                    pos2(rect.left() + 24.0, rect.top() + 22.0),
                    vec2(13.0, 13.0),
                ),
                glyph(op),
                color,
            );
            p.text(
                pos2(rect.left() + 38.0, rect.top() + 22.0),
                Align2::LEFT_CENTER,
                match op {
                    RefineOp::DrumParts => "Split drums into parts",
                    RefineOp::CleanUp => "Clean up the vocal",
                },
                theme::semi(13.0),
                theme::TEXT,
            );
            p.text(
                pos2(rect.right() - 18.0, rect.top() + 22.0),
                Align2::RIGHT_CENTER,
                format!("on {}", kind.label().to_lowercase()),
                theme::regular(12.0),
                theme::TEXT_DIM,
            );
            p.hline(
                rect.left() + 14.0..=rect.right() - 14.0,
                rect.top() + 40.0,
                Stroke::new(1.0, theme::GLASS_STROKE),
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
                    pos2(rect.left() + 8.0, rect.top() + 46.0 + k as f32 * ROW_H),
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
                    ("Export needed".to_owned(), theme::TEXT_FAINT)
                } else if !adds {
                    ("Added".to_owned(), theme::TEXT_FAINT)
                } else if o.downloaded {
                    ("Ready".to_owned(), theme::GOOD)
                } else {
                    (
                        format!("Download · {}", human_bytes(o.model.size_bytes)),
                        theme::CYAN,
                    )
                };
                p.text(
                    pos2(row.right() - 12.0, row.top() + 17.0),
                    Align2::RIGHT_CENTER,
                    status,
                    theme::regular(12.0),
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

/// While a split runs on this card: the waveform shows how far the model has got (done part
/// in full colour, the rest dimmed, a playhead-like marker at the front), and a glass strip
/// along the lane's bottom says what runs, on what, how long it has taken and roughly how
/// long is left, with a cancel button.
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
    let frac = job.frac.map(|_| job.shown.clamp(0.0, 1.0));
    let p = ui.painter();
    // The waveform as a progress track: the part still to do sits under a dark veil.
    let front = frac.map(|f| lane.left() + lane.width() * f);
    let todo = Rect::from_min_max(pos2(front.unwrap_or(lane.left()), lane.top()), lane.max);
    p.rect_filled(
        todo,
        CornerRadius::same(10),
        Color32::from_black_alpha((120.0 * enter) as u8),
    );
    if let Some(x) = front {
        p.vline(
            x,
            lane.top() + 2.0..=lane.bottom() - 2.0,
            Stroke::new(1.5, theme::alpha(color, 0.9 * enter)),
        );
    }

    let h = 30.0;
    let strip = Rect::from_min_size(
        pos2(
            lane.left() + 8.0,
            lane.bottom() - h - 6.0 + 8.0 * (1.0 - enter),
        ),
        vec2(lane.width() - 16.0, h),
    );
    p.rect(
        strip,
        CornerRadius::same(10),
        theme::alpha(theme::BG_PANEL, 0.92 * enter),
        Stroke::new(1.0, theme::alpha(color, 0.45 * enter)),
        StrokeKind::Inside,
    );
    let cy = strip.center().y - 1.0;
    progress::ring(p, pos2(strip.left() + 16.0, cy), 6.5, 2.0, frac, t);
    let elapsed = (t - job.started).max(0.0);
    // Left: what runs.
    let title = p.layout_no_wrap(
        format!("{} · {}", job.op.doing(), job.model_name),
        theme::semi(12.5),
        theme::TEXT,
    );
    let title_w = title.size().x;
    p.galley(
        pos2(strip.left() + 30.0, cy - title.size().y / 2.0),
        title,
        theme::TEXT,
    );
    // Right: detail · elapsed · ~left · percent, mono so the numbers don't jitter.
    let mut right = Vec::new();
    if !job.detail.is_empty() {
        right.push(job.detail.clone());
    }
    right.push(fmt_elapsed(elapsed));
    if let Some(f) = frac
        && f > 0.04
    {
        let left = elapsed / f as f64 - elapsed;
        right.push(format!("~{} left", fmt_elapsed(left)));
    }
    let text_right = strip.right() - 40.0;
    let pct = frac.map(|f| format!("{:>3.0}%", f * 100.0));
    let pct_w = if let Some(pct) = pct {
        let g = p.layout_no_wrap(pct, theme::mono(12.5), theme::TEXT);
        let w = g.size().x;
        p.galley(pos2(text_right - w, cy - g.size().y / 2.0), g, theme::TEXT);
        w + 12.0
    } else {
        0.0
    };
    // Drop detail first when the lane is narrow.
    let room = text_right - pct_w - (strip.left() + 42.0 + title_w);
    let mut info = right.join("  ·  ");
    let mut g = p.layout_no_wrap(info.clone(), theme::mono(11.0), theme::TEXT_DIM);
    while g.size().x > room && right.len() > 1 {
        right.remove(0);
        info = right.join("  ·  ");
        g = p.layout_no_wrap(info.clone(), theme::mono(11.0), theme::TEXT_DIM);
    }
    if g.size().x <= room {
        p.galley(
            pos2(text_right - pct_w - g.size().x, cy - g.size().y / 2.0),
            g,
            theme::TEXT_DIM,
        );
    }
    // Bar along the strip's bottom edge, in the stem's colour.
    let bar = Rect::from_min_max(
        pos2(strip.left() + 10.0, strip.bottom() - 4.0),
        pos2(strip.right() - 10.0, strip.bottom() - 2.0),
    );
    p.rect_filled(bar, CornerRadius::same(1), Color32::from_white_alpha(14));
    match frac {
        Some(f) => {
            let fill = Rect::from_min_size(bar.min, vec2(bar.width() * f, bar.height()));
            p.rect_filled(fill, CornerRadius::same(1), color);
        }
        None => {
            // Indeterminate: a short segment sweeping across.
            let w = bar.width() * 0.25;
            let x = bar.left() - w + (bar.width() + w) * ((t * 0.6) % 1.0) as f32;
            let seg = Rect::from_min_max(
                pos2(x.max(bar.left()), bar.top()),
                pos2((x + w).min(bar.right()), bar.bottom()),
            );
            p.rect_filled(seg, CornerRadius::same(1), color);
        }
    }
    let cr = Rect::from_center_size(pos2(strip.right() - 18.0, cy + 1.0), vec2(22.0, 22.0));
    let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(cr));
    if buttons::icon_button(&mut cu, Icon::Close, 22.0, false, theme::BAD, "Cancel").clicked() {
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
/// edge `to_x`. `more` continues the line past this card for the next sibling. `draw` (0..1)
/// draws it in from the top (the entrance); the node at the card appears once it arrives.
#[allow(clippy::too_many_arguments)]
pub fn tree_line(
    p: &egui::Painter,
    x: f32,
    top: f32,
    mid: f32,
    to_x: f32,
    more: Option<f32>,
    color: Color32,
    draw: f32,
) {
    if draw <= 0.0 {
        return;
    }
    let stroke = Stroke::new(1.5, theme::alpha(color, 0.7));
    let r = 9.0;
    // The whole run as one polyline: trunk, elbow, stub.
    let mut pts = vec![pos2(x, top), pos2(x, mid - r)];
    pts.extend((1..=10).map(|k| {
        let a = std::f32::consts::PI - k as f32 / 10.0 * std::f32::consts::FRAC_PI_2;
        pos2(x + r + r * a.cos(), mid - r + r * a.sin())
    }));
    pts.push(pos2(to_x, mid));
    let total: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    let mut left = total * draw.clamp(0.0, 1.0);
    let mut drawn = vec![pts[0]];
    for w in pts.windows(2) {
        let len = (w[1] - w[0]).length();
        if left >= len {
            drawn.push(w[1]);
            left -= len;
        } else {
            drawn.push(w[0] + (w[1] - w[0]) * (left / len.max(1e-3)));
            break;
        }
    }
    p.add(egui::Shape::line(drawn, stroke));
    if draw >= 1.0 {
        let at = pos2(to_x, mid);
        p.circle_filled(at, 3.0, color);
        p.circle_stroke(at, 3.0, Stroke::new(1.0, theme::BG));
    }
    if let Some(bottom) = more {
        let end = mid - r + (bottom - (mid - r)) * draw.clamp(0.0, 1.0);
        p.line_segment([pos2(x, mid - r), pos2(x, end)], stroke);
    }
}
