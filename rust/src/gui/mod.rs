//! Native egui/eframe GUI for AULA F75 mechanical keyboard configuration.
//!
//! Provides a reactive, non-blocking desktop interface for direct RGB lighting
//! control and host-rendered lighting effects. All USB HID communication is executed
//! on a dedicated worker thread via Linux `hidraw` ioctls, ensuring the UI rendering
//! thread is never blocked.

pub mod panels;
pub mod reconnect;
pub mod service;
pub mod state;
pub mod theme;
pub mod worker;

pub use crate::cli::parse_color_hex;
pub use reconnect::run_worker_reconnect_loop;
pub use service::{
    adopt_service_command, resolve_service_unit_path, resolve_systemd_user_dir, restore_after_gui,
    service_args_for_state, suspend_for_gui, ClosureServiceRunner, FakeServiceRunner,
    NoopServiceRunner, ServiceRunner, SystemdServiceRunner, DEFAULT_SERVICE_NAME,
    DEFAULT_SYSTEMCTL_TIMEOUT,
};
pub use state::{AulaF75App, GuiLightingMode, GuiState};
pub use theme::{
    app_visuals, card_frame, compute_preview_rgb, configure_app_style, corner_radius,
    ConnectionBadgeState, COLOR_ACCENT, COLOR_ACCENT_BG, COLOR_ACCENT_HOVER, COLOR_BG_CARD,
    COLOR_BG_CARD_HOVER, COLOR_BG_CENTRAL, COLOR_BG_INPUT, COLOR_BG_PANEL, COLOR_BORDER_ACCENT,
    COLOR_BORDER_SUBTLE, COLOR_STATUS_CONNECTING, COLOR_STATUS_DISCONNECTED, COLOR_STATUS_ERROR,
    COLOR_STATUS_IDLE, COLOR_STATUS_STREAMING, COLOR_TEXT_MUTED, COLOR_TEXT_PRIMARY,
    COLOR_TEXT_SECONDARY,
};
pub use worker::{AVAILABLE_EFFECTS, run_worker_stream, WorkerMessage, WorkerStatus};
