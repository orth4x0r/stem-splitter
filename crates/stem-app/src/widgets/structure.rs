//! Song-structure lanes on a timeline: lettered section blocks (click to select) and a strip
//! of chord names.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2,
};
use stem_audio::analysis::BeatGrid;
use stem_audio::chords::ChordSpan;
use stem_audio::sections::Section;

use crate::backend::mmss;
use crate::theme;

/// Muted tints per letter: repeats read at a glance without competing with the stem colours.
const TINTS: [Color32; 6] = [
    Color32::from_rgb(150, 156, 170),
    Color32::from_rgb(112, 140, 178),
    Color32::from_rgb(170, 140, 112),
    Color32::from_rgb(116, 160, 138),
    Color32::from_rgb(166, 120, 146),
    Color32::from_rgb(150, 150, 108),
];

pub fn section_color(label: u8) -> Color32 {
    TINTS[label as usize % TINTS.len()]
}

/// A click on a section block.
pub struct SectionClick {
    pub start: f64,
    pub end: f64,
    /// Shift held: grow the current selection to include this section.
    pub extend: bool,
}

impl SectionClick {
    /// The new selection, given the current one.
    pub fn apply(&self, current: Option<(f64, f64)>) -> (f64, f64) {
        match current {
            Some((a, b)) if self.extend => (a.min(self.start), b.max(self.end)),
            _ => (self.start, self.end),
        }
    }
}

/// Labelled section blocks across `rect`; `to_x` maps seconds to x (may fall outside the
/// rect when zoomed). `chorus` gets an accent outline and a "chorus" tag where it fits.
#[allow(clippy::too_many_arguments)]
pub fn sections_lane(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    sections: &[Section],
    chorus: Option<u8>,
    grid: Option<&BeatGrid>,
    selection: Option<(f64, f64)>,
    to_x: &dyn Fn(f64) -> f32,
) -> Option<SectionClick> {
    let resp = ui.interact(rect, id, Sense::click());
    let hover_x = resp.hover_pos().map(|p| p.x);
    let blocks: Vec<(Rect, &Section)> = sections
        .iter()
        .filter_map(|s| {
            let (x0, x1) = (to_x(s.start), to_x(s.end));
            if x1 < rect.left() || x0 > rect.right() {
                return None;
            }
            let r = Rect::from_x_y_ranges(
                x0.max(rect.left()) + 1.0..=(x1.min(rect.right()) - 1.0).max(x0 + 1.0),
                rect.y_range(),
            );
            Some((r, s))
        })
        .collect();
    let hovered = hover_x.and_then(|x| blocks.iter().position(|(r, _)| r.x_range().contains(x)));
    let p = ui.painter_at(rect.expand(2.0));
    for (i, (r, s)) in blocks.iter().enumerate() {
        let c = section_color(s.label);
        let selected =
            selection.is_some_and(|(a, b)| (a - s.start).abs() < 0.02 && (b - s.end).abs() < 0.02);
        let hot = hovered == Some(i);
        let fill = theme::alpha(
            c,
            if selected {
                0.42
            } else if hot {
                0.30
            } else {
                0.18
            },
        );
        let rr = CornerRadius::same(5);
        p.rect_filled(*r, rr, fill);
        // Energy as a thin bar along the bottom.
        let e = Rect::from_min_max(
            pos2(r.left(), r.bottom() - 2.0),
            pos2(r.left() + r.width() * s.energy.clamp(0.0, 1.0), r.bottom()),
        );
        p.rect_filled(e, CornerRadius::same(1), theme::alpha(c, 0.7));
        let is_chorus = chorus == Some(s.label);
        if is_chorus || selected {
            p.rect_stroke(
                *r,
                rr,
                Stroke::new(
                    1.0,
                    if selected {
                        theme::alpha(theme::TEXT, 0.7)
                    } else {
                        theme::alpha(theme::ACCENT, 0.55)
                    },
                ),
                StrokeKind::Inside,
            );
        }
        if r.width() >= 14.0 {
            let fg = if selected || hot {
                theme::TEXT
            } else {
                theme::lerp_color(c, theme::TEXT, 0.45)
            };
            let letter = s.letter().to_string();
            let g = p.layout_no_wrap(letter, theme::bold(12.0), fg);
            let lw = g.size().x;
            p.galley(
                pos2(r.left() + 7.0, r.center().y - g.size().y / 2.0 - 0.5),
                g,
                fg,
            );
            if is_chorus && r.width() >= lw + 64.0 {
                p.text(
                    pos2(r.left() + 13.0 + lw, r.center().y - 0.5),
                    Align2::LEFT_CENTER,
                    "chorus",
                    theme::regular(10.5),
                    theme::alpha(theme::ACCENT, 0.9),
                );
            }
        }
    }
    let mut out = None;
    if let Some(i) = hovered {
        let (_, s) = blocks[i];
        let bars = grid
            .map(|g| g.bars_between(s.start, s.end))
            .map_or(String::new(), |b| format!(" · {:.0} bars", b.round()));
        let tag = if chorus == Some(s.label) {
            " (likely chorus)"
        } else {
            ""
        };
        let resp = resp
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(format!(
                "Section {}{tag} · {} – {}{bars}\nClick to select · Shift-click to extend",
                s.letter(),
                mmss(s.start),
                mmss(s.end),
            ));
        if resp.clicked() {
            out = Some(SectionClick {
                start: s.start,
                end: s.end,
                extend: ui.input(|i| i.modifiers.shift),
            });
        }
    }
    out
}

/// Chord names along `rect`: each change gets a tick and, where there's room, its name.
/// The chord under `playhead` is drawn brighter.
pub fn chords_lane(
    p: &egui::Painter,
    rect: Rect,
    chords: &[ChordSpan],
    playhead: f64,
    to_x: &dyn Fn(f64) -> f32,
) {
    let p = p.with_clip_rect(rect.expand2(egui::vec2(0.0, 2.0)));
    let font = theme::mono(11.0);
    for s in chords {
        let (x0, x1) = (to_x(s.start), to_x(s.end));
        if x1 < rect.left() || x0 > rect.right() {
            continue;
        }
        let Some(c) = s.chord else { continue };
        let now = playhead >= s.start && playhead < s.end;
        let lx = x0.max(rect.left());
        if x0 >= rect.left() {
            p.vline(
                x0,
                rect.top() + 3.0..=rect.bottom() - 3.0,
                Stroke::new(1.0, theme::alpha(theme::TEXT_FAINT, 0.45)),
            );
        }
        let name = c.name();
        let g = p.layout_no_wrap(
            name,
            font.clone(),
            if now { theme::TEXT } else { theme::TEXT_DIM },
        );
        if x1.min(rect.right()) - lx >= g.size().x + 8.0 {
            if now {
                let bg = Rect::from_min_max(
                    pos2(lx + 1.0, rect.top() + 1.0),
                    pos2(lx + g.size().x + 9.0, rect.bottom() - 1.0),
                );
                p.rect_filled(bg, CornerRadius::same(4), theme::CARD_HI);
            }
            p.galley(
                pos2(lx + 5.0, rect.center().y - g.size().y / 2.0),
                g,
                theme::TEXT_DIM,
            );
        }
    }
}
