//! Command-line interface definitions and command handlers for AULA F75.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{Parser, Subcommand};

use crate::device;
use crate::proto;

pub mod discovery;
pub mod fx;
pub mod info_list;
pub mod key;
pub mod music_cmd;
pub mod profile_cmd;
pub mod solid_off;

#[path = "../profile.rs"]
pub mod profile;

pub use profile::Profile;

#[path = "../music.rs"]
pub mod music;

pub use music::{AudioConfig, MusicMode};

pub use crate::device::power;
pub use crate::device::power::BatteryStatus;

pub use discovery::{cmd_battery, cmd_knob, cmd_wireless};
pub use fx::{cmd_effects, run_effect_lighting, validate_effect_speed};
pub use info_list::{cmd_info, cmd_list};
pub use key::{cmd_keys, run_key_lighting, run_keys_lighting};
pub use music_cmd::run_music_lighting;
pub use profile_cmd::cmd_profile;
pub use solid_off::run_lighting;

static RUNNING: AtomicBool = AtomicBool::new(true);

extern "C" fn sigint_handler(_: libc::c_int) {
    RUNNING.store(false, Ordering::SeqCst);
}

/// Install minimal libc SIGINT signal handler toggling `RUNNING` atomic boolean.
pub fn install_sigint_handler() {
    unsafe {
        libc::signal(
            libc::SIGINT,
            sigint_handler as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
    }
}

/// Check if execution is still active or interrupted by SIGINT.
pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// Responsive sleep that polls `is_running()` in short increments to allow instant Ctrl+C exit.
pub fn responsive_sleep(duration: Duration) {
    let step = Duration::from_millis(20);
    let start = std::time::Instant::now();
    while start.elapsed() < duration && is_running() {
        let remaining = duration.saturating_sub(start.elapsed());
        std::thread::sleep(remaining.min(step));
    }
}

/// Parse a 6-digit hex string (with optional leading `#`) into an `(R, G, B)` tuple.
pub fn parse_color_hex(val: &str) -> Result<(u8, u8, u8), String> {
    let s = val.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return Err(format!("Color must be exactly 6 hex digits, got {:?}", val));
    }
    if !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("Invalid hex character in color {:?}", val));
    }
    let r = u8::from_str_radix(&s[0..2], 16)
        .map_err(|e| format!("Invalid hex character in color {:?}: {}", val, e))?;
    let g = u8::from_str_radix(&s[2..4], 16)
        .map_err(|e| format!("Invalid hex character in color {:?}: {}", val, e))?;
    let b = u8::from_str_radix(&s[4..6], 16)
        .map_err(|e| format!("Invalid hex character in color {:?}: {}", val, e))?;
    Ok((r, g, b))
}

/// Parse string integer as hex (with or without 0x) or decimal.
pub fn parse_hex_or_dec(s: &str) -> Result<u16, String> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).map_err(|e| format!("Invalid hex value: {}", e))
    } else if let Ok(dec) = s.parse::<u16>() {
        Ok(dec)
    } else if let Ok(hex) = u16::from_str_radix(s, 16) {
        Ok(hex)
    } else {
        Err(format!("Invalid integer or hex value: {:?}", s))
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "aula-f75-rs",
    version,
    about = "Native Linux configuration utility for AULA F75 keyboard"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// List detected HID devices and flag known AULA hardware
    List,
    /// Print details of matching AULA device
    Info {
        /// Filter by device path (e.g. /dev/hidraw0)
        #[arg(long)]
        path: Option<String>,
        /// Filter by Vendor ID (hex or dec)
        #[arg(long)]
        vid: Option<String>,
        /// Filter by Product ID (hex or dec)
        #[arg(long)]
        pid: Option<String>,
    },
    /// Set solid RGB color on all keys
    Solid {
        /// Hex color string (e.g. 'ff0000', '#00ff00')
        color: String,
        /// Duration to maintain color in seconds (0 = hold until Ctrl+C)
        #[arg(long)]
        seconds: Option<f64>,
    },
    /// Turn off RGB lighting (solid 000000 for ~2s)
    Off {
        /// Duration to maintain off frame in seconds (default: 2.0s)
        #[arg(long, default_value_t = 2.0)]
        seconds: f64,
    },
    /// Set RGB color on a single named key
    Key {
        /// Key name or alias (e.g. 'esc', 'space', 'enter', 'f1')
        name: String,
        /// Hex color string (e.g. 'ff0000', '#00ff00')
        color: String,
        /// Duration to maintain color in seconds (0 = hold until Ctrl+C)
        #[arg(long)]
        seconds: Option<f64>,
    },
    /// List all known key names and their slot numbers
    Keys,
    /// Stream a host-rendered RGB lighting effect
    Fx {
        /// Effect name (solid, rainbow, wave, breathe, stars, off)
        name: String,
        /// Hex color string (e.g. 'ff0000', '#00ff00') for effects requiring color (solid, breathe)
        #[arg(long)]
        color: Option<String>,
        /// Brightness multiplier (0.0 to 1.0)
        #[arg(long, default_value_t = 1.0)]
        brightness: f32,
        /// Animation speed multiplier (0.25 to 3.0)
        #[arg(long, default_value_t = 1.0)]
        speed: f32,
        /// Duration to run effect in seconds (0 or omitted = run until Ctrl+C)
        #[arg(long)]
        seconds: Option<f64>,
    },
    /// List available host-rendered RGB lighting effects
    Effects,
    /// Manage local lighting profiles (save, load, list, delete, show)
    Profile {
        #[command(subcommand)]
        action: ProfileCommands,
    },
    /// Stream music-reactive lighting based on system audio
    Music {
        /// Reactive lighting mode (bars, pulse)
        #[arg(long, default_value = "bars")]
        mode: String,
        /// Hex color string (e.g. '00ffff', '#ff00aa')
        #[arg(long, default_value = "00ffff")]
        color: String,
        /// Audio gain multiplier (default: 1.0)
        #[arg(long, default_value_t = 1.0)]
        gain: f32,
        /// Duration to run music mode in seconds (0 or omitted = run until Ctrl+C)
        #[arg(long)]
        seconds: Option<f64>,
    },
    /// Query battery status via sysfs power supply and safe HID read
    Battery,
    /// Detect 2.4GHz wireless dongle vs wired cable connection topology
    Wireless,
    /// Report currently observed multifunction knob mode and HID usage source
    Knob,
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum ProfileCommands {
    /// Save a new lighting profile
    Save {
        /// Profile name
        name: String,
        /// Effect name (solid, rainbow, wave, breathe, stars, off)
        #[arg(long)]
        effect: String,
        /// Base hex color string (e.g. 'ff0000', '#00ff00') for solid/breathe
        #[arg(long)]
        color: Option<String>,
        /// Brightness multiplier (0.0 to 1.0)
        #[arg(long, default_value_t = 1.0)]
        brightness: f32,
        /// Per-key color overrides in NAME=COLOR format (can be specified multiple times)
        #[arg(long = "key", action = clap::ArgAction::Append)]
        keys: Vec<String>,
    },
    /// Load and apply a lighting profile
    Load {
        /// Profile name
        name: String,
        /// Duration to maintain profile in seconds (0 or omitted = run until Ctrl+C)
        #[arg(long)]
        seconds: Option<f64>,
    },
    /// List all saved profiles
    List,
    /// Delete a saved profile
    Delete {
        /// Profile name
        name: String,
    },
    /// Show details of a saved profile
    Show {
        /// Profile name
        name: String,
    },
}

/// Execute CLI commands and return exit code.
pub fn run() -> i32 {
    let cli = Cli::parse();
    match cli.command {
        Commands::List => {
            let devices = device::enumerate_devices(Path::new(device::DEFAULT_SYSFS_HIDRAW));
            cmd_list(&devices)
        }
        Commands::Info { path, vid, pid } => {
            let parsed_vid = match vid.as_deref().map(parse_hex_or_dec) {
                Some(Ok(v)) => Some(v),
                Some(Err(e)) => {
                    eprintln!("Error parsing --vid: {}", e);
                    return 2;
                }
                None => None,
            };
            let parsed_pid = match pid.as_deref().map(parse_hex_or_dec) {
                Some(Ok(p)) => Some(p),
                Some(Err(e)) => {
                    eprintln!("Error parsing --pid: {}", e);
                    return 2;
                }
                None => None,
            };
            let devices = device::enumerate_devices(Path::new(device::DEFAULT_SYSFS_HIDRAW));
            cmd_info(&devices, path.as_deref(), parsed_vid, parsed_pid)
        }
        Commands::Solid { color, seconds } => {
            let rgb = match parse_color_hex(&color) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error parsing hex color: {}", e);
                    return 2;
                }
            };
            run_lighting(rgb, seconds)
        }
        Commands::Off { seconds } => {
            let sec = if seconds <= 0.0 { 2.0 } else { seconds };
            run_lighting((0, 0, 0), Some(sec))
        }
        Commands::Key {
            name,
            color,
            seconds,
        } => {
            if proto::slot_for_key(&name).is_none() {
                eprintln!(
                    "Error: Unknown key '{}'. Use 'aula-f75-rs keys' to see available keys.",
                    name
                );
                return 2;
            }
            let rgb = match parse_color_hex(&color) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error parsing hex color: {}", e);
                    return 2;
                }
            };
            run_key_lighting(&name, rgb, seconds)
        }
        Commands::Keys => cmd_keys(),
        Commands::Fx {
            name,
            color,
            brightness,
            speed,
            seconds,
        } => {
            if let Err(e) = validate_effect_speed(speed) {
                eprintln!("Error: {}", e);
                return 2;
            }

            let lower_name = name.trim().to_ascii_lowercase();
            if (lower_name == "solid" || lower_name == "breathe") && color.is_none() {
                eprintln!("Error: Effect '{}' requires --color RRGGBB", name);
                return 2;
            }
            if (lower_name == "rainbow"
                || lower_name == "wave"
                || lower_name == "stars"
                || lower_name == "off")
                && color.is_some()
            {
                eprintln!("Warning: --color is ignored for effect '{}'", name);
            }

            let parsed_color = match color.as_deref() {
                Some(c) => match parse_color_hex(c) {
                    Ok((r, g, b)) => Some(crate::effects::RGB(r, g, b)),
                    Err(e) => {
                        eprintln!("Error parsing hex color: {}", e);
                        return 2;
                    }
                },
                None => None,
            };

            let effect = match crate::effects::Effect::from_name_with_speed(&name, parsed_color, speed) {
                Ok(eff) => eff,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    return 2;
                }
            };

            let clamped_brightness = brightness.clamp(0.0, 1.0);
            run_effect_lighting(&effect, clamped_brightness, seconds)
        }
        Commands::Effects => cmd_effects(),
        Commands::Profile { action } => cmd_profile(&action),
        Commands::Music {
            mode,
            color,
            gain,
            seconds,
        } => {
            let parsed_mode = match mode.parse::<music::MusicMode>() {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    return 2;
                }
            };
            let parsed_color = match parse_color_hex(&color) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error parsing hex color: {}", e);
                    return 2;
                }
            };
            let clamped_gain = gain.max(0.01);
            run_music_lighting(parsed_mode, parsed_color, clamped_gain, seconds)
        }
        Commands::Battery => cmd_battery(),
        Commands::Wireless => cmd_wireless(),
        Commands::Knob => cmd_knob(),
    }
}
#[cfg(test)]
mod tests;

