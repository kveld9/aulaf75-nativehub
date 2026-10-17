//! Hardware-free integration tests for AULA F75 Rust implementation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use aula_f75::cli::{parse_color_hex, parse_hex_or_dec};
use aula_f75::device::{
    enumerate_devices, resolve_control_node_with_roots, send_with_retry, KNOWN_PID, KNOWN_VID,
};
use aula_f75::proto::{build_direct_frame, hid_set_feature, FRAME_LEN, HEADER};

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
            "aula_f75_rs_test_{}_{}_{}",
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

fn create_fake_sysfs(
    base: &Path,
    hidraw_name: &str,
    bus_id: &str,
    iface_comp: &str,
    vid_hex: &str,
    pid_hex: &str,
) -> (PathBuf, PathBuf) {
    let sys_class_hidraw = base.join("sys/class/hidraw");
    let usb_devices = base.join("sys/bus/usb/devices");
    fs::create_dir_all(&sys_class_hidraw).unwrap();
    fs::create_dir_all(&usb_devices).unwrap();

    let dev_dir = base.join(format!("sys/devices/pci0000:00/{}", bus_id));
    fs::create_dir_all(&dev_dir).unwrap();
    fs::write(dev_dir.join("idVendor"), format!("{}\n", vid_hex)).unwrap();
    fs::write(dev_dir.join("idProduct"), format!("{}\n", pid_hex)).unwrap();

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

    let hidraw_entry = sys_class_hidraw.join(hidraw_name);
    fs::create_dir_all(&hidraw_entry).unwrap();
    let dev_symlink = hidraw_entry.join("device");
    if !dev_symlink.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(&hid_dir, &dev_symlink).unwrap();
    }

    (sys_class_hidraw, usb_devices)
}

#[test]
fn test_direct_frame_exact_bytes_header_and_slots() {
    let frame = build_direct_frame((0xDE, 0xAD, 0xBE)).expect("Frame building failed");

    // Frame length must be exactly 520 bytes
    assert_eq!(frame.len(), FRAME_LEN);
    assert_eq!(frame.len(), 520);

    // Header exact 8 bytes
    assert_eq!(&frame[0..8], &HEADER);
    assert_eq!(&frame[0..8], &[0x06, 0x08, 0x00, 0x00, 0x01, 0x00, 0x7A, 0x01]);

    // Slot 0 (offset 8)
    assert_eq!(frame[8], 0xDE);
    assert_eq!(frame[9], 0xAD);
    assert_eq!(frame[10], 0xBE);

    // Slot 1 (offset 11)
    assert_eq!(frame[11], 0xDE);
    assert_eq!(frame[12], 0xAD);
    assert_eq!(frame[13], 0xBE);

    // Slot 169 (offset 8 + 3*169 = 515)
    assert_eq!(frame[515], 0xDE);
    assert_eq!(frame[516], 0xAD);
    assert_eq!(frame[517], 0xBE);

    // Zero padding at bytes 518..520
    assert_eq!(frame[518], 0x00);
    assert_eq!(frame[519], 0x00);
}

#[test]
fn test_report_5_guard_blocks_and_errors() {
    let mut payload = [0u8; 520];
    payload[0] = 5; // Report ID 5 forbidden
    let res = hid_set_feature(-1, &payload);
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    assert!(err.to_string().contains("report ID 5"));

    // Empty payload must also error
    let empty_res = hid_set_feature(-1, &[]);
    assert!(empty_res.is_err());
    assert_eq!(empty_res.unwrap_err().kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn test_hex_color_parsing_accepts_and_rejects() {
    // Valid 6-digit hex colors
    assert_eq!(parse_color_hex("ff0000").unwrap(), (255, 0, 0));
    assert_eq!(parse_color_hex("#00ff00").unwrap(), (0, 255, 0));
    assert_eq!(parse_color_hex("#0000ff").unwrap(), (0, 0, 255));
    assert_eq!(parse_color_hex("1A2B3C").unwrap(), (0x1A, 0x2B, 0x3C));
    assert_eq!(parse_color_hex("#FFFFFF").unwrap(), (255, 255, 255));
    assert_eq!(parse_color_hex("000000").unwrap(), (0, 0, 0));

    // Invalid colors
    assert!(parse_color_hex("fff").is_err());
    assert!(parse_color_hex("12345").is_err());
    assert!(parse_color_hex("1234567").is_err());
    assert!(parse_color_hex("12345g").is_err());
    assert!(parse_color_hex("#").is_err());
    assert!(parse_color_hex("").is_err());
    assert!(parse_color_hex("##ff0000").is_err());
}

#[test]
fn test_parse_hex_or_dec_values() {
    assert_eq!(parse_hex_or_dec("0x258A").unwrap(), 0x258A);
    assert_eq!(parse_hex_or_dec("0X010C").unwrap(), 0x010C);
    assert_eq!(parse_hex_or_dec("258a").unwrap(), 0x258A);
    assert_eq!(parse_hex_or_dec("010c").unwrap(), 0x010C);
    assert_eq!(parse_hex_or_dec("9610").unwrap(), 9610);
    assert!(parse_hex_or_dec("invalid").is_err());
}

#[test]
fn test_retry_logic_fail_twice_then_ok() {
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
fn test_retry_logic_always_fail_exhausts_retries() {
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
fn test_retry_logic_immediate_exit_on_fatal_error() {
    let mut attempts = 0;

    let res = send_with_retry(
        || {
            attempts += 1;
            Err(io::Error::from_raw_os_error(libc::EINVAL))
        },
        10,
        || panic!("Fatal errors must not sleep"),
    );

    assert!(res.is_err());
    assert_eq!(attempts, 1);
    assert_eq!(res.unwrap_err().raw_os_error(), Some(libc::EINVAL));
}

#[test]
fn test_sysfs_resolver_matches_interface_1() {
    let guard = TempDirGuard::new("match_iface_1");
    let (sys_class_hidraw, usb_devices) = create_fake_sysfs(
        &guard.path,
        "hidraw1",
        "5-2",
        "5-2:1.1",
        "258a",
        "010c",
    );

    let res = resolve_control_node_with_roots(&sys_class_hidraw, &usb_devices, KNOWN_VID, KNOWN_PID);
    assert_eq!(res, Some(PathBuf::from("/dev/hidraw1")));
}

#[test]
fn test_sysfs_resolver_skips_interface_0() {
    let guard = TempDirGuard::new("skip_iface_0");
    // Interface 0 on hidraw0
    create_fake_sysfs(
        &guard.path,
        "hidraw0",
        "5-2",
        "5-2:1.0",
        "258a",
        "010c",
    );
    // Interface 1 on hidraw1
    let (sys_class_hidraw, usb_devices) = create_fake_sysfs(
        &guard.path,
        "hidraw1",
        "5-2",
        "5-2:1.1",
        "258a",
        "010c",
    );

    let res = resolve_control_node_with_roots(&sys_class_hidraw, &usb_devices, KNOWN_VID, KNOWN_PID);
    assert_eq!(res, Some(PathBuf::from("/dev/hidraw1")));
}

#[test]
fn test_sysfs_resolver_vid_mismatch_returns_none() {
    let guard = TempDirGuard::new("vid_mismatch");
    let (sys_class_hidraw, usb_devices) = create_fake_sysfs(
        &guard.path,
        "hidraw1",
        "5-2",
        "5-2:1.1",
        "9999",
        "8888",
    );

    let res = resolve_control_node_with_roots(&sys_class_hidraw, &usb_devices, KNOWN_VID, KNOWN_PID);
    assert_eq!(res, None);
}

#[test]
fn test_sysfs_resolver_pci_multi_colon_regression() {
    let guard = TempDirGuard::new("pci_multi_colon");
    let sys_class_hidraw = guard.path.join("sys/class/hidraw");
    let usb_devices = guard.path.join("sys/bus/usb/devices");
    fs::create_dir_all(&sys_class_hidraw).unwrap();
    fs::create_dir_all(&usb_devices).unwrap();

    // Dev path with PCI multi-colon component: pci0000:00/0000:00:08.1/usb5/5-2
    let dev_dir = guard
        .path
        .join("sys/devices/pci0000:00/0000:00:08.1/usb5/5-2");
    fs::create_dir_all(&dev_dir).unwrap();
    fs::write(dev_dir.join("idVendor"), "258a\n").unwrap();
    fs::write(dev_dir.join("idProduct"), "010c\n").unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(&dev_dir, usb_devices.join("5-2")).unwrap();

    // Interface 0 (boot keyboard interface -> hidraw1)
    let iface0_dir = dev_dir.join("5-2:1.0");
    fs::create_dir_all(&iface0_dir).unwrap();
    let hid0_dir = iface0_dir.join("0003:258A:010C.0008");
    fs::create_dir_all(&hid0_dir).unwrap();
    let hr1 = sys_class_hidraw.join("hidraw1");
    fs::create_dir_all(&hr1).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&hid0_dir, hr1.join("device")).unwrap();

    // Interface 1 (control interface -> hidraw3)
    let iface1_dir = dev_dir.join("5-2:1.1");
    fs::create_dir_all(&iface1_dir).unwrap();
    let hid1_dir = iface1_dir.join("0003:258A:010C.0009");
    fs::create_dir_all(&hid1_dir).unwrap();
    let hr3 = sys_class_hidraw.join("hidraw3");
    fs::create_dir_all(&hr3).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&hid1_dir, hr3.join("device")).unwrap();

    let res = resolve_control_node_with_roots(&sys_class_hidraw, &usb_devices, KNOWN_VID, KNOWN_PID);
    assert_eq!(res, Some(PathBuf::from("/dev/hidraw3")));
}

#[test]
fn test_enumerate_devices_detects_known_and_interface() {
    let guard = TempDirGuard::new("enumerate");
    let (sys_class_hidraw, _) = create_fake_sysfs(
        &guard.path,
        "hidraw2",
        "5-2",
        "5-2:1.1",
        "258a",
        "010c",
    );

    let devs = enumerate_devices(&sys_class_hidraw);
    assert_eq!(devs.len(), 1);
    assert_eq!(devs[0].vendor_id, KNOWN_VID);
    assert_eq!(devs[0].product_id, KNOWN_PID);
    assert_eq!(devs[0].path, PathBuf::from("/dev/hidraw2"));
    assert_eq!(devs[0].interface_number, Some(1));
    assert!(devs[0].is_known);
    assert_eq!(devs[0].device_name, "AULA F75");
}
