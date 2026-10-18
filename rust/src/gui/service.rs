//! Background systemd service coordination for AULA F75 GUI.
//!
//! Provides single-writer coordination by parking the background `aula-f75.service`
//! while the desktop GUI runs, and restoring it when the GUI closes.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::cli::music::MusicMode;
use crate::effects::Effect;
use crate::gui::state::GuiLightingMode;

/// Default systemd user service unit name for AULA F75.
pub const DEFAULT_SERVICE_NAME: &str = "aula-f75.service";

/// Default timeout for systemctl commands to prevent blocking the UI.
pub const DEFAULT_SYSTEMCTL_TIMEOUT: Duration = Duration::from_millis(300);

/// Trait for inspecting and controlling background service lifecycle.
pub trait ServiceRunner: Send {
    /// Check whether the service is currently running/active.
    fn check_active(&mut self) -> bool;
    /// Stop the background service.
    fn stop(&mut self);
    /// Start the background service.
    fn start(&mut self);
    /// Reload systemd manager configuration.
    fn daemon_reload(&mut self) {}
    /// Enable the service unit.
    fn enable(&mut self) {}
    /// Restart the background service.
    fn restart(&mut self) {}
}

impl<T: ?Sized + ServiceRunner> ServiceRunner for Box<T> {
    fn check_active(&mut self) -> bool {
        (**self).check_active()
    }
    fn stop(&mut self) {
        (**self).stop();
    }
    fn start(&mut self) {
        (**self).start();
    }
    fn daemon_reload(&mut self) {
        (**self).daemon_reload();
    }
    fn enable(&mut self) {
        (**self).enable();
    }
    fn restart(&mut self) {
        (**self).restart();
    }
}

/// Suspend the background service if it is currently active.
///
/// Returns `true` if the service was active and a stop command was issued,
/// or `false` if the service was inactive, unavailable, or an error occurred.
pub fn suspend_for_gui<R: ServiceRunner + ?Sized>(runner: &mut R) -> bool {
    let is_active = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runner.check_active()))
        .unwrap_or(false);
    if is_active {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runner.stop()));
        true
    } else {
        false
    }
}

/// Restore the background service if it was previously suspended for the GUI.
///
/// If `was_active` is `true`, issues a start command to resume background operation.
/// Swallows errors and panics to ensure shutdown never crashes.
pub fn restore_after_gui<R: ServiceRunner + ?Sized>(runner: &mut R, was_active: bool) {
    if was_active {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runner.start()));
    }
}

/// Map the last GUI lighting state, color, brightness, and speed to standalone CLI arguments.
///
/// Output CLI commands are designed to run indefinitely (omitting `--seconds`) so that
/// the systemd background service keeps the lighting active.
pub fn service_args_for_state(
    mode: &GuiLightingMode,
    rgb: (u8, u8, u8),
    brightness: f32,
    speed: f32,
) -> Vec<String> {
    match mode {
        GuiLightingMode::Solid => vec!["solid".to_string(), format_rgb(rgb)],
        GuiLightingMode::Off => vec!["solid".to_string(), "000000".to_string()],
        GuiLightingMode::Effect(effect) => fx_args_for_effect(effect, rgb, brightness, speed),
        GuiLightingMode::Music { mode, gain } => music_args(*mode, rgb, *gain),
    }
}

fn format_rgb(rgb: (u8, u8, u8)) -> String {
    format!("{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2)
}

fn fx_args_for_effect(
    effect: &Effect,
    rgb: (u8, u8, u8),
    brightness: f32,
    speed: f32,
) -> Vec<String> {
    let (name, takes_color) = effect_name_and_color_policy(effect);
    let mut args = vec!["fx".to_string(), name.to_string()];
    if takes_color {
        args.push("--color".to_string());
        args.push(format_rgb(rgb));
    }
    args.push("--brightness".to_string());
    args.push(format!("{}", brightness));
    args.push("--speed".to_string());
    args.push(format!("{}", speed));
    args
}

fn effect_name_and_color_policy(effect: &Effect) -> (&'static str, bool) {
    match effect {
        Effect::Solid(_) => ("solid", true),
        Effect::Breathe { .. } => ("breathe", true),
        Effect::Rainbow { .. } => ("rainbow", false),
        Effect::Wave { .. } => ("wave", false),
        Effect::Stars { .. } => ("stars", false),
        Effect::Off => ("off", false),
    }
}

fn music_args(mode: MusicMode, rgb: (u8, u8, u8), gain: f32) -> Vec<String> {
    vec![
        "music".to_string(),
        "--mode".to_string(),
        mode.to_string(),
        "--color".to_string(),
        format_rgb(rgb),
        "--gain".to_string(),
        format!("{}", gain),
    ]
}

/// Resolve the systemd user service directory.
///
/// Priority:
/// 1. `custom_dir` if provided (for tests)
/// 2. `$XDG_CONFIG_HOME/systemd/user`
/// 3. `$HOME/.config/systemd/user`
pub fn resolve_systemd_user_dir(custom_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = custom_dir {
        return Some(dir.to_path_buf());
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let trimmed = xdg.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join("systemd").join("user"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join(".config").join("systemd").join("user"));
        }
    }
    None
}

/// Resolve the path to `aula-f75.service` user unit file.
pub fn resolve_service_unit_path(custom_dir: Option<&Path>) -> Option<PathBuf> {
    resolve_systemd_user_dir(custom_dir).map(|d| d.join(DEFAULT_SERVICE_NAME))
}

fn format_exec_start_line(cli_args: &[String]) -> String {
    if cli_args.is_empty() {
        "ExecStart=%h/.local/bin/aula-f75-rs".to_string()
    } else {
        format!("ExecStart=%h/.local/bin/aula-f75-rs {}", cli_args.join(" "))
    }
}

fn rewrite_unit_lines(content: &str, new_exec: &str) -> String {
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut found = false;
    for line in &mut lines {
        if line.trim_start().starts_with("ExecStart=") {
            *line = new_exec.to_string();
            found = true;
            break;
        }
    }
    if !found {
        insert_exec_start(&mut lines, new_exec);
    }
    let mut result = lines.join("\n");
    if content.ends_with('\n') || result.is_empty() {
        result.push('\n');
    }
    result
}

fn insert_exec_start(lines: &mut Vec<String>, new_exec: &str) {
    if let Some(idx) = lines.iter().position(|l| l.trim() == "[Service]") {
        lines.insert(idx + 1, new_exec.to_string());
    } else {
        lines.push(new_exec.to_string());
    }
}

fn create_default_unit_content(exec_start: &str) -> String {
    format!(
        "[Unit]\nDescription=AULA F75 RGB Lighting Service\nAfter=default.target\n\n[Service]\nType=simple\n{}\nRestart=on-failure\nRestartSec=3s\n\n[Install]\nWantedBy=default.target\n",
        exec_start
    )
}

fn write_unit_file_atomic(unit_path: &Path, cli_args: &[String]) -> io::Result<()> {
    let new_exec = format_exec_start_line(cli_args);
    let new_content = if unit_path.is_file() {
        let existing = std::fs::read_to_string(unit_path)?;
        rewrite_unit_lines(&existing, &new_exec)
    } else {
        create_default_unit_content(&new_exec)
    };

    let parent = unit_path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    let pid = std::process::id();
    let filename = unit_path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("aula-f75.service");
    let temp_path = parent.join(format!(".{}.tmp.{}", filename, pid));

    std::fs::write(&temp_path, new_content.as_bytes())?;
    if let Err(e) = std::fs::rename(&temp_path, unit_path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e);
    }
    Ok(())
}

/// Adopt the active GUI lighting configuration into the background systemd service.
///
/// Rewrites `ExecStart` in the unit file to `%h/.local/bin/aula-f75-rs <args>`, preserving
/// all other lines (or appending if missing), performs an atomic write via a temp file,
/// and reloads, enables, and restarts the service via the provided runner.
///
/// All errors and panics are swallowed to guarantee shutdown never crashes.
pub fn adopt_service_command<R: ServiceRunner + ?Sized>(
    unit_path: &Path,
    cli_args: &[String],
    runner: &mut R,
) -> bool {
    let rewrite_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        write_unit_file_atomic(unit_path, cli_args)
    }));
    let write_ok = match rewrite_res {
        Ok(Ok(())) => true,
        _ => false,
    };

    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runner.daemon_reload()));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runner.enable()));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runner.restart()));

    write_ok
}

/// Runner wrapping `check_active`, `stop`, and `start` closures.
pub struct ClosureServiceRunner<C, S, T> {
    check_fn: C,
    stop_fn: S,
    start_fn: T,
}

impl<C, S, T> ClosureServiceRunner<C, S, T>
where
    C: FnMut() -> bool + Send,
    S: FnMut() + Send,
    T: FnMut() + Send,
{
    pub fn new(check_fn: C, stop_fn: S, start_fn: T) -> Self {
        Self {
            check_fn,
            stop_fn,
            start_fn,
        }
    }
}

impl<C, S, T> ServiceRunner for ClosureServiceRunner<C, S, T>
where
    C: FnMut() -> bool + Send,
    S: FnMut() + Send,
    T: FnMut() + Send,
{
    fn check_active(&mut self) -> bool {
        (self.check_fn)()
    }
    fn stop(&mut self) {
        (self.stop_fn)();
    }
    fn start(&mut self) {
        (self.start_fn)();
    }
}

/// Helper function to reap a finished or killed child process.
fn reap_child(mut child: std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Helper function to poll child process exit within a given timeout.
fn poll_child_exit(
    child: &mut std::process::Child,
    timeout: Duration,
) -> io::Result<Option<i32>> {
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(10);
    while start.elapsed() < timeout {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.code()),
            Ok(None) => std::thread::sleep(poll_interval),
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

/// Spawn command and wait for completion with a timeout.
fn run_process_command(
    cmd_name: &str,
    args: &[&str],
    timeout: Duration,
) -> io::Result<Option<i32>> {
    let mut child = std::process::Command::new(cmd_name)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    match poll_child_exit(&mut child, timeout) {
        Ok(Some(code)) => Ok(Some(code)),
        Ok(None) => {
            reap_child(child);
            Ok(None)
        }
        Err(e) => {
            reap_child(child);
            Err(e)
        }
    }
}

pub type CommandExecutor =
    Box<dyn FnMut(&str, &[&str], Duration) -> io::Result<Option<i32>> + Send>;

/// Real systemd service runner executing `systemctl --user` commands.
pub struct SystemdServiceRunner {
    unit: String,
    timeout: Duration,
    executor: Option<CommandExecutor>,
}

impl Default for SystemdServiceRunner {
    fn default() -> Self {
        Self::new(DEFAULT_SERVICE_NAME)
    }
}

impl SystemdServiceRunner {
    /// Create a new runner targeting the specified service unit.
    pub fn new(unit: impl Into<String>) -> Self {
        Self {
            unit: unit.into(),
            timeout: DEFAULT_SYSTEMCTL_TIMEOUT,
            executor: None,
        }
    }

    /// Create a runner with a custom timeout.
    pub fn with_timeout(unit: impl Into<String>, timeout: Duration) -> Self {
        Self {
            unit: unit.into(),
            timeout,
            executor: None,
        }
    }

    /// Create a runner with an injectable command executor for tests.
    pub fn with_executor<E>(unit: impl Into<String>, timeout: Duration, executor: E) -> Self
    where
        E: FnMut(&str, &[&str], Duration) -> io::Result<Option<i32>> + Send + 'static,
    {
        Self {
            unit: unit.into(),
            timeout,
            executor: Some(Box::new(executor)),
        }
    }

    fn run_systemctl_args(&mut self, args: &[&str]) -> io::Result<Option<i32>> {
        if let Some(ref mut exec) = self.executor {
            exec("systemctl", args, self.timeout)
        } else {
            run_process_command("systemctl", args, self.timeout)
        }
    }

    fn run_systemctl(&mut self, action: &str) -> io::Result<Option<i32>> {
        let unit = self.unit.clone();
        let args = ["--user", action, unit.as_str()];
        self.run_systemctl_args(&args)
    }
}

impl ServiceRunner for SystemdServiceRunner {
    fn check_active(&mut self) -> bool {
        match self.run_systemctl("is-active") {
            Ok(Some(0)) => true,
            _ => false,
        }
    }

    fn stop(&mut self) {
        let _ = self.run_systemctl("stop");
    }

    fn start(&mut self) {
        let _ = self.run_systemctl("start");
    }

    fn daemon_reload(&mut self) {
        let _ = self.run_systemctl_args(&["--user", "daemon-reload"]);
    }

    fn enable(&mut self) {
        let _ = self.run_systemctl("enable");
    }

    fn restart(&mut self) {
        let _ = self.run_systemctl("restart");
    }
}

/// No-op service runner for environments without systemd.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NoopServiceRunner;

impl ServiceRunner for NoopServiceRunner {
    fn check_active(&mut self) -> bool {
        false
    }
    fn stop(&mut self) {}
    fn start(&mut self) {}
    fn daemon_reload(&mut self) {}
    fn enable(&mut self) {}
    fn restart(&mut self) {}
}

/// Recorded call counts and simulated state for fake service runner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeServiceState {
    pub is_active: bool,
    pub check_active_calls: usize,
    pub stop_calls: usize,
    pub start_calls: usize,
    pub daemon_reload_calls: usize,
    pub enable_calls: usize,
    pub restart_calls: usize,
}

/// Fake service runner that records calls and simulates service state in unit tests.
#[derive(Debug, Clone)]
pub struct FakeServiceRunner {
    pub state: Arc<Mutex<FakeServiceState>>,
    pub should_panic: bool,
}

impl Default for FakeServiceRunner {
    fn default() -> Self {
        Self::new(false)
    }
}

impl FakeServiceRunner {
    /// Create a new fake service runner with the given initial active status.
    pub fn new(is_active: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeServiceState {
                is_active,
                check_active_calls: 0,
                stop_calls: 0,
                start_calls: 0,
                daemon_reload_calls: 0,
                enable_calls: 0,
                restart_calls: 0,
            })),
            should_panic: false,
        }
    }

    /// Configure the fake runner to simulate panics.
    pub fn with_panic(mut self) -> Self {
        self.should_panic = true;
        self
    }

    pub fn is_active(&self) -> bool {
        self.state.lock().unwrap().is_active
    }

    pub fn check_active_count(&self) -> usize {
        self.state.lock().unwrap().check_active_calls
    }

    pub fn stop_count(&self) -> usize {
        self.state.lock().unwrap().stop_calls
    }

    pub fn start_count(&self) -> usize {
        self.state.lock().unwrap().start_calls
    }

    pub fn daemon_reload_count(&self) -> usize {
        self.state.lock().unwrap().daemon_reload_calls
    }

    pub fn enable_count(&self) -> usize {
        self.state.lock().unwrap().enable_calls
    }

    pub fn restart_count(&self) -> usize {
        self.state.lock().unwrap().restart_calls
    }
}

impl ServiceRunner for FakeServiceRunner {
    fn check_active(&mut self) -> bool {
        if self.should_panic {
            panic!("Simulated runner panic during check_active");
        }
        let mut st = self.state.lock().unwrap();
        st.check_active_calls += 1;
        st.is_active
    }

    fn stop(&mut self) {
        if self.should_panic {
            panic!("Simulated runner panic during stop");
        }
        let mut st = self.state.lock().unwrap();
        st.stop_calls += 1;
        st.is_active = false;
    }

    fn start(&mut self) {
        if self.should_panic {
            panic!("Simulated runner panic during start");
        }
        let mut st = self.state.lock().unwrap();
        st.start_calls += 1;
        st.is_active = true;
    }

    fn daemon_reload(&mut self) {
        if self.should_panic {
            panic!("Simulated runner panic during daemon_reload");
        }
        let mut st = self.state.lock().unwrap();
        st.daemon_reload_calls += 1;
    }

    fn enable(&mut self) {
        if self.should_panic {
            panic!("Simulated runner panic during enable");
        }
        let mut st = self.state.lock().unwrap();
        st.enable_calls += 1;
    }

    fn restart(&mut self) {
        if self.should_panic {
            panic!("Simulated runner panic during restart");
        }
        let mut st = self.state.lock().unwrap();
        st.restart_calls += 1;
        st.is_active = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suspend_active_service_stops_and_returns_true() {
        let mut runner = FakeServiceRunner::new(true);
        let suspended = suspend_for_gui(&mut runner);
        assert!(suspended);
        assert_eq!(runner.check_active_count(), 1);
        assert_eq!(runner.stop_count(), 1);
        assert!(!runner.is_active());
    }

    #[test]
    fn test_suspend_inactive_service_does_not_stop() {
        let mut runner = FakeServiceRunner::new(false);
        let suspended = suspend_for_gui(&mut runner);
        assert!(!suspended);
        assert_eq!(runner.check_active_count(), 1);
        assert_eq!(runner.stop_count(), 0);
        assert!(!runner.is_active());
    }

    #[test]
    fn test_restore_after_gui_when_was_active() {
        let mut runner = FakeServiceRunner::new(false);
        restore_after_gui(&mut runner, true);
        assert_eq!(runner.start_count(), 1);
        assert!(runner.is_active());
    }

    #[test]
    fn test_restore_after_gui_when_was_not_active() {
        let mut runner = FakeServiceRunner::new(false);
        restore_after_gui(&mut runner, false);
        assert_eq!(runner.start_count(), 0);
        assert!(!runner.is_active());
    }

    #[test]
    fn test_suspend_and_restore_swallow_runner_panics() {
        let mut runner = FakeServiceRunner::new(true).with_panic();
        let suspended = suspend_for_gui(&mut runner);
        assert!(!suspended);

        restore_after_gui(&mut runner, true);
    }

    #[test]
    fn test_closure_service_runner() {
        struct State {
            active: bool,
            stopped: bool,
            started: bool,
        }

        let state = Arc::new(Mutex::new(State {
            active: true,
            stopped: false,
            started: false,
        }));

        let s1 = state.clone();
        let s2 = state.clone();
        let s3 = state.clone();

        let mut runner = ClosureServiceRunner::new(
            move || s1.lock().unwrap().active,
            move || {
                let mut st = s2.lock().unwrap();
                st.stopped = true;
                st.active = false;
            },
            move || {
                let mut st = s3.lock().unwrap();
                st.started = true;
                st.active = true;
            },
        );

        assert!(suspend_for_gui(&mut runner));
        {
            let st = state.lock().unwrap();
            assert!(st.stopped);
            assert!(!st.active);
        }

        restore_after_gui(&mut runner, true);
        {
            let st = state.lock().unwrap();
            assert!(st.started);
            assert!(st.active);
        }
    }

    #[test]
    fn test_systemd_runner_active_exit_code_zero() {
        let mut executed = Vec::new();
        let mut runner = SystemdServiceRunner::with_executor(
            "test.service",
            Duration::from_millis(50),
            move |_cmd, args, _timeout| {
                executed.push(args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
                Ok(Some(0))
            },
        );

        assert!(runner.check_active());
    }

    #[test]
    fn test_systemd_runner_inactive_exit_code_three() {
        let mut runner = SystemdServiceRunner::with_executor(
            "test.service",
            Duration::from_millis(50),
            |_cmd, _args, _timeout| Ok(Some(3)),
        );

        assert!(!runner.check_active());
    }

    #[test]
    fn test_systemd_runner_swallows_process_error() {
        let mut runner = SystemdServiceRunner::with_executor(
            "test.service",
            Duration::from_millis(50),
            |_cmd, _args, _timeout| Err(io::Error::new(io::ErrorKind::NotFound, "systemctl not found")),
        );

        assert!(!runner.check_active());
        runner.stop();
        runner.start();
    }

    #[test]
    fn test_systemd_runner_swallows_process_timeout() {
        let mut runner = SystemdServiceRunner::with_executor(
            "test.service",
            Duration::from_millis(50),
            |_cmd, _args, _timeout| Ok(None),
        );

        assert!(!runner.check_active());
        runner.stop();
        runner.start();
    }

    #[test]
    fn test_systemd_runner_actions_pass_expected_args() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let calls_clone = calls.clone();

        let mut runner = SystemdServiceRunner::with_executor(
            "aula-f75.service",
            Duration::from_millis(100),
            move |_cmd, args, _timeout| {
                calls_clone.lock().unwrap().push(args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
                Ok(Some(0))
            },
        );

        assert!(runner.check_active());
        runner.stop();
        runner.start();

        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0], vec!["--user", "is-active", "aula-f75.service"]);
        assert_eq!(recorded[1], vec!["--user", "stop", "aula-f75.service"]);
        assert_eq!(recorded[2], vec!["--user", "start", "aula-f75.service"]);
    }

    #[test]
    fn test_service_args_solid() {
        let args = service_args_for_state(&GuiLightingMode::Solid, (255, 128, 0), 1.0, 1.0);
        assert_eq!(args, vec!["solid", "FF8000"]);
    }

    #[test]
    fn test_service_args_off_never_maps_to_off() {
        let args = service_args_for_state(&GuiLightingMode::Off, (255, 0, 0), 1.0, 1.0);
        assert_eq!(args, vec!["solid", "000000"]);
        assert!(!args.contains(&"off".to_string()));
    }

    #[test]
    fn test_service_args_effect_with_color() {
        use crate::effects::RGB;
        // Breathe takes color
        let breathe = Effect::Breathe {
            color: RGB(0, 255, 0),
            period_s: 4.0,
        };
        let args = service_args_for_state(&GuiLightingMode::Effect(breathe), (0, 255, 0), 0.8, 1.5);
        assert_eq!(
            args,
            vec!["fx", "breathe", "--color", "00FF00", "--brightness", "0.8", "--speed", "1.5"]
        );

        // Solid effect variant takes color
        let solid_eff = Effect::Solid(RGB(255, 0, 0));
        let args_solid =
            service_args_for_state(&GuiLightingMode::Effect(solid_eff), (255, 0, 0), 1.0, 1.0);
        assert_eq!(
            args_solid,
            vec!["fx", "solid", "--color", "FF0000", "--brightness", "1", "--speed", "1"]
        );
    }

    #[test]
    fn test_service_args_effect_without_color() {
        // Rainbow takes no color
        let rainbow = Effect::Rainbow { period_s: 5.0 };
        let args = service_args_for_state(&GuiLightingMode::Effect(rainbow), (255, 0, 0), 1.0, 2.0);
        assert_eq!(
            args,
            vec!["fx", "rainbow", "--brightness", "1", "--speed", "2"]
        );
        assert!(!args.contains(&"--color".to_string()));

        // Wave takes no color
        let wave = Effect::Wave { period_s: 3.0 };
        let args_wave =
            service_args_for_state(&GuiLightingMode::Effect(wave), (255, 0, 0), 0.75, 1.25);
        assert_eq!(
            args_wave,
            vec!["fx", "wave", "--brightness", "0.75", "--speed", "1.25"]
        );
        assert!(!args_wave.contains(&"--color".to_string()));

        // Stars takes no color
        let stars = Effect::Stars { density: 0.15 };
        let args_stars =
            service_args_for_state(&GuiLightingMode::Effect(stars), (255, 0, 0), 1.0, 1.0);
        assert_eq!(
            args_stars,
            vec!["fx", "stars", "--brightness", "1", "--speed", "1"]
        );
        assert!(!args_stars.contains(&"--color".to_string()));

        // Effect::Off takes no color
        let eff_off = Effect::Off;
        let args_off =
            service_args_for_state(&GuiLightingMode::Effect(eff_off), (255, 0, 0), 1.0, 1.0);
        assert_eq!(
            args_off,
            vec!["fx", "off", "--brightness", "1", "--speed", "1"]
        );
        assert!(!args_off.contains(&"--color".to_string()));
    }

    #[test]
    fn test_service_args_music() {
        let args_bars = service_args_for_state(
            &GuiLightingMode::Music {
                mode: MusicMode::Bars,
                gain: 1.5,
            },
            (0, 255, 255),
            1.0,
            1.0,
        );
        assert_eq!(
            args_bars,
            vec!["music", "--mode", "bars", "--color", "00FFFF", "--gain", "1.5"]
        );

        let args_pulse = service_args_for_state(
            &GuiLightingMode::Music {
                mode: MusicMode::Pulse,
                gain: 2.0,
            },
            (255, 0, 128),
            1.0,
            1.0,
        );
        assert_eq!(
            args_pulse,
            vec!["music", "--mode", "pulse", "--color", "FF0080", "--gain", "2"]
        );
    }

    #[test]
    fn test_systemd_runner_daemon_reload_enable_restart_args() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let calls_clone = calls.clone();

        let mut runner = SystemdServiceRunner::with_executor(
            "aula-f75.service",
            Duration::from_millis(100),
            move |_cmd, args, _timeout| {
                calls_clone
                    .lock()
                    .unwrap()
                    .push(args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
                Ok(Some(0))
            },
        );

        runner.daemon_reload();
        runner.enable();
        runner.restart();

        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0], vec!["--user", "daemon-reload"]);
        assert_eq!(recorded[1], vec!["--user", "enable", "aula-f75.service"]);
        assert_eq!(recorded[2], vec!["--user", "restart", "aula-f75.service"]);
    }

    #[test]
    fn test_adopt_service_command_rewrites_exec_start_preserving_lines() {
        let temp_dir = std::env::temp_dir().join(format!(
            "aula_adopt_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let unit_path = temp_dir.join("aula-f75.service");

        let initial_content = "[Unit]\nDescription=AULA F75 RGB Lighting Service\nAfter=default.target\n\n[Service]\nType=simple\n# Custom comment\nExecStart=%h/.local/bin/aula-f75-rs solid 303030\nRestart=on-failure\nRestartSec=3s\n\n[Install]\nWantedBy=default.target\n";
        std::fs::write(&unit_path, initial_content).unwrap();

        let mut runner = FakeServiceRunner::new(false);
        let cli_args = vec![
            "fx".to_string(),
            "rainbow".to_string(),
            "--brightness".to_string(),
            "1".to_string(),
            "--speed".to_string(),
            "1".to_string(),
        ];

        let success = adopt_service_command(&unit_path, &cli_args, &mut runner);
        assert!(success);

        assert_eq!(runner.daemon_reload_count(), 1);
        assert_eq!(runner.enable_count(), 1);
        assert_eq!(runner.restart_count(), 1);
        assert!(runner.is_active());

        let updated_content = std::fs::read_to_string(&unit_path).unwrap();
        assert!(updated_content.contains("ExecStart=%h/.local/bin/aula-f75-rs fx rainbow --brightness 1 --speed 1"));
        assert!(updated_content.contains("Description=AULA F75 RGB Lighting Service"));
        assert!(updated_content.contains("# Custom comment"));
        assert!(updated_content.contains("Restart=on-failure"));
        assert!(updated_content.contains("WantedBy=default.target"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_adopt_service_command_appends_exec_start_if_missing() {
        let temp_dir = std::env::temp_dir().join(format!(
            "aula_adopt_missing_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let unit_path = temp_dir.join("aula-f75.service");

        let content_no_exec = "[Unit]\nDescription=AULA F75 RGB\n\n[Service]\nType=simple\nRestart=on-failure\n";
        std::fs::write(&unit_path, content_no_exec).unwrap();

        let mut runner = FakeServiceRunner::new(false);
        let cli_args = vec!["solid".to_string(), "FF0000".to_string()];

        let success = adopt_service_command(&unit_path, &cli_args, &mut runner);
        assert!(success);

        let updated = std::fs::read_to_string(&unit_path).unwrap();
        assert!(updated.contains("ExecStart=%h/.local/bin/aula-f75-rs solid FF0000"));
        assert!(updated.contains("Type=simple"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_adopt_service_command_swallows_runner_panic_and_io_error() {
        let mut runner_panic = FakeServiceRunner::new(false).with_panic();
        let invalid_path = PathBuf::from("/dev/null/forbidden/aula-f75.service");
        let cli_args = vec!["solid".to_string(), "000000".to_string()];

        let result = adopt_service_command(&invalid_path, &cli_args, &mut runner_panic);
        assert!(!result);
    }
}
