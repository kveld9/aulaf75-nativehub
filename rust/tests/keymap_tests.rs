//! Hardware-free integration tests for AULA F75 per-key LED mapping and frame generation.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use aula_f75::cli::{cmd_keys, Cli, Commands};
use aula_f75::device::send_key_frame;
use aula_f75::proto::{build_key_frame, slot_for_key, FRAME_LEN, HEADER, KEY_SLOTS, SLOT_COUNT};
use clap::Parser;

#[test]
fn test_key_frame_sets_exact_offsets_esc_and_space() {
    // 1. Esc key is slot 0: bytes 8..11
    let frame_esc = build_key_frame(&[("esc", (0xAA, 0xBB, 0xCC))])
        .expect("build_key_frame failed for esc");
    assert_eq!(frame_esc.len(), FRAME_LEN);
    assert_eq!(&frame_esc[0..8], &HEADER);
    assert_eq!(frame_esc[8], 0xAA);
    assert_eq!(frame_esc[9], 0xBB);
    assert_eq!(frame_esc[10], 0xCC);

    // Ensure subsequent slots are unlit
    assert_eq!(frame_esc[11], 0x00);
    assert_eq!(frame_esc[12], 0x00);
    assert_eq!(frame_esc[13], 0x00);
    assert_eq!(frame_esc[518], 0x00);
    assert_eq!(frame_esc[519], 0x00);

    // 2. Space key is slot 35: bytes (8 + 35*3) .. (8 + 35*3 + 3) = 113..116
    let space_offset = 8 + 35 * 3;
    assert_eq!(space_offset, 113);
    let frame_space = build_key_frame(&[("space", (0x12, 0x34, 0x56))])
        .expect("build_key_frame failed for space");
    assert_eq!(&frame_space[0..8], &HEADER);
    assert_eq!(frame_space[8], 0x00); // esc must be 0
    assert_eq!(frame_space[9], 0x00);
    assert_eq!(frame_space[10], 0x00);
    assert_eq!(frame_space[space_offset], 0x12);
    assert_eq!(frame_space[space_offset + 1], 0x34);
    assert_eq!(frame_space[space_offset + 2], 0x56);

    // 3. Multi-key frame: esc and space set simultaneously
    let frame_multi = build_key_frame(&[("esc", (0x11, 0x22, 0x33)), ("space", (0x44, 0x55, 0x66))])
        .expect("build_key_frame failed for multiple keys");
    assert_eq!(frame_multi[8], 0x11);
    assert_eq!(frame_multi[9], 0x22);
    assert_eq!(frame_multi[10], 0x33);
    assert_eq!(frame_multi[space_offset], 0x44);
    assert_eq!(frame_multi[space_offset + 1], 0x55);
    assert_eq!(frame_multi[space_offset + 2], 0x66);
}

#[test]
fn test_unknown_key_errors() {
    let res = build_key_frame(&[("nonexistent_key_name", (0xFF, 0x00, 0x00))]);
    assert!(res.is_err(), "Expected error for unknown key name");
    let err = res.unwrap_err();
    assert!(
        err.contains("Unknown key name"),
        "Error message should mention unknown key name, got: {}",
        err
    );

    assert_eq!(slot_for_key("nonexistent_key_name"), None);
    assert_eq!(slot_for_key(""), None);
}

#[test]
fn test_aliases_resolve_correctly() {
    // esc = escape
    assert_eq!(slot_for_key("escape"), slot_for_key("esc"));
    assert_eq!(slot_for_key("esc"), Some(0));

    // minus = -
    assert_eq!(slot_for_key("-"), slot_for_key("minus"));
    assert_eq!(slot_for_key("minus"), Some(67));

    // equal = =
    assert_eq!(slot_for_key("="), slot_for_key("equal"));
    assert_eq!(slot_for_key("equal"), Some(73));

    // lbracket = [
    assert_eq!(slot_for_key("["), slot_for_key("lbracket"));
    assert_eq!(slot_for_key("lbracket"), Some(68));

    // rbracket = ]
    assert_eq!(slot_for_key("]"), slot_for_key("rbracket"));
    assert_eq!(slot_for_key("rbracket"), Some(74));

    // backslash = \
    assert_eq!(slot_for_key("\\"), slot_for_key("backslash"));
    assert_eq!(slot_for_key("backslash"), Some(80));

    // semicolon = ;
    assert_eq!(slot_for_key(";"), slot_for_key("semicolon"));
    assert_eq!(slot_for_key("semicolon"), Some(63));

    // quote = '
    assert_eq!(slot_for_key("'"), slot_for_key("quote"));
    assert_eq!(slot_for_key("quote"), Some(69));

    // comma = ,
    assert_eq!(slot_for_key(","), slot_for_key("comma"));
    assert_eq!(slot_for_key("comma"), Some(52));

    // dot = .
    assert_eq!(slot_for_key("."), slot_for_key("dot"));
    assert_eq!(slot_for_key("dot"), Some(58));

    // slash = /
    assert_eq!(slot_for_key("/"), slot_for_key("slash"));
    assert_eq!(slot_for_key("slash"), Some(64));

    // backtick = `
    assert_eq!(slot_for_key("`"), slot_for_key("backtick"));
    assert_eq!(slot_for_key("backtick"), Some(1));

    // space = spacebar
    assert_eq!(slot_for_key("spacebar"), slot_for_key("space"));
    assert_eq!(slot_for_key("space"), Some(35));

    // Arrow keys: up/down/left/right also as uparrow etc.
    assert_eq!(slot_for_key("uparrow"), slot_for_key("up"));
    assert_eq!(slot_for_key("up"), Some(82));

    assert_eq!(slot_for_key("downarrow"), slot_for_key("down"));
    assert_eq!(slot_for_key("down"), Some(83));

    assert_eq!(slot_for_key("leftarrow"), slot_for_key("left"));
    assert_eq!(slot_for_key("left"), Some(77));

    assert_eq!(slot_for_key("rightarrow"), slot_for_key("right"));
    assert_eq!(slot_for_key("right"), Some(89));

    // Extended aliases
    assert_eq!(slot_for_key("delete"), slot_for_key("del"));
    assert_eq!(slot_for_key("del"), Some(85));

    assert_eq!(slot_for_key("pageup"), slot_for_key("pgup"));
    assert_eq!(slot_for_key("pgup"), Some(86));

    assert_eq!(slot_for_key("pagedown"), slot_for_key("pgdn"));
    assert_eq!(slot_for_key("pgdn"), Some(87));

    assert_eq!(slot_for_key("return"), slot_for_key("enter"));
    assert_eq!(slot_for_key("enter"), Some(81));

    assert_eq!(slot_for_key("bksp"), slot_for_key("backspace"));
    assert_eq!(slot_for_key("backspace"), Some(79));

    assert_eq!(slot_for_key("caps"), slot_for_key("capslock"));
    assert_eq!(slot_for_key("capslock"), Some(3));

    // Case-insensitivity checks
    assert_eq!(slot_for_key("ESC"), Some(0));
    assert_eq!(slot_for_key("EsCaPe"), Some(0));
    assert_eq!(slot_for_key("SPACEBAR"), Some(35));
    assert_eq!(slot_for_key("UpArrow"), Some(82));
    assert_eq!(slot_for_key("F1"), Some(12));
    assert_eq!(slot_for_key("F12"), Some(78));
}

#[test]
fn test_keys_list_count_at_least_80() {
    assert!(
        KEY_SLOTS.len() >= 80,
        "Expected at least 80 key entries in KEY_SLOTS, got {}",
        KEY_SLOTS.len()
    );
    assert_eq!(cmd_keys(), 0);
}

#[test]
fn test_all_slot_values_under_170_and_unique() {
    let mut seen_slots = HashSet::new();

    for &(name, slot) in KEY_SLOTS {
        assert!(
            (slot as usize) < SLOT_COUNT,
            "Slot value {} for key '{}' must be less than 170 (SLOT_COUNT)",
            slot,
            name
        );
        assert!(
            seen_slots.insert(slot),
            "Duplicate slot {} found for key '{}' (uniqueness violation across table)",
            slot,
            name
        );
    }

    assert_eq!(
        seen_slots.len(),
        KEY_SLOTS.len(),
        "All slots in KEY_SLOTS must be distinct"
    );
    assert!(seen_slots.len() >= 80);
}

#[test]
fn test_device_send_key_frame_hardware_free() {
    let mut sent_frames = Vec::new();
    let is_running = Arc::new(AtomicBool::new(true));
    let is_running_clone = Arc::clone(&is_running);

    let res = send_key_frame(
        &[("esc", (0x99, 0x88, 0x77))],
        Some(0.001),
        |frame| {
            sent_frames.push(*frame);
            is_running_clone.store(false, Ordering::SeqCst);
            Ok(())
        },
        |_| {},
        || is_running.load(Ordering::SeqCst),
    );

    assert!(res.is_ok());
    assert!(!sent_frames.is_empty());
    assert_eq!(sent_frames[0][8], 0x99);
    assert_eq!(sent_frames[0][9], 0x88);
    assert_eq!(sent_frames[0][10], 0x77);
}

#[test]
fn test_cli_key_command_parsing() {
    let cli = Cli::try_parse_from(["aula-f75-rs", "key", "spacebar", "00FF00", "--seconds", "5"])
        .expect("Failed to parse 'key' subcommand");

    match cli.command {
        Commands::Key { name, color, seconds } => {
            assert_eq!(name, "spacebar");
            assert_eq!(color, "00FF00");
            assert_eq!(seconds, Some(5.0));
        }
        _ => panic!("Expected Commands::Key variant"),
    }
}
