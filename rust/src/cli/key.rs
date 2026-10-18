//! Key mapping enumeration and per-key RGB lighting commands for AULA F75.

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;

use crate::cli::{install_sigint_handler, is_running, responsive_sleep};
use crate::device;
use crate::proto;

/// List all known key names and their slot numbers.
pub fn cmd_keys() -> i32 {
    println!("AULA F75 Key Map ({} keys):", proto::KEY_SLOTS.len());
    for &(name, slot) in proto::KEY_SLOTS {
        println!("  {:<12} (slot {:>2})", name, slot);
    }
    0
}

/// Run per-key RGB lighting loop for multiple keys on the hardware interface 1 control node.
pub fn run_keys_lighting(pairs: &[(&str, (u8, u8, u8))], seconds: Option<f64>) -> i32 {
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
    let res = device::send_key_frame(
        pairs,
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

/// Run per-key RGB lighting loop on the hardware interface 1 control node.
pub fn run_key_lighting(name: &str, rgb: (u8, u8, u8), seconds: Option<f64>) -> i32 {
    run_keys_lighting(&[(name, rgb)], seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cmd_keys() {
        assert_eq!(cmd_keys(), 0);
    }
}
