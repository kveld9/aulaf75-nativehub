//! State machine, application struct, and eframe App implementation for AULA F75 GUI.

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use eframe::egui::{self, Margin, RichText, Stroke};

use crate::cli::music::MusicMode;
use crate::cli::profile;
use crate::effects::Effect;
use crate::gui::panels;
use crate::gui::reconnect::{run_worker_reconnect_loop, run_worker_thread};
use crate::gui::service::{
    adopt_service_command, resolve_service_unit_path, restore_after_gui, service_args_for_state,
    suspend_for_gui, FakeServiceRunner, ServiceRunner, SystemdServiceRunner,
};
use crate::gui::theme::{
    app_visuals, configure_app_style, corner_radius, COLOR_BG_INPUT, COLOR_BORDER_SUBTLE,
    COLOR_TEXT_MUTED, COLOR_TEXT_PRIMARY, COLOR_TEXT_SECONDARY,
};
use crate::gui::worker::{WorkerMessage, WorkerStatus};
use crate::proto;

/// Lighting mode state in the GUI state machine.
#[derive(Debug, Clone, PartialEq)]
pub enum GuiLightingMode {
    /// Lighting is off or idle (no active keepalive frames).
    Off,
    /// Actively transmitting solid color keepalive frames.
    Solid,
    /// Actively transmitting a host-rendered effect.
    Effect(Effect),
    /// Actively transmitting music-reactive frames.
    Music {
        mode: MusicMode,
        gain: f32,
    },
}

/// State tracking for lighting mode, color, and active effect transitions.
#[derive(Debug, Clone, PartialEq)]
pub struct GuiState {
    pub mode: GuiLightingMode,
    pub rgb: (u8, u8, u8),
    pub streamed: bool,
    pub last_was_keyframe: bool,
}

impl Default for GuiState {
    fn default() -> Self {
        Self {
            mode: GuiLightingMode::Off,
            rgb: (255, 0, 0),
            streamed: false,
            last_was_keyframe: false,
        }
    }
}

impl GuiState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Transition to Solid mode with the requested color.
    pub fn transition_to_solid(&mut self, rgb: (u8, u8, u8)) -> WorkerMessage {
        self.mode = GuiLightingMode::Solid;
        self.rgb = rgb;
        self.streamed = true;
        self.last_was_keyframe = false;
        WorkerMessage::SetColor(rgb)
    }

    /// Transition to Effect mode with the given effect and brightness.
    pub fn transition_to_effect(&mut self, effect: Effect, brightness: f32) -> WorkerMessage {
        self.mode = GuiLightingMode::Effect(effect.clone());
        self.streamed = true;
        self.last_was_keyframe = false;
        WorkerMessage::SetEffect { effect, brightness }
    }

    /// Transition to Music mode with the requested mode, color, and gain.
    pub fn transition_to_music(
        &mut self,
        mode: MusicMode,
        color: (u8, u8, u8),
        gain: f32,
    ) -> WorkerMessage {
        self.mode = GuiLightingMode::Music { mode, gain };
        self.rgb = color;
        self.streamed = true;
        self.last_was_keyframe = false;
        WorkerMessage::SetMusic { mode, color, gain }
    }

    /// Transition to Off mode.
    pub fn transition_to_off(&mut self) -> WorkerMessage {
        self.mode = GuiLightingMode::Off;
        self.streamed = true;
        self.last_was_keyframe = false;
        WorkerMessage::Off
    }

    /// Transition to keyframe mode.
    pub fn transition_to_keyframe(&mut self, keys: Vec<(String, (u8, u8, u8))>) -> WorkerMessage {
        self.streamed = true;
        self.last_was_keyframe = true;
        WorkerMessage::SetKeyFrame(keys)
    }

    /// Transition to stopped state.
    pub fn transition_to_stop(&mut self) -> WorkerMessage {
        self.mode = GuiLightingMode::Off;
        WorkerMessage::Stop
    }
}

/// Main AULA F75 eframe GUI Application.
pub struct AulaF75App {
    /// App state machine tracking lighting mode and color.
    pub state: GuiState,
    /// RGB channel buffer for GUI controls.
    pub color_rgb: [u8; 3],
    /// Hex text field string buffer.
    pub hex_input: String,
    /// Error message for hex parsing if invalid.
    pub hex_error: Option<String>,
    /// Global brightness factor (0.0 to 1.0).
    pub brightness: f32,
    /// Animation speed multiplier (0.25 to 3.0, default 1.0).
    pub effect_speed: f32,
    /// Selected effect name.
    pub selected_effect: String,

    /// Path to resolved control node.
    pub control_node_path: Option<PathBuf>,
    /// Header status summary text.
    pub status_text: String,
    /// Last error encountered during device operations.
    pub last_error: Option<String>,

    /// Saved profile names detected on disk.
    pub saved_profiles: Vec<String>,
    /// Selected profile name in the profiles combo box.
    pub selected_profile: Option<String>,
    /// Text input buffer for profile name when saving.
    pub profile_name_input: String,
    /// Feedback status message specific to profile operations.
    pub profile_status: Option<String>,

    /// Selected music reactive mode (Bars, Pulse).
    pub music_mode: MusicMode,
    /// Gain multiplier for music mode (0.1..=5.0).
    pub music_gain: f32,
    /// Last received audio band energies for level meter.
    pub last_music_bands: Vec<f32>,

    /// Channel sender to worker thread.
    pub worker_tx: Option<Sender<WorkerMessage>>,
    /// Channel receiver for status events from worker thread.
    pub status_rx: Option<Receiver<WorkerStatus>>,
    /// Background worker thread handle.
    pub worker_handle: Option<JoinHandle<()>>,

    /// Background service runner for single-writer coordination.
    pub service_runner: Option<Box<dyn ServiceRunner>>,
    /// Whether the background service was active before GUI takeover.
    pub service_was_active: bool,
    /// Whether lighting was streamed at least once during this GUI session.
    pub streamed: bool,
    /// Whether the last dispatched lighting frame was a per-key frame (SetKeyFrame).
    pub last_was_keyframe: bool,
    /// Custom path to the systemd unit file (for hardware-free tests).
    pub custom_service_unit_path: Option<PathBuf>,
}

impl Default for AulaF75App {
    fn default() -> Self {
        let initial_rgb = [255, 0, 0];
        let initial_hex = "FF0000".to_string();

        let saved = profile::list_profiles(None).unwrap_or_default();
        let initial_selected = saved.first().cloned();

        Self {
            state: GuiState {
                mode: GuiLightingMode::Off,
                rgb: (initial_rgb[0], initial_rgb[1], initial_rgb[2]),
                streamed: false,
                last_was_keyframe: false,
            },
            color_rgb: initial_rgb,
            hex_input: initial_hex,
            hex_error: None,
            brightness: 1.0,
            effect_speed: 1.0,
            selected_effect: "Solid".to_string(),
            control_node_path: None,
            status_text: "Connecting...".to_string(),
            last_error: None,
            saved_profiles: saved,
            selected_profile: initial_selected,
            profile_name_input: String::new(),
            profile_status: None,
            music_mode: MusicMode::Bars,
            music_gain: 1.0,
            last_music_bands: Vec::new(),
            worker_tx: None,
            status_rx: None,
            worker_handle: None,
            service_runner: None,
            service_was_active: false,
            streamed: false,
            last_was_keyframe: false,
            custom_service_unit_path: None,
        }
    }
}

impl AulaF75App {
    /// Create a new instance for eframe initialization, park systemd service, and spawn worker in connect-only mode.
    pub fn new(cc: &eframe::CreationContext) -> Self {
        cc.egui_ctx.set_visuals(app_visuals());
        cc.egui_ctx.style_mut(configure_app_style);
        let mut app = Self::default();
        let mut runner = Box::new(SystemdServiceRunner::default());
        app.service_was_active = suspend_for_gui(runner.as_mut());
        app.service_runner = Some(runner);
        app.start_worker(None);
        app
    }

    /// Create a new instance with custom/fake worker runners (for hardware-free testing).
    pub fn new_with_runner<R, O, D, F, S>(
        reconnect_interval: Duration,
        resolve_fn: R,
        open_fn: O,
        send_fn: F,
        sleep_fn: S,
    ) -> Self
    where
        R: FnMut() -> Option<PathBuf> + Send + 'static,
        O: FnMut(&PathBuf) -> Result<D, io::Error> + Send + 'static,
        F: FnMut(&mut D, &[u8; proto::FRAME_LEN]) -> Result<(), io::Error> + Send + 'static,
        S: FnMut(Duration) + Send + 'static,
        D: Send + 'static,
    {
        let mut app = Self::default();
        let mut runner = Box::new(FakeServiceRunner::default());
        app.service_was_active = suspend_for_gui(runner.as_mut());
        app.service_runner = Some(runner);
        app.start_worker_with(
            None,
            reconnect_interval,
            resolve_fn,
            open_fn,
            send_fn,
            sleep_fn,
        );
        app
    }

    /// Create a test instance with an injectable service runner without spawning worker threads.
    pub fn new_with_service_runner<SR: ServiceRunner + 'static>(mut runner: SR) -> Self {
        let mut app = Self::default();
        app.service_was_active = suspend_for_gui(&mut runner);
        app.service_runner = Some(Box::new(runner));
        app
    }

    /// Restore the background service, adopting the last GUI state if lighting was streamed.
    ///
    /// If the GUI streamed at least once, adopts the last active lighting state into
    /// `aula-f75.service` (rewriting ExecStart and reloading/enabling/restarting),
    /// regardless of whether the service was initially active.
    /// If the GUI was idle (never streamed), restores the service only if it was active
    /// prior to GUI launch.
    ///
    /// Ensures restore/adopt is executed at most once.
    pub fn restore_service(&mut self) {
        let did_stream = self.streamed || self.state.streamed;
        if did_stream {
            self.adopt_service();
            self.streamed = false;
            self.state.streamed = false;
            self.service_was_active = false;
        } else if self.service_was_active {
            if let Some(ref mut runner) = self.service_runner {
                restore_after_gui(runner.as_mut(), true);
            }
            self.service_was_active = false;
        }
    }

    fn adopt_service(&mut self) {
        let is_keyframe = self.last_was_keyframe || self.state.last_was_keyframe;
        let mode = if is_keyframe {
            GuiLightingMode::Solid
        } else {
            self.state.mode.clone()
        };
        let rgb = (self.color_rgb[0], self.color_rgb[1], self.color_rgb[2]);
        let args = service_args_for_state(&mode, rgb, self.brightness, self.effect_speed);

        let runner = match self.service_runner.as_mut() {
            Some(r) => r,
            None => return,
        };

        let unit_path = self
            .custom_service_unit_path
            .clone()
            .or_else(|| resolve_service_unit_path(None));

        if let Some(path) = unit_path {
            adopt_service_command(&path, &args, runner.as_mut());
        } else {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runner.daemon_reload();
                runner.enable();
                runner.restart();
            }));
        }
    }

    /// Check if the streaming worker thread is active.
    pub fn is_worker_running(&self) -> bool {
        match self.worker_handle {
            Some(ref handle) => !handle.is_finished(),
            None => false,
        }
    }

    /// Check if lighting streaming is active (solid, effect, or music).
    pub fn is_streaming(&self) -> bool {
        self.is_worker_running() && self.state.mode != GuiLightingMode::Off
    }

    /// Check if music reactive mode is actively streaming.
    pub fn is_music_running(&self) -> bool {
        self.is_worker_running() && matches!(self.state.mode, GuiLightingMode::Music { .. })
    }

    /// Poll worker status channel non-blockingly and update UI state.
    pub fn poll_worker_status(&mut self) {
        let pending: Vec<WorkerStatus> = {
            let rx = match self.status_rx.as_ref() {
                Some(r) => r,
                None => return,
            };
            let mut v = Vec::new();
            while let Ok(s) = rx.try_recv() {
                v.push(s);
            }
            v
        };

        for status in pending {
            self.handle_worker_status(status);
        }
    }

    fn handle_worker_status(&mut self, status: WorkerStatus) {
        match status {
            WorkerStatus::Connecting => self.on_status_connecting(),
            WorkerStatus::Connected(path) => self.on_status_connected(path),
            WorkerStatus::IdleConnected(path) => self.on_status_idle_connected(path),
            WorkerStatus::Disconnected => self.on_status_disconnected(),
            WorkerStatus::Running(path) => self.on_status_running(path),
            WorkerStatus::Stopped => self.on_status_stopped(),
            WorkerStatus::Error(err) => self.on_status_error(err),
            WorkerStatus::MusicBands(bands) => self.last_music_bands = bands,
        }
    }

    fn on_status_connecting(&mut self) {
        self.control_node_path = None;
        self.status_text = "Connecting...".to_string();
    }

    fn on_status_connected(&mut self, path: PathBuf) {
        self.control_node_path = Some(path.clone());
        self.status_text = format!("Connected ({})", path.display());
        self.last_error = None;
    }

    fn on_status_idle_connected(&mut self, path: PathBuf) {
        self.control_node_path = Some(path);
        self.status_text = "Connected (idle)".to_string();
        self.last_error = None;
    }

    fn on_status_disconnected(&mut self) {
        self.control_node_path = None;
        self.status_text = "Disconnected".to_string();
        self.last_music_bands.clear();
    }

    fn on_status_running(&mut self, path: PathBuf) {
        self.control_node_path = Some(path.clone());
        self.status_text = format!("Streaming active on {}", path.display());
        self.last_error = None;
    }

    fn on_status_stopped(&mut self) {
        self.worker_tx = None;
        self.worker_handle = None;
        self.status_rx = None;
        self.control_node_path = None;
        self.state.mode = GuiLightingMode::Off;
        self.last_music_bands.clear();
        self.status_text = "Disconnected".to_string();
    }

    fn on_status_error(&mut self, err: String) {
        self.last_error = Some(err.clone());
        self.status_text = format!("Error: {}", err);
    }

    /// Track worker message for adopt-on-exit state and keyframe detection.
    pub fn note_worker_message(&mut self, msg: &WorkerMessage) {
        match msg {
            WorkerMessage::SetKeyFrame(_) => {
                self.streamed = true;
                self.last_was_keyframe = true;
                self.state.streamed = true;
                self.state.last_was_keyframe = true;
            }
            WorkerMessage::Stop => {}
            _ => {
                self.streamed = true;
                self.last_was_keyframe = false;
                self.state.streamed = true;
                self.state.last_was_keyframe = false;
            }
        }
    }

    /// Spawn worker thread with initial command or in idle mode if None.
    pub fn start_worker<I: Into<Option<WorkerMessage>>>(&mut self, initial_cmd: I) {
        let cmd = initial_cmd.into();
        if let Some(ref msg) = cmd {
            self.note_worker_message(msg);
        }
        if self.is_worker_running() {
            if let Some(ref tx) = self.worker_tx {
                if let Some(msg) = cmd {
                    let _ = tx.send(msg);
                }
                return;
            }
        }
        self.stop_worker();

        let (tx_cmd, rx_cmd) = mpsc::channel();
        let (tx_status, rx_status) = mpsc::channel();

        let handle = thread::spawn(move || {
            run_worker_thread(rx_cmd, tx_status, cmd);
        });

        self.worker_tx = Some(tx_cmd);
        self.status_rx = Some(rx_status);
        self.worker_handle = Some(handle);
        self.last_error = None;
    }

    /// Spawn worker thread with custom injectable runners (for hardware-free testing).
    pub fn start_worker_with<R, O, D, F, S, I>(
        &mut self,
        initial_cmd: I,
        reconnect_interval: Duration,
        resolve_fn: R,
        open_fn: O,
        send_fn: F,
        sleep_fn: S,
    ) where
        R: FnMut() -> Option<PathBuf> + Send + 'static,
        O: FnMut(&PathBuf) -> Result<D, io::Error> + Send + 'static,
        F: FnMut(&mut D, &[u8; proto::FRAME_LEN]) -> Result<(), io::Error> + Send + 'static,
        S: FnMut(Duration) + Send + 'static,
        D: Send + 'static,
        I: Into<Option<WorkerMessage>>,
    {
        let cmd = initial_cmd.into();
        if let Some(ref msg) = cmd {
            self.note_worker_message(msg);
        }
        if self.is_worker_running() {
            if let Some(ref tx) = self.worker_tx {
                if let Some(msg) = cmd {
                    let _ = tx.send(msg);
                }
                return;
            }
        }
        self.stop_worker();

        let (tx_cmd, rx_cmd) = mpsc::channel();
        let (tx_status, rx_status) = mpsc::channel();
        let keepalive_interval = Duration::from_secs_f64(1.0 / proto::KEEPALIVE_HZ);

        let handle = thread::spawn(move || {
            run_worker_reconnect_loop(
                rx_cmd,
                tx_status,
                cmd,
                reconnect_interval,
                keepalive_interval,
                resolve_fn,
                open_fn,
                send_fn,
                sleep_fn,
            );
        });

        self.worker_tx = Some(tx_cmd);
        self.status_rx = Some(rx_status);
        self.worker_handle = Some(handle);
        self.last_error = None;
    }

    /// Terminate worker thread cleanly without blocking the UI thread.
    pub fn stop_worker(&mut self) {
        let msg = self.state.transition_to_stop();
        if let Some(tx) = self.worker_tx.take() {
            let _ = tx.send(msg);
        }
        self.worker_handle = None;
        self.status_rx = None;
        self.state.mode = GuiLightingMode::Off;
        self.control_node_path = None;
        self.status_text = "Disconnected".to_string();
    }

    /// Render the live footer bar reporting active stream, tick rate, and status.
    pub fn render_status_footer(&self, ui: &mut egui::Ui) {
        let tick_rate_label = if self.is_streaming() {
            if self.is_music_running() {
                "~20 Hz"
            } else {
                match self.selected_effect.to_ascii_lowercase().as_str() {
                    "solid" => "~1.5 Hz",
                    "off" => "0 Hz",
                    _ => "~25 Hz",
                }
            }
        } else {
            "0 Hz"
        };

        let active_name = if self.is_streaming() {
            if self.is_music_running() {
                format!("Music ({})", self.music_mode)
            } else {
                self.selected_effect.clone()
            }
        } else {
            "Idle".to_string()
        };

        egui::Frame::default()
            .fill(COLOR_BG_INPUT)
            .corner_radius(corner_radius(4))
            .stroke(Stroke::new(1.0_f32, COLOR_BORDER_SUBTLE))
            .inner_margin(Margin {
                left: 10,
                right: 10,
                top: 6,
                bottom: 6,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Stream:").size(11.0).color(COLOR_TEXT_MUTED));
                    ui.label(
                        RichText::new(&active_name)
                            .size(11.0)
                            .strong()
                            .color(COLOR_TEXT_PRIMARY),
                    );
                    ui.separator();
                    ui.label(RichText::new("Tick Rate:").size(11.0).color(COLOR_TEXT_MUTED));
                    ui.label(
                        RichText::new(tick_rate_label)
                            .size(11.0)
                            .monospace()
                            .color(COLOR_TEXT_PRIMARY),
                    );
                    ui.separator();
                    ui.label(
                        RichText::new(&self.status_text)
                            .size(11.0)
                            .color(COLOR_TEXT_SECONDARY),
                    );
                });
            });
    }
}

impl Drop for AulaF75App {
    fn drop(&mut self) {
        self.restore_service();
        self.stop_worker();
    }
}

impl eframe::App for AulaF75App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker_status();

        // Left side panel: Hardware Status & Device Metadata
        egui::SidePanel::left("device_info")
            .resizable(false)
            .default_width(230.0)
            .show(ctx, |ui| {
                panels::reference::render_side_panel(self, ui);
            });

        // Central panel: Lighting Controls & Effect Engine
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(4.0);
            ui.label(
                RichText::new("RGB Lighting Control")
                    .strong()
                    .size(16.0)
                    .color(COLOR_TEXT_PRIMARY),
            );
            ui.add_space(8.0);

            panels::effects::render_effects_panel(self, ui);

            ui.add_space(14.0);

            panels::profiles::render_profiles_panel(self, ui);

            ui.add_space(14.0);

            panels::music::render_music_panel(self, ui);

            ui.add_space(14.0);

            self.render_status_footer(ui);
        });

        // Request repaint depending on streaming state
        if self.is_streaming() {
            let repaint_interval = if self.is_music_running() {
                Duration::from_millis(40)
            } else if self.selected_effect == "Solid" {
                Duration::from_millis(200)
            } else {
                Duration::from_millis(40)
            };
            ctx.request_repaint_after(repaint_interval);
        } else if self.is_worker_running() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::service::FakeServiceRunner;

    #[test]
    fn test_app_lifecycle_suspends_and_restores_active_service() {
        let fake = FakeServiceRunner::new(true);
        let app = AulaF75App::new_with_service_runner(fake.clone());

        assert_eq!(fake.check_active_count(), 1);
        assert_eq!(fake.stop_count(), 1);
        assert!(app.service_was_active);
        assert!(!fake.is_active());

        drop(app);

        assert_eq!(fake.start_count(), 1);
        assert!(fake.is_active());
    }

    #[test]
    fn test_app_lifecycle_no_stop_when_inactive() {
        let fake = FakeServiceRunner::new(false);
        let app = AulaF75App::new_with_service_runner(fake.clone());

        assert_eq!(fake.check_active_count(), 1);
        assert_eq!(fake.stop_count(), 0);
        assert!(!app.service_was_active);

        drop(app);

        assert_eq!(fake.start_count(), 0);
    }

    #[test]
    fn test_app_restore_service_executed_only_once() {
        let fake = FakeServiceRunner::new(true);
        let mut app = AulaF75App::new_with_service_runner(fake.clone());

        assert_eq!(fake.stop_count(), 1);
        assert!(app.service_was_active);

        app.restore_service();
        assert_eq!(fake.start_count(), 1);
        assert!(!app.service_was_active);

        // Subsequent call must be a no-op
        app.restore_service();
        assert_eq!(fake.start_count(), 1);

        // Drop must not re-trigger restore
        drop(app);
        assert_eq!(fake.start_count(), 1);
    }

    #[test]
    fn test_app_swallows_panicking_service_runner() {
        let fake = FakeServiceRunner::new(true).with_panic();
        let app = AulaF75App::new_with_service_runner(fake);
        // Suspension panic was swallowed
        assert!(!app.service_was_active);
        // Drop must not panic
        drop(app);
    }

    #[test]
    fn test_app_new_with_runner_uses_fake_service() {
        let app = AulaF75App::new_with_runner(
            Duration::from_millis(10),
            || None,
            |_| Ok(()),
            |_, _| Ok(()),
            |_| {},
        );
        assert!(!app.service_was_active);
    }

    #[test]
    fn test_app_adopt_on_exit_when_streamed() {
        let temp_dir = std::env::temp_dir().join(format!(
            "aula_state_adopt_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let unit_path = temp_dir.join("aula-f75.service");

        let initial_content = "[Unit]\nDescription=AULA F75\n\n[Service]\nType=simple\nExecStart=%h/.local/bin/aula-f75-rs solid 303030\n\n[Install]\nWantedBy=default.target\n";
        std::fs::write(&unit_path, initial_content).unwrap();

        let fake = FakeServiceRunner::new(false);
        let mut app = AulaF75App::new_with_service_runner(fake.clone());
        app.custom_service_unit_path = Some(unit_path.clone());
        app.brightness = 0.8;
        app.effect_speed = 1.5;
        app.state
            .transition_to_effect(Effect::Rainbow { period_s: 5.0 }, 0.8);
        app.streamed = true;

        drop(app);

        assert_eq!(fake.daemon_reload_count(), 1);
        assert_eq!(fake.enable_count(), 1);
        assert_eq!(fake.restart_count(), 1);
        assert!(fake.is_active());

        let content = std::fs::read_to_string(&unit_path).unwrap();
        assert!(content.contains("ExecStart=%h/.local/bin/aula-f75-rs fx rainbow --brightness 0.8 --speed 1.5"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_app_adopt_keyframe_falls_back_to_solid() {
        let temp_dir = std::env::temp_dir().join(format!(
            "aula_state_keyframe_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let unit_path = temp_dir.join("aula-f75.service");

        let initial_content = "[Unit]\nDescription=AULA F75\n\n[Service]\nType=simple\nExecStart=%h/.local/bin/aula-f75-rs solid 303030\n\n[Install]\nWantedBy=default.target\n";
        std::fs::write(&unit_path, initial_content).unwrap();

        let fake = FakeServiceRunner::new(false);
        let mut app = AulaF75App::new_with_service_runner(fake.clone());
        app.custom_service_unit_path = Some(unit_path.clone());
        app.color_rgb = [0, 128, 255];
        app.state.rgb = (0, 128, 255);
        app.note_worker_message(&WorkerMessage::SetKeyFrame(vec![(
            "esc".to_string(),
            (255, 0, 0),
        )]));

        assert!(app.streamed);
        assert!(app.last_was_keyframe);

        drop(app);

        assert_eq!(fake.restart_count(), 1);

        let content = std::fs::read_to_string(&unit_path).unwrap();
        assert!(content.contains("ExecStart=%h/.local/bin/aula-f75-rs solid 0080FF"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_app_adopt_executed_only_once() {
        let temp_dir = std::env::temp_dir().join(format!(
            "aula_state_once_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let unit_path = temp_dir.join("aula-f75.service");

        let initial_content = "[Unit]\nDescription=AULA F75\n\n[Service]\nType=simple\nExecStart=%h/.local/bin/aula-f75-rs solid 303030\n";
        std::fs::write(&unit_path, initial_content).unwrap();

        let fake = FakeServiceRunner::new(false);
        let mut app = AulaF75App::new_with_service_runner(fake.clone());
        app.custom_service_unit_path = Some(unit_path.clone());
        app.state.transition_to_solid((255, 0, 0));
        app.streamed = true;

        app.restore_service();
        assert_eq!(fake.restart_count(), 1);

        app.restore_service();
        assert_eq!(fake.restart_count(), 1);

        drop(app);
        assert_eq!(fake.restart_count(), 1);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
