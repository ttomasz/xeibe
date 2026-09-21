use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::archive;

/// Index of a source within one scan or read, in the order sources were consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceId(pub u32);

impl std::fmt::Display for SourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "source #{}", self.0)
    }
}

/// Raw bytes of one input, before decompression and decoding. Always read as one
/// sequential stream from the start (see `docs/architecture.md#sources-and-remote-input`).
///
/// Implemented here for local files and one-shot readers; `xeibe-io` adds HTTP and
/// object stores. Implementations are synchronous: async stores are bridged in
/// `xeibe-io`, so this crate needs no async runtime.
// `len` is a size hint that may be unknown, so an `is_empty` would mislead.
#[allow(clippy::len_without_is_empty)]
pub trait ByteSource: Send + Sync + std::fmt::Debug {
    /// Path or URL, used in messages.
    fn name(&self) -> &str;

    /// Size in bytes, if known up front (file metadata, `Content-Length`). For progress only.
    fn len(&self) -> Option<u64>;

    /// A fresh stream from the start. One-shot sources (stdin) fail with
    /// [`crate::Error::NotReopenable`] on the second call.
    fn open(&self) -> crate::Result<Box<dyn std::io::Read + Send>>;
}

/// An input to read: a byte source, or one member of a local zip archive.
#[derive(Debug, Clone)]
pub enum Source {
    Stream(Arc<dyn ByteSource>),
    /// Member of a zip archive (see [`crate::archive`]). Archives must be local files.
    ZipMember {
        archive: PathBuf,
        member: String,
    },
}

impl Source {
    /// A local file, or `archive.zip!/member` for one zip member.
    ///
    /// A missing file fails here; a missing zip member fails on
    /// [`Source::open`], because the archive is not opened until then.
    pub fn file(path: impl Into<PathBuf>) -> crate::Result<Self> {
        let path = path.into();
        if let Some((archive, member)) = path.to_str().and_then(archive::split_member_path) {
            return Ok(Source::ZipMember {
                archive: PathBuf::from(archive),
                member: member.to_string(),
            });
        }
        Ok(Source::Stream(Arc::new(FileSource::open(path)?)))
    }

    /// A one-shot reader such as stdin.
    pub fn reader(name: impl Into<String>, reader: Box<dyn std::io::Read + Send>) -> Self {
        Source::Stream(Arc::new(ReaderSource::new(name, reader)))
    }

    pub fn name(&self) -> String {
        match self {
            Source::Stream(source) => source.name().to_string(),
            Source::ZipMember { archive, member } => format!("{}!/{member}", archive.display()),
        }
    }

    /// Open a fresh decompressed and decoded (UTF-8) stream.
    pub fn open(&self) -> crate::Result<Box<dyn std::io::Read + Send>> {
        let raw = match self {
            Source::Stream(source) => source.open()?,
            Source::ZipMember { archive, member } => archive::open_member(archive, member)?,
        };
        crate::decode::decoded_reader(raw)
    }
}

/// The inputs of one scan or read: a list, or a lazily produced sequence (WFS pages).
/// Consumed once; a scan followed by a read needs two `Sources` values.
pub struct Sources(Box<dyn Iterator<Item = crate::Result<Source>> + Send>);

impl Sources {
    pub fn lazy(iter: impl Iterator<Item = crate::Result<Source>> + Send + 'static) -> Self {
        Self(Box::new(iter))
    }
}

impl Iterator for Sources {
    type Item = crate::Result<Source>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

impl From<Vec<Source>> for Sources {
    fn from(sources: Vec<Source>) -> Self {
        Self::lazy(sources.into_iter().map(Ok))
    }
}

impl From<Source> for Sources {
    fn from(source: Source) -> Self {
        Self::from(vec![source])
    }
}

/// Local file.
#[derive(Debug)]
pub struct FileSource {
    path: PathBuf,
    name: String,
    len: Option<u64>,
}

impl FileSource {
    /// Reads the file's metadata; fails if the file does not exist.
    pub fn open(path: impl Into<PathBuf>) -> crate::Result<Self> {
        let path = path.into();
        let metadata = std::fs::metadata(&path).map_err(|e| with_path(e, &path))?;
        Ok(Self {
            name: path.display().to_string(),
            len: Some(metadata.len()),
            path,
        })
    }
}

impl ByteSource for FileSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn len(&self) -> Option<u64> {
        self.len
    }
    fn open(&self) -> crate::Result<Box<dyn std::io::Read + Send>> {
        let file = std::fs::File::open(&self.path).map_err(|e| with_path(e, &self.path))?;
        Ok(Box::new(file))
    }
}

/// An I/O error that names the path.
fn with_path(error: std::io::Error, path: &Path) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

/// Any reader that can be consumed once (stdin, a response body handed in by the
/// caller). The `Mutex` keeps it `Sync`.
pub struct ReaderSource {
    name: String,
    reader: std::sync::Mutex<Option<Box<dyn std::io::Read + Send>>>,
}

impl ReaderSource {
    pub fn new(name: impl Into<String>, reader: Box<dyn std::io::Read + Send>) -> Self {
        Self {
            name: name.into(),
            reader: std::sync::Mutex::new(Some(reader)),
        }
    }
}

impl std::fmt::Debug for ReaderSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let consumed = self.reader.lock().map(|r| r.is_none()).unwrap_or(true);
        f.debug_struct("ReaderSource")
            .field("name", &self.name)
            .field("consumed", &consumed)
            .finish()
    }
}

impl ByteSource for ReaderSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn len(&self) -> Option<u64> {
        None
    }
    fn open(&self) -> crate::Result<Box<dyn std::io::Read + Send>> {
        let mut slot = self
            .reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.take()
            .ok_or_else(|| crate::Error::NotReopenable(self.name.clone()))
    }
}

/// Bytes already in memory (a fetched WFS page).
#[derive(Debug)]
pub struct BytesSource {
    pub name: String,
    pub bytes: bytes::Bytes,
}

impl ByteSource for BytesSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn len(&self) -> Option<u64> {
        Some(self.bytes.len() as u64)
    }
    fn open(&self) -> crate::Result<Box<dyn std::io::Read + Send>> {
        Ok(Box::new(std::io::Cursor::new(self.bytes.clone())))
    }
}

/// File names picked up from directories (case-insensitive); zip archives
/// are expanded into their members.
const DIRECTORY_SUFFIXES: &[&str] = &[
    ".gml", ".xml", ".gml.gz", ".xml.gz", ".gml.zst", ".xml.zst", ".zip",
];

/// Expand local paths, directories, globs, zip archives and `archive.zip!/member`
/// into individual sources. Remote URLs are resolved by `xeibe-io`.
///
/// Directories are walked recursively and contribute files with a GML
/// suffix (see [`DIRECTORY_SUFFIXES`]); a glob contributes every file it
/// matches. Results of one directory or glob are sorted by path.
pub fn expand_sources(inputs: &[String]) -> crate::Result<Vec<Source>> {
    let mut sources = Vec::new();
    for input in inputs {
        if input.contains("://") {
            let lower = input.to_ascii_lowercase();
            if lower.contains(".zip!/") || lower.ends_with(".zip") {
                return Err(crate::Error::RemoteArchive(input.clone()));
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{input}: remote inputs are resolved by xeibe-io"),
            )
            .into());
        }
        if archive::split_member_path(input).is_some() {
            sources.push(Source::file(input)?);
            continue;
        }
        let path = Path::new(input);
        if path.is_dir() {
            let mut files = Vec::new();
            walk(path, usize::MAX, &mut files)?;
            files.retain(|file| has_suffix(file, DIRECTORY_SUFFIXES));
            files.sort();
            for file in files {
                push_file(&file, &mut sources)?;
            }
        } else if !path.exists() && is_glob(input) {
            for file in glob_files(input)? {
                push_file(&file, &mut sources)?;
            }
        } else {
            push_file(path, &mut sources)?;
        }
    }
    Ok(sources)
}

/// One file: a zip archive expands into its members.
fn push_file(path: &Path, sources: &mut Vec<Source>) -> crate::Result<()> {
    if has_suffix(path, &[".zip"]) {
        sources.extend(archive::expand(path, &[])?);
    } else {
        sources.push(Source::Stream(Arc::new(FileSource::open(path)?)));
    }
    Ok(())
}

fn has_suffix(path: &Path, suffixes: &[&str]) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    suffixes.iter().any(|suffix| name.ends_with(suffix))
}

fn is_glob(input: &str) -> bool {
    input.contains(['*', '?', '[', '{'])
}

/// All files under `dir`, down to `depth` levels of subdirectories.
fn walk(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) -> crate::Result<()> {
    for entry in std::fs::read_dir(dir).map_err(|e| with_path(e, dir))? {
        let path = entry?.path();
        if path.is_dir() {
            if depth > 0 {
                walk(&path, depth - 1, files)?;
            }
        } else {
            files.push(path);
        }
    }
    Ok(())
}

/// Files matching a glob, walked from the longest literal directory prefix.
fn glob_files(pattern: &str) -> crate::Result<Vec<PathBuf>> {
    let matcher = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?
        .compile_matcher();
    let literal_end = pattern.find(['*', '?', '[', '{']).unwrap_or(pattern.len());
    let (base, rest) = match pattern[..literal_end].rfind('/') {
        Some(0) => (PathBuf::from("/"), &pattern[1..]),
        Some(slash) => (PathBuf::from(&pattern[..slash]), &pattern[slash + 1..]),
        None => (PathBuf::from("."), pattern),
    };
    // Without `**`, the pattern says how deep to look.
    let depth = if rest.contains("**") {
        usize::MAX
    } else {
        rest.matches('/').count()
    };
    let mut files = Vec::new();
    if base.is_dir() {
        walk(&base, depth, &mut files)?;
    }
    let relative_base = !pattern.starts_with("./") && base == Path::new(".");
    files.retain(|file| {
        let file = if relative_base {
            file.strip_prefix(".").unwrap_or(file)
        } else {
            file
        };
        matcher.is_match(file)
    });
    files.sort();
    Ok(files)
}
