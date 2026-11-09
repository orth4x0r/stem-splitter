//! The keyboard shortcut overlay: press `?` anywhere (outside a text field) for a sheet that
//! lists every shortcut, grouped by where it works. `?` again, Escape or a click outside
//! closes it.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, pos2, vec2,
};

use crate::app::StemApp;
use crate::theme;
use crate::widgets::buttons::Button;
use crate::widgets::icons::Icon;
use crate::widgets::sheet::Sheet;

fn open_id() -> Id {
    Id::unique("shortcuts-open")
}

/// (keys, what it does). Keys are separated by " " (pressed together use "+").
const GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Anywhere",
        &[
            ("Ctrl+1–4", "Split · Library · Models · Settings"),
            ("?", "Show or hide this list"),
            ("Esc", "Close a sheet or menu"),
            ("Tab", "Move focus between controls"),
        ],
    ),
    (
        "Stems and playback",
        &[
            ("Space", "Play / pause"),
            ("Drag", "Select a sample on a waveform"),
            ("Alt+Drag", "Select without snapping to the grid"),
            ("Click", "Seek"),
            ("Ctrl+Wheel", "Zoom the waveforms"),
            ("Wheel", "Scroll when zoomed in"),
        ],
    ),
    (
        "Chop pads",
        &[
            ("Z X C V", "Pads 1–4 (bottom row)"),
            ("A S D F", "Pads 5–8"),
            ("Q W E R", "Pads 9–12"),
            ("1 2 3 4", "Pads 13–16 (top row)"),
            ("Shift+Key", "Softer hit"),
            ("[ ]", "Previous / next pad page"),
            ("Esc", "Stop every pad"),
        ],
    ),
    (
        "Section picker",
        &[
            ("Enter", "Split the section"),
            ("Esc", "Back without splitting"),
            ("Space", "Play / pause"),
        ],
    ),
];

/// Toggle on `?` (typed, so any keyboard layout works) when no text field has focus.
fn toggle_requested(ctx: &egui::Context) -> bool {
    !ctx.egui_wants_keyboard_input()
        && ctx.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::Text(t) if t == "?"))
        })
}

pub fn show(_app: &mut StemApp, ctx: &egui::Context) {
    let open = ctx.data(|d| d.get_temp::<bool>(open_id())).unwrap_or(false);
    let toggle = toggle_requested(ctx);
    if !open {
        if toggle {
            ctx.data_mut(|d| d.insert_temp(open_id(), true));
            ctx.request_repaint();
        }
        return;
    }
    let mut anim = Sheet::begin(ctx, Id::unique("shortcuts-sheet"));
    let mut close = toggle;
    let resp = anim.show(ctx, theme::VIOLET, 24, |ui| {
        ui.set_width(760.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Keyboard shortcuts")
                        .font(theme::bold(22.0))
                        .color(theme::TEXT),
                );
                ui.label(
                    egui::RichText::new("Keys work when no text field has focus.")
                        .font(theme::regular(13.0))
                        .color(theme::TEXT_DIM),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if Button::ghost("")
                    .icon(Icon::Close)
                    .height(32.0)
                    .tooltip("Close (Esc)")
                    .show(ui)
                    .clicked()
                {
                    close = true;
                }
            });
        });
        ui.add_space(14.0);
        // Two columns of groups.
        let col_w = (ui.available_width() - 24.0) / 2.0;
        let (left, right): (Vec<_>, Vec<_>) =
            GROUPS.iter().enumerate().partition(|(i, _)| i % 2 == 0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 24.0;
            for col in [left, right] {
                ui.allocate_ui_with_layout(
                    vec2(col_w, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(col_w);
                        for (_, (title, rows)) in col {
                            group(ui, title, rows, col_w);
                            ui.add_space(14.0);
                        }
                    },
                );
            }
        });
    });
    if resp.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if close {
        anim.close(ctx);
    }
    if anim.finished() {
        ctx.data_mut(|d| d.remove::<bool>(open_id()));
    }
}

fn group(ui: &mut egui::Ui, title: &str, rows: &[(&str, &str)], w: f32) {
    ui.label(
        egui::RichText::new(title)
            .font(theme::semi(13.5))
            .color(theme::TEXT),
    );
    ui.add_space(4.0);
    ui.spacing_mut().item_spacing.y = 0.0;
    for (keys, what) in rows {
        let (r, _) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
        let p = ui.painter();
        p.hline(
            r.x_range(),
            r.bottom(),
            Stroke::new(1.0, Color32::from_white_alpha(8)),
        );
        // Keycaps on the left, description after a fixed column.
        let mut x = r.left();
        for (ki, chord) in keys.split(' ').enumerate() {
            if ki > 0 {
                x += 4.0;
            }
            for (pi, part) in chord.split('+').enumerate() {
                if pi > 0 {
                    p.text(
                        pos2(x + 2.0, r.center().y),
                        Align2::LEFT_CENTER,
                        "+",
                        theme::regular(11.0),
                        theme::TEXT_FAINT,
                    );
                    x += 12.0;
                }
                x = keycap(p, x, r.center().y, part) + 2.0;
            }
        }
        p.text(
            pos2(r.left() + 132.0, r.center().y),
            Align2::LEFT_CENTER,
            *what,
            theme::regular(12.5),
            theme::TEXT_DIM,
        );
    }
}

/// A keycap with `label`, left edge at `x`. Returns its right edge.
fn keycap(p: &egui::Painter, x: f32, cy: f32, label: &str) -> f32 {
    let g = p.layout_no_wrap(label.to_owned(), theme::mono(11.0), theme::TEXT);
    let w = (g.size().x + 12.0).max(22.0);
    let r = Rect::from_min_size(pos2(x, cy - 10.0), vec2(w, 20.0));
    // A slightly raised cap: darker base line under the face.
    p.rect_filled(
        r.translate(vec2(0.0, 1.5)),
        CornerRadius::same(5),
        Color32::from_black_alpha(140),
    );
    p.rect(
        r,
        CornerRadius::same(5),
        theme::CARD_HI,
        Stroke::new(1.0, theme::STROKE_HI),
        StrokeKind::Inside,
    );
    p.galley(
        pos2(r.center().x - g.size().x / 2.0, cy - g.size().y / 2.0),
        g,
        theme::TEXT,
    );
    r.right()
}
