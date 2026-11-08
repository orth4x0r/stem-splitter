//! A waveform lane: envelope drawing, playhead, click-to-seek and drag-to-select.

use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, Id, Mesh, Rect, Sense, Shape, Stroke, Ui, pos2, vec2,
};

use stem_audio::analysis::BeatGrid;

use super::paint;
use crate::analysis::SnapMode;
use crate::theme;
use crate::timeline::{Timeline, WaveSource};

pub enum LaneAction {
    Seek(f64),
    /// Selection in seconds, start < end.
    Select(f64, f64),
}

pub struct Lane<'a> {
    pub id: Id,
    pub wave: WaveSource<'a>,
    /// Visible time window (zoom / scroll).
    pub view: &'a Timeline,
    /// Beat grid for grid lines and snapping.
    pub grid: Option<&'a BeatGrid>,
    /// Selection-edge snapping; hold Alt to bypass.
    pub snap: SnapMode,
    pub color: Color32,
    pub duration: f64,
    pub playhead: f64,
    pub selection: Option<(f64, f64)>,
    /// False when muted / not soloed: drawn desaturated.
    pub audible: bool,
    pub height: f32,
    /// Current output level, for a little glow on the playhead.
    pub level: f32,
    /// Draw the selection length on this lane (the one it was made on).
    pub sel_label: bool,
}

impl Lane<'_> {
    pub fn show(self, ui: &mut Ui) -> Option<LaneAction> {
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, self.height), Sense::click_and_drag());
        let resp = resp.on_hover_cursor(CursorIcon::Text);
        let dur = self.duration.max(1e-6);
        let view = self.view;
        let to_t = |x: f32| {
            view.time_at(((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64)
                .clamp(0.0, dur)
        };
        let to_x = |t: f64| rect.left() + view.frac_of(t).clamp(0.0, 1.0) as f32 * rect.width();
        let free = ui.input(|i| i.modifiers.alt);
        let snap = |t: f64| {
            if free {
                t
            } else {
                crate::analysis::snap(self.grid, self.snap, t, dur)
            }
        };

        // ---- interaction
        let mut action = None;
        let anchor_id = self.id.with("anchor");
        if resp.drag_started()
            && let Some(pos) = ui
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos())
        {
            ui.data_mut(|d| d.insert_temp(anchor_id, to_t(pos.x)));
            ui.data_mut(|d| d.insert_temp(anchor_id.with("raw"), pos.x));
        }
        if resp.dragged()
            && let (Some(pos), Some(a)) = (
                resp.interact_pointer_pos(),
                ui.data(|d| d.get_temp::<f64>(anchor_id)),
            )
        {
            let moved = ui
                .data(|d| d.get_temp::<f32>(anchor_id.with("raw")))
                .is_some_and(|x0| (pos.x - x0).abs() > 3.0);
            let (a, b) = (snap(a), snap(to_t(pos.x)));
            if moved && (b - a).abs() > 1e-4 {
                action = Some(LaneAction::Select(a.min(b), a.max(b)));
            }
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            action = Some(LaneAction::Seek(to_t(pos.x)));
        }

        if !ui.is_rect_visible(rect) {
            return action;
        }

        // ---- painting
        let p = ui.painter_at(rect.expand(6.0));
        let hover = ui
            .ctx()
            .animate_bool_with_time(self.id.with("hover"), resp.hovered(), 0.15);
        let audible = ui
            .ctx()
            .animate_bool_with_time(self.id.with("aud"), self.audible, 0.2);
        let base = theme::lerp_color(Color32::from_rgb(90, 90, 110), self.color, audible);
        p.rect_filled(
            rect,
            CornerRadius::same(10),
            Color32::from_rgba_unmultiplied(0, 0, 0, 90),
        );
        p.rect_filled(
            rect,
            CornerRadius::same(10),
            theme::alpha(self.color, 0.025 + 0.03 * hover),
        );

        // Beat / bar grid, or a line every 5 s without one.
        if let Some(grid) = self.grid {
            crate::timeline::paint_grid(&p, rect, view, grid, self.color);
        } else {
            let mut g = 5.0 * (view.start / 5.0).ceil();
            while g < view.end {
                p.vline(
                    to_x(g),
                    rect.y_range(),
                    Stroke::new(1.0, Color32::from_white_alpha(7)),
                );
                g += 5.0;
            }
        }
        let mid = rect.center().y;
        p.hline(
            rect.x_range(),
            mid,
            Stroke::new(1.0, Color32::from_white_alpha(10)),
        );

        let px = rect.width().max(1.0);
        let cols = (px / 2.0).floor().max(1.0) as usize; // one bar every 2 px
        let scale = 0.92 / self.wave.env.peak().max(0.08);
        let half = rect.height() * 0.5 - 4.0;
        let play_x = to_x(self.playhead);
        let mut mesh = Mesh::default();
        for i in 0..cols {
            let f0 = i as f32 / cols as f32;
            let f1 = (i + 1) as f32 / cols as f32;
            let (lo, hi, rms) =
                self.wave
                    .range(view.time_at(f0 as f64), view.time_at(f1 as f64), dur);
            let x = rect.left() + f0 * px;
            let played = x < play_x;
            let top = mid - (hi * scale).min(1.0) * half;
            let bot = mid - (lo * scale).max(-1.0) * half;
            let peak_c = if played {
                theme::alpha(base, 0.55)
            } else {
                theme::alpha(base, 0.28)
            };
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(x, top), pos2(x + 1.4, bot.max(top + 1.0))),
                peak_c,
            );
            let rh = (rms * scale * 1.2).min(1.0) * half;
            let rms_c = if played {
                theme::lerp_color(base, Color32::WHITE, 0.15)
            } else {
                theme::alpha(base, 0.6)
            };
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(x, mid - rh), pos2(x + 1.4, mid + rh.max(0.5))),
                rms_c,
            );
        }
        p.add(Shape::mesh(mesh));

        // Selection.
        if let Some((a, b)) = self.selection
            && b >= view.start
            && a <= view.end
        {
            let sel = Rect::from_x_y_ranges(to_x(a)..=to_x(b), rect.y_range());
            p.rect_filled(sel, CornerRadius::same(4), Color32::from_white_alpha(18));
            for x in [sel.left(), sel.right()] {
                p.vline(
                    x,
                    rect.y_range(),
                    Stroke::new(1.5, theme::alpha(self.color, 0.9)),
                );
                p.rect_filled(
                    Rect::from_center_size(pos2(x, rect.top() + 7.0), vec2(6.0, 12.0)),
                    CornerRadius::same(2),
                    self.color,
                );
            }
            if self.sel_label {
                let txt = format!("{:.2} s", b - a);
                let g = p.layout_no_wrap(txt, theme::mono(11.0), Color32::from_rgb(14, 10, 22));
                let pill = Rect::from_center_size(
                    pos2(sel.center().x, rect.bottom() - 13.0),
                    vec2(g.size().x + 14.0, 18.0),
                );
                p.rect_filled(pill, CornerRadius::same(9), self.color);
                p.galley(pill.center() - g.size() / 2.0, g, Color32::BLACK);
            }
        }

        // Playhead.
        if self.playhead > 0.0 && self.playhead >= view.start && self.playhead <= view.end {
            let glow_a = 0.35 + 0.5 * self.level.min(1.0);
            paint::glow(
                &p,
                Rect::from_x_y_ranges(play_x - 1.0..=play_x + 1.0, rect.y_range()),
                CornerRadius::ZERO,
                theme::alpha(self.color, glow_a),
                8.0,
            );
            p.vline(play_x, rect.y_range(), Stroke::new(1.5, Color32::WHITE));
        }

        // Hover time cursor.
        if let Some(h) = resp.hover_pos() {
            p.vline(
                h.x,
                rect.y_range(),
                Stroke::new(1.0, Color32::from_white_alpha(40)),
            );
        }
        p.rect_stroke(
            rect,
            CornerRadius::same(10),
            Stroke::new(1.0, theme::alpha(self.color, 0.10 + 0.25 * hover)),
            egui::StrokeKind::Inside,
        );
        action
    }
}
