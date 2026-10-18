//! Background systemd service coordination for AULA F75 GUI.
//!
//! Provides single-writer coordination by parking the background `aula-f75.service`
//! while the desktop GUI runs, and restoring it when the GUI closes.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

    fn run_systemctl(&mut self, action: &str) -> io::Result<Option<i32>> {
        let args = ["--user", action, &self.unit];
        if let Some(ref mut exec) = self.executor {
            exec("systemctl", &args, self.timeout)
        } else {
            run_process_command("systemctl", &args, self.timeout)
        }
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
}

/// Recorded call counts and simulated state for fake service runner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeServiceState {
    pub is_active: bool,
    pub check_active_calls: usize,
    pub stop_calls: usize,
    pub start_calls: usize,
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
}
