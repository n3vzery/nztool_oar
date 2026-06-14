#![windows_subsystem = "windows"]

pub mod features;
pub mod hooks;
pub mod state;
pub mod ui;
pub mod update;
pub mod utils;
pub mod worker;

pub use features::*;
pub use hooks::*;
pub use state::*;
pub use ui::*;
pub use worker::*;

use eframe::egui;
use std::sync::{Arc, Mutex};

fn load_icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("nz.ico")).unwrap();
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    egui::IconData {
        rgba: rgba.into_vec(),
        width,
        height,
    }
}

fn main() -> eframe::Result<()> {
    utils::SimpleLogger::init();
    update::check_for_updates();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 550.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_icon(load_icon()),
        ..Default::default()
    };

    let state = Arc::new(Mutex::new(AppState::new()));
    hooks::start_hotkey_listener(state.clone(), InputState);

    eframe::run_native(
        "nztool oar - v2.4.4",
        options,
        Box::new(|_cc| Ok(Box::new(ui::KeyBindApp::new(state)))),
    )
}
