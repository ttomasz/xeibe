//! `xeibe` command-line tool. See "CLI" in the
//! [architecture document](https://github.com/ttomasz/xeibe/blob/main/docs/architecture.md).
//!
//! <div class="warning">
//!
//! **Experimental, not for production use.** This tool is AI-generated and
//! has not been thoroughly tested yet. Its interfaces are not stable and will
//! change in the near future, in breaking ways.
//!
//! </div>

mod args;
mod commands;

use clap::Parser;

fn main() -> std::process::ExitCode {
    let cli = args::Cli::parse();
    match commands::run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        // `xeibe gml scan … | head` closes the pipe early: not an error.
        Err(err)
            if err
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe) =>
        {
            std::process::ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}
