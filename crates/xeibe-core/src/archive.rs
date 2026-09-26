//! Zip archives, kept minimal (see `docs/architecture.md#zip-archives`).
//!
//! Uses the `zip` crate as it is: stored and deflate members, zip64, name decoding.
//! Other methods and encrypted members are errors. The archive must be a local
//! file (the central directory is at the end); a remote zip is
//! [`crate::Error::RemoteArchive`]. Nested zips are not opened.

use std::io::{Read, Seek, SeekFrom};
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
///
/// Members are listed in archive order. A glob is matched against the whole
/// member name; `*` does not cross `/`.
pub fn list_candidates(archive: &Path, member_globs: &[String]) -> crate::Result<Vec<MemberInfo>> {
    let globs = build_globs(archive, member_globs)?;
    let mut zip = open_archive(archive)?;
    let mut members = Vec::new();
    for index in 0..zip.len() {
        let file = zip
            .by_index_raw(index)
            .map_err(|e| zip_error(archive, "", e))?;
        if !file.is_file() {
            continue;
        }
        let name = file.name();
        let lower = name.to_ascii_lowercase();
        if !CANDIDATE_SUFFIXES
            .iter()
            .any(|suffix| lower.ends_with(suffix))
        {
            continue;
        }
        if let Some(globs) = &globs
            && !globs.is_match(name)
        {
            continue;
        }
        members.push(MemberInfo {
            name: name.to_string(),
            compressed_size: file.compressed_size(),
            size: file.size(),
        });
    }
    Ok(members)
}

/// One source per candidate. Members in which the splitter finds no feature
/// collection or feature member are dropped later and reported, not here.
pub fn expand(archive: &Path, member_globs: &[String]) -> crate::Result<Vec<Source>> {
    let members = list_candidates(archive, member_globs)?;
    if members.is_empty() {
        return Err(crate::Error::NoGmlMembers(archive.display().to_string()));
    }
    Ok(members
        .into_iter()
        .map(|member| Source::ZipMember {
            archive: archive.to_path_buf(),
            member: member.name,
        })
        .collect())
}

/// Split `path.zip!/dir/file.gml` into archive and member.
pub fn split_member_path(input: &str) -> Option<(&str, &str)> {
    let (archive, member) = input.split_once("!/")?;
    (!archive.is_empty() && !member.is_empty()).then_some((archive, member))
}

/// Open one member as a stream (decompressed by the `zip` crate, not yet decoded).
///
/// The `zip` crate's member readers borrow the archive, so the member's data
/// is located through the archive and then streamed from a file handle of its
/// own: the reader is `'static` and holds no more than the decompressor's
/// state, however large the member is.
pub(crate) fn open_member(archive: &Path, member: &str) -> crate::Result<Box<dyn Read + Send>> {
    let mut zip = open_archive(archive)?;
    let file = zip
        .by_name(member)
        .map_err(|e| zip_error(archive, member, e))?;
    let method = file.compression();
    let compressed_size = file.compressed_size();
    let data_start = file.data_start().ok_or_else(|| crate::Error::Zip {
        archive: archive.display().to_string(),
        member: member.to_string(),
        message: "the member's data could not be located".into(),
    })?;
    drop(file);
    drop(zip);

    let mut handle = std::fs::File::open(archive)?;
    handle.seek(SeekFrom::Start(data_start))?;
    let data = handle.take(compressed_size);
    match method {
        zip::CompressionMethod::Stored => Ok(Box::new(data)),
        zip::CompressionMethod::Deflated => {
            Ok(crate::decode::read_ahead(Box::new(flate2::read::DeflateDecoder::new(data)))?)
        }
        other => Err(crate::Error::Zip {
            archive: archive.display().to_string(),
            member: member.to_string(),
            message: format!("unsupported compression method {other}"),
        }),
    }
}

fn open_archive(archive: &Path) -> crate::Result<zip::ZipArchive<std::fs::File>> {
    let file = std::fs::File::open(archive)
        .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", archive.display())))?;
    zip::ZipArchive::new(file).map_err(|e| zip_error(archive, "", e))
}

fn build_globs(archive: &Path, globs: &[String]) -> crate::Result<Option<globset::GlobSet>> {
    if globs.is_empty() {
        return Ok(None);
    }
    let mut set = globset::GlobSetBuilder::new();
    for glob in globs {
        let glob = globset::GlobBuilder::new(glob)
            .literal_separator(true)
            .build()
            .map_err(|e| crate::Error::Zip {
                archive: archive.display().to_string(),
                member: glob.clone(),
                message: e.to_string(),
            })?;
        set.add(glob);
    }
    set.build().map(Some).map_err(|e| crate::Error::Zip {
        archive: archive.display().to_string(),
        member: String::new(),
        message: e.to_string(),
    })
}

fn zip_error(archive: &Path, member: &str, error: zip::result::ZipError) -> crate::Error {
    crate::Error::Zip {
        archive: archive.display().to_string(),
        member: member.to_string(),
        message: error.to_string(),
    }
}
