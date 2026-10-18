//! Standalone GUI binary for AULA F75 keyboard configuration.

use aula_f75::gui::{app_visuals, configure_app_style, AulaF75App};
use eframe::egui;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("aula-f75")
            .with_title("AULA F75")
            .with_inner_size([780.0, 640.0])
            .with_min_inner_size([720.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "aula-f75",
        native_options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(app_visuals());
            cc.egui_ctx.style_mut(configure_app_style);
            Ok(Box::new(AulaF75App::new(cc)))
        }),
    )
}
