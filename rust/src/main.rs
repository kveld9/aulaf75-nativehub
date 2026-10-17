//! Binary entry point for aula-f75-rs CLI.
//!
//! Provides CLI commands for device detection, direct RGB solid/off/key control,
//! host-rendered lighting effects, local profiles, and system-audio reactive music mode.

use aula_f75::cli;

fn main() {
    let exit_code = cli::run();
    std::process::exit(exit_code);
}
