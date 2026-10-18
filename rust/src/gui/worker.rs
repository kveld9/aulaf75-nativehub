//! Background streaming worker loop and channel messages for AULA F75 GUI.

use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::cli::music::{self, MusicMode};
use crate::effects::{self, Effect, RGB};
use crate::proto;

/// Available lighting effect names for the GUI effect selector.
pub const AVAILABLE_EFFECTS: &[&str] = &[
    "Solid",
    "Rainbow",
    "Wave",
    "Breathe",
    "Stars",
    "Off",
];

/// Channel commands dispatched from the GUI thread to the background worker.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerMessage {
    /// Update active solid color to the given `(R, G, B)` tuple.
    SetColor((u8, u8, u8)),
    /// Stream a host-rendered dynamic or static effect with brightness.
    SetEffect {
        effect: Effect,
        brightness: f32,
    },
    /// Stream a per-key direct RGB frame lighting specific keys.
    SetKeyFrame(Vec<(String, (u8, u8, u8))>),
    /// Stream music-reactive lighting based on system audio.
    SetMusic {
        mode: MusicMode,
        color: (u8, u8, u8),
        gain: f32,
    },
    /// Transmit all-zero off frame and cleanly stop the keepalive loop.
    Off,
    /// Immediately terminate keepalive loop and close device descriptor.
    Stop,
}

/// Status and lifecycle events emitted by the background worker.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerStatus {
    /// Worker is waiting or retrying to connect/reconnect to device node.
    Connecting,
    /// Worker successfully opened device node in idle/connect-only mode.
    Connected(PathBuf),
    /// Alias variant for Connected state in idle-connected mode.
    IdleConnected(PathBuf),
    /// Worker lost connection to device node.
    Disconnected,
    /// Worker successfully opened device node and entered keepalive loop.
    Running(PathBuf),
    /// Worker stopped execution cleanly.
    Stopped,
    /// An error occurred during device resolution, opening, or ioctl execution.
    Error(String),
    /// Periodic snapshot of music band energies for GUI level meter.
    MusicBands(Vec<f32>),
}

/// Core worker streaming loop parameterized over injectable send and sleep functions.
///
/// Implements keepalive transmission at `KEEPALIVE_HZ` (~1.5 Hz) for solid mode,
/// ~25 Hz (40 ms) for dynamic effects, and transient error retries (EPIPE, ETIMEDOUT).
pub fn run_worker_stream<F, S>(
    rx: Receiver<WorkerMessage>,
    status_tx: Sender<WorkerStatus>,
    initial_cmd: WorkerMessage,
    interval: Duration,
    mut send_fn: F,
    mut sleep_fn: S,
) where
    F: FnMut(&[u8; proto::FRAME_LEN]) -> Result<(), std::io::Error>,
    S: FnMut(Duration),
{
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let (mut current_effect, mut current_brightness, mut current_keys, mut current_music) =
        match initial_cmd {
            WorkerMessage::SetColor(rgb) => {
                (Effect::Solid(RGB(rgb.0, rgb.1, rgb.2)), 1.0f32, None, None)
            }
            WorkerMessage::SetEffect { effect, brightness } => (effect, brightness, None, None),
            WorkerMessage::SetKeyFrame(keys) => (Effect::Off, 1.0f32, Some(keys), None),
            WorkerMessage::SetMusic { mode, color, gain } => {
                (Effect::Off, 1.0f32, None, Some((mode, color, gain)))
            }
            WorkerMessage::Off => {
                if let Ok(off_frame) = proto::build_direct_frame((0, 0, 0)) {
                    let _ = send_fn(&off_frame);
                }
                let _ = status_tx.send(WorkerStatus::Stopped);
                return;
            }
            WorkerMessage::Stop => {
                let _ = status_tx.send(WorkerStatus::Stopped);
                return;
            }
        };

    let mut consecutive_failures = 0;
    let key_slots = proto::all_key_slots();
    let stream_start = std::time::Instant::now();

    let mut music_child: Option<ChildGuard> = None;
    let mut music_sample_buf = std::collections::VecDeque::with_capacity(2048);
    let mut music_mags = vec![0.0f32; 512];
    let mut music_autogain = music::AutoGain::new();
    let music_plan = music::FftPlan::new(1024);

    'outer: loop {
        let t = stream_start.elapsed().as_secs_f64();

        // Spawn or maintain audio monitor child process if in music mode
        if let Some((_, _, gain)) = current_music {
            if music_child.is_none() {
                match music::spawn_monitor() {
                    Ok(child) => {
                        if let Some(ref stdout) = child.stdout {
                            let fd = stdout.as_raw_fd();
                            unsafe {
                                let flags = libc::fcntl(fd, libc::F_GETFL, 0);
                                if flags >= 0 {
                                    libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
                                }
                            }
                        }
                        music_child = Some(ChildGuard(child));
                    }
                    Err(e) => {
                        let _ = status_tx.send(WorkerStatus::Error(e));
                        current_music = None;
                    }
                }
            }

            // Non-blocking read of available audio samples
            let mut read_failed = false;
            if let Some(ref mut guard) = music_child {
                if let Some(ref mut stdout) = guard.0.stdout {
                    let mut chunk_buf = [0u8; 8820];
                    loop {
                        match std::io::Read::read(stdout, &mut chunk_buf) {
                            Ok(0) => {
                                read_failed = true;
                                break;
                            }
                            Ok(n) => {
                                let num_samples = n / 4;
                                for i in 0..num_samples {
                                    let offset = i * 4;
                                    let left = i16::from_le_bytes([chunk_buf[offset], chunk_buf[offset + 1]]);
                                    let right = i16::from_le_bytes([chunk_buf[offset + 2], chunk_buf[offset + 3]]);
                                    let mono = ((left as f32 + right as f32) / (2.0 * 32768.0)) * gain;
                                    music_sample_buf.push_back(mono);
                                }
                                while music_sample_buf.len() > 1024 {
                                    music_sample_buf.pop_front();
                                }
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                break;
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {
                                continue;
                            }
                            Err(_) => {
                                read_failed = true;
                                break;
                            }
                        }
                    }
                }
            }

            if read_failed {
                music_child = None;
                current_music = None;
            }
        }

        let frame = if let Some((mode, color, _)) = current_music {
            if music_sample_buf.len() >= 1024 {
                let block: Vec<f32> = music_sample_buf.iter().copied().collect();
                music_plan.compute(&block, &mut music_mags);
                let band_energies = music::bands_with_autogain(&music_mags, 16, 44100.0, &mut music_autogain);
                let _ = status_tx.send(WorkerStatus::MusicBands(band_energies.clone()));
                let pairs = music::bands_to_colors(&band_energies, color, mode);
                match proto::build_slot_frame(&pairs) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = status_tx.send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                        break;
                    }
                }
            } else {
                match proto::build_direct_frame((0, 0, 0)) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = status_tx.send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                        break;
                    }
                }
            }
        } else if let Some(ref keys) = current_keys {
            let borrowed: Vec<(&str, (u8, u8, u8))> =
                keys.iter().map(|(k, c)| (k.as_str(), *c)).collect();
            match proto::build_key_frame(&borrowed) {
                Ok(f) => f,
                Err(e) => {
                    let _ =
                        status_tx.send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                    current_keys = None;
                    continue;
                }
            }
        } else {
            match &current_effect {
                Effect::Solid(rgb) => {
                    let scaled_rgb = rgb.scale(current_brightness).to_tuple();
                    match proto::build_direct_frame(scaled_rgb) {
                        Ok(f) => f,
                        Err(e) => {
                            let _ = status_tx
                                .send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                            break;
                        }
                    }
                }
                Effect::Off => match proto::build_direct_frame((0, 0, 0)) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = status_tx
                            .send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                        break;
                    }
                },
                dynamic_effect => {
                    let pairs = effects::render(dynamic_effect, t, &key_slots);
                    let scaled = effects::apply_brightness(&pairs, current_brightness);
                    match proto::build_slot_frame(&scaled) {
                        Ok(f) => f,
                        Err(e) => {
                            let _ = status_tx
                                .send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                            break;
                        }
                    }
                }
            }
        };

        match send_fn(&frame) {
            Ok(()) => {
                consecutive_failures = 0;
            }
            Err(e) => {
                let code = e.raw_os_error();
                let is_transient = code == Some(libc::EPIPE) || code == Some(libc::ETIMEDOUT);
                if is_transient {
                    consecutive_failures += 1;
                    if consecutive_failures >= 10 {
                        let _ = status_tx.send(WorkerStatus::Error(format!(
                            "Device communication failed after 10 retries: {}",
                            e
                        )));
                        break;
                    }
                    let backoff = Duration::from_millis(10 * consecutive_failures as u64);
                    sleep_fn(backoff);
                } else {
                    let _ = status_tx.send(WorkerStatus::Error(format!(
                        "Device communication error: {}",
                        e
                    )));
                    break;
                }
            }
        }

        let tick_interval = if current_music.is_some() {
            Duration::from_millis(50)
        } else if current_keys.is_some() {
            interval
        } else {
            match current_effect {
                Effect::Solid(_) => interval,
                _ => Duration::from_millis(40),
            }
        };

        let start = std::time::Instant::now();
        while start.elapsed() < tick_interval {
            let remaining = tick_interval.saturating_sub(start.elapsed());
            let poll_step = remaining.min(Duration::from_millis(10));
            match rx.recv_timeout(poll_step) {
                Ok(WorkerMessage::SetColor(new_color)) => {
                    current_music = None;
                    music_child = None;
                    current_keys = None;
                    current_effect = Effect::Solid(RGB(new_color.0, new_color.1, new_color.2));
                    current_brightness = 1.0;
                    break;
                }
                Ok(WorkerMessage::SetEffect { effect, brightness }) => {
                    current_music = None;
                    music_child = None;
                    current_keys = None;
                    current_effect = effect;
                    current_brightness = brightness;
                    break;
                }
                Ok(WorkerMessage::SetKeyFrame(keys)) => {
                    current_music = None;
                    music_child = None;
                    current_keys = Some(keys);
                    break;
                }
                Ok(WorkerMessage::SetMusic { mode, color, gain }) => {
                    current_keys = None;
                    current_music = Some((mode, color, gain));
                    break;
                }
                Ok(WorkerMessage::Off) => {
                    music_child = None;
                    if let Ok(off_frame) = proto::build_direct_frame((0, 0, 0)) {
                        let _ = send_fn(&off_frame);
                    }
                    break 'outer;
                }
                Ok(WorkerMessage::Stop) => {
                    music_child = None;
                    break 'outer;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    break 'outer;
                }
            }
        }
    }

    drop(music_child);
    let _ = status_tx.send(WorkerStatus::Stopped);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    #[test]
    fn test_worker_stream_set_color_after_zero_brightness_effect() {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (status_tx, _status_rx) = mpsc::channel();

        let sent_frames = Arc::new(Mutex::new(Vec::new()));
        let sent_frames_clone = sent_frames.clone();

        let handle = std::thread::spawn(move || {
            run_worker_stream(
                cmd_rx,
                status_tx,
                WorkerMessage::SetEffect {
                    effect: Effect::Rainbow { period_s: 5.0 },
                    brightness: 0.0,
                },
                Duration::from_millis(20),
                move |frame| {
                    sent_frames_clone.lock().unwrap().push(*frame);
                    Ok(())
                },
                |_| {},
            );
        });

        // Wait for initial effect frame with brightness 0.0
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
    fn test_worker_stream_survives_bad_key_frame() {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();

        let sent_frames = Arc::new(Mutex::new(Vec::new()));
        let sent_frames_clone = sent_frames.clone();

        let handle = std::thread::spawn(move || {
            run_worker_stream(
                cmd_rx,
                status_tx,
                WorkerMessage::SetColor((10, 20, 30)),
                Duration::from_millis(20),
                move |frame| {
                    sent_frames_clone.lock().unwrap().push(*frame);
                    Ok(())
                },
                |_| {},
            );
        });

        // Wait for initial frame
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            if !sent_frames.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        // Send SetKeyFrame with an unknown key name
        cmd_tx
            .send(WorkerMessage::SetKeyFrame(vec![(
                "completely_invalid_key_alias".to_string(),
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
        assert!(got_error, "Expected WorkerStatus::Error on bad key");

        // Worker must still be alive! Send valid color
        cmd_tx.send(WorkerMessage::SetColor((40, 50, 60))).unwrap();

        // Wait for updated frame
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            let frames = sent_frames.lock().unwrap();
            if let Some(last) = frames.last() {
                if &last[8..11] == &[40, 50, 60] {
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
        assert_eq!(&last_frame[8..11], &[40, 50, 60]);
    }
}
