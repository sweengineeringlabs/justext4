//! `mkext4-rs` operator binary. Thin dispatcher — every command
//! lives in [`cli`] (the lib half of this crate) so integration
//! tests can call the same code paths without spawning a process.

use std::io::{self, Write};
use std::process::ExitCode;

use cli::{cmd_cat, cmd_format, cmd_inspect, cmd_touch, print_usage, CliError};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let stderr = io::stderr();
    let mut err = stderr.lock();

    let result = match argv.get(1).map(String::as_str) {
        Some("format") => cmd_format(&argv[2..], &mut out),
        Some("inspect") => cmd_inspect(&argv[2..], &mut out),
        Some("touch") => cmd_touch(&argv[2..], &mut out),
        Some("cat") => cmd_cat(&argv[2..], &mut out),
        Some("--help") | Some("-h") | Some("help") | None => {
            let _ = print_usage(&mut err);
            return ExitCode::SUCCESS;
        }
        Some(other) => Err(CliError(format!("unknown command: {other} (try --help)",))),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(err, "error: {e}");
            ExitCode::FAILURE
        }
    }
}
