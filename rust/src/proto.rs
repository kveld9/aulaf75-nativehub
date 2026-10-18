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

const KEY_ALIASES: &[(&str, &str)] = &[
    ("escape", "esc"),
    ("-", "minus"),
    ("=", "equal"),
    ("[", "lbracket"),
    ("]", "rbracket"),
    ("\\", "backslash"),
    (";", "semicolon"),
    ("'", "quote"),
    (",", "comma"),
    (".", "dot"),
    ("/", "slash"),
    ("`", "backtick"),
    ("~", "backtick"),
    ("spacebar", "space"),
    ("uparrow", "up"),
    ("up_arrow", "up"),
    ("up-arrow", "up"),
    ("arrowup", "up"),
    ("downarrow", "down"),
    ("down_arrow", "down"),
    ("down-arrow", "down"),
    ("arrowdown", "down"),
    ("leftarrow", "left"),
    ("left_arrow", "left"),
    ("left-arrow", "left"),
    ("arrowleft", "left"),
    ("rightarrow", "right"),
    ("right_arrow", "right"),
    ("right-arrow", "right"),
    ("arrowright", "right"),
    ("delete", "del"),
    ("pageup", "pgup"),
    ("pagedown", "pgdn"),
    ("return", "enter"),
    ("bksp", "backspace"),
    ("caps", "capslock"),
    ("shift", "lshift"),
    ("leftshift", "lshift"),
    ("rightshift", "rshift"),
    ("ctrl", "lctrl"),
    ("leftctrl", "lctrl"),
    ("rightctrl", "rctrl"),
    ("alt", "lalt"),
    ("leftalt", "lalt"),
    ("win", "lwin"),
    ("leftwin", "lwin"),
    ("super", "lwin"),
    ("fn", "rfn"),
];

fn resolve_key_alias(alias: &str) -> &str {
    for &(k, canonical) in KEY_ALIASES {
        if k == alias {
            return canonical;
        }
    }
    alias
}

/// Resolve the frame slot index for a given key name or alias (case-insensitive).
pub fn slot_for_key(name: &str) -> Option<u8> {
    let lower = name.trim().to_ascii_lowercase();
    let canonical = resolve_key_alias(&lower);

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

/// Build a 520-byte direct RGB frame lighting specific key slot indices.
///
/// Starts from a zeroed frame with the standard Report 6 header, sets RGB
/// channels at `8 + 3 * slot`, and verifies that all slot offsets are within frame bounds.
pub fn build_slot_frame(pairs: &[(u8, (u8, u8, u8))]) -> Result<[u8; FRAME_LEN], String> {
    let mut buf = [0u8; FRAME_LEN];
    buf[0..8].copy_from_slice(&HEADER);

    for &(slot, (r, g, b)) in pairs {
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

/// Return slot numbers of all mapped physical keys in `KEY_SLOTS`.
pub fn all_key_slots() -> Vec<u8> {
    KEY_SLOTS.iter().map(|&(_, slot)| slot).collect()
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

/// Read a HID feature report using the Linux HIDIOCGFEATURE ioctl.
///
/// # Safety Rules
/// - HARD SAFETY RULE: Access to Report ID 5 is strictly prohibited to prevent
///   triggering the ISP bootloader / firmware mailbox states.
/// - Requires `dir = 3` (READ|WRITE) ioctl direction on Linux hidraw.
/// - The first byte of `buf` is populated with `report_id`.
pub fn hid_get_feature(fd: RawFd, report_id: u8, buf: &mut [u8]) -> Result<usize, io::Error> {
    if buf.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Feature report buffer cannot be empty",
        ));
    }

    if report_id == 5 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Safety violation: access to report ID 5 is strictly prohibited",
        ));
    }

    buf[0] = report_id;
    let length = buf.len();
    // HIDIOCGFEATURE(len) encoding:
    // dir = 3 (READ|WRITE), size = length, type = 0x48 ('H'), nr = 0x07
    let req = ((3u64 << 30) | ((length as u64) << 16) | (0x48u64 << 8) | 0x07u64) as libc::c_ulong;

    let ret = unsafe { libc::ioctl(fd, req as libc::c_ulong, buf.as_mut_ptr()) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(ret as usize)
}

/// Source of multifunction rotary knob behavior observed by the host OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnobModeSource {
    /// Consumer Control media keys (Volume Increment, Volume Decrement, Mute).
    ConsumerControl,
    /// Generic Desktop rotary dial or wheel usage.
    GenericDesktopDial,
    /// Vendor-defined HID usage.
    VendorCustom(u16),
    /// Undetermined behavior requiring live USB capture.
    Undetermined,
}

/// Structured report of observed multifunction knob behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnobReport {
    pub source: KnobModeSource,
    pub description: String,
    pub details: Vec<String>,
}

/// Parse a raw HID Report Descriptor to detect rotary knob / volume encoder usages.
///
/// Scans for:
/// 1. Consumer Control usage page (0x05, 0x0C) with volume usages:
///    Volume Increment (0x09, 0xE9), Volume Decrement (0x09, 0xEA), or Mute (0x09, 0xE2).
/// 2. Generic Desktop usage page (0x05, 0x01) with Dial (0x09, 0x37) or Wheel (0x09, 0x38).
///
/// Returns `KnobModeSource::Undetermined` with description "undetermined, needs live USB capture"
/// if the descriptor is empty, malformed, or contains no unambiguous rotary knob usages.
#[derive(Default)]
struct KnobScanState {
    has_consumer_page: bool,
    has_volume_inc: bool,
    has_volume_dec: bool,
    has_mute: bool,
    has_generic_desktop_page: bool,
    has_dial: bool,
    has_wheel: bool,
    current_page: Option<u16>,
}

fn skip_long_item(desc: &[u8], i: usize) -> Option<usize> {
    if desc[i] != 0xFE {
        return None;
    }
    if i + 2 < desc.len() {
        let data_len = desc[i + 1] as usize;
        Some(i + 3 + data_len)
    } else {
        Some(desc.len())
    }
}

fn decode_short_item_len(prefix: u8) -> usize {
    match prefix & 0x03 {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 4,
        _ => 0,
    }
}

fn parse_item_u16(data: &[u8]) -> u16 {
    match data.len() {
        1 => data[0] as u16,
        2 => (data[0] as u16) | ((data[1] as u16) << 8),
        _ => 0,
    }
}

fn handle_global_item(state: &mut KnobScanState, b_type: u8, b_tag: u8, data: &[u8]) {
    if b_type != 1 || b_tag != 0 {
        return;
    }
    let page = parse_item_u16(data);
    state.current_page = Some(page);
    if page == 0x000C {
        state.has_consumer_page = true;
    } else if page == 0x0001 {
        state.has_generic_desktop_page = true;
    }
}

fn is_consumer_context(state: &KnobScanState) -> bool {
    state.current_page == Some(0x000C) || (state.current_page.is_none() && state.has_consumer_page)
}

fn is_generic_desktop_context(state: &KnobScanState) -> bool {
    state.current_page == Some(0x0001) || (state.current_page.is_none() && state.has_generic_desktop_page)
}

fn record_consumer_usage(state: &mut KnobScanState, usage: u16) {
    match usage {
        0x00E9 => state.has_volume_inc = true,
        0x00EA => state.has_volume_dec = true,
        0x00E2 => state.has_mute = true,
        _ => {}
    }
}

fn record_generic_desktop_usage(state: &mut KnobScanState, usage: u16) {
    match usage {
        0x0037 => state.has_dial = true,
        0x0038 => state.has_wheel = true,
        _ => {}
    }
}

fn handle_local_item(state: &mut KnobScanState, b_type: u8, b_tag: u8, data: &[u8]) {
    if b_type != 2 || b_tag != 0 {
        return;
    }
    let usage = parse_item_u16(data);
    if is_consumer_context(state) {
        record_consumer_usage(state, usage);
    } else if is_generic_desktop_context(state) {
        record_generic_desktop_usage(state, usage);
    }
}

fn scan_descriptor_items(desc: &[u8]) -> KnobScanState {
    let mut state = KnobScanState::default();
    let mut i = 0;

    while i < desc.len() {
        if let Some(next_i) = skip_long_item(desc, i) {
            i = next_i;
            continue;
        }

        let prefix = desc[i];
        let data_bytes = decode_short_item_len(prefix);
        if i + 1 + data_bytes > desc.len() {
            break;
        }

        let item_data = &desc[i + 1..i + 1 + data_bytes];
        let b_type = (prefix >> 2) & 0x03;
        let b_tag = (prefix >> 4) & 0x0F;

        handle_global_item(&mut state, b_type, b_tag, item_data);
        handle_local_item(&mut state, b_type, b_tag, item_data);

        i += 1 + data_bytes;
    }

    state
}

fn build_consumer_report(state: &KnobScanState) -> KnobReport {
    let mut details = vec!["Usage Page: Consumer Control (0x0C)".to_string()];
    if state.has_volume_inc {
        details.push("Usage: Volume Increment (0x00E9)".to_string());
    }
    if state.has_volume_dec {
        details.push("Usage: Volume Decrement (0x00EA)".to_string());
    }
    if state.has_mute {
        details.push("Usage: Mute (0x00E2)".to_string());
    }
    KnobReport {
        source: KnobModeSource::ConsumerControl,
        description: "Consumer Control (Volume Up / Volume Down / Mute)".to_string(),
        details,
    }
}

fn build_generic_desktop_report(state: &KnobScanState) -> KnobReport {
    let mut details = vec!["Usage Page: Generic Desktop (0x01)".to_string()];
    if state.has_dial {
        details.push("Usage: Dial (0x0037)".to_string());
    }
    if state.has_wheel {
        details.push("Usage: Wheel (0x0038)".to_string());
    }
    KnobReport {
        source: KnobModeSource::GenericDesktopDial,
        description: "Generic Desktop Dial/Wheel (Relative Rotary Encoder)".to_string(),
        details,
    }
}

fn build_undetermined_report() -> KnobReport {
    KnobReport {
        source: KnobModeSource::Undetermined,
        description: "undetermined, needs live USB capture".to_string(),
        details: vec![
            "Report descriptor contains no unambiguous Consumer Control volume or Dial/Wheel usages"
                .to_string(),
        ],
    }
}

fn has_consumer_volume(state: &KnobScanState) -> bool {
    state.has_consumer_page && (state.has_volume_inc || state.has_volume_dec || state.has_mute)
}

fn has_generic_dial(state: &KnobScanState) -> bool {
    state.has_generic_desktop_page && (state.has_dial || state.has_wheel)
}

fn build_knob_report(state: &KnobScanState) -> KnobReport {
    if has_consumer_volume(state) {
        build_consumer_report(state)
    } else if has_generic_dial(state) {
        build_generic_desktop_report(state)
    } else {
        build_undetermined_report()
    }
}

/// Parse a raw HID Report Descriptor to detect rotary knob / volume encoder usages.
///
/// Scans for:
/// 1. Consumer Control usage page (0x05, 0x0C) with volume usages:
///    Volume Increment (0x09, 0xE9), Volume Decrement (0x09, 0xEA), or Mute (0x09, 0xE2).
/// 2. Generic Desktop usage page (0x05, 0x01) with Dial (0x09, 0x37) or Wheel (0x09, 0x38).
///
/// Returns `KnobModeSource::Undetermined` with description "undetermined, needs live USB capture"
/// if the descriptor is empty, malformed, or contains no unambiguous rotary knob usages.
pub fn parse_knob_from_report_descriptor(desc: &[u8]) -> KnobReport {
    if desc.is_empty() {
        return KnobReport {
            source: KnobModeSource::Undetermined,
            description: "undetermined, needs live USB capture".to_string(),
            details: vec!["Empty report descriptor".to_string()],
        };
    }

    let state = scan_descriptor_items(desc);
    build_knob_report(&state)
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

    #[test]
    fn test_hid_get_feature_safety_guard_rejects_report_5() {
        let mut buf = [0u8; 64];
        let res = hid_get_feature(-1, 5, &mut buf);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("report ID 5"));
    }

    #[test]
    fn test_hid_get_feature_empty_buffer_rejects() {
        let res = hid_get_feature(-1, 6, &mut []);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_parse_knob_consumer_control() {
        // Usage Page (Consumer 0x0C), Usage (Volume Increment 0xE9), Usage (Volume Decrement 0xEA)
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
    }

    #[test]
    fn test_parse_knob_generic_desktop_dial() {
        // Usage Page (Generic Desktop 0x01), Usage (Dial 0x37)
        let desc = [
            0x05, 0x01, // Usage Page (Generic Desktop)
            0x09, 0x37, // Usage (Dial)
        ];
        let report = parse_knob_from_report_descriptor(&desc);
        assert_eq!(report.source, KnobModeSource::GenericDesktopDial);
        assert!(report.description.contains("Dial"));
    }

    #[test]
    fn test_parse_knob_empty_and_undetermined() {
        let empty_report = parse_knob_from_report_descriptor(&[]);
        assert_eq!(empty_report.source, KnobModeSource::Undetermined);
        assert_eq!(empty_report.description, "undetermined, needs live USB capture");

        // Keyboard-only descriptor (Usage Page Generic Desktop 0x01, Usage Keyboard 0x06)
        let kbd_desc = [0x05, 0x01, 0x09, 0x06];
        let kbd_report = parse_knob_from_report_descriptor(&kbd_desc);
        assert_eq!(kbd_report.source, KnobModeSource::Undetermined);
        assert_eq!(kbd_report.description, "undetermined, needs live USB capture");
    }
}

