//! Results-page pieces for music analysis: the BPM · key · Camelot chips with halve / double,
//! the beat-grid nudge, the snap control, and the overview strip in the transport bar.

use eframe::egui::{self, Rect, vec2};

use crate::analysis::{SnapMode, bpm_label};
use crate::app::StemApp;
use crate::theme::{self, motion};
use crate::widgets::overview::Overview;
use crate::widgets::{buttons, paint};

/// Chips row under the results title.
pub fn chips(app: &mut StemApp, ui: &mut egui::Ui) {
    // Playback position while playing (screenshot scenes can stage one), for the beat pulse.
    let playing_at = if app.player.is_playing() {
        Some(app.player.position())
    } else {
        ui.data(|d| d.get_temp::<f64>(egui::Id::unique("stem-shot/playing")))
    };
    let playhead = app.player.position();
    let Some(s) = &mut app.session else { return };
    let mut rescale = None;
    let mut edit = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        match &s.analysis {
            None => {
                ui.label(
                    egui::RichText::new("No tempo / key")
                        .font(theme::regular(12.5))
                        .color(theme::TEXT_FAINT),
                );
            }
            Some(a) => {
                if let Some(t) = a.tempo {
                    let detected = a.detected_bpm.map_or(String::new(), |d| {
                        if (d - t.bpm).abs() > 0.01 {
                            format!(" (detected {})", bpm_label(d))
                        } else {
                            String::new()
                        }
                    });
                    let slot = ui.painter().add(egui::Shape::Noop);
                    let chip =
                        buttons::chip(ui, &format!("{} BPM", bpm_label(t.bpm)), theme::PINK, false);
                    if let Some(pos) = playing_at
                        && !motion::reduced()
                    {
                        let beat = match &a.grid {
                            Some(g) => g.beat_position(pos),
                            None => pos * t.bpm as f64 / 60.0,
                        };
                        let down = a
                            .grid
                            .as_ref()
                            .is_some_and(|g| g.is_downbeat(beat.floor() as i64));
                        beat_pulse(ui, slot, chip.rect, beat.fract() as f32, down);
                    }
                    chip.on_hover_text(format!(
                        "Tempo{detected} · confidence {:.0}%",
                        t.confidence * 100.0
                    ));
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if buttons::letter_toggle(ui, "½", 24.0, false, theme::PINK, "Halve tempo")
                        .clicked()
                    {
                        rescale = Some(0.5);
                    }
                    if buttons::letter_toggle(ui, "×2", 24.0, false, theme::PINK, "Double tempo")
                        .clicked()
                    {
                        rescale = Some(2.0);
                    }
                    if a.grid.is_some() {
                        ui.add_space(4.0);
                        edit = nudge_controls(ui, a, playhead);
                    }
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.add_space(6.0);
                }
                if let Some(k) = a.key {
                    let tip = format!("Key · confidence {:.0}%", k.confidence * 100.0);
                    buttons::chip(ui, &k.name(), theme::VIOLET, false).on_hover_text(&tip);
                    buttons::chip(ui, &k.camelot(), theme::CYAN, false)
                        .on_hover_text("Camelot wheel: mix with the same number, ±1, or A↔B");
                }
            }
        }
        if s.analysis.as_ref().is_some_and(|a| a.grid.is_some()) {
            ui.add_space(18.0);
            ui.label(
                egui::RichText::new("Snap")
                    .font(theme::semi(10.5))
                    .color(theme::TEXT_FAINT),
            );
            let opts: Vec<(SnapMode, &str)> =
                SnapMode::ALL.iter().map(|m| (*m, m.label())).collect();
            buttons::segmented(ui, "snap", &mut app.settings.snap, &opts);
        }
    });
    let Some(a) = &mut s.analysis else { return };
    let changed = match (rescale, edit) {
        (Some(f), _) => a.rescale(f),
        (None, Some(GridEdit::Shift(dt))) => a.nudge_by(dt),
        (None, Some(GridEdit::Beats(n))) => a.shift_bar(n),
        (None, Some(GridEdit::BarOne(t))) => a.set_bar_one(t),
        (None, Some(GridEdit::Reset)) => {
            a.reset_nudge();
            true
        }
        (None, None) => false,
    };
    if !changed {
        return;
    }
    // Grid-mode chops follow the new grid; the choice is remembered with the split.
    s.chop.changed();
    if let Some(h) = app
        .settings
        .history
        .iter_mut()
        .find(|h| h.out_dir == s.out_dir)
    {
        h.bpm = a.bpm().or(h.bpm);
        h.grid_nudge = (!a.nudge.is_zero()).then_some(a.nudge);
    }
}

/// A beat-grid edit from the nudge controls.
#[derive(Clone, Copy)]
enum GridEdit {
    /// Move every beat by this many seconds.
    Shift(f64),
    /// Move the bar lines by whole beats.
    Beats(i64),
    /// Bar 1 at the beat nearest this time.
    BarOne(f64),
    Reset,
}

/// Grid nudge step.
const NUDGE_SECS: f64 = 0.005;

/// ‹ › (5 ms, Shift: one beat), "1" (bar 1 at the playhead) and, once nudged, a reset tag.
fn nudge_controls(
    ui: &mut egui::Ui,
    a: &crate::analysis::TrackAnalysis,
    playhead: f64,
) -> Option<GridEdit> {
    let shift = ui.input(|i| i.modifiers.shift);
    let mut edit = None;
    let step = |dir: i64| {
        if shift {
            GridEdit::Beats(dir)
        } else {
            GridEdit::Shift(dir as f64 * NUDGE_SECS)
        }
    };
    if buttons::letter_toggle(
        ui,
        "‹",
        24.0,
        false,
        theme::PINK,
        "Move the beat grid 5 ms earlier · Shift: bar lines one beat earlier",
    )
    .clicked()
    {
        edit = Some(step(-1));
    }
    if buttons::letter_toggle(
        ui,
        "›",
        24.0,
        false,
        theme::PINK,
        "Move the beat grid 5 ms later · Shift: bar lines one beat later",
    )
    .clicked()
    {
        edit = Some(step(1));
    }
    if buttons::letter_toggle(
        ui,
        "1",
        24.0,
        false,
        theme::PINK,
        "Set bar 1 here: the beat nearest the playhead starts a bar",
    )
    .clicked()
    {
        edit = Some(GridEdit::BarOne(playhead));
    }
    let n = a.nudge;
    if !n.is_zero() {
        let mut parts = Vec::new();
        if n.offset.abs() >= 0.0005 {
            parts.push(format!("{:+.0} ms", n.offset * 1000.0));
        }
        if n.downbeat_at.is_some() {
            parts.push("bar 1".to_owned());
        }
        if parts.is_empty() {
            parts.push("0 ms".to_owned());
        }
        ui.add_space(2.0);
        let r = ui
            .add(
                egui::Label::new(
                    egui::RichText::new(format!("{} ↺", parts.join(" · ")))
                        .font(theme::mono(11.0))
                        .color(theme::TEXT_FAINT),
                )
                .sense(egui::Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Beat grid moved by hand · click to put it back");
        if r.clicked() {
            edit = Some(GridEdit::Reset);
        }
    }
    edit
}

/// Overview strip at the top of the transport bar.
pub fn overview(app: &mut StemApp, ui: &mut egui::Ui, rect: Rect) {
    let playhead = app.player.position();
    let Some(s) = &mut app.session else { return };
    let envs = s
        .tracks
        .iter()
        .enumerate()
        .filter(|(i, _)| s.audible(*i))
        .map(|(_, t)| (&t.env, theme::stem_color(t.kind)))
        .collect();
    let seek = Overview {
        envs,
        playhead,
        selection: s.selection,
    }
    .show(ui, rect, &mut s.timeline);
    if let Some(t) = seek {
        app.player.seek(t);
    }
}

/// Height the overview strip takes at the top of the transport.
pub const OVERVIEW_H: f32 = 18.0;

/// Rect for the overview strip inside the transport panel's `panel` rect.
pub fn overview_rect(panel: Rect) -> Rect {
    Rect::from_min_size(panel.min + vec2(0.0, 8.0), vec2(panel.width(), OVERVIEW_H))
}

/// Beat-synced pulse on the BPM chip: a glow that hits on every beat (harder on the
/// downbeat) and decays, plus a thin ring that ripples out. `phase` 0..1 within the beat.
fn beat_pulse(ui: &egui::Ui, slot: egui::layers::ShapeIdx, rect: Rect, phase: f32, downbeat: bool) {
    let k = (1.0 - phase).powi(3) * if downbeat { 1.0 } else { 0.6 };
    let r = egui::CornerRadius::same(12);
    ui.painter().set(
        slot,
        paint::glow_shape(rect, r, theme::alpha(theme::PINK, 0.55 * k), 8.0 + 10.0 * k),
    );
    let e = motion::ease_out(phase.min(1.0));
    if phase < 0.6 {
        let fade = 1.0 - phase / 0.6;
        ui.painter().rect_stroke(
            rect.expand(1.0 + 7.0 * e),
            egui::CornerRadius::same((12.0 + 6.0 * e) as u8),
            egui::Stroke::new(
                1.2,
                theme::alpha(theme::PINK, 0.55 * fade * if downbeat { 1.0 } else { 0.6 }),
            ),
            egui::StrokeKind::Middle,
        );
    }
}
