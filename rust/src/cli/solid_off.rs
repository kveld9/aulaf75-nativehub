//! Solid color and lighting off commands for AULA F75.

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;

use crate::cli::{install_sigint_handler, is_running, responsive_sleep};
use crate::device;
use crate::proto;

/// Run RGB lighting loop on the hardware interface 1 control node.
pub fn run_lighting(rgb: (u8, u8, u8), seconds: Option<f64>) -> i32 {
    let node = match device::resolve_control_node() {
        Some(n) => n,
        None => {
            eprintln!("Error: AULA F75 interface 1 control node not found.");
            return 1;
        }
    };

    let file = match OpenOptions::new().read(true).write(true).open(&node) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error opening control node {}: {}", node.display(), e);
            return 1;
        }
    };

    install_sigint_handler();

    let fd = file.as_raw_fd();
    let res = device::run_keepalive_loop(
        rgb,
        seconds,
        |frame| proto::hid_set_feature(fd, frame),
        responsive_sleep,
        is_running,
    );

    match res {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Device communication error: {}", e);
            1
        }
    }
}
