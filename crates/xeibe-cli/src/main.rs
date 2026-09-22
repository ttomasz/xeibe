//! `xeibe` command-line tool. See `docs/architecture.md` "CLI".

mod args;
mod commands;

use clap::Parser;

fn main() -> std::process::ExitCode {
    let cli = args::Cli::parse();
    match commands::run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        // `xeibe scan … | head` closes the pipe early: not an error.
        Err(err) if err.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe) => {
            std::process::ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}
