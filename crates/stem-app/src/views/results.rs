//! Results: one card per stem with waveform, solo / mute / volume, region selection, and a
//! transport bar with playback and sample export.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Rect, Sense, Stroke, StrokeKind, pos2,
    vec2,
};

use crate::app::StemApp;
use crate::backend::{mmss, mmss_ms};
use crate::theme;
use crate::widgets::buttons::{self, Button};
use crate::widgets::drag_handle::{self, PillAction};
use crate::widgets::icons::{self, Icon};
use crate::widgets::paint;
use crate::widgets::toast::elide;
use crate::widgets::waveform_view::{Lane, LaneAction};

const CONTROLS_W: f32 = 236.0;
const LANE_H: f32 = 84.0;

pub fn page(app: &mut StemApp, ui: &mut egui::Ui) {
    let Some(s) = &app.session else { return };
    let title = s.title.clone();
    let out_dir = s.out_dir.clone();
    let meta = format!(
        "{} stems  ·  {}  ·  {}",
        s.tracks.len(),
        mmss(s.duration),
        s.audio.first().map_or("".into(), |a| format!(
            "{:.1} kHz",
            a.sample_rate as f32 / 1000.0
        )),
    );

    // ---- header row
    let mut open_folder = false;
    let mut export_all = false;
    let mut new_split = false;
    let mut chop_open = s.chop.open;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(elide(&title, 48))
                    .font(theme::bold(28.0))
                    .color(theme::TEXT),
            );
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(meta)
                        .font(theme::mono(12.5))
                        .color(theme::TEXT_DIM),
                );
                ui.label(
                    egui::RichText::new(elide(&out_dir.display().to_string(), 48))
                        .font(theme::mono(12.5))
                        .color(theme::TEXT_FAINT),
                );
            });
            ui.add_space(6.0);
            super::analysis_bar::chips(app, ui);
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            new_split = Button::ghost("New split")
                .icon(Icon::Plus)
                .show(ui)
                .clicked();
            export_all = Button::new("Export stems")
                .icon(Icon::Export)
                .show(ui)
                .clicked();
            open_folder = Button::new("Open folder")
                .icon(Icon::Folder)
                .show(ui)
                .clicked();
            ui.add_space(8.0);
            buttons::segmented(
                ui,
                "results-view",
                &mut chop_open,
                &[(false, "Stems"), (true, "Chop")],
            );
        });
    });
    ui.add_space(14.0);
    if let Some(s) = &mut app.session
        && s.chop.open != chop_open
    {
        s.chop.open = chop_open;
        if chop_open {
            s.chop.opened(s.selection);
        }
    }
    if chop_open {
        super::chop::page(app, ui);
        header_actions(app, &out_dir, open_folder, export_all, new_split);
        return;
    }

    let playhead = app.player.position();
    let playing = app.player.is_playing();
    let snap = app.settings.snap;
    let Some(s) = &mut app.session else { return };
    let dur = s.duration;
    if playing {
        s.timeline.follow(playhead);
    }

    // ---- ruler
    let full_w = ui.available_width();
    let (ruler, _) = ui.allocate_exact_size(vec2(full_w, 22.0), Sense::hover());
    let lane_x = ruler.left() + CONTROLS_W + 18.0..=ruler.right() - 18.0;
    // Ctrl+wheel zoom / wheel scroll over the lanes, before anything is drawn with the view.
    let lanes_area = Rect::from_min_size(
        pos2(ruler.left(), ruler.top()),
        vec2(full_w, 26.0 + s.tracks.len() as f32 * (LANE_H + 26.0)),
    );
    s.timeline
        .handle_input(ui, lanes_area, *lane_x.start(), *lane_x.end());
    let view = s.timeline.clone();
    {
        let p = ui.painter();
        let (x0, x1) = (*lane_x.start(), *lane_x.end());
        let span = view.span();
        let step = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0]
            .into_iter()
            .find(|st| span / st <= 12.0)
            .unwrap_or(120.0);
        let mut tsec = (view.start / step).ceil() * step;
        while tsec <= view.end + 1e-6 {
            let x = x0 + view.frac_of(tsec) as f32 * (x1 - x0);
            p.vline(
                x,
                ruler.bottom() - 6.0..=ruler.bottom(),
                Stroke::new(1.0, theme::TEXT_FAINT),
            );
            p.text(
                pos2(x + 3.0, ruler.center().y - 2.0),
                Align2::LEFT_CENTER,
                if step < 1.0 {
                    format!(
                        "{}.{}",
                        mmss(tsec),
                        ((tsec.fract() * 10.0).round() as u32) % 10
                    )
                } else {
                    mmss(tsec)
                },
                theme::mono(10.5),
                theme::TEXT_FAINT,
            );
            tsec += step;
        }
        let px = x0 + view.frac_of(playhead).clamp(0.0, 1.0) as f32 * (x1 - x0);
        let tri = vec![
            pos2(px - 6.0, ruler.bottom() - 9.0),
            pos2(px + 6.0, ruler.bottom() - 9.0),
            pos2(px, ruler.bottom()),
        ];
        p.add(egui::Shape::convex_polygon(
            tri,
            Color32::WHITE,
            Stroke::NONE,
        ));
        p.text(
            pos2(ruler.left(), ruler.center().y),
            Align2::LEFT_CENTER,
            if view.is_zoomed() {
                format!("STEMS  ·  {:.0}×", view.zoom())
            } else {
                "STEMS".to_owned()
            },
            theme::semi(10.5),
            theme::TEXT_FAINT,
        );
    }
    let grid = s.analysis.as_ref().and_then(|a| a.grid.as_ref());
    ui.add_space(4.0);

    // ---- stem cards
    let any_solo = s.any_solo();
    let mut seek = None;
    let mut new_sel: Option<(usize, Option<(f64, f64)>)> = None;
    let mut drag_out: Option<(std::path::PathBuf, PillAction)> = None;
    for i in 0..s.tracks.len() {
        let audible = s.audible(i);
        let selected_lane = s.selection.is_some() && s.sel_track == i;
        let tr = &mut s.tracks[i];
        let color = theme::stem_color(tr.kind);
        let (card, card_resp) = ui.allocate_exact_size(vec2(full_w, LANE_H + 20.0), Sense::hover());
        let hover =
            ui.ctx()
                .animate_bool_with_time(Id::unique(("card", i)), card_resp.hovered(), 0.15);
        let sel_a =
            ui.ctx()
                .animate_bool_with_time(Id::unique(("card-sel", i)), selected_lane, 0.2);
        {
            let p = ui.painter();
            if sel_a > 0.0 {
                paint::glow(
                    p,
                    card,
                    CornerRadius::same(16),
                    theme::alpha(color, 0.18 * sel_a),
                    18.0,
                );
            }
            p.rect(
                card,
                CornerRadius::same(16),
                theme::lerp_color(theme::CARD, Color32::from_rgb(24, 24, 40), hover),
                Stroke::new(
                    1.0,
                    theme::lerp_color(theme::STROKE, color, 0.15 + 0.5 * sel_a + 0.15 * hover),
                ),
                StrokeKind::Inside,
            );
            // Colour edge.
            let edge =
                Rect::from_min_size(card.min + vec2(0.0, 14.0), vec2(3.0, card.height() - 28.0));
            p.rect_filled(
                edge,
                CornerRadius::same(2),
                theme::alpha(color, if audible { 1.0 } else { 0.3 }),
            );
        }

        // Left controls.
        let ctrl = Rect::from_min_size(
            card.min + vec2(18.0, 14.0),
            vec2(CONTROLS_W - 18.0, LANE_H - 8.0),
        );
        let mut cui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(ctrl)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        {
            let p = cui.painter().clone();
            // Name + meter.
            let name_pos = pos2(ctrl.left() + 2.0, ctrl.top() + 10.0);
            p.text(
                name_pos,
                Align2::LEFT_CENTER,
                tr.kind.label(),
                theme::semi(16.0),
                if audible {
                    theme::TEXT
                } else {
                    theme::TEXT_FAINT
                },
            );
            let file = tr
                .path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            p.text(
                pos2(ctrl.left() + 2.0, ctrl.top() + 30.0),
                Align2::LEFT_CENTER,
                file,
                theme::mono(11.0),
                theme::TEXT_FAINT,
            );
            // Level meter: 10 segments.
            let lvl = if audible { tr.level.min(1.0) } else { 0.0 };
            for seg in 0..10 {
                let f = seg as f32 / 10.0;
                let r = Rect::from_min_size(
                    pos2(ctrl.right() - 60.0 + seg as f32 * 6.0, ctrl.top() + 4.0),
                    vec2(4.0, 12.0),
                );
                let on = lvl > f + 0.02;
                let c = if seg >= 8 {
                    theme::BAD
                } else if seg >= 6 {
                    theme::WARN
                } else {
                    color
                };
                p.rect_filled(
                    r,
                    CornerRadius::same(1),
                    if on { c } else { Color32::from_white_alpha(14) },
                );
            }
        }
        // Drag the stem file straight into a DAW / Explorer.
        let (label, tip) = drag_pill_text();
        let pill = drag_handle::pill_rect(
            ctrl.right(),
            ctrl.top() + 30.0,
            drag_handle::pill_width(ui, label),
        );
        if let (_, Some(a)) =
            drag_handle::drag_pill(ui, pill, Id::unique(("drag", i)), label, color, tip)
        {
            drag_out = Some((tr.path.clone(), a));
        }
        cui.add_space(44.0);
        cui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if buttons::letter_toggle(ui, "S", 28.0, tr.solo, theme::WARN, "Solo").clicked() {
                tr.solo = !tr.solo;
            }
            if buttons::letter_toggle(ui, "M", 28.0, tr.mute, theme::BAD, "Mute").clicked() {
                tr.mute = !tr.mute;
            }
            ui.add_space(4.0);
            volume_slider(
                ui,
                Id::unique(("vol", i)),
                &mut tr.volume,
                color,
                ui.available_width() - 6.0,
            );
        });
        let _ = any_solo;

        // Waveform lane.
        let lane_rect =
            Rect::from_x_y_ranges(lane_x.clone(), card.top() + 10.0..=card.bottom() - 10.0);
        let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(lane_rect));
        let action = Lane {
            id: Id::unique(("lane", i)),
            wave: crate::timeline::WaveSource {
                env: &tr.env,
                pyramid: view.pyramid(i),
                audio: s.audio.get(i),
            },
            view: &view,
            grid,
            snap,
            color,
            duration: dur,
            playhead,
            selection: s.selection,
            audible,
            height: lane_rect.height(),
            level: tr.level,
            sel_label: selected_lane,
        }
        .show(&mut lui);
        match action {
            Some(LaneAction::Seek(t)) => {
                seek = Some(t);
                if let Some((a, b)) = s.selection
                    && (t < a || t > b)
                {
                    new_sel = Some((i, None));
                }
            }
            Some(LaneAction::Select(a, b)) => new_sel = Some((i, Some((a, b)))),
            None => {}
        }
        ui.add_space(6.0);
    }
    if let Some((i, sel)) = new_sel {
        s.selection = sel;
        s.sel_track = i;
        if let Some((a, _)) = sel {
            seek = Some(a);
        }
    }
    if let Some(t) = seek {
        app.player.seek(t);
    }
    if let Some((path, a)) = drag_out {
        app.file_pill(path, a);
    }

    ui.add_space(8.0);
    if app.player.error.is_some() {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 16.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, theme::WARN);
            ui.label(
                egui::RichText::new(
                    "No audio output device — playback is off, sample export still works.",
                )
                .font(theme::regular(12.5))
                .color(theme::TEXT_DIM),
            );
        });
    } else {
        ui.label(
            egui::RichText::new("Drag across a waveform to select a sample (Alt: no snap)  ·  click to seek  ·  Ctrl+wheel to zoom  ·  Space to play / pause")
                .font(theme::regular(12.5))
                .color(theme::TEXT_FAINT),
        );
    }

    header_actions(app, &out_dir, open_folder, export_all, new_split);
}

fn header_actions(
    app: &mut StemApp,
    out_dir: &std::path::Path,
    open_folder: bool,
    export_all: bool,
    new_split: bool,
) {
    if open_folder {
        app.open_folder(out_dir);
    }
    if export_all {
        app.export_all_stems();
    }
    if new_split {
        app.player.unload();
        app.session = None;
    }
}

/// Label and tooltip for drag pills: a real drag on Windows, "reveal" elsewhere.
fn drag_pill_text() -> (&'static str, &'static str) {
    if crate::dragout::NATIVE {
        (
            "Drag",
            "Drag into your DAW or a folder · click to show in Explorer",
        )
    } else {
        ("Show", "Show the file in its folder")
    }
}

/// Thin horizontal volume fader with a stem-coloured fill. 0..=1.5 (up to +3.5 dB).
fn volume_slider(ui: &mut egui::Ui, id: Id, value: &mut f32, color: Color32, width: f32) {
    let (rect, resp) = ui.allocate_exact_size(vec2(width.max(40.0), 28.0), Sense::click_and_drag());
    let resp = resp.on_hover_cursor(CursorIcon::ResizeHorizontal);
    let max = 1.5;
    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        *value = (((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0)) * max;
    }
    if resp.double_clicked() {
        *value = 1.0;
    }
    let h = ui
        .ctx()
        .animate_bool_with_time(id, resp.hovered() || resp.dragged(), 0.12);
    let p = ui.painter();
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
    p.rect_filled(track, CornerRadius::same(2), Color32::from_white_alpha(18));
    let unity = track.left() + track.width() / max;
    p.vline(
        unity,
        track.y_range().expand(4.0),
        Stroke::new(1.0, Color32::from_white_alpha(40)),
    );
    let kx = track.left() + track.width() * (*value / max).clamp(0.0, 1.0);
    let fill = Rect::from_min_max(track.min, pos2(kx, track.max.y));
    p.rect_filled(fill, CornerRadius::same(2), color);
    paint::glow_circle(
        p,
        pos2(kx, track.center().y),
        7.0,
        theme::alpha(color, 0.3 + 0.4 * h),
        8.0,
    );
    p.circle_filled(pos2(kx, track.center().y), 6.0 + 1.5 * h, Color32::WHITE);
    if h > 0.01 {
        let db = if *value <= 0.0001 {
            "-inf".to_owned()
        } else {
            format!("{:+.1} dB", 20.0 * value.log10())
        };
        p.text(
            pos2(kx, rect.top() - 2.0),
            Align2::CENTER_BOTTOM,
            db,
            theme::mono(10.5),
            theme::alpha(theme::TEXT, h),
        );
    }
    resp.on_hover_text("Drag to set volume, double-click for 0 dB");
}

/// Bottom transport bar: play / pause, time, loop, selection export.
pub fn transport(app: &mut StemApp, ui: &mut egui::Ui) {
    egui::Panel::bottom("transport")
        .exact_size(78.0 + 20.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(theme::BG_PANEL)
                .inner_margin(egui::Margin::symmetric(24, 0)),
        )
        .show(ui, |ui| {
            let rect = ui.max_rect();
            ui.painter().hline(
                rect.x_range().expand(24.0),
                rect.top(),
                Stroke::new(1.0, theme::STROKE),
            );
            super::analysis_bar::overview(app, ui, super::analysis_bar::overview_rect(rect));
            ui.add_space(20.0);
            let playing = app.player.is_playing();
            let t = ui.input(|i| i.time);
            let pos = app.player.position();
            let Some(s) = &app.session else { return };
            let dur = s.duration;
            let selection = s.selection;
            let sel_kind = s.tracks.get(s.sel_track).map(|t| t.kind);
            let audible: Vec<usize> = (0..s.tracks.len()).filter(|i| s.audible(*i)).collect();
            let sel_track = s.sel_track;

            ui.horizontal_centered(|ui| {
                // Play button.
                let (r, resp) = ui.allocate_exact_size(vec2(50.0, 50.0), Sense::click());
                let resp = resp
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text("Play / pause (Space)");
                let h = ui
                    .ctx()
                    .animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                let p = ui.painter();
                let pulse = if playing {
                    paint::pulse(t, 0.6667)
                } else {
                    0.0
                };
                paint::glow_circle(
                    p,
                    r.center(),
                    24.0,
                    theme::alpha(theme::PINK, 0.3 + 0.25 * h + 0.25 * pulse),
                    14.0 + 8.0 * pulse,
                );
                let u = if playing { (t * 0.2) as f32 } else { 0.0 };
                paint::gradient_rect(
                    p,
                    r.shrink(1.0 - h),
                    CornerRadius::same(25),
                    u,
                    u + 1.0,
                    Color32::WHITE,
                );
                let ic = if playing { Icon::Pause } else { Icon::Play };
                let off = if playing { 0.0 } else { 1.5 };
                icons::paint(
                    p,
                    Rect::from_center_size(r.center() + vec2(off, 0.0), vec2(20.0, 20.0)),
                    ic,
                    Color32::WHITE,
                );
                if resp.clicked() {
                    app.player.toggle();
                }
                ui.add_space(4.0);
                if buttons::icon_button(ui, Icon::Stop, 36.0, false, theme::VIOLET, "Stop")
                    .clicked()
                {
                    app.player.set_playing(false);
                    app.player.seek(selection.map_or(0.0, |s| s.0));
                }
                ui.add_space(10.0);
                // Time.
                ui.vertical(|ui| {
                    ui.add_space(14.0);
                    ui.label(
                        egui::RichText::new(mmss_ms(pos))
                            .font(theme::mono(20.0))
                            .color(theme::TEXT),
                    );
                    ui.label(
                        egui::RichText::new(format!("/ {}", mmss(dur)))
                            .font(theme::mono(11.5))
                            .color(theme::TEXT_FAINT),
                    );
                });
                ui.add_space(14.0);
                let mut lp = app.settings.loop_region;
                if buttons::icon_button(ui, Icon::Loop, 36.0, lp, theme::CYAN, "Loop selection")
                    .clicked()
                {
                    lp = !lp;
                    app.settings.loop_region = lp;
                }
                ui.add_space(14.0);

                // Selection info.
                if let Some((a, b)) = selection {
                    let sel_rect = ui.available_rect_before_wrap();
                    let _ = sel_rect;
                    ui.vertical(|ui| {
                        ui.add_space(15.0);
                        ui.label(
                            egui::RichText::new("SELECTION")
                                .font(theme::semi(10.5))
                                .color(theme::TEXT_FAINT),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "{} → {}   {:.2} s",
                                mmss_ms(a),
                                mmss_ms(b),
                                b - a
                            ))
                            .font(theme::mono(13.0))
                            .color(theme::TEXT),
                        );
                    });
                    if buttons::icon_button(
                        ui,
                        Icon::Close,
                        28.0,
                        false,
                        theme::BAD,
                        "Clear selection",
                    )
                    .clicked()
                        && let Some(s) = &mut app.session
                    {
                        s.selection = None;
                    }
                } else {
                    ui.label(
                        egui::RichText::new("Drag on a waveform to select a sample")
                            .font(theme::regular(13.0))
                            .color(theme::TEXT_FAINT),
                    );
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let has = selection.is_some();
                    let label = format!("Export {} sample", sel_kind.map_or("stem", |k| k.label()));
                    if Button::primary(&label)
                        .icon(Icon::Scissors)
                        .enabled(has)
                        .height(42.0)
                        .show(ui)
                        .clicked()
                    {
                        app.export_selection(vec![sel_track]);
                    }
                    if Button::new("Export mix")
                        .icon(Icon::Headphones)
                        .enabled(has && !audible.is_empty())
                        .tooltip("Export the selection with every audible stem mixed down")
                        .height(42.0)
                        .show(ui)
                        .clicked()
                    {
                        app.export_selection(audible.clone());
                    }
                    if has {
                        let label = if crate::dragout::NATIVE {
                            "Drag sample"
                        } else {
                            "Show sample"
                        };
                        let w = drag_handle::pill_width(ui, label) + 8.0;
                        let (r, _) = ui.allocate_exact_size(vec2(w, 34.0), Sense::hover());
                        let tip = if crate::dragout::NATIVE {
                            "Drag the selected sample into your DAW (saved to Samples first)"
                        } else {
                            "Save the selected sample and show it in its folder"
                        };
                        if let (_, Some(a)) = drag_handle::drag_pill(
                            ui,
                            r,
                            Id::unique("drag-sample"),
                            label,
                            sel_kind.map_or(theme::VIOLET, theme::stem_color),
                            tip,
                        ) {
                            app.selection_pill(vec![sel_track], a);
                        }
                    }
                });
            });
        });
}
