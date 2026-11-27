//! Thin CLI entry point for the `anvil` engineering harness.

mod cli;
mod commands;
mod daemon;
mod ui;

use std::process::ExitCode;

use clap::Parser;

use cli::Cli;
use commands::ExitCodeError;

/// Stack of the thread that runs the CLI. The derived argument parser and the command dispatch
/// keep large frames in debug builds, and a Windows main thread only has 1 MiB.
const CLI_STACK_BYTES: usize = 16 * 1024 * 1024;

fn main() -> ExitCode {
    let spawned = std::thread::Builder::new().stack_size(CLI_STACK_BYTES).spawn(run);
    match spawned.map(std::thread::JoinHandle::join) {
        Ok(Ok(code)) => code,
        Ok(Err(_)) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("failed to start the CLI thread: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> ExitCode {
    let parsed = Cli::parse();
    ui::init_color_mode(parsed.no_color);
    match commands::dispatch(parsed) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            if let Some(exit_err) = err.downcast_ref::<ExitCodeError>() {
                if let Some(msg) = &exit_err.message {
                    eprintln!("{}", ui::stderr_error(msg));
                }
                let code = u8::try_from(exit_err.code.clamp(1, 255)).unwrap_or(1);
                return ExitCode::from(code);
            }
            eprintln!("{}", ui::stderr_error(&format!("{err:#}")));
            ExitCode::FAILURE
        }
    }
}
