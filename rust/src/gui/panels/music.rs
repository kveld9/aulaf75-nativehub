//! System audio-reactive music mode UI panel for AULA F75.

use eframe::egui::{self, Color32, RichText, Stroke, Vec2};

use crate::cli::music::MusicMode;
use crate::gui::state::AulaF75App;
use crate::gui::theme::{
    card_frame, COLOR_ACCENT, COLOR_BG_CARD, COLOR_BORDER_SUBTLE, COLOR_TEXT_PRIMARY,
    COLOR_TEXT_SECONDARY,
};

impl AulaF75App {
    /// Dispatch music reactive lighting to running worker or start a new worker.
    pub fn start_music(&mut self) {
        let rgb = (self.color_rgb[0], self.color_rgb[1], self.color_rgb[2]);
        let msg = self.state.transition_to_music(self.music_mode, rgb, self.music_gain);
        if self.is_worker_running() {
            if let Some(ref tx) = self.worker_tx {
                let _ = tx.send(msg);
                self.status_text = format!("Streaming music ({})", self.music_mode);
            }
        } else {
            self.start_worker(msg);
        }
    }
}

/// Render the audio reactive music lighting panel.
pub fn render_music_panel(app: &mut AulaF75App, ui: &mut egui::Ui) {
    ui.label(
        RichText::new("Audio Reactive Lighting (Music Mode)")
            .strong()
            .size(15.0)
            .color(COLOR_TEXT_PRIMARY),
    );
    ui.add_space(6.0);

    card_frame().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Reactive Mode:")
                    .size(12.0)
                    .color(COLOR_TEXT_SECONDARY),
            );
            let current_mode_str = match app.music_mode {
                MusicMode::Bars => "Bars (Spectrum across keys)",
                MusicMode::Pulse => "Pulse (Whole-board beat)",
            };
            egui::ComboBox::from_label("##music_mode_select")
                .selected_text(current_mode_str)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(
                            &mut app.music_mode,
                            MusicMode::Bars,
                            "Bars (Spectrum across keys)",
                        )
                        .clicked()
                        && app.is_music_running()
                    {
                        app.start_music();
                    }
                    if ui
                        .selectable_value(
                            &mut app.music_mode,
                            MusicMode::Pulse,
                            "Pulse (Whole-board beat)",
                        )
                        .clicked()
                        && app.is_music_running()
                    {
                        app.start_music();
                    }
                });
        });

        ui.add_space(6.0);

        // Base Color Picker
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Base Color:")
                    .size(12.0)
                    .color(COLOR_TEXT_SECONDARY),
            );
            let mut srgb = app.color_rgb;
            if ui.color_edit_button_srgb(&mut srgb).changed() {
                app.set_color_from_picker(srgb);
            }
        });

        ui.add_space(6.0);

        // Audio Gain Slider
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Audio Gain:")
                    .size(12.0)
                    .color(COLOR_TEXT_SECONDARY),
            );
            let gain_label = format!("{:.1}x", app.music_gain);
            if ui
                .add(egui::Slider::new(&mut app.music_gain, 0.1..=5.0).text(gain_label))
                .changed()
                && app.is_music_running()
            {
                app.start_music();
            }
        });

        ui.add_space(8.0);

        // Start / Stop Music button (plain text, no emoji icons)
        ui.horizontal(|ui| {
            if app.is_music_running() {
                let stop_music_btn = egui::Button::new(
                    RichText::new("Stop Music")
                        .size(12.0)
                        .strong()
                        .color(COLOR_TEXT_PRIMARY),
                )
                .fill(COLOR_BG_CARD)
                .stroke(Stroke::new(1.0_f32, COLOR_BORDER_SUBTLE))
                .min_size(Vec2::new(100.0, 28.0));

                if ui.add(stop_music_btn).clicked() {
                    app.on_off_clicked();
                }
            } else {
                let start_music_btn = egui::Button::new(
                    RichText::new("Start Music")
                        .size(12.0)
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(Color32::from_rgb(0, 140, 175))
                .stroke(Stroke::new(1.0_f32, COLOR_ACCENT))
                .min_size(Vec2::new(100.0, 28.0));

                if ui.add(start_music_btn).clicked() {
                    app.start_music();
                }
            }
        });

        ui.add_space(8.0);

        // Level meter (ProgressBar from last band energies via channel)
        let avg_level = if app.last_music_bands.is_empty() {
            0.0
        } else {
            let sum: f32 = app.last_music_bands.iter().sum();
            (sum / app.last_music_bands.len() as f32).clamp(0.0, 1.0)
        };

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Audio Level:")
                    .size(12.0)
                    .color(COLOR_TEXT_SECONDARY),
            );
            ui.add(
                egui::ProgressBar::new(avg_level)
                    .text(format!("{:.0}%", avg_level * 100.0))
                    .animate(app.is_music_running()),
            );
        });
    });
}
