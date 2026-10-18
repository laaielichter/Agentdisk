//! agentdisk command line. Step 0.0.1: only `--version` and `--help`.
//! A real argument parser (clap) comes with `scan` in a later step.

use std::process::ExitCode;

const HELP: &str = "\
agentdisk — the honest disk report for AI coding agents

USAGE:
    agentdisk [OPTIONS]

OPTIONS:
    -V, --version    Print version
    -h, --help       Print this help

Nothing is scanned or deleted yet. `agentdisk scan` comes in v0.1.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("-h") | Some("--help") => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("-V") | Some("--version") => {
            println!("agentdisk {}", agentdisk_core::VERSION);
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("error: unknown argument '{other}'\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}
