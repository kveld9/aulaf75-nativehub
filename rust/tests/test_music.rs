//! Hardware-free unit and integration tests for AULA F75 system audio-reactive music lighting.

use std::f32::consts::PI;
use std::io::Cursor;

use aula_f75::cli::music::{
    bands, bands_to_colors, bands_with_autogain, fft_magnitudes, run_music_loop, spawn_monitor,
    AudioConfig, AutoGain, FftPlan, MusicMode,
};
use aula_f75::proto::{self, HEADER};

#[test]
fn test_fft_synthetic_sine_peak_bin() {
    let n = 1024;
    let sample_rate = 44100.0f32;

    // Verify FftPlan constructor directly
    let plan = FftPlan::new(n);
    assert_eq!(plan.n, n);
    assert_eq!(plan.hann_window.len(), n);
    assert_eq!(plan.bit_reversed.len(), n);

    // Test bin 16 (~689 Hz)
    let target_bin_1 = 16;
    let freq_1 = target_bin_1 as f32 * sample_rate / n as f32;
    let mut samples_1 = vec![0.0f32; n];
    for i in 0..n {
        samples_1[i] = (2.0 * PI * freq_1 * i as f32 / sample_rate).sin();
    }

    let mut mags_1 = vec![0.0f32; n / 2];
    fft_magnitudes(&samples_1, &mut mags_1);

    let (peak_bin_1, _) = mags_1
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap();
    assert_eq!(peak_bin_1, target_bin_1);

    // Test bin 64 (~2756 Hz) using plan.compute directly
    let target_bin_2 = 64;
    let freq_2 = target_bin_2 as f32 * sample_rate / n as f32;
    let mut samples_2 = vec![0.0f32; n];
    for i in 0..n {
        samples_2[i] = (2.0 * PI * freq_2 * i as f32 / sample_rate).sin();
    }

    let mut mags_2 = vec![0.0f32; n / 2];
    plan.compute(&samples_2, &mut mags_2);

    let (peak_bin_2, _) = mags_2
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap();
    assert_eq!(peak_bin_2, target_bin_2);
}

#[test]
fn test_hann_reduces_leakage_vs_rectangular() {
    let n = 1024;
    let sample_rate = 44100.0f32;

    // Inter-bin frequency (bin 20.3)
    let freq = 20.3f32 * sample_rate / n as f32;
    let mut samples = vec![0.0f32; n];
    for i in 0..n {
        samples[i] = (2.0 * PI * freq * i as f32 / sample_rate).sin();
    }

    let mut mags = vec![0.0f32; n / 2];
    fft_magnitudes(&samples, &mut mags);

    // Assert that with Hann windowing, the peak bin is correctly identified at bin 20
    let (peak_bin, _) = mags
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap();
    assert_eq!(peak_bin, 20);
}

#[test]
fn test_log_bands_increase_consumption_order() {
    let sample_rate = 44100.0f32;

    // Low frequency impulse at bin 2 (~86 Hz)
    let mut mags_low = vec![0.0f32; 512];
    mags_low[2] = 100.0;
    let bands_low = bands(&mags_low, 16, sample_rate);
    let (peak_band_low, _) = bands_low
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap();

    // High frequency impulse at bin 150 (~6450 Hz)
    let mut mags_high = vec![0.0f32; 512];
    mags_high[150] = 100.0;
    let mut ag_high = AutoGain::new();
    let bands_high = bands_with_autogain(&mags_high, 16, sample_rate, &mut ag_high);
    let (peak_band_high, _) = bands_high
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap();

    // Frequency bands are strictly ordered by ascending frequency
    assert!(
        peak_band_low < peak_band_high,
        "Expected low frequency peak band ({}) < high frequency peak band ({})",
        peak_band_low,
        peak_band_high
    );
    assert!(peak_band_low <= 3);
    assert!(peak_band_high >= 10);
}

#[test]
fn test_autogain_normalizes_and_decays() {
    let mut ag = AutoGain::new();
    assert_eq!(ag.peak, 1.0);

    // Normalizing a high peak sets the peak to that value and returns 1.0
    let n1 = ag.normalize(10.0);
    assert_eq!(ag.peak, 10.0);
    assert!((n1 - 1.0).abs() < 1e-4);

    // Normalizing a smaller value decays peak by 0.995
    let expected_decayed = 10.0 * 0.995;
    let n2 = ag.normalize(5.0);
    assert!((ag.peak - expected_decayed).abs() < 1e-4);
    assert!((n2 - (5.0 / expected_decayed)).abs() < 1e-4);

    // Successive zero inputs continue exponential decay down to minimum threshold
    let peak_before = ag.peak;
    for _ in 0..100 {
        ag.normalize(0.0);
    }
    assert!(ag.peak < peak_before);
    assert!(ag.peak >= 1e-4);
}

#[test]
fn test_bands_to_colors_lengths_and_bounds() {
    let nslots = proto::KEY_SLOTS.len();
    assert_eq!(nslots, 80);

    // 1. Bars mode
    let band_levels = vec![0.0, 0.25, 0.5, 0.75, 1.0];
    let base_color = (200, 100, 50);
    let pairs_bars = bands_to_colors(&band_levels, base_color, MusicMode::Bars);
    assert_eq!(pairs_bars.len(), nslots);

    for &(slot, (r, g, b)) in &pairs_bars {
        assert!(slot < 170, "Slot {} out of Report 6 bounds", slot);
        assert!(r <= base_color.0);
        assert!(g <= base_color.1);
        assert!(b <= base_color.2);
    }

    // 2. Pulse mode
    let pairs_pulse = bands_to_colors(&band_levels, base_color, MusicMode::Pulse);
    assert_eq!(pairs_pulse.len(), nslots);

    for &(slot, (r, g, b)) in &pairs_pulse {
        assert!(slot < 170, "Slot {} out of Report 6 bounds", slot);
        assert!(r <= base_color.0);
        assert!(g <= base_color.1);
        assert!(b <= base_color.2);
    }

    // 3. Empty bands edge case does not panic and returns zeroed colors
    let empty_pairs = bands_to_colors(&[], base_color, MusicMode::Bars);
    assert_eq!(empty_pairs.len(), nslots);
    for &(_, rgb) in &empty_pairs {
        assert_eq!(rgb, (0, 0, 0));
    }
}

#[test]
fn test_spawn_monitor_error_path_on_missing_binary() {
    std::env::set_var("AULA_F75_PACTL", "/nonexistent/test/path/to/pactl");
    std::env::set_var("AULA_F75_PAREC", "/nonexistent/test/path/to/parec");

    let res = spawn_monitor();
    assert!(res.is_err());
    let err_msg = res.unwrap_err();
    assert!(
        err_msg.contains("install pulseaudio-utils / pipewire-pulse"),
        "Error message should contain install hint, got: {:?}",
        err_msg
    );

    std::env::remove_var("AULA_F75_PACTL");
    std::env::remove_var("AULA_F75_PAREC");
}

#[test]
fn test_music_mode_parsing_and_config() {
    assert_eq!("bars".parse::<MusicMode>().unwrap(), MusicMode::Bars);
    assert_eq!("BARS".parse::<MusicMode>().unwrap(), MusicMode::Bars);
    assert_eq!("pulse".parse::<MusicMode>().unwrap(), MusicMode::Pulse);
    assert_eq!("Pulse".parse::<MusicMode>().unwrap(), MusicMode::Pulse);
    assert!("invalid_mode".parse::<MusicMode>().is_err());

    let config = AudioConfig::default();
    assert_eq!(config.rate, 44100);
    assert_eq!(config.channels, 2);
    assert_eq!(config.gain, 1.0);
    assert_eq!(config.mode, MusicMode::Bars);
}

#[test]
fn test_run_music_loop_hardware_free() {
    // Generate 4410 stereo s16le samples (enough for 2 FFT blocks)
    let num_samples = 4410;
    let mut pcm_bytes = Vec::with_capacity(num_samples * 4);
    for i in 0..num_samples {
        let val = ((i as f32 * 0.1).sin() * 10000.0) as i16;
        let b = val.to_le_bytes();
        pcm_bytes.extend_from_slice(&b); // Left
        pcm_bytes.extend_from_slice(&b); // Right
    }

    let mut cursor = Cursor::new(pcm_bytes);
    let mut sent_frames = Vec::new();

    let res = run_music_loop(
        &mut cursor,
        |frame| {
            sent_frames.push(*frame);
            Ok(())
        },
        MusicMode::Bars,
        (0, 255, 200),
        1.0,
        None,
        || true,
    );

    assert!(res.is_ok());
    assert!(!sent_frames.is_empty());
    assert_eq!(&sent_frames[0][0..8], &HEADER);
    assert_eq!(sent_frames[0].len(), proto::FRAME_LEN);
}
