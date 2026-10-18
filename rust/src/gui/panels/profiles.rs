//! Lighting profiles management UI panel for AULA F75.

use eframe::egui::{self, RichText};

use crate::cli::profile::{self, Profile};
use crate::gui::state::AulaF75App;
use crate::gui::theme::{
    card_frame, COLOR_STATUS_ERROR, COLOR_STATUS_STREAMING, COLOR_TEXT_PRIMARY,
    COLOR_TEXT_SECONDARY,
};
use crate::gui::worker::WorkerMessage;

impl AulaF75App {
    /// Refresh the list of saved profiles from the config directory.
    pub fn refresh_profiles(&mut self) {
        match profile::list_profiles(None) {
            Ok(list) => {
                self.saved_profiles = list;
                let valid_selection = self
                    .selected_profile
                    .as_ref()
                    .filter(|sel| self.saved_profiles.contains(sel))
                    .cloned();
                self.selected_profile = valid_selection.or_else(|| {
                    AulaF75App::resolve_startup_profile(&self.saved_profiles, None)
                });
            }
            Err(e) => {
                self.profile_status = Some(format!("Error listing profiles: {}", e));
            }
        }
    }

    fn send_or_start_worker(&mut self, msg: WorkerMessage) {
        self.note_worker_message(&msg);
        if self.is_worker_running() {
            if let Some(ref tx) = self.worker_tx {
                let _ = tx.send(msg);
            }
        } else {
            self.start_worker(msg);
        }
    }

    fn set_profile_status_msg(&mut self, msg: String) {
        self.status_text = msg.clone();
        self.profile_status = Some(msg);
    }

    fn update_profile_color(&mut self, c: (u8, u8, u8)) {
        self.color_rgb = [c.0, c.1, c.2];
        self.hex_input = format!("{:02X}{:02X}{:02X}", c.0, c.1, c.2);
        self.hex_error = None;
    }

    fn apply_key_profile(&mut self, prof: &Profile) {
        let msg = self.state.transition_to_keyframe(prof.keys.clone());
        self.send_or_start_worker(msg);
        let msg_text = format!("Profile '{}' applied ({} keys)", prof.name, prof.keys.len());
        self.set_profile_status_msg(msg_text);
    }

    fn apply_effect_profile(&mut self, prof: &Profile) {
        self.selected_effect = resolve_profile_effect_name(&prof.effect).to_string();
        self.brightness = prof.brightness;
        self.effect_speed = prof.speed;
        if let Some(c) = prof.color {
            self.update_profile_color(c);
        }
        self.dispatch_current_effect();
        let msg_text = format!("Profile '{}' applied", prof.name);
        self.set_profile_status_msg(msg_text);
    }

    /// Apply a loaded profile configuration to GUI state and worker thread.
    pub fn apply_profile(&mut self, prof: &Profile) {
        if !prof.keys.is_empty() {
            self.apply_key_profile(prof);
        } else {
            self.apply_effect_profile(prof);
        }
    }

    /// Apply the currently selected profile from the UI combo box.
    pub fn on_apply_profile_clicked(&mut self) {
        let sel = match self.selected_profile.clone() {
            Some(s) => s,
            None => {
                self.profile_status = Some("Error: No profile selected to apply".to_string());
                return;
            }
        };

        match profile::load_profile(&sel, None) {
            Ok(prof) => {
                self.apply_profile(&prof);
            }
            Err(e) => {
                self.profile_status = Some(format!("Error loading profile: {}", e));
            }
        }
    }

    /// Delete the currently selected profile.
    pub fn on_delete_profile_clicked(&mut self) {
        let sel = match self.selected_profile.clone() {
            Some(s) => s,
            None => {
                self.profile_status = Some("Error: No profile selected to delete".to_string());
                return;
            }
        };

        match profile::delete_profile(&sel, None) {
            Ok(()) => {
                let msg = format!("Profile '{}' deleted", sel);
                self.status_text = msg.clone();
                self.profile_status = Some(msg);
                self.selected_profile = None;
                self.refresh_profiles();
            }
            Err(e) => {
                self.profile_status = Some(format!("Error deleting profile: {}", e));
            }
        }
    }

    /// Save the current GUI configuration as a named profile.
    pub fn save_current_profile(&mut self) {
        let name = self.profile_name_input.trim().to_string();
        if name.is_empty() {
            self.profile_status = Some("Error: Profile name cannot be empty".to_string());
            return;
        }

        let is_color_effect = self.selected_effect == "Solid" || self.selected_effect == "Breathe";
        let color = if is_color_effect {
            Some((self.color_rgb[0], self.color_rgb[1], self.color_rgb[2]))
        } else {
            None
        };

        let prof = Profile::new_with_speed(
            name.to_string(),
            self.selected_effect.to_ascii_lowercase(),
            color,
            self.brightness,
            self.effect_speed,
            Vec::new(),
        );

        match profile::save_profile(&prof, None) {
            Ok(_) => {
                let msg = format!("Profile '{}' saved", name);
                self.status_text = msg.clone();
                self.profile_status = Some(msg);
                self.refresh_profiles();
                self.selected_profile = Some(name.to_string());
                self.profile_name_input.clear();
            }
            Err(e) => {
                self.profile_status = Some(format!("Error saving profile: {}", e));
            }
        }
    }

    /// Set the currently selected profile as the default startup profile.
    pub fn on_set_default_clicked(&mut self) {
        let sel = match self.selected_profile.clone() {
            Some(s) => s,
            None => {
                self.profile_status =
                    Some("Error: No profile selected to set as default".to_string());
                return;
            }
        };

        match profile::set_default_profile(&sel, None) {
            Ok(()) => {
                let msg = format!("Profile '{}' set as default", sel);
                self.status_text = msg.clone();
                self.profile_status = Some(msg);
            }
            Err(e) => {
                self.profile_status = Some(format!("Error setting default profile: {}", e));
            }
        }
    }

    /// Clear the default startup profile marker.
    pub fn on_clear_default_clicked(&mut self) {
        match profile::clear_default_profile(None) {
            Ok(()) => {
                let msg = "Default profile cleared".to_string();
                self.status_text = msg.clone();
                self.profile_status = Some(msg);
            }
            Err(e) => {
                self.profile_status = Some(format!("Error clearing default profile: {}", e));
            }
        }
    }
}

fn profile_item_label(name: &str, default_name: Option<&str>) -> String {
    if profile::is_default(name, default_name) {
        profile::format_default_label(name)
    } else {
        name.to_string()
    }
}

fn selected_profile_label(selected: Option<&String>, default_name: Option<&str>) -> String {
    match selected {
        Some(p) => profile_item_label(p, default_name),
        None => "No profiles saved".to_string(),
    }
}

fn render_default_profile_actions(
    app: &mut AulaF75App,
    ui: &mut egui::Ui,
    default_name: Option<&str>,
) {
    let is_default = app
        .selected_profile
        .as_deref()
        .map(|name| profile::is_default(name, default_name))
        .unwrap_or(false);

    if is_default {
        if ui.button("Clear default").clicked() {
            app.on_clear_default_clicked();
        }
    } else {
        if app.selected_profile.is_some() {
            if ui.button("Set as default").clicked() {
                app.on_set_default_clicked();
            }
        }
        if default_name.is_some() {
            if ui.button("Clear default").clicked() {
                app.on_clear_default_clicked();
            }
        }
    }
}

fn render_profile_selection_row(app: &mut AulaF75App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Saved profiles:")
                .size(12.0)
                .color(COLOR_TEXT_SECONDARY),
        );
        let default_name = profile::get_default_profile(None);
        let current_selection =
            selected_profile_label(app.selected_profile.as_ref(), default_name.as_deref());

        egui::ComboBox::from_label("##profile_combo")
            .selected_text(&current_selection)
            .show_ui(ui, |ui| {
                for p in &app.saved_profiles {
                    let label = profile_item_label(p, default_name.as_deref());
                    ui.selectable_value(
                        &mut app.selected_profile,
                        Some(p.clone()),
                        label,
                    );
                }
            });

        if ui.button(RichText::new("Apply").strong()).clicked() {
            app.on_apply_profile_clicked();
        }

        if ui
            .button(RichText::new("Delete").color(COLOR_STATUS_ERROR))
            .clicked()
        {
            app.on_delete_profile_clicked();
        }

        render_default_profile_actions(app, ui, default_name.as_deref());
    });
}

fn render_profile_save_row(app: &mut AulaF75App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Save current as:")
                .size(12.0)
                .color(COLOR_TEXT_SECONDARY),
        );
        ui.text_edit_singleline(&mut app.profile_name_input);
        if ui.button("Save Profile").clicked() {
            app.save_current_profile();
        }
    });
}

fn render_profile_status(app: &AulaF75App, ui: &mut egui::Ui) {
    if let Some(ref status) = app.profile_status {
        ui.add_space(4.0);
        let color = if status.starts_with("Error") {
            COLOR_STATUS_ERROR
        } else {
            COLOR_STATUS_STREAMING
        };
        ui.colored_label(color, status);
    }
}

/// Render the lighting profiles management panel.
pub fn render_profiles_panel(app: &mut AulaF75App, ui: &mut egui::Ui) {
    ui.label(
        RichText::new("Lighting Profiles")
            .strong()
            .size(15.0)
            .color(COLOR_TEXT_PRIMARY),
    );
    ui.add_space(6.0);

    card_frame().show(ui, |ui| {
        render_profile_selection_row(app, ui);
        ui.add_space(8.0);
        render_profile_save_row(app, ui);
        render_profile_status(app, ui);
    });
}

pub(crate) fn resolve_profile_effect_name(effect: &str) -> &'static str {
    match effect.to_ascii_lowercase().as_str() {
        "solid" => "Solid",
        "rainbow" => "Rainbow",
        "wave" => "Wave",
        "breathe" => "Breathe",
        "stars" => "Stars",
        "off" => "Off",
        _ => "Solid",
    }
}
