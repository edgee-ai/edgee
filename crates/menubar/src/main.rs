//! Edgee macOS menubar app — a pure-Rust (egui + tray-icon) shell over the
//! `edgee-cli` library. It surfaces Edgee stats and launches agents/relay.

mod app;
mod tray;

use edgee_cli::config;
use eframe::egui;

fn main() -> eframe::Result<()> {
    // Resolve the active profile the same way the CLI does, so credential reads
    // target the profile the user last selected.
    let profile = config::read_file()
        .ok()
        .and_then(|f| f.active_profile)
        .unwrap_or_else(|| "default".to_string());
    config::set_active_profile(profile);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Edgee")
            .with_inner_size([340.0, 460.0])
            .with_min_inner_size([300.0, 340.0])
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        "Edgee",
        native_options,
        Box::new(|cc| match app::EdgeeApp::new(cc) {
            Ok(app) => Ok(Box::new(app) as Box<dyn eframe::App>),
            Err(e) => Err(e.to_string().into()),
        }),
    )
}
