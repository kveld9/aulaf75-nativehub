//! Device discovery and sysfs resolution for AULA F75 keyboard.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Known Vendor ID for AULA F75 (0x258A).
pub const KNOWN_VID: u16 = 0x258A;

/// Known Product ID for AULA F75 (0x010C).
pub const KNOWN_PID: u16 = 0x010C;

/// Default Linux sysfs mount path for hidraw class.
pub const DEFAULT_SYSFS_HIDRAW: &str = "/sys/class/hidraw";

/// Default Linux sysfs mount path for USB bus devices.
pub const DEFAULT_SYSFS_USB_DEVICES: &str = "/sys/bus/usb/devices";

#[path = "power.rs"]
pub mod power;

/// Normalized metadata for an enumerated HID device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub path: PathBuf,
    pub interface_number: Option<i32>,
    pub manufacturer_string: String,
    pub product_string: String,
    pub release_number: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub is_known: bool,
    pub device_name: String,
    pub usb_parent: String,
    pub phys: String,
}

/// Resolve the `/dev/hidrawN` path for USB interface 1 via sysfs without hidapi.
///
/// Uses default Linux sysfs mount paths: `/sys/class/hidraw` and `/sys/bus/usb/devices`.
pub fn resolve_control_node() -> Option<PathBuf> {
    resolve_control_node_with_roots(
        Path::new(DEFAULT_SYSFS_HIDRAW),
        Path::new(DEFAULT_SYSFS_USB_DEVICES),
        KNOWN_VID,
        KNOWN_PID,
    )
}

/// Resolve the control node with injectable sysfs base directories and target VID/PID.
///
/// Hardware predicate:
/// 1. Scans `sys_class_hidraw` entries numerically (`hidraw0`, `hidraw1`, ...).
/// 2. Resolves real path of `<entry>/device`.
/// 3. Selects the component with exactly one colon whose post-colon part starts with "1."
///    and ends with ".1" (e.g. `5-2:1.1`, matching USB configuration 1, interface 1).
///    This strictly excludes PCI components (multiple colons: `0000:00:08.1`),
///    HID child components (`0003:258A:010C.0008`), and interface 0 (`5-2:1.0`).
/// 4. Confirms parent USB device `idVendor` and `idProduct` equal expected `vid` and `pid`.
pub fn resolve_control_node_with_roots(
    sys_class_hidraw: &Path,
    usb_devices: &Path,
    vid: u16,
    pid: u16,
) -> Option<PathBuf> {
    if !sys_class_hidraw.is_dir() {
        return None;
    }

    let read_dir = fs::read_dir(sys_class_hidraw).ok()?;
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in read_dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("hidraw") {
            entries.push(entry.path());
        }
    }

    // Sort numerically by hidraw index (hidraw0, hidraw1, ...)
    entries.sort_by_key(|p| {
        let s = p.file_name().unwrap_or_default().to_string_lossy();
        s.strip_prefix("hidraw")
            .and_then(|num| num.parse::<u32>().ok())
            .unwrap_or(u32::MAX)
    });

    for entry in entries {
        let dev_link = entry.join("device");
        if !dev_link.exists() {
            continue;
        }

        let real_path = match fs::canonicalize(&dev_link) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let parts: Vec<String> = real_path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();

        // Identify interface component with exactly one colon, starting with "1." and ending with ".1"
        let mut iface_idx = None;
        for (idx, comp) in parts.iter().enumerate() {
            if comp.matches(':').count() == 1 {
                if let Some((_, after_colon)) = comp.split_once(':') {
                    if after_colon.starts_with("1.") && after_colon.ends_with(".1") {
                        iface_idx = Some(idx);
                        break;
                    }
                }
            }
        }

        let iface_idx = match iface_idx {
            Some(i) => i,
            None => continue,
        };

        let iface_comp = &parts[iface_idx];
        let usb_dev_name = iface_comp.split(':').next().unwrap_or("");

        let mut candidate_dirs: Vec<PathBuf> = Vec::new();

        // 1. Immediate parent directory of interface component in real_path
        let parent_dir: PathBuf = real_path.components().take(iface_idx).collect();
        candidate_dirs.push(parent_dir);

        // 2. usb_devices symlink/directory for the device
        if usb_devices.is_dir() {
            candidate_dirs.push(usb_devices.join(usb_dev_name));
        }

        // 3. Ancestors in real_path hierarchy
        let mut curr = real_path.parent();
        for _ in 0..4 {
            if let Some(p) = curr {
                let pb = p.to_path_buf();
                if !candidate_dirs.contains(&pb) {
                    candidate_dirs.push(pb);
                }
                curr = p.parent();
            } else {
                break;
            }
        }

        let mut matched = false;
        for candidate in candidate_dirs {
            let vendor_file = candidate.join("idVendor");
            let product_file = candidate.join("idProduct");
            if vendor_file.is_file() && product_file.is_file() {
                if let (Ok(v_str), Ok(p_str)) = (
                    fs::read_to_string(&vendor_file),
                    fs::read_to_string(&product_file),
                ) {
                    if let (Ok(read_vid), Ok(read_pid)) = (
                        u16::from_str_radix(v_str.trim(), 16),
                        u16::from_str_radix(p_str.trim(), 16),
                    ) {
                        if read_vid == vid && read_pid == pid {
                            matched = true;
                            break;
                        }
                    }
                }
            }
        }

        if matched {
            let entry_name = entry.file_name().unwrap_or_default().to_string_lossy();
            return Some(PathBuf::from(format!("/dev/{}", entry_name)));
        }
    }

    None
}

/// Enumerate HID device nodes from sysfs and parse device attributes.
pub fn enumerate_devices(sys_class_hidraw: &Path) -> Vec<DeviceInfo> {
    let mut results = Vec::new();
    if !sys_class_hidraw.is_dir() {
        return results;
    }

    let read_dir = match fs::read_dir(sys_class_hidraw) {
        Ok(r) => r,
        Err(_) => return results,
    };

    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in read_dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("hidraw") {
            entries.push(entry.path());
        }
    }

    entries.sort_by_key(|p| {
        let s = p.file_name().unwrap_or_default().to_string_lossy();
        s.strip_prefix("hidraw")
            .and_then(|num| num.parse::<u32>().ok())
            .unwrap_or(u32::MAX)
    });

    for entry in entries {
        let entry_name = entry.file_name().unwrap_or_default().to_string_lossy();
        let dev_path = PathBuf::from(format!("/dev/{}", entry_name));

        let mut vendor_id: u16 = 0;
        let mut product_id: u16 = 0;
        let mut product_string = String::new();
        let mut manufacturer_string = String::new();
        let mut interface_number: Option<i32> = None;
        let mut release_number: u16 = 0;
        let mut usb_parent = String::new();
        let mut phys = String::new();

        let dev_dir = entry.join("device");
        let real_path = fs::canonicalize(&dev_dir).ok();

        // 1. Try reading uevent from device directory
        let uevent_file = dev_dir.join("uevent");
        if let Ok(content) = fs::read_to_string(&uevent_file) {
            for line in content.lines() {
                if let Some(val) = line.strip_prefix("HID_ID=") {
                    // Format: BUS:VENDOR:PRODUCT (e.g. 0003:0000258A:0000010C)
                    let sub: Vec<&str> = val.split(':').collect();
                    if sub.len() >= 3 {
                        if let Ok(v) = u16::from_str_radix(sub[1], 16) {
                            vendor_id = v;
                        }
                        if let Ok(p) = u16::from_str_radix(sub[2], 16) {
                            product_id = p;
                        }
                    }
                } else if let Some(val) = line.strip_prefix("HID_NAME=") {
                    product_string = val.trim().to_string();
                } else if let Some(val) = line.strip_prefix("HID_PHYS=") {
                    phys = val.trim().to_string();
                }
            }
        }

        if phys.is_empty() {
            if let Ok(s) = fs::read_to_string(dev_dir.join("phys")) {
                phys = s.trim().to_string();
            }
        }

        // 2. Discover interface number from bInterfaceNumber file or path
        if let Some(ref rp) = real_path {
            let parts: Vec<String> = rp
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            let mut iface_idx = None;
            for (idx, comp) in parts.iter().enumerate() {
                if comp.matches(':').count() == 1 {
                    if let Some((_, after_colon)) = comp.split_once(':') {
                        if after_colon.starts_with("1.") && after_colon.contains('.') {
                            iface_idx = Some(idx);
                        }
                    }
                }
            }
            if let Some(idx) = iface_idx {
                if idx > 0 {
                    usb_parent = parts[idx - 1].clone();
                } else if let Some((before_colon, _)) = parts[idx].split_once(':') {
                    usb_parent = before_colon.to_string();
                }
            }

            if let Some(parent) = rp.parent() {
                let iface_file = parent.join("bInterfaceNumber");
                if let Ok(s) = fs::read_to_string(&iface_file) {
                    if let Ok(num) = s.trim().parse::<i32>() {
                        interface_number = Some(num);
                    }
                }
                if usb_parent.is_empty() {
                    if let Some(gp) = parent.parent() {
                        usb_parent = gp.file_name().unwrap_or_default().to_string_lossy().to_string();
                    }
                }
            }

            // Fallback: check path components for pattern <bus>:<config>.<interface>
            if interface_number.is_none() {
                for comp in rp.components() {
                    let s = comp.as_os_str().to_string_lossy();
                    if s.matches(':').count() == 1 {
                        if let Some((_, after)) = s.split_once(':') {
                            if let Some((_, iface_str)) = after.split_once('.') {
                                if let Ok(num) = iface_str.parse::<i32>() {
                                    interface_number = Some(num);
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            // 3. Discover manufacturer, product, release from USB ancestors
            let mut curr = rp.parent();
            for _ in 0..4 {
                if let Some(p) = curr {
                    if manufacturer_string.is_empty() {
                        let mfg_file = p.join("manufacturer");
                        if let Ok(s) = fs::read_to_string(&mfg_file) {
                            manufacturer_string = s.trim().to_string();
                        }
                    }
                    if product_string.is_empty() {
                        let prod_file = p.join("product");
                        if let Ok(s) = fs::read_to_string(&prod_file) {
                            product_string = s.trim().to_string();
                        }
                    }
                    if release_number == 0 {
                        let bcd_file = p.join("bcdDevice");
                        if let Ok(s) = fs::read_to_string(&bcd_file) {
                            if let Ok(r) = u16::from_str_radix(s.trim(), 16) {
                                release_number = r;
                            }
                        }
                    }
                    if vendor_id == 0 {
                        let v_file = p.join("idVendor");
                        if let Ok(s) = fs::read_to_string(&v_file) {
                            if let Ok(v) = u16::from_str_radix(s.trim(), 16) {
                                vendor_id = v;
                            }
                        }
                    }
                    if product_id == 0 {
                        let p_file = p.join("idProduct");
                        if let Ok(s) = fs::read_to_string(&p_file) {
                            if let Ok(p_val) = u16::from_str_radix(s.trim(), 16) {
                                product_id = p_val;
                            }
                        }
                    }
                    curr = p.parent();
                } else {
                    break;
                }
            }
        }

        let is_known = vendor_id == KNOWN_VID && product_id == KNOWN_PID;
        let device_name = if is_known {
            "AULA F75".to_string()
        } else {
            "Unknown".to_string()
        };

        results.push(DeviceInfo {
            vendor_id,
            product_id,
            path: dev_path,
            interface_number,
            manufacturer_string,
            product_string,
            release_number,
            usage_page: 0,
            usage: 0,
            is_known,
            device_name,
            usb_parent,
            phys,
        });
    }

    results
}

/// Classification of HID device connection topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceConnectionKind {
    /// Direct USB cable connection on interface 1 (control node).
    CableInterface1Control,
    /// Direct USB cable connection on interface 0 (boot keyboard).
    CableInterface0Keyboard,
    /// 2.4GHz wireless USB dongle receiver node.
    WirelessDongle,
    /// Matching AULA F75 device node.
    CableCandidate,
    /// Unknown or non-matching HID device.
    Undetermined,
}

impl DeviceInfo {
    /// Classify whether the device node corresponds to a cable connection or 2.4GHz wireless dongle.
    pub fn classify_connection(&self) -> DeviceConnectionKind {
        let prod_lower = self.product_string.to_ascii_lowercase();
        let mfg_lower = self.manufacturer_string.to_ascii_lowercase();
        let phys_lower = self.phys.to_ascii_lowercase();

        let is_dongle_hint = prod_lower.contains("2.4g")
            || prod_lower.contains("dongle")
            || prod_lower.contains("receiver")
            || prod_lower.contains("wireless")
            || mfg_lower.contains("wireless")
            || phys_lower.contains("wireless");

        if is_dongle_hint {
            return DeviceConnectionKind::WirelessDongle;
        }

        match self.interface_number {
            Some(1) => DeviceConnectionKind::CableInterface1Control,
            Some(0) => DeviceConnectionKind::CableInterface0Keyboard,
            _ => {
                if self.is_known {
                    DeviceConnectionKind::CableCandidate
                } else {
                    DeviceConnectionKind::Undetermined
                }
            }
        }
    }
}

/// Candidate HID node for wireless dongle or cable connection inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WirelessNodeCandidate {
    pub path: PathBuf,
    pub interface_number: Option<i32>,
    pub usb_parent: String,
    pub phys: String,
    pub connection_kind: DeviceConnectionKind,
    pub is_dongle: bool,
    pub detail: String,
}

/// Enumerate AULA F75 HID device nodes and classify each node as cable interface 1 vs wireless dongle.
pub fn enumerate_wireless_nodes_with_roots(sys_class_hidraw: &Path) -> Vec<WirelessNodeCandidate> {
    let devices = enumerate_devices(sys_class_hidraw);
    let mut candidates = Vec::new();

    for dev in devices.into_iter().filter(|d| d.is_known) {
        let kind = dev.classify_connection();
        let is_dongle = kind == DeviceConnectionKind::WirelessDongle;
        let detail = match kind {
            DeviceConnectionKind::CableInterface1Control => {
                "Direct USB cable connection (Interface 1 control node)".to_string()
            }
            DeviceConnectionKind::CableInterface0Keyboard => {
                "Direct USB cable connection (Interface 0 boot keyboard)".to_string()
            }
            DeviceConnectionKind::WirelessDongle => {
                "2.4GHz USB wireless dongle receiver node".to_string()
            }
            DeviceConnectionKind::CableCandidate => {
                "Matching AULA F75 node on USB parent".to_string()
            }
            DeviceConnectionKind::Undetermined => {
                "Undetermined USB HID node".to_string()
            }
        };

        candidates.push(WirelessNodeCandidate {
            path: dev.path,
            interface_number: dev.interface_number,
            usb_parent: dev.usb_parent,
            phys: dev.phys,
            connection_kind: kind,
            is_dongle,
            detail,
        });
    }

    candidates
}

/// Inspect report descriptors of enumerated AULA F75 devices to determine observed knob behavior.
pub fn query_knob_mode_with_roots(sys_class_hidraw: &Path) -> crate::proto::KnobReport {
    if !sys_class_hidraw.is_dir() {
        return crate::proto::KnobReport {
            source: crate::proto::KnobModeSource::Undetermined,
            description: "undetermined, needs live USB capture".to_string(),
            details: vec!["Sysfs hidraw directory not found".to_string()],
        };
    }

    let devs = enumerate_devices(sys_class_hidraw);
    for dev in devs.iter().filter(|d| d.is_known) {
        let entry_name = dev.path.file_name().unwrap_or_default().to_string_lossy();
        let desc_path = sys_class_hidraw.join(entry_name.as_ref()).join("device/report_descriptor");
        if let Ok(bytes) = fs::read(&desc_path) {
            let report = crate::proto::parse_knob_from_report_descriptor(&bytes);
            if report.source != crate::proto::KnobModeSource::Undetermined {
                return report;
            }
        }
    }

    crate::proto::KnobReport {
        source: crate::proto::KnobModeSource::Undetermined,
        description: "undetermined, needs live USB capture".to_string(),
        details: vec![
            "No rotary encoder usage detected in enumerated HID report descriptors".to_string(),
            "AULA F75 rotary knob switches between Volume and Backlight modes via onboard firmware without host report remapping".to_string(),
        ],
    }
}

/// Send with retry and backoff on transient errors (EPIPE, ETIMEDOUT).
///
/// Retries transient bursts up to `max_retries` consecutive failures.
/// Fatal errors (such as EINVAL) fail immediately.
pub fn send_with_retry<F, S>(
    mut send_fn: F,
    max_retries: usize,
    mut sleep_fn: S,
) -> Result<(), io::Error>
where
    F: FnMut() -> Result<(), io::Error>,
    S: FnMut(),
{
    let mut consecutive_failures = 0;
    loop {
        match send_fn() {
            Ok(()) => return Ok(()),
            Err(e) => {
                let code = e.raw_os_error();
                if code == Some(libc::EPIPE) || code == Some(libc::ETIMEDOUT) {
                    consecutive_failures += 1;
                    if consecutive_failures >= max_retries {
                        return Err(e);
                    }
                    sleep_fn();
                } else {
                    return Err(e);
                }
            }
        }
    }
}

/// Run direct RGB frame keepalive loop until `seconds` expires or `is_running_fn` returns false.
pub fn run_keepalive_loop<F, S, R>(
    rgb: (u8, u8, u8),
    seconds: Option<f64>,
    mut send_fn: F,
    mut sleep_fn: S,
    mut is_running_fn: R,
) -> Result<(), io::Error>
where
    F: FnMut(&[u8; crate::proto::FRAME_LEN]) -> Result<(), io::Error>,
    S: FnMut(Duration),
    R: FnMut() -> bool,
{
    let frame = crate::proto::build_direct_frame(rgb)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let interval = Duration::from_secs_f64(1.0 / crate::proto::KEEPALIVE_HZ);
    let start_time = Instant::now();
    let mut count: u64 = 0;
    let mut consecutive_failures = 0;

    while is_running_fn() {
        match send_fn(&frame) {
            Ok(()) => {
                consecutive_failures = 0;
            }
            Err(e) => {
                let code = e.raw_os_error();
                if code == Some(libc::EPIPE) || code == Some(libc::ETIMEDOUT) {
                    consecutive_failures += 1;
                    if consecutive_failures >= 10 {
                        return Err(e);
                    }
                    sleep_fn(interval);
                    continue;
                } else {
                    return Err(e);
                }
            }
        }

        count += 1;
        if let Some(sec) = seconds {
            if sec > 0.0 && start_time.elapsed().as_secs_f64() >= sec && count >= 2 {
                break;
            }
        }

        sleep_fn(interval);

        if let Some(sec) = seconds {
            if sec > 0.0 && start_time.elapsed().as_secs_f64() >= sec && count >= 2 {
                break;
            }
        }
    }

    Ok(())
}

/// Send a direct RGB frame or run keepalive loop for solid whole-board color.
pub fn send_frame<F, S, R>(
    rgb: (u8, u8, u8),
    seconds: Option<f64>,
    send_fn: F,
    sleep_fn: S,
    is_running_fn: R,
) -> Result<(), io::Error>
where
    F: FnMut(&[u8; crate::proto::FRAME_LEN]) -> Result<(), io::Error>,
    S: FnMut(Duration),
    R: FnMut() -> bool,
{
    run_keepalive_loop(rgb, seconds, send_fn, sleep_fn, is_running_fn)
}

/// Send a key frame or run keepalive loop for specified key color pairs.
///
/// Builds a per-key direct RGB frame lighting only the named keys, and maintains
/// keepalive transmission at `KEEPALIVE_HZ` (~1.5 Hz) with transient error retry.
pub fn send_key_frame<F, S, R>(
    pairs: &[(&str, (u8, u8, u8))],
    seconds: Option<f64>,
    mut send_fn: F,
    mut sleep_fn: S,
    mut is_running_fn: R,
) -> Result<(), io::Error>
where
    F: FnMut(&[u8; crate::proto::FRAME_LEN]) -> Result<(), io::Error>,
    S: FnMut(Duration),
    R: FnMut() -> bool,
{
    let frame = crate::proto::build_key_frame(pairs)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let interval = Duration::from_secs_f64(1.0 / crate::proto::KEEPALIVE_HZ);
    let start_time = Instant::now();
    let mut count: u64 = 0;
    let mut consecutive_failures = 0;

    while is_running_fn() {
        match send_fn(&frame) {
            Ok(()) => {
                consecutive_failures = 0;
            }
            Err(e) => {
                let code = e.raw_os_error();
                if code == Some(libc::EPIPE) || code == Some(libc::ETIMEDOUT) {
                    consecutive_failures += 1;
                    if consecutive_failures >= 10 {
                        return Err(e);
                    }
                    sleep_fn(interval);
                    continue;
                } else {
                    return Err(e);
                }
            }
        }

        count += 1;
        if let Some(sec) = seconds {
            if sec > 0.0 && start_time.elapsed().as_secs_f64() >= sec && count >= 2 {
                break;
            }
        }

        sleep_fn(interval);

        if let Some(sec) = seconds {
            if sec > 0.0 && start_time.elapsed().as_secs_f64() >= sec && count >= 2 {
                break;
            }
        }
    }

    Ok(())
}

/// Run host-rendered RGB effect streaming loop until `seconds` expires or `is_running_fn` returns false.
pub fn run_effect<F, S, R>(
    effect: &crate::effects::Effect,
    brightness: f32,
    seconds: Option<f64>,
    tick_hz: f64,
    mut send_fn: F,
    mut sleep_fn: S,
    mut is_running_fn: R,
) -> Result<(), io::Error>
where
    F: FnMut(&[u8; crate::proto::FRAME_LEN]) -> Result<(), io::Error>,
    S: FnMut(Duration),
    R: FnMut() -> bool,
{
    let key_slots = crate::proto::all_key_slots();
    let interval = Duration::from_secs_f64(1.0 / tick_hz.max(1.0));
    let start_time = Instant::now();
    let mut count: u64 = 0;
    let mut consecutive_failures = 0;

    while is_running_fn() {
        let t = start_time.elapsed().as_secs_f64();
        let pairs = crate::effects::render(effect, t, &key_slots);
        let scaled = crate::effects::apply_brightness(&pairs, brightness);
        let frame = crate::proto::build_slot_frame(&scaled)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        match send_fn(&frame) {
            Ok(()) => {
                consecutive_failures = 0;
            }
            Err(e) => {
                let code = e.raw_os_error();
                if code == Some(libc::EPIPE) || code == Some(libc::ETIMEDOUT) {
                    consecutive_failures += 1;
                    if consecutive_failures >= 10 {
                        return Err(e);
                    }
                    sleep_fn(interval);
                    continue;
                } else {
                    return Err(e);
                }
            }
        }

        count += 1;
        if let Some(sec) = seconds {
            if sec > 0.0 && start_time.elapsed().as_secs_f64() >= sec && count >= 2 {
                break;
            }
        }

        sleep_fn(interval);

        if let Some(sec) = seconds {
            if sec > 0.0 && start_time.elapsed().as_secs_f64() >= sec && count >= 2 {
                break;
            }
        }
    }

    Ok(())
}

/// Send a host-rendered effect frame or run keepalive streaming loop.
pub fn send_effect_frame<F, S, R>(
    effect: &crate::effects::Effect,
    brightness: f32,
    seconds: Option<f64>,
    tick_hz: f64,
    send_fn: F,
    sleep_fn: S,
    is_running_fn: R,
) -> Result<(), io::Error>
where
    F: FnMut(&[u8; crate::proto::FRAME_LEN]) -> Result<(), io::Error>,
    S: FnMut(Duration),
    R: FnMut() -> bool,
{
    run_effect(effect, brightness, seconds, tick_hz, send_fn, sleep_fn, is_running_fn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_with_retry_succeeds_after_transient_failures() {
        let mut attempts = 0;
        let mut sleeps = 0;
        let res = send_with_retry(
            || {
                attempts += 1;
                if attempts <= 2 {
                    Err(io::Error::from_raw_os_error(libc::EPIPE))
                } else {
                    Ok(())
                }
            },
            10,
            || sleeps += 1,
        );

        assert!(res.is_ok());
        assert_eq!(attempts, 3);
        assert_eq!(sleeps, 2);
    }

    #[test]
    fn test_send_with_retry_fails_after_max_retries() {
        let mut attempts = 0;
        let mut sleeps = 0;
        let res = send_with_retry(
            || {
                attempts += 1;
                Err(io::Error::from_raw_os_error(libc::ETIMEDOUT))
            },
            10,
            || sleeps += 1,
        );

        assert!(res.is_err());
        assert_eq!(attempts, 10);
        assert_eq!(sleeps, 9);
        assert_eq!(res.unwrap_err().raw_os_error(), Some(libc::ETIMEDOUT));
    }

    #[test]
    fn test_send_with_retry_immediate_failure_on_non_transient_error() {
        let mut attempts = 0;
        let res = send_with_retry(
            || {
                attempts += 1;
                Err(io::Error::from_raw_os_error(libc::EINVAL))
            },
            10,
            || panic!("Should not sleep on unrecoverable error"),
        );

        assert!(res.is_err());
        assert_eq!(attempts, 1);
        assert_eq!(res.unwrap_err().raw_os_error(), Some(libc::EINVAL));
    }

    #[test]
    fn test_run_keepalive_loop_timed_exit() {
        let mut sent_frames = 0;
        let res = run_keepalive_loop(
            (0xFF, 0x00, 0x00),
            Some(0.001),
            |_| {
                sent_frames += 1;
                Ok(())
            },
            |_| {},
            || true,
        );

        assert!(res.is_ok());
        assert!(sent_frames >= 2);
    }

    #[test]
    fn test_send_key_frame_timed_exit() {
        let mut sent_frames = 0;
        let mut last_key_val = 0;
        let res = send_key_frame(
            &[("esc", (0x42, 0x00, 0x00))],
            Some(0.001),
            |frame| {
                sent_frames += 1;
                last_key_val = frame[8];
                Ok(())
            },
            |_| {},
            || true,
        );

        assert!(res.is_ok());
        assert!(sent_frames >= 2);
        assert_eq!(last_key_val, 0x42);
    }

    #[test]
    fn test_send_key_frame_unknown_key_error() {
        let res = send_key_frame(
            &[("invalid_key_alias", (0xFF, 0x00, 0x00))],
            Some(1.0),
            |_| Ok(()),
            |_| {},
            || true,
        );

        assert!(res.is_err());
        assert_eq!(res.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_run_effect_timed_exit() {
        let effect = crate::effects::Effect::Rainbow { period_s: 5.0 };
        let mut sent_frames = 0;
        let res = run_effect(
            &effect,
            0.8,
            Some(0.001),
            25.0,
            |frame| {
                sent_frames += 1;
                assert_eq!(&frame[0..8], &crate::proto::HEADER);
                Ok(())
            },
            |_| {},
            || true,
        );

        assert!(res.is_ok());
        assert!(sent_frames >= 2);
    }
}

