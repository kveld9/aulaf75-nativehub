//! Hardware-free integration tests for AULA F75 GUI state logic and channels.

use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use aula_f75::effects::Effect;
use aula_f75::gui::{
    parse_color_hex, run_worker_stream, GuiLightingMode, GuiState, WorkerMessage, WorkerStatus,
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
