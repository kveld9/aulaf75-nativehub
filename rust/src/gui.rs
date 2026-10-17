//! Native egui/eframe GUI for AULA F75 mechanical keyboard configuration.
//!
//! Provides a reactive, non-blocking desktop interface for direct RGB lighting
//! control. All USB HID communication is executed on a dedicated worker thread
//! via Linux `hidraw` ioctls, ensuring the UI rendering thread is never blocked.

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use eframe::egui;

pub use crate::cli::parse_color_hex;
use crate::device;
use crate::proto;

/// Channel commands dispatched from the GUI thread to the background worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerMessage {
    /// Update active solid color to the given `(R, G, B)` tuple.
    SetColor((u8, u8, u8)),
    /// Transmit all-zero off frame and cleanly stop the keepalive loop.
    Off,
    /// Immediately terminate keepalive loop and close device descriptor.
    Stop,
}

/// Status and lifecycle events emitted by the background worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerStatus {
    /// Worker successfully opened device node and entered keepalive loop.
    Running(PathBuf),
    /// Worker stopped execution cleanly.
    Stopped,
    /// An error occurred during device resolution, opening, or ioctl execution.
    Error(String),
}

/// Lighting mode state in the GUI state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiLightingMode {
    /// Lighting is off or idle (no active keepalive frames).
    Off,
    /// Actively transmitting solid color keepalive frames.
    Solid,
}

/// State tracking for lighting mode and color transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuiState {
    pub mode: GuiLightingMode,
    pub rgb: (u8, u8, u8),
}

impl Default for GuiState {
    fn default() -> Self {
        Self {
            mode: GuiLightingMode::Off,
            rgb: (255, 0, 0),
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
        WorkerMessage::SetColor(rgb)
    }

    /// Transition to Off mode.
    pub fn transition_to_off(&mut self) -> WorkerMessage {
        self.mode = GuiLightingMode::Off;
        WorkerMessage::Off
    }

    /// Transition to stopped state.
    pub fn transition_to_stop(&mut self) -> WorkerMessage {
        self.mode = GuiLightingMode::Off;
        WorkerMessage::Stop
    }
}

/// Core worker streaming loop parameterized over injectable send and sleep functions.
///
/// Implements keepalive transmission at `KEEPALIVE_HZ` (~1.5 Hz) and transient error
/// retries (EPIPE, ETIMEDOUT) matching the CLI implementation.
pub fn run_worker_stream<F, S>(
    rx: Receiver<WorkerMessage>,
    status_tx: Sender<WorkerStatus>,
    initial_cmd: WorkerMessage,
    interval: Duration,
    mut send_fn: F,
    mut sleep_fn: S,
) where
    F: FnMut(&[u8; proto::FRAME_LEN]) -> Result<(), std::io::Error>,
    S: FnMut(Duration),
{
    let mut current_color = match initial_cmd {
        WorkerMessage::SetColor(rgb) => rgb,
        WorkerMessage::Off => {
            if let Ok(off_frame) = proto::build_direct_frame((0, 0, 0)) {
                let _ = send_fn(&off_frame);
            }
            let _ = status_tx.send(WorkerStatus::Stopped);
            return;
        }
        WorkerMessage::Stop => {
            let _ = status_tx.send(WorkerStatus::Stopped);
            return;
        }
    };

    let mut consecutive_failures = 0;

    'outer: loop {
        let frame = match proto::build_direct_frame(current_color) {
            Ok(f) => f,
            Err(e) => {
                let _ = status_tx.send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                break;
            }
        };

        match send_fn(&frame) {
            Ok(()) => {
                consecutive_failures = 0;
            }
            Err(e) => {
                let code = e.raw_os_error();
                if code == Some(libc::EPIPE) || code == Some(libc::ETIMEDOUT) {
                    consecutive_failures += 1;
                    if consecutive_failures >= 10 {
                        let _ = status_tx.send(WorkerStatus::Error(format!(
                            "Device communication failed after 10 retries: {}",
                            e
                        )));
                        break;
                    }
                } else {
                    let _ = status_tx.send(WorkerStatus::Error(format!(
                        "Device communication error: {}",
                        e
                    )));
                    break;
                }
            }
        }

        // Wait for next keepalive tick while polling for UI commands in short steps.
        let start = std::time::Instant::now();
        while start.elapsed() < interval {
            let remaining = interval.saturating_sub(start.elapsed());
            let poll_step = remaining.min(Duration::from_millis(50));
            match rx.recv_timeout(poll_step) {
                Ok(WorkerMessage::SetColor(new_color)) => {
                    current_color = new_color;
                    // Break inner wait loop immediately to dispatch the new color
                    break;
                }
                Ok(WorkerMessage::Off) => {
                    if let Ok(off_frame) = proto::build_direct_frame((0, 0, 0)) {
                        let _ = send_fn(&off_frame);
                    }
                    break 'outer;
                }
                Ok(WorkerMessage::Stop) => {
                    break 'outer;
                }
                Err(RecvTimeoutError::Timeout) => {
                    // Continue sleep interval
                }
                Err(RecvTimeoutError::Disconnected) => {
                    // UI sender dropped (window closed or app exited)
                    break 'outer;
                }
            }
        }
    }

    let _ = status_tx.send(WorkerStatus::Stopped);
}

/// Production worker thread runner that manages device handle lifetime.
fn run_worker_thread(
    rx: Receiver<WorkerMessage>,
    status_tx: Sender<WorkerStatus>,
    initial_cmd: WorkerMessage,
) {
    let node = match device::resolve_control_node() {
        Some(p) => p,
        None => {
            let _ = status_tx.send(WorkerStatus::Error(
                "AULA F75 interface 1 control node not found".to_string(),
            ));
            let _ = status_tx.send(WorkerStatus::Stopped);
            return;
        }
    };

    let file = match OpenOptions::new().read(true).write(true).open(&node) {
        Ok(f) => f,
        Err(e) => {
            let _ = status_tx.send(WorkerStatus::Error(format!(
                "Failed to open {}: {}",
                node.display(),
                e
            )));
            let _ = status_tx.send(WorkerStatus::Stopped);
            return;
        }
    };

    let _ = status_tx.send(WorkerStatus::Running(node));

    let fd = file.as_raw_fd();
    let interval = Duration::from_secs_f64(1.0 / proto::KEEPALIVE_HZ);

    run_worker_stream(
        rx,
        status_tx,
        initial_cmd,
        interval,
        |frame| proto::hid_set_feature(fd, frame),
        thread::sleep,
    );

    // Dropping file closes the underlying file descriptor
    drop(file);
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

    /// Path to resolved control node.
    pub control_node_path: Option<PathBuf>,
    /// Header status summary text.
    pub status_text: String,
    /// Last error encountered during device operations.
    pub last_error: Option<String>,

    /// Channel sender to worker thread.
    worker_tx: Option<Sender<WorkerMessage>>,
    /// Channel receiver for status events from worker thread.
    status_rx: Option<Receiver<WorkerStatus>>,
    /// Background worker thread handle.
    worker_handle: Option<JoinHandle<()>>,
}

impl Default for AulaF75App {
    fn default() -> Self {
        let initial_rgb = [255, 0, 0];
        let initial_hex = "FF0000".to_string();
        let detected = device::resolve_control_node();
        let status_text = match &detected {
            Some(path) => format!("Device detected: {}", path.display()),
            None => "Device not connected".to_string(),
        };

        Self {
            state: GuiState {
                mode: GuiLightingMode::Off,
                rgb: (initial_rgb[0], initial_rgb[1], initial_rgb[2]),
            },
            color_rgb: initial_rgb,
            hex_input: initial_hex,
            hex_error: None,
            control_node_path: detected,
            status_text,
            last_error: None,
            worker_tx: None,
            status_rx: None,
            worker_handle: None,
        }
    }
}

impl AulaF75App {
    /// Create a new instance for eframe initialization.
    pub fn new(_cc: &eframe::CreationContext) -> Self {
        Self::default()
    }

    /// Check if the streaming worker thread is active.
    pub fn is_worker_running(&self) -> bool {
        self.worker_handle.is_some()
    }

    /// Poll worker status channel non-blockingly and update UI state.
    pub fn poll_worker_status(&mut self) {
        if let Some(ref rx) = self.status_rx {
            while let Ok(status) = rx.try_recv() {
                match status {
                    WorkerStatus::Running(path) => {
                        self.control_node_path = Some(path.clone());
                        self.status_text = format!("Streaming active on {}", path.display());
                        self.last_error = None;
                    }
                    WorkerStatus::Stopped => {
                        self.worker_tx = None;
                        if let Some(handle) = self.worker_handle.take() {
                            let _ = handle.join();
                        }
                        self.state.mode = GuiLightingMode::Off;
                        if self.last_error.is_none() {
                            self.status_text = "Idle (Lighting off)".to_string();
                        }
                    }
                    WorkerStatus::Error(err) => {
                        self.last_error = Some(err.clone());
                        self.status_text = format!("Error: {}", err);
                    }
                }
            }
        }
    }

    /// Sync RGB values from color picker or sliders to hex input and active stream.
    pub fn set_color_from_picker(&mut self, srgb: [u8; 3]) {
        self.color_rgb = srgb;
        self.hex_input = format!("{:02X}{:02X}{:02X}", srgb[0], srgb[1], srgb[2]);
        self.hex_error = None;
        if self.is_worker_running() {
            self.send_color((srgb[0], srgb[1], srgb[2]));
        }
    }

    /// Dispatch color change to running worker.
    pub fn send_color(&mut self, rgb: (u8, u8, u8)) {
        let msg = self.state.transition_to_solid(rgb);
        if let Some(ref tx) = self.worker_tx {
            let _ = tx.send(msg);
        }
    }

    /// Handler for "Solid" button click.
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

        if let Some(ref tx) = self.worker_tx {
            let msg = self.state.transition_to_solid(rgb);
            let _ = tx.send(msg);
            self.status_text = format!("Streaming solid #{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2);
        } else {
            self.start_worker(WorkerMessage::SetColor(rgb));
        }
    }

    /// Handler for "Off" button click.
    pub fn on_off_clicked(&mut self) {
        let msg = self.state.transition_to_off();
        if let Some(ref tx) = self.worker_tx {
            let _ = tx.send(msg);
        } else {
            self.start_worker(WorkerMessage::Off);
        }
    }

    /// Spawn worker thread with initial command.
    pub fn start_worker(&mut self, initial_cmd: WorkerMessage) {
        self.stop_worker();

        let (tx_cmd, rx_cmd) = mpsc::channel();
        let (tx_status, rx_status) = mpsc::channel();

        let handle = thread::spawn(move || {
            run_worker_thread(rx_cmd, tx_status, initial_cmd);
        });

        self.worker_tx = Some(tx_cmd);
        self.status_rx = Some(rx_status);
        self.worker_handle = Some(handle);
        self.last_error = None;
    }

    /// Terminate worker thread cleanly and join handle.
    pub fn stop_worker(&mut self) {
        let msg = self.state.transition_to_stop();
        if let Some(tx) = self.worker_tx.take() {
            let _ = tx.send(msg);
        }
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
        self.status_rx = None;
        self.state.mode = GuiLightingMode::Off;
    }
}

impl Drop for AulaF75App {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

impl eframe::App for AulaF75App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker_status();

        egui::CentralPanel::default().show(ctx, |ui| {
            // Status Header
            ui.heading("AULA F75 Control Panel");
            ui.add_space(8.0);

            ui.group(|ui| {
                ui.horizontal(|ui| {
                    let (indicator_color, indicator_text) = if self.is_worker_running() {
                        (egui::Color32::from_rgb(46, 204, 113), "Streaming Active")
                    } else if self.control_node_path.is_some() {
                        (egui::Color32::from_rgb(241, 196, 15), "Device Connected")
                    } else {
                        (egui::Color32::from_rgb(231, 76, 60), "Disconnected")
                    };

                    ui.colored_label(indicator_color, "●");
                    ui.label(format!("{}: {}", indicator_text, self.status_text));
                });

                if let Some(ref path) = self.control_node_path {
                    ui.label(format!("Interface 1 node: {}", path.display()));
                }
                if let Some(ref err) = self.last_error {
                    ui.colored_label(
                        egui::Color32::from_rgb(231, 76, 60),
                        format!("Error: {}", err),
                    );
                }
            });

            ui.add_space(12.0);

            // Lighting Controls
            ui.group(|ui| {
                ui.label("Direct RGB Lighting");
                ui.add_space(6.0);

                // Color picker
                ui.horizontal(|ui| {
                    ui.label("Color picker:");
                    let mut srgb = self.color_rgb;
                    if ui.color_edit_button_srgb(&mut srgb).changed() {
                        self.set_color_from_picker(srgb);
                    }
                });

                ui.add_space(6.0);

                // Manual RGB sliders
                ui.horizontal(|ui| {
                    let mut r = self.color_rgb[0];
                    let mut g = self.color_rgb[1];
                    let mut b = self.color_rgb[2];
                    let mut changed = false;

                    if ui.add(egui::Slider::new(&mut r, 0..=255).text("R")).changed() {
                        changed = true;
                    }
                    if ui.add(egui::Slider::new(&mut g, 0..=255).text("G")).changed() {
                        changed = true;
                    }
                    if ui.add(egui::Slider::new(&mut b, 0..=255).text("B")).changed() {
                        changed = true;
                    }

                    if changed {
                        self.set_color_from_picker([r, g, b]);
                    }
                });

                ui.add_space(6.0);

                // Hex text field
                ui.horizontal(|ui| {
                    ui.label("Hex color:");
                    let hex_resp = ui.text_edit_singleline(&mut self.hex_input);
                    if hex_resp.lost_focus() || hex_resp.changed() {
                        if let Ok(parsed) = parse_color_hex(&self.hex_input) {
                            self.color_rgb = [parsed.0, parsed.1, parsed.2];
                            self.hex_error = None;
                            if self.is_worker_running() {
                                self.send_color(parsed);
                            }
                        } else if !self.hex_input.is_empty() {
                            self.hex_error = Some(
                                "Invalid hex format (expected 6 hex digits, e.g. FF0000 or #00FFFF)".to_string(),
                            );
                        }
                    }
                });

                if let Some(ref err) = self.hex_error {
                    ui.colored_label(egui::Color32::from_rgb(231, 76, 60), err);
                }

                ui.add_space(10.0);

                // Solid / Off Action Buttons
                ui.horizontal(|ui| {
                    if ui.button("Solid").clicked() {
                        self.on_solid_clicked();
                    }

                    if ui.button("Off").clicked() {
                        self.on_off_clicked();
                    }
                });
            });

            ui.add_space(12.0);
            ui.label("Note: Board must be toggled into Custom/Static RGB mode (Fn shortcuts) to display software colors.");
        });

        // Keep UI reactive during streaming
        if self.is_worker_running() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }
}
