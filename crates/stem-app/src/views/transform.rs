//! The Transform panel under the stem cards: tempo (target BPM or %), pitch (semitones +
//! cents, or "to key"), keylock / varispeed, stretch algorithm, reverse; Preview renders in
//! the background and auditions the result, Export writes it next to the other samples.

use eframe::egui::{self, Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, pos2, vec2};
use stem_audio::stretch::{Algorithm, SpeedMode};

use crate::analysis::bpm_label;
use crate::app::StemApp;
use crate::backend::mmss_ms;
use crate::fx::{self, Purpose, Target, TempoUnit};
use crate::theme;
use crate::widgets::buttons::{self, Button};
use crate::widgets::drag_handle::{self, PillAction};
use crate::widgets::icons::{self, Icon};
use crate::widgets::{overline, progress};

const COL_GAP: f32 = 28.0;

pub fn panel(app: &mut StemApp, ui: &mut egui::Ui) {
    let Some(s) = &app.session else { return };
    let now = ui.input(|i| i.time);
    let kinds: Vec<_> = s.tracks.iter().map(|t| t.kind).collect();
    let has_sel = s.selection.is_some();
    let sel_len = s.selection.map(|(a, b)| b - a);
    let duration = s.duration;
    let ana = s.analysis.clone();
    let bpm = ana.as_ref().and_then(|a| a.bpm());
    let key = ana.as_ref().and_then(|a| a.key);
    if app.fx.ui.stem >= kinds.len() {
        app.fx.ui.stem = 0;
    }
    // Follow the lane the selection was made on.
    if has_sel && app.fx.ui.target == Target::Selection {
        app.fx.ui.stem = s.sel_track.min(kinds.len().saturating_sub(1));
    }
    let stem_kind = kinds.get(app.fx.ui.stem).copied();
    let color = stem_kind.map_or(theme::VIOLET, theme::stem_color);

    let mut preview = false;
    let mut export = false;
    let mut cancel = false;
    let mut drag: Option<PillAction> = None;
    let busy = app.fx.render.as_ref().map(|r| (r.run.frac(), r.purpose));
    let current = fx::current_key(app);
    let cached = match (&current, &app.fx.last) {
        (Ok(k), Some(l)) => l.key == *k,
        _ => false,
    };
    let playing = app.fx.audition.playing("transform");
    let play_frac = app.fx.audition.progress();

    let resp = theme::card_frame()
        .inner_margin(egui::Margin::symmetric(22, 18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let u = &mut app.fx.ui;
            // ---- header
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
                ui.painter()
                    .circle_filled(r.center(), 15.0, theme::alpha(color, 0.16));
                icons::paint(ui.painter(), r.shrink(7.0), Icon::Sparkle, color);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Transform")
                            .font(theme::semi(17.0))
                            .color(theme::TEXT),
                    );
                    ui.label(
                        egui::RichText::new(
                            "Re-tempo, re-pitch or flip a sample. Renders in the background; the stems stay untouched.",
                        )
                        .font(theme::regular(12.5))
                        .color(theme::TEXT_DIM),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("tf-stem")
                        .selected_text(stem_kind.map_or("Stem", |k| k.label()))
                        .width(130.0)
                        .show_ui(ui, |ui| {
                            for (i, k) in kinds.iter().enumerate() {
                                ui.selectable_value(&mut u.stem, i, k.label());
                            }
                        });
                    ui.add_space(6.0);
                    let sel_label = match sel_len {
                        Some(l) => format!("Selection {l:.2} s"),
                        None => "Selection".to_owned(),
                    };
                    let whole_label = format!("Whole stem {}", crate::backend::mmss(duration));
                    buttons::segmented(
                        ui,
                        "tf-target",
                        &mut u.target,
                        &[(Target::Selection, &sel_label), (Target::Whole, &whole_label)],
                    );
                });
            });
            ui.add_space(14.0);

            // ---- controls
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // Tempo.
                ui.vertical(|ui| {
                    ui.set_width(250.0);
                    overline(ui, "Tempo");
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if bpm.is_some() {
                            buttons::segmented(
                                ui,
                                "tf-unit",
                                &mut u.unit,
                                &[(TempoUnit::Bpm, "BPM"), (TempoUnit::Percent, "%")],
                            );
                        } else {
                            u.unit = TempoUnit::Percent;
                        }
                        let dv = match (u.unit, bpm) {
                            (TempoUnit::Bpm, Some(_)) => egui::DragValue::new(&mut u.bpm)
                                .range(20.0..=400.0)
                                .speed(0.1)
                                .max_decimals(1)
                                .suffix(" bpm"),
                            _ => egui::DragValue::new(&mut u.percent)
                                .range(25.0..=400.0)
                                .speed(0.2)
                                .max_decimals(1)
                                .suffix(" %"),
                        };
                        big_drag(ui, dv);
                    });
                    ui.add_space(4.0);
                    let factor = match (u.unit, bpm) {
                        (TempoUnit::Bpm, Some(b)) if u.bpm > 0.0 => u.bpm / b,
                        _ => u.percent / 100.0,
                    };
                    ui.horizontal(|ui| {
                        let txt = match bpm {
                            Some(b) => format!("detected {} bpm  ·  ×{factor:.3}", bpm_label(b)),
                            None => format!("no tempo detected  ·  ×{factor:.3}"),
                        };
                        faint(ui, &txt);
                        if (factor - 1.0).abs() > 1e-4
                            && link(ui, "reset").clicked()
                        {
                            u.percent = 100.0;
                            if let Some(b) = bpm {
                                u.bpm = (b * 10.0).round() / 10.0;
                            }
                        }
                    });
                });
                ui.add_space(COL_GAP);
                divider(ui);
                ui.add_space(COL_GAP);

                // Pitch.
                ui.vertical(|ui| {
                    ui.set_width(270.0);
                    overline(ui, "Pitch");
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        big_drag(
                            ui,
                            egui::DragValue::new(&mut u.semis)
                                .range(-12..=12)
                                .speed(0.05)
                                .custom_formatter(|v, _| format!("{v:+.0} st")),
                        );
                        big_drag(
                            ui,
                            egui::DragValue::new(&mut u.cents)
                                .range(-50..=50)
                                .speed(0.3)
                                .custom_formatter(|v, _| format!("{v:+.0} ct")),
                        );
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| match key {
                        Some(k) => {
                            let to = fx::transpose_key(&k, u.semis as f64 + u.cents as f64 / 100.0);
                            faint(ui, &format!("{} → ", k.short()));
                            egui::ComboBox::from_id_salt("tf-key")
                                .selected_text(
                                    egui::RichText::new(to.short()).font(theme::mono(12.5)),
                                )
                                .width(70.0)
                                .show_ui(ui, |ui| {
                                    for target in fx::keys_in(k.mode) {
                                        let d = fx::semitones_to(&k, target.tonic);
                                        let label = format!("{:<4} {d:+}", target.short());
                                        if ui
                                            .selectable_label(
                                                to.tonic == target.tonic && u.cents == 0,
                                                egui::RichText::new(label).font(theme::mono(12.5)),
                                            )
                                            .clicked()
                                        {
                                            u.semis = d;
                                            u.cents = 0;
                                        }
                                    }
                                });
                            if (u.semis != 0 || u.cents != 0) && link(ui, "reset").clicked() {
                                u.semis = 0;
                                u.cents = 0;
                            }
                        }
                        None => faint(ui, "no key detected"),
                    });
                });
                ui.add_space(COL_GAP);
                divider(ui);
                ui.add_space(COL_GAP);

                // Mode.
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    overline(ui, "Mode");
                    ui.add_space(4.0);
                    buttons::segmented(
                        ui,
                        "tf-mode",
                        &mut u.mode,
                        &[(SpeedMode::Keylock, "Keylock"), (SpeedMode::Varispeed, "Varispeed")],
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if u.mode == SpeedMode::Keylock {
                            buttons::segmented(
                                ui,
                                "tf-algo",
                                &mut u.algorithm,
                                &[
                                    (Algorithm::Auto, "Auto"),
                                    (Algorithm::Tonal, "Tonal"),
                                    (Algorithm::Percussive, "Drums"),
                                ],
                            );
                        } else {
                            faint(ui, "pitch follows speed, like a sampler");
                        }
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        buttons::toggle(ui, &mut u.reverse);
                        ui.label(
                            egui::RichText::new("Reverse")
                                .font(theme::regular(13.5))
                                .color(theme::TEXT),
                        );
                    });
                });

                // Actions.
                ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                    let t = u.transform(bpm, stem_kind);
                    let summary = summary_text(&t, bpm, &kinds, u, sel_len.filter(|_| u.target == Target::Selection).unwrap_or(duration));
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(summary)
                                .font(theme::mono(12.5))
                                .color(theme::TEXT),
                        )
                        .extend(),
                    );
                    ui.add_space(8.0);
                    match busy {
                        Some((frac, purpose)) => {
                            ui.horizontal(|ui| {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if Button::new("Cancel")
                                        .kind(buttons::Kind::Danger)
                                        .height(36.0)
                                        .show(ui)
                                        .clicked()
                                    {
                                        cancel = true;
                                    }
                                    let (r, _) = ui.allocate_exact_size(vec2(200.0, 36.0), Sense::hover());
                                    let bar = Rect::from_center_size(
                                        pos2(r.center().x, r.bottom() - 6.0),
                                        vec2(r.width(), 6.0),
                                    );
                                    progress::bar(ui.painter(), bar, Some(frac), now);
                                    ui.painter().text(
                                        pos2(r.left(), r.top() + 10.0),
                                        Align2::LEFT_CENTER,
                                        format!(
                                            "{} {:.0}%",
                                            if purpose == Purpose::Preview { "Rendering preview" } else { "Rendering export" },
                                            frac * 100.0
                                        ),
                                        theme::regular(12.0),
                                        theme::TEXT_DIM,
                                    );
                                });
                            });
                        }
                        None => {
                            ui.horizontal(|ui| {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let enabled = current.is_ok();
                                    let tip = match &current {
                                        Ok(_) => "Write the result to Samples (selection) or Export (whole stem)",
                                        Err(why) => why,
                                    };
                                    if Button::primary("Export")
                                        .icon(Icon::Export)
                                        .enabled(enabled)
                                        .tooltip(tip)
                                        .height(40.0)
                                        .show(ui)
                                        .clicked()
                                    {
                                        export = true;
                                    }
                                    let label = if playing { "Stop" } else { "Preview" };
                                    if Button::new(label)
                                        .icon(if playing { Icon::Stop } else { Icon::Play })
                                        .enabled(enabled)
                                        .tooltip(if cached { "Play the rendered result again" } else { tip })
                                        .height(40.0)
                                        .show(ui)
                                        .clicked()
                                    {
                                        preview = true;
                                    }
                                    if let Some(p) = &app.fx.last_export {
                                        let label = if crate::dragout::NATIVE { "Drag" } else { "Show" };
                                        let w = drag_handle::pill_width(ui, label);
                                        let (r, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
                                        let pill = drag_handle::pill_rect(r.right(), r.center().y, w);
                                        let tip = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                                        if let (_, Some(a)) = drag_handle::drag_pill(ui, pill, Id::unique("tf-drag"), label, color, &tip) {
                                            drag = Some(a);
                                        }
                                    }
                                });
                            });
                        }
                    }
                    ui.add_space(6.0);
                    // Rendered result / playback line.
                    let info = match (&app.fx.last, cached) {
                        (Some(l), true) => {
                            let dur = l.audio.duration_secs();
                            if playing {
                                format!(
                                    "playing {} / {}",
                                    mmss_ms(play_frac as f64 * dur),
                                    mmss_ms(dur)
                                )
                            } else {
                                format!("rendered {} in {:.1} s", mmss_ms(dur), l.secs)
                            }
                        }
                        _ => match &current {
                            Err(why) => why.to_string(),
                            Ok(_) => "not rendered yet".to_owned(),
                        },
                    };
                    faint(ui, &info);
                    if playing {
                        let (r, _) = ui.allocate_exact_size(vec2(200.0, 3.0), Sense::hover());
                        ui.painter().rect_filled(r, CornerRadius::same(1), Color32::from_white_alpha(14));
                        ui.painter().rect_filled(
                            Rect::from_min_size(r.min, vec2(r.width() * play_frac.clamp(0.0, 1.0), 3.0)),
                            CornerRadius::same(1),
                            color,
                        );
                    }
                });
            });
        });

    if app.fx.scroll_to_panel {
        // Keep asking until the panel is on screen (the first frames have no layout yet).
        if ui.clip_rect().contains_rect(resp.response.rect) {
            app.fx.scroll_to_panel = false;
        } else {
            ui.scroll_to_rect(resp.response.rect, Some(egui::Align::Max));
        }
    }
    if cancel {
        app.fx.cancel_render();
    }
    if preview {
        if playing {
            app.fx.audition.stop();
        } else {
            fx::start(app, Purpose::Preview, now);
        }
    }
    if export {
        fx::start(app, Purpose::Export, now);
    }
    if let Some(a) = drag
        && let Some(p) = app.fx.last_export.clone()
    {
        app.file_pill(p, a);
    }
}

/// "→ 95 bpm · +3 st · 2.53 s".
fn summary_text(
    t: &stem_audio::stretch::Transform,
    bpm: Option<f32>,
    _kinds: &[stem_models::StemKind],
    _u: &fx::TransformUi,
    src_len: f64,
) -> String {
    let mut parts = Vec::new();
    let tempo = t.tempo_factor();
    match bpm {
        Some(b) => parts.push(format!("{} bpm", bpm_label(b * tempo as f32))),
        None => parts.push(format!("{:.1}%", tempo * 100.0)),
    }
    let st = t.pitch_semitones();
    parts.push(if st.abs() < 0.005 {
        "±0 st".to_owned()
    } else {
        format!("{st:+.2} st").replace(".00", "")
    });
    parts.push(format!("{:.2} s", src_len * t.length_factor()));
    if t.reverse {
        parts.push("reversed".into());
    }
    format!("→ {}", parts.join("  ·  "))
}

/// Larger DragValue with the mono font on a raised surface.
fn big_drag(ui: &mut egui::Ui, dv: egui::DragValue<'_>) {
    ui.scope(|ui| {
        ui.style_mut().override_font_id = Some(theme::mono(16.0));
        ui.spacing_mut().interact_size = vec2(84.0, 34.0);
        ui.add_sized(vec2(96.0, 34.0), dv)
            .on_hover_text("Drag or click to type");
    });
}

fn faint(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .font(theme::regular(12.0))
            .color(theme::TEXT_FAINT),
    );
}

fn link(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::semi(12.0))
                .color(theme::VIOLET),
        )
        .sense(Sense::click()),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn divider(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(1.0, 92.0), Sense::hover());
    ui.painter()
        .vline(r.center().x, r.y_range(), Stroke::new(1.0, theme::STROKE));
}
