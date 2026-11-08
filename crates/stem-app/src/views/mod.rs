//! Screens. `draw` lays out the chrome (header, sidebar) and dispatches to the current page.

mod chrome;
mod home;
mod library;
mod models;
mod progress;
mod results;
mod settings;

use eframe::egui::{self, Color32, Id, Margin, Rect, pos2, vec2};

use crate::app::{Page, StemApp};
use crate::theme::{self, motion};
use crate::widgets::paint;

/// Height of the results transport bar (see `results::transport`).
const TRANSPORT_H: f32 = 78.0;

pub fn draw(app: &mut StemApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();

    // Ambient room behind everything (panels are glass, so it shows through).
    let (colors, energy) = ambient_mood(app);
    crate::effects::ambient(ui.painter(), screen, &app.fx, &colors, energy);

    chrome::header(app, ui);
    chrome::sidebar(app, ui);

    let results = app.page == Page::Split && app.job.is_none() && app.session.is_some();
    app.toasts.bottom_inset = if results { TRANSPORT_H + 2.0 } else { 0.0 };
    if results {
        results::transport(app, ui);
        chrome::transport_edge(app, ui, screen, TRANSPORT_H);
    }

    let page = app.page;
    // Page entrance: fade + rise, keyed on what the page shows.
    let key = (page as u8, app.job.is_some(), app.session.is_some());
    let enter = motion::ease_out(motion::appear(
        &ctx,
        Id::unique(("page-enter", key)),
        0.0,
        motion::ENTER,
    ));
    let central = egui::CentralPanel::no_frame()
        .frame(egui::Frame::new().inner_margin(Margin::ZERO))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .id_salt(("page", page as u8))
                .show(ui, |ui| {
                    if enter < 1.0 {
                        ui.set_opacity(0.15 + 0.85 * enter);
                    }
                    egui::Frame::new()
                        .inner_margin(Margin {
                            left: 32,
                            right: 32,
                            top: 26 + (16.0 * (1.0 - enter)) as i8,
                            bottom: 26,
                        })
                        .show(ui, |ui| match page {
                            Page::Split => {
                                if app.job.is_some() {
                                    progress::page(app, ui);
                                } else if app.session.is_some() {
                                    results::page(app, ui);
                                } else {
                                    home::page(app, ui);
                                }
                            }
                            Page::Library => library::page(app, ui),
                            Page::Models => models::page(app, ui),
                            Page::Settings => settings::page(app, ui),
                        });
                });
        });
    let area = central.response.rect;
    scroll_fades(ui.painter(), area);

    if app.drop_hover {
        chrome::drop_overlay(&ctx);
    }
    crate::effects::show_burst(&ctx, &mut app.fx, area);
}

/// Colours and energy for the ambient background: the accent trio at rest, the job's or
/// session's stem colours when there is music on screen.
fn ambient_mood(app: &StemApp) -> (Vec<Color32>, f32) {
    if let Some(j) = &app.job {
        let mut c: Vec<Color32> = j.stems.iter().map(|k| theme::stem_color(*k)).collect();
        c.push(theme::VIOLET);
        return (c, 0.6);
    }
    if let Some(s) = &app.session
        && app.page == Page::Split
    {
        let c = s.tracks.iter().map(|t| theme::stem_color(t.kind)).collect();
        return (c, 0.25);
    }
    (vec![theme::VIOLET, theme::PINK, theme::CYAN, theme::VIOLET], 0.0)
}

/// Soft fades at the top and bottom of the scrolling page, so content dissolves into the
/// chrome instead of being cut.
fn scroll_fades(p: &egui::Painter, area: Rect) {
    let h = 18.0;
    let top = Rect::from_min_size(area.min, vec2(area.width(), h));
    paint::vgradient(
        p,
        top,
        Color32::from_rgba_premultiplied(8, 8, 14, 150),
        Color32::TRANSPARENT,
    );
    let bottom = Rect::from_min_size(pos2(area.left(), area.bottom() - h), vec2(area.width(), h));
    paint::vgradient(
        p,
        bottom,
        Color32::TRANSPARENT,
        Color32::from_rgba_premultiplied(8, 8, 14, 150),
    );
}
