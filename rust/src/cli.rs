//! Command-line interface definitions and command handlers for AULA F75.

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{Parser, Subcommand};

use crate::device::{self, DeviceInfo};
use crate::proto;

#[path = "profile.rs"]
pub mod profile;

pub use profile::Profile;

#[path = "music.rs"]
pub mod music;

pub use music::{AudioConfig, MusicMode};

pub use crate::device::power;
pub use crate::device::power::BatteryStatus;

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
    version = "0.2.0",
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

/// List detected HID devices and flag known AULA hardware.
pub fn cmd_list(devices: &[DeviceInfo]) -> i32 {
    if devices.is_empty() {
        println!("No HID devices detected.");
        return 0;
    }

    println!("Detected {} HID interface(s):", devices.len());
    for dev in devices {
        let flag = if dev.is_known { "[MATCH]" } else { "[     ]" };
        let vid_pid = format!("{:04x}:{:04x}", dev.vendor_id, dev.product_id);
        let path = dev.path.to_string_lossy();
        let iface = match dev.interface_number {
            Some(n) if n >= 0 => format!("iface {}", n),
            _ => "iface N/A".to_string(),
        };
        let desc = if !dev.product_string.is_empty() {
            &dev.product_string
        } else {
            &dev.device_name
        };
        println!("  {} {}  {:<20}  ({:<8})  {}", flag, vid_pid, path, iface, desc);
    }
    0
}

/// Print details of matching AULA device.
pub fn cmd_info(
    devices: &[DeviceInfo],
    path: Option<&str>,
    vid: Option<u16>,
    pid: Option<u16>,
) -> i32 {
    let matched: Vec<&DeviceInfo> = if let Some(p) = path {
        devices.iter().filter(|d| d.path.to_string_lossy() == p).collect()
    } else if vid.is_some() && pid.is_some() {
        devices
            .iter()
            .filter(|d| Some(d.vendor_id) == vid && Some(d.product_id) == pid)
            .collect()
    } else {
        devices.iter().filter(|d| d.is_known).collect()
    };

    if matched.is_empty() {
        println!("No matching device found.");
        return 1;
    }

    for (idx, dev) in matched.iter().enumerate() {
        let status_label = if dev.is_known {
            "Known AULA Device"
        } else {
            "Unknown Device"
        };
        println!("Device #{} ({}):", idx + 1, status_label);
        println!("  Device Name:   {}", dev.device_name);
        println!("  Vendor ID:     0x{:04x}", dev.vendor_id);
        println!("  Product ID:    0x{:04x}", dev.product_id);
        println!("  Path:          {}", dev.path.display());
        println!(
            "  Interface:     {}",
            dev.interface_number
                .map(|n| n.to_string())
                .unwrap_or_else(|| "N/A".to_string())
        );
        println!("  Manufacturer:  {}", dev.manufacturer_string);
        println!("  Product:       {}", dev.product_string);
        println!("  Release:       0x{:04x}", dev.release_number);
        println!("  Usage Page:    0x{:04x}", dev.usage_page);
        println!("  Usage:         0x{:04x}", dev.usage);
        println!(
            "  USB Parent:    {}",
            if dev.usb_parent.is_empty() {
                "N/A"
            } else {
                &dev.usb_parent
            }
        );
        println!(
            "  Phys:          {}",
            if dev.phys.is_empty() {
                "N/A"
            } else {
                &dev.phys
            }
        );
        println!("  Connection:    {:?}", dev.classify_connection());
    }

    0
}

/// Run RGB lighting loop on the hardware interface 1 control node.
pub fn run_lighting(rgb: (u8, u8, u8), seconds: Option<f64>) -> i32 {
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
    let res = device::run_keepalive_loop(
        rgb,
        seconds,
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

/// List all known key names and their slot numbers.
pub fn cmd_keys() -> i32 {
    println!("AULA F75 Key Map ({} keys):", proto::KEY_SLOTS.len());
    for &(name, slot) in proto::KEY_SLOTS {
        println!("  {:<12} (slot {:>2})", name, slot);
    }
    0
}

/// Run per-key RGB lighting loop for multiple keys on the hardware interface 1 control node.
pub fn run_keys_lighting(pairs: &[(&str, (u8, u8, u8))], seconds: Option<f64>) -> i32 {
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
    let res = device::send_key_frame(
        pairs,
        seconds,
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

/// Run per-key RGB lighting loop on the hardware interface 1 control node.
pub fn run_key_lighting(name: &str, rgb: (u8, u8, u8), seconds: Option<f64>) -> i32 {
    run_keys_lighting(&[(name, rgb)], seconds)
}

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

/// Stream system-audio reactive lighting.
pub fn run_music_lighting(
    mode: music::MusicMode,
    color: (u8, u8, u8),
    gain: f32,
    seconds: Option<f64>,
) -> i32 {
    let node = match device::resolve_control_node() {
        Some(p) => p,
        None => {
            eprintln!("Error: AULA F75 interface 1 control node not found.");
            eprintln!("Ensure keyboard is connected via USB and udev rule 99-aula-f75.rules is installed.");
            return 1;
        }
    };

    let file = match OpenOptions::new().read(true).write(true).open(&node) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error opening control node {}: {}", node.display(), e);
            eprintln!("Check permissions or install udev rule: sudo cp 99-aula-f75.rules /etc/udev/rules.d/");
            return 1;
        }
    };

    let child = match music::spawn_monitor() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error initializing audio capture: {}", e);
            return 1;
        }
    };

    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut guard = ChildGuard(child);

    let mut stdout = match guard.0.stdout.take() {
        Some(s) => s,
        None => {
            eprintln!("Error: Failed to capture stdout pipe from parec process");
            return 1;
        }
    };

    install_sigint_handler();
    let fd = file.as_raw_fd();

    println!(
        "Streaming music-reactive lighting (mode: {}, color: #{:02x}{:02x}{:02x}, gain: {:.2}) on {}...",
        mode, color.0, color.1, color.2, gain, node.display()
    );
    println!("Press Ctrl+C to stop.");

    let res = music::run_music_loop(
        &mut stdout,
        |frame| {
            device::send_with_retry(
                || proto::hid_set_feature(fd, frame),
                5,
                || std::thread::sleep(Duration::from_millis(5)),
            )
        },
        mode,
        color,
        gain,
        seconds,
        is_running,
    );

    match res {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Device communication error during music mode: {}", e);
            1
        }
    }
}

/// Query and report battery status via sysfs power supply and safe HID read.
pub fn cmd_battery() -> i32 {
    let sys_class_hidraw = Path::new("/sys/class/hidraw");
    let devices = device::enumerate_devices(sys_class_hidraw);
    let aula_devs: Vec<&device::DeviceInfo> = devices.iter().filter(|d| d.is_known).collect();

    if aula_devs.is_empty() {
        eprintln!("Error: AULA F75 device not detected.");
        return 1;
    }

    println!("Querying AULA F75 battery status...");

    let sysfs_power = Path::new("/sys/class/power_supply");
    let status = power::query_sysfs_battery_for_device(sysfs_power, Some("258A:010C"));

    if status.present && status.percent.is_some() {
        println!("Battery Status:");
        println!("  Present:       Yes");
        println!("  Level:         {}%", status.percent.unwrap());
        if let Some(chg) = status.charging {
            println!(
                "  State:         {}",
                if chg {
                    "Charging"
                } else {
                    "Discharging / Not charging"
                }
            );
        }
    } else {
        println!("Battery Status:  Not reported by device");
        println!("  Detail:        No sysfs power_supply or battery telemetry exposed in wired USB mode (258a:010c).");
        println!("  Note:          Requires live USB capture in 2.4GHz wireless / Bluetooth mode.");
    }

    0
}

/// Detect 2.4GHz wireless dongle vs direct cable connection topology.
pub fn cmd_wireless() -> i32 {
    let sys_class_hidraw = Path::new("/sys/class/hidraw");
    let candidates = device::enumerate_wireless_nodes_with_roots(sys_class_hidraw);

    if candidates.is_empty() {
        eprintln!("Error: AULA F75 device not detected.");
        return 1;
    }

    println!("AULA F75 USB Connection & Dongle Detection:");
    let mut dongle_found = false;
    for (i, c) in candidates.iter().enumerate() {
        if c.is_dongle {
            dongle_found = true;
        }
        let iface_str = c
            .interface_number
            .map(|n| format!("Interface {}", n))
            .unwrap_or_else(|| "Interface N/A".to_string());
        println!("  Candidate #{}: {} ({})", i + 1, c.path.display(), iface_str);
        println!(
            "    USB Parent:    {}",
            if c.usb_parent.is_empty() {
                "N/A"
            } else {
                &c.usb_parent
            }
        );
        println!(
            "    Phys:          {}",
            if c.phys.is_empty() { "N/A" } else { &c.phys }
        );
        println!("    Classification: {:?}", c.connection_kind);
        println!("    Detail:        {}", c.detail);
    }

    println!();
    if dongle_found {
        println!("Summary: 2.4GHz wireless dongle receiver detected.");
    } else {
        println!("Summary: Direct USB cable connection active (no 2.4GHz dongle detected).");
    }

    0
}

/// Report currently observed multifunction knob mode.
pub fn cmd_knob() -> i32 {
    let sys_class_hidraw = Path::new("/sys/class/hidraw");
    let devices = device::enumerate_devices(sys_class_hidraw);
    if !devices.iter().any(|d| d.is_known) {
        eprintln!("Error: AULA F75 device not detected.");
        return 1;
    }

    let report = device::query_knob_mode_with_roots(sys_class_hidraw);
    println!("AULA F75 Multifunction Knob Mode:");
    println!("  Behavior Source: {:?}", report.source);
    println!("  Description:     {}", report.description);
    for detail in &report.details {
        println!("  - {}", detail);
    }

    0
}

/// Execute CLI commands and return exit code.
pub fn run() -> i32 {
    let cli = Cli::parse();
    match cli.command {
        Commands::List => {
            let devices = device::enumerate_devices(Path::new("/sys/class/hidraw"));
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
            let devices = device::enumerate_devices(Path::new("/sys/class/hidraw"));
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
            seconds,
        } => {
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

            let effect = match crate::effects::Effect::from_name(&name, parsed_color) {
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

/// Handler for profile CLI subcommands.
pub fn cmd_profile(action: &ProfileCommands) -> i32 {
    match action {
        ProfileCommands::Save {
            name,
            effect,
            color,
            brightness,
            keys,
        } => {
            let parsed_color = match color {
                Some(c) => match parse_color_hex(c) {
                    Ok(rgb) => Some(rgb),
                    Err(e) => {
                        eprintln!("Error parsing --color: {}", e);
                        return 1;
                    }
                },
                None => None,
            };

            let mut parsed_keys = Vec::new();
            for key_arg in keys {
                let (k, hex) = match key_arg.split_once('=') {
                    Some(pair) => pair,
                    None => {
                        eprintln!(
                            "Error: Invalid --key format {:?}, expected NAME=COLOR (e.g. esc=FF0000)",
                            key_arg
                        );
                        return 1;
                    }
                };
                let rgb = match parse_color_hex(hex) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("Error parsing color in --key {:?}: {}", key_arg, e);
                        return 1;
                    }
                };
                parsed_keys.push((k.trim().to_string(), rgb));
            }

            let prof = profile::Profile::new(
                name.clone(),
                effect.clone(),
                parsed_color,
                *brightness,
                parsed_keys,
            );

            match profile::save_profile(&prof, None) {
                Ok(path) => {
                    println!("Saved profile '{}' to {}", prof.name, path.display());
                    0
                }
                Err(e) => {
                    eprintln!("Error saving profile: {}", e);
                    1
                }
            }
        }
        ProfileCommands::Load { name, seconds } => {
            let prof = match profile::load_profile(name, None) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Error loading profile: {}", e);
                    return 1;
                }
            };

            let runner_res = prof.apply_with_runner(*seconds, |params| match params {
                profile::AppliedProfileParams::Effect {
                    effect,
                    brightness,
                    seconds,
                } => {
                    println!(
                        "Applying profile '{}' (effect: {:?}, brightness: {:.2})...",
                        prof.name, effect, brightness
                    );
                    let exit = run_effect_lighting(effect, *brightness, *seconds);
                    if exit == 0 {
                        Ok(())
                    } else {
                        Err(format!("Device communication exited with code {}", exit))
                    }
                }
                profile::AppliedProfileParams::Keys { pairs, seconds } => {
                    println!(
                        "Applying profile '{}' ({} per-key overrides)...",
                        prof.name,
                        pairs.len()
                    );
                    let borrowed: Vec<(&str, (u8, u8, u8))> =
                        pairs.iter().map(|(k, c)| (k.as_str(), *c)).collect();
                    let exit = run_keys_lighting(&borrowed, *seconds);
                    if exit == 0 {
                        Ok(())
                    } else {
                        Err(format!("Device communication exited with code {}", exit))
                    }
                }
            });

            match runner_res {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("Error applying profile: {}", e);
                    1
                }
            }
        }
        ProfileCommands::List => {
            match profile::list_profiles(None) {
                Ok(list) => {
                    if list.is_empty() {
                        println!("No profiles found.");
                    } else {
                        println!("Saved profiles ({}):", list.len());
                        for name in list {
                            println!("  {}", name);
                        }
                    }
                    0
                }
                Err(e) => {
                    eprintln!("Error listing profiles: {}", e);
                    1
                }
            }
        }
        ProfileCommands::Delete { name } => {
            match profile::delete_profile(name, None) {
                Ok(()) => {
                    println!("Profile '{}' deleted.", name);
                    0
                }
                Err(e) => {
                    eprintln!("Error deleting profile: {}", e);
                    1
                }
            }
        }
        ProfileCommands::Show { name } => {
            match profile::load_profile(name, None) {
                Ok(prof) => {
                    println!("Profile: {}", prof.name);
                    println!("  Effect: {}", prof.effect);
                    if let Some(c) = prof.color {
                        println!("  Color: #{:02X}{:02X}{:02X}", c.0, c.1, c.2);
                    }
                    println!("  Brightness: {:.2}", prof.brightness);
                    if !prof.keys.is_empty() {
                        println!("  Key Overrides ({}):", prof.keys.len());
                        for (k, c) in &prof.keys {
                            println!("    {:<12} = #{:02X}{:02X}{:02X}", k, c.0, c.1, c.2);
                        }
                    }
                    0
                }
                Err(e) => {
                    eprintln!("Error showing profile: {}", e);
                    1
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_color_hex_valid() {
        assert_eq!(parse_color_hex("ff0000").unwrap(), (255, 0, 0));
        assert_eq!(parse_color_hex("#00ff00").unwrap(), (0, 255, 0));
        assert_eq!(parse_color_hex("#0000FF").unwrap(), (0, 0, 255));
        assert_eq!(parse_color_hex("1A2B3C").unwrap(), (0x1A, 0x2B, 0x3C));
        assert_eq!(parse_color_hex(" #123456 ").unwrap(), (0x12, 0x34, 0x56));
    }

    #[test]
    fn test_parse_color_hex_invalid() {
        assert!(parse_color_hex("fff").is_err());
        assert!(parse_color_hex("12345").is_err());
        assert!(parse_color_hex("1234567").is_err());
        assert!(parse_color_hex("12345g").is_err());
        assert!(parse_color_hex("#").is_err());
        assert!(parse_color_hex("").is_err());
    }

    #[test]
    fn test_parse_hex_or_dec() {
        assert_eq!(parse_hex_or_dec("0x258A").unwrap(), 0x258A);
        assert_eq!(parse_hex_or_dec("0X010C").unwrap(), 0x010C);
        assert_eq!(parse_hex_or_dec("258a").unwrap(), 0x258A);
        assert_eq!(parse_hex_or_dec("9610").unwrap(), 9610);
        assert!(parse_hex_or_dec("zzzz").is_err());
    }

    #[test]
    fn test_cmd_keys() {
        assert_eq!(cmd_keys(), 0);
    }

    #[test]
    fn test_cli_subcommand_key_parse() {
        let parsed = Cli::try_parse_from(["aula-f75-rs", "key", "esc", "ff0000", "--seconds", "3"]);
        assert!(parsed.is_ok());
        if let Commands::Key { name, color, seconds } = parsed.unwrap().command {
            assert_eq!(name, "esc");
            assert_eq!(color, "ff0000");
            assert_eq!(seconds, Some(3.0));
        } else {
            panic!("Expected Commands::Key");
        }
    }

    #[test]
    fn test_cli_subcommand_keys_parse() {
        let parsed = Cli::try_parse_from(["aula-f75-rs", "keys"]);
        assert!(parsed.is_ok());
        assert!(matches!(parsed.unwrap().command, Commands::Keys));
    }

    #[test]
    fn test_cli_subcommand_fx_parse() {
        let parsed = Cli::try_parse_from([
            "aula-f75-rs",
            "fx",
            "rainbow",
            "--brightness",
            "0.8",
            "--seconds",
            "10",
        ]);
        assert!(parsed.is_ok());
        if let Commands::Fx {
            name,
            color,
            brightness,
            seconds,
        } = parsed.unwrap().command
        {
            assert_eq!(name, "rainbow");
            assert_eq!(color, None);
            assert_eq!(brightness, 0.8);
            assert_eq!(seconds, Some(10.0));
        } else {
            panic!("Expected Commands::Fx");
        }

        let parsed_breathe = Cli::try_parse_from([
            "aula-f75-rs",
            "fx",
            "breathe",
            "--color",
            "ff00ff",
        ]);
        assert!(parsed_breathe.is_ok());
        if let Commands::Fx {
            name,
            color,
            brightness,
            seconds,
        } = parsed_breathe.unwrap().command
        {
            assert_eq!(name, "breathe");
            assert_eq!(color, Some("ff00ff".to_string()));
            assert_eq!(brightness, 1.0);
            assert_eq!(seconds, None);
        } else {
            panic!("Expected Commands::Fx");
        }
    }

    #[test]
    fn test_cli_subcommand_effects_parse() {
        let parsed = Cli::try_parse_from(["aula-f75-rs", "effects"]);
        assert!(parsed.is_ok());
        assert!(matches!(parsed.unwrap().command, Commands::Effects));
        assert_eq!(cmd_effects(), 0);
    }

    #[test]
    fn test_cli_subcommand_profile_parse() {
        // Save with effect, color, brightness, keys
        let parsed_save = Cli::try_parse_from([
            "aula-f75-rs",
            "profile",
            "save",
            "gaming",
            "--effect",
            "rainbow",
            "--color",
            "FF0000",
            "--brightness",
            "0.75",
            "--key",
            "esc=00FF00",
            "--key",
            "space=0000FF",
        ]);
        assert!(parsed_save.is_ok());
        match parsed_save.unwrap().command {
            Commands::Profile {
                action:
                    ProfileCommands::Save {
                        name,
                        effect,
                        color,
                        brightness,
                        keys,
                    },
            } => {
                assert_eq!(name, "gaming");
                assert_eq!(effect, "rainbow");
                assert_eq!(color, Some("FF0000".to_string()));
                assert_eq!(brightness, 0.75);
                assert_eq!(keys, vec!["esc=00FF00", "space=0000FF"]);
            }
            _ => panic!("Expected Commands::Profile::Save"),
        }

        // Load
        let parsed_load = Cli::try_parse_from([
            "aula-f75-rs",
            "profile",
            "load",
            "gaming",
            "--seconds",
            "5.0",
        ]);
        assert!(parsed_load.is_ok());
        match parsed_load.unwrap().command {
            Commands::Profile {
                action: ProfileCommands::Load { name, seconds },
            } => {
                assert_eq!(name, "gaming");
                assert_eq!(seconds, Some(5.0));
            }
            _ => panic!("Expected Commands::Profile::Load"),
        }

        // List
        let parsed_list = Cli::try_parse_from(["aula-f75-rs", "profile", "list"]);
        assert!(parsed_list.is_ok());
        assert!(matches!(
            parsed_list.unwrap().command,
            Commands::Profile {
                action: ProfileCommands::List
            }
        ));

        // Delete
        let parsed_delete = Cli::try_parse_from(["aula-f75-rs", "profile", "delete", "gaming"]);
        assert!(parsed_delete.is_ok());
        match parsed_delete.unwrap().command {
            Commands::Profile {
                action: ProfileCommands::Delete { name },
            } => {
                assert_eq!(name, "gaming");
            }
            _ => panic!("Expected Commands::Profile::Delete"),
        }

        // Show
        let parsed_show = Cli::try_parse_from(["aula-f75-rs", "profile", "show", "gaming"]);
        assert!(parsed_show.is_ok());
        match parsed_show.unwrap().command {
            Commands::Profile {
                action: ProfileCommands::Show { name },
            } => {
                assert_eq!(name, "gaming");
            }
            _ => panic!("Expected Commands::Profile::Show"),
        }
    }

    #[test]
    fn test_cli_subcommand_music_parse() {
        // Default music command
        let parsed_default = Cli::try_parse_from(["aula-f75-rs", "music"]);
        assert!(parsed_default.is_ok());
        match parsed_default.unwrap().command {
            Commands::Music {
                mode,
                color,
                gain,
                seconds,
            } => {
                assert_eq!(mode, "bars");
                assert_eq!(color, "00ffff");
                assert_eq!(gain, 1.0);
                assert_eq!(seconds, None);
            }
            _ => panic!("Expected Commands::Music"),
        }

        // Custom parameters
        let parsed_custom = Cli::try_parse_from([
            "aula-f75-rs",
            "music",
            "--mode",
            "pulse",
            "--color",
            "ff0055",
            "--gain",
            "2.5",
            "--seconds",
            "10.0",
        ]);
        assert!(parsed_custom.is_ok());
        match parsed_custom.unwrap().command {
            Commands::Music {
                mode,
                color,
                gain,
                seconds,
            } => {
                assert_eq!(mode, "pulse");
                assert_eq!(color, "ff0055");
                assert_eq!(gain, 2.5);
                assert_eq!(seconds, Some(10.0));
            }
            _ => panic!("Expected Commands::Music"),
        }
    }

    #[test]
    fn test_cli_subcommand_battery_parse() {
        let parsed = Cli::try_parse_from(["aula-f75-rs", "battery"]);
        assert!(parsed.is_ok());
        assert!(matches!(parsed.unwrap().command, Commands::Battery));
    }

    #[test]
    fn test_cli_subcommand_wireless_parse() {
        let parsed = Cli::try_parse_from(["aula-f75-rs", "wireless"]);
        assert!(parsed.is_ok());
        assert!(matches!(parsed.unwrap().command, Commands::Wireless));
    }

    #[test]
    fn test_cli_subcommand_knob_parse() {
        let parsed = Cli::try_parse_from(["aula-f75-rs", "knob"]);
        assert!(parsed.is_ok());
        assert!(matches!(parsed.unwrap().command, Commands::Knob));
    }
}

