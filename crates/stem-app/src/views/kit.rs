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
use crate::theme::{self, motion};
use crate::widgets::buttons::{self, Button};
use crate::widgets::drag_handle::{self, PillAction};
use crate::widgets::icons::{self, Icon};
use crate::widgets::sheet::Sheet;
use crate::widgets::toast::elide;
use crate::widgets::{paint, progress};

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
    if app.dsp.kit.is_none() {
        return;
    }
    let now = ctx.input(|i| i.time);
    let dest = fx::kit_dir(app);
    let mut close = false;
    let mut export = false;
    let mut drag: Option<PillAction> = None;
    let mut play: Option<usize> = None;
    let mut anchor: Option<egui::Pos2> = None;
    let playing_tag = if app.dsp.audition.is_playing() {
        app.dsp.audition.tag.clone()
    } else {
        None
    };
    let play_frac = app.dsp.audition.progress();
    let Some(sheet) = app.dsp.kit.as_mut() else {
        return;
    };
    let kind = sheet.kind;
    let busy_export = sheet.busy_export;
    let exported = sheet.exported.clone();
    let mut anim = Sheet::begin(ctx, Id::unique("kit-sheet"));
    let resp = anim.show(ctx, theme::stem_color(kind), 24, |ui| {
            ui.set_width(860.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                let c = theme::stem_color(kind);
                crate::widgets::icon_badge(ui.painter(), r, c, Icon::Sparkle);
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
                    let bar = Rect::from_center_size(r.center() + vec2(0.0, 14.0), vec2(r.width() * 0.6, 40.0));
                    progress::live_bar(ui.painter(), bar, run.frac(), now, theme::stem_color(kind));
                    ui.painter().text(
                        bar.center_top() - vec2(0.0, 14.0),
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
                    // As tall as the window allows, so the footer always stays on screen.
                    let list_h = (ui.ctx().content_rect().height() - 330.0).clamp(200.0, 520.0);
                    let mut order = 0usize;
                    egui::ScrollArea::vertical()
                        .max_height(list_h)
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
                                class_header(
                                    ui,
                                    class_color(class),
                                    class_title(class),
                                    &format!("{hits} hits  ·  {} kept", idx.len()),
                                );
                                ui.add_space(4.0);
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                                    for &i in &idx {
                                        order += 1;
                                        let s = &kit.samples[i];
                                        let tag = format!("kit-{i}");
                                        let is_playing = playing_tag.as_deref() == Some(tag.as_str());
                                        let r = tile(
                                            ui,
                                            i,
                                            order,
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
                    let btn = Button::primary(label)
                        .export_shimmer()
                        .icon(Icon::Export)
                        .enabled(ready && n_checked > 0 && !busy_export)
                        .height(40.0)
                        .show(ui);
                    anchor = Some(btn.rect.center());
                    if btn.clicked() {
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
    if let Some(a) = anchor {
        app.fx.set_anchor("kit-export", a);
    }
    if anim.closing {
        // Exit animation playing: the sheet is going away, ignore its buttons.
        (play, export, drag) = (None, false, None);
    }

    if let Some(i) = play {
        let tag = format!("kit-{i}");
        if app.dsp.audition.playing(&tag) {
            app.dsp.audition.stop();
        } else if let Some(fx::KitSheet {
            state: KitState::Ready { kit, .. },
            ..
        }) = &app.dsp.kit
        {
            let audio = Arc::new(kit.samples[i].audio.clone());
            app.player.set_playing(false);
            app.dsp.audition.play(audio, tag);
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
        anim.close(ctx);
    }
    if anim.finished() {
        app.dsp.close_kit();
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
/// Tiles pop in staggered (`order` = position in the sheet), lift and catch the light on
/// hover, and pulse in their class colour while they play.
#[allow(clippy::too_many_arguments)]
fn tile(
    ui: &mut egui::Ui,
    i: usize,
    order: usize,
    name: &str,
    meta: &str,
    peaks: &[(f32, f32)],
    color: Color32,
    checked: &mut bool,
    playing: Option<f32>,
) -> bool {
    let (slot, resp) = ui.allocate_exact_size(TILE, Sense::click());
    let resp = resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Click to audition");
    let ctx = ui.ctx().clone();
    let enter = motion::appear(
        &ctx,
        Id::unique(("kit-tile-in", i)),
        0.12 + order as f32 * 0.05,
        0.38,
    );
    let pop = motion::ease_out_back(enter);
    let h = ctx.animate_bool_with_time(Id::unique(("kit-tile", i)), resp.hovered(), 0.12);
    let lift = 3.0 * h;
    let rect = Rect::from_center_size(
        slot.center() - vec2(0.0, lift - 10.0 * (1.0 - pop)),
        TILE * (0.86 + 0.14 * pop),
    );
    let on = *checked;
    let mut p = ui.painter().clone();
    p.multiply_opacity(motion::ease_out(enter));
    let r = CornerRadius::same(12);
    let now = ui.input(|inp| inp.time);
    // Playing: a slow breathing glow around the tile.
    let breathe = if playing.is_some() && !motion::reduced() {
        0.5 + 0.5 * (now * 6.0).sin() as f32
    } else {
        0.0
    };
    if playing.is_some() {
        paint::glow(
            &p,
            rect,
            r,
            theme::alpha(color, 0.3 + 0.2 * breathe),
            14.0 + 6.0 * breathe,
        );
    }
    paint::glass(
        &p,
        rect,
        r,
        on.then_some(theme::alpha(color, 0.05 + 0.08 * h)),
        h,
    );
    // Class colour wash from the bottom, stronger when kept.
    paint::soft_disc(
        &p,
        pos2(rect.center().x, rect.bottom()),
        rect.width() * 0.55,
        theme::alpha(color, if on { 0.10 + 0.08 * h } else { 0.03 }),
    );
    p.rect_stroke(
        rect,
        r,
        Stroke::new(
            1.0,
            theme::lerp_color(
                theme::GLASS_STROKE,
                color,
                if on { 0.35 + 0.4 * h } else { 0.1 * h },
            ),
        ),
        StrokeKind::Inside,
    );
    if let Some(ptr) = resp.hover_pos() {
        paint::spotlight(&p, rect, ptr, color, 2.0 * h);
    }
    let sweep = ctx.animate_bool_with_time(Id::unique(("kit-tile-shine", i)), resp.hovered(), 0.6);
    if resp.hovered() {
        paint::shine(&p, rect, sweep, 1.6);
    }
    // Waveform; the played part lights up white.
    let wave = Rect::from_min_max(rect.min + vec2(10.0, 30.0), rect.max - vec2(10.0, 30.0));
    let n = peaks.len().max(1);
    let wc = theme::alpha(color, if on { 0.95 } else { 0.35 });
    for (k, (lo, hi)) in peaks.iter().enumerate() {
        let f = (k as f32 + 0.5) / n as f32;
        let x = wave.left() + f * wave.width();
        let (lo, hi) = (lo.clamp(-1.0, 1.0), hi.clamp(-1.0, 1.0));
        let y0 = wave.center().y - hi * wave.height() / 2.0;
        let y1 = wave.center().y - lo * wave.height() / 2.0;
        let c = if playing.is_some_and(|pf| f <= pf) {
            theme::lerp_color(color, Color32::WHITE, 0.6)
        } else {
            wc
        };
        p.vline(x, y0..=y1.max(y0 + 1.0), Stroke::new(1.4, c));
    }
    if let Some(f) = playing {
        let x = wave.left() + f.clamp(0.0, 1.0) * wave.width();
        paint::glow(
            &p,
            Rect::from_x_y_ranges(x - 1.0..=x + 1.0, wave.y_range()),
            0,
            theme::alpha(Color32::WHITE, 0.4),
            8.0,
        );
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
        if on {
            theme::lerp_color(theme::TEXT, Color32::WHITE, h)
        } else {
            theme::TEXT_FAINT
        },
    );
    p.text(
        pos2(rect.left() + 12.0, rect.bottom() - 14.0),
        Align2::LEFT_CENTER,
        meta,
        theme::mono(10.5),
        theme::TEXT_FAINT,
    );
    // Checkbox: pops when ticked, with a hover ring.
    let cb = Rect::from_center_size(
        pos2(rect.right() - 18.0, rect.top() + 16.0),
        vec2(18.0, 18.0),
    );
    let cresp = ui
        .interact(
            Rect::from_center_size(
                pos2(slot.right() - 18.0, slot.top() + 16.0),
                vec2(26.0, 26.0),
            ),
            Id::unique(("kit-check", i)),
            Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if on {
            "Leave out of the kit"
        } else {
            "Put in the kit"
        });
    if cresp.clicked() {
        *checked = !*checked;
    }
    let ch = ctx.animate_bool_with_time(cresp.id.with("h"), cresp.hovered(), motion::FAST);
    let k = motion::bouncy(&ctx, cresp.id.with("pop"), if *checked { 1.0 } else { 0.0 });
    let t = ctx.animate_bool_with_time(cresp.id.with("on"), *checked, motion::BASE);
    let cb = Rect::from_center_size(
        cb.center(),
        cb.size() * (1.0 + 0.2 * (k - t).abs() + 0.08 * ch),
    );
    if t > 0.0 {
        paint::glow(
            &p,
            cb,
            CornerRadius::same(5),
            theme::alpha(color, 0.45 * t),
            8.0,
        );
    }
    p.rect(
        cb,
        CornerRadius::same(5),
        theme::alpha(color, t),
        Stroke::new(
            1.2,
            theme::lerp_color(theme::lerp_color(theme::STROKE_HI, color, ch), color, t),
        ),
        StrokeKind::Inside,
    );
    if t > 0.05 {
        icons::paint(
            &p,
            Rect::from_center_size(cb.center(), cb.shrink(3.0).size() * t.max(0.4)),
            Icon::Check,
            Color32::from_rgb(12, 10, 20),
        );
    }
    paint::focus(&p, &cresp, cb, CornerRadius::same(5));
    paint::focus(&p, &resp, rect, r);
    resp.clicked() && !cresp.clicked()
}

/// Class heading: a glowing dot, the class name in its colour, then the counts.
fn class_header(ui: &mut egui::Ui, color: Color32, title: &str, counts: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
        paint::glow_circle(ui.painter(), r.center(), 3.0, theme::alpha(color, 0.6), 8.0);
        ui.painter().circle_filled(r.center(), 3.5, color);
        ui.label(
            egui::RichText::new(title.to_uppercase())
                .font(theme::semi(theme::size::CAPTION))
                .color(theme::lerp_color(color, theme::TEXT, 0.35))
                .extra_letter_spacing(1.2),
        );
        ui.label(
            egui::RichText::new(counts.to_uppercase())
                .font(theme::semi(theme::size::CAPTION))
                .color(theme::TEXT_FAINT)
                .extra_letter_spacing(1.0),
        );
    });
}
