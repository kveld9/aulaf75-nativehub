//! System audio-reactive music lighting command execution for AULA F75.

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::time::Duration;

use crate::cli::music;
use crate::cli::{install_sigint_handler, is_running};
use crate::device;
use crate::proto;

/// Stream system-audio reactive lighting.
pub fn run_music_lighting(
    mode: music::MusicMode,
    color: (u8, u8, u8),
    gain: f32,
    seconds: Option<f64>,
) -> i32 {
    let node = match device::resolve_control_node() {
        Some(p) => p,
        None => {
            eprintln!("Error: AULA F75 interface 1 control node not found.");
            eprintln!("Ensure keyboard is connected via USB and udev rule 99-aula-f75.rules is installed.");
            return 1;
        }
    };

    let file = match OpenOptions::new().read(true).write(true).open(&node) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error opening control node {}: {}", node.display(), e);
            eprintln!("Check permissions or install udev rule: sudo cp 99-aula-f75.rules /etc/udev/rules.d/");
            return 1;
        }
    };

    let child = match music::spawn_monitor() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error initializing audio capture: {}", e);
            return 1;
        }
    };

    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut guard = ChildGuard(child);

    let mut stdout = match guard.0.stdout.take() {
        Some(s) => s,
        None => {
            eprintln!("Error: Failed to capture stdout pipe from parec process");
            return 1;
        }
    };

    install_sigint_handler();
    let fd = file.as_raw_fd();

    println!(
        "Streaming music-reactive lighting (mode: {}, color: #{:02x}{:02x}{:02x}, gain: {:.2}) on {}...",
        mode, color.0, color.1, color.2, gain, node.display()
    );
    println!("Press Ctrl+C to stop.");

    let res = music::run_music_loop(
        &mut stdout,
        |frame| {
            device::send_with_retry(
                || proto::hid_set_feature(fd, frame),
                5,
                || std::thread::sleep(Duration::from_millis(5)),
            )
        },
        mode,
        color,
        gain,
        seconds,
        is_running,
    );

    match res {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Device communication error during music mode: {}", e);
            1
        }
    }
}
