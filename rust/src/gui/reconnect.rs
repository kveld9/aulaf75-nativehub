//! Device reconnecting worker loop and production runner for AULA F75 GUI.

use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::cli::music::{self, MusicMode};
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

struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct PlaybackState {
    effect: Effect,
    brightness: f32,
    keys: Option<Vec<(String, (u8, u8, u8))>>,
    music: Option<(MusicMode, (u8, u8, u8), f32)>,
    is_idle: bool,
}

impl PlaybackState {
    fn from_initial(initial_cmd: Option<WorkerMessage>) -> Self {
        match initial_cmd {
            Some(WorkerMessage::SetColor(rgb)) => Self {
                effect: Effect::Solid(RGB(rgb.0, rgb.1, rgb.2)),
                brightness: 1.0,
                keys: None,
                music: None,
                is_idle: false,
            },
            Some(WorkerMessage::SetEffect { effect, brightness }) => Self {
                effect,
                brightness,
                keys: None,
                music: None,
                is_idle: false,
            },
            Some(WorkerMessage::SetKeyFrame(keys)) => Self {
                effect: Effect::Off,
                brightness: 1.0,
                keys: Some(keys),
                music: None,
                is_idle: false,
            },
            Some(WorkerMessage::SetMusic { mode, color, gain }) => Self {
                effect: Effect::Off,
                brightness: 1.0,
                keys: None,
                music: Some((mode, color, gain)),
                is_idle: false,
            },
            Some(WorkerMessage::Off) | Some(WorkerMessage::Stop) | None => Self {
                effect: Effect::Off,
                brightness: 1.0,
                keys: None,
                music: None,
                is_idle: true,
            },
        }
    }

    fn apply_disconnected_msg(&mut self, msg: WorkerMessage) {
        match msg {
            WorkerMessage::Stop | WorkerMessage::Off => {
                self.is_idle = true;
                self.music = None;
                self.keys = None;
                self.effect = Effect::Off;
            }
            WorkerMessage::SetColor(rgb) => {
                self.is_idle = false;
                self.music = None;
                self.keys = None;
                self.effect = Effect::Solid(RGB(rgb.0, rgb.1, rgb.2));
                self.brightness = 1.0;
            }
            WorkerMessage::SetEffect { effect, brightness } => {
                self.is_idle = false;
                self.music = None;
                self.keys = None;
                self.effect = effect;
                self.brightness = brightness;
            }
            WorkerMessage::SetKeyFrame(keys) => {
                self.is_idle = false;
                self.music = None;
                self.keys = Some(keys);
            }
            WorkerMessage::SetMusic { mode, color, gain } => {
                self.is_idle = false;
                self.keys = None;
                self.music = Some((mode, color, gain));
            }
        }
    }
}

fn wait_for_reconnect_msg(
    rx: &Receiver<WorkerMessage>,
    state: &mut PlaybackState,
    reconnect_interval: Duration,
) -> bool {
    let wait_start = Instant::now();
    while wait_start.elapsed() < reconnect_interval {
        let remaining = reconnect_interval.saturating_sub(wait_start.elapsed());
        let poll_step = remaining.min(Duration::from_millis(50));
        match rx.recv_timeout(poll_step) {
            Ok(msg) => state.apply_disconnected_msg(msg),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
    true
}

fn try_connect_device<R, O, D>(
    resolve_fn: &mut R,
    open_fn: &mut O,
    status_tx: &Sender<WorkerStatus>,
    reported_connecting: &mut bool,
    is_idle: bool,
) -> Option<(PathBuf, D)>
where
    R: FnMut() -> Option<PathBuf>,
    O: FnMut(&PathBuf) -> Result<D, io::Error>,
{
    if !*reported_connecting {
        let _ = status_tx.send(WorkerStatus::Connecting);
        *reported_connecting = true;
    }
    let node = resolve_fn()?;
    match open_fn(&node) {
        Ok(dev) => {
            let status = if is_idle {
                WorkerStatus::Connected(node.clone())
            } else {
                WorkerStatus::Running(node.clone())
            };
            let _ = status_tx.send(status);
            *reported_connecting = false;
            Some((node, dev))
        }
        Err(e) => {
            let _ = status_tx.send(WorkerStatus::Error(format!(
                "Failed to open {}: {}",
                node.display(),
                e
            )));
            None
        }
    }
}

fn ensure_device_connected<R, O, D>(
    active_device: &mut Option<(PathBuf, D)>,
    resolve_fn: &mut R,
    open_fn: &mut O,
    status_tx: &Sender<WorkerStatus>,
    reported_connecting: &mut bool,
    state: &mut PlaybackState,
    rx: &Receiver<WorkerMessage>,
    reconnect_interval: Duration,
    consecutive_failures: &mut usize,
) -> bool
where
    R: FnMut() -> Option<PathBuf>,
    O: FnMut(&PathBuf) -> Result<D, io::Error>,
{
    *active_device = try_connect_device(
        resolve_fn,
        open_fn,
        status_tx,
        reported_connecting,
        state.is_idle,
    );
    if active_device.is_some() {
        *consecutive_failures = 0;
        return true;
    }
    wait_for_reconnect_msg(rx, state, reconnect_interval)
}

fn notify_running_if_active<D>(
    active_device: &Option<(PathBuf, D)>,
    status_tx: &Sender<WorkerStatus>,
) {
    if let Some((ref node, _)) = active_device {
        let _ = status_tx.send(WorkerStatus::Running(node.clone()));
    }
}

fn notify_idle_if_active<D>(
    active_device: &Option<(PathBuf, D)>,
    status_tx: &Sender<WorkerStatus>,
) {
    if let Some((ref node, _)) = active_device {
        let _ = status_tx.send(WorkerStatus::IdleConnected(node.clone()));
    }
}

fn send_off_frame_if_active<D, F>(
    active_device: &mut Option<(PathBuf, D)>,
    send_fn: &mut F,
) where
    F: FnMut(&mut D, &[u8; proto::FRAME_LEN]) -> Result<(), io::Error>,
{
    if let Some((_, ref mut dev)) = active_device {
        if let Ok(off_frame) = proto::build_direct_frame((0, 0, 0)) {
            let _ = send_fn(dev, &off_frame);
        }
    }
}

fn handle_idle_msg<D, F>(
    msg: WorkerMessage,
    state: &mut PlaybackState,
    active_device: &mut Option<(PathBuf, D)>,
    status_tx: &Sender<WorkerStatus>,
    send_fn: &mut F,
) -> bool
where
    F: FnMut(&mut D, &[u8; proto::FRAME_LEN]) -> Result<(), io::Error>,
{
    match msg {
        WorkerMessage::SetColor(rgb) => {
            state.effect = Effect::Solid(RGB(rgb.0, rgb.1, rgb.2));
            state.brightness = 1.0;
            state.keys = None;
            state.music = None;
            state.is_idle = false;
            notify_running_if_active(active_device, status_tx);
            true
        }
        WorkerMessage::SetEffect { effect, brightness } => {
            state.effect = effect;
            state.brightness = brightness;
            state.keys = None;
            state.music = None;
            state.is_idle = false;
            notify_running_if_active(active_device, status_tx);
            true
        }
        WorkerMessage::SetKeyFrame(keys) => {
            state.keys = Some(keys);
            state.music = None;
            state.is_idle = false;
            notify_running_if_active(active_device, status_tx);
            true
        }
        WorkerMessage::SetMusic { mode, color, gain } => {
            state.music = Some((mode, color, gain));
            state.keys = None;
            state.is_idle = false;
            notify_running_if_active(active_device, status_tx);
            true
        }
        WorkerMessage::Off => {
            send_off_frame_if_active(active_device, send_fn);
            notify_idle_if_active(active_device, status_tx);
            false
        }
        WorkerMessage::Stop => {
            notify_idle_if_active(active_device, status_tx);
            false
        }
    }
}

fn check_idle_device_disconnect<R, D>(
    active_device: &mut Option<(PathBuf, D)>,
    resolve_fn: &mut R,
    status_tx: &Sender<WorkerStatus>,
    reported_connecting: &mut bool,
) where
    R: FnMut() -> Option<PathBuf>,
{
    if let Some((ref current_node, _)) = active_device {
        let still_present = match resolve_fn() {
            Some(ref node) => node == current_node,
            None => false,
        };
        if !still_present {
            let _ = status_tx.send(WorkerStatus::Disconnected);
            *active_device = None;
            *reported_connecting = false;
        }
    }
}

fn wait_in_idle<R, D, F>(
    rx: &Receiver<WorkerMessage>,
    status_tx: &Sender<WorkerStatus>,
    state: &mut PlaybackState,
    active_device: &mut Option<(PathBuf, D)>,
    reconnect_interval: Duration,
    resolve_fn: &mut R,
    send_fn: &mut F,
    reported_connecting: &mut bool,
) -> Result<bool, ()>
where
    R: FnMut() -> Option<PathBuf>,
    F: FnMut(&mut D, &[u8; proto::FRAME_LEN]) -> Result<(), io::Error>,
{
    let wait_start = Instant::now();
    while wait_start.elapsed() < reconnect_interval {
        let remaining = reconnect_interval.saturating_sub(wait_start.elapsed());
        let poll_step = remaining.min(Duration::from_millis(50));
        match rx.recv_timeout(poll_step) {
            Ok(msg) => {
                if handle_idle_msg(msg, state, active_device, status_tx, send_fn) {
                    return Ok(true);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(()),
        }
    }

    check_idle_device_disconnect(active_device, resolve_fn, status_tx, reported_connecting);
    Ok(false)
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

enum FrameStepError {
    DropDevice,
    DropKeys,
}

fn build_frame_step(
    state: &PlaybackState,
    pipeline: &mut MusicAudioPipeline,
    status_tx: &Sender<WorkerStatus>,
    t: f64,
    key_slots: &[u8],
) -> Result<[u8; proto::FRAME_LEN], FrameStepError> {
    if let Some((mode, color, _)) = state.music {
        match build_music_frame(mode, color, pipeline, status_tx) {
            Ok(f) => Ok(f),
            Err(e) => {
                let _ = status_tx.send(WorkerStatus::Error(e));
                Err(FrameStepError::DropDevice)
            }
        }
    } else if let Some(ref keys) = state.keys {
        match render_keys_frame(keys) {
            Ok(f) => Ok(f),
            Err(e) => {
                let _ = status_tx.send(WorkerStatus::Error(e));
                Err(FrameStepError::DropKeys)
            }
        }
    } else {
        match render_effect_frame(&state.effect, state.brightness, t, key_slots) {
            Ok(f) => Ok(f),
            Err(e) => {
                let _ = status_tx.send(WorkerStatus::Error(e));
                Err(FrameStepError::DropDevice)
            }
        }
    }
}

fn handle_transmit_error<S>(
    e: io::Error,
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
            let _ = status_tx.send(WorkerStatus::Disconnected);
            *consecutive_failures = 0;
            false
        } else {
            let backoff = Duration::from_millis(10 * *consecutive_failures as u64);
            sleep_fn(backoff);
            true
        }
    } else {
        let _ = status_tx.send(WorkerStatus::Error(format!(
            "Device communication error: {}",
            e
        )));
        let _ = status_tx.send(WorkerStatus::Disconnected);
        *consecutive_failures = 0;
        false
    }
}

fn transmit_frame<D, F, S>(
    dev: &mut D,
    frame: &[u8; proto::FRAME_LEN],
    consecutive_failures: &mut usize,
    status_tx: &Sender<WorkerStatus>,
    send_fn: &mut F,
    sleep_fn: &mut S,
) -> bool
where
    F: FnMut(&mut D, &[u8; proto::FRAME_LEN]) -> Result<(), io::Error>,
    S: FnMut(Duration),
{
    match send_fn(dev, frame) {
        Ok(()) => {
            *consecutive_failures = 0;
            true
        }
        Err(e) => handle_transmit_error(e, consecutive_failures, status_tx, sleep_fn),
    }
}

fn transmit_frame_if_connected<D, F, S>(
    active_device: &mut Option<(PathBuf, D)>,
    frame: &[u8; proto::FRAME_LEN],
    consecutive_failures: &mut usize,
    status_tx: &Sender<WorkerStatus>,
    send_fn: &mut F,
    sleep_fn: &mut S,
) where
    F: FnMut(&mut D, &[u8; proto::FRAME_LEN]) -> Result<(), io::Error>,
    S: FnMut(Duration),
{
    if let Some((_, ref mut dev)) = active_device {
        if !transmit_frame(dev, frame, consecutive_failures, status_tx, send_fn, sleep_fn) {
            *active_device = None;
        }
    }
}

fn compute_tick_interval(state: &PlaybackState, keepalive_interval: Duration) -> Duration {
    if state.music.is_some() {
        Duration::from_millis(50)
    } else if state.keys.is_some() {
        keepalive_interval
    } else {
        match state.effect {
            Effect::Solid(_) => keepalive_interval,
            _ => Duration::from_millis(40),
        }
    }
}

fn apply_streaming_msg<D, F>(
    msg: WorkerMessage,
    state: &mut PlaybackState,
    pipeline: &mut MusicAudioPipeline,
    active_device: &mut Option<(PathBuf, D)>,
    status_tx: &Sender<WorkerStatus>,
    send_fn: &mut F,
) where
    F: FnMut(&mut D, &[u8; proto::FRAME_LEN]) -> Result<(), io::Error>,
{
    match msg {
        WorkerMessage::SetColor(new_color) => {
            state.music = None;
            pipeline.child = None;
            state.keys = None;
            state.effect = Effect::Solid(RGB(new_color.0, new_color.1, new_color.2));
            state.brightness = 1.0;
        }
        WorkerMessage::SetEffect { effect, brightness } => {
            state.music = None;
            pipeline.child = None;
            state.keys = None;
            state.effect = effect;
            state.brightness = brightness;
        }
        WorkerMessage::SetKeyFrame(keys) => {
            state.music = None;
            pipeline.child = None;
            state.keys = Some(keys);
        }
        WorkerMessage::SetMusic { mode, color, gain } => {
            state.keys = None;
            state.music = Some((mode, color, gain));
        }
        WorkerMessage::Off => {
            pipeline.child = None;
            send_off_frame_if_active(active_device, send_fn);
            state.is_idle = true;
            notify_idle_if_active(active_device, status_tx);
        }
        WorkerMessage::Stop => {
            pipeline.child = None;
            state.is_idle = true;
            notify_idle_if_active(active_device, status_tx);
        }
    }
}

fn wait_streaming_tick<D, F>(
    rx: &Receiver<WorkerMessage>,
    status_tx: &Sender<WorkerStatus>,
    state: &mut PlaybackState,
    pipeline: &mut MusicAudioPipeline,
    active_device: &mut Option<(PathBuf, D)>,
    tick_interval: Duration,
    send_fn: &mut F,
) -> bool
where
    F: FnMut(&mut D, &[u8; proto::FRAME_LEN]) -> Result<(), io::Error>,
{
    let start = Instant::now();
    while start.elapsed() < tick_interval {
        let remaining = tick_interval.saturating_sub(start.elapsed());
        let poll_step = remaining.min(Duration::from_millis(10));
        match rx.recv_timeout(poll_step) {
            Ok(msg) => {
                apply_streaming_msg(msg, state, pipeline, active_device, status_tx, send_fn);
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
    true
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
    let mut state = PlaybackState::from_initial(initial_cmd.into());
    let mut active_device: Option<(PathBuf, D)> = None;
    let mut reported_connecting = false;
    let mut consecutive_failures = 0;
    let key_slots = proto::all_key_slots();
    let stream_start = Instant::now();
    let mut pipeline = MusicAudioPipeline::new();

    'outer: loop {
        // 1. Device resolution & opening when not connected
        if active_device.is_none() {
            if !ensure_device_connected(
                &mut active_device,
                &mut resolve_fn,
                &mut open_fn,
                &status_tx,
                &mut reported_connecting,
                &mut state,
                &rx,
                reconnect_interval,
                &mut consecutive_failures,
            ) {
                break 'outer;
            }
            if active_device.is_none() {
                continue 'outer;
            }
        }

        // 2. Idle / connect-only mode
        if state.is_idle {
            match wait_in_idle(
                &rx,
                &status_tx,
                &mut state,
                &mut active_device,
                reconnect_interval,
                &mut resolve_fn,
                &mut send_fn,
                &mut reported_connecting,
            ) {
                Ok(_) => continue 'outer,
                Err(()) => break 'outer,
            }
        }

        let t = stream_start.elapsed().as_secs_f64();

        // 3. Audio monitor child process for music mode
        poll_music_audio(&mut state.music, &mut pipeline, &status_tx);

        // 4. Build RGB frame
        let frame = match build_frame_step(&state, &mut pipeline, &status_tx, t, &key_slots) {
            Ok(f) => f,
            Err(FrameStepError::DropDevice) => {
                active_device = None;
                continue 'outer;
            }
            Err(FrameStepError::DropKeys) => {
                state.keys = None;
                continue 'outer;
            }
        };

        // 5. Transmit frame to open device
        transmit_frame_if_connected(
            &mut active_device,
            &frame,
            &mut consecutive_failures,
            &status_tx,
            &mut send_fn,
            &mut sleep_fn,
        );

        // 6. Calculate tick interval and process channel messages
        let tick_interval = compute_tick_interval(&state, keepalive_interval);
        if !wait_streaming_tick(
            &rx,
            &status_tx,
            &mut state,
            &mut pipeline,
            &mut active_device,
            tick_interval,
            &mut send_fn,
        ) {
            break 'outer;
        }
    }

    drop(pipeline.child);
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

