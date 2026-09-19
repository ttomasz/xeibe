//! Zip archives, kept minimal (see `docs/architecture.md#zip-archives`).
//!
//! Uses the `zip` crate as it is: stored and deflate members, zip64, name decoding.
//! Other methods and encrypted members are errors. The archive must be a local
//! file (the central directory is at the end); a remote zip is
//! [`crate::Error::RemoteArchive`]. Nested zips are not opened.

use std::path::Path;

use crate::source::Source;

/// Member names considered as GML candidates (case-insensitive).
pub const CANDIDATE_SUFFIXES: &[&str] = &[".gml", ".xml", ".gml.gz", ".xml.gz"];

/// A candidate member, before content checks.
#[derive(Debug, Clone)]
pub struct MemberInfo {
    /// Name as decoded by the `zip` crate.
    pub name: String,
    pub compressed_size: u64,
    pub size: u64,
}

/// List candidate members, optionally filtered by `--member` globs.
pub fn list_candidates(archive: &Path, member_globs: &[String]) -> crate::Result<Vec<MemberInfo>> {
    todo!()
}

/// One source per candidate. Members in which the splitter finds no feature
/// collection or feature member are dropped later and reported, not here.
pub fn expand(archive: &Path, member_globs: &[String]) -> crate::Result<Vec<Source>> {
    todo!()
}

/// Split `path.zip!/dir/file.gml` into archive and member.
pub fn split_member_path(input: &str) -> Option<(&str, &str)> {
    todo!()
}

/// Open one member as a stream (decompressed by the `zip` crate, not yet decoded).
pub(crate) fn open_member(archive: &Path, member: &str) -> crate::Result<Box<dyn std::io::Read + Send>> {
    todo!()
}
