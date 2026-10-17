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

/// Mapping of canonical AULA F75 key names to 170-slot report-6 frame slot indices.
///
/// Layout source: OpenRGB Sinowealth 010C aula_f75_layout cross-checked facts
/// (live hardware verification pending).
pub const KEY_SLOTS: &[(&str, u8)] = &[
    // Row 0
    ("esc", 0),
    ("f1", 12),
    ("f2", 18),
    ("f3", 24),
    ("f4", 30),
    ("f5", 36),
    ("f6", 42),
    ("f7", 48),
    ("f8", 54),
    ("f9", 60),
    ("f10", 66),
    ("f11", 72),
    ("f12", 78),
    ("del", 85),
    ("pgup", 86),
    ("pgdn", 87),
    // Row 1
    ("backtick", 1),
    ("1", 7),
    ("2", 13),
    ("3", 19),
    ("4", 25),
    ("5", 31),
    ("6", 37),
    ("7", 43),
    ("8", 49),
    ("9", 55),
    ("0", 61),
    ("minus", 67),
    ("equal", 73),
    ("backspace", 79),
    // Row 2
    ("tab", 2),
    ("q", 8),
    ("w", 14),
    ("e", 20),
    ("r", 26),
    ("t", 32),
    ("y", 38),
    ("u", 44),
    ("i", 50),
    ("o", 56),
    ("p", 62),
    ("lbracket", 68),
    ("rbracket", 74),
    ("backslash", 80),
    // Row 3
    ("capslock", 3),
    ("a", 9),
    ("s", 15),
    ("d", 21),
    ("f", 27),
    ("g", 33),
    ("h", 39),
    ("j", 45),
    ("k", 51),
    ("l", 57),
    ("semicolon", 63),
    ("quote", 69),
    ("enter", 81),
    // Row 4
    ("lshift", 4),
    ("z", 10),
    ("x", 16),
    ("c", 22),
    ("v", 28),
    ("b", 34),
    ("n", 40),
    ("m", 46),
    ("comma", 52),
    ("dot", 58),
    ("slash", 64),
    ("rshift", 70),
    ("up", 82),
    ("end", 88),
    // Row 5
    ("lctrl", 5),
    ("lwin", 11),
    ("lalt", 17),
    ("space", 35),
    ("rfn", 53),
    ("rctrl", 59),
    ("left", 77),
    ("down", 83),
    ("right", 89),
];

/// Resolve the frame slot index for a given key name or alias (case-insensitive).
pub fn slot_for_key(name: &str) -> Option<u8> {
    let lower = name.trim().to_ascii_lowercase();
    let canonical = match lower.as_str() {
        "escape" => "esc",
        "-" => "minus",
        "=" => "equal",
        "[" => "lbracket",
        "]" => "rbracket",
        "\\" => "backslash",
        ";" => "semicolon",
        "'" => "quote",
        "," => "comma",
        "." => "dot",
        "/" => "slash",
        "`" | "~" => "backtick",
        "spacebar" => "space",
        "uparrow" | "up_arrow" | "up-arrow" | "arrowup" => "up",
        "downarrow" | "down_arrow" | "down-arrow" | "arrowdown" => "down",
        "leftarrow" | "left_arrow" | "left-arrow" | "arrowleft" => "left",
        "rightarrow" | "right_arrow" | "right-arrow" | "arrowright" => "right",
        "delete" => "del",
        "pageup" => "pgup",
        "pagedown" => "pgdn",
        "return" => "enter",
        "bksp" => "backspace",
        "caps" => "capslock",
        "shift" | "leftshift" => "lshift",
        "rightshift" => "rshift",
        "ctrl" | "leftctrl" => "lctrl",
        "rightctrl" => "rctrl",
        "alt" | "leftalt" => "lalt",
        "win" | "leftwin" | "super" => "lwin",
        "fn" => "rfn",
        other => other,
    };

    KEY_SLOTS
        .iter()
        .find(|&&(k, _)| k.eq_ignore_ascii_case(canonical))
        .map(|&(_, slot)| slot)
}

/// Build a 520-byte direct RGB frame lighting specific named keys.
///
/// Starts from a zeroed frame with the standard Report 6 header, resolves
/// key slots (case-insensitively with alias support), sets RGB channels
/// at `8 + 3 * slot`, and returns an error if any key name is unknown.
pub fn build_key_frame(pairs: &[(&str, (u8, u8, u8))]) -> Result<[u8; FRAME_LEN], String> {
    let mut buf = [0u8; FRAME_LEN];
    buf[0..8].copy_from_slice(&HEADER);

    for &(name, (r, g, b)) in pairs {
        let slot = slot_for_key(name)
            .ok_or_else(|| format!("Unknown key name: {:?}", name))?;
        let offset = 8 + 3 * (slot as usize);
        if offset + 2 < FRAME_LEN - 2 {
            buf[offset] = r;
            buf[offset + 1] = g;
            buf[offset + 2] = b;
        } else {
            return Err(format!("Slot index {} out of frame bounds", slot));
        }
    }

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

    #[test]
    fn test_build_key_frame_exact_offsets() {
        // Test esc at byte 8..11 (slot 0)
        let frame = build_key_frame(&[("esc", (0xFF, 0x11, 0x22))]).expect("build_key_frame failed");
        assert_eq!(frame.len(), FRAME_LEN);
        assert_eq!(&frame[0..8], &HEADER);
        assert_eq!(frame[8], 0xFF);
        assert_eq!(frame[9], 0x11);
        assert_eq!(frame[10], 0x22);

        // Verify other slots remain zero
        assert_eq!(frame[11], 0x00);
        assert_eq!(frame[12], 0x00);
        assert_eq!(frame[13], 0x00);
        assert_eq!(frame[518], 0x00);
        assert_eq!(frame[519], 0x00);

        // Test space at byte 8 + 35*3 = 113..116 (slot 35)
        let space_frame =
            build_key_frame(&[("space", (0xAA, 0xBB, 0xCC))]).expect("build_key_frame failed");
        assert_eq!(&space_frame[0..8], &HEADER);
        assert_eq!(space_frame[8], 0x00); // esc is unlit
        assert_eq!(space_frame[9], 0x00);
        assert_eq!(space_frame[10], 0x00);
        assert_eq!(space_frame[8 + 35 * 3], 0xAA);
        assert_eq!(space_frame[8 + 35 * 3 + 1], 0xBB);
        assert_eq!(space_frame[8 + 35 * 3 + 2], 0xCC);
    }

    #[test]
    fn test_build_key_frame_unknown_key_error() {
        let res = build_key_frame(&[("unknown_key_xyz", (255, 0, 0))]);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.contains("Unknown key name"));
    }

    #[test]
    fn test_slot_for_key_aliases_and_case() {
        // Case-insensitivity
        assert_eq!(slot_for_key("ESC"), Some(0));
        assert_eq!(slot_for_key("Esc"), Some(0));
        assert_eq!(slot_for_key("ESCAPE"), Some(0));
        assert_eq!(slot_for_key("SpaceBar"), Some(35));
        assert_eq!(slot_for_key("UpArrow"), Some(82));

        // Exact aliases
        assert_eq!(slot_for_key("escape"), slot_for_key("esc"));
        assert_eq!(slot_for_key("-"), slot_for_key("minus"));
        assert_eq!(slot_for_key("="), slot_for_key("equal"));
        assert_eq!(slot_for_key("["), slot_for_key("lbracket"));
        assert_eq!(slot_for_key("]"), slot_for_key("rbracket"));
        assert_eq!(slot_for_key("\\"), slot_for_key("backslash"));
        assert_eq!(slot_for_key(";"), slot_for_key("semicolon"));
        assert_eq!(slot_for_key("'"), slot_for_key("quote"));
        assert_eq!(slot_for_key(","), slot_for_key("comma"));
        assert_eq!(slot_for_key("."), slot_for_key("dot"));
        assert_eq!(slot_for_key("/"), slot_for_key("slash"));
        assert_eq!(slot_for_key("`"), slot_for_key("backtick"));
        assert_eq!(slot_for_key("spacebar"), slot_for_key("space"));
        assert_eq!(slot_for_key("uparrow"), slot_for_key("up"));
        assert_eq!(slot_for_key("downarrow"), slot_for_key("down"));
        assert_eq!(slot_for_key("leftarrow"), slot_for_key("left"));
        assert_eq!(slot_for_key("rightarrow"), slot_for_key("right"));
        assert_eq!(slot_for_key("delete"), slot_for_key("del"));
        assert_eq!(slot_for_key("pageup"), slot_for_key("pgup"));
        assert_eq!(slot_for_key("pagedown"), slot_for_key("pgdn"));
        assert_eq!(slot_for_key("return"), slot_for_key("enter"));
        assert_eq!(slot_for_key("bksp"), slot_for_key("backspace"));
        assert_eq!(slot_for_key("caps"), slot_for_key("capslock"));

        // Unknown key returns None
        assert_eq!(slot_for_key("not_a_key"), None);
    }

    #[test]
    fn test_key_slots_count_and_uniqueness() {
        assert!(
            KEY_SLOTS.len() >= 80,
            "KEY_SLOTS must contain at least 80 entries, got {}",
            KEY_SLOTS.len()
        );

        let mut seen_slots = std::collections::HashSet::new();
        for &(name, slot) in KEY_SLOTS {
            assert!(
                (slot as usize) < SLOT_COUNT,
                "Slot {} for key '{}' exceeds max frame slots ({})",
                slot,
                name,
                SLOT_COUNT
            );
            assert!(
                seen_slots.insert(slot),
                "Duplicate slot {} found for key '{}'",
                slot,
                name
            );
        }

        assert_eq!(seen_slots.len(), KEY_SLOTS.len());
    }
}

