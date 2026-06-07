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

fn main() -> eframe::Result<()> {
    utils::SimpleLogger::init();
    update::check_for_updates();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 550.0])
            .with_resizable(false)
            .with_maximize_button(false),
        ..Default::default()
    };

    let state = Arc::new(Mutex::new(AppState::new()));
    hooks::start_hotkey_listener(state.clone(), InputState);

    eframe::run_native(
        "nztool oar - v2.4.0",
        options,
        Box::new(|_cc| Ok(Box::new(ui::KeyBindApp::new(state)))),
    )
}
