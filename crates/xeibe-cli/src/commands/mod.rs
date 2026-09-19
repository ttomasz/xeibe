mod convert;
mod scan;
mod wfs;

use crate::args::{Cli, Command, ReadArgs};

pub type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn run(cli: Cli) -> Result {
    match cli.command {
        Command::Scan {
            input,
            read,
            sample,
            explain,
            output,
        } => scan::run(input, read, sample, explain, output),
        Command::Convert {
            input,
            output,
            read,
        } => convert::run(input, output, read),
        Command::Wfs(command) => wfs::run(command),
    }
}

/// Load the settings file (if any) and apply CLI overrides to its options.
pub(crate) fn settings(read: &ReadArgs) -> Result<xeibe_arrow::Settings> {
    todo!()
}
