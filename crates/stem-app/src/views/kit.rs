//! "Extract kit" sheet: the one-shots found in a drum-like stem, grouped by class, with mini
//! waveforms. Click a tile to audition it, untick what you don't want, export the folder
//! (WAVs + SFZ) or drag it straight into a DAW / sampler.

use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, pos2, vec2,
};
use stem_audio::oneshot::{HitClass, note_name};

use crate::app::StemApp;
use crate::fx::{self, KitState};
use crate::theme;
use crate::widgets::buttons::{self, Button};
use crate::widgets::drag_handle::{self, PillAction};
use crate::widgets::icons::{self, Icon};
use crate::widgets::toast::elide;
use crate::widgets::{overline, paint, progress};

const TILE: egui::Vec2 = egui::vec2(172.0, 100.0);

pub fn class_color(c: HitClass) -> Color32 {
    match c {
        HitClass::Kick => theme::PINK,
        HitClass::Snare => theme::WARN,
        HitClass::Hat => theme::CYAN,
        HitClass::Perc => theme::VIOLET,
    }
}

fn class_title(c: HitClass) -> &'static str {
    match c {
        HitClass::Kick => "Kicks",
        HitClass::Snare => "Snares & claps",
        HitClass::Hat => "Hats & cymbals",
        HitClass::Perc => "Percussion & toms",
    }
}

pub fn show(app: &mut StemApp, ctx: &egui::Context) {
    if app.fx.kit.is_none() {
        return;
    }
    let now = ctx.input(|i| i.time);
    let dest = fx::kit_dir(app);
    let mut close = false;
    let mut export = false;
    let mut drag: Option<PillAction> = None;
    let mut play: Option<usize> = None;
    let playing_tag = if app.fx.audition.is_playing() {
        app.fx.audition.tag.clone()
    } else {
        None
    };
    let play_frac = app.fx.audition.progress();
    let Some(sheet) = app.fx.kit.as_mut() else {
        return;
    };
    let kind = sheet.kind;
    let busy_export = sheet.busy_export;
    let exported = sheet.exported.clone();
    let resp = egui::Modal::new(Id::unique("kit-sheet"))
        .frame(theme::card_frame().inner_margin(egui::Margin::same(24)))
        .backdrop_color(Color32::from_black_alpha(150))
        .show(ctx, |ui| {
            ui.set_width(860.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                let c = theme::stem_color(kind);
                paint::glow_circle(ui.painter(), r.center(), 18.0, theme::alpha(c, 0.25), 10.0);
                ui.painter().circle_filled(r.center(), 20.0, theme::alpha(c, 0.18));
                icons::paint(ui.painter(), r.shrink(10.0), Icon::Sparkle, c);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Drum kit")
                            .font(theme::bold(22.0))
                            .color(theme::TEXT),
                    );
                    let sub = match &sheet.state {
                        KitState::Extracting(_) => format!("Finding hits in {}…", kind.label()),
                        KitState::Ready { kit, .. } => format!(
                            "{} hits in {}  →  {} distinct sounds. Separated stems bleed: keep the cleanest.",
                            kit.hits.len(),
                            kind.label(),
                            kit.samples.len()
                        ),
                        KitState::Failed(_) => "Extraction failed".to_owned(),
                    };
                    ui.label(
                        egui::RichText::new(sub)
                            .font(theme::regular(13.0))
                            .color(theme::TEXT_DIM),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if buttons::icon_button(ui, Icon::Close, 30.0, false, theme::BAD, "Close")
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
            ui.add_space(16.0);

            match &mut sheet.state {
                KitState::Extracting(run) => {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 120.0), Sense::hover());
                    let bar = Rect::from_center_size(r.center(), vec2(r.width() * 0.6, 8.0));
                    progress::bar(ui.painter(), bar, Some(run.frac()), now);
                    ui.painter().text(
                        bar.center_top() - vec2(0.0, 16.0),
                        Align2::CENTER_CENTER,
                        "Detecting onsets, cutting and grouping hits",
                        theme::regular(13.0),
                        theme::TEXT_DIM,
                    );
                }
                KitState::Failed(e) => {
                    ui.label(
                        egui::RichText::new(e.as_str())
                            .font(theme::regular(13.0))
                            .color(theme::BAD),
                    );
                }
                KitState::Ready {
                    kit,
                    checked,
                    peaks,
                } => {
                    if kit.samples.is_empty() {
                        ui.label(
                            egui::RichText::new(
                                "No clear hits found. Kits work best on a drums stem.",
                            )
                            .font(theme::regular(13.5))
                            .color(theme::TEXT_DIM),
                        );
                    }
                    let sr = kit.samples.first().map_or(44_100, |s| s.audio.sample_rate);
                    egui::ScrollArea::vertical()
                        .max_height(470.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for class in HitClass::ALL {
                                let idx: Vec<usize> = (0..kit.samples.len())
                                    .filter(|&i| kit.samples[i].class == class)
                                    .collect();
                                if idx.is_empty() {
                                    continue;
                                }
                                let hits = kit.hits.iter().filter(|h| h.class == class).count();
                                ui.horizontal(|ui| {
                                    let (r, _) = ui.allocate_exact_size(vec2(8.0, 14.0), Sense::hover());
                                    ui.painter().circle_filled(r.center(), 3.5, class_color(class));
                                    overline(
                                        ui,
                                        &format!("{}  ·  {} hits  ·  {} kept", class_title(class), hits, idx.len()),
                                    );
                                });
                                ui.add_space(4.0);
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                                    for &i in &idx {
                                        let s = &kit.samples[i];
                                        let tag = format!("kit-{i}");
                                        let is_playing = playing_tag.as_deref() == Some(tag.as_str());
                                        let r = tile(
                                            ui,
                                            i,
                                            &s.name,
                                            &format!(
                                                "×{}  ·  {}  ·  {:.2} s",
                                                s.count,
                                                note_name(s.note),
                                                s.audio.frames() as f32 / sr as f32
                                            ),
                                            &peaks[i],
                                            class_color(class),
                                            &mut checked[i],
                                            is_playing.then_some(play_frac),
                                        );
                                        if r {
                                            play = Some(i);
                                        }
                                    }
                                });
                                ui.add_space(14.0);
                            }
                        });
                }
            }

            // Footer.
            ui.add_space(4.0);
            ui.painter().hline(
                ui.max_rect().x_range(),
                ui.cursor().top(),
                Stroke::new(1.0, theme::STROKE),
            );
            ui.add_space(14.0);
            let ready = matches!(sheet.state, KitState::Ready { .. });
            let n_checked = match &sheet.state {
                KitState::Ready { checked, .. } => checked.iter().filter(|c| **c).count(),
                _ => 0,
            };
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{n_checked} samples  ·  24-bit WAV  ·  SFZ map (kick C1, snare D1, hat F#1)"
                        ))
                        .font(theme::regular(12.5))
                        .color(theme::TEXT_DIM),
                    );
                    if let Some(d) = &dest {
                        ui.label(
                            egui::RichText::new(elide(&d.display().to_string(), 70))
                                .font(theme::mono(11.5))
                                .color(theme::TEXT_FAINT),
                        );
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if exported.is_some() { "Export again" } else { "Export kit" };
                    if Button::primary(label)
                        .icon(Icon::Export)
                        .enabled(ready && n_checked > 0 && !busy_export)
                        .height(40.0)
                        .show(ui)
                        .clicked()
                    {
                        export = true;
                    }
                    if ready && n_checked > 0 {
                        let label = if crate::dragout::NATIVE { "Drag kit folder" } else { "Show kit folder" };
                        let w = drag_handle::pill_width(ui, label) + 6.0;
                        let (r, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
                        let pill = drag_handle::pill_rect(r.right(), r.center().y, w).expand2(vec2(0.0, 4.0));
                        let tip = if crate::dragout::NATIVE {
                            "Drag the kit folder into your DAW or sampler (written first)"
                        } else {
                            "Write the kit and show its folder"
                        };
                        if let (_, Some(a)) = drag_handle::drag_pill(
                            ui,
                            pill,
                            Id::unique("kit-drag"),
                            label,
                            theme::stem_color(kind),
                            tip,
                        ) {
                            drag = Some(a);
                        }
                    }
                    if Button::ghost("Close").height(40.0).show(ui).clicked() {
                        close = true;
                    }
                });
            });
        });
    if resp.should_close() {
        close = true;
    }

    if let Some(i) = play {
        let tag = format!("kit-{i}");
        if app.fx.audition.playing(&tag) {
            app.fx.audition.stop();
        } else if let Some(fx::KitSheet {
            state: KitState::Ready { kit, .. },
            ..
        }) = &app.fx.kit
        {
            let audio = Arc::new(kit.samples[i].audio.clone());
            app.player.set_playing(false);
            app.fx.audition.play(audio, tag);
        }
    }
    if export {
        fx::export_kit(app, false);
    }
    if let Some(a) = drag
        && let Some(dir) = fx::export_kit(app, true)
    {
        match a {
            PillAction::Drag => app.drag_out(vec![dir]),
            PillAction::Click => app.open_folder(&dir),
        }
    }
    if close {
        app.fx.close_kit();
    }
}

/// Small "Kit" pill on a drum card whose right edge is at `right`. True when clicked.
pub fn kit_button(ui: &mut egui::Ui, right: f32, cy: f32, i: usize, color: Color32) -> bool {
    let label = "Kit";
    let g = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::semi(11.5), Color32::WHITE);
    let w = g.size().x + 34.0;
    let rect = Rect::from_min_size(pos2(right - w, cy - 12.0), vec2(w, 24.0));
    let id = Id::unique(("kit-btn", i));
    let resp = ui
        .interact(rect, id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Extract a drum kit: one-shots grouped into kick / snare / hat / perc");
    let h = ui.ctx().animate_bool_with_time(id, resp.hovered(), 0.12);
    let p = ui.painter();
    p.rect(
        rect,
        CornerRadius::same(12),
        theme::lerp_color(theme::alpha(color, 0.10), theme::alpha(color, 0.25), h),
        Stroke::new(1.0, theme::alpha(color, 0.45 + 0.4 * h)),
        StrokeKind::Inside,
    );
    icons::paint(
        p,
        Rect::from_center_size(pos2(rect.left() + 13.0, cy), vec2(12.0, 12.0)),
        Icon::Sparkle,
        color,
    );
    p.galley(
        pos2(rect.left() + 23.0, cy - g.size().y / 2.0),
        g,
        theme::lerp_color(theme::TEXT, Color32::WHITE, h),
    );
    resp.clicked()
}

/// One sample tile. Returns true when clicked (audition). `playing` = playback position.
#[allow(clippy::too_many_arguments)]
fn tile(
    ui: &mut egui::Ui,
    i: usize,
    name: &str,
    meta: &str,
    peaks: &[(f32, f32)],
    color: Color32,
    checked: &mut bool,
    playing: Option<f32>,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(TILE, Sense::click());
    let resp = resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Click to audition");
    let h = ui
        .ctx()
        .animate_bool_with_time(Id::unique(("kit-tile", i)), resp.hovered(), 0.12);
    let on = *checked;
    let p = ui.painter().clone();
    if playing.is_some() {
        paint::glow(
            &p,
            rect,
            CornerRadius::same(12),
            theme::alpha(color, 0.3),
            12.0,
        );
    }
    p.rect(
        rect,
        CornerRadius::same(12),
        theme::lerp_color(
            theme::CARD_HI,
            theme::alpha(color, 0.10),
            h * 0.6 + if playing.is_some() { 0.4 } else { 0.0 },
        ),
        Stroke::new(
            1.0,
            theme::lerp_color(
                theme::STROKE,
                color,
                if on { 0.35 + 0.4 * h } else { 0.1 * h },
            ),
        ),
        StrokeKind::Inside,
    );
    // Waveform.
    let wave = Rect::from_min_max(rect.min + vec2(10.0, 30.0), rect.max - vec2(10.0, 30.0));
    let n = peaks.len().max(1);
    let wc = theme::alpha(color, if on { 0.95 } else { 0.35 });
    for (k, (lo, hi)) in peaks.iter().enumerate() {
        let x = wave.left() + (k as f32 + 0.5) / n as f32 * wave.width();
        let (lo, hi) = (lo.clamp(-1.0, 1.0), hi.clamp(-1.0, 1.0));
        let y0 = wave.center().y - hi * wave.height() / 2.0;
        let y1 = wave.center().y - lo * wave.height() / 2.0;
        p.vline(x, y0..=y1.max(y0 + 1.0), Stroke::new(1.4, wc));
    }
    if let Some(f) = playing {
        let x = wave.left() + f.clamp(0.0, 1.0) * wave.width();
        p.vline(
            x,
            wave.y_range().expand(4.0),
            Stroke::new(1.5, Color32::WHITE),
        );
    }
    p.text(
        pos2(rect.left() + 12.0, rect.top() + 16.0),
        Align2::LEFT_CENTER,
        name,
        theme::semi(13.5),
        if on { theme::TEXT } else { theme::TEXT_FAINT },
    );
    p.text(
        pos2(rect.left() + 12.0, rect.bottom() - 14.0),
        Align2::LEFT_CENTER,
        meta,
        theme::mono(10.5),
        theme::TEXT_FAINT,
    );
    // Checkbox.
    let cb = Rect::from_center_size(
        pos2(rect.right() - 18.0, rect.top() + 16.0),
        vec2(18.0, 18.0),
    );
    let cresp = ui
        .interact(cb.expand(4.0), Id::unique(("kit-check", i)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if on {
            "Leave out of the kit"
        } else {
            "Put in the kit"
        });
    if cresp.clicked() {
        *checked = !*checked;
    }
    p.rect(
        cb,
        CornerRadius::same(5),
        if on { color } else { Color32::TRANSPARENT },
        Stroke::new(1.2, if on { color } else { theme::STROKE_HI }),
        StrokeKind::Inside,
    );
    if on {
        icons::paint(
            &p,
            cb.shrink(3.0),
            Icon::Check,
            Color32::from_rgb(12, 10, 20),
        );
    }
    resp.clicked() && !cresp.clicked()
}
