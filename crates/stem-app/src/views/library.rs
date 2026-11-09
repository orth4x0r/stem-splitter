//! Library: past splits, re-open or reveal in Explorer.

use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, Sense, pos2, vec2};

use crate::app::StemApp;
use crate::backend::mmss;
use crate::settings::relative_time;
use crate::theme::{self, motion};
use crate::widgets::buttons::Button;
use crate::widgets::icons::Icon;
use crate::widgets::toast::elide;
use crate::widgets::{page_title, paint};

pub fn page(app: &mut StemApp, ui: &mut egui::Ui) {
    page_title(
        ui,
        "Library",
        "Everything you've split. Re-open a session to keep chopping.",
    );
    if app.settings.history.is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 260.0), Sense::hover());
        crate::widgets::empty_state(
            ui,
            r,
            Icon::Library,
            "Nothing here yet",
            "Split a track and it shows up here, ready to re-open and keep chopping.",
        );
        ui.vertical_centered(|ui| {
            if Button::primary("Split something")
                .icon(Icon::Sparkle)
                .height(40.0)
                .show(ui)
                .clicked()
            {
                app.page = crate::app::Page::Split;
            }
        });
        return;
    }
    let now = ui.input(|i| i.time);
    let entries = app.settings.history.clone();

    // Stats row: count-up numbers.
    let total_secs: f64 = entries.iter().map(|e| e.duration_secs).sum();
    let total_stems: usize = entries.iter().map(|e| e.stems.len()).sum();
    let gap = 12.0;
    let tw = ((ui.available_width() - 2.0 * gap) / 3.0).min(240.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        crate::widgets::stat_tile(
            ui,
            vec2(tw, 72.0),
            entries.len() as f32,
            |v| format!("{v:.0}"),
            "Sessions",
            theme::PINK,
        );
        crate::widgets::stat_tile(
            ui,
            vec2(tw, 72.0),
            total_stems as f32,
            |v| format!("{v:.0}"),
            "Stems on disk",
            theme::VIOLET,
        );
        crate::widgets::stat_tile(
            ui,
            vec2(tw, 72.0),
            total_secs as f32,
            |v| mmss(v as f64),
            "Audio split",
            theme::CYAN,
        );
    });
    ui.add_space(18.0);

    let mut reopen = None;
    let mut reveal = None;
    let mut remove = None;
    let ctx = ui.ctx().clone();
    // A grid of session cards: waveform thumbnail on top, title and facts below, actions in
    // the footer. Columns follow the window width.
    let gap = 16.0;
    let full = ui.available_width();
    let cols = ((full + gap) / (CARD_MIN_W + gap)).floor().clamp(1.0, 4.0) as usize;
    let cw = (full - gap * (cols - 1) as f32) / cols as f32;
    for (row_i, row) in entries.chunks(cols).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (j, e) in row.iter().enumerate() {
                let i = row_i * cols + j;
                let (slot, resp) = ui.allocate_exact_size(vec2(cw, CARD_H), Sense::hover());
                let h = ctx.animate_bool_with_time(resp.id, resp.hovered(), motion::BASE);
                let enter = motion::ease_out(motion::appear(
                    &ctx,
                    resp.id.with("in"),
                    0.1 + i.min(12) as f32 * motion::STAGGER,
                    motion::ENTER,
                ));
                let rect = slot.translate(vec2(0.0, 18.0 * (1.0 - enter)));
                let p = ui.painter();
                paint::glass(p, rect, CornerRadius::same(16), None, h);
                // Thumbnail: one mirrored mini waveform per stem, stacked.
                let thumb = Rect::from_min_max(
                    rect.min + vec2(12.0, 12.0),
                    pos2(rect.right() - 12.0, rect.top() + 12.0 + THUMB_H),
                );
                let sweep = hover_sweep(&ctx, resp.id.with("sweep"), resp.hovered());
                thumbnail(p, thumb, e, sweep, h);
                // Duration chip on the thumbnail.
                let dur = mmss(e.duration_secs);
                let g = p.layout_no_wrap(dur, theme::mono(11.0), theme::TEXT);
                let chip = Rect::from_min_size(
                    pos2(thumb.right() - g.size().x - 16.0, thumb.bottom() - 26.0),
                    vec2(g.size().x + 10.0, 18.0),
                );
                p.rect_filled(chip, CornerRadius::same(5), Color32::from_black_alpha(170));
                p.galley(
                    pos2(chip.left() + 5.0, chip.center().y - g.size().y / 2.0),
                    g,
                    theme::TEXT,
                );
                // Text block.
                let x = rect.left() + 16.0;
                let max_chars = ((cw - 32.0) / 8.4) as usize;
                p.text(
                    pos2(x, thumb.bottom() + 22.0),
                    Align2::LEFT_CENTER,
                    elide(&e.title, max_chars),
                    theme::semi(15.0),
                    theme::TEXT,
                );
                let tag = crate::analysis::history_tag(e.bpm, e.key.as_deref());
                let meta = match tag {
                    Some(m) => format!("{m}  ·  {}", e.model_name),
                    None => e.model_name.clone(),
                };
                let small = ((cw - 32.0) / 6.6) as usize;
                p.text(
                    pos2(x, thumb.bottom() + 44.0),
                    Align2::LEFT_CENTER,
                    elide(&meta, small),
                    theme::regular(12.5),
                    theme::TEXT_DIM,
                );
                // Stem dots + when.
                let mut sx = x;
                let dy = thumb.bottom() + 64.0;
                for k in &e.stems {
                    p.circle_filled(pos2(sx + 3.5, dy), 3.5, theme::stem_color(*k));
                    sx += 11.0;
                }
                p.text(
                    pos2(sx + 6.0, dy),
                    Align2::LEFT_CENTER,
                    format!("{} stems  ·  {}", e.stems.len(), relative_time(e.when)),
                    theme::regular(12.0),
                    theme::TEXT_FAINT,
                );
                // Footer actions.
                let foot = Rect::from_min_max(
                    pos2(rect.left() + 12.0, rect.bottom() - 46.0),
                    pos2(rect.right() - 12.0, rect.bottom() - 12.0),
                );
                let mut bu = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(foot)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                bu.spacing_mut().item_spacing.x = 8.0;
                if Button::new("Open")
                    .icon(Icon::Play)
                    .height(34.0)
                    .enabled(app.job.is_none())
                    .tooltip("Re-open the session: stems, waveforms and chops")
                    .show(&mut bu)
                    .clicked()
                {
                    reopen = Some(e.clone());
                }
                if Button::new("Folder")
                    .icon(Icon::Folder)
                    .height(34.0)
                    .tooltip("Show the stems in their folder")
                    .show(&mut bu)
                    .clicked()
                {
                    reveal = Some(e.out_dir.clone());
                }
                bu.with_layout(egui::Layout::right_to_left(egui::Align::Center), |bu| {
                    if Button::ghost("")
                        .icon(Icon::Close)
                        .height(34.0)
                        .tooltip("Remove from library (keeps files)")
                        .show(bu)
                        .clicked()
                    {
                        remove = Some(i);
                    }
                });
            }
        });
        ui.add_space(gap - ui.spacing().item_spacing.y);
    }
    if let Some(e) = reopen {
        app.reopen(&e, now);
    }
    if let Some(d) = reveal {
        app.open_folder(&d);
    }
    if let Some(i) = remove {
        app.settings.history.remove(i);
    }
}

const CARD_MIN_W: f32 = 280.0;
const CARD_H: f32 = 252.0;
const THUMB_H: f32 = 104.0;

/// 0..1 position of the hover sweep across a thumbnail: starts when the pointer arrives and
/// plays once (so a resting pointer doesn't keep the app repainting). `None` when not hovered
/// or under reduced motion.
fn hover_sweep(ctx: &egui::Context, id: egui::Id, hovered: bool) -> Option<f32> {
    if !hovered {
        ctx.data_mut(|d| d.remove::<f64>(id));
        return None;
    }
    if motion::reduced() {
        return None;
    }
    let now = ctx.input(|i| i.time);
    let start = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(id, || now));
    let f = ((now - start) / 1.4) as f32;
    if f < 1.0 {
        ctx.request_repaint();
        Some(f)
    } else {
        None
    }
}

/// Deterministic 0..1 noise per (seed, n).
fn hash01(seed: u32, n: u32) -> f32 {
    let mut h = seed ^ n.wrapping_mul(0x9E37_79B9);
    h = (h ^ (h >> 15)).wrapping_mul(0x2C1B_3C6D);
    h = (h ^ (h >> 12)).wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// The session's thumbnail: a dark well with one mirrored bar-waveform per stem, in the stem
/// colours. The shapes are drawn from the session (title + stem) so each card looks its own
/// and stays the same between visits. Hover brightens it; `sweep` runs a playhead across
/// that lifts the bars it passes.
fn thumbnail(
    p: &egui::Painter,
    r: Rect,
    e: &crate::settings::HistoryEntry,
    sweep: Option<f32>,
    hover: f32,
) {
    use stem_models::StemKind;
    p.rect_filled(r, CornerRadius::same(10), Color32::from_rgb(11, 11, 19));
    let seed = e
        .title
        .bytes()
        .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let stems: Vec<StemKind> = e.stems.iter().copied().take(6).collect();
    let n = stems.len().max(1);
    let inner = r.shrink2(vec2(10.0, 8.0));
    let lane_h = inner.height() / n as f32;
    let bars = ((inner.width() / 5.0) as usize).max(12);
    let bw = inner.width() / bars as f32;
    let head = sweep.map(|f| inner.left() + inner.width() * motion::ease_in_out(f));
    for (li, k) in stems.iter().enumerate() {
        let color = theme::stem_color(*k);
        let cy = inner.top() + lane_h * (li as f32 + 0.5);
        let ks = seed.wrapping_add(li as u32 * 7919);
        let drums = matches!(k, StemKind::Drums) || k.is_drum_part();
        for b in 0..bars {
            // Smooth envelope (two octaves of value noise) with a stem-flavoured pulse.
            let f = b as f32 / 6.0;
            let i0 = f.floor() as u32;
            let fr = f - f.floor();
            let v = hash01(ks, i0) * (1.0 - fr) + hash01(ks, i0 + 1) * fr;
            let fine = hash01(ks ^ 0xA5A5, b as u32);
            // Song shape: a slow envelope (verse / drop) over the bar-to-bar movement.
            let sec = hash01(seed ^ 0x51ED, (b / 14) as u32);
            let mut a = (0.12 + 0.55 * v * v + 0.33 * fine) * (0.45 + 0.55 * sec);
            if drums {
                a = if b % 4 == 0 { 0.95 } else { 0.25 + 0.3 * fine };
            }
            let x = inner.left() + b as f32 * bw + bw / 2.0;
            let lift = head.map_or(0.0, |hx| (1.0 - ((x - hx).abs() / 24.0)).max(0.0));
            let passed = head.is_some_and(|hx| x <= hx);
            let amp = (a * (1.0 + 0.35 * lift)).min(1.15) * lane_h * 0.42;
            let alpha = if passed { 0.95 } else { 0.55 + 0.3 * hover };
            p.line_segment(
                [pos2(x, cy - amp), pos2(x, cy + amp)],
                egui::Stroke::new((bw - 1.5).max(1.0), theme::alpha(color, alpha)),
            );
        }
    }
    if let Some(hx) = head {
        p.vline(
            hx,
            r.top() + 4.0..=r.bottom() - 4.0,
            egui::Stroke::new(1.5, Color32::WHITE),
        );
    }
}
