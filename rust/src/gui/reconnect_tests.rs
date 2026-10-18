//! Unit tests for GUI worker reconnecting loop.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

#[test]
fn test_worker_reconnect_loop_set_color_resets_brightness() {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (status_tx, _status_rx) = mpsc::channel();

    let sent_frames = Arc::new(Mutex::new(Vec::new()));
    let sent_frames_clone = sent_frames.clone();

    let handle = std::thread::spawn(move || {
        run_worker_reconnect_loop(
            cmd_rx,
            status_tx,
            WorkerMessage::SetEffect {
                effect: Effect::Rainbow { period_s: 5.0 },
                brightness: 0.0,
            },
            Duration::from_millis(10),
            Duration::from_millis(20),
            || Some(PathBuf::from("/dev/fake_node")),
            |_path| Ok(()),
            move |_dev, frame| {
                sent_frames_clone.lock().unwrap().push(*frame);
                Ok(())
            },
            |_| {},
        );
    });

    // Wait for initial effect frame
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        if !sent_frames.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // Send SetColor((255, 0, 0))
    cmd_tx.send(WorkerMessage::SetColor((255, 0, 0))).unwrap();

    // Wait until solid frame with [255, 0, 0] is emitted
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        let frames = sent_frames.lock().unwrap();
        if let Some(last) = frames.last() {
            if &last[8..11] == &[255, 0, 0] {
                break;
            }
        }
        drop(frames);
        std::thread::sleep(Duration::from_millis(5));
    }

    let _ = cmd_tx.send(WorkerMessage::Stop);
    drop(cmd_tx);
    handle.join().expect("Worker thread panicked");

    let frames = sent_frames.lock().unwrap();
    assert!(!frames.is_empty());
    let last_frame = frames.last().unwrap();
    assert_eq!(&last_frame[8..11], &[255, 0, 0]);
}

#[test]
fn test_worker_reconnect_loop_retains_active_device_on_bad_key() {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (status_tx, status_rx) = mpsc::channel();

    let open_count = Arc::new(AtomicUsize::new(0));
    let open_count_clone = open_count.clone();

    let handle = std::thread::spawn(move || {
        run_worker_reconnect_loop(
            cmd_rx,
            status_tx,
            WorkerMessage::SetColor((10, 20, 30)),
            Duration::from_millis(10),
            Duration::from_millis(20),
            || Some(PathBuf::from("/dev/fake_node")),
            move |_path| {
                open_count_clone.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_dev, _frame| Ok(()),
            |_| {},
        );
    });

    // Wait for device to open
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        if open_count.load(Ordering::SeqCst) >= 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(open_count.load(Ordering::SeqCst), 1);

    // Send bad key
    cmd_tx
        .send(WorkerMessage::SetKeyFrame(vec![(
            "unknown_key_xyz".to_string(),
            (255, 0, 0),
        )]))
        .unwrap();

    // Poll for error status
    let start = Instant::now();
    let mut got_error = false;
    while start.elapsed() < Duration::from_millis(500) {
        while let Ok(status) = status_rx.try_recv() {
            if let WorkerStatus::Error(msg) = status {
                if msg.contains("Frame build error") || msg.contains("Unknown key name") {
                    got_error = true;
                }
            }
        }
        if got_error {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(got_error);

    // active_device was NOT dropped, so open_fn was not called again
    assert_eq!(open_count.load(Ordering::SeqCst), 1);

    let _ = cmd_tx.send(WorkerMessage::Stop);
    drop(cmd_tx);
    handle.join().expect("Worker thread panicked");
}
