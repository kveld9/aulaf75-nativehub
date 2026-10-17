//! Native egui/eframe GUI for AULA F75 mechanical keyboard configuration.
//!
//! Provides a reactive, non-blocking desktop interface for direct RGB lighting
//! control and host-rendered lighting effects. All USB HID communication is executed
//! on a dedicated worker thread via Linux `hidraw` ioctls, ensuring the UI rendering
//! thread is never blocked.

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use eframe::egui;

pub use crate::cli::parse_color_hex;
use crate::cli::music::{self, MusicMode};
use crate::cli::profile::{self, Profile};
use crate::device;
use crate::effects::{self, Effect, RGB};
use crate::proto;

/// Available lighting effect names for the GUI effect selector.
pub const AVAILABLE_EFFECTS: &[&str] = &[
    "Solid",
    "Rainbow",
    "Wave",
    "Breathe",
    "Stars",
    "Off",
];

/// Channel commands dispatched from the GUI thread to the background worker.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerMessage {
    /// Update active solid color to the given `(R, G, B)` tuple.
    SetColor((u8, u8, u8)),
    /// Stream a host-rendered dynamic or static effect with brightness.
    SetEffect {
        effect: Effect,
        brightness: f32,
    },
    /// Stream a per-key direct RGB frame lighting specific keys.
    SetKeyFrame(Vec<(String, (u8, u8, u8))>),
    /// Stream music-reactive lighting based on system audio.
    SetMusic {
        mode: MusicMode,
        color: (u8, u8, u8),
        gain: f32,
    },
    /// Transmit all-zero off frame and cleanly stop the keepalive loop.
    Off,
    /// Immediately terminate keepalive loop and close device descriptor.
    Stop,
}

/// Status and lifecycle events emitted by the background worker.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerStatus {
    /// Worker successfully opened device node and entered keepalive loop.
    Running(PathBuf),
    /// Worker stopped execution cleanly.
    Stopped,
    /// An error occurred during device resolution, opening, or ioctl execution.
    Error(String),
    /// Periodic snapshot of music band energies for GUI level meter.
    MusicBands(Vec<f32>),
}

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

    /// Transition to Effect mode with the given effect and brightness.
    pub fn transition_to_effect(&mut self, effect: Effect, brightness: f32) -> WorkerMessage {
        self.mode = GuiLightingMode::Effect(effect.clone());
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
        WorkerMessage::SetMusic { mode, color, gain }
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
/// Implements keepalive transmission at `KEEPALIVE_HZ` (~1.5 Hz) for solid mode,
/// ~25 Hz (40 ms) for dynamic effects, and transient error retries (EPIPE, ETIMEDOUT).
pub fn run_worker_stream<F, S>(
    rx: Receiver<WorkerMessage>,
    status_tx: Sender<WorkerStatus>,
    initial_cmd: WorkerMessage,
    interval: Duration,
    mut send_fn: F,
    mut _sleep_fn: S,
) where
    F: FnMut(&[u8; proto::FRAME_LEN]) -> Result<(), std::io::Error>,
    S: FnMut(Duration),
{
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let (mut current_effect, mut current_brightness, mut current_keys, mut current_music) =
        match initial_cmd {
            WorkerMessage::SetColor(rgb) => {
                (Effect::Solid(RGB(rgb.0, rgb.1, rgb.2)), 1.0f32, None, None)
            }
            WorkerMessage::SetEffect { effect, brightness } => (effect, brightness, None, None),
            WorkerMessage::SetKeyFrame(keys) => (Effect::Off, 1.0f32, Some(keys), None),
            WorkerMessage::SetMusic { mode, color, gain } => {
                (Effect::Off, 1.0f32, None, Some((mode, color, gain)))
            }
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
    let key_slots = proto::all_key_slots();
    let stream_start = std::time::Instant::now();

    let mut music_child: Option<ChildGuard> = None;
    let mut music_sample_buf = std::collections::VecDeque::with_capacity(2048);
    let mut music_mags = vec![0.0f32; 512];
    let mut music_autogain = music::AutoGain::new();
    let music_plan = music::FftPlan::new(1024);

    'outer: loop {
        let t = stream_start.elapsed().as_secs_f64();

        // Spawn or maintain audio monitor child process if in music mode
        if let Some((_, _, gain)) = current_music {
            if music_child.is_none() {
                match music::spawn_monitor() {
                    Ok(child) => {
                        if let Some(ref stdout) = child.stdout {
                            let fd = stdout.as_raw_fd();
                            unsafe {
                                let flags = libc::fcntl(fd, libc::F_GETFL, 0);
                                if flags >= 0 {
                                    libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                                }
                            }
                        }
                        music_child = Some(ChildGuard(child));
                    }
                    Err(e) => {
                        let _ = status_tx.send(WorkerStatus::Error(e));
                        current_music = None;
                    }
                }
            }

            // Non-blocking read of available audio samples
            let mut read_failed = false;
            if let Some(ref mut guard) = music_child {
                if let Some(ref mut stdout) = guard.0.stdout {
                    let mut chunk_buf = [0u8; 8820];
                    loop {
                        match std::io::Read::read(stdout, &mut chunk_buf) {
                            Ok(0) => {
                                read_failed = true;
                                break;
                            }
                            Ok(n) => {
                                let num_samples = n / 4;
                                for i in 0..num_samples {
                                    let offset = i * 4;
                                    let left = i16::from_le_bytes([chunk_buf[offset], chunk_buf[offset + 1]]);
                                    let right = i16::from_le_bytes([chunk_buf[offset + 2], chunk_buf[offset + 3]]);
                                    let mono = ((left as f32 + right as f32) / (2.0 * 32768.0)) * gain;
                                    music_sample_buf.push_back(mono);
                                }
                                while music_sample_buf.len() > 1024 {
                                    music_sample_buf.pop_front();
                                }
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                break;
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {
                                continue;
                            }
                            Err(_) => {
                                read_failed = true;
                                break;
                            }
                        }
                    }
                }
            }

            if read_failed {
                music_child = None;
                current_music = None;
            }
        }

        let frame = if let Some((mode, color, _)) = current_music {
            if music_sample_buf.len() >= 1024 {
                let block: Vec<f32> = music_sample_buf.iter().copied().collect();
                music_plan.compute(&block, &mut music_mags);
                let band_energies = music::bands_with_autogain(&music_mags, 16, 44100.0, &mut music_autogain);
                let _ = status_tx.send(WorkerStatus::MusicBands(band_energies.clone()));
                let pairs = music::bands_to_colors(&band_energies, color, mode);
                match proto::build_slot_frame(&pairs) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = status_tx.send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                        break;
                    }
                }
            } else {
                match proto::build_direct_frame((0, 0, 0)) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = status_tx.send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                        break;
                    }
                }
            }
        } else if let Some(ref keys) = current_keys {
            let borrowed: Vec<(&str, (u8, u8, u8))> =
                keys.iter().map(|(k, c)| (k.as_str(), *c)).collect();
            match proto::build_key_frame(&borrowed) {
                Ok(f) => f,
                Err(e) => {
                    let _ =
                        status_tx.send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                    break;
                }
            }
        } else {
            match &current_effect {
                Effect::Solid(rgb) => {
                    let scaled_rgb = rgb.scale(current_brightness).to_tuple();
                    match proto::build_direct_frame(scaled_rgb) {
                        Ok(f) => f,
                        Err(e) => {
                            let _ = status_tx
                                .send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                            break;
                        }
                    }
                }
                Effect::Off => match proto::build_direct_frame((0, 0, 0)) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = status_tx
                            .send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                        break;
                    }
                },
                dynamic_effect => {
                    let pairs = effects::render(dynamic_effect, t, &key_slots);
                    let scaled = effects::apply_brightness(&pairs, current_brightness);
                    match proto::build_slot_frame(&scaled) {
                        Ok(f) => f,
                        Err(e) => {
                            let _ = status_tx
                                .send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                            break;
                        }
                    }
                }
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

        let tick_interval = if current_music.is_some() {
            Duration::from_millis(50)
        } else if current_keys.is_some() {
            interval
        } else {
            match current_effect {
                Effect::Solid(_) => interval,
                _ => Duration::from_millis(40),
            }
        };

        let start = std::time::Instant::now();
        while start.elapsed() < tick_interval {
            let remaining = tick_interval.saturating_sub(start.elapsed());
            let poll_step = remaining.min(Duration::from_millis(10));
            match rx.recv_timeout(poll_step) {
                Ok(WorkerMessage::SetColor(new_color)) => {
                    current_music = None;
                    music_child = None;
                    current_keys = None;
                    current_effect = Effect::Solid(RGB(new_color.0, new_color.1, new_color.2));
                    break;
                }
                Ok(WorkerMessage::SetEffect { effect, brightness }) => {
                    current_music = None;
                    music_child = None;
                    current_keys = None;
                    current_effect = effect;
                    current_brightness = brightness;
                    break;
                }
                Ok(WorkerMessage::SetKeyFrame(keys)) => {
                    current_music = None;
                    music_child = None;
                    current_keys = Some(keys);
                    break;
                }
                Ok(WorkerMessage::SetMusic { mode, color, gain }) => {
                    current_keys = None;
                    current_music = Some((mode, color, gain));
                    break;
                }
                Ok(WorkerMessage::Off) => {
                    music_child = None;
                    if let Ok(off_frame) = proto::build_direct_frame((0, 0, 0)) {
                        let _ = send_fn(&off_frame);
                    }
                    break 'outer;
                }
                Ok(WorkerMessage::Stop) => {
                    music_child = None;
                    break 'outer;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    break 'outer;
                }
            }
        }
    }

    drop(music_child);
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
    /// Global brightness factor (0.0 to 1.0).
    pub brightness: f32,
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

        let saved = profile::list_profiles(None).unwrap_or_default();
        let initial_selected = saved.first().cloned();

        Self {
            state: GuiState {
                mode: GuiLightingMode::Off,
                rgb: (initial_rgb[0], initial_rgb[1], initial_rgb[2]),
            },
            color_rgb: initial_rgb,
            hex_input: initial_hex,
            hex_error: None,
            brightness: 1.0,
            selected_effect: "Solid".to_string(),
            control_node_path: detected,
            status_text,
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

    /// Check if music reactive mode is actively streaming.
    pub fn is_music_running(&self) -> bool {
        self.is_worker_running() && matches!(self.state.mode, GuiLightingMode::Music { .. })
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
                        self.last_music_bands.clear();
                        if self.last_error.is_none() {
                            self.status_text = "Idle (Lighting off)".to_string();
                        }
                    }
                    WorkerStatus::Error(err) => {
                        self.last_error = Some(err.clone());
                        self.status_text = format!("Error: {}", err);
                    }
                    WorkerStatus::MusicBands(bands) => {
                        self.last_music_bands = bands;
                    }
                }
            }
        }
    }

    /// Dispatch music reactive lighting to running worker or start a new worker.
    pub fn start_music(&mut self) {
        let rgb = (self.color_rgb[0], self.color_rgb[1], self.color_rgb[2]);
        let msg = self.state.transition_to_music(self.music_mode, rgb, self.music_gain);
        if let Some(ref tx) = self.worker_tx {
            let _ = tx.send(msg);
            self.status_text = format!("Streaming music ({})", self.music_mode);
        } else {
            self.start_worker(msg);
        }
    }

    /// Build current `Effect` based on GUI state.
    pub fn current_effect(&self) -> Effect {
        let base_color = RGB(self.color_rgb[0], self.color_rgb[1], self.color_rgb[2]);
        match self.selected_effect.to_ascii_lowercase().as_str() {
            "solid" => Effect::Solid(base_color),
            "rainbow" => Effect::Rainbow { period_s: 5.0 },
            "wave" => Effect::Wave { period_s: 3.0 },
            "breathe" => Effect::Breathe {
                color: base_color,
                period_s: 4.0,
            },
            "stars" => Effect::Stars { density: 0.15 },
            _ => Effect::Off,
        }
    }

    /// Dispatch current effect and brightness to running worker or start a new worker.
    pub fn dispatch_current_effect(&mut self) {
        let effect = self.current_effect();
        if effect == Effect::Off {
            self.on_off_clicked();
            return;
        }

        let msg = self.state.transition_to_effect(effect.clone(), self.brightness);
        if let Some(ref tx) = self.worker_tx {
            let _ = tx.send(msg);
            self.status_text = format!("Streaming {}", self.selected_effect);
        } else {
            self.start_worker(msg);
        }
    }

    /// Sync RGB values from color picker or sliders to hex input and active stream.
    pub fn set_color_from_picker(&mut self, srgb: [u8; 3]) {
        self.color_rgb = srgb;
        self.hex_input = format!("{:02X}{:02X}{:02X}", srgb[0], srgb[1], srgb[2]);
        self.hex_error = None;
        if self.is_worker_running() {
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

        if let Some(ref tx) = self.worker_tx {
            let msg = self.state.transition_to_solid(rgb);
            let _ = tx.send(msg);
            self.status_text = format!("Streaming solid #{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2);
        } else {
            self.start_worker(WorkerMessage::SetColor(rgb));
        }
    }

    /// Handler for "Off" action button click.
    pub fn on_off_clicked(&mut self) {
        self.selected_effect = "Off".to_string();
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

    /// Refresh the list of saved profiles from the config directory.
    pub fn refresh_profiles(&mut self) {
        match profile::list_profiles(None) {
            Ok(list) => {
                self.saved_profiles = list;
                if let Some(ref sel) = self.selected_profile {
                    if !self.saved_profiles.contains(sel) {
                        self.selected_profile = self.saved_profiles.first().cloned();
                    }
                } else {
                    self.selected_profile = self.saved_profiles.first().cloned();
                }
            }
            Err(e) => {
                self.profile_status = Some(format!("Error listing profiles: {}", e));
            }
        }
    }

    /// Apply a loaded profile configuration to GUI state and worker thread.
    pub fn apply_profile(&mut self, prof: &Profile) {
        if !prof.keys.is_empty() {
            let msg = WorkerMessage::SetKeyFrame(prof.keys.clone());
            if let Some(ref tx) = self.worker_tx {
                let _ = tx.send(msg);
            } else {
                self.start_worker(msg);
            }
            let msg_text = format!("Profile '{}' applied ({} keys)", prof.name, prof.keys.len());
            self.status_text = msg_text.clone();
            self.profile_status = Some(msg_text);
        } else {
            let effect_name = match prof.effect.to_ascii_lowercase().as_str() {
                "solid" => "Solid",
                "rainbow" => "Rainbow",
                "wave" => "Wave",
                "breathe" => "Breathe",
                "stars" => "Stars",
                "off" => "Off",
                _ => "Solid",
            };
            self.selected_effect = effect_name.to_string();
            self.brightness = prof.brightness;
            if let Some(c) = prof.color {
                self.color_rgb = [c.0, c.1, c.2];
                self.hex_input = format!("{:02X}{:02X}{:02X}", c.0, c.1, c.2);
                self.hex_error = None;
            }
            self.dispatch_current_effect();
            let msg_text = format!("Profile '{}' applied", prof.name);
            self.status_text = msg_text.clone();
            self.profile_status = Some(msg_text);
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

        let prof = Profile::new(
            name.to_string(),
            self.selected_effect.to_ascii_lowercase(),
            color,
            self.brightness,
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
}

impl Drop for AulaF75App {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

impl eframe::App for AulaF75App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_visuals(egui::Visuals::dark());
        self.poll_worker_status();

        // Left side panel: Hardware Status & Device Metadata
        egui::SidePanel::left("device_info")
            .resizable(false)
            .default_width(210.0)
            .show(ctx, |ui| {
                ui.heading("Hardware Status");
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
                        ui.label(indicator_text);
                    });

                    ui.add_space(4.0);
                    ui.label("Device: AULA F75");
                    ui.label("VID:PID: 258a:010c");
                    ui.label("Interface: 1 (HID Feature)");

                    if let Some(ref path) = self.control_node_path {
                        ui.label(format!("Node: {}", path.display()));
                    } else {
                        ui.colored_label(
                            egui::Color32::from_rgb(231, 76, 60),
                            "Node: Not Found",
                        );
                    }

                    if let Some(ref err) = self.last_error {
                        ui.add_space(4.0);
                        ui.colored_label(
                            egui::Color32::from_rgb(231, 76, 60),
                            format!("Error: {}", err),
                        );
                    }
                });

                ui.add_space(16.0);
                ui.label("Requirements:");
                ui.label("• Udev rule (99-aula-f75.rules)");
                ui.label("• Keyboard in Custom/Static mode (Fn shortcuts)");
            });

        // Central panel: Lighting Controls & Effect Engine
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("RGB Lighting Control");
            ui.add_space(8.0);

            ui.group(|ui| {
                // Effect selection
                ui.horizontal(|ui| {
                    ui.label("Lighting Effect:");
                    egui::ComboBox::from_label("##effect_select")
                        .selected_text(&self.selected_effect)
                        .show_ui(ui, |ui| {
                            for &eff in AVAILABLE_EFFECTS {
                                if ui
                                    .selectable_value(&mut self.selected_effect, eff.to_string(), eff)
                                    .clicked()
                                {
                                    if self.is_worker_running() {
                                        self.dispatch_current_effect();
                                    }
                                }
                            }
                        });
                });

                ui.add_space(8.0);

                // Parameters: Color picker & Hex input (shown for Solid and Breathe)
                let is_color_effect = self.selected_effect == "Solid" || self.selected_effect == "Breathe";
                if is_color_effect {
                    ui.horizontal(|ui| {
                        ui.label("Color picker:");
                        let mut srgb = self.color_rgb;
                        if ui.color_edit_button_srgb(&mut srgb).changed() {
                            self.set_color_from_picker(srgb);
                        }
                    });

                    ui.add_space(4.0);

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

                    ui.add_space(4.0);

                    // Hex text field
                    ui.horizontal(|ui| {
                        ui.label("Hex color:");
                        let hex_resp = ui.text_edit_singleline(&mut self.hex_input);
                        if hex_resp.lost_focus() || hex_resp.changed() {
                            if let Ok(parsed) = parse_color_hex(&self.hex_input) {
                                self.color_rgb = [parsed.0, parsed.1, parsed.2];
                                self.hex_error = None;
                                if self.is_worker_running() {
                                    self.dispatch_current_effect();
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

                    ui.add_space(6.0);
                }

                // Brightness slider
                ui.horizontal(|ui| {
                    ui.label("Brightness:");
                    let pct_label = format!("{:.0}%", self.brightness * 100.0);
                    if ui
                        .add(egui::Slider::new(&mut self.brightness, 0.0..=1.0).text(pct_label))
                        .changed()
                    {
                        if self.is_worker_running() {
                            self.dispatch_current_effect();
                        }
                    }
                });

                ui.add_space(10.0);

                // Action buttons: Start Streaming, Stop / Off
                ui.horizontal(|ui| {
                    if ui.button("▶ Start Streaming").clicked() {
                        self.dispatch_current_effect();
                    }

                    if ui.button("⏹ Stop / Off").clicked() {
                        self.on_off_clicked();
                    }
                });
            });

            ui.add_space(12.0);

            // Lighting Profiles Panel
            ui.heading("Lighting Profiles");
            ui.add_space(8.0);

            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Saved profiles:");
                    let current_selection = self
                        .selected_profile
                        .as_deref()
                        .unwrap_or("No profiles saved");

                    egui::ComboBox::from_label("##profile_combo")
                        .selected_text(current_selection)
                        .show_ui(ui, |ui| {
                            for p in &self.saved_profiles {
                                ui.selectable_value(
                                    &mut self.selected_profile,
                                    Some(p.clone()),
                                    p,
                                );
                            }
                        });

                    if ui.button("Apply").clicked() {
                        self.on_apply_profile_clicked();
                    }

                    if ui.button("Delete").clicked() {
                        self.on_delete_profile_clicked();
                    }
                });

                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label("Save current as:");
                    ui.text_edit_singleline(&mut self.profile_name_input);
                    if ui.button("Save Profile").clicked() {
                        self.save_current_profile();
                    }
                });

                if let Some(ref status) = self.profile_status {
                    ui.add_space(4.0);
                    let color = if status.starts_with("Error") {
                        egui::Color32::from_rgb(231, 76, 60)
                    } else {
                        egui::Color32::from_rgb(46, 204, 113)
                    };
                    ui.colored_label(color, status);
                }
            });

            ui.add_space(12.0);

            // Music Mode (Audio Reactive Lighting)
            ui.heading("Audio Reactive Lighting (Music Mode)");
            ui.add_space(8.0);

            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Reactive Mode:");
                    let current_mode_str = match self.music_mode {
                        MusicMode::Bars => "Bars (Spectrum across keys)",
                        MusicMode::Pulse => "Pulse (Whole-board beat)",
                    };
                    egui::ComboBox::from_label("##music_mode_select")
                        .selected_text(current_mode_str)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_value(
                                    &mut self.music_mode,
                                    MusicMode::Bars,
                                    "Bars (Spectrum across keys)",
                                )
                                .clicked()
                                && self.is_music_running()
                            {
                                self.start_music();
                            }
                            if ui
                                .selectable_value(
                                    &mut self.music_mode,
                                    MusicMode::Pulse,
                                    "Pulse (Whole-board beat)",
                                )
                                .clicked()
                                && self.is_music_running()
                            {
                                self.start_music();
                            }
                        });
                });

                ui.add_space(6.0);

                // Base Color Picker (reused)
                ui.horizontal(|ui| {
                    ui.label("Base Color:");
                    let mut srgb = self.color_rgb;
                    if ui.color_edit_button_srgb(&mut srgb).changed() {
                        self.set_color_from_picker(srgb);
                    }
                });

                ui.add_space(6.0);

                // Audio Gain Slider
                ui.horizontal(|ui| {
                    ui.label("Audio Gain:");
                    let gain_label = format!("{:.1}x", self.music_gain);
                    if ui
                        .add(egui::Slider::new(&mut self.music_gain, 0.1..=5.0).text(gain_label))
                        .changed()
                        && self.is_music_running()
                    {
                        self.start_music();
                    }
                });

                ui.add_space(8.0);

                // Start / Stop Music button
                ui.horizontal(|ui| {
                    if self.is_music_running() {
                        if ui.button("⏹ Stop Music").clicked() {
                            self.on_off_clicked();
                        }
                    } else if ui.button("▶ Start Music").clicked() {
                        self.start_music();
                    }
                });

                ui.add_space(8.0);

                // Level meter (ProgressBar from last band energies via channel)
                let avg_level = if self.last_music_bands.is_empty() {
                    0.0
                } else {
                    let sum: f32 = self.last_music_bands.iter().sum();
                    (sum / self.last_music_bands.len() as f32).clamp(0.0, 1.0)
                };

                ui.horizontal(|ui| {
                    ui.label("Audio Level:");
                    ui.add(
                        egui::ProgressBar::new(avg_level)
                            .text(format!("{:.0}%", avg_level * 100.0))
                            .animate(self.is_music_running()),
                    );
                });
            });

            ui.add_space(12.0);

            // Status footer
            ui.group(|ui| {
                let tick_rate_label = if self.is_worker_running() {
                    if self.is_music_running() {
                        "~20 Hz (audio stream)"
                    } else {
                        match self.selected_effect.to_ascii_lowercase().as_str() {
                            "solid" => "~1.5 Hz (keepalive)",
                            "off" => "0 Hz (stopped)",
                            _ => "~25 Hz (active stream)",
                        }
                    }
                } else {
                    "0 Hz (idle)"
                };

                let active_name = if self.is_worker_running() {
                    if self.is_music_running() {
                        format!("Music ({})", self.music_mode)
                    } else {
                        self.selected_effect.clone()
                    }
                } else {
                    "None".to_string()
                };

                ui.label(format!(
                    "Active stream: {} | Tick rate: {} | {}",
                    active_name, tick_rate_label, self.status_text
                ));
            });
        });

        // Request repaint depending on streaming state
        if self.is_worker_running() {
            let repaint_interval = if self.is_music_running() {
                Duration::from_millis(40)
            } else if self.selected_effect == "Solid" {
                Duration::from_millis(200)
            } else {
                Duration::from_millis(40)
            };
            ctx.request_repaint_after(repaint_interval);
        }
    }
}
