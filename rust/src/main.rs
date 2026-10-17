//! Binary entry point for aula-f75-rs CLI.

use aula_f75::cli;

fn main() {
    let exit_code = cli::run();
    std::process::exit(exit_code);
}
