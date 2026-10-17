//! Empirically verified RGB protocol implementation for AULA F75 (258a:010c).

use std::io;
use std::os::unix::io::RawFd;

/// Total frame length in bytes for direct RGB feature reports.
pub const FRAME_LEN: usize = 520;

/// Report ID for direct RGB frame writes.
pub const REPORT_ID: u8 = 6;

/// Fixed 8-byte direct RGB frame header.
pub const HEADER: [u8; 8] = [0x06, 0x08, 0x00, 0x00, 0x01, 0x00, 0x7A, 0x01];

/// Number of 3-byte RGB key slots in the frame (170 slots = 510 bytes).
pub const SLOT_COUNT: usize = (FRAME_LEN - 8) / 3;

/// Recommended refresh frequency for direct RGB frame keepalive.
pub const KEEPALIVE_HZ: f64 = 1.5;

/// Build a 520-byte direct RGB frame for the AULA F75 (Report ID 6).
///
/// Accepts an `(r, g, b)` tuple where `u8` channels are inherently in `0..=255`.
/// Fills 8-byte header, 170 3-byte slots, and zero-pads the final 2 bytes.
pub fn build_direct_frame(rgb: (u8, u8, u8)) -> Result<[u8; FRAME_LEN], String> {
    let mut buf = [0u8; FRAME_LEN];
    buf[0..8].copy_from_slice(&HEADER);

    let (r, g, b) = rgb;
    for i in 0..SLOT_COUNT {
        let offset = 8 + 3 * i;
        buf[offset] = r;
        buf[offset + 1] = g;
        buf[offset + 2] = b;
    }

    // Bytes 518..520 remain 0 (initialized as 0)
    Ok(buf)
}

/// Send a HID feature report using the Linux HIDIOCSFEATURE ioctl.
///
/// # Safety Rules
/// - HARD SAFETY RULE: Writes to Report ID 5 are strictly prohibited to prevent
///   triggering the ISP bootloader / soft-bricking the MCU.
/// - Requires `dir = 3` (READ|WRITE) ioctl direction on Linux hidraw.
pub fn hid_set_feature(fd: RawFd, data: &[u8]) -> Result<(), io::Error> {
    if data.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Feature report data cannot be empty",
        ));
    }

    if data[0] == 5 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Safety violation: writes to report ID 5 are strictly prohibited",
        ));
    }

    let length = data.len();
    // HIDIOCSFEATURE(len) encoding:
    // dir = 3 (READ|WRITE), size = length, type = 0x48 ('H'), nr = 0x06
    // SET: req = (3 << 30) | (length << 16) | (0x48 << 8) | 0x06
    let req = ((3u64 << 30) | ((length as u64) << 16) | (0x48u64 << 8) | 0x06u64) as libc::c_ulong;

    // dir = 3 indicates copy-back from kernel, requiring a mutable buffer.
    let mut buf = data.to_vec();
    let ret = unsafe { libc::ioctl(fd, req as libc::c_ulong, buf.as_mut_ptr()) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_direct_frame_length_and_header() {
        let frame = build_direct_frame((0xAA, 0xBB, 0xCC)).expect("Frame build failed");
        assert_eq!(frame.len(), FRAME_LEN);
        assert_eq!(&frame[0..8], &HEADER);
    }

    #[test]
    fn test_build_direct_frame_rgb_slots() {
        let frame = build_direct_frame((0x11, 0x22, 0x33)).expect("Frame build failed");

        // Slot 0 at byte 8..11
        assert_eq!(frame[8], 0x11);
        assert_eq!(frame[9], 0x22);
        assert_eq!(frame[10], 0x33);

        // Slot 1 at byte 11..14
        assert_eq!(frame[11], 0x11);
        assert_eq!(frame[12], 0x22);
        assert_eq!(frame[13], 0x33);

        // Slot 169 at byte 8 + 3*169 = 515..518
        assert_eq!(frame[515], 0x11);
        assert_eq!(frame[516], 0x22);
        assert_eq!(frame[517], 0x33);

        // Zero padding at bytes 518..520
        assert_eq!(frame[518], 0x00);
        assert_eq!(frame[519], 0x00);
    }

    #[test]
    fn test_report_5_guard_rejects_report_5() {
        let mut bad_report = [0u8; FRAME_LEN];
        bad_report[0] = 5;
        let res = hid_set_feature(-1, &bad_report);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("report ID 5"));
    }

    #[test]
    fn test_empty_report_rejects() {
        let res = hid_set_feature(-1, &[]);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
}
