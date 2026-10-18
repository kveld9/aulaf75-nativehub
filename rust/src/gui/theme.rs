//! Theme tokens, visuals, and layout metrics for AULA F75 GUI.

use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke, Vec2};

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
