//! System audio-reactive lighting engine for AULA F75 keyboard.
//!
//! Captures system audio from a PulseAudio/PipeWire monitor stream via `parec`,
//! performs real-time Cooley-Tukey FFT analysis with inline Hann windowing,
//! computes log-spaced frequency band energies with automatic gain control,
//! and maps them to Report 6 direct RGB frames.

use std::collections::VecDeque;
use std::f32::consts::PI;
use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};

use crate::proto::{self, FRAME_LEN, KEY_SLOTS};

/// Lighting pattern mode for music reactivity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicMode {
    /// Frequency spectrum visualized as columns across physical key slots.
    Bars,
    /// Whole-board illumination pulsed by overall audio volume level.
    Pulse,
}

impl FromStr for MusicMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "bars" => Ok(MusicMode::Bars),
            "pulse" => Ok(MusicMode::Pulse),
            other => Err(format!(
                "Unknown music mode: {:?}. Valid modes: bars, pulse",
                other
            )),
        }
    }
}

impl std::fmt::Display for MusicMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MusicMode::Bars => write!(f, "bars"),
            MusicMode::Pulse => write!(f, "pulse"),
        }
    }
}

/// Audio capture configuration settings.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioConfig {
    /// Audio sample rate in Hz (default: 44100).
    pub rate: u32,
    /// Number of audio channels (default: 2 for stereo).
    pub channels: u16,
    /// Input volume gain multiplier (default: 1.0).
    pub gain: f32,
    /// Visualization mode (Bars or Pulse).
    pub mode: MusicMode,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            rate: 44100,
            channels: 2,
            gain: 1.0,
            mode: MusicMode::Bars,
        }
    }
}

/// Resolve path to `pactl` binary, supporting test overrides via `AULA_F75_PACTL`.
pub fn resolve_pactl_bin() -> String {
    std::env::var("AULA_F75_PACTL").unwrap_or_else(|_| "pactl".to_string())
}

/// Resolve path to `parec` binary, supporting test overrides via `AULA_F75_PAREC`.
pub fn resolve_parec_bin() -> String {
    std::env::var("AULA_F75_PAREC").unwrap_or_else(|_| "parec".to_string())
}

/// Spawn PulseAudio/PipeWire monitor recording child process.
///
/// Queries the default sink using `pactl get-default-sink`, resolves its `.monitor`
/// source name, and spawns `parec --format=s16le --rate=44100 --channels=2 --latency-msec=50 -d <monitor>`.
///
/// Returns a clear error message instructing the user to install `pulseaudio-utils` or
/// `pipewire-pulse` if either tool is missing or fails.
pub fn spawn_monitor() -> Result<Child, String> {
    let pactl_bin = resolve_pactl_bin();
    let parec_bin = resolve_parec_bin();

    // 1. Resolve default audio sink name via pactl
    let pactl_output = Command::new(&pactl_bin)
        .arg("get-default-sink")
        .output()
        .map_err(|e| {
            format!(
                "Failed to execute pactl (install pulseaudio-utils / pipewire-pulse): {}",
                e
            )
        })?;

    if !pactl_output.status.success() {
        let err_msg = String::from_utf8_lossy(&pactl_output.stderr);
        return Err(format!(
            "pactl get-default-sink failed (install pulseaudio-utils / pipewire-pulse): {}",
            err_msg.trim()
        ));
    }

    let default_sink = String::from_utf8_lossy(&pactl_output.stdout).trim().to_string();
    if default_sink.is_empty() {
        return Err(
            "pactl returned empty default sink (ensure audio server is running)".to_string(),
        );
    }

    let monitor = format!("{}.monitor", default_sink);

    // 2. Spawn parec monitor recording child process
    let child = Command::new(&parec_bin)
        .arg("--format=s16le")
        .arg("--rate=44100")
        .arg("--channels=2")
        .arg("--latency-msec=50")
        .arg("-d")
        .arg(&monitor)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            format!(
                "Failed to execute parec (install pulseaudio-utils / pipewire-pulse): {}",
                e
            )
        })?;

    Ok(child)
}

/// Precomputed tables for power-of-two iterative Cooley-Tukey Radix-2 FFT.
#[derive(Debug, Clone)]
pub struct FftPlan {
    pub n: usize,
    pub bit_reversed: Vec<usize>,
    pub hann_window: Vec<f32>,
    pub twiddles: Vec<Vec<(f32, f32)>>,
}

impl FftPlan {
    /// Construct FFT tables for block size `n` (must be a power of two).
    pub fn new(n: usize) -> Self {
        assert!(
            n > 0 && n.is_power_of_two(),
            "FFT block size must be a power of two"
        );
        let k = n.trailing_zeros();

        let mut bit_reversed = Vec::with_capacity(n);
        for i in 0..n {
            let mut rev = 0;
            for b in 0..k {
                if (i & (1 << b)) != 0 {
                    rev |= 1 << (k - 1 - b);
                }
            }
            bit_reversed.push(rev);
        }

        // Hann window: w[i] = 0.5 * (1.0 - cos(2*pi*i / n))
        let mut hann_window = Vec::with_capacity(n);
        for i in 0..n {
            let w = 0.5 * (1.0 - (2.0 * PI * i as f32 / n as f32).cos());
            hann_window.push(w);
        }

        // Twiddle factors: W_len^j = e^(-2*pi*i*j / len) -> (cos, sin)
        let mut twiddles = Vec::new();
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let angle_step = -2.0 * PI / len as f32;
            let mut stage_twiddles = Vec::with_capacity(half);
            for j in 0..half {
                let (sin, cos) = (j as f32 * angle_step).sin_cos();
                stage_twiddles.push((cos, sin));
            }
            twiddles.push(stage_twiddles);
            len *= 2;
        }

        Self {
            n,
            bit_reversed,
            hann_window,
            twiddles,
        }
    }

    /// Compute real-to-complex FFT magnitudes using iterative Cooley-Tukey.
    pub fn compute(&self, samples: &[f32], out: &mut [f32]) {
        let n = self.n;
        let block_len = samples.len().min(n);

        let mut re = vec![0.0f32; n];
        let mut im = vec![0.0f32; n];

        // Apply bit-reversal and Hann window
        for i in 0..block_len {
            let rev = self.bit_reversed[i];
            re[rev] = samples[i] * self.hann_window[i];
            im[rev] = 0.0;
        }

        // Butterfly stages
        let mut stage = 0;
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let stage_tw = &self.twiddles[stage];
            for i in (0..n).step_by(len) {
                for j in 0..half {
                    let (w_cos, w_sin) = stage_tw[j];
                    let u_re = re[i + j];
                    let u_im = im[i + j];
                    let v_re = re[i + j + half] * w_cos - im[i + j + half] * w_sin;
                    let v_im = re[i + j + half] * w_sin + im[i + j + half] * w_cos;

                    re[i + j] = u_re + v_re;
                    im[i + j] = u_im + v_im;
                    re[i + j + half] = u_re - v_re;
                    im[i + j + half] = u_im - v_im;
                }
            }
            stage += 1;
            len *= 2;
        }

        // Fill frequency bin magnitudes
        let num_bins = out.len().min(n / 2);
        for k in 0..num_bins {
            out[k] = (re[k] * re[k] + im[k] * im[k]).sqrt();
        }
    }
}

/// Iterative radix-2 Cooley-Tukey FFT with inline Hann windowing using standard library functions only.
///
/// Computes non-redundant frequency bin magnitudes from real-valued samples.
pub fn fft_magnitudes(samples: &[f32], out: &mut [f32]) {
    if samples.is_empty() || out.is_empty() {
        return;
    }
    let n = samples.len();
    if !n.is_power_of_two() {
        let p = 1 << (usize::BITS - 1 - n.leading_zeros());
        let plan = FftPlan::new(p);
        plan.compute(&samples[..p], out);
        return;
    }
    let plan = FftPlan::new(n);
    plan.compute(samples, out);
}

/// Decaying peak tracker for automatic gain control.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoGain {
    pub peak: f32,
}

impl Default for AutoGain {
    fn default() -> Self {
        Self { peak: 1.0 }
    }
}

impl AutoGain {
    /// Create a new AutoGain tracker initialized to 1.0.
    pub fn new() -> Self {
        Self::default()
    }

    /// Update decaying peak with `x` and return normalized value `x / peak`.
    ///
    /// The peak decays at a rate of 0.995 per tick and is bounded below by 1e-4.
    pub fn normalize(&mut self, x: f32) -> f32 {
        self.peak = (self.peak * 0.995).max(x).max(1e-4);
        (x / self.peak).clamp(0.0, 1.0)
    }
}

/// Calculate log-spaced frequency band energies between 40Hz and 12kHz normalized to 0..1 with AutoGain.
pub fn bands_with_autogain(
    mags: &[f32],
    n_bands: usize,
    sample_rate: f32,
    autogain: &mut AutoGain,
) -> Vec<f32> {
    if n_bands == 0 || mags.is_empty() || sample_rate <= 0.0 {
        return vec![0.0; n_bands];
    }

    let f_min = 40.0f32;
    let f_max = 12000.0f32.min(sample_rate * 0.5);
    if f_min >= f_max {
        return vec![0.0; n_bands];
    }

    let fft_points = (2 * mags.len()) as f32;
    let bin_width = sample_rate / fft_points;
    let log_ratio = (f_max / f_min).ln() / n_bands as f32;

    let mut raw_energies = Vec::with_capacity(n_bands);

    for b in 0..n_bands {
        let f_low = f_min * (b as f32 * log_ratio).exp();
        let f_high = f_min * ((b + 1) as f32 * log_ratio).exp();

        let k_start = ((f_low / bin_width).floor() as usize).min(mags.len().saturating_sub(1));
        let mut k_end = ((f_high / bin_width).ceil() as usize).min(mags.len());
        if k_end <= k_start {
            k_end = (k_start + 1).min(mags.len());
        }

        let mut sum = 0.0f32;
        let count = (k_end - k_start) as f32;
        for &m in &mags[k_start..k_end] {
            sum += m;
        }
        let energy = if count > 0.0 { sum / count } else { 0.0 };
        raw_energies.push(energy);
    }

    let frame_max = raw_energies.iter().copied().fold(0.0f32, f32::max);
    autogain.normalize(frame_max);

    let peak = autogain.peak.max(1e-4);
    raw_energies
        .into_iter()
        .map(|e| (e / peak).clamp(0.0, 1.0))
        .collect()
}

thread_local! {
    static GLOBAL_AUTOGAIN: std::cell::RefCell<AutoGain> = std::cell::RefCell::new(AutoGain::new());
}

/// Pure helper computing log-spaced band energies normalized with thread-local AutoGain.
pub fn bands(mags: &[f32], n_bands: usize, sample_rate: f32) -> Vec<f32> {
    GLOBAL_AUTOGAIN.with(|ag| bands_with_autogain(mags, n_bands, sample_rate, &mut ag.borrow_mut()))
}

/// Map frequency band energies to `(slot, (r, g, b))` pairs for Report 6 frames.
///
/// - `Bars`: Maps key slots across `KEY_SLOTS` order to frequency bands (`slot_i = band[i * nbands / nslots]`).
/// - `Pulse`: Scales whole-board base color by overall volume energy.
pub fn bands_to_colors(
    bands: &[f32],
    base: (u8, u8, u8),
    mode: MusicMode,
) -> Vec<(u8, (u8, u8, u8))> {
    let key_slots = KEY_SLOTS;
    let nslots = key_slots.len();
    let nbands = bands.len();

    let mut result = Vec::with_capacity(nslots);

    match mode {
        MusicMode::Bars => {
            for (i, &(_, slot)) in key_slots.iter().enumerate() {
                let brightness = if nbands > 0 {
                    let band_idx = (i * nbands / nslots).min(nbands - 1);
                    bands[band_idx].clamp(0.0, 1.0)
                } else {
                    0.0
                };

                let r = (base.0 as f32 * brightness).round().clamp(0.0, 255.0) as u8;
                let g = (base.1 as f32 * brightness).round().clamp(0.0, 255.0) as u8;
                let b = (base.2 as f32 * brightness).round().clamp(0.0, 255.0) as u8;
                result.push((slot, (r, g, b)));
            }
        }
        MusicMode::Pulse => {
            let overall = if nbands > 0 {
                bands.iter().sum::<f32>() / nbands as f32
            } else {
                0.0
            };
            let brightness = overall.clamp(0.0, 1.0);

            let r = (base.0 as f32 * brightness).round().clamp(0.0, 255.0) as u8;
            let g = (base.1 as f32 * brightness).round().clamp(0.0, 255.0) as u8;
            let b = (base.2 as f32 * brightness).round().clamp(0.0, 255.0) as u8;

            for &(_, slot) in key_slots.iter() {
                result.push((slot, (r, g, b)));
            }
        }
    }

    result
}

fn is_music_time_expired(start: Instant, seconds: Option<f64>) -> bool {
    match seconds {
        Some(sec) => start.elapsed().as_secs_f64() >= sec,
        None => false,
    }
}

enum AudioReadResult {
    Data(usize),
    Eof,
    Retry,
}

fn read_audio_chunk<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<AudioReadResult, io::Error> {
    match reader.read(buf) {
        Ok(0) => Ok(AudioReadResult::Eof),
        Ok(n) => Ok(AudioReadResult::Data(n)),
        Err(ref e) if e.kind() == io::ErrorKind::Interrupted => Ok(AudioReadResult::Retry),
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
            std::thread::sleep(Duration::from_millis(5));
            Ok(AudioReadResult::Retry)
        }
        Err(e) => Err(e),
    }
}

fn ingest_audio_samples(
    sample_buf: &mut VecDeque<f32>,
    chunk_buf: &[u8],
    bytes_read: usize,
    gain: f32,
) {
    let num_stereo_samples = bytes_read / 4;
    for i in 0..num_stereo_samples {
        let offset = i * 4;
        let left = i16::from_le_bytes([chunk_buf[offset], chunk_buf[offset + 1]]);
        let right = i16::from_le_bytes([chunk_buf[offset + 2], chunk_buf[offset + 3]]);
        let mono = ((left as f32 + right as f32) / (2.0 * 32768.0)) * gain;
        sample_buf.push_back(mono);
    }

    while sample_buf.len() > 1024 {
        sample_buf.pop_front();
    }
}

fn is_music_tick_due(last_tick: Instant, sample_count: usize) -> bool {
    sample_count >= 1024 && last_tick.elapsed() >= Duration::from_millis(45)
}

fn render_music_frame(
    sample_buf: &VecDeque<f32>,
    plan: &FftPlan,
    mags: &mut [f32],
    autogain: &mut AutoGain,
    base_color: (u8, u8, u8),
    mode: MusicMode,
) -> Result<[u8; FRAME_LEN], io::Error> {
    let block: Vec<f32> = sample_buf.iter().copied().collect();
    plan.compute(&block, mags);

    let band_energies = bands_with_autogain(mags, 16, 44100.0, autogain);
    let pairs = bands_to_colors(&band_energies, base_color, mode);
    proto::build_slot_frame(&pairs).map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}

/// Generic audio capture streaming loop.
///
/// Reads `s16le` stereo samples from `reader`, mixes them to mono `f32`,
/// maintains a sliding 1024-block buffer, executes FFT at ~20 fps, and transmits
/// Report 6 frames via `send_fn`.
pub fn run_music_loop<R, F, C>(
    reader: &mut R,
    mut send_fn: F,
    mode: MusicMode,
    base_color: (u8, u8, u8),
    gain: f32,
    seconds: Option<f64>,
    is_running_fn: C,
) -> Result<(), io::Error>
where
    R: Read,
    F: FnMut(&[u8; FRAME_LEN]) -> Result<(), io::Error>,
    C: Fn() -> bool,
{
    let plan = FftPlan::new(1024);
    let mut sample_buf = VecDeque::with_capacity(2048);
    let mut mags = vec![0.0f32; 512];
    let mut autogain = AutoGain::new();

    // Buffer for ~50ms worth of stereo 44.1kHz s16le data (2205 frames * 4 bytes = 8820 bytes)
    let mut chunk_buf = [0u8; 8820];
    let start_time = Instant::now();
    let mut last_tick = Instant::now()
        .checked_sub(Duration::from_millis(100))
        .unwrap_or_else(Instant::now);

    while is_running_fn() {
        if is_music_time_expired(start_time, seconds) {
            break;
        }

        match read_audio_chunk(reader, &mut chunk_buf)? {
            AudioReadResult::Eof => break,
            AudioReadResult::Retry => continue,
            AudioReadResult::Data(n) => ingest_audio_samples(&mut sample_buf, &chunk_buf, n, gain),
        }

        if is_music_tick_due(last_tick, sample_buf.len()) {
            last_tick = Instant::now();

            let frame = render_music_frame(
                &sample_buf,
                &plan,
                &mut mags,
                &mut autogain,
                base_color,
                mode,
            )?;
            send_fn(&frame)?;
        }
    }

    Ok(())
}
