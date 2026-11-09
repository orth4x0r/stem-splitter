//! Chop mode: a slicing toolbar, the chosen stem's waveform with numbered, colour-coded
//! slices and draggable markers, an MPC-style 4×4 pad grid you can finger-drum from the
//! keyboard, and kit export (numbered files + SFZ / Decent Sampler).

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Mesh, Rect, Sense, Shape, Stroke,
    StrokeKind, pos2, vec2,
};
use stem_models::StemKind;

use crate::app::StemApp;
use crate::backend::mmss_ms;
use crate::chop::{ChopMode, GridChoice, PAD_KEYS, PADS};
use crate::chop_export::{self, ChopExport};
use crate::theme::{self, motion};
use crate::widgets::buttons::{self, Button};
use crate::widgets::drag_handle;
use crate::widgets::icons::Icon;
use crate::widgets::pads::{self, PadEvent, PadInfo, slice_color};
use crate::widgets::paint;

const PAD_CARD_W: f32 = 404.0;
const GAP: f32 = 16.0;
const TOOLBAR_H: f32 = 104.0;
/// Extra toolbar height on narrow windows (a third row for the region picker).
const NARROW_ROW: f32 = 46.0;
const COLUMNS_H: f32 = 440.0;
const EXPORT_H: f32 = 108.0;
/// Pixels within which the pointer grabs a marker.
const GRAB: f32 = 6.0;

/// Things the view wants done after it has released its borrows.
#[derive(Default)]
struct Intents {
    trigger: Vec<(usize, f32)>,
    release: Vec<usize>,
    drag_pad: Option<usize>,
    drag_folder: bool,
    export: bool,
    open_folder: bool,
    toast: Option<(&'static str, &'static str)>,
    /// Export button centre, for the micro-burst when the export finishes.
    anchor: Option<egui::Pos2>,
}

pub fn page(app: &mut StemApp, ui: &mut egui::Ui) {
    let now = ui.input(|i| i.time);
    {
        let Some(s) = &mut app.session else { return };
        let kinds: Vec<StemKind> = s.tracks.iter().map(|t| t.kind).collect();
        let grid = s.analysis.as_ref().and_then(|a| a.grid.as_ref());
        s.chop.update(&kinds, &s.audio, grid, s.duration);
    }
    let mut it = Intents::default();
    keyboard(app, ui, &mut it);

    let full_w = ui.available_width();
    // Narrow windows: the region picker gets a third toolbar row instead of colliding.
    let toolbar_h = if full_w < 1080.0 {
        TOOLBAR_H + NARROW_ROW
    } else {
        TOOLBAR_H
    };
    let (area, _) =
        ui.allocate_exact_size(vec2(full_w, toolbar_h + GAP + COLUMNS_H), Sense::hover());
    let toolbar = Rect::from_min_size(area.min, vec2(full_w, toolbar_h));
    let cols_top = toolbar.bottom() + GAP;
    let left_w = full_w - PAD_CARD_W - GAP;
    let export = Rect::from_min_size(
        pos2(area.left(), area.bottom() - EXPORT_H),
        vec2(left_w, EXPORT_H),
    );
    let lane = Rect::from_min_max(
        pos2(area.left(), cols_top),
        pos2(area.left() + left_w, export.top() - GAP),
    );
    let pads_rect = Rect::from_min_max(pos2(area.right() - PAD_CARD_W, cols_top), area.max);

    // Cards rise and fade in, staggered, when chop mode opens.
    let ctx = ui.ctx().clone();
    let staged = |ui: &mut egui::Ui, i: usize, rect: Rect| -> (egui::Ui, Rect) {
        let e = motion::ease_out(motion::appear(
            &ctx,
            Id::unique(("chop-card-in", i)),
            i as f32 * motion::STAGGER * 1.5,
            motion::ENTER,
        ));
        let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(area));
        if e < 1.0 {
            cu.multiply_opacity(e);
        }
        (cu, rect.translate(vec2(0.0, 18.0 * (1.0 - e))))
    };
    let (mut cu, r) = staged(ui, 0, toolbar);
    toolbar_card(app, &mut cu, r, &mut it);
    let (mut cu, r) = staged(ui, 1, lane);
    lane_card(app, &mut cu, r, now, &mut it);
    let (mut cu, r) = staged(ui, 3, export);
    export_card(app, &mut cu, r, &mut it);
    let (mut cu, r) = staged(ui, 2, pads_rect);
    pads_card(app, &mut cu, r, now, &mut it);

    ui.add_space(10.0);
    ui.label(
        egui::RichText::new(
            "Click a slice to hear it  ·  double-click to add a cut  ·  drag a cut to move it, right-click to remove  ·  Ctrl+wheel to zoom",
        )
        .font(theme::regular(12.5))
        .color(theme::TEXT_FAINT),
    );
    apply(app, ui, it, now);
}

// ---- keyboard ------------------------------------------------------------------------------

fn keyboard(app: &mut StemApp, ui: &egui::Ui, it: &mut Intents) {
    if ui.ctx().egui_wants_keyboard_input() {
        return;
    }
    let Some(s) = &mut app.session else { return };
    let page = s.chop.settings.page;
    let pages = s.chop.pages();
    ui.input(|i| {
        for ev in &i.events {
            let egui::Event::Key {
                key,
                pressed,
                repeat: false,
                modifiers,
                ..
            } = ev
            else {
                continue;
            };
            if let Some(n) = PAD_KEYS.iter().position(|(k, _)| k == key) {
                let slice = page * PADS + n;
                if *pressed {
                    // Shift plays a softer hit.
                    it.trigger
                        .push((slice, if modifiers.shift { 0.55 } else { 1.0 }));
                } else {
                    it.release.push(slice);
                }
            } else if *pressed {
                match key {
                    egui::Key::Escape => s.chop.stop_all(),
                    egui::Key::OpenBracket if page > 0 => s.chop.settings.page -= 1,
                    egui::Key::CloseBracket if page + 1 < pages => s.chop.settings.page += 1,
                    _ => {}
                }
            }
        }
    });
}

// ---- cards ---------------------------------------------------------------------------------

/// A glass card with a faint rim in `accent` and a short accent light along the top edge
/// (the same surface as the rest of the app).
fn card(ui: &egui::Ui, rect: Rect, accent: Color32) {
    let p = ui.painter();
    paint::glass(
        p,
        rect,
        CornerRadius::same(theme::R_CARD),
        Some(theme::alpha(accent, 0.07)),
        0.0,
    );
    let w = (rect.width() * 0.45).min(320.0);
    paint::edge_light(
        p,
        rect,
        Some(rect.left() + 20.0 + w / 2.0),
        w,
        theme::alpha(accent, 0.8),
    );
}

fn label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .font(theme::semi(10.5))
            .color(theme::TEXT_FAINT),
    );
}

#[derive(Clone, Copy, PartialEq)]
enum RegionChoice {
    Whole,
    Selection,
}

fn toolbar_card(app: &mut StemApp, ui: &mut egui::Ui, rect: Rect, it: &mut Intents) {
    let Some(s) = &mut app.session else { return };
    let color = s
        .tracks
        .get(s.chop.track)
        .map_or(theme::VIOLET, |t| theme::stem_color(t.kind));
    card(ui, rect, color);
    let mut cui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(20.0, 14.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let ui = &mut cui;
    ui.spacing_mut().item_spacing = vec2(8.0, 12.0);
    let has_grid = s.analysis.as_ref().is_some_and(|a| a.grid.is_some());
    let narrow = rect.height() > TOOLBAR_H + 1.0;
    let selection = s.selection;
    let chop = &mut s.chop;
    let mut changed = false;
    let mut retarget = false;
    // Row 1: stem, mode, region.
    ui.horizontal(|ui| {
        label(ui, "Stem");
        for (i, t) in s.tracks.iter().enumerate() {
            if buttons::chip(
                ui,
                t.kind.label(),
                theme::stem_color(t.kind),
                i == chop.track,
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
                && i != chop.track
            {
                chop.settings.stem = Some(t.kind);
                retarget = true;
            }
        }
        ui.add_space(14.0);
        label(ui, "Chop by");
        let opts: Vec<(ChopMode, &str)> = ChopMode::ALL.iter().map(|m| (*m, m.label())).collect();
        if buttons::segmented(ui, "chop-mode", &mut chop.settings.mode, &opts) {
            changed = true;
        }
        if !narrow {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                retarget |= region_picker(ui, chop, selection, it);
                label(ui, "Region");
            });
        }
    });
    // Row 2: the mode's settings, slice count.
    ui.horizontal(|ui| {
        ui.set_min_height(30.0);
        match chop.settings.mode {
            ChopMode::Transients => {
                label(ui, "Sensitivity");
                changed |= buttons::slider(ui, &mut chop.settings.sensitivity, 0.0..=100.0, "%", 170.0);
                ui.add_space(12.0);
                label(ui, "Min gap");
                changed |=
                    buttons::slider(ui, &mut chop.settings.min_gap_ms, 20.0..=500.0, " ms", 150.0);
            }
            ChopMode::Grid if has_grid => {
                label(ui, "Every");
                let opts: Vec<(GridChoice, &str)> =
                    GridChoice::ALL.iter().map(|g| (*g, g.label())).collect();
                changed |= buttons::segmented(ui, "chop-grid", &mut chop.settings.grid, &opts);
            }
            ChopMode::Grid | ChopMode::Equal => {
                if chop.settings.mode == ChopMode::Grid {
                    ui.label(
                        egui::RichText::new("No beat grid for this track: equal parts instead.")
                            .font(theme::regular(12.5))
                            .color(theme::WARN),
                    );
                }
                label(ui, "Parts");
                let opts = [
                    (4u32, "4"),
                    (8, "8"),
                    (12, "12"),
                    (16, "16"),
                    (24, "24"),
                    (32, "32"),
                ];
                changed |= buttons::segmented(ui, "chop-parts", &mut chop.settings.parts, &opts);
            }
            ChopMode::Manual => {
                ui.label(
                    egui::RichText::new(
                        "Hand-placed cuts. Double-click the waveform to add one, drag to move, right-click to remove.",
                    )
                    .font(theme::regular(13.0))
                    .color(theme::TEXT_DIM),
                );
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let n = chop.slices.len();
            buttons::chip(
                ui,
                &format!("{n} slice{}", if n == 1 { "" } else { "s" }),
                color,
                true,
            );
            if n >= crate::chop::MAX_SLICES {
                ui.label(
                    egui::RichText::new("max")
                        .font(theme::regular(12.0))
                        .color(theme::TEXT_FAINT),
                )
                .on_hover_text("Chop a selection, or lower the sensitivity, for fewer slices.");
            }
        });
    });
    if narrow {
        ui.horizontal(|ui| {
            ui.set_min_height(30.0);
            label(ui, "Region");
            retarget |= region_picker(ui, chop, selection, it);
        });
    }
    if retarget {
        chop.retarget();
    } else if changed {
        chop.changed();
    }
}

/// Whole stem / Selection picker. True when the region changed (retarget the chop).
fn region_picker(
    ui: &mut egui::Ui,
    chop: &mut crate::chop::ChopState,
    selection: Option<(f64, f64)>,
    it: &mut Intents,
) -> bool {
    let mut region = if chop.settings.region.is_some() {
        RegionChoice::Selection
    } else {
        RegionChoice::Whole
    };
    if !buttons::segmented(
        ui,
        "chop-region",
        &mut region,
        &[
            (RegionChoice::Whole, "Whole stem"),
            (RegionChoice::Selection, "Selection"),
        ],
    ) {
        return false;
    }
    match region {
        RegionChoice::Whole => chop.settings.region = None,
        RegionChoice::Selection => match selection {
            Some(sel) => chop.settings.region = Some(sel),
            None => {
                it.toast = Some((
                    "Select a region first",
                    "Switch to Stems and drag across a waveform, then come back.",
                ))
            }
        },
    }
    true
}

fn lane_card(app: &mut StemApp, ui: &mut egui::Ui, rect: Rect, now: f64, it: &mut Intents) {
    let snap_mode = app.settings.snap;
    let Some(s) = &mut app.session else { return };
    let Some(track) = s.tracks.get(s.chop.track) else {
        return;
    };
    let color = theme::stem_color(track.kind);
    card(ui, rect, color);
    let inner = rect.shrink2(vec2(18.0, 14.0));
    let dur = s.duration.max(1e-6);
    let (ra, rb) = s.chop.region(dur);
    let (va, vb) = s.chop.view.unwrap_or((ra, rb));
    let sr = s
        .audio
        .get(s.chop.track)
        .map_or(44_100.0, |a| a.sample_rate as f64);

    // Header line.
    {
        let p = ui.painter();
        p.text(
            inner.left_top(),
            Align2::LEFT_TOP,
            track.kind.label(),
            theme::semi(16.0),
            theme::TEXT,
        );
        let mode = if s.chop.settings.mode == ChopMode::Manual {
            "hand-placed cuts".to_owned()
        } else {
            format!("chopped by {}", s.chop.settings.mode.label().to_lowercase())
        };
        p.text(
            inner.left_top() + vec2(64.0, 3.0),
            Align2::LEFT_TOP,
            format!("{} → {}  ·  {mode}", mmss_ms(ra), mmss_ms(rb)),
            theme::mono(11.5),
            theme::TEXT_FAINT,
        );
        if (vb - va) < (rb - ra) * 0.999 {
            p.text(
                inner.right_top(),
                Align2::RIGHT_TOP,
                format!("{:.1}× zoom", (rb - ra) / (vb - va)),
                theme::mono(11.0),
                theme::TEXT_FAINT,
            );
        }
    }
    let wave = Rect::from_min_max(inner.min + vec2(0.0, 30.0), inner.max);

    // Zoom / scroll inside the region with the shared timeline logic.
    let mut tl = s.timeline.clone();
    tl.start = va;
    tl.end = vb;
    tl.handle_input(ui, wave, wave.left(), wave.right());
    let (mut va, mut vb) = (tl.start, tl.end);
    if vb - va < 0.05 {
        (va, vb) = (ra, rb);
    }
    let span = (vb - va).max(1e-6);
    let to_x = |t: f64| wave.left() + ((t - va) / span) as f32 * wave.width();
    let to_t = |x: f32| (va + ((x - wave.left()) / wave.width()) as f64 * span).clamp(ra, rb);

    let chop = &mut s.chop;
    let resp = ui.interact(wave, Id::unique("chop-lane"), Sense::click_and_drag());
    let hover_pos = resp.hover_pos();
    let markers: Vec<f64> = chop.settings.markers.clone();
    // Marker under the pointer (by pixels), only markers inside the region count.
    let near_marker = |x: f32| -> Option<usize> {
        markers
            .iter()
            .enumerate()
            .filter(|(_, t)| **t > ra && **t < rb)
            .map(|(i, t)| (i, (to_x(*t) - x).abs()))
            .filter(|(_, d)| *d <= GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    let cut = chop.slices.clone();
    let slice_at = |t: f64| -> Option<usize> {
        let f = (t * sr) as usize;
        cut.iter().position(|sl| f >= sl.start && f < sl.end)
    };
    let free = ui.input(|i| i.modifiers.alt);
    let grid = s.analysis.as_ref().and_then(|a| a.grid.as_ref());
    let snap = |t: f64| {
        if free {
            t
        } else {
            crate::analysis::snap(grid, snap_mode, t, dur).clamp(ra, rb)
        }
    };

    // ---- interaction
    let grab = hover_pos.and_then(|p| near_marker(p.x));
    if chop.dragging.is_some() || grab.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    } else if hover_pos.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if resp.drag_started()
        && let Some(pos) = ui.input(|i| i.pointer.press_origin())
        && let Some(i) = near_marker(pos.x)
    {
        chop.dragging = Some(i);
    }
    if let Some(i) = chop.dragging {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = snap(to_t(pos.x));
            chop.dragging = Some(chop.move_marker(i, t));
        }
        if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            chop.end_drag();
        }
    }
    if resp.double_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        if near_marker(pos.x).is_none() {
            let t = snap(to_t(pos.x));
            if t > ra && t < rb {
                chop.add_marker(t);
            }
        }
    } else if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && near_marker(pos.x).is_none()
        && let Some(i) = slice_at(to_t(pos.x))
    {
        it.trigger.push((i, 1.0));
    }
    if resp.secondary_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some(i) = near_marker(pos.x)
    {
        chop.remove_marker(i);
    }
    let lane_hover = hover_pos.and_then(|p| slice_at(to_t(p.x)));
    if hover_pos.is_some() {
        chop.hover = lane_hover;
    }
    // Store the zoom (an un-zoomed view is just the region).
    chop.view = if (vb - va) < (rb - ra) * 0.999 {
        Some((va, vb))
    } else {
        None
    };

    // ---- painting
    let p = ui.painter_at(wave.expand(2.0));
    p.rect_filled(
        wave,
        CornerRadius::same(10),
        Color32::from_rgba_unmultiplied(0, 0, 0, 90),
    );
    if let Some(g) = grid {
        crate::timeline::paint_grid(&p, wave, &tl_view(&tl, va, vb), g, color);
    }
    let mid = wave.center().y;
    let half = wave.height() * 0.5 - 18.0;
    let slices = &chop.slices;
    let sec = |f: usize| f as f64 / sr;

    // Slice backgrounds.
    for (i, sl) in slices.iter().enumerate() {
        let (x0, x1) = (to_x(sec(sl.start)), to_x(sec(sl.end)));
        if x1 < wave.left() || x0 > wave.right() {
            continue;
        }
        let c = slice_color(i);
        let hit = chop.hits.get(i).copied().unwrap_or_default();
        let age = (now - hit.at) as f32;
        let flash = if age.is_finite() && age >= 0.0 {
            hit.velocity * (-age * 5.0).exp()
        } else {
            0.0
        };
        let hov = chop.hover == Some(i);
        let a = 0.035
            + if i % 2 == 0 { 0.0 } else { 0.025 }
            + if hov { 0.08 } else { 0.0 }
            + 0.25 * flash;
        let r = Rect::from_x_y_ranges(x0.max(wave.left())..=x1.min(wave.right()), wave.y_range());
        p.rect_filled(r, CornerRadius::ZERO, theme::alpha(c, a));
        // Playback progress.
        let len = sec(sl.len());
        let el = now - hit.at;
        if el >= 0.0 && el < len {
            let px = to_x(sec(sl.start) + el);
            p.vline(px, wave.y_range(), Stroke::new(1.5, Color32::WHITE));
        }
    }
    // Outside the region.
    for (a, b) in [(va, ra), (rb, vb)] {
        if b > a {
            let r = Rect::from_x_y_ranges(to_x(a)..=to_x(b), wave.y_range());
            p.rect_filled(
                r,
                CornerRadius::ZERO,
                Color32::from_rgba_unmultiplied(0, 0, 0, 120),
            );
        }
    }

    // Waveform, coloured by slice.
    let wave_src = crate::timeline::WaveSource {
        env: &track.env,
        pyramid: tl.pyramid(chop.track),
        audio: s.audio.get(chop.track),
    };
    let scale = 0.92 / track.env.peak().max(0.08);
    let cols = (wave.width() / 2.0).floor().max(1.0) as usize;
    let mut mesh = Mesh::default();
    let mut si = 0usize;
    for c in 0..cols {
        let f0 = c as f64 / cols as f64;
        let f1 = (c + 1) as f64 / cols as f64;
        let (t0, t1) = (va + f0 * span, va + f1 * span);
        let (lo, hi, rms) = wave_src.range(t0, t1, dur);
        let x = wave.left() + f0 as f32 * wave.width();
        let fr = (t0 * sr) as usize;
        while si + 1 < slices.len() && fr >= slices[si].end {
            si += 1;
        }
        let inside = t0 >= ra && t1 <= rb + 1e-9;
        let base = if inside && !slices.is_empty() {
            slice_color(si)
        } else {
            Color32::from_rgb(90, 90, 110)
        };
        let bright = chop.hover == Some(si) && inside;
        let top = mid - (hi * scale).min(1.0) * half;
        let bot = mid - (lo * scale).max(-1.0) * half;
        mesh.add_colored_rect(
            Rect::from_min_max(pos2(x, top), pos2(x + 1.4, bot.max(top + 1.0))),
            theme::alpha(base, if bright { 0.6 } else { 0.38 }),
        );
        let rh = (rms * scale * 1.2).min(1.0) * half;
        mesh.add_colored_rect(
            Rect::from_min_max(pos2(x, mid - rh), pos2(x + 1.4, mid + rh.max(0.5))),
            if bright {
                theme::lerp_color(base, Color32::WHITE, 0.3)
            } else {
                theme::alpha(base, 0.85)
            },
        );
    }
    p.add(Shape::mesh(mesh));

    // Cut lines with numbered flags, and pad names along the bottom.
    for (i, sl) in slices.iter().enumerate() {
        let x = to_x(sec(sl.start));
        let c = slice_color(i);
        if x >= wave.left() - 1.0 && x <= wave.right() {
            let dragged = chop.dragging.is_some()
                && i > 0
                && chop
                    .dragging
                    .and_then(|d| chop.settings.markers.get(d))
                    .is_some_and(|t| (to_x(*t) - x).abs() < 2.0);
            let hot = grab.is_some_and(|g| {
                chop.settings
                    .markers
                    .get(g)
                    .is_some_and(|t| (to_x(*t) - x).abs() < 2.0)
            }) || dragged;
            if i > 0 {
                if hot {
                    paint::glow(
                        &p,
                        Rect::from_x_y_ranges(x - 1.0..=x + 1.0, wave.y_range()),
                        CornerRadius::ZERO,
                        theme::alpha(c, 0.6),
                        10.0,
                    );
                }
                p.vline(
                    x,
                    wave.y_range(),
                    Stroke::new(if hot { 2.0 } else { 1.2 }, theme::alpha(c, 0.9)),
                );
            }
            let w = (to_x(sec(sl.end)) - x).max(0.0);
            if w > 14.0 || i == 0 {
                let label = format!("{}", i + 1);
                let g = p.layout_no_wrap(label, theme::bold(10.5), Color32::BLACK);
                let flag = Rect::from_min_size(pos2(x, wave.top()), vec2(g.size().x + 10.0, 16.0));
                p.rect_filled(
                    flag,
                    CornerRadius {
                        nw: 0,
                        ne: 6,
                        sw: 0,
                        se: 6,
                    },
                    c,
                );
                p.galley(
                    flag.center() - g.size() / 2.0,
                    g,
                    Color32::from_rgb(14, 10, 24),
                );
            }
            if w > 34.0 {
                p.text(
                    pos2(x + w / 2.0, wave.bottom() - 9.0),
                    Align2::CENTER_CENTER,
                    crate::chop::pad_name(i),
                    theme::mono(10.0),
                    theme::alpha(c, 0.8),
                );
            }
        }
    }
    if let Some(h) = hover_pos
        && chop.dragging.is_none()
    {
        p.vline(
            h.x,
            wave.y_range(),
            Stroke::new(1.0, Color32::from_white_alpha(40)),
        );
    }
    p.rect_stroke(
        wave,
        CornerRadius::same(10),
        Stroke::new(1.0, theme::alpha(color, 0.18)),
        StrokeKind::Inside,
    );
    if slices.is_empty() {
        p.text(
            wave.center(),
            Align2::CENTER_CENTER,
            "Nothing to chop here",
            theme::semi(14.0),
            theme::TEXT_DIM,
        );
    }
}

/// A timeline showing `[a, b]` (for the shared grid painter).
fn tl_view(tl: &crate::timeline::Timeline, a: f64, b: f64) -> crate::timeline::Timeline {
    let mut v = tl.clone();
    v.start = a;
    v.end = b;
    v
}

fn export_card(app: &mut StemApp, ui: &mut egui::Ui, rect: Rect, it: &mut Intents) {
    let format = chop_export::chop_format(app.settings.export_format.audio_format());
    let Some(s) = &mut app.session else { return };
    card(ui, rect, theme::VIOLET);
    let n = s.chop.slices.len();
    let stem = s
        .tracks
        .get(s.chop.track)
        .map_or("Stem", |t| t.kind.label());
    let busy = s.chop.export_rx.is_some();
    let mut cui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(20.0, 16.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let ui = &mut cui;
    ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
    ui.horizontal(|ui| {
        label(ui, "Export kit");
        let base = chop_export::base_name(&s.title, stem);
        ui.label(
            egui::RichText::new(format!(
                "Chops/{base} - Chop 01{}.{}  +  .sfz{}",
                if n > 1 {
                    format!("…{n:02}")
                } else {
                    String::new()
                },
                format.extension(),
                if s.chop.settings.dspreset {
                    " + .dspreset"
                } else {
                    ""
                }
            ))
            .font(theme::mono(11.5))
            .color(theme::TEXT_DIM),
        );
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let btn = Button::primary(if busy { "Exporting…" } else { "Export chops" })
            .export_shimmer()
            .icon(Icon::Scissors)
            .enabled(n > 0 && !busy)
            .tooltip(
                "Write every slice as a numbered file plus an SFZ kit (slice 1 on C1 / MIDI 36)",
            )
            .height(40.0)
            .show(ui);
        it.anchor = Some(btn.rect.center());
        if btn.clicked() {
            it.export = true;
        }
        let label_txt = if crate::dragout::NATIVE {
            "Drag Chops folder"
        } else {
            "Show Chops folder"
        };
        let w = drag_handle::pill_width_tall(ui, label_txt);
        let (pill, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
        let tip = if crate::dragout::NATIVE {
            "Writes the chops, then drag the whole folder into your DAW or sampler"
        } else {
            "Writes the chops and shows the folder"
        };
        if let (_, Some(_)) = drag_handle::drag_pill(
            ui,
            pill,
            Id::unique("chop-drag-folder"),
            label_txt,
            theme::VIOLET,
            tip,
        ) && n > 0
        {
            it.drag_folder = true;
        }
        ui.add_space(6.0);
        buttons::toggle(ui, &mut s.chop.settings.dspreset)
            .on_hover_text("Also write a Decent Sampler preset (.dspreset)");
        ui.label(
            egui::RichText::new("Decent Sampler")
                .font(theme::regular(13.0))
                .color(theme::TEXT_DIM),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if Button::ghost("Open")
                .icon(Icon::Folder)
                .tooltip("Open the Chops folder")
                .show(ui)
                .clicked()
            {
                it.open_folder = true;
            }
        });
    });
}

fn pads_card(app: &mut StemApp, ui: &mut egui::Ui, rect: Rect, now: f64, it: &mut Intents) {
    let Some(s) = &mut app.session else { return };
    let chop = &mut s.chop;
    card(ui, rect, theme::PINK);
    let inner = rect.shrink2(vec2(18.0, 16.0));
    let pages = chop.pages();
    let page = chop.settings.page.min(pages - 1);

    // Header: title, page tabs, choke.
    let head = Rect::from_min_size(inner.min, vec2(inner.width(), 30.0));
    let mut hui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(head)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    hui.spacing_mut().item_spacing.x = 5.0;
    hui.label(
        egui::RichText::new("Pads")
            .font(theme::semi(10.5))
            .color(theme::TEXT_FAINT),
    );
    hui.add_space(6.0);
    for pg in 0..pages.min(8) {
        let l = ((b'A' + pg as u8) as char).to_string();
        let tip = format!(
            "Page {l}: slices {}–{}  ([ and ] switch pages)",
            pg * PADS + 1,
            ((pg + 1) * PADS).min(chop.slices.len())
        );
        if buttons::letter_toggle(&mut hui, &l, 26.0, pg == page, theme::PINK, &tip).clicked() {
            chop.settings.page = pg;
        }
    }
    hui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let mut choke = chop.settings.choke;
        if buttons::toggle(ui, &mut choke)
            .on_hover_text(
                "Mono / choke: each hit cuts the one still ringing (like an MPC mute group)",
            )
            .changed()
        {
            chop.settings.choke = choke;
            if choke {
                chop.stop_all();
            }
        }
        ui.label(
            egui::RichText::new("Choke")
                .font(theme::regular(13.0))
                .color(if choke { theme::TEXT } else { theme::TEXT_DIM }),
        );
    });

    // The grid.
    let foot_h = 22.0;
    let grid_rect = Rect::from_min_max(
        pos2(inner.left(), head.bottom() + 14.0),
        pos2(inner.right(), inner.bottom() - foot_h - 10.0),
    );
    let base = page * PADS;
    let held = chop.held.clone();
    let linked = chop.hover;
    // Pads pop in, staggered from pad 1, whenever the page, stem or chop mode changes.
    let pop_key = (page, chop.track, chop.settings.mode.label());
    let ctx = ui.ctx().clone();
    let enter = |n: usize| {
        motion::appear(
            &ctx,
            Id::unique(("pad-pop", pop_key, n)),
            0.08 + n as f32 * 0.025,
            0.34,
        )
    };
    let events = {
        let chop_ref = &*chop;
        pads::grid(ui, grid_rect, Id::unique("chop-pads"), now, |n| {
            let i = base + n;
            let has = i < chop_ref.slices.len();
            PadInfo {
                slice: has.then_some(i),
                key: PAD_KEYS[n].1,
                mini: chop_ref.minis.get(i).map(|v| v.as_slice()),
                hit: chop_ref.hits.get(i).copied().unwrap_or_default(),
                held: held.contains(&i),
                secs: chop_ref.pads.get(i).map_or(0.0, |p| p.duration_secs()),
                linked: has && linked == Some(i),
                enter: enter(n),
            }
        })
    };
    // Pad hover links back to the waveform.
    if let Some(pos) = ui.input(|i| i.pointer.hover_pos())
        && grid_rect.contains(pos)
    {
        let gap = 10.0;
        let col = ((pos.x - grid_rect.left()) / ((grid_rect.width() + gap) / 4.0)) as usize;
        let row = ((pos.y - grid_rect.top()) / ((grid_rect.height() + gap) / 4.0)) as usize;
        let i = base + pads::pad_at(row.min(3), col.min(3));
        chop.hover = (i < chop.slices.len()).then_some(i);
    }
    for (n, e) in events {
        let i = base + n;
        match e {
            PadEvent::Press(v) => it.trigger.push((i, v)),
            PadEvent::Release => it.release.push(i),
            PadEvent::DragOut => {
                it.release.push(i);
                it.drag_pad = Some(i);
            }
        }
    }

    // Footer: key map.
    let p = ui.painter();
    p.text(
        pos2(inner.left(), inner.bottom() - foot_h / 2.0),
        Align2::LEFT_CENTER,
        "Keys  1–4 · Q–R · A–F · Z–V   Shift = soft   Esc = stop",
        theme::regular(12.0),
        theme::TEXT_FAINT,
    );
    let voices = chop
        .slices
        .len()
        .min((page + 1) * PADS)
        .saturating_sub(base);
    p.text(
        pos2(inner.right(), inner.bottom() - foot_h / 2.0),
        Align2::RIGHT_CENTER,
        format!("{voices}/16"),
        theme::mono(11.0),
        theme::TEXT_FAINT,
    );
}

// ---- actions -------------------------------------------------------------------------------

fn apply(app: &mut StemApp, ui: &egui::Ui, it: Intents, now: f64) {
    if let Some((title, body)) = it.toast {
        app.toasts.info(title, body);
    }
    if let Some(a) = it.anchor {
        app.fx.set_anchor("chop-export", a);
    }
    let shot = app.shot.is_some();
    let ctx = ui.ctx().clone();
    let Some(s) = &mut app.session else { return };
    for (i, v) in it.trigger {
        if i < s.chop.slices.len() {
            s.chop.trigger(i, v, now, &mut app.player);
            if !s.chop.held.contains(&i) {
                s.chop.held.push(i);
            }
            // Follow the hit to its page.
            s.chop.settings.page = i / PADS;
        }
    }
    for i in it.release {
        s.chop.held.retain(|h| *h != i);
    }
    // Mouse-held pads are released even if the release happened elsewhere.
    if !ui.input(|i| i.pointer.primary_down()) {
        let keys_down: Vec<usize> = ui.input(|i| {
            PAD_KEYS
                .iter()
                .enumerate()
                .filter(|(_, (k, _))| i.key_down(*k))
                .map(|(n, _)| s.chop.settings.page * PADS + n)
                .collect()
        });
        s.chop.held.retain(|h| keys_down.contains(h));
    }
    let job = |s: &crate::app::Session, format| ChopExport {
        track: s.title.clone(),
        stem: s
            .tracks
            .get(s.chop.track)
            .map_or("Stem", |t| t.kind.label())
            .to_owned(),
        slices: s.chop.pads.clone(),
        format,
        dir: s.out_dir.join("Chops"),
        choke: s.chop.settings.choke,
        dspreset: s.chop.settings.dspreset,
    };
    let format = app.settings.export_format.audio_format();
    if it.export && s.chop.export_rx.is_none() {
        s.chop.export_rx = Some(chop_export::spawn(job(s, format), ctx.clone()));
    }
    if shot {
        return;
    }
    let mut drag: Option<Vec<std::path::PathBuf>> = None;
    let mut err: Option<String> = None;
    if let Some(i) = it.drag_pad {
        match chop_export::write_one(&job(s, format), i) {
            Ok(p) => drag = Some(vec![p]),
            Err(e) => err = Some(e),
        }
    }
    if it.drag_folder {
        let j = job(s, format);
        match chop_export::write_all(&j) {
            Ok(_) => drag = Some(vec![j.dir]),
            Err(e) => err = Some(e),
        }
    }
    let dir = s.out_dir.join("Chops");
    if let Some(e) = err {
        app.toasts.error("Couldn't write the chops", e);
    }
    if let Some(paths) = drag {
        app.drag_out(paths);
    }
    if it.open_folder {
        app.open_folder(&dir);
    }
}

/// Per-frame housekeeping, also while the chop view is hidden: finished exports, saving the
/// settings. Returns true while something animates.
pub fn tick(app: &mut StemApp, now: f64) -> bool {
    let shot = app.shot.is_some();
    let Some(s) = &mut app.session else {
        return false;
    };
    if let Some(rx) = &s.chop.export_rx
        && let Ok(res) = rx.try_recv()
    {
        s.chop.export_rx = None;
        match res {
            Ok((n, dir)) => {
                crate::widgets::buttons::export_succeeded();
                let colors = (0..n.clamp(1, 8)).map(slice_color).collect();
                app.fx.sparkle_at("chop-export", colors);
                app.toasts.success(
                    format!("{n} chops exported"),
                    format!("{}  ·  SFZ kit from C1", dir.display()),
                )
            }
            Err(e) => app.toasts.error("Chop export failed", e),
        }
    }
    if !shot {
        s.chop.persist(&s.out_dir, now);
    }
    s.chop.open && s.chop.animating(now)
}
