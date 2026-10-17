//! Command-line interface definitions and command handlers for AULA F75.

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{Parser, Subcommand};

use crate::device::{self, DeviceInfo};
use crate::proto;

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

/// Run per-key RGB lighting loop on the hardware interface 1 control node.
pub fn run_key_lighting(name: &str, rgb: (u8, u8, u8), seconds: Option<f64>) -> i32 {
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
        &[(name, rgb)],
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
}

