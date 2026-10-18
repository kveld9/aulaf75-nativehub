//! Side panel hardware status, device metadata, and keyboard reference.

use eframe::egui::{self, Color32, Margin, RichText, Stroke, Vec2};

use crate::gui::state::AulaF75App;
use crate::gui::theme::{
    card_frame, corner_radius, ConnectionBadgeState, COLOR_ACCENT, COLOR_BG_INPUT,
    COLOR_BORDER_SUBTLE, COLOR_STATUS_CONNECTING, COLOR_STATUS_ERROR, COLOR_TEXT_MUTED,
    COLOR_TEXT_PRIMARY, COLOR_TEXT_SECONDARY,
};

/// Render a pill status badge with a drawn circle dot indicator.
pub fn render_status_badge(ui: &mut egui::Ui, state: ConnectionBadgeState) {
    let text = state.label();
    let dot_color = state.dot_color();
    let bg_color = state.bg_color();

    egui::Frame::default()
        .fill(bg_color)
        .corner_radius(corner_radius(4))
        .stroke(Stroke::new(1.0_f32, COLOR_BORDER_SUBTLE))
        .inner_margin(Margin {
            left: 8,
            right: 8,
            top: 4,
            bottom: 4,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, dot_color);
                ui.label(
                    RichText::new(text)
                        .size(12.0)
                        .strong()
                        .color(COLOR_TEXT_PRIMARY),
                );
            });
        });
}

/// Render the side panel content: hardware status badge, device facts grid, and keyboard reference.
pub fn render_side_panel(app: &AulaF75App, ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.label(
        RichText::new("Hardware Status")
            .strong()
            .size(15.0)
            .color(COLOR_TEXT_PRIMARY),
    );
    ui.add_space(8.0);

    // Hardware Status Card
    card_frame().show(ui, |ui| {
        let badge_state = ConnectionBadgeState::determine(
            app.control_node_path.is_some(),
            app.is_streaming(),
            &app.status_text,
            app.last_error.is_some(),
        );
        render_status_badge(ui, badge_state);

        ui.add_space(10.0);

        // Device Facts Grid
        egui::Grid::new("device_facts_grid")
            .num_columns(2)
            .spacing([8.0, 5.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Device").size(11.0).color(COLOR_TEXT_MUTED));
                ui.label(
                    RichText::new("AULA F75")
                        .size(11.0)
                        .strong()
                        .color(COLOR_TEXT_PRIMARY),
                );
                ui.end_row();

                ui.label(RichText::new("VID:PID").size(11.0).color(COLOR_TEXT_MUTED));
                ui.label(
                    RichText::new("258a:010c")
                        .size(11.0)
                        .monospace()
                        .color(COLOR_TEXT_PRIMARY),
                );
                ui.end_row();

                ui.label(RichText::new("Interface").size(11.0).color(COLOR_TEXT_MUTED));
                ui.label(
                    RichText::new("1 (Feature Report)")
                        .size(11.0)
                        .color(COLOR_TEXT_PRIMARY),
                );
                ui.end_row();

                ui.label(RichText::new("Node").size(11.0).color(COLOR_TEXT_MUTED));
                if let Some(ref path) = app.control_node_path {
                    ui.label(
                        RichText::new(path.display().to_string())
                            .size(11.0)
                            .monospace()
                            .color(COLOR_ACCENT),
                    );
                } else if app.status_text == "Connecting..." {
                    ui.label(
                        RichText::new("Searching...")
                            .size(11.0)
                            .italics()
                            .color(COLOR_STATUS_CONNECTING),
                    );
                } else {
                    ui.label(
                        RichText::new("Not Found")
                            .size(11.0)
                            .color(COLOR_STATUS_ERROR),
                    );
                }
                ui.end_row();
            });

        // Diagnostic microcopy for missing node or error
        if let Some(ref err) = app.last_error {
            ui.add_space(8.0);
            egui::Frame::default()
                .fill(Color32::from_rgb(44, 18, 22))
                .corner_radius(corner_radius(4))
                .stroke(Stroke::new(1.0_f32, COLOR_STATUS_ERROR))
                .inner_margin(Margin::same(6))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Error")
                                .size(11.0)
                                .strong()
                                .color(COLOR_STATUS_ERROR),
                        );
                        ui.label(
                            RichText::new(err)
                                .size(10.0)
                                .color(COLOR_TEXT_PRIMARY),
                        );
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new("Action: Check USB connection or udev rules.")
                                .size(10.0)
                                .color(COLOR_TEXT_MUTED),
                        );
                    });
                });
        } else if app.control_node_path.is_none() && app.status_text == "Disconnected" {
            ui.add_space(8.0);
            egui::Frame::default()
                .fill(COLOR_BG_INPUT)
                .corner_radius(corner_radius(4))
                .stroke(Stroke::new(1.0_f32, COLOR_BORDER_SUBTLE))
                .inner_margin(Margin::same(6))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Cause: No hidraw node detected.")
                                .size(10.0)
                                .color(COLOR_TEXT_SECONDARY),
                        );
                        ui.label(
                            RichText::new("Action: Plug in USB cable and verify 99-aula-f75.rules.")
                                .size(10.0)
                                .color(COLOR_TEXT_MUTED),
                        );
                    });
                });
        }
    });

    ui.add_space(14.0);

    // Keyboard Reference Collapsible Section
    egui::CollapsingHeader::new(
        RichText::new("Keyboard Reference")
            .strong()
            .size(13.0)
            .color(COLOR_TEXT_PRIMARY),
    )
    .default_open(true)
    .show(ui, |ui| {
        card_frame().show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("Onboard Shortcuts")
                        .strong()
                        .size(11.0)
                        .color(COLOR_ACCENT),
                );
                ui.add_space(4.0);

                ui.label(
                    RichText::new("• Custom RGB Mode:")
                        .strong()
                        .size(10.0)
                        .color(COLOR_TEXT_PRIMARY),
                );
                ui.label(
                    RichText::new("  Required for host software lighting.")
                        .size(10.0)
                        .color(COLOR_TEXT_MUTED),
                );

                ui.add_space(3.0);
                ui.label(
                    RichText::new("• Fn + Backspace:")
                        .strong()
                        .size(10.0)
                        .color(COLOR_TEXT_PRIMARY),
                );
                ui.label(
                    RichText::new("  Toggle backlight on / off.")
                        .size(10.0)
                        .color(COLOR_TEXT_MUTED),
                );

                ui.add_space(3.0);
                ui.label(
                    RichText::new("• Knob Long-Press (~3-5s):")
                        .strong()
                        .size(10.0)
                        .color(COLOR_TEXT_PRIMARY),
                );
                ui.label(
                    RichText::new("  Switch Multimedia (vol) / Lighting.")
                        .size(10.0)
                        .color(COLOR_TEXT_MUTED),
                );

                ui.add_space(3.0);
                ui.label(
                    RichText::new("• Fn + B:")
                        .strong()
                        .size(10.0)
                        .color(COLOR_TEXT_PRIMARY),
                );
                ui.label(
                    RichText::new("  Battery level check (1-0 row indicator).")
                        .size(10.0)
                        .color(COLOR_TEXT_MUTED),
                );
            });
        });
    });

    ui.add_space(14.0);

    // System Requirements
    egui::CollapsingHeader::new(
        RichText::new("System Requirements")
            .size(12.0)
            .color(COLOR_TEXT_SECONDARY),
    )
    .default_open(false)
    .show(ui, |ui| {
        ui.label(
            RichText::new("• Udev rule: 99-aula-f75.rules in /etc/udev/rules.d/")
                .size(10.0)
                .color(COLOR_TEXT_MUTED),
        );
        ui.label(
            RichText::new("• Keyboard mode: Custom/Static onboard profile")
                .size(10.0)
                .color(COLOR_TEXT_MUTED),
        );
        ui.label(
            RichText::new("• Linux hidraw access without root")
                .size(10.0)
                .color(COLOR_TEXT_MUTED),
        );
    });
}
