//! Standalone GUI binary for AULA F75 keyboard configuration.

use aula_f75::gui::AulaF75App;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "AULA F75",
        native_options,
        Box::new(|cc| Ok(Box::new(AulaF75App::new(cc)))),
    )
}
