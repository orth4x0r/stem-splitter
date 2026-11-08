#![allow(unused_variables)] // remove once stubs are implemented
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt::init();
    eframe::run_native(
        "Stem Splitter",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::<App>::default())),
    )
}

#[derive(Default)]
struct App;

impl eframe::App for App {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading("Stem Splitter");
    }
}
