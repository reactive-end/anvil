//! Thin CLI entry point for the `anvil` engineering harness.

mod cli;
mod commands;
mod daemon;

use std::process::ExitCode;

use clap::Parser;

use cli::Cli;
use commands::ExitCodeError;

fn main() -> ExitCode {
    let parsed = Cli::parse();
    match commands::dispatch(parsed) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            if let Some(exit_err) = err.downcast_ref::<ExitCodeError>() {
                if let Some(msg) = &exit_err.message {
                    eprintln!("{msg}");
                }
                let code = u8::try_from(exit_err.code.clamp(1, 255)).unwrap_or(1);
                return ExitCode::from(code);
            }
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
