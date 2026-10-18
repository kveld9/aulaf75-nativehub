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
fn get_sorted_hidraw_entries(sys_class_hidraw: &Path) -> Option<Vec<PathBuf>> {
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
    entries.sort_by_key(|p| {
        let s = p.file_name().unwrap_or_default().to_string_lossy();
        s.strip_prefix("hidraw")
            .and_then(|num| num.parse::<u32>().ok())
            .unwrap_or(u32::MAX)
    });
    Some(entries)
}

fn is_control_interface_comp(comp: &str) -> bool {
    if comp.matches(':').count() != 1 {
        return false;
    }
    match comp.split_once(':') {
        Some((_, after)) => after.starts_with("1.") && after.ends_with(".1"),
        None => false,
    }
}

fn find_control_iface_index(parts: &[String]) -> Option<usize> {
    parts.iter().position(|comp| is_control_interface_comp(comp))
}

fn collect_candidate_usb_dirs(
    real_path: &Path,
    parts: &[String],
    iface_idx: usize,
    usb_devices: &Path,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let parent_dir: PathBuf = real_path.components().take(iface_idx).collect();
    candidates.push(parent_dir);

    if usb_devices.is_dir() {
        let usb_dev_name = parts[iface_idx].split(':').next().unwrap_or("");
        candidates.push(usb_devices.join(usb_dev_name));
    }

    let mut curr = real_path.parent();
    for _ in 0..4 {
        let p = match curr {
            Some(p) => p,
            None => break,
        };
        let pb = p.to_path_buf();
        if !candidates.contains(&pb) {
            candidates.push(pb);
        }
        curr = p.parent();
    }
    candidates
}

fn read_sysfs_string(dir: &Path, file_name: &str) -> Option<String> {
    let s = fs::read_to_string(dir.join(file_name)).ok()?;
    Some(s.trim().to_string())
}

fn read_sysfs_hex_u16(dir: &Path, file_name: &str) -> Option<u16> {
    let s = fs::read_to_string(dir.join(file_name)).ok()?;
    u16::from_str_radix(s.trim(), 16).ok()
}

fn candidate_matches_vid_pid(candidate: &Path, vid: u16, pid: u16) -> bool {
    let read_vid = read_sysfs_hex_u16(candidate, "idVendor");
    let read_pid = read_sysfs_hex_u16(candidate, "idProduct");
    read_vid == Some(vid) && read_pid == Some(pid)
}

fn check_hidraw_control_node(entry: &Path, usb_devices: &Path, vid: u16, pid: u16) -> bool {
    let dev_link = entry.join("device");
    let real_path = match fs::canonicalize(&dev_link) {
        Ok(p) => p,
        Err(_) => return false,
    };

    let parts: Vec<String> = real_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();

    let iface_idx = match find_control_iface_index(&parts) {
        Some(idx) => idx,
        None => return false,
    };

    let candidates = collect_candidate_usb_dirs(&real_path, &parts, iface_idx, usb_devices);
    candidates.iter().any(|c| candidate_matches_vid_pid(c, vid, pid))
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
    let entries = get_sorted_hidraw_entries(sys_class_hidraw)?;

    for entry in entries {
        if check_hidraw_control_node(&entry, usb_devices, vid, pid) {
            let entry_name = entry.file_name().unwrap_or_default().to_string_lossy();
            return Some(PathBuf::from(format!("/dev/{}", entry_name)));
        }
    }

    None
}

fn parse_hid_id_line(val: &str) -> (u16, u16) {
    let sub: Vec<&str> = val.split(':').collect();
    if sub.len() < 3 {
        return (0, 0);
    }
    let vid = u16::from_str_radix(sub[1], 16).unwrap_or(0);
    let pid = u16::from_str_radix(sub[2], 16).unwrap_or(0);
    (vid, pid)
}

fn parse_uevent_line(
    line: &str,
    vendor_id: &mut u16,
    product_id: &mut u16,
    product_string: &mut String,
    phys: &mut String,
) {
    if let Some(val) = line.strip_prefix("HID_ID=") {
        let (v, p) = parse_hid_id_line(val);
        if v != 0 {
            *vendor_id = v;
        }
        if p != 0 {
            *product_id = p;
        }
    } else if let Some(val) = line.strip_prefix("HID_NAME=") {
        *product_string = val.trim().to_string();
    } else if let Some(val) = line.strip_prefix("HID_PHYS=") {
        *phys = val.trim().to_string();
    }
}

fn read_fallback_phys(dev_dir: &Path, phys: &mut String) {
    if phys.is_empty() {
        if let Ok(s) = fs::read_to_string(dev_dir.join("phys")) {
            *phys = s.trim().to_string();
        }
    }
}

fn parse_uevent(dev_dir: &Path) -> (u16, u16, String, String) {
    let mut vendor_id = 0;
    let mut product_id = 0;
    let mut product_string = String::new();
    let mut phys = String::new();

    let uevent_file = dev_dir.join("uevent");
    if let Ok(content) = fs::read_to_string(&uevent_file) {
        for line in content.lines() {
            parse_uevent_line(line, &mut vendor_id, &mut product_id, &mut product_string, &mut phys);
        }
    }

    read_fallback_phys(dev_dir, &mut phys);
    (vendor_id, product_id, product_string, phys)
}

fn is_usb_interface_pattern(comp: &str) -> bool {
    if comp.matches(':').count() != 1 {
        return false;
    }
    match comp.split_once(':') {
        Some((_, after)) => after.starts_with("1.") && after.contains('.'),
        None => false,
    }
}

fn find_usb_interface_component_index(parts: &[String]) -> Option<usize> {
    let mut iface_idx = None;
    for (idx, comp) in parts.iter().enumerate() {
        if is_usb_interface_pattern(comp) {
            iface_idx = Some(idx);
        }
    }
    iface_idx
}

fn extract_usb_parent_from_parts(parts: &[String], idx: usize) -> String {
    if idx > 0 {
        parts[idx - 1].clone()
    } else {
        parts[idx]
            .split_once(':')
            .map(|(before, _)| before.to_string())
            .unwrap_or_default()
    }
}

fn read_b_interface_number(parent: &Path) -> Option<i32> {
    let s = fs::read_to_string(parent.join("bInterfaceNumber")).ok()?;
    s.trim().parse::<i32>().ok()
}

fn parse_interface_from_comp(comp: &str) -> Option<i32> {
    if comp.matches(':').count() != 1 {
        return None;
    }
    let (_, after) = comp.split_once(':')?;
    let (_, iface_str) = after.split_once('.')?;
    iface_str.parse::<i32>().ok()
}

fn fallback_interface_from_path(rp: &Path) -> Option<i32> {
    for comp in rp.components() {
        let s = comp.as_os_str().to_string_lossy();
        if let Some(num) = parse_interface_from_comp(&s) {
            return Some(num);
        }
    }
    None
}

fn discover_interface_and_parent(rp: &Path) -> (Option<i32>, String) {
    let parts: Vec<String> = rp
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();

    let mut usb_parent = find_usb_interface_component_index(&parts)
        .map(|idx| extract_usb_parent_from_parts(&parts, idx))
        .unwrap_or_default();

    let mut interface_number = None;
    if let Some(parent) = rp.parent() {
        interface_number = read_b_interface_number(parent);
        if usb_parent.is_empty() {
            if let Some(gp) = parent.parent() {
                usb_parent = gp.file_name().unwrap_or_default().to_string_lossy().to_string();
            }
        }
    }

    if interface_number.is_none() {
        interface_number = fallback_interface_from_path(rp);
    }

    (interface_number, usb_parent)
}

struct AncestorUsbInfo {
    manufacturer: String,
    product: String,
    release: u16,
    vendor_id: u16,
    product_id: u16,
}

fn update_if_empty(target: &mut String, dir: &Path, file: &str) {
    if target.is_empty() {
        if let Some(s) = read_sysfs_string(dir, file) {
            *target = s;
        }
    }
}

fn update_if_zero(target: &mut u16, dir: &Path, file: &str) {
    if *target == 0 {
        if let Some(v) = read_sysfs_hex_u16(dir, file) {
            *target = v;
        }
    }
}

fn populate_ancestor_dir(info: &mut AncestorUsbInfo, p: &Path) {
    update_if_empty(&mut info.manufacturer, p, "manufacturer");
    update_if_empty(&mut info.product, p, "product");
    update_if_zero(&mut info.release, p, "bcdDevice");
    update_if_zero(&mut info.vendor_id, p, "idVendor");
    update_if_zero(&mut info.product_id, p, "idProduct");
}

fn discover_ancestor_usb_info(
    rp: &Path,
    vendor_id: u16,
    product_id: u16,
    product: String,
) -> AncestorUsbInfo {
    let mut info = AncestorUsbInfo {
        manufacturer: String::new(),
        product,
        release: 0,
        vendor_id,
        product_id,
    };
    let mut curr = rp.parent();
    for _ in 0..4 {
        let p = match curr {
            Some(p) => p,
            None => break,
        };
        populate_ancestor_dir(&mut info, p);
        curr = p.parent();
    }
    info
}

fn build_device_info(entry: &Path) -> DeviceInfo {
    let entry_name = entry.file_name().unwrap_or_default().to_string_lossy();
    let dev_path = PathBuf::from(format!("/dev/{}", entry_name));
    let dev_dir = entry.join("device");
    let (mut vid, mut pid, prod_str, phys) = parse_uevent(&dev_dir);

    let real_path = fs::canonicalize(&dev_dir).ok();
    let (interface_number, usb_parent) = real_path
        .as_deref()
        .map(discover_interface_and_parent)
        .unwrap_or_default();

    let (mfg_str, prod_str, release) = if let Some(ref rp) = real_path {
        let anc = discover_ancestor_usb_info(rp, vid, pid, prod_str);
        vid = anc.vendor_id;
        pid = anc.product_id;
        (anc.manufacturer, anc.product, anc.release)
    } else {
        (String::new(), prod_str, 0)
    };

    let is_known = vid == KNOWN_VID && pid == KNOWN_PID;
    let device_name = if is_known {
        "AULA F75".to_string()
    } else {
        "Unknown".to_string()
    };

    DeviceInfo {
        vendor_id: vid,
        product_id: pid,
        path: dev_path,
        interface_number,
        manufacturer_string: mfg_str,
        product_string: prod_str,
        release_number: release,
        usage_page: 0,
        usage: 0,
        is_known,
        device_name,
        usb_parent,
        phys,
    }
}

/// Enumerate HID device nodes from sysfs and parse device attributes.
pub fn enumerate_devices(sys_class_hidraw: &Path) -> Vec<DeviceInfo> {
    let entries = match get_sorted_hidraw_entries(sys_class_hidraw) {
        Some(e) => e,
        None => return Vec::new(),
    };
    entries.iter().map(|e| build_device_info(e)).collect()
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

fn try_send_frame_step<F, S>(
    send_fn: &mut F,
    frame: &[u8; crate::proto::FRAME_LEN],
    consecutive_failures: &mut usize,
    interval: Duration,
    sleep_fn: &mut S,
) -> Result<bool, io::Error>
where
    F: FnMut(&[u8; crate::proto::FRAME_LEN]) -> Result<(), io::Error>,
    S: FnMut(Duration),
{
    match send_fn(frame) {
        Ok(()) => {
            *consecutive_failures = 0;
            Ok(true)
        }
        Err(e) => {
            let code = e.raw_os_error();
            if code == Some(libc::EPIPE) || code == Some(libc::ETIMEDOUT) {
                *consecutive_failures += 1;
                if *consecutive_failures >= 10 {
                    return Err(e);
                }
                sleep_fn(interval);
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
}

fn is_loop_timeout_reached(start_time: Instant, seconds: Option<f64>, count: u64) -> bool {
    match seconds {
        Some(sec) if sec > 0.0 => count >= 2 && start_time.elapsed().as_secs_f64() >= sec,
        _ => false,
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
        if !try_send_frame_step(
            &mut send_fn,
            &frame,
            &mut consecutive_failures,
            interval,
            &mut sleep_fn,
        )? {
            continue;
        }

        count += 1;
        if is_loop_timeout_reached(start_time, seconds, count) {
            break;
        }

        sleep_fn(interval);

        if is_loop_timeout_reached(start_time, seconds, count) {
            break;
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
        if !try_send_frame_step(
            &mut send_fn,
            &frame,
            &mut consecutive_failures,
            interval,
            &mut sleep_fn,
        )? {
            continue;
        }

        count += 1;
        if is_loop_timeout_reached(start_time, seconds, count) {
            break;
        }

        sleep_fn(interval);

        if is_loop_timeout_reached(start_time, seconds, count) {
            break;
        }
    }

    Ok(())
}

fn build_effect_frame(
    effect: &crate::effects::Effect,
    brightness: f32,
    start_time: Instant,
    key_slots: &[u8],
) -> Result<[u8; crate::proto::FRAME_LEN], io::Error> {
    let t = start_time.elapsed().as_secs_f64();
    let pairs = crate::effects::render(effect, t, key_slots);
    let scaled = crate::effects::apply_brightness(&pairs, brightness);
    crate::proto::build_slot_frame(&scaled)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
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
        let frame = build_effect_frame(effect, brightness, start_time, &key_slots)?;

        if !try_send_frame_step(
            &mut send_fn,
            &frame,
            &mut consecutive_failures,
            interval,
            &mut sleep_fn,
        )? {
            continue;
        }

        count += 1;
        if is_loop_timeout_reached(start_time, seconds, count) {
            break;
        }

        sleep_fn(interval);

        if is_loop_timeout_reached(start_time, seconds, count) {
            break;
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

