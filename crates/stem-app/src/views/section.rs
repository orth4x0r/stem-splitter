//! The pre-split step: the full input's waveform with in / out handles that snap to the beat
//! grid, a length / estimated-time readout, and "Split section" / "Whole track".

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Mesh, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, pos2, vec2,
};
use stem_models::Device;

use crate::analysis::{SnapMode, bpm_label};
use crate::app::StemApp;
use crate::backend::mmss;
use crate::section::{SectionPick, estimate_secs};
use crate::theme::{self, motion};
use crate::widgets::buttons::{self, Button};
use crate::widgets::icons::Icon;
use crate::widgets::paint;
use crate::widgets::toast::elide;

const CARD_H: f32 = 562.0;
const PAD: f32 = 30.0;
const WAVE_H: f32 = 200.0;
/// Grab distance around a handle, in points.
const GRAB: f32 = 10.0;

/// What a drag on the waveform is doing.
#[derive(Clone, Copy)]
enum Drag {
    Edge(usize),
    /// Moving the whole section; offset from its start where it was grabbed.
    Move(f64),
    /// Drawing a new section from this anchor.
    New(f64),
}

/// "1:02.4".
fn mmss_t(t: f64) -> String {
    let t = t.max(0.0);
    let tenths = ((t * 10.0).floor() as u64) % 10;
    format!("{}.{tenths}", mmss(t))
}

pub fn page(app: &mut StemApp, ui: &mut egui::Ui) {
    let playing = app.player.is_playing();
    let playhead = app.player.position();
    let gpu = app.settings.device != Device::Cpu && (cfg!(windows) || app.demo());
    let model = app
        .backend
        .model(app.settings.model_id.as_deref())
        .map(|m| (m.arch.clone(), m.name));
    let Some(pick) = &mut app.section else { return };
    let ctx = ui.ctx().clone();
    let t = ui.input(|i| i.time);
    let free = ui.input(|i| i.modifiers.alt);

    let w = ui.available_width().min(1040.0);
    let mut split = false;
    let mut cancel = false;
    let mut toggle_play = false;
    ui.vertical_centered(|ui| {
        ui.set_max_width(w);
        let free_h = ui.ctx().content_rect().height() - 64.0 - 52.0 - CARD_H - 40.0;
        ui.add_space((free_h / 2.0).clamp(4.0, 120.0));
        let (card, _) = ui.allocate_exact_size(vec2(w, CARD_H), Sense::hover());
        let p = ui.painter().clone();
        let rr = CornerRadius::same(24);
        paint::glass(&p, card, rr, Some(theme::alpha(theme::VIOLET, 0.07)), 0.0);
        paint::inner_glow(&p, card, rr, theme::alpha(theme::VIOLET, 0.16));
        paint::edge_light(&p, card, None, card.width() * 0.6, theme::alpha(theme::VIOLET, 0.8));
        let inner = card.shrink2(vec2(PAD, 0.0));

        // ---- header
        let mut y = card.top() + 34.0;
        let pulse = if motion::reduced() {
            1.0
        } else {
            paint::pulse(t, 1.6)
        };
        p.circle_filled(
            pos2(inner.left() + 4.0, y),
            4.0,
            theme::alpha(theme::CYAN, 0.5 + 0.5 * pulse),
        );
        p.text(
            pos2(inner.left() + 16.0, y),
            Align2::LEFT_CENTER,
            "PICK A SECTION",
            theme::semi(11.5),
            theme::CYAN,
        );
        y += 30.0;
        p.text(
            pos2(inner.left(), y),
            Align2::LEFT_CENTER,
            elide(&pick.prep.title, 46),
            theme::bold(26.0),
            theme::TEXT,
        );
        y += 26.0;
        p.text(
            pos2(inner.left(), y),
            Align2::LEFT_CENTER,
            elide(&pick.prep.source.display(), 70),
            theme::mono(12.0),
            theme::TEXT_FAINT,
        );
        // Tempo / key / length chips, top right.
        let mut cx = inner.right();
        let chip_y = card.top() + 36.0;
        cx = chip(&p, cx, chip_y, &mmss(pick.prep.duration), theme::CYAN) - 8.0;
        if let Some(a) = &pick.prep.analysis {
            if let Some(k) = &a.key {
                cx = chip(&p, cx, chip_y, &k.name(), theme::VIOLET) - 8.0;
            }
            if let Some(b) = a.bpm() {
                chip(&p, cx, chip_y, &format!("{} BPM", bpm_label(b)), theme::PINK);
            }
        }

        // ---- waveform
        let ruler = Rect::from_min_size(
            pos2(inner.left(), card.top() + 122.0),
            vec2(inner.width(), 18.0),
        );
        let wave = Rect::from_min_size(
            pos2(inner.left(), ruler.bottom() + 4.0),
            vec2(inner.width(), WAVE_H),
        );
        let resp = ui.interact(wave, Id::unique("section-wave"), Sense::click_and_drag());
        let dur = pick.prep.duration.max(1e-6);
        let to_t = |x: f32| (((x - wave.left()) / wave.width()).clamp(0.0, 1.0) as f64) * dur;
        let to_x = |s: f64| wave.left() + (s / dur).clamp(0.0, 1.0) as f32 * wave.width();
        interact(pick, &resp, ui, wave, free, to_t, to_x);
        let seek = resp.clicked().then(|| resp.interact_pointer_pos()).flatten();

        // Displayed edges glide when a button moves them; drags stay 1:1.
        let dragging = resp.dragged();
        let glide = |id: &str, v: f64| -> f64 {
            let id = Id::unique(("section-edge", id));
            let secs = if dragging || motion::reduced() {
                0.0
            } else {
                motion::SLOW
            };
            ctx.animate_value_with_time(id, v as f32, secs) as f64
        };
        let (a, b) = (glide("a", pick.sel.0), glide("b", pick.sel.1));
        let (xa, xb) = (to_x(a), to_x(b));
        let hover_edge = resp.hover_pos().map(|h| {
            if (h.x - to_x(pick.sel.0)).abs() < GRAB || (h.x - to_x(pick.sel.1)).abs() < GRAB {
                CursorIcon::ResizeHorizontal
            } else if h.x > to_x(pick.sel.0) && h.x < to_x(pick.sel.1) {
                CursorIcon::Grab
            } else {
                CursorIcon::Crosshair
            }
        });
        if let Some(c) = hover_edge {
            ui.ctx()
                .set_cursor_icon(if dragging { CursorIcon::Grabbing } else { c });
        }

        draw_bar_ruler(&p, ruler, pick);
        let wp = ui.painter_at(wave.expand(8.0));
        draw_wave(&wp, wave, pick, xa, xb, t);
        draw_selection(&wp, wave, xa, xb, t);
        if (playing || playhead > 0.0) && playhead < dur {
            let px = to_x(playhead);
            paint::glow(
                &wp,
                Rect::from_x_y_ranges(px - 1.0..=px + 1.0, wave.y_range()),
                CornerRadius::ZERO,
                theme::alpha(Color32::WHITE, 0.4),
                8.0,
            );
            wp.vline(px, wave.y_range(), Stroke::new(1.5, Color32::WHITE));
        }
        if let Some(h) = resp.hover_pos()
            && !dragging
        {
            wp.vline(
                h.x,
                wave.y_range(),
                Stroke::new(1.0, Color32::from_white_alpha(36)),
            );
        }
        // Time ruler with the edge tags.
        let tr = Rect::from_min_size(pos2(wave.left(), wave.bottom() + 6.0), vec2(wave.width(), 18.0));
        draw_time_ruler(&p, tr, dur, xa, xb);
        edge_tag(&p, xa, tr.center().y, &mmss_t(pick.sel.0), theme::PINK, true, wave);
        edge_tag(&p, xb, tr.center().y, &mmss_t(pick.sel.1), theme::CYAN, false, wave);

        // ---- readout tiles
        let tiles_top = tr.bottom() + 22.0;
        let gap = 12.0;
        let tw = (inner.width() - 3.0 * gap) / 4.0;
        // The readout tiles rise in one after another when the picker opens.
        let tile_in = |i: usize| {
            motion::ease_out(motion::appear(
                &ctx,
                Id::unique(("section-tile", i)),
                0.12 + i as f32 * motion::STAGGER,
                motion::ENTER,
            ))
        };
        let tile_r = |i: usize| {
            Rect::from_min_size(
                pos2(inner.left() + i as f32 * (tw + gap), tiles_top + 12.0 * (1.0 - tile_in(i))),
                vec2(tw, 78.0),
            )
        };
        let tp = |i: usize| {
            let mut q = p.clone();
            q.multiply_opacity(tile_in(i));
            q
        };
        let has_grid = pick.grid().is_some();
        let bar_at = |s: f64| pick.bar_number(s).map(|n| format!("bar {n}"));
        let len = pick.sel.1 - pick.sel.0;
        let whole = pick.range().is_none();
        tile(&tp(0), tile_r(0), "In", &mmss_t(pick.sel.0), bar_at(pick.sel.0).as_deref(), theme::PINK);
        tile(&tp(1), tile_r(1), "Out", &mmss_t(pick.sel.1), bar_at(pick.sel.1).as_deref(), theme::CYAN);
        let len_sub = if whole {
            "whole track".to_owned()
        } else {
            let pct = 100.0 * len / dur;
            let of = if tw < 200.0 { "" } else { " of the track" };
            match pick.bars() {
                Some(bars) if (bars - bars.round()).abs() < 0.05 => {
                    format!("{} bars · {pct:.0}%{of}", bars.round())
                }
                Some(bars) => format!("{bars:.1} bars · {pct:.0}%{of}"),
                None => format!("{pct:.0}%{of}"),
            }
        };
        tile(&tp(2), tile_r(2), "Length", &mmss_t(len), Some(&len_sub), theme::VIOLET);
        if let Some((arch, _)) = &model {
            let est = estimate_secs(arch, gpu, len);
            let full = estimate_secs(arch, gpu, dur);
            let on = if gpu { "GPU" } else { "CPU" };
            let sub = if whole {
                format!("rough guess · {on}")
            } else {
                format!("whole track ~{} · {on}", mmss(full))
            };
            tile(&tp(3), tile_r(3), "Est. split time", &format!("~{}", mmss(est.max(1.0))), Some(&sub), theme::GOOD);
        }

        // ---- controls
        let row = Rect::from_min_size(
            pos2(inner.left(), tiles_top + 78.0 + 24.0),
            vec2(inner.width(), 42.0),
        );
        let mut left = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(row)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        left.spacing_mut().item_spacing.x = 10.0;
        // Narrow windows: bar chips lose their unit (a "BARS" label carries it) and
        // "Whole track" shrinks to its icon, so the two button groups never meet.
        let narrow = inner.width() < 900.0;
        if narrow {
            left.spacing_mut().item_spacing.x = 6.0;
        }
        toggle_play = buttons::icon_button(
            &mut left,
            if playing { Icon::Pause } else { Icon::Play },
            40.0,
            playing,
            theme::PINK,
            "Play the section on a loop (Space)",
        )
        .clicked();
        left.add_space(6.0);
        if has_grid {
            left.label(
                egui::RichText::new("SNAP")
                    .font(theme::semi(10.5))
                    .color(theme::TEXT_FAINT),
            );
            let opts = [
                (SnapMode::Off, "Off"),
                (SnapMode::Beat, "Beat"),
                (SnapMode::Bar, "Bar"),
            ];
            buttons::segmented(&mut left, "section-snap", &mut pick.snap, &opts);
            left.add_space(8.0);
            if narrow {
                left.label(
                    egui::RichText::new("BARS")
                        .font(theme::semi(10.5))
                        .color(theme::TEXT_FAINT),
                );
            }
            for bars in [4usize, 8, 16, 32] {
                let on = pick
                    .bars()
                    .is_some_and(|b| (b - bars as f64).abs() < 0.05 && !whole);
                let text = if narrow {
                    bars.to_string()
                } else {
                    format!("{bars} bars")
                };
                if buttons::chip(&mut left, &text, theme::VIOLET, on)
                    .on_hover_text(format!("{bars} bars from the in point"))
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    pick.set_bars(bars);
                }
            }
        } else {
            left.label(
                egui::RichText::new("No beat grid found: edges move freely")
                    .font(theme::regular(12.5))
                    .color(theme::TEXT_FAINT),
            );
        }
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(row)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        right.spacing_mut().item_spacing.x = 10.0;
        let label = if whole {
            "Split whole track"
        } else {
            "Split section"
        };
        primary_glow(&right);
        split = Button::primary(label)
            .icon(Icon::Sparkle)
            .height(42.0)
            .min_width(if narrow { 150.0 } else { 170.0 })
            .tooltip("Separate only this part (Enter)")
            .show(&mut right)
            .clicked();
        if Button::new(if narrow { "" } else { "Whole track" })
            .icon(Icon::Refresh)
            .height(42.0)
            .enabled(!whole)
            .tooltip("Select the entire track again")
            .show(&mut right)
            .clicked()
        {
            pick.whole();
        }
        cancel = Button::ghost("Cancel").height(42.0).show(&mut right).clicked();

        if let Some(pos) = seek {
            app.player.seek(to_t(pos.x));
        }

        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(
                "Drag the handles to trim, drag the section to move it, or draw a new one  ·  Alt: no snap  ·  Space: play  ·  Enter: split",
            )
            .font(theme::regular(12.5))
            .color(theme::TEXT_FAINT),
        );
    });

    let Some(pick) = &app.section else { return };
    let sel = pick.sel;
    app.player.set_loop(Some(sel));
    if toggle_play {
        let pos = app.player.position();
        if !app.player.is_playing() && (pos < sel.0 || pos >= sel.1 - 0.05) {
            app.player.seek(sel.0);
        }
        app.player.toggle();
    }
    let (enter, esc) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    let now = ui.input(|i| i.time);
    if split || (enter && !ui.ctx().egui_wants_keyboard_input()) {
        app.split_section(now);
    } else if cancel || esc {
        app.close_section();
    }
}

/// Edge / move / new-section dragging on the waveform.
fn interact(
    pick: &mut SectionPick,
    resp: &egui::Response,
    ui: &egui::Ui,
    wave: Rect,
    free: bool,
    to_t: impl Fn(f32) -> f64,
    to_x: impl Fn(f64) -> f32,
) {
    let id = Id::unique("section-drag");
    if resp.drag_started()
        && let Some(pos) = ui
            .input(|i| i.pointer.press_origin())
            .or(resp.interact_pointer_pos())
    {
        let (xa, xb) = (to_x(pick.sel.0), to_x(pick.sel.1));
        let mode = if (pos.x - xb).abs() < GRAB && (pos.x - xb).abs() <= (pos.x - xa).abs() {
            Drag::Edge(1)
        } else if (pos.x - xa).abs() < GRAB {
            Drag::Edge(0)
        } else if pos.x > xa && pos.x < xb {
            Drag::Move(to_t(pos.x) - pick.sel.0)
        } else {
            Drag::New(pick.snap_time(to_t(pos.x), free))
        };
        ui.data_mut(|d| d.insert_temp(id, mode));
    }
    if resp.dragged()
        && let (Some(pos), Some(mode)) = (
            resp.interact_pointer_pos(),
            ui.data(|d| d.get_temp::<Drag>(id)),
        )
    {
        let x = pos.x.clamp(wave.left(), wave.right());
        match mode {
            Drag::Edge(e) => {
                let s = pick.snap_time(to_t(x), free);
                pick.set_edge(e, s);
            }
            Drag::Move(off) => {
                let start = pick.snap_time(to_t(x) - off, free);
                pick.slide_to(start);
            }
            Drag::New(anchor) => {
                let s = pick.snap_time(to_t(x), free);
                if (s - anchor).abs() >= crate::section::MIN_SECS {
                    pick.sel = (anchor.min(s), anchor.max(s));
                }
            }
        }
    }
}

/// Bar numbers above the waveform (every 1, 2, 4… bars, whatever fits).
fn draw_bar_ruler(p: &egui::Painter, r: Rect, pick: &SectionPick) {
    let Some(g) = pick.grid() else {
        p.text(
            pos2(r.left(), r.center().y),
            Align2::LEFT_CENTER,
            "FULL TRACK",
            theme::semi(10.5),
            theme::TEXT_FAINT,
        );
        return;
    };
    let dur = pick.prep.duration.max(1e-6);
    let bar_len = g.period() * g.beats_per_bar.max(1) as f64;
    let px_per_bar = r.width() as f64 * bar_len / dur;
    let every = [1usize, 2, 4, 8, 16, 32]
        .into_iter()
        .find(|n| px_per_bar * *n as f64 >= 34.0)
        .unwrap_or(64);
    let bpb = g.beats_per_bar.max(1) as i64;
    let first = pick.first_bar_beat().unwrap_or(g.downbeat as i64);
    let mut bar = 0usize;
    loop {
        let s = g.beat_time(first + bar as i64 * bpb);
        if s > dur {
            break;
        }
        if bar.is_multiple_of(every) && s >= -0.06 {
            let x = r.left() + (s.max(0.0) / dur) as f32 * r.width();
            p.vline(
                x,
                r.bottom() - 5.0..=r.bottom(),
                Stroke::new(1.0, theme::alpha(theme::VIOLET, 0.7)),
            );
            p.text(
                pos2(x + 3.0, r.center().y - 2.0),
                Align2::LEFT_CENTER,
                format!("{}", bar + 1),
                theme::mono(10.5),
                theme::TEXT_FAINT,
            );
        }
        bar += 1;
        if bar > 10_000 {
            break;
        }
    }
}

fn draw_time_ruler(p: &egui::Painter, r: Rect, dur: f64, xa: f32, xb: f32) {
    let step = [5.0, 10.0, 15.0, 30.0, 60.0, 120.0]
        .into_iter()
        .find(|st| dur / st <= 14.0)
        .unwrap_or(300.0);
    let mut s = 0.0;
    while s <= dur + 1e-6 {
        let x = r.left() + (s / dur) as f32 * r.width();
        // Leave room for the edge tags.
        if (x - xa).abs() > 46.0 && (x - xb).abs() > 46.0 {
            p.text(
                pos2(x, r.center().y),
                if s == 0.0 {
                    Align2::LEFT_CENTER
                } else {
                    Align2::CENTER_CENTER
                },
                mmss(s),
                theme::mono(10.5),
                theme::TEXT_GHOST,
            );
        }
        s += step;
    }
}

/// The full mix: bright accent gradient inside the section, a dim ghost outside.
fn draw_wave(p: &egui::Painter, r: Rect, pick: &SectionPick, xa: f32, xb: f32, _t: f64) {
    p.rect_filled(r, CornerRadius::same(14), Color32::from_black_alpha(96));
    if let Some(g) = pick.grid() {
        crate::timeline::paint_grid(p, r, &pick.view, g, theme::VIOLET);
    }
    let mid = r.center().y;
    p.hline(
        r.x_range(),
        mid,
        Stroke::new(1.0, Color32::from_white_alpha(10)),
    );
    let env = &pick.prep.env;
    let cols = (r.width() / 2.0).floor().max(1.0) as usize;
    // Scale to the loud part of the track rather than its single highest peak, so a dense
    // master still fills the height (the rare peaks above it just clip at the edge).
    let mut tops: Vec<f32> = env.max.iter().map(|v| v.abs()).collect();
    tops.sort_by(|a, b| a.total_cmp(b));
    let loud = tops.get(tops.len() * 3 / 4).copied().unwrap_or(0.0);
    let scale = 0.92 / loud.max(env.peak() * 0.3).max(0.05);
    let half = r.height() * 0.5 - 6.0;
    let mut mesh = Mesh::default();
    for i in 0..cols {
        let f0 = i as f32 / cols as f32;
        let f1 = (i + 1) as f32 / cols as f32;
        let (lo, hi, rms) = env.range(f0, f1);
        let x = r.left() + f0 * r.width();
        let inside = x + 1.0 >= xa && x <= xb;
        let c = if inside {
            let u = ((x - xa) / (xb - xa).max(1.0)).clamp(0.0, 1.0);
            theme::accent_at(u)
        } else {
            Color32::from_rgb(92, 92, 118)
        };
        let top = mid - (hi * scale).min(1.0) * half;
        let bot = mid - (lo * scale).max(-1.0) * half;
        mesh.add_colored_rect(
            Rect::from_min_max(pos2(x, top), pos2(x + 1.4, bot.max(top + 1.0))),
            theme::alpha(c, if inside { 0.5 } else { 0.3 }),
        );
        let rh = (rms * scale * 1.2).min(1.0) * half;
        mesh.add_colored_rect(
            Rect::from_min_max(pos2(x, mid - rh), pos2(x + 1.4, mid + rh.max(0.5))),
            if inside {
                theme::lerp_color(c, Color32::WHITE, 0.12)
            } else {
                theme::alpha(c, 0.4)
            },
        );
    }
    p.add(Shape::mesh(mesh));
}

/// Shade outside the section, gradient rails along it, glowing handles with grips.
fn draw_selection(p: &egui::Painter, r: Rect, xa: f32, xb: f32, t: f64) {
    let shade = Color32::from_black_alpha(120);
    if xa > r.left() + 0.5 {
        p.rect_filled(
            Rect::from_x_y_ranges(r.left()..=xa, r.y_range()),
            CornerRadius {
                nw: 14,
                sw: 14,
                ne: 0,
                se: 0,
            },
            shade,
        );
    }
    if xb < r.right() - 0.5 {
        p.rect_filled(
            Rect::from_x_y_ranges(xb..=r.right(), r.y_range()),
            CornerRadius {
                nw: 0,
                sw: 0,
                ne: 14,
                se: 14,
            },
            shade,
        );
    }
    let sel = Rect::from_x_y_ranges(xa..=xb, r.y_range());
    paint::glow(
        p,
        sel,
        CornerRadius::same(6),
        theme::alpha(theme::VIOLET, 0.10),
        18.0,
    );
    p.rect_filled(sel, CornerRadius::same(4), Color32::from_white_alpha(8));
    let u = if motion::reduced() {
        0.0
    } else {
        (t * 0.06) as f32
    };
    for y in [r.top(), r.bottom() - 2.0] {
        paint::gradient_rect(
            p,
            Rect::from_min_max(pos2(xa, y), pos2(xb, y + 2.0)),
            CornerRadius::same(1),
            u,
            u + 1.0,
            Color32::WHITE,
        );
    }
    handle(p, r, xa, theme::PINK);
    handle(p, r, xb, theme::CYAN);
}

fn handle(p: &egui::Painter, r: Rect, x: f32, color: Color32) {
    let line = Rect::from_x_y_ranges(x - 1.0..=x + 1.0, r.y_range());
    paint::glow(p, line, CornerRadius::ZERO, theme::alpha(color, 0.7), 10.0);
    p.rect_filled(line, CornerRadius::same(1), color);
    for cy in [r.top() + 22.0, r.bottom() - 22.0] {
        let knob = Rect::from_center_size(pos2(x, cy), vec2(14.0, 30.0));
        paint::glow(
            p,
            knob,
            CornerRadius::same(7),
            theme::alpha(color, 0.55),
            12.0,
        );
        p.rect(
            knob,
            CornerRadius::same(7),
            color,
            Stroke::new(1.0, Color32::from_white_alpha(120)),
            StrokeKind::Inside,
        );
        for dy in [-5.0, 0.0, 5.0] {
            p.hline(
                x - 3.5..=x + 3.5,
                cy + dy,
                Stroke::new(1.2, Color32::from_black_alpha(110)),
            );
        }
    }
}

/// Time tag under a handle; the in tag hangs left of its line, the out tag right.
fn edge_tag(p: &egui::Painter, x: f32, y: f32, text: &str, color: Color32, left: bool, wave: Rect) {
    let g = p.layout_no_wrap(text.to_owned(), theme::mono(11.5), Color32::BLACK);
    let w = g.size().x + 16.0;
    let mut r = if left {
        Rect::from_min_size(pos2(x - w + 7.0, y - 10.0), vec2(w, 20.0))
    } else {
        Rect::from_min_size(pos2(x - 7.0, y - 10.0), vec2(w, 20.0))
    };
    // Stay inside the waveform's width.
    if r.left() < wave.left() {
        r = r.translate(vec2(wave.left() - r.left(), 0.0));
    }
    if r.right() > wave.right() {
        r = r.translate(vec2(wave.right() - r.right(), 0.0));
    }
    paint::glow(
        p,
        r,
        CornerRadius::same(10),
        theme::alpha(color, 0.45),
        10.0,
    );
    p.rect_filled(r, CornerRadius::same(10), color);
    p.galley(
        r.center() - g.size() / 2.0,
        g,
        Color32::from_rgb(12, 10, 20),
    );
}

/// Non-interactive chip, right-aligned at `right`. Returns its left edge.
fn chip(p: &egui::Painter, right: f32, y: f32, text: &str, color: Color32) -> f32 {
    let g = p.layout_no_wrap(text.to_owned(), theme::semi(12.0), theme::TEXT);
    let r = Rect::from_min_size(
        pos2(right - g.size().x - 26.0, y - 12.0),
        vec2(g.size().x + 26.0, 24.0),
    );
    p.rect(
        r,
        CornerRadius::same(12),
        theme::alpha(color, 0.12),
        Stroke::new(1.0, theme::alpha(color, 0.5)),
        StrokeKind::Inside,
    );
    let dot: Pos2 = pos2(r.left() + 11.0, r.center().y);
    paint::glow_circle(p, dot, 2.0, theme::alpha(color, 0.6), 6.0);
    p.circle_filled(dot, 3.0, color);
    p.galley(
        pos2(r.left() + 18.0, r.center().y - g.size().y / 2.0),
        g,
        theme::TEXT,
    );
    r.left()
}

/// Glass readout tile: label, big value, small line under it, colour bar on the left.
fn tile(p: &egui::Painter, r: Rect, label: &str, value: &str, sub: Option<&str>, color: Color32) {
    paint::glass(p, r, CornerRadius::same(14), None, 0.0);
    paint::soft_disc(
        p,
        pos2(r.left() + 26.0, r.center().y),
        54.0,
        theme::alpha(color, 0.12),
    );
    let bar = Rect::from_min_size(pos2(r.left(), r.top() + 14.0), vec2(3.0, r.height() - 28.0));
    paint::glow(p, bar, 2, theme::alpha(color, 0.6), 8.0);
    p.rect_filled(bar, 2, color);
    p.text(
        pos2(r.left() + 18.0, r.top() + 17.0),
        Align2::LEFT_CENTER,
        label.to_uppercase(),
        theme::semi(10.5),
        theme::TEXT_FAINT,
    );
    p.text(
        pos2(r.left() + 18.0, r.top() + 41.0),
        Align2::LEFT_CENTER,
        value,
        theme::bold(21.0),
        theme::TEXT,
    );
    if let Some(s) = sub {
        p.text(
            pos2(r.left() + 18.0, r.bottom() - 15.0),
            Align2::LEFT_CENTER,
            elide(s, ((r.width() - 30.0) / 6.4) as usize),
            theme::regular(11.5),
            theme::TEXT_DIM,
        );
    }
}

/// Soft pink glow where the primary button is about to be laid out (right-to-left row).
fn primary_glow(ui: &egui::Ui) {
    let r = Rect::from_min_size(
        pos2(
            ui.max_rect().right() - 170.0,
            ui.max_rect().center().y - 21.0,
        ),
        vec2(170.0, 42.0),
    );
    paint::glow(
        ui.painter(),
        r,
        CornerRadius::same(13),
        theme::alpha(theme::PINK, 0.3),
        20.0,
    );
}
