//! Hardware-free unit and integration tests for battery status, wireless dongle detection,
//! and knob mode reporting for the AULA F75 keyboard.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use aula_f75::cli::{Cli, Commands};
use aula_f75::device::power::{
    parse_battery_feature_report, query_battery_with_sources, query_sysfs_battery_for_device,
    read_sysfs_battery, BatteryStatus,
};
use aula_f75::device::{
    enumerate_wireless_nodes_with_roots, query_knob_mode_with_roots, DeviceConnectionKind,
};
use aula_f75::proto::{
    hid_get_feature, parse_knob_from_report_descriptor, KnobModeSource,
};
use clap::Parser;

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "aula_f75_power_test_{}_{}_{}",
            prefix,
            std::process::id(),
            now
        ));
        fs::create_dir_all(&path).expect("Failed to create temp dir");
        Self { path }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn create_fake_hidraw_node(
    base: &Path,
    hidraw_name: &str,
    bus_id: &str,
    iface_comp: &str,
    vid_hex: &str,
    pid_hex: &str,
    product_name: &str,
    phys_str: Option<&str>,
) -> (PathBuf, PathBuf) {
    let sys_class_hidraw = base.join("sys/class/hidraw");
    let usb_devices = base.join("sys/bus/usb/devices");
    fs::create_dir_all(&sys_class_hidraw).unwrap();
    fs::create_dir_all(&usb_devices).unwrap();

    let dev_dir = base.join(format!("sys/devices/pci0000:00/{}", bus_id));
    fs::create_dir_all(&dev_dir).unwrap();
    fs::write(dev_dir.join("idVendor"), format!("{}\n", vid_hex)).unwrap();
    fs::write(dev_dir.join("idProduct"), format!("{}\n", pid_hex)).unwrap();
    fs::write(dev_dir.join("product"), format!("{}\n", product_name)).unwrap();

    let usb_link = usb_devices.join(bus_id);
    if !usb_link.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dev_dir, &usb_link).unwrap();
    }

    let iface_dir = dev_dir.join(iface_comp);
    fs::create_dir_all(&iface_dir).unwrap();

    let hid_dir = iface_dir.join(format!(
        "0003:{}:{}.0001",
        vid_hex.to_uppercase(),
        pid_hex.to_uppercase()
    ));
    fs::create_dir_all(&hid_dir).unwrap();

    let uevent_content = format!(
        "HID_ID=0003:{}:{}\nHID_NAME={}\n{}",
        vid_hex.to_uppercase(),
        pid_hex.to_uppercase(),
        product_name,
        phys_str
            .map(|p| format!("HID_PHYS={}\n", p))
            .unwrap_or_default()
    );
    fs::write(hid_dir.join("uevent"), uevent_content).unwrap();

    if let Some(p) = phys_str {
        fs::write(hid_dir.join("phys"), format!("{}\n", p)).unwrap();
    }

    let hidraw_entry = sys_class_hidraw.join(hidraw_name);
    fs::create_dir_all(&hidraw_entry).unwrap();
    let dev_symlink = hidraw_entry.join("device");
    if !dev_symlink.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(&hid_dir, &dev_symlink).unwrap();
    }

    (sys_class_hidraw, usb_devices)
}

// ============================================================================
// 1. Battery Status Query Tests
// ============================================================================

#[test]
fn test_read_sysfs_battery_with_valid_capacity_and_charging() {
    let guard = TempDirGuard::new("battery_charging");
    fs::write(guard.path.join("capacity"), "85\n").unwrap();
    fs::write(guard.path.join("status"), "Charging\n").unwrap();
    fs::write(guard.path.join("present"), "1\n").unwrap();

    let status = read_sysfs_battery(&guard.path);
    assert!(status.present);
    assert_eq!(status.percent, Some(85));
    assert_eq!(status.charging, Some(true));
    assert!(!status.raw.is_empty());
}

#[test]
fn test_read_sysfs_battery_discharging() {
    let guard = TempDirGuard::new("battery_discharging");
    fs::write(guard.path.join("capacity"), "42\n").unwrap();
    fs::write(guard.path.join("status"), "Discharging\n").unwrap();
    fs::write(guard.path.join("present"), "1\n").unwrap();

    let status = read_sysfs_battery(&guard.path);
    assert!(status.present);
    assert_eq!(status.percent, Some(42));
    assert_eq!(status.charging, Some(false));
}

#[test]
fn test_read_sysfs_battery_nonexistent_directory() {
    let guard = TempDirGuard::new("battery_missing");
    let missing_path = guard.path.join("nonexistent_subdir");
    let status = read_sysfs_battery(&missing_path);
    assert!(!status.present);
    assert_eq!(status.percent, None);
    assert_eq!(status.charging, None);
    assert!(status.raw.is_empty());
}

#[test]
fn test_read_sysfs_battery_invalid_capacity_graceful() {
    let guard = TempDirGuard::new("battery_invalid_cap");
    fs::write(guard.path.join("capacity"), "not_a_number\n").unwrap();
    fs::write(guard.path.join("status"), "Full\n").unwrap();

    let status = read_sysfs_battery(&guard.path);
    assert_eq!(status.percent, None);
    assert_eq!(status.charging, Some(false));
}

#[test]
fn test_query_sysfs_battery_with_subdirectories_and_filter() {
    let guard = TempDirGuard::new("battery_subdirs");
    let psupply_root = guard.path.join("power_supply");
    fs::create_dir_all(&psupply_root).unwrap();

    // Create entry for another device
    let other_dev = psupply_root.join("BAT0");
    fs::create_dir_all(&other_dev).unwrap();
    fs::write(other_dev.join("capacity"), "10\n").unwrap();

    // Create entry matching AULA F75 filter
    let aula_dev = psupply_root.join("hid-0003:258A:010C.0001-battery");
    fs::create_dir_all(&aula_dev).unwrap();
    fs::write(aula_dev.join("capacity"), "95\n").unwrap();
    fs::write(aula_dev.join("status"), "Not charging\n").unwrap();
    fs::write(aula_dev.join("present"), "1\n").unwrap();

    let status = query_sysfs_battery_for_device(&psupply_root, Some("258A:010C"));
    assert!(status.present);
    assert_eq!(status.percent, Some(95));
    assert_eq!(status.charging, Some(false));
}

#[test]
fn test_query_sysfs_battery_filter_bypasses_unrelated_devices() {
    let guard = TempDirGuard::new("battery_unrelated");
    let psupply_root = guard.path.join("power_supply");
    fs::create_dir_all(&psupply_root).unwrap();

    let bat0 = psupply_root.join("BAT0");
    fs::create_dir_all(&bat0).unwrap();
    fs::write(bat0.join("capacity"), "50\n").unwrap();
    fs::write(bat0.join("status"), "Discharging\n").unwrap();
    fs::write(bat0.join("present"), "1\n").unwrap();

    // Query with filter for AULA F75 must return empty (skipping unrelated BAT0)
    let filtered_status = query_sysfs_battery_for_device(&psupply_root, Some("258A:010C"));
    assert!(!filtered_status.present);
    assert_eq!(filtered_status.percent, None);
    assert_eq!(filtered_status.charging, None);

    // Query without filter must retain existing behavior and find BAT0
    let unfiltered_status = query_sysfs_battery_for_device(&psupply_root, None);
    assert!(unfiltered_status.present);
    assert_eq!(unfiltered_status.percent, Some(50));
    assert_eq!(unfiltered_status.charging, Some(false));
}

#[test]
fn test_parse_battery_feature_report_empty_and_zeroes() {
    let empty_status = parse_battery_feature_report(&[]);
    assert!(!empty_status.present);
    assert_eq!(empty_status.percent, None);
    assert_eq!(empty_status.charging, None);

    let zero_buf = [0u8; 8];
    let zero_status = parse_battery_feature_report(&zero_buf);
    assert!(!zero_status.present);
    assert_eq!(zero_status.percent, None);
    assert_eq!(zero_status.charging, None);
}

#[test]
fn test_parse_battery_feature_report_valid_payload() {
    // Report ID 8, 72% battery, charging = 1
    let payload = [0x08, 72, 1, 0, 0];
    let status = parse_battery_feature_report(&payload);
    assert!(status.present);
    assert_eq!(status.percent, Some(72));
    assert_eq!(status.charging, Some(true));
    assert_eq!(status.raw, payload.to_vec());

    // Report ID 8, 30% battery, discharging = 0
    let payload_dischg = [0x08, 30, 0];
    let status_dischg = parse_battery_feature_report(&payload_dischg);
    assert!(status_dischg.present);
    assert_eq!(status_dischg.percent, Some(30));
    assert_eq!(status_dischg.charging, Some(false));
}

#[test]
fn test_query_battery_with_sources_sysfs_precedence() {
    let guard = TempDirGuard::new("battery_sources");
    fs::write(guard.path.join("capacity"), "88\n").unwrap();
    fs::write(guard.path.join("status"), "Charging\n").unwrap();

    let mut hid_read_called = false;
    let status = query_battery_with_sources(&guard.path, None, |_buf| {
        hid_read_called = true;
        Ok(0)
    });

    assert_eq!(status.percent, Some(88));
    assert_eq!(status.charging, Some(true));
    assert!(!hid_read_called, "Should not query HID feature report if sysfs succeeds");
}

#[test]
fn test_query_battery_with_sources_fallback_to_feature_report() {
    let guard = TempDirGuard::new("battery_fallback");
    let missing_sysfs = guard.path.join("nonexistent");

    let status = query_battery_with_sources(&missing_sysfs, None, |buf| {
        buf[0] = 0x08;
        buf[1] = 65;
        buf[2] = 0;
        Ok(3)
    });

    assert!(status.present);
    assert_eq!(status.percent, Some(65));
    assert_eq!(status.charging, Some(false));
}

#[test]
fn test_query_battery_with_sources_empty_graceful() {
    let guard = TempDirGuard::new("battery_empty_graceful");
    let missing_sysfs = guard.path.join("nonexistent");

    let status = query_battery_with_sources(&missing_sysfs, None, |_buf| Ok(0));

    assert_eq!(status, BatteryStatus::empty());
}

#[test]
fn test_hid_get_feature_prohibits_report_5() {
    let mut buf = [0u8; 64];
    let res = hid_get_feature(-1, 5, &mut buf);
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    assert!(err.to_string().contains("report ID 5"));
}

#[test]
fn test_hid_get_feature_empty_buffer_rejected() {
    let res = hid_get_feature(-1, 6, &mut []);
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().kind(), io::ErrorKind::InvalidInput);
}

// ============================================================================
// 2. Wireless Dongle Detection Tests (2.4GHz vs Cable)
// ============================================================================

#[test]
fn test_wireless_dongle_classification_cable_vs_dongle() {
    let guard = TempDirGuard::new("dongle_vs_cable");

    // Cable interface 0 (boot keyboard) on bus 5-2
    create_fake_hidraw_node(
        &guard.path,
        "hidraw0",
        "5-2",
        "5-2:1.0",
        "258a",
        "010c",
        "AULA F75",
        Some("usb-0000:00:14.0-2/input0"),
    );

    // Cable interface 1 (control node) on bus 5-2
    create_fake_hidraw_node(
        &guard.path,
        "hidraw1",
        "5-2",
        "5-2:1.1",
        "258a",
        "010c",
        "AULA F75",
        Some("usb-0000:00:14.0-2/input1"),
    );

    // Wireless dongle node on bus 1-4 with dongle product name
    create_fake_hidraw_node(
        &guard.path,
        "hidraw2",
        "1-4",
        "1-4:1.1",
        "258a",
        "010c",
        "AULA F75 2.4G Receiver",
        Some("usb-0000:00:14.0-4/input1"),
    );

    let sys_class = guard.path.join("sys/class/hidraw");
    let candidates = enumerate_wireless_nodes_with_roots(&sys_class);

    assert_eq!(candidates.len(), 3);

    // Node 0: Cable Interface 0
    let c0 = candidates.iter().find(|c| c.path == PathBuf::from("/dev/hidraw0")).unwrap();
    assert!(!c0.is_dongle);
    assert_eq!(c0.connection_kind, DeviceConnectionKind::CableInterface0Keyboard);
    assert_eq!(c0.usb_parent, "5-2");

    // Node 1: Cable Interface 1
    let c1 = candidates.iter().find(|c| c.path == PathBuf::from("/dev/hidraw1")).unwrap();
    assert!(!c1.is_dongle);
    assert_eq!(c1.connection_kind, DeviceConnectionKind::CableInterface1Control);
    assert_eq!(c1.usb_parent, "5-2");

    // Node 2: Wireless Dongle
    let c2 = candidates.iter().find(|c| c.path == PathBuf::from("/dev/hidraw2")).unwrap();
    assert!(c2.is_dongle);
    assert_eq!(c2.connection_kind, DeviceConnectionKind::WirelessDongle);
    assert_eq!(c2.usb_parent, "1-4");
}

#[test]
fn test_wireless_dongle_classification_by_phys_keyword() {
    let guard = TempDirGuard::new("dongle_phys");

    create_fake_hidraw_node(
        &guard.path,
        "hidraw3",
        "1-3",
        "1-3:1.1",
        "258a",
        "010c",
        "AULA F75",
        Some("usb-0000:00:14.0-3/wireless0"),
    );

    let sys_class = guard.path.join("sys/class/hidraw");
    let candidates = enumerate_wireless_nodes_with_roots(&sys_class);
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].is_dongle);
    assert_eq!(candidates[0].connection_kind, DeviceConnectionKind::WirelessDongle);
}

// ============================================================================
// 3. Knob Mode Reporting Tests
// ============================================================================

#[test]
fn test_parse_knob_consumer_control_volume() {
    let desc = [
        0x05, 0x0C, // Usage Page (Consumer)
        0x09, 0xE9, // Usage (Volume Increment)
        0x09, 0xEA, // Usage (Volume Decrement)
        0x09, 0xE2, // Usage (Mute)
    ];

    let report = parse_knob_from_report_descriptor(&desc);
    assert_eq!(report.source, KnobModeSource::ConsumerControl);
    assert!(report.description.contains("Consumer Control"));
    assert!(report.details.iter().any(|d| d.contains("Volume Increment")));
    assert!(report.details.iter().any(|d| d.contains("Volume Decrement")));
    assert!(report.details.iter().any(|d| d.contains("Mute")));
}

#[test]
fn test_parse_knob_generic_desktop_dial() {
    let desc = [
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x37, // Usage (Dial)
    ];

    let report = parse_knob_from_report_descriptor(&desc);
    assert_eq!(report.source, KnobModeSource::GenericDesktopDial);
    assert!(report.description.contains("Dial"));
}

#[test]
fn test_parse_knob_undetermined_report_descriptor() {
    let empty_report = parse_knob_from_report_descriptor(&[]);
    assert_eq!(empty_report.source, KnobModeSource::Undetermined);
    assert_eq!(empty_report.description, "undetermined, needs live USB capture");

    // Standard Keyboard descriptor (Generic Desktop, Keyboard usage)
    let kbd_desc = [0x05, 0x01, 0x09, 0x06];
    let kbd_report = parse_knob_from_report_descriptor(&kbd_desc);
    assert_eq!(kbd_report.source, KnobModeSource::Undetermined);
    assert_eq!(kbd_report.description, "undetermined, needs live USB capture");
}

#[test]
fn test_query_knob_mode_with_roots_missing_dir() {
    let guard = TempDirGuard::new("knob_missing");
    let missing_path = guard.path.join("nonexistent");
    let report = query_knob_mode_with_roots(&missing_path);
    assert_eq!(report.source, KnobModeSource::Undetermined);
    assert_eq!(report.description, "undetermined, needs live USB capture");
}

#[test]
fn test_query_knob_mode_with_fake_sysfs_report_descriptor() {
    let guard = TempDirGuard::new("knob_fake_sysfs");

    create_fake_hidraw_node(
        &guard.path,
        "hidraw0",
        "5-2",
        "5-2:1.1",
        "258a",
        "010c",
        "AULA F75",
        Some("usb-0000:00:14.0-2/input1"),
    );

    // Write Consumer Control descriptor into the device directory
    let desc_path = guard.path.join("sys/devices/pci0000:00/5-2/5-2:1.1/0003:258A:010C.0001/report_descriptor");
    let desc_bytes = [
        0x05, 0x0C, // Usage Page (Consumer)
        0x09, 0xE9, // Usage (Volume Increment)
        0x09, 0xEA, // Usage (Volume Decrement)
    ];
    fs::write(&desc_path, desc_bytes).unwrap();

    let sys_class = guard.path.join("sys/class/hidraw");
    let report = query_knob_mode_with_roots(&sys_class);
    assert_eq!(report.source, KnobModeSource::ConsumerControl);
    assert!(report.description.contains("Consumer Control"));
}

// ============================================================================
// 4. CLI Subcommands Parsing Tests
// ============================================================================

#[test]
fn test_cli_subcommands_battery_wireless_knob() {
    let parsed_battery = Cli::try_parse_from(["aula-f75-rs", "battery"]);
    assert!(parsed_battery.is_ok());
    assert!(matches!(parsed_battery.unwrap().command, Commands::Battery));

    let parsed_wireless = Cli::try_parse_from(["aula-f75-rs", "wireless"]);
    assert!(parsed_wireless.is_ok());
    assert!(matches!(parsed_wireless.unwrap().command, Commands::Wireless));

    let parsed_knob = Cli::try_parse_from(["aula-f75-rs", "knob"]);
    assert!(parsed_knob.is_ok());
    assert!(matches!(parsed_knob.unwrap().command, Commands::Knob));
}
