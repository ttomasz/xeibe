//! Zip archives, kept minimal (see `docs/architecture.md#zip-archives`).
//!
//! Uses the `zip` crate for the directory: zip64, name decoding. Stored,
//! deflate and Deflate64 members are read; other methods and encrypted
//! members are errors, found when the archive is listed. The archive must be
//! a local file or readable at any offset ([`RandomAccess`]: bytes in memory,
//! a browser `File`), since the central directory is at the end; a remote
//! zip is [`crate::Error::RemoteArchive`]. Nested zips are not opened.

use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::source::{RANDOM_ACCESS_CHUNK, RandomAccess, RandomReader, Source};

/// Where a zip archive is: a local file, or bytes readable at any offset (in
/// memory, or a file dropped on the web page).
#[derive(Debug, Clone)]
pub enum Archive {
    File(PathBuf),
    Random {
        name: String,
        data: Arc<dyn RandomAccess>,
    },
}

impl Archive {
    /// An archive held in memory.
    pub fn bytes(name: impl Into<String>, bytes: bytes::Bytes) -> Self {
        Archive::Random {
            name: name.into(),
            data: Arc::new(bytes),
        }
    }

    /// The path, or the name given with the bytes, for messages and the
    /// members' names (`name!/member`).
    pub fn name(&self) -> String {
        match self {
            Archive::File(path) => path.display().to_string(),
            Archive::Random { name, .. } => name.clone(),
        }
    }

    fn open(&self) -> crate::Result<zip::ZipArchive<ArchiveReader>> {
        let reader = match self {
            Archive::File(path) => {
                ArchiveReader::File(std::fs::File::open(path).map_err(|e| {
                    std::io::Error::new(e.kind(), format!("{}: {e}", path.display()))
                })?)
            }
            // The directory is read in small pieces: buffer them.
            Archive::Random { data, .. } => ArchiveReader::Random(BufReader::with_capacity(
                64 << 10,
                RandomReader::new(data.clone()),
            )),
        };
        zip::ZipArchive::new(reader).map_err(|e| zip_error(self, "", e))
    }

    /// `length` bytes from `start`, as a stream of their own.
    fn range(&self, start: u64, length: u64) -> crate::Result<Box<dyn Read + Send>> {
        match self {
            Archive::File(path) => {
                let mut handle = std::fs::File::open(path)?;
                handle.seek(SeekFrom::Start(start))?;
                Ok(Box::new(handle.take(length)))
            }
            Archive::Random { data, .. } => Ok(Box::new(BufReader::with_capacity(
                RANDOM_ACCESS_CHUNK,
                RandomReader::range(data.clone(), start, length),
            ))),
        }
    }
}

impl From<&Path> for Archive {
    fn from(path: &Path) -> Self {
        Archive::File(path.to_path_buf())
    }
}

impl From<&PathBuf> for Archive {
    fn from(path: &PathBuf) -> Self {
        Archive::File(path.clone())
    }
}

impl From<PathBuf> for Archive {
    fn from(path: PathBuf) -> Self {
        Archive::File(path)
    }
}

/// An open archive's bytes.
enum ArchiveReader {
    File(std::fs::File),
    Random(BufReader<RandomReader>),
}

impl Read for ArchiveReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            ArchiveReader::File(file) => file.read(buf),
            ArchiveReader::Random(reader) => reader.read(buf),
        }
    }
}

impl Seek for ArchiveReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            ArchiveReader::File(file) => file.seek(pos),
            ArchiveReader::Random(reader) => reader.seek(pos),
        }
    }
}

/// A member whose data can't be read, by what is wrong: an encrypted member
/// or a compression method other than stored, deflate and Deflate64.
fn unreadable(file: &zip::read::ZipFile<'_, ArchiveReader>) -> Option<String> {
    if file.encrypted() {
        return Some("encrypted members are not supported".into());
    }
    match file.compression() {
        zip::CompressionMethod::Stored
        | zip::CompressionMethod::Deflated
        | zip::CompressionMethod::Deflate64 => None,
        other => Some(format!("unsupported compression method {other}")),
    }
}

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

/// List candidate members, optionally filtered by member globs
/// (`archive.zip!/<glob>`).
///
/// Members are listed in archive order. A glob is matched against the whole
/// member name; `*` does not cross `/`.
pub fn list_candidates(
    archive: impl Into<Archive>,
    member_globs: &[String],
) -> crate::Result<Vec<MemberInfo>> {
    let archive = &archive.into();
    let globs = build_globs(archive, member_globs)?;
    let mut zip = archive.open()?;
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
        // Now rather than when the read reaches the member, maybe after
        // gigabytes of the others.
        if let Some(message) = unreadable(&file) {
            return Err(crate::Error::Zip {
                archive: archive.name(),
                member: name.to_string(),
                message,
            });
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
pub fn expand(archive: impl Into<Archive>, member_globs: &[String]) -> crate::Result<Vec<Source>> {
    let archive = archive.into();
    let members = list_candidates(archive.clone(), member_globs)?;
    if members.is_empty() {
        let mut name = archive.name();
        if !member_globs.is_empty() {
            name.push_str(&format!(" matching {}", member_globs.join(", ")));
        }
        return Err(crate::Error::NoGmlMembers(name));
    }
    Ok(members
        .into_iter()
        .map(|member| Source::ZipMember {
            archive: archive.clone(),
            member: member.name,
        })
        .collect())
}

/// Split `path.zip!/dir/file.gml` into archive and member.
pub fn split_member_path(input: &str) -> Option<(&str, &str)> {
    let (archive, member) = input.split_once("!/")?;
    (!archive.is_empty() && !member.is_empty()).then_some((archive, member))
}

/// Open one member as a stream (decompressed by the `zip` crate, not yet
/// decoded), and whether it is compressed.
///
/// The `zip` crate's member readers borrow the archive, so the member's data
/// is located through the archive and then streamed from a file handle (or a
/// slice of the bytes) of its own: the reader is `'static` and holds no more
/// than the decompressor's state, however large the member is.
pub(crate) fn open_member(
    archive: &Archive,
    member: &str,
) -> crate::Result<(Box<dyn Read + Send>, bool)> {
    let mut zip = archive.open()?;
    // Raw: the `zip` crate would refuse a method it wasn't built to inflate,
    // and this module inflates members itself.
    let index = zip
        .index_for_name(member)
        .ok_or_else(|| crate::Error::Zip {
            archive: archive.name(),
            member: member.to_string(),
            message: "no such member".into(),
        })?;
    let file = zip
        .by_index_raw(index)
        .map_err(|e| zip_error(archive, member, e))?;
    if let Some(message) = unreadable(&file) {
        return Err(crate::Error::Zip {
            archive: archive.name(),
            member: member.to_string(),
            message,
        });
    }
    let method = file.compression();
    let compressed_size = file.compressed_size();
    let data_start = file.data_start().ok_or_else(|| crate::Error::Zip {
        archive: archive.name(),
        member: member.to_string(),
        message: "the member's data could not be located".into(),
    })?;
    drop(file);
    drop(zip);

    let data = archive.range(data_start, compressed_size)?;
    match method {
        zip::CompressionMethod::Stored => Ok((Box::new(data), false)),
        zip::CompressionMethod::Deflated => {
            Ok((Box::new(flate2::read::DeflateDecoder::new(data)), true))
        }
        // Windows writes it for large members; GUGiK's PRG zips have it.
        zip::CompressionMethod::Deflate64 => {
            Ok((Box::new(deflate64::Deflate64Decoder::new(data)), true))
        }
        _ => unreachable!("checked by unreadable()"),
    }
}

fn build_globs(archive: &Archive, globs: &[String]) -> crate::Result<Option<globset::GlobSet>> {
    if globs.is_empty() {
        return Ok(None);
    }
    let mut set = globset::GlobSetBuilder::new();
    for glob in globs {
        let glob = globset::GlobBuilder::new(glob)
            .literal_separator(true)
            .build()
            .map_err(|e| crate::Error::Zip {
                archive: archive.name(),
                member: glob.clone(),
                message: e.to_string(),
            })?;
        set.add(glob);
    }
    set.build().map(Some).map_err(|e| crate::Error::Zip {
        archive: archive.name(),
        member: String::new(),
        message: e.to_string(),
    })
}

fn zip_error(archive: &Archive, member: &str, error: zip::result::ZipError) -> crate::Error {
    crate::Error::Zip {
        archive: archive.name(),
        member: member.to_string(),
        message: error.to_string(),
    }
}
