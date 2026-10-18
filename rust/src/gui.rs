//! Native egui/eframe GUI for AULA F75 mechanical keyboard configuration.
//!
//! Provides a reactive, non-blocking desktop interface for direct RGB lighting
//! control and host-rendered lighting effects. All USB HID communication is executed
//! on a dedicated worker thread via Linux `hidraw` ioctls, ensuring the UI rendering
//! thread is never blocked.

use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, Vec2};

pub use crate::cli::parse_color_hex;
use crate::cli::music::{self, MusicMode};
use crate::cli::profile::{self, Profile};
use crate::device;
use crate::effects::{self, Effect, RGB};
use crate::proto;

// --- Theme Tokens & Layout Metrics ---

/// Neutral background colors.
pub const COLOR_BG_PANEL: Color32 = Color32::from_rgb(18, 20, 26);
pub const COLOR_BG_CENTRAL: Color32 = Color32::from_rgb(23, 25, 33);
pub const COLOR_BG_CARD: Color32 = Color32::from_rgb(29, 32, 42);
pub const COLOR_BG_CARD_HOVER: Color32 = Color32::from_rgb(37, 41, 54);
pub const COLOR_BG_INPUT: Color32 = Color32::from_rgb(15, 17, 22);

/// Border and separator strokes.
pub const COLOR_BORDER_SUBTLE: Color32 = Color32::from_rgb(42, 46, 60);
pub const COLOR_BORDER_ACCENT: Color32 = Color32::from_rgb(0, 180, 216);

/// Typography contrast hierarchy.
pub const COLOR_TEXT_PRIMARY: Color32 = Color32::from_rgb(240, 243, 250);
pub const COLOR_TEXT_SECONDARY: Color32 = Color32::from_rgb(160, 166, 184);
pub const COLOR_TEXT_MUTED: Color32 = Color32::from_rgb(110, 116, 134);

/// Single cohesive accent: Nordic Teal.
pub const COLOR_ACCENT: Color32 = Color32::from_rgb(0, 180, 216);
pub const COLOR_ACCENT_HOVER: Color32 = Color32::from_rgb(20, 200, 235);
pub const COLOR_ACCENT_BG: Color32 = Color32::from_rgb(12, 44, 54);

/// Connection and status indicator colors.
pub const COLOR_STATUS_STREAMING: Color32 = Color32::from_rgb(34, 197, 94);
pub const COLOR_STATUS_IDLE: Color32 = Color32::from_rgb(56, 189, 248);
pub const COLOR_STATUS_CONNECTING: Color32 = Color32::from_rgb(250, 204, 21);
pub const COLOR_STATUS_DISCONNECTED: Color32 = Color32::from_rgb(148, 163, 184);
pub const COLOR_STATUS_ERROR: Color32 = Color32::from_rgb(248, 113, 113);

/// Helper to construct a uniform CornerRadius.
pub const fn corner_radius(radius: u8) -> CornerRadius {
    CornerRadius {
        nw: radius,
        ne: radius,
        sw: radius,
        se: radius,
    }
}

/// Compute live rendered RGB output tuple after applying brightness scaling.
pub fn compute_preview_rgb(rgb: [u8; 3], brightness: f32) -> (u8, u8, u8) {
    let factor = brightness.clamp(0.0, 1.0);
    (
        ((rgb[0] as f32) * factor).round() as u8,
        ((rgb[1] as f32) * factor).round() as u8,
        ((rgb[2] as f32) * factor).round() as u8,
    )
}

/// Connection state category for the side panel hardware status badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionBadgeState {
    Streaming,
    ConnectedIdle,
    Connecting,
    Disconnected,
    Error,
}

impl ConnectionBadgeState {
    /// Determine badge state from app fields.
    pub fn determine(
        has_node: bool,
        is_streaming: bool,
        status_text: &str,
        has_error: bool,
    ) -> Self {
        if has_error {
            Self::Error
        } else if has_node {
            if is_streaming {
                Self::Streaming
            } else {
                Self::ConnectedIdle
            }
        } else if status_text == "Disconnected" {
            Self::Disconnected
        } else {
            Self::Connecting
        }
    }

    /// Label text displayed on the badge.
    pub fn label(self) -> &'static str {
        match self {
            Self::Streaming => "Streaming Active",
            Self::ConnectedIdle => "Connected (idle)",
            Self::Connecting => "Connecting...",
            Self::Disconnected => "Disconnected",
            Self::Error => "Error",
        }
    }

    /// Primary dot indicator color.
    pub fn dot_color(self) -> Color32 {
        match self {
            Self::Streaming => COLOR_STATUS_STREAMING,
            Self::ConnectedIdle => COLOR_STATUS_IDLE,
            Self::Connecting => COLOR_STATUS_CONNECTING,
            Self::Disconnected => COLOR_STATUS_DISCONNECTED,
            Self::Error => COLOR_STATUS_ERROR,
        }
    }

    /// Background pill fill color.
    pub fn bg_color(self) -> Color32 {
        match self {
            Self::Streaming => Color32::from_rgb(18, 48, 30),
            Self::ConnectedIdle => Color32::from_rgb(16, 42, 54),
            Self::Connecting => Color32::from_rgb(48, 40, 16),
            Self::Disconnected => Color32::from_rgb(32, 36, 44),
            Self::Error => Color32::from_rgb(52, 20, 24),
        }
    }
}

/// Create a cohesive, modern dark theme visuals configuration for the AULA F75 GUI.
pub fn app_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.dark_mode = true;
    visuals.panel_fill = COLOR_BG_PANEL;
    visuals.window_fill = COLOR_BG_CENTRAL;
    visuals.faint_bg_color = COLOR_BG_CARD;
    visuals.extreme_bg_color = COLOR_BG_INPUT;
    visuals.code_bg_color = COLOR_BG_INPUT;

    let card_radius = corner_radius(6);
    visuals.window_corner_radius = card_radius;

    // Non-interactive widgets
    visuals.widgets.noninteractive.bg_fill = COLOR_BG_CARD;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, COLOR_BORDER_SUBTLE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, COLOR_TEXT_SECONDARY);
    visuals.widgets.noninteractive.corner_radius = card_radius;

    // Inactive interactive widgets
    visuals.widgets.inactive.bg_fill = COLOR_BG_CARD;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, COLOR_BORDER_SUBTLE);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, COLOR_TEXT_PRIMARY);
    visuals.widgets.inactive.corner_radius = card_radius;

    // Hovered widgets
    visuals.widgets.hovered.bg_fill = COLOR_BG_CARD_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, COLOR_ACCENT);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.hovered.corner_radius = card_radius;

    // Active (pressed) widgets
    visuals.widgets.active.bg_fill = COLOR_ACCENT_BG;
    visuals.widgets.active.bg_stroke = Stroke::new(1.5_f32, COLOR_ACCENT_HOVER);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.active.corner_radius = card_radius;

    // Open menus / dropdowns
    visuals.widgets.open.bg_fill = COLOR_BG_CARD;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, COLOR_BORDER_SUBTLE);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, COLOR_TEXT_PRIMARY);
    visuals.widgets.open.corner_radius = card_radius;

    // Selection highlight
    visuals.selection.bg_fill = COLOR_ACCENT_BG;
    visuals.selection.stroke = Stroke::new(1.0_f32, COLOR_ACCENT);

    visuals
}

/// Configure 4px-based spacing and sizing hierarchy for the application style.
pub fn configure_app_style(style: &mut egui::Style) {
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.indent = 16.0;
}

/// Standard container frame for cards in the modern UI layout.
pub fn card_frame() -> egui::Frame {
    egui::Frame::default()
        .fill(COLOR_BG_CARD)
        .corner_radius(corner_radius(6))
        .stroke(Stroke::new(1.0_f32, COLOR_BORDER_SUBTLE))
        .inner_margin(Margin::same(12))
}

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
    /// Worker is waiting or retrying to connect/reconnect to device node.
    Connecting,
    /// Worker successfully opened device node in idle/connect-only mode.
    Connected(PathBuf),
    /// Alias variant for Connected state in idle-connected mode.
    IdleConnected(PathBuf),
    /// Worker lost connection to device node.
    Disconnected,
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
    mut sleep_fn: S,
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
                    current_keys = None;
                    continue;
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
                let is_transient = code == Some(libc::EPIPE) || code == Some(libc::ETIMEDOUT);
                if is_transient {
                    consecutive_failures += 1;
                    if consecutive_failures >= 10 {
                        let _ = status_tx.send(WorkerStatus::Error(format!(
                            "Device communication failed after 10 retries: {}",
                            e
                        )));
                        break;
                    }
                    let backoff = Duration::from_millis(10 * consecutive_failures as u64);
                    sleep_fn(backoff);
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
                    current_brightness = 1.0;
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

/// Helper to build direct, off, or dynamic effect frame.
fn render_effect_frame(
    effect: &Effect,
    brightness: f32,
    t: f64,
    key_slots: &[u8],
) -> Result<[u8; proto::FRAME_LEN], String> {
    match effect {
        Effect::Solid(rgb) => {
            let scaled_rgb = rgb.scale(brightness).to_tuple();
            proto::build_direct_frame(scaled_rgb)
                .map_err(|e| format!("Frame build error: {}", e))
        }
        Effect::Off => proto::build_direct_frame((0, 0, 0))
            .map_err(|e| format!("Frame build error: {}", e)),
        dynamic_effect => {
            let pairs = effects::render(dynamic_effect, t, key_slots);
            let scaled = effects::apply_brightness(&pairs, brightness);
            proto::build_slot_frame(&scaled)
                .map_err(|e| format!("Frame build error: {}", e))
        }
    }
}

/// Helper to build per-key frame.
fn render_keys_frame(
    keys: &[(String, (u8, u8, u8))],
) -> Result<[u8; proto::FRAME_LEN], String> {
    let borrowed: Vec<(&str, (u8, u8, u8))> =
        keys.iter().map(|(k, c)| (k.as_str(), *c)).collect();
    proto::build_key_frame(&borrowed)
        .map_err(|e| format!("Frame build error: {}", e))
}

/// Core reconnecting worker loop that automatically discovers and reopens the device.
///
/// When no device is open (initial startup, unplug, or communication error),
/// retries `resolve_fn` at `reconnect_interval` (~2s in production) and resumes streaming
/// automatically with the current parameters when the device reappears.
pub fn run_worker_reconnect_loop<R, O, D, F, S, I>(
    rx: Receiver<WorkerMessage>,
    status_tx: Sender<WorkerStatus>,
    initial_cmd: I,
    reconnect_interval: Duration,
    keepalive_interval: Duration,
    mut resolve_fn: R,
    mut open_fn: O,
    mut send_fn: F,
    mut sleep_fn: S,
) where
    R: FnMut() -> Option<PathBuf>,
    O: FnMut(&PathBuf) -> Result<D, io::Error>,
    F: FnMut(&mut D, &[u8; proto::FRAME_LEN]) -> Result<(), io::Error>,
    S: FnMut(Duration),
    I: Into<Option<WorkerMessage>>,
{
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let (mut current_effect, mut current_brightness, mut current_keys, mut current_music, mut is_idle) =
        match initial_cmd.into() {
            Some(WorkerMessage::SetColor(rgb)) => (
                Effect::Solid(RGB(rgb.0, rgb.1, rgb.2)),
                1.0f32,
                None,
                None,
                false,
            ),
            Some(WorkerMessage::SetEffect { effect, brightness }) => {
                (effect, brightness, None, None, false)
            }
            Some(WorkerMessage::SetKeyFrame(keys)) => {
                (Effect::Off, 1.0f32, Some(keys), None, false)
            }
            Some(WorkerMessage::SetMusic { mode, color, gain }) => {
                (Effect::Off, 1.0f32, None, Some((mode, color, gain)), false)
            }
            Some(WorkerMessage::Off) => (Effect::Off, 1.0f32, None, None, true),
            Some(WorkerMessage::Stop) | None => (Effect::Off, 1.0f32, None, None, true),
        };

    let mut active_device: Option<(PathBuf, D)> = None;
    let mut reported_connecting = false;
    let mut consecutive_failures = 0;
    let key_slots = proto::all_key_slots();
    let stream_start = Instant::now();

    let mut music_child: Option<ChildGuard> = None;
    let mut music_sample_buf = std::collections::VecDeque::with_capacity(2048);
    let mut music_mags = vec![0.0f32; 512];
    let mut music_autogain = music::AutoGain::new();
    let music_plan = music::FftPlan::new(1024);

    'outer: loop {
        // 1. Device resolution & opening when not connected
        if active_device.is_none() {
            if !reported_connecting {
                let _ = status_tx.send(WorkerStatus::Connecting);
                reported_connecting = true;
            }

            if let Some(node) = resolve_fn() {
                match open_fn(&node) {
                    Ok(dev) => {
                        if is_idle {
                            let _ = status_tx.send(WorkerStatus::Connected(node.clone()));
                        } else {
                            let _ = status_tx.send(WorkerStatus::Running(node.clone()));
                        }
                        active_device = Some((node, dev));
                        reported_connecting = false;
                        consecutive_failures = 0;
                    }
                    Err(e) => {
                        let _ = status_tx.send(WorkerStatus::Error(format!(
                            "Failed to open {}: {}",
                            node.display(),
                            e
                        )));
                    }
                }
            }

            if active_device.is_none() {
                let wait_start = Instant::now();
                while wait_start.elapsed() < reconnect_interval {
                    let remaining = reconnect_interval.saturating_sub(wait_start.elapsed());
                    let poll_step = remaining.min(Duration::from_millis(50));
                    match rx.recv_timeout(poll_step) {
                        Ok(WorkerMessage::Stop) => {
                            is_idle = true;
                            current_music = None;
                            current_keys = None;
                            current_effect = Effect::Off;
                        }
                        Ok(WorkerMessage::Off) => {
                            is_idle = true;
                            current_music = None;
                            current_keys = None;
                            current_effect = Effect::Off;
                        }
                        Ok(WorkerMessage::SetColor(rgb)) => {
                            is_idle = false;
                            current_music = None;
                            current_keys = None;
                            current_effect = Effect::Solid(RGB(rgb.0, rgb.1, rgb.2));
                            current_brightness = 1.0;
                        }
                        Ok(WorkerMessage::SetEffect { effect, brightness }) => {
                            is_idle = false;
                            current_music = None;
                            current_keys = None;
                            current_effect = effect;
                            current_brightness = brightness;
                        }
                        Ok(WorkerMessage::SetKeyFrame(keys)) => {
                            is_idle = false;
                            current_music = None;
                            current_keys = Some(keys);
                        }
                        Ok(WorkerMessage::SetMusic { mode, color, gain }) => {
                            is_idle = false;
                            current_keys = None;
                            current_music = Some((mode, color, gain));
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => {
                            break 'outer;
                        }
                    }
                }
                continue 'outer;
            }
        }

        // 2. If in idle/connect-only mode, send NO frames and wait for commands or device disconnect
        if is_idle {
            let wait_start = Instant::now();
            while wait_start.elapsed() < reconnect_interval {
                let remaining = reconnect_interval.saturating_sub(wait_start.elapsed());
                let poll_step = remaining.min(Duration::from_millis(50));
                match rx.recv_timeout(poll_step) {
                    Ok(WorkerMessage::SetColor(rgb)) => {
                        is_idle = false;
                        current_music = None;
                        current_keys = None;
                        current_effect = Effect::Solid(RGB(rgb.0, rgb.1, rgb.2));
                        current_brightness = 1.0;
                        if let Some((ref node, _)) = active_device {
                            let _ = status_tx.send(WorkerStatus::Running(node.clone()));
                        }
                        continue 'outer;
                    }
                    Ok(WorkerMessage::SetEffect { effect, brightness }) => {
                        is_idle = false;
                        current_music = None;
                        current_keys = None;
                        current_effect = effect;
                        current_brightness = brightness;
                        if let Some((ref node, _)) = active_device {
                            let _ = status_tx.send(WorkerStatus::Running(node.clone()));
                        }
                        continue 'outer;
                    }
                    Ok(WorkerMessage::SetKeyFrame(keys)) => {
                        is_idle = false;
                        current_music = None;
                        current_keys = Some(keys);
                        if let Some((ref node, _)) = active_device {
                            let _ = status_tx.send(WorkerStatus::Running(node.clone()));
                        }
                        continue 'outer;
                    }
                    Ok(WorkerMessage::SetMusic { mode, color, gain }) => {
                        is_idle = false;
                        current_keys = None;
                        current_music = Some((mode, color, gain));
                        if let Some((ref node, _)) = active_device {
                            let _ = status_tx.send(WorkerStatus::Running(node.clone()));
                        }
                        continue 'outer;
                    }
                    Ok(WorkerMessage::Off) => {
                        if let Some((_, ref mut dev)) = active_device {
                            if let Ok(off_frame) = proto::build_direct_frame((0, 0, 0)) {
                                let _ = send_fn(dev, &off_frame);
                            }
                        }
                        if let Some((ref node, _)) = active_device {
                            let _ = status_tx.send(WorkerStatus::IdleConnected(node.clone()));
                        }
                    }
                    Ok(WorkerMessage::Stop) => {
                        if let Some((ref node, _)) = active_device {
                            let _ = status_tx.send(WorkerStatus::IdleConnected(node.clone()));
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        break 'outer;
                    }
                }
            }

            // Check if device is still connected
            if let Some((ref current_node, _)) = active_device {
                let still_present = match resolve_fn() {
                    Some(ref node) => node == current_node,
                    None => false,
                };
                if !still_present {
                    let _ = status_tx.send(WorkerStatus::Disconnected);
                    active_device = None;
                    reported_connecting = false;
                }
            }
            continue 'outer;
        }

        let t = stream_start.elapsed().as_secs_f64();

        // 3. Audio monitor child process for music mode
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

        // 4. Build RGB frame
        let frame = if let Some((mode, color, _)) = current_music {
            if music_sample_buf.len() >= 1024 {
                let block: Vec<f32> = music_sample_buf.iter().copied().collect();
                music_plan.compute(&block, &mut music_mags);
                let band_energies =
                    music::bands_with_autogain(&music_mags, 16, 44100.0, &mut music_autogain);
                let _ = status_tx.send(WorkerStatus::MusicBands(band_energies.clone()));
                let pairs = music::bands_to_colors(&band_energies, color, mode);
                match proto::build_slot_frame(&pairs) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = status_tx
                            .send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                        active_device = None;
                        continue 'outer;
                    }
                }
            } else {
                match proto::build_direct_frame((0, 0, 0)) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = status_tx
                            .send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                        active_device = None;
                        continue 'outer;
                    }
                }
            }
        } else if let Some(ref keys) = current_keys {
            match render_keys_frame(keys) {
                Ok(f) => f,
                Err(e) => {
                    let _ = status_tx.send(WorkerStatus::Error(e));
                    current_keys = None;
                    continue 'outer;
                }
            }
        } else {
            match render_effect_frame(&current_effect, current_brightness, t, &key_slots) {
                Ok(f) => f,
                Err(e) => {
                    let _ = status_tx.send(WorkerStatus::Error(e));
                    active_device = None;
                    continue 'outer;
                }
            }
        };

        // 5. Transmit frame to open device
        if let Some((_, ref mut dev)) = active_device {
            match send_fn(dev, &frame) {
                Ok(()) => {
                    consecutive_failures = 0;
                }
                Err(e) => {
                    let code = e.raw_os_error();
                    let is_transient = code == Some(libc::EPIPE) || code == Some(libc::ETIMEDOUT);
                    if is_transient {
                        consecutive_failures += 1;
                        if consecutive_failures >= 10 {
                            let _ = status_tx.send(WorkerStatus::Error(format!(
                                "Device communication failed after 10 retries: {}",
                                e
                            )));
                            let _ = status_tx.send(WorkerStatus::Disconnected);
                            // Close device handle and transition to reconnecting
                            active_device = None;
                            consecutive_failures = 0;
                        } else {
                            let backoff = Duration::from_millis(10 * consecutive_failures as u64);
                            sleep_fn(backoff);
                        }
                    } else {
                        let _ = status_tx.send(WorkerStatus::Error(format!(
                            "Device communication error: {}",
                            e
                        )));
                        let _ = status_tx.send(WorkerStatus::Disconnected);
                        // Fatal error (e.g. ENODEV on unplug): close and reconnect
                        active_device = None;
                        consecutive_failures = 0;
                    }
                }
            }
        }

        // 6. Calculate tick interval and process channel messages
        let tick_interval = if current_music.is_some() {
            Duration::from_millis(50)
        } else if current_keys.is_some() {
            keepalive_interval
        } else {
            match current_effect {
                Effect::Solid(_) => keepalive_interval,
                _ => Duration::from_millis(40),
            }
        };

        let start = Instant::now();
        while start.elapsed() < tick_interval {
            let remaining = tick_interval.saturating_sub(start.elapsed());
            let poll_step = remaining.min(Duration::from_millis(10));
            match rx.recv_timeout(poll_step) {
                Ok(WorkerMessage::SetColor(new_color)) => {
                    current_music = None;
                    music_child = None;
                    current_keys = None;
                    current_effect = Effect::Solid(RGB(new_color.0, new_color.1, new_color.2));
                    current_brightness = 1.0;
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
                    if let Some((_, ref mut dev)) = active_device {
                        if let Ok(off_frame) = proto::build_direct_frame((0, 0, 0)) {
                            let _ = send_fn(dev, &off_frame);
                        }
                    }
                    is_idle = true;
                    if let Some((ref node, _)) = active_device {
                        let _ = status_tx.send(WorkerStatus::IdleConnected(node.clone()));
                    }
                    break;
                }
                Ok(WorkerMessage::Stop) => {
                    music_child = None;
                    is_idle = true;
                    if let Some((ref node, _)) = active_device {
                        let _ = status_tx.send(WorkerStatus::IdleConnected(node.clone()));
                    }
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    break 'outer;
                }
            }
        }
    }

    drop(music_child);
    drop(active_device);
    let _ = status_tx.send(WorkerStatus::Stopped);
}

/// Production worker thread runner that manages device handle lifetime.
fn run_worker_thread<I: Into<Option<WorkerMessage>>>(
    rx: Receiver<WorkerMessage>,
    status_tx: Sender<WorkerStatus>,
    initial_cmd: I,
) {
    let keepalive_interval = Duration::from_secs_f64(1.0 / proto::KEEPALIVE_HZ);
    run_worker_reconnect_loop(
        rx,
        status_tx,
        initial_cmd,
        Duration::from_secs(2),
        keepalive_interval,
        device::resolve_control_node,
        |path| OpenOptions::new().read(true).write(true).open(path),
        |file, frame| proto::hid_set_feature(file.as_raw_fd(), frame),
        thread::sleep,
    );
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
        }
    }
}

impl AulaF75App {
    /// Create a new instance for eframe initialization and spawn worker in connect-only mode.
    pub fn new(cc: &eframe::CreationContext) -> Self {
        cc.egui_ctx.set_visuals(app_visuals());
        cc.egui_ctx.style_mut(configure_app_style);
        let mut app = Self::default();
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

    /// Spawn worker thread with initial command or in idle mode if None.
    pub fn start_worker<I: Into<Option<WorkerMessage>>>(&mut self, initial_cmd: I) {
        let cmd = initial_cmd.into();
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
            if self.is_worker_running() {
                if let Some(ref tx) = self.worker_tx {
                    let _ = tx.send(msg);
                }
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
            self.effect_speed = prof.speed;
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
}

impl Drop for AulaF75App {
    fn drop(&mut self) {
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
                        self.control_node_path.is_some(),
                        self.is_streaming(),
                        &self.status_text,
                        self.last_error.is_some(),
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
                            if let Some(ref path) = self.control_node_path {
                                ui.label(
                                    RichText::new(path.display().to_string())
                                        .size(11.0)
                                        .monospace()
                                        .color(COLOR_ACCENT),
                                );
                            } else if self.status_text == "Connecting..." {
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
                    if let Some(ref err) = self.last_error {
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
                    } else if self.control_node_path.is_none() && self.status_text == "Disconnected" {
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

            card_frame().show(ui, |ui| {
                // Effect selection as a segmented selectable row
                ui.label(
                    RichText::new("Lighting Effect")
                        .size(12.0)
                        .color(COLOR_TEXT_SECONDARY),
                );
                ui.add_space(4.0);

                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                    for &eff in AVAILABLE_EFFECTS {
                        let is_selected = self.selected_effect.eq_ignore_ascii_case(eff);
                        let (bg, stroke, text_color) = if is_selected {
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
                        };

                        let btn = egui::Button::new(
                            RichText::new(eff).size(12.0).strong().color(text_color),
                        )
                        .fill(bg)
                        .stroke(stroke)
                        .min_size(Vec2::new(62.0, 28.0));

                        if ui.add(btn).clicked() {
                            self.selected_effect = eff.to_string();
                            if self.is_streaming() {
                                self.dispatch_current_effect();
                            }
                        }
                    }
                });

                ui.add_space(8.0);

                // Parameters: Color picker, Sliders, Hex input & Live Color Swatch
                let is_color_effect =
                    self.selected_effect == "Solid" || self.selected_effect == "Breathe";
                if is_color_effect {
                    ui.horizontal(|ui| {
                        // Left column: Controls
                        ui.vertical(|ui| {
                            // Color picker
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Color:")
                                        .size(12.0)
                                        .color(COLOR_TEXT_SECONDARY),
                                );
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
                                ui.label(
                                    RichText::new("Hex:")
                                        .size(12.0)
                                        .color(COLOR_TEXT_SECONDARY),
                                );
                                let hex_resp = ui.text_edit_singleline(&mut self.hex_input);
                                if hex_resp.lost_focus() || hex_resp.changed() {
                                    if let Ok(parsed) = parse_color_hex(&self.hex_input) {
                                        self.color_rgb = [parsed.0, parsed.1, parsed.2];
                                        self.hex_error = None;
                                        if self.is_streaming() {
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
                                ui.colored_label(COLOR_STATUS_ERROR, err);
                            }
                        });

                        ui.add_space(16.0);

                        // Right column: Live Color Preview Swatch
                        let preview_rgb = compute_preview_rgb(self.color_rgb, self.brightness);
                        let preview_color =
                            Color32::from_rgb(preview_rgb.0, preview_rgb.1, preview_rgb.2);
                        let base_hex = format!(
                            "#{:02X}{:02X}{:02X}",
                            self.color_rgb[0], self.color_rgb[1], self.color_rgb[2]
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
                    });

                    ui.add_space(6.0);
                }

                // Brightness slider
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Brightness:")
                            .size(12.0)
                            .color(COLOR_TEXT_SECONDARY),
                    );
                    let pct_label = format!("{:.0}%", self.brightness * 100.0);
                    if ui
                        .add(egui::Slider::new(&mut self.brightness, 0.0..=1.0).text(pct_label))
                        .changed()
                    {
                        if self.is_streaming() {
                            self.dispatch_current_effect();
                        }
                    }
                });

                // Speed slider (visible only for effects supporting animation speed: Rainbow, Wave, Breathe)
                let is_speed_effect = self.selected_effect == "Rainbow"
                    || self.selected_effect == "Wave"
                    || self.selected_effect == "Breathe"
                    || self.selected_effect.eq_ignore_ascii_case("rainbow")
                    || self.selected_effect.eq_ignore_ascii_case("wave")
                    || self.selected_effect.eq_ignore_ascii_case("breathe");
                if is_speed_effect {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Speed:")
                                .size(12.0)
                                .color(COLOR_TEXT_SECONDARY),
                        );
                        let speed_label = format!("{:.1}x", self.effect_speed);
                        if ui
                            .add(egui::Slider::new(&mut self.effect_speed, 0.25..=3.0).text(speed_label))
                            .changed()
                        {
                            if self.is_streaming() {
                                self.dispatch_current_effect();
                            }
                        }
                    });
                }

                ui.add_space(10.0);

                // Action buttons: Start Streaming, Stop / Off (no emoji icons)
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
                        self.dispatch_current_effect();
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
                        self.on_off_clicked();
                    }
                });
            });

            ui.add_space(14.0);

            // Lighting Profiles Panel
            ui.label(
                RichText::new("Lighting Profiles")
                    .strong()
                    .size(15.0)
                    .color(COLOR_TEXT_PRIMARY),
            );
            ui.add_space(6.0);

            card_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Saved profiles:")
                            .size(12.0)
                            .color(COLOR_TEXT_SECONDARY),
                    );
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

                    if ui.button(RichText::new("Apply").strong()).clicked() {
                        self.on_apply_profile_clicked();
                    }

                    if ui
                        .button(RichText::new("Delete").color(COLOR_STATUS_ERROR))
                        .clicked()
                    {
                        self.on_delete_profile_clicked();
                    }
                });

                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Save current as:")
                            .size(12.0)
                            .color(COLOR_TEXT_SECONDARY),
                    );
                    ui.text_edit_singleline(&mut self.profile_name_input);
                    if ui.button("Save Profile").clicked() {
                        self.save_current_profile();
                    }
                });

                if let Some(ref status) = self.profile_status {
                    ui.add_space(4.0);
                    let color = if status.starts_with("Error") {
                        COLOR_STATUS_ERROR
                    } else {
                        COLOR_STATUS_STREAMING
                    };
                    ui.colored_label(color, status);
                }
            });

            ui.add_space(14.0);

            // Music Mode (Audio Reactive Lighting)
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
                    ui.label(
                        RichText::new("Base Color:")
                            .size(12.0)
                            .color(COLOR_TEXT_SECONDARY),
                    );
                    let mut srgb = self.color_rgb;
                    if ui.color_edit_button_srgb(&mut srgb).changed() {
                        self.set_color_from_picker(srgb);
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

                // Start / Stop Music button (plain text, no emoji icons)
                ui.horizontal(|ui| {
                    if self.is_music_running() {
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
                            self.on_off_clicked();
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
                            self.start_music();
                        }
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
                    ui.label(
                        RichText::new("Audio Level:")
                            .size(12.0)
                            .color(COLOR_TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::ProgressBar::new(avg_level)
                            .text(format!("{:.0}%", avg_level * 100.0))
                            .animate(self.is_music_running()),
                    );
                });
            });

            ui.add_space(14.0);

            // Status footer
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_worker_stream_set_color_after_zero_brightness_effect() {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (status_tx, _status_rx) = mpsc::channel();

        let sent_frames = Arc::new(Mutex::new(Vec::new()));
        let sent_frames_clone = sent_frames.clone();

        let handle = std::thread::spawn(move || {
            run_worker_stream(
                cmd_rx,
                status_tx,
                WorkerMessage::SetEffect {
                    effect: Effect::Rainbow { period_s: 5.0 },
                    brightness: 0.0,
                },
                Duration::from_millis(20),
                move |frame| {
                    sent_frames_clone.lock().unwrap().push(*frame);
                    Ok(())
                },
                |_| {},
            );
        });

        // Wait for initial effect frame with brightness 0.0
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            if !sent_frames.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        // Send SetColor((255, 0, 0))
        cmd_tx.send(WorkerMessage::SetColor((255, 0, 0))).unwrap();

        // Wait until solid frame with [255, 0, 0] is emitted
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            let frames = sent_frames.lock().unwrap();
            if let Some(last) = frames.last() {
                if &last[8..11] == &[255, 0, 0] {
                    break;
                }
            }
            drop(frames);
            std::thread::sleep(Duration::from_millis(5));
        }

        let _ = cmd_tx.send(WorkerMessage::Stop);
        drop(cmd_tx);
        handle.join().expect("Worker thread panicked");

        let frames = sent_frames.lock().unwrap();
        assert!(!frames.is_empty());
        let last_frame = frames.last().unwrap();
        assert_eq!(&last_frame[8..11], &[255, 0, 0]);
    }

    #[test]
    fn test_worker_reconnect_loop_set_color_resets_brightness() {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (status_tx, _status_rx) = mpsc::channel();

        let sent_frames = Arc::new(Mutex::new(Vec::new()));
        let sent_frames_clone = sent_frames.clone();

        let handle = std::thread::spawn(move || {
            run_worker_reconnect_loop(
                cmd_rx,
                status_tx,
                WorkerMessage::SetEffect {
                    effect: Effect::Rainbow { period_s: 5.0 },
                    brightness: 0.0,
                },
                Duration::from_millis(10),
                Duration::from_millis(20),
                || Some(PathBuf::from("/dev/fake_node")),
                |_path| Ok(()),
                move |_dev, frame| {
                    sent_frames_clone.lock().unwrap().push(*frame);
                    Ok(())
                },
                |_| {},
            );
        });

        // Wait for initial effect frame
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            if !sent_frames.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        // Send SetColor((255, 0, 0))
        cmd_tx.send(WorkerMessage::SetColor((255, 0, 0))).unwrap();

        // Wait until solid frame with [255, 0, 0] is emitted
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            let frames = sent_frames.lock().unwrap();
            if let Some(last) = frames.last() {
                if &last[8..11] == &[255, 0, 0] {
                    break;
                }
            }
            drop(frames);
            std::thread::sleep(Duration::from_millis(5));
        }

        let _ = cmd_tx.send(WorkerMessage::Stop);
        drop(cmd_tx);
        handle.join().expect("Worker thread panicked");

        let frames = sent_frames.lock().unwrap();
        assert!(!frames.is_empty());
        let last_frame = frames.last().unwrap();
        assert_eq!(&last_frame[8..11], &[255, 0, 0]);
    }

    #[test]
    fn test_worker_stream_survives_bad_key_frame() {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();

        let sent_frames = Arc::new(Mutex::new(Vec::new()));
        let sent_frames_clone = sent_frames.clone();

        let handle = std::thread::spawn(move || {
            run_worker_stream(
                cmd_rx,
                status_tx,
                WorkerMessage::SetColor((10, 20, 30)),
                Duration::from_millis(20),
                move |frame| {
                    sent_frames_clone.lock().unwrap().push(*frame);
                    Ok(())
                },
                |_| {},
            );
        });

        // Wait for initial frame
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            if !sent_frames.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        // Send SetKeyFrame with an unknown key name
        cmd_tx
            .send(WorkerMessage::SetKeyFrame(vec![(
                "completely_invalid_key_alias".to_string(),
                (255, 0, 0),
            )]))
            .unwrap();

        // Poll for error status
        let start = Instant::now();
        let mut got_error = false;
        while start.elapsed() < Duration::from_millis(500) {
            while let Ok(status) = status_rx.try_recv() {
                if let WorkerStatus::Error(msg) = status {
                    if msg.contains("Frame build error") || msg.contains("Unknown key name") {
                        got_error = true;
                    }
                }
            }
            if got_error {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(got_error, "Expected WorkerStatus::Error on bad key");

        // Worker must still be alive! Send valid color
        cmd_tx.send(WorkerMessage::SetColor((40, 50, 60))).unwrap();

        // Wait for updated frame
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            let frames = sent_frames.lock().unwrap();
            if let Some(last) = frames.last() {
                if &last[8..11] == &[40, 50, 60] {
                    break;
                }
            }
            drop(frames);
            std::thread::sleep(Duration::from_millis(5));
        }

        let _ = cmd_tx.send(WorkerMessage::Stop);
        drop(cmd_tx);
        handle.join().expect("Worker thread panicked");

        let frames = sent_frames.lock().unwrap();
        assert!(!frames.is_empty());
        let last_frame = frames.last().unwrap();
        assert_eq!(&last_frame[8..11], &[40, 50, 60]);
    }

    #[test]
    fn test_worker_reconnect_loop_retains_active_device_on_bad_key() {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();

        let open_count = Arc::new(AtomicUsize::new(0));
        let open_count_clone = open_count.clone();

        let handle = std::thread::spawn(move || {
            run_worker_reconnect_loop(
                cmd_rx,
                status_tx,
                WorkerMessage::SetColor((10, 20, 30)),
                Duration::from_millis(10),
                Duration::from_millis(20),
                || Some(PathBuf::from("/dev/fake_node")),
                move |_path| {
                    open_count_clone.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                |_dev, _frame| Ok(()),
                |_| {},
            );
        });

        // Wait for device to open
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            if open_count.load(Ordering::SeqCst) >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(open_count.load(Ordering::SeqCst), 1);

        // Send bad key
        cmd_tx
            .send(WorkerMessage::SetKeyFrame(vec![(
                "unknown_key_xyz".to_string(),
                (255, 0, 0),
            )]))
            .unwrap();

        // Poll for error status
        let start = Instant::now();
        let mut got_error = false;
        while start.elapsed() < Duration::from_millis(500) {
            while let Ok(status) = status_rx.try_recv() {
                if let WorkerStatus::Error(msg) = status {
                    if msg.contains("Frame build error") || msg.contains("Unknown key name") {
                        got_error = true;
                    }
                }
            }
            if got_error {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(got_error);

        // active_device was NOT dropped, so open_fn was not called again
        assert_eq!(open_count.load(Ordering::SeqCst), 1);

        let _ = cmd_tx.send(WorkerMessage::Stop);
        drop(cmd_tx);
        handle.join().expect("Worker thread panicked");
    }
}

