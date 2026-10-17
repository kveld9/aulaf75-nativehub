//! Hardware-free integration tests for AULA F75 GUI state logic and channels.

use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use aula_f75::effects::Effect;
use aula_f75::gui::{
    parse_color_hex, run_worker_reconnect_loop, run_worker_stream, AulaF75App, GuiLightingMode,
    GuiState, WorkerMessage, WorkerStatus,
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
    assert_eq!(app.status_text, "Device not connected");
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
    app.start_worker(WorkerMessage::SetColor((255, 0, 0)));
    assert!(app.is_worker_running());
    let thread_id_1 = app.worker_handle.as_ref().unwrap().thread().id();

    // Attempt to start worker again while it is already running
    app.start_worker(WorkerMessage::SetColor((0, 255, 0)));
    assert!(app.is_worker_running());
    let thread_id_2 = app.worker_handle.as_ref().unwrap().thread().id();

    // Worker thread must NOT be duplicated or respawned
    assert_eq!(thread_id_1, thread_id_2);

    // Clean up worker
    app.stop_worker();
    assert!(!app.is_worker_running());

    // Calling start_worker after stopping should respawn a fresh thread
    app.start_worker(WorkerMessage::SetColor((0, 0, 255)));
    assert!(app.is_worker_running());
    let thread_id_3 = app.worker_handle.as_ref().unwrap().thread().id();
    assert_ne!(thread_id_1, thread_id_3);

    app.stop_worker();
    assert!(!app.is_worker_running());
}

