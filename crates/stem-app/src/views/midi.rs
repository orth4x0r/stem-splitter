//! "→ MIDI" sheet: piano-roll preview of a stem's notes, detection / quantise knobs, a synth
//! audition, and `.mid` export or drag-out.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui,
    pos2, vec2,
};
use stem_models::Device;
use stem_models::transcribe::QuantizeGrid;

use crate::app::StemApp;
use crate::backend::{mmss, mmss_ms};
use crate::midi::{MidiSheet, MidiSource, Status, Wave, note_name};
use crate::theme::{self, motion};
use crate::widgets::buttons::{self, Button};
use crate::widgets::drag_handle::{self, PillAction};
use crate::widgets::icons::Icon;
use crate::widgets::sheet::Sheet;
use crate::widgets::toast::elide;
use crate::widgets::{overline, paint};

/// Piano-roll height on a roomy window; it gives way on short windows so the sheet fits.
const ROLL_H: f32 = 340.0;
const KEYS_W: f32 = 46.0;
const RULER_H: f32 = 18.0;

/// Open the sheet for `track`, over the selection when `selection` is set and there is one.
pub fn open_sheet(app: &mut StemApp, track: usize, selection: bool, ctx: &egui::Context) {
    let Some(s) = &app.session else { return };
    let Some(t) = s.tracks.get(track) else { return };
    let (start, end, is_sel) = match s.selection {
        Some((a, b)) if selection && b > a => (a, b, true),
        _ => (0.0, s.duration, false),
    };
    let ana = s.analysis.as_ref();
    let src = MidiSource {
        title: s.title.clone(),
        out_dir: s.out_dir.clone(),
        kind: t.kind,
        stems: s.audio.clone(),
        track,
        start,
        end,
        is_selection: is_sel,
        bpm: ana.and_then(|a| a.bpm()),
        key: ana.and_then(|a| a.key),
        grid: ana.and_then(|a| a.grid.clone()),
    };
    app.player.set_playing(false);
    let blocking = app.shot.is_some() || std::env::var_os("STEM_SHOT").is_some();
    app.midi = Some(MidiSheet::open(
        src,
        app.midi_prefs.clone(),
        app.backend.manager(),
        app.demo(),
        ctx,
        blocking,
    ));
}

/// Small square "→ MIDI" button with a piano-keys glyph, for the stem cards.
pub fn midi_button(ui: &mut Ui, size: f32, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let resp = resp
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Audio → MIDI: turn this stem into notes (selection only, if any)");
    let h = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.12);
    let p = ui.painter();
    let r = CornerRadius::same((size * 0.5) as u8);
    if h > 0.0 {
        paint::glow(p, rect, r, theme::alpha(color, 0.3 * h), 10.0);
    }
    p.rect(
        rect,
        r,
        theme::lerp_color(theme::CARD_HI, theme::alpha(color, 0.25), h),
        Stroke::new(1.0, theme::lerp_color(theme::STROKE, color, 0.35 + 0.5 * h)),
        StrokeKind::Inside,
    );
    keys_glyph(
        p,
        rect.shrink(size * 0.27),
        theme::lerp_color(theme::TEXT_DIM, Color32::WHITE, h),
    );
    resp
}

/// Three white keys with two black keys on top.
fn keys_glyph(p: &egui::Painter, r: Rect, color: Color32) {
    let w = r.width() / 3.0;
    for i in 0..3 {
        let k = Rect::from_min_size(
            pos2(r.left() + i as f32 * w + 0.6, r.top()),
            vec2(w - 1.2, r.height()),
        );
        p.rect_stroke(k, 1.5, Stroke::new(1.2, color), StrokeKind::Inside);
    }
    for i in 1..3 {
        let k = Rect::from_center_size(
            pos2(r.left() + i as f32 * w, r.top() + r.height() * 0.3),
            vec2(w * 0.55, r.height() * 0.6),
        );
        p.rect_filled(k, 1.0, color);
    }
}

enum Action {
    Close,
    Export,
    Drag(PillAction),
    Play(f64),
    Stop,
    Rerun,
}

pub fn show(app: &mut StemApp, ctx: &egui::Context) {
    let Some(sheet) = &mut app.midi else { return };
    sheet.poll();
    if sheet.is_running() || sheet.is_previewing() {
        ctx.request_repaint();
    }
    let color = theme::stem_color(sheet.src.kind);
    let mut actions: Vec<Action> = Vec::new();
    let space = !ctx.egui_wants_keyboard_input()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space));
    if space {
        actions.push(if sheet.is_previewing() {
            Action::Stop
        } else {
            Action::Play(0.0)
        });
    }
    let width = (ctx.content_rect().width() - 120.0).clamp(760.0, 1180.0);
    let mut anim = Sheet::begin(ctx, Id::unique("midi-sheet"));
    let resp = anim.show(ctx, color, 24, |ui| {
        ui.set_width(width);
        header(ui, sheet, color, &mut actions);
        ui.add_space(14.0);
        let roll_h = (ui.ctx().content_rect().height() - 540.0).clamp(150.0, ROLL_H);
        let (roll, _) = ui.allocate_exact_size(vec2(width, roll_h), Sense::hover());
        if let Some(t) = piano_roll(ui, roll, sheet, color) {
            actions.push(Action::Play(t));
        }
        ui.add_space(14.0);
        controls(ui, sheet, color, width, &mut actions);
        ui.add_space(12.0);
        footer(ui, sheet, &mut actions);
    });
    if resp.should_close() {
        actions.push(Action::Close);
    }
    if anim.closing {
        // Exit animation playing: the sheet is going away, ignore its buttons.
        actions.clear();
    }
    if actions.iter().any(|a| matches!(a, Action::Close)) {
        anim.close(ctx);
        actions.retain(|a| !matches!(a, Action::Close | Action::Play(_)));
        if sheet.is_previewing() {
            actions.push(Action::Stop);
        }
    }
    if anim.finished() {
        actions.push(Action::Close);
    }

    for a in actions {
        let Some(sheet) = &mut app.midi else { return };
        match a {
            Action::Close => {
                app.midi_prefs = sheet.prefs.clone();
                app.midi = None;
            }
            Action::Export => match sheet.export() {
                Ok(p) => {
                    crate::widgets::buttons::export_succeeded();
                    app.toasts.success(
                        "MIDI exported",
                        p.file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    )
                }
                Err(e) => app.toasts.error("MIDI export failed", e),
            },
            Action::Drag(a) => {
                if app.shot.is_some() {
                    continue;
                }
                match sheet.export() {
                    Ok(p) => app.file_pill(p, a),
                    Err(e) => app.toasts.error("MIDI export failed", e),
                }
            }
            Action::Play(from) => {
                if !sheet.start_preview(&mut app.player, from) {
                    app.toasts
                        .info("No audio output", "Playback is off; export still works.");
                }
            }
            Action::Stop => sheet.stop_preview(),
            Action::Rerun => {
                let src = sheet.src.clone();
                let prefs = sheet.prefs.clone();
                let range = sheet.range;
                let view = (sheet.view_start, sheet.view_span);
                let blocking = app.shot.is_some() || std::env::var_os("STEM_SHOT").is_some();
                let mut new =
                    MidiSheet::open(src, prefs, app.backend.manager(), app.demo(), ctx, blocking);
                new.range = range;
                (new.view_start, new.view_span) = view;
                app.midi = Some(new);
            }
        }
    }
}

fn header(ui: &mut Ui, sheet: &MidiSheet, color: Color32, actions: &mut Vec<Action>) {
    let src = &sheet.src;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                paint::glow(
                    ui.painter(),
                    r,
                    CornerRadius::same(8),
                    theme::alpha(color, 0.35),
                    10.0,
                );
                ui.painter()
                    .rect_filled(r, CornerRadius::same(8), theme::alpha(color, 0.9));
                keys_glyph(ui.painter(), r.shrink(6.0), Color32::from_rgb(14, 12, 24));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("{} → MIDI", src.kind.label()))
                        .font(theme::bold(24.0))
                        .color(theme::TEXT),
                );
            });
            let mut meta = vec![
                elide(&src.title, 40),
                if src.is_selection {
                    format!("{} – {}", mmss_ms(src.start), mmss_ms(src.end))
                } else {
                    mmss(src.duration())
                },
            ];
            if let Some(b) = src.bpm {
                meta.push(format!("{} BPM", crate::analysis::bpm_label(b)));
            }
            if let Some(k) = &src.key {
                meta.push(k.name());
            }
            if !sheet.is_running() {
                meta.push(format!("{} notes", sheet.notes.len()));
            }
            ui.label(
                egui::RichText::new(meta.join("  ·  "))
                    .font(theme::mono(12.5))
                    .color(theme::TEXT_DIM),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            if buttons::icon_button(ui, Icon::Close, 32.0, false, theme::BAD, "Close (Esc)")
                .clicked()
            {
                actions.push(Action::Close);
            }
        });
    });
}

/// Rows shown: the notes' span plus a margin, at least two octaves, inside A0..C8.
fn pitch_window(sheet: &MidiSheet) -> (u8, u8) {
    let (mut lo, mut hi) = match (
        sheet.notes.iter().map(|n| n.pitch).min(),
        sheet.notes.iter().map(|n| n.pitch).max(),
    ) {
        (Some(a), Some(b)) => (a.saturating_sub(2), b.saturating_add(2)),
        _ => {
            let (a, b) = sheet.range;
            let mid = (a as u16 + b as u16) / 2;
            (mid.saturating_sub(12) as u8, (mid + 12) as u8)
        }
    };
    while hi - lo < 24 {
        lo = lo.saturating_sub(1).max(21);
        hi = (hi + 1).min(108);
        if lo == 21 && hi == 108 {
            break;
        }
    }
    (lo.max(21), hi.min(108))
}

fn is_black(p: u8) -> bool {
    matches!(p % 12, 1 | 3 | 6 | 8 | 10)
}

/// Draws the roll; returns a time when the user clicked to audition from there.
fn piano_roll(ui: &mut Ui, rect: Rect, sheet: &mut MidiSheet, color: Color32) -> Option<f64> {
    let dur = sheet.src.duration().max(0.1);
    let resp = ui.interact(rect, Id::unique("midi-roll"), Sense::click_and_drag());
    let p = ui.painter().with_clip_rect(rect);
    let now = ui.input(|i| i.time);
    p.rect(
        rect,
        CornerRadius::same(14),
        Color32::from_rgb(11, 11, 19),
        Stroke::new(1.0, theme::STROKE),
        StrokeKind::Inside,
    );
    // Depth: a faint stem-coloured floor light and darker corners.
    paint::soft_disc(
        &p,
        pos2(rect.center().x, rect.bottom() + 40.0),
        rect.width() * 0.45,
        theme::alpha(color, 0.10),
    );
    let keys = Rect::from_min_max(
        rect.min + vec2(0.0, RULER_H),
        pos2(rect.left() + KEYS_W, rect.bottom()),
    );
    let roll = Rect::from_min_max(pos2(keys.right(), keys.top()), rect.max - vec2(10.0, 8.0));
    let ruler = Rect::from_min_max(
        pos2(roll.left(), rect.top()),
        pos2(roll.right(), roll.top()),
    );

    // Zoom (Ctrl+wheel) and pan (wheel / drag).
    if resp.hovered() {
        let (zoom, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
        let pos = ui.input(|i| i.pointer.hover_pos()).unwrap_or(roll.center());
        let anchor =
            sheet.view_start + ((pos.x - roll.left()) / roll.width()) as f64 * sheet.view_span;
        if (zoom - 1.0).abs() > 1e-4 {
            let span = (sheet.view_span / zoom as f64).clamp(1.0, dur);
            sheet.view_start = anchor - (anchor - sheet.view_start) * span / sheet.view_span;
            sheet.view_span = span;
        }
        let d = if scroll.x.abs() > 0.0 {
            scroll.x
        } else {
            scroll.y
        };
        if d != 0.0 {
            sheet.view_start -= d as f64 / roll.width() as f64 * sheet.view_span;
            ui.ctx()
                .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
        }
    }
    if resp.dragged() {
        sheet.view_start -= resp.drag_delta().x as f64 / roll.width() as f64 * sheet.view_span;
    }
    // Follow the audition. (Screenshot scenes can park a fake playhead in temp memory.)
    let playhead = sheet
        .preview_pos()
        .or_else(|| ui.data(|d| d.get_temp::<f64>(Id::unique("stem-shot/midi-playhead"))));
    if let Some(t) = playhead
        && (t > sheet.view_start + sheet.view_span || t < sheet.view_start)
    {
        sheet.view_start = t - sheet.view_span * 0.1;
    }
    sheet.view_span = sheet.view_span.clamp(0.5, dur);
    sheet.view_start = sheet
        .view_start
        .clamp(0.0, (dur - sheet.view_span).max(0.0));
    let (v0, span) = (sheet.view_start, sheet.view_span);
    let x_of = |t: f64| roll.left() + ((t - v0) / span) as f32 * roll.width();
    let t_of = |x: f32| v0 + ((x - roll.left()) / roll.width()) as f64 * span;

    let (lo, hi) = pitch_window(sheet);
    let rows = (hi - lo + 1) as f32;
    let row_h = roll.height() / rows;
    let y_of = |pitch: u8| roll.bottom() - (pitch - lo + 1) as f32 * row_h;

    // Rows and keyboard.
    for pitch in lo..=hi {
        let y = y_of(pitch);
        let row = Rect::from_min_size(pos2(roll.left(), y), vec2(roll.width(), row_h));
        if is_black(pitch) {
            p.rect_filled(row, 0.0, Color32::from_black_alpha(70));
        }
        if pitch % 12 == 0 {
            p.hline(
                roll.x_range(),
                row.bottom(),
                Stroke::new(1.0, Color32::from_white_alpha(14)),
            );
        }
        let key = Rect::from_min_size(pos2(keys.left() + 8.0, y), vec2(KEYS_W - 10.0, row_h));
        let (fill, w) = if is_black(pitch) {
            (Color32::from_rgb(24, 24, 38), 0.62)
        } else {
            (Color32::from_rgb(52, 52, 74), 1.0)
        };
        let k = Rect::from_min_size(key.min, vec2(key.width() * w, key.height()))
            .shrink2(vec2(0.0, 0.5));
        p.rect_filled(k, 2.0, fill);
        if pitch % 12 == 0 && row_h >= 7.0 {
            p.text(
                pos2(key.right() - 2.0, key.center().y),
                Align2::RIGHT_CENTER,
                note_name(pitch),
                theme::mono(row_h.clamp(8.0, 10.5)),
                theme::TEXT_DIM,
            );
        }
    }
    // Keys sounding at the playhead light up.
    if let Some(t) = playhead {
        for n in sheet.notes.iter().filter(|n| n.start <= t && t < n.end) {
            if (lo..=hi).contains(&n.pitch) {
                let k = Rect::from_min_size(
                    pos2(keys.left() + 8.0, y_of(n.pitch)),
                    vec2(KEYS_W - 10.0, row_h),
                );
                paint::glow(&p, k, 2, theme::alpha(color, 0.6), 8.0);
                p.rect_filled(k.shrink2(vec2(0.0, 0.5)), 2.0, color);
            }
        }
    }

    // Beat grid.
    let beats = sheet.src.beats();
    let grid_bpb = sheet
        .src
        .grid
        .as_ref()
        .map_or(4, |g| g.beats_per_bar.max(1)) as i64;
    let downbeat = sheet.src.grid.as_ref().map_or(0, |g| g.downbeat as i64);
    let b0 = beats.position(v0).floor() as i64 - 1;
    let b1 = beats.position(v0 + span).ceil() as i64 + 1;
    let px_per_beat = roll.width() / (b1 - b0).max(1) as f32;
    let sub = sheet
        .prefs
        .quantize
        .map_or(if px_per_beat > 60.0 { 4 } else { 1 }, |q| q.per_beat()) as i64;
    for i in b0..=b1 {
        let bar = (i - downbeat).rem_euclid(grid_bpb) == 0;
        let x = x_of(beats.time(i as f64));
        if x >= roll.left() - 1.0 && x <= roll.right() + 1.0 {
            let (a, w) = if bar { (34, 1.2) } else { (16, 1.0) };
            p.vline(
                x,
                roll.y_range(),
                Stroke::new(w, Color32::from_white_alpha(a)),
            );
            if bar && (px_per_beat * grid_bpb as f32 > 28.0) {
                let n = (i - downbeat).div_euclid(grid_bpb) + 1;
                p.text(
                    pos2(x + 4.0, ruler.center().y + 1.0),
                    Align2::LEFT_CENTER,
                    n.to_string(),
                    theme::mono(10.0),
                    theme::TEXT_FAINT,
                );
            }
        }
        if px_per_beat / sub as f32 > 6.0 {
            for k in 1..sub {
                let xs = x_of(beats.time(i as f64 + k as f64 / sub as f64));
                if xs >= roll.left() && xs <= roll.right() {
                    p.vline(
                        xs,
                        roll.y_range(),
                        Stroke::new(1.0, Color32::from_white_alpha(7)),
                    );
                }
            }
        }
    }

    // Notes: raw positions ghosted behind quantised ones.
    let note_rect = |start: f64, end: f64, pitch: u8| {
        let y = y_of(pitch);
        let x0 = x_of(start);
        let x1 = x_of(end).max(x0 + 2.5);
        Rect::from_min_max(pos2(x0, y + 0.75), pos2(x1, y + row_h - 0.75))
    };
    let radius = CornerRadius::same((row_h * 0.3).clamp(1.0, 4.0) as u8);
    // Fresh notes sweep in from the left (on open and after every re-detect).
    let reveal = if matches!(sheet.status, Status::Ready) {
        motion::ease_in_out(motion::appear(
            ui.ctx(),
            Id::unique((
                "midi-reveal",
                sheet.notes.len(),
                sheet.prefs.quantize.map(|q| q.per_beat()),
            )),
            0.1,
            0.7,
        ))
    } else {
        1.0
    };
    let reveal_x = roll.left() + (roll.width() + 60.0) * reveal;
    // Playhead trail: a fading wash of the stem colour behind the playhead.
    if let Some(t) = playhead {
        let x = x_of(t);
        let len = 90.0f32.min(x - roll.left()).max(0.0);
        if len > 1.0 {
            paint::hgradient(
                &p,
                Rect::from_min_max(pos2(x - len, roll.top()), pos2(x, roll.bottom())),
                Color32::TRANSPARENT,
                theme::alpha(color, 0.16),
            );
        }
    }
    if sheet.prefs.quantize.is_some() {
        for n in &sheet.raw {
            if (lo..=hi).contains(&n.pitch) && n.end >= v0 && n.start <= v0 + span {
                p.rect_stroke(
                    note_rect(n.start, n.end, n.pitch),
                    radius,
                    Stroke::new(1.0, theme::alpha(color, 0.28)),
                    StrokeKind::Inside,
                );
            }
        }
    }
    let hover = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| roll.contains(*p));
    let mut hovered = None;
    for n in &sheet.notes {
        if !(lo..=hi).contains(&n.pitch) || n.end < v0 || n.start > v0 + span {
            continue;
        }
        let r = note_rect(n.start, n.end, n.pitch);
        let shown = ((reveal_x - r.left()) / 60.0).clamp(0.0, 1.0);
        if shown <= 0.0 {
            continue;
        }
        let mut p = p.clone();
        p.multiply_opacity(shown);
        let r = Rect::from_min_max(
            r.min,
            pos2(
                (r.left() + r.width() * motion::ease_out(shown)).max(r.left() + 2.0),
                r.bottom(),
            ),
        );
        let v = n.velocity() as f32 / 127.0;
        let on = playhead.is_some_and(|t| n.start <= t && t < n.end);
        // Afterglow: a note that just ended keeps glowing for a moment.
        let after = playhead.map_or(0.0, |t| {
            let d = (t - n.end) as f32;
            if (0.0..0.45).contains(&d) {
                1.0 - d / 0.45
            } else {
                0.0
            }
        });
        // Onset: how recently the playhead struck this note.
        let struck = playhead.map_or(1.0, |t| ((t - n.start) as f32 / 0.3).max(0.0));
        if on || after > 0.0 {
            let k = if on { 1.0 } else { after };
            paint::glow(
                &p,
                r,
                radius,
                theme::alpha(color, 0.75 * k),
                10.0 + 10.0 * k,
            );
        } else if v > 0.45 {
            paint::glow(&p, r, radius, theme::alpha(color, 0.32 * v), 8.0);
        }
        if on && struck < 1.0 && !motion::reduced() {
            let e = motion::ease_out(struck);
            p.rect_stroke(
                r.expand(2.0 + 10.0 * e),
                CornerRadius::same((radius.nw as f32 + 6.0 * e) as u8),
                Stroke::new(
                    1.5 * (1.0 - struck),
                    theme::alpha(Color32::WHITE, 0.8 * (1.0 - struck)),
                ),
                StrokeKind::Middle,
            );
        }
        let base = theme::lerp_color(Color32::from_rgb(34, 30, 54), color, 0.3 + 0.7 * v);
        let fill = if on {
            theme::lerp_color(base, Color32::WHITE, 0.45)
        } else {
            theme::lerp_color(base, Color32::WHITE, 0.3 * after)
        };
        p.rect_filled(r, radius, fill);
        // The sounding part of a held note burns brighter than the rest.
        if on && let Some(t) = playhead {
            let px = x_of(t).clamp(r.left(), r.right());
            p.rect_filled(
                Rect::from_min_max(r.min, pos2(px, r.bottom())),
                radius,
                theme::alpha(Color32::WHITE, 0.25),
            );
        }
        if r.height() > 4.0 {
            let top = Rect::from_min_size(r.min, vec2(r.width(), (r.height() * 0.35).min(3.0)));
            p.rect_filled(
                top.shrink2(vec2(1.0, 0.0)),
                radius,
                Color32::from_white_alpha((30.0 + 60.0 * v) as u8),
            );
        }
        if hover.is_some_and(|h| r.expand(1.0).contains(h)) {
            hovered = Some(n.clone());
            p.rect_stroke(
                r,
                radius,
                Stroke::new(1.0, Color32::WHITE),
                StrokeKind::Outside,
            );
        }
    }

    if reveal > 0.0 && reveal < 1.0 {
        let x = reveal_x - 30.0;
        paint::glow(
            &p,
            Rect::from_min_max(pos2(x - 1.0, roll.top()), pos2(x + 1.0, roll.bottom())),
            0,
            theme::alpha(color, 0.5 * (1.0 - reveal)),
            14.0,
        );
    }

    // Playhead: a glowing line with a bright cap on the ruler.
    if let Some(t) = playhead {
        let x = x_of(t);
        paint::glow(
            &p,
            Rect::from_min_max(pos2(x - 1.0, roll.top()), pos2(x + 1.0, roll.bottom())),
            0,
            theme::alpha(color, 0.45),
            12.0,
        );
        p.vline(
            x,
            rect.top()..=roll.bottom(),
            Stroke::new(1.5, Color32::WHITE),
        );
        let cap = vec![
            pos2(x - 5.0, ruler.top() + 3.0),
            pos2(x + 5.0, ruler.top() + 3.0),
            pos2(x, ruler.bottom() - 2.0),
        ];
        p.add(egui::Shape::convex_polygon(
            cap,
            Color32::WHITE,
            Stroke::NONE,
        ));
        let _ = now;
    } else if let Some(h) = hover
        && !sheet.is_running()
    {
        p.vline(
            h.x,
            roll.y_range(),
            Stroke::new(1.0, Color32::from_white_alpha(28)),
        );
    }
    p.text(
        pos2(keys.left() + 10.0, ruler.center().y + 1.0),
        Align2::LEFT_CENTER,
        "BAR",
        theme::semi(9.5),
        theme::TEXT_FAINT,
    );

    // Status overlays.
    match &sheet.status {
        Status::Running { msg, frac } => {
            p.rect_filled(rect, CornerRadius::same(14), Color32::from_black_alpha(150));
            let c = rect.center();
            let t = ui.input(|i| i.time);
            crate::widgets::progress::dots(&p, c - vec2(0.0, 30.0), 16.0, t, color);
            p.text(
                c + vec2(0.0, 6.0),
                Align2::CENTER_CENTER,
                msg,
                theme::semi(15.0),
                theme::TEXT,
            );
            let bar = Rect::from_center_size(c + vec2(0.0, 32.0), vec2(280.0, 6.0));
            crate::widgets::progress::bar(&p, bar, *frac, t);
        }
        Status::Failed(e) => {
            p.rect_filled(rect, CornerRadius::same(14), Color32::from_black_alpha(170));
            p.text(
                rect.center() - vec2(0.0, 12.0),
                Align2::CENTER_CENTER,
                "Couldn't transcribe this stem",
                theme::semi(16.0),
                theme::BAD,
            );
            p.text(
                rect.center() + vec2(0.0, 14.0),
                Align2::CENTER_CENTER,
                elide(e, 110),
                theme::regular(12.5),
                theme::TEXT_DIM,
            );
        }
        Status::Ready if sheet.notes.is_empty() => {
            p.text(
                roll.center(),
                Align2::CENTER_CENTER,
                "No notes at these settings: raise the sensitivity or widen the range",
                theme::regular(14.0),
                theme::TEXT_DIM,
            );
        }
        Status::Ready => {}
    }

    if let Some(n) = hovered {
        resp.clone().on_hover_ui_at_pointer(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{}  ·  velocity {}  ·  {}  ·  {:.2} s",
                    note_name(n.pitch),
                    n.velocity(),
                    mmss_ms(n.start + sheet.src.start),
                    n.end - n.start
                ))
                .font(theme::mono(12.0)),
            );
        });
    }
    if resp.hovered() && !sheet.is_running() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if resp.clicked()
        && matches!(sheet.status, Status::Ready)
        && let Some(pos) = resp.interact_pointer_pos()
    {
        return Some(t_of(pos.x).clamp(0.0, dur));
    }
    None
}

/// A titled group of controls.
fn group(ui: &mut Ui, w: f32, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(
        vec2(w, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(16, 16, 27))
                .stroke(Stroke::new(1.0, theme::STROKE))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_width(w - 32.0);
                    ui.set_min_height(150.0);
                    overline(ui, title);
                    ui.add_space(4.0);
                    add(ui);
                });
        },
    );
}

fn row(ui: &mut Ui, label: &str, tip: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(100.0, 30.0), Sense::hover());
        ui.painter().text(
            pos2(r.left(), r.center().y),
            Align2::LEFT_CENTER,
            label,
            theme::regular(13.0),
            theme::TEXT_DIM,
        );
        if !tip.is_empty() {
            resp.on_hover_text(tip);
        }
        add(ui);
    });
}

fn controls(
    ui: &mut Ui,
    sheet: &mut MidiSheet,
    color: Color32,
    width: f32,
    actions: &mut Vec<Action>,
) {
    let gap = 12.0;
    let w = (width - 2.0 * gap) / 3.0;
    let slider_w = (w - 32.0 - 100.0 - 80.0).max(80.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        group(ui, w, "Detection", |ui| {
            let pr = &mut sheet.prefs;
            let mut sens = 100.0 - pr.onset_pct;
            row(
                ui,
                "Sensitivity",
                "How readily a new note starts (Basic Pitch onset threshold, inverted)",
                |ui| {
                    if buttons::slider(ui, &mut sens, 5.0..=95.0, "%", slider_w) {
                        pr.onset_pct = 100.0 - sens;
                    }
                },
            );
            let mut sustain = 100.0 - pr.frame_pct;
            row(
                ui,
                "Sustain",
                "How long notes keep ringing (frame threshold, inverted)",
                |ui| {
                    if buttons::slider(ui, &mut sustain, 5.0..=95.0, "%", slider_w) {
                        pr.frame_pct = 100.0 - sustain;
                    }
                },
            );
            row(
                ui,
                "Shortest note",
                "Notes shorter than this are dropped",
                |ui| {
                    buttons::slider(ui, &mut pr.min_note_ms, 20.0..=400.0, " ms", slider_w);
                },
            );
            row(
                ui,
                "Soft notes",
                "Also catch notes without a clear attack (slides, legato)",
                |ui| {
                    buttons::toggle(ui, &mut pr.soft_notes);
                    ui.add_space(14.0);
                    ui.label(
                        egui::RichText::new("Bends")
                            .font(theme::regular(13.0))
                            .color(theme::TEXT_DIM),
                    )
                    .on_hover_text("Write pitch bends into the file (not when quantised)");
                    buttons::toggle(ui, &mut pr.pitch_bends);
                },
            );
        });
        group(ui, w, "Range & grid", |ui| {
            row(
                ui,
                "Pitch range",
                "Notes outside this range are ignored",
                |ui| {
                    range_slider(ui, &mut sheet.range, slider_w + 56.0, color);
                },
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Quantize to the beat grid")
                    .font(theme::regular(13.0))
                    .color(theme::TEXT_DIM),
            );
            ui.add_space(2.0);
            let mut q = sheet.prefs.quantize;
            let mut opts: Vec<(Option<QuantizeGrid>, &str)> = vec![(None, "Off")];
            opts.extend(QuantizeGrid::ALL.iter().map(|g| (Some(*g), g.label())));
            if buttons::segmented(ui, "midi-q", &mut q, &opts) {
                sheet.prefs.quantize = q;
            }
            ui.add_space(2.0);
            row(
                ui,
                "Strength",
                "100% snaps fully, 50% moves notes halfway",
                |ui| {
                    ui.add_enabled_ui(sheet.prefs.quantize.is_some(), |ui| {
                        buttons::slider(
                            ui,
                            &mut sheet.prefs.strength_pct,
                            0.0..=100.0,
                            "%",
                            slider_w,
                        );
                    });
                },
            );
            let tempo = sheet
                .src
                .bpm
                .map_or("120 BPM (no tempo detected)".to_owned(), |b| {
                    format!("{} BPM from the beat grid", crate::analysis::bpm_label(b))
                });
            ui.label(
                egui::RichText::new(tempo)
                    .font(theme::regular(12.0))
                    .color(theme::TEXT_FAINT),
            );
        });
        group(ui, w, "Audition & export", |ui| {
            let ready = matches!(sheet.status, Status::Ready) && !sheet.notes.is_empty();
            ui.horizontal(|ui| {
                let playing = sheet.is_previewing();
                let (label, icon) = if playing {
                    ("Stop", Icon::Stop)
                } else {
                    ("Play", Icon::Play)
                };
                if Button::new(label)
                    .icon(icon)
                    .enabled(ready || playing)
                    .height(34.0)
                    .min_width(92.0)
                    .tooltip("Hear the notes on a simple synth (Space)")
                    .show(ui)
                    .clicked()
                {
                    actions.push(if playing {
                        Action::Stop
                    } else {
                        Action::Play(0.0)
                    });
                }
                let mut wave = sheet.prefs.wave;
                if buttons::segmented(
                    ui,
                    "midi-wave",
                    &mut wave,
                    &[(Wave::Saw, "Saw"), (Wave::Sine, "Sine")],
                ) {
                    sheet.prefs.wave = wave;
                    if sheet.is_previewing() {
                        actions.push(Action::Play(sheet.preview_pos().unwrap_or(0.0)));
                    }
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if buttons::toggle(ui, &mut sheet.prefs.with_stem).changed()
                    && sheet.is_previewing()
                {
                    actions.push(Action::Play(sheet.preview_pos().unwrap_or(0.0)));
                }
                ui.label(
                    egui::RichText::new(format!(
                        "Play the {} under it",
                        sheet.src.kind.label().to_lowercase()
                    ))
                    .font(theme::regular(13.0))
                    .color(theme::TEXT_DIM),
                );
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if Button::primary("Export .mid")
                    .export_shimmer()
                    .icon(Icon::Export)
                    .enabled(ready)
                    .height(38.0)
                    .tooltip("Save to the MIDI folder next to the stems")
                    .show(ui)
                    .clicked()
                {
                    actions.push(Action::Export);
                }
                if ready {
                    let label = if crate::dragout::NATIVE {
                        "Drag .mid"
                    } else {
                        "Show .mid"
                    };
                    let pw = drag_handle::pill_width(ui, label) + 6.0;
                    let (r, _) = ui.allocate_exact_size(vec2(pw, 38.0), Sense::hover());
                    let r = Rect::from_center_size(r.center(), vec2(pw, 28.0));
                    let tip = if crate::dragout::NATIVE {
                        "Drag the MIDI clip straight onto a DAW track"
                    } else {
                        "Save the MIDI file and show it in its folder"
                    };
                    if let (_, Some(a)) =
                        drag_handle::drag_pill(ui, r, Id::unique("midi-drag"), label, color, tip)
                    {
                        actions.push(Action::Drag(a));
                    }
                }
            });
            ui.add_space(2.0);
            let name = format!("{}.mid", sheet.src.file_stem());
            ui.label(
                egui::RichText::new(elide(&name, 46))
                    .font(theme::mono(11.0))
                    .color(theme::TEXT_FAINT),
            );
        });
    });
}

fn footer(ui: &mut Ui, sheet: &mut MidiSheet, actions: &mut Vec<Action>) {
    // Narrow sheets keep only the how-to, so the hint never runs into the GPU switch.
    let hint = if ui.available_width() < 1100.0 {
        "Click the roll to play from there · Ctrl+wheel to zoom."
    } else {
        "Basic Pitch works best on bass and lead lines; dense chords come out approximate.  \
         Click the roll to play from there · Ctrl+wheel to zoom."
    };
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(hint)
                .font(theme::regular(12.0))
                .color(theme::TEXT_FAINT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut gpu = sheet.prefs.gpu;
            if buttons::toggle(ui, &mut gpu).changed() {
                sheet.prefs.gpu = gpu;
                actions.push(Action::Rerun);
            }
            let dev = match sheet.device {
                Some(Device::DirectMl(_)) => "on the GPU",
                Some(_) => "on the CPU",
                None => "",
            };
            ui.label(
                egui::RichText::new(format!("Basic Pitch {dev}  ·  GPU"))
                    .font(theme::regular(12.0))
                    .color(theme::TEXT_DIM),
            )
            .on_hover_text(
                "Run the model with DirectML. It is tiny, so the CPU is usually just as fast.",
            );
        });
    });
}

/// Two-thumb pitch range slider with a note-name readout.
fn range_slider(ui: &mut Ui, range: &mut (u8, u8), width: f32, color: Color32) {
    let (lo_lim, hi_lim) = (21.0f32, 108.0f32);
    let (rect, resp) = ui.allocate_exact_size(
        vec2((width - 70.0).max(60.0), 28.0),
        Sense::click_and_drag(),
    );
    let resp = resp.on_hover_cursor(CursorIcon::ResizeHorizontal);
    let track = Rect::from_min_max(
        pos2(rect.left() + 8.0, rect.center().y - 3.0),
        pos2(rect.right() - 8.0, rect.center().y + 3.0),
    );
    let x_of = |v: f32| track.left() + track.width() * (v - lo_lim) / (hi_lim - lo_lim);
    let v_of = |x: f32| (lo_lim + (x - track.left()) / track.width() * (hi_lim - lo_lim)).round();
    let drag_id = resp.id.with("which");
    if (resp.drag_started() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let which_hi = (pos.x - x_of(range.1 as f32)).abs() < (pos.x - x_of(range.0 as f32)).abs();
        ui.data_mut(|d| d.insert_temp(drag_id, which_hi));
    }
    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let which_hi = ui.data(|d| d.get_temp::<bool>(drag_id)).unwrap_or(false);
        let v = v_of(pos.x).clamp(lo_lim, hi_lim) as u8;
        if which_hi {
            range.1 = v.max(range.0.saturating_add(1));
        } else {
            range.0 = v.min(range.1.saturating_sub(1));
        }
    }
    let p = ui.painter();
    p.rect_filled(track, CornerRadius::same(3), Color32::from_white_alpha(16));
    let (a, b) = (x_of(range.0 as f32), x_of(range.1 as f32));
    let sel = Rect::from_min_max(pos2(a, track.top()), pos2(b, track.bottom()));
    paint::glow(
        p,
        sel,
        CornerRadius::same(3),
        theme::alpha(color, 0.35),
        8.0,
    );
    p.rect_filled(sel, CornerRadius::same(3), color);
    for x in [a, b] {
        paint::glow_circle(
            p,
            pos2(x, track.center().y),
            7.0,
            theme::alpha(color, 0.4),
            8.0,
        );
        p.circle_filled(Pos2::new(x, track.center().y), 7.0, Color32::WHITE);
    }
    ui.label(
        egui::RichText::new(format!("{}–{}", note_name(range.0), note_name(range.1)))
            .font(theme::mono(13.0))
            .color(theme::TEXT),
    );
}
