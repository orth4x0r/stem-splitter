//! Screens. `draw` lays out the chrome (header, sidebar) and dispatches to the current page.

mod chrome;
mod export_dialog;
mod home;
mod library;
mod models;
mod models_dl;
mod progress;
mod queue;
mod results;
mod settings;
mod settings_tools;

use eframe::egui::{self, Margin};

use crate::app::{Page, StemApp};
use crate::theme;

pub fn draw(app: &mut StemApp, ui: &mut egui::Ui) {
    chrome::header(app, ui);
    chrome::sidebar(app, ui);

    let results = app.page == Page::Split && app.job.is_none() && app.session.is_some();
    app.toasts.bottom_inset = if results { 80.0 } else { 0.0 };
    if results {
        results::transport(app, ui);
    }

    egui::CentralPanel::no_frame()
        .frame(
            egui::Frame::new()
                .fill(theme::BG)
                .inner_margin(Margin::ZERO),
        )
        .show(ui, |ui| {
            chrome::backdrop(ui);
            let page = app.page;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .id_salt(("page", page as u8))
                .show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(32, 26))
                        .show(ui, |ui| match page {
                            Page::Split => {
                                if app.job.is_some() {
                                    progress::page(app, ui);
                                    if !app.queue.is_empty() {
                                        ui.add_space(18.0);
                                        let w = ui.available_width().min(1000.0);
                                        ui.vertical_centered(|ui| {
                                            ui.set_max_width(w);
                                            queue::panel(app, ui);
                                        });
                                    }
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

    export_dialog::show(app, ui.ctx());

    if app.drop_hover {
        chrome::drop_overlay(ui.ctx());
    }
}
