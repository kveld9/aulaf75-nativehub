//! Hardware-free integration tests for AULA F75 GUI state logic and channels.

use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use aula_f75::cli::profile::{save_profile, set_default_profile, Profile};
use aula_f75::effects::Effect;
use aula_f75::gui::state::{
    initial_worker_message_for_profile, resolve_initial_worker_message, resolve_startup_profile,
};
use aula_f75::gui::{
    parse_color_hex, run_worker_reconnect_loop, run_worker_stream, AulaF75App, FakeServiceRunner,
    GuiLightingMode, GuiState, WorkerMessage, WorkerStatus,
};
use aula_f75::proto::HEADER;

#[test]
fn test_hex_parsing_shared_with_gui() {
    // Valid 6-digit hex representations
    assert_eq!(parse_color_hex("ff0000").unwrap(), (255, 0, 0));
    assert_eq!(parse_color_hex("#00ff00").unwrap(), (0, 255, 0));
    assert_eq!(parse_color_hex("#0000FF").unwrap(), (0, 0, 255));
    assert_eq!(parse_color_hex("1A2B3C").unwrap(), (0x1A, 0x2B, 0x3C));
    assert_eq!(parse_color_hex("  #123456  ").unwrap(), (0x12, 0x34, 0x56));

    // Invalid inputs
    assert!(parse_color_hex("").is_err());
    assert!(parse_color_hex("#").is_err());
    assert!(parse_color_hex("12345").is_err());
    assert!(parse_color_hex("1234567").is_err());
    assert!(parse_color_hex("12345g").is_err());
    assert!(parse_color_hex("#xyz123").is_err());
}

#[test]
fn test_app_state_transitions_solid_to_off() {
    let mut state = GuiState::new();
    assert_eq!(state.mode, GuiLightingMode::Off);

    // Transition from Off to Solid
    let cmd = state.transition_to_solid((100, 150, 200));
    assert_eq!(state.mode, GuiLightingMode::Solid);
    assert_eq!(state.rgb, (100, 150, 200));
    assert_eq!(cmd, WorkerMessage::SetColor((100, 150, 200)));

    // Transition from Solid to Effect
    let rainbow = Effect::Rainbow { period_s: 5.0 };
    let cmd = state.transition_to_effect(rainbow.clone(), 0.75);
    assert_eq!(state.mode, GuiLightingMode::Effect(rainbow.clone()));
    assert_eq!(
        cmd,
        WorkerMessage::SetEffect {
            effect: rainbow,
            brightness: 0.75
        }
    );

    // Transition from Effect to Off
    let cmd = state.transition_to_off();
    assert_eq!(state.mode, GuiLightingMode::Off);
    assert_eq!(cmd, WorkerMessage::Off);

    // Transition from Off to Stop
    let cmd = state.transition_to_stop();
    assert_eq!(state.mode, GuiLightingMode::Off);
    assert_eq!(cmd, WorkerMessage::Stop);
}

#[test]
fn test_channel_message_shapes() {
    let (tx, rx) = mpsc::channel();

    // Verify SetColor message variant shape
    tx.send(WorkerMessage::SetColor((0xAA, 0xBB, 0xCC))).unwrap();
    let received = rx.recv().unwrap();
    match received {
        WorkerMessage::SetColor(rgb) => {
            assert_eq!(rgb, (0xAA, 0xBB, 0xCC));
        }
        _ => panic!("Expected WorkerMessage::SetColor"),
    }

    // Verify SetEffect message variant shape
    let wave = Effect::Wave { period_s: 3.0 };
    tx.send(WorkerMessage::SetEffect {
        effect: wave.clone(),
        brightness: 0.9,
    })
    .unwrap();
    let received = rx.recv().unwrap();
    match received {
        WorkerMessage::SetEffect { effect, brightness } => {
            assert_eq!(effect, wave);
            assert_eq!(brightness, 0.9);
        }
        _ => panic!("Expected WorkerMessage::SetEffect"),
    }

    // Verify Off message variant shape
    tx.send(WorkerMessage::Off).unwrap();
    assert_eq!(rx.recv().unwrap(), WorkerMessage::Off);

    // Verify Stop message variant shape
    tx.send(WorkerMessage::Stop).unwrap();
    assert_eq!(rx.recv().unwrap(), WorkerMessage::Stop);
}

#[test]
fn test_worker_status_channel_shapes() {
    let (tx, rx) = mpsc::channel();

    tx.send(WorkerStatus::Running(PathBuf::from("/dev/hidraw1"))).unwrap();
    match rx.recv().unwrap() {
        WorkerStatus::Running(p) => assert_eq!(p, PathBuf::from("/dev/hidraw1")),
        _ => panic!("Expected WorkerStatus::Running"),
    }

    tx.send(WorkerStatus::Error("Permission denied".to_string())).unwrap();
    match rx.recv().unwrap() {
        WorkerStatus::Error(msg) => assert_eq!(msg, "Permission denied"),
        _ => panic!("Expected WorkerStatus::Error"),
    }

    tx.send(WorkerStatus::Connecting).unwrap();
    assert_eq!(rx.recv().unwrap(), WorkerStatus::Connecting);

    tx.send(WorkerStatus::Connected(PathBuf::from("/dev/hidraw1"))).unwrap();
    match rx.recv().unwrap() {
        WorkerStatus::Connected(p) => assert_eq!(p, PathBuf::from("/dev/hidraw1")),
        _ => panic!("Expected WorkerStatus::Connected"),
    }

    tx.send(WorkerStatus::Disconnected).unwrap();
    assert_eq!(rx.recv().unwrap(), WorkerStatus::Disconnected);

    tx.send(WorkerStatus::Stopped).unwrap();
    assert_eq!(rx.recv().unwrap(), WorkerStatus::Stopped);
}

#[test]
fn test_worker_stream_lifecycle_off() {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (status_tx, status_rx) = mpsc::channel();

    let mut sent_frames = Vec::new();

    // Starting worker stream directly with Off should send an all-zero frame and stop immediately
    run_worker_stream(
        cmd_rx,
        status_tx,
        WorkerMessage::Off,
        Duration::from_millis(10),
        |frame| {
            sent_frames.push(*frame);
            Ok(())
        },
        |_| {},
    );

    drop(cmd_tx);

    assert_eq!(sent_frames.len(), 1);
    assert_eq!(&sent_frames[0][0..8], &HEADER);
    // All RGB slots in off frame are zero
    for b in &sent_frames[0][8..518] {
        assert_eq!(*b, 0);
    }

    // Worker should report Stopped status
    let status = status_rx.recv().unwrap();
    assert_eq!(status, WorkerStatus::Stopped);
}

#[test]
fn test_worker_stream_color_update_and_stop() {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (status_tx, status_rx) = mpsc::channel();

    let mut sent_colors = Vec::new();

    // Spawn worker in a background thread to test asynchronous updates
    let handle = std::thread::spawn(move || {
        run_worker_stream(
            cmd_rx,
            status_tx,
            WorkerMessage::SetColor((10, 20, 30)),
            Duration::from_millis(50),
            |frame| {
                // Read slot 0 RGB values
                sent_colors.push((frame[8], frame[9], frame[10]));
                Ok(())
            },
            |_| {},
        );
        sent_colors
    });

    // Allow initial frame to be processed
    std::thread::sleep(Duration::from_millis(20));

    // Send updated color
    cmd_tx.send(WorkerMessage::SetColor((40, 50, 60))).unwrap();
    std::thread::sleep(Duration::from_millis(20));

    // Send Stop command to cleanly terminate worker
    cmd_tx.send(WorkerMessage::Stop).unwrap();

    let recorded = handle.join().expect("Worker thread panicked");

    assert!(!recorded.is_empty());
    assert_eq!(recorded[0], (10, 20, 30));
    assert!(recorded.contains(&(40, 50, 60)));

    let status = status_rx.recv().unwrap();
    assert_eq!(status, WorkerStatus::Stopped);
}

#[test]
fn test_worker_stream_transient_retry_and_fatal_error() {
    let (_cmd_tx, cmd_rx) = mpsc::channel();
    let (status_tx, status_rx) = mpsc::channel();

    let mut attempts = 0;

    // Simulate persistent EPIPE error reaching the 10-retry threshold
    run_worker_stream(
        cmd_rx,
        status_tx,
        WorkerMessage::SetColor((255, 0, 0)),
        Duration::from_millis(5),
        |_frame| {
            attempts += 1;
            Err(io::Error::from_raw_os_error(libc::EPIPE))
        },
        |_| {},
    );

    assert_eq!(attempts, 10);

    let mut errors = Vec::new();
    while let Ok(status) = status_rx.try_recv() {
        if let WorkerStatus::Error(err) = status {
            errors.push(err);
        }
    }

    assert!(!errors.is_empty());
    assert!(errors[0].contains("after 10 retries"));
}

#[test]
fn test_worker_stream_effect_streaming() {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (status_tx, status_rx) = mpsc::channel();

    let mut sent_frames = Vec::new();

    let handle = std::thread::spawn(move || {
        run_worker_stream(
            cmd_rx,
            status_tx,
            WorkerMessage::SetEffect {
                effect: Effect::Rainbow { period_s: 5.0 },
                brightness: 0.8,
            },
            Duration::from_millis(30),
            |frame| {
                sent_frames.push(*frame);
                Ok(())
            },
            |_| {},
        );
        sent_frames
    });

    std::thread::sleep(Duration::from_millis(50));
    cmd_tx.send(WorkerMessage::Stop).unwrap();

    let recorded = handle.join().expect("Worker thread panicked");
    assert!(!recorded.is_empty());
    assert_eq!(&recorded[0][0..8], &HEADER);

    let status = status_rx.recv().unwrap();
    assert_eq!(status, WorkerStatus::Stopped);
}

#[test]
fn test_gui_app_startup_never_blocks_or_queries_hardware() {
    let start = Instant::now();
    let app = AulaF75App::default();
    let elapsed = start.elapsed();

    // Startup must complete in < 50ms without performing USB or sysfs discovery
    assert!(elapsed < Duration::from_millis(50));
    assert_eq!(app.control_node_path, None);
    assert_eq!(app.status_text, "Connecting...");
    assert!(!app.is_worker_running());
    assert!(app.last_error.is_none());
}

#[test]
fn test_poll_worker_status_drives_cycles_nonblocking() {
    let mut app = AulaF75App::default();
    let (tx, rx) = mpsc::channel();
    app.status_rx = Some(rx);

    // Send a running event
    tx.send(WorkerStatus::Running(PathBuf::from("/dev/hidraw1"))).unwrap();

    let start = Instant::now();
    for i in 0..60 {
        app.poll_worker_status();
        if i == 30 {
            tx.send(WorkerStatus::MusicBands(vec![0.5; 16])).unwrap();
        }
    }
    let elapsed = start.elapsed();

    // 60 poll cycles must execute virtually instantaneously (well under 1s)
    assert!(elapsed < Duration::from_millis(100));
    assert_eq!(app.control_node_path, Some(PathBuf::from("/dev/hidraw1")));
    assert!(app.status_text.contains("Streaming active on /dev/hidraw1"));
    assert_eq!(app.last_music_bands.len(), 16);
}

#[test]
fn test_stop_worker_never_joins_or_blocks_ui_thread() {
    let mut app = AulaF75App::default();
    let (tx_cmd, rx_cmd) = mpsc::channel();
    app.worker_tx = Some(tx_cmd);

    // Simulate a background worker that would sleep or hang if joined
    let worker_handle = std::thread::spawn(move || {
        let _ = rx_cmd.recv_timeout(Duration::from_secs(5));
    });
    app.worker_handle = Some(worker_handle);

    let start = Instant::now();
    app.stop_worker();
    let elapsed = start.elapsed();

    // stop_worker must return immediately without blocking/joining on the worker thread
    assert!(elapsed < Duration::from_millis(50));
    assert!(!app.is_worker_running());
    assert_eq!(app.state.mode, GuiLightingMode::Off);
}

#[test]
fn test_worker_status_stopped_never_joins_stuck_worker() {
    let mut app = AulaF75App::default();
    let (status_tx, status_rx) = mpsc::channel();
    app.status_rx = Some(status_rx);

    let (tx_cmd, rx_cmd) = mpsc::channel();
    app.worker_tx = Some(tx_cmd);

    // Worker that sleeps if joined
    let worker_handle = std::thread::spawn(move || {
        let _ = rx_cmd.recv_timeout(Duration::from_secs(5));
    });
    app.worker_handle = Some(worker_handle);

    // Signal Stopped via status channel
    status_tx.send(WorkerStatus::Stopped).unwrap();

    let start = Instant::now();
    app.poll_worker_status();
    let elapsed = start.elapsed();

    // Must process Stopped and clear worker without blocking or joining
    assert!(elapsed < Duration::from_millis(50));
    assert!(!app.is_worker_running());
    assert_eq!(app.status_text, "Disconnected");
}

#[test]
fn test_worker_stream_transient_error_backoff_sleep() {
    let (_cmd_tx, cmd_rx) = mpsc::channel();
    let (status_tx, status_rx) = mpsc::channel();

    let mut backoff_sleeps = Vec::new();

    run_worker_stream(
        cmd_rx,
        status_tx,
        WorkerMessage::SetColor((255, 0, 0)),
        Duration::from_millis(5),
        |_frame| Err(io::Error::from_raw_os_error(libc::EPIPE)),
        |d| backoff_sleeps.push(d),
    );

    // 9 retries before the 10th failure terminates the loop
    assert_eq!(backoff_sleeps.len(), 9);
    for (idx, duration) in backoff_sleeps.iter().enumerate() {
        assert_eq!(*duration, Duration::from_millis(10 * (idx as u64 + 1)));
    }

    let mut errors = Vec::new();
    while let Ok(status) = status_rx.try_recv() {
        if let WorkerStatus::Error(err) = status {
            errors.push(err);
        }
    }
    assert!(!errors.is_empty());
    assert!(errors[0].contains("after 10 retries"));
}

#[test]
fn test_worker_fake_resolver_reconnects_and_resumes_sends() {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (status_tx, status_rx) = mpsc::channel();

    let resolve_attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let resolve_attempts_clone = resolve_attempts.clone();
    let fake_resolver = move || {
        let count = resolve_attempts_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if count < 3 {
            None
        } else {
            Some(PathBuf::from("/dev/fake_hidraw1"))
        }
    };

    let open_attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let open_attempts_clone = open_attempts.clone();
    let fake_opener = move |path: &PathBuf| {
        open_attempts_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(path, &PathBuf::from("/dev/fake_hidraw1"));
        Ok(())
    };

    let sent_frames = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sent_frames_clone = sent_frames.clone();
    let fake_sender = move |_dev: &mut (), frame: &[u8; aula_f75::proto::FRAME_LEN]| {
        sent_frames_clone.lock().unwrap().push(*frame);
        Ok(())
    };

    let handle = std::thread::spawn(move || {
        run_worker_reconnect_loop(
            cmd_rx,
            status_tx,
            WorkerMessage::SetColor((12, 34, 56)),
            Duration::from_millis(5),
            Duration::from_millis(5),
            fake_resolver,
            fake_opener,
            fake_sender,
            |_| {},
        );
    });

    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        if !sent_frames.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(
        !sent_frames.lock().unwrap().is_empty(),
        "Worker failed to send frames after resolver succeeded"
    );

    cmd_tx.send(WorkerMessage::Stop).unwrap();
    drop(cmd_tx);
    handle.join().expect("Worker thread panicked");

    let mut statuses = Vec::new();
    while let Ok(s) = status_rx.try_recv() {
        statuses.push(s);
    }

    assert!(
        statuses.contains(&WorkerStatus::Connecting),
        "Expected WorkerStatus::Connecting while resolver was failing"
    );
    assert!(
        statuses.contains(&WorkerStatus::Running(PathBuf::from("/dev/fake_hidraw1"))),
        "Expected WorkerStatus::Running when device resolved"
    );
    assert_eq!(statuses.last(), Some(&WorkerStatus::Stopped));
    assert!(resolve_attempts.load(std::sync::atomic::Ordering::SeqCst) >= 4);
    assert!(open_attempts.load(std::sync::atomic::Ordering::SeqCst) >= 1);
}

#[test]
fn test_start_when_worker_alive_does_not_duplicate() {
    let mut app = AulaF75App::default();

    // Start worker first time
    let msg1 = app.state.transition_to_solid((255, 0, 0));
    app.start_worker(msg1);
    assert!(app.is_worker_running());
    assert_eq!(app.state.mode, GuiLightingMode::Solid);
    assert_eq!(app.state.rgb, (255, 0, 0));
    let thread_id_1 = app.worker_handle.as_ref().unwrap().thread().id();

    // Attempt to start worker again while it is already running (warm path)
    let msg2 = app.state.transition_to_solid((0, 255, 0));
    app.start_worker(msg2);
    assert!(app.is_worker_running());
    assert_eq!(app.state.mode, GuiLightingMode::Solid);
    assert_eq!(app.state.rgb, (0, 255, 0));
    let thread_id_2 = app.worker_handle.as_ref().unwrap().thread().id();

    // Worker thread must NOT be duplicated or respawned
    assert_eq!(thread_id_1, thread_id_2);

    // Clean up worker
    app.stop_worker();
    assert!(!app.is_worker_running());
    assert_eq!(app.state.mode, GuiLightingMode::Off);

    // Calling start_worker after stopping should respawn a fresh thread
    let msg3 = app.state.transition_to_solid((0, 0, 255));
    app.start_worker(msg3);
    assert!(app.is_worker_running());
    assert_eq!(app.state.mode, GuiLightingMode::Solid);
    assert_eq!(app.state.rgb, (0, 0, 255));
    let thread_id_3 = app.worker_handle.as_ref().unwrap().thread().id();
    assert_ne!(thread_id_1, thread_id_3);

    app.stop_worker();
    assert!(!app.is_worker_running());
    assert_eq!(app.state.mode, GuiLightingMode::Off);
}

#[test]
fn test_app_visuals_returns_cohesive_dark_theme() {
    let visuals = aula_f75::gui::app_visuals();
    assert!(visuals.dark_mode);
    assert_eq!(visuals.panel_fill, aula_f75::gui::COLOR_BG_PANEL);
    assert_eq!(visuals.window_fill, aula_f75::gui::COLOR_BG_CENTRAL);
    assert_eq!(
        visuals.widgets.inactive.corner_radius,
        aula_f75::gui::corner_radius(6)
    );
}

#[test]
fn test_compute_preview_rgb() {
    assert_eq!(
        aula_f75::gui::compute_preview_rgb([255, 100, 50], 1.0),
        (255, 100, 50)
    );
    assert_eq!(
        aula_f75::gui::compute_preview_rgb([200, 100, 50], 0.5),
        (100, 50, 25)
    );
    assert_eq!(
        aula_f75::gui::compute_preview_rgb([255, 255, 255], 0.0),
        (0, 0, 0)
    );
    assert_eq!(
        aula_f75::gui::compute_preview_rgb([100, 100, 100], 1.5),
        (100, 100, 100)
    );
    assert_eq!(
        aula_f75::gui::compute_preview_rgb([100, 100, 100], -0.5),
        (0, 0, 0)
    );
}

#[test]
fn test_connection_badge_state_transitions() {
    use aula_f75::gui::ConnectionBadgeState;

    // Error state takes precedence
    let state = ConnectionBadgeState::determine(true, true, "Running", true);
    assert_eq!(state, ConnectionBadgeState::Error);
    assert_eq!(state.label(), "Error");

    // Streaming active
    let state = ConnectionBadgeState::determine(true, true, "Streaming active", false);
    assert_eq!(state, ConnectionBadgeState::Streaming);
    assert_eq!(state.label(), "Streaming Active");

    // Connected idle
    let state = ConnectionBadgeState::determine(true, false, "Connected (idle)", false);
    assert_eq!(state, ConnectionBadgeState::ConnectedIdle);
    assert_eq!(state.label(), "Connected (idle)");

    // Disconnected
    let state = ConnectionBadgeState::determine(false, false, "Disconnected", false);
    assert_eq!(state, ConnectionBadgeState::Disconnected);
    assert_eq!(state.label(), "Disconnected");

    // Connecting / Searching
    let state = ConnectionBadgeState::determine(false, false, "Connecting...", false);
    assert_eq!(state, ConnectionBadgeState::Connecting);
    assert_eq!(state.label(), "Connecting...");
}

#[test]
fn test_gui_default_effect_speed_is_one() {
    let app = AulaF75App::default();
    assert_eq!(app.effect_speed, 1.0);
}

#[test]
fn test_gui_current_effect_period_mapping_default_speed() {
    let mut app = AulaF75App::default();
    app.color_rgb = [255, 0, 128];

    // Default speed 1.0 keeps rainbow: 5.0s, wave: 3.0s, breathe: 4.0s
    app.selected_effect = "Rainbow".to_string();
    assert_eq!(app.current_effect(), Effect::Rainbow { period_s: 5.0 });

    app.selected_effect = "Wave".to_string();
    assert_eq!(app.current_effect(), Effect::Wave { period_s: 3.0 });

    app.selected_effect = "Breathe".to_string();
    assert_eq!(
        app.current_effect(),
        Effect::Breathe {
            color: aula_f75::effects::RGB(255, 0, 128),
            period_s: 4.0,
        }
    );

    // Solid, Stars, Off are unchanged by speed
    app.selected_effect = "Solid".to_string();
    assert_eq!(
        app.current_effect(),
        Effect::Solid(aula_f75::effects::RGB(255, 0, 128))
    );

    app.selected_effect = "Stars".to_string();
    assert_eq!(app.current_effect(), Effect::Stars { density: 0.15 });

    app.selected_effect = "Off".to_string();
    assert_eq!(app.current_effect(), Effect::Off);
}

#[test]
fn test_gui_current_effect_speed_multiplier_halves_periods() {
    let mut app = AulaF75App::default();
    app.effect_speed = 2.0;
    app.color_rgb = [0, 255, 0];

    // Speed 2.0 halves periods: rainbow 5.0 / 2.0 = 2.5s, wave 3.0 / 2.0 = 1.5s, breathe 4.0 / 2.0 = 2.0s
    app.selected_effect = "Rainbow".to_string();
    assert_eq!(app.current_effect(), Effect::Rainbow { period_s: 2.5 });

    app.selected_effect = "Wave".to_string();
    assert_eq!(app.current_effect(), Effect::Wave { period_s: 1.5 });

    app.selected_effect = "Breathe".to_string();
    assert_eq!(
        app.current_effect(),
        Effect::Breathe {
            color: aula_f75::effects::RGB(0, 255, 0),
            period_s: 2.0,
        }
    );

    // Non-speed effects unaffected
    app.selected_effect = "Stars".to_string();
    assert_eq!(app.current_effect(), Effect::Stars { density: 0.15 });

    app.selected_effect = "Solid".to_string();
    assert_eq!(
        app.current_effect(),
        Effect::Solid(aula_f75::effects::RGB(0, 255, 0))
    );
}

#[test]
fn test_gui_current_effect_speed_clamping_and_guards() {
    let mut app = AulaF75App::default();

    // Below 0.25 clamps to 0.25 (5.0 / 0.25 = 20.0s, 3.0 / 0.25 = 12.0s)
    app.effect_speed = 0.1;
    app.selected_effect = "Rainbow".to_string();
    assert_eq!(app.current_effect(), Effect::Rainbow { period_s: 20.0 });
    app.selected_effect = "Wave".to_string();
    assert_eq!(app.current_effect(), Effect::Wave { period_s: 12.0 });

    // Above 3.0 clamps to 3.0 (3.0 / 3.0 = 1.0s)
    app.effect_speed = 5.0;
    app.selected_effect = "Wave".to_string();
    assert_eq!(app.current_effect(), Effect::Wave { period_s: 1.0 });

    // Zero clamps to 0.25 without division by zero
    app.effect_speed = 0.0;
    app.selected_effect = "Rainbow".to_string();
    assert_eq!(app.current_effect(), Effect::Rainbow { period_s: 20.0 });

    // NaN defaults safely to 1.0
    app.effect_speed = f32::NAN;
    app.selected_effect = "Rainbow".to_string();
    assert_eq!(app.current_effect(), Effect::Rainbow { period_s: 5.0 });
}

#[test]
fn test_gui_dispatch_current_effect_with_speed() {
    let mut app = AulaF75App::default();
    let (tx, rx) = mpsc::channel();
    app.worker_tx = Some(tx);
    // Mark worker running via mock handle
    let handle = std::thread::spawn(|| {});
    app.worker_handle = Some(handle);

    app.effect_speed = 2.0;
    app.selected_effect = "Wave".to_string();
    app.dispatch_current_effect();

    let received = rx.recv_timeout(Duration::from_millis(200)).unwrap();
    assert_eq!(
        received,
        WorkerMessage::SetEffect {
            effect: Effect::Wave { period_s: 1.5 },
            brightness: 1.0,
        }
    );

    let _ = app.worker_handle.take().unwrap().join();
}

#[test]
fn test_cli_fx_speed_parsing_and_defaults() {
    use aula_f75::cli::{Cli, Commands};
    use clap::Parser;

    // Default speed is 1.0
    let parsed_default = Cli::try_parse_from(["aula-f75-rs", "fx", "rainbow"]).unwrap();
    if let Commands::Fx { speed, .. } = parsed_default.command {
        assert_eq!(speed, 1.0);
    } else {
        panic!("Expected Commands::Fx");
    }

    // Explicit --speed
    let parsed_custom = Cli::try_parse_from(["aula-f75-rs", "fx", "wave", "--speed", "2.0"]).unwrap();
    if let Commands::Fx { speed, .. } = parsed_custom.command {
        assert_eq!(speed, 2.0);
    } else {
        panic!("Expected Commands::Fx");
    }
}

#[test]
fn test_cli_fx_speed_validation() {
    use aula_f75::cli::validate_effect_speed;

    assert!(validate_effect_speed(0.25).is_ok());
    assert!(validate_effect_speed(1.0).is_ok());
    assert!(validate_effect_speed(3.0).is_ok());

    assert!(validate_effect_speed(0.24).is_err());
    assert!(validate_effect_speed(3.01).is_err());
    assert!(validate_effect_speed(-0.5).is_err());
    assert!(validate_effect_speed(f32::NAN).is_err());
}

#[test]
fn test_effects_from_name_with_speed_halves_periods() {
    use aula_f75::effects::{Effect, RGB};

    let rainbow_half = Effect::from_name_with_speed("rainbow", None, 2.0).unwrap();
    assert_eq!(rainbow_half, Effect::Rainbow { period_s: 2.5 });

    let wave_half = Effect::from_name_with_speed("wave", None, 2.0).unwrap();
    assert_eq!(wave_half, Effect::Wave { period_s: 1.5 });

    let breathe_half = Effect::from_name_with_speed("breathe", Some(RGB(255, 0, 0)), 2.0).unwrap();
    assert_eq!(
        breathe_half,
        Effect::Breathe {
            color: RGB(255, 0, 0),
            period_s: 2.0,
        }
    );

    let solid = Effect::from_name_with_speed("solid", Some(RGB(10, 20, 30)), 2.0).unwrap();
    assert_eq!(solid, Effect::Solid(RGB(10, 20, 30)));

    let stars = Effect::from_name_with_speed("stars", None, 2.0).unwrap();
    assert_eq!(stars, Effect::Stars { density: 0.15 });

    let off = Effect::from_name_with_speed("off", None, 2.0).unwrap();
    assert_eq!(off, Effect::Off);
}

static GUI_TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct GuiTestTempDir {
    path: PathBuf,
}

impl GuiTestTempDir {
    fn new(prefix: &str) -> Self {
        let unique = format!(
            "aula_f75_gui_test_{}_{}_{}",
            prefix,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path).expect("Failed to create temp dir");
        Self { path }
    }
}

impl Drop for GuiTestTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn test_gui_startup_selection_prefers_default_over_alphabetical_first() {
    let _lock = GUI_TEST_ENV_LOCK.lock().unwrap();
    let tmp = GuiTestTempDir::new("startup_pref");
    let prev_env = std::env::var("AULA_F75_CONFIG").ok();
    std::env::set_var("AULA_F75_CONFIG", &tmp.path);

    let p1 = Profile::new("alpha", "rainbow", None, 1.0, Vec::new());
    let p2 = Profile::new("bravo", "wave", None, 1.0, Vec::new());
    let p3 = Profile::new("charlie", "stars", None, 1.0, Vec::new());

    save_profile(&p1, None).unwrap();
    save_profile(&p2, None).unwrap();
    save_profile(&p3, None).unwrap();

    let saved = vec!["alpha".to_string(), "bravo".to_string(), "charlie".to_string()];

    // Without default: startup selection resolves to alphabetical first ("alpha")
    assert_eq!(
        resolve_startup_profile(&saved, None),
        Some("alpha".to_string())
    );
    let app_bare = AulaF75App::default();
    assert_eq!(app_bare.selected_profile, Some("alpha".to_string()));
    assert_eq!(app_bare.state.mode, GuiLightingMode::Off);
    assert_eq!(app_bare.brightness, 1.0);
    assert_eq!(app_bare.effect_speed, 1.0);
    assert_eq!(app_bare.selected_effect, "Solid");
    assert!(!app_bare.streamed);
    assert!(!app_bare.is_worker_running());

    // Set "charlie" as default: startup selection must now prefer "charlie"
    set_default_profile("charlie", None).unwrap();
    assert_eq!(
        resolve_startup_profile(&saved, None),
        Some("charlie".to_string())
    );
    let app_default = AulaF75App::default();
    assert_eq!(app_default.selected_profile, Some("charlie".to_string()));
    assert_eq!(app_default.selected_effect, "Stars");
    assert!(app_default.streamed);
    assert!(!app_default.is_worker_running());

    match prev_env {
        Some(v) => std::env::set_var("AULA_F75_CONFIG", v),
        None => std::env::remove_var("AULA_F75_CONFIG"),
    }
}

#[test]
fn test_gui_initial_worker_message_mapping_and_fallback() {
    let _lock = GUI_TEST_ENV_LOCK.lock().unwrap();
    let tmp = GuiTestTempDir::new("worker_msg_map");
    let prev_env = std::env::var("AULA_F75_CONFIG").ok();
    std::env::set_var("AULA_F75_CONFIG", &tmp.path);

    // Fallback when no default exists: resolve_initial_worker_message is None
    assert_eq!(resolve_initial_worker_message(None), None);
    let app = AulaF75App::default();
    assert_eq!(app.initial_worker_message(), None);

    // 1. Initial WorkerMessage mapping for an effect profile
    let prof_wave = Profile::new_with_speed("speedy_wave", "wave", None, 0.75, 1.5, Vec::new());
    save_profile(&prof_wave, None).unwrap();
    set_default_profile("speedy_wave", None).unwrap();

    let msg_fx = resolve_initial_worker_message(None).unwrap();
    assert_eq!(
        msg_fx,
        WorkerMessage::SetEffect {
            effect: Effect::Wave { period_s: 2.0 },
            brightness: 0.75,
        }
    );

    let app_fx = AulaF75App::default();
    assert_eq!(app_fx.brightness, 0.75);
    assert_eq!(app_fx.effect_speed, 1.5);
    assert_eq!(app_fx.selected_effect, "Wave");
    assert!(app_fx.streamed);
    assert!(!app_fx.last_was_keyframe);
    assert!(!app_fx.is_worker_running());
    assert_eq!(app_fx.initial_worker_message(), Some(msg_fx));

    // Pure helper in-memory verification for effects
    assert_eq!(
        initial_worker_message_for_profile(&prof_wave),
        WorkerMessage::SetEffect {
            effect: Effect::Wave { period_s: 2.0 },
            brightness: 0.75,
        }
    );
    let prof_off = Profile::new("shutoff", "off", None, 1.0, Vec::new());
    assert_eq!(
        initial_worker_message_for_profile(&prof_off),
        WorkerMessage::Off
    );

    // 2. Initial WorkerMessage mapping for a keyframe profile
    let keys = vec![
        ("esc".to_string(), (255, 0, 0)),
        ("space".to_string(), (0, 255, 0)),
    ];
    let prof_keys = Profile::new("gaming", "off", None, 0.85, keys.clone());
    save_profile(&prof_keys, None).unwrap();
    set_default_profile("gaming", None).unwrap();

    let msg_keys = resolve_initial_worker_message(None).unwrap();
    assert_eq!(msg_keys, WorkerMessage::SetKeyFrame(keys.clone()));

    let app_keys = AulaF75App::default();
    assert_eq!(app_keys.brightness, 0.85);
    assert!(app_keys.streamed);
    assert!(app_keys.last_was_keyframe);
    assert!(!app_keys.is_worker_running());
    assert_eq!(app_keys.initial_worker_message(), Some(msg_keys));

    // Pure helper in-memory verification for keyframe
    assert_eq!(
        initial_worker_message_for_profile(&prof_keys),
        WorkerMessage::SetKeyFrame(keys)
    );

    match prev_env {
        Some(v) => std::env::set_var("AULA_F75_CONFIG", v),
        None => std::env::remove_var("AULA_F75_CONFIG"),
    }
}

#[test]
fn test_keyframe_transition_to_stop_and_note_stop_clears_flags_and_adopts_off() {
    let _lock = GUI_TEST_ENV_LOCK.lock().unwrap();
    let tmp = GuiTestTempDir::new("keyframe_stop_adopt");
    let prev_env = std::env::var("AULA_F75_CONFIG").ok();
    std::env::set_var("AULA_F75_CONFIG", &tmp.path);

    let unit_path = tmp.path.join("aula-f75.service");

    let initial_content = "[Unit]\nDescription=AULA F75\n\n[Service]\nType=simple\nExecStart=%h/.local/bin/aula-f75-rs solid 303030\n\n[Install]\nWantedBy=default.target\n";
    std::fs::write(&unit_path, initial_content).unwrap();

    let fake = FakeServiceRunner::new(false);
    let mut app = AulaF75App::new_with_service_runner(fake.clone());
    app.custom_service_unit_path = Some(unit_path.clone());
    app.color_rgb = [0, 128, 255];
    app.state.rgb = (0, 128, 255);

    // Stream a keyframe message
    app.note_worker_message(&WorkerMessage::SetKeyFrame(vec![(
        "esc".to_string(),
        (255, 0, 0),
    )]));
    assert!(app.streamed);
    assert!(app.state.streamed);
    assert!(app.last_was_keyframe);
    assert!(app.state.last_was_keyframe);

    // Transition to stop and note the Stop message
    let stop_msg = app.state.transition_to_stop();
    assert_eq!(stop_msg, WorkerMessage::Stop);
    app.note_worker_message(&stop_msg);

    // Stop paths must clear keyframe flags while preserving streamed
    assert_eq!(app.state.mode, GuiLightingMode::Off);
    assert!(!app.last_was_keyframe);
    assert!(!app.state.last_was_keyframe);
    assert!(app.streamed);
    assert!(app.state.streamed);

    // Exit adopt path must write solid 000000 instead of stale RGB
    drop(app);

    assert_eq!(fake.restart_count(), 1);
    let content = std::fs::read_to_string(&unit_path).unwrap();
    assert!(content.contains("ExecStart=%h/.local/bin/aula-f75-rs solid 000000"));

    match prev_env {
        Some(v) => std::env::set_var("AULA_F75_CONFIG", v),
        None => std::env::remove_var("AULA_F75_CONFIG"),
    }
}

#[test]
fn test_on_off_clicked_marks_streamed_and_adopts_off_on_exit() {
    let _lock = GUI_TEST_ENV_LOCK.lock().unwrap();
    let tmp = GuiTestTempDir::new("on_off_adopt");
    let prev_env = std::env::var("AULA_F75_CONFIG").ok();
    std::env::set_var("AULA_F75_CONFIG", &tmp.path);

    let unit_path = tmp.path.join("aula-f75.service");

    let initial_content = "[Unit]\nDescription=AULA F75\n\n[Service]\nType=simple\nExecStart=%h/.local/bin/aula-f75-rs solid 303030\n\n[Install]\nWantedBy=default.target\n";
    std::fs::write(&unit_path, initial_content).unwrap();

    let fake = FakeServiceRunner::new(true);
    let mut app = AulaF75App::new_with_service_runner(fake.clone());
    app.custom_service_unit_path = Some(unit_path.clone());

    // Initially idle
    assert!(!app.streamed);
    assert!(!app.state.streamed);

    // Explicit Off click
    app.on_off_clicked();

    assert!(app.streamed);
    assert!(app.state.streamed);
    assert_eq!(app.state.mode, GuiLightingMode::Off);
    assert!(!app.last_was_keyframe);
    assert!(!app.state.last_was_keyframe);

    // Exit must adopt off rather than taking idle restore
    drop(app);

    assert_eq!(fake.daemon_reload_count(), 1);
    assert_eq!(fake.enable_count(), 1);
    assert_eq!(fake.restart_count(), 1);
    assert_eq!(fake.start_count(), 0);

    let content = std::fs::read_to_string(&unit_path).unwrap();
    assert!(content.contains("ExecStart=%h/.local/bin/aula-f75-rs solid 000000"));

    match prev_env {
        Some(v) => std::env::set_var("AULA_F75_CONFIG", v),
        None => std::env::remove_var("AULA_F75_CONFIG"),
    }
}

#[test]
fn test_is_streaming_reports_true_during_active_keyframe_and_false_after_stop() {
    let _lock = GUI_TEST_ENV_LOCK.lock().unwrap();
    let tmp = GuiTestTempDir::new("is_streaming");
    let prev_env = std::env::var("AULA_F75_CONFIG").ok();
    std::env::set_var("AULA_F75_CONFIG", &tmp.path);

    let mut app = AulaF75App::default();
    assert!(!app.is_streaming());

    // Mock active running worker
    let (tx_cmd, rx_cmd) = mpsc::channel();
    app.worker_tx = Some(tx_cmd);
    let handle = std::thread::spawn(move || {
        let _ = rx_cmd.recv();
    });
    app.worker_handle = Some(handle);

    // While in default Off mode and no keyframe flags, not streaming
    assert!(app.is_worker_running());
    assert!(!app.is_streaming());

    // When keyframe is active, is_streaming must report true
    app.note_worker_message(&WorkerMessage::SetKeyFrame(vec![(
        "space".to_string(),
        (0, 255, 0),
    )]));
    assert!(app.is_streaming());

    // After stop_worker, is_streaming must report false
    app.stop_worker();
    assert!(!app.is_worker_running());
    assert!(!app.last_was_keyframe);
    assert!(!app.state.last_was_keyframe);
    assert!(!app.is_streaming());

    match prev_env {
        Some(v) => std::env::set_var("AULA_F75_CONFIG", v),
        None => std::env::remove_var("AULA_F75_CONFIG"),
    }
}

#[test]
fn test_truly_idle_session_preserves_service_state_without_adopting() {
    let _lock = GUI_TEST_ENV_LOCK.lock().unwrap();
    let tmp = GuiTestTempDir::new("truly_idle_restore");
    let prev_env = std::env::var("AULA_F75_CONFIG").ok();
    std::env::set_var("AULA_F75_CONFIG", &tmp.path);

    let unit_path = tmp.path.join("aula-f75.service");

    let initial_content = "[Unit]\nDescription=AULA F75\n\n[Service]\nType=simple\nExecStart=%h/.local/bin/aula-f75-rs solid 303030\n\n[Install]\nWantedBy=default.target\n";
    std::fs::write(&unit_path, initial_content).unwrap();

    let fake = FakeServiceRunner::new(true);
    let mut app = AulaF75App::new_with_service_runner(fake.clone());
    app.custom_service_unit_path = Some(unit_path.clone());

    // Never streamed anything
    assert!(!app.streamed);
    assert!(!app.state.streamed);
    assert!(app.service_was_active);

    // Dropping truly idle session restores previous active service without adopt
    drop(app);

    assert_eq!(fake.start_count(), 1);
    assert_eq!(fake.daemon_reload_count(), 0);
    assert_eq!(fake.restart_count(), 0);

    let content = std::fs::read_to_string(&unit_path).unwrap();
    assert!(content.contains("ExecStart=%h/.local/bin/aula-f75-rs solid 303030"));
    assert!(!content.contains("solid 000000"));

    match prev_env {
        Some(v) => std::env::set_var("AULA_F75_CONFIG", v),
        None => std::env::remove_var("AULA_F75_CONFIG"),
    }
}

#[test]
fn test_cold_start_worker_keyframe_preserves_flags_and_adopts_fallback_color() {
    let _lock = GUI_TEST_ENV_LOCK.lock().unwrap();
    let tmp = GuiTestTempDir::new("cold_keyframe");
    let prev_env = std::env::var("AULA_F75_CONFIG").ok();
    std::env::set_var("AULA_F75_CONFIG", &tmp.path);

    let unit_path = tmp.path.join("aula-f75.service");
    let initial_content = "[Unit]\nDescription=AULA F75\n\n[Service]\nType=simple\nExecStart=%h/.local/bin/aula-f75-rs solid 303030\n\n[Install]\nWantedBy=default.target\n";
    std::fs::write(&unit_path, initial_content).unwrap();

    let fake = FakeServiceRunner::new(false);
    let mut app = AulaF75App::new_with_service_runner(fake.clone());
    app.custom_service_unit_path = Some(unit_path.clone());
    app.color_rgb = [0, 255, 128];
    app.state.rgb = (0, 255, 128);

    assert!(!app.is_worker_running());

    let keyframe_msg = app.state.transition_to_keyframe(vec![("esc".to_string(), (255, 0, 0))]);
    app.start_worker(keyframe_msg);

    assert!(app.is_worker_running());
    assert!(app.last_was_keyframe);
    assert!(app.state.last_was_keyframe);
    assert!(app.is_streaming());

    drop(app);

    assert_eq!(fake.restart_count(), 1);
    let content = std::fs::read_to_string(&unit_path).unwrap();
    assert!(content.contains("ExecStart=%h/.local/bin/aula-f75-rs solid 00FF80"));
    assert!(!content.contains("solid 000000"));

    match prev_env {
        Some(v) => std::env::set_var("AULA_F75_CONFIG", v),
        None => std::env::remove_var("AULA_F75_CONFIG"),
    }
}

#[test]
fn test_cold_start_worker_effect_preserves_mode_and_adopts_fx_args() {
    let _lock = GUI_TEST_ENV_LOCK.lock().unwrap();
    let tmp = GuiTestTempDir::new("cold_effect");
    let prev_env = std::env::var("AULA_F75_CONFIG").ok();
    std::env::set_var("AULA_F75_CONFIG", &tmp.path);

    let unit_path = tmp.path.join("aula-f75.service");
    let initial_content = "[Unit]\nDescription=AULA F75\n\n[Service]\nType=simple\nExecStart=%h/.local/bin/aula-f75-rs solid 303030\n\n[Install]\nWantedBy=default.target\n";
    std::fs::write(&unit_path, initial_content).unwrap();

    let fake = FakeServiceRunner::new(false);
    let mut app = AulaF75App::new_with_service_runner(fake.clone());
    app.custom_service_unit_path = Some(unit_path.clone());
    app.brightness = 0.8;
    app.effect_speed = 1.5;
    app.selected_effect = "Rainbow".to_string();

    assert!(!app.is_worker_running());

    let rainbow = Effect::Rainbow { period_s: 5.0 };
    let msg = app.state.transition_to_effect(rainbow.clone(), 0.8);
    app.start_worker(msg);

    assert!(app.is_worker_running());
    assert_eq!(app.state.mode, GuiLightingMode::Effect(rainbow));
    assert!(app.is_streaming());

    drop(app);

    assert_eq!(fake.restart_count(), 1);
    let content = std::fs::read_to_string(&unit_path).unwrap();
    assert!(content.contains("ExecStart=%h/.local/bin/aula-f75-rs fx rainbow --brightness 0.8 --speed 1.5"));
    assert!(!content.contains("solid 000000"));

    match prev_env {
        Some(v) => std::env::set_var("AULA_F75_CONFIG", v),
        None => std::env::remove_var("AULA_F75_CONFIG"),
    }
}

#[test]
fn test_cold_on_solid_clicked_sets_solid_mode_and_adopts_color() {
    let _lock = GUI_TEST_ENV_LOCK.lock().unwrap();
    let tmp = GuiTestTempDir::new("cold_solid");
    let prev_env = std::env::var("AULA_F75_CONFIG").ok();
    std::env::set_var("AULA_F75_CONFIG", &tmp.path);

    let unit_path = tmp.path.join("aula-f75.service");
    let initial_content = "[Unit]\nDescription=AULA F75\n\n[Service]\nType=simple\nExecStart=%h/.local/bin/aula-f75-rs solid 303030\n\n[Install]\nWantedBy=default.target\n";
    std::fs::write(&unit_path, initial_content).unwrap();

    let fake = FakeServiceRunner::new(false);
    let mut app = AulaF75App::new_with_service_runner(fake.clone());
    app.custom_service_unit_path = Some(unit_path.clone());
    app.hex_input = "00E0A0".to_string();

    assert!(!app.is_worker_running());
    assert_eq!(app.state.mode, GuiLightingMode::Off);

    app.on_solid_clicked();

    assert!(app.is_worker_running());
    assert_eq!(app.state.mode, GuiLightingMode::Solid);
    assert_eq!(app.state.rgb, (0, 0xE0, 0xA0));
    assert_eq!(app.color_rgb, [0, 0xE0, 0xA0]);
    assert!(app.is_streaming());

    drop(app);

    assert_eq!(fake.restart_count(), 1);
    let content = std::fs::read_to_string(&unit_path).unwrap();
    assert!(content.contains("ExecStart=%h/.local/bin/aula-f75-rs solid 00E0A0"));
    assert!(!content.contains("solid 000000"));

    match prev_env {
        Some(v) => std::env::set_var("AULA_F75_CONFIG", v),
        None => std::env::remove_var("AULA_F75_CONFIG"),
    }
}

#[test]
fn test_stop_worker_resets_mode_to_off_and_clears_flags() {
    let mut app = AulaF75App::default();
    let keyframe_msg = app.state.transition_to_keyframe(vec![("esc".to_string(), (255, 0, 0))]);
    app.start_worker(keyframe_msg);

    assert!(app.is_worker_running());
    assert!(app.last_was_keyframe);
    assert!(app.state.last_was_keyframe);

    app.stop_worker();

    assert!(!app.is_worker_running());
    assert_eq!(app.state.mode, GuiLightingMode::Off);
    assert!(!app.last_was_keyframe);
    assert!(!app.state.last_was_keyframe);
    assert!(app.worker_tx.is_none());
    assert!(app.worker_handle.is_none());
    assert!(app.status_rx.is_none());
    assert_eq!(app.status_text, "Disconnected");
}

#[test]
fn test_on_status_stopped_clears_app_and_state_keyframe_flags() {
    let mut app = AulaF75App::default();
    app.last_was_keyframe = true;
    app.state.last_was_keyframe = true;
    app.state.mode = GuiLightingMode::Solid;

    let (status_tx, status_rx) = mpsc::channel();
    app.status_rx = Some(status_rx);
    status_tx.send(WorkerStatus::Stopped).unwrap();

    app.poll_worker_status();

    assert_eq!(app.state.mode, GuiLightingMode::Off);
    assert!(!app.last_was_keyframe);
    assert!(!app.state.last_was_keyframe);
    assert_eq!(app.status_text, "Disconnected");
}

#[test]
fn test_cold_start_worker_idle_preserves_idle_and_restores_service() {
    let _lock = GUI_TEST_ENV_LOCK.lock().unwrap();
    let tmp = GuiTestTempDir::new("cold_idle");
    let prev_env = std::env::var("AULA_F75_CONFIG").ok();
    std::env::set_var("AULA_F75_CONFIG", &tmp.path);

    let unit_path = tmp.path.join("aula-f75.service");
    let initial_content = "[Unit]\nDescription=AULA F75\n\n[Service]\nType=simple\nExecStart=%h/.local/bin/aula-f75-rs solid 303030\n\n[Install]\nWantedBy=default.target\n";
    std::fs::write(&unit_path, initial_content).unwrap();

    let fake = FakeServiceRunner::new(true);
    let mut app = AulaF75App::new_with_service_runner(fake.clone());
    app.custom_service_unit_path = Some(unit_path.clone());

    assert!(!app.is_worker_running());
    assert!(!app.streamed);
    assert!(!app.state.streamed);

    // Truly-idle cold spawn (cmd None)
    app.start_worker(None);

    assert!(app.is_worker_running());
    assert_eq!(app.state.mode, GuiLightingMode::Off);
    assert!(!app.streamed);
    assert!(!app.state.streamed);
    assert!(!app.is_streaming());

    drop(app);

    // Must restore service without adopting
    assert_eq!(fake.start_count(), 1);
    assert_eq!(fake.restart_count(), 0);
    let content = std::fs::read_to_string(&unit_path).unwrap();
    assert!(content.contains("ExecStart=%h/.local/bin/aula-f75-rs solid 303030"));
    assert!(!content.contains("solid 000000"));

    match prev_env {
        Some(v) => std::env::set_var("AULA_F75_CONFIG", v),
        None => std::env::remove_var("AULA_F75_CONFIG"),
    }
}

#[test]
fn test_cold_start_worker_with_runner_preserves_default_profile_state() {
    let _lock = GUI_TEST_ENV_LOCK.lock().unwrap();
    let tmp = GuiTestTempDir::new("default_prof_start");
    let prev_env = std::env::var("AULA_F75_CONFIG").ok();
    std::env::set_var("AULA_F75_CONFIG", &tmp.path);

    let unit_path = tmp.path.join("aula-f75.service");
    let initial_content = "[Unit]\nDescription=AULA F75\n\n[Service]\nType=simple\nExecStart=%h/.local/bin/aula-f75-rs solid 303030\n\n[Install]\nWantedBy=default.target\n";
    std::fs::write(&unit_path, initial_content).unwrap();

    let prof = Profile::new_with_speed("startup_wave", "wave", None, 0.75, 1.5, Vec::new());
    save_profile(&prof, None).unwrap();
    set_default_profile("startup_wave", None).unwrap();

    let mut app = AulaF75App::new_with_runner(
        Duration::from_millis(10),
        || None,
        |_| Ok(()),
        |_, _| Ok(()),
        |_| {},
    );
    app.custom_service_unit_path = Some(unit_path.clone());

    assert!(app.is_worker_running());
    assert_eq!(
        app.state.mode,
        GuiLightingMode::Effect(Effect::Wave { period_s: 2.0 })
    );
    assert_eq!(app.selected_effect, "Wave");
    assert_eq!(app.brightness, 0.75);
    assert_eq!(app.effect_speed, 1.5);
    assert!(app.is_streaming());

    drop(app);

    let content = std::fs::read_to_string(&unit_path).unwrap();
    assert!(content.contains("ExecStart=%h/.local/bin/aula-f75-rs fx wave --brightness 0.75 --speed 1.5"));
    assert!(!content.contains("solid 000000"));

    match prev_env {
        Some(v) => std::env::set_var("AULA_F75_CONFIG", v),
        None => std::env::remove_var("AULA_F75_CONFIG"),
    }
}



