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

struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct StreamState {
    effect: Effect,
    brightness: f32,
    keys: Option<Vec<(String, (u8, u8, u8))>>,
    music: Option<(MusicMode, (u8, u8, u8), f32)>,
}

fn handle_initial_cmd<F>(
    cmd: WorkerMessage,
    status_tx: &Sender<WorkerStatus>,
    send_fn: &mut F,
) -> Option<StreamState>
where
    F: FnMut(&[u8; proto::FRAME_LEN]) -> Result<(), std::io::Error>,
{
    match cmd {
        WorkerMessage::SetColor(rgb) => Some(StreamState {
            effect: Effect::Solid(RGB(rgb.0, rgb.1, rgb.2)),
            brightness: 1.0,
            keys: None,
            music: None,
        }),
        WorkerMessage::SetEffect { effect, brightness } => Some(StreamState {
            effect,
            brightness,
            keys: None,
            music: None,
        }),
        WorkerMessage::SetKeyFrame(keys) => Some(StreamState {
            effect: Effect::Off,
            brightness: 1.0,
            keys: Some(keys),
            music: None,
        }),
        WorkerMessage::SetMusic { mode, color, gain } => Some(StreamState {
            effect: Effect::Off,
            brightness: 1.0,
            keys: None,
            music: Some((mode, color, gain)),
        }),
        WorkerMessage::Off => {
            if let Ok(off_frame) = proto::build_direct_frame((0, 0, 0)) {
                let _ = send_fn(&off_frame);
            }
            let _ = status_tx.send(WorkerStatus::Stopped);
            None
        }
        WorkerMessage::Stop => {
            let _ = status_tx.send(WorkerStatus::Stopped);
            None
        }
    }
}

fn set_child_nonblocking(child: &std::process::Child) {
    if let Some(ref stdout) = child.stdout {
        let fd = stdout.as_raw_fd();
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL, 0);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
    }
}

struct MusicAudioPipeline {
    child: Option<ChildGuard>,
    sample_buf: std::collections::VecDeque<f32>,
    mags: Vec<f32>,
    autogain: music::AutoGain,
    plan: music::FftPlan,
}

impl MusicAudioPipeline {
    fn new() -> Self {
        Self {
            child: None,
            sample_buf: std::collections::VecDeque::with_capacity(2048),
            mags: vec![0.0f32; 512],
            autogain: music::AutoGain::new(),
            plan: music::FftPlan::new(1024),
        }
    }

    fn ensure_spawned(&mut self, status_tx: &Sender<WorkerStatus>) -> bool {
        if self.child.is_some() {
            return true;
        }
        match music::spawn_monitor() {
            Ok(child) => {
                set_child_nonblocking(&child);
                self.child = Some(ChildGuard(child));
                true
            }
            Err(e) => {
                let _ = status_tx.send(WorkerStatus::Error(e));
                false
            }
        }
    }

    fn poll_samples(&mut self, gain: f32) -> bool {
        let guard = match self.child.as_mut() {
            Some(g) => g,
            None => return true,
        };
        let stdout = match guard.0.stdout.as_mut() {
            Some(s) => s,
            None => return true,
        };
        read_audio_stream(stdout, &mut self.sample_buf, gain)
    }
}

fn append_mono_samples(
    buf: &mut std::collections::VecDeque<f32>,
    raw: &[u8],
    gain: f32,
) {
    let num_samples = raw.len() / 4;
    for i in 0..num_samples {
        let offset = i * 4;
        let left = i16::from_le_bytes([raw[offset], raw[offset + 1]]);
        let right = i16::from_le_bytes([raw[offset + 2], raw[offset + 3]]);
        let mono = ((left as f32 + right as f32) / (2.0 * 32768.0)) * gain;
        buf.push_back(mono);
    }
    while buf.len() > 1024 {
        buf.pop_front();
    }
}

fn read_audio_stream(
    stdout: &mut std::process::ChildStdout,
    sample_buf: &mut std::collections::VecDeque<f32>,
    gain: f32,
) -> bool {
    let mut chunk_buf = [0u8; 8820];
    loop {
        match std::io::Read::read(stdout, &mut chunk_buf) {
            Ok(0) => return false,
            Ok(n) => append_mono_samples(sample_buf, &chunk_buf[..n], gain),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => return true,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
}

fn poll_music_audio(
    music: &mut Option<(MusicMode, (u8, u8, u8), f32)>,
    pipeline: &mut MusicAudioPipeline,
    status_tx: &Sender<WorkerStatus>,
) {
    if let Some((_, _, gain)) = *music {
        let ok = pipeline.ensure_spawned(status_tx) && pipeline.poll_samples(gain);
        if !ok {
            pipeline.child = None;
            *music = None;
        }
    }
}

fn render_keys_frame(
    keys: &[(String, (u8, u8, u8))],
) -> Result<[u8; proto::FRAME_LEN], String> {
    let borrowed: Vec<(&str, (u8, u8, u8))> =
        keys.iter().map(|(k, c)| (k.as_str(), *c)).collect();
    proto::build_key_frame(&borrowed)
        .map_err(|e| format!("Frame build error: {}", e))
}

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

fn build_music_frame(
    mode: MusicMode,
    color: (u8, u8, u8),
    pipeline: &mut MusicAudioPipeline,
    status_tx: &Sender<WorkerStatus>,
) -> Result<[u8; proto::FRAME_LEN], String> {
    if pipeline.sample_buf.len() >= 1024 {
        let block: Vec<f32> = pipeline.sample_buf.iter().copied().collect();
        pipeline.plan.compute(&block, &mut pipeline.mags);
        let band_energies =
            music::bands_with_autogain(&pipeline.mags, 16, 44100.0, &mut pipeline.autogain);
        let _ = status_tx.send(WorkerStatus::MusicBands(band_energies.clone()));
        let pairs = music::bands_to_colors(&band_energies, color, mode);
        proto::build_slot_frame(&pairs).map_err(|e| format!("Frame build error: {}", e))
    } else {
        proto::build_direct_frame((0, 0, 0)).map_err(|e| format!("Frame build error: {}", e))
    }
}

enum FrameResult {
    Ok([u8; proto::FRAME_LEN]),
    DropKeys,
    Fatal,
}

fn build_stream_frame(
    state: &StreamState,
    pipeline: &mut MusicAudioPipeline,
    status_tx: &Sender<WorkerStatus>,
    t: f64,
    key_slots: &[u8],
) -> FrameResult {
    if let Some((mode, color, _)) = state.music {
        match build_music_frame(mode, color, pipeline, status_tx) {
            Ok(f) => FrameResult::Ok(f),
            Err(e) => {
                let _ = status_tx.send(WorkerStatus::Error(e));
                FrameResult::Fatal
            }
        }
    } else if let Some(ref keys) = state.keys {
        match render_keys_frame(keys) {
            Ok(f) => FrameResult::Ok(f),
            Err(e) => {
                let _ = status_tx.send(WorkerStatus::Error(e));
                FrameResult::DropKeys
            }
        }
    } else {
        match render_effect_frame(&state.effect, state.brightness, t, key_slots) {
            Ok(f) => FrameResult::Ok(f),
            Err(e) => {
                let _ = status_tx.send(WorkerStatus::Error(e));
                FrameResult::Fatal
            }
        }
    }
}

fn handle_send_error<S>(
    e: std::io::Error,
    consecutive_failures: &mut usize,
    status_tx: &Sender<WorkerStatus>,
    sleep_fn: &mut S,
) -> bool
where
    S: FnMut(Duration),
{
    let code = e.raw_os_error();
    let is_transient = code == Some(libc::EPIPE) || code == Some(libc::ETIMEDOUT);
    if is_transient {
        *consecutive_failures += 1;
        if *consecutive_failures >= 10 {
            let _ = status_tx.send(WorkerStatus::Error(format!(
                "Device communication failed after 10 retries: {}",
                e
            )));
            return false;
        }
        let backoff = Duration::from_millis(10 * *consecutive_failures as u64);
        sleep_fn(backoff);
        true
    } else {
        let _ = status_tx.send(WorkerStatus::Error(format!(
            "Device communication error: {}",
            e
        )));
        false
    }
}

fn transmit_stream_frame<F, S>(
    frame: &[u8; proto::FRAME_LEN],
    consecutive_failures: &mut usize,
    status_tx: &Sender<WorkerStatus>,
    send_fn: &mut F,
    sleep_fn: &mut S,
) -> bool
where
    F: FnMut(&[u8; proto::FRAME_LEN]) -> Result<(), std::io::Error>,
    S: FnMut(Duration),
{
    match send_fn(frame) {
        Ok(()) => {
            *consecutive_failures = 0;
            true
        }
        Err(e) => handle_send_error(e, consecutive_failures, status_tx, sleep_fn),
    }
}

fn compute_stream_tick_interval(state: &StreamState, interval: Duration) -> Duration {
    if state.music.is_some() {
        Duration::from_millis(50)
    } else if state.keys.is_some() {
        interval
    } else {
        match state.effect {
            Effect::Solid(_) => interval,
            _ => Duration::from_millis(40),
        }
    }
}

enum MsgHandleOutcome {
    UpdateAndBreak,
    StopLoop,
}

fn apply_stream_msg<F>(
    msg: WorkerMessage,
    state: &mut StreamState,
    pipeline: &mut MusicAudioPipeline,
    send_fn: &mut F,
) -> MsgHandleOutcome
where
    F: FnMut(&[u8; proto::FRAME_LEN]) -> Result<(), std::io::Error>,
{
    match msg {
        WorkerMessage::SetColor(new_color) => {
            state.music = None;
            pipeline.child = None;
            state.keys = None;
            state.effect = Effect::Solid(RGB(new_color.0, new_color.1, new_color.2));
            state.brightness = 1.0;
            MsgHandleOutcome::UpdateAndBreak
        }
        WorkerMessage::SetEffect { effect, brightness } => {
            state.music = None;
            pipeline.child = None;
            state.keys = None;
            state.effect = effect;
            state.brightness = brightness;
            MsgHandleOutcome::UpdateAndBreak
        }
        WorkerMessage::SetKeyFrame(keys) => {
            state.music = None;
            pipeline.child = None;
            state.keys = Some(keys);
            MsgHandleOutcome::UpdateAndBreak
        }
        WorkerMessage::SetMusic { mode, color, gain } => {
            state.keys = None;
            state.music = Some((mode, color, gain));
            MsgHandleOutcome::UpdateAndBreak
        }
        WorkerMessage::Off => {
            pipeline.child = None;
            if let Ok(off_frame) = proto::build_direct_frame((0, 0, 0)) {
                let _ = send_fn(&off_frame);
            }
            MsgHandleOutcome::StopLoop
        }
        WorkerMessage::Stop => {
            pipeline.child = None;
            MsgHandleOutcome::StopLoop
        }
    }
}

fn wait_stream_tick<F>(
    rx: &Receiver<WorkerMessage>,
    state: &mut StreamState,
    pipeline: &mut MusicAudioPipeline,
    tick_interval: Duration,
    send_fn: &mut F,
) -> bool
where
    F: FnMut(&[u8; proto::FRAME_LEN]) -> Result<(), std::io::Error>,
{
    let start = std::time::Instant::now();
    while start.elapsed() < tick_interval {
        let remaining = tick_interval.saturating_sub(start.elapsed());
        let poll_step = remaining.min(Duration::from_millis(10));
        match rx.recv_timeout(poll_step) {
            Ok(msg) => match apply_stream_msg(msg, state, pipeline, send_fn) {
                MsgHandleOutcome::UpdateAndBreak => break,
                MsgHandleOutcome::StopLoop => return false,
            },
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
    true
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
    let mut state = match handle_initial_cmd(initial_cmd, &status_tx, &mut send_fn) {
        Some(s) => s,
        None => return,
    };

    let mut consecutive_failures = 0;
    let key_slots = proto::all_key_slots();
    let stream_start = std::time::Instant::now();
    let mut pipeline = MusicAudioPipeline::new();

    'outer: loop {
        let t = stream_start.elapsed().as_secs_f64();

        poll_music_audio(&mut state.music, &mut pipeline, &status_tx);

        let frame = match build_stream_frame(&state, &mut pipeline, &status_tx, t, &key_slots) {
            FrameResult::Ok(f) => f,
            FrameResult::DropKeys => {
                state.keys = None;
                continue 'outer;
            }
            FrameResult::Fatal => break 'outer,
        };

        if !transmit_stream_frame(
            &frame,
            &mut consecutive_failures,
            &status_tx,
            &mut send_fn,
            &mut sleep_fn,
        ) {
            break 'outer;
        }

        let tick_interval = compute_stream_tick_interval(&state, interval);
        if !wait_stream_tick(&rx, &mut state, &mut pipeline, tick_interval, &mut send_fn) {
            break 'outer;
        }
    }

    drop(pipeline.child);
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
