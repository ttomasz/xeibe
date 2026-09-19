use std::path::PathBuf;

use crate::args::{InputArgs, ReadArgs};

/// Prints axis-order conflicts first, then the layer summary (and `--explain`);
/// with `-o`, writes the settings file.
pub fn run(
    input: InputArgs,
    read: ReadArgs,
    sample: Option<u64>,
    explain: bool,
    output: Option<PathBuf>,
) -> super::Result {
    todo!()
}
