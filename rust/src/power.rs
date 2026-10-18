//! Battery status discovery and sysfs power supply inspection for AULA F75.

use std::fs;
use std::io;
use std::path::Path;

/// Normalized battery status reported by the device or host operating system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatteryStatus {
    /// Whether a battery is detected and present.
    pub present: bool,
    /// Charge percentage (0..=100) if exposed.
    pub percent: Option<u8>,
    /// Charging state (true = charging, false = discharging/full/not charging) if exposed.
    pub charging: Option<bool>,
    /// Raw telemetry bytes returned by device or sysfs.
    pub raw: Vec<u8>,
}

impl BatteryStatus {
    /// Return an empty battery status indicating no battery reported.
    pub fn empty() -> Self {
        Self {
            present: false,
            percent: None,
            charging: None,
            raw: Vec::new(),
        }
    }
}

/// Read battery attributes from a specific sysfs power supply directory.
///
/// Inspects standard Linux `power_supply` class attributes:
/// - `capacity`: charge percentage (0..=100)
/// - `status`: charging state string ("Charging", "Discharging", "Full", etc.)
/// - `present`: "1" or "0"
pub fn read_sysfs_battery(power_supply_dir: &Path) -> BatteryStatus {
    if !power_supply_dir.is_dir() {
        return BatteryStatus::empty();
    }

    let mut percent: Option<u8> = None;
    let mut charging: Option<bool> = None;
    let mut present = false;
    let mut raw = Vec::new();

    // Check capacity
    let cap_file = power_supply_dir.join("capacity");
    if let Ok(s) = fs::read_to_string(&cap_file) {
        raw.extend_from_slice(s.as_bytes());
        if let Ok(num) = s.trim().parse::<u16>() {
            percent = Some(num.min(100) as u8);
            present = true;
        }
    }

    // Check status
    let status_file = power_supply_dir.join("status");
    if let Ok(s) = fs::read_to_string(&status_file) {
        raw.extend_from_slice(s.as_bytes());
        let lower = s.trim().to_ascii_lowercase();
        if lower.contains("charging") && !lower.contains("not charging") && !lower.contains("discharging") {
            charging = Some(true);
        } else if lower.contains("discharging") || lower.contains("not charging") || lower.contains("full") {
            charging = Some(false);
        }
    }

    // Check present
    let present_file = power_supply_dir.join("present");
    if let Ok(s) = fs::read_to_string(&present_file) {
        if s.trim() == "1" {
            present = true;
        }
    }

    BatteryStatus {
        present,
        percent,
        charging,
        raw,
    }
}

/// Query sysfs power supply entries with an injectable root path.
///
/// Checks subdirectories in `power_supply_root` (typically `/sys/class/power_supply`).
/// If `device_filter` is provided, prioritizes entries whose name contains the filter.
pub fn query_sysfs_battery_for_device(
    power_supply_root: &Path,
    device_filter: Option<&str>,
) -> BatteryStatus {
    if !power_supply_root.is_dir() {
        return BatteryStatus::empty();
    }

    // Direct power_supply entry check (if root itself has capacity/status files)
    if power_supply_root.join("capacity").is_file() {
        if let Some(filter) = device_filter {
            let f_lower = filter.to_ascii_lowercase();
            let name = power_supply_root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            if !name.contains(&f_lower) {
                return BatteryStatus::empty();
            }
        }
        return read_sysfs_battery(power_supply_root);
    }

    let read_dir = match fs::read_dir(power_supply_root) {
        Ok(r) => r,
        Err(_) => return BatteryStatus::empty(),
    };

    let mut candidates = Vec::new();
    for entry in read_dir.flatten() {
        candidates.push(entry.path());
    }

    // When filter is supplied, skip non-matching entries entirely
    if let Some(filter) = device_filter {
        let f_lower = filter.to_ascii_lowercase();
        candidates.retain(|p| {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            name.contains(&f_lower)
        });
    }

    for path in candidates {
        let status = read_sysfs_battery(&path);
        if status.present || status.percent.is_some() {
            return status;
        }
    }

    BatteryStatus::empty()
}

/// Parse a raw HID GET_FEATURE response payload for battery status.
///
/// In wired mode (PID 010c), the AULA F75 firmware does not expose a standard battery feature report.
/// If `buf` is empty, all-zero, or unparseable, returns `BatteryStatus::empty()`.
/// If a valid battery report payload is provided, extracts the percentage and charging status.
/// No heuristic percentages are invented from unrelated fields.
pub fn parse_battery_feature_report(buf: &[u8]) -> BatteryStatus {
    if buf.is_empty() || buf.iter().all(|&b| b == 0) {
        return BatteryStatus {
            present: false,
            percent: None,
            charging: None,
            raw: buf.to_vec(),
        };
    }

    // Known battery report payload candidates:
    // Report ID in buf[0], battery percentage (1..=100) in buf[1], charging indicator in buf[2].
    if buf.len() >= 2 && buf[1] >= 1 && buf[1] <= 100 {
        let percent = Some(buf[1]);
        let charging = if buf.len() >= 3 {
            match buf[2] {
                1 => Some(true),
                0 => Some(false),
                _ => None,
            }
        } else {
            None
        };
        return BatteryStatus {
            present: true,
            percent,
            charging,
            raw: buf.to_vec(),
        };
    }

    BatteryStatus {
        present: false,
        percent: None,
        charging: None,
        raw: buf.to_vec(),
    }
}

/// Query battery status using both sysfs and an injectable HID feature report read function.
pub fn query_battery_with_sources<F>(
    power_supply_root: &Path,
    device_filter: Option<&str>,
    mut read_feature_fn: F,
) -> BatteryStatus
where
    F: FnMut(&mut [u8]) -> Result<usize, io::Error>,
{
    let sysfs_status = query_sysfs_battery_for_device(power_supply_root, device_filter);
    if sysfs_status.present && sysfs_status.percent.is_some() {
        return sysfs_status;
    }

    let mut buf = [0u8; 64];
    if let Ok(len) = read_feature_fn(&mut buf) {
        if len > 0 {
            let hid_status = parse_battery_feature_report(&buf[..len]);
            if hid_status.present && hid_status.percent.is_some() {
                return hid_status;
            }
        }
    }

    BatteryStatus::empty()
}
