//! Standalone GUI binary for AULA F75 keyboard configuration.

use aula_f75::gui::AulaF75App;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("aula-f75")
            .with_title("AULA F75"),
        ..Default::default()
    };
    eframe::run_native(
        "aula-f75",
        native_options,
        Box::new(|cc| Ok(Box::new(AulaF75App::new(cc)))),
    )
}
