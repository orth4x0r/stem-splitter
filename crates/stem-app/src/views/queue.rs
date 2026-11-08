//! Batch queue panel: one row per link / file with status, reorder, cancel / remove, and
//! "Open" once a track is done. Shown on the start screen and under the progress card.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2,
    vec2,
};

use crate::app::StemApp;
use crate::queue::ItemStatus;
use crate::theme;
use crate::widgets::buttons::{self, Button};
use crate::widgets::icons::{self, Icon};
use crate::widgets::toast::elide;
use crate::widgets::{overline, progress};

const ROW_H: f32 = 46.0;

enum Action {
    Up(u64),
    Down(u64),
    Remove(u64),
    Retry(u64),
    Open(crate::settings::HistoryEntry),
    ClearFinished,
}

/// Small chevron button (up or down) for reordering.
fn chevron(ui: &mut Ui, id: Id, up: bool, enabled: bool) -> bool {
    let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
    let resp = ui.interact(
        r,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let resp = if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(if up { "Move up" } else { "Move down" })
    } else {
        resp
    };
    let h = ui
        .ctx()
        .animate_bool_with_time(id.with("h"), resp.hovered() && enabled, 0.12);
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(8), theme::alpha(theme::CARD_HI, h));
    let c = r.center();
    let dy = if up { 2.0 } else { -2.0 };
    let color = if enabled {
        theme::lerp_color(theme::TEXT_DIM, Color32::WHITE, h)
    } else {
        theme::alpha(theme::TEXT_FAINT, 0.4)
    };
    p.add(egui::Shape::line(
        vec![
            pos2(c.x - 5.0, c.y + dy),
            pos2(c.x, c.y - dy * 1.5),
            pos2(c.x + 5.0, c.y + dy),
        ],
        Stroke::new(1.8, color),
    ));
    enabled && resp.clicked()
}

/// The queue card. Draws nothing when the queue is empty.
pub fn panel(app: &mut StemApp, ui: &mut Ui) {
    if app.queue.is_empty() {
        return;
    }
    let t = ui.input(|i| i.time);
    let (done, total) = app.queue.progress();
    let running_frac = app.job.as_ref().map(|j| j.shown);
    let mut action = None;

    ui.add_space(4.0);
    theme::card_frame()
        .inner_margin(egui::Margin::symmetric(18, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                overline(ui, "Queue");
                ui.label(
                    egui::RichText::new(format!("{done} of {total} done"))
                        .font(theme::mono(11.5))
                        .color(theme::TEXT_DIM),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if done > 0
                        && Button::ghost("Clear finished")
                            .height(26.0)
                            .show(ui)
                            .clicked()
                    {
                        action = Some(Action::ClearFinished);
                    }
                });
            });
            ui.add_space(6.0);
            let n = app.queue.items.len();
            for (idx, item) in app.queue.items.iter().enumerate() {
                let w = ui.available_width();
                let (row, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::hover());
                let hover = ui.ctx().animate_bool_with_time(
                    Id::unique(("q-row", item.id)),
                    resp.hovered(),
                    0.12,
                );
                let running = matches!(item.status, ItemStatus::Running { .. });
                let p = ui.painter().clone();
                p.rect(
                    row,
                    CornerRadius::same(12),
                    theme::lerp_color(
                        theme::alpha(theme::CARD_HI, if running { 0.9 } else { 0.45 }),
                        theme::CARD_HI,
                        hover,
                    ),
                    Stroke::new(
                        1.0,
                        if running {
                            theme::alpha(theme::VIOLET, 0.6)
                        } else {
                            theme::STROKE
                        },
                    ),
                    StrokeKind::Inside,
                );
                // Position number.
                p.text(
                    pos2(row.left() + 16.0, row.center().y),
                    Align2::LEFT_CENTER,
                    format!("{:02}", idx + 1),
                    theme::mono(11.5),
                    theme::TEXT_FAINT,
                );
                // Status dot / spinner.
                let dot = pos2(row.left() + 48.0, row.center().y);
                let (status, color) = match &item.status {
                    ItemStatus::Pending => ("Waiting".to_owned(), theme::TEXT_FAINT),
                    ItemStatus::Expanding => ("Reading playlist…".to_owned(), theme::CYAN),
                    ItemStatus::Running { .. } => (
                        app.job
                            .as_ref()
                            .map_or("Working".to_owned(), |j| j.stage.label().to_owned()),
                        theme::VIOLET,
                    ),
                    ItemStatus::Done(_) => ("Done".to_owned(), theme::GOOD),
                    ItemStatus::Failed(_) => ("Failed".to_owned(), theme::BAD),
                    ItemStatus::Cancelled => ("Cancelled".to_owned(), theme::WARN),
                };
                if running || matches!(item.status, ItemStatus::Expanding) {
                    progress::dots(&p, dot, 6.0, t, color);
                } else if matches!(item.status, ItemStatus::Done(_)) {
                    p.circle_filled(dot, 8.0, theme::alpha(theme::GOOD, 0.2));
                    icons::paint(
                        &p,
                        Rect::from_center_size(dot, vec2(10.0, 10.0)),
                        Icon::Check,
                        theme::GOOD,
                    );
                } else {
                    p.circle_filled(dot, 4.0, color);
                }
                // Title + source.
                let text_x = row.left() + 68.0;
                let actions_w = 190.0;
                let max_chars = (((row.width() - 68.0 - actions_w - 150.0) / 7.5) as usize).max(12);
                p.text(
                    pos2(text_x, row.top() + 16.0),
                    Align2::LEFT_CENTER,
                    elide(&item.title, max_chars),
                    theme::semi(13.5),
                    theme::TEXT,
                );
                let sub = match &item.status {
                    ItemStatus::Failed(msg) => elide(msg, max_chars + 10),
                    _ => elide(&item.source.display(), max_chars + 10),
                };
                p.text(
                    pos2(text_x, row.top() + 32.0),
                    Align2::LEFT_CENTER,
                    sub,
                    theme::mono(10.5),
                    if matches!(item.status, ItemStatus::Failed(_)) {
                        theme::alpha(theme::BAD, 0.8)
                    } else {
                        theme::TEXT_FAINT
                    },
                );
                // Status text (and a bar while running).
                let sx = row.right() - actions_w - 130.0;
                p.text(
                    pos2(sx, row.top() + 16.0),
                    Align2::LEFT_CENTER,
                    status,
                    theme::semi(12.0),
                    color,
                );
                if running {
                    let bar = Rect::from_min_size(pos2(sx, row.top() + 28.0), vec2(110.0, 5.0));
                    progress::bar(&p, bar, running_frac, t);
                }
                // Actions, right to left.
                let ar = Rect::from_min_max(
                    pos2(row.right() - actions_w, row.top()),
                    pos2(row.right() - 10.0, row.bottom()),
                );
                let mut au = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(ar)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                au.spacing_mut().item_spacing.x = 4.0;
                let tip = if running {
                    "Cancel this track"
                } else {
                    "Remove from the queue"
                };
                if buttons::icon_button(&mut au, Icon::Close, 26.0, false, theme::BAD, tip)
                    .clicked()
                {
                    action = Some(Action::Remove(item.id));
                }
                match &item.status {
                    ItemStatus::Done(entry) => {
                        if Button::new("Open")
                            .icon(Icon::Play)
                            .height(28.0)
                            .enabled(app.job.is_none())
                            .show(&mut au)
                            .clicked()
                        {
                            action = Some(Action::Open((**entry).clone()));
                        }
                    }
                    ItemStatus::Failed(_) | ItemStatus::Cancelled => {
                        if Button::new("Retry")
                            .icon(Icon::Refresh)
                            .height(28.0)
                            .show(&mut au)
                            .clicked()
                        {
                            action = Some(Action::Retry(item.id));
                        }
                    }
                    ItemStatus::Pending => {
                        if chevron(&mut au, Id::unique(("q-dn", item.id)), false, idx + 1 < n) {
                            action = Some(Action::Down(item.id));
                        }
                        if chevron(&mut au, Id::unique(("q-up", item.id)), true, idx > 0) {
                            action = Some(Action::Up(item.id));
                        }
                    }
                    _ => {}
                }
                ui.add_space(4.0);
            }
        });
    ui.add_space(8.0);

    let now = ui.input(|i| i.time);
    match action {
        Some(Action::Up(id)) => app.queue.shift(id, -1),
        Some(Action::Down(id)) => app.queue.shift(id, 1),
        Some(Action::Remove(id)) => app.queue_remove(id),
        Some(Action::Retry(id)) => app.queue_retry(id),
        Some(Action::Open(e)) => app.reopen(&e, now),
        Some(Action::ClearFinished) => app.queue.clear_finished(),
        None => {}
    }
}
