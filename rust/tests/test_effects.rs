//! Hardware-free unit and integration tests for AULA F75 host-rendered RGB effects engine.

use aula_f75::effects::{apply_brightness, render, Effect, RGB};
use aula_f75::proto::{all_key_slots, build_slot_frame, FRAME_LEN, HEADER};

#[test]
fn test_hsv_to_rgb_known_points() {
    // Red at h=0 and h=360
    let red_0 = RGB::hsv_to_rgb(0.0, 1.0, 1.0);
    assert_eq!(red_0, RGB(255, 0, 0));

    let red_360 = RGB::hsv_to_rgb(360.0, 1.0, 1.0);
    assert_eq!(red_360, RGB(255, 0, 0));

    // Green at h=120
    let green_120 = RGB::hsv_to_rgb(120.0, 1.0, 1.0);
    assert_eq!(green_120, RGB(0, 255, 0));

    // Blue at h=240
    let blue_240 = RGB::hsv_to_rgb(240.0, 1.0, 1.0);
    assert_eq!(blue_240, RGB(0, 0, 255));
}

#[test]
fn test_rainbow_changes_over_time() {
    let effect = Effect::Rainbow { period_s: 5.0 };
    let slots = [0, 12, 35];
    let frame_0 = render(&effect, 0.0, &slots);
    let frame_1 = render(&effect, 1.0, &slots);

    assert_eq!(frame_0.len(), 3);
    assert_eq!(frame_1.len(), 3);
    // At t=0.0 vs t=1.0, color triplet changes
    assert_ne!(frame_0[0].1, frame_1[0].1);
}

#[test]
fn test_wave_varies_by_slot() {
    let effect = Effect::Wave { period_s: 3.0 };
    let slots = [0, 35];
    let rendered = render(&effect, 0.0, &slots);

    assert_eq!(rendered.len(), 2);
    assert_eq!(rendered[0].0, 0);
    assert_eq!(rendered[1].0, 35);
    // At t=0.0, slot 0 and slot 35 have different hues
    assert_ne!(rendered[0].1, rendered[1].1);
}

#[test]
fn test_breathe_oscillates() {
    let period = 4.0;
    let effect = Effect::Breathe {
        color: RGB(200, 100, 50),
        period_s: period,
    };
    let slots = [0];

    let t0 = render(&effect, 0.0, &slots);
    let t_max = render(&effect, period / 4.0, &slots);
    let t_min = render(&effect, period * 3.0 / 4.0, &slots);

    // At t_max (sin = 1.0, factor = 1.0), should equal original color
    assert_eq!(t_max[0].1, (200, 100, 50));
    // At t_min (sin = -1.0, factor = 0.0), should be black (0, 0, 0)
    assert_eq!(t_min[0].1, (0, 0, 0));
    // At t0 (sin = 0.0, factor = 0.5), channels are halved
    assert_eq!(t0[0].1, (100, 50, 25));
}

#[test]
fn test_stars_deterministic() {
    let effect = Effect::Stars { density: 0.15 };
    let slots = all_key_slots();
    let run1 = render(&effect, 2.5, &slots);
    let run2 = render(&effect, 2.5, &slots);

    // Deterministic hash produces exact identical pairs
    assert_eq!(run1, run2);
}

#[test]
fn test_apply_brightness_scaling() {
    let pairs = [(0, (200, 200, 200)), (1, (100, 50, 0))];

    // Brightness 0.5 halves 200 to 100
    let half = apply_brightness(&pairs, 0.5);
    assert_eq!(half[0], (0, (100, 100, 100)));
    assert_eq!(half[1], (1, (50, 25, 0)));

    // Brightness 0.0 zeros all channels
    let zero = apply_brightness(&pairs, 0.0);
    assert_eq!(zero[0], (0, (0, 0, 0)));
    assert_eq!(zero[1], (1, (0, 0, 0)));

    // Brightness 1.0 preserves original channels
    let full = apply_brightness(&pairs, 1.0);
    assert_eq!(full, pairs.to_vec());
}

#[test]
fn test_effect_from_name_valid_and_unknown() {
    // Valid named effects without requiring color
    assert_eq!(
        Effect::from_name("rainbow", None).unwrap(),
        Effect::Rainbow { period_s: 5.0 }
    );
    assert_eq!(
        Effect::from_name("RAINBOW", None).unwrap(),
        Effect::Rainbow { period_s: 5.0 }
    );
    assert_eq!(
        Effect::from_name("wave", None).unwrap(),
        Effect::Wave { period_s: 3.0 }
    );
    assert_eq!(
        Effect::from_name("stars", None).unwrap(),
        Effect::Stars { density: 0.15 }
    );
    assert_eq!(Effect::from_name("off", None).unwrap(), Effect::Off);

    // Solid requires color
    assert!(Effect::from_name("solid", None).is_err());
    let solid_res = Effect::from_name("solid", Some(RGB(255, 0, 128))).unwrap();
    assert_eq!(solid_res, Effect::Solid(RGB(255, 0, 128)));

    // Breathe requires color
    assert!(Effect::from_name("breathe", None).is_err());
    let breathe_res = Effect::from_name("breathe", Some(RGB(10, 20, 30))).unwrap();
    assert_eq!(
        breathe_res,
        Effect::Breathe {
            color: RGB(10, 20, 30),
            period_s: 4.0
        }
    );

    // Unknown name produces descriptive error
    let err = Effect::from_name("invalid_fx", None);
    assert!(err.is_err());
    assert!(err.unwrap_err().contains("Unknown effect name"));
}

#[test]
fn test_slot_frame_exact_offsets() {
    let pairs = [(0, (0xFF, 0x11, 0x22)), (35, (0xAA, 0xBB, 0xCC))];
    let frame = build_slot_frame(&pairs).expect("build_slot_frame failed");

    assert_eq!(frame.len(), FRAME_LEN);
    assert_eq!(&frame[0..8], &HEADER);

    // Slot 0 at byte 8..11
    assert_eq!(frame[8], 0xFF);
    assert_eq!(frame[9], 0x11);
    assert_eq!(frame[10], 0x22);

    // Slot 1 is unlit (0)
    assert_eq!(frame[11], 0x00);
    assert_eq!(frame[12], 0x00);
    assert_eq!(frame[13], 0x00);

    // Slot 35 at byte 8 + 35*3 = 113..116
    assert_eq!(frame[8 + 35 * 3], 0xAA);
    assert_eq!(frame[8 + 35 * 3 + 1], 0xBB);
    assert_eq!(frame[8 + 35 * 3 + 2], 0xCC);

    // Final padding bytes 518..520 remain zero
    assert_eq!(frame[518], 0x00);
    assert_eq!(frame[519], 0x00);
}

#[test]
fn test_effect_off_renders_empty() {
    let slots = all_key_slots();
    let rendered = render(&Effect::Off, 1.0, &slots);
    assert!(rendered.is_empty());
}
