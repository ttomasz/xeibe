//! Turn CLI/library inputs (paths, globs, `-`, `http(s)://`, `s3://`, …, optionally
//! with `!/member` for local zip archives) into sources. Local inputs are expanded
//! with `xeibe_core::source::expand_sources`; a remote zip is
//! `xeibe_core::Error::RemoteArchive`.

use xeibe_core::Source;

use crate::options::IoOptions;

pub fn resolve_sources(inputs: &[String], options: &IoOptions) -> crate::Result<Vec<Source>> {
    todo!()
}
