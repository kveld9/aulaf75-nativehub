//! Host-rendered lighting effect streaming commands and validation for AULA F75.

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;

use crate::cli::{install_sigint_handler, is_running, responsive_sleep};
use crate::device;
use crate::proto;

/// Print available effect names with descriptions.
pub fn cmd_effects() -> i32 {
    println!("Available RGB effects:");
    println!("  solid:   Static single color across all keys (requires --color)");
    println!("  rainbow: Smooth cyclic whole-board spectrum shift");
    println!("  wave:    Horizontal chromatic wave propagating across keys");
    println!("  breathe: Pulsing illumination fading between black and color (requires --color)");
    println!("  stars:   Deterministic twinkling starry night across keys");
    println!("  off:     Turn off RGB lighting");
    0
}

/// Validate animation speed multiplier is within acceptable range `0.25..=3.0`.
pub fn validate_effect_speed(speed: f32) -> Result<f32, String> {
    if speed < 0.25 || speed > 3.0 || speed.is_nan() {
        Err(format!("Speed must be between 0.25 and 3.0 (got {})", speed))
    } else {
        Ok(speed)
    }
}

/// Run host-rendered RGB effect streaming loop on the hardware interface 1 control node.
pub fn run_effect_lighting(
    effect: &crate::effects::Effect,
    brightness: f32,
    seconds: Option<f64>,
) -> i32 {
    let node = match device::resolve_control_node() {
        Some(n) => n,
        None => {
            eprintln!("Error: AULA F75 interface 1 control node not found.");
            return 1;
        }
    };

    let file = match OpenOptions::new().read(true).write(true).open(&node) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error opening control node {}: {}", node.display(), e);
            return 1;
        }
    };

    install_sigint_handler();

    let fd = file.as_raw_fd();
    let tick_hz = match effect {
        crate::effects::Effect::Solid(_) => proto::KEEPALIVE_HZ,
        _ => 25.0,
    };

    let res = device::run_effect(
        effect,
        brightness,
        seconds,
        tick_hz,
        |frame| proto::hid_set_feature(fd, frame),
        responsive_sleep,
        is_running,
    );

    match res {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Device communication error: {}", e);
            1
        }
    }
}
