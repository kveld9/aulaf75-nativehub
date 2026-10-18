//! Device reconnecting worker loop and production runner for AULA F75 GUI.

use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::cli::music;
use crate::device;
use crate::effects::{self, Effect, RGB};
use crate::gui::worker::{WorkerMessage, WorkerStatus};
use crate::proto;

/// Helper to build direct, off, or dynamic effect frame.
fn render_effect_frame(
    effect: &Effect,
    brightness: f32,
    t: f64,
    key_slots: &[u8],
) -> Result<[u8; proto::FRAME_LEN], String> {
    match effect {
        Effect::Solid(rgb) => {
            let scaled_rgb = rgb.scale(brightness).to_tuple();
            proto::build_direct_frame(scaled_rgb)
                .map_err(|e| format!("Frame build error: {}", e))
        }
        Effect::Off => proto::build_direct_frame((0, 0, 0))
            .map_err(|e| format!("Frame build error: {}", e)),
        dynamic_effect => {
            let pairs = effects::render(dynamic_effect, t, key_slots);
            let scaled = effects::apply_brightness(&pairs, brightness);
            proto::build_slot_frame(&scaled)
                .map_err(|e| format!("Frame build error: {}", e))
        }
    }
}

/// Helper to build per-key frame.
fn render_keys_frame(
    keys: &[(String, (u8, u8, u8))],
) -> Result<[u8; proto::FRAME_LEN], String> {
    let borrowed: Vec<(&str, (u8, u8, u8))> =
        keys.iter().map(|(k, c)| (k.as_str(), *c)).collect();
    proto::build_key_frame(&borrowed)
        .map_err(|e| format!("Frame build error: {}", e))
}

/// Core reconnecting worker loop that automatically discovers and reopens the device.
///
/// When no device is open (initial startup, unplug, or communication error),
/// retries `resolve_fn` at `reconnect_interval` (~2s in production) and resumes streaming
/// automatically with the current parameters when the device reappears.
pub fn run_worker_reconnect_loop<R, O, D, F, S, I>(
    rx: Receiver<WorkerMessage>,
    status_tx: Sender<WorkerStatus>,
    initial_cmd: I,
    reconnect_interval: Duration,
    keepalive_interval: Duration,
    mut resolve_fn: R,
    mut open_fn: O,
    mut send_fn: F,
    mut sleep_fn: S,
) where
    R: FnMut() -> Option<PathBuf>,
    O: FnMut(&PathBuf) -> Result<D, io::Error>,
    F: FnMut(&mut D, &[u8; proto::FRAME_LEN]) -> Result<(), io::Error>,
    S: FnMut(Duration),
    I: Into<Option<WorkerMessage>>,
{
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let (mut current_effect, mut current_brightness, mut current_keys, mut current_music, mut is_idle) =
        match initial_cmd.into() {
            Some(WorkerMessage::SetColor(rgb)) => (
                Effect::Solid(RGB(rgb.0, rgb.1, rgb.2)),
                1.0f32,
                None,
                None,
                false,
            ),
            Some(WorkerMessage::SetEffect { effect, brightness }) => {
                (effect, brightness, None, None, false)
            }
            Some(WorkerMessage::SetKeyFrame(keys)) => {
                (Effect::Off, 1.0f32, Some(keys), None, false)
            }
            Some(WorkerMessage::SetMusic { mode, color, gain }) => {
                (Effect::Off, 1.0f32, None, Some((mode, color, gain)), false)
            }
            Some(WorkerMessage::Off) => (Effect::Off, 1.0f32, None, None, true),
            Some(WorkerMessage::Stop) | None => (Effect::Off, 1.0f32, None, None, true),
        };

    let mut active_device: Option<(PathBuf, D)> = None;
    let mut reported_connecting = false;
    let mut consecutive_failures = 0;
    let key_slots = proto::all_key_slots();
    let stream_start = Instant::now();

    let mut music_child: Option<ChildGuard> = None;
    let mut music_sample_buf = std::collections::VecDeque::with_capacity(2048);
    let mut music_mags = vec![0.0f32; 512];
    let mut music_autogain = music::AutoGain::new();
    let music_plan = music::FftPlan::new(1024);

    'outer: loop {
        // 1. Device resolution & opening when not connected
        if active_device.is_none() {
            if !reported_connecting {
                let _ = status_tx.send(WorkerStatus::Connecting);
                reported_connecting = true;
            }

            if let Some(node) = resolve_fn() {
                match open_fn(&node) {
                    Ok(dev) => {
                        if is_idle {
                            let _ = status_tx.send(WorkerStatus::Connected(node.clone()));
                        } else {
                            let _ = status_tx.send(WorkerStatus::Running(node.clone()));
                        }
                        active_device = Some((node, dev));
                        reported_connecting = false;
                        consecutive_failures = 0;
                    }
                    Err(e) => {
                        let _ = status_tx.send(WorkerStatus::Error(format!(
                            "Failed to open {}: {}",
                            node.display(),
                            e
                        )));
                    }
                }
            }

            if active_device.is_none() {
                let wait_start = Instant::now();
                while wait_start.elapsed() < reconnect_interval {
                    let remaining = reconnect_interval.saturating_sub(wait_start.elapsed());
                    let poll_step = remaining.min(Duration::from_millis(50));
                    match rx.recv_timeout(poll_step) {
                        Ok(WorkerMessage::Stop) => {
                            is_idle = true;
                            current_music = None;
                            current_keys = None;
                            current_effect = Effect::Off;
                        }
                        Ok(WorkerMessage::Off) => {
                            is_idle = true;
                            current_music = None;
                            current_keys = None;
                            current_effect = Effect::Off;
                        }
                        Ok(WorkerMessage::SetColor(rgb)) => {
                            is_idle = false;
                            current_music = None;
                            current_keys = None;
                            current_effect = Effect::Solid(RGB(rgb.0, rgb.1, rgb.2));
                            current_brightness = 1.0;
                        }
                        Ok(WorkerMessage::SetEffect { effect, brightness }) => {
                            is_idle = false;
                            current_music = None;
                            current_keys = None;
                            current_effect = effect;
                            current_brightness = brightness;
                        }
                        Ok(WorkerMessage::SetKeyFrame(keys)) => {
                            is_idle = false;
                            current_music = None;
                            current_keys = Some(keys);
                        }
                        Ok(WorkerMessage::SetMusic { mode, color, gain }) => {
                            is_idle = false;
                            current_keys = None;
                            current_music = Some((mode, color, gain));
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => {
                            break 'outer;
                        }
                    }
                }
                continue 'outer;
            }
        }

        // 2. If in idle/connect-only mode, send NO frames and wait for commands or device disconnect
        if is_idle {
            let wait_start = Instant::now();
            while wait_start.elapsed() < reconnect_interval {
                let remaining = reconnect_interval.saturating_sub(wait_start.elapsed());
                let poll_step = remaining.min(Duration::from_millis(50));
                match rx.recv_timeout(poll_step) {
                    Ok(WorkerMessage::SetColor(rgb)) => {
                        is_idle = false;
                        current_music = None;
                        current_keys = None;
                        current_effect = Effect::Solid(RGB(rgb.0, rgb.1, rgb.2));
                        current_brightness = 1.0;
                        if let Some((ref node, _)) = active_device {
                            let _ = status_tx.send(WorkerStatus::Running(node.clone()));
                        }
                        continue 'outer;
                    }
                    Ok(WorkerMessage::SetEffect { effect, brightness }) => {
                        is_idle = false;
                        current_music = None;
                        current_keys = None;
                        current_effect = effect;
                        current_brightness = brightness;
                        if let Some((ref node, _)) = active_device {
                            let _ = status_tx.send(WorkerStatus::Running(node.clone()));
                        }
                        continue 'outer;
                    }
                    Ok(WorkerMessage::SetKeyFrame(keys)) => {
                        is_idle = false;
                        current_music = None;
                        current_keys = Some(keys);
                        if let Some((ref node, _)) = active_device {
                            let _ = status_tx.send(WorkerStatus::Running(node.clone()));
                        }
                        continue 'outer;
                    }
                    Ok(WorkerMessage::SetMusic { mode, color, gain }) => {
                        is_idle = false;
                        current_keys = None;
                        current_music = Some((mode, color, gain));
                        if let Some((ref node, _)) = active_device {
                            let _ = status_tx.send(WorkerStatus::Running(node.clone()));
                        }
                        continue 'outer;
                    }
                    Ok(WorkerMessage::Off) => {
                        if let Some((_, ref mut dev)) = active_device {
                            if let Ok(off_frame) = proto::build_direct_frame((0, 0, 0)) {
                                let _ = send_fn(dev, &off_frame);
                            }
                        }
                        if let Some((ref node, _)) = active_device {
                            let _ = status_tx.send(WorkerStatus::IdleConnected(node.clone()));
                        }
                    }
                    Ok(WorkerMessage::Stop) => {
                        if let Some((ref node, _)) = active_device {
                            let _ = status_tx.send(WorkerStatus::IdleConnected(node.clone()));
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        break 'outer;
                    }
                }
            }

            // Check if device is still connected
            if let Some((ref current_node, _)) = active_device {
                let still_present = match resolve_fn() {
                    Some(ref node) => node == current_node,
                    None => false,
                };
                if !still_present {
                    let _ = status_tx.send(WorkerStatus::Disconnected);
                    active_device = None;
                    reported_connecting = false;
                }
            }
            continue 'outer;
        }

        let t = stream_start.elapsed().as_secs_f64();

        // 3. Audio monitor child process for music mode
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

        // 4. Build RGB frame
        let frame = if let Some((mode, color, _)) = current_music {
            if music_sample_buf.len() >= 1024 {
                let block: Vec<f32> = music_sample_buf.iter().copied().collect();
                music_plan.compute(&block, &mut music_mags);
                let band_energies =
                    music::bands_with_autogain(&music_mags, 16, 44100.0, &mut music_autogain);
                let _ = status_tx.send(WorkerStatus::MusicBands(band_energies.clone()));
                let pairs = music::bands_to_colors(&band_energies, color, mode);
                match proto::build_slot_frame(&pairs) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = status_tx
                            .send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                        active_device = None;
                        continue 'outer;
                    }
                }
            } else {
                match proto::build_direct_frame((0, 0, 0)) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = status_tx
                            .send(WorkerStatus::Error(format!("Frame build error: {}", e)));
                        active_device = None;
                        continue 'outer;
                    }
                }
            }
        } else if let Some(ref keys) = current_keys {
            match render_keys_frame(keys) {
                Ok(f) => f,
                Err(e) => {
                    let _ = status_tx.send(WorkerStatus::Error(e));
                    current_keys = None;
                    continue 'outer;
                }
            }
        } else {
            match render_effect_frame(&current_effect, current_brightness, t, &key_slots) {
                Ok(f) => f,
                Err(e) => {
                    let _ = status_tx.send(WorkerStatus::Error(e));
                    active_device = None;
                    continue 'outer;
                }
            }
        };

        // 5. Transmit frame to open device
        if let Some((_, ref mut dev)) = active_device {
            match send_fn(dev, &frame) {
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
                            let _ = status_tx.send(WorkerStatus::Disconnected);
                            // Close device handle and transition to reconnecting
                            active_device = None;
                            consecutive_failures = 0;
                        } else {
                            let backoff = Duration::from_millis(10 * consecutive_failures as u64);
                            sleep_fn(backoff);
                        }
                    } else {
                        let _ = status_tx.send(WorkerStatus::Error(format!(
                            "Device communication error: {}",
                            e
                        )));
                        let _ = status_tx.send(WorkerStatus::Disconnected);
                        // Fatal error (e.g. ENODEV on unplug): close and reconnect
                        active_device = None;
                        consecutive_failures = 0;
                    }
                }
            }
        }

        // 6. Calculate tick interval and process channel messages
        let tick_interval = if current_music.is_some() {
            Duration::from_millis(50)
        } else if current_keys.is_some() {
            keepalive_interval
        } else {
            match current_effect {
                Effect::Solid(_) => keepalive_interval,
                _ => Duration::from_millis(40),
            }
        };

        let start = Instant::now();
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
                    if let Some((_, ref mut dev)) = active_device {
                        if let Ok(off_frame) = proto::build_direct_frame((0, 0, 0)) {
                            let _ = send_fn(dev, &off_frame);
                        }
                    }
                    is_idle = true;
                    if let Some((ref node, _)) = active_device {
                        let _ = status_tx.send(WorkerStatus::IdleConnected(node.clone()));
                    }
                    break;
                }
                Ok(WorkerMessage::Stop) => {
                    music_child = None;
                    is_idle = true;
                    if let Some((ref node, _)) = active_device {
                        let _ = status_tx.send(WorkerStatus::IdleConnected(node.clone()));
                    }
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    break 'outer;
                }
            }
        }
    }

    drop(music_child);
    drop(active_device);
    let _ = status_tx.send(WorkerStatus::Stopped);
}

/// Production worker thread runner that manages device handle lifetime.
pub(crate) fn run_worker_thread<I: Into<Option<WorkerMessage>>>(
    rx: Receiver<WorkerMessage>,
    status_tx: Sender<WorkerStatus>,
    initial_cmd: I,
) {
    let keepalive_interval = Duration::from_secs_f64(1.0 / proto::KEEPALIVE_HZ);
    run_worker_reconnect_loop(
        rx,
        status_tx,
        initial_cmd,
        Duration::from_secs(2),
        keepalive_interval,
        device::resolve_control_node,
        |path| OpenOptions::new().read(true).write(true).open(path),
        |file, frame| proto::hid_set_feature(file.as_raw_fd(), frame),
        thread::sleep,
    );
}
#[cfg(test)]
#[path = "reconnect_tests.rs"]
mod tests;

