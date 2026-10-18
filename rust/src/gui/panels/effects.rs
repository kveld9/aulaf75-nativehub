//! Lighting effect controls, color picker, sliders, and live preview swatch panel.

use eframe::egui::{self, Color32, Margin, RichText, Stroke, Vec2};

use crate::cli::parse_color_hex;
use crate::effects::{Effect, RGB};
use crate::gui::state::AulaF75App;
use crate::gui::theme::{
    card_frame, compute_preview_rgb, corner_radius, COLOR_ACCENT, COLOR_ACCENT_BG,
    COLOR_BG_CARD, COLOR_BG_INPUT, COLOR_BORDER_SUBTLE, COLOR_STATUS_ERROR, COLOR_TEXT_MUTED,
    COLOR_TEXT_PRIMARY, COLOR_TEXT_SECONDARY,
};
use crate::gui::worker::{WorkerMessage, AVAILABLE_EFFECTS};

impl AulaF75App {
    /// Build current `Effect` based on GUI state.
    pub fn current_effect(&self) -> Effect {
        let base_color = RGB(self.color_rgb[0], self.color_rgb[1], self.color_rgb[2]);
        Effect::from_name_with_speed(&self.selected_effect, Some(base_color), self.effect_speed)
            .unwrap_or(Effect::Off)
    }

    /// Dispatch current effect and brightness to running worker or start a new worker.
    pub fn dispatch_current_effect(&mut self) {
        let effect = self.current_effect();
        if effect == Effect::Off {
            self.on_off_clicked();
            return;
        }

        let msg = self.state.transition_to_effect(effect.clone(), self.brightness);
        if self.is_worker_running() {
            if let Some(ref tx) = self.worker_tx {
                let _ = tx.send(msg);
                self.status_text = format!("Streaming {}", self.selected_effect);
            }
        } else {
            self.start_worker(msg);
        }
    }

    /// Sync RGB values from color picker or sliders to hex input and active stream.
    pub fn set_color_from_picker(&mut self, srgb: [u8; 3]) {
        self.color_rgb = srgb;
        self.hex_input = format!("{:02X}{:02X}{:02X}", srgb[0], srgb[1], srgb[2]);
        self.hex_error = None;
        if self.is_streaming() {
            if self.is_music_running() {
                self.start_music();
            } else if self.selected_effect == "Solid" || self.selected_effect == "Breathe" {
                self.dispatch_current_effect();
            }
        }
    }

    /// Dispatch color change to running worker.
    pub fn send_color(&mut self, rgb: (u8, u8, u8)) {
        self.color_rgb = [rgb.0, rgb.1, rgb.2];
        self.hex_input = format!("{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2);
        let msg = self.state.transition_to_solid(rgb);
        if let Some(ref tx) = self.worker_tx {
            let _ = tx.send(msg);
        }
    }

    /// Handler for "Solid" action button click.
    pub fn on_solid_clicked(&mut self) {
        let rgb = match parse_color_hex(&self.hex_input) {
            Ok(c) => c,
            Err(e) => {
                self.hex_error = Some(e);
                return;
            }
        };
        self.hex_error = None;
        self.color_rgb = [rgb.0, rgb.1, rgb.2];
        self.selected_effect = "Solid".to_string();

        if self.is_worker_running() {
            if let Some(ref tx) = self.worker_tx {
                let msg = self.state.transition_to_solid(rgb);
                let _ = tx.send(msg);
                self.status_text = format!("Streaming solid #{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2);
            }
        } else {
            self.start_worker(WorkerMessage::SetColor(rgb));
        }
    }

    /// Handler for "Off" action button click.
    pub fn on_off_clicked(&mut self) {
        self.selected_effect = "Off".to_string();
        let msg = self.state.transition_to_stop();
        if let Some(ref tx) = self.worker_tx {
            let _ = tx.send(msg);
        } else {
            self.start_worker(WorkerMessage::Stop);
        }
        if self.control_node_path.is_some() {
            self.status_text = "Connected (idle)".to_string();
        }
    }
}

fn effect_button_style(is_selected: bool) -> (Color32, Stroke, Color32) {
    if is_selected {
        (
            COLOR_ACCENT_BG,
            Stroke::new(1.5_f32, COLOR_ACCENT),
            COLOR_TEXT_PRIMARY,
        )
    } else {
        (
            COLOR_BG_CARD,
            Stroke::new(1.0_f32, COLOR_BORDER_SUBTLE),
            COLOR_TEXT_SECONDARY,
        )
    }
}

fn render_effect_selector(app: &mut AulaF75App, ui: &mut egui::Ui) {
    ui.label(
        RichText::new("Lighting Effect")
            .size(12.0)
            .color(COLOR_TEXT_SECONDARY),
    );
    ui.add_space(4.0);

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for &eff in AVAILABLE_EFFECTS {
            let is_selected = app.selected_effect.eq_ignore_ascii_case(eff);
            let (bg, stroke, text_color) = effect_button_style(is_selected);

            let btn = egui::Button::new(
                RichText::new(eff).size(12.0).strong().color(text_color),
            )
            .fill(bg)
            .stroke(stroke)
            .min_size(Vec2::new(62.0, 28.0));

            if ui.add(btn).clicked() {
                app.selected_effect = eff.to_string();
                if app.is_streaming() {
                    app.dispatch_current_effect();
                }
            }
        }
    });
}

fn render_rgb_sliders(app: &mut AulaF75App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let mut r = app.color_rgb[0];
        let mut g = app.color_rgb[1];
        let mut b = app.color_rgb[2];
        let r_ch = ui.add(egui::Slider::new(&mut r, 0..=255).text("R")).changed();
        let g_ch = ui.add(egui::Slider::new(&mut g, 0..=255).text("G")).changed();
        let b_ch = ui.add(egui::Slider::new(&mut b, 0..=255).text("B")).changed();

        if r_ch || g_ch || b_ch {
            app.set_color_from_picker([r, g, b]);
        }
    });
}

fn handle_hex_input_change(app: &mut AulaF75App) {
    if let Ok(parsed) = parse_color_hex(&app.hex_input) {
        app.color_rgb = [parsed.0, parsed.1, parsed.2];
        app.hex_error = None;
        if app.is_streaming() {
            app.dispatch_current_effect();
        }
    } else if !app.hex_input.is_empty() {
        app.hex_error = Some(
            "Invalid hex format (expected 6 hex digits, e.g. FF0000 or #00FFFF)".to_string(),
        );
    }
}

fn render_hex_input(app: &mut AulaF75App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Hex:")
                .size(12.0)
                .color(COLOR_TEXT_SECONDARY),
        );
        let hex_resp = ui.text_edit_singleline(&mut app.hex_input);
        if hex_resp.lost_focus() || hex_resp.changed() {
            handle_hex_input_change(app);
        }
    });

    if let Some(ref err) = app.hex_error {
        ui.colored_label(COLOR_STATUS_ERROR, err);
    }
}

fn render_color_controls(app: &mut AulaF75App, ui: &mut egui::Ui) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Color:")
                    .size(12.0)
                    .color(COLOR_TEXT_SECONDARY),
            );
            let mut srgb = app.color_rgb;
            if ui.color_edit_button_srgb(&mut srgb).changed() {
                app.set_color_from_picker(srgb);
            }
        });

        ui.add_space(4.0);
        render_rgb_sliders(app, ui);
        ui.add_space(4.0);
        render_hex_input(app, ui);
    });
}

fn render_color_swatch(app: &AulaF75App, ui: &mut egui::Ui) {
    let preview_rgb = compute_preview_rgb(app.color_rgb, app.brightness);
    let preview_color = Color32::from_rgb(preview_rgb.0, preview_rgb.1, preview_rgb.2);
    let base_hex = format!(
        "#{:02X}{:02X}{:02X}",
        app.color_rgb[0], app.color_rgb[1], app.color_rgb[2]
    );
    let output_hex = format!(
        "#{:02X}{:02X}{:02X}",
        preview_rgb.0, preview_rgb.1, preview_rgb.2
    );

    egui::Frame::default()
        .fill(COLOR_BG_INPUT)
        .corner_radius(corner_radius(6))
        .stroke(Stroke::new(1.0_f32, COLOR_BORDER_SUBTLE))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("Output Swatch")
                        .size(11.0)
                        .color(COLOR_TEXT_MUTED),
                );
                ui.add_space(4.0);
                egui::Frame::default()
                    .fill(preview_color)
                    .corner_radius(corner_radius(4))
                    .stroke(Stroke::new(1.0_f32, COLOR_BORDER_SUBTLE))
                    .show(ui, |ui| {
                        ui.allocate_exact_size(
                            Vec2::new(72.0, 40.0),
                            egui::Sense::hover(),
                        );
                    });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(output_hex)
                        .size(11.0)
                        .monospace()
                        .strong()
                        .color(COLOR_TEXT_PRIMARY),
                );
                ui.label(
                    RichText::new(format!("Base: {}", base_hex))
                        .size(10.0)
                        .monospace()
                        .color(COLOR_TEXT_MUTED),
                );
            });
        });
}

fn is_color_effect(effect: &str) -> bool {
    effect == "Solid" || effect == "Breathe"
}

fn render_color_section(app: &mut AulaF75App, ui: &mut egui::Ui) {
    if !is_color_effect(&app.selected_effect) {
        return;
    }

    ui.horizontal(|ui| {
        render_color_controls(app, ui);
        ui.add_space(16.0);
        render_color_swatch(app, ui);
    });

    ui.add_space(6.0);
}

fn render_brightness_slider(app: &mut AulaF75App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Brightness:")
                .size(12.0)
                .color(COLOR_TEXT_SECONDARY),
        );
        let pct_label = format!("{:.0}%", app.brightness * 100.0);
        let changed = ui
            .add(egui::Slider::new(&mut app.brightness, 0.0..=1.0).text(pct_label))
            .changed();
        if changed && app.is_streaming() {
            app.dispatch_current_effect();
        }
    });
}

fn is_speed_effect(name: &str) -> bool {
    name.eq_ignore_ascii_case("rainbow")
        || name.eq_ignore_ascii_case("wave")
        || name.eq_ignore_ascii_case("breathe")
}

fn render_speed_slider(app: &mut AulaF75App, ui: &mut egui::Ui) {
    if !is_speed_effect(&app.selected_effect) {
        return;
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Speed:")
                .size(12.0)
                .color(COLOR_TEXT_SECONDARY),
        );
        let speed_label = format!("{:.1}x", app.effect_speed);
        let changed = ui
            .add(egui::Slider::new(&mut app.effect_speed, 0.25..=3.0).text(speed_label))
            .changed();
        if changed && app.is_streaming() {
            app.dispatch_current_effect();
        }
    });
}

fn render_effect_actions(app: &mut AulaF75App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let start_btn = egui::Button::new(
            RichText::new("Start Streaming")
                .size(13.0)
                .strong()
                .color(Color32::WHITE),
        )
        .fill(Color32::from_rgb(0, 140, 175))
        .stroke(Stroke::new(1.0_f32, COLOR_ACCENT))
        .min_size(Vec2::new(120.0, 32.0));

        if ui.add(start_btn).clicked() {
            app.dispatch_current_effect();
        }

        let stop_btn = egui::Button::new(
            RichText::new("Stop / Off")
                .size(13.0)
                .color(COLOR_TEXT_PRIMARY),
        )
        .fill(COLOR_BG_CARD)
        .stroke(Stroke::new(1.0_f32, COLOR_BORDER_SUBTLE))
        .min_size(Vec2::new(100.0, 32.0));

        if ui.add(stop_btn).clicked() {
            app.on_off_clicked();
        }
    });
}

/// Render the lighting effects panel.
pub fn render_effects_panel(app: &mut AulaF75App, ui: &mut egui::Ui) {
    card_frame().show(ui, |ui| {
        render_effect_selector(app, ui);
        ui.add_space(8.0);
        render_color_section(app, ui);
        render_brightness_slider(app, ui);
        render_speed_slider(app, ui);
        ui.add_space(10.0);
        render_effect_actions(app, ui);
    });
}
