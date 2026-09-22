//! `xeibe` command-line tool. See `docs/architecture.md` "CLI".

mod args;
mod commands;

use clap::Parser;

fn main() -> std::process::ExitCode {
    let cli = args::Cli::parse();
    match commands::run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}
